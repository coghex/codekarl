"""`ck gc`: a dry run and `--apply` (plan section 10).

GC removes only state whose writers and readers are enrolled in the lease protocol, plus trash:
superseded lease-capable checkpoints, the build output (`target/`) of such checkpoints, and trash
left by an interrupted run. The list is closed. Captures, records, pre-protocol checkpoints and
their build output, the legacy `.pipeline/target` cache, worker copies and everything outside
`.pipeline/checkpoints` and `.pipeline/trash` are never removed; the dry run reports them with the
rule that keeps them.

Every candidate has two objects: the *lease object* (the checkpoint directory, locked
`LOCK_EX | LOCK_NB`) and the *deletion object* (the directory renamed into trash while that lock
is held). Paths come from directory listings only and are validated again before use.
"""

from __future__ import annotations

import contextlib
import errno
import fcntl
import json
import os
import re
import shutil
import stat
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

from . import lease, locks, meta, owned, pointer

NS = 10**9
WINDOW_NS = 7 * 24 * 3600 * NS  # checkpoints created in the last 7 days are kept (rule 8)
NEWEST = 3  # and so are the 3 newest of each kind
TERMINAL = ("published", "failed", "rejected", "cancelled")
BATCH_KINDS = ("combined", "split")
SCHEMA = 2

# The closed list of checkpoint names GC recognizes; anything else is never touched.
PATTERNS = (
    ("docs", re.compile(r"docs-D(?P<ns>\d+)")),
    ("combined", re.compile(r"(?P<bid>\d+)-combined")),
    ("split", re.compile(r"(?P<bid>\d+)-split-(?:\d+|final)")),
    ("bootstrap", re.compile(r"bootstrap-(?P<bid>\d+)")),
    ("restore", re.compile(r"restore-(?P<bid>\d+)-.+-(?P<ns>\d+)")),
    ("legacy", re.compile(r"(?P<ns>\d+)-[0-9a-f]{8}")),
)
TRASH_NAME = re.compile(r"\d+-(?:checkpoint|target)-.+")

# The dry run's promise is about gc itself. `tools/ck` verifies the current checkpoint with Pijul
# before any controller runs, and that verification (not gc, which runs no Pijul command) can
# rewrite the checkpoint's `.pijul/pristine/db`; the first line says so rather than over-promise.
DRY_RUN_TITLE = (
    "gc dry run: gc itself deletes and writes nothing and runs no Pijul command "
    "(the tools/ck entry's own verification of the current checkpoint, which runs before gc, "
    "may rewrite that checkpoint's .pijul/pristine/db)"
)

# A seam for tests: called with the stage name while the planning locks are held.
HOOK: Callable[[str], None] | None = None


def _point(stage: str) -> None:
    if HOOK is not None:
        HOOK(stage)


def _now() -> int:
    return time.time_ns()


def _same_device(first: Path, second: Path) -> bool:
    return first.stat().st_dev == second.stat().st_dev


@dataclass
class Item:
    """One thing the plan reports: a candidate, or something kept and the rule that keeps it."""

    kind: str  # checkpoint, target, trash, legacy-target, capture, record-files, worker
    path: Path
    size: int
    status: str  # candidate, in-use, protected, report-only, refused, info
    reason: str
    name: str = ""
    lease_path: Path | None = None  # the lease object, for candidates


@dataclass
class Plan:
    items: list[Item] = field(default_factory=list)
    recovery: list[str] = field(
        default_factory=list
    )  # each: the condition and its recovery command
    pointer_problem: str | None = None
    current: Path | None = None
    current_id: tuple[int, int] | None = None

    def candidates(self) -> list[Item]:
        return [item for item in self.items if item.status == "candidate"]


def classify(name: str) -> tuple[str, int, str | None] | None:
    """(kind, creation time in ns, batch id) of a checkpoint directory name, or None."""
    for kind, pattern in PATTERNS:
        match = pattern.fullmatch(name)
        if match:
            fields = match.groupdict()
            bid = fields.get("bid")
            created = fields.get("ns") or bid
            if created is None:  # unreachable: every pattern names a time or a batch id
                return None
            return kind, int(created), bid if kind in BATCH_KINDS else None
    return None


