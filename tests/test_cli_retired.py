"""CK-40: `ck --help` lists only current commands; retired `accept` and `retry` still refuse.

Every case runs the real `tools/ck` trampoline against a post-activation fixture hub (the accepted
checkpoint holds this tree's controller), as `test_bootstrap.py` does for the `retry` refusal.
"""

import hashlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import pipeline
import procfix
from environment import signing
from gate import pointer

REPO = Path(__file__).resolve().parents[1]
CURRENT = (
    "init",
    "status",
    "worker",
    "doctor",
    "record",
    "issue",
    "meta",
    "review",
    "session",
    "withdraw",
    "batch",
    "docs",
    "gc",
    "sync",
    "submit",
)
RETIRED = ("accept", "retry")


def snapshot(*roots: Path) -> dict[str, str]:
    """Every file, directory and symlink under each root, keyed by path, with a digest of its bytes."""
    result: dict[str, str] = {}
    for root in roots:
        if not root.exists():
            result[str(root)] = "absent"
            continue
        for path in [root, *sorted(root.rglob("*"))]:
            if path.is_symlink():
                result[str(path)] = f"link {path.readlink()}"
            elif path.is_dir():
                result[str(path)] = "dir"
            else:
                result[str(path)] = hashlib.sha256(path.read_bytes()).hexdigest()
    return result


class RetiredCommands(unittest.TestCase):
    def setUp(self) -> None:
        procfix.isolate(self)
        self.temp = tempfile.TemporaryDirectory(prefix="ck-cli-retired-")
        self.addCleanup(self.temp.cleanup)
        base = Path(self.temp.name).resolve()
        self.hub = base / "hub"
        self.meta = base / "codekarl-meta"
        self.root = base / "caller"
        # As on the real hub, the current accepted checkpoint lives under `.pipeline/checkpoints`.
        checkpoint = self.hub / ".pipeline/checkpoints/current"
        self.checkpoint = checkpoint
        for directory in (self.meta, self.root / "tools", checkpoint):
            directory.mkdir(parents=True)
        (self.meta / "counters.json").write_text('{"issue": 0}\n')
        (self.root / "work.txt").write_text("caller working copy\n")
        shutil.copy2(REPO / "rust-toolchain.toml", self.root / "rust-toolchain.toml")
        shutil.copy2(REPO / "tools/ck", self.root / "tools/ck")
        (self.root / ".ck-hub").write_text(f"{self.hub}\n")
        with signing(Path.home() / "work/codekarl"):
            pipeline.pijul(checkpoint, "init")
            (checkpoint / ".ignore").write_text("/.pipeline\n/.ck-hub\n")
            (checkpoint / "tools").mkdir()
            for name in ("pipeline.py", "environment.py", "launcher.py", "check.py"):
                shutil.copy2(REPO / "tools" / name, checkpoint / "tools" / name)
            shutil.copytree(REPO / "tools/gate", checkpoint / "tools/gate")
            pipeline.pijul(checkpoint, "add", "--recursive", ".")
            pipeline.pijul(checkpoint, "record", "-am", "checkpoint", "--author", "CLI tests")
            self.state = {
                "schema": 2,
                "repo": str(checkpoint),
                "fingerprint": pipeline.fingerprint(checkpoint),
                "submissions": [],
            }
        pointer.atomic_bytes(self.hub / ".pipeline/accepted-v2.json", pointer.canonical(self.state))
        pointer.atomic_bytes(
            self.hub / ".pipeline/accepted.json", pointer.canonical({"schema": "tombstone"})
        )

    def ck(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "-B", str(self.root / "tools/ck"), *args],
            cwd=self.root,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_01_help_lists_only_current_commands(self) -> None:
        for flag in ("--help", "-h"):
            result = self.ck(flag)
            self.assertEqual(result.returncode, 0, result.stderr)
            choices = re.search(r"\{([^}]*)\}", result.stdout)
            assert choices is not None
            self.assertEqual(tuple(choices.group(1).split(",")), CURRENT)
            for name in CURRENT:
                self.assertRegex(result.stdout, rf"\b{name}\b")
            for name in RETIRED:
                self.assertNotRegex(result.stdout, rf"\b{name}\b", flag)

    def test_02_unknown_command_usage_omits_retired_commands(self) -> None:
        result = self.ck("nosuch")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("usage:", result.stderr)
        self.assertIn("invalid choice: 'nosuch'", result.stderr)
        for name in RETIRED:
            self.assertNotRegex(result.stderr.replace("'nosuch'", ""), rf"\b{name}\b")

    def test_03_retired_commands_refuse_with_guidance_and_write_nothing(self) -> None:
        # The `tools/ck` entry verifies the current checkpoint with Pijul before any controller
        # runs, which may rewrite the checkpoint's pristine database for every command. That one
        # file is the only allowed difference; the rest of `.pipeline/` (pointers, receipts and
        # the checkpoint's other files), the meta store and the caller's copy must not change.
        allowed = {str(self.checkpoint / ".pijul/pristine/db")}
        before = snapshot(self.hub / ".pipeline", self.meta, self.root)
        cases: tuple[tuple[str, ...], ...] = (
            ("accept",),
            ("accept", "--help"),
            ("accept", "BID", "extra"),
            ("retry",),
            ("retry", "ID"),
            ("retry", "ID", "extra"),
            ("retry", "--help"),
        )
        for args in cases:
            result = self.ck(*args)
            self.assertNotEqual(result.returncode, 0, args)
            self.assertEqual(result.stdout, "", args)
            self.assertNotIn("invalid choice", result.stderr, args)
            self.assertEqual(len(result.stderr.strip().splitlines()), 1, (args, result.stderr))
            if args[0] == "retry":
                self.assertIn("ck retry is retired", result.stderr, args)
                self.assertIn("ck submit --supersedes ID", result.stderr, args)
            else:
                self.assertIn("ck accept is removed", result.stderr, args)
                self.assertIn("ck batch prepare", result.stderr, args)
                self.assertIn("single-submission", result.stderr, args)
                self.assertIn("ck batch publish", result.stderr, args)
                self.assertIn("ck review batch", result.stderr, args)
            after = snapshot(self.hub / ".pipeline", self.meta, self.root)
            changed = {
                path for path in before.keys() | after.keys() if before.get(path) != after.get(path)
            }
            self.assertLessEqual(changed, allowed, args)
        # The checkpoint's logical identity is unchanged: its patches and file map still equal the
        # accepted pointer's fingerprint.
        self.assertEqual(pipeline.fingerprint(self.checkpoint), self.state["fingerprint"])


if __name__ == "__main__":
    unittest.main()
