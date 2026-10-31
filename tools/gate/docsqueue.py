"""FIFO ordinary-documentation capture, queue, flush, and recovery."""

from __future__ import annotations

import hashlib
import json
import time
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path

from . import lease, locks, pipeline_api, pointer
from .pointer import atomic_bytes, canonical, file_digest
from .records import (
    AcceptedPointer,
    DocsEntry,
    DocsJournal,
    DocsPublishReceipt,
    is_hold,
    load_accepted_pointer,
    load_docs_entry,
    load_docs_journal,
    load_pijul_status,
    load_pointer,
    read_json,
)

Hook = Callable[..., None]  # a test seam: called with the stage name and keyword details


def _queue(hub: Path) -> list[DocsEntry]:
    root = hub / ".pipeline/docs"
    return [
        load_docs_entry(read_json(path), str(path))
        for path in sorted(root.glob("*.json"))
        if path.name != "counters.json"
    ]


def _blocked_entries(queue: Sequence[DocsEntry]) -> set[str]:
    """Return pending entries that depend on an unresolved docs conflict."""
    blocked = {
        row["id"]: {"hashes": set(row.get("hashes", [])), "files": set(row.get("files", []))}
        for row in queue
        if row.get("state") == "conflict"
    }
    changed = True
    while changed:
        changed = False
        for row in queue:
            if row.get("state") not in ("pending", "blocked") or row["id"] in blocked:
                continue
            hashes = set(row.get("hashes", []))
            files = set(row.get("files", []))
            if any(
                hashes & prior["hashes"] or files & prior["files"] for prior in blocked.values()
            ):
                blocked[row["id"]] = {"hashes": hashes, "files": files}
                changed = True
    return set(blocked)


def _active(hub: Path) -> str | None:
    directory = hub / ".pipeline/batches"
    for path in directory.glob("*/") if directory.exists() else []:
        if not any(
            (path / f"{name}.json").exists()
            for name in ("published", "failed", "rejected", "cancelled")
        ):
            return path.name
    boot = hub / ".pipeline/bootstrap"
    for path in boot.glob("*/") if boot.exists() else []:
        if (path / "frozen.json").exists() and not any(
            (path / f"{name}.json").exists() for name in ("published", "restored")
        ):
            return path.name
    return None


# An account name shorter than this ("me", "al", "bob") would match ordinary words in any document and
# refuse legitimate docs, so below it only the home path is checked; the path check always applies.
ACCOUNT_NAME_MIN = 4


def _read_bundle(source: Path, names: Sequence[str]) -> dict[str, bytes]:
    """The bytes of each named file in `source`; a missing file is a deletion and is left out."""
    found: dict[str, bytes] = {}
    for name in names:
        try:
            found[name] = (source / name).read_bytes()
        except FileNotFoundError:
            continue
    return found


def _refuse_home(contents: Mapping[str, bytes]) -> None:
    """Refuse the whole bundle, naming each offending file, before anything is queued or recorded."""
    offenders = home_offenders(contents, Path.home())
    if offenders:
        raise RuntimeError(
            f"ck docs refused, nothing queued or published: {', '.join(offenders)} "
            "contain the current user's home path or account name"
        )


def home_offenders(contents: Mapping[str, bytes], home: Path) -> list[str]:
    """The names whose bytes hold the user's home path or account name (both from `home`)."""
    needles = [home.name.encode()] if len(home.name) >= ACCOUNT_NAME_MIN else []
    if len(home.parts) > 1:
        needles.append(str(home).encode())
    return sorted(name for name, data in contents.items() if any(n in data for n in needles))


