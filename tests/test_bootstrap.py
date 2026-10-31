"""Bootstrap prefixes and tombstone recovery use real legacy Pijul repositories."""

import hashlib
import importlib
import importlib.util
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from collections.abc import Callable, Collection, Mapping
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import pipeline
import procfix
from environment import signing
from gate import batch, bootstrap_core, docsqueue, meta, pointer, procscan, reviews
from gate import cli as gate_cli
from gate.records import (
    BootstrapValidated,
    DocsEntry,
    Fingerprint,
    ManifestLegacy,
    ProcessRow,
    RecordError,
    load_bootstrap_validated,
)

# Stable stand-in bytes for the gate plan: the tests never read the repository's own docs/.
SPEC_FIXTURE = Path(__file__).resolve().parent / "fixtures/bootstrap/spec.md"


def complete(kind: str, partial: Mapping[str, object]) -> dict[str, object]:
    """A fixture bootstrap record with every field its writer writes.

    The fixtures state only what their scenario depends on; the boundary that reads these records
    (`records.load_bootstrap_*`) requires the rest, so they are filled with neutral values."""
    zero = "0" * 64
    defaults: dict[str, dict[str, object]] = {
        "frozen": {
            "bootstrap": "fixture",
            "submission": "s1",
            "base_pointer_sha256": zero,
            "queue_snapshot": {},
            "manifest_sha256": zero,
            "fingerprint_sha256": zero,
            "superseded_legacy": [],
            "owner": {"pid": 1, "pid_start": "fixture"},
        },
        "failed": {"reason": "fixture", "at": 1},
        "cancelled": {"reason": "fixture", "at": 1},
        "restoring": {
            "mode": "restored",
            "pointer": "",
            "pointer_sha256": zero,
            "entries": [],
            "links": [],
            "conflicts": [],
        },
        "validated": {
            "bootstrap": "fixture",
            "submission": "s1",
            "base_pointer_sha256": zero,
            "manifest_sha256": zero,
            "fingerprint_sha256": zero,
            "candidate": "/fixture/candidate",
            "candidate_fingerprint": {"hashes": [], "files": {}},
            "candidate_fingerprint_sha256": zero,
            "ci_receipt": "/fixture/ci.json",
            "ci_receipt_sha256": zero,
            "helper_sha256": zero,
            "superseded_legacy": [],
        },
    }
    return {**defaults[kind], **partial}


