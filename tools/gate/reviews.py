"""Immutable review receipts and fail-closed coverage calculations."""

from __future__ import annotations

import hashlib
import os
import time
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Literal, Protocol, TypedDict

from . import classes, meta, pipeline_api, provenance, sessions
from .pointer import canonical, file_digest
from .records import (
    AgentSession,
    AnySubject,
    Author,
    BootstrapReceipt,
    Issue,
    IssueRef,
    Manifest,
    ManifestInfo,
    MetaEvent,
    OwnerReviewer,
    ProvenancePayload,
    ReviewerIdentity,
    ReviewReceipt,
    ReviewRow,
    SpecSubject,
    Subject,
    SubmissionSubject,
    is_agent,
    is_manifest_v2,
    is_owner_reviewer,
    is_review_row,
    load_review_receipt,
    read_json,
    review_row,
)

ReviewKind = Literal["spec", "submission", "batch"]
Person = Mapping[str, object]  # an author, submitter or reviewer: only identity and session matter


def families(authors: Sequence[Author]) -> set[str]:
    result = set()
    for author in authors:
        values = author.get("families")
        if values:
            result.update(values)
        else:
            result.add(author.get("family", "unknown"))
    return result or {"unknown"}


def requirements(authors: Sequence[Author]) -> set[str]:
    source = families(authors)
    if source == {"claude"}:
        return {"codex"}
    if source == {"codex"}:
        return {"claude"}
    return {"claude", "codex"}


def independent(
    reviewer: Person, authors: Sequence[Person], submitters: Sequence[Person] | None = None
) -> bool:
    """The reviewer differs (session and identity) from every author and every submitter.

    A submitter is excluded from reviewing but is never part of the author family set."""
    return all(
        reviewer.get("session") != person.get("session")
        and reviewer.get("identity") != person.get("identity")
        for person in [*authors, *(submitters or [])]
    )


class Derived(TypedDict):
    authors: list[Author]
    submitter: Author | None
    payloads: dict[str, ProvenancePayload | None]
    digests: dict[str, str | None]


def derive(manifest: Manifest) -> Derived | None:
    """Authors, submitter and provenance digests of a submission, recomputed from its capture.

    The manifest's own `authors` are never trusted or used as a fallback. None means the
    capture cannot vouch for the manifest (not clean, fingerprint differs, provenance cannot be
    read, or a recorded digest disagrees with the patch), so the submission has no coverage."""
    pipeline = pipeline_api.load()
    capture = Path(manifest["repo"])
    try:
        pipeline.clean(capture)
        if pipeline.fingerprint(capture) != manifest.get("fingerprint"):
            return None
        payloads = provenance.from_capture(capture, manifest["hashes"])
    except (RuntimeError, OSError, ValueError, KeyError, TypeError):
        return None
    digests = {
        patch: provenance.payload_digest(payload) if payload else None
        for patch, payload in payloads.items()
    }
    recorded = (manifest.get("provenance") if is_manifest_v2(manifest) else None) or {}
    if any(value is not None and digests.get(patch) != value for patch, value in recorded.items()):
        return None
    return {
        "authors": provenance.authors(payloads),
        "submitter": manifest.get("submitter") if is_manifest_v2(manifest) else None,
        "payloads": payloads,
        "digests": digests,
    }


OWNER: OwnerReviewer = {
    "identity": "human",
    "role": "owner",
    "family": "human",
    "owner_signoff": True,
}


def is_owner_receipt(receipt: ReviewRow) -> bool:
    """An approved, unrevoked, owner-labelled receipt (exactly the shape `write_owner_review` writes)."""
    reviewer = receipt["reviewer"]
    return (
        receipt.get("verdict") == "approved"
        and receipt.get("revoked") is not True
        and receipt.get("incremental_over") is None
        and is_owner_reviewer(reviewer)
        and reviewer.get("role") == "owner"
        and reviewer.get("identity") == "human"
        and reviewer.get("family") == "human"
    )


class ManifestRows(Protocol):
    """Coverage rows by submission id: a dict, or rows built when first asked for."""

    def get(self, key: str, /) -> ManifestInfo | None: ...


