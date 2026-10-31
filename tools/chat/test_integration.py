"""Run in the staged shared chat suite, which installs its live-state guard first."""

import contextlib
import copy
import fcntl
import hashlib
import importlib.machinery
import importlib.util
import io
import itertools
import json
import os
import sqlite3
import sys
import threading
import time
import unittest
from collections.abc import Callable, Sequence
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

import _isolation  # noqa: F401  # imported for its isolation side effect
from test_bridge import CFG, STATE, bridge, chatlib, privmsg, reset_state

# isort: split
import codekarl_chat  # the staged module the bridge imported: the scripts directory is on sys.path now
import identities
from codekarl_chat import Inbox, InboxItem, Routing

# ck-chat is not installed: the suite runs the one in the checkout it is testing
CHECKOUT = Path(os.environ.get("CK_CHAT_SOURCE_ROOT") or Path.home() / "work/codekarl")
sys.path.append(str(CHECKOUT / "tools/chat"))
import ck_chat  # noqa: E402  # the checkout's tools/chat is on sys.path now


def table(value: object) -> dict[str, object]:
    """A JSON object of the test configuration (the shared fixture is plain parsed JSON)."""
    assert isinstance(value, dict)
    return value


def codekarl_project(config: dict[str, object]) -> dict[str, object]:
    return table(table(config["projects"])["codekarl"])


class CodekarlFixture(unittest.TestCase):
    def setUp(self) -> None:
        reset_state()
        self.cfg: dict[str, object] = copy.deepcopy(CFG)
        table(self.cfg["projects"])["codekarl"] = {
            "channel": "#codekarl",
            "prefix": "#ck-",
            "manager": "ck-manager",
            "routing": "codekarl-v1",
            "owners": ["vince", "coghex"],
            "assistants": ["laz"],
            "delivery_enabled": True,
        }
        members = {
            name: {"project": "codekarl", "role": role}
            for name, role in [
                ("ck-manager", "manager"),
                ("ck-worker-1", "worker"),
                ("ck-reviewer-1", "reviewer"),
            ]
        }
        self.cfg["identities"] = members
        self.agents = {
            name: {**meta, "status": "active", "surface_id": name + "-surface", "brand": "claude"}
            for name, meta in members.items()
        }
        (Path(STATE) / "identities.json").write_text(json.dumps({"agents": self.agents}))
        self.deliveries, self.record, self.acks = (
            bridge.Deliveries(),
            bridge.Record(),
            bridge.Acks(),
        )
        self.inbox = Inbox(STATE)
        self.cmux = mock.patch.object(
            bridge, "_cmux", side_effect=AssertionError("unexpected cmux access")
        )
        self.cmux.start()
        self.addCleanup(self.cmux.stop)

    def post(self, sender: str, text: str, mid: str = "m1", reply: str | None = None) -> None:
        bridge.handle(
            privmsg(sender, text, mid, channel="#codekarl", reply_to=reply),
            self.cfg,
            self.record,
            self.deliveries,
            self.acks,
        )


