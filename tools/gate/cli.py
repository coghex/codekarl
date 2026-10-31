"""Command line for the activated Codekarl review gate."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import sys
import tempfile
import time
from pathlib import Path
from typing import Literal

from . import classes, lease, meta, owned, pipeline_api, postpublish, provenance, reviews, sessions
from .pointer import canonical, file_digest, read_pointer
from .records import (
    AcceptedPointer,
    Author,
    BatchValidated,
    FileEntry,
    Issue,
    IssueRef,
    ManifestV2,
    MetaEvent,
    Subject,
    is_agent,
    is_manifest_v2,
    is_owner_reviewer,
    load_accepted_pointer,
    load_manifest,
    load_quarantine,
    load_queue_summary,
    read_json,
)

ROLES = ("manager", "worker", "reviewer")


# Retired commands are not registered with argparse, so `ck --help` and its usage line list only
# current commands. They are still named here, ahead of parsing, so an old invocation (with any
# arguments, or `--help`) gets one line of migration guidance instead of "invalid choice".
RETIRED = {
    "retry": "ck retry is retired; resubmit quarantined work with ck submit --supersedes ID",
    "accept": (
        "ck accept is removed; use ck batch prepare, then ck batch status BID: publish with "
        "ck batch publish BID when its publication_basis is single-submission, otherwise obtain "
        "ck review batch first"
    ),
}


def parser() -> argparse.ArgumentParser:  # noqa: PLR0915  # linear workflow kept whole so failure ordering stays visible
    root = argparse.ArgumentParser(prog="ck")
    commands = root.add_subparsers(dest="command", required=True)
    commands.add_parser("init")
    status = commands.add_parser("status")
    status.set_defaults(action="status")
    worker = commands.add_parser("worker")
    worker.add_argument("name")
    commands.add_parser("doctor")
    record = commands.add_parser("record")
    record.add_argument("-m", "--message", required=True)
    record.add_argument("--issue", required=True)
    record.add_argument("--role", choices=ROLES)
    record.add_argument("paths", nargs="*")
    issues = commands.add_parser("issue")
    issue_cmds = issues.add_subparsers(dest="action", required=True)
    new = issue_cmds.add_parser("new")
    new.add_argument("--title", required=True)
    new.add_argument(
        "--type", required=True, choices=("bug", "feature", "task", "design", "debt", "docs")
    )
    new.add_argument("--priority", default="P2")
    new.add_argument("--spec", required=True)
    new.add_argument("--role", choices=ROLES)
    show = issue_cmds.add_parser("show")
    show.add_argument("issue")
    listing = issue_cmds.add_parser("list")
    listing.add_argument("--ready", action="store_true")
    comment = issue_cmds.add_parser("comment")
    comment.add_argument("issue")
    comment.add_argument("--file", required=True)
    edit = issue_cmds.add_parser("edit")
    edit.add_argument("issue")
    for key in ("title", "type", "priority", "assign", "depends"):
        edit.add_argument("--" + key)
    edit.add_argument("--role", choices=ROLES)
    spec = issue_cmds.add_parser("spec")
    spec.add_argument("issue")
    spec.add_argument("--file", required=True)
    spec.add_argument("--role", choices=ROLES)
    close = issue_cmds.add_parser("close")
    close.add_argument("issue")
    close.add_argument("--reason", required=True)
    close.add_argument("--role", choices=ROLES)
    reopen = issue_cmds.add_parser("reopen")
    reopen.add_argument("issue")
    reopen.add_argument("--reason", required=True)
    reopen.add_argument("--role", choices=ROLES)
    mcmd = commands.add_parser("meta")
    mcmd.add_subparsers(dest="action", required=True).add_parser("recover")
    review = commands.add_parser("review")
    review.add_argument("kind", choices=("submission", "spec", "batch", "revoke"))
    review.add_argument("subject")
    review.add_argument("--verdict", choices=("approved", "changes-requested", "rejected"))
    review.add_argument("--report", type=Path)
    review.add_argument("--incremental-over")
    review.add_argument("--reason")
    review.add_argument("--blame", nargs="*")
    review.add_argument("--role", choices=ROLES)
    review.add_argument("--owner", action="store_true")
    diagnostic = commands.add_parser("session")
    diagnostic.add_argument("--role", choices=ROLES)
    withdraw = commands.add_parser("withdraw")
    withdraw.add_argument("submission")
    withdraw.add_argument("--reason", required=True)
    batch = commands.add_parser("batch")
    batch_sub = batch.add_subparsers(dest="action", required=True)
    prepare = batch_sub.add_parser("prepare")
    prepare.add_argument("--no-split", action="store_true")
    for action in ("status", "publish"):
        sub = batch_sub.add_parser(action)
        sub.add_argument("batch")
    cancel = batch_sub.add_parser("cancel")
    cancel.add_argument("batch")
    cancel.add_argument("--reason", required=True)
    docs = commands.add_parser("docs")
    docs.add_argument("paths", nargs="*")
    docs.add_argument("-m", "--message")
    docs.add_argument("--from", dest="from_copy", type=Path)
    docs.add_argument("--record", action="store_true")
    gc = commands.add_parser("gc")
    gc.add_argument("--apply", action="store_true")
    commands.add_parser("sync")
    submit = commands.add_parser("submit")
    submit.add_argument("--issue", required=True)
    submit.add_argument("--spec", required=True)
    submit.add_argument("--supersedes")
    submit.add_argument("--role", choices=ROLES)
    return root


def run(root: Path, hub: Path, argv: list[str] | None = None) -> int:
    """One `ck` command; if it advanced the accepted pointer, the owner's hook starts after it ends."""
    postpublish.begin()
    try:
        return _dispatch(root, hub, argv)
    finally:
        postpublish.after_command(hub)


