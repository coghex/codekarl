"""Per-criterion tracker, identity and review coverage on real Pijul metadata."""

import contextlib
import hashlib
import io
import json
import os
import sys
import tempfile
import threading
import unittest
from collections.abc import Mapping
from pathlib import Path
from typing import ClassVar
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
from environment import signing
from gate import classes, cli, meta, pointer, provenance, reviews, sessions
from gate.records import (
    AiReviewer,
    Author,
    Fingerprint,
    Issue,
    IssueEvent,
    ManifestInfo,
    ManifestLegacy,
    ManifestV2,
    MetaEvent,
    ModelUse,
    RecordError,
    ReviewRow,
    RoleMapping,
    SubmissionSubject,
    is_manifest_v2,
    load_author,
    load_issue_event,
    load_manifest,
    load_provenance,
)
from pipeline import doc_path, pijul
from sessionfix import CopilotSession, no_agent

EMPTY: Fingerprint = {"hashes": [], "files": {}}


def person(identity: str, session: str, family: str) -> Author:
    return {"identity": identity, "session": session, "family": family}


def subject_of(name: str) -> SubmissionSubject:
    return {"submission": name, "manifest_sha256": "m", "fingerprint_sha256": "f"}


def reviewer_of(
    identity: str, session: str, *, family: str = "claude", rank: str = "A", effort: str = "high"
) -> AiReviewer:
    return {
        "identity": identity,
        "family": family,
        "role": "reviewer",
        "client": family,
        "session": session,
        "model": "fixture-model",
        "effort": effort,
        "class": rank,
        "evidence": {"path": "/fixture/session.jsonl", "offset": 0, "sha256": "0" * 64},
    }


def row_of(
    subject: SubmissionSubject,
    reviewer: AiReviewer,
    *,
    verdict: str = "approved",
    incremental_over: str | None = None,
    revoked: bool = False,
) -> ReviewRow:
    """A stored `ck-review/1` receipt as `receipt_rows` returns it."""
    return {
        "schema": "ck-review/1",
        "id": f"review-{reviewer['session']}",
        "kind": "submission",
        "verdict": verdict,
        "recorded": 1,
        "report_sha256": "0" * 64,
        "issues": [],
        "reviewer": reviewer,
        "subject": subject,
        "base_examined": None,
        "incremental_over": incremental_over,
        "revoked": revoked,
    }


def legacy(identifier: str, hashes: list[str]) -> ManifestLegacy:
    return {
        "id": identifier,
        "repo": "/fixture",
        "spec": "fixture",
        "hashes": hashes,
        "fingerprint": EMPTY,
    }


