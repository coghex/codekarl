"""S7d: the chat record loaders accept every stored form and reject the rest by name.

The fixtures are sanitized copies of the shapes in the live inbox (the report adds a read-only run
over the real files). A loader returns the parsed object itself, so unknown additive fields survive,
and a value of the wrong type at any depth is refused with an error that names the next action or
is accepted as the type the loader declares."""

from __future__ import annotations

import copy
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import types
import typing
import unittest
from collections.abc import Callable, Iterable, Mapping
from pathlib import Path
from typing import ClassVar, cast

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools/chat"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import codekarl_chat
import inputs
import install
from codekarl_chat import Inbox
from gate import records
from gate.records import JsonObject
from shapes import conforms, overlay, sample
from test_records import DELETE, JUNK, keys_of, replaced, required_keys

TESTS = Path(__file__).resolve().parent
ROOT = TESTS.parent
FIXTURES = TESTS / "fixtures/chat"

# ck_chat and identity_conflicts import the shared chatlib and identities when they are loaded. These
# stand-ins last for those imports only, so the suite does not depend on the machine's chat skill.
CHAT_ERROR = type("ChatError", (RuntimeError,), {})
CHATLIB = types.ModuleType("chatlib")
CHATLIB.__dict__.update(ChatError=CHAT_ERROR, STATE_DIR=Path("/nonexistent"))
IDENTITIES = types.ModuleType("identities")
_saved = {name: sys.modules.get(name) for name in ("chatlib", "identities")}
sys.modules.update(chatlib=CHATLIB, identities=IDENTITIES)
import ck_chat  # noqa: E402  # needs the stand-ins above
import identity_conflicts  # noqa: E402  # needs the stand-ins above

for _name, _module in _saved.items():
    if _module is None:
        del sys.modules[_name]
    else:
        sys.modules[_name] = _module
for _name in ("ck_chat", "identity_conflicts"):
    del sys.modules[_name]  # bound to the stand-ins: no other test module should find them

Loader = Callable[[object, str], object]
Failure = type[Exception]
Subject = tuple[str, Loader, object, tuple[Failure, ...]]
CHAT_ERRORS: tuple[Failure, ...] = (codekarl_chat.RecordError,)
NEXT_ACTIONS = ("inspect", "correct", "restage")


def state_dir(path: Path) -> None:
    CHATLIB.__dict__["STATE_DIR"] = path


def fixture() -> dict[str, dict[str, JsonObject]]:
    raw = json.loads((FIXTURES / "records.json").read_text())
    return {
        "entries": raw["entries"],
        "routings": raw["routings"],
        "settings": {"settings": raw["settings"]},
    }


def stage_manifest(*, shared: bool = True) -> JsonObject:
    value: JsonObject = {
        "baseline": {"chat-bridge": "a" * 64, "tests/test_codekarl.py": None},
        "installed": {"chat-bridge": "b" * 64, "tests/test_codekarl.py": "c" * 64},
        "source": {"tools/chat/adapter.py": "d" * 64, "tools/ck-chat": "e" * 64},
    }
    if shared:
        value["shared"] = {"files": {"chat/SKILL.md": "f" * 64}, "aggregate": "1" * 64}
    return value


def conflict_rows() -> list[JsonObject]:
    return [
        {"project": "codekarl", "role": "worker", "surface_id": "SURFACE-1", "incumbent": "ck-x"},
        {"project": None, "role": None, "surface_id": None, "incumbent": None},
    ]


def load_conflicts(value: object, label: str) -> list[identity_conflicts.ConflictRow]:
    return identity_conflicts.load_conflicts(value)


def configuration() -> JsonObject:
    return {
        "owner": "vince",
        "assistants": ["codex"],
        "projects": {"codekarl": fixture()["settings"]["settings"], "mosk": {"channel": "#m"}},
    }


def settings_loader(value: object, label: str) -> codekarl_chat.ProjectSettings | None:
    return codekarl_chat.settings_of(cast("Mapping[str, object]", value))


def chat_inputs_record() -> JsonObject:
    files = {"chat/scripts/a.py": "0" * 64, "project-manager/b": "f" * 64}
    return {"schema": 1, "files": files, "aggregate": inputs.aggregate(files)}


def declared_return(loader: Loader) -> object:
    return typing.get_type_hints(loader)["return"]