def _dispatch(root: Path, hub: Path, argv: list[str] | None = None) -> int:  # noqa: PLR0911,PLR0912,PLR0915  # PLR0911: one exit code per command outcome; PLR0912: linear workflow kept whole so failure ordering stays visible; PLR0915: linear workflow kept whole so failure ordering stays visible
    arguments = sys.argv[1:] if argv is None else argv
    if arguments and arguments[0] in RETIRED:
        raise RuntimeError(RETIRED[arguments[0]])
    args = parser().parse_args(arguments)
    if args.command == "init":
        raise RuntimeError("ck init is one-time setup and is unavailable after activation")
    if args.command == "status":
        pointer = hub / ".pipeline/accepted-v2.json"
        if pointer.exists():
            state = load_accepted_pointer(read_json(pointer), str(pointer))
            accepted = set(state.get("submissions", []))
            legacy_late = []
            for path in sorted((hub / ".pipeline/queue").glob("*.json")):
                queued = load_queue_summary(read_json(path), str(path))
                if queued.get("schema") != 2 and queued.get("id") not in accepted:  # noqa: PLR2004  # schema version 2
                    legacy_late.append(
                        {
                            "id": queued.get("id", path.stem),
                            "status": "legacy-late",
                            "spec": queued.get("spec"),
                        }
                    )
            print(
                json.dumps(
                    {
                        "accepted": state.get("submissions", []),
                        **_owner_signed_members(hub, meta.Store(hub), state.get("submissions", [])),
                        "receipt": state.get("receipt"),
                        "previous": state.get("previous"),
                        "legacy": legacy_late,
                        "schema": 2,
                    },
                    indent=2,
                )
            )
        else:
            print("Review gate is not activated")
        return 0
    if args.command == "worker":
        if not re.fullmatch(r"[a-z0-9_-]+", args.name):
            raise RuntimeError("Worker name must contain lowercase letters, digits, '-' or '_'.")
        destination = hub / ".pipeline/workers" / args.name
        if destination.exists():
            raise RuntimeError(f"Worker already exists: {destination}")
        # The pointer's repository is leased before the clone and the Pijul child inherits the lease;
        # if it was collected first, the pointer is read again (up to 3 attempts).
        with lease.source(lambda: read_pointer(hub)) as state:
            _create_worker(Path(state["repo"]), destination, hub)
        print(destination)
        return 0
    if args.command == "doctor":
        import check  # noqa: PLC0415  # command-local import: other commands do not load this module

        check.doctor()
        return 0
    if args.command == "session":
        print(json.dumps(_session(args.role), indent=2))
        return 0
    if args.command == "gc":
        from . import gc  # noqa: PLC0415  # command-local import

        return gc.command(hub, apply=args.apply)
    store = meta.Store(hub)
    if args.command == "issue":
        return meta.issue_command(store, args)
    if args.command == "meta" and args.action == "recover":
        store.recover()
        return 0
    if args.command == "record":
        from .provenance import record  # noqa: PLC0415  # command-local import

        record(root, hub, args.message, args.issue, args.paths, args.role)
        return 0
    if args.command == "submit":
        return _submit(root, hub, store, args)
    if args.command == "batch":
        from .batch import Manager  # noqa: PLC0415  # command-local import

        manager = Manager(hub)
        if args.action == "prepare":
            print(manager.prepare(args.no_split))
        elif args.action == "status":
            print(json.dumps(manager.status(args.batch), indent=2))
        elif args.action == "cancel":
            manager.cancel(args.batch, args.reason)
        elif args.action == "publish":
            manager.publish(args.batch)
        return 0
    if args.command == "review":
        return _review(hub, store, args)
    if args.command == "docs":
        from .docsqueue import Queue  # noqa: PLC0415  # command-local import

        if args.paths == ["flush"] and not args.message:
            print(Queue(hub).flush())
            return 0
        if args.paths[:1] == ["resolve"]:
            if len(args.paths) != 2:  # noqa: PLR2004  # a pair of paths is required
                raise RuntimeError("Usage: ck docs resolve DID [--record -m MESSAGE]")
            print(Queue(hub).resolve(args.paths[1], root, record=args.record, message=args.message))
            return 0
        if not args.from_copy:
            raise RuntimeError("ck docs requires --from COPY")
        Queue(hub).capture(args.from_copy, args.message or "Update documentation", args.paths)
        return 0
    if args.command == "sync":
        from .docsqueue import sync  # noqa: PLC0415  # command-local import

        sync(root, hub)
        return 0
    if args.command == "withdraw":
        _withdraw(store, args.submission, args.reason)
        return 0
    raise RuntimeError(f"Unsupported command: {args.command}")


