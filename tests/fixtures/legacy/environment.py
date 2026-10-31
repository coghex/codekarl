"""Project-local tools; never change the user's shell or Xcode selection."""

import contextlib
import fcntl
import os
import re
import subprocess
import tempfile
from pathlib import Path


def hub_for(root):
    marker = root / ".ck-hub"
    return Path(marker.read_text().strip()).resolve() if marker.exists() else root.resolve()


def configure(root):
    hub = hub_for(root)
    os.environ["CK_HUB"] = str(hub)
    os.environ["PATH"] = os.pathsep.join(
        ["/opt/homebrew/opt/rustup/bin", str(Path.home() / ".local/bin"), os.environ["PATH"]]
    )
    os.environ.setdefault("DEVELOPER_DIR", "/Applications/Xcode.app/Contents/Developer")
    os.environ["PIJUL_CONFIG_DIR"] = str(hub / ".tools/pijul-config")
    return hub


@contextlib.contextmanager
def lock(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as stream:
        try:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RuntimeError(f"Already running: {path}") from error
        yield stream.fileno()


def ci_descriptor(hub):
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
def signing(hub):
    """An isolated, short-lived agent. The local key never authenticates to a remote."""
    if os.environ.get("CK_SIGNING_ACTIVE") == str(hub):
        yield
        return
    key = hub / ".tools/signing/ed25519"
    with lock(hub / ".tools/signing/setup.lock"):
        if not key.exists():
            subprocess.run(
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
        output = subprocess.check_output(
            ["ssh-agent", "-s", "-a", str(Path(directory) / "s")], text=True
        )
        for key_name in ("SSH_AUTH_SOCK", "SSH_AGENT_PID"):
            os.environ[key_name] = re.search(rf"{key_name}=([^;]+);", output).group(1)
        os.environ["CK_SIGNING_ACTIVE"] = str(hub)
        try:
            subprocess.run(["ssh-add", str(key)], check=True, capture_output=True)
            yield
        finally:
            subprocess.run(["ssh-agent", "-k"], capture_output=True, check=False)
            for key_name, value in old.items():
                if value is None:
                    os.environ.pop(key_name, None)
                else:
                    os.environ[key_name] = value