class GateCriteria(unittest.TestCase):
    def setUp(self) -> None:
        no_agent(self)
        self.temp = tempfile.TemporaryDirectory(prefix="ck-gate-criteria-")
        self.addCleanup(self.temp.cleanup)
        self.signing = signing(Path.home() / "work/codekarl")
        self.signing.__enter__()
        self.addCleanup(self.signing.__exit__, None, None, None)
        self.root = Path(self.temp.name) / "project"
        self.root.mkdir()
        pijul(self.root, "init")
        (self.root / ".ignore").write_text("/.pipeline\n")
        (self.root / "source.txt").write_text("base\n")
        pijul(self.root, "add", "--recursive", ".")
        pijul(self.root, "record", "-am", "base", "--author", "Gate criteria")
        self.hub = Path(self.temp.name) / "hub"
        self.hub.mkdir()
        self.store = meta.Store(self.hub)
        self.store.ensure()

    def agent(
        self,
        *,
        family: str = "codex",
        role: str = "manager",
        session: str = "worker-1",
        model: str | None = None,
    ) -> Author:
        client = family
        model = model or ("gpt-6-luna" if family == "codex" else "claude-opus-5.5")
        return {
            "identity": f"{session}-identity",
            "client": client,
            "session": session,
            "role": role,
            "family": family,
            "models": [{"model": model, "effort": "high"}],
            "evidence": {"path": "/fixture/session.jsonl", "offset": 1, "sha256": "0" * 64},
        }

    def bind(self, row: Mapping[str, object], client: str, content: str) -> None:
        root = Path(self.temp.name) / f"{client}-logs"
        root.mkdir(exist_ok=True)
        (root / f"{row['session_id']}.jsonl").write_text(content)
        registry = Path(self.temp.name) / "registry.json"
        registry.write_text(json.dumps(row))
        os_env = {
            "CK_SESSION_EVIDENCE": str(registry),
            "CK_SESSION_ROOTS": json.dumps({client: str(root)}),
        }
        for key, value in os_env.items():
            self.addCleanup(mock.patch.dict, __import__("os").environ, {key: value})
        patcher = mock.patch.dict(__import__("os").environ, os_env)
        patcher.start()
        self.addCleanup(patcher.stop)

    def issue(self, name: str = "issue", spec: bytes = b"contract\n") -> str:
        source = Path(self.temp.name) / f"{name}.md"
        source.write_bytes(spec)
        with mock.patch("gate.meta._authors", return_value=[self.agent()]):
            return self.store.new_issue(name, "task", "P2", source)

    def spec_file(self, name: str = "copilot-spec") -> Path:
        source = Path(self.temp.name) / f"{name}.md"
        source.write_bytes(b"contract\n")
        return source

    def shown(self, identifier: str) -> Issue:
        issue = self.store.show(identifier)
        assert issue is not None
        return issue

    def run_cli(self, *argv: str) -> str:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            cli.run(self.root, self.hub, list(argv))
        return output.getvalue()

    def test_c30_role_is_accepted_wherever_an_unbound_agent_authenticates(self) -> None:
        for argv in (
            ["record", "-m", "x", "--issue", "CK-1"],
            ["issue", "new", "--title", "t", "--type", "task", "--spec", "s"],
            ["issue", "spec", "CK-1", "--file", "f"],
            ["issue", "edit", "CK-1"],
            ["issue", "close", "CK-1", "--reason", "wontfix"],
            ["issue", "reopen", "CK-1", "--reason", "r"],
            ["submit", "--issue", "CK-1", "--spec", "s"],
            ["review", "submission", "S", "--verdict", "approved", "--report", "r"],
            ["review", "spec", "CK-1@r1"],
            ["review", "batch", "B"],
            ["review", "revoke", "R", "--reason", "r"],
            ["session"],
        ):
            with self.subTest(argv=argv):
                self.assertEqual(
                    cli.parser().parse_args([*argv, "--role", "reviewer"]).role, "reviewer"
                )
                self.assertIsNone(cli.parser().parse_args(argv).role)

    def test_c31_unbound_copilot_agent_must_declare_its_role_for_record(self) -> None:
        CopilotSession(self, "claude-sonnet-5.5").activate()
        with self.assertRaisesRegex(RuntimeError, "must declare --role"):
            provenance.make_payload(["CK-1"])
        payload = provenance.make_payload(["CK-1"], "worker")
        self.assertEqual(
            (payload["client"], payload["role"], payload["families"], payload["identity"][:8]),
            ("copilot", "worker", ["claude"], "copilot:"),
        )
        self.assertEqual(
            (payload["mapping"]["class"], payload["mapping"]["model"]), ("B", "claude-sonnet-5-5")
        )
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            provenance.make_payload(["CK-1"], "reviewer")

    def test_c32_human_declaring_a_role_is_refused_everywhere(self) -> None:
        message = "declared role requires verifiable agent session evidence"
        with self.assertRaisesRegex(RuntimeError, message):
            provenance.make_payload(["CK-1"], "worker")
        with self.assertRaisesRegex(RuntimeError, message):
            self.store.new_issue("t", "task", "P2", self.spec_file(), "manager")
        with self.assertRaisesRegex(RuntimeError, message):
            reviews._reviewer("reviewer")
        with self.assertRaisesRegex(RuntimeError, message):
            self.run_cli("session", "--role", "worker")
        self.assertEqual(self.store.issues(), [])
        self.assertEqual(meta._authors()[0]["role"], "owner")
        self.assertEqual(json.loads(self.run_cli("session")), {"human": True})
        with self.assertRaisesRegex(RuntimeError, "Human-authored reviews"):
            reviews._reviewer(None)

    def test_c33_bound_role_contradiction_and_non_reviewer_review_refuse(self) -> None:
        row = {"name": "bound", "brand": "codex", "session_id": "bound", "role": "worker"}
        self.bind(
            row,
            "codex",
            '{"type":"turn_context","payload":{"model":"gpt-6-luna","effort":"high"}}\n',
        )
        with self.assertRaisesRegex(RuntimeError, "contradicts"):
            meta._authors("manager")
        with self.assertRaisesRegex(RuntimeError, "contradicts"):
            reviews._reviewer("reviewer")
        with self.assertRaisesRegex(RuntimeError, "require the reviewer role"):
            reviews._reviewer(None)
        self.assertEqual(meta._authors()[0]["role"], "worker")

    def test_c34_copilot_manager_issue_records_effective_role_and_refuses_without_it(self) -> None:
        session = CopilotSession(self, "claude-opus-5.5").activate()
        with self.assertRaisesRegex(RuntimeError, "must declare --role"):
            self.store.new_issue("t", "task", "P2", self.spec_file())
        self.assertEqual(self.store.issues(), [])
        spec = self.spec_file()
        output = self.run_cli(
            "issue",
            "new",
            "--title",
            "t",
            "--type",
            "task",
            "--spec",
            str(spec),
            "--role",
            "manager",
        )
        author = self.shown(output.strip())["spec_authors"][0]
        self.assertEqual(
            (author["identity"], author["client"], author["role"], author["family"]),
            (f"copilot:{session.id}", "copilot", "manager", "claude"),
        )
        with self.assertRaisesRegex(RuntimeError, "must declare --role"):
            self.run_cli("issue", "spec", output.strip(), "--file", str(spec))
        self.run_cli("issue", "spec", output.strip(), "--file", str(spec), "--role", "manager")
        self.assertEqual(self.shown(output.strip())["revision"], "r2")
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            self.run_cli("issue", "edit", output.strip(), "--title", "u", "--role", "worker")

    def test_c35_submit_refuses_before_side_effects_without_a_valid_role(self) -> None:
        args = ["submit", "--issue", "CK-1", "--spec", "x"]
        ready = {"id": "CK-1", "revision": "r1", "spec_sha256": "0"}
        with (
            mock.patch.object(meta.Store, "show", return_value=ready),
            mock.patch.object(meta, "ready", return_value=True),
            mock.patch("pipeline.clean") as clean,
        ):
            with self.assertRaisesRegex(RuntimeError, "declared role requires"):
                self.run_cli(*args, "--role", "worker")
            CopilotSession(self, "claude-sonnet-5.5").activate()
            with self.assertRaisesRegex(RuntimeError, "must declare --role"):
                self.run_cli(*args)
        clean.assert_not_called()
        self.assertEqual(
            [meta._authors("worker")[0][key] for key in ("client", "role", "family")],
            ["copilot", "worker", "claude"],
        )

    def test_c36_copilot_reviewer_needs_the_reviewer_role_for_reviews_and_revocation(self) -> None:
        number = self.issue()
        report = Path(self.temp.name) / "report.md"
        report.write_text("approved\n")
        session = CopilotSession(self, "claude-opus-5.5").activate()
        subject = f"{number}@r1"
        review = ["review", "spec", subject, "--verdict", "approved", "--report", str(report)]
        with self.assertRaisesRegex(RuntimeError, "must declare --role"):
            self.run_cli(*review)
        with self.assertRaisesRegex(RuntimeError, "require the reviewer role"):
            self.run_cli(*review, "--role", "worker")
        self.assertEqual(reviews.receipt_rows(self.store), [])
        rid = self.run_cli(*review, "--role", "reviewer").strip()
        (receipt,) = reviews.receipt_rows(self.store)
        self.assertEqual(receipt["reviewer"].get("session"), session.id)
        self.assertEqual(
            [receipt["reviewer"].get(key) for key in ("client", "role", "class", "family")],
            ["copilot", "reviewer", "A", "claude"],
        )
        revoke = ["review", "revoke", rid, "--reason", "superseded"]
        with self.assertRaisesRegex(RuntimeError, "must declare --role"):
            self.run_cli(*revoke)
        self.assertFalse(reviews.receipt_rows(self.store)[0]["revoked"])
        self.run_cli(*revoke, "--role", "reviewer")
        self.assertTrue(reviews.receipt_rows(self.store)[0]["revoked"])

    def test_c37_session_diagnostic_reports_without_writing(self) -> None:
        before = sorted(path.relative_to(self.hub) for path in self.hub.rglob("*"))
        CopilotSession(self, "claude-sonnet-5.5").activate()
        report = json.loads(self.run_cli("session", "--role", "worker"))
        self.assertEqual(
            (report["client"], report["role"], report["mapping"]),
            (
                "copilot",
                "worker",
                {"class": "B", "family": "claude", "model": "claude-sonnet-5-5", "effort": "high"},
            ),
        )
        self.assertTrue(report["identity"].startswith("copilot:"))
        unbound = json.loads(self.run_cli("session"))
        self.assertIsNone(unbound["role"])
        self.assertIn("must declare --role", unbound["error"])
        wrong = json.loads(self.run_cli("session", "--role", "reviewer"))
        self.assertIn("does not match role", wrong["error"])
        self.assertEqual(before, sorted(path.relative_to(self.hub) for path in self.hub.rglob("*")))

    def test_c01_role_alias_effort_and_bound_identity(self) -> None:
        self.assertEqual(
            classes.verify("reviewer", "copilot", [{"model": "claude-opus-5.5", "effort": "high"}])[
                "family"
            ],
            "claude",
        )
        self.assertEqual(
            classes.verify("manager", "claude", [{"model": "claude-opus-5-5", "effort": "high"}])[
                "class"
            ],
            "S",
        )
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            classes.verify("reviewer", "claude", [{"model": "claude-opus-5-5", "effort": "medium"}])
        row = {"name": "bound-worker", "brand": "codex", "session_id": "bound", "role": "manager"}
        self.bind(
            row,
            "codex",
            '{"type":"turn_context","payload":{"model":"gpt-6-astra","effort":"high"}}\n',
        )
        with self.assertRaisesRegex(RuntimeError, "contradicts"):
            provenance.make_payload(["CK-1"], "reviewer")

    def test_c02_copilot_utility_and_model_history(self) -> None:
        log = Path(self.temp.name) / "copilot.jsonl"
        log.write_text(
            "\n".join(
                [
                    json.dumps(
                        {
                            "type": "session.start",
                            "payload": {
                                "selectedModel": "claude-opus-5.5",
                                "reasoningEffort": "high",
                            },
                        }
                    ),
                    json.dumps({"type": "model.gpt-4o-mini", "payload": {}}),
                    json.dumps({"type": "model.gpt-5.6-luna-utility", "payload": {}}),
                    json.dumps({"type": "assistant.message", "data": {"model": "claude-opus-5.5"}}),
                    json.dumps(
                        {"type": "session.model_change", "payload": {"newModel": "gpt-6-luna"}}
                    ),
                    json.dumps({"type": "model.gpt-5.4-nano", "payload": {}}),
                    json.dumps({"type": "assistant.message", "data": {"model": "gpt-6-luna"}}),
                ]
            )
            + "\n"
        )
        history, _, _ = sessions.history(log, "copilot")
        self.assertEqual(
            history,
            [
                {"model": "claude-opus-5.5", "effort": "high"},
                {"model": "gpt-6-luna", "effort": "high"},
            ],
        )
        fallback = Path(self.temp.name) / "copilot-fallback.jsonl"
        fallback.write_text(
            json.dumps(
                {
                    "type": "session.start",
                    "payload": {"selectedModel": "claude-opus-5.5", "reasoningEffort": "high"},
                }
            )
            + "\n"
            + json.dumps({"type": "assistant.message", "data": {"model": "claude-opus-5.5"}})
            + "\n"
            + json.dumps({"type": "assistant.message", "data": {"model": "gpt-6-luna"}})
            + "\n"
        )
        row = {"name": "multi", "brand": "copilot", "session_id": "multi", "role": "reviewer"}
        self.bind(row, "copilot", fallback.read_text())
        models, _, _, current = sessions.parse(fallback, "copilot")
        self.assertEqual({item["model"] for item in models}, {"claude-opus-5.5", "gpt-6-luna"})
        self.assertEqual(current, {"model": "gpt-6-luna", "effort": "high"})
        # PF-12: the current pair is Class B on a Class A role, so the mixed history no longer
        # bypasses the current-selection check.
        with mock.patch("gate.sessions._session_log", return_value=fallback):
            with self.assertRaisesRegex(RuntimeError, "does not match role"):
                provenance.make_payload(["CK-1"])
            with self.assertRaisesRegex(RuntimeError, "does not match role"):
                reviews._reviewer("reviewer")
        # An on-class mixed history is recorded in full, and `ck review` still refuses the family.
        onclass = Path(self.temp.name) / "copilot-onclass.jsonl"
        onclass.write_text(fallback.read_text().replace("gpt-6-luna", "gpt-6.1-sol"))
        row = {"name": "multi2", "brand": "copilot", "session_id": "multi2", "role": "reviewer"}
        self.bind(row, "copilot", onclass.read_text())
        with mock.patch("gate.sessions._session_log", return_value=onclass):
            payload = provenance.make_payload(["CK-1"])
            self.assertEqual(
                {item["model"] for item in payload["models"]},
                {"claude-opus-5.5", "gpt-6.1-sol"},
            )
            self.assertCountEqual(payload["families"], ["claude", "codex"])
            self.assertEqual(payload["mapping"]["family"], "multi-family")
            self.assertEqual(
                (payload["mapping"]["model"], payload["mapping"]["effort"]), ("gpt-6.1-sol", "high")
            )
            self.assertEqual(payload["evidence"]["path"], str(onclass))
            with self.assertRaisesRegex(RuntimeError, "Unknown or multi-family"):
                reviews._reviewer("reviewer")

    def test_c03_codex_missing_log_and_unselected_assistant(self) -> None:
        fixture = Path(self.temp.name) / "codex.jsonl"
        fixture.write_text(
            '{"type":"turn_context","payload":{"model":"gpt-6-luna","effort":"high"}}\n'
        )
        self.assertEqual(
            sessions.history(fixture, "codex")[0], [{"model": "gpt-6-luna", "effort": "high"}]
        )
        unselected = Path(self.temp.name) / "unselected.jsonl"
        unselected.write_text('{"type":"assistant.message","data":{"model":"gpt-6-luna"}}\n')
        with self.assertRaisesRegex(RuntimeError, "Unselected agent model"):
            sessions.history(unselected, "copilot")
        row = {"name": "x", "brand": "codex", "session_id": "missing", "role": "worker"}
        self.bind(row, "codex", "")
        roots = json.loads(__import__("os").environ["CK_SESSION_ROOTS"])
        (Path(roots["codex"]) / "missing.jsonl").unlink()
        with self.assertRaisesRegex(RuntimeError, "missing or ambiguous"):
            sessions.resolve()

    def test_c04_record_payload_requires_session_and_raw_family_is_unknown(self) -> None:
        with mock.patch("gate.provenance.sessions.resolve", return_value={"human": True}):
            with self.assertRaisesRegex(RuntimeError, "requires verifiable"):
                provenance.make_payload(["CK-1"])
            with self.assertRaisesRegex(RuntimeError, "requires verifiable"):
                cli.run(
                    self.root, self.hub, ["record", "-m", "raw", "--issue", "CK-1", "source.txt"]
                )
        payload = {"schema": "ck-provenance/1", "issues": ["CK-1"], "identity": "worker"}
        self.assertEqual(provenance.parse_description("ordinary raw patch"), None)
        self.assertEqual(
            reviews.requirements([{"identity": "raw", "family": "unknown"}]), {"claude", "codex"}
        )
        self.assertEqual(
            provenance.parse_description(
                provenance.PREFIX
                + provenance.payload_digest(payload)
                + "\n"
                + pointer.canonical(payload).decode()
            ),
            payload,
        )

    def test_c05_concurrent_issue_ids_are_unique(self) -> None:
        spec = Path(self.temp.name) / "spec.md"
        spec.write_text("spec\n")
        barrier = threading.Barrier(2)
        ids = []
        failures = []

        original = meta._authors

        def create(title: str) -> None:
            try:
                barrier.wait()
                ids.append(self.store.new_issue(title, "task", "P2", spec))
            except Exception as error:  # pragma: no cover - assertion reports concurrent failure
                failures.append(error)

        threads = [
            threading.Thread(target=create, args=(f"worker-{n}",), name=f"worker-{n}")
            for n in range(2)
        ]
        # Patch once in the main thread: overlapping per-thread patches can restore a stale mock.
        with mock.patch(
            "gate.meta._authors",
            side_effect=lambda *a, **k: [self.agent(session=threading.current_thread().name)],
        ):
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join()
        self.assertIs(meta._authors, original)
        self.assertEqual(failures, [])
        self.assertEqual(len(set(ids)), 2)

    def test_c06_spec_revision_invalidates_but_descriptive_edit_preserves(self) -> None:
        identifier = self.issue("revision")
        before = self.shown(identifier)
        self.store.append_issue_event(identifier, "edit", {"title": "renamed"})
        descriptive = self.shown(identifier)
        self.assertEqual(descriptive["spec_sha256"], before["spec_sha256"])
        source = Path(self.temp.name) / "revised.md"
        source.write_text("new contract\n")
        self.store.append_issue_event(identifier, "spec", {}, source.read_bytes())
        after = self.shown(identifier)
        self.assertEqual(after["revision"], "r2")
        self.assertNotEqual(after["spec_sha256"], before["spec_sha256"])
        self.assertNotEqual(reviews.subject_for_spec(after), reviews.subject_for_spec(before))

    def test_c07_dirty_metadata_requires_recovery_and_event_edits_are_detected(self) -> None:
        self.store.write(
            self.store.repo / "events/e.json", pointer.canonical({"kind": "test"}), "test"
        )
        event = self.store.repo / "events/e.json"
        event.write_text('{"kind":"edited"}\n')
        with self.assertRaisesRegex(RuntimeError, "dirty"):
            self.store.events()
        self.store.recover()
        self.assertEqual(self.store.events()[0]["kind"], "edited")

    def test_c08_author_families_require_opposite_and_both_when_mixed(self) -> None:
        self.assertEqual(reviews.requirements([person("a", "s", "claude")]), {"codex"})
        self.assertEqual(reviews.requirements([person("a", "s", "codex")]), {"claude"})
        mixed: Author = {"identity": "a", "family": "unknown", "families": ["claude", "codex"]}
        for author in [
            mixed,
            person("a", "s", "unknown"),
            person("a", "s", "human"),
            mixed,
        ]:
            self.assertEqual(reviews.requirements([author]), {"claude", "codex"})

    def test_c09_author_and_wrong_reviewer_provenance_are_refused(self) -> None:
        author = person("same", "s1", "codex")
        self.assertFalse(reviews.independent({"identity": "same", "session": "s2"}, [author]))
        self.assertFalse(reviews.independent({"identity": "other", "session": "s1"}, [author]))
        subject = subject_of("S")
        for reviewer in [
            reviewer_of("r", "r1", rank="B"),
            reviewer_of("r", "r1", family="multi-family"),
            reviewer_of("r", "r1", effort="medium"),
        ]:
            self.assertFalse(reviews.coverage([author], [row_of(subject, reviewer)], subject))

    def test_c10_coverage_is_exact_and_incremental_requires_covered_subset(self) -> None:
        author = person("a", "a1", "codex")
        reviewer = reviewer_of("r", "r1")
        parent = subject_of("P")
        child = subject_of("S")
        receipts = [
            row_of(parent, reviewer),
            row_of(child, reviewer_of("r", "r2"), incremental_over="P"),
        ]
        manifests: dict[str, ManifestInfo] = {
            "P": {"subject": parent, "hashes": ["p1"], "authors": [author]},
            "S": {"subject": child, "hashes": ["p1", "s1"], "authors": [author]},
        }
        self.assertTrue(reviews.coverage([author], receipts, child, manifests))
        self.assertFalse(reviews.coverage([author], receipts, subject_of("subset"), manifests))
        manifests["S"]["hashes"] = ["s1"]
        self.assertFalse(reviews.coverage([author], receipts, child, manifests))

    def test_c11_revoked_stale_and_dead_submissions_are_ineligible(self) -> None:
        manifest = legacy("S", ["p1"])
        subject = subject_of("S")
        good = row_of(subject, reviewer_of("r", "r1"))
        author = [person("a", "a1", "codex")]
        self.assertTrue(reviews.coverage(author, [good], subject))
        self.assertFalse(reviews.coverage(author, [{**good, "revoked": True}], subject))
        self.assertFalse(
            reviews.coverage(author, [{**good, "subject": subject_of("edited")}], subject)
        )
        withdrawn: MetaEvent = {"kind": "withdrawn", "submission": "S"}
        superseded: MetaEvent = {"kind": "supersession", "submission": "S"}
        self.assertFalse(reviews.live(manifest, [withdrawn], set()))
        self.assertFalse(reviews.live(manifest, [superseded], set()))

    def test_c12_superseded_changes_request_does_not_block_but_live_sibling_does(self) -> None:
        predecessor = legacy("P", ["h"])
        successor = legacy("S", ["h", "n"])
        receipt = row_of(subject_of("P"), reviewer_of("r", "r1"), verdict="changes-requested")
        self.assertFalse(
            reviews.blocking(successor, [predecessor, successor], [receipt], set(), {"P"})
        )
        self.assertTrue(
            reviews.blocking(
                successor,
                [successor, legacy("T", ["h"])],
                [{**receipt, "subject": subject_of("T")}],
                set(),
                set(),
            )
        )
        self.assertFalse(
            reviews.blocking(successor, [successor, legacy("T", ["z"])], [], set(), set())
        )

    def test_c13_docs_path_classification_rejects_executable_markdown(self) -> None:
        for path in ("README.md", "docs/guide.md"):
            self.assertTrue(doc_path(path))
        for path in ("skills/example/SKILL.md", "AGENTS.md", "tests/fixture.md"):
            self.assertFalse(doc_path(path))

    # ---- model-switch readiness (spec cases 1-9) ----

    OPUS, SONNET = ("claude-opus-5-5", "high"), ("claude-sonnet-5-5", "high")
    OPUS_MEDIUM = ("claude-opus-5-5", "medium")
    SONNET_ENTRY: ClassVar[dict[str, object]] = {
        "model": "claude-sonnet-5-5",
        "effort": "high",
        "events": 2,
    }

    @staticmethod
    def claude_log(*pairs: tuple[str, str]) -> str:
        return "".join(
            json.dumps({"type": "assistant", "message": {"model": model}, "effort": effort}) + "\n"
            for model, effort in pairs
        )

    @staticmethod
    def codex_log(*contexts: Mapping[str, str]) -> str:
        return "".join(
            json.dumps({"type": "turn_context", "payload": context}) + "\n" for context in contexts
        )

    @staticmethod
    def copilot_log(*events: Mapping[str, object]) -> str:
        return "".join(json.dumps(event) + "\n" for event in events)

    @staticmethod
    def selection(model: str, effort: str = "high") -> dict[str, object]:
        return {
            "type": "session.start",
            "data": {"selectedModel": model, "reasoningEffort": effort},
        }

    @staticmethod
    def change(model: str, effort: str | None = None) -> dict[str, object]:
        data = {"newModel": model}
        if effort:
            data["reasoningEffort"] = effort
        return {"type": "session.model_change", "data": data}

    @staticmethod
    def spoke(model: str | None = None, kind: str = "assistant.message") -> dict[str, object]:
        return {"type": kind, "data": {"model": model} if model else {}}

    def bound(self, client: str, role: str, content: str, name: str = "native") -> Path:
        row = {"name": f"{name}-identity", "brand": client, "session_id": name, "role": role}
        self.bind(row, client, content)
        return Path(json.loads(os.environ["CK_SESSION_ROOTS"])[client]) / f"{name}.jsonl"

    def readiness_of(
        self, client: str, role: str, content: str
    ) -> tuple[RoleMapping, ModelUse | None]:
        log = Path(self.temp.name) / f"{client}-readiness.jsonl"
        log.write_text(content)
        models, _, _, current = sessions.parse(log, client)
        return classes.readiness(role, client, models, current), current

    def author_events(self, number: str) -> dict[str, IssueEvent]:
        root = self.store.repo / "issues" / number
        return {
            path.name.split("-", 1)[1]: load_issue_event(json.loads(path.read_bytes()), str(path))
            for path in sorted(root.glob("[0-9]*-*.json"))
        }

    def test_m01_observed_case_is_ready_and_keeps_the_whole_history(self) -> None:
        content = self.claude_log(self.SONNET, self.SONNET, self.OPUS)
        self.bound("claude", "manager", content)
        report = json.loads(self.run_cli("session", "--role", "manager"))
        self.assertNotIn("error", report)
        self.assertEqual(report["current"], {"model": "claude-opus-5-5", "effort": "high"})
        self.assertEqual(
            report["mapping"],
            {
                "class": "S",
                "family": "claude",
                "model": "claude-opus-5-5",
                "effort": "high",
                "off_class": [self.SONNET_ENTRY],
            },
        )
        payload = provenance.make_payload(["CK-1"])
        self.assertEqual(
            payload["models"],
            [
                {"model": "claude-sonnet-5-5", "effort": "high", "turns": 2},
                {"model": "claude-opus-5-5", "effort": "high", "turns": 1},
            ],
        )
        self.assertEqual(payload["mapping"]["off_class"], [self.SONNET_ENTRY])
        self.assertEqual(payload["families"], ["claude"])
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            classes.verify("manager", "claude", sessions.expand_history(payload["models"]))

    def test_ck143_long_histories_store_as_runs_and_digest_the_stored_form(self) -> None:
        def model_use(model: str, effort: str) -> ModelUse:
            return {"model": model, "effort": effort}

        history: list[ModelUse] = (
            [model_use("model-a", "high") for _ in range(4_000)]
            + [model_use("model-b", "medium") for _ in range(3_500)]
            + [model_use("model-a", "high") for _ in range(2_500)]
        )
        runs = sessions.compress_history(history)
        self.assertEqual(
            runs,
            [
                {"model": "model-a", "effort": "high", "turns": 4_000},
                {"model": "model-b", "effort": "medium", "turns": 3_500},
                {"model": "model-a", "effort": "high", "turns": 2_500},
            ],
        )
        self.assertEqual(sessions.expand_history(runs), history)

        same = {"model": "gpt-6.1-sol", "effort": "high"}
        session = {
            "identity": "bound-worker",
            "client": "codex",
            "session": "long-session",
            "role": "worker",
            "models": [same] * 500_000,
            "current": same,
            "families": ["codex"],
            "evidence": {"path": "/session.jsonl", "offset": 1, "sha256": "0" * 64},
        }
        mapping = {
            "class": "B",
            "family": "codex",
            "model": "gpt-6.1-sol",
            "effort": "high",
            "families": ["codex"],
        }
        with (
            mock.patch("gate.provenance.sessions.resolve", return_value=session),
            mock.patch("gate.provenance.classes.readiness", return_value=mapping),
        ):
            payload = provenance.make_payload(["CK-143"])
        models_text = json.dumps(payload["models"], separators=(",", ":"))
        self.assertLess(len(models_text.encode()), 4_000)
        self.assertEqual(payload["models"], [{**same, "turns": 500_000}])
        description = (
            provenance.PREFIX
            + provenance.payload_digest(payload)
            + "\n"
            + pointer.canonical(payload).decode()
        )
        self.assertEqual(provenance.parse_description(description), payload)

    def test_ck143_old_and_run_shapes_validate_with_the_same_author_coverage(self) -> None:
        old = {
            "schema": "ck-provenance/1",
            "identity": "worker",
            "client": "claude",
            "session": "s-1",
            "families": ["claude"],
            "models": [
                {"model": "claude-sonnet-5-5", "effort": "high"},
                {"model": "claude-sonnet-5-5", "effort": "high"},
            ],
        }
        compact = {
            **old,
            "models": [{"model": "claude-sonnet-5-5", "effort": "high", "turns": 2}],
        }
        old_loaded = load_provenance(old, "old payload")
        compact_loaded = load_provenance(compact, "compact payload")
        self.assertEqual(
            sessions.expand_history(old_loaded["models"]),
            sessions.expand_history(compact_loaded["models"]),
        )
        self.assertEqual(
            provenance.authors({"PATCH": old_loaded}),
            provenance.authors({"PATCH": compact_loaded}),
        )
        self.assertEqual(
            load_author(
                {"identity": "worker", "family": "claude", "models": old["models"]}, "old author"
            )["models"],
            old["models"],
        )
        self.assertEqual(
            load_author(
                {"identity": "worker", "family": "claude", "models": compact["models"]},
                "compact author",
            )["models"],
            compact["models"],
        )

    def test_ck143_model_run_turns_must_be_positive_non_boolean_integers(self) -> None:
        base = {
            "schema": "ck-provenance/1",
            "identity": "worker",
            "client": "claude",
            "session": "s-1",
            "families": ["claude"],
        }
        for invalid in (0, -1):
            with self.subTest(turns=invalid), self.assertRaisesRegex(RecordError, "at least 1"):
                load_provenance(
                    {**base, "models": [{"model": "m", "effort": "high", "turns": invalid}]},
                    "payload",
                )
        invalid_values: list[object] = [True, 1.5, "2"]
        for invalid_non_integer in invalid_values:
            with (
                self.subTest(turns=invalid_non_integer),
                self.assertRaisesRegex(RecordError, "integer"),
            ):
                load_provenance(
                    {
                        **base,
                        "models": [{"model": "m", "effort": "high", "turns": invalid_non_integer}],
                    },
                    "payload",
                )

    def test_m02_reconciled_author_evidence_survives_storage(self) -> None:
        content = self.claude_log(self.SONNET, self.SONNET, self.OPUS)
        log = self.bound("claude", "manager", content)
        raw = log.read_bytes()
        expected = {
            "off_class": [self.SONNET_ENTRY],
            "models": [
                {"model": "claude-sonnet-5-5", "effort": "high", "turns": 2},
                {"model": "claude-opus-5-5", "effort": "high", "turns": 1},
            ],
            "evidence": {
                "path": str(log),
                "offset": len(raw),
                "sha256": hashlib.sha256(raw).hexdigest(),
            },
        }
        spec = self.spec_file()
        number = self.run_cli(
            "issue", "new", "--title", "t", "--type", "task", "--spec", str(spec)
        ).strip()
        self.run_cli("issue", "spec", number, "--file", str(spec))
        events = self.author_events(number)
        for name in ("created.json", "spec.json"):
            with self.subTest(name):
                (author,) = events[name]["authors"]
                self.assertEqual(author["families"], ["claude"])
                self.assertEqual({k: dict(author)[k] for k in expected}, expected)
        self.assertEqual(self.shown(number)["spec_authors"][0]["off_class"], [self.SONNET_ENTRY])

        # RC-2: the submitting session is recorded as `submitter` (with its reconciled evidence);
        # the manifest's authors come from the patches, here one raw patch: unknown.
        manifest = self.submitted(number)
        submitter = manifest["submitter"]
        self.assertEqual(submitter["families"], ["claude"])
        self.assertEqual({k: dict(submitter)[k] for k in expected}, expected)
        self.assertEqual([a["family"] for a in manifest["authors"]], ["unknown"])

        self.bound("claude", "manager", self.claude_log(self.OPUS), name="strict")
        strict = meta._authors()[0]
        self.assertEqual(
            strict,
            {
                "identity": "strict-identity",
                "client": "claude",
                "session": "strict",
                "role": "manager",
                "family": "claude",
                "families": ["claude"],
            },
        )
        number = self.run_cli(
            "issue", "new", "--title", "t", "--type", "task", "--spec", str(spec)
        ).strip()
        stored = self.author_events(number)["created.json"]["authors"][0]
        self.assertEqual(stored, strict)
        self.assertEqual(self.submitted(number)["submitter"], strict)

    def submitted(self, number: str) -> ManifestV2:
        """Run the real `ck submit` author path; only the Pijul closure replay is replaced."""
        args = ["submit", "--issue", number, "--spec", "x"]
        pointer_path = Path(self.temp.name) / "pointer.json"
        state = {
            "fingerprint": {"hashes": [], "files": {}},
            "_pointer_path": str(pointer_path),
            "submissions": [],
        }
        pointer_path.write_text("{}")
        with (
            mock.patch.object(meta, "ready", return_value=True),
            mock.patch("pipeline.clean"),
            mock.patch("pipeline.hashes", return_value={"h1"}),
            mock.patch("pipeline.clone"),
            mock.patch("pipeline.fingerprint", return_value={"hashes": [], "files": {}}),
            mock.patch("gate.pointer.read_pointer", return_value=state),
            mock.patch("gate.docsqueue.outstanding_hashes", return_value=set()),
            mock.patch("gate.docsqueue._queue", return_value=None),
            mock.patch("gate.cli._closure_files", return_value=["a.txt"]),
            mock.patch("gate.provenance.from_capture", return_value={"h1": None}),
            mock.patch("gate.batch.Manager.manifests", return_value=[]),
        ):
            identifier = self.run_cli(*args).strip()
        queued = self.hub / ".pipeline/queue" / f"{identifier}.json"
        manifest = load_manifest(json.loads(queued.read_bytes()), str(queued))
        assert is_manifest_v2(manifest)
        return manifest

    def test_m03_switching_away_fails_in_both_directions_for_every_client(self) -> None:
        opus_copilot, sonnet_copilot = "claude-opus-5.5", "claude-sonnet-5.5"
        astra, luna = "gpt-6-astra", "gpt-6-luna"
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            self.readiness_of("claude", "manager", self.claude_log(self.OPUS, self.SONNET))
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            self.readiness_of("claude", "manager", self.claude_log(self.OPUS, self.OPUS_MEDIUM))

        away = self.copilot_log(
            self.selection(opus_copilot), self.spoke(opus_copilot), self.change(sonnet_copilot)
        )
        log = Path(self.temp.name) / "away.jsonl"
        for name, events in (
            ("sonnet", away),
            (
                "effort",
                self.copilot_log(
                    self.selection(opus_copilot),
                    self.spoke(opus_copilot),
                    self.change(opus_copilot, "medium"),
                ),
            ),
            (
                "back-without-confirmation",
                self.copilot_log(
                    self.selection(sonnet_copilot),
                    self.spoke(sonnet_copilot),
                    self.change(opus_copilot),
                ),
            ),
        ):
            with self.subTest(name):
                log.write_text(events)
                models, _, _, current = sessions.parse(log, "copilot")
                self.assertIsNone(current)
                with self.assertRaisesRegex(RuntimeError, "unresolved"):
                    classes.readiness("manager", "copilot", models, current)
        confirmed, current = self.readiness_of(
            "copilot",
            "manager",
            self.copilot_log(
                self.selection(sonnet_copilot),
                self.spoke(sonnet_copilot),
                self.change(opus_copilot),
                self.spoke(kind="tool.execution_start"),
            ),
        )
        self.assertEqual(current, {"model": opus_copilot, "effort": "high"})
        self.assertEqual(
            confirmed["off_class"], [{"model": "claude-sonnet-5-5", "effort": "high", "events": 1}]
        )

        with self.assertRaisesRegex(RuntimeError, "unresolved"):
            self.readiness_of(
                "codex",
                "manager",
                self.codex_log({"model": astra, "effort": "high"}, {"model": astra}),
            )
        recovered, _ = self.readiness_of(
            "codex",
            "manager",
            self.codex_log({"model": luna, "effort": "high"}, {"model": astra, "effort": "high"}),
        )
        self.assertEqual(recovered["off_class"], [{"model": luna, "effort": "high", "events": 1}])
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            self.readiness_of(
                "codex",
                "manager",
                self.codex_log(
                    {"model": astra, "effort": "high"}, {"model": astra, "effort": "medium"}
                ),
            )

    def test_m04_effort_reconciliation_for_a_manager(self) -> None:
        result, _ = self.readiness_of(
            "claude", "manager", self.claude_log(self.OPUS_MEDIUM, self.OPUS)
        )
        self.assertEqual(
            result["off_class"], [{"model": "claude-opus-5-5", "effort": "medium", "events": 1}]
        )

    def test_m05_same_family_changes_for_a_worker(self) -> None:
        result, _ = self.readiness_of(
            "claude", "worker", self.claude_log(self.SONNET, self.OPUS, self.SONNET)
        )
        self.assertEqual(result["class"], "B")
        self.assertEqual(
            result["off_class"], [{"model": "claude-opus-5-5", "effort": "high", "events": 1}]
        )

    def test_m06_reviews_stay_strict_over_the_whole_history(self) -> None:
        self.bound("claude", "reviewer", self.claude_log(self.SONNET, self.OPUS))
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            reviews._reviewer("reviewer")
        self.assertIn(
            "does not match role",
            json.loads(self.run_cli("session", "--role", "reviewer"))["error"],
        )
        self.bound("claude", "reviewer", self.claude_log(self.OPUS), name="strict")
        identity, _ = reviews._reviewer("reviewer")
        self.assertEqual((identity["class"], identity["family"]), ("A", "claude"))
        report = json.loads(self.run_cli("session", "--role", "reviewer"))
        self.assertNotIn("off_class", report["mapping"])
        self.assertNotIn("error", report)
        self.bound(
            "codex",
            "reviewer",
            self.codex_log({"model": "gpt-6.1-sol", "effort": "high"}, {"model": "gpt-6.1-sol"}),
            name="trailing",
        )
        with self.assertRaisesRegex(RuntimeError, "unresolved"):
            reviews._reviewer("reviewer")

    def test_m07_mixed_and_unknown_histories_keep_their_mapping(self) -> None:
        opus, luna = "claude-opus-5.5", "gpt-6-luna"
        confirmed = self.copilot_log(
            self.selection(opus), self.spoke(opus), self.change(luna), self.spoke(luna)
        )
        log = self.bound("copilot", "reviewer", confirmed)
        models, _, _, current = sessions.parse(log, "copilot")
        self.assertEqual(current, {"model": luna, "effort": "high"})
        # PF-12: a mixed-history reviewer whose current pair is Class B is refused.
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            classes.readiness("reviewer", "copilot", models, current)
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            provenance.make_payload(["CK-1"])
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            reviews._reviewer("reviewer")

        sol = "gpt-6.1-sol"
        on_class = self.copilot_log(
            self.selection(opus), self.spoke(opus), self.change(sol), self.spoke(sol)
        )
        log = self.bound("copilot", "reviewer", on_class, name="onclass")
        models, _, _, current = sessions.parse(log, "copilot")
        mapping = classes.readiness("reviewer", "copilot", models, current)
        verified = classes.verify("reviewer", "copilot", models)
        # The history classification is exactly verify's; the rest is the current expectation.
        self.assertEqual({k: dict(mapping)[k] for k in verified}, verified)
        self.assertEqual(
            (mapping["model"], mapping["effort"], mapping["current_family"]),
            (sol, "high", "codex"),
        )
        self.assertEqual(
            mapping["off_class"],
            [{"model": "claude-opus-5-5", "effort": "high", "events": 1, "family": "claude"}],
        )
        payload = provenance.make_payload(["CK-1"])
        self.assertCountEqual(payload["families"], ["claude", "codex"])
        with self.assertRaisesRegex(RuntimeError, "Unknown or multi-family"):
            reviews._reviewer("reviewer")

        unconfirmed = self.copilot_log(
            self.selection(opus), self.spoke(opus), self.spoke(luna), self.change(opus)
        )
        self.bound("copilot", "reviewer", unconfirmed)
        with self.assertRaisesRegex(RuntimeError, "unresolved"):
            provenance.make_payload(["CK-1"])
        with self.assertRaisesRegex(RuntimeError, "unresolved"):
            reviews._reviewer("reviewer")

        # PF-12: an author whose current model is unknown is refused.
        self.bound("codex", "worker", self.codex_log({"model": "mystery-9", "effort": "high"}))
        with self.assertRaisesRegex(RuntimeError, "current model family is unresolved"):
            meta._authors()
        self.bound(
            "codex",
            "worker",
            self.codex_log(
                {"model": "mystery-9", "effort": "high"}, {"model": luna, "effort": "high"}
            ),
            name="unknown-then-known",
        )
        (author,) = meta._authors()
        self.assertEqual(
            (author["family"], author["families"]), ("multi-family", ["codex", "unknown"])
        )
        self.assertEqual(reviews.requirements([author]), {"claude", "codex"})

    def test_m08_strict_sessions_are_unchanged(self) -> None:
        fixtures = Path(__file__).parent / "fixtures/sessions"
        cases = [(fixtures / "codex.jsonl", "codex", "worker")]
        claude = Path(self.temp.name) / "claude.jsonl"
        claude.write_text(self.claude_log(self.SONNET))
        cases.append((claude, "claude", "worker"))
        for path, client, role in cases:
            with self.subTest(client):
                models, offset, digest, current = sessions.parse(path, client)
                self.assertEqual(sessions.history(path, client), (models, offset, digest))
                self.assertIsNotNone(current)
                self.assertEqual(
                    classes.readiness(role, client, models, current),
                    classes.verify(role, client, models),
                )
        # The Copilot fixture is a mixed Opus -> Luna history whose current Luna/high pair is
        # Class B, so it is judged as a worker (PF-12): the new mapping, not readiness == verify.
        copilot = fixtures / "copilot.jsonl"
        models, offset, digest, current = sessions.parse(copilot, "copilot")
        self.assertEqual(sessions.history(copilot, "copilot"), (models, offset, digest))
        self.assertEqual(current, {"model": "gpt-6-luna", "effort": "high"})
        mapping = classes.readiness("worker", "copilot", models, current)
        verified = classes.verify("worker", "copilot", models)
        self.assertEqual({key: dict(mapping)[key] for key in verified}, verified)
        self.assertEqual(
            (mapping["model"], mapping["effort"], mapping["current_family"]),
            ("gpt-6-luna", "high", "codex"),
        )
        self.assertEqual(
            mapping["off_class"],
            [{"model": "claude-opus-5-5", "effort": "high", "events": 1, "family": "claude"}],
        )
        old_author: Author = {
            "identity": "a",
            "client": "claude",
            "session": "s",
            "role": "manager",
            "family": "claude",
            "families": ["claude"],
        }
        subject = subject_of("S")
        self.assertTrue(
            reviews.coverage(
                [old_author], [row_of(subject, reviewer_of("r", "r1", family="codex"))], subject
            )
        )
        old_payload = {"schema": "ck-provenance/1", "issues": ["CK-1"], "mapping": {"class": "S"}}
        self.assertEqual(
            provenance.parse_description(
                provenance.PREFIX
                + provenance.payload_digest(old_payload)
                + "\n"
                + pointer.canonical(old_payload).decode()
            ),
            old_payload,
        )

    def test_m09_bound_role_contradiction_is_still_refused(self) -> None:
        number = self.issue()
        self.bound("claude", "manager", self.claude_log(self.SONNET, self.OPUS))
        spec = self.spec_file()
        for argv in (
            ["issue", "new", "--title", "t", "--type", "task", "--spec", str(spec)],
            ["issue", "close", number, "--reason", "wontfix"],
            ["record", "-m", "x", "--issue", "CK-1"],
            ["review", "spec", f"{number}@r1", "--verdict", "approved", "--report", str(spec)],
        ):
            with (
                self.subTest(argv[:2]),
                self.assertRaisesRegex(RuntimeError, "contradicts"),
            ):
                self.run_cli(*argv, "--role", "worker")
        ready = {"id": "CK-1", "revision": "r1", "spec_sha256": "0"}
        with (
            mock.patch.object(meta.Store, "show", return_value=ready),
            mock.patch.object(meta, "ready", return_value=True),
            mock.patch("pipeline.clean") as clean,
            self.assertRaisesRegex(RuntimeError, "contradicts"),
        ):
            self.run_cli("submit", "--issue", "CK-1", "--spec", "x", "--role", "worker")
        clean.assert_not_called()

    # ---- current-class readiness for mixed and unknown histories (PF-12, CK-9 tests 1-7) ----

    MASTER = """schema_version = 1

[classes.S]
claude = { model = "claude-opus-5-5", effort = "high" }
codex = { model = "gpt-6-astra", effort = "high" }

[classes.A]
claude = { model = "claude-opus-5-5", effort = "high" }
codex = { model = "gpt-6.1-sol", effort = "high" }

[classes.B]
claude = { model = "claude-sonnet-5-5", effort = "high" }
codex = { model = "gpt-6-luna", effort = "high" }

[clients.copilot.model_aliases]
"claude-opus-5-5" = "claude-opus-5.5"
"claude-sonnet-5-5" = "claude-sonnet-5.5"
"""
    COPILOT_SPELLING: ClassVar[dict[str, str]] = {
        "claude-opus-5-5": "claude-opus-5.5",
        "claude-sonnet-5-5": "claude-sonnet-5.5",
    }

    def fixture_master(self) -> str:
        path = Path(self.temp.name) / "model-classes.toml"
        path.write_text(self.MASTER)
        patcher = mock.patch.dict(os.environ, {"CK_MODEL_CLASSES": str(path)})
        patcher.start()
        self.addCleanup(patcher.stop)
        return hashlib.sha256(path.read_bytes()).hexdigest()

    def client_log(self, client: str, *pairs: tuple[str, str]) -> str:
        """Fixture log bytes for `client` whose last pair is the current selection."""
        if client == "claude":
            return self.claude_log(*pairs)
        if client == "codex":
            return self.codex_log(*({"model": model, "effort": effort} for model, effort in pairs))
        events: list[dict[str, object]] = []
        for index, (model, effort) in enumerate(pairs):
            name = self.COPILOT_SPELLING.get(model, model)
            events.append(self.selection(name, effort) if index == 0 else self.change(name, effort))
            events.append(self.spoke(name))
        return self.copilot_log(*events)

    OPUS_H, SONNET_H = ("claude-opus-5-5", "high"), ("claude-sonnet-5-5", "high")
    ASTRA, LUNA, SOL = ("gpt-6-astra", "high"), ("gpt-6-luna", "high"), ("gpt-6.1-sol", "high")

    def test_h01_mixed_history_with_an_off_class_current_is_refused(self) -> None:
        self.fixture_master()
        for label, role, pairs in (
            ("low effort", "manager", (self.OPUS_H, ("gpt-6-astra", "low"))),
            ("Class B model on a manager", "manager", (self.OPUS_H, self.LUNA)),
            ("low effort worker", "worker", (self.SONNET_H, ("gpt-6-luna", "low"))),
            ("Class S model on a worker", "worker", (self.SONNET_H, self.ASTRA)),
        ):
            with self.subTest(label):
                content = self.client_log("copilot", *pairs)
                log = Path(self.temp.name) / "mixed.jsonl"
                log.write_text(content)
                models, _, _, current = sessions.parse(log, "copilot")
                with self.assertRaisesRegex(RuntimeError, "does not match role"):
                    classes.readiness(role, "copilot", models, current)

    def test_h02_mixed_history_with_an_on_class_current_is_accepted_with_a_full_mapping(
        self,
    ) -> None:
        master_sha = self.fixture_master()
        content = self.client_log("copilot", self.OPUS_H, self.LUNA, self.ASTRA)
        mapping, current = self.readiness_of("copilot", "manager", content)
        self.assertEqual(current, {"model": "gpt-6-astra", "effort": "high"})
        self.assertEqual(
            mapping,
            {
                "class": "S",
                "family": "multi-family",
                "families": ["claude", "codex"],
                "current_family": "codex",
                "model": "gpt-6-astra",
                "effort": "high",
                "master_sha256": master_sha,
                "off_class": [
                    {"model": "claude-opus-5-5", "effort": "high", "events": 1, "family": "claude"},
                    {"model": "gpt-6-luna", "effort": "high", "events": 1, "family": "codex"},
                ],
            },
        )

    def test_h03_unknown_model_in_history_then_an_on_class_current(self) -> None:
        self.fixture_master()
        mystery = ("mystery-9", "high")
        mapping, _ = self.readiness_of(
            "codex", "manager", self.client_log("codex", mystery, self.ASTRA)
        )
        # Today's exact classification, kept: multi-family with both history families.
        self.assertEqual(
            (mapping["family"], mapping["families"]), ("multi-family", ["codex", "unknown"])
        )
        self.assertEqual(mapping["current_family"], "codex")
        self.assertEqual(
            (mapping["class"], mapping["model"], mapping["effort"]), ("S", "gpt-6-astra", "high")
        )
        self.assertEqual(
            mapping["off_class"],
            [{"model": "mystery-9", "effort": "high", "events": 1, "family": "unknown"}],
        )
        log = Path(self.temp.name) / "unknown-history.jsonl"
        log.write_text(self.client_log("codex", mystery, self.ASTRA))
        models, _, _, _ = sessions.parse(log, "codex")
        verified = classes.verify("manager", "codex", models)
        self.assertEqual({k: dict(mapping)[k] for k in verified}, verified)
        # Coverage still needs both reviewer families.
        author: Author = {
            "identity": "a",
            "family": mapping["family"],
            "families": mapping["families"],
        }
        self.assertEqual(reviews.requirements([author]), {"claude", "codex"})

    def test_h03a_an_all_unknown_history_is_refused_as_unresolved(self) -> None:
        self.fixture_master()
        for client in ("claude", "codex", "copilot"):
            with self.subTest(client):
                content = self.client_log(client, ("mystery-9", "high"), ("mystery-8", "high"))
                with self.assertRaisesRegex(
                    RuntimeError, "Session current model family is unresolved for role worker"
                ):
                    self.readiness_of(client, "worker", content)

    def test_h04_an_unknown_current_model_is_refused_as_unresolved(self) -> None:
        self.fixture_master()
        for role, pairs in (
            ("manager", (self.OPUS_H, ("mystery-9", "high"))),
            ("worker", (self.LUNA, ("mystery-9", "high"))),
            ("reviewer", (self.SOL, ("mystery-9", "high"))),
        ):
            with (
                self.subTest(role),
                self.assertRaisesRegex(
                    RuntimeError, f"Session current model family is unresolved for role {role}"
                ),
            ):
                self.readiness_of("codex", role, self.client_log("codex", *pairs))

    def test_h05_a_mixed_history_reviewer_is_refused_off_class_and_cannot_review_on_class(
        self,
    ) -> None:
        self.fixture_master()
        off = self.client_log("copilot", self.OPUS_H, self.LUNA)
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            self.readiness_of("copilot", "reviewer", off)
        self.bound("copilot", "reviewer", off, name="reviewer-off")
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            reviews._reviewer("reviewer")
        on = self.client_log("copilot", self.OPUS_H, self.SOL)
        mapping, _ = self.readiness_of("copilot", "reviewer", on)
        self.assertEqual(
            (mapping["family"], mapping["families"]), ("multi-family", ["claude", "codex"])
        )
        self.bound("copilot", "reviewer", on, name="reviewer-on")
        with self.assertRaisesRegex(RuntimeError, "Unknown or multi-family sessions cannot review"):
            reviews._reviewer("reviewer")

    def test_h06_coverage_of_a_mixed_history_author_is_unchanged(self) -> None:
        self.fixture_master()
        self.bound(
            "copilot", "manager", self.client_log("copilot", self.OPUS_H, self.ASTRA), name="mixed"
        )
        (author,) = meta._authors()
        self.assertEqual(
            (author["family"], author["families"]), ("multi-family", ["claude", "codex"])
        )
        subject = subject_of("S")

        def approvals(*families: str) -> list[ReviewRow]:
            return [
                row_of(
                    subject,
                    reviewer_of(f"{family}-reviewer", f"{family}-1", family=family),
                )
                for family in families
            ]

        self.assertFalse(reviews.coverage([author], approvals("claude"), subject))
        self.assertFalse(reviews.coverage([author], approvals("codex"), subject))
        self.assertTrue(reviews.coverage([author], approvals("claude", "codex"), subject))

    def test_h07_every_client_parser_flows_through_session_and_authors(self) -> None:
        self.fixture_master()
        for client in ("claude", "codex", "copilot"):
            for role, expected, other, wrong in (
                ("worker", self.LUNA, self.SONNET_H, self.ASTRA),
                ("manager", self.ASTRA, self.OPUS_H, self.LUNA),
            ):
                with self.subTest(client=client, role=role):
                    self.check_client_flow(client, role, expected, other, wrong)

    def check_client_flow(
        self,
        client: str,
        role: str,
        expected: tuple[str, str],
        other: tuple[str, str],
        wrong: tuple[str, str],
    ) -> None:
        name = f"{client}-{role}"
        low = (expected[0], "low")
        # Refused: off-class current (low effort, then a wrong-class model).
        for label, pairs in (("low effort", (other, low)), ("wrong class", (other, wrong))):
            log = self.bound(client, role, self.client_log(client, *pairs), name=f"{name}-{label}")
            report = json.loads(self.run_cli("session", "--role", role))
            self.assertIn("does not match role", report["error"], label)
            self.assertEqual(report["current"]["effort"], pairs[-1][1])
            with self.assertRaisesRegex(RuntimeError, "does not match role"):
                meta._authors()
            self.assertTrue(log.exists())
        # Accepted: on-class current, full history and evidence preserved.
        pairs = (other, expected)
        content = self.client_log(client, *pairs)
        log = self.bound(client, role, content, name=f"{name}-ok")
        report = json.loads(self.run_cli("session", "--role", role))
        self.assertNotIn("error", report)
        self.assertEqual(report["current"]["effort"], "high")
        mapping = report["mapping"]
        self.assertEqual(mapping["family"], "multi-family")
        self.assertEqual((mapping["model"], mapping["effort"]), expected)
        (author,) = meta._authors()
        self.assertEqual(author["families"], ["claude", "codex"])
        self.assertEqual(author["family"], "multi-family")
        self.assertEqual([entry["model"] for entry in author["off_class"]], [other[0]])
        self.assertEqual(author["off_class"][0]["family"], self.family_of(other))
        raw = log.read_bytes()
        self.assertEqual(
            author["evidence"],
            {"path": str(log), "offset": len(raw), "sha256": hashlib.sha256(raw).hexdigest()},
        )
        parsed, _, _, _ = sessions.parse(log, client)
        self.assertEqual(sessions.expand_history(author["models"]), parsed)
        self.assertEqual(len(author["models"]), 2)
        # Unknown history then an on-class current: accepted; unknown current: refused.
        mystery = ("mystery-9", "high")
        self.bound(client, role, self.client_log(client, mystery, expected), name=f"{name}-unk")
        (unknown_author,) = meta._authors()
        self.assertEqual(
            (unknown_author["family"], unknown_author["families"]),
            ("multi-family", sorted([self.family_of(expected), "unknown"])),
        )
        self.bound(client, role, self.client_log(client, expected, mystery), name=f"{name}-cur")
        self.assertIn(
            "current model family is unresolved",
            json.loads(self.run_cli("session", "--role", role))["error"],
        )
        with self.assertRaisesRegex(RuntimeError, "current model family is unresolved"):
            meta._authors()

    @staticmethod
    def family_of(pair: tuple[str, str]) -> str:
        return "codex" if pair[0].startswith("gpt") else "claude"

    def test_h07a_a_copilot_current_that_is_not_confirmed_refuses_before_readiness(self) -> None:
        self.fixture_master()
        opus, luna = "claude-opus-5.5", "gpt-6-luna"
        unconfirmed = self.copilot_log(
            self.selection(opus), self.spoke(opus), self.spoke(luna), self.change(opus)
        )
        self.bound("copilot", "manager", unconfirmed, name="unconfirmed")
        report = json.loads(self.run_cli("session", "--role", "manager"))
        self.assertIsNone(report["current"])
        self.assertIn("Session current model selection is unresolved", report["error"])
        with self.assertRaisesRegex(RuntimeError, "current model selection is unresolved"):
            meta._authors()


if __name__ == "__main__":
    unittest.main()
