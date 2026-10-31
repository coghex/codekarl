"""ck-guide: the mechanical half of the Codekarl guide skill.

The helper pins an accepted pointer, rebuilds the publication chain behind it, snapshots the issues
and the vision, and keeps the guide's coverage and cursor honest. It makes no judgments: verdicts,
findings and readings come from the agent that runs the skill.

**Boundary.** It reads record bytes directly, from the hub's `.pipeline` files, the metadata
repository's files, the review copy and the log file it is given. It runs no subprocess of any kind
(no `tools/ck`, no Pijul, no `git`, no `gh`): the agent creates the review copy and captures its
history, and anything that needs a command is a step the skill tells the agent to run. It writes only
its run directory `<hub>/.pipeline/coordination/guide/<stem>/` and, through `finish` and
`rebuild-cursor`, the docs copy it is given. It never touches a checkpoint, a capture, a pointer, a
manifest, a receipt, the metadata repository or any other copy.

The ordering of publications comes only from the pointer links the hub already records (each batch's
`frozen.json`, `validated.json` and `publishing.json`, and the docs journal); patch dates and record timestamps are
never used, and the patch membership of the given log only verifies the chain.
"""

import argparse
import hashlib
import json
import re
import stat
import sys
import time
from collections.abc import Callable, Collection, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

# A run must write nothing outside its run directory, so importing the gate modules must not leave
# `__pycache__` files in the code tree. This is set before the first project import.
sys.dont_write_bytecode = True

from gate import records  # noqa: E402  # after the bytecode setting above, on purpose

JsonObject = dict[str, object]

JUDGMENT = ("verdict", "depth", "note")  # the only fields of a coverage item that the agent sets

SCHEMA = "ck-guide/1"
POINTER_SCHEMA = 2  # the accepted-pointer format whose `submissions` the gate appends to
VERDICTS = ("reviewed", "inherited", "gap", "unreviewed", "outside-range")
COVERED = ("reviewed", "inherited")
READINGS = ("aligned", "drifting", "not exercised")
MODES = ("full", "review-only", "scoped")
PRINCIPLE_KEY = re.compile(r"vision\.[a-z0-9][a-z0-9-]*")
# The heading is `docs/vision.md` and the game design it indexes, `docs/game/*.md`: every direct child ending in
# `.md`, whatever its name (spaces, Unicode, a leading underscore), but no subdirectory and no traversal (no `/`).
VISION = "docs/vision.md"
GAME_DOCUMENT = re.compile(r"docs/game/[^/\x00-\x1f\x7f]+\.md")
STEM = re.compile(r"[0-9]{8}t[0-9]{6}z-[0-9a-f]{8}")
HEADING = re.compile(r"^###\s+(?:\[(#\d+|no-issue|deferred)\]\s+)?(GUIDE-\d+)\.\s*(.*)$", re.M)
BOUNDARY = re.compile(
    r"Resume after:\s*`([^`]+)`(?: · patch `([^`]*)`)?(?: · pointer `([0-9a-f]{64})`)?"
)
FENCE = re.compile(r"```json\n(.*?)\n```", re.S)


class GuideError(Exception):
    """A refusal: the message says what is wrong, and the command exits non-zero."""


# --- typed access to JSON ---------------------------------------------------------------------


def as_object(value: object, where: str) -> JsonObject:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise GuideError(f"{where}: expected a JSON object")
    return {str(key): item for key, item in value.items()}


def as_list(value: object, where: str) -> list[object]:
    if not isinstance(value, list):
        raise GuideError(f"{where}: expected a JSON array")
    return list(value)


def text_of(value: object, where: str) -> str:
    if not isinstance(value, str):
        raise GuideError(f"{where}: expected a string")
    return value


def field(obj: JsonObject, key: str, where: str) -> object:
    if key not in obj:
        raise GuideError(f"{where}: missing `{key}`")
    return obj[key]


def string_field(obj: JsonObject, key: str, where: str) -> str:
    return text_of(field(obj, key, where), f"{where}: `{key}`")


def strings_field(obj: JsonObject, key: str, where: str) -> list[str]:
    return [text_of(item, f"{where}: `{key}`") for item in as_list(field(obj, key, where), where)]


def objects_field(obj: JsonObject, key: str, where: str) -> list[JsonObject]:
    return [as_object(item, f"{where}: `{key}`") for item in as_list(field(obj, key, where), where)]


def optional_string(obj: JsonObject, key: str) -> str:
    value = obj.get(key)
    return value if isinstance(value, str) else ""


def read_json(path: Path) -> JsonObject:
    try:
        return as_object(json.loads(path.read_bytes()), str(path))
    except (OSError, ValueError) as error:
        raise GuideError(f"cannot read {path}: {error}") from error


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    try:
        return sha256_bytes(path.read_bytes())
    except OSError as error:
        raise GuideError(f"cannot read {path}: {error}") from error


def canonical(value: object) -> bytes:
    """Canonical JSON: sorted keys, compact separators, non-ASCII kept."""
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def pretty(value: object) -> str:
    return json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False) + "\n"


def utc(seconds: float) -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(seconds))


# --- the hub ----------------------------------------------------------------------------------


@dataclass(frozen=True)
class Hub:
    """The hub's `.pipeline` directory and the metadata repository, as plain paths."""

    root: Path
    meta: Path

    @property
    def pipeline(self) -> Path:
        return self.root / ".pipeline"

    @property
    def pointer_path(self) -> Path:
        return self.pipeline / "accepted-v2.json"

    def run_dir(self, stem: str) -> Path:
        return self.pipeline / "coordination" / "guide" / stem


def locate_hub(hub: str | None, meta: str | None) -> Hub:
    """The hub named on the command line, else `CK_HUB`, else this checkout's `.ck-hub` marker."""
    tools = Path(__file__).resolve().parents[1]
    chosen: Path
    if hub:
        chosen = Path(hub)
    else:
        marker = tools.parent / ".ck-hub"
        chosen = Path(marker.read_text().strip()) if marker.is_file() else tools.parent
    chosen = chosen.resolve()
    if meta:
        return Hub(chosen, Path(meta).resolve())
    gate = chosen / ".pipeline" / "gate.json"
    if gate.is_file():
        configured = optional_string(read_json(gate), "meta_repo")
        if configured:
            return Hub(chosen, Path(configured))
    return Hub(chosen, chosen.parent / "codekarl-meta")


# --- the publication chain --------------------------------------------------------------------


@dataclass(frozen=True)
class Member:
    """A submission published by a batch: its manifest digest and the patches it carries."""

    id: str
    manifest_sha256: str
    hashes: tuple[str, ...]
    issues: tuple[str, ...]


@dataclass(frozen=True)
class Publication:
    """One step of the chain: a batch or a docs publication, with the pointers it links."""

    id: str
    kind: str
    base_sha256: str
    pointer_sha256: str
    pointer_text: str
    basis: str
    members: tuple[Member, ...]
    hashes: tuple[str, ...]
    queued_hashes: tuple[str, ...]
    files: tuple[str, ...]

    def inventory(self) -> JsonObject:
        return {
            "id": self.id,
            "kind": self.kind,
            "basis": self.basis,
            "base_sha256": self.base_sha256,
            "pointer_sha256": self.pointer_sha256,
            "members": [
                {
                    "id": member.id,
                    "manifest_sha256": member.manifest_sha256,
                    "hashes": list(member.hashes),
                    "issues": list(member.issues),
                }
                for member in self.members
            ],
            "hashes": list(self.hashes),
            "queued_hashes": list(self.queued_hashes),
            "files": list(self.files),
        }


def check_pointer_submissions(
    where: str, batch: str, pointer_text: str, validated: list[str], rejected: Collection[str]
) -> None:
    """The published pointer (schema 2) is this batch's: it ends with the validated members, has
    none of the others and names the batch in its `integration`."""
    try:
        pointer = as_object(json.loads(pointer_text), f"{where}: publishing.json pointer")
    except ValueError as error:
        raise GuideError(f"{where}: publishing.json pointer is not JSON: {error}") from error
    if pointer.get("schema") != POINTER_SCHEMA:
        raise GuideError(f"{where}: publishing.json pointer is not a schema 2 pointer")
    integration = as_object(
        field(pointer, "integration", f"{where}: publishing.json pointer"),
        f"{where}: publishing.json pointer `integration`",
    )
    if integration.get("batch") != batch:
        raise GuideError(
            f"{where}: the published pointer's integration names batch "
            f"{integration.get('batch')!r}, not {batch}"
        )
    submissions = strings_field(pointer, "submissions", f"{where}: publishing.json pointer")
    if submissions[len(submissions) - len(validated) :] != validated:
        raise GuideError(
            f"{where}: the published pointer's submissions do not end with the validated members "
            f"{', '.join(validated)}"
        )
    for member_id in rejected:
        if member_id in submissions:
            raise GuideError(
                f"{where}: submission {member_id} is in the published pointer but was not validated"
            )


