"""Private diagnostics for live identity ownership conflicts."""

import contextlib
import json
import os
import tempfile
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import TypedDict, cast

import chatlib

KEYS = ("project", "role", "surface_id", "incumbent")


class ConflictRow(TypedDict, total=False):
    """One recorded conflict; a writer stores all four keys, null where it knows nothing."""

    project: str | None
    role: str | None
    surface_id: str | None
    incumbent: str | None


class IdentityConflict(chatlib.ChatError):
    """A registration was rejected because another live process owns its name."""

    def __init__(self, incumbent: Mapping[str, object], candidate: Mapping[str, object]) -> None:
        self.incumbent = {
            "name": incumbent.get("name"),
            "pid": incumbent.get("pid"),
            "pid_start": incumbent.get("pid_start"),
        }
        self.candidate = {
            "project": candidate.get("project"),
            "role": candidate.get("role"),
            "surface_id": candidate.get("surface_id"),
        }
        super().__init__(f"{self.incumbent['name']} already has a live session")


def diagnostic_path() -> Path:
    return Path(chatlib.STATE_DIR) / "identity-conflicts.json"


def load_conflicts(value: object) -> list[ConflictRow]:
    """The diagnostic file's rows: a list of objects whose known keys are text or null; any other
    key a newer writer added is kept."""
    if not isinstance(value, list):
        raise chatlib.ChatError(
            f"invalid identity conflict diagnostic: not a list; inspect {diagnostic_path()} "
            "(the next identity sync rewrites it)"
        )
    for number, row in enumerate(value, 1):
        if not isinstance(row, dict) or not all(isinstance(key, str) for key in row):
            raise chatlib.ChatError(
                f"invalid identity conflict diagnostic: row {number} is not an object; "
                f"inspect {diagnostic_path()} (the next identity sync rewrites it)"
            )
        for key in KEYS:
            if key in row and not (row[key] is None or isinstance(row[key], str)):
                raise chatlib.ChatError(
                    f"invalid identity conflict diagnostic: row {number} field {key!r} is not "
                    f"text or null; inspect {diagnostic_path()} (the next identity sync rewrites it)"
                )
    return cast(list[ConflictRow], value)  # every known key of every row was checked above


def read() -> list[ConflictRow]:
    try:
        value: object = json.loads(diagnostic_path().read_text())
    except FileNotFoundError:
        return []
    return load_conflicts(value)


def _sort_key(item: Mapping[str, object]) -> tuple[str, str]:
    surface, incumbent = item.get("surface_id"), item.get("incumbent")
    return (
        surface if isinstance(surface, str) else "",
        incumbent if isinstance(incumbent, str) else "",
    )


def replace_all(conflicts: Sequence[Mapping[str, object]]) -> None:
    """Persist only current conflicts, without rewriting an unchanged file."""
    path = diagnostic_path()
    rows = sorted(
        ({k: conflict.get(k) for k in KEYS} for conflict in conflicts),
        key=_sort_key,
    )
    if not rows:
        with contextlib.suppress(FileNotFoundError):
            path.unlink()
        return
    data = (json.dumps(rows, sort_keys=True, indent=2) + "\n").encode()
    try:
        if path.read_bytes() == data:
            return
    except FileNotFoundError:
        pass
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=path.name + ".", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        Path(temporary).chmod(0o600)
        Path(temporary).replace(path)
    finally:
        if Path(temporary).exists():
            Path(temporary).unlink()


def for_surface(surface_id: str | None) -> ConflictRow | None:
    if not surface_id:
        return None
    surface = surface_id.lower()
    return next(
        (item for item in read() if (item.get("surface_id") or "").lower() == surface), None
    )


def raise_for_surface(surface_id: str | None) -> None:
    conflict = for_surface(surface_id)
    if conflict:
        raise IdentityConflict(
            {"name": conflict.get("incumbent")},
            {
                "project": conflict.get("project"),
                "role": conflict.get("role"),
                "surface_id": conflict.get("surface_id"),
            },
        )