class BootstrapGateSpecification(unittest.TestCase):
    def setUp(self) -> None:
        procfix.isolate(self)
        self.temp = tempfile.TemporaryDirectory(prefix="ck-bootstrap-gate-")
        self.addCleanup(self.temp.cleanup)
        self.signing = signing(Path.home() / "work/codekarl")
        self.signing.__enter__()
        self.addCleanup(self.signing.__exit__, None, None, None)
        self.hub = Path(self.temp.name) / "hub"
        self.hub.mkdir()
        self.base = Path(self.temp.name) / "base"
        self.base.mkdir()
        pipeline.pijul(self.base, "init")
        (self.base / ".ignore").write_text("/.pipeline\n/.ck-hub\n")
        (self.base / "source.txt").write_text("base\n")
        spec_source = SPEC_FIXTURE
        (self.base / "docs").mkdir()
        (self.base / "docs/review-gate-plan.md").write_bytes(spec_source.read_bytes())
        (self.base / "tools").mkdir()
        (self.base / "tools/check.py").write_text("raise SystemExit(0)\n")
        self.record(self.base, "legacy base")
        self.legacy = pipeline.Pipeline(self.hub, lambda repo, log: True)
        self.legacy.initialize(self.base)
        self.capture = self.hub / ".pipeline/submissions/s1"
        pipeline.clone(Path(self.legacy.state()["repo"]), self.capture, self.hub)
        (self.capture / "tools/gate").mkdir(parents=True)
        (self.capture / "tools/gate/bootstrap.py").write_text("# reviewed bootstrap helper\n")
        (self.capture / "tools/check.py").write_text("raise SystemExit(0)\n")
        self.record(self.capture, "gate helper")
        state = self.legacy.state()
        self.manifest: ManifestLegacy = {
            "id": "s1",
            "spec": "gate helper fixture",
            "repo": str(self.capture),
            "hashes": sorted(pipeline.hashes(self.capture) - set(state["fingerprint"]["hashes"])),
            "fingerprint": pipeline.fingerprint(self.capture),
        }
        queue = self.hub / ".pipeline/queue/s1.json"
        pointer.atomic_bytes(queue, pointer.canonical(self.manifest))
        self.manifest_path = queue
        self.initial_pointer = (self.hub / ".pipeline/accepted.json").read_bytes()

    def validated_record(self, bid: str) -> BootstrapValidated:
        """The bootstrap's `validated.json`, read through the checked loader (never raw JSON)."""
        path = self.hub / ".pipeline/bootstrap" / bid / "validated.json"
        return load_bootstrap_validated(json.loads(path.read_bytes()), str(path))

    def record(self, repo: Path, message: str) -> None:
        pipeline.pijul(repo, "add", "--recursive", ".")
        pipeline.pijul(repo, "record", "-am", message, "--author", "Bootstrap tests")

    def checker(self, candidate: Path, log: Path, ci_fd: int) -> bool:
        log.write_text("injected check passed\n")
        return True

    def evidence_for_publish(self, bid: str, *, copilot_alias: bool = False) -> tuple[Path, str]:
        evidence_root = self.hub / ".pipeline/coordination/gate-bootstrap"
        evidence_root.mkdir(parents=True, exist_ok=True)
        spec = SPEC_FIXTURE
        (self.hub / "docs").mkdir(exist_ok=True)
        (self.hub / "docs/review-gate-plan.md").write_bytes(spec.read_bytes())
        pointer.atomic_bytes(evidence_root / "spec.md", spec.read_bytes())

        author = {
            "identity": "codex-author",
            "session": "codex-author-session",
            "family": "codex",
            "families": ["codex"],
        }
        authors = {
            "schema": "ck-bootstrap-authors/1",
            "spec_authors": [author],
            "submission_authors": [author],
            "patches": {patch: [0] for patch in self.manifest["hashes"]},
        }
        pointer.atomic_bytes(evidence_root / "authors.json", pointer.canonical(authors))

        session_root = Path(self.temp.name) / "reviewer-logs"
        session_root.mkdir(exist_ok=True)
        session_log = session_root / "claude-reviewer.jsonl"
        session_log.write_text(
            '{"type":"assistant","payload":{"message":{"model":"claude-opus-5-5"},"effort":"high"}}\n'
        )
        self.session_roots = session_root

        spec_subject = {
            "path": "docs/review-gate-plan.md",
            "sha256": pointer.file_digest(self.hub / "docs/review-gate-plan.md"),
        }
        manifest_subject = reviews.subject_for_submission(self.manifest_path, self.manifest)
        validated = json.loads(
            (self.hub / ".pipeline/bootstrap" / bid / "validated.json").read_bytes()
        )
        integration_subject = {
            "bootstrap": bid,
            "validated_sha256": pointer.file_digest(
                self.hub / ".pipeline/bootstrap" / bid / "validated.json"
            ),
            "candidate_fingerprint_sha256": validated["candidate_fingerprint_sha256"],
            "ci_receipt_sha256": validated["ci_receipt_sha256"],
            "membership_manifest_sha256": [validated["manifest_sha256"]],
        }
        reviewer = {
            "identity": "claude-reviewer",
            "client": "claude",
            "session": "claude-reviewer",
            "role": "reviewer",
            "class": "A",
            "family": "claude",
            "model": "claude-opus-5-5",
            "effort": "high",
        }
        if copilot_alias:
            reviewer.update(
                identity="copilot-reviewer",
                client="copilot",
                session="copilot-reviewer-session",
                model="claude-opus-5.5",
            )
            (session_root / "copilot-reviewer-session.jsonl").write_text(
                '{"type":"session.start","payload":{"selectedModel":"claude-opus-5.5","reasoningEffort":"high"}}\n'
                '{"type":"assistant.message","payload":{"model":"claude-opus-5.5"}}\n'
            )
        rows: list[dict[str, str]] = []
        for kind, subject in (
            ("spec", spec_subject),
            ("submission", manifest_subject),
            ("integration", integration_subject),
        ):
            receipt = {
                "schema": "ck-bootstrap-review/1",
                "kind": kind,
                "subject": subject,
                "verdict": "approved",
                "reviewer": reviewer,
                "recorded": "2026-01-01T00:00:00Z",
                "report": {"path": f"{kind}.md", "sha256": "0" * 64},
            }
            name = f"{kind}.json"
            pointer.atomic_bytes(evidence_root / name, pointer.canonical(receipt))
            rows.append({"path": name, "sha256": pointer.file_digest(evidence_root / name)})
        index = {
            "schema": "ck-bootstrap-evidence/1",
            "bootstrap": bid,
            "validated_sha256": pointer.file_digest(
                self.hub / ".pipeline/bootstrap" / bid / "validated.json"
            ),
            "spec": {"path": "spec.md", "sha256": pointer.file_digest(evidence_root / "spec.md")},
            "authors": {
                "path": "authors.json",
                "sha256": pointer.file_digest(evidence_root / "authors.json"),
            },
            "receipts": rows,
        }
        path = evidence_root / "index.json"
        pointer.atomic_bytes(path, pointer.canonical(index))
        return path, hashlib.sha256(path.read_bytes()).hexdigest()

    def test_c24_prepare_installs_hold_and_validates_without_publication(self) -> None:
        helper = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker)
        bid = helper.prepare(
            "s1", __import__("hashlib").sha256(self.manifest_path.read_bytes()).hexdigest()
        )
        current = json.loads((self.hub / ".pipeline/accepted.json").read_bytes())
        self.assertEqual(current["schema"], "bootstrap-hold")
        self.assertEqual(current["bootstrap"], bid)
        self.assertTrue((self.hub / ".pipeline/bootstrap" / bid / "validated.json").exists())
        self.assertEqual((self.hub / ".pipeline/accepted-v2.json").exists(), False)
        frozen = json.loads((self.hub / ".pipeline/bootstrap" / bid / "frozen.json").read_bytes())
        self.assertEqual(frozen["owner"]["pid_start"], procscan.process_start(os.getpid()))
        with self.assertRaises(FileNotFoundError):
            self.legacy.verified_state()

    def test_c24_vendored_commands_and_trampoline_fail_closed_under_hold(self) -> None:
        helper = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker)
        bid = helper.prepare("s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest())
        root = Path(self.temp.name) / "legacy-caller"
        pipeline.clone(self.base, root, self.hub)
        legacy = Path(__file__).parent / "fixtures/legacy"
        for name in ("ck", "pipeline.py", "environment.py"):
            shutil.copy2(legacy / name, root / "tools" / name)
        pipeline.pijul(root, "add", "--recursive", "tools")
        pipeline.pijul(
            root, "record", "-am", "install legacy fixture", "--author", "Bootstrap tests"
        )
        original = (self.hub / ".pipeline/accepted.json").read_bytes()
        queue_before = {
            path.name: path.read_bytes() for path in (self.hub / ".pipeline/queue").glob("*.json")
        }
        commands = [
            ["status"],
            ["submit", "--spec", "hold test"],
            ["accept"],
            ["docs", "-m", "hold docs", "README.md"],
            ["worker", "hold-worker"],
            ["sync"],
        ]
        for args in commands:
            result = subprocess.run(
                [sys.executable, str(root / "tools/ck"), *args],
                cwd=root,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0, args)
            self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), original, args)
            self.assertEqual(
                {
                    path.name: path.read_bytes()
                    for path in (self.hub / ".pipeline/queue").glob("*.json")
                },
                queue_before,
                args,
            )
        shutil.copy2(Path(__file__).resolve().parents[1] / "tools/ck", root / "tools/ck")
        result = subprocess.run(
            [sys.executable, str(root / "tools/ck"), "status"],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f"bootstrap {bid} in progress", result.stderr)
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), original)

    def test_c09_prepare_refuses_legacy_blocked_submission(self) -> None:
        blocked = self.hub / ".pipeline/blocked/s1.json"
        pointer.atomic_bytes(blocked, pointer.canonical(self.manifest))
        with self.assertRaisesRegex(RuntimeError, "legacy-blocked"):
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).prepare(
                "s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest()
            )
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), self.initial_pointer)

    def test_c09_prepare_refuses_ci_that_changes_candidate_fingerprint(self) -> None:
        def mutating_check(candidate: Path, log: Path, ci_fd: int) -> bool:
            (candidate / "source.txt").write_text("CI altered tracked source\n")
            self.record(candidate, "CI altered source")
            log.write_text("reported success\n")
            return True

        with self.assertRaisesRegex(RuntimeError, "CI changed the bootstrap candidate"):
            bootstrap_core.Bootstrap(self.hub, self.capture, mutating_check).prepare(
                "s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest()
            )

    def test_c24_end_to_end_publish_runs_activation_tail_and_status(self) -> None:
        # Give the fixture hub a clean root clone of its accepted base so the
        # activation tail must pull the published candidate into that root.
        control = self.hub / ".pipeline"
        saved_control = Path(self.temp.name) / "saved-control"
        accepted_base = Path(self.legacy.state()["repo"]).resolve()
        saved_base = saved_control.resolve() / accepted_base.relative_to(control.resolve())
        control.rename(saved_control)
        shutil.rmtree(self.hub)
        pipeline.clone(saved_base, self.hub, self.hub)
        (self.hub / "docs").mkdir(exist_ok=True)
        (self.hub / "docs/review-gate-plan.md").write_bytes(SPEC_FIXTURE.read_bytes())
        pipeline.pijul(self.hub, "add", "--recursive", ".")
        pipeline.pijul(self.hub, "record", "-am", "hub fixture docs", "--author", "Bootstrap tests")
        saved_control.rename(self.hub / ".pipeline")
        helper = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker)
        bid = helper.prepare("s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest())
        docs_source = Path(self.temp.name) / "tail-docs"
        pipeline.clone(Path(json.loads(self.initial_pointer)["repo"]), docs_source, self.hub)
        (docs_source / "README.md").write_text("queued bootstrap docs\n")
        docs_id = docsqueue.Queue(self.hub).capture(
            docs_source, "queued bootstrap docs", ["README.md"]
        )
        evidence, digest = self.evidence_for_publish(bid)
        roots = json.dumps({"claude": str(self.session_roots)})
        with mock.patch.dict(os.environ, {"CK_SESSION_ROOTS": roots}):
            helper.publish(bid, evidence, digest)
        state = pointer.read_pointer(self.hub)
        self.assertIn("s1", state["submissions"])
        self.assertEqual(
            json.loads((self.hub / ".pipeline/docs" / f"{docs_id}.json").read_bytes())["state"],
            "published",
        )
        chat_tools = str(Path(__file__).resolve().parents[1] / "tools/chat")
        sys.path.insert(0, chat_tools)
        self.addCleanup(sys.path.remove, chat_tools)
        chat_install = importlib.import_module(
            "install"
        )  # importable only after sys.path is set above

        self.assertEqual(chat_install.read_pointer(self.hub)["schema"], 2)
        self.assertTrue((self.hub / ".pipeline/gate.json").exists())
        store = meta.Store(self.hub)
        self.assertTrue(
            any(
                row.get("kind") == "bootstrap" and row.get("bootstrap") == bid
                for row in store.events()
            )
        )
        docs_hashes = set(
            json.loads((self.hub / ".pipeline/docs" / f"{docs_id}.json").read_bytes())["hashes"]
        )
        self.assertTrue(
            set(state["fingerprint"]["hashes"]) - docs_hashes <= pipeline.hashes(self.hub)
        )

        late = {
            "id": "legacy-after-publication",
            "repo": str(self.capture),
            "hashes": [],
            "spec": "late",
        }
        pointer.atomic_bytes(
            self.hub / ".pipeline/queue/legacy-after-publication.json", pointer.canonical(late)
        )
        output = io.StringIO()
        with redirect_stdout(output):
            gate_cli.run(self.hub, self.hub, ["status"])
        self.assertIn('"status": "legacy-late"', output.getvalue())
        self.assertNotIn(
            "legacy-after-publication", {item["id"] for item in batch.Manager(self.hub).eligible()}
        )
        # The hub is activated (accepted-v2.json exists): a new bootstrap cannot start.
        with self.assertRaisesRegex(RuntimeError, "accepted-v2.json already exists"):
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).prepare(
                "s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest()
            )

    def prepared_with_evidence(
        self, *, hook: Callable[..., None] | None = None
    ) -> tuple[bootstrap_core.Bootstrap, str, Path, str]:
        helper = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker, hook=hook)
        bid = helper.prepare("s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest())
        evidence, digest = self.evidence_for_publish(bid)
        return helper, bid, evidence, digest

    def _held_bid(self, bid: str) -> Path:
        directory = self.hub / ".pipeline/bootstrap" / bid
        state = json.loads(self.initial_pointer)
        digest = hashlib.sha256(self.initial_pointer).hexdigest()
        pointer.atomic_bytes(
            directory / "frozen.json",
            pointer.canonical(
                complete("frozen", {"bootstrap": bid, "base_pointer_sha256": digest})
            ),
        )
        pointer.atomic_bytes(directory / "legacy-pointer.json", self.initial_pointer)
        hold = pointer.tombstone(self.hub, state, bootstrap=bid)
        hold_bytes = pointer.canonical(hold)
        pointer.atomic_bytes(directory / "hold.json", hold_bytes)
        pointer.atomic_bytes(self.hub / ".pipeline/accepted.json", hold_bytes)
        return directory

    def _assert_bootstrap_docs_refuses(
        self, expected_message: str = "legacy tools/ck docs"
    ) -> None:
        source = Path(self.temp.name) / "late-bootstrap-docs"
        pipeline.clone(self.base, source, self.hub)
        (source / "README.md").write_text("must use legacy docs outside the window\n")
        pointer_before = (self.hub / ".pipeline/accepted.json").read_bytes()
        docs_dir = self.hub / ".pipeline/docs"
        entries_before = (
            sorted(path.relative_to(docs_dir).as_posix() for path in docs_dir.rglob("*"))
            if docs_dir.exists()
            else []
        )
        with self.assertRaisesRegex(RuntimeError, expected_message):
            bootstrap_core.execute(
                self.hub,
                self.capture,
                ["docs", "--from", str(source), "-m", "late docs", "README.md"],
            )
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), pointer_before)
        entries_after = (
            sorted(path.relative_to(docs_dir).as_posix() for path in docs_dir.rglob("*"))
            if docs_dir.exists()
            else []
        )
        self.assertEqual(entries_after, entries_before)

    def test_c30_helper_docs_refuses_plain_legacy_hub_without_bootstrap(self) -> None:
        self._assert_bootstrap_docs_refuses()

    def test_c30_helper_docs_refuses_after_cancelled_bid(self) -> None:
        bid = "docs-after-cancel"
        self._held_bid(bid)
        bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).cancel(bid, "test cancel")
        self.assertTrue((self.hub / ".pipeline/bootstrap" / bid / "cancelled.json").exists())
        self._assert_bootstrap_docs_refuses()

    def test_c30_helper_docs_refuses_after_failed_bid_was_restored(self) -> None:
        bid = "docs-after-failed-restore"
        directory = self._held_bid(bid)
        pointer.atomic_bytes(
            directory / "failed.json", pointer.canonical(complete("failed", {"reason": "test"}))
        )
        bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).restore(bid)
        self.assertTrue((directory / "restored.json").exists())
        self._assert_bootstrap_docs_refuses()

    def test_c31_helper_docs_pending_failed_bid_names_cancel(self) -> None:
        bid = "docs-pending-failed"
        directory = self._held_bid(bid)
        pointer.atomic_bytes(
            directory / "failed.json", pointer.canonical(complete("failed", {"reason": "test"}))
        )
        self._assert_bootstrap_docs_refuses(f"run cancel {bid}")

    def test_c31_helper_docs_pending_cancelled_bid_names_cancel(self) -> None:
        bid = "docs-pending-cancelled"
        directory = self._held_bid(bid)
        pointer.atomic_bytes(
            directory / "cancelled.json",
            pointer.canonical(complete("cancelled", {"reason": "test"})),
        )
        self._assert_bootstrap_docs_refuses(f"run cancel {bid}")

    def test_c31_helper_docs_after_restoring_intent_before_pointer_names_cancel(self) -> None:
        bid = "docs-pending-before-restore-pointer"
        self._held_bid(bid)

        def crash_after_intent(point: str, **_: object) -> None:
            if point == "after_restoring":
                raise RuntimeError("intent written")

        with self.assertRaisesRegex(RuntimeError, "intent written"):
            bootstrap_core.Bootstrap(
                self.hub, self.capture, self.checker, hook=crash_after_intent
            ).restore(bid)
        self._assert_bootstrap_docs_refuses(f"run cancel {bid}")

    def test_c31_helper_docs_after_restore_pointer_names_cancel_then_finishes(self) -> None:
        bid = "docs-pending-after-restore-pointer"
        directory = self._held_bid(bid)

        def crash_after_pointer(point: str, **_: object) -> None:
            if point == "after_restore_pointer":
                raise RuntimeError("pointer written")

        with self.assertRaisesRegex(RuntimeError, "pointer written"):
            bootstrap_core.Bootstrap(
                self.hub, self.capture, self.checker, hook=crash_after_pointer
            ).restore(bid)
        restored_pointer = (self.hub / ".pipeline/accepted.json").read_bytes()
        self.assertNotEqual(restored_pointer, (directory / "hold.json").read_bytes())
        self._assert_bootstrap_docs_refuses(f"run cancel {bid}")
        bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).cancel(
            bid, "complete interrupted restore"
        )
        self.assertTrue((directory / "restored.json").exists())
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), self.initial_pointer)

    def test_c29_crash_after_final_tombstone_recovers_without_mutation(self) -> None:
        def crash(point: str, **_: object) -> None:
            if point == "after_final_tombstone":
                raise RuntimeError("injected interruption")

        helper, bid, evidence, digest = self.prepared_with_evidence(hook=crash)
        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with mock.patch.dict(os.environ, env):
            with self.assertRaisesRegex(RuntimeError, "injected interruption"):
                helper.publish(bid, evidence, digest)
            self.assertFalse((self.hub / ".pipeline/accepted-v2.json").exists())
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).publish(
                bid, evidence, digest
            )
        self.assertTrue((self.hub / ".pipeline/accepted-v2.json").exists())
        self.assertTrue((self.hub / ".pipeline/bootstrap" / bid / "published.json").exists())

    def test_c30_tombstone_recovery_ignores_later_reviewer_model_change(self) -> None:
        def crash(point: str, **_: object) -> None:
            if point == "after_final_tombstone":
                raise RuntimeError("injected interruption")

        helper, bid, evidence, digest = self.prepared_with_evidence(hook=crash)
        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with mock.patch.dict(os.environ, env):
            with self.assertRaisesRegex(RuntimeError, "injected interruption"):
                helper.publish(bid, evidence, digest)
            session_log = self.session_roots / "claude-reviewer.jsonl"
            with session_log.open("a") as stream:
                stream.write(
                    '{"type":"assistant","payload":{"message":{"model":"claude-sonnet-5-5"},"effort":"high"}}\n'
                )
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).publish(
                bid, evidence, digest
            )
        self.assertTrue((self.hub / ".pipeline/accepted-v2.json").exists())
        self.assertTrue((self.hub / ".pipeline/bootstrap" / bid / "published.json").exists())

    def test_c29_crash_before_publishing_intent_leaves_hold_and_reruns(self) -> None:
        _helper, bid, evidence, digest = self.prepared_with_evidence()
        before = (self.hub / ".pipeline/accepted.json").read_bytes()

        def crash(point: str, **_: object) -> None:
            if point == "before_publishing":
                raise RuntimeError("before bootstrap intent")

        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with mock.patch.dict(os.environ, env):
            with self.assertRaisesRegex(RuntimeError, "before bootstrap intent"):
                bootstrap_core.Bootstrap(self.hub, self.capture, self.checker, hook=crash).publish(
                    bid, evidence, digest
                )
            self.assertFalse((self.hub / ".pipeline/bootstrap" / bid / "publishing.json").exists())
            self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), before)
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).publish(
                bid, evidence, digest
            )

    def test_c29_mutated_candidate_after_tombstone_is_an_incident(self) -> None:
        def crash(point: str, **_: object) -> None:
            if point == "after_final_tombstone":
                raise RuntimeError("injected interruption")

        helper, bid, evidence, digest = self.prepared_with_evidence(hook=crash)
        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with mock.patch.dict(os.environ, env):
            with self.assertRaisesRegex(RuntimeError, "injected interruption"):
                helper.publish(bid, evidence, digest)
            validated = self.validated_record(bid)
            candidate = Path(validated["candidate"])
            (candidate / "source.txt").write_text("mutated candidate\n")
            self.record(candidate, "mutate candidate")
            with self.assertRaisesRegex(RuntimeError, "candidate changed"):
                bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).publish(
                    bid, evidence, digest
                )
        self.assertFalse((self.hub / ".pipeline/accepted-v2.json").exists())

    def _assert_tombstone_mutation_is_an_incident(
        self, target_name: str, expected_error: str
    ) -> None:
        def crash(point: str, **_: object) -> None:
            if point == "after_final_tombstone":
                raise RuntimeError("injected interruption")

        helper, bid, evidence, digest = self.prepared_with_evidence(hook=crash)
        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with mock.patch.dict(os.environ, env):
            with self.assertRaisesRegex(RuntimeError, "injected interruption"):
                helper.publish(bid, evidence, digest)
            validated = self.validated_record(bid)
            target = (
                Path(validated["ci_receipt"])
                if target_name == "ci receipt"
                else self.hub / ".pipeline/bootstrap" / bid / "evidence/spec.md"
            )
            original = target.read_bytes()
            pointer.atomic_bytes(target, original + b"tampered after tombstone\n")
            with self.assertRaisesRegex(RuntimeError, expected_error):
                bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).publish(
                    bid, evidence, digest
                )
        self.assertFalse((self.hub / ".pipeline/accepted-v2.json").exists())

    def test_c29_mutated_ci_receipt_after_tombstone_is_an_incident(self) -> None:
        self._assert_tombstone_mutation_is_an_incident("ci receipt", "CI receipt changed")

    def test_c29_mutated_evidence_after_tombstone_is_an_incident(self) -> None:
        self._assert_tombstone_mutation_is_an_incident(
            "evidence", "Copied evidence changed: spec.md"
        )

    def test_c29_crash_after_v2_pointer_runs_only_activation_tail(self) -> None:
        def crash(point: str, **_: object) -> None:
            if point == "after_v2_pointer":
                raise RuntimeError("injected interruption")

        helper, bid, evidence, digest = self.prepared_with_evidence(hook=crash)
        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with mock.patch.dict(os.environ, env):
            with self.assertRaisesRegex(RuntimeError, "injected interruption"):
                helper.publish(bid, evidence, digest)
            self.assertTrue((self.hub / ".pipeline/accepted-v2.json").exists())
            pointer.atomic_bytes(
                self.hub / ".pipeline/queue/legacy-late.json",
                pointer.canonical({"id": "legacy-late", "hashes": [], "schema": 1}),
            )
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).publish(
                bid, evidence, digest
            )
        events = [
            row
            for row in meta.Store(self.hub).events()
            if row.get("kind") == "bootstrap" and row.get("bootstrap") == bid
        ]
        self.assertEqual(len(events), 1)
        self.assertTrue((self.hub / ".pipeline/gate.json").exists())

    def test_c29_published_tail_rerun_ignores_late_queue_manifest(self) -> None:
        def crash(point: str, **_: object) -> None:
            if point == "after_gate_file":
                raise RuntimeError("activation tail interrupted")

        helper, bid, evidence, digest = self.prepared_with_evidence(hook=crash)
        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with mock.patch.dict(os.environ, env):
            with self.assertRaisesRegex(RuntimeError, "activation tail interrupted"):
                helper.publish(bid, evidence, digest)
            self.assertTrue((self.hub / ".pipeline/bootstrap" / bid / "published.json").exists())
            pointer.atomic_bytes(
                self.hub / ".pipeline/queue/post-publication.json",
                pointer.canonical({"id": "post-publication", "hashes": [], "schema": 1}),
            )
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).publish(
                bid, Path(self.temp.name) / "unused-evidence.json", "ignored-after-published"
            )
        events = [
            row
            for row in meta.Store(self.hub).events()
            if row.get("kind") == "bootstrap" and row.get("bootstrap") == bid
        ]
        self.assertEqual(len(events), 1)

    def test_c27_publish_refuses_cancelled_bid_after_cancel_restore(self) -> None:
        helper, bid, evidence, digest = self.prepared_with_evidence()
        helper.cancel(bid, "owner cancelled")
        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with (
            mock.patch.dict(os.environ, env),
            self.assertRaisesRegex(RuntimeError, "failed or cancelled"),
        ):
            helper.publish(bid, evidence, digest)

    def test_c27_copilot_receipt_model_alias_matches_session_history(self) -> None:
        helper, bid, _, _ = self.prepared_with_evidence()
        evidence, digest = self.evidence_for_publish(bid, copilot_alias=True)
        env = {"CK_SESSION_ROOTS": json.dumps({"copilot": str(self.session_roots)})}
        with mock.patch.dict(os.environ, env):
            helper.publish(bid, evidence, digest)
        self.assertTrue((self.hub / ".pipeline/accepted-v2.json").exists())

    def test_c27_authors_ledger_must_map_exact_submission_patches(self) -> None:
        helper, bid, evidence, _digest = self.prepared_with_evidence()
        root = self.hub / ".pipeline/coordination/gate-bootstrap"
        authors_path = root / "authors.json"
        authors = json.loads(authors_path.read_bytes())
        first_hash = next(iter(authors["patches"]))
        index = json.loads(evidence.read_bytes())
        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with mock.patch.dict(os.environ, env):
            del authors["patches"][first_hash]
            pointer.atomic_bytes(authors_path, pointer.canonical(authors))
            index["authors"]["sha256"] = pointer.file_digest(authors_path)
            pointer.atomic_bytes(evidence, pointer.canonical(index))
            with self.assertRaisesRegex(RuntimeError, "map every submission patch"):
                helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())

            authors["patches"][first_hash] = [0]
            authors["patches"]["NOT-IN-MANIFEST"] = [0]
            pointer.atomic_bytes(authors_path, pointer.canonical(authors))
            index["authors"]["sha256"] = pointer.file_digest(authors_path)
            pointer.atomic_bytes(evidence, pointer.canonical(index))
            with self.assertRaisesRegex(RuntimeError, "map every submission patch"):
                helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())

            del authors["patches"]["NOT-IN-MANIFEST"]
            authors["patches"][first_hash] = [1]
            pointer.atomic_bytes(authors_path, pointer.canonical(authors))
            index["authors"]["sha256"] = pointer.file_digest(authors_path)
            pointer.atomic_bytes(evidence, pointer.canonical(index))
            with self.assertRaisesRegex(RuntimeError, "invalid author index"):
                helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())

    def test_c27_publish_rejects_unsafe_evidence_index_paths(self) -> None:
        helper, bid, evidence, _ = self.prepared_with_evidence()
        original = json.loads(evidence.read_bytes())
        hold_bytes = (self.hub / ".pipeline/accepted.json").read_bytes()
        for unsafe_path in ("../outside.md", "/tmp/outside.md", "index.json"):
            with self.subTest(path=unsafe_path):
                index = json.loads(json.dumps(original))
                index["spec"]["path"] = unsafe_path
                pointer.atomic_bytes(evidence, pointer.canonical(index))
                with self.assertRaisesRegex(RuntimeError, "Unsafe bootstrap evidence path"):
                    helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())
                self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), hold_bytes)

    def test_c07_spec_receipt_is_bound_to_candidate_copy(self) -> None:
        helper, bid, evidence, _ = self.prepared_with_evidence()
        root_spec = self.hub / "docs/review-gate-plan.md"
        root_spec.write_bytes(b"the mutable hub root has a different spec\n")
        index = json.loads(evidence.read_bytes())
        evidence_root = self.hub / ".pipeline/coordination/gate-bootstrap"
        authors = json.loads((evidence_root / index["authors"]["path"]).read_bytes())
        receipts = [
            json.loads((evidence_root / row["path"]).read_bytes()) for row in index["receipts"]
        ]
        validated = json.loads(
            (self.hub / ".pipeline/bootstrap" / bid / "validated.json").read_bytes()
        )
        with mock.patch.dict(
            os.environ, {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        ):
            helper._verify_receipts(bid, validated, authors, receipts, index, self.manifest)

    def test_c08_publish_checks_ci_receipt_passed_flag(self) -> None:
        helper, bid, evidence, _ = self.prepared_with_evidence()
        directory = self.hub / ".pipeline/bootstrap" / bid
        validated_path = directory / "validated.json"
        validated = json.loads(validated_path.read_bytes())
        ci_path = Path(validated["ci_receipt"])
        ci = json.loads(ci_path.read_bytes())
        ci["passed"] = False
        pointer.atomic_bytes(ci_path, pointer.canonical(ci))
        validated["ci_receipt_sha256"] = pointer.file_digest(ci_path)
        pointer.atomic_bytes(validated_path, pointer.canonical(validated))

        evidence_root = self.hub / ".pipeline/coordination/gate-bootstrap"
        index = json.loads(evidence.read_bytes())
        integration_path = evidence_root / "integration.json"
        integration = json.loads(integration_path.read_bytes())
        integration["subject"].update(
            validated_sha256=pointer.file_digest(validated_path),
            ci_receipt_sha256=validated["ci_receipt_sha256"],
        )
        pointer.atomic_bytes(integration_path, pointer.canonical(integration))
        for row in index["receipts"]:
            if row["path"] == "integration.json":
                row["sha256"] = pointer.file_digest(integration_path)
        index["validated_sha256"] = pointer.file_digest(validated_path)
        pointer.atomic_bytes(evidence, pointer.canonical(index))
        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with (
            mock.patch.dict(os.environ, env),
            self.assertRaisesRegex(RuntimeError, "CI receipt did not pass"),
        ):
            helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())

    def test_c07b_the_validated_record_reader_refuses_a_malformed_fingerprint(self) -> None:
        # The helper the tests read `validated.json` through is the checked loader: a fingerprint
        # whose hashes are not a list of strings is refused instead of being handed on as typed.
        _helper, bid, _evidence, _digest = self.prepared_with_evidence()
        path = self.hub / ".pipeline/bootstrap" / bid / "validated.json"
        original = path.read_bytes()
        self.assertEqual(self.validated_record(bid)["bootstrap"], bid)
        record = json.loads(original)
        for damage in ("wrong", ["ok", 3], None):
            with self.subTest(damage=damage):
                record["candidate_fingerprint"]["hashes"] = damage
                path.write_text(json.dumps(record))
                with self.assertRaises(RecordError):
                    self.validated_record(bid)
        record["candidate_fingerprint"] = {"hashes": [], "files": []}
        path.write_text(json.dumps(record))
        with self.assertRaises(RecordError):
            self.validated_record(bid)
        path.write_bytes(original)

    def test_c08_publish_checks_candidate_hashes_against_base_and_submission(self) -> None:
        helper, bid, evidence, digest = self.prepared_with_evidence()
        validated = self.validated_record(bid)
        candidate = Path(validated["candidate"])
        real_hashes = pipeline.hashes
        real_fingerprint = pipeline.fingerprint

        def extra_hash(path: Path) -> Collection[str]:
            hashes = real_hashes(path)
            return [*hashes, "unexpected-patch"] if Path(path) == candidate else hashes

        def original_fingerprint(path: Path) -> Fingerprint:
            if Path(path) == candidate:
                return validated["candidate_fingerprint"]  # decoded by the loader, shape checked
            return real_fingerprint(path)

        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with (
            mock.patch.dict(os.environ, env),
            mock.patch.object(pipeline, "hashes", side_effect=extra_hash),
            mock.patch.object(pipeline, "fingerprint", side_effect=original_fingerprint),
            self.assertRaisesRegex(RuntimeError, "candidate hashes differ"),
        ):
            helper.publish(bid, evidence, digest)

    def test_c08_publish_checks_candidate_helper_bytes(self) -> None:
        helper, bid, evidence, digest = self.prepared_with_evidence()
        candidate_helper = (
            Path(
                json.loads(
                    (self.hub / ".pipeline/bootstrap" / bid / "validated.json").read_bytes()
                )["candidate"]
            )
            / "tools/gate/bootstrap.py"
        )
        real_digest = pointer.file_digest

        def altered_digest(path: Path) -> str:
            if Path(path) == candidate_helper:
                return "0" * 64
            return real_digest(path)

        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with (
            mock.patch.dict(os.environ, env),
            mock.patch.object(bootstrap_core, "file_digest", side_effect=altered_digest),
            self.assertRaisesRegex(RuntimeError, "differs from candidate helper"),
        ):
            helper.publish(bid, evidence, digest)

    def test_c27_publish_refuses_after_cancel_record_before_restore(self) -> None:
        def crash(point: str, **_: object) -> None:
            if point == "after_cancelled":
                raise RuntimeError("cancel interrupted before restore")

        helper, bid, evidence, digest = self.prepared_with_evidence()
        with self.assertRaisesRegex(RuntimeError, "cancel interrupted"):
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker, hook=crash).cancel(
                bid, "owner cancelled"
            )
        self.assertTrue((self.hub / ".pipeline/bootstrap" / bid / "cancelled.json").exists())
        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with (
            mock.patch.dict(os.environ, env),
            self.assertRaisesRegex(RuntimeError, "failed or cancelled"),
        ):
            helper.publish(bid, evidence, digest)

    def test_c29_two_tail_docs_crash_resumes_on_the_published_descendant(self) -> None:
        _helper, bid, evidence, digest = self.prepared_with_evidence()
        base = Path(json.loads(self.initial_pointer)["repo"])
        first_source = Path(self.temp.name) / "tail-docs-one"
        pipeline.clone(base, first_source, self.hub)
        (first_source / "README.md").write_text("bootstrap tail docs one\n")
        first = docsqueue.Queue(self.hub).capture(first_source, "tail one", ["README.md"])
        second_source = Path(self.temp.name) / "tail-docs-two"
        pipeline.clone(base, second_source, self.hub)
        (second_source / "docs").mkdir(exist_ok=True)
        (second_source / "docs/tail.md").write_text("bootstrap tail docs two\n")
        second = docsqueue.Queue(self.hub).capture(second_source, "tail two", ["docs/tail.md"])

        def crash(point: str, *, entry: DocsEntry | None = None, **_: object) -> None:
            if point == "after_pointer" and entry and entry["id"] == first:
                raise RuntimeError("tail docs interruption")

        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with mock.patch.dict(os.environ, env):
            with self.assertRaisesRegex(RuntimeError, "tail docs interruption"):
                bootstrap_core.Bootstrap(self.hub, self.capture, self.checker, hook=crash).publish(
                    bid, evidence, digest
                )
            base_v2 = json.loads(
                (self.hub / ".pipeline/bootstrap" / bid / "publishing.json").read_bytes()
            )["v2"].encode()
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).publish(
                bid, evidence, digest
            )
        current = (self.hub / ".pipeline/accepted-v2.json").read_bytes()
        self.assertTrue(docsqueue.is_docs_descendant(self.hub, current, base_v2))
        self.assertEqual(
            [
                json.loads((self.hub / ".pipeline/docs" / f"{item}.json").read_bytes())["state"]
                for item in (first, second)
            ],
            ["published", "published"],
        )

    def test_c28b_restore_pending_rejects_docs_until_cancel_then_legacy_docs_works(self) -> None:
        bid = "restore-pending-docs"
        directory = self.hub / ".pipeline/bootstrap" / bid
        digest = hashlib.sha256(self.initial_pointer).hexdigest()
        state = json.loads(self.initial_pointer)
        pointer.atomic_bytes(
            directory / "frozen.json",
            pointer.canonical(
                complete("frozen", {"bootstrap": bid, "base_pointer_sha256": digest})
            ),
        )
        pointer.atomic_bytes(directory / "legacy-pointer.json", self.initial_pointer)
        hold = pointer.tombstone(self.hub, state, bootstrap=bid)
        pointer.atomic_bytes(directory / "hold.json", pointer.canonical(hold))
        pointer.atomic_bytes(self.hub / ".pipeline/accepted.json", pointer.canonical(hold))
        pointer.atomic_bytes(
            directory / "failed.json",
            pointer.canonical(complete("failed", {"reason": "fixture failure"})),
        )
        source = Path(self.temp.name) / "restore-pending-source"
        pipeline.clone(Path(state["repo"]), source, self.hub)
        (source / "README.md").write_text("deferred under failure\n")
        with self.assertRaisesRegex(RuntimeError, "run cancel"):
            docsqueue.Queue(self.hub).capture(source, "must wait", ["README.md"])
        bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).cancel(bid, "restore docs")
        legacy_source = Path(self.temp.name) / "legacy-docs-after-restore"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), legacy_source, self.hub)
        (legacy_source / "README.md").write_text("legacy docs after restore\n")
        self.legacy.docs(legacy_source, ["README.md"], "Legacy docs after restore")
        self.assertIn("README.md", pointer.read_pointer(self.hub)["fingerprint"]["files"])

    def test_c28b_restoring_crash_refuses_capture_then_cancel_drains_docs(self) -> None:
        bid = "restore-intent-capture"
        directory = self.hub / ".pipeline/bootstrap" / bid
        digest = hashlib.sha256(self.initial_pointer).hexdigest()
        state = json.loads(self.initial_pointer)
        pointer.atomic_bytes(
            directory / "frozen.json",
            pointer.canonical(
                complete("frozen", {"bootstrap": bid, "base_pointer_sha256": digest})
            ),
        )
        pointer.atomic_bytes(directory / "legacy-pointer.json", self.initial_pointer)
        hold = pointer.tombstone(self.hub, state, bootstrap=bid)
        pointer.atomic_bytes(directory / "hold.json", pointer.canonical(hold))
        pointer.atomic_bytes(self.hub / ".pipeline/accepted.json", pointer.canonical(hold))
        source = Path(self.temp.name) / "restore-intent-queued-docs"
        pipeline.clone(Path(state["repo"]), source, self.hub)
        (source / "README.md").write_text("drain after restoring crash\n")
        did = docsqueue.Queue(self.hub).capture(source, "queued before restore", ["README.md"])
        pointer.atomic_bytes(
            directory / "failed.json", pointer.canonical(complete("failed", {"reason": "cancel"}))
        )

        def crash(point: str, **_: object) -> None:
            if point == "after_restore_pointer":
                raise RuntimeError("restored pointer before acknowledgement")

        with self.assertRaisesRegex(RuntimeError, "before acknowledgement"):
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker, hook=crash).restore(bid)
        self.assertTrue((directory / "restoring.json").exists())
        with self.assertRaisesRegex(RuntimeError, "run cancel"):
            docsqueue.Queue(self.hub).capture(source, "must wait", ["README.md"])

        bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).cancel(
            bid, "resume interrupted restoration"
        )
        current = pointer.read_pointer(self.hub)
        self.assertIn("README.md", current["fingerprint"]["files"])
        self.assertEqual(
            json.loads((self.hub / ".pipeline/docs" / f"{did}.json").read_bytes())["state"],
            "published",
        )

    def _held_bootstrap_with_two_docs(self) -> tuple[str, list[str]]:
        bid = "restore-two-docs"
        directory = self.hub / ".pipeline/bootstrap" / bid
        state = json.loads(self.initial_pointer)
        digest = hashlib.sha256(self.initial_pointer).hexdigest()
        pointer.atomic_bytes(
            directory / "frozen.json",
            pointer.canonical(
                complete("frozen", {"bootstrap": bid, "base_pointer_sha256": digest})
            ),
        )
        pointer.atomic_bytes(directory / "legacy-pointer.json", self.initial_pointer)
        hold = pointer.tombstone(self.hub, state, bootstrap=bid)
        pointer.atomic_bytes(directory / "hold.json", pointer.canonical(hold))
        pointer.atomic_bytes(self.hub / ".pipeline/accepted.json", pointer.canonical(hold))
        entries: list[str] = []
        for name, content in (
            ("README.md", "restored documentation one\n"),
            ("docs/restore-two.md", "restored documentation two\n"),
        ):
            source = Path(self.temp.name) / f"restore-source-{len(entries)}"
            pipeline.clone(Path(state["repo"]), source, self.hub)
            target = source / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content)
            entries.append(docsqueue.Queue(self.hub).capture(source, "restore docs", [name]))
        return bid, entries

    def _crash_cancel_after_first_restored_doc(self, bid: str, entries: list[str]) -> None:
        crashed = False
        hold_bytes = (self.hub / ".pipeline/bootstrap" / bid / "hold.json").read_bytes()

        def crash(point: str, *, entry: DocsEntry | None = None, **_: object) -> None:
            nonlocal crashed
            if (
                point == "after_restore_link"
                and entry is not None
                and entry["id"] == entries[0]
                and not crashed
            ):
                crashed = True
                self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), hold_bytes)
                raise RuntimeError("restore docs chain interruption")

        with self.assertRaisesRegex(RuntimeError, "restore docs chain interruption"):
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker, hook=crash).cancel(
                bid, "cancel with queued docs"
            )
        self.assertFalse((self.hub / ".pipeline/bootstrap" / bid / "restoring.json").exists())
        self.assertFalse((self.hub / ".pipeline/bootstrap" / bid / "restored.json").exists())
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), hold_bytes)

    def _assert_restore_docs_published(self, entries: list[str]) -> None:
        self.assertEqual(
            [
                json.loads((self.hub / ".pipeline/docs" / f"{did}.json").read_bytes())["state"]
                for did in entries
            ],
            ["published", "published"],
        )

    def test_c28b_cancel_recovers_two_docs_after_first_restore_link_crash(self) -> None:
        bid, entries = self._held_bootstrap_with_two_docs()
        self._crash_cancel_after_first_restored_doc(bid, entries)
        hold = (self.hub / ".pipeline/bootstrap" / bid / "hold.json").read_bytes()
        points = []

        def observe(point: str, **_: object) -> None:
            if point == "after_restore_link":
                points.append(point)
                self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), hold)
            elif point == "after_restore_pointer":
                points.append(point)
                self.assertNotEqual((self.hub / ".pipeline/accepted.json").read_bytes(), hold)

        bootstrap_core.Bootstrap(self.hub, self.capture, self.checker, hook=observe).cancel(
            bid, "retry interrupted cancellation"
        )
        self.assertEqual(points.count("after_restore_pointer"), 1)
        self.assertGreaterEqual(points.count("after_restore_link"), 2)
        self._assert_restore_docs_published(entries)
        state = pointer.read_pointer(self.hub)
        self.assertIn("README.md", state["fingerprint"]["files"])
        self.assertIn("docs/restore-two.md", state["fingerprint"]["files"])

    def test_c28b_restore_intent_verifies_recorded_chain_and_live_queue(self) -> None:
        bid, entries = self._held_bootstrap_with_two_docs()
        directory = self.hub / ".pipeline/bootstrap" / bid

        def crash(point: str, **_: object) -> None:
            if point == "after_restoring":
                raise RuntimeError("crash after restore intent")

        with self.assertRaisesRegex(RuntimeError, "crash after restore intent"):
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker, hook=crash).cancel(
                bid, "test restore intent"
            )
        hold = (directory / "hold.json").read_bytes()
        restoring = json.loads((directory / "restoring.json").read_bytes())
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), hold)
        self.assertEqual([row["id"] for row in restoring["entries"]], entries)
        self.assertEqual(len(restoring["links"]), 2)
        extra = {
            "id": "late-live-docs",
            "state": "pending",
            "repo": str(Path(json.loads(self.initial_pointer)["repo"])),
            "hashes": [],
            "files": ["README.md"],
            "created": 1,
            "message": "late live docs",
            "fingerprint": {"hashes": [], "files": {}},
        }
        pointer.atomic_bytes(
            self.hub / ".pipeline/docs/late-live-docs.json", pointer.canonical(extra)
        )
        with self.assertRaisesRegex(RuntimeError, "omits or mismatches a live docs entry"):
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).cancel(
                bid, "resume restore"
            )
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), hold)

    def test_c28b_next_prepare_recovers_two_docs_after_first_restore_link_crash(self) -> None:
        bid, entries = self._held_bootstrap_with_two_docs()
        self._crash_cancel_after_first_restored_doc(bid, entries)

        next_bid = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).prepare(
            "s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest()
        )
        self._assert_restore_docs_published(entries)
        restored_base = json.loads(
            (self.hub / ".pipeline/bootstrap" / next_bid / "legacy-pointer.json").read_bytes()
        )
        self.assertIn("README.md", restored_base["fingerprint"]["files"])
        self.assertIn("docs/restore-two.md", restored_base["fingerprint"]["files"])

    def test_c25_process_scan_refuses_prepare_and_publish_writers(self) -> None:
        own: ProcessRow = {"pid": os.getpid(), "ppid": 1, "command": "python bootstrap test"}
        external: ProcessRow = {
            "pid": os.getpid() + 10000,
            "ppid": 1,
            "command": "/fixture/tools/ck status",
        }

        def busy() -> list[ProcessRow]:
            return [own, external]

        def clean() -> list[ProcessRow]:
            return [own]

        helper = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker, process_lister=busy)
        with self.assertRaisesRegex(RuntimeError, "State writers are still running"):
            helper.prepare("s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest())
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), self.initial_pointer)
        helper = bootstrap_core.Bootstrap(
            self.hub, self.capture, self.checker, process_lister=clean
        )
        bid = helper.prepare("s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest())
        evidence, digest = self.evidence_for_publish(bid)
        external["command"] = f"python {self.hub}/tools/pipeline.py submit"
        helper.process_lister = busy
        with self.assertRaisesRegex(RuntimeError, "State writers are still running"):
            helper.publish(bid, evidence, digest)
        self.assertFalse((self.hub / ".pipeline/accepted-v2.json").exists())

    @procfix.real_process
    def test_rc3_03_real_ck_helpers_are_matched_by_their_subcommand(self) -> None:
        script = Path(self.temp.name) / "bin" / "ck"
        script.parent.mkdir()
        script.write_text("#!/bin/sh\nread _\n")
        script.chmod(0o755)

        def start(*words: str) -> tuple[subprocess.Popen[bytes], str]:
            helper = subprocess.Popen([str(script), *words], stdin=subprocess.PIPE)

            def stop() -> None:
                helper.kill()
                helper.wait()
                assert helper.stdin is not None
                helper.stdin.close()

            self.addCleanup(stop)
            deadline = time.time() + 10
            suffix = " ".join(words)
            while time.time() < deadline:
                rows = {row["pid"]: row for row in procfix.REAL_PROCESSES()}
                found = rows.get(helper.pid)
                command = found["command"] if found else ""
                if str(script) in command and command.endswith(suffix):
                    return helper, command
                time.sleep(0.05)
            self.fail(f"helper {words} never appeared in the process table")
            return None

        session, session_command = start("session", "--role", "worker")
        worker, worker_command = start("worker", "scratch")
        self.assertTrue(session_command.endswith(f"{script} session --role worker"))
        self.assertTrue(worker_command.endswith(f"{script} worker scratch"))
        own = procscan.ancestors(lister=procfix.REAL_PROCESSES)

        def lister(*helpers: subprocess.Popen[bytes]) -> Callable[[], list[ProcessRow]]:
            wanted = own | {helper.pid for helper in helpers}
            return lambda: [row for row in procfix.REAL_PROCESSES() if row["pid"] in wanted]

        digest = hashlib.sha256(self.manifest_path.read_bytes()).hexdigest()
        busy = bootstrap_core.Bootstrap(
            self.hub, self.capture, self.checker, process_lister=lister(session, worker)
        )
        with self.assertRaisesRegex(RuntimeError, "State writers are still running") as raised:
            busy.prepare("s1", digest)
        self.assertIn(f"pid {worker.pid}:", str(raised.exception))
        self.assertNotIn(f"pid {session.pid}:", str(raised.exception))
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), self.initial_pointer)
        # The read-only readiness check does not block the path.
        quiet = bootstrap_core.Bootstrap(
            self.hub, self.capture, self.checker, process_lister=lister(session)
        )
        self.assertTrue(quiet.prepare("s1", digest))

    def test_c26_paused_vendored_submit_after_freeze_blocks_publish(self) -> None:
        helper = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker)
        caller = Path(self.temp.name) / "paused-legacy-submitter"
        pipeline.clone(Path(self.legacy.state()["repo"]), caller, self.hub)
        (caller / "late.txt").write_text("recorded before the submit pauses\n")
        pipeline.pijul(caller, "add", "late.txt")
        pipeline.pijul(
            caller, "record", "-am", "late legacy submission", "--author", "Bootstrap tests"
        )

        fixture = Path(__file__).parent / "fixtures/legacy/pipeline.py"
        spec = importlib.util.spec_from_file_location("vendored_legacy_pipeline", fixture)
        assert spec is not None
        vendored = importlib.util.module_from_spec(spec)
        assert spec.loader is not None
        spec.loader.exec_module(vendored)
        legacy = vendored.Pipeline(self.hub, lambda repo, log: True)
        read_pointer = threading.Event()
        resume_submit = threading.Event()
        original_verified_state = legacy.verified_state

        def pause_after_pointer_read() -> object:
            state = original_verified_state()
            read_pointer.set()
            if not resume_submit.wait(timeout=10):
                raise TimeoutError("test did not resume the paused vendored submit")
            return state

        legacy.verified_state = pause_after_pointer_read
        outcome: dict[str, object] = {}

        def submit() -> None:
            try:
                outcome["id"] = legacy.submit(caller, "paused legacy submission")
            except BaseException as error:
                outcome["error"] = error

        worker = threading.Thread(target=submit)
        worker.start()
        try:
            self.assertTrue(
                read_pointer.wait(timeout=10), "vendored submit did not read the pointer"
            )
            bid = helper.prepare("s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest())
        finally:
            resume_submit.set()
            worker.join(timeout=10)
        self.assertFalse(worker.is_alive(), "vendored submit did not finish")
        self.assertNotIn("error", outcome, outcome.get("error"))
        self.assertTrue((self.hub / ".pipeline/queue" / f"{outcome['id']}.json").exists())

        evidence, digest = self.evidence_for_publish(bid)
        with (
            mock.patch.dict(
                os.environ, {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
            ),
            self.assertRaisesRegex(RuntimeError, "Queue changed after bootstrap freeze"),
        ):
            helper.publish(bid, evidence, digest)

    def test_c27_publish_refuses_digest_hold_subject_and_reviewer_failures(self) -> None:  # noqa: PLR0915  # long scenario test: the statements are the scenario
        helper, bid, evidence, _digest = self.prepared_with_evidence()
        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        evidence_root = self.hub / ".pipeline/coordination/gate-bootstrap"
        index = json.loads(evidence.read_bytes())
        with mock.patch.dict(os.environ, env):
            with self.assertRaisesRegex(RuntimeError, "Evidence index digest mismatch"):
                helper.publish(bid, evidence, "0" * 64)

            spec_file = evidence_root / "spec.md"
            original_spec = spec_file.read_bytes()
            pointer.atomic_bytes(spec_file, original_spec + b"changed\n")
            with self.assertRaisesRegex(RuntimeError, "Evidence changed: spec.md"):
                helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())
            pointer.atomic_bytes(spec_file, original_spec)

            original_receipt = (evidence_root / "integration.json").read_bytes()
            index["receipts"] = [
                row for row in index["receipts"] if row["path"] != "integration.json"
            ]
            pointer.atomic_bytes(evidence, pointer.canonical(index))
            with self.assertRaisesRegex(
                RuntimeError, "Insufficient opposite-family coverage for integration"
            ):
                helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())

            index["receipts"].append(
                {"path": "integration.json", "sha256": hashlib.sha256(original_receipt).hexdigest()}
            )
            receipt = json.loads(original_receipt)
            receipt["subject"]["bootstrap"] = "stale-bootstrap"
            stale_raw = pointer.canonical(receipt)
            pointer.atomic_bytes(evidence_root / "integration.json", stale_raw)
            index["receipts"][-1]["sha256"] = hashlib.sha256(stale_raw).hexdigest()
            pointer.atomic_bytes(evidence, pointer.canonical(index))
            with self.assertRaisesRegex(
                RuntimeError, "Insufficient opposite-family coverage for integration"
            ):
                helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())

            receipt["subject"]["bootstrap"] = bid
            receipt["reviewer"]["class"] = "B"
            bad_class = pointer.canonical(receipt)
            pointer.atomic_bytes(evidence_root / "integration.json", bad_class)
            index["receipts"][-1]["sha256"] = hashlib.sha256(bad_class).hexdigest()
            pointer.atomic_bytes(evidence, pointer.canonical(index))
            with self.assertRaisesRegex(RuntimeError, "does not match Class A/high"):
                helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())

            def replace_integration_receipt(updated: Mapping[str, object]) -> str:
                raw = pointer.canonical(updated)
                pointer.atomic_bytes(evidence_root / "integration.json", raw)
                row = next(row for row in index["receipts"] if row["path"] == "integration.json")
                row["sha256"] = hashlib.sha256(raw).hexdigest()
                pointer.atomic_bytes(evidence, pointer.canonical(index))
                return hashlib.sha256(evidence.read_bytes()).hexdigest()

            receipt["reviewer"]["class"] = "A"
            receipt["reviewer"]["effort"] = "medium"
            with self.assertRaisesRegex(RuntimeError, "does not match Class A/high"):
                helper.publish(bid, evidence, replace_integration_receipt(receipt))

            receipt["reviewer"]["effort"] = "high"
            receipt["reviewer"]["identity"] = "codex-author"
            receipt["reviewer"]["session"] = "codex-author-session"
            with self.assertRaisesRegex(RuntimeError, "authored this subject"):
                helper.publish(bid, evidence, replace_integration_receipt(receipt))

            receipt["reviewer"]["identity"] = "claude-reviewer"
            receipt["reviewer"]["session"] = "missing-reviewer-log"
            with self.assertRaisesRegex(RuntimeError, "missing or ambiguous"):
                helper.publish(bid, evidence, replace_integration_receipt(receipt))

            receipt["reviewer"]["session"] = "claude-reviewer"
            receipt["reviewer"]["model"] = "claude-sonnet-5-5"
            with self.assertRaisesRegex(RuntimeError, "session evidence does not match"):
                helper.publish(bid, evidence, replace_integration_receipt(receipt))

            codex_root = Path(self.temp.name) / "codex-reviewer-logs"
            codex_root.mkdir(exist_ok=True)
            (codex_root / "codex-reviewer.jsonl").write_text(
                '{"type":"turn_context","payload":{"model":"gpt-6.1-sol","effort":"high"}}\n'
            )
            receipt["reviewer"].update(
                client="codex", family="codex", model="gpt-6.1-sol", session="codex-reviewer"
            )
            roots = json.dumps({"claude": str(self.session_roots), "codex": str(codex_root)})
            with (
                mock.patch.dict(os.environ, {"CK_SESSION_ROOTS": roots}),
                self.assertRaisesRegex(
                    RuntimeError, "Insufficient opposite-family coverage for integration"
                ),
            ):
                helper.publish(bid, evidence, replace_integration_receipt(receipt))

            validated = self.validated_record(bid)
            ci_path = Path(validated["ci_receipt"])
            ci_bytes = ci_path.read_bytes()
            pointer.atomic_bytes(ci_path, ci_bytes + b" ")
            with self.assertRaisesRegex(RuntimeError, "CI receipt changed"):
                helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())
            pointer.atomic_bytes(ci_path, ci_bytes)

            saved_path = self.hub / ".pipeline/bootstrap" / bid / "legacy-pointer.json"
            saved_bytes = saved_path.read_bytes()
            pointer.atomic_bytes(saved_path, saved_bytes + b" ")
            with self.assertRaisesRegex(RuntimeError, "Saved base pointer changed"):
                helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())
            pointer.atomic_bytes(saved_path, saved_bytes)

            manifest_bytes = self.manifest_path.read_bytes()
            pointer.atomic_bytes(self.manifest_path, manifest_bytes + b" ")
            with self.assertRaisesRegex(RuntimeError, "submission manifest changed"):
                helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())
            pointer.atomic_bytes(self.manifest_path, manifest_bytes)

            (self.capture / "source.txt").write_text("capture mutation\n")
            self.record(self.capture, "mutate bootstrap capture")
            with self.assertRaisesRegex(RuntimeError, "submission capture changed"):
                helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())

            hold = (self.hub / ".pipeline/accepted.json").read_bytes()
            pointer.atomic_bytes(
                self.hub / ".pipeline/accepted.json",
                pointer.canonical({"schema": "bootstrap-hold", "bootstrap": "tampered"}),
            )
            with self.assertRaisesRegex(RuntimeError, "hold tombstone changed"):
                helper.publish(bid, evidence, hashlib.sha256(evidence.read_bytes()).hexdigest())
            pointer.atomic_bytes(self.hub / ".pipeline/accepted.json", hold)

    def test_c30_trampoline_uses_checkpoint_and_never_imports_modified_local_modules(self) -> None:  # noqa: PLR0915  # long scenario test: the statements are the scenario
        root = Path(self.temp.name) / "caller"
        tools = root / "tools"
        tools.mkdir(parents=True)
        shutil.copy2(
            Path(__file__).resolve().parents[1] / "rust-toolchain.toml",
            root / "rust-toolchain.toml",
        )
        marker = Path(self.temp.name) / "local-imported"
        shutil.copy2(Path(__file__).resolve().parents[1] / "tools/ck", tools / "ck")
        (tools / "pipeline.py").write_text(
            f"from pathlib import Path\nPath({str(marker)!r}).write_text('bad')\n"
        )
        (tools / "environment.py").write_text(
            f"from pathlib import Path\nPath({str(marker)!r}).write_text('bad')\n"
        )
        (root / ".ck-hub").write_text(str(self.hub) + "\n")

        checkpoint = Path(self.temp.name) / "checkpoint"
        checkpoint.mkdir()
        pipeline.pijul(checkpoint, "init")
        (checkpoint / ".ignore").write_text("/.pipeline\n/.ck-hub\n")
        accepted_tools = Path(__file__).resolve().parents[1] / "tools"
        (checkpoint / "tools").mkdir()
        shutil.copy2(accepted_tools / "pipeline.py", checkpoint / "tools/pipeline.py")
        shutil.copy2(accepted_tools / "environment.py", checkpoint / "tools/environment.py")
        shutil.copytree(accepted_tools / "gate", checkpoint / "tools/gate")
        shutil.copy2(accepted_tools / "check.py", checkpoint / "tools/check.py")
        pipeline.pijul(checkpoint, "add", "--recursive", ".")
        pipeline.pijul(
            checkpoint, "record", "-am", "checkpoint controller", "--author", "Bootstrap tests"
        )
        state = {
            "schema": 2,
            "repo": str(checkpoint),
            "fingerprint": pipeline.fingerprint(checkpoint),
            "submissions": ["accepted"],
        }
        control = self.hub / ".pipeline"
        control.mkdir(exist_ok=True)
        pointer.atomic_bytes(control / "accepted-v2.json", pointer.canonical(state))
        pointer.atomic_bytes(control / "accepted.json", pointer.canonical({"schema": "tombstone"}))
        (control / "queue").mkdir(exist_ok=True)
        pointer.atomic_bytes(
            control / "queue/legacy-late.json",
            pointer.canonical({"id": "legacy-late", "repo": str(checkpoint), "spec": "late"}),
        )
        result = subprocess.run(
            [sys.executable, "-B", str(tools / "ck"), "status"],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('"accepted": [\n    "accepted"', result.stdout)
        self.assertIn('"status": "legacy-late"', result.stdout)
        self.assertFalse(marker.exists())

        worker = subprocess.run(
            [sys.executable, "-B", str(tools / "ck"), "worker", "fresh-worker"],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(worker.returncode, 0, worker.stderr)
        worker_root = self.hub / ".pipeline/workers/fresh-worker"
        self.assertTrue(worker_root.is_dir())
        self.assertEqual((worker_root / ".ck-hub").read_text(), str(self.hub.resolve()) + "\n")
        self.assertEqual(pipeline.fingerprint(worker_root), state["fingerprint"])
        self.assertFalse(marker.exists())

        bin_directory = self.hub / ".tools/python/bin"
        bin_directory.mkdir(parents=True, exist_ok=True)
        configured_hub = Path(os.environ.get("CK_HUB", Path.home() / "work/codekarl"))
        # Every pinned tool the doctor checks (S5 added ShellCheck and mypy to Ruff).
        for tool in ("ruff", "shellcheck", "mypy"):
            (bin_directory / tool).symlink_to(configured_hub / ".tools/python/bin" / tool)
        doctor = subprocess.run(
            [sys.executable, "-B", str(tools / "ck"), "doctor"],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(doctor.returncode, 0, doctor.stderr)
        self.assertIn("Apple metal version", doctor.stdout)
        self.assertFalse(marker.exists())
        for args, guidance in (
            (["init"], "one-time setup"),
            (["retry", "legacy-late"], "ck submit --supersedes ID"),
        ):
            retired = subprocess.run(
                [sys.executable, "-B", str(tools / "ck"), *args],
                cwd=root,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(retired.returncode, 0, args)
            self.assertIn(guidance, retired.stderr)
        self.assertFalse(marker.exists())

    def test_c30_vendored_commands_fail_closed_after_publication(self) -> None:
        helper, bid, evidence, digest = self.prepared_with_evidence()
        env = {"CK_SESSION_ROOTS": json.dumps({"claude": str(self.session_roots)})}
        with mock.patch.dict(os.environ, env):
            helper.publish(bid, evidence, digest)
        root = Path(self.temp.name) / "post-activation-caller"
        pipeline.clone(self.base, root, self.hub)
        legacy = Path(__file__).parent / "fixtures/legacy"
        for name in ("ck", "pipeline.py", "environment.py"):
            shutil.copy2(legacy / name, root / "tools" / name)
        pipeline.pijul(root, "add", "--recursive", "tools")
        pipeline.pijul(
            root, "record", "-am", "install legacy fixture", "--author", "Bootstrap tests"
        )
        final = (self.hub / ".pipeline/accepted.json").read_bytes()
        for args in (
            ["status"],
            ["submit", "--spec", "late"],
            ["accept"],
            ["docs", "README.md"],
            ["worker", "old"],
            ["sync"],
        ):
            result = subprocess.run(
                [sys.executable, str(root / "tools/ck"), *args],
                cwd=root,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0, args)
            self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), final, args)

    def test_c30_trampoline_runs_legacy_controller_before_activation(self) -> None:
        legacy = Path(__file__).parent / "fixtures/legacy"
        checkpoint = Path(self.temp.name) / "legacy-checkpoint"
        pipeline.clone(self.base, checkpoint, self.hub)
        for name in ("ck", "pipeline.py", "environment.py"):
            shutil.copy2(legacy / name, checkpoint / "tools" / name)
        pipeline.pijul(checkpoint, "add", "--recursive", "tools")
        pipeline.pijul(
            checkpoint,
            "record",
            "-am",
            "legacy checkpoint controller",
            "--author",
            "Bootstrap tests",
        )
        legacy_state = {
            "repo": str(checkpoint),
            "fingerprint": pipeline.fingerprint(checkpoint),
            "submissions": [],
            "receipt": "legacy-receipt",
        }
        pointer.atomic_bytes(self.hub / ".pipeline/accepted.json", pointer.canonical(legacy_state))
        caller = Path(self.temp.name) / "legacy-trampoline-caller"
        pipeline.clone(self.base, caller, self.hub)
        shutil.copy2(Path(__file__).resolve().parents[1] / "tools/ck", caller / "tools/ck")
        result = subprocess.run(
            [sys.executable, "-B", str(caller / "tools/ck"), "status"],
            cwd=caller,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Accepted checkpoint:", result.stdout)

    def test_c31_bootstrap_helper_checks_origin_hub_and_runs_with_only_home(self) -> None:
        source_tools = Path(__file__).resolve().parents[1] / "tools"
        helper_path = self.capture / "tools/gate/bootstrap.py"
        shutil.copy2(source_tools / "gate/bootstrap.py", helper_path)
        wrong_hub = Path(self.temp.name) / "wrong-hub"
        wrong = subprocess.run(
            [sys.executable, "-B", str(helper_path), "--hub", str(wrong_hub), "status"],
            env={"HOME": str(Path.home())},
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(wrong.returncode, 0)
        self.assertIn("--hub differs", wrong.stderr)
        outside = Path(self.temp.name) / "outside.py"
        shutil.copy2(helper_path, outside)
        out = subprocess.run(
            [sys.executable, "-B", str(outside), "--hub", str(self.hub), "status"],
            env={"HOME": str(Path.home())},
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("outside its immutable submission capture", out.stderr)

        shutil.copy2(source_tools / "environment.py", self.capture / "tools/environment.py")
        shutil.copy2(source_tools / "pipeline.py", self.capture / "tools/pipeline.py")
        shutil.copytree(
            source_tools / "gate",
            self.capture / "tools/gate",
            dirs_exist_ok=True,
            ignore=shutil.ignore_patterns("__pycache__", "*.pyc"),
        )
        pipeline.pijul(self.capture, "add", "--recursive", "tools")
        pipeline.pijul(
            self.capture,
            "record",
            "-am",
            "Fixture reviewed helper",
            "--author",
            "Bootstrap tests",
        )
        submitted = self.hub / ".pipeline/queue/s1.json"
        pointer.atomic_bytes(
            submitted,
            pointer.canonical(
                {
                    "id": "s1",
                    "spec": "reviewed helper fixture",
                    "repo": str(self.capture),
                    "hashes": sorted(
                        pipeline.hashes(self.capture)
                        - set(json.loads(self.initial_pointer)["fingerprint"]["hashes"])
                    ),
                    "fingerprint": pipeline.fingerprint(self.capture),
                }
            ),
        )
        self.assertFalse((self.hub / ".pipeline/submissions/s1.json").exists())
        status_bid = "status-fixture"
        status_dir = self.hub / ".pipeline/bootstrap" / status_bid
        fingerprint_sha = hashlib.sha256(
            pointer.canonical(pipeline.fingerprint(self.capture))
        ).hexdigest()
        pointer.atomic_bytes(
            status_dir / "frozen.json",
            pointer.canonical(
                complete(
                    "frozen",
                    {
                        "submission": "s1",
                        "manifest_sha256": pointer.file_digest(submitted),
                        "fingerprint_sha256": fingerprint_sha,
                    },
                )
            ),
        )
        pointer.atomic_bytes(
            status_dir / "validated.json",
            pointer.canonical(
                complete(
                    "validated",
                    {
                        "candidate_fingerprint_sha256": "1" * 64,
                        "ci_receipt_sha256": "2" * 64,
                        "manifest_sha256": pointer.file_digest(submitted),
                    },
                )
            ),
        )
        clean_env = subprocess.run(
            [sys.executable, "-B", str(helper_path), "--hub", str(self.hub), "status", status_bid],
            env={"HOME": str(Path.home())},
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(clean_env.returncode, 0, clean_env.stderr)
        self.assertIn('"submission":"s1"', clean_env.stdout)
        status = json.loads(clean_env.stdout)
        self.assertEqual(status["manifest_sha256"], pointer.file_digest(submitted))
        self.assertEqual(status["integration"]["ci_receipt_sha256"], "2" * 64)
        self.assertTrue((self.hub / ".tools/signing/ed25519").exists())

    def test_c28b_failed_validation_restores_the_exact_legacy_pointer(self) -> None:
        helper = bootstrap_core.Bootstrap(self.hub, self.capture, lambda *args: False)
        with self.assertRaisesRegex(RuntimeError, "validation failed"):
            helper.prepare(
                "s1", __import__("hashlib").sha256(self.manifest_path.read_bytes()).hexdigest()
            )
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), self.initial_pointer)
        states = list((self.hub / ".pipeline/bootstrap").glob("*/restored.json"))
        self.assertEqual(len(states), 1)
        self.assertEqual(json.loads(states[0].read_bytes())["mode"], "restored")

    def test_c28b_prepare_recovers_interrupted_failed_bid_without_rewriting_failure(self) -> None:
        bids = []

        def crash_after_failed(point: str, *, bid: str, **_: object) -> None:
            if point == "after_failed":
                bids.append(bid)
                raise RuntimeError("crash after failed record")

        failing = bootstrap_core.Bootstrap(
            self.hub, self.capture, lambda *args: False, hook=crash_after_failed
        )
        with self.assertRaisesRegex(RuntimeError, "crash after failed record"):
            failing.prepare("s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest())
        old_bid = bids[0]
        directory = self.hub / ".pipeline/bootstrap" / old_bid
        failed_bytes = (directory / "failed.json").read_bytes()
        self.assertFalse((directory / "restoring.json").exists())
        next_bid = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).prepare(
            "s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest()
        )
        self.assertEqual((directory / "failed.json").read_bytes(), failed_bytes)
        self.assertTrue((directory / "restored.json").exists())
        self.assertEqual(
            json.loads((self.hub / ".pipeline/accepted.json").read_bytes())["bootstrap"], next_bid
        )

    def test_c28a_freeze_prefix_without_hold_aborts_without_touching_pointer(self) -> None:
        bid = "prefix"
        directory = self.hub / ".pipeline/bootstrap" / bid
        pointer.atomic_bytes(
            directory / "frozen.json", pointer.canonical(complete("frozen", {"bootstrap": bid}))
        )
        helper = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker)
        helper.restore(bid)
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), self.initial_pointer)
        self.assertEqual(
            json.loads((directory / "restored.json").read_bytes())["mode"], "never-held"
        )

    def test_c28a_every_pre_hold_prefix_recovers_without_replacing_legacy_pointer(self) -> None:
        helper = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker)
        prefixes = ("frozen", "legacy-pointer", "hold")
        for index, _final_record in enumerate(prefixes):
            bid = f"pre-hold-{index}"
            directory = self.hub / ".pipeline/bootstrap" / bid
            pointer.atomic_bytes(
                directory / "frozen.json", pointer.canonical(complete("frozen", {"bootstrap": bid}))
            )
            if index >= 1:
                pointer.atomic_bytes(directory / "legacy-pointer.json", self.initial_pointer)
            if index >= 2:  # noqa: PLR2004  # index of the third fixture prefix
                state = json.loads(self.initial_pointer)
                hold = pointer.tombstone(self.hub, state, bootstrap=bid)
                pointer.atomic_bytes(directory / "hold.json", pointer.canonical(hold))
            helper.restore(bid)
            self.assertEqual(
                (self.hub / ".pipeline/accepted.json").read_bytes(), self.initial_pointer
            )
            self.assertEqual(
                json.loads((directory / "restored.json").read_bytes())["mode"], "never-held"
            )

    def test_c28a_never_held_recovery_preserves_intervening_legacy_docs(self) -> None:
        bid = "never-held-with-docs"
        directory = self.hub / ".pipeline/bootstrap" / bid
        pointer.atomic_bytes(
            directory / "frozen.json", pointer.canonical(complete("frozen", {"bootstrap": bid}))
        )
        source = Path(self.temp.name) / "legacy-docs-during-freeze"
        pipeline.clone(Path(self.legacy.state()["repo"]), source, self.hub)
        (source / "README.md").write_text("legacy docs advanced while bootstrap never held\n")
        self.legacy.docs(source, ["README.md"], "legacy docs during never-held freeze")
        advanced_pointer = (self.hub / ".pipeline/accepted.json").read_bytes()
        self.assertNotEqual(advanced_pointer, self.initial_pointer)

        bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).restore(bid)
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), advanced_pointer)
        self.assertEqual(
            json.loads((directory / "restored.json").read_bytes())["mode"], "never-held"
        )

    def test_c28a_injected_freeze_prefixes_recover_by_cancel_and_next_prepare(self) -> None:
        ids = []

        class PrefixCrash(Exception):
            pass

        def after_frozen(point: str, *, bid: str, **_: object) -> None:
            if point == "after_frozen":
                ids.append(bid)
                raise PrefixCrash()

        helper = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker, hook=after_frozen)
        with self.assertRaises(PrefixCrash):
            helper.prepare("s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest())
        helper.cancel(ids[-1], "cancel never-held freeze")
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), self.initial_pointer)

        def after_hold(point: str, *, bid: str, **_: object) -> None:
            if point == "after_hold_pointer":
                ids.append(bid)
                raise PrefixCrash()

        helper = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker, hook=after_hold)
        with self.assertRaises(PrefixCrash):
            helper.prepare("s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest())
        interrupted = ids[-1]
        new_bid = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).prepare(
            "s1", hashlib.sha256(self.manifest_path.read_bytes()).hexdigest()
        )
        self.assertEqual(
            json.loads(
                (self.hub / ".pipeline/bootstrap" / interrupted / "restored.json").read_bytes()
            )["mode"],
            "restored",
        )
        self.assertEqual(
            json.loads((self.hub / ".pipeline/accepted.json").read_bytes())["bootstrap"], new_bid
        )

    def test_c28b_hold_prefix_and_restore_intent_recover_idempotently(self) -> None:
        helper = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker)
        bid = "held"
        directory = self.hub / ".pipeline/bootstrap" / bid
        state = json.loads(self.initial_pointer)
        pointer.atomic_bytes(
            directory / "frozen.json",
            pointer.canonical(
                complete(
                    "frozen",
                    {
                        "bootstrap": bid,
                        "base_pointer_sha256": __import__("hashlib")
                        .sha256(self.initial_pointer)
                        .hexdigest(),
                    },
                )
            ),
        )
        pointer.atomic_bytes(directory / "legacy-pointer.json", self.initial_pointer)
        hold = pointer.tombstone(self.hub, state, bootstrap=bid)
        hold_bytes = pointer.canonical(hold)
        pointer.atomic_bytes(directory / "hold.json", hold_bytes)
        pointer.atomic_bytes(self.hub / ".pipeline/accepted.json", hold_bytes)
        digest = __import__("hashlib").sha256(self.initial_pointer).hexdigest()
        pointer.atomic_bytes(
            directory / "restoring.json",
            pointer.canonical(
                complete(
                    "restoring",
                    {
                        "pointer": self.initial_pointer.decode(),
                        "pointer_sha256": digest,
                        "mode": "restored",
                    },
                )
            ),
        )
        helper.restore(bid)
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), self.initial_pointer)
        helper.restore(bid)
        self.assertEqual((directory / "restored.json").exists(), True)
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), self.initial_pointer)

    def test_c28b_docs_captured_under_hold_are_drained_by_cancel(self) -> None:
        bid = "held-docs"
        directory = self.hub / ".pipeline/bootstrap" / bid
        digest = __import__("hashlib").sha256(self.initial_pointer).hexdigest()
        state = json.loads(self.initial_pointer)
        pointer.atomic_bytes(
            directory / "frozen.json",
            pointer.canonical(
                complete("frozen", {"bootstrap": bid, "base_pointer_sha256": digest})
            ),
        )
        pointer.atomic_bytes(directory / "legacy-pointer.json", self.initial_pointer)
        hold = pointer.tombstone(self.hub, state, bootstrap=bid)
        pointer.atomic_bytes(directory / "hold.json", pointer.canonical(hold))
        pointer.atomic_bytes(self.hub / ".pipeline/accepted.json", pointer.canonical(hold))

        source = Path(self.temp.name) / "bootstrap-docs"
        pipeline.clone(Path(state["repo"]), source, self.hub)
        (source / "README.md").write_text("docs during bootstrap\n")
        did = docsqueue.Queue(self.hub).capture(source, "Bootstrap docs", ["README.md"])
        self.assertEqual(
            json.loads((self.hub / ".pipeline/docs" / f"{did}.json").read_bytes())["state"],
            "pending",
        )

        bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).cancel(bid, "test cancel")
        restored = json.loads((self.hub / ".pipeline/accepted.json").read_bytes())
        self.assertIn("README.md", restored["fingerprint"]["files"])
        self.assertEqual(
            json.loads((self.hub / ".pipeline/docs" / f"{did}.json").read_bytes())["state"],
            "published",
        )

    def test_c28b_docs_capture_requires_exact_hold_bytes(self) -> None:
        bid = "hold-bytes-check"
        directory = self.hub / ".pipeline/bootstrap" / bid
        state = json.loads(self.initial_pointer)
        hold_bytes = pointer.canonical(pointer.tombstone(self.hub, state, bootstrap=bid))
        pointer.atomic_bytes(
            directory / "frozen.json", pointer.canonical(complete("frozen", {"bootstrap": bid}))
        )
        pointer.atomic_bytes(directory / "legacy-pointer.json", self.initial_pointer)
        pointer.atomic_bytes(directory / "hold.json", hold_bytes)
        pointer.atomic_bytes(
            self.hub / ".pipeline/accepted.json", json.dumps(json.loads(hold_bytes)).encode()
        )
        source = Path(self.temp.name) / "docs-mismatched-hold"
        pipeline.clone(Path(state["repo"]), source, self.hub)
        (source / "README.md").write_text("must not capture on a rewritten hold\n")
        with self.assertRaisesRegex(RuntimeError, "no longer equals bootstrap hold"):
            docsqueue.Queue(self.hub).capture(source, "hold bytes", ["README.md"])

    def test_c28b_restore_acknowledges_pointer_written_before_crash(self) -> None:
        helper = bootstrap_core.Bootstrap(self.hub, self.capture, self.checker)
        bid = "restore-ack"
        directory = self.hub / ".pipeline/bootstrap" / bid
        saved = self.initial_pointer
        digest = __import__("hashlib").sha256(saved).hexdigest()
        pointer.atomic_bytes(
            directory / "frozen.json",
            pointer.canonical(
                complete("frozen", {"bootstrap": bid, "base_pointer_sha256": digest})
            ),
        )
        pointer.atomic_bytes(directory / "legacy-pointer.json", saved)
        state = json.loads(saved)
        hold = pointer.tombstone(self.hub, state, bootstrap=bid)
        pointer.atomic_bytes(directory / "hold.json", pointer.canonical(hold))
        pointer.atomic_bytes(self.hub / ".pipeline/accepted.json", saved)
        pointer.atomic_bytes(
            directory / "restoring.json",
            pointer.canonical(
                complete(
                    "restoring",
                    {"pointer": saved.decode(), "pointer_sha256": digest, "mode": "restored"},
                )
            ),
        )
        helper.restore(bid)
        self.assertEqual((directory / "restored.json").exists(), True)
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), saved)

    def test_c30_restoring_recovery_rechecks_base_pointer_before_write(self) -> None:
        bid = "restore-recheck-base"
        directory = self._held_bid(bid)

        def crash_after_intent(point: str, **_: object) -> None:
            if point == "after_restoring":
                raise RuntimeError("intent written")

        helper = bootstrap_core.Bootstrap(
            self.hub, self.capture, self.checker, hook=crash_after_intent
        )
        with self.assertRaisesRegex(RuntimeError, "intent written"):
            helper.restore(bid)
        self.assertTrue((directory / "restoring.json").exists())
        pointer.atomic_bytes(directory / "legacy-pointer.json", self.initial_pointer + b"tampered")
        hold_bytes = (directory / "hold.json").read_bytes()
        with self.assertRaisesRegex(RuntimeError, "legacy pointer digest does not match freeze"):
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).restore(bid)
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), hold_bytes)

    def test_c28b_restore_refuses_pointer_matching_neither_hold_nor_intent(self) -> None:
        bid = "restore-pointer-incident"
        directory = self.hub / ".pipeline/bootstrap" / bid
        state = json.loads(self.initial_pointer)
        digest = hashlib.sha256(self.initial_pointer).hexdigest()
        pointer.atomic_bytes(
            directory / "frozen.json",
            pointer.canonical(
                complete("frozen", {"bootstrap": bid, "base_pointer_sha256": digest})
            ),
        )
        pointer.atomic_bytes(directory / "legacy-pointer.json", self.initial_pointer)
        hold = pointer.canonical(pointer.tombstone(self.hub, state, bootstrap=bid))
        pointer.atomic_bytes(directory / "hold.json", hold)
        incident = pointer.canonical(pointer.tombstone(self.hub, state, bootstrap="other-bid"))
        pointer.atomic_bytes(self.hub / ".pipeline/accepted.json", incident)
        with self.assertRaisesRegex(RuntimeError, "accepted pointer is not the saved hold"):
            bootstrap_core.Bootstrap(self.hub, self.capture, self.checker).restore(bid)
        self.assertEqual((self.hub / ".pipeline/accepted.json").read_bytes(), incident)


if __name__ == "__main__":
    unittest.main()
