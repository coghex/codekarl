"""Checkpoint leases and the one spawn helper (plan sections 10.4 and 13.3).

A lease is a shared `flock` on a checkpoint directory's own descriptor. It needs no file inside the
checkpoint, so cleanliness and fingerprints are unchanged. The descriptor is inheritable: every
process that holds a copy of it, however it got it, keeps the lease, and the lease ends when the last
copy closes. A later garbage collector takes the exclusive lock without waiting and skips whatever is
leased, so a lease must be held by every process that can still read the checkpoint, including the
children a controller starts and the descendants a candidate's check starts.

The spawn helper is how controller code starts a subprocess: `subprocess.run` closes inherited
descriptors by default, so the helper adds every held lease descriptor to `pass_fds`.
"""

import contextlib
import ctypes
import ctypes.util
import fcntl
import json
import os
import signal
import stat
import subprocess
import threading
import time
from collections.abc import Callable, Iterator, Mapping, Sequence
from pathlib import Path
from types import FrameType
from typing import IO, AnyStr, Literal, TypedDict, TypeVar, Unpack, overload

WAIT = 5.0  # how long a lease polls while the directory is exclusively locked
POLL = 0.05
GRACE = 3.0  # between SIGTERM and SIGKILL when a controller ends its child's group
KILL_WAIT = 10.0
COLLECTED = "controller checkpoint was collected; rerun the command"
ANNOUNCE = "CK_LEASE_FDS"  # tells a child which of its inherited descriptors are leases

_held: list[tuple[int, Path]] = []
_adopted = False


class LeaseUnavailable(RuntimeError):  # a condition the caller reports, not only an error
    """The directory is gone, was replaced, or could not be leased in time."""


def acquire(path: Path | str, wait: float = WAIT) -> int:
    """Take a shared lease on the directory and return its inheritable descriptor.

    Polls nonblocking for up to `wait` seconds, then confirms that the path still names the opened
    directory (a rename keeps the inode; a replacement does not)."""
    path = Path(path)
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    except OSError as error:
        raise LeaseUnavailable(f"cannot open {path}: {error.strerror}") from error
    try:
        deadline = time.monotonic() + wait
        while True:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_SH | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise LeaseUnavailable(
                        f"{path} stayed exclusively locked for {wait:g} s"
                    ) from None
                time.sleep(POLL)
        try:
            current = path.stat()
        except OSError as error:
            raise LeaseUnavailable(f"{path} disappeared: {error.strerror}") from error
        opened = os.fstat(descriptor)
        if (current.st_dev, current.st_ino) != (opened.st_dev, opened.st_ino):
            raise LeaseUnavailable(f"{path} was replaced after it was opened")
        os.set_inheritable(descriptor, True)
    except BaseException:
        os.close(descriptor)
        raise
    _held.append((descriptor, path))
    return descriptor


def release(descriptor: int) -> None:
    """Close this process's copy; children that inherited the descriptor keep the lease."""
    for entry in list(_held):
        if entry[0] == descriptor:
            _held.remove(entry)
    with contextlib.suppress(OSError):
        os.close(descriptor)


def adopt() -> None:
    """Take over the leases a parent announced in `CK_LEASE_FDS`, once per process.

    A child that was started by the helper receives the lease descriptors with the same numbers but
    an empty registry, whether it is a fresh interpreter, an `exec` or a late import. An announced
    descriptor is adopted only if it is still a directory with the announced device and inode and a
    shared lock can be taken on it; anything else is ignored, so a stale announcement grants nothing."""
    global _adopted  # noqa: PLW0603  # adoption happens once per process
    if _adopted:
        return
    _adopted = True
    try:
        entries = json.loads(os.environ.get(ANNOUNCE) or "[]")
    except ValueError:
        return
    for entry in entries if isinstance(entries, list) else []:
        try:
            descriptor, device, inode, path = entry
            info = os.fstat(descriptor)
            if not stat.S_ISDIR(info.st_mode) or (info.st_dev, info.st_ino) != (device, inode):
                continue
            fcntl.flock(descriptor, fcntl.LOCK_SH | fcntl.LOCK_NB)
        except (TypeError, ValueError, OSError):
            continue
        if descriptor not in [held_descriptor for held_descriptor, _ in _held]:
            _held.append((descriptor, Path(path)))


def announcement() -> str:
    """The value of `CK_LEASE_FDS` that lets a child adopt every lease this process holds."""
    entries = []
    for descriptor, path in held():
        info = os.fstat(descriptor)
        entries.append([descriptor, info.st_dev, info.st_ino, str(path)])
    return json.dumps(entries)


