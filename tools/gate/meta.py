"""Append-only issue metadata stored in a sibling Pijul repository."""

from __future__ import annotations

import argparse
import json
import re
import time
from pathlib import Path
from typing import Literal

from . import classes, lease, locks, sessions
from .pointer import atomic_bytes, canonical, file_digest
from .records import (
    Author,
    Counters,
    Issue,
    IssueEvent,
    IssueFields,
    MetaEvent,
    SpecRevision,
    fold_issue,
    is_agent,
    load_counters,
    load_gate_config,
    load_issue_event,
    load_meta_event,
    load_pointer,
    read_json,
)


def _run(repo: Path, *args: str) -> str:
    result = lease.run(["pijul", *args], check=False, cwd=repo, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f"pijul {' '.join(args)}: {result.stderr.strip()}")
    return str(result.stdout)


def _clean(repo: Path) -> None:
    status = json.loads(_run(repo, "status", "--output-format", "json"))
    if status.get("changes") or status.get("untracked"):
        raise RuntimeError("Metadata repository is dirty; run ck meta recover")


class Store:
    def __init__(self, hub: Path) -> None:
        self.hub = hub
        gate_file = hub / ".pipeline/gate.json"
        if gate_file.exists():
            self.repo = Path(load_gate_config(read_json(gate_file), str(gate_file))["meta_repo"])
        else:
            self.repo = hub.parent / "codekarl-meta"
        self.lock_path = self.repo / ".lock"

    def ensure(self) -> None:
        if not (self.repo / ".pijul").exists():
            self.repo.mkdir(parents=True, exist_ok=True)
            _run(self.repo, "init")
            (self.repo / ".ignore").write_text("/.lock\n/.tmp*\n")
            atomic_bytes(
                self.repo / "counters.json", canonical({"issue": 0, "event": 0, "review": 0})
            )
            _run(self.repo, "add", "--recursive", ".")
            _run(self.repo, "record", "-am", "Initialize Codekarl metadata", "--author", "ck-meta")
        self.repo.mkdir(parents=True, exist_ok=True)

    def clean(self) -> None:
        if not (self.repo / ".pijul").exists():
            raise RuntimeError("Metadata repository is not initialized")
        _clean(self.repo)

    def _record(self, message: str) -> None:
        _run(self.repo, "add", "--recursive", ".")
        _run(self.repo, "record", "-am", message, "--author", "ck-meta")

    def write(self, path: Path, data: bytes, message: str) -> None:
        self.ensure()
        with locks.meta(self.repo):
            self.clean()
            if path.exists():
                raise RuntimeError(f"Metadata event already exists: {path.relative_to(self.repo)}")
            path.parent.mkdir(parents=True, exist_ok=True)
            atomic_bytes(path, data)
            self._record(message)

    def update_counters(self, key: Literal["issue", "event", "review"]) -> int:
        path = self.repo / "counters.json"
        value = (
            load_counters(read_json(path), str(path))
            if path.exists()
            else Counters(issue=0, event=0, review=0)
        )
        number = value.get(key, 0) + 1
        value[key] = number
        atomic_bytes(path, canonical(value))
        return number

    def recover(self) -> None:
        self.ensure()
        with locks.meta(self.repo):
            # Only incomplete atomic-write temporaries are removed; complete JSON/MD
            # events are preserved and recorded as the operator requested.
            for path in self.repo.rglob(".*.tmp"):
                path.unlink(missing_ok=True)
            status = json.loads(_run(self.repo, "status", "--output-format", "json"))
            if status.get("changes") or status.get("untracked"):
                self._record("Recover metadata repository")

    def issues(self) -> list[Issue]:
        self.clean()
        result: list[Issue] = []
        root = self.repo / "issues"
        for issue_dir in sorted(root.glob("CK-*")) if root.exists() else []:
            issue = self.show(issue_dir.name)
            if issue:
                result.append(issue)
        return result

    def read_events(self) -> list[MetaEvent]:
        """Every file of `events/`, validated, in name order (the caller has checked the repository)."""
        directory = self.repo / "events"
        return (
            [
                load_meta_event(read_json(path), str(path))
                for path in sorted(directory.glob("*.json"))
            ]
            if directory.exists()
            else []
        )

    def events(self) -> list[MetaEvent]:
        self.clean()
        return self.read_events()

    def append_event_locked(self, event: MetaEvent, message: str) -> Path:
        """Append one event when the caller already holds state then meta locks."""
        directory = self.repo / "events"
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / f"{time.time_ns()}-{event['kind']}.json"
        atomic_bytes(path, canonical(event))
        self._record(message)
        return path

    def show(self, issue_id: str) -> Issue | None:
        self.clean()
        issue_id = _issue_id(issue_id)
        root = self.repo / "issues" / issue_id
        events = [
            load_issue_event(read_json(path), str(path))
            for path in (sorted(root.glob("[0-9]*-*.json")) if root.exists() else [])
        ]
        if not events:
            return None
        revisions = sorted(root.glob("spec/r[0-9]*.md"), key=lambda item: int(item.stem[1:]))
        spec = (
            SpecRevision(revisions[-1].stem, file_digest(revisions[-1]), str(revisions[-1]))
            if revisions
            else None
        )
        return fold_issue(issue_id, events, spec)

    def new_issue(
        self, title: str, kind: str, priority: str, spec_file: Path, role: str | None = None
    ) -> str:
        if kind not in {"bug", "feature", "task", "design", "debt", "docs"}:
            raise RuntimeError(f"Unsupported issue type {kind!r}")
        if priority not in {"P0", "P1", "P2", "P3"}:
            raise RuntimeError(f"Unsupported priority {priority!r}")
        content = spec_file.read_bytes()
        if not content.strip():
            raise RuntimeError("Issue spec must not be empty")
        authors = _authors(role)
        self.ensure()
        with locks.meta(self.repo):
            self.clean()
            number = self.update_counters("issue")
            issue_id = f"CK-{number}"
            now = time.time_ns()
            issue_root = self.repo / "issues" / issue_id
            spec_path = issue_root / "spec/r1.md"
            spec_path.parent.mkdir(parents=True, exist_ok=True)
            spec_path.write_bytes(content)
            event: IssueEvent = {
                "id": issue_id,
                "kind": "created",
                "at": now,
                "fields": {
                    "title": title,
                    "type": kind,
                    "priority": priority,
                    "state": "open",
                    "depends": [],
                },
                "spec_sha256": file_digest(spec_path),
                "authors": authors,
            }
            atomic_bytes(issue_root / "0001-created.json", canonical(event))
            self._record(f"Create {issue_id}")
            return issue_id

    def append_issue_event(
        self,
        issue_id: str,
        kind: str,
        fields: IssueFields,
        content: bytes | None = None,
        role: str | None = None,
    ) -> None:
        issue_id = _issue_id(issue_id)
        authors = _authors(role)
        self.ensure()
        with locks.meta(self.repo):
            self.clean()
            current = self.show(issue_id)
            if current is None:
                raise RuntimeError(f"Unknown issue {issue_id}")
            root = self.repo / "issues" / issue_id
            event_paths = sorted(root.glob("[0-9]*-*.json"))
            seq = int(event_paths[-1].name.split("-", 1)[0]) + 1
            event: IssueEvent = {
                "id": issue_id,
                "kind": kind,
                "at": time.time_ns(),
                "fields": fields,
                "authors": authors,
            }
            if content is not None:
                revision_paths = sorted(
                    root.glob("spec/r[0-9]*.md"), key=lambda item: int(item.stem[1:])
                )
                revision = int(revision_paths[-1].stem[1:]) + 1 if revision_paths else 1
                spec_path = root / f"spec/r{revision}.md"
                spec_path.parent.mkdir(parents=True, exist_ok=True)
                spec_path.write_bytes(content)
                event["spec_sha256"] = file_digest(spec_path)
                fields = {"revision": spec_path.stem}
                event["fields"] = fields
            atomic_bytes(root / f"{seq:04d}-{kind}.json", canonical(event))
            self._record(f"{kind} {issue_id}")


