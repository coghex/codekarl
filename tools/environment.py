"""Project-local tools; never change the user's shell or Xcode selection."""

from __future__ import annotations

import contextlib
import fcntl
import os
import re
import tempfile
from collections.abc import Iterator
from pathlib import Path

from gate import lease


def hub_for(root: Path) -> Path:
    marker = root / ".ck-hub"
    return Path(marker.read_text().strip()).resolve() if marker.exists() else root.resolve()


def configure(root: Path) -> Path:
    hub = hub_for(root)
    os.environ["CK_HUB"] = str(hub)
    os.environ["PATH"] = os.pathsep.join(
        ["/opt/homebrew/opt/rustup/bin", str(Path.home() / ".local/bin"), os.environ["PATH"]]
    )
    os.environ.setdefault("DEVELOPER_DIR", "/Applications/Xcode.app/Contents/Developer")
    os.environ["PIJUL_CONFIG_DIR"] = str(hub / ".tools/pijul-config")
    return hub


@contextlib.contextmanager
def lock(path: Path) -> Iterator[int]:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as stream:
        try:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RuntimeError(f"Already running: {path}") from error
        yield stream.fileno()


def ci_descriptor(hub: Path) -> int | None:
    """Return an inherited CI lock descriptor only if it names this hub's lock."""
    try:
        descriptor = int(os.environ.get("CK_CI_LOCK_FD", "-1"))
        actual = os.fstat(descriptor)
        expected = (hub / ".pipeline/ci.lock").stat()
        if (actual.st_dev, actual.st_ino) == (expected.st_dev, expected.st_ino):
            return descriptor
    except (OSError, ValueError):
        pass
    return None


@contextlib.contextmanager
def signing(hub: Path) -> Iterator[None]:
    """An isolated, short-lived agent. The local key never authenticates to a remote."""
    if os.environ.get("CK_SIGNING_ACTIVE") == str(hub):
        yield
        return
    key = hub / ".tools/signing/ed25519"
    with lock(hub / ".tools/signing/setup.lock"):
        if not key.exists():
            lease.run(
                [
                    "ssh-keygen",
                    "-q",
                    "-t",
                    "ed25519",
                    "-N",
                    "",
                    "-C",
                    "codekarl-local",
                    "-f",
                    str(key),
                ],
                check=True,
            )
    old = {k: os.environ.get(k) for k in ("SSH_AUTH_SOCK", "SSH_AGENT_PID", "CK_SIGNING_ACTIVE")}
    with tempfile.TemporaryDirectory(prefix="ck-agent-") as directory:
        # The agent is a daemon that outlives this call: it must not inherit the leases it would hold.
        output = lease.check_output(
            ["ssh-agent", "-s", "-a", str(Path(directory) / "s")],
            text=True,
            inherit=False,
            group=False,
        )
        for key_name in ("SSH_AUTH_SOCK", "SSH_AGENT_PID"):
            found = re.search(rf"{key_name}=([^;]+);", output)
            if found is None:
                raise RuntimeError(f"ssh-agent did not report {key_name}: {output!r}")
            os.environ[key_name] = found.group(1)
        os.environ["CK_SIGNING_ACTIVE"] = str(hub)
        try:
            lease.run(["ssh-add", str(key)], check=True, capture_output=True)
            yield
        finally:
            lease.run(["ssh-agent", "-k"], capture_output=True, check=False)
            for key_name, value in old.items():
                if value is None:
                    os.environ.pop(key_name, None)
                else:
                    os.environ[key_name] = value
