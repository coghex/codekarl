"""Typed on-disk records of the review gate: one validating loader per record kind (S7a).

Every loader takes a parsed JSON value and a label naming where it came from, checks the required
fields and their types, and returns a `TypedDict`. The returned dict is the parsed object itself, so
unknown additive fields are preserved untouched. A value that does not fit raises `RecordError`,
which names the field and the next action: it never guesses a shape and never suggests deleting
anything. Shapes are never falsified to satisfy the type checker: every valid on-disk form loads.

`JsonObject` is the one place `Any` enters: it is what a parsed JSON object is before a loader has
looked at it. Nothing else in this module is typed `Any`, and the standard library only is imported.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Literal, NamedTuple, NotRequired, TypedDict, TypeGuard, cast

JsonObject = dict[str, Any]  # a parsed JSON object, before a loader has validated it


class RecordError(RuntimeError):
    """A record on disk does not have the shape its kind requires."""


# --- field checks -------------------------------------------------------------------------------


def as_object(raw: object, where: str) -> JsonObject:
    """`raw` as a JSON object, or a RecordError naming `where`."""
    if not isinstance(raw, dict):
        raise RecordError(
            f"{where}: expected a JSON object, found {type(raw).__name__}; "
            "inspect the file, and restore it from its history if it was damaged"
        )
    return raw


def _missing(where: str, key: str, expected: str) -> RecordError:
    return RecordError(
        f"{where}: field `{key}` {expected}; inspect the record (do not delete it) and restore "
        "or repair it from its history"
    )


def _text(obj: JsonObject, key: str, where: str) -> str:
    value = obj.get(key)
    if not isinstance(value, str):
        raise _missing(where, key, "must be a string")
    return value


def _optional_text(obj: JsonObject, key: str, where: str) -> str | None:
    value = obj.get(key)
    if value is not None and not isinstance(value, str):
        raise _missing(where, key, "must be a string or null")
    return value


def _nullable_text(obj: JsonObject, key: str, where: str) -> str | None:
    """A key that must be present, holding a string or null."""
    if key not in obj:
        raise _missing(where, key, "is required (a string or null)")
    return _optional_text(obj, key, where)


def _is_version(value: object, expected: int) -> bool:
    """True only for the integer `expected`: not True, 1.0 or 2.0, which compare equal to integers."""
    return type(value) is int and value == expected


def _integer(obj: JsonObject, key: str, where: str) -> int:
    value = obj.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        raise _missing(where, key, "must be an integer")
    return value


def _texts(obj: JsonObject, key: str, where: str) -> list[str]:
    value = obj.get(key)
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise _missing(where, key, "must be a list of strings")
    return cast("list[str]", value)  # every item was just checked to be a string


def _mapping(obj: JsonObject, key: str, where: str) -> JsonObject:
    value = obj.get(key)
    if not isinstance(value, dict):
        raise _missing(where, key, "must be a JSON object")
    return value


def _objects(obj: JsonObject, key: str, where: str) -> list[JsonObject]:
    value = obj.get(key)
    if not isinstance(value, list) or not all(isinstance(item, dict) for item in value):
        raise _missing(where, key, "must be a list of JSON objects")
    return cast("list[JsonObject]", value)  # every item was just checked to be an object


def _model_uses(obj: JsonObject, key: str, where: str) -> list[StoredModelUse]:
    uses = _objects(obj, key, where)
    for index, item in enumerate(uses):
        item_where = f"{where}: {key}[{index}]"
        _text(item, "model", item_where)
        _text(item, "effort", item_where)
        if "turns" in item and _integer(item, "turns", item_where) < 1:
            raise _missing(item_where, "turns", "must be at least 1")
    return cast(
        "list[StoredModelUse]", uses
    )  # each entry has required text and optional positive turns


def _evidence(obj: JsonObject, key: str, where: str) -> Evidence:
    evidence = _mapping(obj, key, where)
    _text(evidence, "path", f"{where}: {key}")
    _text(evidence, "sha256", f"{where}: {key}")
    _integer(evidence, "offset", f"{where}: {key}")
    return cast("Evidence", evidence)  # path, sha256 and offset were checked above


def read_json(path: Path) -> object:
    """The parsed JSON in `path`, or a RecordError naming the file."""
    try:
        return cast("object", json.loads(path.read_bytes()))
    except (OSError, ValueError) as error:
        raise RecordError(f"{path}: cannot be read as JSON ({error}); inspect the file") from error


# --- fingerprints and authors -------------------------------------------------------------------


class FileEntry(TypedDict):
    sha256: str
    executable: bool


class Fingerprint(TypedDict):
    hashes: list[str]
    files: dict[str, FileEntry]


def load_fingerprint(raw: object, where: str) -> Fingerprint:
    obj = as_object(raw, where)
    _texts(obj, "hashes", where)
    files = _mapping(obj, "files", where)
    for name, entry in files.items():
        entry_where = f"{where}: files[{name!r}]"
        item = as_object(entry, entry_where)
        _text(item, "sha256", entry_where)
        if not isinstance(item.get("executable"), bool):
            raise _missing(entry_where, "executable", "must be a boolean")
    return cast("Fingerprint", obj)  # hashes, files and every entry were checked above


class Author(TypedDict):
    """Who wrote or reviewed: an agent, an `unknown` author for a patch without provenance, or the
    owner (`identity: human`, which has no `families`)."""

    identity: str
    family: str
    families: NotRequired[list[str]]
    hashes: NotRequired[list[str]]
    client: NotRequired[str]
    session: NotRequired[str]
    role: NotRequired[str | None]
    off_class: NotRequired[list[OffClass]]  # written when the history was off the role's class
    models: NotRequired[list[StoredModelUse]]
    evidence: NotRequired[Evidence]


def load_author(raw: object, where: str) -> Author:
    obj = as_object(raw, where)
    _text(obj, "identity", where)
    _text(obj, "family", where)
    for key in ("families", "hashes"):
        if key in obj:
            _texts(obj, key, where)
    for key in ("client", "session"):
        if key in obj:
            _text(obj, key, where)
    if "role" in obj:
        _optional_text(obj, "role", where)
    if "off_class" in obj:
        for index, item in enumerate(_objects(obj, "off_class", where)):
            _text(item, "model", f"{where}: off_class[{index}]")
            _nullable_text(item, "effort", f"{where}: off_class[{index}]")
            _integer(item, "events", f"{where}: off_class[{index}]")
            if "family" in item:
                _text(item, "family", f"{where}: off_class[{index}]")
    if "models" in obj:
        _model_uses(obj, "models", where)
    if "evidence" in obj:
        _evidence(obj, "evidence", where)
    return cast("Author", obj)


def _authors(obj: JsonObject, where: str, *, key: str = "authors") -> list[Author]:
    return [
        load_author(item, f"{where}: {key}[{index}]")
        for index, item in enumerate(_objects(obj, key, where))
    ]


class IssueRef(TypedDict):
    id: str
    revision: str
    spec_sha256: str


def load_issue_ref(raw: object, where: str) -> IssueRef:
    obj = as_object(raw, where)
    for key in ("id", "revision", "spec_sha256"):
        _text(obj, key, where)
    return cast("IssueRef", obj)


def _issue_refs(obj: JsonObject, key: str, where: str) -> list[IssueRef]:
    return [
        load_issue_ref(item, f"{where}: {key}[{index}]")
        for index, item in enumerate(_objects(obj, key, where))
    ]


# --- the accepted pointer -----------------------------------------------------------------------


class BatchIntegration(TypedDict):
    """The integration a batch publication records: the batch-subject review receipts."""

    batch: str
    receipts: list[str]
    basis: NotRequired[Literal["integration-review"]]  # absent in every pointer before CK-38


class SingleSubmissionTiming(TypedDict):
    """The CI timing evidence a single-submission publication was decided on (CK-38 1.8)."""

    log_sha256: str
    slowest_step: str
    slowest_seconds: float
    margin: float


class SingleSubmissionIntegration(TypedDict):
    """A one-member batch published on the submission review alone (CK-38): no batch receipt."""

    batch: str
    basis: Literal["single-submission"]
    submission: str
    submission_receipts: list[str]
    receipts: list[str]  # always empty: no invented batch receipt
    timing: SingleSubmissionTiming


class BootstrapIntegration(TypedDict):
    """The integration the one-time legacy-to-v2 bootstrap publication records."""

    bootstrap: str
    evidence_sha256: str


Integration = BatchIntegration | SingleSubmissionIntegration | BootstrapIntegration


class PointerV2(TypedDict):
    """`accepted-v2.json`: the authoritative accepted state."""

    schema: Literal[2]
    repo: str
    fingerprint: Fingerprint
    submissions: list[str]
    receipt: NotRequired[str | None]
    previous: NotRequired[str | None]
    superseded_legacy: NotRequired[list[str]]
    integration: NotRequired[Integration]
    _pointer_path: NotRequired[str]  # added by `read_pointer` (the file it read); never stored


class PointerSchema1(TypedDict):
    """The accepted state before schema 2 (display and audit only): `schema` is absent or 1."""

    repo: str
    fingerprint: Fingerprint
    submissions: list[str]
    schema: NotRequired[Literal[1]]
    receipt: NotRequired[str | None]
    previous: NotRequired[str | None]
    _pointer_path: NotRequired[str]  # added by `read_pointer` (the file it read); never stored


class PointerHold(TypedDict):
    """The pointer while a bootstrap runs: state commands are paused."""

    schema: Literal["bootstrap-hold"]
    bootstrap: str
    repo: str
    fingerprint: Fingerprint
    submissions: NotRequired[list[str]]
    receipt: NotRequired[str | None]
    previous: NotRequired[str | None]


class PointerTombstone(TypedDict):
    """`accepted.json` once `accepted-v2.json` exists, and the pointer when activation is incomplete."""

    schema: Literal["tombstone"]
    repo: str
    fingerprint: Fingerprint
    upgrade: NotRequired[str]
    submissions: NotRequired[list[str]]
    receipt: NotRequired[str | None]
    previous: NotRequired[str | None]


PointerRecord = PointerV2 | PointerSchema1 | PointerHold | PointerTombstone
AcceptedPointer = PointerV2 | PointerSchema1  # the two forms that name an accepted checkpoint


def _pointer_common(obj: JsonObject, where: str, *, submissions_required: bool) -> None:
    _text(obj, "repo", where)
    load_fingerprint(obj.get("fingerprint"), f"{where}: fingerprint")
    if submissions_required or "submissions" in obj:
        _texts(obj, "submissions", where)
    for key in ("receipt", "previous"):
        if key in obj:
            _optional_text(obj, key, where)
    # only ever added by `read_pointer`, but it is declared, so it is checked
    if "_pointer_path" in obj:
        _text(obj, "_pointer_path", where)


def _integration(integration: JsonObject, where: str) -> None:
    """Check one of the three integration forms: bootstrap, batch (with or without its basis) and
    the single-submission basis."""
    if "bootstrap" in integration:
        _text(integration, "bootstrap", where)
        _text(integration, "evidence_sha256", where)
        return
    _text(integration, "batch", where)
    _texts(integration, "receipts", where)
    if "basis" not in integration:
        return  # every integration before CK-38
    basis = integration["basis"]
    if basis == "integration-review":
        return
    if basis != "single-submission":
        raise _missing(where, "basis", "must be `integration-review` or `single-submission`")
    _text(integration, "submission", where)
    _texts(integration, "submission_receipts", where)
    timing = _mapping(integration, "timing", where)
    _text(timing, "log_sha256", f"{where}: timing")
    _text(timing, "slowest_step", f"{where}: timing")
    _number(timing, "slowest_seconds", f"{where}: timing")
    _number(timing, "margin", f"{where}: timing")


def load_pointer(raw: object, where: str) -> PointerRecord:
    """Any pointer form: schema 2, schema 1, the bootstrap hold or the tombstone."""
    obj = as_object(raw, where)
    schema = obj.get("schema")
    if _is_version(schema, 2):  # the pointer schema version
        _pointer_common(obj, where, submissions_required=True)
        if "superseded_legacy" in obj:
            _texts(obj, "superseded_legacy", where)
        if "integration" in obj:
            _integration(_mapping(obj, "integration", where), f"{where}: integration")
        return cast("PointerV2", obj)
    if schema == "bootstrap-hold":
        _pointer_common(obj, where, submissions_required=False)
        _text(obj, "bootstrap", where)
        return cast("PointerHold", obj)
    if schema == "tombstone":
        _pointer_common(obj, where, submissions_required=False)
        if "upgrade" in obj:
            _text(obj, "upgrade", where)
        return cast("PointerTombstone", obj)
    if "schema" not in obj or _is_version(schema, 1):
        _pointer_common(obj, where, submissions_required=True)
        return cast("PointerSchema1", obj)
    raise RecordError(f"{where}: unsupported pointer schema {schema!r}; inspect the file")


# --- submission manifests -----------------------------------------------------------------------


class ManifestBase(TypedDict):
    fingerprint_sha256: str
    pointer_sha256: str


class ManifestLegacy(TypedDict):
    """A manifest from before issues and provenance: no `schema`."""

    id: str
    repo: str
    spec: str
    hashes: list[str]
    fingerprint: Fingerprint


class ManifestV2(TypedDict):
    id: str
    repo: str
    spec: str
    hashes: list[str]
    fingerprint: Fingerprint
    schema: Literal[2]
    files: list[str]
    issues: list[IssueRef]
    provenance: dict[str, str | None]
    authors: list[Author]
    base: ManifestBase
    supersedes: list[str]
    submitter: NotRequired[Author]


Manifest = ManifestV2 | ManifestLegacy


def is_manifest_v2(manifest: Manifest) -> TypeGuard[ManifestV2]:
    return "schema" in manifest


def load_manifest(raw: object, where: str) -> Manifest:
    obj = as_object(raw, where)
    for key in ("id", "repo", "spec"):
        _text(obj, key, where)
    _texts(obj, "hashes", where)
    load_fingerprint(obj.get("fingerprint"), f"{where}: fingerprint")
    if "schema" not in obj:
        return cast("ManifestLegacy", obj)
    if not _is_version(obj["schema"], 2):  # the manifest schema version
        raise RecordError(
            f"{where}: unsupported manifest schema {obj['schema']!r}; inspect the file"
        )
    _texts(obj, "files", where)
    _issue_refs(obj, "issues", where)
    provenance = _mapping(obj, "provenance", where)
    for patch, digest in provenance.items():
        if digest is not None and not isinstance(digest, str):
            raise _missing(where, f"provenance[{patch!r}]", "must be a digest string or null")
    _authors(obj, where)
    base = _mapping(obj, "base", where)
    for key in ("fingerprint_sha256", "pointer_sha256"):
        _text(base, key, f"{where}: base")
    _texts(obj, "supersedes", where)
    if "submitter" in obj:
        load_author(obj["submitter"], f"{where}: submitter")
    return cast("ManifestV2", obj)


# --- review receipts ----------------------------------------------------------------------------


class Evidence(TypedDict):
    path: str
    offset: int
    sha256: str


AiReviewer = TypedDict(  # functional form: `class` is a Python keyword
    "AiReviewer",
    {
        "identity": str,
        "family": str,
        "role": str,
        "client": str,
        "session": str,
        "model": str,
        "effort": str,
        "class": str,
        "evidence": Evidence,
    },
)


ReviewerIdentity = TypedDict(  # an AI reviewer before its session evidence is attached
    "ReviewerIdentity",
    {
        "identity": str,
        "family": str,
        "role": str,
        "client": str,
        "session": str,
        "model": str,
        "effort": str,
        "class": str,
    },
)


class OwnerReviewer(TypedDict):
    identity: str
    family: str
    role: str
    owner_signoff: Literal[True]


class SpecSubject(TypedDict):
    issue: str
    revision: str
    spec_sha256: str


class SubmissionSubject(TypedDict):
    submission: str
    manifest_sha256: str
    fingerprint_sha256: str


class BatchSubject(TypedDict):
    batch: str
    candidate_fingerprint_sha256: str
    ci_receipt_sha256: str
    membership_manifest_sha256: list[str]  # the member manifests' digests, in batch order
    validated_sha256: str


class ReviewReceipt(TypedDict):
    """A `ck-review/1` receipt (an AI review, or the owner's sign-off in the same shape)."""

    schema: Literal["ck-review/1"]
    id: str
    kind: Literal["spec", "submission", "batch"]
    verdict: str
    recorded: int
    report_sha256: str
    issues: list[IssueRef]
    reviewer: AiReviewer | OwnerReviewer
    subject: SpecSubject | SubmissionSubject | BatchSubject
    base_examined: str | None
    incremental_over: str | None


_SUBJECT_KEYS = {
    "spec": ("issue", "revision", "spec_sha256"),
    "submission": ("submission", "manifest_sha256", "fingerprint_sha256"),
    "batch": (
        "batch",
        "candidate_fingerprint_sha256",
        "ci_receipt_sha256",
        "membership_manifest_sha256",
        "validated_sha256",
    ),
}
_BATCH_TEXT_KEYS = (
    "batch",
    "candidate_fingerprint_sha256",
    "ci_receipt_sha256",
    "validated_sha256",
)
_AI_REVIEWER_KEYS = ("identity", "family", "role", "client", "session", "model", "effort", "class")


def _reviewer_identity(reviewer: JsonObject, where: str) -> bool:
    """Check who reviewed (or revoked): the owner's sign-off form or an AI reviewer's identity.

    Returns True for the owner form, which has no session evidence."""
    if reviewer.get("owner_signoff") is True:
        for key in ("identity", "family", "role"):
            _text(reviewer, key, where)
        return True
    for key in _AI_REVIEWER_KEYS:
        _text(reviewer, key, where)
    return False


def load_review_receipt(raw: object, where: str) -> ReviewReceipt:
    obj = as_object(raw, where)
    if obj.get("schema") != "ck-review/1":
        raise _missing(where, "schema", "must be `ck-review/1`")
    for key in ("id", "verdict", "report_sha256"):
        _text(obj, key, where)
    _integer(obj, "recorded", where)
    _issue_refs(obj, "issues", where)
    _nullable_text(obj, "base_examined", where)
    _nullable_text(obj, "incremental_over", where)
    kind = obj.get("kind")
    if not isinstance(kind, str) or kind not in _SUBJECT_KEYS:
        raise _missing(where, "kind", "must be `spec`, `submission` or `batch`")
    subject = _mapping(obj, "subject", where)
    for key in _SUBJECT_KEYS[kind]:
        if key == "membership_manifest_sha256":
            _texts(subject, key, f"{where}: subject")
        else:
            _text(subject, key, f"{where}: subject")
    reviewer = _mapping(obj, "reviewer", where)
    if not _reviewer_identity(reviewer, f"{where}: reviewer"):
        _evidence(reviewer, "evidence", f"{where}: reviewer")
    return cast("ReviewReceipt", obj)


# --- patch provenance and issue events ----------------------------------------------------------


class ProvenancePayload(TypedDict):
    """The `ck-provenance/1` payload in a patch description (its digest is checked elsewhere).

    The first five fields are what coverage relies on and are always validated; the others are
    validated when present."""

    schema: Literal["ck-provenance/1"]
    identity: str
    client: str
    session: str
    families: list[str]
    issues: NotRequired[list[str]]
    role: NotRequired[str | None]
    mapping: NotRequired[dict[str, object]]
    models: NotRequired[list[StoredModelUse]]
    evidence: NotRequired[Evidence]
    recorded: NotRequired[str]


class UnknownProvenance(TypedDict):
    """What `provenance.parse_description` returns for a prefixed description it cannot trust."""

    schema: Literal["unknown"]
    digest: str


def load_provenance(raw: object, where: str) -> ProvenancePayload:
    obj = as_object(raw, where)
    if obj.get("schema") != "ck-provenance/1":
        raise _missing(where, "schema", "must be `ck-provenance/1`")
    for key in ("identity", "client", "session"):
        if not _text(obj, key, where):
            raise _missing(where, key, "must not be empty")
    families = _texts(obj, "families", where)
    if not families:
        raise _missing(where, "families", "must not be empty")
    if "issues" in obj:
        _texts(obj, "issues", where)
    if "role" in obj:
        _optional_text(obj, "role", where)
    if "mapping" in obj:
        _mapping(obj, "mapping", where)
    if "models" in obj:
        _model_uses(obj, "models", where)
    if "evidence" in obj:
        _evidence(obj, "evidence", where)
    if "recorded" in obj:
        _text(obj, "recorded", where)
    return cast("ProvenancePayload", obj)


class IssueFields(TypedDict, total=False):
    """The `fields` of an issue event: whichever of these the event changes."""

    title: str
    type: str
    priority: str
    state: str
    depends: list[str]
    reason: str
    revision: str
    assign: str


class IssueEvent(TypedDict):
    """One file of `issues/CK-N/` in the metadata repository (`created`, `edit`, `spec`, `close`, ...)."""

    id: str
    kind: str
    at: int
    fields: IssueFields
    authors: list[Author]
    spec_sha256: NotRequired[str]


_ISSUE_OWN_KEYS = ("id", "spec_sha256", "spec_path", "spec_authors")  # set by `fold_issue` only


def load_issue_event(raw: object, where: str) -> IssueEvent:
    obj = as_object(raw, where)
    _text(obj, "id", where)
    _text(obj, "kind", where)
    _integer(obj, "at", where)
    fields = _mapping(obj, "fields", where)
    for key in _ISSUE_OWN_KEYS:
        if key in fields:
            raise _missing(where, f"fields.{key}", "is derived from the issue, not set by an event")
    for key in ("title", "type", "priority", "state", "reason", "revision", "assign"):
        if key in fields:
            _text(fields, key, f"{where}: fields")
    if "depends" in fields:
        _texts(fields, "depends", f"{where}: fields")
    _authors(obj, where)
    if "spec_sha256" in obj:
        _text(obj, "spec_sha256", where)
    return cast("IssueEvent", obj)


# --- lock holders and process rows --------------------------------------------------------------


class Holder(TypedDict):
    """The hint a lock's holder writes into the lock file: never proof of ownership."""

    pid: int
    command: NotRequired[str]
    started: NotRequired[float]
    batch: NotRequired[str]


def load_holder(raw: object, where: str) -> Holder:
    obj = as_object(raw, where)
    _integer(obj, "pid", where)
    if "command" in obj:
        _text(obj, "command", where)
    if "batch" in obj:
        _text(obj, "batch", where)
    if "started" in obj and (
        not isinstance(obj["started"], int | float) or isinstance(obj["started"], bool)
    ):
        raise _missing(where, "started", "must be a number")
    return cast("Holder", obj)


def frozen_owner_pid(raw: object) -> int | None:
    """The pid recorded as the owner in a batch's `frozen.json`, or None when it has none."""
    if not isinstance(raw, dict):
        return None
    owner = raw.get("owner")
    if not isinstance(owner, dict):
        return None
    pid = owner.get("pid")
    return pid if isinstance(pid, int) and not isinstance(pid, bool) else None


class ProcessRow(TypedDict):
    """One row of the host's process table, as `ps` lists it."""

    pid: int
    ppid: int
    command: str


# --- the shared model-class master --------------------------------------------------------------


class ModelEffort(TypedDict):
    model: str
    effort: str


class ClassRow(TypedDict, total=False):
    """One model class. A family entry is checked where it is used (`classes.resolve_class`), which
    names the class and family when it lacks a model or effort; here it is only an object."""

    description: str
    route: str
    claude: object
    codex: object


class ClientRow(TypedDict, total=False):
    model_aliases: dict[str, str]


class ModelClasses(TypedDict, total=False):
    """`~/.config/model-classes.toml`: the parts the gate reads (other sections are left alone)."""

    classes: dict[str, ClassRow]
    clients: dict[str, ClientRow]


def load_model_classes(raw: object, where: str) -> ModelClasses:
    obj = as_object(raw, where)
    if "classes" in obj:
        rows = _mapping(obj, "classes", where)
        for name, row in rows.items():
            row_where = f"{where}: classes.{name}"
            item = as_object(row, row_where)
            if "route" in item:
                _text(item, "route", row_where)
            if "description" in item:
                _text(item, "description", row_where)
    if "clients" in obj:
        for name, client in _mapping(obj, "clients", where).items():
            client_where = f"{where}: clients.{name}"
            item = as_object(client, client_where)
            if "model_aliases" in item:
                aliases = _mapping(item, "model_aliases", client_where)
                for alias, target in aliases.items():
                    if not isinstance(target, str):
                        raise _missing(client_where, f"model_aliases.{alias}", "must be a string")
    return cast("ModelClasses", obj)


class ModelUse(TypedDict):
    """One contribution a session log shows: a model at an effort."""

    model: str
    effort: str


class ModelRun(TypedDict):
    """A compact stored run of consecutive model contributions."""

    model: str
    effort: str
    turns: int


StoredModelUse = ModelUse | ModelRun


class OffClass(TypedDict):
    model: str
    effort: str | None
    events: int
    family: NotRequired[str]


RoleMapping = TypedDict(  # functional form: `class` is a Python keyword
    "RoleMapping",
    {
        "class": str,
        "family": str,
        "model": str,
        "effort": str,
        "master_sha256": str,
        "families": list[str],
        "current_family": str,
        "off_class": list[OffClass],
    },
    total=False,
)


# --- bound sessions -----------------------------------------------------------------------------


class RegistryRow(TypedDict, total=False):
    """A row of the chat registry (or of `CK_SESSION_EVIDENCE`); only these fields are read."""

    name: str
    brand: str
    session_id: str
    role: str | None
    pid: int
    pid_start: str


def registry_row(raw: object) -> RegistryRow | None:
    """A registry row with every present field of the right type, or None for any other value
    (such a row can never match this process, so it is skipped, not an error)."""
    if not isinstance(raw, dict):
        return None
    for key in ("name", "brand", "session_id", "pid_start"):
        if key in raw and not isinstance(raw[key], str):
            return None
    if "role" in raw and raw["role"] is not None and not isinstance(raw["role"], str):
        return None
    if "pid" in raw and (not isinstance(raw["pid"], int) or isinstance(raw["pid"], bool)):
        return None
    return cast("RegistryRow", raw)


class AgentSession(TypedDict):
    identity: str
    client: str
    session: str
    role: str | None
    models: list[ModelUse]
    current: ModelUse | None
    families: list[str]
    evidence: Evidence
    pid: NotRequired[int]


class HumanSession(TypedDict):
    human: Literal[True]
    identity: Literal["human"]
    family: Literal["human"]
    role: Literal["owner"]


Session = AgentSession | HumanSession


def is_agent(session: Session) -> TypeGuard[AgentSession]:
    return "human" not in session


# --- issues and the metadata events -------------------------------------------------------------


class Issue(TypedDict):
    """An issue folded from its events (`Store.show`)."""

    id: str
    title: str
    type: str
    priority: str
    state: str
    depends: list[str]
    reason: NotRequired[str]
    assign: NotRequired[str]
    revision: NotRequired[str]
    spec_sha256: NotRequired[str]
    spec_path: NotRequired[str]
    spec_authors: NotRequired[list[Author]]


class SpecRevision(NamedTuple):
    """The newest spec file of an issue: its stem (`r3`), digest and path."""

    revision: str
    sha256: str
    path: str


def _checked_issue(folded: JsonObject) -> Issue:
    """Every declared field of the folded issue, checked before it is called an `Issue`."""
    where = f"issue {folded.get('id')!r}"
    for key in ("id", "title", "type", "priority", "state"):
        _text(folded, key, where)
    _texts(folded, "depends", where)
    for key in ("reason", "assign", "revision", "spec_sha256", "spec_path"):
        if key in folded:
            _text(folded, key, where)
    if "spec_authors" in folded:
        _authors(folded, where, key="spec_authors")
    return cast("Issue", folded)  # each declared field was checked above


def fold_issue(issue_id: str, events: list[IssueEvent], spec: SpecRevision | None) -> Issue:
    """The issue its events describe: defaults, then each event's fields in order, then the spec.

    Fields an event carries beyond the known ones are kept, as they always were."""
    folded: JsonObject = {
        "id": issue_id,
        "title": "",
        "type": "task",
        "priority": "P2",
        "state": "open",
        "depends": [],
    }
    spec_authors: list[Author] = []
    for event in events:
        fields = event.get("fields", {})
        for key in _ISSUE_OWN_KEYS:
            if key in fields:
                raise _missing(
                    f"issue {issue_id}",
                    f"fields.{key}",
                    "is derived from the issue, not set by an event",
                )
        folded.update(fields)
        if event.get("spec_sha256"):
            spec_authors = event.get("authors", [])
    if spec is not None:
        folded["revision"] = spec.revision
        folded["spec_sha256"] = spec.sha256
        folded["spec_path"] = spec.path
        folded["spec_authors"] = spec_authors
    return _checked_issue(folded)


class MetaEvent(TypedDict):
    """A file of `events/` in the metadata repository: `supersession`, `revocation`, `withdrawn`,
    `superseded`, `bootstrap`. Only `kind` is common; the others are validated when present."""

    kind: str
    submission: NotRequired[str]
    supersedes: NotRequired[list[str]]
    by: NotRequired[str]
    review: NotRequired[str]
    reason: NotRequired[str]
    bootstrap: NotRequired[str]
    evidence_sha256: NotRequired[str]
    authors: NotRequired[list[Author]]
    actor: NotRequired[ReviewerIdentity | OwnerReviewer]


def load_meta_event(raw: object, where: str) -> MetaEvent:
    obj = as_object(raw, where)
    _text(obj, "kind", where)
    for key in ("submission", "by", "review", "reason", "bootstrap", "evidence_sha256"):
        if key in obj:
            _text(obj, key, where)
    if "supersedes" in obj:
        _texts(obj, "supersedes", where)
    if "authors" in obj:
        _authors(obj, where)
    if "actor" in obj:
        _reviewer_identity(_mapping(obj, "actor", where), f"{where}: actor")
    return cast("MetaEvent", obj)


# --- review receipts as the gate reads them -----------------------------------------------------

ReviewerRecord = AiReviewer | OwnerReviewer
Subject = SpecSubject | SubmissionSubject | BatchSubject


class ReviewRow(ReviewReceipt):
    """A receipt as `receipt_rows` returns it: the stored receipt plus whether it was revoked."""

    revoked: bool


def review_row(receipt: ReviewReceipt, *, revoked: bool) -> ReviewRow:
    """The receipt, marked revoked or not (the stored file never carries the mark)."""
    row = cast("JsonObject", receipt)
    row["revoked"] = revoked
    return cast("ReviewRow", row)


def is_review_row(receipt: ReviewRow | BootstrapReceipt) -> TypeGuard[ReviewRow]:
    """A `ck-review/1` receipt, as opposed to a bootstrap evidence receipt (its own schema tag)."""
    return receipt.get("schema") == "ck-review/1"


def is_owner_reviewer(reviewer: ReviewerRecord) -> TypeGuard[OwnerReviewer]:
    return reviewer.get("owner_signoff") is True


class ManifestInfo(TypedDict, total=False):
    """What the batch assembles about one manifest for the coverage calculation."""

    subject: Subject
    hashes: list[str]
    authors: list[Author]
    submitter: Author | None
    unreadable: bool


# --- the small configuration and bookkeeping files the migrated modules read --------------------


class GateConfig(TypedDict):
    """`.pipeline/gate.json`: where the metadata repository is."""

    meta_repo: str
    schema: NotRequired[int]


def load_gate_config(raw: object, where: str) -> GateConfig:
    obj = as_object(raw, where)
    _text(obj, "meta_repo", where)
    if "schema" in obj:
        _integer(obj, "schema", where)
    return cast("GateConfig", obj)


class Counters(TypedDict, total=False):
    """`counters.json` in the metadata repository: the last number issued of each kind."""

    issue: int
    event: int
    review: int


def load_counters(raw: object, where: str) -> Counters:
    obj = as_object(raw, where)
    for key in ("issue", "event", "review"):
        if key in obj:
            _integer(obj, key, where)
    return cast("Counters", obj)


def load_session_roots(raw: object, where: str) -> dict[str, str]:
    """`CK_SESSION_ROOTS`: the directory holding each client's session logs."""
    obj = as_object(raw, where)
    for client, root in obj.items():
        if not isinstance(root, str):
            raise _missing(where, client, "must be a directory path (a string)")
    return cast("dict[str, str]", obj)


def patch_descriptions(raw: object, where: str) -> dict[str, str]:
    """`pijul log --description --output-format json`: each patch hash and its description ('' when none)."""
    if not isinstance(raw, list):
        raise RecordError(
            f"{where}: expected a JSON list of patches; run the command by hand to inspect it"
        )
    described: dict[str, str] = {}
    for index, row in enumerate(raw):
        entry = as_object(row, f"{where}[{index}]")
        _text(entry, "hash", f"{where}[{index}]")
        description = entry.get("description")
        if description is not None and not isinstance(description, str):
            raise _missing(f"{where}[{index}]", "description", "must be a string or null")
        described[entry["hash"]] = description or ""
    return described


# --- more field checks --------------------------------------------------------------------------


def _boolean(obj: JsonObject, key: str, where: str) -> bool:
    value = obj.get(key)
    if not isinstance(value, bool):
        raise _missing(where, key, "must be true or false")
    return value


def _number(obj: JsonObject, key: str, where: str) -> float:
    value = obj.get(key)
    if not isinstance(value, int | float) or isinstance(value, bool):
        raise _missing(where, key, "must be a number")
    return value


def _string_map(obj: JsonObject, key: str, where: str) -> dict[str, str]:
    """A JSON object whose values are all strings (for example file name -> digest)."""
    value = _mapping(obj, key, where)
    for name, item in value.items():
        if not isinstance(item, str):
            raise _missing(where, f"{key}.{name}", "must be a string")
    return cast("dict[str, str]", value)  # every value was just checked to be a string


def _list(obj: JsonObject, key: str, where: str) -> list[object]:
    """A JSON list whose items the caller checks (for example Pijul's conflict reports)."""
    value = obj.get(key)
    if not isinstance(value, list):
        raise _missing(where, key, "must be a list")
    return cast("list[object]", value)


# --- shared chat inputs and CI receipts ---------------------------------------------------------


class ChatInputs(TypedDict):
    """The shared chat inputs a CI run tested (`<log>.chat-inputs.json`, and a receipt's binding).

    Whether the digests are real and the aggregate is right is `inputs.valid_record`'s check; this
    is the shape."""

    schema: Literal[1]
    files: dict[str, str]
    aggregate: str


def load_chat_inputs(raw: object, where: str) -> ChatInputs:
    obj = as_object(raw, where)
    if not _is_version(obj.get("schema"), 1):
        raise _missing(where, "schema", "must be the integer 1")
    _string_map(obj, "files", where)
    _text(obj, "aggregate", where)
    return cast("ChatInputs", obj)


class BatchCIReceipt(TypedDict):
    """`.pipeline/ci/<batch>.json`: one batch's CI run (passed or failed)."""

    kind: Literal["ci"]
    passed: bool
    fingerprint: Fingerprint
    submissions: list[str]
    failed: list[str]
    logs: list[str]
    recorded: int
    chat_inputs: NotRequired[ChatInputs]
    reason: NotRequired[str]


class BootstrapCIReceipt(TypedDict):
    """`.pipeline/ci/bootstrap-<id>.json`: the bootstrap candidate's CI run."""

    kind: Literal["ci"]
    passed: bool
    fingerprint: Fingerprint
    submissions: list[str]
    bootstrap: str
    log: str


class LegacyCIReceipt(TypedDict):
    """`.pipeline/checkpoints/<id>.json` from the pre-gate controller; a failed run has no `kind`."""

    submissions: list[str]
    repo: str
    started: float
    passed: bool
    finished: NotRequired[float]
    kind: NotRequired[Literal["ci"]]
    fingerprint: NotRequired[Fingerprint]
    error: NotRequired[str]


CIReceipt = BatchCIReceipt | BootstrapCIReceipt | LegacyCIReceipt


def is_batch_ci_receipt(receipt: CIReceipt) -> TypeGuard[BatchCIReceipt]:
    """The batch form: `logs` and not `bootstrap` (a record with both tags is not loaded at all)."""
    return "logs" in receipt and "bootstrap" not in receipt


def is_bootstrap_ci_receipt(receipt: CIReceipt) -> TypeGuard[BootstrapCIReceipt]:
    return "bootstrap" in receipt and "logs" not in receipt


def load_ci_receipt(raw: object, where: str) -> CIReceipt:
    obj = as_object(raw, where)
    _boolean(obj, "passed", where)
    _texts(obj, "submissions", where)
    if "kind" in obj and obj["kind"] != "ci":
        raise _missing(where, "kind", "must be `ci` (or absent in a legacy failed report)")
    if "bootstrap" in obj and "logs" in obj:
        raise _missing(
            where,
            "logs",
            "must not accompany `bootstrap`: a CI receipt is either a batch's or the bootstrap's",
        )
    if "fingerprint" in obj or "bootstrap" in obj or "logs" in obj:
        load_fingerprint(obj.get("fingerprint"), f"{where}: fingerprint")
    if "bootstrap" in obj:
        _text(obj, "bootstrap", where)
        _text(obj, "log", where)
        if obj.get("kind") != "ci":
            raise _missing(where, "kind", "must be `ci`")
        return cast("BootstrapCIReceipt", obj)
    if "logs" in obj:
        _texts(obj, "failed", where)
        _texts(obj, "logs", where)
        _integer(obj, "recorded", where)
        if obj.get("kind") != "ci":
            raise _missing(where, "kind", "must be `ci`")
        if "chat_inputs" in obj:
            load_chat_inputs(obj["chat_inputs"], f"{where}: chat_inputs")
        if "reason" in obj:
            _text(obj, "reason", where)
        return cast("BatchCIReceipt", obj)
    _text(obj, "repo", where)
    _number(obj, "started", where)
    if "finished" in obj:
        _number(obj, "finished", where)
    if "error" in obj:
        _text(obj, "error", where)
    return cast("LegacyCIReceipt", obj)


class DocsRestoreReceipt(TypedDict):
    """A docs receipt the bootstrap restore writes beside a restore checkpoint."""

    kind: Literal["docs"]
    passed: bool
    inherits: str | None


class DocsPublishReceipt(TypedDict):
    """`.pipeline/docs/receipts/<id>.json`: one published documentation entry."""

    kind: Literal["docs"]
    id: str
    fingerprint: Fingerprint
    hashes: list[str]
    previous: str | None


DocsReceipt = DocsRestoreReceipt | DocsPublishReceipt


def load_docs_receipt(raw: object, where: str) -> DocsReceipt:
    obj = as_object(raw, where)
    if obj.get("kind") != "docs":
        raise _missing(where, "kind", "must be `docs`")
    if "inherits" in obj and "id" in obj:
        raise _missing(
            where,
            "id",
            "must not accompany `inherits`: a docs receipt is either a restore's or a publication's",
        )
    if "inherits" in obj:
        _boolean(obj, "passed", where)
        _nullable_text(obj, "inherits", where)
        return cast("DocsRestoreReceipt", obj)
    _text(obj, "id", where)
    load_fingerprint(obj.get("fingerprint"), f"{where}: fingerprint")
    _texts(obj, "hashes", where)
    _nullable_text(obj, "previous", where)
    return cast("DocsPublishReceipt", obj)


# --- batch records: `.pipeline/batches/<id>/*.json` ---------------------------------------------


class BatchOwner(TypedDict):
    """The process that froze the batch (a hint for recovery; never proof of ownership)."""

    pid: int
    pid_start: int | str


def _owner(obj: JsonObject, where: str) -> BatchOwner:
    owner = _mapping(obj, "owner", where)
    _integer(owner, "pid", f"{where}: owner")
    start = owner.get("pid_start")
    if not (isinstance(start, str) or (isinstance(start, int) and not isinstance(start, bool))):
        raise _missing(f"{where}: owner", "pid_start", "must be a string or an integer")
    return cast("BatchOwner", owner)


class BatchMember(TypedDict):
    id: str
    manifest_sha256: str
    fingerprint_sha256: str


class BatchFrozen(TypedDict):
    batch: str
    base_pointer_sha256: str
    members: list[BatchMember]
    queue_snapshot: dict[str, str]
    authors: list[Author]
    owner: BatchOwner


def load_batch_frozen(raw: object, where: str) -> BatchFrozen:
    obj = as_object(raw, where)
    _text(obj, "batch", where)
    _text(obj, "base_pointer_sha256", where)
    for index, member in enumerate(_objects(obj, "members", where)):
        for key in ("id", "manifest_sha256", "fingerprint_sha256"):
            _text(member, key, f"{where}: members[{index}]")
    _string_map(obj, "queue_snapshot", where)
    _authors(obj, where)
    _owner(obj, where)
    return cast("BatchFrozen", obj)


class BatchValidating(TypedDict):
    at: int


def load_batch_validating(raw: object, where: str) -> BatchValidating:
    obj = as_object(raw, where)
    _integer(obj, "at", where)
    return cast("BatchValidating", obj)


class BatchValidated(TypedDict):
    candidate: str
    candidate_fingerprint_sha256: str
    ci_receipt: str
    ci_receipt_sha256: str
    members: list[str]
    membership_manifest_sha256: list[str]
    base_pointer_sha256: str
    authors: list[Author]
    submitters: NotRequired[list[Author]]
    ci_log_sha256: NotRequired[str]  # the digest of the accepted run's log, bound when prepared


def load_batch_validated(raw: object, where: str) -> BatchValidated:
    obj = as_object(raw, where)
    for key in (
        "candidate",
        "candidate_fingerprint_sha256",
        "ci_receipt",
        "ci_receipt_sha256",
        "base_pointer_sha256",
    ):
        _text(obj, key, where)
    _texts(obj, "members", where)
    _texts(obj, "membership_manifest_sha256", where)
    _authors(obj, where)
    if "submitters" in obj:
        _authors(obj, where, key="submitters")
    if "ci_log_sha256" in obj:
        _text(obj, "ci_log_sha256", where)
    return cast("BatchValidated", obj)


class BatchApproved(TypedDict):
    subject: BatchSubject
    receipts: list[str]
    owner_signoff: NotRequired[list[str]]


def _batch_subject(obj: JsonObject, key: str, where: str) -> BatchSubject:
    subject = _mapping(obj, key, where)
    for name in ("batch", "candidate_fingerprint_sha256", "ci_receipt_sha256", "validated_sha256"):
        _text(subject, name, f"{where}: {key}")
    _texts(subject, "membership_manifest_sha256", f"{where}: {key}")
    return cast("BatchSubject", subject)


def load_batch_approved(raw: object, where: str) -> BatchApproved:
    obj = as_object(raw, where)
    _batch_subject(obj, "subject", where)
    _texts(obj, "receipts", where)
    if "owner_signoff" in obj:
        _texts(obj, "owner_signoff", where)
    return cast("BatchApproved", obj)


class BatchPublishing(TypedDict):
    """The publication intent: the exact pointer text and its digest."""

    pointer: str
    sha256: str


def load_batch_publishing(raw: object, where: str) -> BatchPublishing:
    obj = as_object(raw, where)
    _text(obj, "pointer", where)
    _text(obj, "sha256", where)
    return cast("BatchPublishing", obj)


class BatchPublished(TypedDict):
    pointer_sha256: str
    owner_signoff: NotRequired[list[str]]
    basis: NotRequired[Literal["integration-review", "single-submission"]]  # absent before CK-38


def load_batch_published(raw: object, where: str) -> BatchPublished:
    obj = as_object(raw, where)
    _text(obj, "pointer_sha256", where)
    if "basis" in obj and obj["basis"] not in ("integration-review", "single-submission"):
        raise _missing(where, "basis", "must be `integration-review` or `single-submission`")
    if "owner_signoff" in obj:
        _texts(obj, "owner_signoff", where)
    return cast("BatchPublished", obj)


class BatchFailed(TypedDict):
    """Why a batch ended without publishing: validation, infrastructure, recording, interruption."""

    reason: str
    at: NotRequired[int]
    failed: NotRequired[list[str]]
    exit: NotRequired[int]
    detail: NotRequired[str]


def load_batch_failed(raw: object, where: str) -> BatchFailed:
    obj = as_object(raw, where)
    _text(obj, "reason", where)
    if "at" in obj:
        _integer(obj, "at", where)
    if "failed" in obj:
        _texts(obj, "failed", where)
    if "exit" in obj:
        _integer(obj, "exit", where)
    if "detail" in obj:
        _text(obj, "detail", where)
    return cast("BatchFailed", obj)


class BatchRejected(TypedDict):
    blame: list[str]
    members: list[str]
    at: int


def load_batch_rejected(raw: object, where: str) -> BatchRejected:
    obj = as_object(raw, where)
    _texts(obj, "blame", where)
    _texts(obj, "members", where)
    _integer(obj, "at", where)
    return cast("BatchRejected", obj)


class BatchCancelled(TypedDict):
    reason: str
    at: int


def load_batch_cancelled(raw: object, where: str) -> BatchCancelled:
    obj = as_object(raw, where)
    _text(obj, "reason", where)
    _integer(obj, "at", where)
    return cast("BatchCancelled", obj)


class QuarantineRecord(TypedDict):
    """`.pipeline/quarantine/<submission>.json`: why a submission left the eligible set."""

    submission: str
    batch: str
    reason: str


def load_quarantine(raw: object, where: str) -> QuarantineRecord:
    obj = as_object(raw, where)
    for key in ("submission", "batch", "reason"):
        _text(obj, key, where)
    return cast("QuarantineRecord", obj)


# --- pointer helpers shared by the workflows ----------------------------------------------------


def is_accepted_pointer(record: PointerRecord) -> TypeGuard[AcceptedPointer]:
    return record.get("schema") not in ("bootstrap-hold", "tombstone")


def is_hold(record: PointerRecord) -> TypeGuard[PointerHold]:
    return record.get("schema") == "bootstrap-hold"


def is_tombstone(record: PointerRecord) -> TypeGuard[PointerTombstone]:
    return record.get("schema") == "tombstone"


def load_accepted_pointer(raw: object, where: str) -> AcceptedPointer:
    """A pointer that names an accepted checkpoint (not the bootstrap hold or the tombstone)."""
    record = load_pointer(raw, where)
    if not is_accepted_pointer(record):
        raise RecordError(
            f"{where}: is the bootstrap {record.get('schema')} record, not an accepted state; "
            "inspect the bootstrap with `ck status` before continuing"
        )
    return record


# --- Pijul's own JSON ---------------------------------------------------------------------------


class PijulChange(TypedDict):
    path: str


class PijulStatus(TypedDict):
    """`pijul status --output-format json`: the pending changes and the untracked paths."""

    changes: list[PijulChange]
    untracked: list[str]


def load_pijul_status(raw: object, where: str) -> PijulStatus:
    obj = as_object(raw, where)
    for index, change in enumerate(_objects(obj, "changes", where)):
        _text(change, "path", f"{where}: changes[{index}]")
    _texts(obj, "untracked", where)
    return cast("PijulStatus", obj)


class PijulPull(TypedDict):
    """`pijul pull --json`: the conflicts the pull left (an empty list when none)."""

    conflicts: list[object]


def load_pijul_pull(raw: object, where: str) -> PijulPull:
    obj = as_object(raw, where)
    _list(obj, "conflicts", where)
    return cast("PijulPull", obj)


# --- the queue, as `ck status` lists it ----------------------------------------------------------


class QueueSummary(TypedDict, total=False):
    """The fields `ck status` shows of a queued manifest of any age; each is checked when present.

    A late legacy submission is listed by id and spec, so it is not required to be a complete
    manifest here (the full forms are `ManifestV2` and `ManifestLegacy`)."""

    schema: int
    id: str
    spec: str


def load_queue_summary(raw: object, where: str) -> QueueSummary:
    obj = as_object(raw, where)
    for key in ("id", "spec"):
        if key in obj:
            _text(obj, key, where)
    if "schema" in obj:
        _integer(obj, "schema", where)
    return cast("QueueSummary", obj)


# --- the documentation queue: `.pipeline/docs/` --------------------------------------------------


class DocsEntry(TypedDict):
    """One queued documentation change (`D<ns>.json`), or a resolution group (`DRES<ns>.json`).

    `state` is `pending`, `blocked`, `conflict`, `published` or `resolved-by <group>`."""

    id: str
    state: str
    created: int
    repo: str
    hashes: list[str]
    files: list[str]
    fingerprint: Fingerprint
    message: str
    base_pointer_sha256: NotRequired[str]
    resolves: NotRequired[str]
    conflicts: NotRequired[list[object]]


def load_docs_entry(raw: object, where: str) -> DocsEntry:
    obj = as_object(raw, where)
    for key in ("id", "state", "repo", "message"):
        _text(obj, key, where)
    _integer(obj, "created", where)
    _texts(obj, "hashes", where)
    _texts(obj, "files", where)
    load_fingerprint(obj.get("fingerprint"), f"{where}: fingerprint")
    for key in ("base_pointer_sha256", "resolves"):
        if key in obj:
            _text(obj, key, where)
    if "conflicts" in obj:
        _list(obj, "conflicts", where)
    return cast("DocsEntry", obj)


class DocsJournal(TypedDict):
    """`.pipeline/docs/journal/<seq>.json`: a durable intent to publish documentation."""

    seq: int
    entries: list[str]
    prev_sha256: str
    prev: str
    next: str


def load_docs_journal(raw: object, where: str) -> DocsJournal:
    obj = as_object(raw, where)
    _integer(obj, "seq", where)
    _texts(obj, "entries", where)
    for key in ("prev_sha256", "prev", "next"):
        _text(obj, key, where)
    return cast("DocsJournal", obj)


# --- bootstrap records: `.pipeline/bootstrap/<id>/` ----------------------------------------------


class BootstrapFrozen(TypedDict):
    bootstrap: str
    submission: str
    base_pointer_sha256: str
    queue_snapshot: dict[str, str]
    manifest_sha256: str
    fingerprint_sha256: str
    superseded_legacy: list[str]
    owner: BatchOwner


def load_bootstrap_frozen(raw: object, where: str) -> BootstrapFrozen:
    obj = as_object(raw, where)
    for key in (
        "bootstrap",
        "submission",
        "base_pointer_sha256",
        "manifest_sha256",
        "fingerprint_sha256",
    ):
        _text(obj, key, where)
    _string_map(obj, "queue_snapshot", where)
    _texts(obj, "superseded_legacy", where)
    _owner(obj, where)
    return cast("BootstrapFrozen", obj)


class BootstrapValidated(TypedDict):
    bootstrap: str
    submission: str
    base_pointer_sha256: str
    manifest_sha256: str
    fingerprint_sha256: str
    candidate: str
    candidate_fingerprint: Fingerprint
    candidate_fingerprint_sha256: str
    ci_receipt: str
    ci_receipt_sha256: str
    helper_sha256: str
    superseded_legacy: list[str]


def load_bootstrap_validated(raw: object, where: str) -> BootstrapValidated:
    obj = as_object(raw, where)
    for key in (
        "bootstrap",
        "submission",
        "base_pointer_sha256",
        "manifest_sha256",
        "fingerprint_sha256",
        "candidate",
        "candidate_fingerprint_sha256",
        "ci_receipt",
        "ci_receipt_sha256",
        "helper_sha256",
    ):
        _text(obj, key, where)
    load_fingerprint(obj.get("candidate_fingerprint"), f"{where}: candidate_fingerprint")
    _texts(obj, "superseded_legacy", where)
    return cast("BootstrapValidated", obj)


class BootstrapPublishing(TypedDict):
    """The publication intent: the exact final tombstone and v2 pointer texts."""

    tombstone: str
    v2: str
    evidence_sha256: str


def load_bootstrap_publishing(raw: object, where: str) -> BootstrapPublishing:
    obj = as_object(raw, where)
    for key in ("tombstone", "v2", "evidence_sha256"):
        _text(obj, key, where)
    return cast("BootstrapPublishing", obj)


class BootstrapPublished(TypedDict):
    v2_sha256: str
    at: int


def load_bootstrap_published(raw: object, where: str) -> BootstrapPublished:
    obj = as_object(raw, where)
    _text(obj, "v2_sha256", where)
    _integer(obj, "at", where)
    return cast("BootstrapPublished", obj)


class BootstrapCancelled(TypedDict):
    reason: str
    at: int


def load_bootstrap_cancelled(raw: object, where: str) -> BootstrapCancelled:
    obj = as_object(raw, where)
    _text(obj, "reason", where)
    _integer(obj, "at", where)
    return cast("BootstrapCancelled", obj)


class BootstrapFailed(TypedDict):
    reason: str
    at: int
    conflicts: NotRequired[list[object]]


def load_bootstrap_failed(raw: object, where: str) -> BootstrapFailed:
    obj = as_object(raw, where)
    _text(obj, "reason", where)
    _integer(obj, "at", where)
    if "conflicts" in obj:
        _list(obj, "conflicts", where)
    return cast("BootstrapFailed", obj)


class RestoreEntry(TypedDict):
    """A queued docs entry the restore found already present or published again."""

    id: str
    entry_sha256: str
    state: str
    entry: DocsEntry
    already_present: NotRequired[bool]


class RestoreConflict(TypedDict):
    """A queued docs entry the restore could not publish again (blocked or conflicting)."""

    id: str
    entry_sha256: str
    state: str
    entry: DocsEntry
    conflicts: NotRequired[list[object]]


class RestoreLink(TypedDict):
    """One step of the restore chain: the pointer after republishing one docs entry."""

    id: str
    entry_sha256: str
    candidate: str
    fingerprint: Fingerprint
    receipt: str
    receipt_sha256: str
    hashes: list[str]
    previous_pointer_sha256: str
    pointer: str


def _restore_entries(obj: JsonObject, key: str, where: str, *, conflict: bool) -> None:
    for index, row in enumerate(_objects(obj, key, where)):
        here = f"{where}: {key}[{index}]"
        for name in ("id", "entry_sha256", "state"):
            _text(row, name, here)
        load_docs_entry(row.get("entry"), f"{here}: entry")
        if conflict and "conflicts" in row:
            _list(row, "conflicts", here)
        if not conflict and "already_present" in row:
            _boolean(row, "already_present", here)


class BootstrapRestoring(TypedDict):
    """The restore intent: the final pointer text and the chain that leads to it."""

    mode: Literal["restored"]
    pointer: str
    pointer_sha256: str
    entries: list[RestoreEntry]
    links: list[RestoreLink]
    conflicts: list[RestoreConflict]


def load_bootstrap_restoring(raw: object, where: str) -> BootstrapRestoring:
    obj = as_object(raw, where)
    if obj.get("mode") != "restored":
        raise _missing(where, "mode", "must be `restored`")
    _text(obj, "pointer", where)
    _text(obj, "pointer_sha256", where)
    _restore_entries(obj, "entries", where, conflict=False)
    _restore_entries(obj, "conflicts", where, conflict=True)
    for index, link in enumerate(_objects(obj, "links", where)):
        here = f"{where}: links[{index}]"
        for name in (
            "id",
            "entry_sha256",
            "candidate",
            "receipt",
            "receipt_sha256",
            "previous_pointer_sha256",
            "pointer",
        ):
            _text(link, name, here)
        load_fingerprint(link.get("fingerprint"), f"{here}: fingerprint")
        _texts(link, "hashes", here)
    return cast("BootstrapRestoring", obj)


class BootstrapRestored(TypedDict):
    mode: Literal["restored", "never-held"]
    pointer_sha256: str
    conflicts: NotRequired[list[RestoreConflict]]


def load_bootstrap_restored(raw: object, where: str) -> BootstrapRestored:
    obj = as_object(raw, where)
    if obj.get("mode") not in ("restored", "never-held"):
        raise _missing(where, "mode", "must be `restored` or `never-held`")
    _text(obj, "pointer_sha256", where)
    if "conflicts" in obj:
        _restore_entries(obj, "conflicts", where, conflict=True)
    return cast("BootstrapRestored", obj)


# --- the bootstrap's reviewed evidence -----------------------------------------------------------


class EvidenceRow(TypedDict):
    path: str
    sha256: str


def _evidence_row(obj: JsonObject, key: str, where: str) -> EvidenceRow:
    row = _mapping(obj, key, where)
    _text(row, "path", f"{where}: {key}")
    _text(row, "sha256", f"{where}: {key}")
    return cast("EvidenceRow", row)


class EvidenceIndex(TypedDict):
    """`index.json` of a bootstrap's evidence: the files it binds, by path and digest."""

    schema: Literal["ck-bootstrap-evidence/1"]
    bootstrap: str
    spec: EvidenceRow
    authors: EvidenceRow
    receipts: list[EvidenceRow]
    validated_sha256: str


def load_evidence_index(raw: object, where: str) -> EvidenceIndex:
    obj = as_object(raw, where)
    if obj.get("schema") != "ck-bootstrap-evidence/1":
        raise _missing(where, "schema", "must be `ck-bootstrap-evidence/1`")
    _text(obj, "bootstrap", where)
    _text(obj, "validated_sha256", where)
    _evidence_row(obj, "spec", where)
    _evidence_row(obj, "authors", where)
    for index, row in enumerate(_objects(obj, "receipts", where)):
        _text(row, "path", f"{where}: receipts[{index}]")
        _text(row, "sha256", f"{where}: receipts[{index}]")
    return cast("EvidenceIndex", obj)


class AuthorsLedger(TypedDict):
    """The bootstrap's authors ledger: who wrote the spec and the submission, patch by patch."""

    schema: Literal["ck-bootstrap-authors/1"]
    patches: dict[str, list[int]]
    spec_authors: list[Author]
    submission_authors: list[Author]


def load_authors_ledger(raw: object, where: str) -> AuthorsLedger:
    obj = as_object(raw, where)
    if obj.get("schema") != "ck-bootstrap-authors/1":
        raise _missing(where, "schema", "must be `ck-bootstrap-authors/1`")
    patches = _mapping(obj, "patches", where)
    for patch, indexes in patches.items():
        if (
            not isinstance(indexes, list)
            or not indexes
            or not all(type(item) is int for item in indexes)
        ):
            raise _missing(where, f"patches.{patch}", "must be a non-empty list of integers")
    _authors(obj, where, key="spec_authors")
    _authors(obj, where, key="submission_authors")
    return cast("AuthorsLedger", obj)


class BootstrapSpecSubject(TypedDict):
    path: str
    sha256: str


class BootstrapIntegrationSubject(TypedDict):
    bootstrap: str
    validated_sha256: str
    candidate_fingerprint_sha256: str
    ci_receipt_sha256: str
    membership_manifest_sha256: list[str]


AnySubject = Subject | BootstrapSpecSubject | BootstrapIntegrationSubject


class BootstrapReceipt(TypedDict):
    """A `ck-bootstrap-review/1` receipt: the reviews that approved the one-time bootstrap."""

    schema: Literal["ck-bootstrap-review/1"]
    kind: Literal["spec", "submission", "integration"]
    verdict: str
    recorded: str
    report: EvidenceRow
    reviewer: ReviewerIdentity
    subject: BootstrapSpecSubject | SubmissionSubject | BootstrapIntegrationSubject


def load_bootstrap_receipt(raw: object, where: str) -> BootstrapReceipt:
    obj = as_object(raw, where)
    if obj.get("schema") != "ck-bootstrap-review/1":
        raise _missing(where, "schema", "must be `ck-bootstrap-review/1`")
    kind = obj.get("kind")
    if not isinstance(kind, str) or kind not in ("spec", "submission", "integration"):
        raise _missing(where, "kind", "must be `spec`, `submission` or `integration`")
    _text(obj, "verdict", where)
    _text(obj, "recorded", where)
    _evidence_row(obj, "report", where)
    reviewer = _mapping(obj, "reviewer", where)
    for key in _AI_REVIEWER_KEYS:
        _text(reviewer, key, f"{where}: reviewer")
    subject = _mapping(obj, "subject", where)
    if kind == "spec":
        for key in ("path", "sha256"):
            _text(subject, key, f"{where}: subject")
    elif kind == "submission":
        for key in _SUBJECT_KEYS["submission"]:
            _text(subject, key, f"{where}: subject")
    else:
        for key in (
            "bootstrap",
            "validated_sha256",
            "candidate_fingerprint_sha256",
            "ci_receipt_sha256",
        ):
            _text(subject, key, f"{where}: subject")
        _texts(subject, "membership_manifest_sha256", f"{where}: subject")
    return cast("BootstrapReceipt", obj)
