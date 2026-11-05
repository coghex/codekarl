"""Detect live legacy state writers before the bootstrap hold is installed."""

from __future__ import annotations

import os
import subprocess
from collections.abc import Callable
from pathlib import Path

from . import lease
from .records import ProcessRow

Lister = Callable[[], list[ProcessRow]]  # lists the host's processes; tests pass their own


def processes() -> list[ProcessRow]:
    raw = lease.check_output(["ps", "-axo", "pid=,ppid=,command="], text=True)
    rows: list[ProcessRow] = []
    for line in raw.splitlines():
        parts = line.strip().split(None, 2)
        if len(parts) == 3:  # noqa: PLR2004  # a ps row has three fields
            rows.append({"pid": int(parts[0]), "ppid": int(parts[1]), "command": parts[2]})
    return rows


def process_start(pid: int) -> str:
    """Return the OS-reported process start time used by the chat registry."""
    output = lease.check_output(["ps", "-o", "lstart=", "-p", str(pid)], text=True)
    value = output.strip()
    if not value:
        raise RuntimeError(f"Cannot resolve process start time for pid {pid}")
    return value


def open_paths() -> dict[int, list[Path]]:
    """Return every process's cwd and open-file paths, or fail the whole scan closed.

    `lsof -F0pn` separates fields with NUL and terminates each process/file set with a newline.
    The output also includes `f` descriptor fields even though only `p` and `n` were requested.
    A nonzero command, warning, malformed record or empty result is not evidence of idleness:
    callers keep every candidate. The `n` field includes cwd as well as ordinary open descriptors.
    """
    completed = _lsof("-F0pn")
    return _parse_open_paths(completed.stdout)


def open_paths_with_parents() -> tuple[dict[int, list[Path]], dict[int, int]]:
    """Return process paths and PPIDs from one fail-closed lsof scan."""
    completed = _lsof("-F0pnR")
    return _parse_open_paths_with_parents(completed.stdout)


def _lsof(fields: str) -> subprocess.CompletedProcess[str]:
    try:
        completed = lease.run(["lsof", "-nP", fields], check=False, capture_output=True, text=True)
    except (OSError, subprocess.SubprocessError) as error:
        raise RuntimeError(
            f"cannot inspect process working directories and open files: {error}"
        ) from error
    if completed.returncode != 0 or completed.stderr:
        stderr = completed.stderr
        detail = (
            stderr.strip()
            if isinstance(stderr, str) and stderr.strip()
            else f"lsof exited {completed.returncode}"
        )
        raise RuntimeError(f"cannot inspect process working directories and open files: {detail}")
    raw = completed.stdout
    if not isinstance(raw, str):
        raise RuntimeError("process path scan returned bytes, not text")
    return completed


def _parse_open_paths(raw: str) -> dict[int, list[Path]]:
    """Parse lsof's NUL-delimited fields and newline-terminated sets."""
    paths_by_pid: dict[int, list[Path]] = {}
    current: int | None = None
    if not raw.endswith("\0\n"):
        raise RuntimeError("process path scan returned an unterminated field set")
    for token in raw[:-2].split("\0"):
        # The newline belongs to the separator between lsof sets, not to the field.
        field = token[1:] if token.startswith("\n") else token
        if not field:
            raise RuntimeError("process path scan returned an empty field")
        if field.startswith("p"):
            try:
                current = int(field[1:])
            except ValueError as error:
                raise RuntimeError(
                    f"process path scan returned an invalid pid record: {field!r}"
                ) from error
            paths_by_pid.setdefault(current, [])
        elif field.startswith("f"):
            # lsof emits descriptor fields (cwd, txt, 0, …) in -F0 output regardless of the
            # requested fields. They identify the following name but are not paths themselves.
            if current is None or not field[1:]:
                raise RuntimeError(f"process path scan returned an unreadable record: {field!r}")
        elif field.startswith("n") and current is not None:
            # Some descriptors have an empty name in lsof's real output; they cannot establish
            # that a process is using a worker path.
            if field[1:]:
                paths_by_pid[current].append(Path(field[1:]))
        else:
            raise RuntimeError(f"process path scan returned an unreadable record: {field!r}")
    if not paths_by_pid:
        raise RuntimeError("process path scan returned no process records")
    return paths_by_pid


