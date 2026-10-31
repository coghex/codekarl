"""RC-1 fixtures: a fake shared chat tree, a fake checkout and the forbidden-fallback guard.

Nothing here reads ~/.codex/skills or live state. The expected values are computed with a
separate, deliberately plain implementation so the tests do not just echo tools/chat/inputs.py.
"""

import contextlib
import hashlib
import importlib.util
import json
import os
import pwd
import shutil
import subprocess
import sys
import threading
import types
import unittest
from collections.abc import Callable, Iterator, Mapping
from pathlib import Path
from unittest import mock

REPO = Path(__file__).resolve().parents[1]
REAL_SKILLS = Path(os.path.realpath(pwd.getpwuid(os.getuid()).pw_dir)) / ".codex/skills"
GUARDED = {
    "open",
    "os.listdir",
    "os.scandir",
    "os.mkdir",
    "os.rename",
    "os.remove",
    "os.rmdir",
    "shutil.copyfile",
    "shutil.copytree",
    "glob.glob",
    "pathlib.Path.glob",
    "pathlib.Path.rglob",
}
_lock = threading.Lock()
_active = 0


class ForbiddenFallback(AssertionError):
    """A test reached for the owner's real ~/.codex/skills instead of its fixture."""


def _hook(event: str, args: tuple[object, ...]) -> None:
    if not _active or event not in GUARDED:
        return
    for value in args:
        if isinstance(value, int) or value is None:
            continue
        try:
            # any audit argument: a TypeError means it is no path, exactly as before the types
            path = os.path.realpath(os.fsdecode(os.fspath(value)))  # type: ignore[call-overload]
        except (TypeError, ValueError):
            continue
        if path == str(REAL_SKILLS) or path.startswith(str(REAL_SKILLS) + os.sep):
            raise ForbiddenFallback(f"test touched the real shared skills: {event} {path}")


sys.addaudithook(_hook)


def guard(test: unittest.TestCase) -> None:
    """Fail the test, loudly, if anything opens or lists the real ~/.codex/skills."""
    global _active  # noqa: PLW0603  # process-wide count of active guards, protected by _lock
    with _lock:
        _active += 1

    def release() -> None:
        global _active  # noqa: PLW0603  # same counter as acquire, protected by _lock
        with _lock:
            _active -= 1

    test.addCleanup(release)


def load(path: str | Path, name: str) -> types.ModuleType:
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def adapter_stubs() -> dict[str, str]:
    """Source text on which the real TRANSFORMS are no-ops: every edit's new text, joined."""
    adapter = load(REPO / "tools/chat/adapter.py", "ck_adapter_fixture")
    stubs: dict[str, str] = {}
    for name, transform in adapter.TRANSFORMS.items():
        news: list[str] = []

        def record(source: str, old: str, new: str, news: list[str] = news) -> str:
            news.append(new)
            return source

        with mock.patch.object(adapter, "replace", record):
            transform("")
        stubs[name] = "\n".join(news) + "\n"
    return stubs


PASSING = "import unittest\n\n\nclass T(unittest.TestCase):\n    def test_ok(self):\n        pass\n"
FAILING = (
    "import unittest\n\n\nclass T(unittest.TestCase):\n    def test_regression(self):\n"
    "        self.fail('chat regression')\n"
)


def write_tree(root: str | Path, files: Mapping[str, str]) -> None:
    for name, text in files.items():
        path = Path(root) / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)


def build_shared(root: str | Path) -> Path:
    """A minimal ~/.codex/skills lookalike: chat/scripts (with tests) and project-manager."""
    stubs = adapter_stubs()
    write_tree(
        root,
        {
            "chat/SKILL.md": "# chat\n",
            "chat/scripts/pchat": "#!/usr/bin/env python3\n",
            "chat/scripts/chatlib.py": "VALUE = 1\n",
            "chat/scripts/chat-bridge": stubs["chat-bridge"],
            "chat/scripts/identities.py": stubs["identities.py"],
            "chat/scripts/role_colors.py": stubs["role_colors.py"],
            "chat/scripts/tests/_isolation.py": "",
            "chat/scripts/tests/test_shared.py": PASSING,
            "project-manager/SKILL.md": "# project manager\n",
            "project-manager/scripts/pm": "#!/usr/bin/env python3\n",
        },
    )
    return Path(root)