class Queue:
    def __init__(self, hub: Path, *, hook: Hook | None = None) -> None:
        self.hub = hub
        self.directory = hub / ".pipeline/docs"
        self.hook = hook

    def _point(self, name: str, **details: object) -> None:
        if self.hook is not None:
            self.hook(name, **details)

    def capture(  # noqa: PLR0912,PLR0915  # linear workflow kept whole so failure ordering stays visible
        self, source: Path, message: str, names: list[str], *, bootstrap: bool = False
    ) -> str:
        pipeline = pipeline_api.load()
        if not names or not message.strip():
            raise RuntimeError("ck docs requires a message and at least one path")
        normalized = []
        for name in names:
            path = Path(name)
            if path.is_absolute() or ".." in path.parts or not pipeline.doc_path(path.as_posix()):
                raise RuntimeError(
                    f"{name}: only README.md and docs/**/*.md are ordinary documentation"
                )
            normalized.append(path.as_posix())
        # The bundle is read once; these bytes are checked and, below, the very bytes published.
        snapshot = _read_bundle(source, normalized)
        _refuse_home(snapshot)
        with locks.state(self.hub):
            bootstrap_id = None
            legacy_pointer = self.hub / ".pipeline/accepted.json"
            accepted_bytes = legacy_pointer.read_bytes()
            if bootstrap:
                bootstrap_root = self.hub / ".pipeline/bootstrap"
                pending = []
                for bootstrap_dir in bootstrap_root.glob("*/") if bootstrap_root.exists() else []:
                    if not (bootstrap_dir / "frozen.json").exists():
                        continue
                    if any(
                        (bootstrap_dir / f"{name}.json").exists()
                        for name in ("published", "restored")
                    ):
                        continue
                    if any(
                        (bootstrap_dir / f"{name}.json").exists()
                        for name in ("failed", "cancelled", "restoring")
                    ):
                        pending.append(bootstrap_dir.name)
                if pending:
                    raise RuntimeError(
                        f"Bootstrap restore is pending; run cancel {sorted(pending)[0]}"
                    )
                if (self.hub / ".pipeline/accepted-v2.json").exists():
                    raise RuntimeError("Bootstrap docs helper is unavailable after activation")
                active_holds = []
                for bootstrap_dir in bootstrap_root.glob("*/") if bootstrap_root.exists() else []:
                    if not (bootstrap_dir / "frozen.json").exists():
                        continue
                    if any(
                        (bootstrap_dir / f"{name}.json").exists()
                        for name in ("published", "restored")
                    ):
                        continue
                    hold_path = bootstrap_dir / "hold.json"
                    if hold_path.exists() and accepted_bytes == hold_path.read_bytes():
                        active_holds.append(bootstrap_dir)
                if len(active_holds) != 1:
                    raise RuntimeError(
                        "Bootstrap docs window is not active; use legacy tools/ck docs"
                    )
                bootstrap_id = active_holds[0].name
            bootstrap_dirs = [
                path
                for path in (self.hub / ".pipeline/bootstrap").glob("*/")
                if (path / "frozen.json").exists()
                and not any((path / f"{name}.json").exists() for name in ("published", "restored"))
            ]
            active_holds = []
            for bootstrap_dir in bootstrap_dirs:
                hold_path = bootstrap_dir / "hold.json"
                if (bootstrap_dir / "restoring.json").exists():
                    raise RuntimeError(
                        f"Bootstrap restore is pending; run cancel {bootstrap_dir.name}"
                    )
                if hold_path.exists():
                    if accepted_bytes == hold_path.read_bytes():
                        active_holds.append(bootstrap_dir)
                    else:
                        held = load_pointer(json.loads(accepted_bytes), str(legacy_pointer))
                        if is_hold(held) and held["bootstrap"] == bootstrap_dir.name:
                            raise RuntimeError(
                                f"accepted.json no longer equals bootstrap hold {bootstrap_dir.name}"
                            )
            if bootstrap_dirs and not active_holds and not bootstrap:
                raise RuntimeError(
                    "Bootstrap window is not held; recover it with prepare or cancel before docs"
                )
            state: AcceptedPointer
            try:
                state = pointer.read_pointer(self.hub)
            except RuntimeError as error:
                raw = load_pointer(json.loads(accepted_bytes), str(legacy_pointer))
                if not is_hold(raw):
                    raise error
                bootstrap_id = raw["bootstrap"]
                bootstrap_dir = self.hub / ".pipeline/bootstrap" / bootstrap_id
                hold_path = bootstrap_dir / "hold.json"
                if not hold_path.exists() or accepted_bytes != hold_path.read_bytes():
                    raise RuntimeError(
                        f"accepted.json no longer equals bootstrap hold {bootstrap_id}"
                    ) from error
                if any(
                    (bootstrap_dir / f"{name}.json").exists()
                    for name in ("failed", "cancelled", "restoring")
                ):
                    raise RuntimeError(
                        f"Bootstrap restore is pending; run cancel {bootstrap_id}"
                    ) from error
                saved = bootstrap_dir / "legacy-pointer.json"
                state = load_accepted_pointer(read_json(saved), str(saved))
                state["_pointer_path"] = str(legacy_pointer)
            base = Path(state["repo"])
            queue = _queue(self.hub)
            blocked_ids = _blocked_entries(queue)
            conflicted = {
                path for row in queue if row["id"] in blocked_ids for path in row.get("files", [])
            }
            overlap = conflicted & set(normalized)
            if overlap:
                raise RuntimeError(
                    f"Documentation path is blocked; resolve {sorted(overlap)} with ck docs resolve"
                )
            identifier = f"D{time.time_ns()}"
            destination = self.directory / "captures" / identifier
            required = set(state["fingerprint"]["hashes"])
            for row in queue:
                if row.get("state") == "pending" and set(row.get("files", [])) & set(normalized):
                    required.update(row.get("hashes", []))
            if not required <= pipeline.hashes(source):
                raise RuntimeError("Working copy is stale for these docs paths; run ck sync first")
            pipeline.clone(base, destination, self.hub)
            live_entries = [
                row
                for row in queue
                if row.get("state") == "pending"
                and row["id"] not in blocked_ids
                and set(row.get("files", [])) & set(normalized)
            ]
            for row in live_entries:
                conflicts = pipeline.pull(destination, Path(row["repo"]), row.get("hashes", []))
                if conflicts:
                    raise RuntimeError(f"Queued docs conflict; resolve {row['id']} before capture")
            for name in normalized:
                target_path = destination / name
                if name in snapshot:
                    target_path.parent.mkdir(parents=True, exist_ok=True)
                    target_path.write_bytes(snapshot[name])
                else:
                    target_path.unlink(missing_ok=True)
            pipeline.pijul(destination, "add", "--recursive", ".")
            pipeline.pijul(destination, "record", "-am", message, "--author", "ck-docs")
            hashes = sorted(pipeline.hashes(destination) - set(state["fingerprint"]["hashes"]))
            manifest: DocsEntry = {
                "id": identifier,
                "state": "pending",
                "created": time.time_ns(),
                "base_pointer_sha256": file_digest(Path(state["_pointer_path"])),
                "repo": str(destination),
                "hashes": hashes,
                "files": normalized,
                "fingerprint": pipeline.fingerprint(destination),
                "message": message,
            }
            atomic_bytes(self.directory / f"{identifier}.json", canonical(manifest))
            if bootstrap or bootstrap_id or _active(self.hub):
                print(f"queued {identifier}")
                return identifier
            self.flush_locked()
            return identifier

    def flush(self) -> int:
        with locks.state(self.hub):
            return self.flush_locked()

    def resolve(self, did: str, root: Path, *, record: bool, message: str | None) -> Path | str:
        pipeline = pipeline_api.load()
        entry_path = self.directory / f"{did}.json"
        if not entry_path.exists():
            raise RuntimeError(f"Unknown docs entry {did}")
        entry = load_docs_entry(read_json(entry_path), str(entry_path))
        if entry.get("state") != "conflict":
            raise RuntimeError(f"Docs entry {did} is not conflicted")
        resolution = self.directory / f"{did}-resolve"
        if not record:
            if not resolution.exists():
                state = pointer.read_pointer(self.hub)
                pipeline.clone(Path(state["repo"]), resolution, self.hub)
                conflicts = pipeline.pull(resolution, Path(entry["repo"]), entry["hashes"])
                if not conflicts:
                    raise RuntimeError(
                        "Conflict could not be reproduced from current accepted state"
                    )
            print(resolution)
            return resolution
        if not message or not message.strip():
            raise RuntimeError("Resolution recording requires -m MESSAGE")
        status = load_pijul_status(
            json.loads(pipeline.pijul(resolution, "status", "--output-format", "json")),
            f"{resolution}: pijul status",
        )
        changed = {row["path"] for row in status["changes"]} | set(status["untracked"])
        if any(not pipeline.doc_path(name) for name in changed):
            raise RuntimeError("Docs resolution changed a non-documentation path")
        # The resolution admits new bytes: check exactly those, and record the very bytes checked.
        bundle = _read_bundle(resolution, sorted(changed))
        _refuse_home(bundle)
        for name, data in bundle.items():
            (resolution / name).write_bytes(data)
        pipeline.pijul(resolution, "add", "--recursive", ".")
        pipeline.pijul(resolution, "record", "-am", message, "--author", "ck-docs-resolve")
        state = pointer.read_pointer(self.hub)
        hashes = sorted(pipeline.hashes(resolution) - set(state["fingerprint"]["hashes"]))
        group_id = f"DRES{time.time_ns()}"
        group: DocsEntry = {
            "id": group_id,
            "state": "pending",
            "created": time.time_ns(),
            "resolves": did,
            "repo": str(resolution),
            "hashes": hashes,
            "files": entry["files"],
            "fingerprint": pipeline.fingerprint(resolution),
            "message": message,
        }
        atomic_bytes(self.directory / f"{group_id}.json", canonical(group))
        entry["state"] = f"resolved-by {group_id}"
        atomic_bytes(entry_path, canonical(entry))
        return group_id

    def flush_locked(self) -> int:  # noqa: PLR0912,PLR0915  # linear workflow kept whole so failure ordering stays visible
        pipeline = pipeline_api.load()
        active = _active(self.hub)
        if active:
            print(f"deferred behind batch {active}")
            return 0
        count = self._recover_journal_locked()
        queue = _queue(self.hub)
        priority = [row for row in queue if row.get("state") == "pending" and row.get("resolves")]
        ordered = priority + [row for row in queue if row not in priority]
        for entry in ordered:
            if entry.get("state") not in ("pending", "blocked"):
                continue
            blocked_ids = _blocked_entries(_queue(self.hub))
            if entry["id"] in blocked_ids:
                entry["state"] = "blocked"
                atomic_bytes(self.directory / f"{entry['id']}.json", canonical(entry))
                continue
            replayed = entry.get("state") == "blocked"
            if replayed:
                entry["state"] = "pending"
            current = pointer.read_pointer(self.hub)
            base = Path(current["repo"])
            candidate = self.hub / ".pipeline/checkpoints" / f"docs-{entry['id']}"
            pipeline.clone(base, candidate, self.hub)
            pipeline.clean(Path(entry["repo"]))
            if replayed:
                source = Path(entry["repo"])
                for name in entry.get("files", []):
                    source_path, target_path = source / name, candidate / name
                    if source_path.exists():
                        target_path.parent.mkdir(parents=True, exist_ok=True)
                        target_path.write_bytes(source_path.read_bytes())
                    else:
                        target_path.unlink(missing_ok=True)
                pipeline.pijul(candidate, "add", "--recursive", ".")
                pipeline.pijul(
                    candidate,
                    "record",
                    "-am",
                    f"Replay queued documentation {entry['id']}",
                    "--author",
                    "ck-docs-replay",
                )
                publish_hashes = sorted(
                    pipeline.hashes(candidate) - set(current["fingerprint"]["hashes"])
                )
                conflicts = []
            else:
                publish_hashes = entry["hashes"]
                conflicts = pipeline.pull(candidate, Path(entry["repo"]), publish_hashes)
            if conflicts:
                entry["state"] = "conflict"
                entry["conflicts"] = conflicts
                atomic_bytes(self.directory / f"{entry['id']}.json", canonical(entry))
                continue
            for name, digest in current["fingerprint"]["files"].items():
                if (
                    not pipeline.doc_path(name)
                    and pipeline.fingerprint(candidate)["files"].get(name) != digest
                ):
                    raise RuntimeError("Docs candidate changed a non-documentation path")
            next_state = current.copy()
            next_state.pop("_pointer_path", None)
            next_state["repo"] = str(candidate)
            next_state["fingerprint"] = pipeline.fingerprint(candidate)
            next_state["previous"] = current.get("receipt")
            receipt_path = self.hub / ".pipeline/docs" / "receipts" / f"{entry['id']}.json"
            next_state["receipt"] = str(receipt_path)
            receipt: DocsPublishReceipt = {
                "kind": "docs",
                "id": entry["id"],
                "fingerprint": next_state["fingerprint"],
                "hashes": publish_hashes,
                "previous": current.get("receipt"),
            }
            atomic_bytes(receipt_path, canonical(receipt))
            journal_dir = self.directory / "journal"
            journal_dir.mkdir(parents=True, exist_ok=True)
            seq = len(list(journal_dir.glob("*.json"))) + 1
            journal: DocsJournal = {
                "seq": seq,
                "entries": [entry["id"]],
                "prev_sha256": file_digest(Path(current["_pointer_path"])),
                "prev": Path(current["_pointer_path"]).read_text(),
                "next": canonical(next_state).decode(),
            }
            atomic_bytes(journal_dir / f"{seq:08d}.json", canonical(journal))
            self._point("after_journal", entry=entry, journal=journal_dir / f"{seq:08d}.json")
            pointer.write_pointer(Path(current["_pointer_path"]), next_state)
            self._point("after_pointer", entry=entry)
            entry["state"] = "published"
            atomic_bytes(self.directory / f"{entry['id']}.json", canonical(entry))
            self._point("after_ack", entry=entry)
            count += 1
        return count

    def _recover_journal_locked(self) -> int:
        """Finish durable docs intents before considering the next FIFO entry."""
        pipeline = pipeline_api.load()
        journal_dir = self.directory / "journal"
        if not journal_dir.exists():
            return 0
        count = 0
        pointer_path = self.hub / ".pipeline/accepted-v2.json"
        if not pointer_path.exists():
            pointer_path = self.hub / ".pipeline/accepted.json"
        for path in sorted(journal_dir.glob("*.json")):
            journal = load_docs_journal(read_json(path), str(path))
            entry_paths = [
                self.directory / f"{identifier}.json" for identifier in journal["entries"]
            ]
            if entry_paths and all(
                entry_path.exists()
                and load_docs_entry(read_json(entry_path), str(entry_path)).get("state")
                == "published"
                for entry_path in entry_paths
            ):
                continue
            current = pointer_path.read_bytes()
            previous = journal["prev"].encode()
            following = journal["next"].encode()
            if current == previous:
                next_state = load_pointer(json.loads(following), f"{path}: next")
                candidate = Path(next_state["repo"])
                pipeline.clean(candidate)
                if pipeline.fingerprint(candidate) != next_state["fingerprint"]:
                    raise RuntimeError(f"Docs journal candidate changed: {path.name}")
                pointer.write_pointer(pointer_path, next_state)
                current = following
            elif current != following and not is_docs_descendant(self.hub, current, following):
                raise RuntimeError(f"Docs journal no longer follows accepted state: {path.name}")
            for identifier in journal["entries"]:
                entry_path = self.directory / f"{identifier}.json"
                if not entry_path.exists():
                    raise RuntimeError(f"Docs journal entry is missing: {identifier}")
                entry = load_docs_entry(read_json(entry_path), str(entry_path))
                if entry.get("state") == "pending":
                    entry["state"] = "published"
                    atomic_bytes(entry_path, canonical(entry))
                    count += 1
        return count