def _issue_id(value: str) -> str:
    value = value.upper()
    if not re.fullmatch(r"CK-[1-9][0-9]*", value):
        raise RuntimeError(f"Invalid issue ID {value!r}")
    return value


def _authors(declared: str | None = None) -> list[Author]:
    session = sessions.resolve()
    role = sessions.effective_role(session, declared)
    if not is_agent(session):
        return [{"identity": "human", "family": "human", "role": "owner"}]
    if role is None:
        raise RuntimeError("unbound agent session must declare --role")
    mapping = classes.readiness(role, session["client"], session["models"], session.get("current"))
    author: Author = {
        "identity": session["identity"],
        "client": session["client"],
        "session": session["session"],
        "role": role,
        "family": mapping.get("family", "unknown"),
        "families": mapping.get("families", [mapping.get("family", "unknown")]),
    }
    if "off_class" in mapping:
        author["off_class"] = mapping["off_class"]
        author["models"] = sessions.compress_history(session["models"])
        author["evidence"] = session["evidence"]
    return [author]


def ready(store: Store, issue: Issue) -> bool:
    if issue.get("state") != "open":
        return False
    for dependency in issue.get("depends", []):
        other = store.show(dependency)
        if not other or other.get("state") != "closed":
            return False
    if not issue.get("revision"):
        return False
    from . import reviews  # noqa: PLC0415  # lazy: reviews imports meta (import cycle)

    return reviews.coverage(
        issue.get("spec_authors", []),
        reviews.receipt_rows(store),
        reviews.subject_for_spec(issue),
    )