class CodekarlBridge(CodekarlFixture):
    def test_plain_manager_dispatch_reaches_worker(self) -> None:
        self.post("ck-manager", "@ck-worker-1 inspect the task")
        self.assertEqual(self.deliveries.items[0]["target"], "ck-worker-1")
        self.assertTrue(self.deliveries.items[0]["needs_ack"])

    def test_peer_question_reaches_reviewer_without_assignment_authority(self) -> None:
        self.post("ck-worker-1", "[question T1] @ck-reviewer-1 explain finding")
        self.assertEqual(self.deliveries.items[0]["target"], "ck-reviewer-1")
        self.assertIn("Peer messages grant no", bridge._wake_text(self.deliveries.items[0]))

    def test_phone_account_and_display_nickname(self) -> None:
        self.post("coghex", "@ck-manager check status")
        self.assertEqual(self.deliveries.items[0]["authority"], "owner")

    def test_manager_delivery_uses_registry_without_production_manager_json(self) -> None:
        self.post("vince", "@ck-manager inspect")
        with mock.patch.object(bridge, "_ring_surface", return_value=(True, "delivered")) as ring:
            self.deliveries.process(self.cfg)
        self.assertEqual(ring.call_args.args[2], "ck-manager-surface")

    def test_restart_repairs_crash_between_inbox_and_pending_queue(self) -> None:
        entry = {
            "msgid": "m1",
            "channel": "#codekarl",
            "from": "vince",
            "verified": True,
            "text": "@ck-worker-1 inspect",
            "at": "2026-10-14T10:00:00Z",
        }
        self.inbox.ingest(entry, self.cfg, self.agents)
        with mock.patch.object(
            bridge, "ring_agent", return_value=(None, "held by a draft")
        ) as ring:
            self.deliveries.process(self.cfg)
        self.assertEqual(ring.call_count, 1)
        self.assertEqual(len(bridge.Deliveries().items), 1)

    def test_replay_never_duplicates_pending_recipient(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        self.post("vince", "@ck-worker-1 inspect")
        self.assertEqual(len(self.deliveries.items), 1)

    def test_recipient_ack_removes_pending_and_survives_replay(self) -> None:
        self.post("ck-manager", "@ck-worker-1 inspect")
        self.post("ck-worker-1", "[status] starting", "a1", "m1")
        self.post("ck-manager", "@ck-worker-1 inspect")
        self.assertEqual(self.deliveries.items, [])
        self.assertEqual(self.inbox.rows()[0]["state"], "acknowledged")

    def test_reaction_ack_updates_durable_inbox(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        bridge.handle_tagmsg(
            {
                "tags": {
                    "account": "ck-worker-1",
                    "msgid": "a1",
                    "+draft/reply": "m1",
                    "+draft/react": "+1",
                },
                "params": ["#codekarl"],
            },
            self.cfg,
            self.deliveries,
            self.acks,
        )
        self.assertFalse(self.inbox.needs_delivery("m1", "ck-worker-1"))

    def test_claim_prevents_pending_delivery_and_completion_is_separate(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        self.inbox.claim("m1", "ck-worker-1")
        self.deliveries.process(self.cfg)
        self.assertEqual(self.deliveries.items, [])
        self.post("ck-worker-1", "[done] evidence path", "d1", "m1")
        original = next(r for r in self.inbox.rows(history=True) if r["message"] == "m1")
        self.assertEqual(original["state"], "completed")

    def test_activation_hold_prevents_cmux_access_but_preserves_inbox(self) -> None:
        table(table(self.cfg["projects"])["codekarl"])["delivery_enabled"] = False
        self.post("vince", "@ck-worker-1 inspect")
        self.deliveries.process(self.cfg)
        self.assertEqual(self.deliveries.items, [])
        self.assertEqual(len(self.inbox.rows()), 1)

    def test_dead_letter_stays_visible_without_reenqueue(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        item = self.deliveries.items[0]
        item["attempt"] = len(bridge.BACKOFF)
        with mock.patch.object(bridge, "ring_agent", return_value=(False, "no live session")):
            self.deliveries.process(self.cfg)
        self.assertEqual(self.inbox.rows()[0]["notice"], "failed")
        self.assertEqual(self.inbox.notices(self.cfg), [])

    def test_worker_role_numbers_are_independent_and_never_reused(self) -> None:
        registry: dict[str, object] = {"agents": {}, "counters": {}}
        first = identities.allocate(
            self.cfg, registry, project="codekarl", role="worker", key="session:1"
        )
        reviewer = identities.allocate(
            self.cfg, registry, project="codekarl", role="reviewer", key="session:2"
        )
        first["status"] = "retired"
        second = identities.allocate(
            self.cfg, registry, project="codekarl", role="worker", key="session:3"
        )
        resumed = identities.allocate(
            self.cfg, registry, project="codekarl", role="worker", key="session:1"
        )
        self.assertEqual(
            (first["name"], reviewer["name"], second["name"], resumed["name"]),
            ("ck-worker-1", "ck-reviewer-1", "ck-worker-2", "ck-worker-1"),
        )

    def test_worker_role_is_not_added_to_other_projects(self) -> None:
        with self.assertRaisesRegex(chatlib.ChatError, "scoped"):
            identities.allocate(
                self.cfg,
                {"agents": {}, "counters": {}},
                project="mosk",
                role="worker",
                key="session:1",
            )


class CodekarlCLI(CodekarlFixture):
    def setUp(self) -> None:
        super().setUp()
        self.cli = ck_chat
        self.enterContext(mock.patch.object(chatlib, "load_config", return_value=self.cfg))
        self.enterContext(
            mock.patch.object(
                identities,
                "current",
                return_value={"name": "ck-worker-1", "project": "codekarl", "status": "active"},
            )
        )

    def command(self, *args: str) -> int:
        with mock.patch.object(sys, "argv", ["ck-chat", *args]), redirect_stdout(io.StringIO()):
            return self.cli.main()

    def test_post_options_and_literal_message_reach_existing_transport(self) -> None:
        with mock.patch.object(self.cli, "pchat", return_value=0) as post:
            self.command("post", "--type", "question", "@ck-manager literal $text")
        post.assert_called_once_with(
            ["post", "#codekarl", "--type", "question", "@ck-manager literal $text"]
        )

    def test_duplicate_claim_does_not_send_another_ack(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        with mock.patch.object(self.cli, "pchat", return_value=0) as ack:
            self.assertEqual(self.command("claim", "m1"), 0)
            # CK-5: a plain request is actionable, so the claim makes the reaction and the
            # visible acknowledgement; the repeated claim still adds nothing.
            self.assertEqual(
                ack.call_args_list,
                [
                    mock.call(["ack", "#codekarl", "m1"]),
                    mock.call(
                        [
                            "post",
                            "#codekarl",
                            "--reply-to",
                            "m1",
                            "[ack] claimed by ck-worker-1",
                        ]
                    ),
                ],
            )
            self.assertEqual(self.command("claim", "m1"), 4)
            self.assertEqual(ack.call_count, 2)

    def test_outbox_failure_keeps_claim_for_recovery(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        with mock.patch.object(self.cli, "pchat", return_value=3):
            self.assertEqual(self.command("claim", "m1"), 3)
        self.assertEqual(self.inbox.rows()[0]["state"], "acknowledged")

    def test_other_project_agent_cannot_post(self) -> None:
        with (
            mock.patch.object(identities, "current", return_value={"project": "synarchy"}),
            self.assertRaisesRegex(ValueError, "another project"),
        ):
            self.command("post", "unrelated message")

    def test_finish_uses_exact_message_reference(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        with mock.patch.object(self.cli, "pchat", return_value=0) as post:
            self.command("done", "m1", "receipt path")
        post.assert_called_once_with(
            ["post", "#codekarl", "--reply-to", "m1", "[done] receipt path"]
        )


DELIVERED_DETAIL = "typed into prompt and submitted: ck-worker-1"
ACCEPTED_DETAIL = "accepted by ck-worker-1 (ack:ack)"


class InboxCorrectness(CodekarlFixture):
    """CK-4: delivery observations, unknown kinds and bridge status, on a real temporary Inbox."""

    def setUp(self) -> None:
        super().setUp()
        self.ck = codekarl_chat

    def entry(  # noqa: PLR0913  # test helper with explicit scenario arguments
        self,
        sender: str,
        text: str,
        mid: str,
        reply_to: str | None = None,
        verified: bool = True,
        channel: str = "#codekarl",
    ) -> dict[str, object]:
        entry: dict[str, object] = {
            "msgid": mid,
            "channel": channel,
            "from": sender,
            "verified": verified,
            "text": text,
            "at": "2026-10-15T10:00:00Z",
        }
        if reply_to:
            entry["reply_to"] = reply_to
        return entry

    def ingest(
        self,
        sender: str,
        text: str,
        mid: str = "m1",
        reply_to: str | None = None,
        verified: bool = True,
    ) -> Routing:
        return self.inbox.ingest(
            self.entry(sender, text, mid, reply_to=reply_to, verified=verified),
            self.cfg,
            self.agents,
        )

    def item(
        self, mid: str = "m1", target: str = "ck-worker-1", **extra: object
    ) -> dict[str, object]:
        return {"ck_policy": self.ck.POLICY, "msgid": mid, "target": target, "attempt": 0, **extra}

    def row(
        self, mid: str = "m1", recipient: str = "ck-worker-1", inbox: Inbox | None = None
    ) -> InboxItem:
        rows = (inbox or self.inbox).rows(history=True)
        return next(r for r in rows if r["message"] == mid and r["recipient"] == recipient)

    def kinds(self, mid: str = "m1", recipient: str = "ck-worker-1") -> list[str]:
        return [o["kind"] for o in self.row(mid, recipient)["observations"]]

    def events(self, mid: str = "m1", recipient: str = "ck-worker-1") -> list[tuple[object, ...]]:
        db = sqlite3.connect(self.inbox.path)
        try:
            return [
                tuple(r)
                for r in db.execute(
                    "SELECT state, detail FROM events WHERE message=? AND recipient=? ORDER BY id",
                    (mid, recipient),
                )
            ]
        finally:
            db.close()

    def test_01_early_claim_then_late_confirmation(self) -> None:
        self.ingest("vince", "@ck-worker-1 inspect")
        self.inbox.claim("m1", "ck-worker-1")
        self.inbox.outcome(self.item(), True, DELIVERED_DETAIL)
        row = self.row()
        self.assertEqual(row["state"], "acknowledged")
        self.assertEqual(self.kinds(), ["delivered"])
        self.assertEqual(row["notice"], "delivered")

    def test_02_completion_before_confirmation(self) -> None:
        self.ingest("vince", "@ck-worker-1 inspect")
        self.ingest("ck-worker-1", "[done] evidence", "d1", reply_to="m1")
        self.assertEqual(self.row()["state"], "completed")
        self.inbox.outcome(self.item(), True, DELIVERED_DETAIL)
        row = self.row()
        self.assertEqual(row["state"], "completed")
        self.assertEqual(self.kinds(), ["delivered"])
        self.assertEqual(row["notice"], "delivered")

    def test_03_acceptance_is_not_delivery(self) -> None:
        self.ingest("vince", "@ck-worker-1 inspect")
        self.inbox.outcome(self.item(), True, ACCEPTED_DETAIL)
        row = self.row()
        self.assertEqual((row["state"], row["notice"]), ("queued", "accepted"))
        self.inbox.outcome(self.item(), True, DELIVERED_DETAIL)
        row = self.row()
        self.assertEqual((row["state"], row["notice"]), ("delivered", "delivered"))
        self.assertEqual(self.kinds(), ["accepted", "delivered"])

    def test_04_manual_claim_without_a_wake(self) -> None:
        self.ingest("vince", "@ck-worker-1 inspect")
        self.inbox.claim("m1", "ck-worker-1")
        row = self.row()
        self.assertEqual(row["observations"], [])
        self.assertEqual((row["state"], row["notice"]), ("acknowledged", "pending"))

    def test_05_duplicate_receipts_are_each_recorded_but_advance_once(self) -> None:
        self.ingest("vince", "@ck-worker-1 inspect")
        self.inbox.outcome(self.item(), True, DELIVERED_DETAIL)
        self.inbox.outcome(self.item(), True, DELIVERED_DETAIL)
        self.assertEqual(self.kinds(), ["delivered", "delivered"])
        self.assertEqual(self.row()["state"], "delivered")
        self.assertEqual(
            [state for state, _ in self.events() if state == "delivered"], ["delivered"]
        )

    def test_06_observations_are_per_recipient(self) -> None:
        self.ingest("vince", "@ck-worker-1 @ck-reviewer-1 inspect")
        self.inbox.outcome(self.item(), True, DELIVERED_DETAIL)
        other = self.row(recipient="ck-reviewer-1")
        self.assertEqual(
            (other["state"], other["notice"], other["observations"]), ("queued", "pending", [])
        )
        self.assertEqual(self.row()["notice"], "delivered")

    def test_07_restart_keeps_observations_and_old_databases_gain_the_table(self) -> None:
        self.ingest("vince", "@ck-worker-1 inspect")
        self.inbox.outcome(self.item(), True, DELIVERED_DETAIL)
        again = self.ck.Inbox(STATE)
        self.assertEqual([o["kind"] for o in self.row(inbox=again)["observations"]], ["delivered"])
        db = sqlite3.connect(self.inbox.path)
        db.execute("DROP TABLE observations")
        db.commit()
        db.close()
        old = self.ck.Inbox(STATE)
        row = self.row(inbox=old)
        self.assertEqual(
            (row["state"], row["notice"], row["observations"]), ("delivered", "delivered", [])
        )
        old.outcome(self.item(), True, ACCEPTED_DETAIL)
        self.assertEqual([o["kind"] for o in self.row(inbox=old)["observations"]], ["accepted"])

    def test_08_failed_delivery_and_dead_letter(self) -> None:
        self.ingest("vince", "@ck-worker-1 inspect")
        self.inbox.outcome(self.item(), None, "queued at ck-worker-1 (cmux)")
        self.assertEqual(self.row()["notice"], "waiting")
        self.inbox.outcome(self.item(), False, "no live session")
        row = self.row()
        self.assertEqual((row["state"], row["notice"]), ("queued", "attempt-failed"))
        self.inbox.outcome(self.item(), False, "dead-lettered: no live session")
        row = self.row()
        self.assertEqual((row["state"], row["notice"]), ("queued", "failed"))
        self.assertIn("needs-attention", [state for state, _ in self.events()])
        self.assertEqual(len(self.inbox.rows()), 1)

    def test_08a_dead_letter_after_delivery_is_never_rerung(self) -> None:
        self.ingest("vince", "@ck-worker-1 inspect")
        self.inbox.outcome(self.item(), True, DELIVERED_DETAIL)
        for n in (1, 2, 3):
            self.inbox.outcome(self.item(), False, f"not accepted after 15 min; reminder {n}")
        self.inbox.outcome(self.item(), False, "dead-lettered: delivered but never accepted")
        for inbox in (self.inbox, self.ck.Inbox(STATE)):
            self.assertEqual(self.row(inbox=inbox)["notice"], "failed")
            self.assertEqual(inbox.notices(self.cfg), [])
            self.assertEqual(len(inbox.rows()), 1)
        self.assertIn("needs-attention", [state for state, _ in self.events()])
        # A pre-change dead letter has the notice but no observations; it is excluded too.
        self.ingest("vince", "@ck-worker-1 older", "m2")
        db = sqlite3.connect(self.inbox.path)
        db.execute("UPDATE inbox SET notice='failed' WHERE message='m2'")
        db.commit()
        db.close()
        self.assertEqual(self.inbox.notices(self.cfg), [])
        self.inbox.outcome(self.item("m2"), True, DELIVERED_DETAIL)
        self.assertEqual(self.inbox.notices(self.cfg), [])

    def test_09_prompt_and_agent_message_paths(self) -> None:
        self.ingest("vince", "@ck-worker-1 inspect")
        self.inbox.outcome(self.item(), True, DELIVERED_DETAIL)
        self.assertEqual(self.row()["state"], "delivered")
        self.inbox.claim("m1", "ck-worker-1")
        row = self.row()
        self.assertEqual((row["state"], row["notice"]), ("acknowledged", "delivered"))

        self.ingest("vince", "@ck-worker-1 second", "m2")
        self.inbox.outcome(self.item("m2", agent_msg_id="cmux-1"), None, "queued at ck-worker-1")
        self.inbox.claim("m2", "ck-worker-1")
        self.inbox.outcome(self.item("m2", agent_msg_id="cmux-1"), True, ACCEPTED_DETAIL)
        row = self.row("m2")
        self.assertEqual(self.kinds("m2"), ["waiting", "accepted"])
        self.assertEqual((row["state"], row["notice"]), ("acknowledged", "accepted"))
        self.assertNotIn("delivered", [state for state, _ in self.events("m2")])

    def test_10_unknown_kinds_are_actionable_requests(self) -> None:
        routing = self.ingest("vince", "[task T1] @ck-worker-1 leave unfinished")
        self.assertTrue(routing["actionable"])
        self.assertEqual(routing["recipients"], ["ck-worker-1"])
        self.inbox.claim("m1", "ck-worker-1")
        self.assertEqual([r["message"] for r in self.inbox.rows()], ["m1"])
        self.ingest("ck-worker-1", "[done] finished", "d1", reply_to="m1")
        self.assertEqual([r for r in self.inbox.rows() if r["message"] == "m1"], [])

        for index, (text, actionable) in enumerate(
            [
                ("@ck-worker-1 plain request", True),
                ("[question] @ck-worker-1 which?", True),
                ("[blocked] @ck-worker-1 stuck", True),
                ("!! @ck-worker-1 hold", True),
                ("[status] @ck-worker-1 note", False),
                ("[done] @ck-worker-1 finished", False),
                ("[answer] @ck-worker-1 yes", False),
                ("[declined] @ck-worker-1 no", False),
                ("[ack] @ck-worker-1 ok", False),
                ("[decision] @ck-worker-1 chosen", False),
            ]
        ):
            with self.subTest(text=text):
                routing = self.ingest("vince", text, f"k{index}")
                self.assertEqual(routing["actionable"], actionable)
        self.assertEqual(self.ingest("vince", "[ack] @ck-worker-1 ok", "kack")["recipients"], [])
        self.assertEqual(self.ck.message_kind("[task T1] x"), "request")
        self.assertEqual(self.ck.message_kind("[status] x"), "status")

        untrusted = self.ingest("mallory", "[task T2] @ck-worker-1 do it", "u1", verified=False)
        self.assertEqual((untrusted["role"], untrusted["recipients"]), ("untrusted", []))
        unknown_account = self.ingest("mallory", "[task T3] @ck-worker-1 do it", "u2")
        self.assertEqual(
            (unknown_account["role"], unknown_account["recipients"]), ("untrusted", [])
        )

    def test_11_stored_unknown_kind_rows_are_outstanding_after_a_claim(self) -> None:
        self.ingest("vince", "[task T1] @ck-worker-1 leave unfinished")
        db = sqlite3.connect(self.inbox.path)
        routing = json.loads(db.execute("SELECT routing FROM messages WHERE id='m1'").fetchone()[0])
        routing["actionable"] = False
        db.execute("UPDATE messages SET routing=? WHERE id='m1'", (json.dumps(routing),))
        db.commit()
        db.close()
        self.inbox.claim("m1", "ck-worker-1")
        self.assertEqual([r["message"] for r in self.inbox.rows()], ["m1"])
        # A stored informational row stays quiet: widening only covers actionable kinds.
        self.ingest("vince", "[status] @ck-worker-1 note", "m2")
        self.inbox.claim("m2", "ck-worker-1")
        self.assertEqual([r["message"] for r in self.inbox.rows()], ["m1"])

    def test_12_bridge_status_has_a_narrow_role_and_everything_else_stays_untrusted(self) -> None:
        for index, text in enumerate(
            [
                "[status] chatbridge: interrupt \u2192 ck-worker-1: hold",
                "[status] chatbridge: note @ck-worker-1 please look",
                "[status] chatbridge: !! @ck-worker-1 hold",
            ]
        ):
            with self.subTest(text=text):
                routing = self.ingest("chatbridge", text, f"b{index}")
                self.assertEqual(routing["role"], "bridge")
                self.assertEqual(routing["recipients"], [])
                self.assertFalse(routing.get("interrupt"))
                self.assertFalse(routing["actionable"])
                self.assertEqual(routing["reason"], "")
        self.assertEqual(self.inbox.diagnostics(), [])
        self.assertEqual(self.inbox.rows(history=True), [])

        for index, (sender, text, verified) in enumerate(
            [
                ("chatbridge", "[status] chatbridge: note", False),
                ("chatbridge", "[request] chatbridge: @ck-worker-1 do it", True),
                ("chatbridge", "[status] note @ck-worker-1", True),
                ("chatbridge", "chatbridge: plain text @ck-worker-1", True),
                ("mallory", "[status] chatbridge: note", True),
            ]
        ):
            with self.subTest(sender=sender, text=text, verified=verified):
                routing = self.ingest(sender, text, f"x{index}", verified=verified)
                self.assertEqual((routing["role"], routing["recipients"]), ("untrusted", []))
                self.assertTrue(routing["reason"])
        self.assertEqual(len(self.inbox.diagnostics()), 5)
        # Authority is not conferred elsewhere: a worker posting the same text keeps its own role.
        self.assertEqual(
            self.ingest("ck-worker-1", "[status] chatbridge: note", "w1")["role"], "worker"
        )


EMPTY = "empty cmux process inventory; existing identities preserved"
WORKER = {"name": "ck-worker-1", "project": "codekarl", "status": "active"}
VISIBLE = "[ack] claimed by ck-worker-1"
ACTIONABLE_MESSAGES = [
    ("vince", "@ck-worker-1 inspect"),
    ("ck-manager", "[question] @ck-worker-1 which file?"),
    ("ck-manager", "[blocked] @ck-worker-1 waiting on you"),
    ("vince", "!! @ck-worker-1 stop and read this"),
    ("ck-manager", "[task T1] @ck-worker-1 do the thing"),
]
INFORMATIONAL_MESSAGES = [
    ("ck-manager", "[status] @ck-worker-1 progress note"),
    ("ck-manager", "[done] @ck-worker-1 thanks"),
    ("ck-manager", "[answer] @ck-worker-1 yes"),
    ("ck-manager", "[declined] @ck-worker-1 no"),
]


class Stop(BaseException):
    """A process dying at an exact point: nothing in ck-chat may catch it."""


class RecoveryAcks(CodekarlFixture):
    """CK-5: ck-chat discovery recovery (PF-02) and visible acknowledgements (PF-05)."""

    def setUp(self) -> None:
        super().setUp()
        # The real pchat resolves who is speaking from these, so results must never depend on
        # who runs the suite (a manager shell exports CHAT_AS and CHAT_AGENT_ID).
        environment = mock.patch.dict(os.environ)
        environment.start()
        self.addCleanup(environment.stop)
        for name in list(os.environ):
            if name in {"CHAT_AS", "CHAT_AGENT_ID", "CHAT_ROLE"} or name.startswith("CMUX_"):
                del os.environ[name]
        self.cli = ck_chat
        self.Inbox = Inbox
        self.calls: list[list[str]] = []
        self.codes: dict[str, int] = {"ack": 0, "post": 0}
        self.hook: Callable[[Sequence[str]], object] | None = None
        self.real_current = identities.current
        self.current = mock.patch.object(identities, "current", return_value=dict(WORKER)).start()
        self.pchat = mock.patch.object(self.cli, "pchat", side_effect=self.fake_pchat)
        self.pchat.start()
        self.sleeps = mock.patch.object(time, "sleep").start()
        mock.patch.object(chatlib, "load_config", return_value=self.cfg).start()
        self.addCleanup(mock.patch.stopall)

    def test_00_the_suite_does_not_depend_on_the_callers_identity(self) -> None:
        leaked = [
            name
            for name in os.environ
            if name in {"CHAT_AS", "CHAT_AGENT_ID", "CHAT_ROLE"} or name.startswith("CMUX_")
        ]
        self.assertEqual(leaked, [])

    def fake_pchat(self, arguments: Sequence[str]) -> int:
        self.calls.append(list(arguments))
        if self.hook:
            self.hook(arguments)
        return self.codes[arguments[0]]

    def run_cli(self, *args: str) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        with (
            mock.patch.object(sys, "argv", ["ck-chat", *args]),
            redirect_stdout(out),
            redirect_stderr(err),
        ):
            code = self.cli.main()
        return code, out.getvalue(), err.getvalue()

    def ack_call(self, mid: str = "m1") -> list[str]:
        return ["ack", "#codekarl", mid]

    def visible_call(self, mid: str = "m1") -> list[str]:
        return ["post", "#codekarl", "--reply-to", mid, VISIBLE]

    def states(self, mid: str = "m1", inbox: Inbox | None = None) -> dict[str, str]:
        rows = (inbox or self.Inbox(STATE)).effects(mid, "ck-worker-1")
        return {effect: row["state"] for effect, row in rows.items()}

    def task_state(self, mid: str = "m1") -> str:
        rows = self.Inbox(STATE).rows(history=True)
        return next(r for r in rows if r["message"] == mid and r["recipient"] == "ck-worker-1")[
            "state"
        ]

    def set_effect(self, effect: str, state: str, mid: str = "m1", holder: str = "pid=1") -> None:
        """Put an effect row into a state the way a finished (or killed) command would have."""
        db = sqlite3.connect(self.Inbox(STATE).path)
        try:
            with db:
                db.execute(
                    "UPDATE ack_effects SET state=?, token='t', holder=? "
                    "WHERE message=? AND recipient='ck-worker-1' AND effect=?",
                    (state, holder, mid, effect),
                )
        finally:
            db.close()

    def use_real_pchat(self) -> None:
        """The shared pchat (the staged copy under the suite), in-process; chatlib network
        functions are faked per test. ck-chat loads it from the owner's home, which the
        sandbox replaces, so the same loader runs on the file next to chatlib."""
        path = Path(chatlib.__file__).resolve().parent / "pchat"

        def staged_pchat(arguments: Sequence[str]) -> int:
            loader = importlib.machinery.SourceFileLoader("ck_pchat_staged", str(path))
            spec = importlib.util.spec_from_loader("ck_pchat_staged", loader)
            assert spec is not None
            module = importlib.util.module_from_spec(spec)
            loader.exec_module(module)
            code: object = module.main(arguments) or 0
            assert isinstance(code, int)
            return code

        import runstore  # noqa: PLC0415  # module is on sys.path only after the test bridge has loaded

        self.pchat.stop()
        mock.patch.object(self.cli, "pchat", staged_pchat).start()
        self.cfg["agent_identities_enabled"] = True
        table(self.cfg["accounts"])["ck-worker-1"] = "x"
        mock.patch.object(runstore, "silent_refusal", return_value=None).start()
        conn = mock.Mock(caps=set())
        mock.patch.object(chatlib, "login", return_value=conn).start()
        mock.patch.object(chatlib, "_round", return_value=set()).start()

    # --- Part A: PF-02 ---------------------------------------------------------------------

    def test_01_transient_discovery_failure_is_retried_once_resolved(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        self.current.side_effect = [chatlib.ChatError(EMPTY), chatlib.ChatError(EMPTY), WORKER]
        code, _, _ = self.run_cli("done", "m1", "receipt")
        self.assertEqual(code, 0)
        self.assertEqual(self.current.call_count, 3)
        self.assertEqual(self.calls, [["post", "#codekarl", "--reply-to", "m1", "[done] receipt"]])
        self.assertEqual([c.args[0] for c in self.sleeps.call_args_list], [0.5, 1.0])

    def test_02_persistent_discovery_failure_sends_and_changes_nothing(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        self.current.side_effect = chatlib.ChatError(EMPTY)
        for args in (("claim", "m1"), ("done", "m1", "x"), ("decline", "m1", "x"), ("post", "hi")):
            with self.subTest(args=args):
                self.current.reset_mock()
                code, out, err = self.run_cli(*args)
                self.assertEqual(code, 5)
                self.assertEqual(self.current.call_count, 3)
                self.assertEqual(out, "")
                self.assertIn(
                    "ck-chat: NOT SENT: identity discovery failed before sending "
                    "(empty cmux process inventory); nothing was posted, acknowledged or "
                    "queued; safe to retry",
                    err,
                )
        self.assertEqual(self.calls, [])
        self.assertEqual(self.task_state(), "queued")
        self.assertEqual(self.states(), {})

    def test_03_other_errors_are_not_retried(self) -> None:
        import identity_conflicts  # noqa: PLC0415  # module is on sys.path only after the test bridge has loaded

        self.post("vince", "@ck-worker-1 inspect")
        for error in (
            chatlib.ChatError("invalid CHAT_AGENT_ID"),
            identity_conflicts.IdentityConflict({"name": "ck-worker-1"}, {}),
        ):
            with self.subTest(error=str(error)):
                self.current.reset_mock(side_effect=True)
                self.current.side_effect = error
                with self.assertRaises(chatlib.ChatError):
                    self.run_cli("done", "m1", "x")
                self.assertEqual(self.current.call_count, 1)
        self.assertEqual(self.calls, [])
        self.sleeps.assert_not_called()

    def test_04_one_resolution_and_the_environment_is_restored(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        seen: list[str | None] = []
        self.hook = lambda arguments: seen.append(os.environ.get("CHAT_AGENT_ID"))
        with mock.patch.dict(os.environ):
            os.environ.pop("CHAT_AGENT_ID", None)
            self.run_cli("done", "m1", "x")
            self.assertNotIn("CHAT_AGENT_ID", os.environ)
            os.environ["CHAT_AGENT_ID"] = "previous"
            self.run_cli("done", "m1", "y")
            self.assertEqual(os.environ["CHAT_AGENT_ID"], "previous")
            os.environ.pop("CHAT_AGENT_ID")
            self.run_cli("claim", "m1")
            self.assertNotIn("CHAT_AGENT_ID", os.environ)
        self.assertEqual(seen, ["ck-worker-1"] * 4)

        # An exception inside the call restores it too.
        def explode(arguments: Sequence[str]) -> None:
            raise OSError("boom")

        self.hook = explode
        with mock.patch.dict(os.environ):
            os.environ["CHAT_AGENT_ID"] = "previous"
            with self.assertRaises(OSError):
                self.run_cli("done", "m1", "z")
            self.assertEqual(os.environ["CHAT_AGENT_ID"], "previous")
        # An owner shell has no bound actor, so nothing is set.
        self.hook = lambda arguments: seen.append(os.environ.get("CHAT_AGENT_ID"))
        self.current.side_effect = None
        self.current.return_value = None
        with mock.patch.dict(os.environ):
            os.environ.pop("CHAT_AGENT_ID", None)
            seen.clear()
            self.assertEqual(self.run_cli("post", "owner note")[0], 0)
        self.assertEqual(seen, [None])

    def test_05_a_bound_call_still_refuses_a_surface_conflict(self) -> None:
        import identity_conflicts  # noqa: PLC0415  # module is on sys.path only after the test bridge has loaded

        surface = "SURFACE-1"
        self.cfg["agent_identities_enabled"] = True
        env = {"CMUX_SURFACE_ID": surface, "CHAT_AGENT_ID": "ck-worker-1"}
        with (
            mock.patch.dict(os.environ, env),
            mock.patch.object(identities, "current", self.real_current),
            mock.patch.object(identities, "sync", side_effect=AssertionError("second discovery")),
        ):
            agent = identities.current(self.cfg)
            assert agent is not None
            self.assertEqual(agent["role"], "worker")
            conflict = {
                "surface_id": surface,
                "project": "codekarl",
                "role": "worker",
                "incumbent": "ck-worker-1",
            }
            Path(STATE, "identity-conflicts.json").write_text(json.dumps([conflict]))
            with self.assertRaises(identity_conflicts.IdentityConflict):
                identities.current(self.cfg)

    def test_06_queued_posts_are_not_retried(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        self.codes["post"] = 3
        code, _, _ = self.run_cli("done", "m1", "x")
        self.assertEqual(code, 3)
        self.assertEqual(len(self.calls), 1)
        self.sleeps.assert_not_called()

    # --- Part B: PF-05 ---------------------------------------------------------------------

    def test_07_actionable_kinds_get_a_visible_acknowledgement(self) -> None:
        for index, (sender, text) in enumerate(ACTIONABLE_MESSAGES):
            mid = f"a{index}"
            with self.subTest(text=text):
                self.post(sender, text, mid)
                self.calls.clear()
                code, out, _ = self.run_cli("claim", mid)
                self.assertEqual(code, 0)
                self.assertEqual(self.calls, [self.ack_call(mid), self.visible_call(mid)])
                self.assertEqual(
                    json.loads(out)["effects"], {"reaction": "sent", "visible": "sent"}
                )

    def test_08_informational_kinds_get_the_reaction_only(self) -> None:
        for index, (sender, text) in enumerate(INFORMATIONAL_MESSAGES):
            mid = f"i{index}"
            with self.subTest(text=text):
                self.post(sender, text, mid)
                self.calls.clear()
                code, out, _ = self.run_cli("claim", mid)
                self.assertEqual(code, 0)
                self.assertEqual(self.calls, [self.ack_call(mid)])
                report = json.loads(out)
                self.assertEqual(report["effects"], {"reaction": "sent", "visible": None})
                self.assertTrue(report["claim"]["fresh"])
                self.assertEqual(set(self.states(mid)), {"reaction"})

    def test_09_a_duplicate_claim_posts_nothing(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        self.assertEqual(self.run_cli("claim", "m1")[0], 0)
        self.calls.clear()
        code, out, err = self.run_cli("claim", "m1")
        self.assertEqual((code, self.calls), (4, []))
        self.assertFalse(json.loads(out)["claim"]["fresh"])
        self.assertNotIn("ck-chat effects", err)

    def test_10_exit_codes_and_per_effect_states(self) -> None:
        cases = [
            ((0, 0), 0, ("sent", "sent")),
            ((0, 3), 3, ("sent", "queued")),
            ((3, 0), 3, ("queued", "sent")),
            ((3, 3), 3, ("queued", "queued")),
            ((3, 2), 6, ("queued", "not-sent")),
            ((2, 0), 6, ("not-sent", "sent")),
            ((2, 3), 6, ("not-sent", "queued")),
            ((4, 0), 6, ("not-sent", "sent")),
        ]
        for index, (codes, expected, states) in enumerate(cases):
            mid = f"c{index}"
            with self.subTest(codes=codes):
                self.post("vince", "@ck-worker-1 inspect", mid)
                self.calls.clear()
                self.codes.update(ack=codes[0], post=codes[1])
                code, out, err = self.run_cli("claim", mid)
                self.assertEqual(code, expected)
                self.assertEqual(self.calls, [self.ack_call(mid), self.visible_call(mid)])
                report = json.loads(out)
                self.assertEqual(
                    report["effects"], dict(zip(("reaction", "visible"), states, strict=False))
                )
                self.assertEqual(self.states(mid), report["effects"])
                self.assertEqual(self.task_state(mid), "acknowledged")
                if expected == 6:  # noqa: PLR2004  # exit code 6 is 'ack effect incomplete'
                    self.assertIn(f"ck-chat effects {mid}", err)
                    for effect, state in report["effects"].items():
                        if state == "not-sent":
                            self.assertIn(effect, err)
        self.post("ck-manager", "[status] @ck-worker-1 note", "s1")
        self.calls.clear()
        self.codes.update(ack=3, post=0)
        code, out, _ = self.run_cli("claim", "s1")
        self.assertEqual(code, 3)
        self.assertEqual(json.loads(out)["effects"], {"reaction": "queued", "visible": None})
        self.assertEqual(self.states("s1"), {"reaction": "queued"})
        self.assertEqual(self.calls, [self.ack_call("s1")])

    def test_10a_interruption_leaves_stored_states_that_effects_reports(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        # Before the first send.
        with (
            mock.patch.object(self.cli, "attempt", side_effect=Stop),
            self.assertRaises(Stop),
        ):
            self.run_cli("claim", "m1")
        self.assertEqual(self.states(), {"reaction": "pending", "visible": "pending"})
        code, out, _ = self.run_cli("effects", "m1")
        self.assertEqual(code, 0)
        shown = json.loads(out)["effects"]
        self.assertEqual({k: v["state"] for k, v in shown.items()}, self.states())
        self.assertEqual(self.calls, [])

        # Between the sends.
        self.post("vince", "@ck-worker-1 inspect", "m2")
        real, counter = self.Inbox.acquire, itertools.count()

        def second_acquire_dies(
            inbox: Inbox, mid: str, recipient: str, effect: str, holder: str
        ) -> str | None:
            if next(counter) == 1:
                raise Stop
            return real(inbox, mid, recipient, effect, holder)

        with (
            mock.patch.object(self.Inbox, "acquire", second_acquire_dies),
            self.assertRaises(Stop),
        ):
            self.run_cli("claim", "m2")
        self.assertEqual(self.states("m2"), {"reaction": "sent", "visible": "pending"})
        self.assertEqual(self.calls, [self.ack_call("m2")])

        # After a send, before its result is stored.
        self.post("vince", "@ck-worker-1 inspect", "m3")
        self.calls.clear()
        with (
            mock.patch.object(self.Inbox, "record_effect", side_effect=Stop),
            self.assertRaises(Stop),
        ):
            self.run_cli("claim", "m3")
        self.assertEqual(self.states("m3"), {"reaction": "attempting", "visible": "pending"})
        _, out, _ = self.run_cli("effects", "m3")
        reaction = json.loads(out)["effects"]["reaction"]
        self.assertEqual(reaction["state"], "attempting")
        self.assertIn(str(os.getpid()), reaction["holder"])
        self.assertTrue(reaction["alive"])
        with mock.patch.object(self.cli, "pid_alive", return_value=False):
            _, out, _ = self.run_cli("effects", "m3")
        self.assertFalse(json.loads(out)["effects"]["reaction"]["alive"])
        # It still counts toward exit 6 in a retry that attempts the other effect, and is
        # never resent.
        self.calls.clear()
        code, _, err = self.run_cli("effects", "m3", "--retry")
        self.assertEqual(code, 6)
        self.assertEqual(self.calls, [self.visible_call("m3")])
        self.assertEqual(self.states("m3"), {"reaction": "attempting", "visible": "sent"})
        self.assertIn("reaction", err)

    def test_10b_retry_sends_only_pending_and_not_sent(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        self.Inbox(STATE).claim("m1", "ck-worker-1")
        for state in ("sent", "queued", "attempting", "uncertain"):
            with self.subTest(state=state):
                self.set_effect("reaction", state)
                self.set_effect("visible", state)
                code, _out, _ = self.run_cli("effects", "m1", "--retry")
                self.assertEqual((code, self.calls), (0, []))
                self.assertEqual(self.states(), {"reaction": state, "visible": state})
        for state in ("pending", "not-sent"):
            with self.subTest(state=state):
                self.set_effect("reaction", state)
                self.set_effect("visible", "sent")
                self.calls.clear()
                code, _, _ = self.run_cli("effects", "m1", "--retry")
                self.assertEqual((code, self.calls), (0, [self.ack_call()]))
                self.assertEqual(self.states(), {"reaction": "sent", "visible": "sent"})
                self.assertEqual(self.run_cli("effects", "m1", "--retry")[0], 0)
                self.assertEqual(self.calls, [self.ack_call()])
        # Retry follows the classification: a failing retry is reported, not hidden.
        self.set_effect("reaction", "not-sent")
        self.set_effect("visible", "pending")
        self.codes.update(ack=2, post=3)
        code, _, _ = self.run_cli("effects", "m1", "--retry")
        self.assertEqual(code, 6)
        self.assertEqual(self.states(), {"reaction": "not-sent", "visible": "queued"})
        # `--confirm-unsent` no longer exists.
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            self.run_cli("effects", "m1", "--confirm-unsent")

    def test_10b_queue_then_interrupt_never_resends(self) -> None:
        self.post("ck-manager", "[done] @ck-worker-1 thanks")
        self.use_real_pchat()
        login = mock.patch.object(chatlib, "login").start()
        login.side_effect = chatlib.ChatError("irc is down")
        with (
            mock.patch.object(self.Inbox, "record_effect", side_effect=Stop),
            self.assertRaises(Stop),
        ):
            self.run_cli("claim", "m1")
        self.assertEqual(self.states(), {"reaction": "attempting"})
        entries = [json.loads(line) for line in chatlib.OUTBOX.read_text().splitlines()]
        self.assertEqual([e.get("ack") for e in entries], ["m1"])
        history = [json.loads(x) for x in chatlib.channel_log("#codekarl").read_text().splitlines()]
        self.assertEqual([e["from"] for e in history if e["from"] == "ck-worker-1"], [])
        self.assertFalse((chatlib.STATE_DIR / "acks.jsonl").exists())
        with mock.patch.object(self.cli, "pchat") as fresh_call:
            self.assertEqual(self.run_cli("effects", "m1", "--retry")[0], 0)
            fresh_call.assert_not_called()
            claimed = chatlib.outbox_claim()
            self.assertEqual(len(claimed), 1)
            digest = hashlib.sha256(claimed[0].read_bytes()).hexdigest()
            self.assertEqual(self.run_cli("effects", "m1", "--retry")[0], 0)
            fresh_call.assert_not_called()
        self.assertEqual(hashlib.sha256(claimed[0].read_bytes()).hexdigest(), digest)
        self.assertEqual(self.states(), {"reaction": "attempting"})

    def test_10b_delayed_observations_never_change_a_state(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        self.post("vince", "@ck-worker-1 inspect", "m2")
        inbox = self.Inbox(STATE)
        for mid in ("m1", "m2"):
            inbox.claim(mid, "ck-worker-1")
            for effect in ("reaction", "visible"):
                self.set_effect(effect, "uncertain", mid)
        # m1's acknowledgements later appear; m2's never do.
        log = chatlib.channel_log("#codekarl")
        log.parent.mkdir(parents=True, exist_ok=True)
        log.write_text(
            json.dumps(
                {"channel": "#codekarl", "from": "ck-worker-1", "text": VISIBLE, "reply_to": "m1"}
            )
            + "\n"
        )
        (chatlib.STATE_DIR / "acks.jsonl").write_text(
            json.dumps({"msgid": "m1", "account": "ck-worker-1"}) + "\n"
        )
        for mid in ("m1", "m2"):
            with self.subTest(mid=mid):
                code, out, _ = self.run_cli("effects", mid)
                self.assertEqual(code, 0)
                shown = json.loads(out)["effects"]
                self.assertEqual(shown["reaction"]["state"], "uncertain")
                self.assertIn("notice", shown["reaction"])
                self.assertIn("inspect", shown["reaction"])
                self.assertEqual(self.run_cli("effects", mid, "--retry")[0], 0)
                self.assertEqual(
                    self.states(mid), {"reaction": "uncertain", "visible": "uncertain"}
                )
        self.assertEqual(self.calls, [])

    def test_10b_an_active_attempt_is_in_progress_and_not_retried(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        holder = self.Inbox(STATE)
        holder.claim("m1", "ck-worker-1")
        token = holder.acquire("m1", "ck-worker-1", "reaction", f"pid={os.getpid()}")
        self.assertTrue(token)
        assert token is not None
        code, out, _ = self.run_cli("effects", "m1")
        self.assertEqual(code, 0)
        reaction = json.loads(out)["effects"]["reaction"]
        self.assertEqual((reaction["state"], reaction["alive"]), ("attempting", True))
        self.assertEqual(self.run_cli("effects", "m1", "--retry")[0], 6)
        self.assertEqual(self.calls, [self.visible_call()])
        self.assertEqual(self.states(), {"reaction": "attempting", "visible": "sent"})
        self.assertTrue(
            holder.record_effect("m1", "ck-worker-1", "reaction", token, "sent", "pchat 0")
        )

    def test_10b_a_duplicate_claim_with_incomplete_effects_hints(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        self.codes.update(ack=3, post=2)
        self.assertEqual(self.run_cli("claim", "m1")[0], 6)
        self.calls.clear()
        code, out, err = self.run_cli("claim", "m1")
        self.assertEqual((code, self.calls), (4, []))
        self.assertIn("ck-chat effects m1", err)
        self.assertEqual(json.loads(out)["effects"], {"reaction": "queued", "visible": "not-sent"})

    def test_10c_claim_and_effect_rows_commit_together(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        with (
            mock.patch.object(self.Inbox, "_open_effects", side_effect=RuntimeError("forced")),
            self.assertRaises(RuntimeError),
        ):
            self.Inbox(STATE).claim("m1", "ck-worker-1")
        self.assertEqual(self.task_state(), "queued")
        self.assertEqual(self.states(), {})
        self.Inbox(STATE).claim("m1", "ck-worker-1")
        self.assertEqual(self.states(), {"reaction": "pending", "visible": "pending"})
        # A duplicate claim inserts nothing more.
        self.assertFalse(self.Inbox(STATE).claim("m1", "ck-worker-1")["fresh"])
        self.assertEqual(len(self.Inbox(STATE).effects("m1", "ck-worker-1")), 2)
        # A claim made before this change has no rows: not recorded, never inferred.
        self.post("vince", "@ck-worker-1 inspect", "old")
        with self.Inbox(STATE).connect() as db:
            self.Inbox._advance(db, "old", "ck-worker-1", "acknowledged", "legacy claim")
        code, out, _ = self.run_cli("effects", "old")
        self.assertEqual(code, 0)
        shown = json.loads(out)["effects"]
        self.assertEqual(
            {k: v["state"] for k, v in shown.items()},
            {"reaction": "not recorded", "visible": "not recorded"},
        )
        self.calls.clear()
        self.assertEqual(self.run_cli("effects", "old", "--retry")[0], 0)
        self.assertEqual(self.calls, [])
        self.assertEqual(self.states("old"), {})

    def test_10d_classification_probes_with_the_real_pchat(self) -> None:
        import runstore  # noqa: PLC0415  # module is on sys.path only after the test bridge has loaded

        self.use_real_pchat()
        reaction, post = ["ack", "#codekarl", "m1"], ["post", "#codekarl", "--reply-to", "m1", "hi"]
        runtime = RuntimeError("boom")

        def case(
            arguments: list[str], state: str, current: mock.MagicMock, **network: object
        ) -> None:
            with contextlib.ExitStack() as stack:
                stack.enter_context(mock.patch.object(identities, "current", current))
                for name, value in network.items():
                    stack.enter_context(mock.patch.object(chatlib, name, value))
                before = (chatlib.login, chatlib.outbox_append)
                result, _ = self.cli.classified_call("ck-worker-1", arguments)
                self.assertEqual((chatlib.login, chatlib.outbox_append), before)
            self.assertEqual(result, state)

        worker = mock.MagicMock(return_value=dict(WORKER))
        with self.subTest("who() error"):
            case(reaction, "not-sent", mock.MagicMock(side_effect=chatlib.ChatError("no identity")))
        with (
            self.subTest("silent refusal"),
            mock.patch.object(runstore, "silent_refusal", return_value="silent run"),
        ):
            case(post, "not-sent", worker)
        with self.subTest("server refusal after login was entered"):
            case(post, "uncertain", worker, _round=mock.Mock(return_value={"FAIL"}))
        with self.subTest("outbox_append entered, then raising"):
            case(
                reaction,
                "uncertain",
                worker,
                login=mock.Mock(side_effect=chatlib.ChatError("down")),
                outbox_append=mock.Mock(side_effect=OSError("disk full")),
            )
        with self.subTest("exception after login was entered"):
            case(post, "uncertain", worker, _round=mock.Mock(side_effect=runtime))
        with self.subTest("exception before either probe"):
            case(reaction, "not-sent", mock.MagicMock(side_effect=runtime))
        with self.subTest("sent"):
            case(post, "sent", worker)
        with self.subTest("queued"):
            case(reaction, "queued", worker, login=mock.Mock(side_effect=chatlib.ChatError("down")))
        self.assertEqual(len(chatlib.OUTBOX.read_text().splitlines()), 1)

    def test_10e_overlapping_commands_acquire_once(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        first, second = self.Inbox(STATE), self.Inbox(STATE)
        first.claim("m1", "ck-worker-1")
        grants: list[str | None] = [None, None]
        barrier = threading.Barrier(2)

        def race(slot: int, inbox: Inbox) -> None:
            barrier.wait()
            grants[slot] = inbox.acquire("m1", "ck-worker-1", "reaction", f"pid={slot}")

        threads = [
            threading.Thread(target=race, args=(0, first)),
            threading.Thread(target=race, args=(1, second)),
        ]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        self.assertEqual(sum(1 for token in grants if token), 1)
        winner = grants[0] or grants[1]
        assert winner is not None
        loser = first if grants[0] is None else second
        self.assertIsNone(loser.acquire("m1", "ck-worker-1", "reaction", "pid=3"))
        # A stale result write with another attempt's token changes nothing.
        self.assertFalse(
            loser.record_effect("m1", "ck-worker-1", "reaction", "stale", "sent", "stale")
        )
        self.assertEqual(self.states(), {"reaction": "attempting", "visible": "pending"})
        self.assertTrue(
            first.record_effect("m1", "ck-worker-1", "reaction", winner, "not-sent", "pchat 2")
            or second.record_effect("m1", "ck-worker-1", "reaction", winner, "not-sent", "pchat 2")
        )
        # not-sent is eligible again, but a recorded result can't be written twice.
        self.assertFalse(
            first.record_effect("m1", "ck-worker-1", "reaction", winner, "sent", "late")
        )

    def test_10e_a_claim_and_a_retry_never_both_send(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        nested: list[tuple[int, str, str]] = []

        def retry_during_the_reaction(arguments: Sequence[str]) -> None:
            if arguments[0] == "ack" and not nested:
                nested.append(self.run_cli("effects", "m1", "--retry"))

        self.hook = retry_during_the_reaction
        code, out, _ = self.run_cli("claim", "m1")
        self.assertEqual(sorted(c[0] for c in self.calls), ["ack", "post"])
        self.assertEqual(len(self.calls), 2)
        self.assertEqual(nested[0][0], 6)  # the claim's own reaction is still attempting
        self.assertEqual(
            (code, json.loads(out)["effects"]), (0, {"reaction": "sent", "visible": "sent"})
        )

    def test_11_the_visible_line_routes_nowhere(self) -> None:
        self.post("vince", "@ck-worker-1 inspect")
        entry = {
            "msgid": "v1",
            "channel": "#codekarl",
            "from": "ck-worker-1",
            "verified": True,
            "text": VISIBLE,
            "reply_to": "m1",
            "at": "2026-10-15T10:00:00Z",
        }
        parent = {"from": "ck-manager", "text": "x"}
        routing = codekarl_chat.plan(entry, self.cfg, self.agents, parent)
        self.assertEqual(routing["recipients"], [])
        self.post("ck-worker-1", VISIBLE, "v2", reply="m1")
        rows = self.Inbox(STATE).rows(history=True)
        self.assertEqual([r["message"] for r in rows], ["m1"])


MANAGER = {"name": "ck-manager", "project": "codekarl", "status": "active"}
REASON = "pause for the live check"


class DeliveryCommand(CodekarlFixture):
    """CK-10: `ck-chat delivery status|on|off` (PF-11), on a fixture chat config."""

    def setUp(self) -> None:
        super().setUp()
        environment = mock.patch.dict(os.environ)
        environment.start()
        self.addCleanup(environment.stop)
        for name in list(os.environ):
            if name in {"CHAT_AS", "CHAT_AGENT_ID", "CHAT_ROLE"} or name.startswith("CMUX_"):
                del os.environ[name]
        self.cli = ck_chat
        self.config = copy.deepcopy(self.cfg)
        table(self.config["accounts"]).update({"ck-manager": "x", "vince": "x"})
        self.config["agent_identities_enabled"] = True
        table(table(self.config["projects"])["mosk"])["note"] = "another project's key"
        self.config["unrelated"] = {"keep": [1, 2, 3]}
        self.write(self.config)
        self.calls: list[list[str]] = []
        self.code = 0
        self.hook: Callable[[Sequence[str]], object] | None = None
        self.current = mock.patch.object(identities, "current", return_value=dict(MANAGER)).start()
        mock.patch.object(self.cli, "pchat", side_effect=self.fake_pchat).start()
        self.addCleanup(mock.patch.stopall)

    # --- helpers ---------------------------------------------------------------------------

    def write(self, config: dict[str, object]) -> None:
        identities.atomic_json(chatlib.CONFIG_PATH, config)

    def read(self) -> dict[str, object]:
        return table(json.loads(chatlib.CONFIG_PATH.read_text()))

    def value(self) -> object:
        return codekarl_project(self.read())["delivery_enabled"]

    def set_value(self, value: bool) -> None:
        """A second writer: a locked atomic replacement, as every real writer does."""
        with identities.locked():
            config = self.read()
            codekarl_project(config)["delivery_enabled"] = value
            self.write(config)

    def fake_pchat(self, arguments: Sequence[str]) -> int:
        self.calls.append(list(arguments))
        if self.hook:
            self.hook(arguments)
        return self.code

    def run_cli(self, *args: str) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        with (
            mock.patch.object(sys, "argv", ["ck-chat", *args]),
            redirect_stdout(out),
            redirect_stderr(err),
        ):
            code = self.cli.main()
        return code, out.getvalue(), err.getvalue()

    def snapshot(self) -> dict[str, bytes]:
        """Every file in the chat state directory, byte for byte."""
        return {
            str(path.relative_to(STATE)): path.read_bytes()
            for path in sorted(Path(STATE).rglob("*"))
            if path.is_file()
        }

    def expected(self, word: str, label: str = "ck-manager", reason: str = REASON) -> list[str]:
        return ["post", "#codekarl", f"[status] delivery {word} by {label}: {reason}"]

    def without_delivery(self, config: dict[str, object]) -> dict[str, object]:
        rest = copy.deepcopy(config)
        del codekarl_project(rest)["delivery_enabled"]
        return rest

    # --- tests -----------------------------------------------------------------------------

    def test_01_off_by_the_manager_changes_only_that_key_and_posts_one_quiet_record(self) -> None:
        before = self.read()
        code, out, _ = self.run_cli("delivery", "off", "--reason", REASON)
        self.assertEqual(code, 0)
        after = self.read()
        self.assertFalse(codekarl_project(after)["delivery_enabled"])
        self.assertEqual(self.without_delivery(after), self.without_delivery(before))
        self.assertEqual(table(after["projects"])["mosk"], table(before["projects"])["mosk"])
        self.assertEqual(oct(Path(chatlib.CONFIG_PATH).stat().st_mode & 0o777), "0o600")
        self.assertEqual(self.calls, [self.expected("off")])
        self.assertFalse(json.loads(out)["configured"])
        entry = {
            "msgid": "r1",
            "channel": "#codekarl",
            "from": "ck-manager",
            "verified": True,
            "text": self.calls[0][2],
            "at": "2026-10-16T10:00:00Z",
        }
        routing = codekarl_chat.plan(entry, self.cfg, self.agents)
        self.assertEqual((routing["recipients"], routing["actionable"]), ([], False))

    def test_02_on_when_already_on_writes_and_posts_nothing(self) -> None:
        before = chatlib.CONFIG_PATH.read_bytes(), Path(chatlib.CONFIG_PATH).stat().st_mtime_ns
        code, out, _ = self.run_cli("delivery", "on", "--reason", REASON)
        self.assertEqual(code, 0)
        self.assertTrue(json.loads(out)["configured"])
        self.assertEqual(self.calls, [])
        self.assertEqual(
            (chatlib.CONFIG_PATH.read_bytes(), Path(chatlib.CONFIG_PATH).stat().st_mtime_ns), before
        )

    def test_03_a_missing_empty_multiline_long_or_addressing_reason_is_refused(self) -> None:
        before = self.snapshot(), chatlib.CONFIG_PATH.read_bytes()
        for label, reason in (
            ("empty", ""),
            ("blank", "   "),
            ("multi-line", "first\nsecond"),
            ("carriage return", "first\rsecond"),
            ("too long", "x" * 201),
            ("handle", "ask @ck-worker-1"),
        ):
            with self.subTest(label), self.assertRaises(ValueError):
                self.run_cli("delivery", "off", "--reason", reason)
        with self.assertRaises(ValueError):
            self.run_cli("delivery", "off")
        self.assertEqual((self.snapshot(), chatlib.CONFIG_PATH.read_bytes()), before)
        self.assertEqual(self.calls, [])
        # The boundary is accepted.
        self.assertEqual(self.run_cli("delivery", "off", "--reason", "x" * 200)[0], 0)

    def test_04_a_bound_worker_or_reviewer_is_refused_before_any_write(self) -> None:
        before = chatlib.CONFIG_PATH.read_bytes()
        for actor in (
            {"name": "ck-worker-1", "project": "codekarl", "status": "active"},
            {"name": "ck-reviewer-1", "project": "codekarl", "status": "active"},
            {"name": "ck-manager", "project": "codekarl", "status": "retired"},
            {"name": "other-manager", "project": "mosk", "status": "active"},
        ):
            with self.subTest(actor=actor["name"]):
                self.current.return_value = actor
                with self.assertRaises(ValueError):
                    self.run_cli("delivery", "off", "--reason", REASON)
        self.assertEqual(chatlib.CONFIG_PATH.read_bytes(), before)
        self.assertEqual(self.calls, [])

    def test_05_an_owner_shell_is_allowed_and_recorded_as_the_owner(self) -> None:
        self.current.return_value = None
        os.environ["CHAT_AS"] = "vince"
        self.assertEqual(self.run_cli("delivery", "off", "--reason", REASON)[0], 0)
        self.assertFalse(self.value())
        self.assertEqual(self.calls, [self.expected("off", "vince")])
        before = chatlib.CONFIG_PATH.read_bytes()
        for account in ("laz", "mallory", None):  # assistants and strangers are not owners
            with self.subTest(account=account):
                if account:
                    os.environ["CHAT_AS"] = account
                else:
                    os.environ.pop("CHAT_AS")
                with self.assertRaises(ValueError):
                    self.run_cli("delivery", "on", "--reason", REASON)
        self.assertEqual(chatlib.CONFIG_PATH.read_bytes(), before)

    def test_06_a_queued_record_keeps_the_change_and_an_uncertain_one_says_so(self) -> None:
        self.code = 3
        code, _, _ = self.run_cli("delivery", "off", "--reason", REASON)
        self.assertEqual(code, 3)
        self.assertFalse(self.value())
        self.set_value(True)

        def reach_the_network(arguments: Sequence[str]) -> None:
            with contextlib.suppress(chatlib.ChatError):
                chatlib.login("ck-manager")

        self.code, self.hook = 2, reach_the_network
        with mock.patch.object(chatlib, "login", side_effect=chatlib.ChatError("down")):
            code, _, err = self.run_cli("delivery", "off", "--reason", REASON)
        self.assertEqual(code, 6)
        self.assertIn("record fate unknown; check the room", err)
        self.assertFalse(self.value())  # the change is kept

    def test_07_queued_work_is_preserved(self) -> None:
        state = Path(STATE)
        (state / "outbox.jsonl").write_text('{"channel": "#codekarl", "text": "ordinary"}\n')
        (state / "outbox.claimed-1.jsonl").write_text(
            '{"channel": "#codekarl", "text": "claimed"}\n'
        )
        (state / "deliveries.jsonl").write_text('{"msgid": "m9", "ok": null}\n')
        (state / "dead-letters.jsonl").write_text('{"msgid": "m8", "detail": "dead"}\n')
        self.post("vince", "@ck-worker-1 an inbox row", "m1")
        before = {k: v for k, v in self.snapshot().items() if k != "identities.lock"}
        self.assertEqual(self.run_cli("delivery", "off", "--reason", REASON)[0], 0)
        self.assertEqual(self.run_cli("delivery", "on", "--reason", REASON)[0], 0)
        after = {k: v for k, v in self.snapshot().items() if k != "identities.lock"}
        self.assertEqual(after, before)

    def test_07a_a_queued_record_appends_exactly_one_outbox_entry(self) -> None:
        state = Path(STATE)
        existing = '{"channel": "#codekarl", "text": "already queued"}\n'
        (state / "outbox.jsonl").write_text(existing)
        claimed = '{"channel": "#codekarl", "text": "claimed"}\n'
        (state / "outbox.claimed-1.jsonl").write_text(claimed)
        self.use_real_pchat()
        with mock.patch.object(chatlib, "login", side_effect=chatlib.ChatError("irc is down")):
            code, _, _ = self.run_cli("delivery", "off", "--reason", REASON)
        self.assertEqual(code, 3)
        lines = (state / "outbox.jsonl").read_text().splitlines()
        self.assertEqual(lines[0], existing.strip())
        self.assertEqual(len(lines), 2)
        self.assertIn("delivery off by ck-manager", json.loads(lines[1])["text"])
        self.assertEqual((state / "outbox.claimed-1.jsonl").read_text(), claimed)
        self.assertFalse(self.value())

    def use_real_pchat(self) -> None:
        path = Path(chatlib.__file__).resolve().parent / "pchat"

        def staged_pchat(arguments: Sequence[str]) -> int:
            loader = importlib.machinery.SourceFileLoader("ck_pchat_staged", str(path))
            spec = importlib.util.spec_from_loader("ck_pchat_staged", loader)
            assert spec is not None
            module = importlib.util.module_from_spec(spec)
            loader.exec_module(module)
            code: object = module.main(arguments) or 0
            assert isinstance(code, int)
            return code

        import runstore  # noqa: PLC0415  # module is on sys.path only after the test bridge has loaded

        mock.patch.object(self.cli, "pchat", staged_pchat).start()
        mock.patch.object(runstore, "silent_refusal", return_value=None).start()

    def test_08_status_reports_the_configured_value_and_writes_nothing(self) -> None:
        before = self.snapshot(), Path(chatlib.CONFIG_PATH).stat().st_mtime_ns
        code, out, err = self.run_cli("delivery", "status")
        self.assertEqual(code, 0)
        report = json.loads(out)
        self.assertEqual(report["bridge_effective"], "unknown")
        self.assertTrue(report["configured"])
        self.assertRegex(report["config_mtime"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d$")
        self.assertIn("does not report it", err)
        self.assertEqual((self.snapshot(), Path(chatlib.CONFIG_PATH).stat().st_mtime_ns), before)
        self.assertEqual(self.calls, [])
        # Anyone may ask, including a bound worker.
        self.current.return_value = {
            "name": "ck-worker-1",
            "project": "codekarl",
            "status": "active",
        }
        self.assertEqual(self.run_cli("delivery", "status")[0], 0)

    def test_09_reasons_that_address_anyone_are_refused_before_any_write(self) -> None:
        before = chatlib.CONFIG_PATH.read_bytes()
        for reason in (
            "@ck-worker-1 stop",
            "tell @CK-Manager",
            "see @vince",
            "ping @laz now",
            "cc @nobody-known",
            "mixed @Ck-Reviewer-1",
            "email a@b",
        ):
            with self.subTest(reason), self.assertRaises(ValueError):
                self.run_cli("delivery", "off", "--reason", reason)
        self.assertEqual(chatlib.CONFIG_PATH.read_bytes(), before)
        self.assertEqual(self.calls, [])

    def test_11_the_real_lock_is_not_held_during_the_post(self) -> None:
        self.use_real_pchat()
        state: dict[str, bool | None] = {"free_during_post": None}

        def login(account: str, *args: object, **kwargs: object) -> None:
            with Path(STATE).joinpath("identities.lock").open("a") as lock:
                try:
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    state["free_during_post"] = True
                except BlockingIOError:
                    state["free_during_post"] = False
            raise chatlib.ChatError("irc is down")

        result: dict[str, int] = {}

        def command() -> None:
            with mock.patch.object(chatlib, "login", login):
                result["code"] = self.run_cli("delivery", "off", "--reason", REASON)[0]

        worker = threading.Thread(target=command, daemon=True)
        worker.start()
        worker.join(30)
        self.assertFalse(worker.is_alive(), "the command deadlocked on the identity lock")
        self.assertTrue(state["free_during_post"])
        self.assertEqual(result["code"], 3)
        self.assertFalse(self.value())

    def test_12_a_newer_writer_during_the_post_blocks_the_rollback(self) -> None:
        for label, change in (
            ("delivery value", lambda: self.set_value(True)),
            (
                "unrelated key",
                lambda: self.mutate({"unrelated": {"keep": [1, 2, 3, 4]}}),
            ),
        ):
            with self.subTest(label):
                self.set_value(True)
                self.calls.clear()
                self.code = 2
                self.hook = lambda arguments, change=change: change()
                code, _, err = self.run_cli("delivery", "off", "--reason", REASON)
                self.assertEqual(code, 6)
                self.assertIn("the config changed since", err)
                self.assertIn("not rolled back", err)
                config = self.read()
                if label == "delivery value":
                    self.assertTrue(codekarl_project(config)["delivery_enabled"])
                else:
                    self.assertFalse(codekarl_project(config)["delivery_enabled"])
                    self.assertEqual(config["unrelated"], {"keep": [1, 2, 3, 4]})

    def mutate(self, extra: dict[str, object]) -> None:
        with identities.locked():
            config = self.read()
            config.update(extra)
            self.write(config)

    def test_12a_an_off_on_off_sequence_is_detected_by_the_file_identity(self) -> None:
        def b_then_c(arguments: Sequence[str]) -> None:
            self.set_value(True)
            self.set_value(False)  # the content equals A's own write again

        self.code = 2
        self.hook = b_then_c
        code, _, err = self.run_cli("delivery", "off", "--reason", REASON)
        self.assertEqual(code, 6)
        self.assertIn("the config changed since", err)
        self.assertFalse(self.value())  # C's off stands; A made no rollback write

    def test_13_a_record_that_was_not_sent_rolls_the_change_back(self) -> None:
        before = self.read()
        self.code = 2
        with self.assertRaisesRegex(ValueError, "delivery NOT changed: record could not be posted"):
            self.run_cli("delivery", "off", "--reason", REASON)
        self.assertEqual(self.read(), before)

    def test_14_failures_in_the_real_atomic_json(self) -> None:
        fsync = os.fsync
        # (a) an error before the replace (the temp-file fsync): unchanged, no post.
        before = chatlib.CONFIG_PATH.read_bytes()
        with (
            mock.patch("os.fsync", side_effect=OSError("disk")),
            self.assertRaisesRegex(ValueError, "delivery NOT changed"),
        ):
            self.run_cli("delivery", "off", "--reason", REASON)
        self.assertEqual((chatlib.CONFIG_PATH.read_bytes(), self.calls), (before, []))
        # (b) an error after the replace (the directory fsync): the re-read finds the change and
        # the command continues to post its record.
        count = {"n": 0}

        def second_fails(descriptor: int) -> None:
            count["n"] += 1
            if count["n"] == 2:  # noqa: PLR2004  # second call of the stubbed login
                raise OSError("directory fsync")
            return fsync(descriptor)

        with mock.patch("os.fsync", second_fails):
            code, _, _ = self.run_cli("delivery", "off", "--reason", REASON)
        self.assertEqual(code, 0)
        self.assertFalse(self.value())
        self.assertEqual(self.calls, [self.expected("off")])
        # (c) an unreadable config after a write error: exit 6, no value printed.
        self.set_value(True)
        self.calls.clear()
        real_load = chatlib.load_config
        loads = {"n": 0}

        def third_fails(*args: object) -> dict[str, object]:
            loads["n"] += 1
            if loads["n"] >= 3:  # noqa: PLR2004  # third load of the stubbed config
                raise chatlib.ChatError("config unreadable")
            return real_load()

        with (
            mock.patch("os.fsync", side_effect=OSError("disk")),
            mock.patch.object(chatlib, "load_config", third_fails),
        ):
            code, out, err = self.run_cli("delivery", "off", "--reason", REASON)
        self.assertEqual(code, 6)
        self.assertIn("delivery state unknown: config unreadable after a write error", err)
        self.assertNotRegex(err + out, r"\b(True|False|true|false)\b")
        self.assertEqual(self.calls, [])
        # (d) a forced re-read mismatch after a successful write: exit 6, no post.
        self.set_value(True)
        self.calls.clear()
        loads["n"] = 0

        def mismatch(*args: object) -> dict[str, object]:
            loads["n"] += 1
            config = real_load()
            if loads["n"] >= 3:  # noqa: PLR2004  # third load of the stubbed config
                codekarl_project(config)["delivery_enabled"] = True
            return config

        with mock.patch.object(chatlib, "load_config", mismatch):
            code, _, err = self.run_cli("delivery", "off", "--reason", REASON)
        self.assertEqual(code, 6)
        self.assertIn("delivery state uncertain: config verification failed", err)
        self.assertEqual(self.calls, [])

    def test_14a_rollback_write_failures_are_never_reported_as_clean_unless_proven(self) -> None:
        fsync = os.fsync
        for label, failing_call, expected in (
            ("before its replace", 3, 6),  # the rollback's temp-file fsync
            ("after its replace", 4, 2),  # the rollback's directory fsync: proven by the re-read
        ):
            with self.subTest(label):
                self.set_value(True)
                self.calls.clear()
                count = {"n": 0}

                def failing(
                    descriptor: int, failing_call: int = failing_call, count: dict[str, int] = count
                ) -> None:
                    count["n"] += 1
                    if count["n"] == failing_call:
                        raise OSError("fsync")
                    return fsync(descriptor)

                self.code = 2  # the record is not sent, so a rollback is attempted
                with mock.patch("os.fsync", failing):
                    if expected == 6:  # noqa: PLR2004  # exit code 6 is 'ack effect incomplete'
                        code, _, err = self.run_cli("delivery", "off", "--reason", REASON)
                        self.assertEqual(code, 6, err)
                        self.assertIn("rollback failed", err)
                        self.assertFalse(self.value())  # the change was not undone
                    else:
                        with self.assertRaisesRegex(ValueError, "delivery NOT changed"):
                            self.run_cli("delivery", "off", "--reason", REASON)
                        self.assertTrue(self.value())  # proven undone by the re-read


if __name__ == "__main__":
    unittest.main()