def live_entries(queue: Sequence[DocsEntry]) -> list[DocsEntry]:
    """Return pending entries that are not blocked, in queue (FIFO) order."""
    blocked = _blocked_entries(queue)
    return [row for row in queue if row.get("state") == "pending" and row["id"] not in blocked]


def outstanding_hashes(queue: Sequence[DocsEntry], accepted: set[str]) -> set[str]:
    """Hashes of unpublished queue entries that the accepted snapshot does not yet contain.

    Entry hash lists are cumulative from their capture base, so a nonterminal entry can still
    list a prerequisite that a later publication made part of the accepted base.
    """
    unpublished: set[str] = set()
    for row in queue:
        state = row.get("state", "")
        if state != "published" and not state.startswith("resolved-by"):
            unpublished.update(row.get("hashes", []))
    return unpublished - accepted


def sync(root: Path, hub: Path) -> None:
    pipeline = pipeline_api.load()
    live: list[DocsEntry] = []

    def snapshot() -> AcceptedPointer:
        # The pointer and the live queue are read together, again on every retry of the lease.
        with locks.state(hub):
            state = pointer.read_pointer(hub)
            live[:] = live_entries(_queue(hub))
        return state

    pipeline.clean(root)
    # The pointer's repository is leased for the pull, and the Pijul child inherits the lease; if
    # it was collected first, the pointer is read again (up to 3 attempts).
    with lease.source(snapshot) as state:
        missing = set(state["fingerprint"]["hashes"]) - pipeline.hashes(root)
        conflicts = pipeline.pull(root, Path(state["repo"]), sorted(missing))
    if conflicts:
        raise RuntimeError(f"sync has conflicts: {conflicts}")
    for entry in live:
        missing = set(entry.get("hashes", [])) - pipeline.hashes(root)
        conflicts = pipeline.pull(root, Path(entry["repo"]), sorted(missing))
        if conflicts:
            raise RuntimeError(f"sync has conflicts: {conflicts}")
    status = load_pijul_status(
        json.loads(pipeline.pijul(root, "status", "--output-format", "json")),
        f"{root}: pijul status",
    )
    if status["changes"] or status["untracked"]:
        raise RuntimeError("sync did not produce a clean working copy")