def coverage(  # noqa: PLR0913  # explicit arguments; a parameter object would obscure the call sites
    authors: Sequence[Author],
    receipts: Sequence[ReviewRow | BootstrapReceipt],
    subject: AnySubject,
    manifests: ManifestRows | None = None,
    _seen: frozenset[str] = frozenset(),
    submitters: Sequence[Author] | None = None,
    owner: bool = True,
) -> bool:
    """Covered by one owner receipt for this exact subject, or by the independent AI reviews.

    The owner rule applies only to the subject itself: the recursive `incremental_over` check
    passes `owner=False`, so an owner receipt is never an incremental parent, and an owner receipt
    never counts toward a Claude or Codex family below."""
    rows: ManifestRows = manifests if manifests is not None else {}
    if (
        owner
        and ("submission" in subject or "batch" in subject)  # never a spec subject
        and any(
            is_review_row(r) and is_owner_receipt(r) and r.get("subject") == subject
            for r in receipts
        )
    ):
        return True
    accepted: dict[str, set[object]] = {}  # family -> the sessions that reviewed
    for receipt in receipts:
        reviewer = receipt["reviewer"]
        if (
            receipt.get("verdict") == "approved"
            and receipt.get("subject") == subject
            and receipt.get("revoked") is not True
            and reviewer.get("role") == "reviewer"
            and reviewer.get("class") == "A"
            and reviewer.get("effort") == "high"
            and reviewer.get("family") in ("claude", "codex")
            and independent(reviewer, authors, submitters)
        ):
            parent_id = receipt.get("incremental_over")
            if parent_id:
                if not isinstance(parent_id, str):
                    continue  # not a receipt id: it cannot name a covered parent
                parent = rows.get(parent_id)
                if not parent or parent.get("unreadable") or parent_id in _seen:
                    continue
                parent_subject = parent.get("subject")
                parent_hashes = set(parent.get("hashes", []))
                submission = subject.get("submission")
                current = rows.get(submission if isinstance(submission, str) else "") or {}
                current_hashes = set(current.get("hashes", []))
                if (
                    not parent_subject
                    or not parent_hashes <= current_hashes
                    or not coverage(
                        parent.get("authors", []),
                        receipts,
                        parent_subject,
                        manifests,
                        _seen | {parent_id},
                        [submitter] if (submitter := parent.get("submitter")) else [],
                        owner=False,
                    )
                ):
                    continue
            accepted.setdefault(reviewer["family"], set()).add(reviewer.get("session"))
    needed = requirements(authors)
    return all(accepted.get(family) for family in needed) and (
        len(needed) < 2 or len(set.union(*(accepted[f] for f in needed))) >= 2  # noqa: PLR2004  # two independent families are required
    )


def _reviewer(declared: str | None) -> tuple[ReviewerIdentity, AgentSession]:
    session = sessions.resolve()
    if not is_agent(session) and not declared:
        raise RuntimeError("Human-authored reviews are not valid gate receipts")
    role = sessions.effective_role(session, declared)
    if role != "reviewer" or not is_agent(session):
        raise RuntimeError("Reviews require the reviewer role")
    mapping = classes.readiness(role, session["client"], session["models"], session.get("current"))
    if mapping.get("family") not in ("claude", "codex"):
        raise RuntimeError("Unknown or multi-family sessions cannot review")
    identity: ReviewerIdentity = {
        "identity": session["identity"],
        "client": session["client"],
        "session": session["session"],
        "role": role,
        "class": mapping["class"],
        "family": mapping["family"],
        "model": mapping["model"],
        "effort": mapping["effort"],
    }
    return identity, session


def write_review(  # noqa: PLR0913  # explicit arguments; a parameter object would obscure the call sites
    store: meta.Store,
    kind: ReviewKind,
    subject: Subject,
    verdict: str,
    report_path: Path,
    *,
    authors: Sequence[Author],
    submitters: Sequence[Author] | None = None,
    issues: list[IssueRef] | None = None,
    base_examined: str | None = None,
    incremental_over: str | None = None,
    role: str | None = None,
) -> str:
    if verdict not in {"approved", "changes-requested", "rejected"}:
        raise RuntimeError(f"Unsupported review verdict {verdict!r}")
    reviewer, session = _reviewer(role)
    if not independent(reviewer, authors):
        raise RuntimeError("Reviewer is an author of this subject")
    if not independent(reviewer, [], submitters):
        raise RuntimeError("Reviewer is the submitter of this subject")
    report = report_path.read_bytes()
    store.ensure()
    rid = hashlib.sha256(
        canonical({"subject": subject, "reviewer": reviewer, "at": time.time_ns()})
    ).hexdigest()[:20]
    receipt: ReviewReceipt = {
        "schema": "ck-review/1",
        "id": rid,
        "kind": kind,
        "subject": subject,
        "issues": issues or [],
        "base_examined": base_examined,
        "reviewer": {**reviewer, "evidence": session["evidence"]},
        "verdict": verdict,
        "report_sha256": hashlib.sha256(report).hexdigest(),
        "incremental_over": incremental_over,
        "recorded": time.time_ns(),
    }
    store.write(store.repo / "reviews" / f"{rid}.json", canonical(receipt), f"Review {kind} {rid}")
    # The report is a separate immutable event file; its bytes are bound by receipt.
    store.write(store.repo / "reviews" / f"{rid}.md", report, f"Review report {rid}")
    return rid


def require_owner(summary: list[str], expect: str) -> None:
    """Refuse unless this is the owner at a terminal and confirms with the last six characters.

    Every check names its failure and nothing has been written yet. There is no seam that fakes it."""
    if "CK_SESSION_EVIDENCE" in os.environ:  # any present value, including an empty one
        raise RuntimeError("Owner sign-off refused: CK_SESSION_EVIDENCE is set")
    if not sessions.resolve().get("human"):
        raise RuntimeError("Owner sign-off refused: this is a bound agent session, not the owner")
    ancestor = sessions.agent_ancestor()
    if ancestor:
        raise RuntimeError(f"Owner sign-off refused: an agent client is an ancestor: {ancestor}")
    if not sessions.terminals_present():
        raise RuntimeError("Owner sign-off refused: standard input and output must be terminals")
    for line in summary:
        print(line)
    answer = sessions.read_confirmation(f"Type {expect} to confirm: ").strip()
    if answer != expect:
        raise RuntimeError("Owner sign-off refused: the confirmation did not match")