def published_members(
    directory: Path, where: str, frozen_record: JsonObject, pointer_text: str
) -> list[tuple[str, str]]:
    """The (submission, manifest hash) pairs a batch actually published, in publication order.

    Supported record format: `frozen.json` lists every member the batch froze (each with its manifest
    hash) and `validated.json` lists the members validated into the candidate, with their manifest
    hashes in the same order; the gate then publishes a pointer (schema 2) whose `submissions` end
    with exactly the validated members. A split batch (CK-119 rejected, CK-127 published) therefore
    froze more members than it published, and only the published ones are inventoried. Each record
    must agree with the others; a missing, malformed or inconsistent one is refused rather than
    guessed around, so a genuinely published patch is never dropped.
    """
    validated_path = directory / "validated.json"
    if not validated_path.is_file():
        raise GuideError(
            f"{where}: validated.json is missing, so the published members cannot be told from "
            "the frozen ones"
        )
    validated = read_json(validated_path)
    frozen_base = string_field(frozen_record, "base_pointer_sha256", f"{where}: frozen.json")
    validated_base = string_field(validated, "base_pointer_sha256", f"{where}: validated.json")
    if validated_base != frozen_base:
        raise GuideError(
            f"{where}: validated.json is for base {validated_base} but frozen.json froze the batch "
            f"on base {frozen_base}"
        )
    frozen_hashes: dict[str, str] = {}
    for entry in objects_field(frozen_record, "members", f"{where}: frozen.json"):
        member_id = string_field(entry, "id", f"{where}: member")
        if member_id in frozen_hashes:
            raise GuideError(f"{where}: frozen.json lists submission {member_id} twice")
        frozen_hashes[member_id] = string_field(
            entry, "manifest_sha256", f"{where}: member {member_id}"
        )
    ids = strings_field(validated, "members", f"{where}: validated.json")
    bound = strings_field(validated, "membership_manifest_sha256", f"{where}: validated.json")
    if not ids:
        raise GuideError(f"{where}: validated.json lists no published member")
    if len(set(ids)) != len(ids):
        raise GuideError(f"{where}: validated.json lists a submission twice")
    if len(bound) != len(ids):
        raise GuideError(
            f"{where}: validated.json lists {len(ids)} members but {len(bound)} manifest hashes"
        )
    selected: list[tuple[str, str]] = []
    for member_id, digest in zip(ids, bound, strict=True):
        frozen_digest = frozen_hashes.get(member_id)
        if frozen_digest is None:
            raise GuideError(
                f"{where}: validated.json names submission {member_id}, which frozen.json did not freeze"
            )
        if frozen_digest != digest:
            raise GuideError(
                f"{where}: the manifest hash validated.json binds to submission {member_id} "
                f"({digest}) differs from the frozen one ({frozen_digest})"
            )
        selected.append((member_id, digest))
    check_pointer_submissions(
        where, directory.name, pointer_text, ids, frozen_hashes.keys() - set(ids)
    )
    return selected


def load_batch(hub: Hub, directory: Path) -> Publication | None:
    """A batch that was published, with every stored hash recomputed; `None` for one that never was.

    Only the members it actually published are inventoried (`published_members`), not every member it froze.
    """
    publishing = directory / "publishing.json"
    frozen = directory / "frozen.json"
    if not publishing.is_file() or not frozen.is_file():
        return None
    where = f"batch {directory.name}"
    record = read_json(publishing)
    pointer_text = string_field(record, "pointer", f"{where}: publishing.json")
    recorded = string_field(record, "sha256", f"{where}: publishing.json")
    actual = sha256_bytes(pointer_text.encode())
    if actual != recorded:
        raise GuideError(
            f"{where}: stored hash mismatch: publishing.json records pointer {recorded} but its "
            f"bytes hash to {actual}"
        )
    published = directory / "published.json"
    basis = ""
    if published.is_file():
        marker = read_json(published)
        basis = optional_string(marker, "basis")
        if optional_string(marker, "pointer_sha256") not in ("", actual):
            raise GuideError(f"{where}: stored hash mismatch: published.json names another pointer")
    frozen_record = read_json(frozen)
    selected = published_members(directory, where, frozen_record, pointer_text)
    members: list[Member] = []
    for member_id, expected in selected:
        manifest_path = hub.pipeline / "queue" / f"{member_id}.json"
        if not manifest_path.is_file():
            raise GuideError(f"{where}: the manifest of submission {member_id} is missing")
        if sha256_file(manifest_path) != expected:
            raise GuideError(
                f"{where}: stored hash mismatch: the manifest of submission {member_id} does not "
                f"hash to {expected}"
            )
        manifest = read_json(manifest_path)
        issues = (
            tuple(
                string_field(item, "id", f"{where}: manifest {member_id} issue")
                for item in objects_field(manifest, "issues", f"{where}: manifest {member_id}")
            )
            if "issues" in manifest
            else ()
        )
        members.append(
            Member(
                member_id,
                expected,
                tuple(strings_field(manifest, "hashes", f"{where}: manifest {member_id}")),
                issues,
            )
        )
    return Publication(
        id=directory.name,
        kind="batch",
        base_sha256=string_field(frozen_record, "base_pointer_sha256", f"{where}: frozen.json"),
        pointer_sha256=actual,
        pointer_text=pointer_text,
        basis=basis,
        members=tuple(members),
        hashes=tuple(hash_ for member in members for hash_ in member.hashes),
        queued_hashes=(),
        files=(),
    )


def load_docs(hub: Hub, row_path: Path) -> Publication:
    """A docs publication from its journal row, its receipt and (as provenance) its queued entry."""
    row = read_json(row_path)
    where = f"docs journal {row_path.name}"
    previous = string_field(row, "prev", where)
    following = string_field(row, "next", where)
    recorded = string_field(row, "prev_sha256", where)
    if sha256_bytes(previous.encode()) != recorded:
        raise GuideError(f"{where}: stored hash mismatch: prev does not hash to {recorded}")
    entries = strings_field(row, "entries", where)
    if len(entries) != 1:
        raise GuideError(f"{where}: expected exactly one entry, found {len(entries)}")
    identifier = entries[0]
    receipt_path = hub.pipeline / "docs" / "receipts" / f"{identifier}.json"
    if not receipt_path.is_file():
        raise GuideError(f"{where}: the receipt of {identifier} is missing")
    receipt = read_json(receipt_path)
    if receipt.get("fingerprint") != as_object(json.loads(following), where).get("fingerprint"):
        raise GuideError(
            f"{where}: the receipt of {identifier} has a fingerprint that differs from the "
            "journal's next pointer"
        )
    queued: tuple[str, ...] = ()
    files: tuple[str, ...] = ()
    entry_path = hub.pipeline / "docs" / f"{identifier}.json"
    if entry_path.is_file():
        entry = read_json(entry_path)
        queued = tuple(strings_field(entry, "hashes", f"docs entry {identifier}"))
        files = tuple(strings_field(entry, "files", f"docs entry {identifier}"))
    return Publication(
        id=identifier,
        kind="docs",
        base_sha256=recorded,
        pointer_sha256=sha256_bytes(following.encode()),
        pointer_text=following,
        basis="",
        members=(),
        hashes=tuple(strings_field(receipt, "hashes", f"receipt {identifier}")),
        queued_hashes=queued,
        files=files,
    )


def load_publications(hub: Hub) -> list[Publication]:
    """Every published batch and docs publication the hub records, each verified from its bytes."""
    found: list[Publication] = []
    batches = hub.pipeline / "batches"
    if batches.is_dir():
        for directory in sorted(batches.iterdir()):
            publication = load_batch(hub, directory) if directory.is_dir() else None
            if publication is not None:
                found.append(publication)
    journal = hub.pipeline / "docs" / "journal"
    if journal.is_dir():
        found.extend(load_docs(hub, path) for path in sorted(journal.glob("*.json")))
    return found


def index_by_product(publications: Sequence[Publication]) -> dict[str, Publication]:
    index: dict[str, Publication] = {}
    for publication in publications:
        other = index.get(publication.pointer_sha256)
        if other is not None:
            raise GuideError(
                f"ambiguous link: two records produce pointer {publication.pointer_sha256} "
                f"({other.id} and {publication.id})"
            )
        index[publication.pointer_sha256] = publication
    return index