def build_checkout(root: str | Path, *, chat_test: str = PASSING) -> Path:
    """A fake candidate: the real install.py, inputs.py, adapter.py and gate, stub chat files."""
    root = Path(root)
    (root / "tools/chat").mkdir(parents=True, exist_ok=True)
    for name in ("install.py", "inputs.py", "adapter.py"):
        source = REPO / "tools/chat" / name
        if source.exists():
            shutil.copyfile(source, root / "tools/chat" / name)
    shutil.copytree(
        REPO / "tools/gate", root / "tools/gate", ignore=shutil.ignore_patterns("__pycache__")
    )
    write_tree(
        root,
        {
            "tools/chat/codekarl_chat.py": "VALUE = 1\n",
            "tools/chat/identity_conflicts.py": "VALUE = 1\n",
            "tools/chat/ck_chat.py": "VALUE = 1\n",
            "tools/chat/test_integration.py": chat_test,
            "tools/chat/test_identity_conflict.py": PASSING,
            "tools/ck-chat": "#!/usr/bin/env python3\n",
            "skills/ck-chat/SKILL.md": "# ck-chat\n",
        },
    )
    return root


def expected_inputs(root: str | Path) -> dict[str, str]:
    """Every regular file under chat/ and project-manager/, minus __pycache__."""
    files: dict[str, str] = {}
    for tree in ("chat", "project-manager"):
        for current, directories, names in os.walk(Path(root) / tree):
            directories[:] = [d for d in directories if d != "__pycache__"]
            for name in names:
                path = Path(current) / name
                files[path.relative_to(root).as_posix()] = hashlib.sha256(
                    path.read_bytes()
                ).hexdigest()
    return files


def expected_aggregate(files: Mapping[str, str]) -> str:
    text = json.dumps(files, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(text.encode()).hexdigest()


def record_for(files: Mapping[str, str]) -> dict[str, object]:
    return {"schema": 1, "files": files, "aggregate": expected_aggregate(files)}


def write_record(path: str | Path, files: Mapping[str, str]) -> None:
    Path(path).write_text(json.dumps(record_for(files)))


@contextlib.contextmanager
def shared_env(shared: str | Path) -> Iterator[None]:
    with mock.patch.dict(os.environ, {"CK_CHAT_SHARED_ROOT": str(shared)}):
        yield


Completed = subprocess.CompletedProcess[bytes] | subprocess.CompletedProcess[str]


def quiet_run(real_run: Callable[..., Completed]) -> Callable[..., Completed]:
    """subprocess.run that keeps a passing nested suite silent and shows a failing one in full.

    Output is captured unless the caller already routes the streams. On a nonzero exit (or a
    CalledProcessError) the captured stdout and stderr are replayed, whole, to this process's
    stdout and stderr, so a failure never loses its diagnostics."""

    def replay(stdout: str | bytes | None, stderr: str | bytes | None) -> None:
        for stream, data in ((sys.stdout, stdout), (sys.stderr, stderr)):
            if data:
                text = data if isinstance(data, str) else data.decode(errors="replace")
                stream.write(text)
                stream.flush()

    def run(arguments: object, **kwargs: object) -> Completed:
        captured = not {"stdout", "stderr", "capture_output"} & kwargs.keys()
        if captured:
            kwargs["capture_output"] = True
        try:
            result = real_run(arguments, **kwargs)
        except subprocess.CalledProcessError as error:
            if captured:
                replay(error.stdout, error.stderr)
            raise
        if captured and result.returncode:
            replay(result.stdout, result.stderr)
        return result

    return run