def _parse_open_paths_with_parents(  # noqa: PLR0912  # every lsof field needs a fail-closed parse branch
    raw: str,
) -> tuple[dict[int, list[Path]], dict[int, int]]:
    """Parse lsof paths plus each process's parent PID; missing ancestry data fails closed."""
    paths_by_pid: dict[int, list[Path]] = {}
    parents: dict[int, int] = {}
    current: int | None = None
    if not raw.endswith("\0\n"):
        raise RuntimeError("process path scan returned an unterminated field set")
    for token in raw[:-2].split("\0"):
        field = token[1:] if token.startswith("\n") else token
        if not field:
            raise RuntimeError("process path scan returned an empty field")
        if field.startswith("p"):
            try:
                current = int(field[1:])
            except ValueError as error:
                raise RuntimeError(
                    f"process path scan returned an invalid pid record: {field!r}"
                ) from error
            if current in paths_by_pid:
                raise RuntimeError(f"process path scan repeated pid {current}")
            paths_by_pid[current] = []
        elif field.startswith("R") and current is not None:
            if current in parents:
                raise RuntimeError(f"process path scan repeated parent pid for {current}")
            try:
                parents[current] = int(field[1:])
            except ValueError as error:
                raise RuntimeError(
                    f"process path scan returned an invalid parent pid record: {field!r}"
                ) from error
        elif field.startswith("f") and current is not None:
            if not field[1:]:
                raise RuntimeError(f"process path scan returned an unreadable record: {field!r}")
        elif field.startswith("n") and current is not None:
            if field[1:]:
                paths_by_pid[current].append(Path(field[1:]))
        else:
            raise RuntimeError(f"process path scan returned an unreadable record: {field!r}")
    if not paths_by_pid:
        raise RuntimeError("process path scan returned no process records")
    missing = paths_by_pid.keys() - parents.keys()
    if missing:
        raise RuntimeError(f"process path scan omitted parent pid for {min(missing)}")
    return paths_by_pid, parents


def users_of(
    directory: Path,
    observed: dict[int, list[Path]] | None = None,
    *,
    ignore_pid: int | None = None,
    parents: dict[int, int] | None = None,
) -> list[tuple[int, Path]]:
    """Processes whose cwd or an open file resolves inside `directory`.

    Resolve both sides so an alternate spelling or symlink into a worker copy cannot evade the
    check. Unresolvable absolute names fail closed instead of being treated as unrelated.
    """
    root = directory.resolve(strict=True)
    paths_by_pid = open_paths() if observed is None else observed
    ignored = {ignore_pid} if ignore_pid is not None else set()
    if ignored:
        if parents is None:
            raise RuntimeError("process path scan omitted parent pid records")
        if ignore_pid not in parents or ignore_pid not in paths_by_pid:
            raise RuntimeError(f"process path scan omitted collector pid {ignore_pid}")
        while True:
            children = {pid for pid, parent in parents.items() if parent in ignored}
            if children <= ignored:
                break
            ignored.update(children)
    users: list[tuple[int, Path]] = []
    for pid, paths in paths_by_pid.items():
        if pid in ignored:
            continue
        for path in paths:
            if not path.is_absolute():
                continue  # lsof also reports non-path names for some special descriptors
            try:
                resolved = path.resolve(strict=False)
            except OSError as error:
                raise RuntimeError(f"cannot resolve process {pid} path {path}: {error}") from error
            if resolved == root or root in resolved.parents:
                users.append((pid, path))
    return users


STATE_COMMANDS = {"submit", "accept", "docs", "worker", "sync", "status", "retry", "init"}


def ancestors(pid: int | None = None, lister: Lister | None = None) -> set[int]:
    # Resolved when called, so a patch of `processes` reaches every default-bound caller.
    table = {row["pid"]: row for row in (lister or processes)()}
    current = os.getpid() if pid is None else pid
    result = set()
    while current > 1 and current not in result:
        result.add(current)
        row = table.get(current)
        if row is None:
            break
        current = row["ppid"]
    return result


def state_command(words: list[str]) -> bool:
    """True when the subcommand, the word after the first `ck` word, is a state-changing one.

    Options and arguments never match: `ck session --role worker` is a read-only check."""
    for index, word in enumerate(words):
        if Path(word).name == "ck":
            return index + 1 < len(words) and words[index + 1] in STATE_COMMANDS
    return False


def scan(
    hub: Path, *, self_pid: int | None = None, lister: Lister | None = None
) -> list[ProcessRow]:
    lister = lister or processes
    own = ancestors(self_pid, lister)
    found: list[ProcessRow] = []
    for row in lister():
        command = row["command"]
        if row["pid"] in own:
            continue
        words = command.split()
        pipeline = next((Path(word) for word in words if Path(word).name == "pipeline.py"), None)
        if state_command(words) or (pipeline is not None and str(hub) in str(pipeline.resolve())):
            found.append(row)
    return found


def require_quiescent(hub: Path, lister: Lister | None = None) -> None:
    rows = scan(hub, lister=lister)
    if rows:
        details = "; ".join(f"pid {row['pid']}: {row['command']}" for row in rows)
        raise RuntimeError(f"State writers are still running: {details}")
