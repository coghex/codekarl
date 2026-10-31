"""S4: project locks record who holds them, and a contender's message names the holder."""

import json
import os
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from gate import locks

TOOLS = Path(__file__).resolve().parents[1] / "tools"

HOLDER = """
import subprocess, sys, time
sys.path.insert(0, {tools!r})
from pathlib import Path
from gate import locks
sys.argv = ["ck", "batch", "publish"]
with locks.acquire(Path({path!r}), {kind!r}) as fd:
    {inherit}
    print("ready", flush=True)
    time.sleep({seconds})
"""


class LockHolders(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ck-lock-holders-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(os.path.realpath(self.temp.name))
        self.paths = {
            "ci": self.base / "hub/.pipeline/ci.lock",
            "state": self.base / "hub/.pipeline/state.lock",
            "meta": self.base / "meta/.lock",
        }
        for path in self.paths.values():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.touch()
        self.children: list[subprocess.Popen[str]] = []
        self.addCleanup(self.reap)

    def reap(self) -> None:
        for process in self.children:
            if process.poll() is None:
                process.kill()
            process.wait()

    def hold(self, kind: str, *, inherit: str = "", seconds: int = 60) -> subprocess.Popen[str]:
        code = HOLDER.format(
            tools=str(TOOLS),
            path=str(self.paths[kind]),
            kind=kind,
            inherit=inherit,
            seconds=seconds,
        )
        process = subprocess.Popen(
            [sys.executable, "-c", textwrap.dedent(code)], stdout=subprocess.PIPE, text=True
        )
        self.children.append(process)
        assert process.stdout is not None
        self.assertEqual(process.stdout.readline().strip(), "ready")
        return process

    def test_01_holder_metadata_is_written_in_place_for_each_lock(self) -> None:
        for kind, path in self.paths.items():
            with self.subTest(kind):
                before = path.stat().st_ino
                process = self.hold(kind)
                record = json.loads(path.read_bytes())
                self.assertEqual(record["pid"], process.pid)
                self.assertEqual(record["command"], "ck batch publish")
                self.assertAlmostEqual(record["started"], time.time(), delta=30)
                self.assertEqual(path.stat().st_ino, before)  # same inode: written in place
                process.kill()
                process.wait()

    def test_02_a_contender_timeout_names_pid_command_and_age(self) -> None:
        for kind in ("state", "meta"):
            with self.subTest(kind):
                process = self.hold(kind)
                time.sleep(1.2)
                with (
                    self.assertRaises(RuntimeError) as caught,
                    locks.acquire(self.paths[kind], kind, timeout=0.3),
                ):
                    pass
                message = str(caught.exception)
                self.assertIn(f"Timed out waiting for {kind}.lock", message)
                self.assertIn(f"held by pid {process.pid} (`ck batch publish`, ", message)
                self.assertRegex(message, r", \d+ s\)")
                process.kill()
                process.wait()

    def test_03_a_busy_ci_lock_names_the_holder(self) -> None:
        process = self.hold("ci")
        with (
            self.assertRaises(RuntimeError) as caught,
            locks.acquire(self.paths["ci"], "ci", blocking=False),
        ):
            pass
        message = str(caught.exception)
        self.assertIn("Already running", message)
        self.assertIn(f"pid {process.pid}", message)
        self.assertIn("`ck batch publish`", message)

    def test_04_a_dead_pid_with_a_held_lock_is_reported_as_such(self) -> None:
        # The recorded holder exits, but a child that inherited the descriptor keeps the lock.
        inherit = (
            "child = subprocess.Popen(['sleep', '60'], pass_fds=(fd,))\n"
            "    print(child.pid, flush=True)\n"
        )
        code = HOLDER.format(
            tools=str(TOOLS),
            path=str(self.paths["state"]),
            kind="state",
            inherit=inherit,
            seconds=0,
        )
        holder = subprocess.Popen(
            [sys.executable, "-c", textwrap.dedent(code)], stdout=subprocess.PIPE, text=True
        )
        self.children.append(holder)
        assert holder.stdout is not None
        child_pid = int(holder.stdout.readline())
        self.addCleanup(lambda: os.kill(child_pid, 9))
        holder.wait()
        with (
            self.assertRaises(RuntimeError) as caught,
            locks.acquire(self.paths["state"], "state", timeout=0.3),
        ):
            pass
        message = str(caught.exception)
        self.assertIn(f"pid {holder.pid}", message)
        self.assertIn("which has exited", message)
        self.assertIn("still held", message)
        self.assertIn("inherited the descriptor", message)

    def test_05_unusable_metadata_is_reported_not_trusted(self) -> None:
        process = self.hold("state")
        self.paths["state"].write_bytes(b'{"pid": ')  # a torn write
        with (
            self.assertRaises(RuntimeError) as caught,
            locks.acquire(self.paths["state"], "state", timeout=0.3),
        ):
            pass
        self.assertIn("wrote no usable metadata", str(caught.exception))
        process.kill()
        process.wait()


if __name__ == "__main__":
    unittest.main()