def held() -> list[tuple[int, Path]]:
    """(descriptor, directory) of every lease this process still holds, inherited ones included."""
    adopt()
    live = []
    for entry in list(_held):
        try:
            os.fstat(entry[0])
        except OSError:
            _held.remove(entry)  # closed behind our back: nothing to pass on
        else:
            live.append(entry)
    return live


def descriptors() -> tuple[int, ...]:
    return tuple(descriptor for descriptor, _ in held())


@contextlib.contextmanager
def leased(path: Path | str) -> Iterator[int]:
    descriptor = acquire(path)
    try:
        yield descriptor
    finally:
        release(descriptor)


class HasRepo(TypedDict):
    """The one field the lease needs of a pointer: the repository it leases."""

    repo: str


State = TypeVar("State", bound=HasRepo)


@contextlib.contextmanager
def source(read_state: Callable[[], State], attempts: int = 3) -> Iterator[State]:
    """Lease the pointer's repository for a reader (`worker`, `sync`) and yield the pointer state.

    If the source disappeared before the lease, the pointer is read again and the lease retried, up to
    `attempts` times in all."""
    for attempt in range(1, attempts + 1):
        state = read_state()
        try:
            descriptor = acquire(state["repo"])
        except LeaseUnavailable as error:
            if attempt == attempts:
                raise RuntimeError(
                    f"the accepted checkpoint {state['repo']} could not be leased after {attempts} "
                    f"attempts ({error}); rerun the command"
                ) from error
            continue
        break
    try:
        yield state
    finally:
        release(descriptor)


RENAME_EXCL = 0x4  # macOS renamex_np: fail instead of replacing an existing destination


def rename_exclusive(source: Path | str, destination: Path | str) -> None:
    """Rename `source` to `destination` only if nothing exists there (even an empty directory).

    `os.rename` silently replaces an empty directory; this does not. FileExistsError if taken."""
    libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)
    if libc.renamex_np(os.fsencode(source), os.fsencode(destination), RENAME_EXCL) != 0:
        number = ctypes.get_errno()
        raise OSError(number, os.strerror(number), os.fspath(destination))


def rename_exclusive_at(source_fd: int, source: str, destination_fd: int, destination: str) -> None:
    """`rename_exclusive` between names relative to two open directories, so neither parent is
    looked up by path again. FileExistsError if `destination` exists."""
    libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)
    if (
        libc.renameatx_np(
            source_fd, os.fsencode(source), destination_fd, os.fsencode(destination), RENAME_EXCL
        )
        != 0
    ):
        number = ctypes.get_errno()
        raise OSError(number, os.strerror(number), destination)


def enroll_controller(tools: Path | str) -> int:
    """The controller's lease: the checkpoint directory that contains its real `tools` path."""
    try:
        return acquire(Path(tools).resolve().parent)
    except LeaseUnavailable as error:
        raise RuntimeError(COLLECTED) from error


def _inherit(pass_fds: Sequence[int], inherit: bool) -> tuple[int, ...]:
    return tuple(dict.fromkeys([*pass_fds, *(descriptors() if inherit else ())]))


