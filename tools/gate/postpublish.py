"""Detached post-publication maintenance (CK-176) and the owner's optional hook (CK-152).

When a `ck` command has itself advanced the accepted pointer, automatic gc and (when installed) the
owner's hook are started detached after the command has finished and every transactional lock is
released. Automatic gc is best effort, deduplicated by a nonblocking maintenance lock, and writes its
report to `.pipeline/maintenance/gc.log`. The hook remains untracked and installed by the owner. A
spawn failure never changes the publication command's exit status.
"""

from __future__ import annotations

import os
import stat
import subprocess
import sys
import warnings
from pathlib import Path
from typing import IO

from . import lease

HOOK = ".pipeline/hooks/post-publish"
LOG = ".pipeline/hooks/post-publish.log"
GC_LOG = ".pipeline/maintenance/gc.log"
# Environment that tells a child which descriptors it inherited; the hook inherits none.
INHERITED = (lease.ANNOUNCE, "CK_CI_LOCK_FD")

_STATE = {"advanced": False}


def begin() -> None:
    """A command starts: nothing has advanced the pointer yet."""
    _STATE["advanced"] = False


def note_advance() -> None:
    """This process wrote the accepted pointer."""
    _STATE["advanced"] = True


def after_command(hub: Path) -> None:
    """Start best-effort post-publication maintenance if the pointer advanced; never raises."""
    if not _STATE["advanced"]:
        return
    _STATE["advanced"] = False
    tools = Path(__file__).resolve().parent.parent
    _start_gc(hub, tools)
    hook = hub / HOOK
    if hook.is_file() and os.access(hook, os.X_OK):
        _start_hook(hub, hook)


def _start_gc(hub: Path, tools: Path) -> None:
    """Run `gate.gc.automatic` from this pinned controller in its own detached process."""
    try:
        log_path = hub / GC_LOG
        log_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        if log_path.parent.is_symlink() or not stat.S_ISDIR(os.lstat(log_path.parent).st_mode):
            raise OSError(f"maintenance is not a real directory: {log_path.parent}")
        code = (
            "import sys; from pathlib import Path; "
            "tools, hub = Path(sys.argv[1]), Path(sys.argv[2]); "
            "sys.path.insert(0, str(tools)); "
            "from gate.gc import automatic; raise SystemExit(automatic(hub, tools))"
        )
        descriptor = os.open(
            log_path,
            os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW,
            0o600,
        )
        with os.fdopen(descriptor, "ab") as log:
            _spawn(
                [sys.executable, "-B", "-c", code, str(tools), str(hub)],
                cwd=hub,
                env=_child_environment(hub),
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=subprocess.STDOUT,
                close_fds=True,
                start_new_session=True,
            )
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"ck: warning: automatic gc not started: {error}", file=sys.stderr)


def _start_hook(hub: Path, hook: Path) -> None:
    try:
        with (hub / LOG).open("ab") as log, warnings.catch_warnings():
            # The child is detached on purpose and never waited for: drop the handle at once, quietly.
            warnings.simplefilter("ignore", ResourceWarning)
            _spawn(
                [str(hook)],
                cwd=hub,
                env=_child_environment(hub),
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=log,
                close_fds=True,
                start_new_session=True,
            )
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"ck: warning: post-publish hook not started: {error}", file=sys.stderr)


def _child_environment(hub: Path) -> dict[str, str]:
    return {
        **{key: value for key, value in os.environ.items() if key not in INHERITED},
        "CK_HUB": str(hub),
    }


def _spawn(  # noqa: PLR0913  # explicit child settings make inherited process state auditable
    command: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    stdin: int,
    stdout: IO[bytes],
    stderr: int | IO[bytes],
    close_fds: bool,
    start_new_session: bool,
) -> subprocess.Popen[bytes]:
    """The two detached children outlive publication and deliberately inherit no lease."""
    return subprocess.Popen(
        command,
        cwd=cwd,
        env=env,
        stdin=stdin,
        stdout=stdout,
        stderr=stderr,
        close_fds=close_fds,
        start_new_session=start_new_session,
    )