def _doc_state(repo: Path) -> dict[str, FileEntry]:
    """Fingerprint (content and executable bit) every tracked ordinary-documentation file."""
    pipeline = pipeline_api.load()
    return {name: entry for name, entry in pipeline.files(repo).items() if pipeline.doc_path(name)}


def _closure_files(root: Path, hub: Path, state: AcceptedPointer, closure: set[str]) -> list[str]:
    """Replay the closure on a copy of the accepted base and return the paths it changes.

    Pulling a change also pulls its dependencies, so any hash beyond the closure and the
    accepted base is a dependency on queued documentation. Each patch is applied on its own so
    that ordinary documentation changed by one patch is refused even when a later patch cancels
    it, including through directory moves that `pijul log --files` does not expand.
    """
    pipeline = pipeline_api.load()
    base_hashes = set(state["fingerprint"]["hashes"])
    ordered = [
        digest
        for digest in reversed(pipeline.pijul(root, "log", "--hash-only").splitlines())
        if digest in closure
    ]
    with tempfile.TemporaryDirectory(prefix="ck-closure-") as directory:
        probe = Path(directory) / "probe"
        pipeline.clone(Path(state["repo"]), probe, hub)
        docs = _doc_state(probe)
        for digest in ordered:
            conflicts = pipeline.pull(probe, root, [digest])
            if conflicts:
                raise RuntimeError(f"Submission does not apply to the accepted state: {conflicts}")
            if pipeline.hashes(probe) - base_hashes - closure:
                raise RuntimeError(
                    "Submission depends on queued documentation; "
                    "record code that does not depend on it"
                )
            replayed = _doc_state(probe)
            if replayed != docs:
                raise RuntimeError("Ordinary documentation must use ck docs")
            docs = replayed
        if pipeline.hashes(probe) - base_hashes != closure:
            raise RuntimeError("Submission closure did not replay completely")
        accepted_files = state["fingerprint"]["files"]
        probe_files = pipeline.files(probe)
        changed = {
            name
            for name in set(probe_files) | set(accepted_files)
            if probe_files.get(name) != accepted_files.get(name)
        }
    if any(pipeline.doc_path(name) for name in changed):
        raise RuntimeError("Ordinary documentation must use ck docs")
    return sorted(changed)