def issue_command(store: Store, args: argparse.Namespace) -> int:  # noqa: PLR0912  # linear workflow kept whole so failure ordering stays visible
    if args.action == "new":
        identifier = store.new_issue(
            args.title, args.type, args.priority, Path(args.spec), getattr(args, "role", None)
        )
        print(identifier)
    elif args.action == "show":
        value = store.show(args.issue)
        if value is None:
            raise RuntimeError(f"Unknown issue {args.issue}")
        print(json.dumps(value, indent=2, ensure_ascii=False))
    elif args.action == "list":
        rows = store.issues()
        if args.ready:
            rows = [row for row in rows if ready(store, row)]
        print(json.dumps(rows, indent=2, ensure_ascii=False))
    elif args.action == "spec":
        store.append_issue_event(
            args.issue, "spec", {}, Path(args.file).read_bytes(), getattr(args, "role", None)
        )
    elif args.action == "comment":
        content = Path(args.file).read_bytes()
        store.write(
            store.repo / "issues" / args.issue / "comments" / f"{time.time_ns()}.md",
            content,
            f"Comment {args.issue}",
        )
    elif args.action in {"edit", "close", "reopen"}:
        fields: IssueFields = {}
        if getattr(args, "title", None) is not None:
            fields["title"] = args.title
        if getattr(args, "type", None) is not None:
            fields["type"] = args.type
        if getattr(args, "priority", None) is not None:
            fields["priority"] = args.priority
        if getattr(args, "assign", None) is not None:
            fields["assign"] = args.assign
        if getattr(args, "depends", None) is not None:
            fields["depends"] = args.depends.split(",")
        if args.action == "close":
            if args.reason not in {"wontfix", "duplicate"}:
                state_path = store.hub / ".pipeline/accepted-v2.json"
                accepted = (
                    load_pointer(read_json(state_path), str(state_path)).get("submissions", [])
                    if state_path.exists()
                    else []
                )
                if args.reason not in accepted:
                    raise RuntimeError(
                        "Closing requires an accepted submission ID or wontfix/duplicate"
                    )
            fields["state"] = "closed"
            fields["reason"] = args.reason
        if args.action == "reopen":
            fields["state"] = "open"
            fields["reason"] = args.reason
        store.append_issue_event(args.issue, args.action, fields, role=getattr(args, "role", None))
    else:
        raise RuntimeError(f"Unsupported issue action: {args.action}")
    return 0
