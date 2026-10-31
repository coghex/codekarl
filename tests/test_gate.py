"""Gate primitives are exercised against real Pijul working copies."""

import fcntl
import hashlib
import json
import os
import shutil
import sys
import tempfile
import unittest
from collections.abc import Callable
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
from environment import signing
from gate import classes, locks, meta, pointer, provenance, reviews, sessions
from gate.records import Author, PointerSchema1, ReviewRow, SubmissionSubject, is_agent
from pipeline import pijul
from sessionfix import CopilotSession, no_agent


def author(identity: str, session: str, family: str) -> Author:
    return {
        "identity": identity,
        "client": family,
        "session": session,
        "role": "worker",
        "family": family,
        "families": [family],
        "hashes": [],
    }


def approval(subject: SubmissionSubject, session: str, family: str) -> ReviewRow:
    """An approving review of `subject` by an independent reviewer session of `family`."""
    return {
        "schema": "ck-review/1",
        "id": f"review-{session}",
        "kind": "submission",
        "verdict": "approved",
        "recorded": 1,
        "report_sha256": "0" * 64,
        "issues": [],
        "reviewer": {
            "identity": "reviewer",
            "family": family,
            "role": "reviewer",
            "client": family,
            "session": session,
            "model": "m",
            "effort": "high",
            "class": "A",
            "evidence": {"path": "/session/events.jsonl", "offset": 0, "sha256": "0" * 64},
        },
        "subject": subject,
        "base_examined": None,
        "incremental_over": None,
        "revoked": False,
    }


def submission(name: str) -> SubmissionSubject:
    return {"submission": name, "manifest_sha256": "m", "fingerprint_sha256": "f"}


