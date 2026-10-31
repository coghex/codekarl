"""Codekarl's IRC policy and durable inbox; no network or terminal operations.

This module is installed on its own next to the shared chat code, so it imports only the standard
library. The shared files it reads (the pchat configuration, the bridge's message entries and
delivery items) and its own SQLite rows are parsed at the boundary: a loader checks every field it
declares, keeps any additive field it does not know, and returns a TypedDict."""

import contextlib
import hashlib
import json
import re
import sqlite3
import time
import uuid
from collections.abc import Callable, Iterator, Mapping
from pathlib import Path
from typing import NotRequired, TypedDict, cast

POLICY = "codekarl-v1"
TERMINAL = {"completed", "declined"}
QUIET = {"status", "ack"}
KNOWN_INFORMATIONAL = {"status", "ack", "done", "declined", "answer", "decision"}
KNOWN_ACTIONABLE = {"request", "question", "blocked", "interrupt"}
BRIDGE_STATUS = "[status] chatbridge: "
EFFECTS = ("reaction", "visible")
# Only these can be retried by the tool; `uncertain` and `attempting` never are.
RETRYABLE = {"pending", "not-sent"}
RANK = {"queued": 0, "delivered": 1, "acknowledged": 2, "completed": 3, "declined": 3}
INSPECT = "inspect with: ck-chat inbox --all --history"
CONFIGURE = (
    "correct the codekarl project in the pchat configuration, then run the accepted chat installer"
)


class RecordError(ValueError):
    """A chat record or configuration is not in a form this code can use."""


Entry = TypedDict(
    "Entry",
    {
        "channel": str,
        "from": str,
        "text": str,
        "msgid": NotRequired[str | None],
        "verified": NotRequired[bool],
        "reply_to": NotRequired[str | None],
        "at": NotRequired[str],
        "nick": NotRequired[str],
        "role": NotRequired[str],
        "project": NotRequired[str | None],
        "session_id": NotRequired[str],
        "surface_id": NotRequired[str],
    },
)
"""One IRC message as the bridge parsed it; unknown additive fields (parent, replayed, ...) survive."""


class Routing(TypedDict):
    """The routing decision stored with a message. Rows written before interrupts and owner
    notices existed carry neither of the last two fields."""

    role: str
    recipients: list[str]
    reason: str
    owner_direct: bool
    actionable: bool
    interrupt: NotRequired[bool]
    notify_owner: NotRequired[bool]


class ProjectSettings(TypedDict):
    """The `projects.codekarl` section of the pchat configuration when it selects this policy."""

    channel: str
    prefix: str
    manager: str
    routing: str
    owners: list[str]
    assistants: list[str]
    delivery_enabled: NotRequired[bool]


class Claim(TypedDict):
    fresh: bool
    previous: str
    message: str
    recipient: str


class EffectRow(TypedDict):
    """One stored acknowledgement effect (CK-5)."""

    message: str
    recipient: str
    effect: str
    state: str
    detail: str
    token: str
    holder: str
    at: float


class Observation(TypedDict):
    kind: str
    ok: bool | None
    detail: str
    at: float


class InboxItem(TypedDict):
    """One inbox row with its message and routing; `observations` only when history is asked for."""

    message: str
    recipient: str
    state: str
    notice: str
    updated: float
    payload: Entry
    routing: Routing
    observations: NotRequired[list[Observation]]


class Diagnostic(TypedDict):
    id: str
    payload: Entry
    routing: Routing


class Notice(TypedDict):
    """The delivery item handed to the bridge for one recipient."""

    kind: str
    msgid: str
    channel: str
    sender: str
    text: str
    project: str
    target: str
    attempt: int
    phase: str
    next_at: float
    needs_ack: bool
    interrupt: bool
    ck_policy: str
    authority: str
    owner_direct: bool


def _fail(label: str, problem: str, action: str = INSPECT) -> RecordError:
    return RecordError(f"{label}: {problem}; {action}")