def subjects() -> list[Subject]:
    """(label, loader, a valid value, the errors it may raise)."""
    shapes = fixture()
    found: list[Subject] = []
    for name, entry in shapes["entries"].items():
        found.append((f"entry {name}", codekarl_chat.load_entry, entry, CHAT_ERRORS))
    for name, routing in shapes["routings"].items():
        found.append((f"routing {name}", codekarl_chat.load_routing, routing, CHAT_ERRORS))
    found.append(
        (
            "project settings",
            codekarl_chat.load_project_settings,
            shapes["settings"]["settings"],
            CHAT_ERRORS,
        )
    )
    manifest_errors: tuple[Failure, ...] = (records.RecordError,)
    found += [
        ("stage manifest", install.load_stage_manifest, stage_manifest(), manifest_errors),
        (
            "stage manifest (before shared inputs)",
            install.load_stage_manifest,
            stage_manifest(shared=False),
            manifest_errors,
        ),
        ("identity conflicts", load_conflicts, conflict_rows(), (CHAT_ERROR,)),
        ("pchat configuration", settings_loader, configuration(), CHAT_ERRORS),
    ]
    # The same records with every optional field the declared type has, so the fields a fixture
    # happens not to carry are audited too.
    full = [
        (f"{label} (every optional field)", loader, overlay(value, sample(hint)), errors)
        for label, loader, value, errors in found
        if typing.is_typeddict(hint := declared_return(loader))
    ]
    return [*found, *full]


class LoaderAudit(unittest.TestCase):
    def test_01_every_stored_form_loads_and_additive_fields_survive(self) -> None:
        for label, loader, value, _ in subjects():
            with self.subTest(label):
                candidate = copy.deepcopy(value)
                targets = candidate if isinstance(candidate, list) else [candidate]
                for item in targets:
                    assert isinstance(item, dict)
                    item["added_by_a_later_slice"] = {"anything": 1}
                loaded = loader(candidate, label)
                self.assertTrue(conforms(loaded, declared_return(loader)), label)
                if loader is settings_loader:
                    continue  # answers with the policy section, which is not the whole value
                self.assertIs(loaded, candidate)
                for item in targets:
                    assert isinstance(item, dict)
                    self.assertEqual(item["added_by_a_later_slice"], {"anything": 1})

    def test_02_declared_required_keys_are_required_and_named(self) -> None:
        for label, loader, value, errors in subjects():
            hint = declared_return(loader)
            if not typing.is_typeddict(hint) or not isinstance(value, dict):
                continue
            needed = required_keys(hint)
            for key in value:
                with self.subTest(f"{label}: delete {key}"):
                    candidate = replaced(cast("JsonObject", value), (key,), DELETE)
                    if key in needed:
                        with self.assertRaises(errors) as caught:
                            loader(candidate, label)
                        self.assertIn(key, str(caught.exception))
                    else:
                        loader(candidate, label)  # optional by declaration: absence is valid
            for key in needed:
                self.assertIn(key, value, f"{label}: the fixture lacks the required key {key}")

    def test_03_a_value_of_any_wrong_type_at_any_depth_is_named_or_valid(self) -> None:
        failures: list[str] = []
        accepted = rejected = 0
        for label, loader, value, errors in subjects():
            hint = declared_return(loader)
            for path in keys_of(value):
                for junk in (*JUNK, DELETE):
                    candidate = replaced(cast("JsonObject", value), path, junk)
                    try:
                        loaded = loader(candidate, label)
                    except errors as error:
                        rejected += 1
                        if not any(word in str(error) for word in NEXT_ACTIONS):
                            failures.append(f"{label}: {path} -> {junk!r}: no next action: {error}")
                        continue
                    accepted += 1
                    if not conforms(loaded, hint):
                        failures.append(f"{label}: {path} -> {junk!r}: accepted as {loaded!r}")
        self.assertEqual(
            failures[:10], [], f"{len(failures)} nonconforming of {accepted + rejected}"
        )
        self.assertGreater(accepted, 100)
        self.assertGreater(rejected, 1000)

    def test_04_a_top_level_value_of_the_wrong_kind_is_named(self) -> None:
        for label, loader, value, errors in subjects():
            wrong: tuple[object, ...] = (
                None,
                True,
                1,
                1.5,
                "x",
                [] if isinstance(value, dict) else {},
            )
            for junk in wrong:
                with self.subTest(f"{label}: {junk!r}"), self.assertRaises(errors):
                    loader(junk, label)