class GatePrimitives(unittest.TestCase):
    def setUp(self) -> None:
        no_agent(self)
        self.temp = tempfile.TemporaryDirectory(prefix="ck-gate-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "project"
        self.root.mkdir()
        self.signing = signing(Path.home() / "work/codekarl")
        self.signing.__enter__()
        self.addCleanup(self.signing.__exit__, None, None, None)
        pijul(self.root, "init")
        (self.root / ".ignore").write_text("/.pipeline\n")
        (self.root / "source.txt").write_text("base\n")
        pijul(self.root, "add", "--recursive", ".")
        pijul(self.root, "record", "-am", "base", "--author", "Gate tests")

    def test_pointer_refuses_hold_and_final_tombstone(self) -> None:
        control = self.root / ".pipeline"
        control.mkdir()
        legacy: PointerSchema1 = {
            "repo": str(self.root),
            "fingerprint": {"hashes": [], "files": {}},
            "submissions": [],
        }
        (control / "accepted.json").write_text(json.dumps(legacy))
        self.assertEqual(pointer.read_pointer(self.root)["repo"], str(self.root))
        hold = pointer.tombstone(self.root, legacy, bootstrap="B1")
        (control / "accepted.json").write_bytes(pointer.canonical(hold))
        with self.assertRaisesRegex(RuntimeError, "B1 in progress"):
            pointer.read_pointer(self.root)
        final = pointer.tombstone(self.root, legacy)
        (control / "accepted.json").write_bytes(pointer.canonical(final))
        with self.assertRaisesRegex(RuntimeError, "activation incomplete"):
            pointer.read_pointer(self.root)

    def test_v2_pointer_takes_precedence_over_legacy_pointer(self) -> None:
        control = self.root / ".pipeline"
        control.mkdir()
        (control / "accepted.json").write_text(json.dumps({"repo": "legacy"}))
        (control / "accepted-v2.json").write_text(
            json.dumps(
                {
                    "schema": 2,
                    "repo": "v2",
                    "fingerprint": {"hashes": [], "files": {}},
                    "submissions": [],
                }
            )
        )
        self.assertEqual(pointer.read_pointer(self.root)["repo"], "v2")

    def test_atomic_pointer_is_canonical_and_digest_stable(self) -> None:
        path = self.root / ".pipeline/accepted.json"
        value: PointerSchema1 = {
            "repo": "x",
            "fingerprint": {"files": {}, "hashes": []},
            "submissions": [],
        }
        written = pointer.write_pointer(path, value)
        self.assertEqual(written, pointer.canonical(value))
        self.assertEqual(pointer.file_digest(path), hashlib.sha256(written).hexdigest())
        self.assertFalse(list(path.parent.glob("accepted.json.*")))

    def test_lock_order_and_nonblocking_ci_ownership(self) -> None:
        with (
            locks.state(self.root),
            self.assertRaisesRegex(RuntimeError, "Lock order violation"),
            locks.acquire(locks.ci_path(self.root), "ci", blocking=False),
        ):
            pass
        with (
            locks.ci(self.root),
            self.assertRaisesRegex(RuntimeError, "Already running"),
            locks.ci(self.root),
        ):
            pass

    def test_vendored_legacy_controller_is_pinned(self) -> None:
        fixtures = Path(__file__).parent / "fixtures/legacy"
        expected = {
            "ck": "fda9d9d695f397397f047acb23c4a7151dee4c1700cf3cb42705c5507b30aa8e",
            "pipeline.py": "edef207eafc29da04a17d0ece73ddc68819fc4b414b3872dd5eaadd2e39f076a",
            "environment.py": "726392067af8b76d1157aa264b2e747c25b26047fe4c10c155b9dd318ebcb0d1",
        }
        self.assertEqual(
            {name: hashlib.sha256((fixtures / name).read_bytes()).hexdigest() for name in expected},
            expected,
        )

    def test_model_class_alias_and_role_resolution(self) -> None:
        data, _ = classes.master()
        self.assertEqual(classes.family_for_model(data, "copilot", "claude-opus-5.5"), "claude")
        resolved = classes.verify(
            "reviewer", "copilot", [{"model": "claude-opus-5.5", "effort": "high"}]
        )
        self.assertEqual((resolved["class"], resolved["family"]), ("A", "claude"))
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            classes.verify(
                "reviewer", "copilot", [{"model": "claude-opus-5.5", "effort": "medium"}]
            )

    def test_copilot_model_history_ignores_utility_and_preserves_inherited_effort(self) -> None:
        fixture = Path(__file__).parent / "fixtures/sessions/copilot.jsonl"
        models, offset, digest = sessions.history(fixture, "copilot")
        self.assertEqual(
            models,
            [
                {"model": "claude-opus-5.5", "effort": "high"},
                {"model": "gpt-6-luna", "effort": "high"},
            ],
        )
        self.assertEqual(offset, fixture.stat().st_size)
        self.assertEqual(digest, hashlib.sha256(fixture.read_bytes()).hexdigest())

    def test_copilot_fallback_observation_records_both_families(self) -> None:
        fixture = Path(__file__).parent / "fixtures/sessions/copilot-fallback.jsonl"
        models, _, _ = sessions.history(fixture, "copilot")
        self.assertEqual([item["model"] for item in models], ["claude-opus-5.5", "gpt-6-luna"])
        result = classes.verify("reviewer", "copilot", models)
        self.assertEqual(result["family"], "multi-family")
        self.assertCountEqual(result["families"], ["claude", "codex"])

    def test_codex_evidence_and_unresolvable_claude_effort(self) -> None:
        fixtures = Path(__file__).parent / "fixtures/sessions"
        models, _, _ = sessions.history(fixtures / "codex.jsonl", "codex")
        self.assertEqual(models, [{"model": "gpt-6-luna", "effort": "high"}])
        with self.assertRaisesRegex(RuntimeError, "Unresolvable Claude effort"):
            sessions.history(fixtures / "claude-no-effort.jsonl", "claude")

    def test_claude_history_skips_synthetic_api_errors_and_no_response_before_validation(
        self,
    ) -> None:
        events = [
            {
                "type": "assistant",
                "message": {"model": "claude-opus-5-5"},
                "effort": "high",
            },
            {
                "type": "assistant",
                "message": {"model": "<synthetic>"},
                "effort": {"invalid": "effort"},
                "isApiErrorMessage": False,
            },
            {
                "type": "assistant",
                "message": {"model": "<synthetic>"},
                "effort": False,
                "isApiErrorMessage": True,
            },
            {
                "type": "assistant",
                "message": {"model": "claude-haiku-4-5"},
                "effort": "high",
                "isApiErrorMessage": True,
            },
            {
                "type": "assistant",
                "message": {"model": ["invalid", "model"]},
                "effort": "high",
                "isApiErrorMessage": True,
            },
            {
                "type": "assistant",
                "message": None,
                "effort": False,
                "isApiErrorMessage": True,
            },
            {
                "type": "assistant.message",
                "message": {"model": "claude-sonnet-4-5"},
                "effort": "medium",
            },
        ]
        path = Path(self.temp.name) / "claude-synthetic.jsonl"
        path.write_text("".join(json.dumps(event) + "\n" for event in events))
        models, _, _, current = sessions.parse(path, "claude")
        self.assertEqual(
            models,
            [
                {"model": "claude-opus-5-5", "effort": "high"},
                {"model": "claude-sonnet-4-5", "effort": "medium"},
            ],
        )
        self.assertEqual(current, {"model": "claude-sonnet-4-5", "effort": "medium"})

    def test_claude_history_rejects_invalid_real_and_synthetic_only_events(self) -> None:
        path = Path(self.temp.name) / "claude-invalid.jsonl"
        path.write_text(
            json.dumps({"type": "assistant", "message": {"model": "claude-opus-5-5"}}) + "\n"
        )
        with self.assertRaisesRegex(RuntimeError, "Unresolvable Claude effort at event 1"):
            sessions.history(path, "claude")

        path.write_text(
            json.dumps(
                {
                    "type": "assistant",
                    "message": {"model": "<synthetic>"},
                    "effort": "high",
                }
            )
            + "\n"
        )
        with self.assertRaisesRegex(RuntimeError, "Session model history is unresolvable"):
            sessions.history(path, "claude")

    def test_provenance_digest_and_raw_description_fallback(self) -> None:
        payload = {"schema": "ck-provenance/1", "issues": ["CK-1"], "identity": "agent"}
        encoded = pointer.canonical(payload)
        parsed = provenance.parse_description(
            provenance.PREFIX + provenance.payload_digest(payload) + "\n" + encoded.decode()
        )
        self.assertEqual(parsed, payload)
        self.assertIsNone(provenance.parse_description("ordinary Pijul description"))
        unknown = provenance.parse_description(provenance.PREFIX + "bad\n{}")
        assert unknown is not None
        self.assertEqual(unknown["schema"], "unknown")

    def test_c10_review_coverage_is_per_subject_and_independent(self) -> None:
        authors = [author("worker", "w", "codex")]
        subject = submission("S1")
        receipt = approval(subject, "r", "claude")
        self.assertTrue(reviews.coverage(authors, [receipt], subject))
        self.assertFalse(reviews.coverage(authors, [receipt], {**subject, "submission": "S2"}))
        same_author = approval(subject, "w", "claude")
        self.assertFalse(reviews.coverage(authors, [same_author], subject))
        mixed_authors = [
            author("codex-author", "ca", "codex"),
            author("claude-author", "la", "claude"),
        ]
        codex_receipt = approval(submission("S1"), "claude-review", "claude")
        claude_receipt = approval(submission("S1"), "codex-review", "codex")
        self.assertTrue(
            reviews.coverage(mixed_authors, [codex_receipt, claude_receipt], submission("S1"))
        )
        self.assertFalse(
            reviews.coverage(mixed_authors, [codex_receipt, claude_receipt], submission("S2"))
        )

    def test_metadata_issue_events_are_recorded_in_real_pijul_repo(self) -> None:
        hub = Path(self.temp.name) / "hub"
        hub.mkdir()
        (hub / ".pipeline").mkdir()
        spec = Path(self.temp.name) / "spec.md"
        spec.write_text("Contract\n\nAcceptance: prove it.\n")
        store = meta.Store(hub)
        issue = store.new_issue("Gate metadata", "task", "P2", spec)
        shown = store.show(issue)
        assert shown is not None
        self.assertEqual(shown["id"], issue)
        self.assertEqual(shown["revision"], "r1")
        self.assertEqual(shown["title"], "Gate metadata")
        self.assertFalse(meta.ready(store, shown))
        self.assertEqual(json.loads((store.repo / "counters.json").read_bytes())["issue"], 1)


class CopilotEvidence(unittest.TestCase):
    """Direct Copilot discovery fails closed and never degrades to the owner."""

    def codex_row(self, directory: str, pid: int) -> tuple[dict[str, object], dict[str, str]]:
        root = Path(directory) / "codex"
        root.mkdir()
        (root / "native.jsonl").write_text(
            '{"type":"turn_context","payload":{"model":"gpt-6-astra","effort":"high"}}\n'
        )
        row = {"name": "native", "brand": "codex", "session_id": "native", "role": "worker"}
        return {**row, "pid": pid}, {"codex": str(root)}

    def test_valid_session_resolves_with_class_family_and_evidence(self) -> None:
        for model, role, expected in (
            ("claude-sonnet-5.5", "worker", ("B", "claude", "claude-sonnet-5-5")),
            ("claude-opus-5.5", "manager", ("S", "claude", "claude-opus-5-5")),
            ("claude-opus-5.5", "reviewer", ("A", "claude", "claude-opus-5-5")),
        ):
            with self.subTest(model=model, role=role):
                session = CopilotSession(self, model).activate()
                resolved = sessions.resolve()
                assert is_agent(resolved)
                self.assertEqual(resolved["identity"], f"copilot:{session.id}")
                self.assertEqual(
                    (resolved["client"], resolved["session"], resolved["role"], resolved["pid"]),
                    ("copilot", session.id, None, session.pid),
                )
                self.assertEqual(resolved["models"], [{"model": model, "effort": "high"}])
                self.assertEqual(
                    resolved["evidence"],
                    {
                        "path": str(session.log),
                        "offset": session.log.stat().st_size,
                        "sha256": session.digest(),
                    },
                )
                mapping = classes.verify(role, resolved["client"], resolved["models"])
                self.assertEqual(
                    (mapping["class"], mapping["family"], mapping["model"], mapping["effort"]),
                    (expected[0], expected[1], expected[2], "high"),
                )

    def test_invalid_evidence_is_a_distinct_refusal_never_human(self) -> None:
        def mutate_log(session: CopilotSession, text: str) -> None:
            session.log.write_text(text)

        def symlink_directory(session: CopilotSession) -> None:
            session.directory.rename(session.root / "real")
            session.directory.symlink_to(session.root / "real")

        def second_lock(session: CopilotSession) -> None:
            (session.directory / "inuse.424242.lock").write_text("424242\n")

        first = json.dumps(
            {"type": "session.start", "data": {"sessionId": "x", "selectedModel": "m"}}
        )
        cases: list[
            tuple[str, str | None, str | Callable[[CopilotSession], object] | None, str]
        ] = [
            ("malformed uuid", "not-a-uuid", None, "canonical lowercase UUID"),
            ("path-like uuid", "../" + "a" * 36, None, "canonical lowercase UUID"),
            ("uppercase uuid", "UPPER", None, "canonical lowercase UUID"),
            (
                "missing directory",
                None,
                lambda s: shutil.rmtree(s.directory),
                "missing or a symlink",
            ),
            ("missing log", None, lambda s: s.log.unlink(), "events.jsonl"),
            (
                "symlinked directory",
                None,
                symlink_directory,
                "missing or a symlink",
            ),
            ("no ancestor lock", None, "chain", "exactly one ancestor inuse lock, found 0"),
            ("two locks", None, second_lock, "exactly one ancestor inuse lock, found 2"),
            ("lock content", None, lambda s: s.lock.write_text("1\n"), "content is not its pid"),
            ("not copilot", None, "bash", "is not a copilot process"),
            (
                "stale hold",
                None,
                "stale",
                "is not held by a live client",
            ),
            (
                "session id mismatch",
                None,
                lambda s: mutate_log(s, first + "\n"),
                "first event is not session.start",
            ),
            (
                "not session start",
                None,
                lambda s: mutate_log(s, '{"type":"user.message","data":{}}\n'),
                "first event is not session.start",
            ),
            (
                "unparsable history",
                None,
                lambda s: mutate_log(s, s.log.read_text() + "not json\n"),
                "Unparseable copilot session event",
            ),
            (
                "unselected history",
                None,
                lambda s: mutate_log(
                    s,
                    json.dumps({"type": "session.start", "data": {"sessionId": s.id}})
                    + '\n{"type":"assistant.message","data":{"model":"claude-opus-5.5"}}\n',
                ),
                "Unselected agent model",
            ),
        ]
        for name, variable, action, message in cases:
            with self.subTest(name):
                session = CopilotSession(self, held=action != "stale")
                chain: list[int] | None = None
                executable = "copilot"
                if action == "chain":
                    chain = [os.getpid()]
                elif action == "bash":
                    executable = "bash"
                elif action is second_lock:
                    chain = [os.getpid(), session.pid, 424242]
                if callable(action):
                    action(session)
                session.activate(chain=chain, executable=executable)
                if variable is not None:
                    os.environ["COPILOT_AGENT_SESSION_ID"] = variable
                with self.assertRaisesRegex(RuntimeError, message):
                    sessions.resolve()

    def test_stale_hold_probe_releases_its_lock(self) -> None:
        session = CopilotSession(self, held=False).activate()
        with self.assertRaisesRegex(RuntimeError, "not held by a live client"):
            sessions.resolve()
        descriptor = os.open(session.hold, os.O_RDONLY)
        self.addCleanup(os.close, descriptor)
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_nearest_evidence_wins_between_registry_and_copilot(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            row, roots = self.codex_row(directory, 777001)
            near_registry = CopilotSession(self)
            near_registry.activate(
                chain=[os.getpid(), 777001, near_registry.pid], registry=[row], roots=roots
            )
            self.assertEqual(sessions.resolve()["identity"], "native")
            near_copilot = CopilotSession(self)
            near_copilot.activate(
                chain=[os.getpid(), near_copilot.pid, 777001], registry=[row], roots=roots
            )
            self.assertEqual(sessions.resolve()["identity"], f"copilot:{near_copilot.id}")

    def test_invalid_copilot_evidence_refuses_even_when_registry_matches(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            row, roots = self.codex_row(directory, 777001)
            stale = CopilotSession(self, held=False)
            stale.activate(chain=[os.getpid(), 777001, stale.pid], registry=[row], roots=roots)
            with self.assertRaisesRegex(RuntimeError, "not held by a live client"):
                sessions.resolve()

    def test_invalid_copilot_history_refuses_even_when_a_nearer_registry_row_matches(self) -> None:
        def start(session: CopilotSession, **fields: str) -> str:
            data = {"sessionId": session.id, **fields}
            return json.dumps({"type": "session.start", "data": data}) + "\n"

        message = '{"type":"assistant.message","data":{"model":"claude-opus-5.5"}}\n'
        cases = (
            ("malformed line", lambda s: s.log.read_text() + "not json\n", "Unparseable copilot"),
            (
                "unselected model",
                lambda s: start(s, reasoningEffort="high") + message,
                "Unselected agent model",
            ),
            (
                "missing effort",
                lambda s: start(s, selectedModel="claude-opus-5.5") + message,
                "Unselected agent model",
            ),
        )
        for name, rewrite, expected in cases:
            with self.subTest(name), tempfile.TemporaryDirectory() as directory:
                row, roots = self.codex_row(directory, 777001)
                session = CopilotSession(self)
                session.log.write_text(rewrite(session))
                session.activate(
                    chain=[os.getpid(), 777001, session.pid], registry=[row], roots=roots
                )
                with self.assertRaisesRegex(RuntimeError, expected):
                    sessions.resolve()

    def test_winning_copilot_evidence_reuses_the_validated_history_read(self) -> None:
        session = CopilotSession(self).activate()
        with mock.patch.object(sessions, "parse", wraps=sessions.parse) as read:
            resolved = sessions.resolve()
        self.assertEqual(read.call_count, 1)
        assert is_agent(resolved)
        self.assertEqual(resolved["evidence"]["sha256"], session.digest())

    def test_copilot_ancestor_without_variable_refuses_even_when_a_registry_row_matches(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            row, roots = self.codex_row(directory, 777001)
            session = CopilotSession(self)
            session.activate(chain=[os.getpid(), 777001, session.pid], registry=[row], roots=roots)
            self.assertEqual(sessions.resolve()["identity"], "native")
            del os.environ["COPILOT_AGENT_SESSION_ID"]
            with self.assertRaisesRegex(RuntimeError, "Copilot ancestor without COPILOT_AGENT"):
                sessions.resolve()

    def test_copilot_ancestor_without_variable_refuses_but_plain_shell_is_human(self) -> None:
        session = CopilotSession(self).activate()
        del os.environ["COPILOT_AGENT_SESSION_ID"]
        with self.assertRaisesRegex(RuntimeError, "Copilot ancestor without COPILOT_AGENT"):
            sessions.resolve()
        no_agent(self)
        self.assertTrue(sessions.resolve().get("human"))
        self.assertIsNone(sessions.bound_session())
        self.assertNotEqual(session.pid, os.getpid())

    def test_ancestor_chain_is_ordered_nearest_first(self) -> None:
        chain = sessions._ancestors()
        self.assertEqual(chain[:2], [os.getpid(), os.getppid()])

    def test_multi_family_history_cannot_review_but_utility_models_do_not_count(self) -> None:
        def events(session_id: str, *later: dict[str, object]) -> list[dict[str, object]]:
            start = {"sessionId": session_id, "selectedModel": "claude-opus-5.5"}
            start["reasoningEffort"] = "high"
            return [
                {"type": "session.start", "data": start},
                {"type": "assistant.message", "data": {"model": "claude-opus-5.5"}},
                *later,
            ]

        mixed = CopilotSession(self)
        mixed.log.write_text(
            "".join(
                json.dumps(line) + "\n"
                for line in events(
                    mixed.id, {"type": "assistant.message", "data": {"model": "gpt-6-luna"}}
                )
            )
        )
        mixed.activate()
        # PF-12: the current pair is Class B on a Class A role, so it is refused as off-class.
        with self.assertRaisesRegex(RuntimeError, "does not match role"):
            reviews._reviewer("reviewer")
        onclass = CopilotSession(self)
        onclass.log.write_text(
            "".join(
                json.dumps(line) + "\n"
                for line in events(
                    onclass.id, {"type": "assistant.message", "data": {"model": "gpt-6.1-sol"}}
                )
            )
        )
        onclass.activate()
        # An on-class mixed history is still refused as multi-family.
        with self.assertRaisesRegex(RuntimeError, "Unknown or multi-family"):
            reviews._reviewer("reviewer")
        utility = CopilotSession(self)
        utility.log.write_text(
            "".join(
                json.dumps(line) + "\n"
                for line in events(
                    utility.id,
                    {"type": "model.turn_started", "data": {"model": "gpt-4o-mini"}},
                    {"type": "assistant.message", "data": {"model": "claude-opus-5.5"}},
                )
            )
        )
        utility.activate()
        reviewer, _ = reviews._reviewer("reviewer")
        self.assertEqual((reviewer["family"], reviewer["client"]), ("claude", "copilot"))


if __name__ == "__main__":
    unittest.main()