def _ready_issues(store: meta.Store, identifiers: list[str]) -> list[Issue]:
    """The named issues, each of which must be open, dependency-closed and spec-approved."""
    rows = [store.show(identifier) for identifier in identifiers]
    ready = [row for row in rows if row is not None and meta.ready(store, row)]
    if len(ready) != len(rows):
        raise RuntimeError("Every named issue must be open, dependency-closed and spec-approved")
    return ready


def _submit(root: Path, hub: Path, store: meta.Store, args: argparse.Namespace) -> int:  # noqa: PLR0915  # linear workflow kept whole so failure ordering stays visible
    pipeline = pipeline_api.load()

    from . import docsqueue, locks  # noqa: PLC0415  # command-local import
    from . import pointer as pointer_module  # noqa: PLC0415  # command-local import
    from .batch import Manager  # noqa: PLC0415  # command-local import

    issue_ids = (
        [meta._issue_id(args.issue)]
        if "," not in args.issue
        else [meta._issue_id(item) for item in args.issue.split(",")]
    )
    _ready_issues(store, issue_ids)
    submitter = meta._authors(args.role)[0]
    pipeline.clean(root)
    with locks.state(hub):
        state = pointer_module.read_pointer(hub)
        base_hashes = set(state["fingerprint"]["hashes"])
        queued = docsqueue.outstanding_hashes(docsqueue._queue(hub), base_hashes)
    closure = pipeline.hashes(root) - base_hashes - queued
    if not closure:
        raise RuntimeError("No new recorded changes to submit")
    changed = _closure_files(root, hub, state, closure)
    supersedes = [item for item in (args.supersedes or "").split(",") if item]
    existing = {row["id"]: row for row in Manager(hub).manifests()}
    events = store.events()
    dead = {
        event["submission"]
        for event in events
        if event.get("kind") in ("withdrawn", "supersession") and "submission" in event
    }
    accepted = set(state.get("submissions", []))
    requested_issues = set(issue_ids)
    for old_id in supersedes:
        old = existing.get(old_id)
        if old is None or old_id in dead or old_id in accepted:
            raise RuntimeError(f"Cannot supersede non-live submission {old_id}")
        if not requested_issues & {item["id"] for item in old.get("issues", [])}:
            raise RuntimeError(f"Superseded submission {old_id} shares no issue")
    quarantine = {
        path.stem: load_quarantine(read_json(path), str(path))
        for path in (hub / ".pipeline/quarantine").glob("*.json")
    }
    superseded_set = set(supersedes)
    for old in existing.values():
        if old["id"] in accepted or old["id"] in dead or old["id"] in superseded_set:
            continue
        if set(old.get("hashes", [])) & closure and old["id"] in quarantine:
            raise RuntimeError(
                f"Closure contains quarantined submission {old['id']}; supersede it explicitly"
            )
    capture_id = str(time.time_ns())
    capture = hub / ".pipeline/submissions" / capture_id
    pipeline.clone(root, capture, hub)
    try:
        # Authors come from the immutable patches in the capture, never from the submitter.
        payloads = provenance.from_capture(capture, sorted(closure))
        named = set(issue_ids)
        for payload in payloads.values():
            listed = payload.get("issues") if payload else None
            if listed is not None:
                named.update(meta._issue_id(item) for item in listed)
        issue_rows = _ready_issues(store, sorted(named))
        fingerprint = pipeline.fingerprint(capture)
    except BaseException:
        shutil.rmtree(capture, ignore_errors=True)
        raise
    authors = provenance.authors(payloads)
    manifest: ManifestV2 = {
        "schema": 2,
        "id": capture_id,
        "repo": str(capture),
        "base": {
            "pointer_sha256": file_digest(Path(state["_pointer_path"])),
            "fingerprint_sha256": hashlib.sha256(canonical(state["fingerprint"])).hexdigest(),
        },
        "hashes": sorted(closure),
        "files": changed,
        "issues": [
            {"id": row["id"], "revision": row["revision"], "spec_sha256": row["spec_sha256"]}
            for row in issue_rows
        ],
        "provenance": {
            patch: provenance.payload_digest(payload) if payload else None
            for patch, payload in sorted(payloads.items())
        },
        "authors": authors,
        "submitter": submitter,
        "supersedes": supersedes,
        "fingerprint": fingerprint,
        "spec": args.spec,
    }
    path = hub / ".pipeline/queue" / f"{capture_id}.json"
    with locks.state(hub), locks.meta(store.repo):
        path.parent.mkdir(parents=True, exist_ok=True)
        from .pointer import atomic_bytes  # noqa: PLC0415  # command-local import

        atomic_bytes(path, canonical(manifest))
        for old_id in supersedes:
            supersession: MetaEvent = {
                "kind": "supersession",
                "submission": old_id,
                "by": capture_id,
                "authors": authors,
            }
            store.append_event_locked(supersession, f"Supersede {old_id} with {capture_id}")
    print(capture_id)
    return 0


