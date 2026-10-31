"""Specification tests using real Pijul repositories, not a mocked merge model."""

import json
import subprocess
import sys
import tempfile
import unittest
from collections.abc import Callable
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from environment import lock
from gate.records import AcceptedPointer, is_accepted_pointer
from pipeline import Pipeline, fingerprint, hashes, pijul, read


class BatchSpecification(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="ck-spec-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name) / "project"
        self.root.mkdir()
        pijul(self.root, "init")
        (self.root / ".ignore").write_text("/.pipeline\n/.ck-hub\n")
        (self.root / "engine.txt").write_text("baseline\n")
        (self.root / "README.md").write_text("Initial docs\n")
        self.record(self.root, "Initial")
        self.calls: list[set[str]] = []
        self.checker: Callable[[Path], bool] = lambda repo: True
        self.pipeline = Pipeline(self.root, self.validate)
        self.pipeline.initialize(self.root)
        self.calls.clear()

    def validate(self, repo: Path, log: Path) -> bool:
        self.calls.append(set(fingerprint(repo)["files"]))
        log.write_text("Test validator\n")
        return self.checker(repo)

    def accepted(self) -> AcceptedPointer:
        """The accepted pointer (every state these tests reach is an accepted one)."""
        state = self.pipeline.state()
        assert is_accepted_pointer(state)
        return state

    def record(self, repo: Path, message: str) -> None:
        pijul(repo, "add", "--recursive", ".")
        pijul(repo, "record", "-am", message, "--author", "Pipeline specification")

    def worker(self, name: str, filename: str | None = None, content: str = "new\n") -> Path:
        worker = self.pipeline.worker(name)
        if filename:
            (worker / filename).write_text(content)
            self.record(worker, name)
        return worker

    def submit(self, worker: Path) -> str:
        return self.pipeline.submit(worker, "Exercise the batch acceptance specification")

    def test_independent_patches_pass_in_one_combined_run(self) -> None:
        left = self.submit(self.worker("left", "left.txt"))
        right = self.submit(self.worker("right", "right.txt"))
        accepted = self.pipeline.accept()
        self.assertCountEqual(accepted, [left, right])
        self.assertEqual(len(self.calls), 1)
        self.assertTrue({"left.txt", "right.txt"} <= self.calls[0])
        self.assertFalse((self.root / "left.txt").exists())
        self.pipeline.sync(self.root)
        self.assertTrue((self.root / "left.txt").exists())
        self.assertEqual(hashes(self.root), set(self.pipeline.state()["fingerprint"]["hashes"]))

    def test_failed_batch_splits_and_does_not_poison_accepted_state(self) -> None:
        good = self.submit(self.worker("good", "good.txt"))
        bad = self.submit(self.worker("bad", "bad.txt"))
        self.checker = lambda repo: not (repo / "bad.txt").exists()
        self.assertEqual(self.pipeline.accept(), [good])
        self.assertEqual(len(self.calls), 3)
        self.assertEqual(self.pipeline.blocked(), {bad})
        accepted = Path(self.accepted()["repo"])
        self.assertTrue((accepted / "good.txt").exists())
        self.assertFalse((accepted / "bad.txt").exists())

    def test_separately_valid_patches_with_bad_interaction_are_not_combined(self) -> None:
        first = self.submit(self.worker("first", "one.txt"))
        second = self.submit(self.worker("second", "two.txt"))
        self.checker = lambda repo: not (
            (repo / "one.txt").exists() and (repo / "two.txt").exists()
        )
        self.assertEqual(self.pipeline.accept(), [first])
        self.assertIn(second, self.pipeline.blocked())
        self.assertEqual(len(self.calls), 3)
        self.assertTrue({"one.txt", "two.txt"} <= self.calls[-1])

    def test_new_submission_waits_for_next_frozen_batch(self) -> None:
        first = self.submit(self.worker("first", "first.txt"))
        second_worker = self.worker("second", "second.txt")
        later: list[str] = []

        def check(repo: Path) -> bool:
            later.append(self.submit(second_worker))
            return True

        self.checker = check
        self.assertEqual(self.pipeline.accept(), [first])
        self.assertNotIn(later[0], self.pipeline.state()["submissions"])
        self.assertEqual(len(self.calls), 1)
        self.checker = lambda repo: True
        self.assertEqual(self.pipeline.accept(), later)

    def test_dependent_submissions_are_an_indivisible_split_group(self) -> None:
        worker = self.worker("stack", "prerequisite.txt")
        prerequisite = self.submit(worker)
        (worker / "dependent.txt").write_text("Uses prerequisite\n")
        self.record(worker, "Dependent")
        dependent = self.submit(worker)
        good = self.submit(self.worker("unrelated", "good.txt"))
        self.checker = lambda repo: not (repo / "prerequisite.txt").exists()
        self.assertEqual(self.pipeline.accept(), [good])
        self.assertEqual(self.pipeline.blocked(), {prerequisite, dependent})
        for attempted in self.calls:
            self.assertEqual("prerequisite.txt" in attempted, "dependent.txt" in attempted)
        with self.assertRaisesRegex(RuntimeError, "quarantined"):
            self.submit(worker)

    def test_check_cannot_change_source_then_publish(self) -> None:
        submission = self.submit(self.worker("worker", "new.txt"))
        original = self.pipeline.state()

        def check(repo: Path) -> bool:
            (repo / "engine.txt").write_text("changed during test\n")
            return True

        self.checker = check
        self.assertEqual(self.pipeline.accept(), [])
        self.assertEqual(self.pipeline.state(), original)
        self.assertIn(submission, self.pipeline.blocked())

    def test_late_dependent_submission_cannot_bypass_quarantine(self) -> None:
        worker = self.worker("stack", "bad.txt")
        first = self.submit(worker)
        (worker / "dependent.txt").write_text("dependent\n")
        self.record(worker, "dependent")
        later: list[str] = []

        def check(repo: Path) -> bool:
            later.append(self.submit(worker))
            return False

        self.checker = check
        self.pipeline.accept()
        self.assertIn(first, self.pipeline.blocked())
        self.checker = lambda repo: True
        self.assertEqual(self.pipeline.accept(), [])
        self.assertIn(later[0], self.pipeline.blocked())
        self.assertEqual(len(self.calls), 1)

    def test_checkpoint_tampering_fails_closed(self) -> None:
        accepted = Path(self.accepted()["repo"])
        (accepted / "engine.txt").write_text("tampered\n")
        with self.assertRaisesRegex(RuntimeError, "pending changes"):
            self.pipeline.worker("fresh")

    def test_queued_changes_are_captured_before_worker_changes(self) -> None:
        worker = self.worker("worker", "first.txt")
        self.submit(worker)
        (worker / "later.txt").write_text("not submitted\n")
        self.record(worker, "Later")
        self.pipeline.accept()
        self.assertNotIn("later.txt", self.calls[0])

    def test_only_one_acceptance_or_full_check_owns_ci_slot(self) -> None:
        self.submit(self.worker("worker", "new.txt"))
        with (
            lock(self.pipeline.directory / "ci.lock"),
            self.assertRaisesRegex(RuntimeError, "Already running"),
        ):
            self.pipeline.accept()
        self.assertEqual(self.calls, [])
        self.assertEqual(len(self.pipeline.accept()), 1)

    def test_running_check_child_keeps_ci_slot_after_parent_releases_it(self) -> None:
        path = self.pipeline.directory / "ci.lock"
        child: subprocess.Popen[str] | None = None
        try:
            with lock(path) as descriptor:
                child = subprocess.Popen(
                    [
                        sys.executable,
                        "-c",
                        "import sys; print('ready', flush=True); sys.stdin.read()",
                    ],
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    text=True,
                    pass_fds=(descriptor,),
                )
                assert child.stdout is not None
                self.assertEqual(child.stdout.readline().strip(), "ready")
            with self.assertRaisesRegex(RuntimeError, "Already running"), lock(path):
                self.fail("A running check must retain the slot")
        finally:
            if child is not None:
                child.communicate(timeout=5)
        with lock(path):
            pass

    def test_crash_before_pointer_publication_leaves_batch_pending(self) -> None:
        submission = self.submit(self.worker("worker", "new.txt"))
        original = self.pipeline.state()

        def crash(*args: object, **kwargs: object) -> None:
            raise RuntimeError("simulated interruption before publication")

        with mock.patch.object(self.pipeline, "publish", crash):
            with self.assertRaisesRegex(RuntimeError, "interruption"):
                self.pipeline.accept()
            self.assertEqual(self.pipeline.state(), original)
            self.assertNotIn(submission, self.pipeline.blocked())
        self.assertEqual(self.pipeline.accept(), [submission])

    def test_structural_conflict_never_reaches_ci_or_accepted_checkpoint(self) -> None:
        first = self.submit(self.worker("first", "engine.txt", "first\n"))
        second = self.submit(self.worker("second", "engine.txt", "second\n"))
        self.assertEqual(self.pipeline.accept(), [first])
        self.assertEqual(len(self.calls), 1)
        self.assertIn(second, self.pipeline.blocked())
        self.assertEqual((Path(self.accepted()["repo"]) / "engine.txt").read_text(), "first\n")

    def test_docs_publish_without_checks_and_preserve_code(self) -> None:
        before = self.accepted()
        (self.root / "README.md").write_text("Updated explanation\n")
        (self.root / "engine.txt").write_text("Unfinished code\n")
        self.pipeline.docs(self.root, ["README.md"], "Docs")
        state = self.accepted()
        self.assertEqual(self.calls, [])
        self.assertEqual(
            state["fingerprint"]["files"]["engine.txt"],
            before["fingerprint"]["files"]["engine.txt"],
        )
        receipt_path = state.get("receipt")
        assert receipt_path is not None
        docs_receipt = read(Path(receipt_path))
        assert isinstance(docs_receipt, dict)
        self.assertEqual(docs_receipt["kind"], "docs")
        self.assertEqual((Path(state["repo"]) / "README.md").read_text(), "Updated explanation\n")
        status = json.loads(pijul(self.root, "status", "--output-format", "json"))
        self.assertEqual([change["path"] for change in status["changes"]], ["engine.txt"])
        self.assertEqual((self.root / "engine.txt").read_text(), "Unfinished code\n")

    def test_new_docs_are_published_and_working_copy_is_clean(self) -> None:
        (self.root / "docs").mkdir()
        (self.root / "docs/design.md").write_text("A design\n")
        self.pipeline.docs(self.root, ["docs/design.md"], "Design")
        status = json.loads(pijul(self.root, "status", "--output-format", "json"))
        self.assertEqual(status["changes"], [])
        self.assertEqual(status["untracked"], [])

    def test_approved_pijul_record_hook_blocks_a_failed_check(self) -> None:
        worker = self.worker("hook")
        (worker / "pijul.toml").write_text('[hooks]\nrecord = ["exit 17"]\n')
        pijul(worker, "hooks", "approve")
        (worker / "engine.txt").write_text("blocked\n")
        before = hashes(worker)
        with self.assertRaises(RuntimeError):
            self.record(worker, "Must fail")
        self.assertEqual(hashes(worker), before)

    def test_docs_lane_rejects_executable_markdown_and_parent_traversal(self) -> None:
        (self.root / "AGENTS.md").write_text("Instructions\n")
        with self.assertRaisesRegex(RuntimeError, "ordinary documentation"):
            self.pipeline.docs(self.root, ["AGENTS.md"], "Cannot bypass")
        (self.root / "docs").mkdir()
        with self.assertRaisesRegex(RuntimeError, "ordinary documentation"):
            self.pipeline.docs(self.root, ["docs/../AGENTS.md"], "Cannot bypass")

    def test_full_acceptance_receipt_records_exact_patch_set(self) -> None:
        self.submit(self.worker("worker", "new.txt"))
        self.pipeline.accept()
        state = self.accepted()
        receipt_path = state.get("receipt")
        assert receipt_path is not None
        receipt = json.loads(Path(receipt_path).read_text())
        self.assertEqual(receipt["fingerprint"], fingerprint(Path(state["repo"])))
        self.assertTrue(receipt["passed"])

    def test_initial_check_failure_does_not_create_an_accepted_checkpoint(self) -> None:
        source = self.root.parent / "fresh"
        pijul(self.root, "clone", self.root, source)
        before = fingerprint(source)
        pipeline = Pipeline(source, lambda repo, log: False)
        with self.assertRaisesRegex(RuntimeError, "nothing was accepted"):
            pipeline.initialize(source)
        self.assertFalse(pipeline.pointer.exists())
        self.assertEqual(fingerprint(source), before)


if __name__ == "__main__":
    unittest.main()
