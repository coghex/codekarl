"""Codekarl chat routing and recovery contracts, with no live IRC or cmux."""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools/chat"))
from codekarl_chat import Inbox, plan
from codekarl_chat import Routing as RoutingRecord


def table(value: object) -> dict[str, object]:
    """A JSON object of the fixture configuration (plain parsed data)."""
    assert isinstance(value, dict)
    return value


def config() -> dict[str, object]:
    return {
        "owner": "vince",
        "assistants": ["laz", "codex", "claude"],
        "projects": {
            "codekarl": {
                "channel": "#codekarl",
                "prefix": "#ck-",
                "manager": "ck-manager",
                "routing": "codekarl-v1",
                "owners": ["vince", "coghex"],
                "assistants": ["laz"],
                "delivery_enabled": True,
            }
        },
        "identities": {
            name: {"project": "codekarl", "role": role}
            for name, role in [
                ("ck-manager", "manager"),
                ("ck-worker-7", "worker"),
                ("ck-worker-8", "worker"),
                ("ck-reviewer-4", "reviewer"),
            ]
        },
    }


def message(
    sender: str = "vince", text: str = "@ck-worker-7 do the task", mid: str = "m1", **extra: object
) -> dict[str, object]:
    return (
        dict(
            msgid=mid,
            channel="#codekarl",
            **{"from": sender},
            nick=sender,
            verified=True,
            text=text,
            at="2026-10-14T10:00:00Z",
        )
        | extra
    )


class Routing(unittest.TestCase):
    def setUp(self) -> None:
        self.cfg = config()
        self.agents = {name: {"status": "active"} for name in table(self.cfg["identities"])}

    def route(
        self, sender: str = "vince", text: str = "@ck-worker-7 do the task", **extra: object
    ) -> RoutingRecord:
        return plan({**message(sender, text), **extra}, self.cfg, self.agents)

    def test_both_owner_accounts_and_laz_reach_worker(self) -> None:
        for sender in ("vince", "coghex", "laz"):
            with self.subTest(sender=sender):
                result = self.route(sender)
                self.assertEqual(result["recipients"], ["ck-worker-7"])
                self.assertEqual(result["owner_direct"], sender != "laz")

    def test_phone_nickname_does_not_change_authenticated_actor(self) -> None:
        self.assertEqual(self.route("vince", nick="coghex")["role"], "owner")
        self.assertEqual(self.route("outsider", nick="vince")["recipients"], [])

    def test_unverified_or_missing_message_id_cannot_dispatch(self) -> None:
        for extra in ({"verified": False}, {"msgid": None}):
            self.assertFalse(self.route(**extra)["recipients"])

    def test_legacy_global_assistants_have_no_codekarl_authority(self) -> None:
        for sender in ("codex", "claude"):
            self.assertFalse(self.route(sender)["recipients"])

    def test_manager_dispatch_and_worker_peer_question(self) -> None:
        self.assertEqual(self.route("ck-manager")["recipients"], ["ck-worker-7"])
        result = self.route("ck-worker-7", "[question T1] @ck-reviewer-4 which case failed?")
        self.assertEqual(result["recipients"], ["ck-reviewer-4"])
        self.assertEqual(result["role"], "worker")

    def test_progress_and_ack_do_not_wake_manager(self) -> None:
        for text in ("[status T1] compiling", "[ack] @ck-manager received"):
            self.assertFalse(self.route("ck-worker-7", text)["recipients"])

    def test_actionable_milestones_reach_manager(self) -> None:
        for kind in ("question", "blocked", "done"):
            self.assertEqual(
                self.route("ck-worker-7", f"[{kind} T1] details")["recipients"], ["ck-manager"]
            )

    def test_unknown_target_does_not_turn_into_manager_assignment(self) -> None:
        result = self.route(text="@syn-solver-1 change something")
        self.assertFalse(result["recipients"])
        self.assertIn("out-of-project", result["reason"])

    def test_interrupt_requires_authorized_actor(self) -> None:
        for sender in ("vince", "coghex", "laz", "ck-manager"):
            self.assertTrue(self.route(sender, "!! @ck-worker-7 hold")["interrupt"])
        self.assertFalse(self.route("ck-reviewer-4", "!! @ck-worker-7 hold")["recipients"])

    def test_retired_sender_cannot_dispatch(self) -> None:
        self.agents["ck-manager"]["status"] = "retired"
        self.assertFalse(self.route("ck-manager")["recipients"])

    def test_offline_recipient_stays_addressed_to_original_identity(self) -> None:
        self.agents["ck-worker-7"]["status"] = "retired"
        self.assertEqual(self.route()["recipients"], ["ck-worker-7"])

    def test_owner_notification_only_for_actionable_questions(self) -> None:
        self.assertTrue(self.route("ck-worker-7", "[question] @coghex choose?")["notify_owner"])
        self.assertFalse(self.route("ck-worker-7", "[status] @vince finished")["notify_owner"])