def _owner_signed_members(
    hub: Path, store: meta.Store, accepted: list[str]
) -> dict[str, dict[str, list[str]]]:
    """`{"owner_signoff": {submission: [receipt ids]}}` for accepted members whose coverage came
    from an owner receipt (labelled "owner sign-off", never an AI verdict); empty otherwise."""
    if not (store.repo / ".pijul").exists():
        return {}
    rows = [row for row in reviews.receipt_rows(store) if reviews.is_owner_receipt(row)]
    found: dict[str, list[str]] = {}
    for member in accepted:
        path = hub / ".pipeline/queue" / f"{member}.json"
        if not path.exists():
            continue
        subject = reviews.subject_for_submission(path, load_manifest(read_json(path), str(path)))
        ids = [row["id"] for row in rows if row.get("subject") == subject]
        if ids:
            found[member] = ids
    return {"owner_signoff": found} if found else {}


def _submission_manifest(path: Path) -> ManifestV2:
    """The queued manifest a gate review is about: legacy submissions cannot be reviewed."""
    raw = read_json(path)
    if not isinstance(raw, dict) or raw.get("schema") != 2:  # noqa: PLR2004  # schema version 2
        raise RuntimeError("Legacy submissions cannot receive gate reviews")
    manifest = load_manifest(raw, str(path))
    if not is_manifest_v2(manifest):
        raise RuntimeError("Legacy submissions cannot receive gate reviews")
    return manifest


def _validated_batch(hub: Path, bid: str) -> BatchValidated:
    from .batch import batch_path, read_validated  # noqa: PLC0415  # command-local import

    directory = batch_path(hub, bid)
    if not (directory / "validated.json").exists():
        raise RuntimeError("Batch has not been validated")
    return read_validated(directory)


def _owner_subject(
    hub: Path, args: argparse.Namespace
) -> tuple[Literal["submission", "batch"], Subject, list[IssueRef], list[str]]:
    """(kind, subject, issues, summary lines) of the submission or batch an owner sign-off covers."""
    if args.kind == "submission":
        path = hub / ".pipeline/queue" / f"{args.subject}.json"
        manifest = _submission_manifest(path)
        if reviews.derive(manifest) is None:
            raise RuntimeError("Submission provenance cannot be verified from its capture")
        issues = manifest.get("issues", [])
        summary = [
            f"Owner sign-off for submission {args.subject}",
            "issues: " + (", ".join(f"{i['id']}@{i.get('revision')}" for i in issues) or "none"),
            "patches: " + ", ".join(manifest.get("hashes", [])),
            "files: " + ", ".join(manifest.get("files", [])),
        ]
        return "submission", reviews.subject_for_submission(path, manifest), issues, summary
    from .batch import subject as batch_subject  # noqa: PLC0415  # command-local import

    validated = _validated_batch(hub, args.subject)
    summary = [
        f"Owner sign-off for batch {args.subject}",
        "member submissions: " + ", ".join(validated["members"]),
    ]
    return "batch", batch_subject(hub, args.subject), [], summary