def walk(
    publications: Sequence[Publication], pin_sha: str, baseline_sha: str | None
) -> list[Publication]:
    """The publications from `baseline_sha` (exclusive) to `pin_sha` (inclusive), oldest first.

    With no baseline the walk follows the links as far back as they go. Ordering comes only from
    the recorded pointer links."""
    index = index_by_product(publications)
    steps: list[Publication] = []
    seen: set[str] = set()
    current = pin_sha
    while current != baseline_sha:
        if current in seen:
            raise GuideError(f"the publication chain loops at pointer {current}")
        seen.add(current)
        step = index.get(current)
        if step is None:
            if baseline_sha is None:
                break
            # The baseline starts a recorded chain, so a link between it and the pin is missing.
            if any(item.base_sha256 == baseline_sha for item in publications):
                raise GuideError(
                    f"missing link: no record produces pointer {current}, so the walk from the pin "
                    f"cannot reach the baseline {baseline_sha}"
                )
            raise GuideError(
                f"baseline {baseline_sha} is never reached: the chain from the pin ends at pointer "
                f"{current}"
            )
        steps.append(step)
        current = step.base_sha256
    steps.reverse()
    return steps


def log_hashes(log: Path) -> set[str]:
    """The patch hashes in a `pijul log --hash-only` capture, one per line."""
    try:
        lines = log.read_text().splitlines()
    except OSError as error:
        raise GuideError(f"cannot read the log {log}: {error}") from error
    return {line.strip() for line in lines if line.strip()}


def verify_membership(steps: Sequence[Publication], present: set[str]) -> None:
    """Every actually published patch must be in the pinned log (queued docs hashes are provenance)."""
    for step in steps:
        for hash_ in step.hashes:
            if hash_ not in present:
                raise GuideError(
                    f"{step.kind} publication {step.id}: patch {hash_} is absent from the given log"
                )


def resolve_baseline(publications: Sequence[Publication], position: str) -> str:
    """A publication ID (the pointer it produced) or a pointer SHA-256 recorded on the chain."""
    for publication in publications:
        if publication.id == position:
            return publication.pointer_sha256
    if re.fullmatch(r"[0-9a-f]{64}", position):
        for publication in publications:
            if position in (publication.pointer_sha256, publication.base_sha256):
                return position
    raise GuideError(
        f"baseline {position} is neither a recorded publication nor a pointer on the chain"
    )


# --- the pin ----------------------------------------------------------------------------------


@dataclass(frozen=True)
class Pin:
    """The accepted pointer a run is pinned to: its text, SHA-256, checkpoint and receipt."""

    text: str
    sha256: str
    repo: Path
    receipt: str
    files: dict[str, str]
    executable: dict[str, bool]

    def doc(self, relative: str) -> Path:
        return self.repo / relative


def read_pin(pointer_path: Path) -> Pin:
    try:
        data = pointer_path.read_bytes()
    except OSError as error:
        raise GuideError(f"cannot read the accepted pointer {pointer_path}: {error}") from error
    return pin_from_text(data.decode())


def pin_from_text(text: str) -> Pin:
    where = "the accepted pointer"
    pointer = as_object(json.loads(text), where)
    fingerprint = as_object(field(pointer, "fingerprint", where), f"{where}: fingerprint")
    entries = {
        name: as_object(entry, f"{where}: file {name}")
        for name, entry in as_object(field(fingerprint, "files", where), where).items()
    }
    files = {name: string_field(entry, "sha256", where) for name, entry in entries.items()}
    executable = {name: entry.get("executable") is True for name, entry in entries.items()}
    return Pin(
        text=text,
        sha256=sha256_bytes(text.encode()),
        repo=Path(string_field(pointer, "repo", where)),
        receipt=optional_string(pointer, "receipt"),
        files=files,
        executable=executable,
    )


def check_copy(pin: Pin, copy: Path) -> list[str]:
    """Refuse a review copy whose files differ from the pin's fingerprint; list files beyond it."""
    if not copy.is_dir():
        raise GuideError(f"the review copy {copy} is not a directory")
    for name, digest in sorted(pin.files.items()):
        path = copy / name
        if not path.is_file():
            raise GuideError(f"the review copy lacks {name}, which the pin's fingerprint lists")
        if sha256_file(path) != digest:
            raise GuideError(
                f"the review copy's {name} differs from the pin's fingerprint "
                f"(the copy is not at pin {pin.sha256[:8]})"
            )
        if bool(path.stat().st_mode & stat.S_IXUSR) != pin.executable[name]:
            raise GuideError(
                f"the review copy's {name} has a different executable flag from the pin's "
                f"fingerprint (the copy is not at pin {pin.sha256[:8]})"
            )
    return []


# --- the metadata repository ------------------------------------------------------------------


def digest_file(path: Path) -> str:
    return sha256_file(path)


def snapshot_issue(hub: Hub, issue_id: str, reviews: Sequence[JsonObject]) -> JsonObject | None:
    """An issue as the metadata files describe it, with its two fingerprints. Read-only."""
    root = hub.meta / "issues" / issue_id
    event_paths = sorted(root.glob("[0-9]*-*.json")) if root.is_dir() else []
    if not event_paths:
        return None
    events = [records.load_issue_event(read_json(path), str(path)) for path in event_paths]
    revisions = sorted(root.glob("spec/r[0-9]*.md"), key=lambda item: int(item.stem[1:]))
    spec = (
        records.SpecRevision(revisions[-1].stem, digest_file(revisions[-1]), str(revisions[-1]))
        if revisions
        else None
    )
    issue = records.fold_issue(issue_id, events, spec)
    comments = [digest_file(path) for path in sorted((root / "comments").glob("*.md"))]
    receipts = [
        {
            "id": string_field(review, "id", "review receipt"),
            "kind": optional_string(review, "kind"),
            "verdict": optional_string(review, "verdict"),
        }
        for review in reviews
        if issue_id
        in [
            optional_string(item, "id")
            for item in (
                as_object(entry, "review issue")
                for entry in as_list(review.get("issues", []), "review issues")
            )
        ]
    ]
    receipts.sort(key=lambda item: str(item["id"]))
    definition = {
        "title": issue["title"],
        "type": issue["type"],
        "depends": sorted(issue["depends"]),
        "spec": {
            "revision": issue.get("revision", ""),
            "sha256": issue.get("spec_sha256", ""),
        },
        "comments": comments,
    }
    state = {
        "state": issue["state"],
        "reason": issue.get("reason", ""),
        "assign": issue.get("assign", ""),
        "priority": issue["priority"],
        "reviews": [{"id": item["id"], "verdict": item["verdict"]} for item in receipts],
    }
    return {
        "id": issue_id,
        "title": issue["title"],
        "type": issue["type"],
        "state": issue["state"],
        "reason": issue.get("reason", ""),
        "depends": sorted(issue["depends"]),
        "spec_revision": issue.get("revision", ""),
        "spec_sha256": issue.get("spec_sha256", ""),
        "comments": comments,
        "reviews": receipts,
        "definition_sha256": sha256_bytes(canonical(definition)),
        "state_sha256": sha256_bytes(canonical(state)),
    }


def load_reviews(hub: Hub) -> list[JsonObject]:
    directory = hub.meta / "reviews"
    return (
        [read_json(path) for path in sorted(directory.glob("*.json"))] if directory.is_dir() else []
    )


def all_issue_ids(hub: Hub) -> list[str]:
    root = hub.meta / "issues"
    return (
        sorted(
            (path.name for path in root.iterdir() if path.is_dir()),
            key=lambda name: (len(name), name),
        )
        if root.is_dir()
        else []
    )


def changed_since_covered(now: JsonObject, before: JsonObject | None) -> bool:
    """The one rule behind `status`, `start` and `recheck`: an earlier guide report covered this issue
    and its definition fingerprint has changed since (title, type, dependencies, spec or a comment).
    A state-only change, an unchanged issue and an issue never covered are not."""
    return before is not None and before.get("definition_sha256") != now["definition_sha256"]


def snapshot_issues(
    hub: Hub, touched: set[str], covered: dict[str, JsonObject]
) -> list[JsonObject]:
    """Every issue that is open, touched by the range, or closed but changed since an earlier report
    covered it (`covered` holds the latest covering entry of each issue), with both fingerprints."""
    reviews = load_reviews(hub)
    snapshots: list[JsonObject] = []
    for issue_id in all_issue_ids(hub):
        snapshot = snapshot_issue(hub, issue_id, reviews)
        if snapshot is None:
            continue
        if (
            snapshot["state"] != "closed"
            or issue_id in touched
            or changed_since_covered(snapshot, covered.get(issue_id))
        ):
            snapshots.append(snapshot)
    return snapshots


# --- earlier reports and the cursor file -------------------------------------------------------


def parse_coverage(path: Path) -> JsonObject:
    match = FENCE.search(path.read_text())
    if match is None:
        raise GuideError(f"{path}: no fenced JSON block")
    coverage = as_object(json.loads(match.group(1)), str(path))
    if coverage.get("schema") != SCHEMA:
        raise GuideError(f"{path}: not a {SCHEMA} coverage record")
    return coverage


