"""S7c rollout: a `tools/ck` from before the launcher moved, and one after, against either checkpoint.

A copy synced before S7c keeps its own `tools/ck` (with the launcher string inside it) and must still
work against a checkpoint accepted after S7c; a copy whose `tools/ck` is newer than the selected
checkpoint (a worker that changed the entry point, or the first runs after publication) must work
against a checkpoint accepted before it. Every case runs the real trampoline in a disposable fixture
hub: nothing here touches this project's hub.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import pipeline
from gate.pointer import atomic_bytes, canonical

REPO = Path(__file__).resolve().parents[1]
OLD_CK = REPO / "tests/fixtures/trampoline/ck-before-s7c"
NEW_CK = REPO / "tools/ck"


def build_checkpoint(directory: Path, *, with_launcher: bool) -> Path:
    """A Pijul repository holding this tree's controller (`tools/*.py`, `tools/gate/`)."""
    directory.mkdir()
    pipeline.pijul(directory, "init")
    (directory / ".ignore").write_text("/.pipeline\n/.ck-hub\n__pycache__\n")
    tools = directory / "tools"
    tools.mkdir()
    for source in (REPO / "tools").glob("*.py"):
        if with_launcher or source.name != "launcher.py":
            shutil.copy2(source, tools / source.name)
    shutil.copytree(
        REPO / "tools/gate", tools / "gate", ignore=shutil.ignore_patterns("__pycache__")
    )
    pipeline.pijul(directory, "add", "--recursive", ".")
    pipeline.pijul(directory, "record", "-am", "checkpoint controller", "--author", "Rollout tests")
    return directory


