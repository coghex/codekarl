"""Atomic pointer access for legacy, v2, and bootstrap tombstone states."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
from collections.abc import Callable
from pathlib import Path
from typing import TypeGuard

from . import postpublish
from .records import (
    AcceptedPointer,
    Fingerprint,
    PointerHold,
    PointerRecord,
    PointerTombstone,
    as_object,
    is_accepted_pointer,
    load_pointer,
)


def file_digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def pointer_paths(hub: Path) -> tuple[Path, Path]:
    directory = hub / ".pipeline"
    return directory / "accepted-v2.json", directory / "accepted.json"


def read_pointer(hub: Path) -> AcceptedPointer:
    """Read the authoritative pointer and refuse both tombstone states.

    The result is validated (`records.load_pointer`) and carries `_pointer_path`, the file read."""
    v2, legacy = pointer_paths(hub)
    path = v2 if v2.exists() else legacy
    try:
        raw = json.loads(path.read_bytes())
    except FileNotFoundError as error:
        raise RuntimeError("No accepted checkpoint pointer; run ck init.") from error
    value = as_object(raw, str(path))
    schema = value.get("schema")
    if schema == "bootstrap-hold":
        raise RuntimeError(
            f"bootstrap {value.get('bootstrap', '<unknown>')} in progress; state commands are paused"
        )
    if schema == "tombstone":
        raise RuntimeError("activation incomplete; rerun the bootstrap publish recovery (§11.7)")
    if schema not in (None, 1, 2):
        raise RuntimeError(f"Unsupported accepted pointer schema: {schema!r}")
    record = load_pointer(value, str(path))
    if record.get("schema") in ("bootstrap-hold", "tombstone"):  # unreachable: refused above
        raise RuntimeError(f"{path}: not an accepted pointer")
    accepted = _accepted(record)
    accepted["_pointer_path"] = str(path)
    return accepted


def _accepted(record: PointerRecord) -> AcceptedPointer:
    """The pointer forms that name an accepted checkpoint (the hold and tombstone are refused)."""
    if is_accepted(record):
        return record
    raise RuntimeError("Pointer is a bootstrap hold or tombstone; state commands are paused")


def is_accepted(record: PointerRecord) -> TypeGuard[AcceptedPointer]:
    return is_accepted_pointer(record)


def tombstone(
    hub: Path, legacy: AcceptedPointer, *, bootstrap: str | None = None
) -> PointerHold | PointerTombstone:
    """The tombstone written over `accepted.json`, or the hold while a bootstrap runs."""
    repo = str(hub / ".pipeline/UPGRADE-REQUIRED-see-docs-review-gate-plan")
    fingerprint: Fingerprint = {"hashes": [], "files": {}}
    result: PointerHold | PointerTombstone
    if bootstrap:
        result = {
            "schema": "bootstrap-hold",
            "bootstrap": bootstrap,
            "repo": repo,
            "fingerprint": fingerprint,
        }
    else:
        result = {
            "schema": "tombstone",
            "upgrade": "accepted-v2.json",
            "repo": repo,
            "fingerprint": fingerprint,
        }
    if "submissions" in legacy:
        result["submissions"] = legacy["submissions"]
    if "receipt" in legacy:
        result["receipt"] = legacy["receipt"]
    if "previous" in legacy:
        result["previous"] = legacy["previous"]
    return result


def atomic_bytes(path: Path, payload: bytes, *, replaced: Callable[[], None] | None = None) -> None:
    """Write atomically; `replaced` runs the moment the new bytes are in place, before the directory sync."""
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, name = tempfile.mkstemp(prefix=path.name + ".", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        Path(name).replace(path)
        if replaced is not None:
            replaced()
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if Path(name).exists():
            Path(name).unlink()


def write_accepted(path: Path, payload: bytes) -> None:
    """Write pointer bytes. When they change `accepted-v2.json`, the post-publish hook is owed from the
    moment they are in place, even if the directory sync after that fails; identical bytes owe nothing."""
    changed = path.name == "accepted-v2.json" and (
        not path.exists() or path.read_bytes() != payload
    )
    atomic_bytes(path, payload, replaced=postpublish.note_advance if changed else None)


def write_pointer(path: Path, value: PointerRecord) -> bytes:
    payload = canonical(value)
    write_accepted(path, payload)
    return payload


def verified_pointer(
    hub: Path, clean: Callable[[Path], object], fingerprint: Callable[[Path], object]
) -> AcceptedPointer:
    """Check the accepted repo through the caller's Pijul verifier."""
    value = read_pointer(hub)
    repo = Path(value["repo"])
    clean(repo)
    actual = fingerprint(repo)
    if actual != value["fingerprint"]:
        raise RuntimeError("Accepted checkpoint fingerprint mismatch.")
    return value