def _owner_review(hub: Path, store: meta.Store, args: argparse.Namespace) -> int:
    """`ck review submission|batch SUBJECT --owner` and `ck review revoke RID --owner`."""
    if args.kind == "spec":
        raise RuntimeError("--owner is refused for spec reviews: spec approvals stay AI reviews")
    if args.role or args.incremental_over:
        raise RuntimeError("--owner cannot be combined with --role or --incremental-over")
    if args.kind == "revoke":
        if not args.reason:
            raise RuntimeError("ck review revoke requires --reason")
        target = next((r for r in reviews.receipt_rows(store) if r.get("id") == args.subject), None)
        if target is None:
            raise RuntimeError(f"Unknown review receipt {args.subject}")
        if not is_owner_reviewer(target["reviewer"]):
            raise RuntimeError("The owner path cannot revoke an AI review receipt")
        summary = [
            f"Owner revocation of receipt {args.subject} ({target.get('kind')})",
            f"subject: {json.dumps(target.get('subject'), sort_keys=True)}",
            f"reason: {args.reason}",
        ]
        reviews.require_owner(summary, args.subject[-6:])
        reviews.revoke(store, args.subject, args.reason, owner=True)
        return 0
    if args.verdict != "approved":
        raise RuntimeError(
            "--owner accepts only --verdict approved (an owner who disagrees does not sign)"
        )
    if not args.report:
        raise RuntimeError("Review requires --verdict and --report")
    kind, subject, issues, summary = _owner_subject(hub, args)
    report = args.report.read_bytes()
    summary.append(f"report: {args.report} sha256 {hashlib.sha256(report).hexdigest()}")
    reviews.require_owner(summary, args.subject[-6:])
    # The terminal wait can be long: refuse before any write if the subject or the report moved.
    if _owner_subject(hub, args)[1] != subject or args.report.read_bytes() != report:
        raise RuntimeError(
            "Owner sign-off refused: the subject or the report changed during confirmation"
        )
    rid = reviews.write_owner_review(
        store,
        kind,
        subject,
        report,
        issues=issues,
        base_examined=str(hub / ".pipeline/accepted-v2.json"),
    )
    if kind == "batch":
        from .batch import Manager  # noqa: PLC0415  # command-local import

        Manager(hub).approve(args.subject)
    print(rid)
    return 0


def _review(hub: Path, store: meta.Store, args: argparse.Namespace) -> int:  # noqa: PLR0912,PLR0915  # linear workflow kept whole so failure ordering stays visible
    if getattr(args, "owner", False):  # callers that build their own args may omit the flag
        return _owner_review(hub, store, args)
    if args.kind == "revoke":
        if not args.reason:
            raise RuntimeError("ck review revoke requires --reason")
        reviews.revoke(store, args.subject, args.reason, args.role)
        return 0
    if not args.verdict or not args.report:
        raise RuntimeError("Review requires --verdict and --report")

    subject: Subject
    authors: list[Author]
    submitters: list[Author]
    issues: list[IssueRef]
    if args.kind == "spec":
        identifier, revision = args.subject.split("@", 1)
        issue = store.show(identifier)
        if not issue or issue.get("revision") != revision:
            raise RuntimeError("Spec subject is missing or stale")
        subject = reviews.subject_for_spec(issue)
        authors = issue.get("spec_authors", [])
        submitters = []
        issues = [{"id": identifier, "revision": revision, "spec_sha256": issue["spec_sha256"]}]
    elif args.kind == "submission":
        path = hub / ".pipeline/queue" / f"{args.subject}.json"
        manifest = _submission_manifest(path)
        subject = reviews.subject_for_submission(path, manifest)
        derived = reviews.derive(manifest)
        if derived is None:
            raise RuntimeError("Submission provenance cannot be verified from its capture")
        authors = derived["authors"]
        submitters = [derived["submitter"]] if derived["submitter"] else []
        issues = manifest.get("issues", [])
    elif args.kind == "batch":
        from .batch import Manager  # noqa: PLC0415  # command-local import
        from .batch import subject as batch_subject  # noqa: PLC0415  # command-local import

        manager = Manager(hub)
        validated = _validated_batch(hub, args.subject)
        subject = batch_subject(hub, args.subject)
        authors = validated.get("authors", [])
        submitters = validated.get("submitters", [])
        issues = []
    else:
        raise RuntimeError("Batch reviews require ck batch status and a validated record")
    rid = reviews.write_review(
        store,
        args.kind,
        subject,
        args.verdict,
        args.report,
        authors=authors,
        submitters=submitters,
        issues=issues,
        base_examined=str(hub / ".pipeline/accepted-v2.json"),
        incremental_over=args.incremental_over,
        role=args.role,
    )
    if args.kind == "batch":
        if args.verdict == "approved":
            manager.approve(args.subject)
        elif args.verdict == "rejected":
            for identifier in args.blame or []:
                path = hub / ".pipeline/queue" / f"{identifier}.json"
                blamed = load_manifest(read_json(path), str(path))
                derived = reviews.derive(blamed)
                declared = blamed if is_manifest_v2(blamed) else None
                reviews.write_review(
                    store,
                    "submission",
                    reviews.subject_for_submission(path, blamed),
                    "changes-requested",
                    args.report,
                    # Independence only: a capture that cannot vouch keeps the declared authors.
                    authors=derived["authors"]
                    if derived
                    else (declared["authors"] if declared else []),
                    submitters=[derived["submitter"]] if derived and derived["submitter"] else [],
                    issues=declared["issues"] if declared else [],
                    role=args.role,
                )
            manager.reject(args.subject, args.blame or [])
        else:
            raise RuntimeError("Batch integration verdict must be approved or rejected")
    print(rid)
    return 0