def tree_size(path: Path) -> int:
    """Bytes in a tree by `lstat` (symlinks are counted, never followed)."""
    total = 0
    pending = [path]
    while pending:
        current = pending.pop()
        try:
            with os.scandir(current) as entries:
                for entry in entries:
                    info = entry.stat(follow_symlinks=False)
                    total += info.st_size
                    if stat.S_ISDIR(info.st_mode):
                        pending.append(Path(entry.path))
        except OSError:
            continue
    return total


def _identity(path: Path, *, follow: bool = False) -> tuple[int, int]:
    """(device, inode) of a path; `follow` resolves a symlink, which a listing entry never does."""
    info = path.stat() if follow else path.lstat()
    return info.st_dev, info.st_ino


def _is_real_directory(path: Path) -> bool:
    try:
        return stat.S_ISDIR(os.lstat(path).st_mode)
    except OSError:
        return False


def _parse(path: Path, plan: Plan) -> dict[str, object] | None:
    """A protecting record, or None after naming it as a recovery condition when it cannot be used:
    it cannot be read or parsed, or it is valid JSON that is not an object (a protecting record is
    always one, so a list or null is as unusable as a syntax error)."""
    try:
        value = json.loads(path.read_bytes())
    except (OSError, ValueError) as error:
        plan.recovery.append(
            f"a record that defines the protected set does not parse: {path} ({error}); "
            "repair or restore it, then rerun"
        )
        return None
    if not isinstance(value, dict):
        plan.recovery.append(
            f"a record that defines the protected set has an unexpected shape: {path} "
            f"(a JSON {type(value).__name__}, expected an object); repair or restore it, then rerun"
        )
        return None
    return value


def _batches(hub: Path, plan: Plan) -> tuple[set[str], set[str]]:
    """(non-terminal batch ids, all batch ids), noting recovery conditions as they are found."""
    open_batches: set[str] = set()
    known: set[str] = set()
    root = hub / ".pipeline/batches"
    for directory in sorted(root.iterdir()) if root.is_dir() else []:
        if not _is_real_directory(directory):
            continue
        known.add(directory.name)
        present = {
            name for name in (*TERMINAL, "publishing") if (directory / f"{name}.json").exists()
        }
        for name in sorted(present):
            _parse(directory / f"{name}.json", plan)
        if not present & set(TERMINAL):
            open_batches.add(directory.name)
            if (directory / "frozen.json").exists():
                _parse(directory / "frozen.json", plan)
        if "publishing" in present and "published" not in present:
            plan.recovery.append(
                f"batch {directory.name} has publishing.json but no published.json; "
                f"finish it with `tools/ck batch publish {directory.name}`"
            )
    return open_batches, known


def _bootstrap(hub: Path, plan: Plan) -> None:
    root = hub / ".pipeline/bootstrap"
    for directory in sorted(root.iterdir()) if root.is_dir() else []:
        if not _is_real_directory(directory):
            continue
        done = [name for name in ("published", "restored") if (directory / f"{name}.json").exists()]
        for name in done:
            _parse(directory / f"{name}.json", plan)
        if not done:
            plan.recovery.append(
                f"bootstrap {directory.name} has neither published.json nor restored.json; "
                "rerun the bootstrap publish recovery (plan section 11.7)"
            )


def _journal(hub: Path, plan: Plan) -> None:
    """A journal record is settled when every entry it names is recorded as published."""
    docs = hub / ".pipeline/docs"
    journal_dir = docs / "journal"
    for path in sorted(journal_dir.glob("*.json")) if journal_dir.is_dir() else []:
        journal = _parse(path, plan)
        if journal is None:
            continue
        entries = journal.get("entries", [])
        if not isinstance(entries, list) or not all(isinstance(item, str) for item in entries):
            plan.recovery.append(
                f"a record that defines the protected set has an unexpected shape: {path} "
                "(`entries` must be a list of entry ids); repair or restore it, then rerun"
            )
            continue
        settled = bool(entries)
        broken = False
        for identifier in entries:
            entry_path = docs / f"{identifier}.json"
            entry = _parse(entry_path, plan) if entry_path.exists() else None
            broken = broken or (entry_path.exists() and entry is None)
            settled = settled and entry is not None and entry.get("state") == "published"
        if not settled and not broken:
            plan.recovery.append(
                f"docs journal record {path.name} has entries that are not all published; "
                "finish it with `tools/ck docs flush`"
            )


