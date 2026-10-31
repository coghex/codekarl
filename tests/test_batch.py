"""The gate batch state machine exercises immutable manifests and real Pijul merges."""

import contextlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
import threading
import time
import unittest
from collections.abc import Callable, Iterable, Iterator, Mapping, Sequence
from pathlib import Path
from typing import ClassVar
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import chatfix
import pipeline
import procfix
from environment import signing
from gate import batch, docsqueue, locks, meta, pointer, postpublish, procscan, provenance, reviews
from gate import cli as gate_cli
from gate.records import (
    Author,
    BatchCIReceipt,
    BatchValidated,
    DocsEntry,
    IssueRef,
    ManifestV2,
    ProcessRow,
    ProvenancePayload,
    is_batch_ci_receipt,
    is_manifest_v2,
    load_batch_validated,
    load_ci_receipt,
    load_docs_entry,
    load_manifest,
    load_provenance,
)
from sessionfix import no_agent


def complete_receipt(partial: Mapping[str, object]) -> dict[str, object]:
    """A fixture receipt with every field a stored `ck-review/1` receipt has.

    The fixtures below state only what their scenario depends on; the boundary that reads receipts
    (`records.load_review_receipt`) requires the rest, so they are filled with neutral values."""
    receipt = {
        "schema": "ck-review/1",
        "recorded": 1,
        "report_sha256": "0" * 64,
        "issues": [],
        "base_examined": None,
        "incremental_over": None,
        **partial,
    }
    held = partial["reviewer"]
    assert isinstance(held, dict)
    reviewer = dict(held)
    if not reviewer.get("owner_signoff"):
        reviewer.setdefault("client", "fixture")
        reviewer.setdefault("model", "fixture-model")
        reviewer.setdefault("evidence", {"path": "/fixture", "offset": 0, "sha256": "0" * 64})
    receipt["reviewer"] = reviewer
    return receipt