def merged_files(directories: Sequence[Path], pattern: str) -> list[Path]:
    """The files matching `pattern` across the guide directories, by name; a later directory wins."""
    found: dict[str, Path] = {}
    for directory in directories:
        if directory.is_dir():
            found.update({path.name: path for path in directory.glob(pattern)})
    return [found[name] for name in sorted(found)]


def load_coverages(directories: Sequence[Path]) -> list[JsonObject]:
    """The coverage records in the `docs/guide` directories, oldest stem first."""
    return [parse_coverage(path) for path in merged_files(directories, "*.coverage.md")]


@dataclass(frozen=True)
class Boundary:
    """The cursor's `Resume after` line: a publication, its last patch and its pointer."""

    id: str
    patch: str
    pointer_sha256: str


def read_cursor(path: Path) -> Boundary | None:
    if not path.is_file():
        return None
    match = BOUNDARY.search(path.read_text())
    if match is None:
        return None
    return Boundary(match.group(1), match.group(2) or "", match.group(3) or "")


def read_cursors(directories: Sequence[Path]) -> list[Boundary]:
    """The cursor boundary of each directory that has a readable `CURSOR.md`."""
    found = (read_cursor(directory / "CURSOR.md") for directory in directories)
    return [boundary for boundary in found if boundary is not None]


def report_findings(directories: Sequence[Path]) -> list[tuple[str, str, str, str]]:
    """(stem, finding ID, marker, title) for every heading of every report, oldest first."""
    found: list[tuple[str, str, str, str]] = []
    for path in merged_files(directories, "*.md"):
        if path.name == "CURSOR.md" or path.name.endswith(".coverage.md"):
            continue
        for marker, identifier, title in HEADING.findall(path.read_text()):
            found.append((path.name.removesuffix(".md"), identifier, marker, title.strip()))
    return found


# --- a run ------------------------------------------------------------------------------------


def make_stem(pin: Pin, seconds: float) -> str:
    """`yyyymmddthhmmssz-<8 hex of the pointer>`, all lowercase so `guide-<stem>` is a worker name."""
    return time.strftime("%Y%m%dt%H%M%Sz", time.gmtime(seconds)) + "-" + pin.sha256[:8]


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(pretty(value))


def load_run(hub: Hub, stem: str) -> tuple[Path, JsonObject]:
    if not STEM.fullmatch(stem):
        raise GuideError(f"`{stem}` is not a run stem (yyyymmddthhmmssz-<8 hex>)")
    run = hub.run_dir(stem)
    if not (run / "run.json").is_file():
        raise GuideError(f"no run {stem}: {run} has no run.json")
    return run, read_json(run / "run.json")


def run_pin(state: JsonObject) -> Pin:
    return pin_from_text(string_field(state, "pointer_text", "run.json"))


def coverage_items(coverage: JsonObject, key: str) -> list[JsonObject]:
    return objects_field(coverage, key, "coverage")


def previous_verdicts(
    guide_dir: Path,
) -> tuple[dict[str, JsonObject], dict[str, JsonObject]]:
    """The latest covering entry of each publication and of each issue in the earlier guide reports.

    A `review-only` run is not coverage that later runs may inherit."""
    publications: dict[str, JsonObject] = {}
    issues: dict[str, JsonObject] = {}
    for coverage in load_coverages([guide_dir]):
        if coverage.get("mode") == "review-only":
            continue
        for item in coverage_items(coverage, "publications"):
            if item.get("verdict") in COVERED:
                publications[string_field(item, "id", "coverage publication")] = item
        for item in coverage_items(coverage, "issues"):
            if item.get("verdict") in COVERED:
                issues[string_field(item, "id", "coverage issue")] = item
    return publications, issues


def document_origin(chain: Sequence[Publication], name: str) -> str:
    """The last docs publication on the chain that changed `name`; empty if none is recorded."""
    origin = ""
    for step in chain:
        if step.kind == "docs" and name in step.files:
            origin = step.id
    return origin


def is_heading_document(name: str) -> bool:
    """Whether `name` may be a principle's document: the vision or a direct `docs/game/*.md` file."""
    return name == VISION or GAME_DOCUMENT.fullmatch(name) is not None


def heading_documents(files: Mapping[str, str]) -> list[str]:
    """The heading documents a fingerprint lists: the vision (always named, listed or not) and every
    `docs/game/*.md` file in it."""
    return sorted({VISION, *(name for name in files if GAME_DOCUMENT.fullmatch(name))})


def build_documents(
    pin: Pin, chain: Sequence[Publication], in_range: Sequence[Publication]
) -> list[JsonObject]:
    """The heading documents and the contracts the in-range docs publications changed, as pinned.

    The heading is `docs/vision.md` and every `docs/game/*.md` the pin's fingerprint lists, pinned
    whether or not the range changed them.

    A file the pin's fingerprint lists must be readable here and match its digest, or the start is
    refused: it is never silently left out. A file the fingerprint does not list (a contract that
    has since been deleted, or a vision that was never published) is recorded as absent, with no
    digest. Each entry also gives the publication that last changed the file, which may be before
    the baseline; it is empty when no recorded publication changed it."""
    names = set(heading_documents(pin.files))
    names.update(
        name
        for step in in_range
        for name in step.files
        if name.startswith("docs/") and not name.startswith("docs/guide/")
    )
    documents: list[JsonObject] = []
    for name in sorted(names):
        origin = document_origin(chain, name)
        if name not in pin.files:
            documents.append({"path": name, "absent": True, "sha256": "", "publication": origin})
            continue
        path = pin.doc(name)
        if not path.is_file():
            raise GuideError(f"the pinned {name} cannot be read from the checkpoint {pin.repo}")
        digest = digest_file(path)
        if pin.files[name] != digest:
            raise GuideError(f"the pinned {name} does not match the pin's fingerprint")
        documents.append({"path": name, "sha256": digest, "publication": origin})
    return documents


def store_documents(run: Path, pin: Pin, documents: Sequence[JsonObject]) -> None:
    """Copy the pinned bytes of each document into the run directory, so later checks use them."""
    for document in documents:
        if document.get("absent") is True:
            continue
        name = string_field(document, "path", "document")
        target = run / "pinned" / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(pin.doc(name).read_bytes())


def resolve_baseline_sha(
    publications: Sequence[Publication], state: JsonObject, cursor: Boundary | None
) -> tuple[str, str]:
    """The baseline pointer and where it came from: the option, else the cursor's boundary."""
    given = optional_string(state, "baseline")
    if given:
        return resolve_baseline(publications, given), given
    if cursor is None:
        raise GuideError(
            "no cursor boundary and no --baseline: pass --baseline PUBLICATION_OR_POINTER"
        )
    return cursor.pointer_sha256 or resolve_baseline(publications, cursor.id), f"cursor {cursor.id}"


def split_range(
    steps: Sequence[Publication], until: str
) -> tuple[list[Publication], list[Publication]]:
    """The publications up to and including `until`, and those after it (outside the range)."""
    if not until:
        return list(steps), []
    ids = [step.id for step in steps]
    if until not in ids:
        raise GuideError(f"--until {until} is not a publication between the baseline and the pin")
    cut = ids.index(until) + 1
    return list(steps[:cut]), list(steps[cut:])


def inventory_publications(
    steps: Sequence[Publication],
    outside: Sequence[Publication],
    earlier: dict[str, JsonObject],
) -> list[JsonObject]:
    entries: list[JsonObject] = []
    for step in steps:
        entry = step.inventory()
        before = earlier.get(step.id)
        if step in outside:
            entry.update({"verdict": "outside-range", "note": "after the --until publication"})
        elif before is not None and before.get("hashes") == entry["hashes"]:
            entry.update({"verdict": "inherited", "note": "covered by an earlier guide report"})
        else:
            entry.update({"verdict": "unreviewed", "note": ""})
        entry["depth"] = ""
        entries.append(entry)
    return entries


def inventory_issues(
    issues: Sequence[JsonObject], earlier: dict[str, JsonObject]
) -> list[JsonObject]:
    """Issues with their change against the earlier reports: a definition change is reviewed again."""
    snapshots: list[JsonObject] = []
    for issue in issues:
        snapshot = dict(issue)
        before = earlier.get(string_field(issue, "id", "issue"))
        if before is None:
            snapshot.update({"change": "new", "verdict": "unreviewed"})
        elif before.get("definition_sha256") != issue["definition_sha256"]:
            snapshot.update({"change": "changed", "verdict": "unreviewed"})
        else:
            snapshot.update(
                {
                    "change": "unchanged",
                    "verdict": "inherited",
                    "state_changed": before.get("state_sha256") != issue["state_sha256"],
                }
            )
        snapshot.update({"note": "", "depth": ""})
        snapshots.append(snapshot)
    return snapshots