def group_alive(group: int) -> bool:
    """True while any non-zombie process is in the group (a zombie-only group answers EPERM)."""
    try:
        os.killpg(group, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        rows = subprocess.run(
            ["ps", "-axo", "pgid=,stat="], capture_output=True, text=True, check=False
        ).stdout.splitlines()
        return any(
            fields[0] == str(group) and not fields[1].startswith("Z")
            for fields in (row.split() for row in rows)
            if len(fields) == 2  # noqa: PLR2004  # a ps row has two fields: pgid, state
        )
    return True


# Calls that keep the child in the caller's process group (`group=False`) are named here, each with
# its reason; any other such call fails the guard test in tests/test_lease.py. The `ssh-agent -s`
# daemon is the intentional exception of the spec (it also gets `inherit=False`, as it would hold
# the leases for ever). The interactive passthrough of `cargo`, `pijul` and `check` must share the
# terminal's foreground group, or a background group reading the terminal is stopped; its leases are
# inherited and the helper still records the cause and ends the child.
UNGROUPED = {
    "ssh-agent -s": "a daemon meant to outlive the command: no leases, no group cleanup",
    "interactive passthrough": "legacy caller-copy commands (cargo, pijul, check) run by tools/pipeline.py, outside the controller: the child must stay in the terminal's foreground process group",
}


class Interrupted(Exception):  # internal marker: a cause was recorded while a child ran
    """A SIGINT or SIGTERM was recorded while a child was owned; `run` acts on it after the cleanup."""


# The causes of the process, append-only: the first element is the first cause (S4's contract). While
# any spawn helper call is running, and for the rest of the process once a cause exists, the SIGINT
# and SIGTERM handlers only append here. They raise nothing, so no cleanup can be bypassed and no
# later signal can change which cause decides.
_causes: list[str] = []
_depth = 0  # spawn helper calls in flight (main thread)
Handler = Callable[[int, FrameType | None], object] | int | signal.Handlers | None
_previous: dict[int, Handler] = {}


def _on_interrupt(_signum: int, _frame: object) -> None:
    _causes.append("interrupted")


def _on_terminate(_signum: int, _frame: object) -> None:
    _causes.append("terminated")


_HANDLERS = ((signal.SIGINT, _on_interrupt), (signal.SIGTERM, _on_terminate))


def _install() -> None:
    """Make the handlers record-only, from just before a spawn (main thread only: elsewhere a
    handler cannot be installed and the default behaviour applies)."""
    global _depth  # noqa: PLW0603  # module state shared by every spawn helper call
    if threading.current_thread() is not threading.main_thread():
        return
    if _depth == 0:
        for number, handler in _HANDLERS:
            if signal.getsignal(number) is not handler:  # still ours after a cause: keep _previous
                _previous[number] = signal.signal(number, handler)
    _depth += 1


def _leave() -> None:
    """End one call's use of the handlers. They are restored only when the last call leaves and no
    cause was recorded; after a cause they stay record-only, so unwinding cannot be interrupted.

    The decision and the restoration happen with SIGINT and SIGTERM blocked, so no cause can be
    recorded between the check and the restoration (masking is safe here: nothing is spawned at this
    point). A signal that arrives meanwhile stays pending and is delivered on unmasking to whichever
    handlers are installed then: the previous ones if none was recorded, else the record-only ones.
    (The mask is the calling thread's; the handlers only ever run in the main thread.)"""
    global _depth  # noqa: PLW0603  # module state shared by every spawn helper call
    if threading.current_thread() is not threading.main_thread():
        return
    blocked = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGINT, signal.SIGTERM})
    try:
        _depth -= 1
        if _depth == 0 and not _causes:
            for number, previous in _previous.items():
                if previous is not None:
                    signal.signal(number, previous)
            _previous.clear()
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK, blocked)


def _first_cause(mark: int) -> str | None:
    """The first cause recorded since `mark`, or None if a cause already existed (it is already
    propagating and stays the first) or none was recorded."""
    return _causes[mark] if mark == 0 and len(_causes) > mark else None


def _raise(cause: str) -> None:
    """The exception the signal would have raised."""
    if cause == "terminated":
        raise SystemExit(128 + signal.SIGTERM)
    raise KeyboardInterrupt


def _signal_group(process: subprocess.Popen[AnyStr], signum: int, group: bool) -> None:
    with contextlib.suppress(ProcessLookupError, PermissionError):
        if group:
            os.killpg(process.pid, signum)
        elif process.poll() is None:
            process.send_signal(signum)


def end_group(process: subprocess.Popen[AnyStr], group: bool = True) -> None:
    """Kill the child and, when it leads its own group, every member of that group; reap the child.

    Every wait is a bounded poll: SIGTERM, then SIGKILL after GRACE, then up to KILL_WAIT more. It
    returns only when the leader is reaped and the group is empty, whether or not the leader exited
    first. If either is still alive after SIGKILL and KILL_WAIT the result is a RuntimeError naming
    what survived, never an unlimited wait."""
    pgid = process.pid
    for signum, patience in ((signal.SIGTERM, GRACE), (signal.SIGKILL, KILL_WAIT)):
        _signal_group(process, signum, group)
        deadline = time.monotonic() + patience
        while True:
            if process.poll() is not None and not (group and group_alive(pgid)):
                return
            if time.monotonic() >= deadline:
                break
            time.sleep(POLL / 2)
    leader = "still running" if process.poll() is None else "reaped"
    raise RuntimeError(
        f"could not end the process group {pgid}: leader {leader}, "
        f"{'members remain' if group and group_alive(pgid) else 'no member remains'} after SIGKILL"
    )


