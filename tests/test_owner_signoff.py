"""CK-34: owner sign-off receipts (`ck review ... --owner`).

The owner path is exercised only in disposable fixture hubs, by patching the detection functions
in-process (ancestry names, terminal checks and the /dev/tty reader). Nothing here detaches a
process, masks ancestry, gives a process a terminal, or touches this hub's meta store.
"""

import contextlib
import io
import json
import os
import pty
import subprocess
import sys
import tempfile
import textwrap
from collections.abc import Callable, Iterator
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import pipeline
import test_batch
from gate import batch, docsqueue, pointer, reviews, sessions
from gate import cli as gate_cli
from gate.records import (
    Author,
    ManifestInfo,
    ReviewReceipt,
    ReviewRow,
    SpecSubject,
    is_manifest_v2,
    load_manifest,
    load_review_receipt,
)

HUMAN: Author = {
    "identity": "human",
    "client": "terminal",
    "session": "owner-session",
    "family": "human",
}


class OwnerSignoff(test_batch.BatchGateSpecification):
    """The batch fixture's hub helpers, without re-running that class's own tests."""

    def setUp(self) -> None:
        super().setUp()
        self.report = Path(self.temp.name) / "owner-report.md"
        self.report.write_text("Owner reviewed this subject.\n")

    # ---- driving the owner path -------------------------------------------------------------

    def owner(
        self,
        *argv: str,
        answer: str | None = None,
        ancestor: str | None = None,
        terminals: bool = True,
        report: bool = True,
        expect: str | None = None,
    ) -> str:
        """Run `ck review ... --owner` in-process with injected owner detection; returns stdout."""
        words = list(argv)
        if report and "--report" not in words and "revoke" not in words:
            words += ["--report", str(self.report)]
        confirmation = answer if answer is not None else (expect or words[1])[-6:]
        output = io.StringIO()
        with (
            mock.patch.object(sessions, "agent_ancestor", return_value=ancestor),
            mock.patch.object(sessions, "terminals_present", return_value=terminals),
            mock.patch.object(sessions, "read_confirmation", return_value=confirmation),
            contextlib.redirect_stdout(output),
        ):
            gate_cli.run(self.hub, self.hub, ["review", *words, "--owner"])
        return output.getvalue()

    def sign_submission(self, name: str) -> str:
        return self.owner("submission", name, "--verdict", "approved").strip().splitlines()[-1]

    def snapshot(self) -> dict[str, bytes]:
        repo = self.store.repo
        return {
            path.relative_to(repo).as_posix(): path.read_bytes()
            for path in sorted(repo.rglob("*"))
            if path.is_file() and ".pijul" not in path.parts
        }

    def receipt(self, rid: str) -> ReviewReceipt:
        path = self.store.repo / "reviews" / f"{rid}.json"
        return load_review_receipt(json.loads(path.read_bytes()), str(path))

    def owner_batch(self, name: str = "ownersub") -> tuple[batch.Manager, str]:
        """A validated batch of one submission with no AI review, signed by the owner."""
        self.submission(name, review=False)
        self.sign_submission(name)
        manager = batch.Manager(self.hub, self.passing)
        bid = manager.prepare()
        return manager, bid

    # ---- 1. happy path ----------------------------------------------------------------------

    def test_owner_01_a_signed_submission_is_batched_approved_and_published_with_labels(
        self,
    ) -> None:
        self.submission("ownersub", authors=[self.CLAUDE], review=False)
        self.assertNotIn("ownersub", self.eligible_ids())
        rid = self.sign_submission("ownersub")
        receipt = self.receipt(rid)
        manifest_path = self.hub / ".pipeline/queue/ownersub.json"
        self.assertEqual(receipt["schema"], "ck-review/1")
        self.assertEqual(receipt["kind"], "submission")
        self.assertEqual(
            receipt["reviewer"],
            {"identity": "human", "role": "owner", "family": "human", "owner_signoff": True},
        )
        self.assertEqual(receipt["verdict"], "approved")
        self.assertEqual(
            receipt["subject"],
            reviews.subject_for_submission(manifest_path, json.loads(manifest_path.read_bytes())),
        )
        self.assertEqual(
            receipt["report_sha256"],
            __import__("hashlib").sha256(self.report.read_bytes()).hexdigest(),
        )
        self.assertIsNone(receipt["incremental_over"])
        for forbidden in ("class", "model", "effort", "session"):
            self.assertNotIn(forbidden, receipt["reviewer"])
        self.assertEqual(
            (self.store.repo / "reviews" / f"{rid}.md").read_bytes(), self.report.read_bytes()
        )

        manager = batch.Manager(self.hub, self.passing)
        self.assertIn("ownersub", self.eligible_ids())
        bid = manager.prepare()
        self.assertEqual(self.validated(bid)["members"], ["ownersub"])
        out = io.StringIO()
        with (
            mock.patch.object(sessions, "agent_ancestor", return_value=None),
            mock.patch.object(sessions, "terminals_present", return_value=True),
            mock.patch.object(sessions, "read_confirmation", return_value=bid[-6:]),
            contextlib.redirect_stdout(out),
        ):
            gate_cli.run(
                self.hub,
                self.hub,
                [
                    "review",
                    "batch",
                    bid,
                    "--owner",
                    "--verdict",
                    "approved",
                    "--report",
                    str(self.report),
                ],
            )
        batch_rid = out.getvalue().strip().splitlines()[-1]
        approved = json.loads((batch.batch_path(self.hub, bid) / "approved.json").read_bytes())
        self.assertEqual(approved["owner_signoff"], [batch_rid])
        manager.publish(bid)
        self.assertIn("ownersub", pointer.read_pointer(self.hub)["submissions"])
        published = json.loads((batch.batch_path(self.hub, bid) / "published.json").read_bytes())
        self.assertEqual(published["owner_signoff"], [batch_rid])
        status = batch.Manager(self.hub).status(bid)
        signoffs = status["owner_signoff"]
        assert isinstance(signoffs, list)
        labels = {(row["receipt"], row["subject"]) for row in signoffs}
        self.assertEqual(labels, {(batch_rid, "batch"), (rid, "submission ownersub")})
        self.assertTrue(all(row["owner_signoff"] is True for row in signoffs))
        # `ck status` shows the accepted member as an owner sign-off.
        shown = io.StringIO()
        with contextlib.redirect_stdout(shown):
            gate_cli.run(self.hub, self.hub, ["status"])
        self.assertEqual(json.loads(shown.getvalue())["owner_signoff"], {"ownersub": [rid]})

    # ---- 2. refusals write nothing ----------------------------------------------------------

    def assert_refused(
        self,
        message: str,
        *argv: str,
        answer: str | None = None,
        ancestor: str | None = None,
        terminals: bool = True,
    ) -> None:
        before = self.snapshot()
        with self.assertRaisesRegex(RuntimeError, message):
            self.owner(*argv, answer=answer, ancestor=ancestor, terminals=terminals)
        self.assertEqual(self.snapshot(), before, "a refusal must write nothing")

    def test_owner_02_every_refusal_names_its_check_and_writes_nothing(self) -> None:
        self.submission("refused", review=False)
        argv = ("submission", "refused", "--verdict", "approved")
        self.assert_refused("terminals", *argv, terminals=False)
        self.assert_refused("did not match", *argv, answer="nope")
        self.assert_refused(
            "agent client is an ancestor: claude \\(pid 7\\)", *argv, ancestor="claude (pid 7)"
        )
        self.assert_refused(
            "only --verdict approved", "submission", "refused", "--verdict", "rejected"
        )
        self.assert_refused(
            "only --verdict approved", "submission", "refused", "--verdict", "changes-requested"
        )
        self.assert_refused("refused for spec", "spec", "CK-1@r1", "--verdict", "approved")
        self.assert_refused("cannot be combined", *argv, "--role", "reviewer")
        self.assert_refused("cannot be combined", *argv, "--incremental-over", "x")
        for value in ("/tmp/anything", "", " "):  # every present value refuses, including empty
            with mock.patch.dict(os.environ, {"CK_SESSION_EVIDENCE": value}):
                self.assert_refused("CK_SESSION_EVIDENCE is set", *argv)
        bound = {"identity": "ck-worker-9", "client": "claude", "session": "s9", "models": []}
        with mock.patch.object(sessions, "resolve", return_value=bound):
            self.assert_refused("bound agent session", *argv)

    def test_owner_02b_a_real_claude_or_codex_ancestor_is_refused(self) -> None:
        self.submission("ancestry", review=False)
        runner = Path(self.temp.name) / "runner.py"
        runner.write_text(
            textwrap.dedent(
                f"""
                import sys
                from pathlib import Path
                from unittest import mock
                sys.path.insert(0, {str(Path(sessions.__file__).parents[1])!r})
                from gate import cli, sessions
                # Only the registry, terminals and tty are patched: ancestry is the real process tree.
                with (
                    mock.patch.object(sessions, "_registry", return_value=[]),
                    mock.patch.object(sessions, "terminals_present", return_value=True),
                    mock.patch.object(sessions, "read_confirmation", return_value="ncestry"[-6:]),
                ):
                    hub = Path({str(self.hub)!r})
                    cli.run(hub, hub, ["review", "submission", "ancestry", "--owner",
                        "--verdict", "approved", "--report", {str(self.report)!r}])
                """
            )
        )
        before = self.snapshot()
        # Real ancestors: a tiny compiled launcher, copied under the client names `ps` reports.
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "launcher.c"
            source.write_text(
                "#include <stdlib.h>\n#include <sys/wait.h>\nint main(int c, char **v) {\n    if (c < 2) return 2;\n    int r = system(v[1]);\n    return WIFEXITED(r) ? WEXITSTATUS(r) : 1;\n}\n"
            )
            built = Path(directory) / "built"
            subprocess.run(
                ["xcrun", "--sdk", "macosx", "clang", "-o", str(built), str(source)], check=True
            )
            for name in ("claude", "codex", "codex-helper"):
                with self.subTest(name):
                    fake = Path(directory) / name
                    fake.write_bytes(built.read_bytes())
                    fake.chmod(0o755)
                    result = subprocess.run(
                        [str(fake), f"{sys.executable} {runner}"],
                        capture_output=True,
                        text=True,
                        check=False,
                        env={k: v for k, v in os.environ.items() if k != "CK_SESSION_EVIDENCE"},
                    )
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertIn(f"an agent client is an ancestor: {name} (pid ", result.stderr)
        self.assertEqual(self.snapshot(), before)

    def test_owner_02c_standard_streams_that_are_pipes_are_not_terminals(self) -> None:
        probe = (
            f"import sys; sys.path.insert(0, {str(Path(sessions.__file__).parents[1])!r});"
            "from gate import sessions; print(sessions.terminals_present())"
        )
        result = subprocess.run(
            [sys.executable, "-c", probe],
            capture_output=True,
            text=True,
            stdin=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.stdout.strip(), "False")

    # ---- the real /dev/tty reader on non-seekable streams -----------------------------------

    @contextlib.contextmanager
    def tty_at(self, path: str) -> Iterator[None]:
        """Point the reader at a disposable non-seekable device instead of /dev/tty (in-process)."""
        real_open: Callable[..., object] = open

        def redirect(file: str, *args: object, **kwargs: object) -> object:
            return real_open(path if file == "/dev/tty" else file, *args, **kwargs)

        with (
            mock.patch.object(sessions, "TTY_PATH", path, create=True),
            mock.patch("builtins.open", redirect),
        ):
            yield

    def test_owner_02d_the_reader_works_on_a_real_non_seekable_terminal(self) -> None:
        master, slave = pty.openpty()
        self.addCleanup(os.close, master)
        self.addCleanup(os.close, slave)
        os.write(master, b"abc123\n")
        with self.tty_at(os.ttyname(slave)):
            answer = sessions.read_confirmation("Type abc123 to confirm: ")
        self.assertEqual(answer, "abc123")
        self.assertIn(b"Type abc123 to confirm: ", os.read(master, 4096))  # the prompt reached it

    def test_owner_02e_the_reader_works_on_a_named_pipe_too(self) -> None:
        fifo = Path(self.temp.name) / "tty.fifo"
        os.mkfifo(fifo)
        feeder = os.open(fifo, os.O_RDWR)
        self.addCleanup(os.close, feeder)
        os.write(feeder, b"xyz789\n")
        with self.tty_at(str(fifo)):
            self.assertEqual(sessions.read_confirmation("Type xyz789: "), "xyz789")

    def test_owner_02f_an_unopenable_terminal_is_a_refusal_not_a_crash(self) -> None:
        with (
            self.tty_at(str(Path(self.temp.name) / "missing-tty")),
            self.assertRaisesRegex(RuntimeError, "Cannot read a confirmation"),
        ):
            sessions.read_confirmation("Type x: ")

    # ---- 3. exactness -----------------------------------------------------------------------

    def test_owner_03_a_receipt_covers_only_the_exact_subject(self) -> None:
        self.submission("exact-a", review=False)
        self.sign_submission("exact-a")
        self.assertIn("exact-a", self.eligible_ids())
        successor = self.submission("exact-b", review=False)  # a superseding submission is new
        self.assertEqual(successor["id"], "exact-b")
        self.assertNotIn("exact-b", self.eligible_ids())
        capture = Path(json.loads((self.hub / ".pipeline/queue/exact-a.json").read_bytes())["repo"])
        (capture / "exact-a.txt").write_text("edited after sign-off\n")
        self.record(capture, "edit the immutable capture")
        self.assertNotIn("exact-a", self.eligible_ids())

    def test_owner_03b_a_batch_receipt_does_not_cover_a_re_prepared_batch(self) -> None:
        manager, bid = self.owner_batch("rebatch")
        self.owner("batch", bid, "--verdict", "approved")
        manager.cancel(bid, "re-prepare")
        second = manager.prepare()
        self.assertNotEqual(second, bid)
        with self.assertRaisesRegex(RuntimeError, "fresh independent integration approval"):
            manager.approve(second)

    # ---- 4. owner-authored subjects ----------------------------------------------------------

    def test_owner_04_a_receipt_covers_a_subject_the_owner_authored_and_is_labelled(self) -> None:
        self.submission("authored", authors=[HUMAN], review=False)
        self.assertNotIn("authored", self.eligible_ids())
        rid = self.sign_submission("authored")
        self.assertIn("authored", self.eligible_ids())
        self.assertTrue(self.receipt(rid)["reviewer"].get("owner_signoff"))

    # ---- 5. revocation ----------------------------------------------------------------------

    def test_owner_05_revocation_removes_coverage_and_publish_refuses(self) -> None:
        manager, bid = self.owner_batch("revokeme")
        batch_rid = self.owner("batch", bid, "--verdict", "approved").strip().splitlines()[-1]
        self.owner(
            "revoke",
            batch_rid,
            "--reason",
            "owner changed their mind",
            report=False,
            expect=batch_rid,
        )
        with self.assertRaisesRegex(RuntimeError, "integration review coverage"):
            manager.publish(bid)

    def test_owner_05c_an_owner_submission_receipt_is_not_a_single_submission_basis(self) -> None:
        # CK-38: the single-submission path needs AI review receipts; the owner's receipt covers
        # exactly the submission it signed and a batch still needs a batch-subject approval.
        self.single_submission("owner-solo", review=False)
        self.sign_submission("owner-solo")
        manager = batch.Manager(self.hub, self.timed())
        bid = manager.prepare()
        status = manager.status(bid)
        self.assertEqual(status["publication_basis"], "integration-review")
        self.assertIn("AI review receipts alone", str(status["publication_basis_reason"]))
        self.refused_single(manager, bid, "AI review receipts alone")
        self.owner("batch", bid, "--verdict", "approved")
        manager.approve(bid)
        manager.publish(bid)
        published = json.loads((batch.batch_path(self.hub, bid) / "published.json").read_bytes())
        self.assertEqual(published["basis"], "integration-review")
        self.assertIn("owner-solo", pointer.read_pointer(self.hub)["submissions"])

    def test_owner_05b_an_ai_reviewer_may_revoke_an_owner_receipt_but_not_the_reverse(self) -> None:
        self.submission("mutual", review=True)
        owner_rid = self.sign_submission("mutual")
        actor = {
            "identity": "claude-reviewer-9",
            "client": "claude",
            "model": "claude-opus-5-5",
            "session": "other",
            "role": "reviewer",
            "class": "A",
            "family": "claude",
            "effort": "high",
        }
        with mock.patch.object(reviews, "_reviewer", return_value=(actor, {})):
            reviews.revoke(self.store, owner_rid, "AI reviewer revokes an owner receipt")
        self.assertTrue(
            next(r for r in reviews.receipt_rows(self.store) if r["id"] == owner_rid)["revoked"]
        )
        # The owner path cannot revoke an AI receipt.
        before = self.snapshot()
        with self.assertRaisesRegex(RuntimeError, "cannot revoke an AI review receipt"):
            self.owner(
                "revoke", "review-mutual", "--reason", "nope", report=False, expect="review-mutual"
            )
        with self.assertRaisesRegex(RuntimeError, "cannot revoke an AI review receipt"):
            reviews.revoke(self.store, "review-mutual", "nope", owner=True)  # the library layer too
        self.assertEqual(self.snapshot(), before)

    # ---- 6. no mislabelling ------------------------------------------------------------------

    def test_owner_06_an_owner_receipt_never_counts_as_an_ai_family(self) -> None:
        manifest = self.submission("mixed-owner", authors=[self.CODEX, self.CLAUDE], review=False)
        path = self.hub / ".pipeline/queue/mixed-owner.json"
        subject = reviews.subject_for_submission(path, manifest)
        owner_rid = self.sign_submission("mixed-owner")
        claude_only = self.review_as(
            "mixed-owner", "claude", "only-claude", "claude-reviewer", rid="only-claude"
        )
        rows = reviews.receipt_rows(self.store)
        derived = reviews.derive(manifest)
        assert derived is not None
        authors = derived["authors"]
        owner_rows = [r for r in rows if r["id"] == owner_rid]
        self.assertTrue(reviews.coverage(authors, owner_rows, subject))  # owner authority, labelled
        # Without the owner rule the owner receipt is not a Claude or Codex review: two families
        # are still required, and a Claude review alone does not make up for the missing Codex one.
        self.assertFalse(reviews.coverage(authors, owner_rows, subject, owner=False))
        both = [r for r in rows if r["id"] in (owner_rid, claude_only)]
        self.assertFalse(reviews.coverage(authors, both, subject, owner=False))

    def test_owner_06b_an_owner_receipt_is_never_an_incremental_parent(self) -> None:
        self.submission("parent-sub", review=False)
        parent_rid = self.sign_submission("parent-sub")
        parent_path = self.hub / ".pipeline/queue/parent-sub.json"
        stored = load_manifest(json.loads(parent_path.read_bytes()), str(parent_path))
        assert is_manifest_v2(stored)
        parent = stored
        child = self.submission("child-sub", review=False, parent=parent)
        self.review_as(
            "child-sub",
            "claude",
            "child-reviewer",
            "child-reviewer",
            rid="child-ai",
            over="parent-sub",
        )
        rows = reviews.receipt_rows(self.store)
        manifests: dict[str, ManifestInfo] = {}
        for m in (parent, child):
            derived = reviews.derive(m)
            assert derived is not None
            manifests[m["id"]] = {
                "hashes": m["hashes"],
                "subject": reviews.subject_for_submission(
                    self.hub / ".pipeline/queue" / f"{m['id']}.json", m
                ),
                "authors": derived["authors"],
                "submitter": None,
                "unreadable": False,
            }
        child_subject = reviews.subject_for_submission(
            self.hub / ".pipeline/queue/child-sub.json", child
        )
        covered = reviews.coverage(
            manifests["child-sub"].get("authors", []), rows, child_subject, manifests
        )
        self.assertFalse(covered, f"the parent {parent_rid} is covered only by an owner receipt")

    # ---- confirmation window: the hash shown is the hash written; changes are refused ---------

    def test_owner_07_the_shown_report_hash_is_the_written_hash(self) -> None:
        self.submission("shown", review=False)
        out = self.owner("submission", "shown", "--verdict", "approved")
        rid = out.strip().splitlines()[-1]
        digest = self.receipt(rid)["report_sha256"]
        self.assertIn(f"sha256 {digest}", out)
        self.assertIn("patches: ", out)
        self.assertIn("files: shown.txt", out)

    def test_owner_07b_a_report_or_subject_that_changes_during_confirmation_is_refused(
        self,
    ) -> None:
        self.submission("moving", review=False)
        before = self.snapshot()

        def edit_report(prompt: str) -> str:
            self.report.write_text("changed while the owner was reading\n")
            return "moving"[-6:]

        with (
            mock.patch.object(sessions, "agent_ancestor", return_value=None),
            mock.patch.object(sessions, "terminals_present", return_value=True),
            mock.patch.object(sessions, "read_confirmation", side_effect=edit_report),
            self.assertRaisesRegex(RuntimeError, "changed during confirmation"),
        ):
            gate_cli.run(
                self.hub,
                self.hub,
                [
                    "review",
                    "submission",
                    "moving",
                    "--owner",
                    "--verdict",
                    "approved",
                    "--report",
                    str(self.report),
                ],
            )
        self.assertEqual(self.snapshot(), before)
        self.report.write_text("Owner reviewed this subject.\n")

        def change_manifest(prompt: str) -> str:
            path = self.hub / ".pipeline/queue/moving.json"
            manifest = json.loads(path.read_bytes())
            manifest["spec"] = "edited while the owner was reading"
            pointer.atomic_bytes(path, pointer.canonical(manifest))
            return "moving"[-6:]

        with (
            mock.patch.object(sessions, "agent_ancestor", return_value=None),
            mock.patch.object(sessions, "terminals_present", return_value=True),
            mock.patch.object(sessions, "read_confirmation", side_effect=change_manifest),
            self.assertRaisesRegex(RuntimeError, "changed during confirmation"),
        ):
            gate_cli.run(
                self.hub,
                self.hub,
                [
                    "review",
                    "submission",
                    "moving",
                    "--owner",
                    "--verdict",
                    "approved",
                    "--report",
                    str(self.report),
                ],
            )
        self.assertEqual(self.snapshot(), before)

    def test_owner_07c_an_owner_receipt_never_covers_a_spec_subject(self) -> None:
        subject: SpecSubject = {"issue": "CK-1", "revision": "r1", "spec_sha256": "x"}
        receipt: ReviewRow = {
            "schema": "ck-review/1",
            "id": "forged",
            "kind": "spec",
            "subject": subject,
            "verdict": "approved",
            "reviewer": reviews.OWNER.copy(),
            "recorded": 1,
            "report_sha256": "0" * 64,
            "issues": [],
            "base_examined": None,
            "incremental_over": None,
            "revoked": False,
        }
        self.assertTrue(reviews.is_owner_receipt(receipt))
        self.assertFalse(reviews.coverage([], [receipt], subject))

    # ---- 8. owner receipts never bypass the integrity checks ----------------------------------

    def signed_batch(self, name: str) -> tuple[batch.Manager, str]:
        manager, bid = self.owner_batch(name)
        self.owner("batch", bid, "--verdict", "approved")
        return manager, bid

    def test_owner_08_publish_still_refuses_a_mutated_candidate(self) -> None:
        manager, bid = self.signed_batch("tamper-candidate")
        candidate = Path(self.validated(bid)["candidate"])
        (candidate / "source.txt").write_text("tampered candidate\n")
        self.record(candidate, "Tamper candidate")
        before = (self.hub / ".pipeline/accepted-v2.json").read_bytes()
        with self.assertRaisesRegex(RuntimeError, "candidate fingerprint changed"):
            manager.publish(bid)
        self.assertEqual((self.hub / ".pipeline/accepted-v2.json").read_bytes(), before)

    def test_owner_08b_publish_still_refuses_an_advanced_base(self) -> None:
        manager, bid = self.signed_batch("advanced-base")
        docs_source = Path(self.temp.name) / "docs-before-publish"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), docs_source, self.hub)
        (docs_source / "README.md").write_text("advance base\n")
        docsqueue.Queue(self.hub).capture(docs_source, "advance", ["README.md"])
        pointer.atomic_bytes(
            batch.batch_path(self.hub, bid) / "cancelled.json",
            pointer.canonical({"reason": "advance fixture"}),
        )
        self.assertEqual(docsqueue.Queue(self.hub).flush(), 1)
        with self.assertRaisesRegex(RuntimeError, "base advanced"):
            manager.publish(bid)

    def test_owner_08c_publish_still_refuses_a_changed_member_manifest(self) -> None:
        manager, bid = self.signed_batch("tamper-member")
        path = self.hub / ".pipeline/queue/tamper-member.json"
        manifest = json.loads(path.read_bytes())
        manifest["spec"] = "edited after the owner signed"
        pointer.atomic_bytes(path, pointer.canonical(manifest))
        with self.assertRaises(RuntimeError):
            manager.publish(bid)
        self.assertNotIn("tamper-member", pointer.read_pointer(self.hub).get("submissions", []))

    def test_owner_08d_publish_still_refuses_a_ci_receipt_that_did_not_pass(self) -> None:
        manager, bid = self.signed_batch("failed-ci")
        receipt_path = Path(self.validated(bid)["ci_receipt"])
        receipt = json.loads(receipt_path.read_bytes())
        receipt["passed"] = False
        pointer.atomic_bytes(receipt_path, pointer.canonical(receipt))
        with self.assertRaises(RuntimeError):
            manager.publish(bid)
        self.assertNotIn("failed-ci", pointer.read_pointer(self.hub).get("submissions", []))

    def test_owner_08e_prepare_still_excludes_a_member_whose_issue_is_not_ready(self) -> None:
        manifest = self.submission("unready-issue", review=False)
        manifest["issues"] = [{"id": "CK-404", "revision": "r1", "spec_sha256": "x"}]
        path = self.hub / ".pipeline/queue/unready-issue.json"
        pointer.atomic_bytes(path, pointer.canonical(manifest))
        self.sign_submission("unready-issue")
        self.assertNotIn("unready-issue", self.eligible_ids())
        with self.assertRaisesRegex(RuntimeError, "No eligible submissions"):
            batch.Manager(self.hub, self.passing).prepare()


if __name__ == "__main__":
    import unittest

    unittest.main()


# The inherited scenario tests belong to test_batch.py; only the owner tests run here.
for _name in dir(test_batch.BatchGateSpecification):
    if _name.startswith("test_") and not _name.startswith("test_owner_"):
        setattr(OwnerSignoff, _name, None)
