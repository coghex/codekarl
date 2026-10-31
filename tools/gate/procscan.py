"""Detect live legacy state writers before the bootstrap hold is installed."""

from __future__ import annotations

import os
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