def _lease_state(path: Path, own: list[tuple[int, Path]]) -> str | None:
    """None if the exclusive lease is free right now, else why not ('in use' or 'in use by this gc').

    The probe takes and drops `LOCK_EX | LOCK_NB` on a descriptor of its own; it writes nothing."""
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except OSError as error:
        return f"cannot open the lease object ({error.strerror})"
    try:
        identity = os.fstat(descriptor)
        for _held, held_path in own:
            with contextlib.suppress(OSError):
                held = Path(held_path).stat()
                if (held.st_dev, held.st_ino) == (identity.st_dev, identity.st_ino):
                    return "in use by this gc"
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return "in use"
        fcntl.flock(descriptor, fcntl.LOCK_UN)
        return None
    finally:
        os.close(descriptor)


def build_plan(hub: Path) -> Plan:  # noqa: PLR0912,PLR0915  # one linear pass over the rules in plan 10.3, kept whole
    """The whole plan, computed from scratch from the gate's own records (call under the locks)."""
    plan = Plan()
    pipeline_dir = hub / ".pipeline"
    checkpoints = pipeline_dir / "checkpoints"
    own = lease.held()
    now = _now()

    current_id = None
    try:
        state = pointer.read_pointer(hub)
        if state.get("schema") != SCHEMA:
            raise RuntimeError(
                f"gc requires a schema-{SCHEMA} pointer, found {state.get('schema')!r}"
            )
        plan.current = Path(state["repo"])
        # The current repository is recognized by identity, never by the spelling of its path (a
        # hub reached through a symlink, such as /var -> /private/var, spells it differently).
        plan.current_id = _identity(plan.current, follow=True)
        current_id = plan.current_id
    except (RuntimeError, KeyError, ValueError, OSError, AttributeError, TypeError) as error:
        plan.pointer_problem = str(error)
        plan.recovery.append(
            f"the pointer cannot be used ({error}); gc plans nothing for removal until it is repaired"
        )

    open_batches, known_batches = _batches(hub, plan)
    _bootstrap(hub, plan)
    _journal(hub, plan)

    entries = sorted(checkpoints.iterdir()) if checkpoints.is_dir() else []
    directories = [path for path in entries if path.name != "target"]
    record_files = [
        path for path in directories if not _is_real_directory(path) and not path.is_symlink()
    ]
    by_kind: dict[str, list[int]] = {}
    for path in directories:
        parsed = classify(path.name)
        if parsed and _is_real_directory(path):
            by_kind.setdefault(parsed[0], []).append(parsed[1])
    newest = {kind: set(sorted(values, reverse=True)[:NEWEST]) for kind, values in by_kind.items()}

    for path in directories:
        if not _is_real_directory(path):
            if path.is_symlink() and classify(path.name):
                plan.items.append(
                    Item(
                        "checkpoint",
                        path,
                        0,
                        "refused",
                        "a symlink, not a checkpoint directory",
                        path.name,
                    )
                )
            continue
        parsed = classify(path.name)
        if parsed is None:
            plan.items.append(
                Item(
                    "checkpoint",
                    path,
                    tree_size(path),
                    "protected",
                    "name outside the checkpoint patterns: never touched (path safety)",
                    path.name,
                )
            )
            continue
        kind, created, bid = parsed
        capable = (path / "tools/gate/lease.py").is_file() and not (
            path / "tools/gate/lease.py"
        ).is_symlink()
        in_batch = bid in open_batches if bid else False
        batch_unknown = kind in BATCH_KINDS and bid not in known_batches

        source_reason = None
        if plan.pointer_problem:
            source_reason = (
                "protected",
                "the pointer is unusable, so nothing can be proved superseded",
            )
        elif _identity(path) == current_id:
            source_reason = ("protected", "rule 1: the current pointer's repository")
        elif in_batch:
            source_reason = ("protected", f"rule 2: checkpoint of non-terminal batch {bid}")
        elif kind == "bootstrap":
            source_reason = (
                "protected",
                "rule 6: the bootstrap checkpoint predates the lease protocol",
            )
        elif not capable:
            source_reason = (
                "report-only",
                "rule 7: published before the lease protocol; source is kept",
            )
        elif batch_unknown:
            source_reason = ("protected", f"no record of batch {bid}: kept")
        elif now - created < WINDOW_NS or created in newest.get(kind, set()):
            source_reason = (
                "protected",
                "rule 8: inside the retention window or among the 3 newest of its kind",
            )
        size = tree_size(path)
        if source_reason:
            plan.items.append(Item("checkpoint", path, size, *source_reason, name=path.name))
        else:
            why = _lease_state(path, own)
            if why:
                plan.items.append(Item("checkpoint", path, size, "in-use", why, path.name))
            else:
                plan.items.append(
                    Item(
                        "checkpoint",
                        path,
                        size,
                        "candidate",
                        f"superseded {kind} checkpoint, terminal, unleased, outside the window",
                        path.name,
                        lease_path=path,
                    )
                )
                continue  # its target/ goes with it

        target = path / "target"
        if not target.exists() and not target.is_symlink():
            continue
        if not _is_real_directory(target):
            plan.items.append(
                Item(
                    "target",
                    target,
                    0,
                    "refused",
                    "target/ is not a real directory (symlink)",
                    path.name,
                )
            )
            continue
        size = tree_size(target)
        if plan.pointer_problem:
            plan.items.append(
                Item("target", target, size, "protected", "the pointer is unusable", path.name)
            )
        elif in_batch:
            plan.items.append(
                Item(
                    "target",
                    target,
                    size,
                    "protected",
                    f"rule 2: non-terminal batch {bid}",
                    path.name,
                )
            )
        elif kind == "bootstrap" or not capable:
            plan.items.append(
                Item(
                    "target",
                    target,
                    size,
                    "report-only",
                    "rule 7: built before the lease protocol; no lease can show its writers are gone",
                    path.name,
                )
            )
        elif batch_unknown:
            plan.items.append(
                Item(
                    "target",
                    target,
                    size,
                    "protected",
                    f"no record of batch {bid}: kept",
                    path.name,
                )
            )
        else:
            why = _lease_state(
                path, own
            )  # the lease object is the checkpoint, never target/ itself
            if why:
                reason = why + (
                    " (rule 1: the current checkpoint)" if _identity(path) == current_id else ""
                )
                plan.items.append(Item("target", target, size, "in-use", reason, path.name))
            else:
                plan.items.append(
                    Item(
                        "target",
                        target,
                        size,
                        "candidate",
                        "build output of a lease-capable checkpoint; unleased",
                        path.name,
                        lease_path=path,
                    )
                )

    if record_files:
        plan.items.append(
            Item(
                "record-files",
                checkpoints,
                sum(os.lstat(path).st_size for path in record_files),
                "protected",
                f"records (logs, receipts' JSON, chat inputs): {len(record_files)} files, never removed",
            )
        )

    legacy = pipeline_dir / "target"
    if _is_real_directory(legacy):
        plan.items.append(
            Item(
                "legacy-target",
                legacy,
                tree_size(legacy),
                "report-only",
                "rule 7: the legacy build cache predates the lease protocol",
            )
        )
    for root, label in (
        (pipeline_dir / "submissions", "code capture"),
        (pipeline_dir / "docs/captures", "docs capture"),
    ):
        for capture in sorted(root.iterdir()) if root.is_dir() else []:
            if _is_real_directory(capture):
                plan.items.append(
                    Item(
                        "capture",
                        capture,
                        tree_size(capture),
                        "protected",
                        f"rule 3: {label}, never removed",
                    )
                )
    trash = pipeline_dir / "trash"
    for entry in sorted(trash.iterdir()) if trash.is_dir() else []:
        plan.items.append(
            Item(
                "trash",
                entry,
                tree_size(entry) if _is_real_directory(entry) else 0,
                "candidate",
                "left by an interrupted --apply; no reader can reach it",
                entry.name,
                lease_path=None,
            )
        )
    return plan