def display_path(hub: Hub, path: Path) -> str:
    """A path as a public report records it: hub-relative, else `~`-relative, else fully resolved.

    The reports are tracked and may be mirrored, so they carry no absolute path into the owner's home.
    """
    resolved = path.resolve()
    for root, prefix in ((hub.root, ""), (Path.home(), "~/")):
        try:
            return prefix + resolved.relative_to(root.resolve()).as_posix()
        except ValueError:
            continue
    return str(resolved)  # never the path as given: `~/../x` or a link into the home would keep it


def build_inventory(
    hub: Hub, state: JsonObject, copy: Path, log: Path
) -> tuple[JsonObject, JsonObject]:
    """The inventory and the coverage skeleton for a run, with every item `unreviewed`."""
    pin = run_pin(state)
    check_copy(pin, copy)
    publications = load_publications(hub)
    guide_dir = pin.doc("docs/guide")
    cursor = read_cursor(guide_dir / "CURSOR.md")
    baseline_sha, source = resolve_baseline_sha(publications, state, cursor)
    steps = walk(publications, pin.sha256, baseline_sha)
    verify_membership(steps, log_hashes(log))
    until = optional_string(state, "until")
    in_range, outside = split_range(steps, until)
    touched = {issue for step in in_range for member in step.members for issue in member.issues}
    earlier_publications, earlier_issues = previous_verdicts(guide_dir)
    entries = inventory_publications(steps, outside, earlier_publications)
    snapshots = inventory_issues(snapshot_issues(hub, touched, earlier_issues), earlier_issues)
    documents = build_documents(pin, walk(publications, pin.sha256, None), in_range)
    pin_step = steps[-1].id if steps else (cursor.id if cursor else "")
    inventory: JsonObject = {
        "schema": SCHEMA,
        "stem": string_field(state, "stem", "run.json"),
        "pin": {
            "pointer_sha256": pin.sha256,
            "repo": display_path(hub, pin.repo),
            "publication": pin_step,
        },
        "baseline": {"pointer_sha256": baseline_sha, "source": source},
        "until": until,
        "publications": entries,
        "issues": snapshots,
        "documents": documents,
        "review_copy": {
            "path": display_path(hub, copy),
            "log": display_path(hub, log),
            "log_sha256": digest_file(log),
        },
    }
    coverage: JsonObject = {
        "schema": SCHEMA,
        "stem": inventory["stem"],
        "mode": string_field(state, "mode", "run.json"),
        "scope": strings_field(state, "scope", "run.json"),
        "started_at": string_field(state, "started_at", "run.json"),
        "completed_at": None,
        "pin": inventory["pin"],
        "baseline": inventory["baseline"],
        "until": until,
        "review_copy": inventory["review_copy"],
        "publications": entries,
        "issues": snapshots,
        "documents": documents,
        "principles": {},
        "findings": [],
        "pending": [],
        "boundary": None,
        "checks": [],
    }
    return inventory, coverage


# --- recheck ----------------------------------------------------------------------------------


def recheck_pending(hub: Hub, state: JsonObject, coverage: JsonObject) -> list[JsonObject]:
    """What was published or changed since the pin. The pin never moves; these are for the next run."""
    pin = run_pin(state)
    pending: list[JsonObject] = []
    current = read_pin(hub.pointer_path)
    if current.sha256 != pin.sha256:
        publications = load_publications(hub)
        try:
            later = walk(publications, current.sha256, pin.sha256)
        except GuideError as error:
            pending.append({"kind": "diverged", "note": str(error)})
        else:
            pending.extend(
                {"kind": "publication", "id": step.id, "pointer_sha256": step.pointer_sha256}
                for step in later
            )
    reviews = load_reviews(hub)
    covered = {
        string_field(item, "id", "coverage issue"): item
        for item in coverage_items(coverage, "issues")
    }
    earlier = previous_verdicts(pin.doc("docs/guide"))[1]
    seen: set[str] = set()
    for issue_id in all_issue_ids(hub):
        now = snapshot_issue(hub, issue_id, reviews)
        if now is None:
            continue
        before = covered.get(issue_id)
        if before is None:
            # Not in this run's inventory (a closed issue unchanged since it was covered, or never
            # covered): the earlier reports decide whether it has changed during the run.
            earlier_item = earlier.get(issue_id)
            if changed_since_covered(now, earlier_item):
                pending.append({"kind": "issue", "id": issue_id, "change": "definition"})
            elif now["state"] != "closed":
                # Open, and not in this run's inventory: never covered (new), or covered and then
                # reopened with its definition unchanged: a state notice, not a new correctness review.
                change = "new" if earlier_item is None else "state"
                pending.append({"kind": "issue", "id": issue_id, "change": change})
            continue
        seen.add(issue_id)
        if changed_since_covered(now, before):
            pending.append({"kind": "issue", "id": issue_id, "change": "definition"})
        elif before.get("state_sha256") != now["state_sha256"]:
            pending.append({"kind": "issue", "id": issue_id, "change": "state"})
    pending.extend(heading_changes(current, coverage))
    pending.extend(contract_changes(current, coverage))
    return pending


def contract_changes(current: Pin, coverage: JsonObject) -> list[JsonObject]:
    """The pinned contracts (every inventoried document that is not a heading document) whose bytes in the
    current checkpoint differ from the pin."""
    pending: list[JsonObject] = []
    for document in coverage_items(coverage, "documents"):
        name = string_field(document, "path", "coverage document")
        if is_heading_document(name):
            continue  # reported by `heading_changes`: an edit, an addition or a deletion
        path = current.doc(name)
        if path.is_file() and digest_file(path) != document.get("sha256"):
            pending.append({"kind": "document", "path": name, "change": "published since the pin"})
    return pending


def heading_changes(current: Pin, coverage: JsonObject) -> list[JsonObject]:
    """What changed in the heading documents (the vision and `docs/game/*.md`) since the pin: an edit, a
    document added to the heading and a document deleted from it. The pinned inventory is compared with the
    current accepted fingerprint, so the pin itself never moves."""
    pinned = {
        string_field(document, "path", "coverage document"): document
        for document in coverage_items(coverage, "documents")
        if is_heading_document(string_field(document, "path", "coverage document"))
    }
    pending: list[JsonObject] = []
    for name in sorted({*pinned, *heading_documents(current.files)}):
        before = pinned.get(name)
        pinned_sha = optional_string(before, "sha256") if before is not None else ""
        was = before is not None and before.get("absent") is not True
        now = name in current.files
        path = current.doc(name)
        if was and not now:
            change = "deleted since the pin"
        elif now and not was:
            change = "added since the pin"
        elif was and (
            current.files[name] != pinned_sha
            or (path.is_file() and digest_file(path) != pinned_sha)
        ):
            change = "published since the pin"
        else:
            continue
        pending.append({"kind": "document", "path": name, "change": change})
    return pending


# --- validation -------------------------------------------------------------------------------


def load_coverage(run: Path) -> JsonObject:
    return read_json(run / "coverage.json")


def pinned_text(run: Path, coverage: JsonObject, name: str) -> str | None:
    """The pinned bytes of a document as stored at `start`, or `None` if missing or altered."""
    for document in coverage_items(coverage, "documents"):
        if document.get("path") == name and document.get("absent") is not True:
            path = run / "pinned" / name
            if path.is_file() and sha256_file(path) == document.get("sha256"):
                return path.read_text()
    return None


def validate_items(coverage: JsonObject) -> list[str]:
    """Every publication and issue has a verdict that its own fields support."""
    problems: list[str] = []
    mode = optional_string(coverage, "mode")
    scope = set(strings_field(coverage, "scope", "coverage")) if "scope" in coverage else set()
    if mode not in MODES:
        problems.append(f"mode `{mode}` is not one of {', '.join(MODES)}")
    for kind in ("publications", "issues"):
        for item in coverage_items(coverage, kind):
            identifier = string_field(item, "id", f"coverage {kind}")
            what = f"{kind[:-1]} {identifier}"
            verdict = optional_string(item, "verdict")
            if verdict not in VERDICTS:
                problems.append(f"{what} has no verdict (found `{verdict}`)")
            elif verdict == "unreviewed":
                problems.append(f"{what} has no verdict: it is still unreviewed")
            elif verdict == "gap" and not optional_string(item, "note"):
                problems.append(f"{what} is a gap without a reason in `note`")
            elif verdict == "reviewed" and not optional_string(item, "depth"):
                problems.append(f"{what} is reviewed without a `depth`")
            if mode == "scoped" and verdict in COVERED and identifier not in scope:
                problems.append(
                    f"a scoped run covers only its scope, but {what} is {verdict} and not in the scope"
                )
    return problems