class Guards(unittest.TestCase):
    """A TypeGuard that returns True must leave a value of the type it narrows to."""

    def test_01_a_chat_inputs_record_narrows_only_when_it_is_one(self) -> None:
        value = chat_inputs_record()
        self.assertTrue(inputs.valid_record(value))
        self.assertTrue(conforms(value, inputs.InputRecord))
        failures: list[str] = []
        accepted = rejected = 0
        for path in keys_of(value):
            for junk in (*JUNK, DELETE, 1.0, -0.0, "1", [1]):
                candidate = replaced(value, path, junk)
                if inputs.valid_record(candidate):
                    accepted += 1
                    if not conforms(candidate, inputs.InputRecord):
                        failures.append(f"{path} -> {junk!r}: accepted as {candidate!r}")
                else:
                    rejected += 1
        for junk in (*JUNK, 1.0, "1"):  # a field added later is neither required nor checked
            candidate = {**value, "added_later": junk}
            if inputs.valid_record(candidate):
                accepted += 1
                if not conforms(candidate, inputs.InputRecord):
                    failures.append(f"added_later -> {junk!r}: accepted as {candidate!r}")
            else:
                rejected += 1
        self.assertEqual(failures, [], f"{len(failures)} nonconforming of {accepted + rejected}")
        self.assertGreater(accepted, 10)
        self.assertGreater(rejected, 50)

    def test_02_the_schema_is_the_integer_one_itself(self) -> None:
        for schema in (True, 1.0, 1.5, "1", None, [1], 2, 0):
            with self.subTest(schema=schema):
                self.assertFalse(inputs.valid_record({**chat_inputs_record(), "schema": schema}))
        added = {**chat_inputs_record(), "added_later": {"x": 1}}
        self.assertTrue(inputs.valid_record(added))  # unknown additive fields are fine

    def test_03_the_aggregate_read_from_receipts_is_the_recorded_one_or_none(self) -> None:
        record = chat_inputs_record()
        good = {
            "docs": {"kind": "docs", "previous": "ci.json"},
            "ci": {"kind": "ci", "passed": True, "chat_inputs": record},
        }
        failures: list[str] = []
        checked = 0
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            pointer = cast("records.AcceptedPointer", {"receipt": str(base / "docs.json")})
            for path in (None, *keys_of(good)):
                for junk in (*JUNK, DELETE):
                    pair = good if path is None else replaced(good, path, junk)
                    docs, ci = pair.get("docs"), pair.get("ci")
                    if isinstance(docs, dict) and isinstance(docs.get("previous"), str):
                        docs = {**docs, "previous": str(base / "ci.json")}
                    (base / "docs.json").write_text(json.dumps(docs))
                    (base / "ci.json").write_text(json.dumps(ci))
                    checked += 1
                    found = install.accepted_receipt_aggregate(pointer)
                    if found is not None and found != record["aggregate"]:
                        failures.append(f"{path} -> {junk!r}: invented {found!r}")
                    if path is None:
                        self.assertEqual(found, record["aggregate"])
        self.assertEqual(failures[:10], [], f"{len(failures)} of {checked}")


class Configuration(unittest.TestCase):
    def config(self) -> dict[str, object]:
        return {
            "owner": "vince",
            "assistants": ["codex"],
            "projects": {"codekarl": fixture()["settings"]["settings"], "mosk": {"channel": "#m"}},
        }

    def projects(self, config: dict[str, object]) -> dict[str, object]:
        projects = config["projects"]
        assert isinstance(projects, dict)
        return projects

    def test_01_the_codekarl_project_is_the_policy_only_when_it_selects_it(self) -> None:
        config = self.config()
        found = codekarl_chat.settings_of(config)
        assert found is not None
        self.assertEqual(found["manager"], "ck-manager")
        self.assertEqual(codekarl_chat.settings(config), found)
        self.projects(config)["codekarl"] = {"channel": "#codekarl", "routing": "other"}
        self.assertIsNone(codekarl_chat.settings_of(config))
        self.assertEqual(codekarl_chat.settings(config), {})
        self.assertFalse(codekarl_chat.applies(config, "#codekarl"))
        del self.projects(config)["codekarl"]
        self.assertIsNone(codekarl_chat.settings_of(config))

    def test_02_a_malformed_policy_section_is_named_with_the_next_action(self) -> None:
        for key, junk in (
            ("owners", DELETE),
            ("owners", "vince"),
            ("assistants", [1]),
            ("delivery_enabled", "yes"),
            ("prefix", None),
        ):
            config = self.config()
            project = self.projects(config)["codekarl"]
            assert isinstance(project, dict)
            if junk is DELETE:
                del project[key]
            else:
                project[key] = junk
            with self.subTest(key), self.assertRaisesRegex(codekarl_chat.RecordError, "installer"):
                codekarl_chat.settings_of(config)

    def test_03_authority_is_the_policys_people_for_codekarl_and_the_owner_elsewhere(self) -> None:
        config = self.config()
        self.assertEqual(codekarl_chat.authorities(config, "codekarl"), {"vince", "coghex", "laz"})
        self.assertEqual(codekarl_chat.authorities(config, "mosk"), {"vince", "codex"})
        config["assistants"] = "codex"
        with self.assertRaisesRegex(codekarl_chat.RecordError, "installer"):
            codekarl_chat.authorities(config, "mosk")

    def test_04_channels_belong_to_the_policy_by_name_or_prefix(self) -> None:
        config = self.config()
        for channel, expected in (
            ("#codekarl", True),
            ("#CODEKARL", True),
            ("#ck-task", True),
            ("#mosk", False),
        ):
            self.assertEqual(codekarl_chat.applies(config, channel), expected, channel)


