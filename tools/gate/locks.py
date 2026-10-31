"""Ordered, bounded locks shared by gate state writers."""

from __future__ import annotations

import contextlib
import fcntl
import json
import os
import sys
import threading
import time
from collections.abc import Iterator
from pathlib import Path

from .records import Holder, RecordError, frozen_owner_pid, load_holder

ORDER = {"ci": 0, "state": 1, "meta": 2}


_LOCAL = threading.local()


def _held() -> list[tuple[str, Path]]:
    stack = getattr(_LOCAL, "stack", None)
    if stack is None:
        stack = []
        _LOCAL.stack = stack
    return stack


def _command() -> str:
    """What this process was asked to do, for the holder hint: `ck batch publish`."""
    argv = [Path(sys.argv[0]).name, *sys.argv[1:]] if sys.argv and sys.argv[0] else ["?"]
    return " ".join(argv)[:200]


def _write_holder(fd: int, **extra: object) -> None:
    """Record who holds the lock, in place (same inode): a hint, never proof of ownership."""
    record = {"pid": os.getpid(), "command": _command(), "started": time.time(), **extra}
    os.ftruncate(fd, 0)
    os.pwrite(fd, json.dumps(record, sort_keys=True).encode(), 0)


def annotate(fd: int, **fields: object) -> None:
    """Add fields (for example the batch id) to this process's own holder record."""
    record = read_holder(fd)
    if record and record.get("pid") == os.getpid():
        _write_holder(
            fd,
            **{
                **{k: v for k, v in record.items() if k not in ("pid", "command", "started")},
                **fields,
            },
        )


def read_holder(fd: int) -> Holder | None:
    """The holder record in an open lock file, or None when absent, partial or unreadable."""
    try:
        data = os.pread(fd, 4096, 0)
        return load_holder(json.loads(data), "lock holder note") if data else None
    except (OSError, ValueError, RecordError):
        return None


def _pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def describe_holder(fd: int) -> str:
    """`held by pid 123 (`ck batch publish`, 41 s)`; names a dead pid as such."""
    record = read_holder(fd)
    if record is None:
        return "the holder wrote no usable metadata"
    pid = record["pid"]
    age = max(0, int(time.time() - record.get("started", time.time())))
    what = f"pid {pid} (`{record.get('command', '?')}`, {age} s)"
    if _pid_alive(pid):
        return f"held by {what}"
    return (
        f"metadata names {what}, which has exited, while the lock is still held; "
        "a child that inherited the descriptor probably holds it"
    )


def _batch_of(path: Path, record: Holder | None) -> str | None:
    """The id of the batch whose prepare holds ci.lock, from the holder note or the frozen record."""
    if record and record.get("batch"):
        return str(record["batch"])
    if record:
        for frozen in sorted(path.parent.glob("batches/*/frozen.json")):
            with contextlib.suppress(OSError, ValueError):
                if frozen_owner_pid(json.loads(frozen.read_bytes())) == record["pid"]:
                    return frozen.parent.name
    return None


@contextlib.contextmanager
def acquire(
    path: Path, kind: str, *, blocking: bool = True, timeout: float = 60, quiet: bool = False
) -> Iterator[int]:
    """Take the lock. `quiet` is for the gc dry run only: the kernel lock is taken exactly as usual,
    no holder metadata is written, and a missing lock file is refused, never created."""
    if kind not in ORDER:
        raise ValueError(f"Unknown gate lock: {kind}")
    stack = _held()
    if stack and ORDER[kind] < ORDER[stack[-1][0]]:
        raise RuntimeError(
            f"Lock order violation: {kind}.lock acquired while holding {stack[-1][0]}.lock"
        )
    if quiet:
        if not path.exists():
            raise RuntimeError(
                f"gc dry run needs the lock file {path}; it does not exist (nothing was created)"
            )
        fd = os.open(path, os.O_RDONLY)
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        fd = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
    operation = fcntl.LOCK_EX | (0 if blocking else fcntl.LOCK_NB)
    deadline = time.monotonic() + timeout
    try:
        while True:
            try:
                fcntl.flock(fd, operation | (fcntl.LOCK_NB if blocking else 0))
                break
            except BlockingIOError as error:
                if not blocking:
                    record = read_holder(fd)
                    batch = _batch_of(path, record) if kind == "ci" else None
                    who = f"batch {batch}, " if batch else ""
                    raise RuntimeError(
                        f"Already running: {who}{describe_holder(fd)}: {path}"
                    ) from error
                if time.monotonic() >= deadline:
                    raise RuntimeError(
                        f"Timed out waiting for {kind}.lock: {path}; {describe_holder(fd)}"
                    ) from error
                time.sleep(min(0.05, max(0, deadline - time.monotonic())))
        if not quiet:
            _write_holder(fd)
        stack.append((kind, path))
        yield fd
    finally:
        if stack and stack[-1] == (kind, path):
            stack.pop()
        os.close(fd)


def ci_path(hub: Path) -> Path:
    return hub / ".pipeline/ci.lock"


def state_path(hub: Path) -> Path:
    return hub / ".pipeline/state.lock"


def meta_path(meta: Path) -> Path:
    return meta / ".lock"


@contextlib.contextmanager
def ci(hub: Path, *, blocking: bool = False) -> Iterator[int]:
    """Honor a matching inherited CI descriptor; otherwise acquire nonblocking."""
    inherited = os.environ.get("CK_CI_LOCK_FD")
    if inherited is not None:
        try:
            fd = int(inherited)
            actual, expected = os.fstat(fd), ci_path(hub).stat()
            if (actual.st_dev, actual.st_ino) == (expected.st_dev, expected.st_ino):
                yield fd
                return
        except (OSError, ValueError):
            pass
    with acquire(ci_path(hub), "ci", blocking=blocking) as fd:
        yield fd


def state(hub: Path, *, quiet: bool = False) -> contextlib.AbstractContextManager[int]:
    return acquire(state_path(hub), "state", quiet=quiet)


def meta(meta_repo: Path, *, quiet: bool = False) -> contextlib.AbstractContextManager[int]:
    return acquire(meta_path(meta_repo), "meta", quiet=quiet)
