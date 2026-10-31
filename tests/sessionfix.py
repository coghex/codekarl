"""Hermetic session fixtures: a plain owner shell, or a fixture Copilot client session."""

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
import uuid
from collections.abc import Iterable, Mapping, Sequence
from pathlib import Path
from unittest import mock

HOLDER = (
    "import fcntl, os, sys\n"
    "descriptor = os.open(sys.argv[1], os.O_RDWR | os.O_CREAT)\n"
    "if sys.argv[2] == 'hold':\n"
    "    fcntl.flock(descriptor, fcntl.LOCK_EX)\n"
    "print('ready', flush=True)\n"
    "sys.stdin.read()\n"
)


def no_agent(test: unittest.TestCase) -> None:
    """Make the real resolver see a plain owner shell whatever session runs the suite."""
    patcher = mock.patch.dict(os.environ)
    patcher.start()
    test.addCleanup(patcher.stop)
    for name in ("COPILOT_AGENT_SESSION_ID", "CK_SESSION_EVIDENCE", "CK_CHAT_REGISTRY"):
        os.environ.pop(name, None)
    for target, value in (
        ("gate.sessions._ancestors", [os.getpid()]),
        ("gate.sessions._registry", []),
    ):
        patch = mock.patch(target, return_value=value)
        patch.start()
        test.addCleanup(patch.stop)


class CopilotSession:
    """A session directory under a temporary root and a real child that holds its flock."""

    def __init__(
        self,
        test: unittest.TestCase,
        model: str = "claude-opus-5.5",
        *,
        held: bool = True,
        events: Sequence[Mapping[str, object]] | None = None,
    ) -> None:
        self.test = test
        self.id = str(uuid.uuid4())
        self.temp = tempfile.TemporaryDirectory(prefix="ck-copilot-")
        test.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.directory = self.root / self.id
        self.directory.mkdir()
        self.log = self.directory / "events.jsonl"
        start = {"sessionId": self.id, "selectedModel": model, "reasoningEffort": "high"}
        lines = events or [
            {"type": "session.start", "data": start},
            {"type": "assistant.message", "data": {"model": model}},
        ]
        self.log.write_text("".join(json.dumps(line) + "\n" for line in lines))
        self.holder = subprocess.Popen(
            [
                sys.executable,
                "-c",
                HOLDER,
                str(self.directory / "hold"),
                "hold" if held else "free",
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        )
        test.addCleanup(self.stop)
        assert self.holder.stdout is not None
        self.holder.stdout.readline()
        self.pid = self.holder.pid
        (self.directory / "hold").rename(self.directory / f"inuse.{self.pid}.hold")
        self.lock = self.directory / f"inuse.{self.pid}.lock"
        self.lock.write_text(f"{self.pid}\n")
        self.hold = self.directory / f"inuse.{self.pid}.hold"

    def stop(self) -> None:
        assert self.holder.stdin is not None
        assert self.holder.stdout is not None
        self.holder.stdin.close()
        self.holder.stdout.close()
        self.holder.wait()

    def digest(self) -> str:
        return hashlib.sha256(self.log.read_bytes()).hexdigest()

    def activate(
        self,
        *,
        chain: Sequence[int] | None = None,
        executable: str = "copilot",
        registry: Iterable[object] = (),
        roots: Mapping[str, str] | None = None,
    ) -> "CopilotSession":
        """Install the process view: ancestors, executables, registry and environment."""
        test = self.test
        patcher = mock.patch.dict(os.environ)
        patcher.start()
        test.addCleanup(patcher.stop)
        for name in ("CK_SESSION_EVIDENCE", "CK_CHAT_REGISTRY"):
            os.environ.pop(name, None)
        os.environ["COPILOT_AGENT_SESSION_ID"] = self.id
        os.environ["CK_SESSION_ROOTS"] = json.dumps({"copilot": str(self.root), **(roots or {})})
        chain = [os.getpid(), self.pid] if chain is None else chain

        def names(pid: int) -> str:
            return executable if pid == self.pid else "bash"

        for target, kwargs in (
            ("gate.sessions._ancestors", {"return_value": chain}),
            ("gate.sessions._executable", {"side_effect": names}),
            ("gate.sessions._registry", {"return_value": list(registry)}),
        ):
            patch = mock.patch(target, **kwargs)
            patch.start()
            test.addCleanup(patch.stop)
        return self