class Rollout(unittest.TestCase):
    with_launcher: Path
    without_launcher: Path
    temp: tempfile.TemporaryDirectory[str]

    @classmethod
    def setUpClass(cls) -> None:
        cls.temp = tempfile.TemporaryDirectory(prefix="ck-rollout-")
        base = Path(cls.temp.name).resolve()
        cls.with_launcher = build_checkpoint(base / "checkpoint-with-launcher", with_launcher=True)
        cls.without_launcher = build_checkpoint(
            base / "checkpoint-before-launcher", with_launcher=False
        )

    @classmethod
    def tearDownClass(cls) -> None:
        cls.temp.cleanup()

    def fixture(self, name: str, ck: Path, checkpoint: Path) -> tuple[Path, Path, Path]:
        """(caller copy, hub, marker) with `ck` as the copy's tools/ck, selecting `checkpoint`."""
        base = Path(self.temp.name).resolve() / name
        hub, root = base / "hub", base / "caller"
        (hub / ".pipeline").mkdir(parents=True)
        (root / "tools").mkdir(parents=True)
        shutil.copy2(REPO / "rust-toolchain.toml", root / "rust-toolchain.toml")
        shutil.copy2(ck, root / "tools/ck")
        (root / ".ck-hub").write_text(f"{hub}\n")
        marker = base / "local-module-imported"
        for module in (
            "pipeline",
            "environment",
            "launcher",
        ):  # modules a caller copy must never run
            (root / f"tools/{module}.py").write_text(
                f"from pathlib import Path\nPath({str(marker)!r}).write_text('{module}')\n"
            )
        pointer = {
            "schema": 2,
            "repo": str(checkpoint),
            "fingerprint": pipeline.fingerprint(checkpoint),
            "submissions": ["accepted"],
        }
        atomic_bytes(hub / ".pipeline/accepted-v2.json", canonical(pointer))
        atomic_bytes(hub / ".pipeline/accepted.json", canonical({"schema": "tombstone"}))
        return root, hub, marker

    @staticmethod
    def ck(root: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "-B", str(root / "tools/ck"), *arguments],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
        )

    def assert_status(self, result: subprocess.CompletedProcess[str], marker: Path) -> None:
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["accepted"], ["accepted"])
        self.assertEqual(report["schema"], 2)
        if marker.exists():
            self.fail(f"the caller copy's own module ran: {marker.read_text()}")

    def test_01_a_trampoline_from_before_s7c_runs_an_s7c_checkpoint(self) -> None:
        root, _, marker = self.fixture("old-new", OLD_CK, self.with_launcher)
        self.assert_status(self.ck(root, "status"), marker)

    def test_02_the_s7c_trampoline_runs_an_s7c_checkpoint(self) -> None:
        root, _, marker = self.fixture("new-new", NEW_CK, self.with_launcher)
        self.assert_status(self.ck(root, "status"), marker)

    def test_03_the_s7c_trampoline_runs_a_checkpoint_accepted_before_the_launcher_existed(
        self,
    ) -> None:
        root, _, marker = self.fixture("new-old", NEW_CK, self.without_launcher)
        self.assertFalse((self.without_launcher / "tools/launcher.py").exists())
        self.assert_status(self.ck(root, "status"), marker)

    def test_04_a_trampoline_from_before_s7c_runs_a_checkpoint_before_the_launcher(self) -> None:
        root, _, marker = self.fixture("old-old", OLD_CK, self.without_launcher)
        self.assert_status(self.ck(root, "status"), marker)

    def test_05_the_s7c_trampoline_still_refuses_what_it_refused(self) -> None:
        root, hub, marker = self.fixture("refusals", NEW_CK, self.with_launcher)
        edited = self.with_launcher / "tools/edited.py"
        edited.write_text("VALUE = 1\n")  # an untracked change: the checkpoint no longer verifies
        try:
            result = self.ck(root, "status")
        finally:
            edited.unlink()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Accepted checkpoint has pending changes", result.stderr)
        self.assertFalse(marker.exists())
        atomic_bytes(
            hub / ".pipeline/accepted-v2.json",
            canonical({"schema": "bootstrap-hold", "bootstrap": "b"}),
        )
        held = self.ck(root, "status")
        self.assertNotEqual(held.returncode, 0)
        self.assertIn("bootstrap b in progress", held.stderr)

    def test_06_the_s7c_trampoline_and_the_old_one_report_the_same_failures(self) -> None:
        for ck in (OLD_CK, NEW_CK):
            with self.subTest(ck.name):
                root, hub, _ = self.fixture(f"errors-{ck.name}", ck, self.with_launcher)
                (hub / ".pipeline/accepted-v2.json").unlink()
                (hub / ".pipeline/accepted.json").unlink()
                result = self.ck(root, "status")
                self.assertEqual(result.returncode, 1)
                self.assertTrue(result.stderr.startswith("ck: "), result.stderr)

    def preload(self, name: str, module: str) -> dict[str, str]:
        """An environment whose `sitecustomize` plants a foreign `module` in every new interpreter.

        The foreign module claims a file outside the checkpoint and writes `marker` if anything of it
        is ever called, so a refusal is proven by the marker's absence as well as the exit status."""
        base = Path(self.temp.name).resolve() / name
        site = base / "site"
        site.mkdir(parents=True, exist_ok=True)
        marker = base / "foreign-ran"
        (site / "sitecustomize.py").write_text(
            "import sys, types\n"
            f"fake = types.ModuleType({module!r})\n"
            f"fake.__file__ = {str(base / 'elsewhere' / (module + '.py'))!r}\n"
            "def anything(*arguments, **keywords):\n"
            f"    open({str(marker)!r}, 'w').write('foreign ' + {module!r} + ' ran')\n"
            "    print('FOREIGN_RAN')\n"
            "fake.run = fake.configure = fake.signing = fake.main = anything\n"
            "fake.lock = fake.ci_descriptor = fake.hub_for = anything\n"
            f"sys.modules[{module!r}] = fake\n"
        )
        return {"PYTHONPATH": str(site), "CK_TEST_MARKER": str(marker)}

    def ck_with(
        self, root: Path, extra: dict[str, str], *arguments: str
    ) -> subprocess.CompletedProcess[str]:
        environment = {**os.environ, **extra}
        return subprocess.run(
            [sys.executable, "-B", str(root / "tools/ck"), *arguments],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
            env=environment,
        )

    def test_07_a_foreign_controller_module_preloaded_into_the_interpreter_is_refused(self) -> None:
        # A module already present in sys.modules is not replaced by the checkpoint's path entry, so
        # each controller module's origin is checked, the launcher's by the trampoline's own string
        # before it runs anything (review of S7c, B1) and the rest by the launcher.
        for ck in (NEW_CK, OLD_CK):
            for module in ("launcher", "environment", "pipeline"):
                if ck is OLD_CK and module == "launcher":
                    continue  # the old launcher string never imports a launcher module
                with self.subTest(f"{ck.name}: foreign {module}"):
                    root, _, caller_marker = self.fixture(
                        f"preload-{ck.name}-{module}", ck, self.with_launcher
                    )
                    extra = self.preload(f"preload-{ck.name}-{module}-site", module)
                    result = self.ck_with(root, extra, "status")
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertNotIn("FOREIGN_RAN", result.stdout)
                    self.assertIn("controller origin check failed", result.stderr)
                    self.assertFalse(Path(extra["CK_TEST_MARKER"]).exists(), "foreign code ran")
                    self.assertFalse(caller_marker.exists())

    def test_08_without_the_preload_the_same_environment_runs_normally(self) -> None:
        # The control: a sitecustomize that plants nothing foreign changes nothing.
        root, _, marker = self.fixture("preload-control", NEW_CK, self.with_launcher)
        site = Path(self.temp.name).resolve() / "preload-control-site"
        site.mkdir()
        (site / "sitecustomize.py").write_text("VALUE = 1\n")
        self.assert_status(self.ck_with(root, {"PYTHONPATH": str(site)}, "status"), marker)

    def test_09_a_foreign_launcher_file_earlier_on_pythonpath_is_not_the_one_imported(self) -> None:
        # The checkpoint's `tools` is put first on the path, so a `launcher.py` that PYTHONPATH (or
        # the working directory) offers is never found; if it were, the launch string's origin check
        # would refuse it. Either way no foreign code runs and the command completes.
        for ck in (NEW_CK, OLD_CK):
            with self.subTest(ck.name):
                root, _, caller_marker = self.fixture(f"shadow-{ck.name}", ck, self.with_launcher)
                site = Path(self.temp.name).resolve() / f"shadow-{ck.name}-site"
                site.mkdir()
                marker = site / "foreign-ran"
                (site / "launcher.py").write_text(
                    f"open({str(marker)!r}, 'w').write('foreign launcher file')\n"
                    "def run(*arguments, **keywords): print('FOREIGN_RAN')\n"
                )
                result = self.ck_with(root, {"PYTHONPATH": str(site)}, "status")
                self.assert_status(result, caller_marker)
                self.assertNotIn("FOREIGN_RAN", result.stdout)
                self.assertFalse(marker.exists(), "the foreign launcher file was executed")


if __name__ == "__main__":
    unittest.main()