def human(size: float) -> str:
    for unit in ("B", "KiB", "MiB"):
        if size < 1024:  # noqa: PLR2004  # one binary kilo
            return f"{size:.0f} {unit}" if unit == "B" else f"{size:.1f} {unit}"
        size /= 1024
    return f"{size:.1f} GiB"


def render(plan: Plan, title: str) -> list[str]:
    lines = [title]
    if plan.pointer_problem:
        lines.append(f"pointer state: {plan.pointer_problem} (this is why nothing is a candidate)")
    order = {"candidate": 0, "in-use": 1, "refused": 2, "report-only": 3, "protected": 4}
    for item in sorted(plan.items, key=lambda i: (order[i.status], str(i.path))):
        lines.append(
            f"{item.status.upper():11} {item.kind:13} {human(item.size):>10}  {item.path}  -- {item.reason}"
        )
    candidates = plan.candidates()
    lines.append(
        f"summary: {len(candidates)} candidate(s), {human(sum(i.size for i in candidates))} reclaimable; "
        f"{sum(1 for i in plan.items if i.status == 'in-use')} in use; "
        f"{sum(1 for i in plan.items if i.status in ('report-only', 'protected'))} kept by rule"
    )
    for condition in plan.recovery:
        lines.append(f"RECOVERY CONDITION: {condition}")
    return lines