def validate_principles(coverage: JsonObject, texts: Mapping[str, str | None]) -> list[str]:
    """Every principle key has a pinned section, a quote that is in its document, and its SHA-256.

    `texts` maps each heading document the run pinned to its stored text (`None` when the stored copy is
    missing or altered). A principle names its document, which is the vision unless it says otherwise and
    must be the vision or a `docs/game/*.md` file the run pinned; its quote and section are checked in that
    document alone."""
    problems: list[str] = []
    principles = as_object(coverage.get("principles", {}), "coverage principles")
    for key, raw in principles.items():
        entry = as_object(raw, f"principle {key}")
        # The vision only when `document` is absent; a present value must be a heading document, whatever it is.
        document = entry.get("document", VISION)
        if not isinstance(document, str) or not is_heading_document(document):
            problems.append(
                f"principle {key}: `document` {document!r} is not a heading document "
                f"({VISION} or a docs/game/*.md file)"
            )
            continue
        if document not in texts or texts[document] is None:
            problems.append(
                f"the run has no usable pinned {document} (not in the pin's fingerprint, or its "
                "stored copy is missing or altered), so the principles that name it cannot be checked"
            )
            continue
        text = texts[document] or ""
        if not PRINCIPLE_KEY.fullmatch(key):
            problems.append(f"principle key `{key}` is not of the form `vision.some-name`")
        problems.extend(
            f"principle {key} has no pinned `{part}`"
            for part in ("section", "quote", "sha256", "reading")
            if not optional_string(entry, part)
        )
        quote = optional_string(entry, "quote")
        section = optional_string(entry, "section")
        reading = optional_string(entry, "reading")
        checks = (
            (
                quote and optional_string(entry, "sha256") != sha256_bytes(quote.encode()),
                f"principle {key}: `sha256` is not the SHA-256 of its quote",
            ),
            (
                quote and quote not in text,
                f"principle {key}: the quote is not in {document} at the pin",
            ),
            (
                section and section not in text,
                f"principle {key}: the section `{section}` is not in {document} at the pin",
            ),
            (
                reading and reading not in READINGS,
                f"principle {key}: reading must be one of {', '.join(READINGS)}",
            ),
        )
        problems.extend(message for failed, message in checks if failed)
    return problems


def validate_findings(coverage: JsonObject, report: Path) -> list[str]:
    """The finding IDs of the coverage and the headings of the report agree, both ways."""
    if not report.is_file():
        return [f"the report {report} does not exist"]
    listed = (
        set(strings_field(coverage, "findings", "coverage")) if "findings" in coverage else set()
    )
    headings = {identifier for _marker, identifier, _title in HEADING.findall(report.read_text())}
    problems = [
        f"finding {identifier} is in the coverage but has no report heading"
        for identifier in sorted(listed - headings)
    ]
    problems.extend(
        f"the report has a heading for {identifier} that the coverage does not list"
        for identifier in sorted(headings - listed)
    )
    return problems


def bind_verdict(what: str, verdict: str, original: str, mode: str) -> list[str]:
    """A verdict may not leave the range the inventory fixed, nor invent earlier coverage."""
    if original == "outside-range" and verdict != "outside-range":
        return [f"{what} is outside the run's range (--until) and cannot be {verdict or 'judged'}"]
    if verdict == "outside-range" and original != "outside-range" and mode != "scoped":
        return [f"{what} is marked outside-range but is inside the run's range"]
    if verdict == "inherited" and original != "inherited":
        return [f"{what} is marked inherited, but no earlier guide report covers it unchanged"]
    return []


def bind_items(
    kind: str, have: Sequence[JsonObject], want: Sequence[JsonObject], mode: str
) -> list[str]:
    """The coverage's items are the inventory's, in order; only the judgment fields differ."""
    ids_have = [optional_string(item, "id") for item in have]
    ids_want = [optional_string(item, "id") for item in want]
    if ids_have != ids_want:
        missing = [item for item in ids_want if item not in ids_have]
        extra = [item for item in ids_have if item not in ids_want]
        return [
            f"coverage {kind} do not match the inventory (missing: {', '.join(missing) or 'none'}; "
            f"extra: {', '.join(extra) or 'none'}; order {'differs' if not missing and not extra else 'n/a'})"
        ]
    problems: list[str] = []
    for mine, theirs in zip(have, want, strict=True):
        what = f"{kind[:-1]} {optional_string(mine, 'id')}"
        problems.extend(
            f"{what}: mechanical field `{key}` differs from the inventory"
            for key in sorted(set(mine) | set(theirs))
            if key not in JUDGMENT and mine.get(key) != theirs.get(key)
        )
        problems.extend(
            bind_verdict(
                what, optional_string(mine, "verdict"), optional_string(theirs, "verdict"), mode
            )
        )
    return problems


def validate_binding(coverage: JsonObject, inventory: JsonObject, state: JsonObject) -> list[str]:
    """Compare the coverage with `inventory.json` and `run.json`: only judgments may differ."""
    problems = [
        f"coverage `{key}` differs from the run's recorded value (mechanical fields are the helper's)"
        for source, keys in (
            (state, ("stem", "mode", "scope", "started_at")),
            (inventory, ("pin", "baseline", "until", "review_copy", "documents")),
        )
        for key in keys
        if coverage.get(key) != source.get(key)
    ]
    mode = optional_string(state, "mode")
    for kind in ("publications", "issues"):
        problems.extend(
            bind_items(kind, coverage_items(coverage, kind), coverage_items(inventory, kind), mode)
        )
    return problems


def validate_documents(coverage: JsonObject, run: Path) -> list[str]:
    """Every inventoried document the pin lists is stored in the run directory with its digest."""
    problems: list[str] = []
    for document in coverage_items(coverage, "documents"):
        name = optional_string(document, "path")
        if document.get("absent") is True:
            continue
        stored = run / "pinned" / name
        if not stored.is_file() or sha256_file(stored) != document.get("sha256"):
            problems.append(
                f"the run's stored copy of {name} is missing or does not match its recorded SHA-256"
            )
    return problems


def validate_coverage(coverage: JsonObject, run: Path, report: Path | None) -> list[str]:
    """Every problem with a coverage record, as messages; empty when it can be finished."""
    problems = validate_documents(coverage, run)
    problems.extend(
        validate_binding(coverage, read_json(run / "inventory.json"), read_json(run / "run.json"))
    )
    problems.extend(validate_items(coverage))
    texts = {
        name: pinned_text(run, coverage, name)
        for name in (
            string_field(document, "path", "coverage document")
            for document in coverage_items(coverage, "documents")
        )
        if is_heading_document(name)
    }
    problems.extend(validate_principles(coverage, texts))
    if report is not None:
        problems.extend(validate_findings(coverage, report))
    return problems


# --- the cursor -------------------------------------------------------------------------------


@dataclass(frozen=True)
class Derived:
    """A derived cursor: its text and the position of its boundary on the chain."""

    text: str
    boundary: Boundary
    notes: tuple[str, ...]


def chain_positions(publications: Sequence[Publication], head: str) -> dict[str, int]:
    """Position of each pointer on the chain behind `head`: the oldest link's base is -1."""
    steps = walk(publications, head, None)
    positions = {step.pointer_sha256: index for index, step in enumerate(steps)}
    if steps:
        positions[steps[0].base_sha256] = -1
    return positions


def contiguous_boundary(coverage: JsonObject) -> JsonObject:
    """The boundary a run's own verdicts support: the last publication of the covered prefix."""
    baseline = as_object(field(coverage, "baseline", "coverage"), "coverage baseline")
    result: JsonObject = {
        "id": optional_string(baseline, "source"),
        "patch": "",
        "pointer_sha256": string_field(baseline, "pointer_sha256", "coverage baseline"),
    }
    for item in coverage_items(coverage, "publications"):
        if optional_string(item, "verdict") not in COVERED:
            break
        hashes = strings_field(item, "hashes", "coverage publication")
        result = {
            "id": string_field(item, "id", "coverage publication"),
            "patch": hashes[-1] if hashes else "",
            "pointer_sha256": string_field(item, "pointer_sha256", "coverage publication"),
        }
    return result


def covered_publications(coverage: JsonObject) -> list[JsonObject]:
    return [
        item
        for item in coverage_items(coverage, "publications")
        if optional_string(item, "verdict") in COVERED
    ]