def _object(value: object, label: str, action: str = INSPECT) -> dict[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise _fail(label, f"must be a JSON object, not {type(value).__name__}", action)
    return value


def _is_number(value: object) -> bool:
    return isinstance(value, int | float) and not isinstance(value, bool)


Check = tuple[Callable[[object], bool], str]


def _check(
    record: Mapping[str, object],
    label: str,
    required: Mapping[str, Check],
    optional: Mapping[str, Check],
    action: str = INSPECT,
) -> None:
    """Every declared field present with its type; absent optional fields are fine."""
    for fields, needed in ((required, True), (optional, False)):
        for key, (valid, described) in fields.items():
            if key not in record:
                if needed:
                    raise _fail(label, f"field {key!r} is missing", action)
                continue
            if not valid(record[key]):
                raise _fail(label, f"field {key!r} must be {described}", action)


def _text(value: object) -> bool:
    return isinstance(value, str)


def _text_or_none(value: object) -> bool:
    return value is None or isinstance(value, str)


def _flag(value: object) -> bool:
    return isinstance(value, bool)


def _names(value: object) -> bool:
    return isinstance(value, list) and all(isinstance(item, str) for item in value)


STR = (_text, "a string")
STR_OR_NONE = (_text_or_none, "a string or null")
FLAG = (_flag, "true or false")
NAMES = (_names, "a list of strings")
NUMBER = (_is_number, "a number")


def load_entry(value: object, label: str = "message") -> Entry:
    """A message entry: channel, sender and text are required; the rest is checked when present."""
    record = _object(value, label)
    _check(
        record,
        label,
        {"channel": STR, "from": STR, "text": STR},
        {
            "msgid": STR_OR_NONE,
            "verified": FLAG,
            "reply_to": STR_OR_NONE,
            "at": STR,
            "nick": STR,
            "role": STR,
            "project": STR_OR_NONE,
            "session_id": STR,
            "surface_id": STR,
        },
    )
    return cast(Entry, record)  # every declared field was checked above; additive fields remain


def load_routing(value: object, label: str = "routing") -> Routing:
    """A routing decision, including the older rows that predate `interrupt` and `notify_owner`."""
    record = _object(value, label)
    _check(
        record,
        label,
        {
            "role": STR,
            "recipients": NAMES,
            "reason": STR,
            "owner_direct": FLAG,
            "actionable": FLAG,
        },
        {"interrupt": FLAG, "notify_owner": FLAG},
    )
    return cast(Routing, record)  # every declared field was checked above; additive fields remain


def load_project_settings(value: object, label: str = "projects.codekarl") -> ProjectSettings:
    record = _object(value, label, CONFIGURE)
    _check(
        record,
        label,
        {
            "channel": STR,
            "prefix": STR,
            "manager": STR,
            "routing": STR,
            "owners": NAMES,
            "assistants": NAMES,
        },
        {"delivery_enabled": FLAG},
        CONFIGURE,
    )
    return cast(ProjectSettings, record)  # checked above; the other keys (path, ...) remain


def _stored(text: object, label: str) -> object:
    """A JSON column of a row, parsed; the only place stored text becomes data."""
    if not isinstance(text, str):
        raise _fail(label, f"column must hold JSON text, not {type(text).__name__}")
    try:
        parsed: object = json.loads(text)
    except ValueError as error:
        raise _fail(label, f"column is not valid JSON ({error})") from error
    return parsed


def _columns(
    row: sqlite3.Row, label: str, required: Mapping[str, Check], trailing: int = 0
) -> dict[str, object]:
    """A row's columns in table order, each declared one checked and any other kept as it is.

    The last `trailing` cells are values a query selected from another table: they are read by
    position (`_trailing`), so a column of that name in this one can neither hide nor replace them."""
    record: dict[str, object] = {}
    names = row.keys()
    for index, key in enumerate(names[: len(names) - trailing]):
        cell: object = row[index]
        record[key] = cell
    _check(record, label, required, {})
    return record


def _trailing(row: sqlite3.Row) -> tuple[object, object]:
    """The last two cells of a joined row: the message's payload and routing text."""
    payload: object = row[len(row) - 2]
    routing: object = row[len(row) - 1]
    return payload, routing


def settings_of(cfg: Mapping[str, object]) -> ProjectSettings | None:
    """The Codekarl project settings when the configuration selects this policy, else None."""
    cfg = _object(cfg, "pchat configuration", CONFIGURE)
    projects = _object(cfg.get("projects", {}), "projects", CONFIGURE)
    project = _object(projects.get("codekarl", {}), "projects.codekarl", CONFIGURE)
    if project.get("routing") != POLICY:
        return None
    return load_project_settings(project)


def _cell_text(row: sqlite3.Row, column: str, label: str) -> str:
    cell: object = row[column]
    if not isinstance(cell, str):
        raise _fail(label, f"column {column!r} must be text, not {type(cell).__name__}")
    return cell


def _int_or_none(value: object) -> bool:
    return value is None or (isinstance(value, int) and not isinstance(value, bool))


def _text_or_int_or_none(value: object) -> bool:
    return isinstance(value, str) or _int_or_none(value)


INBOX_COLUMNS = {
    "message": STR,
    "recipient": STR,
    "state": STR,
    "notice": STR,
    "updated": NUMBER,
}
EFFECT_COLUMNS = {
    "message": STR,
    "recipient": STR,
    "effect": STR,
    "state": STR,
    "detail": STR,
    "token": STR,
    "holder": STR,
    "at": NUMBER,
}
OBSERVATION_COLUMNS = {
    "message": STR,
    "recipient": STR,
    "kind": STR,
    "ok": (_int_or_none, "an integer or null"),
    "detail": STR,
    "at": NUMBER,
}
UNCONFIGURED = "Codekarl chat is not configured; run the accepted chat installer"


def settings(cfg: Mapping[str, object]) -> Mapping[str, object]:
    """The Codekarl project settings, or an empty mapping when this policy is not configured."""
    return settings_of(cfg) or {}


def applies(cfg: Mapping[str, object], channel: str) -> bool:
    project = settings_of(cfg)
    return project is not None and (
        channel.lower() == project["channel"].lower()
        or channel.lower().startswith(project["prefix"].lower())
    )


def authorities(cfg: Mapping[str, object], project: str | None) -> set[str | None]:
    policy = settings_of(cfg)
    if project == "codekarl" and policy is not None:
        return {*policy["owners"], *policy["assistants"]}
    owner = cfg.get("owner")
    assistants = cfg.get("assistants", [])
    if (owner is not None and not isinstance(owner, str)) or not _names(assistants):
        raise _fail("owner and assistants", "must be a name and a list of names", CONFIGURE)
    return {owner, *cast(list[str], assistants)}  # _names checked every element above


def message_kind(text: str) -> str:
    """A known bracket word, otherwise "request": an unknown kind is never silently quiet."""
    match = re.match(r"^\[([a-z]+)(?:\s[^\]]+)?\]", text)
    kind = match[1] if match else "request"
    return kind if kind in KNOWN_INFORMATIONAL | KNOWN_ACTIONABLE else "request"


def observation_kind(ok: bool | None, detail: object) -> str:
    """Classify one bridge delivery report. Acceptance is not delivery evidence."""
    text = str(detail)
    if ok is None:
        return "waiting"
    if ok:
        return "accepted" if text.startswith("accepted by ") else "delivered"
    return "failed" if text.startswith("dead-lettered:") else "attempt-failed"


def addresses(text: str) -> set[str]:
    names = set(re.findall(r"(?<![\w])@([A-Za-z0-9_-]+)", text))
    leading = re.match(r"^(?:!!\s*)?([A-Za-z0-9_-]+)[:,]\s", text)
    if leading:
        names.add(leading[1])
    return {name.lower() for name in names}


def _roles(cfg: Mapping[str, object]) -> dict[str, str]:
    """Role of every identity that belongs to the Codekarl project."""
    identities = _object(cfg.get("identities", {}), "identities", CONFIGURE)
    members: dict[str, str] = {}
    for name, meta in identities.items():
        record = _object(meta, f"identities.{name}", CONFIGURE)
        if record.get("project") == "codekarl":
            role = record.get("role")
            if not isinstance(role, str):
                raise _fail(f"identities.{name}", "field 'role' must be a string", CONFIGURE)
            members[name] = role
    return members


def _active(agents: Mapping[str, object], name: str) -> bool:
    agent = agents.get(name, {})
    if not isinstance(agent, dict):
        raise _fail(f"agents.{name}", "must be a JSON object", "inspect the identity registry")
    return agent.get("status") == "active"


def _parent_sender(parent: Mapping[str, object]) -> str:
    """Who sent the message being answered: the only part of it that routing reads."""
    sender = parent.get("from")
    if not isinstance(sender, str):
        raise _fail("parent message", "field 'from' must be a string")
    return sender


def plan(  # noqa: PLR0912  # linear workflow kept whole so failure ordering stays visible
    entry: Mapping[str, object],
    cfg: Mapping[str, object],
    agents: Mapping[str, object],
    parent: Mapping[str, object] | None = None,
) -> Routing:
    """Decide recipients without delivering anything or interpreting task prose."""
    message = load_entry(entry)
    policy = settings_of(cfg)
    if policy is None:
        raise _fail("routing", UNCONFIGURED, CONFIGURE)
    sender = message["from"].lower()
    kind = message_kind(message["text"])
    result: Routing = {
        "role": "untrusted",
        "recipients": [],
        "reason": "",
        "owner_direct": False,
        "actionable": kind in KNOWN_ACTIONABLE,
    }
    if not message.get("verified") or not message.get("msgid"):
        result["reason"] = "Authenticated account and server message ID required for automation"
        return result
    if message["channel"].lower() != policy["channel"].lower():
        result["reason"] = "Codekarl automation uses #codekarl only"
        return result
    if sender == "chatbridge" and message["text"].startswith(BRIDGE_STATUS):
        # The bridge's own service-status lines: recorded, never routed, interrupting or trusted.
        result.update(
            {
                "role": "bridge",
                "reason": "",
                "actionable": False,
                "interrupt": False,
                "notify_owner": False,
            }
        )
        return result
    people = set(policy["owners"]) | set(policy["assistants"])
    manager = policy["manager"]
    members = _roles(cfg)
    if sender in policy["owners"]:
        result["role"] = "owner"
    elif sender in policy["assistants"]:
        result["role"] = "assistant"
    elif sender in members and _active(agents, sender):
        result["role"] = members[sender]
    else:
        result["reason"] = "Sender has no active Codekarl role"
        return result
    named = addresses(message["text"]) - {sender}
    known = set(members) | people | {manager, "chatbridge"}
    unknown = named - known
    if unknown:
        result["reason"] = "Unknown or out-of-project recipient: " + ", ".join(sorted(unknown))
    interrupt = message["text"].lstrip().startswith("!!") or kind == "interrupt"
    result["actionable"] = result["actionable"] or interrupt
    if interrupt and sender not in people | {manager}:
        result["reason"] = "Only the owner, laz, or ck-manager may interrupt Codekarl agents"
        return result
    targets = named & (set(members) | {manager})
    if not named and kind not in QUIET:
        parent_sender = _parent_sender(parent) if parent else None
        if parent_sender is not None and parent_sender in members and parent_sender != sender:
            targets.add(parent_sender)
        elif sender in people or kind in {"question", "blocked", "done", "declined"}:
            targets.add(manager)
    if named & people and sender not in people and kind in {"question", "blocked"}:
        targets.add(manager)
    targets.discard(sender)
    if kind == "ack":
        targets.clear()
    result["recipients"] = sorted(targets)
    result["interrupt"] = interrupt
    result["owner_direct"] = result["role"] == "owner" and bool(targets - {manager})
    result["notify_owner"] = bool(
        named & set(policy["owners"]) and sender not in people and kind in {"question", "blocked"}
    )
    return result


class Inbox:
    """Transactional message/recipient records survive wake-up retries and restarts."""

    def __init__(self, state: str | Path) -> None:
        self.path = Path(state) / "codekarl" / "inbox.sqlite3"
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        with self.connect() as db:
            db.executescript("""
                CREATE TABLE IF NOT EXISTS messages (
                    id TEXT PRIMARY KEY, payload TEXT NOT NULL, routing TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS inbox (
                    message TEXT NOT NULL, recipient TEXT NOT NULL,
                    state TEXT NOT NULL DEFAULT 'queued', notice TEXT NOT NULL DEFAULT 'pending',
                    updated REAL NOT NULL, PRIMARY KEY (message, recipient));
                CREATE TABLE IF NOT EXISTS responses (
                    event TEXT NOT NULL, message TEXT NOT NULL, sender TEXT NOT NULL,
                    state TEXT NOT NULL, at REAL NOT NULL, PRIMARY KEY (event, sender));
                CREATE TABLE IF NOT EXISTS events (
                    id INTEGER PRIMARY KEY, message TEXT NOT NULL, recipient TEXT NOT NULL,
                    state TEXT NOT NULL, at REAL NOT NULL, detail TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS observations (
                    id INTEGER PRIMARY KEY, message TEXT NOT NULL, recipient TEXT NOT NULL,
                    kind TEXT NOT NULL, ok INTEGER, detail TEXT NOT NULL, attempt INTEGER,
                    reminders INTEGER, transport_id TEXT, at REAL NOT NULL);
                CREATE TABLE IF NOT EXISTS ack_effects (
                    message TEXT NOT NULL, recipient TEXT NOT NULL, effect TEXT NOT NULL,
                    state TEXT NOT NULL, detail TEXT NOT NULL DEFAULT '',
                    token TEXT NOT NULL DEFAULT '', holder TEXT NOT NULL DEFAULT '',
                    at REAL NOT NULL, PRIMARY KEY (message, recipient, effect));
            """)
        self.path.chmod(0o600)

    @contextlib.contextmanager
    def connect(self) -> Iterator[sqlite3.Connection]:
        db = sqlite3.connect(self.path, timeout=10)
        db.row_factory = sqlite3.Row
        try:
            with db:
                yield db
        finally:
            db.close()

    def ingest(
        self, entry: Mapping[str, object], cfg: Mapping[str, object], agents: Mapping[str, object]
    ) -> Routing:
        message = load_entry(entry)
        mid = (
            message.get("msgid")
            or "unidentified:"
            + hashlib.sha256(json.dumps(entry, sort_keys=True).encode()).hexdigest()
        )
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            existing = db.execute("SELECT routing FROM messages WHERE id=?", (mid,)).fetchone()
            if existing:
                return load_routing(
                    _stored(existing["routing"], f"message {mid} routing"), f"message {mid} routing"
                )
            parent = db.execute(
                "SELECT payload FROM messages WHERE id=?", (message.get("reply_to"),)
            ).fetchone()
            routing = plan(
                message,
                cfg,
                agents,
                _object(_stored(parent["payload"], "parent payload"), "parent payload")
                if parent
                else None,
            )
            db.execute(
                "INSERT INTO messages VALUES (?, ?, ?)",
                (mid, json.dumps(entry), json.dumps(routing)),
            )
            for recipient in routing["recipients"]:
                db.execute(
                    "INSERT INTO inbox(message, recipient, updated) VALUES (?, ?, ?)",
                    (mid, recipient, time.time()),
                )
                # A replay can deliver the reply before the original request.
                for response in db.execute(
                    "SELECT * FROM responses WHERE message=? AND sender=? ORDER BY at",
                    (mid, recipient),
                ).fetchall():
                    self._advance(
                        db,
                        mid,
                        recipient,
                        _cell_text(response, "state", f"response to {mid}"),
                        "recovered response",
                    )
            reply_to = message.get("reply_to")
            if message.get("verified") and reply_to:
                kind = message_kind(message["text"])
                state = {"done": "completed", "declined": "declined", "answer": "answered"}.get(
                    kind, "acknowledged"
                )
                self._respond(db, mid, reply_to, message["from"].lower(), state)
            return routing

    @staticmethod
    def _advance(db: sqlite3.Connection, mid: str, recipient: str, state: str, detail: str) -> bool:
        if state == "answered":
            original = db.execute("SELECT payload FROM messages WHERE id=?", (mid,)).fetchone()
            question = (
                original
                and message_kind(
                    load_entry(_stored(original["payload"], f"message {mid} payload"))["text"]
                )
                == "question"
            )
            state = "completed" if question else "acknowledged"
        row = db.execute(
            "SELECT state FROM inbox WHERE message=? AND recipient=?", (mid, recipient)
        ).fetchone()
        if not row:
            return False
        current = _cell_text(row, "state", f"inbox {mid} for {recipient}")
        if current in TERMINAL:
            return False
        if state not in RANK or current not in RANK:
            raise _fail(f"inbox {mid} for {recipient}", f"unknown state {state!r} or {current!r}")
        if RANK[state] <= RANK[current]:
            return False
        at = time.time()
        db.execute(
            "UPDATE inbox SET state=?, updated=? WHERE message=? AND recipient=?",
            (state, at, mid, recipient),
        )
        db.execute(
            "INSERT INTO events(message,recipient,state,at,detail) VALUES (?,?,?,?,?)",
            (mid, recipient, state, at, detail),
        )
        return True

    def _respond(
        self, db: sqlite3.Connection, event: str, mid: str, sender: str, state: str
    ) -> None:
        db.execute(
            "INSERT OR IGNORE INTO responses VALUES (?,?,?,?,?)",
            (event, mid, sender, state, time.time()),
        )
        self._advance(db, mid, sender, state, f"IRC response {event}")

    def acknowledge(self, event: str | None, mid: str, sender: str) -> None:
        with self.connect() as db:
            self._respond(db, event or f"ack:{mid}:{sender}", mid, sender.lower(), "acknowledged")

    def claim(self, mid: str, recipient: str) -> Claim:
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            row = db.execute(
                "SELECT inbox.state, messages.payload, messages.routing "
                "FROM inbox JOIN messages ON inbox.message = messages.id "
                "WHERE inbox.message=? AND inbox.recipient=?",
                (mid, recipient),
            ).fetchone()
            if not row:
                raise ValueError("Message is not addressed to this identity; inspect ck-chat inbox")
            fresh = self._advance(
                db, mid, recipient, "acknowledged", "claimed locally before execution"
            )
            if fresh:
                # In the claim's own transaction: the claim and its effect rows exist together.
                self._open_effects(
                    db,
                    mid,
                    recipient,
                    load_entry(_stored(row["payload"], f"message {mid} payload")),
                    load_routing(_stored(row["routing"], f"message {mid} routing")),
                )
            return {
                "fresh": fresh,
                "previous": _cell_text(row, "state", f"inbox {mid} for {recipient}"),
                "message": mid,
                "recipient": recipient,
            }

    @staticmethod
    def _open_effects(
        db: sqlite3.Connection, mid: str, recipient: str, payload: Entry, routing: Routing
    ) -> None:
        """Record the acknowledgements a fresh claim owes: always the reaction, and a visible
        line when the message is actionable. Both start `pending`: definitely not attempted."""
        actionable = (
            routing["actionable"]
            or routing.get("interrupt", False)
            or message_kind(payload["text"]) in KNOWN_ACTIONABLE
        )
        for effect in EFFECTS if actionable else EFFECTS[:1]:
            db.execute(
                "INSERT INTO ack_effects(message,recipient,effect,state,at) VALUES (?,?,?,?,?)",
                (mid, recipient, effect, "pending", time.time()),
            )

    def effects(self, mid: str, recipient: str) -> dict[str, EffectRow]:
        """The stored effect rows, in effect order. Nothing outside this table decides a state."""
        with self.connect() as db:
            rows = db.execute(
                "SELECT * FROM ack_effects WHERE message=? AND recipient=?", (mid, recipient)
            ).fetchall()
        found: dict[str, EffectRow] = {}
        for row in rows:
            record = cast(
                EffectRow, _columns(row, f"acknowledgement effect of {mid}", EFFECT_COLUMNS)
            )  # every declared column was checked
            found[record["effect"]] = record
        return {effect: found[effect] for effect in EFFECTS if effect in found}

    def acquire(self, mid: str, recipient: str, effect: str, holder: str) -> str | None:
        """Atomically take an effect for one attempt: a token, or None when someone else holds it
        or it is not eligible. Committed before the caller touches the transport."""
        token = uuid.uuid4().hex
        with self.connect() as db:
            changed = db.execute(
                "UPDATE ack_effects SET state='attempting', token=?, holder=?, at=? "
                "WHERE message=? AND recipient=? AND effect=? AND state IN ('pending','not-sent')",
                (token, holder, time.time(), mid, recipient, effect),
            ).rowcount
        return token if changed == 1 else None

    def record_effect(  # noqa: PLR0913  # explicit arguments; a parameter object would obscure the call sites
        self, mid: str, recipient: str, effect: str, token: str, state: str, detail: str
    ) -> bool:
        """Store an attempt's result only if that same attempt still holds the effect."""
        with self.connect() as db:
            changed = db.execute(
                "UPDATE ack_effects SET state=?, detail=?, at=? "
                "WHERE message=? AND recipient=? AND effect=? AND state='attempting' AND token=?",
                (state, detail, time.time(), mid, recipient, effect, token),
            ).rowcount
        return changed == 1

    def notices(self, cfg: Mapping[str, object]) -> list[Notice]:
        project = settings_of(cfg)
        if project is None or not project.get("delivery_enabled", False):
            return []
        with self.connect() as db:
            rows = db.execute("""SELECT inbox.*, messages.payload, messages.routing
                FROM inbox JOIN messages ON inbox.message = messages.id
                WHERE inbox.state IN ('queued','delivered') AND inbox.notice != 'failed'
                AND NOT EXISTS (SELECT 1 FROM observations o WHERE o.message=inbox.message
                    AND o.recipient=inbox.recipient AND o.kind='failed')""").fetchall()
        notices: list[Notice] = []
        for row in rows:
            record = _columns(row, "inbox row", INBOX_COLUMNS, trailing=2)
            mid = str(record["message"])  # a declared column: checked as text above
            payload_text, routing_text = _trailing(row)
            entry = load_entry(_stored(payload_text, f"message {mid} payload"))
            routing = load_routing(_stored(routing_text, f"message {mid} routing"))
            delivered = record["state"] == "delivered"
            updated = cast(float, record["updated"])  # checked as a number above
            notices.append(
                {
                    "kind": "ring",
                    "msgid": mid,
                    "channel": entry["channel"],
                    "sender": entry["from"],
                    "text": entry["text"],
                    "project": "codekarl",
                    "target": str(record["recipient"]),  # a declared column: checked as text
                    "attempt": 0,
                    "phase": "accept" if delivered else "deliver",
                    "next_at": updated + 900 if delivered else 0,
                    "needs_ack": True,
                    "interrupt": routing.get("interrupt", False),
                    "ck_policy": POLICY,
                    "authority": routing["role"],
                    "owner_direct": routing["owner_direct"],
                }
            )
        return notices

    def needs_delivery(self, mid: str, recipient: str) -> bool:
        with self.connect() as db:
            row = db.execute(
                "SELECT state FROM inbox WHERE message=? AND recipient=?", (mid, recipient)
            ).fetchone()
            return bool(row) and _cell_text(row, "state", f"inbox {mid}") in {"queued", "delivered"}

    def outcome(self, item: Mapping[str, object], ok: bool | None, detail: object) -> None:
        """Record one delivery observation; task state only ever advances, never reopens."""
        if not item.get("ck_policy"):
            return
        mid, recipient = _delivery_text(item, "msgid"), _delivery_text(item, "target")
        text = str(detail)
        kind = observation_kind(ok, text)
        at = time.time()
        attempt, reminders = item.get("attempt"), item.get("reminders")
        transport = item.get("agent_msg_id")
        if not (
            _int_or_none(attempt) and _int_or_none(reminders) and _text_or_int_or_none(transport)
        ):
            raise _fail(
                f"delivery item {mid}",
                "attempt and reminders must be integers and agent_msg_id text or an integer",
                "inspect the bridge delivery state with: ck-chat status",
            )
        with self.connect() as db:
            db.execute("BEGIN IMMEDIATE")
            db.execute(
                "INSERT INTO observations(message,recipient,kind,ok,detail,attempt,reminders,"
                "transport_id,at) VALUES (?,?,?,?,?,?,?,?,?)",
                (
                    mid,
                    recipient,
                    kind,
                    None if ok is None else int(bool(ok)),
                    text,
                    attempt,
                    reminders,
                    transport,
                    at,
                ),
            )
            if kind == "delivered":
                self._advance(db, mid, recipient, "delivered", text)
            elif kind == "failed":
                db.execute(
                    "INSERT INTO events(message,recipient,state,at,detail) VALUES (?,?,?,?,?)",
                    (mid, recipient, "needs-attention", at, text),
                )
            self._summarize(db, mid, recipient)

    @staticmethod
    def _summarize(db: sqlite3.Connection, mid: str, recipient: str) -> None:
        """Recompute notice from the row's whole observation set, so it stays true as evidence grows."""
        kinds = [
            _cell_text(r, "kind", f"observation of {mid}")
            for r in db.execute(
                "SELECT kind FROM observations WHERE message=? AND recipient=? ORDER BY id",
                (mid, recipient),
            )
        ]
        current = db.execute(
            "SELECT notice FROM inbox WHERE message=? AND recipient=?", (mid, recipient)
        ).fetchone()
        if "failed" in kinds or (
            current and _cell_text(current, "notice", f"inbox {mid}") == "failed"
        ):
            # The second case is a dead letter recorded before observations existed.
            notice = "failed"
        elif "delivered" in kinds:
            notice = "delivered"
        elif "accepted" in kinds:
            notice = "accepted"
        else:
            notice = next(
                (k for k in reversed(kinds) if k in {"attempt-failed", "waiting"}), "pending"
            )
        db.execute(
            "UPDATE inbox SET notice=? WHERE message=? AND recipient=?", (notice, mid, recipient)
        )

    def rows(self, recipient: str | None = None, history: bool = False) -> list[InboxItem]:
        with self.connect() as db:
            rows = db.execute("""SELECT inbox.*, messages.payload, messages.routing
                FROM inbox JOIN messages ON inbox.message = messages.id
                ORDER BY inbox.updated, inbox.message, inbox.recipient""").fetchall()
            observed: dict[tuple[str, str], list[Observation]] = {}
            if history:
                for item in db.execute(
                    "SELECT message, recipient, kind, ok, detail, at FROM observations ORDER BY id"
                ):
                    seen = _columns(item, "observation", OBSERVATION_COLUMNS)
                    key = (
                        _cell_text(item, "message", "observation"),
                        _cell_text(item, "recipient", "observation"),
                    )
                    flag = seen["ok"]
                    observed.setdefault(key, []).append(
                        {
                            "kind": _cell_text(item, "kind", "observation"),
                            "ok": None if flag is None else bool(flag),
                            "detail": _cell_text(item, "detail", "observation"),
                            "at": cast(float, seen["at"]),  # checked as a number above
                        }
                    )
        result: list[InboxItem] = []
        for row in rows:
            record = _columns(row, "inbox row", INBOX_COLUMNS, trailing=2)
            for computed in ("payload", "routing", "observations"):
                if computed in record:  # these are built here; a column must not stand in for one
                    raise _fail(
                        "inbox row",
                        f"the inbox table has a column named {computed!r}, which would be "
                        "returned in place of the value ck-chat computes",
                        f"rename that column (ALTER TABLE inbox RENAME COLUMN {computed} TO "
                        "<name>); the messages and the delivery history are the messages and "
                        "observations tables",
                    )
            payload_text, routing_text = _trailing(row)
            if recipient is not None and record["recipient"] != recipient:
                continue
            label = f"message {record['message']}"
            payload = load_entry(_stored(payload_text, f"{label} payload"), f"{label} payload")
            routing = load_routing(_stored(routing_text, f"{label} routing"), f"{label} routing")
            outstanding = record["state"] not in TERMINAL and (
                record["state"] != "acknowledged"
                or routing["actionable"]
                # Rows stored before unknown kinds became requests carry a stale flag.
                or message_kind(payload["text"]) in KNOWN_ACTIONABLE
            )
            if history or outstanding:
                record["payload"] = payload
                record["routing"] = routing
                item = cast(InboxItem, record)  # declared columns checked; payload, routing parsed
                if history:
                    item["observations"] = observed.get(
                        (str(record["message"]), str(record["recipient"])), []
                    )
                result.append(item)
        return result

    def diagnostics(self) -> list[Diagnostic]:
        with self.connect() as db:
            rows = db.execute(
                "SELECT id, payload, routing FROM messages ORDER BY rowid DESC LIMIT 50"
            ).fetchall()
        found: list[Diagnostic] = []
        for row in rows:
            record = _columns(row, "message row", {"id": STR})
            label = f"message {record['id']}"
            routing = load_routing(_stored(row["routing"], f"{label} routing"), f"{label} routing")
            if routing["reason"]:
                record["payload"] = load_entry(
                    _stored(row["payload"], f"{label} payload"), f"{label} payload"
                )
                record["routing"] = routing
                found.append(cast(Diagnostic, record))  # id checked; payload and routing parsed
        return found


def _delivery_text(item: Mapping[str, object], key: str) -> str:
    value = item.get(key)
    if not isinstance(value, str):
        raise _fail(
            f"delivery item field {key!r}",
            "must be text",
            "inspect the bridge delivery state with: ck-chat status",
        )
    return value