def write_owner_review(  # noqa: PLR0913  # explicit arguments; a parameter object would obscure the call sites
    store: meta.Store,
    kind: ReviewKind,
    subject: Subject,
    report: bytes,
    *,
    issues: list[IssueRef] | None = None,
    base_examined: str | None = None,
) -> str:
    """The owner's approval: the same receipt shape as an AI review, labelled `owner_signoff`.

    It carries no class, model, effort or session, so it never claims to be an AI review. The
    report hash is that of the bytes written."""
    store.ensure()
    rid = hashlib.sha256(
        canonical({"subject": subject, "reviewer": OWNER, "at": time.time_ns()})
    ).hexdigest()[:20]
    receipt: ReviewReceipt = {
        "schema": "ck-review/1",
        "id": rid,
        "kind": kind,
        "subject": subject,
        "issues": issues or [],
        "base_examined": base_examined,
        "reviewer": OWNER.copy(),
        "verdict": "approved",
        "report_sha256": hashlib.sha256(report).hexdigest(),
        "incremental_over": None,
        "recorded": time.time_ns(),
    }
    store.write(
        store.repo / "reviews" / f"{rid}.json", canonical(receipt), f"Owner sign-off {kind} {rid}"
    )
    store.write(store.repo / "reviews" / f"{rid}.md", report, f"Owner sign-off report {rid}")
    return rid


def receipt_rows(store: meta.Store) -> list[ReviewRow]:
    store.clean()
    revoked = {
        event.get("review") for event in store.read_events() if event["kind"] == "revocation"
    }
    output: list[ReviewRow] = []
    for path in sorted((store.repo / "reviews").glob("*.json")):
        receipt = load_review_receipt(read_json(path), str(path))
        output.append(review_row(receipt, revoked=receipt["id"] in revoked))
    return output


def revoke(
    store: meta.Store, rid: str, reason: str, role: str | None = None, *, owner: bool = False
) -> None:
    """Revoke a receipt. `owner=True` is the owner path: it may revoke only owner receipts, and the
    caller has already run `require_owner` for this receipt."""
    if not reason.strip():
        raise RuntimeError("Review revocation requires a reason")
    receipts = receipt_rows(store)
    target = next((row for row in receipts if row.get("id") == rid), None)
    if target is None:
        raise RuntimeError(f"Unknown review receipt {rid}")
    original = target["reviewer"]
    actor: ReviewerIdentity | OwnerReviewer
    if owner:
        if not is_owner_reviewer(original):
            raise RuntimeError("The owner path cannot revoke an AI review receipt")
        actor = OWNER.copy()
    else:
        actor, _ = _reviewer(role)
        if actor.get("session") != original.get("session") and not independent(actor, [original]):
            raise RuntimeError("Only the original or an independent reviewer may revoke")
    path = store.repo / "events" / f"{time.time_ns()}-revocation.json"
    store.write(
        path,
        canonical({"kind": "revocation", "review": rid, "actor": actor, "reason": reason}),
        f"Revoke review {rid}",
    )


def subject_for_spec(issue: Issue) -> SpecSubject:
    return {
        "issue": issue["id"],
        "revision": issue["revision"],
        "spec_sha256": issue["spec_sha256"],
    }


def subject_for_submission(manifest_path: Path, manifest: Manifest) -> SubmissionSubject:
    fingerprint = manifest["fingerprint"]
    return {
        "submission": manifest["id"],
        "manifest_sha256": file_digest(manifest_path),
        "fingerprint_sha256": hashlib.sha256(canonical(fingerprint)).hexdigest(),
    }


def live(manifest: Manifest, events: Sequence[MetaEvent], accepted: set[str]) -> bool:
    identifier = manifest.get("id")
    if identifier in accepted:
        return False
    return not any(
        event.get("kind") in ("withdrawn", "supersession", "superseded")
        and (event.get("submission") == identifier or identifier in event.get("supersedes", []))
        for event in events
    )


def blocking(
    manifest: Manifest,
    manifests: Sequence[Manifest],
    receipts: Sequence[ReviewRow],
    accepted: set[str],
    dead: set[str],
) -> bool:
    hashes = set(manifest.get("hashes", []))
    for other in manifests:
        oid = other.get("id")
        if oid in accepted or oid in dead:
            continue
        if any(
            receipt.get("kind") == "submission"
            and receipt["subject"].get("submission") == oid
            and receipt.get("verdict") == "changes-requested"
            and not receipt.get("revoked")
            and (oid == manifest.get("id") or hashes & set(other.get("hashes", [])))
            for receipt in receipts
        ):
            return True
    return False