def _session(declared: str | None) -> dict[str, object]:
    session = sessions.resolve()
    if not is_agent(session):
        sessions.effective_role(session, declared)
        return {"human": True}
    result: dict[str, object] = {
        "identity": session["identity"],
        "client": session["client"],
        "session": session["session"],
        "role": None,
        "current": session.get("current"),
    }
    try:
        role = sessions.effective_role(session, declared)
        result["role"] = role
        if role is None:
            raise RuntimeError("unbound agent session must declare --role")
        mapping = classes.readiness(
            role, session["client"], session["models"], session.get("current")
        )
        shown: dict[str, object] = {
            key: mapping.get(key) for key in ("class", "family", "model", "effort")
        }
        if "off_class" in mapping:
            shown["off_class"] = mapping["off_class"]
        result["mapping"] = shown
    except RuntimeError as error:
        result["error"] = str(error)
    return result


def _create_worker(source: Path, destination: Path, hub: Path) -> None:
    """Build the worker in a private container, then promote it to `destination` atomically.

    The clone and all setup (hub link, hook approval) happen at a fresh path inside a `mkdtemp`
    container; the finished repository takes the destination name only through an exclusive rename,
    which fails rather than replace anything. The destination name is never created, opened or
    removed otherwise: on any failure only the container is removed, through its descriptor. A
    same-user process deliberately racing on the private container is outside this guarantee."""
    if destination.exists() or destination.is_symlink():
        raise RuntimeError(f"Worker already exists: {destination}")
    pipeline = pipeline_api.load()
    destination.parent.mkdir(parents=True, exist_ok=True)
    container = owned.Container(destination.parent, f".{destination.name}-")
    try:
        clone = container.path / "repo"
        pipeline.clone(source, clone, hub)
        pipeline.pijul(clone, "hooks", "approve")
        try:
            container.promote("repo", destination.name)
        except FileExistsError as error:
            raise RuntimeError(f"Worker already exists: {destination}") from error
    finally:
        try:
            container.discard()
        finally:
            container.close()


def _withdraw(store: meta.Store, identifier: str, reason: str) -> None:
    if not reason.strip():
        raise RuntimeError("A reason is required")
    store.ensure()
    path = store.repo / "events" / f"{time.time_ns()}-withdrawn.json"
    event: MetaEvent = {"kind": "withdrawn", "submission": identifier, "reason": reason}
    store.write(path, canonical(event), f"Withdraw {identifier}")


def main(root: Path, hub: Path) -> None:
    try:
        # First thing, before dispatch and before any project lock: hold this checkpoint for the
        # command's whole life, so a collector never takes it from under a lazy import or a child.
        lease.enroll_controller(Path(__file__).resolve().parent.parent)
        run(root, hub)
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        print(f"ck: {error}", file=sys.stderr)
        raise SystemExit(1) from error