def place_boundary(
    positions: dict[str, int], directories: Sequence[Path], extras: Sequence[Boundary]
) -> tuple[tuple[int, Boundary, str, str], list[tuple[int, JsonObject]], list[str]]:
    """The furthest boundary on the chain among the completed reports, and the reports on it.

    A published boundary with no report behind it (in `extras`) is kept only if no report reaches it."""
    notes: list[str] = []
    best: tuple[int, Boundary, str, str] | None = None
    ordered: list[tuple[int, JsonObject]] = []
    for coverage in load_coverages(directories):
        if coverage.get("mode") == "review-only" or not coverage.get("completed_at"):
            continue
        stem = string_field(coverage, "stem", "coverage")
        boundary = as_object(field(coverage, "boundary", f"coverage {stem}"), f"coverage {stem}")
        sha = string_field(boundary, "pointer_sha256", f"coverage {stem}")
        if sha not in positions:
            notes.append(
                f"{stem} is on a diverging chain (its boundary {sha[:8]} is not behind the current "
                "pointer) and was not used to place the boundary"
            )
            continue
        ordered.append((positions[sha], coverage))
        if best is None or positions[sha] > best[0]:
            value = Boundary(
                string_field(boundary, "id", f"coverage {stem}"),
                optional_string(boundary, "patch"),
                sha,
            )
            best = (positions[sha], value, stem, optional_string(coverage, "completed_at"))
    for extra in extras:
        if extra.pointer_sha256 in positions and (
            best is None or positions[extra.pointer_sha256] > best[0]
        ):
            best = (positions[extra.pointer_sha256], extra, "", "")
    if best is None:
        raise GuideError("no completed guide report to derive a cursor from")
    return best, ordered, notes


def current_readings(
    ordered: Sequence[tuple[int, JsonObject]],
) -> dict[str, tuple[str, str, str]]:
    """Per principle: the latest exercised reading, the stem that set it, and its note."""
    readings: dict[str, tuple[str, str, str]] = {}
    for _position, coverage in sorted(
        ordered, key=lambda pair: (pair[0], optional_string(pair[1], "stem"))
    ):
        for key, raw in as_object(coverage.get("principles", {}), "principles").items():
            entry = as_object(raw, f"principle {key}")
            reading = optional_string(entry, "reading")
            if reading and reading != "not exercised":
                stem = optional_string(coverage, "stem")
                readings[key] = (reading, stem, optional_string(entry, "note"))
    return readings


@dataclass(frozen=True)
class CursorFacts:
    """Everything the cursor file shows, gathered before it is written."""

    boundary: Boundary
    stem: str
    finished: str
    covered_beyond: tuple[str, ...]
    open_findings: tuple[tuple[str, str, str, str], ...]
    after: tuple[str, ...]
    notes: tuple[str, ...]
    readings: dict[str, tuple[str, str, str]]


def render_cursor(facts: CursorFacts) -> str:
    boundary, stem, finished = facts.boundary, facts.stem, facts.finished
    covered_beyond, open_findings = facts.covered_beyond, facts.open_findings
    after, notes, readings = facts.after, facts.notes, facts.readings
    resume = f"Resume after: `{boundary.id}`"
    resume += f" · patch `{boundary.patch}`" if boundary.patch else ""
    resume += f" · pointer `{boundary.pointer_sha256}`" if boundary.pointer_sha256 else ""
    resume += f" · [{stem}]({stem}.md) · {finished}" if stem else ""
    if open_findings:
        nxt = "Process the open findings below, then continue from the boundary."
    elif after:
        nxt = f"Review the {len(after)} publication(s) after the boundary."
    else:
        nxt = "Nothing is pending after the boundary."
    lines = [
        "# Guide cursor",
        "",
        resume,
        "Covered beyond the boundary: " + (", ".join(covered_beyond) if covered_beyond else "none"),
        "",
        "## Next",
        "",
        nxt,
        "",
        "## Open findings",
        "",
    ]
    lines.extend(
        [
            f"- {s}/{i} — {t} — " + ("deferred" if m == "deferred" else "unprocessed")
            for s, i, m, t in open_findings
        ]
        or ["- none"]
    )
    lines.extend(["", "## Pending handoff", ""])
    pending = [f"- publication {identifier} is after the boundary" for identifier in after]
    pending.extend(f"- {note}" for note in notes)
    lines.extend(pending or ["- none"])
    lines.extend(
        ["", "## Alignment", "", "| Principle | Reading | Since | Note |", "|---|---|---|---|"]
    )
    lines.extend(
        f"| {key} | {value[0]} | {value[1]} | {value[2]} |"
        for key, value in sorted(readings.items())
    )
    if not readings:
        lines.append("| — | not exercised | — | |")
    return "\n".join(lines) + "\n"


def derive_cursor(
    publications: Sequence[Publication],
    head: str,
    directories: Sequence[Path],
    extras: Sequence[Boundary],
) -> Derived:
    """The cursor from the guide reports in `directories` (a later directory's file of the same name
    wins), plus the published boundaries in `extras` to keep.

    Deterministic: finishing a run and rebuilding from the reports give the same text."""
    positions = chain_positions(publications, head)
    (position, boundary, stem, finished), ordered, notes = place_boundary(
        positions, directories, extras
    )
    covered_beyond: list[str] = []
    for _position, coverage in ordered:
        for item in covered_publications(coverage):
            label = f"{string_field(item, 'id', 'coverage publication')} ({optional_string(coverage, 'stem')})"
            if (
                positions.get(optional_string(item, "pointer_sha256"), -2) > position
                and label not in covered_beyond
            ):
                covered_beyond.append(label)
    open_findings = [
        found
        for found in report_findings(directories)
        if not found[2].startswith("#") and found[2] != "no-issue"
    ]
    after = [
        step.id
        for step in walk(publications, head, None)
        if positions.get(step.pointer_sha256, -2) > position
    ]
    text = render_cursor(
        CursorFacts(
            boundary,
            stem,
            finished,
            tuple(covered_beyond),
            tuple(open_findings),
            tuple(after),
            tuple(notes),
            current_readings(ordered),
        )
    )
    return Derived(text, boundary, tuple(notes))


# --- commands ---------------------------------------------------------------------------------


def cmd_status(hub: Hub) -> int:
    """The cursor boundary, the open findings and what is pending after it."""
    pin = read_pin(hub.pointer_path)
    guide_dir = pin.doc("docs/guide")
    cursor = read_cursor(guide_dir / "CURSOR.md")
    publications = load_publications(hub)
    print(f"accepted pointer {pin.sha256}")
    if cursor is None:
        print("cursor: none (the first run needs --baseline)")
    else:
        print(f"cursor boundary: {cursor.id} (pointer {cursor.pointer_sha256 or 'not recorded'})")
    findings = [
        (s, i, m, t)
        for s, i, m, t in report_findings([guide_dir])
        if not m.startswith("#") and m != "no-issue"
    ]
    print(f"open findings: {len(findings)}")
    for stem, identifier, marker, title in findings:
        print(f"  {stem}/{identifier} {'(deferred) ' if marker == 'deferred' else ''}{title}")
    chain = walk(publications, pin.sha256, None)
    if cursor is not None and cursor.pointer_sha256:
        positions = chain_positions(publications, pin.sha256)
        at = positions.get(cursor.pointer_sha256)
        later = [step for index, step in enumerate(chain) if at is not None and index > at]
        print(f"pending publications after the boundary: {len(later)}")
        for step in later:
            print(f"  {step.kind} {step.id}")
    covered = previous_verdicts(guide_dir)[1]
    reviews = load_reviews(hub)
    changed: list[str] = []
    for issue_id in all_issue_ids(hub):
        now = snapshot_issue(hub, issue_id, reviews)
        if now is None:
            continue
        before = covered.get(issue_id)
        if changed_since_covered(now, before):
            changed.append(f"{issue_id} (definition changed since it was covered)")
        elif before is None and now["state"] != "closed":
            changed.append(f"{issue_id} (open, not yet covered)")
    print(f"issues pending review: {len(changed)}")
    for line in changed:
        print(f"  {line}")
    runs = hub.pipeline / "coordination" / "guide"
    unfinished = (
        sorted(
            path.name
            for path in runs.iterdir()
            if path.is_dir() and not (path / "coverage.final.json").is_file()
        )
        if runs.is_dir()
        else []
    )
    print(f"unfinished runs: {', '.join(unfinished) if unfinished else 'none'}")
    return 0