class BatchGateSpecification(unittest.TestCase):
    def setUp(self) -> None:
        no_agent(self)
        chatfix.guard(self)
        procfix.isolate(self)
        self.temp = tempfile.TemporaryDirectory(prefix="ck-batch-gate-")
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        # RC-1: publication binds the shared chat inputs; every test uses a fixture tree.
        self.shared = chatfix.build_shared(root / "shared-skills")
        os.environ["CK_CHAT_SHARED_ROOT"] = str(self.shared)
        self.signing = signing(Path.home() / "work/codekarl")
        self.signing.__enter__()
        self.addCleanup(self.signing.__exit__, None, None, None)
        self.hub = root / "hub"
        self.hub.mkdir()
        self.base = root / "base"
        self.base.mkdir()
        pipeline.pijul(self.base, "init")
        (self.base / ".ignore").write_text("/.pipeline\n/.ck-hub\n")
        (self.base / "source.txt").write_text("base\n")
        (self.base / "README.md").write_text("base docs\n")
        self.populate_base()
        self.record(self.base, "base")
        legacy = pipeline.Pipeline(self.hub, lambda repo, log: True)
        legacy.initialize(self.base)
        old = json.loads(legacy.pointer.read_bytes())
        old["schema"] = 2
        pointer.write_pointer(self.hub / ".pipeline/accepted-v2.json", old)
        self.store = meta.Store(self.hub)
        self.store.ensure()
        self.calls: list[object] = []

    def populate_base(self) -> None:
        """Extra files for the accepted base before it is recorded (a subclass hook)."""

    def chat_inputs(self, log: Path, files: Mapping[str, str] | None = None) -> None:
        """What the real recipe's `install.py ci` writes beside the log (CK_CHAT_INPUTS)."""
        chatfix.write_record(
            log.with_suffix(".chat-inputs.json"), files or chatfix.expected_inputs(self.shared)
        )

    def passing(self, repo: Path, log: Path, fd: int) -> bool:
        log.write_text("ok")
        self.chat_inputs(log)
        return True

    def record(self, repo: Path, message: str) -> None:
        pipeline.pijul(repo, "add", "--recursive", ".")
        pipeline.pijul(repo, "record", "-am", message, "--author", "Gate batch spec")

    CODEX: ClassVar[Author] = {
        "identity": "worker",
        "session": "worker-session",
        "client": "codex",
        "family": "codex",
    }
    CLAUDE: ClassVar[Author] = {
        "identity": "claude-worker",
        "session": "claude-worker-session",
        "client": "claude",
        "family": "claude",
    }

    def description(self, author: Author, issues: Iterable[str] = (), **override: object) -> str:
        """The description `ck record` writes: a ck-provenance/1 payload and its digest line."""
        payload = {
            "schema": "ck-provenance/1",
            "issues": list(issues),
            "identity": author["identity"],
            "client": author["client"],
            "session": author["session"],
            "role": "worker",
            "families": [author["family"]],
            "recorded": "2026-10-16T00:00:00+00:00",
            **override,
        }
        payload = {key: value for key, value in payload.items() if value is not None}
        return (
            provenance.PREFIX
            + provenance.payload_digest(payload)
            + "\n"
            + (pointer.canonical(payload).decode())
        )

    def record_as(
        self,
        repo: Path,
        message: str,
        author: Author | None,
        issues: Iterable[str] = (),
        description: str | None = None,
    ) -> None:
        """A real Pijul record carrying provenance (or, with author None, a raw patch)."""
        pipeline.pijul(repo, "add", "--recursive", ".")
        if author is None and description is None:
            self.record(repo, message)
            return
        pipeline.pijul(
            repo,
            "record",
            "-a",
            "-m",
            message,
            "--author",
            (author or self.CODEX)["identity"],
            "--description",
            description or self.description(author or self.CODEX, issues),
        )

    def submission(  # noqa: PLR0913  # test helper with explicit scenario arguments
        self,
        name: str,
        *,
        authors: Sequence[Author] | None = None,
        raw: bool = False,
        declared: list[Author] | None = None,
        submitter: Author | None = None,
        slots: Callable[[list[str]], dict[str, str | None]] | None = None,
        review: bool = True,
        parent: ManifestV2 | None = None,
        issues: Iterable[IssueRef] = (),
        descriptions: Mapping[int, str] | None = None,
    ) -> ManifestV2:
        """A live submission. `authors` record one real patch each (default: one Codex worker);
        `raw` records them without provenance; `declared` overrides the manifest's authors."""
        authors = list(authors or [self.CODEX])
        base_state = pointer.read_pointer(self.hub)
        worker = Path(self.temp.name) / name
        source = Path(parent["repo"]) if parent else Path(base_state["repo"])
        pipeline.clone(source, worker, self.hub)
        files = []
        for index, author in enumerate(authors):
            filename = f"{name}.txt" if index == 0 else f"{name}-{index}.txt"
            (worker / filename).write_text(f"{name} {index}\n")
            files.append(filename)
            override = (descriptions or {}).get(index)
            self.record_as(
                worker,
                f"{name} {index}",
                None if raw else author,
                description=override,
            )
        capture = self.hub / ".pipeline/submissions" / name
        pipeline.clone(worker, capture, self.hub)
        hashes = sorted(pipeline.hashes(capture) - set(base_state["fingerprint"]["hashes"]))
        manifest: ManifestV2 = {
            "schema": 2,
            "id": name,
            "repo": str(capture),
            "hashes": hashes,
            "files": files,
            "fingerprint": pipeline.fingerprint(capture),
            "issues": list(issues),
            "authors": declared
            or [
                {"identity": a["identity"], "session": a["session"], "family": a["family"]}
                for a in authors
            ],
            "supersedes": [],
            "spec": f"fixture submission {name}",
            "provenance": {},
            "base": {"pointer_sha256": "0" * 64, "fingerprint_sha256": "0" * 64},
        }
        if slots is not None:
            manifest["provenance"] = slots(hashes)
        if submitter is not None:
            manifest["submitter"] = submitter
        path = self.hub / ".pipeline/queue" / f"{name}.json"
        pointer.atomic_bytes(path, pointer.canonical(manifest))
        if review:
            self.review_as(
                name, "claude", f"review-{name}", "claude-reviewer", rid=f"review-{name}"
            )
        return manifest

    def review_as(  # noqa: PLR0913  # test helper with explicit scenario arguments
        self,
        name: str,
        family: str,
        session: str,
        identity: str | None = None,
        *,
        rid: str | None = None,
        over: str | None = None,
        kind: str = "submission",
    ) -> str:
        """A fixture approval receipt for a live submission by a reviewer of `family`."""
        path = self.hub / ".pipeline/queue" / f"{name}.json"
        manifest = json.loads(path.read_bytes())
        rid = rid or f"{family}-{session}-{name}"
        receipt = {
            "schema": "ck-review/1",
            "id": rid,
            "kind": kind,
            "subject": reviews.subject_for_submission(path, manifest),
            "verdict": "approved",
            "reviewer": {
                "identity": identity or f"{family}-reviewer-{session}",
                "session": session,
                "role": "reviewer",
                "class": "A",
                "family": family,
                "effort": "high",
            },
        }
        if over:
            receipt["incremental_over"] = over
        self.store.write(
            self.store.repo / "reviews" / f"{rid}.json",
            pointer.canonical(complete_receipt(receipt)),
            "Review",
        )
        return rid

    def eligible_ids(self) -> set[str]:
        return {item["id"] for item in batch.Manager(self.hub).eligible()}

    def integration_receipt(
        self, bid: str, identifier: str = "integration", verdict: str = "approved"
    ) -> None:
        integration_subject = batch.subject(self.hub, bid)
        receipt = {
            "schema": "ck-review/1",
            "id": identifier,
            "kind": "batch",
            "subject": integration_subject,
            "verdict": verdict,
            "reviewer": {
                "identity": "claude-integration",
                "session": f"{identifier}-session",
                "role": "reviewer",
                "class": "A",
                "family": "claude",
                "effort": "high",
            },
        }
        self.store.write(
            self.store.repo / "reviews" / f"{identifier}.json",
            pointer.canonical(complete_receipt(receipt)),
            "Review batch",
        )

    def test_c11_manager_excludes_revoked_withdrawn_superseded_and_edited_submissions(self) -> None:
        manager = batch.Manager(self.hub, self.passing)
        self.submission("revoked-member")
        self.store.write(
            self.store.repo / "events/revoke-member.json",
            pointer.canonical(
                {"kind": "revocation", "review": "review-revoked-member", "reason": "fixture"}
            ),
            "Revoke member review",
        )
        self.assertNotIn("revoked-member", {item["id"] for item in manager.eligible()})

        withdrawn = self.submission("withdrawn-member")
        self.store.write(
            self.store.repo / "events/withdraw-member.json",
            pointer.canonical(
                {"kind": "withdrawn", "submission": withdrawn["id"], "reason": "fixture"}
            ),
            "Withdraw member",
        )
        self.assertNotIn("withdrawn-member", {item["id"] for item in manager.eligible()})

        superseded = self.submission("superseded-member")
        self.store.write(
            self.store.repo / "events/supersede-member.json",
            pointer.canonical(
                {"kind": "supersession", "submission": superseded["id"], "by": "successor"}
            ),
            "Supersede member",
        )
        self.assertNotIn("superseded-member", {item["id"] for item in manager.eligible()})

        edited = self.submission("edited-capture")
        capture = Path(edited["repo"])
        (capture / "edited-capture.txt").write_text("edited after capture\n")
        self.record(capture, "edit immutable capture")
        self.assertNotIn("edited-capture", {item["id"] for item in manager.eligible()})

    def test_c14_two_eligible_submissions_use_one_ci_run_and_wait_for_review(self) -> None:
        left = self.submission("left")
        right = self.submission("right")

        def validate(repo: Path, log: Path, fd: int) -> bool:
            self.calls.append(
                ("left.txt" in pipeline.files(repo), "right.txt" in pipeline.files(repo))
            )
            log.write_text("passed\n")
            self.chat_inputs(log)
            return True

        manager = batch.Manager(self.hub, validate)
        bid = manager.prepare()
        state = pointer.read_pointer(self.hub)
        self.assertNotIn("left", state.get("submissions", []))
        self.assertEqual(self.calls, [(True, True)])
        status = manager.status(bid)
        # the batch's records, plus CK-38's advisory publication basis (two members: needs review)
        self.assertEqual(
            set(status),
            {"frozen", "validating", "validated", "publication_basis", "publication_basis_reason"},
        )
        self.assertEqual(status["publication_basis"], "integration-review")
        validated = json.loads((batch.batch_path(self.hub, bid) / "validated.json").read_bytes())
        self.assertCountEqual(validated["members"], [left["id"], right["id"]])
        with locks.ci(self.hub):
            pass

        self.integration_receipt(bid)
        manager.approve(bid)
        manager.publish(bid)
        self.assertCountEqual(pointer.read_pointer(self.hub)["submissions"], ["left", "right"])
        self.assertIn("published", manager.status(bid))
        with locks.ci(self.hub):
            pass

    def test_c16_mixed_family_batch_requires_both_integration_reviewers(self) -> None:
        manifest = self.submission("mixed", authors=[self.CODEX, self.CLAUDE])
        path = self.hub / ".pipeline/queue/mixed.json"
        subject = reviews.subject_for_submission(path, manifest)
        for family, identity in (("claude", "claude-reviewer"), ("codex", "codex-reviewer")):
            receipt = {
                "schema": "ck-review/1",
                "id": f"mixed-{family}",
                "kind": "submission",
                "subject": subject,
                "verdict": "approved",
                "reviewer": {
                    "identity": identity,
                    "session": f"mixed-{family}-session",
                    "role": "reviewer",
                    "class": "A",
                    "family": family,
                    "effort": "high",
                },
            }
            self.store.write(
                self.store.repo / "reviews" / f"mixed-{family}.json",
                pointer.canonical(complete_receipt(receipt)),
                "Mixed author review",
            )
        manager = batch.Manager(self.hub, self.passing)
        bid = manager.prepare()
        self.integration_receipt(bid, "mixed-integration-claude")
        with self.assertRaisesRegex(RuntimeError, "does not have fresh independent"):
            manager.approve(bid)
        receipt = {
            "schema": "ck-review/1",
            "id": "mixed-integration-codex",
            "kind": "batch",
            "subject": batch.subject(self.hub, bid),
            "verdict": "approved",
            "reviewer": {
                "identity": "codex-integration",
                "session": "mixed-integration-codex-session",
                "role": "reviewer",
                "class": "A",
                "family": "codex",
                "effort": "high",
            },
        }
        self.store.write(
            self.store.repo / "reviews/mixed-integration-codex.json",
            pointer.canonical(complete_receipt(receipt)),
            "Codex integration review",
        )
        manager.approve(bid)
        self.assertTrue((batch.batch_path(self.hub, bid) / "approved.json").exists())

    def test_c19_publish_pointer_crash_is_completed_by_rerun(self) -> None:
        self.submission("recovery")
        base_manager = batch.Manager(self.hub, self.passing)
        bid = base_manager.prepare()
        self.integration_receipt(bid, "recovery-integration")
        base_manager.approve(bid)

        def crash(point: str, **_: object) -> None:
            if point == "after_pointer_write":
                raise RuntimeError("injected pointer interruption")

        with self.assertRaisesRegex(RuntimeError, "injected pointer interruption"):
            batch.Manager(self.hub, self.passing, hook=crash).publish(bid)
        self.assertIn("recovery", pointer.read_pointer(self.hub)["submissions"])
        batch.Manager(self.hub, self.passing).publish(bid)
        self.assertTrue((batch.batch_path(self.hub, bid) / "published.json").exists())

    def test_c19_crash_before_publishing_intent_leaves_pointer_unchanged(self) -> None:
        self.submission("before-intent")
        manager = batch.Manager(self.hub, self.passing)
        bid = manager.prepare()
        self.integration_receipt(bid, "before-intent-integration")
        manager.approve(bid)
        before = (self.hub / ".pipeline/accepted-v2.json").read_bytes()

        def crash(point: str, **_: object) -> None:
            if point == "before_publish_intent":
                raise RuntimeError("before intent")

        with self.assertRaisesRegex(RuntimeError, "before intent"):
            batch.Manager(self.hub, hook=crash).publish(bid)
        self.assertFalse((batch.batch_path(self.hub, bid) / "publishing.json").exists())
        self.assertEqual((self.hub / ".pipeline/accepted-v2.json").read_bytes(), before)
        manager.publish(bid)

    def test_c20_freeze_keeps_ci_owner_until_interrupted_recovery(self) -> None:
        self.submission("owned")
        manager = batch.Manager(self.hub, self.passing)
        captured: dict[str, str] = {}

        class FreezePause(Exception):
            pass

        def pause(point: str, *, bid: str, **_: object) -> None:
            if point != "after_frozen":
                return
            captured["bid"] = bid
            errors: list[str] = []

            def attempt(operation: Callable[[], object]) -> None:
                try:
                    operation()
                except RuntimeError as error:
                    errors.append(str(error))

            attempts = [
                threading.Thread(
                    target=attempt,
                    args=(lambda: batch.Manager(self.hub, self.passing).prepare(),),
                ),
                threading.Thread(
                    target=attempt,
                    args=(lambda: batch.Manager(self.hub).cancel(bid, "racing cancel"),),
                ),
            ]
            for thread in attempts:
                thread.start()
            for thread in attempts:
                thread.join()
            self.assertEqual(len(errors), 2)
            self.assertTrue(all("Already running" in error for error in errors), errors)
            raise FreezePause()

        with self.assertRaises(FreezePause):
            batch.Manager(self.hub, self.passing, hook=pause).prepare()
        # Once the interrupted owner releases CI, the next prepare records the
        # prior freeze as interrupted before considering the same member again.
        bid = manager.prepare()
        self.assertIn("failed", manager.status(captured["bid"]))
        self.assertIn("validated", manager.status(bid))
        manager.cancel(bid, "finish owner test")
        self.submission("validating")

        class ValidatingPause(Exception):
            pass

        def pause_validating(point: str, **_: object) -> None:
            if point == "after_validating":
                raise ValidatingPause()

        with self.assertRaises(ValidatingPause):
            batch.Manager(self.hub, self.passing, hook=pause_validating).prepare()
        validating = sorted((self.hub / ".pipeline/batches").glob("*/validating.json"))[
            -1
        ].parent.name
        with self.assertRaisesRegex(RuntimeError, "validating batch"):
            manager.cancel(validating, "must not cancel CI")

    def test_c21_docs_capture_flushes_immediately_and_is_a_verified_descendant(self) -> None:
        original = (self.hub / ".pipeline/accepted-v2.json").read_bytes()
        copy = Path(self.temp.name) / "docs-copy"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), copy, self.hub)
        (copy / "README.md").write_text("Updated docs\n")
        did = docsqueue.Queue(self.hub).capture(copy, "Update docs", ["README.md"])
        row = json.loads((self.hub / ".pipeline/docs" / f"{did}.json").read_bytes())
        self.assertEqual(row["state"], "published")
        current = (self.hub / ".pipeline/accepted-v2.json").read_bytes()
        self.assertTrue(docsqueue.is_docs_descendant(self.hub, current, original))
        self.assertIn("README.md", pointer.read_pointer(self.hub)["fingerprint"]["files"])

    def test_c21_docs_during_active_batch_queue_without_publishing(self) -> None:
        batch_dir = self.hub / ".pipeline/batches/B-HOLD"
        batch_dir.mkdir(parents=True)
        pointer.atomic_bytes(batch_dir / "frozen.json", b"{}")
        copy = Path(self.temp.name) / "queued-docs-copy"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), copy, self.hub)
        (copy / "README.md").write_text("Queued docs\n")
        did = docsqueue.Queue(self.hub).capture(copy, "Queue docs", ["README.md"])
        row = json.loads((self.hub / ".pipeline/docs" / f"{did}.json").read_bytes())
        self.assertEqual(row["state"], "pending")
        self.assertEqual(docsqueue.Queue(self.hub).flush(), 0)

    def test_c21_flush_and_prepare_lock_schedules_are_serialized(self) -> None:  # noqa: PLR0915  # long scenario test: the statements are the scenario
        dummy = self.hub / ".pipeline/batches/FLUSH-FIRST"
        dummy.mkdir(parents=True)
        pointer.atomic_bytes(dummy / "frozen.json", b"{}")
        source = Path(self.temp.name) / "flush-first-docs"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), source, self.hub)
        (source / "README.md").write_text("flush gets state first\n")
        did = docsqueue.Queue(self.hub).capture(source, "flush first", ["README.md"])
        pointer.atomic_bytes(dummy / "failed.json", b"{}")
        self.submission("after-flush")

        flush_holds_state = threading.Event()
        prepare_attempted_state = threading.Event()
        prepare_result: dict[str, str] = {}
        prepare_thread: dict[str, threading.Thread] = {}
        original_state = locks.state

        def observed_state(hub: Path) -> object:
            if threading.current_thread() is prepare_thread.get("thread"):
                prepare_attempted_state.set()
            return original_state(hub)

        def prepare_worker() -> None:
            prepare_result["bid"] = batch.Manager(self.hub, self.passing).prepare()

        def pause_flush(point: str, **_: object) -> None:
            if point != "after_journal":
                return
            prepare_thread["thread"] = threading.Thread(target=prepare_worker)
            prepare_thread["thread"].start()
            self.assertTrue(prepare_attempted_state.wait(2))
            flush_holds_state.set()

        with mock.patch.object(locks, "state", observed_state):
            docsqueue.Queue(self.hub, hook=pause_flush).flush()
        prepare_thread["thread"].join()
        self.assertTrue(flush_holds_state.is_set())
        bid = prepare_result["bid"]
        frozen = json.loads((batch.batch_path(self.hub, bid) / "frozen.json").read_bytes())
        self.assertEqual(
            frozen["base_pointer_sha256"],
            pointer.file_digest(Path(pointer.read_pointer(self.hub)["_pointer_path"])),
        )
        self.assertEqual(
            json.loads((self.hub / ".pipeline/docs" / f"{did}.json").read_bytes())["state"],
            "published",
        )
        batch.Manager(self.hub).cancel(bid, "finish flush-first schedule")

        self.submission("after-prepare")
        before = (self.hub / ".pipeline/accepted-v2.json").read_bytes()
        source = Path(self.temp.name) / "prepare-first-docs"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), source, self.hub)
        (source / "README.md").write_text("prepare gets state first\n")
        capture_attempted = threading.Event()
        capture_result: dict[str, str] = {}
        capture_thread: dict[str, threading.Thread] = {}

        def observed_capture_state(hub: Path) -> object:
            if threading.current_thread() is capture_thread.get("thread"):
                capture_attempted.set()
            return original_state(hub)

        def capture_worker() -> None:
            capture_result["id"] = docsqueue.Queue(self.hub).capture(
                source, "prepare first", ["README.md"]
            )

        def pause_prepare(point: str, **_: object) -> None:
            if point != "after_frozen":
                return
            capture_thread["thread"] = threading.Thread(target=capture_worker)
            capture_thread["thread"].start()
            self.assertTrue(capture_attempted.wait(2))

        with mock.patch.object(locks, "state", observed_capture_state):
            bid = batch.Manager(
                self.hub,
                self.passing,
                hook=pause_prepare,
            ).prepare()
        capture_thread["thread"].join()
        self.assertEqual(docsqueue.Queue(self.hub).flush(), 0)
        self.assertEqual((self.hub / ".pipeline/accepted-v2.json").read_bytes(), before)
        self.assertEqual(
            json.loads((self.hub / ".pipeline/docs" / f"{capture_result['id']}.json").read_bytes())[
                "state"
            ],
            "pending",
        )
        batch.Manager(self.hub).cancel(bid, "finish prepare-first schedule")

    def test_c22_docs_journal_crashes_resume_in_fifo_order(self) -> None:  # noqa: PLR0915  # long scenario test: the statements are the scenario
        sentinel = self.hub / ".pipeline/batches/JOURNAL-HOLD"
        sentinel.mkdir(parents=True)
        pointer.atomic_bytes(sentinel / "frozen.json", b"{}")
        base = Path(pointer.read_pointer(self.hub)["repo"])
        first_copy = Path(self.temp.name) / "journal-docs-one"
        pipeline.clone(base, first_copy, self.hub)
        (first_copy / "README.md").write_text("one\n")
        first = docsqueue.Queue(self.hub).capture(first_copy, "one", ["README.md"])
        second_copy = Path(self.temp.name) / "journal-docs-two"
        pipeline.clone(base, second_copy, self.hub)
        (second_copy / "docs").mkdir(exist_ok=True)
        (second_copy / "docs/two.md").write_text("two\n")
        second = docsqueue.Queue(self.hub).capture(second_copy, "two", ["docs/two.md"])
        pointer.atomic_bytes(sentinel / "failed.json", b"{}")

        def first_crash(point: str, *, entry: DocsEntry, **_: object) -> None:
            if point == "after_journal" and entry["id"] == first:
                raise RuntimeError("after journal")

        with self.assertRaisesRegex(RuntimeError, "after journal"):
            docsqueue.Queue(self.hub, hook=first_crash).flush()

        def second_crash(point: str, *, entry: DocsEntry, **_: object) -> None:
            if point == "after_pointer" and entry["id"] == second:
                raise RuntimeError("after pointer")

        with self.assertRaisesRegex(RuntimeError, "after pointer"):
            docsqueue.Queue(self.hub, hook=second_crash).flush()
        docsqueue.Queue(self.hub).flush()
        rows = [
            json.loads((self.hub / ".pipeline/docs" / f"{identifier}.json").read_bytes())
            for identifier in (first, second)
        ]
        self.assertEqual([row["state"] for row in rows], ["published", "published"])
        journals = sorted((self.hub / ".pipeline/docs/journal").glob("*.json"))
        self.assertEqual(
            [json.loads(path.read_bytes())["entries"] for path in journals], [[first], [second]]
        )

        third_source = Path(self.temp.name) / "journal-docs-three"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), third_source, self.hub)
        (third_source / "docs").mkdir(exist_ok=True)
        (third_source / "docs/three.md").write_text("three\n")
        third: dict[str, str] = {}

        def final_crash(point: str, *, entry: DocsEntry, **_: object) -> None:
            if point == "after_ack":
                third["id"] = entry["id"]
                raise RuntimeError("after final publication")

        with self.assertRaisesRegex(RuntimeError, "after final publication"):
            docsqueue.Queue(self.hub, hook=final_crash).capture(
                third_source, "three", ["docs/three.md"]
            )
        self.assertEqual(docsqueue.Queue(self.hub).flush(), 0)
        self.assertEqual(
            json.loads((self.hub / ".pipeline/docs" / f"{third['id']}.json").read_bytes())["state"],
            "published",
        )

        hold = self.hub / ".pipeline/batches/TERMINAL-PREPARE"
        hold.mkdir(parents=True)
        pointer.atomic_bytes(hold / "frozen.json", b"{}")
        queued_source = Path(self.temp.name) / "terminal-prepare-docs"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), queued_source, self.hub)
        (queued_source / "docs/prepare.md").write_text("before next prepare\n")
        queued_id = docsqueue.Queue(self.hub).capture(
            queued_source, "before prepare", ["docs/prepare.md"]
        )
        self.submission("next-prepare")
        pointer.atomic_bytes(hold / "failed.json", b"{}")
        next_bid = batch.Manager(self.hub, self.passing).prepare()
        current = pointer.read_pointer(self.hub)
        frozen = json.loads((batch.batch_path(self.hub, next_bid) / "frozen.json").read_bytes())
        self.assertEqual(
            frozen["base_pointer_sha256"], pointer.file_digest(Path(current["_pointer_path"]))
        )
        self.assertEqual(
            json.loads((self.hub / ".pipeline/docs" / f"{queued_id}.json").read_bytes())["state"],
            "published",
        )

    def test_c13_submit_refuses_doc_path_changes_and_queued_doc_dependencies(self) -> None:
        # First reject a net README edit through the actual submit entry point.
        docs = Path(self.temp.name) / "submit-docs"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), docs, self.hub)
        (docs / "README.md").write_text("ordinary docs\n")
        self.record(docs, "ordinary docs")
        args = type(
            "Args", (), {"issue": "CK-1", "spec": "fixture", "supersedes": None, "role": None}
        )()
        with (
            mock.patch.object(meta.Store, "show", return_value={"id": "CK-1"}),
            mock.patch.object(meta, "ready", return_value=True),
            self.assertRaisesRegex(RuntimeError, "Ordinary documentation"),
        ):
            gate_cli._submit(docs, self.hub, self.store, args)

        # A revert can leave the working tree's net docs unchanged while the
        # recorded closure still depends on the queued docs patch.
        sentinel = self.hub / ".pipeline/batches/SUBMIT-DOCS-HOLD"
        sentinel.mkdir(parents=True)
        pointer.atomic_bytes(sentinel / "frozen.json", b"{}")
        docs_source = Path(self.temp.name) / "queued-source"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), docs_source, self.hub)
        (docs_source / "README.md").write_text("queued docs\n")
        did = docsqueue.Queue(self.hub).capture(docs_source, "queued", ["README.md"])
        row = json.loads((self.hub / ".pipeline/docs" / f"{did}.json").read_bytes())
        dependent = Path(self.temp.name) / "dependent-code"
        pipeline.clone(Path(row["repo"]), dependent, self.hub)
        (dependent / "README.md").write_text("base docs\n")
        (dependent / "dependent.txt").write_text("code\n")
        self.record(dependent, "revert docs and add code")
        with (
            mock.patch.object(meta.Store, "show", return_value={"id": "CK-1"}),
            mock.patch.object(meta, "ready", return_value=True),
            self.assertRaisesRegex(RuntimeError, "depends on queued documentation"),
        ):
            gate_cli._submit(dependent, self.hub, self.store, args)

    def test_c23_conflicting_docs_resolution_records_a_verified_group(self) -> None:  # noqa: PLR0915  # long scenario test: the statements are the scenario
        sentinel = self.hub / ".pipeline/batches/DOCS-HOLD"
        sentinel.mkdir(parents=True)
        pointer.atomic_bytes(sentinel / "frozen.json", b"{}")
        base = Path(pointer.read_pointer(self.hub)["repo"])
        left = Path(self.temp.name) / "docs-left"
        right = Path(self.temp.name) / "docs-right"
        pipeline.clone(base, left, self.hub)
        pipeline.clone(base, right, self.hub)
        (left / "README.md").write_text("left docs\n")
        (right / "README.md").write_text("right docs\n")
        first = docsqueue.Queue(self.hub).capture(left, "Left docs", ["README.md"])
        self.record(right, "Right docs")
        second = "Z-CONFLICT"
        second_repo = self.hub / ".pipeline/docs/captures" / second
        pipeline.clone(right, second_repo, self.hub)
        state = json.loads((self.hub / ".pipeline/accepted-v2.json").read_bytes())
        second_row: DocsEntry = {
            "id": second,
            "state": "pending",
            "created": 1,
            "base_pointer_sha256": "fixture",
            "repo": str(second_repo),
            "hashes": sorted(pipeline.hashes(second_repo) - set(state["fingerprint"]["hashes"])),
            "files": ["README.md"],
            "fingerprint": pipeline.fingerprint(second_repo),
            "message": "Right docs",
        }
        pointer.atomic_bytes(
            self.hub / ".pipeline/docs" / f"{second}.json", pointer.canonical(second_row)
        )
        dependent_repo = self.hub / ".pipeline/docs/captures/Z-DEPENDENT"
        pipeline.clone(second_repo, dependent_repo, self.hub)
        (dependent_repo / "docs").mkdir(exist_ok=True)
        (dependent_repo / "docs/dependent.md").write_text("depends on D1\n")
        self.record(dependent_repo, "Dependent docs")
        dependent_row = {
            "id": "Z-DEPENDENT",
            "state": "pending",
            "created": 2,
            "repo": str(dependent_repo),
            "hashes": sorted(pipeline.hashes(dependent_repo) - set(state["fingerprint"]["hashes"])),
            "files": ["docs/dependent.md"],
            "fingerprint": pipeline.fingerprint(dependent_repo),
            "message": "Dependent docs",
        }
        pointer.atomic_bytes(
            self.hub / ".pipeline/docs/Z-DEPENDENT.json", pointer.canonical(dependent_row)
        )
        pointer.atomic_bytes(sentinel / "published.json", b"{}")
        self.assertEqual(docsqueue.Queue(self.hub).flush(), 1)
        conflict = json.loads((self.hub / ".pipeline/docs" / f"{second}.json").read_bytes())
        self.assertEqual(conflict["state"], "conflict")
        blocked = json.loads((self.hub / ".pipeline/docs/Z-DEPENDENT.json").read_bytes())
        self.assertEqual(blocked["state"], "blocked")
        blocked_source = Path(self.temp.name) / "blocked-capture"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), blocked_source, self.hub)
        with self.assertRaisesRegex(RuntimeError, "path is blocked"):
            docsqueue.Queue(self.hub).capture(blocked_source, "blocked D1", ["README.md"])
        with self.assertRaisesRegex(RuntimeError, "path is blocked"):
            docsqueue.Queue(self.hub).capture(blocked_source, "blocked D2", ["docs/dependent.md"])
        with self.assertRaisesRegex(RuntimeError, "only README.md and docs"):
            docsqueue.Queue(self.hub).capture(
                blocked_source, "executable markdown", ["skills/example/SKILL.md"]
            )
        disjoint_copy = Path(self.temp.name) / "disjoint-docs"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), disjoint_copy, self.hub)
        (disjoint_copy / "docs").mkdir(exist_ok=True)
        (disjoint_copy / "docs/disjoint.md").write_text("independent\n")
        independent_id = docsqueue.Queue(self.hub).capture(
            disjoint_copy, "Independent", ["docs/disjoint.md"]
        )
        self.assertEqual(
            json.loads((self.hub / ".pipeline/docs" / f"{independent_id}.json").read_bytes())[
                "state"
            ],
            "published",
        )
        stale = Path(self.temp.name) / "stale-sync"
        pipeline.clone(base, stale, self.hub)
        docsqueue.sync(stale, self.hub)
        self.assertFalse(set(second_row["hashes"]) <= pipeline.hashes(stale))
        resolution = docsqueue.Queue(self.hub).resolve(second, left, record=False, message=None)
        resolution = Path(resolution)
        # CK-151: a resolution that admits the home path or account name is refused whole, before it is recorded
        home = Path(self.temp.name) / "someaccountname"
        (resolution / "README.md").write_text(f"resolved by {home.name}\n")
        with (
            mock.patch.object(Path, "home", return_value=home),
            self.assertRaisesRegex(RuntimeError, r"refused.*README\.md contain"),
        ):
            docsqueue.Queue(self.hub).resolve(second, resolution, record=True, message="Leaky")
        self.assertEqual(list((self.hub / ".pipeline/docs").glob("DRES*.json")), [])
        conflicted = json.loads((self.hub / ".pipeline/docs" / f"{second}.json").read_bytes())
        self.assertEqual(conflicted["state"], "conflict")
        (resolution / "README.md").write_text("resolved docs\n")
        real = docsqueue.home_offenders

        def check_then_swap(contents: Mapping[str, bytes], where: Path) -> list[str]:
            found = real(contents, where)
            (resolution / "README.md").write_text(f"swapped in {home}\n")  # after the check
            return found

        with (
            mock.patch.object(Path, "home", return_value=home),
            mock.patch.object(docsqueue, "home_offenders", check_then_swap),
        ):
            group_id = docsqueue.Queue(self.hub).resolve(
                second, resolution, record=True, message="Resolve README conflict"
            )
        self.assertEqual(
            (resolution / "README.md").read_text(), "resolved docs\n"
        )  # the bytes checked
        group = json.loads((self.hub / ".pipeline/docs" / f"{group_id}.json").read_bytes())
        self.assertEqual(group["state"], "pending")
        self.assertEqual(group["resolves"], second)
        self.assertIn("README.md", group["files"])
        self.assertEqual(
            first,
            next(row["id"] for row in docsqueue._queue(self.hub) if row["state"] == "published"),
        )
        flushed = docsqueue.Queue(self.hub).flush()
        final_rows = {
            path.stem: json.loads(path.read_bytes())
            for path in (self.hub / ".pipeline/docs").glob("*.json")
            if path.name != "counters.json"
        }
        self.assertEqual(flushed, 2, {name: row.get("state") for name, row in final_rows.items()})
        self.assertEqual(
            json.loads((self.hub / ".pipeline/docs/Z-DEPENDENT.json").read_bytes())["state"],
            "published",
        )

    def test_rc3_04b_batch_tests_run_under_the_process_table_guard(self) -> None:
        with self.assertRaises(procfix.RealProcessTableRead):
            subprocess.check_output(["ps", "-axo", "pid=,ppid=,command="])
        with self.assertRaises(procfix.RealProcessTableRead):
            procscan.scan(self.hub, lister=procfix.REAL_PROCESSES)
        self.assertEqual([row["pid"] for row in procscan.processes()], [os.getpid()])

    def test_c25_process_scan_finds_a_legacy_state_writer(self) -> None:
        fake: list[ProcessRow] = [
            {"pid": 2, "ppid": 3, "command": "bootstrap helper"},
            {"pid": 3, "ppid": 1, "command": "/tmp/tools/ck status"},
            {"pid": 7, "ppid": 1, "command": f"python {self.hub}/tools/pipeline.py submit"},
        ]
        found = procscan.scan(self.hub, self_pid=2, lister=lambda: fake)
        self.assertEqual([row["pid"] for row in found], [7])

    def test_c15_failed_combined_candidate_splits_and_quarantines_only_failure(self) -> None:
        self.submission("bad")
        self.submission("good")
        calls = []

        def validate(repo: Path, log: Path, fd: int) -> bool:
            files = pipeline.files(repo)
            calls.append(("bad.txt" in files, "good.txt" in files))
            log.write_text("failed\n" if "bad.txt" in files else "passed\n")
            self.chat_inputs(log)
            return "bad.txt" not in files

        manager = batch.Manager(self.hub, validate)
        bid = manager.prepare()
        validated = json.loads((batch.batch_path(self.hub, bid) / "validated.json").read_bytes())
        self.assertEqual(validated["members"], ["good"])
        self.assertEqual(calls[0], (True, True))
        self.assertTrue((self.hub / ".pipeline/quarantine/bad.json").exists())
        with self.assertRaisesRegex(RuntimeError, "fresh independent integration approval"):
            manager.approve(bid)
        self.integration_receipt(bid, "split-candidate-review")
        manager.approve(bid)
        manager.publish(bid)
        self.assertEqual(pointer.read_pointer(self.hub)["submissions"], ["good"])

    # RC-1: the batch receipt binds the shared chat inputs; publication checks them once.

    def prepared(self, name: str = "bound") -> tuple[batch.Manager, str]:
        self.submission(name)
        manager = batch.Manager(self.hub, self.passing)
        return manager, manager.prepare()

    def validated(self, bid: str) -> BatchValidated:
        path = batch.batch_path(self.hub, bid) / "validated.json"
        return load_batch_validated(json.loads(path.read_bytes()), str(path))

    def ci_receipt(self, bid: str) -> BatchCIReceipt:
        path = Path(self.validated(bid)["ci_receipt"])
        receipt = load_ci_receipt(json.loads(path.read_bytes()), str(path))
        assert is_batch_ci_receipt(receipt)
        return receipt

    def approve(
        self, manager: batch.Manager, bid: str, identifier: str = "rc1-integration"
    ) -> None:
        self.integration_receipt(bid, identifier)
        manager.approve(bid)

    def unbind(self, bid: str) -> None:
        """The receipt of a batch prepared by the previously accepted gate: no chat_inputs."""
        validated = self.validated(bid)
        path = Path(validated["ci_receipt"])
        receipt = json.loads(path.read_bytes())
        del receipt["chat_inputs"]
        pointer.atomic_bytes(path, pointer.canonical(receipt))
        validated["ci_receipt_sha256"] = pointer.file_digest(path)
        pointer.atomic_bytes(
            batch.batch_path(self.hub, bid) / "validated.json", pointer.canonical(validated)
        )

    def drift(self) -> None:
        (self.shared / "chat/scripts/pchat").write_text("#!/usr/bin/env python3\n# drifted\n")

    def crash_at(self, point: str) -> Callable[..., None]:
        def crash(name: str, **_: object) -> None:
            if name == point:
                raise RuntimeError(f"injected {point}")

        return crash

    def test_rc1_05_prepare_embeds_the_accepted_run_record_in_the_receipt(self) -> None:
        _manager, bid = self.prepared()
        record = chatfix.record_for(chatfix.expected_inputs(self.shared))
        receipt = self.ci_receipt(bid)
        self.assertEqual(receipt["chat_inputs"], record)
        self.assertTrue(receipt["passed"])
        validated = self.validated(bid)
        self.assertEqual(
            validated["ci_receipt_sha256"], pointer.file_digest(Path(validated["ci_receipt"]))
        )

    def test_rc1_06_publication_refuses_drift_before_the_intent_and_cancel_still_works(
        self,
    ) -> None:
        manager, bid = self.prepared()
        self.approve(manager, bid)
        before = (self.hub / ".pipeline/accepted-v2.json").read_bytes()
        original = (self.shared / "chat/scripts/pchat").read_bytes()
        self.drift()
        message = "Shared chat inputs changed after batch validation; cancel and prepare again"
        with self.assertRaisesRegex(RuntimeError, message):
            manager.publish(bid)
        self.assertFalse((batch.batch_path(self.hub, bid) / "publishing.json").exists())
        self.assertEqual((self.hub / ".pipeline/accepted-v2.json").read_bytes(), before)
        # The same bytes restored: the same batch publishes.
        (self.shared / "chat/scripts/pchat").write_bytes(original)
        manager.publish(bid)
        self.assertIn("published", manager.status(bid))

    def test_rc1_06a_cancel_after_a_refused_publication_strands_nothing(self) -> None:
        manager, bid = self.prepared("strand")
        self.approve(manager, bid)
        self.drift()
        with self.assertRaisesRegex(RuntimeError, "Shared chat inputs changed"):
            manager.publish(bid)
        manager.cancel(bid, "shared chat inputs changed")
        self.assertIn("cancelled", manager.status(bid))
        self.assertNotIn("strand", pointer.read_pointer(self.hub).get("submissions", []))
        again = batch.Manager(self.hub, self.passing)
        new_bid = again.prepare()
        self.approve(again, new_bid, "rc1-second")
        again.publish(new_bid)
        self.assertEqual(pointer.read_pointer(self.hub)["submissions"], ["strand"])

    def test_rc1_07_publication_refuses_a_passing_receipt_that_binds_nothing(self) -> None:
        manager, bid = self.prepared()
        self.unbind(bid)
        self.approve(manager, bid)
        with self.assertRaisesRegex(
            RuntimeError, "Batch CI receipt does not bind shared chat inputs; prepare again"
        ):
            manager.publish(bid)
        self.assertFalse((batch.batch_path(self.hub, bid) / "publishing.json").exists())

    def test_rc1_11_recovery_after_the_pointer_write_never_checks_shared_inputs(self) -> None:
        # (a) the introducing batch: an unbound receipt, first published by the old controller.
        manager, bid = self.prepared("introducing")
        self.unbind(bid)
        self.approve(manager, bid)
        with (
            mock.patch.object(batch.Manager, "_require_bound"),
            self.assertRaisesRegex(RuntimeError, "injected after_pointer_write"),
        ):
            batch.Manager(self.hub, hook=self.crash_at("after_pointer_write")).publish(bid)
        self.assertIn("introducing", pointer.read_pointer(self.hub)["submissions"])
        self.drift()
        batch.Manager(self.hub).publish(bid)
        self.assertIn("published", manager.status(bid))
        self.assertTrue((batch.batch_path(self.hub, bid) / "published.json").exists())

    def test_rc1_11b_a_bound_batch_recovers_although_the_shared_inputs_changed(self) -> None:
        manager, bid = self.prepared("bound-crash")
        self.approve(manager, bid)
        with self.assertRaisesRegex(RuntimeError, "injected after_pointer_write"):
            batch.Manager(self.hub, hook=self.crash_at("after_pointer_write")).publish(bid)
        self.drift()
        batch.Manager(self.hub).publish(bid)
        intent = json.loads((batch.batch_path(self.hub, bid) / "publishing.json").read_bytes())
        self.assertEqual(
            (self.hub / ".pipeline/accepted-v2.json").read_bytes(), intent["pointer"].encode()
        )
        self.assertTrue((batch.batch_path(self.hub, bid) / "published.json").exists())

    def test_rc1_11c_recovery_completes_after_a_verified_docs_publication(self) -> None:
        manager, bid = self.prepared("docs-crash")
        self.approve(manager, bid)
        with self.assertRaisesRegex(RuntimeError, "injected after_pointer_write"):
            batch.Manager(self.hub, hook=self.crash_at("after_pointer_write")).publish(bid)
        intent = json.loads((batch.batch_path(self.hub, bid) / "publishing.json").read_bytes())
        copy = self.accepted_clone("rc1-docs")
        (copy / "README.md").write_text("docs after the crash\n")
        did = docsqueue.Queue(self.hub).capture(copy, "Docs after the crash", ["README.md"])
        self.assertEqual(self.docs_row(did)["state"], "pending")
        with mock.patch.object(docsqueue, "_active", return_value=None):
            docsqueue.Queue(self.hub).flush()
        current = (self.hub / ".pipeline/accepted-v2.json").read_bytes()
        self.assertNotEqual(current, intent["pointer"].encode())
        self.assertTrue(docsqueue.is_docs_descendant(self.hub, current, intent["pointer"].encode()))
        self.drift()
        batch.Manager(self.hub).publish(bid)
        self.assertTrue((batch.batch_path(self.hub, bid) / "published.json").exists())
        self.assertIn("docs-crash", pointer.read_pointer(self.hub)["submissions"])

    def test_rc1_12_a_written_intent_is_completed_without_the_freshness_check(self) -> None:
        manager, bid = self.prepared("intent-written")
        self.approve(manager, bid)
        with self.assertRaisesRegex(RuntimeError, "injected after_publish_intent"):
            batch.Manager(self.hub, hook=self.crash_at("after_publish_intent")).publish(bid)
        with self.assertRaisesRegex(RuntimeError, "Cannot cancel after publication intent"):
            manager.cancel(bid, "too late")
        self.drift()
        batch.Manager(self.hub).publish(bid)
        self.assertTrue((batch.batch_path(self.hub, bid) / "published.json").exists())
        self.assertIn("intent-written", pointer.read_pointer(self.hub)["submissions"])

    def test_rc1_13_only_the_accepted_candidates_record_is_embedded(self) -> None:
        self.submission("alpha-good")
        self.submission("zeta-bad")
        shared = chatfix.expected_inputs(self.shared)
        seen = []

        def validate(repo: Path, log: Path, fd: int) -> bool:
            files = pipeline.files(repo)
            log.write_text("run\n")
            digest = pointer.canonical([log.name]).hex()[:64].ljust(64, "0")
            self.chat_inputs(log, {**shared, f"chat/ran-{log.stem}": digest})
            seen.append((log.name, "zeta-bad.txt" in files))
            return "zeta-bad.txt" not in files

        bid = batch.Manager(self.hub, validate).prepare()
        # The combined run failed, then alpha-good passed alone, then zeta-bad failed last.
        self.assertEqual([item[0] for item in seen][-1], f"{bid}-split-1.log")
        record = self.ci_receipt(bid)["chat_inputs"]
        self.assertIn(f"chat/ran-{bid}-split-0", record["files"])
        self.assertEqual(self.validated(bid)["members"], ["alpha-good"])
        self.assertTrue((self.hub / ".pipeline/quarantine/zeta-bad.json").exists())
        self.assertFalse((self.hub / ".pipeline/quarantine/alpha-good.json").exists())

    def test_rc1_13a_an_invalid_record_is_a_recording_failure_not_a_patch_failure(self) -> None:
        self.submission("recorded")
        files = chatfix.expected_inputs(self.shared)
        good = chatfix.record_for(files)
        digest = next(iter(files.values()))
        cases = {
            "malformed json": "{not json",
            "not an object": json.dumps([good]),
            "aggregate mismatch": json.dumps({**good, "aggregate": "0" * 64}),
            "non-hex digest": json.dumps(
                chatfix.record_for({**files, "chat/x": "zz" + digest[2:]})
            ),
            "empty files": json.dumps(chatfix.record_for({})),
            "wrong schema": json.dumps({**good, "schema": 2}),
        }
        for label, text in cases.items():
            with self.subTest(label):

                def validate(repo: Path, log: Path, fd: int, text: str = text) -> bool:
                    log.write_text("ok")
                    log.with_suffix(".chat-inputs.json").write_text(text)
                    return True

                with self.assertRaisesRegex(RuntimeError, "chat inputs"):
                    batch.Manager(self.hub, validate).prepare()
                self.assertEqual(list((self.hub / ".pipeline/quarantine").glob("*.json")), [])

    def test_rc1_13b_record_keys_that_are_not_relative_input_paths_fail_the_recording(self) -> None:
        self.submission("keyed")
        files = chatfix.expected_inputs(self.shared)
        digest = next(iter(files.values()))
        for index, key in enumerate(("/outside", "../escape", "chat/../../escape")):
            with self.subTest(key):
                known = {path.name for path in (self.hub / ".pipeline/batches").glob("*/")}

                def validate(repo: Path, log: Path, fd: int, key: str = key) -> bool:
                    log.write_text("ok")
                    chatfix.write_record(
                        log.with_suffix(".chat-inputs.json"), {**files, key: digest}
                    )
                    return True

                def queue_docs(point: str, index: int = index, **_: object) -> None:
                    if point == "after_validating":
                        copy = self.accepted_clone(f"rc1-keyed-docs-{index}")
                        (copy / "README.md").write_text(f"docs during rejected record {index}\n")
                        self.queued = docsqueue.Queue(self.hub).capture(
                            copy, "Queued", ["README.md"]
                        )

                with self.assertRaisesRegex(RuntimeError, "chat inputs"):
                    batch.Manager(self.hub, validate, hook=queue_docs).prepare()
                (directory,) = [
                    path
                    for path in (self.hub / ".pipeline/batches").glob("*/")
                    if path.name not in known
                ]
                failed = json.loads((directory / "failed.json").read_bytes())
                self.assertEqual(failed["reason"], "chat inputs not recorded")
                ci = json.loads((self.hub / ".pipeline/ci" / f"{directory.name}.json").read_bytes())
                self.assertFalse(ci["passed"])
                self.assertEqual(ci["reason"], "chat inputs not recorded")
                self.assertNotIn("chat_inputs", ci)
                quarantine = self.hub / ".pipeline/quarantine"
                self.assertFalse(quarantine.exists() and list(quarantine.glob("*.json")))
                self.assertEqual(self.docs_row(self.queued)["state"], "published")
        self.assertIn("keyed", {item["id"] for item in batch.Manager(self.hub).eligible()})

    def test_rc1_14_a_missing_record_fails_the_batch_but_quarantines_nobody(self) -> None:
        self.submission("unrecorded")

        def silent(repo: Path, log: Path, fd: int) -> bool:
            log.write_text("ok")
            return True

        def queue_docs(point: str, **_: object) -> None:
            if point == "after_validating":
                copy = self.accepted_clone("rc1-queued-docs")
                (copy / "README.md").write_text("docs during a failed batch\n")
                self.queued = docsqueue.Queue(self.hub).capture(copy, "Queued", ["README.md"])

        with self.assertRaisesRegex(RuntimeError, "chat inputs"):
            batch.Manager(self.hub, silent, hook=queue_docs).prepare()
        (directory,) = (self.hub / ".pipeline/batches").glob("*/")
        failed = json.loads((directory / "failed.json").read_bytes())
        self.assertEqual(failed["reason"], "chat inputs not recorded")
        receipt = json.loads((self.hub / ".pipeline/ci" / f"{directory.name}.json").read_bytes())
        self.assertFalse(receipt["passed"])
        self.assertEqual(receipt["reason"], "chat inputs not recorded")
        self.assertNotIn("chat_inputs", receipt)
        quarantine = self.hub / ".pipeline/quarantine"
        self.assertFalse(quarantine.exists() and list(quarantine.glob("*.json")))
        self.assertEqual(self.docs_row(self.queued)["state"], "published")
        self.assertIn("unrecorded", {item["id"] for item in batch.Manager(self.hub).eligible()})
        # Still eligible: the next prepare, with a recording validator, succeeds.
        bid = batch.Manager(self.hub, self.passing).prepare()
        self.assertEqual(self.validated(bid)["members"], ["unrecorded"])

    # ---- S4: infrastructure failures are not patch failures ----

    def test_s4_01_a_check_that_exits_75_is_an_infrastructure_failure_not_a_patch_failure(
        self,
    ) -> None:
        self.submission("first")
        self.submission("second")
        calls = []

        def hung(repo: Path, log: Path, fd: int) -> bool:
            calls.append(log.name)
            log.write_text("step unittest exceeded the watchdog")
            raise batch.InfrastructureFailure("the batch check exited 75")

        def queue_docs(point: str, **_: object) -> None:
            if point == "after_validating":
                copy = self.accepted_clone("s4-queued-docs")
                (copy / "README.md").write_text("docs during an infrastructure failure\n")
                self.queued = docsqueue.Queue(self.hub).capture(copy, "Queued", ["README.md"])

        with self.assertRaisesRegex(RuntimeError, "infrastructure failure"):
            batch.Manager(self.hub, hung, hook=queue_docs).prepare()
        self.assertEqual(len(calls), 1, calls)  # no split: the validator ran once
        (directory,) = (self.hub / ".pipeline/batches").glob("*/")
        failed = json.loads((directory / "failed.json").read_bytes())
        self.assertEqual(failed["reason"], "infrastructure")
        self.assertEqual(failed["exit"], 75)
        quarantine = self.hub / ".pipeline/quarantine"
        self.assertFalse(quarantine.exists() and list(quarantine.glob("*.json")))
        self.assertFalse((self.hub / ".pipeline/ci" / f"{directory.name}.json").exists())
        self.assertEqual(self.docs_row(self.queued)["state"], "published")  # docs drain
        eligible = {item["id"] for item in batch.Manager(self.hub).eligible()}
        self.assertEqual(eligible, {"first", "second"})
        bid = batch.Manager(self.hub, self.passing).prepare()  # the next prepare succeeds
        self.assertEqual(sorted(self.validated(bid)["members"]), ["first", "second"])

    def test_s4_02_the_default_check_maps_exit_codes(self) -> None:
        candidate = self.base.parent / "s4-candidate"
        (candidate / "tools").mkdir(parents=True, exist_ok=True)
        manager = batch.Manager(self.hub)
        read_end, write_end = os.pipe()
        self.addCleanup(os.close, read_end)
        self.addCleanup(os.close, write_end)
        log = self.hub / "s4.log"
        for code, expected in ((0, True), (1, False)):
            (candidate / "tools/check.py").write_text(f"raise SystemExit({code})\n")
            self.assertIs(manager._full_check(candidate, log, write_end), expected)
        (candidate / "tools/check.py").write_text("print('watchdog fired')\nraise SystemExit(75)\n")
        with self.assertRaisesRegex(batch.InfrastructureFailure, "exited 75"):
            manager._full_check(candidate, log, write_end)
        self.assertIn("watchdog fired", log.read_text())  # the log is kept for the failure

    def test_s4_03_a_busy_ci_lock_names_the_batch_and_its_pid(self) -> None:
        self.submission("busy")
        seen = {}

        def contend(point: str, *, bid: str, **_: object) -> None:
            if point != "after_frozen":
                return
            seen["bid"] = bid
            errors = []

            def attempt() -> None:
                try:
                    batch.Manager(self.hub, self.passing).prepare()
                except RuntimeError as error:
                    errors.append(str(error))

            thread = threading.Thread(target=attempt)
            thread.start()
            thread.join()
            seen["error"] = errors[0]

        batch.Manager(self.hub, self.passing, hook=contend).prepare()
        self.assertIn("Already running", seen["error"])
        self.assertIn(f"batch {seen['bid']}", seen["error"])
        self.assertIn(f"pid {os.getpid()}", seen["error"])

    # ---- RC-2: eligibility derives authors from immutable patch provenance ----

    SUBMITTER: ClassVar[Author] = {
        "identity": "codex-submitter",
        "session": "codex-submitter-session",
        "client": "codex",
        "role": "worker",
        "family": "codex",
        "families": ["codex"],
    }

    def new_issue(self, title: str, *, approved: bool) -> str:
        spec = Path(self.temp.name) / f"{title}.md"
        spec.write_text(f"{title} spec\n")
        author: Author = {
            "identity": "spec-author",
            "session": "spec-author-session",
            "family": "codex",
        }
        with mock.patch("gate.meta._authors", return_value=[author]):
            issue_id = self.store.new_issue(title, "task", "P2", spec)
        if approved:
            issue = self.store.show(issue_id)
            assert issue is not None
            self.store.write(
                self.store.repo / "reviews" / f"spec-{issue_id}.json",
                pointer.canonical(
                    complete_receipt(
                        {
                            "id": f"spec-{issue_id}",
                            "kind": "spec",
                            "subject": reviews.subject_for_spec(issue),
                            "verdict": "approved",
                            "reviewer": {
                                "identity": "claude-reviewer",
                                "session": f"spec-reviewer-{issue_id}",
                                "role": "reviewer",
                                "class": "A",
                                "family": "claude",
                                "effort": "high",
                            },
                        }
                    )
                ),
                "Approve spec",
            )
        return issue_id

    def worker_copy(
        self,
        name: str,
        author: Author | None,
        *,
        issues: Iterable[str] = (),
        description: str | None = None,
    ) -> Path:
        """A worker clone of the accepted state with one real recorded patch."""
        base_state = pointer.read_pointer(self.hub)
        worker = Path(self.temp.name) / name
        pipeline.clone(Path(base_state["repo"]), worker, self.hub)
        (worker / f"{name}.txt").write_text(name + "\n")
        self.record_as(worker, name, author, issues, description)
        return worker

    def real_submit(self, copy: Path, issue: str, submitter: Author | None = None) -> ManifestV2:
        args = type("Args", (), {"issue": issue, "spec": "rc2", "supersedes": None, "role": None})()
        with (
            mock.patch("gate.meta._authors", return_value=[submitter or self.SUBMITTER]),
            contextlib.redirect_stdout(io.StringIO()) as output,
        ):
            gate_cli._submit(copy, self.hub, self.store, args)
        queued = self.hub / ".pipeline/queue" / f"{output.getvalue().strip()}.json"
        manifest = load_manifest(json.loads(queued.read_bytes()), str(queued))
        assert is_manifest_v2(manifest)
        return manifest

    def digest_of(self, author: Author, issues: Iterable[str] = ()) -> str:
        return self.description(author, issues)[len(provenance.PREFIX) :].split("\n")[0]

    def new_hashes(self, repo: Path, before: set[str]) -> list[str]:
        return sorted(pipeline.hashes(repo) - before)

    def test_rc2_01_a_raw_patch_submitted_by_a_codex_session_is_unknown_authored(self) -> None:
        issue = self.new_issue("raw", approved=True)
        manifest = self.real_submit(self.worker_copy("raw1", None), issue)
        (digest,) = manifest["hashes"]
        self.assertEqual(manifest["provenance"], {digest: None})
        self.assertEqual(
            manifest["authors"],
            [
                {
                    "identity": "unknown",
                    "family": "unknown",
                    "families": ["unknown"],
                    "hashes": [digest],
                }
            ],
        )
        self.assertEqual(manifest["submitter"], self.SUBMITTER)
        identifier = manifest["id"]
        self.review_as(identifier, "claude", "claude-1")
        self.assertNotIn(identifier, self.eligible_ids())
        self.review_as(identifier, "codex", "codex-1")
        self.assertIn(identifier, self.eligible_ids())

    def test_rc2_02_authors_are_the_recorder_and_the_submitter_cannot_review(self) -> None:
        issue = self.new_issue("recorded", approved=True)
        copy = self.worker_copy("rec", self.CLAUDE, issues=[issue])
        manifest = self.real_submit(copy, issue)
        (author,) = manifest["authors"]
        self.assertEqual(
            (author["identity"], author["session"], author["family"], author["families"]),
            ("claude-worker", "claude-worker-session", "claude", ["claude"]),
        )
        self.assertEqual(manifest["submitter"], self.SUBMITTER)
        (digest,) = manifest["hashes"]
        self.assertEqual(manifest["provenance"], {digest: self.digest_of(self.CLAUDE, [issue])})
        identifier = manifest["id"]
        # A Codex review by the submitter's own session does not count, nor does its identity.
        self.review_as(identifier, "codex", self.SUBMITTER["session"], "someone-else")
        self.review_as(identifier, "codex", "another-session", self.SUBMITTER["identity"])
        self.assertNotIn(identifier, self.eligible_ids())
        self.review_as(identifier, "codex", "independent-codex")
        self.assertIn(identifier, self.eligible_ids())

    def test_rc2_03_invalid_provenance_is_unknown(self) -> None:
        base = set(pointer.read_pointer(self.hub)["fingerprint"]["hashes"])
        worker = Path(self.temp.name) / "invalid"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), worker, self.hub)
        good = self.description(self.CODEX)
        payload = json.loads(good.split("\n", 1)[1])
        cases = {
            "digest mismatch": provenance.PREFIX + "0" * 64 + "\n" + good.split("\n", 1)[1],
            "wrong schema": self.description(self.CODEX, schema="ck-provenance/2"),
            "malformed json": provenance.PREFIX + self.digest_of(self.CODEX) + "\n{not json",
            "not an object": provenance.PREFIX + "0" * 64 + "\n[1, 2]",
            "foreign description": "an ordinary patch description",
        }
        hashes = {}
        for label, description in cases.items():
            (worker / f"{label.replace(' ', '-')}.txt").write_text(label + "\n")
            before = pipeline.hashes(worker)
            self.record_as(worker, label, self.CODEX, description=description)
            (hashes[label],) = self.new_hashes(worker, before)
        (worker / "valid.txt").write_text("valid\n")
        before = pipeline.hashes(worker)
        self.record_as(worker, "valid", self.CODEX, description=good)
        (valid,) = self.new_hashes(worker, before)
        found = provenance.from_capture(worker, [*hashes.values(), valid])
        for label, digest in hashes.items():
            self.assertIsNone(found[digest], label)
        self.assertEqual(found[valid], payload)
        derived = provenance.authors(found)
        unknown = [entry for entry in derived if entry["identity"] == "unknown"]
        self.assertEqual(len(unknown), 1)
        self.assertEqual(unknown[0]["hashes"], sorted(hashes.values()))
        self.assertEqual(reviews.requirements(derived), {"claude", "codex"})
        self.assertTrue(base)
        with self.assertRaisesRegex(RuntimeError, "does not contain"):
            provenance.from_capture(worker, ["NOSUCHHASH" + "A" * 42])

    def test_rc2_04_a_mixed_closure_requires_both_families(self) -> None:
        self.submission("mix", authors=[self.CLAUDE, self.CODEX], review=False)
        self.review_as("mix", "claude", "claude-r")
        self.assertNotIn("mix", self.eligible_ids())
        self.review_as("mix", "codex", "codex-r")
        self.assertIn("mix", self.eligible_ids())

    def test_rc2_05_a_pre_change_manifest_follows_its_patches_not_its_declared_authors(
        self,
    ) -> None:
        # Declared (submitter-derived) authors say Codex; the patch says Claude.
        declared: list[Author] = [
            {"identity": "worker", "session": "worker-session", "family": "codex"}
        ]
        self.submission(
            "pre",
            authors=[self.CLAUDE],
            declared=declared,
            slots=lambda hashes: dict.fromkeys(hashes),
        )
        self.assertNotIn("pre", self.eligible_ids())  # a Claude review cannot cover Claude
        self.review_as("pre", "codex", "codex-pre")
        self.assertIn("pre", self.eligible_ids())

    def test_rc2_06_a_mismatched_non_null_digest_makes_the_submission_ineligible(self) -> None:
        self.submission("slot", authors=[self.CODEX], slots=lambda h: dict.fromkeys(h, "0" * 64))
        self.assertNotIn("slot", self.eligible_ids())
        good = self.digest_of(self.CODEX)
        self.submission("slot-ok", authors=[self.CODEX], slots=lambda h: dict.fromkeys(h, good))
        self.assertIn("slot-ok", self.eligible_ids())

    def test_rc2_07_every_issue_a_payload_names_must_be_ready_and_is_bound(self) -> None:
        first = self.new_issue("first", approved=True)
        unready = self.new_issue("unready", approved=False)
        second = self.new_issue("second", approved=True)

        def refuse(copy: Path, issue: str) -> None:
            queue = sorted(path.name for path in (self.hub / ".pipeline").glob("queue/*.json"))
            captures = sorted(path.name for path in (self.hub / ".pipeline").glob("submissions/*"))
            with self.assertRaisesRegex(RuntimeError, "Every named issue must be open"):
                self.real_submit(copy, issue)
            self.assertEqual(
                queue, sorted(path.name for path in (self.hub / ".pipeline").glob("queue/*.json"))
            )
            self.assertEqual(
                captures,
                sorted(path.name for path in (self.hub / ".pipeline").glob("submissions/*")),
            )

        refuse(self.worker_copy("both", self.CODEX, issues=[first, unready]), first)
        refuse(self.worker_copy("only", self.CODEX, issues=[unready]), first)
        manifest = self.real_submit(
            self.worker_copy("ok", self.CODEX, issues=[first, second]), first
        )
        bound = {row["id"]: row for row in manifest["issues"]}
        self.assertEqual(set(bound), {first, second})
        for identifier, row in bound.items():
            shown = self.store.show(identifier)
            assert shown is not None
            self.assertEqual(
                (row["revision"], row["spec_sha256"]), (shown["revision"], shown["spec_sha256"])
            )

    def batch_receipt(self, bid: str, family: str, session: str, identity: str, rid: str) -> None:
        self.store.write(
            self.store.repo / "reviews" / f"{rid}.json",
            pointer.canonical(
                complete_receipt(
                    {
                        "schema": "ck-review/1",
                        "id": rid,
                        "kind": "batch",
                        "subject": batch.subject(self.hub, bid),
                        "verdict": "approved",
                        "reviewer": {
                            "identity": identity,
                            "session": session,
                            "role": "reviewer",
                            "class": "A",
                            "family": family,
                            "effort": "high",
                        },
                    }
                )
            ),
            "Review batch",
        )

    def staged_batch(self, name: str, *, legacy: bool = False) -> tuple[batch.Manager, str]:
        """A prepared batch for one Claude-recorded, Codex-reviewed submission."""
        self.submission(name, authors=[self.CLAUDE], submitter=self.SUBMITTER, review=False)
        self.review_as(name, "codex", f"codex-{name}", rid=f"member-{name}")
        manager = batch.Manager(self.hub, self.passing)
        bid = manager.prepare()
        if legacy:
            path = batch.batch_path(self.hub, bid) / "validated.json"
            validated = json.loads(path.read_bytes())
            del validated["submitters"]
            pointer.atomic_bytes(path, pointer.canonical(validated))
        return manager, bid

    def test_rc2_08_prepare_records_recomputed_authors_and_submitters(self) -> None:
        manager, bid = self.staged_batch("batched")
        validated = self.validated(bid)
        (author,) = validated["authors"]
        self.assertEqual((author["identity"], author["family"]), ("claude-worker", "claude"))
        self.assertEqual(validated["submitters"], [self.SUBMITTER])
        # The submitter cannot supply the integration review, whatever its family.
        self.batch_receipt(
            bid, "codex", self.SUBMITTER["session"], self.SUBMITTER["identity"], "by-submitter"
        )
        with self.assertRaisesRegex(RuntimeError, "does not have fresh independent"):
            manager.approve(bid)
        self.batch_receipt(bid, "codex", "integration-codex", "integration-codex-id", "indep")
        manager.approve(bid)

    def test_rc2_08a_a_revocation_after_a_crash_at_the_intent_stops_publication(self) -> None:
        manager, bid = self.staged_batch("state-a")
        self.batch_receipt(bid, "codex", "integration-a", "integration-a-id", "int-a")
        manager.approve(bid)
        with self.assertRaisesRegex(RuntimeError, "injected after_publish_intent"):
            batch.Manager(self.hub, hook=self.crash_at("after_publish_intent")).publish(bid)
        self.store.write(
            self.store.repo / "events/revoke-state-a.json",
            pointer.canonical(
                {"kind": "revocation", "review": "member-state-a", "reason": "after the crash"}
            ),
            "Revoke member review",
        )
        with self.assertRaisesRegex(RuntimeError, "no longer eligible"):
            batch.Manager(self.hub).publish(bid)
        self.assertNotIn("published", manager.status(bid))

    def test_rc2_08b_completion_after_the_pointer_write_does_not_rerun_eligibility(self) -> None:
        manager, bid = self.staged_batch("state-b", legacy=True)
        self.assertNotIn("submitters", self.validated(bid))
        self.batch_receipt(bid, "codex", "integration-b", "integration-b-id", "int-b")
        manager.approve(bid)
        with self.assertRaisesRegex(RuntimeError, "injected after_pointer_write"):
            batch.Manager(self.hub, hook=self.crash_at("after_pointer_write")).publish(bid)
        self.store.write(
            self.store.repo / "events/revoke-state-b.json",
            pointer.canonical(
                {"kind": "revocation", "review": "member-state-b", "reason": "after the pointer"}
            ),
            "Revoke member review",
        )
        batch.Manager(self.hub).publish(bid)
        self.assertTrue((batch.batch_path(self.hub, bid) / "published.json").exists())

    def test_rc2_08c_completion_after_a_docs_publication_does_not_rerun_eligibility(self) -> None:
        manager, bid = self.staged_batch("state-c", legacy=True)
        self.batch_receipt(bid, "codex", "integration-c", "integration-c-id", "int-c")
        manager.approve(bid)
        with self.assertRaisesRegex(RuntimeError, "injected after_pointer_write"):
            batch.Manager(self.hub, hook=self.crash_at("after_pointer_write")).publish(bid)
        copy = self.accepted_clone("rc2-docs")
        (copy / "README.md").write_text("docs after the crash\n")
        docsqueue.Queue(self.hub).capture(copy, "Docs", ["README.md"])
        with mock.patch.object(docsqueue, "_active", return_value=None):
            docsqueue.Queue(self.hub).flush()
        self.store.write(
            self.store.repo / "events/revoke-state-c.json",
            pointer.canonical({"kind": "revocation", "review": "member-state-c", "reason": "x"}),
            "Revoke member review",
        )
        batch.Manager(self.hub).publish(bid)
        self.assertTrue((batch.batch_path(self.hub, bid) / "published.json").exists())

    def test_rc2_10_an_incremental_parent_is_covered_by_its_recomputed_authors(self) -> None:
        parent = self.submission(
            "parent",
            raw=True,
            review=False,
            declared=[{"identity": "worker", "session": "worker-session", "family": "codex"}],
        )
        self.submission("child", authors=[self.CODEX], parent=parent, review=False)
        self.review_as("parent", "claude", "claude-parent")
        self.review_as("child", "claude", "claude-child", over="parent")
        self.review_as("child", "codex", "codex-child", over="parent")
        # The parent claims Codex but its raw patch is unknown: one Claude review is not enough.
        self.assertEqual(self.eligible_ids(), set())
        self.review_as("parent", "codex", "codex-parent")
        self.assertEqual(self.eligible_ids(), {"parent", "child"})

    def test_rc2_11_patches_by_one_session_merge_with_the_union_of_their_families(self) -> None:
        def payload(family: str, families: list[str] | None = None) -> ProvenancePayload:
            author: Author = {**self.CODEX, "family": family}
            text = (
                self.description(author)
                if families is None
                else self.description(author, families=families)
            )
            return load_provenance(provenance.parse_description(text), "fixture provenance")

        merged = provenance.authors(
            {
                "H1": payload("codex"),
                "H2": payload("claude", families=["claude"]),
                "H3": payload("codex"),
            }
        )
        (entry,) = merged
        self.assertEqual(
            (entry["identity"], entry["session"], entry["family"]),
            ("worker", "worker-session", "unknown"),
        )
        self.assertEqual(entry["families"], ["claude", "codex"])
        self.assertEqual(entry["hashes"], ["H1", "H2", "H3"])
        self.assertEqual(reviews.requirements(merged), {"claude", "codex"})
        same = provenance.authors({"H1": payload("codex"), "H2": payload("codex")})
        self.assertEqual(
            (len(same), same[0]["family"], same[0]["hashes"]), (1, "codex", ["H1", "H2"])
        )

    def test_rc2_12_a_malformed_payload_is_unknown_and_never_breaks_a_scan(self) -> None:
        worker = Path(self.temp.name) / "malformed-patches"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), worker, self.hub)
        bad = {
            "missing session": self.description(self.CODEX, session=None),
            "empty families": self.description(self.CODEX, families=[]),
            "unknown family": self.description(self.CODEX, families=["gemini"]),
            "non-string identity": self.description(self.CODEX, identity=5),
            "missing client": self.description(self.CODEX, client=None),
        }
        hashes = []
        for label, description in bad.items():
            (worker / f"{label.replace(' ', '-')}.txt").write_text(label + "\n")
            before = pipeline.hashes(worker)
            self.record_as(worker, label, self.CODEX, description=description)
            hashes.extend(self.new_hashes(worker, before))
        self.assertEqual(set(provenance.from_capture(worker, hashes).values()), {None})
        self.submission("malformed", descriptions={0: bad["missing session"]})
        self.submission("well-formed", authors=[self.CLAUDE], review=False)
        self.review_as("well-formed", "codex", "codex-wf")
        self.assertEqual(self.eligible_ids(), {"well-formed"})

    def test_rc2_13_the_submitter_cannot_review_its_own_submission(self) -> None:
        manifest = self.submission(
            "own", authors=[self.CLAUDE], submitter=self.SUBMITTER, review=False
        )
        report = Path(self.temp.name) / "report.md"
        report.write_text("report\n")

        def reviewer(identity: str, session: str) -> tuple[dict[str, object], dict[str, object]]:
            return (
                {
                    "identity": identity,
                    "client": "codex",
                    "session": session,
                    "role": "reviewer",
                    "class": "A",
                    "family": "codex",
                    "model": "gpt-6.1-sol",
                    "effort": "high",
                },
                {"evidence": {"path": "/fixture", "offset": 0, "sha256": "0" * 64}},
            )

        args = type(
            "Args",
            (),
            {
                "kind": "submission",
                "subject": manifest["id"],
                "verdict": "approved",
                "report": report,
                "incremental_over": None,
                "role": None,
                "blame": None,
            },
        )()
        for identity, session in (
            (self.SUBMITTER["identity"], "fresh-session"),
            ("fresh-identity", self.SUBMITTER["session"]),
        ):
            with (
                mock.patch.object(reviews, "_reviewer", return_value=reviewer(identity, session)),
                self.assertRaisesRegex(RuntimeError, "submitter"),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                gate_cli._review(self.hub, self.store, args)
        with (
            mock.patch.object(reviews, "_reviewer", return_value=reviewer("indep", "indep-s")),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            gate_cli._review(self.hub, self.store, args)
        self.assertIn("own", self.eligible_ids())

    def test_c18_publish_refuses_a_recorded_candidate_mutation(self) -> None:
        self.submission("candidate-change")
        manager = batch.Manager(self.hub, self.passing)
        bid = manager.prepare()
        self.integration_receipt(bid)
        manager.approve(bid)
        before = (self.hub / ".pipeline/accepted-v2.json").read_bytes()
        candidate = Path(
            json.loads((batch.batch_path(self.hub, bid) / "validated.json").read_bytes())[
                "candidate"
            ]
        )
        (candidate / "source.txt").write_text("tampered candidate\n")
        self.record(candidate, "Tamper candidate")
        with self.assertRaisesRegex(RuntimeError, "candidate fingerprint changed"):
            manager.publish(bid)
        self.assertEqual((self.hub / ".pipeline/accepted-v2.json").read_bytes(), before)

    def test_c18_publish_refuses_base_advance_after_review(self) -> None:
        self.submission("base-advance")
        manager = batch.Manager(self.hub, self.passing)
        bid = manager.prepare()
        self.integration_receipt(bid, "base-advance-integration")
        manager.approve(bid)
        docs_source = Path(self.temp.name) / "docs-before-publish"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), docs_source, self.hub)
        (docs_source / "README.md").write_text("advance base\n")
        did = docsqueue.Queue(self.hub).capture(docs_source, "advance", ["README.md"])
        pointer.atomic_bytes(
            batch.batch_path(self.hub, bid) / "cancelled.json",
            pointer.canonical({"reason": "advance fixture"}),
        )
        self.assertEqual(docsqueue.Queue(self.hub).flush(), 1)
        self.assertEqual(
            json.loads((self.hub / ".pipeline/docs" / f"{did}.json").read_bytes())["state"],
            "published",
        )
        with self.assertRaisesRegex(RuntimeError, "base advanced"):
            manager.publish(bid)

    def test_c18_publish_refuses_revoked_integration_receipt(self) -> None:
        self.submission("revoked-integration")
        manager = batch.Manager(self.hub, self.passing)
        bid = manager.prepare()
        self.integration_receipt(bid, "integration-to-revoke")
        manager.approve(bid)
        self.store.write(
            self.store.repo / "events/revoke-integration.json",
            pointer.canonical(
                {"kind": "revocation", "review": "integration-to-revoke", "reason": "fixture"}
            ),
            "Revoke integration fixture",
        )
        with self.assertRaisesRegex(RuntimeError, "fresh independent integration review"):
            manager.publish(bid)

    def test_c18_spec_revision_after_approval_makes_member_ineligible(self) -> None:
        spec = Path(self.temp.name) / "issue-spec.md"
        spec.write_text("spec r1\n")
        with mock.patch(
            "gate.meta._authors",
            return_value=[{"identity": "author", "session": "author-session", "family": "codex"}],
        ):
            issue_id = self.store.new_issue("Spec issue", "task", "P2", spec)
        issue = self.store.show(issue_id)
        assert issue is not None
        self.store.write(
            self.store.repo / "reviews/spec-approval.json",
            pointer.canonical(
                complete_receipt(
                    {
                        "id": "spec-approval",
                        "kind": "spec",
                        "subject": reviews.subject_for_spec(issue),
                        "verdict": "approved",
                        "reviewer": {
                            "identity": "claude-reviewer",
                            "session": "spec-reviewer",
                            "role": "reviewer",
                            "class": "A",
                            "family": "claude",
                            "effort": "high",
                        },
                    }
                )
            ),
            "Approve spec fixture",
        )
        manifest = self.submission("spec-bound")
        manifest["issues"] = [
            {"id": issue_id, "revision": issue["revision"], "spec_sha256": issue["spec_sha256"]}
        ]
        manifest_path = self.hub / ".pipeline/queue/spec-bound.json"
        pointer.atomic_bytes(manifest_path, pointer.canonical(manifest))
        subject = reviews.subject_for_submission(manifest_path, manifest)
        self.store.write(
            self.store.repo / "reviews/spec-bound.json",
            pointer.canonical(
                complete_receipt(
                    {
                        "id": "spec-bound-review",
                        "kind": "submission",
                        "subject": subject,
                        "verdict": "approved",
                        "reviewer": {
                            "identity": "claude-reviewer",
                            "session": "submission-reviewer",
                            "role": "reviewer",
                            "class": "A",
                            "family": "claude",
                            "effort": "high",
                        },
                    }
                )
            ),
            "Review spec-bound submission",
        )
        manager = batch.Manager(self.hub, self.passing)
        bid = manager.prepare()
        self.integration_receipt(bid, "spec-bound-integration")
        manager.approve(bid)
        revised = Path(self.temp.name) / "issue-spec-r2.md"
        revised.write_text("spec r2\n")
        self.store.append_issue_event(issue_id, "spec", {}, revised.read_bytes())
        with self.assertRaisesRegex(RuntimeError, "no longer eligible"):
            manager.publish(bid)

    def test_c18_spec_edit_waits_for_publish_locks_then_invalidates_next_member(self) -> None:
        spec = Path(self.temp.name) / "locked-spec.md"
        spec.write_text("locked spec r1\n")
        with mock.patch(
            "gate.meta._authors",
            return_value=[{"identity": "author", "session": "author-session", "family": "codex"}],
        ):
            issue_id = self.store.new_issue("Locked issue", "task", "P2", spec)
        issue = self.store.show(issue_id)
        assert issue is not None
        self.store.write(
            self.store.repo / "reviews/locked-spec.json",
            pointer.canonical(
                complete_receipt(
                    {
                        "id": "locked-spec",
                        "kind": "spec",
                        "subject": reviews.subject_for_spec(issue),
                        "verdict": "approved",
                        "reviewer": {
                            "identity": "claude-reviewer",
                            "session": "locked-spec-reviewer",
                            "role": "reviewer",
                            "class": "A",
                            "family": "claude",
                            "effort": "high",
                        },
                    }
                )
            ),
            "Approve locked spec",
        )

        def bound_submission(name: str) -> ManifestV2:
            manifest = self.submission(name)
            manifest["issues"] = [
                {"id": issue_id, "revision": issue["revision"], "spec_sha256": issue["spec_sha256"]}
            ]
            path = self.hub / ".pipeline/queue" / f"{name}.json"
            pointer.atomic_bytes(path, pointer.canonical(manifest))
            self.store.write(
                self.store.repo / "reviews" / f"{name}-bound.json",
                pointer.canonical(
                    complete_receipt(
                        {
                            "id": f"{name}-bound",
                            "kind": "submission",
                            "subject": reviews.subject_for_submission(path, manifest),
                            "verdict": "approved",
                            "reviewer": {
                                "identity": "claude-reviewer",
                                "session": f"{name}-reviewer",
                                "role": "reviewer",
                                "class": "A",
                                "family": "claude",
                                "effort": "high",
                            },
                        }
                    )
                ),
                f"Review {name}",
            )
            return manifest

        bound_submission("locked-first")
        manager = batch.Manager(self.hub, self.passing)
        bid = manager.prepare()
        self.integration_receipt(bid, "locked-integration")
        manager.approve(bid)
        second = bound_submission("locked-second")
        edit_started = threading.Event()
        edit_done = threading.Event()
        edit_errors: list[Exception] = []
        editor_thread: dict[str, threading.Thread] = {}
        original_meta_lock = locks.meta

        def observed_meta_lock(repo: Path) -> object:
            if threading.current_thread() is editor_thread.get("thread"):
                edit_started.set()
            return original_meta_lock(repo)

        revised = Path(self.temp.name) / "locked-spec-r2.md"
        revised.write_text("locked spec r2\n")

        def edit_spec() -> None:
            try:
                self.store.append_issue_event(issue_id, "spec", {}, revised.read_bytes())
            except Exception as error:  # pragma: no cover - surfaced below
                edit_errors.append(error)
            finally:
                edit_done.set()

        def publish_hook(point: str, **_: object) -> None:
            if point != "before_publish_intent":
                return
            editor_thread["thread"] = threading.Thread(target=edit_spec)
            editor_thread["thread"].start()
            self.assertTrue(edit_started.wait(2))
            self.assertFalse(edit_done.is_set())

        with mock.patch.object(locks, "meta", observed_meta_lock):
            batch.Manager(self.hub, hook=publish_hook).publish(bid)
        editor_thread["thread"].join()
        self.assertEqual(edit_errors, [])
        self.assertTrue(edit_done.is_set())
        self.assertNotIn(second["id"], {item["id"] for item in manager.eligible()})

    def test_c17_rejection_quarantines_only_blame_and_drains_queued_docs(self) -> None:
        self.submission("blamed")
        self.submission("unblamed")
        manager = batch.Manager(self.hub, self.passing)
        bid = manager.prepare()
        source = Path(self.temp.name) / "rejection-docs"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), source, self.hub)
        (source / "README.md").write_text("docs survive rejection\n")
        did = docsqueue.Queue(self.hub).capture(source, "Docs during rejected batch", ["README.md"])
        manager.reject(bid, ["blamed"])
        self.assertTrue((self.hub / ".pipeline/quarantine/blamed.json").exists())
        self.assertFalse((self.hub / ".pipeline/quarantine/unblamed.json").exists())
        row = json.loads((self.hub / ".pipeline/docs" / f"{did}.json").read_bytes())
        self.assertEqual(row["state"], "published")
        accepted = pointer.read_pointer(self.hub)
        self.assertIn("README.md", accepted["fingerprint"]["files"])
        self.assertNotIn("blamed", accepted["submissions"])

    # G1: queued documentation reaches synced copies and never enters a code closure.

    def hold(self, name: str = "G1-HOLD") -> Path:
        directory = self.hub / ".pipeline/batches" / name
        directory.mkdir(parents=True)
        pointer.atomic_bytes(directory / "frozen.json", b"{}")
        return directory

    def accepted_clone(self, name: str) -> Path:
        copy = Path(self.temp.name) / name
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), copy, self.hub)
        return copy

    def docs_row(self, identifier: str) -> DocsEntry:
        path = self.hub / ".pipeline/docs" / f"{identifier}.json"
        return load_docs_entry(json.loads(path.read_bytes()), str(path))

    def queue_docs(self, name: str, files: Mapping[str, str], source: Path | None = None) -> str:
        copy = source or self.accepted_clone(name)
        for name_, text in files.items():
            (copy / name_).parent.mkdir(parents=True, exist_ok=True)
            (copy / name_).write_text(text)
        return docsqueue.Queue(self.hub).capture(copy, name, list(files))

    def submit_closure(self, copy: Path) -> ManifestV2:
        args = type("Args", (), {"issue": "CK-1", "spec": "g1", "supersedes": None, "role": None})()
        row = {"id": "CK-1", "revision": "r1", "spec_sha256": "g1"}
        with (
            mock.patch.object(meta.Store, "show", return_value=row),
            mock.patch.object(meta, "ready", return_value=True),
            contextlib.redirect_stdout(io.StringIO()) as output,
        ):
            gate_cli._submit(copy, self.hub, self.store, args)
        queued = self.hub / ".pipeline/queue" / f"{output.getvalue().strip()}.json"
        manifest = load_manifest(json.loads(queued.read_bytes()), str(queued))
        assert is_manifest_v2(manifest)
        return manifest

    def refused_submit(self, copy: Path, pattern: str) -> None:
        before = sorted(path.name for path in (self.hub / ".pipeline").glob("queue/*.json"))
        captures = sorted(path.name for path in (self.hub / ".pipeline").glob("submissions/*"))
        with self.assertRaisesRegex(RuntimeError, pattern):
            self.submit_closure(copy)
        self.assertEqual(
            before, sorted(path.name for path in (self.hub / ".pipeline").glob("queue/*.json"))
        )
        self.assertEqual(
            captures, sorted(path.name for path in (self.hub / ".pipeline").glob("submissions/*"))
        )

    def frozen_candidate(self, manifest: ManifestV2) -> Path:
        state = pointer.read_pointer(self.hub)
        manager = batch.Manager(self.hub, self.passing)
        passing, failures, _ = manager._validate_groups(
            "G1",
            Path(state["repo"]),
            [manifest],
            False,
            None,  # type: ignore[arg-type]  # the validator here never uses the CI descriptor
        )
        self.assertEqual((len(passing), failures), (1, []))
        return Path(manager._last_candidate)

    def test_g1_sync_delivers_pending_docs_and_followup_capture_queues_behind_them(self) -> None:
        self.hold()
        first = self.queue_docs("g1-first", {"README.md": "first\n"})
        row = self.docs_row(first)
        self.assertEqual(row["state"], "pending")
        reader = self.accepted_clone("g1-reader")
        docsqueue.sync(reader, self.hub)
        docsqueue.sync(reader, self.hub)
        self.assertTrue(set(row["hashes"]) <= pipeline.hashes(reader))
        self.assertEqual((reader / "README.md").read_text(), "first\n")
        pipeline.clean(reader)
        (reader / "README.md").write_text("first\nsecond\n")
        second = docsqueue.Queue(self.hub).capture(reader, "second", ["README.md"])
        followup = self.docs_row(second)
        self.assertEqual(followup["state"], "pending")
        self.assertTrue(set(row["hashes"]) < set(followup["hashes"]))

    def test_g1_sync_skips_conflict_and_blocked_entries_but_keeps_what_a_copy_has(self) -> None:
        base_lines = self.queue_docs("g1-lines", {"README.md": "one\ntwo\nthree\n"})
        self.assertEqual(self.docs_row(base_lines)["state"], "published")
        hold = self.hold("G1-CONFLICT-HOLD")
        accepted_hashes = set(pointer.read_pointer(self.hub)["fingerprint"]["hashes"])
        winner = self.queue_docs("g1-winner", {"README.md": "winner\ntwo\nthree\n"})
        rivals: dict[str, DocsEntry] = {}
        for name, text in (
            ("Z1-CONFLICT", "rival\ntwo\nthree\n"),
            ("Z2-BLOCKED", "one\ntwo\nblocked\n"),
        ):
            repo = self.hub / ".pipeline/docs/captures" / name
            pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), repo, self.hub)
            (repo / "README.md").write_text(text)
            self.record(repo, name)
            rivals[name] = {
                "id": name,
                "state": "pending",
                "created": 1,
                "repo": str(repo),
                "hashes": sorted(pipeline.hashes(repo) - accepted_hashes),
                "files": ["README.md"],
                "fingerprint": pipeline.fingerprint(repo),
                "message": name,
            }
            pointer.atomic_bytes(
                self.hub / ".pipeline/docs" / f"{name}.json", pointer.canonical(rivals[name])
            )
        pointer.atomic_bytes(hold / "published.json", b"{}")
        self.assertEqual(docsqueue.Queue(self.hub).flush(), 1)
        self.assertEqual(self.docs_row(winner)["state"], "published")
        self.assertEqual(self.docs_row("Z1-CONFLICT")["state"], "conflict")
        self.assertEqual(self.docs_row("Z2-BLOCKED")["state"], "blocked")
        self.hold("G1-LIVE-HOLD")
        live = self.queue_docs("g1-live", {"docs/live.md": "live\n"})
        live_hashes = set(self.docs_row(live)["hashes"])

        fresh = self.accepted_clone("g1-fresh")
        docsqueue.sync(fresh, self.hub)
        have = pipeline.hashes(fresh)
        self.assertTrue(live_hashes <= have)
        self.assertFalse(set(rivals["Z1-CONFLICT"]["hashes"]) & have)
        self.assertFalse(set(rivals["Z2-BLOCKED"]["hashes"]) & have)
        pipeline.clean(fresh)

        holder = Path(self.temp.name) / "g1-holder"
        pipeline.clone(Path(rivals["Z2-BLOCKED"]["repo"]), holder, self.hub)
        docsqueue.sync(holder, self.hub)
        have = pipeline.hashes(holder)
        self.assertTrue(set(rivals["Z2-BLOCKED"]["hashes"]) <= have)
        self.assertTrue(live_hashes <= have)
        self.assertFalse(set(rivals["Z1-CONFLICT"]["hashes"]) & have)
        with self.assertRaisesRegex(RuntimeError, "ck docs resolve"):
            docsqueue.Queue(self.hub).capture(fresh, "blocked", ["README.md"])

    def test_g1_code_submission_excludes_queued_docs_and_refuses_dependencies(self) -> None:
        self.queue_docs("g1-seed", {"docs/seed.md": "seed\n"})
        self.hold()
        queued = self.queue_docs("g1-queued", {"README.md": "queued\n", "docs/sub/guide.md": "g\n"})
        queued_hashes = set(self.docs_row(queued)["hashes"])

        worker = self.accepted_clone("g1-code")
        docsqueue.sync(worker, self.hub)
        self.assertTrue(queued_hashes <= pipeline.hashes(worker))
        (worker / "code.txt").write_text("code\n")
        self.record(worker, "code only")
        manifest = self.submit_closure(worker)
        self.assertFalse(set(manifest["hashes"]) & queued_hashes)
        self.assertEqual(len(manifest["hashes"]), 1)
        self.assertEqual(manifest["files"], ["code.txt"])
        candidate = self.frozen_candidate(manifest)
        self.assertFalse(queued_hashes & pipeline.hashes(candidate))
        self.assertEqual((candidate / "README.md").read_text(), "base docs\n")
        self.assertFalse((candidate / "docs/sub").exists())

        dependent = self.accepted_clone("g1-dependent")
        docsqueue.sync(dependent, self.hub)
        (dependent / "docs/sub/helper.rs").write_text("fn main() {}\n")
        self.record(dependent, "code inside a queued directory")
        self.refused_submit(dependent, "depends on queued documentation")

        touching = self.accepted_clone("g1-touching")
        docsqueue.sync(touching, self.hub)
        (touching / "docs/new.md").write_text("new\n")
        self.record(touching, "ordinary docs in a closure")
        self.refused_submit(touching, "Ordinary documentation must use ck docs")

        only_docs = self.accepted_clone("g1-only-docs")
        docsqueue.sync(only_docs, self.hub)
        self.refused_submit(only_docs, "No new recorded changes")

    def test_g1_closure_replay_refuses_a_directory_renamed_to_docs_and_back(self) -> None:
        copy = self.accepted_clone("g1-rename-cycle")
        (copy / "assets").mkdir()
        (copy / "assets/guide.md").write_text("guide\n")
        self.record(copy, "add guide outside docs")
        pipeline.pijul(copy, "move", "assets", "docs")
        self.assertTrue(pipeline.doc_path("docs/guide.md") and (copy / "docs/guide.md").exists())
        self.record(copy, "rename assets to docs")
        pipeline.pijul(copy, "move", "docs", "assets")
        self.record(copy, "rename docs back")
        self.assertFalse((copy / "docs").exists())
        self.refused_submit(copy, "Ordinary documentation must use ck docs")

    def test_g1_closure_replay_refuses_directory_moves_through_docs(self) -> None:
        self.queue_docs("g1-seed", {"docs/seed.md": "seed\n"})
        into = self.accepted_clone("g1-move-in")
        (into / "assets").mkdir()
        (into / "assets/guide.md").write_text("guide\n")
        self.record(into, "add guide outside docs")
        pipeline.pijul(into, "move", "assets", "docs/assets")
        self.record(into, "move guide into docs")
        pipeline.pijul(into, "move", "docs/assets", "assets")
        self.record(into, "move guide back out")
        self.assertFalse((into / "docs/assets").exists())
        self.refused_submit(into, "Ordinary documentation must use ck docs")

        cycle = self.accepted_clone("g1-move-cycle")
        (cycle / "assets").mkdir()
        (cycle / "assets/guide.md").write_text("guide\n")
        self.record(cycle, "add guide outside docs")
        pipeline.pijul(cycle, "move", "assets/guide.md", "docs/guide.md")
        self.record(cycle, "move guide into docs")
        pipeline.pijul(cycle, "move", "docs/guide.md", "assets/guide.md")
        self.record(cycle, "move guide back out")
        self.refused_submit(cycle, "Ordinary documentation must use ck docs")

        out = self.accepted_clone("g1-move-out")
        pipeline.pijul(out, "move", "docs/seed.md", "assets-seed.md")
        self.record(out, "move docs out")
        self.refused_submit(out, "Ordinary documentation must use ck docs")

        reverted = self.accepted_clone("g1-readme-revert")
        (reverted / "README.md").write_text("edited\n")
        self.record(reverted, "edit README")
        (reverted / "README.md").write_text("base docs\n")
        self.record(reverted, "revert README")
        self.refused_submit(reverted, "Ordinary documentation must use ck docs")

        legitimate = self.accepted_clone("g1-move-code")
        (legitimate / "src").mkdir()
        (legitimate / "src/a.rs").write_text("a\n")
        self.record(legitimate, "add code")
        pipeline.pijul(legitimate, "move", "src", "lib")
        self.record(legitimate, "move code")
        manifest = self.submit_closure(legitimate)
        self.assertEqual(manifest["files"], ["lib/a.rs"])

    def test_g1_closure_replay_refuses_a_permission_only_docs_change(self) -> None:
        alone = self.accepted_clone("g1-chmod-docs")
        (alone / "README.md").chmod(0o755)
        self.record(alone, "make README executable")
        self.refused_submit(alone, "Ordinary documentation must use ck docs")

    def test_g1_closure_replay_refuses_a_docs_executable_bit_set_and_cleared(self) -> None:
        cycle = self.accepted_clone("g1-chmod-cycle")
        (cycle / "README.md").chmod(0o755)
        self.record(cycle, "make README executable")
        (cycle / "README.md").chmod(0o644)
        self.record(cycle, "make README plain again")
        (cycle / "code.txt").write_text("code\n")
        self.record(cycle, "add code")
        self.refused_submit(cycle, "Ordinary documentation must use ck docs")

    def test_g1_closure_replay_allows_a_permission_change_to_code(self) -> None:
        code = self.accepted_clone("g1-chmod-code")
        (code / "code.txt").write_text("code\n")
        self.record(code, "add code")
        (code / "code.txt").chmod(0o755)
        self.record(code, "make code executable")
        manifest = self.submit_closure(code)
        self.assertEqual(manifest["files"], ["code.txt"])

    def test_g1_partially_published_prefix_keeps_accepted_prerequisites_legitimate(self) -> None:
        hold = self.hold()
        first = self.queue_docs("g1-d1", {"docs/guide.md": "d1\n"})
        first_row = self.docs_row(first)
        source = self.accepted_clone("g1-d2")
        pipeline.pull(source, Path(first_row["repo"]), first_row["hashes"])
        second = self.queue_docs(
            "g1-d2", {"docs/guide.md": "d2\n", "docs/sub/page.md": "p\n"}, source
        )
        pointer.atomic_bytes(hold / "published.json", b"{}")

        def interrupt(point: str, *, entry: DocsEntry, **_: object) -> None:
            if point == "after_ack" and entry["id"] == first:
                raise RuntimeError("interrupted after D1")

        with self.assertRaisesRegex(RuntimeError, "interrupted after D1"):
            docsqueue.Queue(self.hub, hook=interrupt).flush()
        accepted = set(pointer.read_pointer(self.hub)["fingerprint"]["hashes"])
        second_row = self.docs_row(second)
        self.assertEqual(self.docs_row(first)["state"], "published")
        self.assertEqual(second_row["state"], "pending")
        self.assertTrue(set(first_row["hashes"]) <= accepted & set(second_row["hashes"]))
        outstanding = set(second_row["hashes"]) - accepted

        worker = self.accepted_clone("g1-prefix-code")
        docsqueue.sync(worker, self.hub)
        self.assertTrue(outstanding <= pipeline.hashes(worker))
        (worker / "docs/helper.rs").write_text("fn main() {}\n")
        self.record(worker, "code in the accepted docs directory")
        manifest = self.submit_closure(worker)
        self.assertFalse(set(manifest["hashes"]) & (outstanding | set(first_row["hashes"])))
        self.assertEqual(manifest["files"], ["docs/helper.rs"])
        candidate = self.frozen_candidate(manifest)
        self.assertEqual((candidate / "docs/guide.md").read_text(), "d1\n")
        self.assertFalse(outstanding & pipeline.hashes(candidate))
        self.assertFalse((candidate / "docs/sub").exists())

        dependent = self.accepted_clone("g1-prefix-dependent")
        docsqueue.sync(dependent, self.hub)
        (dependent / "docs/sub/x.rs").write_text("fn main() {}\n")
        self.record(dependent, "code inside D2's directory")
        self.refused_submit(dependent, "depends on queued documentation")

    # ---- CK-38: a single-submission batch publishes on its submission review alone ------------

    def single_submission(
        self,
        name: str,
        *,
        review: bool = True,
        raw: bool = False,
        issues: Iterable[IssueRef] = (),
    ) -> ManifestV2:
        """A submission made on the hub's accepted base, as `ck submit` writes it: its manifest
        names the pointer it was made on (the fixture default names none)."""
        manifest = self.submission(name, review=False, raw=raw, issues=issues)
        manifest["base"] = {
            "pointer_sha256": pointer.file_digest(self.hub / ".pipeline/accepted-v2.json"),
            "fingerprint_sha256": "0" * 64,
        }
        path = self.hub / ".pipeline/queue" / f"{name}.json"
        pointer.atomic_bytes(path, pointer.canonical(manifest))
        if review:
            self.review_as(
                name, "claude", f"review-{name}", "claude-reviewer", rid=f"review-{name}"
            )
        return manifest

    def timed(
        self,
        steps: Mapping[str, float] | None = None,
        *,
        watchdog: float = 2700,
        header: str | None = None,
        summary: bool = True,
        raw: Sequence[str] | None = None,
    ) -> Callable[[Path, Path, int], bool]:
        """A passing CI run whose log is what a candidate's `check.py` writes: the recipe header,
        a timing line per step and the final summary."""
        timings = {"fmt": 0.2, "test": 9.6, "unittest-check": 96.0} if steps is None else steps

        def validate(repo: Path, log: Path, fd: int) -> bool:
            if raw is not None:  # the log exactly as given
                log.write_text("\n".join(raw) + "\n")
                self.chat_inputs(log)
                return True
            lines = ["check: candidate lease held on /candidate"]
            lines.append(
                header
                if header is not None
                else f"check: recipe full; watchdog {watchdog:g} s; steps {','.join(timings)}"
            )
            lines += [
                f"check: step {name} took {seconds:.1f} s" for name, seconds in timings.items()
            ]
            if summary:
                lines.append(f"check full: {', '.join(f'{n} ok' for n in timings)} => PASS")
            log.write_text("\n".join(lines) + "\n")
            self.chat_inputs(log)
            return True

        return validate

    def single_batch(
        self, name: str = "solo", validator: Callable[[Path, Path, int], bool] | None = None
    ) -> tuple[batch.Manager, str]:
        self.single_submission(name)
        manager = batch.Manager(self.hub, validator or self.timed())
        return manager, manager.prepare()

    def refused_single(self, manager: batch.Manager, bid: str, reason: str) -> None:
        """The single path refuses with `reason` and the next action, and writes nothing."""
        directory = batch.batch_path(self.hub, bid)
        before = (self.hub / ".pipeline/accepted-v2.json").read_bytes()
        with self.assertRaises(RuntimeError) as caught:
            manager.publish(bid)
        self.assertRegex(str(caught.exception), reason)
        self.assertIn("request an integration review", str(caught.exception))
        self.assertEqual((self.hub / ".pipeline/accepted-v2.json").read_bytes(), before)
        self.assertFalse((directory / "publishing.json").exists())
        self.assertFalse((directory / "published.json").exists())

    def retire(self, manager: batch.Manager, bid: str, name: str) -> None:
        """End a batch and withdraw its member so that the next batch of the test is its own."""
        manager.cancel(bid, "next case")
        self.store.write(
            self.store.repo / f"events/withdraw-{name}.json",
            pointer.canonical({"kind": "withdrawn", "submission": name, "reason": "fixture"}),
            "Withdraw a fixture member",
        )

    def revoke(self, review: str) -> None:
        self.store.write(
            self.store.repo / f"events/revoke-{review}.json",
            pointer.canonical({"kind": "revocation", "review": review, "reason": "fixture"}),
            "Revoke review fixture",
        )

    def test_single_01_a_one_member_batch_publishes_without_a_batch_receipt(self) -> None:
        manager, bid = self.single_batch("solo")
        directory = batch.batch_path(self.hub, bid)
        before = manager.status(bid)
        self.assertEqual(before["publication_basis"], "single-submission")
        self.assertNotIn("publication_basis_reason", before)
        self.assertFalse((directory / "approved.json").exists())
        manager.publish(bid)
        written = json.loads((self.hub / ".pipeline/accepted-v2.json").read_bytes())
        integration = written["integration"]
        self.assertEqual(integration["basis"], "single-submission")
        self.assertEqual(integration["batch"], bid)
        self.assertEqual(integration["submission"], "solo")
        self.assertEqual(integration["submission_receipts"], ["review-solo"])
        self.assertEqual(integration["receipts"], [])
        self.assertEqual(integration["timing"]["slowest_step"], "unittest-check")
        self.assertEqual(integration["timing"]["slowest_seconds"], 96.0)
        self.assertEqual(integration["timing"]["margin"], 2700 / 96.0)
        self.assertEqual(integration["timing"]["log_sha256"], self.validated(bid)["ci_log_sha256"])
        self.assertIn("solo", written["submissions"])
        published = json.loads((directory / "published.json").read_bytes())
        self.assertEqual(published["basis"], "single-submission")
        self.assertEqual(manager.status(bid)["publication_basis"], "single-submission")
        self.assertFalse((directory / "approved.json").exists())

    def test_single_02_two_members_still_need_an_integration_review(self) -> None:
        self.single_submission("left")
        self.single_submission("right")
        manager = batch.Manager(self.hub, self.timed())
        bid = manager.prepare()
        self.assertEqual(manager.status(bid)["publication_basis"], "integration-review")
        self.refused_single(manager, bid, "exactly one member")
        self.integration_receipt(bid, "two-members")  # the integration path is unchanged
        manager.approve(bid)
        manager.publish(bid)
        published = json.loads((batch.batch_path(self.hub, bid) / "published.json").read_bytes())
        self.assertEqual(published["basis"], "integration-review")
        integration = json.loads((self.hub / ".pipeline/accepted-v2.json").read_bytes())[
            "integration"
        ]
        self.assertNotIn("basis", integration)  # the pointer keeps today's form on this path
        self.assertEqual(integration["receipts"], ["two-members"])

    def test_single_03_a_candidate_that_differs_from_its_capture_needs_a_review(self) -> None:
        self.single_submission("first")
        first = batch.Manager(self.hub, self.timed())
        first_bid = first.prepare()
        self.single_submission("late")  # made on the base that the first batch is about to move
        self.integration_receipt(first_bid, "first-integration")
        first.approve(first_bid)
        first.publish(first_bid)
        manager = batch.Manager(self.hub, self.timed())
        bid = manager.prepare()
        self.assertEqual(self.validated(bid)["members"], ["late"])
        self.assertEqual(manager.status(bid)["publication_basis"], "integration-review")
        self.refused_single(manager, bid, "differs from the member's capture")

    def test_single_04_the_base_advancing_after_validation_is_still_refused(self) -> None:
        manager, bid = self.single_batch("moved")
        state = pointer.read_pointer(self.hub)
        advanced = {**state, "submissions": [*state.get("submissions", []), "elsewhere"]}
        advanced.pop("_pointer_path", None)
        pointer.atomic_bytes(self.hub / ".pipeline/accepted-v2.json", pointer.canonical(advanced))
        with self.assertRaisesRegex(RuntimeError, "base advanced"):
            manager.publish(bid)
        self.assertFalse((batch.batch_path(self.hub, bid) / "publishing.json").exists())

    def test_single_05_a_revoked_or_missing_submission_review_needs_a_review(self) -> None:
        manager, bid = self.single_batch("revoked")
        self.revoke("review-revoked")
        self.refused_single(manager, bid, "AI review receipts alone")
        # Unknown authors need both families; losing one of them is losing coverage.
        self.single_submission("raw", raw=True, review=False)
        self.review_as("raw", "claude", "raw-claude", rid="raw-claude")
        codex = self.review_as("raw", "codex", "raw-codex", rid="raw-codex")
        self.assertIn("raw", self.eligible_ids())
        self.retire(manager, bid, "revoked")
        second = batch.Manager(self.hub, self.timed())
        raw_bid = second.prepare()
        self.assertEqual(self.validated(raw_bid)["members"], ["raw"])
        self.assertEqual(second.status(raw_bid)["publication_basis"], "single-submission")
        self.revoke(codex)
        self.refused_single(second, raw_bid, "AI review receipts alone")

    def test_single_06_a_revised_spec_is_not_published(self) -> None:
        issue_id = self.new_issue("Single spec", approved=True)
        issue = self.store.show(issue_id)
        assert issue is not None
        self.single_submission(
            "spec-solo",
            issues=[
                {
                    "id": issue_id,
                    "revision": issue["revision"],
                    "spec_sha256": issue["spec_sha256"],
                }
            ],
        )
        manager = batch.Manager(self.hub, self.timed())
        bid = manager.prepare()
        revised = Path(self.temp.name) / "single-spec-r2.md"
        revised.write_text("spec r2\n")
        self.store.append_issue_event(issue_id, "spec", {}, revised.read_bytes())
        with self.assertRaisesRegex(RuntimeError, "no longer eligible"):
            manager.publish(bid)
        self.assertFalse((batch.batch_path(self.hub, bid) / "publishing.json").exists())

    def test_single_07_a_failed_or_altered_ci_receipt_is_not_published(self) -> None:
        manager, bid = self.single_batch("receipt")
        path = Path(self.validated(bid)["ci_receipt"])
        original = path.read_bytes()
        altered = json.loads(original)
        pointer.atomic_bytes(path, pointer.canonical({**altered, "recorded": 1}))
        with self.assertRaisesRegex(RuntimeError, "CI receipt changed"):
            manager.publish(bid)
        failed = {**altered, "passed": False}
        pointer.atomic_bytes(path, pointer.canonical(failed))
        record = batch.batch_path(self.hub, bid) / "validated.json"
        validated = json.loads(record.read_bytes())
        validated["ci_receipt_sha256"] = pointer.file_digest(path)
        pointer.atomic_bytes(record, pointer.canonical(validated))
        with self.assertRaisesRegex(RuntimeError, "CI receipt did not pass"):
            manager.publish(bid)
        self.assertFalse((batch.batch_path(self.hub, bid) / "publishing.json").exists())

    def test_single_08_a_rejected_batch_or_a_non_approving_batch_receipt_wins(self) -> None:
        manager, bid = self.single_batch("rejected-solo")
        self.assertEqual(manager.status(bid)["publication_basis"], "single-submission")
        self.integration_receipt(bid, "changes", verdict="rejected")
        self.assertEqual(manager.status(bid)["publication_basis"], "integration-review")
        self.refused_single(manager, bid, "not approved")

    def test_single_08b_a_rejected_batch_is_not_published(self) -> None:
        manager, bid = self.single_batch("rejected-batch")
        manager.reject(bid, [])
        self.refused_single(manager, bid, "rejected.json")

    def test_single_10_recovery_keeps_the_recorded_basis(self) -> None:
        # (a) the intent is written, the pointer is not: the conditions are verified again.
        _, bid = self.single_batch("intent-solo")
        with self.assertRaisesRegex(RuntimeError, "injected after_publish_intent"):
            batch.Manager(self.hub, hook=self.crash_at("after_publish_intent")).publish(bid)
        directory = batch.batch_path(self.hub, bid)
        intent_pointer = json.loads((directory / "publishing.json").read_bytes())["pointer"]
        self.assertEqual(json.loads(intent_pointer)["integration"]["basis"], "single-submission")
        self.revoke("review-intent-solo")  # the basis no longer holds
        before = (self.hub / ".pipeline/accepted-v2.json").read_bytes()
        with self.assertRaisesRegex(RuntimeError, "AI review receipts alone"):
            batch.Manager(self.hub).publish(bid)
        self.assertEqual((self.hub / ".pipeline/accepted-v2.json").read_bytes(), before)
        self.assertFalse((directory / "published.json").exists())

    def test_single_10a_an_intent_without_a_pointer_is_finished_on_the_same_basis(self) -> None:
        _, bid = self.single_batch("intent-ok")
        with self.assertRaisesRegex(RuntimeError, "injected after_publish_intent"):
            batch.Manager(self.hub, hook=self.crash_at("after_publish_intent")).publish(bid)
        batch.Manager(self.hub).publish(bid)
        directory = batch.batch_path(self.hub, bid)
        intent = json.loads((directory / "publishing.json").read_bytes())
        self.assertEqual(
            (self.hub / ".pipeline/accepted-v2.json").read_bytes(), intent["pointer"].encode()
        )
        published = json.loads((directory / "published.json").read_bytes())
        self.assertEqual(published["basis"], "single-submission")
        self.assertFalse((directory / "approved.json").exists())

    def test_single_10b_the_pointer_written_is_finished_without_the_eligibility_check(
        self,
    ) -> None:
        _, bid = self.single_batch("pointer-solo")
        with self.assertRaisesRegex(RuntimeError, "injected after_pointer_write"):
            batch.Manager(self.hub, hook=self.crash_at("after_pointer_write")).publish(bid)
        with mock.patch.object(
            batch.Manager, "eligible", side_effect=AssertionError("eligible() consulted")
        ):
            batch.Manager(self.hub).publish(bid)
        published = json.loads((batch.batch_path(self.hub, bid) / "published.json").read_bytes())
        self.assertEqual(published["basis"], "single-submission")

    def test_single_10c_recovery_after_a_docs_publication_keeps_checking_the_submission(
        self,
    ) -> None:
        _, bid = self.single_batch("docs-solo")
        with self.assertRaisesRegex(RuntimeError, "injected after_pointer_write"):
            batch.Manager(self.hub, hook=self.crash_at("after_pointer_write")).publish(bid)
        copy = self.accepted_clone("single-docs")
        (copy / "README.md").write_text("docs after the crash\n")
        docsqueue.Queue(self.hub).capture(copy, "Docs after the crash", ["README.md"])
        with mock.patch.object(docsqueue, "_active", return_value=None):
            docsqueue.Queue(self.hub).flush()
        # The submission review is read from the receipt rows: revoking it stops recovery ...
        self.revoke("review-docs-solo")
        with (
            mock.patch.object(
                batch.Manager, "eligible", side_effect=AssertionError("eligible() consulted")
            ),
            self.assertRaisesRegex(RuntimeError, "AI review receipts alone"),
        ):
            batch.Manager(self.hub).publish(bid)
        self.assertFalse((batch.batch_path(self.hub, bid) / "published.json").exists())

    def test_single_10d_recovery_after_a_docs_publication_completes_with_the_review(
        self,
    ) -> None:
        _, bid = self.single_batch("docs-ok")
        with self.assertRaisesRegex(RuntimeError, "injected after_pointer_write"):
            batch.Manager(self.hub, hook=self.crash_at("after_pointer_write")).publish(bid)
        copy = self.accepted_clone("single-docs-ok")
        (copy / "README.md").write_text("docs after the crash\n")
        docsqueue.Queue(self.hub).capture(copy, "Docs after the crash", ["README.md"])
        with mock.patch.object(docsqueue, "_active", return_value=None):
            docsqueue.Queue(self.hub).flush()
        with mock.patch.object(
            batch.Manager, "eligible", side_effect=AssertionError("eligible() consulted")
        ):
            batch.Manager(self.hub).publish(bid)
        published = json.loads((batch.batch_path(self.hub, bid) / "published.json").read_bytes())
        self.assertEqual(published["basis"], "single-submission")

    def test_single_11_a_step_over_a_tenth_of_the_watchdog_needs_a_review(self) -> None:
        manager, bid = self.single_batch("slow", self.timed({"fmt": 1.0, "clippy": 300.0}))
        self.assertEqual(manager.status(bid)["publication_basis"], "integration-review")
        self.refused_single(manager, bid, "step clippy took 300 s.*margin 9.00x")

    def test_single_12_a_log_without_a_header_or_a_step_timing_needs_a_review(self) -> None:
        listed = "check: recipe full; watchdog 2700 s; steps fmt,ghost"
        for name, validator, reason in (
            ("no-header", self.timed(header="check: nothing here"), "no recipe header"),
            (
                "missing-step",
                self.timed({"fmt": 1.0}, header=listed),
                "no timing line for step ghost",
            ),
            (
                "no-summary",
                self.timed(summary=False),
                "does not end with the full run's passing summary",
            ),
            (
                "quick-recipe",
                self.timed(header="check: recipe quick; watchdog 2700 s; steps fmt"),
                "not a full run",
            ),
            (
                # CK-77: a planned check names itself `planned`, whatever groups it ran and however it
                # ends, and is never batch evidence.
                "planned-recipe",
                self.timed(header="check: recipe planned; watchdog 2700 s; steps fmt"),
                "not a full run",
            ),
        ):
            with self.subTest(name):
                manager, bid = self.single_batch(name, validator)
                self.refused_single(manager, bid, reason)
                self.retire(manager, bid, name)

    def test_single_12c_ambiguous_or_non_finite_evidence_needs_a_review(self) -> None:
        head = "check: recipe full; watchdog 2700 s; steps fmt,test"
        took = ["check: step fmt took 1.0 s", "check: step test took 2.0 s"]
        full = "check full: fmt ok, test ok => PASS"
        huge = "9" * 400  # overflows a float to infinity
        cases = {
            "quick-pass-then-full-fail": (
                [head, *took, "check quick: fmt ok => PASS", "check full: fmt ok, test ok => FAIL"],
                "does not end with the full run's passing summary",
            ),
            "full-pass-then-another-summary": (
                [head, *took, full, "check quick: fmt ok => FAIL"],
                "does not end with the full run's passing summary",
            ),
            "summary-names-other-steps": (
                [head, *took, "check full: fmt ok => PASS"],
                "does not end with the full run's passing summary",
            ),
            "duplicate-timing": (
                [head, took[0], "check: step test took 2000.0 s", took[1], full],
                "more than one timing line for step test",
            ),
            "timing-for-an-unlisted-step": (
                [head, *took, "check: step ghost took 1.0 s", full],
                "timing line for step ghost",
            ),
            "two-headers": (
                [head, "check: recipe full; watchdog 9999 s; steps fmt,test", *took, full],
                "more than one recipe header",
            ),
            "real-checker-duplicate": (
                [
                    "check: candidate lease held on /candidate",
                    "check: recipe full; watchdog 10 s; steps slow,noise",
                    "check: batch run; project lock held: ci.lock, inherited as CK_CI_LOCK_FD",
                    "check: step slow took 1.2 s",
                    "check: step slow took 0.1 s",
                    "check: step noise took 0.0 s",
                    "check full: slow ok, noise ok => PASS",
                ],
                "more than one timing line for step slow",
            ),
            "duplicate-step-names": (
                ["check: recipe full; watchdog 2700 s; steps fmt,fmt", took[0], full],
                "not a full run of distinct, non-empty steps",
            ),
            "empty-step-name": (
                ["check: recipe full; watchdog 2700 s; steps fmt,,test", *took, full],
                "not a full run of distinct, non-empty steps",
            ),
            "trailing-empty-step-name": (
                ["check: recipe full; watchdog 2700 s; steps fmt,test,", *took, full],
                "not a full run of distinct, non-empty steps",
            ),
            "infinite-margin": (
                [
                    f"check: recipe full; watchdog 1{'0' * 308} s; steps fmt,test",
                    "check: step fmt took 0.0 s",
                    f"check: step test took 0.{'0' * 307}1 s",
                    full,
                ],
                "margin for step test is not a finite number",
            ),
            "summary-with-a-trailing-space": (
                [head, *took, full, "check full: test FAILED => FAIL "],
                "malformed summary line",
            ),
            "second-header-with-a-nan": (
                [head, *took, "check: recipe full; watchdog NaN s; steps test", full],
                "malformed recipe line",
            ),
            "step-line-with-a-nan": (
                [head, *took, "check: step ghost took NaN s", full],
                "malformed step line",
            ),
            "step-line-of-another-shape": (
                [head, took[0], "check: step test exceeded the 2700 s watchdog after 2701 s", full],
                "malformed step line",
            ),
            "step-line-with-a-trailing-space": (
                [head, took[0], "check: step test took 2.0 s ", full],
                "malformed step line",
            ),
            "summary-of-another-shape": (
                [head, *took, "check full: test FAILED", full],
                "malformed summary line",
            ),
            "no-steps": (
                ["check: recipe full; watchdog 2700 s; steps ", full],
                "not a full run of distinct, non-empty steps",
            ),
            "overflowing-timing": (
                [head, took[0], f"check: step test took {huge}.0 s", full],
                "not a finite number",
            ),
            "overflowing-watchdog": (
                [f"check: recipe full; watchdog {huge} s; steps fmt,test", *took, full],
                "not a finite positive number",
            ),
        }
        for name, (lines, reason) in cases.items():
            with self.subTest(name):
                manager, bid = self.single_batch(name, self.timed(raw=lines))
                self.refused_single(manager, bid, reason)
                self.retire(manager, bid, name)
        manager, bid = self.single_batch("clean", self.timed(raw=[head, *took, full]))
        self.assertEqual(manager.status(bid)["publication_basis"], "single-submission")

    def assert_slowest_step_matches_the_log(self, text: str, slowest: object) -> None:
        """The gate's `slowest_step` is a step of the log's header whose logged time is the log's highest.

        Read from the log with its own patterns, not with the gate's timing parser. A tie is fine: the checker
        rounds each time to 0.1 s and the gate takes the first of equal times, so which of two quick steps is
        named depends on the machine, and the log is what says whether the name is consistent.
        """
        header = re.search(r"^check: recipe \S+; watchdog \d+ s; steps (\S+)$", text, re.MULTILINE)
        self.assertIsNotNone(header, "the log has no run header")
        assert header is not None  # for the type checker
        steps = header.group(1).split(",")
        times = {
            name: float(seconds)
            for name, seconds in re.findall(
                r"^check: step (\S+) took ([0-9.]+) s$", text, re.MULTILINE
            )
        }
        self.assertIn(slowest, steps)
        self.assertIn(slowest, times, "the log has no timing line for the named step")
        self.assertEqual(times[str(slowest)], max(times.values()))

    def test_single_12d_the_slowest_step_check_rejects_a_step_that_is_not_the_logged_slowest(
        self,
    ) -> None:
        # The negative control of the next test, run every time: a log whose highest time belongs to another step.
        log = (
            "check: recipe full; watchdog 2700 s; steps fmt,slow\n"
            "check: step fmt took 0.3 s\n"
            "check: step slow took 0.1 s\n"
        )
        self.assert_slowest_step_matches_the_log(log, "fmt")
        with self.assertRaises(AssertionError):
            self.assert_slowest_step_matches_the_log(log, "slow")
        with self.assertRaises(AssertionError):
            self.assert_slowest_step_matches_the_log(log, "elsewhere")  # not a step of the header
        tie = log.replace("0.3 s", "0.1 s")
        self.assert_slowest_step_matches_the_log(tie, "fmt")  # a tie names either step
        self.assert_slowest_step_matches_the_log(tie, "slow")
        with self.assertRaises(AssertionError):
            self.assert_slowest_step_matches_the_log(
                "check: step fmt took 0.1 s\n", "fmt"
            )  # no header

    def test_single_12d_a_real_checker_log_publishes_on_the_single_path(self) -> None:
        # The validator runs the real `check.execute` in batch mode on fixture steps, so the log has
        # every kind of line the checker prints (lease/lock lines, `+ command` echoes, timings). Both steps are
        # quick: which is slowest is a matter of timing and is not asserted, only that the gate's answer fits the log.
        tools = Path(__file__).resolve().parents[1] / "tools"

        def real_checker(repo: Path, log: Path, fd: int) -> bool:
            code = textwrap.dedent(
                f"""
                import signal, sys
                from pathlib import Path
                sys.path.insert(0, {str(tools)!r})
                import check
                from check import Step
                ok = [[sys.executable, "-c", "print('hello')"]]
                slow = [[sys.executable, "-c", "print('hello again')"]]
                steps = [Step("fmt", ("full",), lambda: ok), Step("slow", ("full",), lambda: slow)]
                signal.signal(signal.SIGTERM, check.terminate)
                sys.exit(check.execute(Path({str(repo)!r}), Path({str(self.hub)!r}), "full", steps=steps))
                """
            )
            environment = {**os.environ, "CK_CI_LOCK_FD": str(fd)}
            with log.open("w") as stream:
                done = subprocess.run(
                    [sys.executable, "-c", code],
                    env=environment,
                    stdout=stream,
                    stderr=subprocess.STDOUT,
                    pass_fds=(fd,),
                    check=False,
                    timeout=120,
                )
            self.chat_inputs(log)
            return done.returncode == 0

        manager, bid = self.single_batch("real-checker", real_checker)
        text = Path(self.ci_receipt(bid)["logs"][0]).read_text()
        self.assertIn("check: recipe full; watchdog 2700 s; steps fmt,slow", text)
        self.assertIn("check: batch run; project lock held: ci.lock", text)
        self.assertEqual(manager.status(bid)["publication_basis"], "single-submission")
        manager.publish(bid)
        written = json.loads((self.hub / ".pipeline/accepted-v2.json").read_bytes())
        self.assertEqual(written["integration"]["basis"], "single-submission")
        self.assert_slowest_step_matches_the_log(
            text, written["integration"]["timing"]["slowest_step"]
        )

    def test_single_12b_a_missing_log_needs_a_review(self) -> None:
        def silent(repo: Path, log: Path, fd: int) -> bool:
            self.chat_inputs(log)  # the chat-inputs record, but no log text at all
            return True

        manager, bid = self.single_batch("no-log", silent)
        self.refused_single(manager, bid, "without a bound CI log digest")

    def test_single_13_a_changed_log_or_an_unbound_batch_falls_back_to_the_review(self) -> None:
        manager, bid = self.single_batch("changed-log")
        log = Path(self.ci_receipt(bid)["logs"][0])
        log.write_text(log.read_text() + "check: edited later\n")
        self.refused_single(manager, bid, "changed after prepare")
        self.retire(manager, bid, "changed-log")
        manager, bid = self.single_batch("unbound")
        record = batch.batch_path(self.hub, bid) / "validated.json"
        validated = json.loads(record.read_bytes())
        del validated["ci_log_sha256"]
        pointer.atomic_bytes(record, pointer.canonical(validated))
        self.refused_single(manager, bid, "without a bound CI log digest")
        self.integration_receipt(bid, "unbound-integration")  # the integration path still works
        manager.approve(bid)
        manager.publish(bid)
        published = json.loads((batch.batch_path(self.hub, bid) / "published.json").read_bytes())
        self.assertEqual(published["basis"], "integration-review")

    def test_single_14_the_watchdog_comes_from_the_run_header_not_the_controller(self) -> None:
        manager, bid = self.single_batch("short", self.timed(watchdog=600))  # 96 s x 10 > 600 s
        self.refused_single(manager, bid, "step unittest-check took 96 s")
        self.retire(manager, bid, "short")
        # 500 s x 10 is over the accepted controller's 2700 s, within the run's own 99999 s.
        long = self.timed({"fmt": 1.0, "slow": 500.0}, watchdog=99999)
        manager, bid = self.single_batch("long", long)
        manager.publish(bid)
        written = json.loads((self.hub / ".pipeline/accepted-v2.json").read_bytes())
        self.assertEqual(written["integration"]["timing"]["slowest_step"], "slow")

    # CK-152: the owner's optional post-publish hook, started once per command that advanced the pointer.

    HOOK_SCRIPT = textwrap.dedent(
        """\
        #!{python}
        import json, os, sys
        sys.path.insert(0, {tools!r})
        from pathlib import Path
        from gate import locks
        hub = Path(os.environ["CK_HUB"])
        with locks.ci(hub), locks.state(hub):  # a held lock would refuse here
            pass
        row = {{"cwd": os.getcwd(), "hub": str(hub), "stdin": sys.stdin.read(), "leases": [
            name for name in ("CK_LEASE_FDS", "CK_CI_LOCK_FD") if name in os.environ]}}
        print("hook ran", flush=True)  # the log first: the row is the parent's witness that all is written
        with (hub / "hook-calls.jsonl").open("a") as stream:
            stream.write(json.dumps(row) + "\\n")
        sys.exit({code})
        """
    )

    def install_hook(self, *, code: int = 0, executable: bool = True) -> Path:
        hook = self.hub / postpublish.HOOK
        hook.parent.mkdir(parents=True, exist_ok=True)
        tools = Path(pointer.__file__).resolve().parent.parent
        hook.write_text(self.HOOK_SCRIPT.format(python=sys.executable, tools=str(tools), code=code))
        hook.chmod(0o755 if executable else 0o644)
        return self.hub / "hook-calls.jsonl"

    def hook_calls(self, calls: Path, count: int) -> list[dict[str, object]]:
        """The rows the detached hook wrote, once `count` of them exist (it runs after ck returns)."""
        deadline = time.monotonic() + 30
        rows: list[dict[str, object]] = []
        while time.monotonic() < deadline:
            if calls.exists():
                rows = [json.loads(line) for line in calls.read_text().splitlines()]
                if len(rows) >= count:
                    break
            time.sleep(0.05)
        log = self.hub / postpublish.LOG
        self.assertEqual(len(rows), count, log.read_text() if log.exists() else "no hook log")
        return rows

    @contextlib.contextmanager
    def spawns(self, fail: Exception | None = None) -> Iterator[list[str]]:
        """The hook launches while it is active (Pijul and other children pass through)."""
        launched: list[str] = []
        real = subprocess.Popen

        def popen(command: list[str], *args: object, **kwargs: object) -> object:
            if str(command[0]).endswith(postpublish.HOOK):
                launched.append(str(command[0]))
                if fail is not None:
                    raise fail
            return real(command, *args, **kwargs)  # type: ignore[call-overload]  # a pass-through

        with mock.patch.object(subprocess, "Popen", popen):
            yield launched

    def ck(self, *argv: str) -> int:
        with contextlib.redirect_stdout(io.StringIO()):
            return gate_cli.run(self.hub, self.hub, list(argv))

    def approved(self, name: str = "hooked") -> str:
        manager, bid = self.prepared(name)
        self.approve(manager, bid)
        return bid

    def docs_copy(self, text: str) -> Path:
        copy = Path(self.temp.name) / f"docs-copy-{len(text)}"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), copy, self.hub)
        (copy / "README.md").write_text(text)
        return copy

    # CK-151: `ck docs` refuses a bundle holding the user's home path or account name.

    def home_docs(self, name: str, text: str, home: Path) -> Path:
        copy = Path(self.temp.name) / name
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), copy, self.hub)
        (copy / "README.md").write_text(text)
        return copy

    def docs_state(self) -> tuple[bytes, list[str]]:
        docs = self.hub / ".pipeline/docs"
        return (
            (self.hub / ".pipeline/accepted-v2.json").read_bytes(),
            sorted(path.name for path in [*docs.glob("*.json"), *(docs / "captures").glob("*")]),
        )

    def test_ck151_01_a_bundle_with_the_home_path_or_account_name_is_refused_whole(self) -> None:
        home = Path(self.temp.name) / "someaccountname"
        before = self.docs_state()
        for text in (f"see {home}/work/codekarl\n", "owned by someaccountname\n"):
            copy = self.home_docs("leaky", text, home)
            (copy / "docs").mkdir(exist_ok=True)
            (copy / "docs/clean.md").write_text("nothing private\n")
            with (
                mock.patch.object(Path, "home", return_value=home),
                self.assertRaisesRegex(RuntimeError, r"refused.*README\.md contain"),
            ):
                docsqueue.Queue(self.hub).capture(copy, "leak", ["README.md", "docs/clean.md"])
            self.assertEqual(self.docs_state(), before)  # nothing queued, captured or published
            shutil.rmtree(copy)

    def test_ck151_02_the_same_bundle_without_it_publishes(self) -> None:
        home = Path(self.temp.name) / "someaccountname"
        copy = self.home_docs("clean", "see ~/work/codekarl\n", home)
        with mock.patch.object(Path, "home", return_value=home):
            did = docsqueue.Queue(self.hub).capture(copy, "clean", ["README.md"])
        row = json.loads((self.hub / ".pipeline/docs" / f"{did}.json").read_bytes())
        self.assertEqual(row["state"], "published")

    def test_ck151_03_the_bytes_checked_are_the_bytes_published(self) -> None:
        home = Path(self.temp.name) / "someaccountname"
        copy = self.home_docs("swap", "clean text\n", home)
        real = docsqueue.home_offenders

        def check_then_swap(contents: Mapping[str, bytes], where: Path) -> list[str]:
            found = real(contents, where)
            (copy / "README.md").write_text(f"swapped in {home}\n")  # after the check
            return found

        with (
            mock.patch.object(Path, "home", return_value=home),
            mock.patch.object(docsqueue, "home_offenders", check_then_swap),
        ):
            docsqueue.Queue(self.hub).capture(copy, "swap", ["README.md"])
        checkpoint = Path(pointer.read_pointer(self.hub)["repo"])
        self.assertEqual((checkpoint / "README.md").read_text(), "clean text\n")

    def test_ck151_04_the_predicate_uses_the_home_it_is_given(self) -> None:
        found = docsqueue.home_offenders(
            {"a.md": b"/Users/someaccountname/x", "b.md": b"someaccountname", "c.md": b"fine"},
            Path("/Users/someaccountname"),
        )
        self.assertEqual(found, ["a.md", "b.md"])
        short = docsqueue.home_offenders({"a.md": b"/home/me and me"}, Path("/home/me"))
        self.assertEqual(short, ["a.md"])  # the path matches; the short name alone does not
        self.assertEqual(docsqueue.home_offenders({"a.md": b"/ and root"}, Path("/")), [])

    def test_ck152_01_a_batch_publish_runs_the_hook_once_with_the_locks_released(self) -> None:
        calls = self.install_hook()
        bid = self.approved()
        with self.spawns() as launched:
            self.assertEqual(self.ck("batch", "publish", bid), 0)
        self.assertEqual(len(launched), 1)
        (row,) = self.hook_calls(calls, 1)
        self.assertEqual(
            row, {"cwd": str(self.hub.resolve()), "hub": str(self.hub), "stdin": "", "leases": []}
        )
        self.assertIn("hook ran", (self.hub / postpublish.LOG).read_text())
        self.assertEqual(pointer.read_pointer(self.hub)["submissions"], ["hooked"])

    def test_ck152_02_a_docs_capture_and_a_docs_flush_each_run_it_once(self) -> None:
        calls = self.install_hook()
        with self.spawns() as launched:
            self.ck(
                "docs",
                "--from",
                str(self.docs_copy("flushed at once\n")),
                "-m",
                "docs",
                "README.md",
            )
        self.assertEqual(len(launched), 1)
        self.hook_calls(calls, 1)
        hold = self.hub / ".pipeline/batches/B-HOLD"
        hold.mkdir(parents=True)
        pointer.atomic_bytes(hold / "frozen.json", b"{}")
        self.ck("docs", "--from", str(self.docs_copy("queued\n")), "-m", "queued", "README.md")
        self.assertEqual(len(self.hook_calls(calls, 1)), 1)  # queued behind the batch: no change
        (hold / "frozen.json").unlink()
        hold.rmdir()
        with self.spawns() as launched:
            self.assertEqual(self.ck("docs", "flush"), 0)
        self.assertEqual(len(launched), 1)
        self.hook_calls(calls, 2)

    def test_ck152_03_a_refused_or_unchanged_command_does_not_run_it(self) -> None:
        calls = self.install_hook()
        bid = self.approved()
        self.drift()
        with self.spawns() as launched:
            with self.assertRaisesRegex(RuntimeError, "Shared chat inputs changed"):
                self.ck("batch", "publish", bid)
            self.assertEqual(self.ck("docs", "flush"), 0)  # nothing queued
            self.assertEqual(self.ck("status"), 0)
        self.assertEqual(launched, [])
        self.assertFalse(calls.exists())

    def test_ck152_04_an_earlier_library_publication_is_not_this_commands(self) -> None:
        calls = self.install_hook()
        docsqueue.Queue(self.hub).capture(
            self.docs_copy("library\n"), "library", ["README.md"]
        )  # advances the pointer in this process, outside any command
        with self.spawns() as launched:
            self.assertEqual(self.ck("status"), 0)
        self.assertEqual(launched, [])
        self.assertFalse(calls.exists())

    def test_ck152_05_an_error_after_the_pointer_changed_still_runs_it(self) -> None:
        calls = self.install_hook()
        accepted = self.hub / ".pipeline/accepted-v2.json"

        def changed_then_failed(*_: object) -> int:
            record = json.loads(accepted.read_bytes())
            record["previous"] = "changed by the command"
            pointer.write_accepted(accepted, pointer.canonical(record))
            raise RuntimeError("later ordinary error")

        with (
            mock.patch.object(gate_cli, "_dispatch", changed_then_failed),
            self.assertRaisesRegex(RuntimeError, "later ordinary error"),
        ):
            gate_cli.run(self.hub, self.hub, ["status"])
        self.hook_calls(calls, 1)
        self.assertEqual(json.loads(accepted.read_bytes())["previous"], "changed by the command")

    def test_ck152_05b_a_failing_directory_sync_after_the_replace_still_runs_it(self) -> None:
        calls = self.install_hook()
        accepted = self.hub / ".pipeline/accepted-v2.json"

        def replaced_then_sync_fails(*_: object) -> int:
            record = json.loads(accepted.read_bytes())
            record["previous"] = "replaced before the sync failed"
            payload = pointer.canonical(record)
            with mock.patch.object(os, "fsync", side_effect=[None, OSError("dir sync")]):
                pointer.write_accepted(accepted, payload)  # file sync passes, directory sync fails
            return 0

        with (
            mock.patch.object(gate_cli, "_dispatch", replaced_then_sync_fails),
            self.assertRaisesRegex(OSError, "dir sync"),
        ):
            gate_cli.run(self.hub, self.hub, ["status"])
        self.hook_calls(calls, 1)

    def test_ck152_05c_rewriting_identical_pointer_bytes_is_not_an_advance(self) -> None:
        calls = self.install_hook()
        accepted = self.hub / ".pipeline/accepted-v2.json"

        def same_bytes(*_: object) -> int:
            pointer.write_accepted(accepted, accepted.read_bytes())
            return 0

        with self.spawns() as launched, mock.patch.object(gate_cli, "_dispatch", same_bytes):
            self.assertEqual(gate_cli.run(self.hub, self.hub, ["status"]), 0)
        self.assertEqual(launched, [])
        self.assertFalse(calls.exists())

    def test_ck152_06_an_absent_or_non_executable_hook_is_a_no_op(self) -> None:
        bid = self.approved("quiet")
        with self.spawns() as launched:
            self.assertEqual(self.ck("batch", "publish", bid), 0)  # no hooks directory at all
        self.assertEqual(launched, [])
        self.assertEqual(pointer.read_pointer(self.hub)["submissions"], ["quiet"])
        calls = self.install_hook(executable=False)
        with self.spawns() as launched:
            self.ck("docs", "--from", str(self.docs_copy("plain\n")), "-m", "docs", "README.md")
        self.assertEqual(launched, [])
        self.assertFalse(calls.exists())

    def test_ck152_07_a_failing_hook_or_a_failed_spawn_never_changes_the_publication(self) -> None:
        calls = self.install_hook(code=3)
        bid = self.approved("failing")
        self.assertEqual(self.ck("batch", "publish", bid), 0)
        self.hook_calls(calls, 1)
        self.assertEqual(pointer.read_pointer(self.hub)["submissions"], ["failing"])
        error = io.StringIO()
        with self.spawns(OSError("no exec")), contextlib.redirect_stderr(error):
            self.ck(
                "docs", "--from", str(self.docs_copy("spawn fails\n")), "-m", "docs", "README.md"
            )
        self.assertEqual(error.getvalue().count("warning"), 1)
        self.assertIn("no exec", error.getvalue())
        self.assertIn("README.md", pointer.read_pointer(self.hub)["fingerprint"]["files"])
        self.assertEqual(len(self.hook_calls(calls, 1)), 1)


if __name__ == "__main__":
    unittest.main()