def is_docs_descendant(hub: Path, current: bytes, ancestor: bytes) -> bool:
    """Verify a pointer chain written exclusively by the docs journal."""
    pipeline = pipeline_api.load()
    target = current.decode()
    base = ancestor.decode()
    seen: set[str] = set()
    journal_dir = hub / ".pipeline/docs/journal"
    rows = (
        [
            load_docs_journal(read_json(path), str(path))
            for path in sorted(journal_dir.glob("*.json"))
        ]
        if journal_dir.exists()
        else []
    )
    while target != base:
        if target in seen:
            return False
        seen.add(target)
        record = next((row for row in rows if row.get("next") == target), None)
        if record is None:
            return False
        previous = record["prev"].encode()
        if hashlib.sha256(previous).hexdigest() != record["prev_sha256"]:
            return False
        before, after = (
            load_pointer(json.loads(previous), "journal prev"),
            load_pointer(json.loads(target), "journal next"),
        )
        left, right = Path(before["repo"]), Path(after["repo"])
        pipeline.clean(left)
        pipeline.clean(right)
        old_files, new_files = pipeline.files(left), pipeline.files(right)
        changed = {
            name
            for name in set(old_files) | set(new_files)
            if old_files.get(name) != new_files.get(name)
        }
        if any(not pipeline.doc_path(name) for name in changed):
            return False
        target = previous.decode()
    return True