class Recovery(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.cfg = config()
        self.agents = {name: {"status": "active"} for name in table(self.cfg["identities"])}
        self.inbox = Inbox(self.temp.name)

    def ingest(self, entry: dict[str, object]) -> RoutingRecord:
        return self.inbox.ingest(entry, self.cfg, self.agents)

    def test_record_survives_restart_before_notification_is_enqueued(self) -> None:
        self.ingest(message())
        reopened = Inbox(self.temp.name)
        self.assertEqual(reopened.notices(self.cfg)[0]["target"], "ck-worker-7")

    def test_replay_and_duplicate_claim_do_not_create_new_assignment(self) -> None:
        self.ingest(message())
        self.assertTrue(self.inbox.claim("m1", "ck-worker-7")["fresh"])
        self.ingest(message(replayed=True))
        self.assertFalse(self.inbox.claim("m1", "ck-worker-7")["fresh"])
        self.assertEqual(self.inbox.notices(self.cfg), [])
        self.assertEqual(len(self.inbox.rows()), 1)

    def test_only_intended_recipient_can_claim(self) -> None:
        self.ingest(message())
        with self.assertRaisesRegex(ValueError, "not addressed"):
            self.inbox.claim("m1", "ck-reviewer-4")

    def test_multi_recipient_acknowledgements_are_independent(self) -> None:
        self.ingest(message(text="@ck-worker-7 @ck-worker-8 inspect"))
        self.inbox.acknowledge("a1", "m1", "ck-worker-7")
        self.assertEqual([x["target"] for x in self.inbox.notices(self.cfg)], ["ck-worker-8"])

    def test_completion_is_distinct_from_acknowledgement(self) -> None:
        self.ingest(message())
        self.inbox.acknowledge("a1", "m1", "ck-worker-7")
        self.assertEqual(self.inbox.rows()[0]["state"], "acknowledged")
        self.ingest(message("ck-worker-7", "[done] receipt /path", "done1", reply_to="m1"))
        original = next(row for row in self.inbox.rows(history=True) if row["message"] == "m1")
        self.assertEqual(original["state"], "completed")
        self.inbox.acknowledge("a2", "m1", "ck-worker-7")
        self.assertFalse(self.inbox.needs_delivery("m1", "ck-worker-7"))

    def test_wrong_actor_reply_does_not_complete_request(self) -> None:
        self.ingest(message())
        self.ingest(message("ck-reviewer-4", "[done] done", "wrong", reply_to="m1"))
        self.assertTrue(self.inbox.needs_delivery("m1", "ck-worker-7"))

    def test_out_of_order_reply_is_reconciled(self) -> None:
        self.ingest(message("ck-worker-7", "[done] receipt", "done1", reply_to="m1"))
        self.ingest(message())
        original = next(row for row in self.inbox.rows(history=True) if row["message"] == "m1")
        self.assertEqual(original["state"], "completed")

    def test_acknowledgement_does_not_create_response_loop(self) -> None:
        self.ingest(message("ck-manager"))
        result = self.ingest(message("ck-worker-7", "[status] started", "a1", reply_to="m1"))
        self.assertEqual(result["recipients"], [])

    def test_delivery_failure_remains_visible_but_does_not_retry_forever(self) -> None:
        self.ingest(message())
        notice = self.inbox.notices(self.cfg)[0]
        self.inbox.outcome(notice, False, "dead-lettered: no session")
        self.assertEqual(self.inbox.notices(self.cfg), [])
        self.assertEqual(self.inbox.rows()[0]["notice"], "failed")

    def test_activation_hold_records_without_waking(self) -> None:
        table(table(self.cfg["projects"])["codekarl"])["delivery_enabled"] = False
        self.ingest(message())
        self.assertEqual(self.inbox.notices(self.cfg), [])
        table(table(self.cfg["projects"])["codekarl"])["delivery_enabled"] = True
        self.assertEqual(len(self.inbox.notices(self.cfg)), 1)

    def test_delivered_receipt_recovers_ack_wait_after_restart(self) -> None:
        self.ingest(message())
        notice = self.inbox.notices(self.cfg)[0]
        self.inbox.outcome(notice, True, "delivered")
        self.assertEqual(Inbox(self.temp.name).notices(self.cfg)[0]["phase"], "accept")

    def test_diagnostics_preserve_unroutable_message(self) -> None:
        self.ingest(message(text="@unknown please act"))
        self.assertEqual(self.inbox.diagnostics()[0]["payload"]["msgid"], "m1")

    def test_acknowledged_result_leaves_outstanding_inbox(self) -> None:
        self.ingest(message("ck-worker-7", "[done] a task completed"))
        self.inbox.claim("m1", "ck-manager")
        self.assertEqual(self.inbox.rows(), [])
        self.assertEqual(self.inbox.rows(history=True)[0]["state"], "acknowledged")

    def test_answer_finishes_question_but_not_assignment(self) -> None:
        for kind, expected in (("question", "completed"), ("request", "acknowledged")):
            mid = "original-" + kind
            self.ingest(message("ck-manager", f"[{kind}] @ck-worker-7 details?", mid))
            self.ingest(message("ck-worker-7", "[answer] details", "answer-" + kind, reply_to=mid))
            row = next(row for row in self.inbox.rows(history=True) if row["message"] == mid)
            self.assertEqual(row["state"], expected)

    def test_answer_before_question_replay_still_completes_it(self) -> None:
        self.ingest(message("ck-worker-7", "[answer] yes", "answer", reply_to="m1"))
        self.ingest(message("ck-manager", "[question] @ck-worker-7 ready?"))
        row = next(row for row in self.inbox.rows(history=True) if row["message"] == "m1")
        self.assertEqual(row["state"], "completed")


if __name__ == "__main__":
    unittest.main()