def _close(process: subprocess.Popen[AnyStr]) -> None:
    """Close the pipes without waiting for the child (an implicit wait could be unbounded)."""
    for stream in (process.stdin, process.stdout, process.stderr):
        if stream is not None:
            with contextlib.suppress(OSError):
                stream.close()


def _wait(
    process: subprocess.Popen[AnyStr],
    input: AnyStr | None,
    timeout: float | None,
    mark: int,
) -> tuple[AnyStr, AnyStr]:
    """The child's output, polling in short bounded steps so a recorded cause or the deadline is
    acted on within one step. Raises Interrupted on a cause and TimeoutExpired at the deadline."""
    deadline = None if timeout is None else time.monotonic() + timeout
    pending = input
    while True:
        if len(_causes) > mark:
            raise Interrupted
        step = POLL
        if deadline is not None:
            left = deadline - time.monotonic()
            if left <= 0:
                raise subprocess.TimeoutExpired(process.args, timeout or 0.0)
            step = min(step, left)
        try:
            return process.communicate(pending, timeout=step)
        except subprocess.TimeoutExpired:
            pending = None  # the input was handed over on the first call


Stream = int | IO[str] | IO[bytes] | None  # what subprocess accepts for stdin, stdout and stderr


class SpawnOptions(TypedDict, total=False):
    """The `subprocess.Popen` options the controller supports (anything else is a type error)."""

    cwd: str | os.PathLike[str]
    env: Mapping[str, str]
    stdin: Stream
    stdout: Stream
    stderr: Stream
    pass_fds: Sequence[int]


def _child_environment(env: Mapping[str, str] | None, inherit: bool) -> dict[str, str]:
    """The child's environment: the caller's (or ours), announcing the leases it inherits."""
    environment = dict(env) if env is not None else os.environ.copy()
    if inherit and descriptors():
        environment[ANNOUNCE] = announcement()
    else:
        environment.pop(ANNOUNCE, None)
    return environment


class _Spawn:
    """The streams and environment a child is started with, decided before anything is spawned."""

    def __init__(  # one attribute per Popen argument
        self,
        options: SpawnOptions,
        *,
        input_given: bool,
        capture_output: bool,
        inherit: bool,
        group: bool,
    ) -> None:
        self.stdout: Stream = subprocess.PIPE if capture_output else options.get("stdout")
        self.stderr: Stream = subprocess.PIPE if capture_output else options.get("stderr")
        self.stdin: Stream = subprocess.PIPE if input_given else options.get("stdin")
        self.cwd = options.get("cwd")
        self.pass_fds = _inherit(options.get("pass_fds", ()), inherit)
        self.env = _child_environment(options.get("env"), inherit)
        self.group = group


def _execute(  # noqa: PLR0913  # the whole contract of one spawn
    arguments: Sequence[str | os.PathLike[str]],
    spawn: Callable[[], subprocess.Popen[AnyStr]],
    input: AnyStr | None,
    *,
    check: bool,
    timeout: float | None,
    owns_group: bool,
) -> subprocess.CompletedProcess[AnyStr]:
    """Start the child with `spawn` and run it to completion under the cleanup contract of `run`."""
    mark = len(_causes)
    _install()
    process: subprocess.Popen[AnyStr] | None = None
    result: tuple[AnyStr | None, AnyStr | None] = (None, None)
    failure: BaseException | None = None
    try:
        if len(_causes) == mark:  # a cause recorded before the spawn starts nothing
            try:
                process = spawn()
                result = _wait(process, input, timeout, mark)
            except BaseException as error:  # re-raised below, after the cleanup
                failure = error
        if process is not None:
            try:
                end_group(process, group=owns_group)
            except RuntimeError as survivor:
                survivor.__cause__ = failure
                failure = survivor
            finally:
                _close(process)
    finally:
        cause = _first_cause(mark)
        _leave()
        cause = cause or _first_cause(mark)  # one more look after the restoration
    if cause is not None:
        _raise(cause)  # deliberately replaces whatever else is in flight
    if failure is not None and not isinstance(failure, Interrupted):
        raise failure
    # No process means a cause or a failure was raised above, so 0 is never returned for it.
    code = process.returncode if process is not None else 0
    if check and code:
        raise subprocess.CalledProcessError(code, arguments, output=result[0], stderr=result[1])
    return subprocess.CompletedProcess(arguments, code, result[0], result[1])