class Stored(unittest.TestCase):
    """The loaders at the SQLite boundary: the rows the inbox itself wrote, and damaged ones."""

    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.cfg: dict[str, object] = {
            "projects": {"codekarl": fixture()["settings"]["settings"]},
            "identities": {
                name: {"project": "codekarl", "role": role}
                for name, role in (
                    ("ck-manager", "manager"),
                    ("ck-worker-1", "worker"),
                    ("ck-reviewer-1", "reviewer"),
                    ("ck-guide", "guide"),
                )
            },
        }
        self.agents = {
            name: {"status": "active"}
            for name in ("ck-manager", "ck-worker-1", "ck-reviewer-1", "ck-guide")
        }
        self.inbox = Inbox(self.temp.name)
        for index, entry in enumerate(fixture()["entries"].values()):
            routed = copy.deepcopy(entry)
            routed["msgid"] = f"m{index}"
            routed["text"] = "@ck-worker-1 " + routed["text"]
            routed.pop("reply_to", None)
            self.inbox.ingest(routed, self.cfg, self.agents)
        reply = {
            "msgid": "r1",
            "channel": "#codekarl",
            "from": "ck-guide",
            "verified": True,
            "text": "[done] sanitized",
            "reply_to": "m1",
        }
        self.inbox.ingest(reply, self.cfg, self.agents)
        self.inbox.claim("m0", "ck-worker-1")
        token = self.inbox.acquire("m0", "ck-worker-1", "reaction", "pid=1")
        assert token is not None
        self.inbox.record_effect("m0", "ck-worker-1", "reaction", token, "sent", "pchat exit 0")
        for ok, detail, extra in (
            (None, "queued at ck-worker-1", {"agent_msg_id": "cmux-1"}),
            (False, "not accepted after 15 min; reminder 1", {"reminders": 1}),
            (True, "accepted by ck-worker-1 (ack:ack)", {}),
            (True, "typed into prompt and submitted: ck-worker-1", {}),
        ):
            item: dict[str, object] = {
                "ck_policy": codekarl_chat.POLICY,
                "msgid": "m0",
                "target": "ck-worker-1",
                "attempt": 0,
                **extra,
            }
            self.inbox.outcome(item, ok, detail)

    def mutate(self, sql: str) -> None:
        db = sqlite3.connect(self.inbox.path)
        try:
            with db:
                db.execute(sql)
        finally:
            db.close()

    def test_01_what_the_inbox_returns_has_the_shape_it_declares(self) -> None:
        rows = self.inbox.rows(history=True)
        self.assertGreaterEqual(len(rows), 4)
        self.assertTrue(conforms(rows, list[codekarl_chat.InboxItem]), rows)
        self.assertTrue(conforms(self.inbox.rows(), list[codekarl_chat.InboxItem]))
        self.assertTrue(conforms(self.inbox.diagnostics(), list[codekarl_chat.Diagnostic]))
        self.assertTrue(conforms(self.inbox.notices(self.cfg), list[codekarl_chat.Notice]))
        effects = self.inbox.effects("m0", "ck-worker-1")
        self.assertEqual(list(effects), ["reaction", "visible"])
        self.assertTrue(conforms(effects, dict[str, codekarl_chat.EffectRow]), effects)
        claim = self.inbox.claim("m2", "ck-worker-1")
        self.assertTrue(conforms(claim, codekarl_chat.Claim), claim)
        observed = next(r for r in rows if r["message"] == "m0")["observations"]
        self.assertEqual(
            [o["kind"] for o in observed], ["waiting", "attempt-failed", "accepted", "delivered"]
        )
        self.assertTrue(conforms(observed, list[codekarl_chat.Observation]), observed)

    def test_02_the_keys_and_their_order_are_the_ones_the_row_was_stored_with(self) -> None:
        item = self.inbox.rows(history=True)[0]
        self.assertEqual(
            list(item),
            [
                "message",
                "recipient",
                "state",
                "notice",
                "updated",
                "payload",
                "routing",
                "observations",
            ],
        )
        self.assertEqual(list(self.inbox.diagnostics()[0]), ["id", "payload", "routing"])
        self.assertEqual(
            list(self.inbox.effects("m0", "ck-worker-1")["reaction"]),
            ["message", "recipient", "effect", "state", "detail", "token", "holder", "at"],
        )

    def test_03_a_column_a_later_schema_adds_survives_in_the_row(self) -> None:
        self.mutate("ALTER TABLE inbox ADD COLUMN added_later TEXT DEFAULT 'kept'")
        item = self.inbox.rows()[0]
        self.assertEqual(item["added_later"], "kept")  # type: ignore[typeddict-item]  # additive

    def test_04_a_damaged_row_is_named_with_the_message_and_the_next_action(self) -> None:
        bad_payload = '{"from": 3, "channel": "#c", "text": "t"}'
        cases = (
            (f"UPDATE messages SET payload='{bad_payload}' WHERE id='m1'", "message m1 payload"),
            ("UPDATE messages SET payload='not json' WHERE id='m1'", "not valid JSON"),
            ("UPDATE messages SET routing='[]' WHERE id='m1'", "message m1 routing"),
            ("UPDATE messages SET routing='{\"role\": \"x\"}' WHERE id='m1'", "message m1 routing"),
            ("UPDATE inbox SET updated='soon' WHERE message='m1'", "updated"),
            ("UPDATE inbox SET notice=X'4142' WHERE message='m1'", "notice"),
        )
        for sql, mentions in cases:
            with self.subTest(sql):
                self.setUp()
                self.mutate(sql)
                with self.assertRaises(codekarl_chat.RecordError) as caught:
                    self.inbox.rows(history=True)
                self.assertIn(mentions, str(caught.exception))
                self.assertIn("ck-chat inbox --all --history", str(caught.exception))

    def test_05_a_damaged_effect_row_is_named(self) -> None:
        self.mutate("UPDATE ack_effects SET at='then' WHERE message='m0'")
        with self.assertRaisesRegex(codekarl_chat.RecordError, "acknowledgement effect of m0"):
            self.inbox.effects("m0", "ck-worker-1")

    def test_06_a_delivery_item_the_bridge_hands_back_is_checked(self) -> None:
        item: dict[str, object] = {"ck_policy": codekarl_chat.POLICY, "target": "ck-worker-1"}
        with self.assertRaisesRegex(codekarl_chat.RecordError, "msgid"):
            self.inbox.outcome(item, True, "x")
        item = {**item, "msgid": "m0", "attempt": "first"}
        with self.assertRaisesRegex(codekarl_chat.RecordError, "attempt"):
            self.inbox.outcome(item, True, "x")
        before = self.inbox.rows(history=True)
        self.inbox.outcome({"msgid": "m0", "target": "ck-worker-1"}, True, "x")  # not Codekarl's
        self.assertEqual(self.inbox.rows(history=True), before)

    def test_07_an_entry_without_a_sender_or_text_is_refused_before_anything_is_stored(
        self,
    ) -> None:
        before = len(self.inbox.diagnostics()), len(self.inbox.rows(history=True))
        for entry in (
            {"msgid": "z"},
            {"msgid": "z", "channel": "#codekarl", "from": 1, "text": "t"},
        ):
            with self.assertRaises(codekarl_chat.RecordError):
                self.inbox.ingest(entry, self.cfg, self.agents)
        self.assertEqual(
            (len(self.inbox.diagnostics()), len(self.inbox.rows(history=True))), before
        )


