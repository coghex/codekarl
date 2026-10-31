"""The owner's optional post-publication hook (CK-152).

When a `ck` command has itself advanced the accepted pointer, and `<hub>/.pipeline/hooks/post-publish`
is an executable file, the hook is started once, detached, after the command has finished and every
transactional lock is released. The hook is the owner's: untracked, installed by nothing here,
and best effort. A missing or non-executable hook is a no-op, and a spawn failure is one warning that
never changes the command's exit status. Nothing is queued or replayed: the next command that advances
the pointer starts the hook again.
"""

from __future__ import annotations

import os
import subprocess
import sys
import warnings
from pathlib import Path

from . import lease

HOOK = ".pipeline/hooks/post-publish"
LOG = ".pipeline/hooks/post-publish.log"
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
    """Start the hook if this command advanced the pointer; never raises."""
    if not _STATE["advanced"]:
        return
    _STATE["advanced"] = False
    hook = hub / HOOK
    if not (hook.is_file() and os.access(hook, os.X_OK)):
        return
    try:
        with (hub / LOG).open("ab") as log, warnings.catch_warnings():
            # The child is detached on purpose and never waited for: drop the handle at once, quietly.
            warnings.simplefilter("ignore", ResourceWarning)
            subprocess.Popen(
                [str(hook)],
                cwd=hub,
                env={
                    **{k: v for k, v in os.environ.items() if k not in INHERITED},
                    "CK_HUB": str(hub),
                },
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=log,
                close_fds=True,
                start_new_session=True,
            )
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"ck: warning: post-publish hook not started: {error}", file=sys.stderr)