def _unsafe(path: Path, hub: Path) -> str | None:
    """Why a path cannot be used, or None: a real directory (no symlink) inside the hub's .pipeline."""
    root = Path(os.path.realpath(hub / ".pipeline"))
    try:
        resolved = Path(os.path.realpath(path))
    except OSError as error:
        return f"cannot resolve the path ({error.strerror})"
    if root not in resolved.parents:
        return "the path resolves outside the hub's .pipeline"
    if not _is_real_directory(path):
        return "not a real directory (a symlink or a file)"
    return None


def _precheck(item: Item, hub: Path, trash: Path, current_id: tuple[int, int] | None) -> str | None:
    """Why a candidate must not be moved (names, real directories, location, device), or None."""
    checkpoints = hub / ".pipeline/checkpoints"
    lease_path = item.lease_path
    if lease_path is None or lease_path.parent != checkpoints or classify(lease_path.name) is None:
        return "the name does not match a checkpoint pattern"
    problem = _unsafe(lease_path, hub) or (
        _unsafe(item.path, hub) if item.kind == "target" else None
    )
    if problem:
        return problem
    if item.kind == "target" and item.path != lease_path / "target":
        return "target/ is not inside its checkpoint"
    if item.kind == "checkpoint" and (current_id is None or _identity(lease_path) == current_id):
        return "this is the current pointer's repository (or the pointer is unknown)"
    if not _same_device(trash, checkpoints):
        return "trash is on another device, so the rename would not be atomic"
    return None