FORGED = json.dumps(
    {
        "channel": "#forged",
        "from": "mallory",
        "text": "forged",
        "role": "owner",
        "recipients": [],
        "reason": "",
        "owner_direct": False,
        "actionable": False,
    }
)  # valid as a message and as a routing decision, and different from every real value


class StoredDamage(unittest.TestCase):
    """Every cell of the inbox database damaged in turn, and every column a later schema could add
    under the name of a returned field: each reader names the problem or returns its declared type."""

    JUNK_SQL = ("NULL", "0", "1", "1.5", "'x'", "''", "X'41'", "'[]'", "'{}'", "'null'", "'3'")
    COLUMNS: ClassVar[dict[str, tuple[str, ...]]] = {
        "messages": ("id", "payload", "routing"),
        "inbox": ("message", "recipient", "state", "notice", "updated"),
        "ack_effects": (
            "message",
            "recipient",
            "effect",
            "state",
            "detail",
            "token",
            "holder",
            "at",
        ),
        "observations": ("message", "recipient", "kind", "ok", "detail", "attempt", "at"),
    }
    CARRIERS: ClassVar[dict[str, tuple[str, ...]]] = {
        "inbox": ("rows", "rows (history)", "rows (a recipient)"),
        "ack_effects": ("effects",),
    }
    RETURNED = (
        "observations", "payload", "routing", "message", "recipient", "state", "notice", "updated",
        "id", "effect", "detail", "token", "holder", "at", "kind", "ok",
        "payload_json", "routing_json", "payload_text", "routing_text", "Payload", "ROUTING",
    )  # fmt: skip

    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        base = Path(self.temp.name)
        self.cfg: dict[str, object] = {
            "projects": {"codekarl": fixture()["settings"]["settings"]},
            "identities": {"ck-worker-1": {"project": "codekarl", "role": "worker"}},
        }
        source = Inbox(base / "source")
        agents = {"ck-worker-1": {"status": "active"}}
        for index in range(3):
            entry = {
                "msgid": f"m{index}",
                "channel": "#codekarl",
                "from": "vince",
                "verified": True,
                "text": f"@ck-worker-1 sanitized {index}",
            }
            source.ingest(entry, self.cfg, agents)
        source.ingest(
            {"msgid": "bad", "channel": "#codekarl", "from": "vince", "verified": True,
             "text": "!! @nobody-known sanitized"},
            self.cfg,
            agents,
        )  # fmt: skip
        source.claim("m0", "ck-worker-1")
        source.outcome(
            {
                "ck_policy": codekarl_chat.POLICY,
                "msgid": "m0",
                "target": "ck-worker-1",
                "attempt": 0,
            },
            True,
            "accepted by ck-worker-1 (ack:ack)",
        )
        self.template = source.path
        self.count = 0
        self.expected: dict[str, object] = {}
        for label, read, _ in self.readers(self.fresh()):
            self.expected[label] = read()  # what the undamaged database answers

    def fresh(self) -> Inbox:
        self.count += 1
        state = Path(self.temp.name) / f"case{self.count}"
        (state / "codekarl").mkdir(parents=True)
        shutil.copyfile(self.template, state / "codekarl/inbox.sqlite3")
        return Inbox(state)

    def readers(self, inbox: Inbox) -> list[tuple[str, Callable[[], object], object]]:
        return [
            ("rows", inbox.rows, list[codekarl_chat.InboxItem]),
            ("rows (history)", lambda: inbox.rows(history=True), list[codekarl_chat.InboxItem]),
            (
                "rows (a recipient)",
                lambda: inbox.rows("ck-worker-1", True),
                list[codekarl_chat.InboxItem],
            ),
            ("diagnostics", inbox.diagnostics, list[codekarl_chat.Diagnostic]),
            ("notices", lambda: inbox.notices(self.cfg), list[codekarl_chat.Notice]),
            (
                "effects",
                lambda: inbox.effects("m0", "ck-worker-1"),
                dict[str, codekarl_chat.EffectRow],
            ),
            ("claim", lambda: inbox.claim("m1", "ck-worker-1"), codekarl_chat.Claim),
            ("needs_delivery", lambda: inbox.needs_delivery("m1", "ck-worker-1"), bool),
        ]

    @staticmethod
    def without(value: object, name: str) -> object:
        """`value` without the key `name` on its items: a column a later schema added is kept."""
        if isinstance(value, list):
            return [StoredDamage.without(item, name) for item in value]
        if isinstance(value, dict):
            inner = {key: StoredDamage.without(item, name) for key, item in value.items()}
            return (
                inner
                if all(isinstance(item, dict) for item in value.values())
                else {key: item for key, item in value.items() if key != name}
            )
        return value

    def judge(
        self, inbox: Inbox, what: str, failures: list[str], added: tuple[str, str] | None = None
    ) -> int:
        """How many readers answered with a value; a refusal must be named, an answer must conform
        and, when a column was only added, be the answer the undamaged database gives (apart from
        that column itself)."""
        answered = 0
        for label, read, hint in self.readers(inbox):
            try:
                value = read()
            except ValueError as error:  # a RecordError, or the claim's "not addressed to you"
                if not any(word in str(error) for word in ("inspect", "rename", "correct")):
                    failures.append(f"{what}: {label}: no next action: {error}")
                continue
            except Exception as error:  # anything else escaping is the finding
                failures.append(f"{what}: {label}: raised {type(error).__name__}: {error}")
                continue
            answered += 1
            if not conforms(value, hint):
                failures.append(f"{what}: {label}: answered with {value!r}")
            elif added is not None:
                table, name = added
                # an added inbox or effect column is carried into those rows (and only those);
                # anywhere else an added column must change nothing at all
                carried = label in self.CARRIERS.get(table, ())
                shown = self.without(value, name) if carried else value
                if shown != self.expected[label]:
                    failures.append(f"{what}: {label}: answered from the wrong source: {value!r}")
                elif carried and not self.survives(value, name):
                    failures.append(f"{what}: {label}: the added column {name} was lost: {value!r}")
        return answered

    @staticmethod
    def survives(value: object, name: str) -> bool:
        """Every row the reader returned still carries the added column, with the value stored."""
        rows = value.values() if isinstance(value, dict) else value
        assert isinstance(rows, Iterable)
        found = [row.get(name) == FORGED for row in rows if isinstance(row, dict)]
        return bool(found) and all(found)

    def update(self, inbox: Inbox, table: str, column: str, junk: str) -> bool:
        db = sqlite3.connect(inbox.path)
        try:
            with db:
                db.execute(
                    f"UPDATE {table} SET {column}={junk} "
                    f"WHERE rowid = (SELECT min(rowid) FROM {table})"
                )
        except sqlite3.IntegrityError:
            return False  # the database itself refuses this value: it cannot be stored
        finally:
            db.close()
        return True

    def test_01_a_damaged_cell_is_named_or_the_answer_has_its_declared_shape(self) -> None:
        failures: list[str] = []
        cases = answered = 0
        for table, columns in self.COLUMNS.items():
            for column in columns:
                for junk in self.JUNK_SQL:
                    inbox = self.fresh()
                    if not self.update(inbox, table, column, junk):
                        continue
                    cases += 1
                    answered += self.judge(inbox, f"{table}.{column}={junk}", failures)
        self.assertEqual(failures[:10], [], f"{len(failures)} nonconforming in {cases} cases")
        self.assertGreater(cases, 150)
        self.assertGreater(answered, 500)

    def test_02_a_column_added_under_a_returned_name_never_replaces_the_returned_value(
        self,
    ) -> None:
        failures: list[str] = []
        cases = 0
        for table in self.COLUMNS:
            for name in self.RETURNED:
                inbox = self.fresh()
                db = sqlite3.connect(inbox.path)
                try:
                    with db:
                        db.execute(f"ALTER TABLE {table} ADD COLUMN {name} TEXT DEFAULT '{FORGED}'")
                except sqlite3.OperationalError:
                    continue  # the table already has a column of that name
                finally:
                    db.close()
                cases += 1
                self.judge(inbox, f"{table} + column {name}", failures, added=(table, name))
        self.assertEqual(failures[:10], [], f"{len(failures)} nonconforming in {cases} cases")
        self.assertGreater(cases, 30)

    def test_03_the_collision_with_the_computed_history_is_named_with_a_recovery(self) -> None:
        inbox = self.fresh()
        db = sqlite3.connect(inbox.path)
        with db:
            db.execute("ALTER TABLE inbox ADD COLUMN observations TEXT DEFAULT 'not-a-list'")
        db.close()
        for history in (False, True):
            with self.assertRaisesRegex(codekarl_chat.RecordError, "observations.*RENAME COLUMN"):
                inbox.rows(history=history)
        for name in ("payload", "routing"):
            inbox = self.fresh()
            db = sqlite3.connect(inbox.path)
            with db:
                db.execute(f"ALTER TABLE inbox ADD COLUMN {name} TEXT DEFAULT 'x'")
            db.close()
            with self.assertRaisesRegex(codekarl_chat.RecordError, f"{name}.*RENAME COLUMN"):
                inbox.rows()

    def test_04_a_column_named_like_an_internal_alias_cannot_substitute_the_stored_message(
        self,
    ) -> None:
        forged_message = json.dumps(
            {"channel": "#codekarl", "from": "ck-manager", "text": "@ck-worker-1 forged"}
        )
        forged_routing = json.dumps(
            {
                "role": "worker",
                "recipients": ["ck-worker-1"],
                "reason": "",
                "owner_direct": False,
                "actionable": True,
                "interrupt": True,
            }
        )
        stored = self.fresh()
        before: dict[str, list[dict[str, object]]] = {
            "rows": [dict(row) for row in stored.rows()],
            "history": [dict(row) for row in stored.rows(history=True)],
            "notices": [dict(notice) for notice in stored.notices(self.cfg)],
        }
        self.assertEqual(stored.rows(history=True)[0]["payload"]["from"], "vince")
        self.assertEqual(before["notices"][0]["authority"], "owner")
        for name in (
            "payload_json",
            "routing_json",
            "PAYLOAD_JSON",
            "Routing_Json",
            "payload_text",
        ):
            for forged in (forged_message, forged_routing):
                with self.subTest(column=name, holds=forged[:20]):
                    inbox = self.fresh()
                    db = sqlite3.connect(inbox.path)
                    with db:
                        db.execute(f"ALTER TABLE inbox ADD COLUMN {name} TEXT DEFAULT '{forged}'")
                    db.close()
                    for label, now in (
                        ("rows", [dict(row) for row in inbox.rows()]),
                        ("history", [dict(row) for row in inbox.rows(history=True)]),
                    ):
                        self.assertEqual(
                            [{k: v for k, v in r.items() if k != name} for r in now],
                            before[label],
                            label,
                        )
                        self.assertTrue(all(r[name] == forged for r in now), label)
                    notices = [dict(notice) for notice in inbox.notices(self.cfg)]
                    self.assertEqual(notices, before["notices"])