def cmd_start(hub: Hub, args: argparse.Namespace, now: Callable[[], float]) -> int:
    if bool(args.copy) != bool(args.log):
        raise GuideError("--copy and --log go together")
    if args.resume:
        if args.baseline or args.until or args.scope or args.mode:
            raise GuideError("--resume takes the baseline, range and mode the run was started with")
        stem = str(args.resume)
        run, state = load_run(hub, stem)
        if (run / "coverage.json").is_file():
            raise GuideError(
                f"run {stem} already has coverage.json; its judgments are not overwritten"
            )
    else:
        mode = str(args.mode or "full")
        scope = [str(item) for item in args.scope or []]
        if mode == "scoped" and not scope:
            raise GuideError("a scoped run needs --scope ISSUE_OR_PATH ...")
        pin = read_pin(hub.pointer_path)
        stem = make_stem(pin, now())
        run = hub.run_dir(stem)
        if run.exists():
            raise GuideError(f"run {stem} already exists")
        state = {
            "schema": SCHEMA,
            "stem": stem,
            "mode": mode,
            "scope": scope,
            "baseline": str(args.baseline or ""),
            "until": str(args.until or ""),
            "pointer_text": pin.text,
            "pointer_sha256": pin.sha256,
            "started_at": utc(now()),
        }
        write_json(run / "run.json", state)
    if not args.copy:
        print(f"pinned {stem} at pointer {string_field(state, 'pointer_sha256', 'run.json')}")
        print(f"next: tools/ck worker guide-{stem}")
        print(f"      tools/ck pijul log --hash-only > {run}/log.txt   (in that copy)")
        print(f"      tools/ck-guide start --resume {stem} --copy <that copy> --log {run}/log.txt")
        return 0
    inventory, coverage = build_inventory(hub, state, Path(args.copy), Path(args.log))
    store_documents(run, run_pin(state), coverage_items(coverage, "documents"))
    write_json(run / "inventory.json", inventory)
    write_json(run / "coverage.json", coverage)
    publications = coverage_items(coverage, "publications")
    print(
        f"run {stem}: {len(publications)} publication(s), {len(coverage_items(coverage, 'issues'))} issue(s)"
    )
    for item in publications:
        print(f"  {item['kind']} {item['id']}: {item['verdict']}")
    print(f"inventory: {run / 'inventory.json'}")
    print(f"coverage skeleton: {run / 'coverage.json'}")
    return 0


def cmd_recheck(hub: Hub, stem: str) -> int:
    run, state = load_run(hub, stem)
    coverage = load_coverage(run)
    pending = recheck_pending(hub, state, coverage)
    coverage["pending"] = pending
    write_json(run / "coverage.json", coverage)
    print(f"{len(pending)} item(s) moved since the pin (recorded as pending; the pin is unchanged)")
    for item in pending:
        print(f"  {json.dumps(item, sort_keys=True)}")
    return 0


def cmd_validate(hub: Hub, stem: str, docs_copy: str | None) -> int:
    run, _state = load_run(hub, stem)
    coverage = load_coverage(run)
    report = Path(docs_copy) / "docs" / "guide" / f"{stem}.md" if docs_copy else None
    problems = validate_coverage(coverage, run, report)
    if problems:
        raise GuideError("coverage is not valid:\n- " + "\n- ".join(problems))
    print("coverage is valid" + ("" if report else " (the report was not checked: no --docs-copy)"))
    return 0


def check_destination(hub: Hub, destination: Path) -> Path:
    """The resolved docs copy, refused before any write if it is one of the hub's record trees.

    Allowed: a copy outside the hub, or one under `.pipeline/workers/`. Refused: the checkpoint the
    pointer names, anything else under `.pipeline/` (checkpoints, submission captures, the queue,
    batches, docs records), the metadata repository, and any directory that contains one of them
    (including the hub root). Symlinks are resolved first."""
    resolved = destination.resolve()
    pipeline = hub.pipeline.resolve()
    trees = [hub.meta.resolve(), read_pin(hub.pointer_path).repo.resolve()]
    refusal = f"refusing --docs-copy {destination}: "
    if resolved.is_relative_to(pipeline) and not resolved.is_relative_to(pipeline / "workers"):
        raise GuideError(refusal + f"it is inside {pipeline}, which holds immutable records")
    for tree in [*trees, pipeline]:
        if tree.is_relative_to(resolved):
            raise GuideError(refusal + f"it is, or contains, the protected tree {tree}")
    for tree in trees:
        if resolved.is_relative_to(tree):
            raise GuideError(refusal + f"it is inside the protected tree {tree}")
    return resolved


def check_outputs(root: Path, targets: Sequence[Path]) -> None:
    """Refuse, before any write, an output directory or file that does not really lie inside the
    docs copy: each target is resolved through every existing symlink, ancestors and leaf included.

    `root` is the resolved docs copy, which `check_destination` has already kept clear of the hub's
    record trees and the metadata repository, so a target that stays inside it cannot reach them: a
    `docs` or `docs/guide` symlink, or a symlinked `CURSOR.md` or coverage file, that leads
    anywhere else is refused."""
    for target in targets:
        real = target.resolve()
        if not real.is_relative_to(root):
            raise GuideError(
                f"refusing to write {target}: it resolves to {real}, outside the docs copy {root}"
            )


def write_cursor(path: Path, derived: Derived) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(derived.text)


def cmd_finish(
    hub: Hub, stem: str, docs_copy: str, review_only: bool, now: Callable[[], float]
) -> int:
    run, state = load_run(hub, stem)
    root = check_destination(hub, Path(docs_copy))
    guide_dir = root / "docs" / "guide"
    check_outputs(
        root,
        [guide_dir, guide_dir / "CURSOR.md", guide_dir / f"{stem}.coverage.md"],
    )
    if review_only != (state.get("mode") == "review-only"):
        raise GuideError(
            "--review-only goes with a run started as --mode review-only, and only with one"
        )
    coverage = load_coverage(run)
    problems = validate_coverage(coverage, run, guide_dir / f"{stem}.md")
    if problems:
        raise GuideError("cannot finish; coverage is not valid:\n- " + "\n- ".join(problems))
    current = read_pin(hub.pointer_path)
    coverage["pending"] = recheck_pending(hub, state, coverage)
    coverage["completed_at"] = utc(now())
    coverage["boundary"] = contiguous_boundary(coverage)
    guide_dir.mkdir(parents=True, exist_ok=True)
    (guide_dir / f"{stem}.coverage.md").write_text(
        f"# Guide coverage {stem}\n\nThe machine record of the run; the report is `{stem}.md`.\n\n"
        f"```json\n{pretty(coverage).rstrip()}\n```\n"
    )
    write_json(run / "coverage.final.json", coverage)
    if review_only:
        print(f"review-only: wrote {stem}.coverage.md; CURSOR.md is untouched")
        return 0
    return write_derived_cursor(hub, current, guide_dir, f"wrote {stem}.coverage.md and CURSOR.md")


def write_derived_cursor(hub: Hub, current: Pin, guide_dir: Path, message: str) -> int:
    """Derive `CURSOR.md` from the published guide files and the docs copy's, and write it there."""
    directories = [current.doc("docs/guide"), guide_dir]
    derived = derive_cursor(
        load_publications(hub), current.sha256, directories, read_cursors(directories)
    )
    write_cursor(guide_dir / "CURSOR.md", derived)
    print(f"{message} (boundary {derived.boundary.id})")
    for note in derived.notes:
        print(f"note: {note}")
    return 0


def cmd_rebuild(hub: Hub, docs_copy: str) -> int:
    root = check_destination(hub, Path(docs_copy))
    guide_dir = root / "docs" / "guide"
    check_outputs(root, [guide_dir, guide_dir / "CURSOR.md"])
    return write_derived_cursor(
        hub, read_pin(hub.pointer_path), guide_dir, "rebuilt CURSOR.md from the reports"
    )


# --- the command line -------------------------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="ck-guide", description=__doc__)
    parser.add_argument("--hub", help="the hub (default: CK_HUB-style marker of this checkout)")
    parser.add_argument(
        "--meta", help="the metadata repository (default: the hub's configured path)"
    )
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("status")
    start = commands.add_parser("start")
    start.add_argument("--baseline")
    start.add_argument("--until")
    start.add_argument("--mode", choices=MODES)
    start.add_argument("--scope", nargs="+")
    start.add_argument("--resume")
    start.add_argument("--copy")
    start.add_argument("--log")
    for name in ("recheck", "validate", "finish"):
        command = commands.add_parser(name)
        command.add_argument("--run", required=True)
        if name in ("validate", "finish"):
            command.add_argument("--docs-copy", required=name == "finish")
        if name == "finish":
            command.add_argument("--review-only", action="store_true")
    rebuild = commands.add_parser("rebuild-cursor")
    rebuild.add_argument("--docs-copy", required=True)
    return parser


def dispatch(hub: Hub, args: argparse.Namespace, now: Callable[[], float]) -> int:
    match args.command:
        case "status":
            return cmd_status(hub)
        case "start":
            return cmd_start(hub, args, now)
        case "recheck":
            return cmd_recheck(hub, str(args.run))
        case "validate":
            return cmd_validate(hub, str(args.run), args.docs_copy)
        case "finish":
            return cmd_finish(hub, str(args.run), str(args.docs_copy), bool(args.review_only), now)
        case _:
            return cmd_rebuild(hub, str(args.docs_copy))


def main(argv: Sequence[str] | None = None, *, now: Callable[[], float] = time.time) -> int:
    args = build_parser().parse_args(argv)
    try:
        return dispatch(locate_hub(args.hub, args.meta), args, now)
    except (GuideError, OSError, ValueError, records.RecordError) as error:
        print(f"ck-guide: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