def _run(  # noqa: PLR0913  # the whole option set of `run`
    arguments: Sequence[str | os.PathLike[str]],
    *,
    check: bool,
    input: str | bytes | None,
    timeout: float | None,
    capture_output: bool,
    inherit: bool,
    group: bool,
    text: bool | None,
    encoding: str | None,
    errors: str | None,
    universal_newlines: bool | None,
    options: SpawnOptions,
) -> subprocess.CompletedProcess[str] | subprocess.CompletedProcess[bytes]:
    if (
        text is not None
        and universal_newlines is not None
        and bool(universal_newlines) != bool(text)
    ):
        raise subprocess.SubprocessError(  # subprocess's own refusal, before any child exists
            "Cannot disambiguate when both text and universal_newlines are supplied but "
            "different. Pass one or the other."
        )
    unsupported = sorted(set(options) - set(SpawnOptions.__annotations__))
    if unsupported:  # the types refuse these; this is the same refusal for an untyped caller
        raise TypeError(
            f"unexpected keyword argument(s) {unsupported}; supported: {sorted(SpawnOptions.__annotations__)}"
        )
    spawn = _Spawn(
        options,
        input_given=input is not None,
        capture_output=capture_output,
        inherit=inherit,
        group=group,
    )
    process_group = 0 if group else None
    if text or encoding or errors or universal_newlines:  # subprocess's own rule for text mode
        if input is not None and not isinstance(input, str):
            raise TypeError("input must be a str when the output is read as text")

        def spawn_text() -> subprocess.Popen[str]:
            return subprocess.Popen(
                arguments,
                stdin=spawn.stdin,
                stdout=spawn.stdout,
                stderr=spawn.stderr,
                cwd=spawn.cwd,
                env=spawn.env,
                pass_fds=spawn.pass_fds,
                process_group=process_group,
                text=True,
                encoding=encoding,
                errors=errors,
            )

        return _execute(
            arguments, spawn_text, input, check=check, timeout=timeout, owns_group=group
        )
    if input is not None and not isinstance(input, bytes):
        raise TypeError("input must be bytes when the output is read as bytes")

    def spawn_bytes() -> subprocess.Popen[bytes]:
        return subprocess.Popen(
            arguments,
            stdin=spawn.stdin,
            stdout=spawn.stdout,
            stderr=spawn.stderr,
            cwd=spawn.cwd,
            env=spawn.env,
            pass_fds=spawn.pass_fds,
            process_group=process_group,
        )

    return _execute(arguments, spawn_bytes, input, check=check, timeout=timeout, owns_group=group)


@overload
def run(  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    check: bool = False,
    input: str | None = None,
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    text: Literal[True],
    encoding: str | None = None,
    errors: str | None = None,
    universal_newlines: bool | None = None,
    **options: Unpack[SpawnOptions],
) -> subprocess.CompletedProcess[str]: ...


@overload
def run(  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    check: bool = False,
    input: str | None = None,
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    encoding: Literal["utf-8"],
    errors: str | None = None,
    text: bool | None = None,
    universal_newlines: bool | None = None,
    **options: Unpack[SpawnOptions],
) -> subprocess.CompletedProcess[str]: ...


@overload
def run(  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    check: bool = False,
    input: str | None = None,
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    errors: Literal["strict"],
    encoding: str | None = None,
    text: bool | None = None,
    universal_newlines: bool | None = None,
    **options: Unpack[SpawnOptions],
) -> subprocess.CompletedProcess[str]: ...


@overload
def run(  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    check: bool = False,
    input: str | None = None,
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    universal_newlines: Literal[True],
    text: bool | None = None,
    encoding: str | None = None,
    errors: str | None = None,
    **options: Unpack[SpawnOptions],
) -> subprocess.CompletedProcess[str]: ...


@overload
def run(  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    check: bool = False,
    input: bytes | None = None,
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    text: Literal[False] | None = None,
    encoding: None = None,
    errors: None = None,
    universal_newlines: Literal[False] | None = None,
    **options: Unpack[SpawnOptions],
) -> subprocess.CompletedProcess[bytes]: ...


@overload
def run(  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    check: bool = False,
    input: str | bytes | None = None,
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    text: bool | None = None,
    encoding: str | None = None,
    errors: str | None = None,
    universal_newlines: bool | None = None,
    **options: Unpack[SpawnOptions],
) -> subprocess.CompletedProcess[str] | subprocess.CompletedProcess[bytes]: ...