class IdentityConflictFile(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        state_dir(Path(self.temp.name))
        self.addCleanup(state_dir, Path("/nonexistent"))

    def test_01_what_the_writer_writes_the_reader_reads_and_a_missing_file_is_empty(self) -> None:
        self.assertEqual(identity_conflicts.read(), [])
        identity_conflicts.replace_all(
            [
                {"project": "codekarl", "role": "worker", "surface_id": "S-2", "incumbent": "ck-b"},
                {"project": "codekarl", "role": "worker", "surface_id": "S-1", "incumbent": "ck-a"},
            ]
        )
        rows = identity_conflicts.read()
        self.assertEqual([row.get("surface_id") for row in rows], ["S-1", "S-2"])
        self.assertTrue(conforms(rows, list[identity_conflicts.ConflictRow]))
        found = identity_conflicts.for_surface("s-1")
        assert found is not None
        self.assertEqual(found.get("incumbent"), "ck-a")
        with self.assertRaises(identity_conflicts.IdentityConflict):
            identity_conflicts.raise_for_surface("S-2")
        identity_conflicts.raise_for_surface("S-9")
        identity_conflicts.replace_all([])
        self.assertEqual(identity_conflicts.read(), [])

    def test_02_a_damaged_file_names_itself_and_how_it_recovers(self) -> None:
        path = identity_conflicts.diagnostic_path()
        for text in ('{"a": 1}', "[3]", '[{"surface_id": 3}]'):
            path.write_text(text)
            with self.assertRaisesRegex(CHAT_ERROR, "identity-conflicts.json.*rewrites it"):
                identity_conflicts.read()


class EntryPoint(unittest.TestCase):
    """`tools/ck-chat` is a shim around `tools/chat/ck_chat.py`, the code mypy checks."""

    def environment(self, home: Path) -> dict[str, str]:
        scripts = home / ".codex/skills/chat/scripts"
        scripts.mkdir(parents=True)
        (scripts / "chatlib.py").write_text(
            "from pathlib import Path\nclass ChatError(RuntimeError):\n    pass\n"
            "STATE_DIR = Path('state')\nCONFIG_PATH = Path('config.json')\n"
            "def load_config():\n    return {}\n"
        )
        (scripts / "identities.py").write_text("def current(cfg):\n    return None\n")
        return {**os.environ, "HOME": str(home)}

    def run_shim(
        self, command: Path, home: Path, environment: dict[str, str], *arguments: str
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(command), *arguments],
            capture_output=True,
            text=True,
            check=False,
            env=environment,
            cwd=home,
        )

    def test_01_the_shim_is_three_lines_and_runs_the_module(self) -> None:
        shim = ROOT / "tools/ck-chat"
        text = shim.read_text()
        self.assertLessEqual(len(text.splitlines()), 3)
        self.assertTrue(text.startswith("#!/usr/bin/env python3\n"))
        self.assertTrue(os.access(shim, os.X_OK))
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp).resolve()
            environment = self.environment(home)
            link = home / "elsewhere"
            link.symlink_to(shim)  # the skills call it by other paths
            for command in (shim, link):
                done = self.run_shim(command, home, environment, "--help")
                self.assertEqual(done.returncode, 0, done.stderr)
                self.assertIn("usage: ck-chat", done.stdout)  # not the script that happens to run
                self.assertIn("Codekarl inbox and posting entry point", done.stdout)
                self.assertIn("delivery", done.stdout)
            refused = self.run_shim(shim, home, environment, "claim")
            self.assertEqual(refused.returncode, 2)  # argparse: the message is missing
            self.assertIn("message", refused.stderr)

    def test_02_a_bound_record_without_a_name_is_named(self) -> None:
        bound: dict[str, object] = {"name": "ck-x", "project": "codekarl", "status": "active"}
        self.assertEqual(ck_chat.bound_name(bound), "ck-x")
        self.assertIsNone(ck_chat.bound_name({**bound, "status": "retired"}))
        with self.assertRaisesRegex(ValueError, "ck-chat status"):
            ck_chat.bound_name({"project": "codekarl", "status": "active"})


if __name__ == "__main__":
    unittest.main()