def _to_trash(item: Item, hub: Path, trash: Path, current_id: tuple[int, int] | None) -> str:  # noqa: PLR0911  # one outcome per refusal, in rule order
    """Rename one candidate into trash while holding the exclusive lease; return its outcome."""
    problem = _precheck(item, hub, trash, current_id)
    if problem:
        return f"refused: {problem}"
    lease_path = item.lease_path
    if lease_path is None:  # unreachable: _precheck refuses an item without a lease object
        return "refused: the item has no lease object"
    checkpoints = hub / ".pipeline/checkpoints"
    try:
        descriptor = os.open(lease_path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except OSError as error:
        return f"refused: cannot open the lease object ({error.strerror})"
    try:
        named, opened = lease_path.lstat(), os.fstat(descriptor)
        if (named.st_dev, named.st_ino) != (opened.st_dev, opened.st_ino):
            return "refused: the checkpoint was replaced while opening it"
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return "in use"
        label = "checkpoint" if item.kind == "checkpoint" else "target"
        destination = f"{_now()}-{label}-{lease_path.name}"
        trash_fd = os.open(trash, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        parent_fd = os.open(checkpoints, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            if item.kind == "target":
                lease.rename_exclusive_at(descriptor, "target", trash_fd, destination)
            else:
                lease.rename_exclusive_at(parent_fd, lease_path.name, trash_fd, destination)
        finally:
            os.close(trash_fd)
            os.close(parent_fd)
        return f"moved to trash/{destination}"
    except FileNotFoundError:
        return "gone already"
    except OSError as error:
        return f"refused: {error.strerror}"
    finally:
        os.close(descriptor)  # closing the descriptor releases the lease


def delete_trash(trash: Path) -> list[str]:
    """Remove every trash entry, fd-relative; an entry still being written is retried, not failed."""
    results: list[str] = []
    if not trash.is_dir():
        return results
    if not shutil.rmtree.avoids_symlink_attacks:
        return ["refused: this platform's rmtree cannot avoid symlink attacks; trash is kept"]
    try:
        directory = os.open(trash, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except OSError as error:
        return [f"refused: cannot open trash ({error.strerror})"]
    try:
        for name in sorted(os.listdir(directory)):  # noqa: PTH208  # listing an open directory descriptor
            try:
                info = os.stat(name, dir_fd=directory, follow_symlinks=False)
            except FileNotFoundError:
                continue  # another cleanup finished it
            if not stat.S_ISDIR(info.st_mode) or not TRASH_NAME.fullmatch(name):
                results.append(f"refused: trash/{name} is not a trash directory; kept")
                continue
            try:
                owned.remove_tree(directory, name, missing_ok=True)
                results.append(f"removed trash/{name}")
            except OSError as error:
                if error.errno in (errno.ENOTEMPTY, errno.EBUSY, errno.EEXIST):
                    results.append(f"retry later: trash/{name} is still being written")
                else:
                    results.append(f"retry later: trash/{name}: {error.strerror}")
    finally:
        os.close(directory)
    return results


def command(hub: Path, *, apply: bool) -> int:
    """`ck gc` (a dry run: gc deletes and writes nothing) or `ck gc --apply`."""
    hub = hub.resolve()
    store = meta.Store(hub)
    if not apply:
        return _dry_run(hub, store)
    return _apply(hub, store)


def _dry_run(hub: Path, store: meta.Store) -> int:
    # The locks are taken exactly as for --apply but write no holder metadata, and a missing lock
    # file is a refusal, never a creation.
    with locks.state(hub, quiet=True), locks.meta(store.repo, quiet=True):
        _point("locked")
        plan = build_plan(hub)
        _point("planned")
    for line in render(plan, DRY_RUN_TITLE):
        print(line)
    return 0


def _apply(hub: Path, store: meta.Store) -> int:
    trash = hub / ".pipeline/trash"
    outcomes: list[tuple[Item, str]] = []
    if not shutil.rmtree.avoids_symlink_attacks:
        raise RuntimeError(
            "gc --apply refused: this platform's rmtree cannot avoid symlink attacks; "
            "nothing was renamed or removed"
        )
    with locks.state(hub), locks.meta(store.repo):
        _point("locked")
        plan = build_plan(hub)
        for line in render(plan, "gc --apply: plan recomputed under state.lock and meta.lock"):
            print(line)
        # Recovery conditions come first: a refused --apply removes nothing, trash included.
        if plan.recovery:
            raise RuntimeError("gc --apply refused; resolve first: " + "; ".join(plan.recovery))
        _point("planned")
        # Trash left by an interrupted run is unreachable by any reader and goes first.
        for line in delete_trash(trash):
            print(line)
        movable = [item for item in plan.candidates() if item.kind in ("checkpoint", "target")]
        if movable:
            trash.mkdir(mode=0o700, exist_ok=True)
            if not _is_real_directory(trash):
                raise RuntimeError(f"trash is not a real directory: {trash}")
        for item in movable:
            outcomes.append((item, _to_trash(item, hub, trash, plan.current_id)))
    for item, outcome in outcomes:
        print(f"{item.kind} {item.path}: {outcome}")
    _point("renamed")
    for line in delete_trash(trash):
        print(line)
    return 0