def run(  # noqa: PLR0913  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    check: bool = False,
    input: str | bytes | None = None,  # same name as subprocess.run
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    text: bool | None = None,
    encoding: str | None = None,
    errors: str | None = None,
    universal_newlines: bool | None = None,
    **options: Unpack[SpawnOptions],
) -> subprocess.CompletedProcess[str] | subprocess.CompletedProcess[bytes]:
    """`subprocess.run` for controller code: every held lease descriptor is passed to the child (and
    announced in its environment, so a Python child adopts them and passes them on), and the child
    runs in its own process group.

    The return type follows the options that select text mode (`text`, `encoding`, `errors`,
    `universal_newlines`, as subprocess does): `CompletedProcess[str]` in text mode and
    `CompletedProcess[bytes]` otherwise; only a call whose options are not literal gets the union.

    **Cleanup contract (S4's).** From just before the spawn until the child's whole group is empty
    and reaped, SIGINT and SIGTERM only record their cause (append-only, the first wins) and raise
    nothing. The group is ended on every path, including after a normal exit of the leader (members
    are killed and reaped), with bounded polls, SIGTERM then SIGKILL, and a bounded error naming the
    survivor if it cannot be emptied. The previous handlers are restored last, and only when no cause
    was recorded; with a cause they stay record-only and the first recorded cause is raised, replacing
    whatever else was in flight. A signal between the last decision and the restoration is found by
    one more look after it, so no cause is lost.

    `inherit=False` withholds the leases (for a daemon that would hold them for ever);
    `group=False` keeps the child in the caller's process group and does not group-clean it (the
    explicit `ssh-agent -s` exception: a daemon meant to outlive the command)."""
    return _run(
        arguments,
        check=check,
        input=input,
        timeout=timeout,
        capture_output=capture_output,
        inherit=inherit,
        group=group,
        text=text,
        encoding=encoding,
        errors=errors,
        universal_newlines=universal_newlines,
        options=options,
    )


@overload
def check_output(  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    input: str | None = None,
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    text: Literal[True],
    encoding: str | None = None,
    errors: str | None = None,
    universal_newlines: bool | None = None,
    **options: Unpack[SpawnOptions],
) -> str: ...


@overload
def check_output(  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    input: str | None = None,
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    encoding: Literal["utf-8"],
    errors: str | None = None,
    text: bool | None = None,
    universal_newlines: bool | None = None,
    **options: Unpack[SpawnOptions],
) -> str: ...


@overload
def check_output(  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    input: str | None = None,
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    errors: Literal["strict"],
    encoding: str | None = None,
    text: bool | None = None,
    universal_newlines: bool | None = None,
    **options: Unpack[SpawnOptions],
) -> str: ...


@overload
def check_output(  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    input: str | None = None,
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    universal_newlines: Literal[True],
    text: bool | None = None,
    encoding: str | None = None,
    errors: str | None = None,
    **options: Unpack[SpawnOptions],
) -> str: ...


@overload
def check_output(  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    input: bytes | None = None,
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    text: Literal[False] | None = None,
    encoding: None = None,
    errors: None = None,
    universal_newlines: Literal[False] | None = None,
    **options: Unpack[SpawnOptions],
) -> bytes: ...


@overload
def check_output(  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    input: str | bytes | None = None,
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    text: bool | None = None,
    encoding: str | None = None,
    errors: str | None = None,
    universal_newlines: bool | None = None,
    **options: Unpack[SpawnOptions],
) -> str | bytes: ...


def check_output(  # noqa: PLR0913  # the subset of subprocess options the controller supports
    arguments: Sequence[str | os.PathLike[str]],
    *,
    input: str | bytes | None = None,
    timeout: float | None = None,
    capture_output: bool = False,
    inherit: bool = True,
    group: bool = True,
    text: bool | None = None,
    encoding: str | None = None,
    errors: str | None = None,
    universal_newlines: bool | None = None,
    **options: Unpack[SpawnOptions],
) -> str | bytes:
    """`subprocess.check_output` through the same contract as `run` (leases, group, bounded cleanup)."""
    options["stdout"] = subprocess.PIPE  # a caller's own stdout is replaced, as before
    output: str | bytes = _run(
        arguments,
        check=True,
        input=input,
        timeout=timeout,
        capture_output=capture_output,
        inherit=inherit,
        group=group,
        text=text,
        encoding=encoding,
        errors=errors,
        universal_newlines=universal_newlines,
        options=options,
    ).stdout
    return output


adopt()  # a Python child adopts its parent's announced leases at import
