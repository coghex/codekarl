"""The shared chat inputs: one definition for stage, the CI step, publication and install."""

import hashlib
import json
import os
import re
import stat
from collections.abc import Mapping
from pathlib import Path, PurePosixPath
from typing import Literal, TypedDict, TypeGuard

TREES = ("chat", "project-manager")
HEX = re.compile(r"[0-9a-f]{64}")


class InputRecord(TypedDict):
    """A chat-inputs record (a CI receipt's `chat_inputs`, or `<log>.chat-inputs.json`) once
    `valid_record` accepts it. The gate keeps the same shape as `gate.records.ChatInputs`; this
    module is loaded on its own, by path, so it cannot import it."""

    schema: Literal[1]
    files: dict[str, str]
    aggregate: str


class InputError(ValueError):
    """The shared chat inputs cannot be bound: a link or an unsupported entry is present."""


def shared_root() -> Path:
    """The installed shared skills; CK_CHAT_SHARED_ROOT replaces them in tests only."""
    override = os.environ.get("CK_CHAT_SHARED_ROOT")
    return Path(override) if override else Path.home() / ".codex/skills"


def _walk(root: Path, directory: Path, files: dict[str, str]) -> None:
    # os.lstat and scandir(follow_symlinks=False) throughout: a link is refused, never followed.
    with os.scandir(directory) as entries:
        for entry in sorted(entries, key=lambda item: item.name):
            if entry.name == "__pycache__":
                continue
            path = Path(entry.path)
            mode = os.lstat(path).st_mode
            if stat.S_ISLNK(mode):
                raise InputError(f"symlink in shared chat inputs: {path}; refused")
            if stat.S_ISDIR(mode):
                _walk(root, path, files)
            elif stat.S_ISREG(mode):
                files[path.relative_to(root).as_posix()] = hashlib.sha256(
                    path.read_bytes()
                ).hexdigest()
            else:
                raise InputError(f"unsupported entry in shared chat inputs: {path}; refused")


def shared_inputs(root: str | Path | None = None) -> dict[str, str]:
    """{relative path: sha256} for every regular file under root/chat and root/project-manager."""
    root = Path(shared_root() if root is None else root)
    files: dict[str, str] = {}
    for tree in TREES:
        path = root / tree
        try:
            mode = os.lstat(path).st_mode
        except FileNotFoundError as error:
            raise InputError(f"missing shared chat input tree: {path}") from error
        if stat.S_ISLNK(mode):
            raise InputError(f"symlink in shared chat inputs: {path}; refused")
        if not stat.S_ISDIR(mode):
            raise InputError(f"unsupported entry in shared chat inputs: {path}; refused")
        _walk(root, path, files)
    return files


def aggregate(files: Mapping[str, str]) -> str:
    """SHA-256 of the canonical JSON of the map: order-independent, any byte change shows."""
    text = json.dumps(files, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(text.encode()).hexdigest()


def relative_input_path(name: object) -> bool:
    """A canonical relative POSIX path, the form shared_inputs emits: never absolute, never
    escaping, never with a '.', '..' or empty component."""
    if not isinstance(name, str) or not name:
        return False
    path = PurePosixPath(name)
    return (
        not path.is_absolute()
        and path.as_posix() == name
        and all(part not in ("", ".", "..") for part in name.split("/"))
    )


def valid_record(record: object) -> TypeGuard[InputRecord]:
    """A chat-inputs record is schema 1, a non-empty map of relative input paths to digests,
    and its aggregate. The schema must be the integer 1 itself: not True, not 1.0."""
    if not isinstance(record, dict):
        return False
    schema = record.get("schema")
    if type(schema) is not int or schema != 1:
        return False
    files = record.get("files")
    if not isinstance(files, dict) or not files:
        return False
    if not all(relative_input_path(name) for name in files):
        return False
    if not all(isinstance(value, str) and HEX.fullmatch(value) for value in files.values()):
        return False
    return record.get("aggregate") == aggregate(files)
