#!/usr/bin/env python3
"""Stage/test the shared IRC adapter; install only the accepted Codekarl source."""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import NotRequired, TypedDict, cast

from adapter import TRANSFORMS
from inputs import TREES, aggregate, shared_inputs, shared_root, valid_record

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from gate.pointer import read_pointer  # noqa: E402  # sys.path is set up above
from gate.records import (  # noqa: E402  # sys.path is set up above
    AcceptedPointer,
    ChatInputs,
    RecordError,
    as_object,
)

CHANGED = "shared chat inputs changed while copying"
EXTRA = {
    "codekarl_chat.py": "tools/chat/codekarl_chat.py",
    "identity_conflicts.py": "tools/chat/identity_conflicts.py",
    "tests/test_codekarl.py": "tools/chat/test_integration.py",
    "tests/test_identity_conflict.py": "tools/chat/test_identity_conflict.py",
}
SOURCES = [
    *EXTRA.values(),
    "tools/chat/adapter.py",
    "tools/chat/inputs.py",
    "tools/chat/ck_chat.py",
    "tools/ck-chat",
]


class SharedRecord(TypedDict):
    """The shared chat inputs a stage tested, as its manifest keeps them."""

    files: dict[str, str]
    aggregate: str


class StageManifest(TypedDict):
    """`tested.json` in a stage directory: what was tested, from what, against which shared files.

    `shared` is absent from stages made before the shared inputs were bound."""

    baseline: dict[str, str | None]
    installed: dict[str, str | None]
    source: dict[str, str | None]
    shared: NotRequired[SharedRecord]


def digests(value: object, where: str, key: str) -> dict[str, str | None]:
    if not isinstance(value, dict) or not all(
        isinstance(name, str) and (item is None or isinstance(item, str))
        for name, item in value.items()
    ):
        raise RecordError(
            f"{where}: {key!r} must map file names to digests or null; "
            "restage with: python3 tools/chat/install.py stage"
        )
    return value


def load_stage_manifest(raw: object, where: str) -> StageManifest:
    """A stage manifest, or a RecordError naming the field; unknown additive fields are kept."""
    manifest = as_object(raw, where)
    for key in ("baseline", "installed", "source"):
        digests(manifest.get(key), where, key)
    if "shared" in manifest:
        shared = as_object(manifest["shared"], f"{where}: shared")
        files = shared.get("files")
        if (
            not isinstance(files, dict)
            or not all(
                isinstance(name, str) and isinstance(item, str) for name, item in files.items()
            )
            or not isinstance(shared.get("aggregate"), str)
        ):
            raise RecordError(
                f"{where}: 'shared' must hold the tested files and their aggregate; "
                "restage with: python3 tools/chat/install.py stage"
            )
    return cast("StageManifest", manifest)  # each declared field was checked above


def digest(path: Path) -> str | None:
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None


IDENTITY = {"CHAT_AS", "CHAT_AGENT_ID", "CHAT_ROLE", "CHAT_PROJECT"}


def suite_environment() -> dict[str, str]:
    """The suite's environment: the caller's, minus what identifies the calling agent session or
    terminal, so the result is the same for a manager, worker, reviewer or the owner."""
    kept = {
        name: value
        for name, value in os.environ.items()
        if name not in IDENTITY and not name.startswith("CMUX_")
    }
    return {**kept, "CK_CHAT_SOURCE_ROOT": str(ROOT)}


def prepare_stage() -> tuple[Path, StageManifest]:
    """Copy the shared trees, apply Codekarl's files and run the suite; returns the stage and
    its manifest. A failure leaves the staged directory behind for inspection."""
    root = shared_root()
    before = shared_inputs(root)  # refuses links before anything is copied or executed
    sources = {name: digest(ROOT / name) for name in SOURCES}
    baseline = {name: before.get(f"chat/scripts/{name}") for name in [*TRANSFORMS, *EXTRA]}
    destination = Path(tempfile.mkdtemp(prefix="ck-chat-tested-"))
    for name in TREES:
        # A link that appears during the copy is copied as a link, not followed.
        shutil.copytree(
            root / name,
            destination / name,
            symlinks=True,
            ignore=shutil.ignore_patterns("__pycache__"),
        )
    if shared_inputs(destination) != before or shared_inputs(root) != before:
        raise ValueError(f"{CHANGED}; restage")
    scripts = destination / "chat/scripts"
    for name, transform in TRANSFORMS.items():
        path = scripts / name
        path.write_text(transform(path.read_text()))
    for name, source in EXTRA.items():
        shutil.copyfile(ROOT / source, scripts / name)
    result = subprocess.run(
        [sys.executable, "-m", "unittest", "discover", "-s", str(scripts / "tests"), "-b", "-q"],
        check=False,
        env=suite_environment(),
    )
    if result.returncode:
        raise ValueError(f"Staged integration tests failed; inspect {destination}")
    if any(digest(ROOT / name) != expected for name, expected in sources.items()):
        raise ValueError("Codekarl source changed during staging; restage")
    manifest: StageManifest = {
        "baseline": baseline,
        "installed": {name: digest(scripts / name) for name in baseline},
        "source": sources,
        "shared": {"files": before, "aggregate": aggregate(before)},
    }
    return destination, manifest


def stage() -> None:
    destination, manifest = prepare_stage()
    (destination / "tested.json").write_text(json.dumps(manifest, indent=2))
    print(destination)


def ci() -> None:
    """The batch CI step: the same copy-and-test as stage, then bind the inputs it tested."""
    destination, manifest = prepare_stage()
    shared = manifest.get("shared")
    if shared is None:  # prepare_stage always binds the shared inputs it tested
        raise ValueError("the staged manifest binds no shared chat inputs; rerun the CI step")
    print(f"chat-inputs {shared['aggregate']} files={len(shared['files'])}", flush=True)
    target = os.environ.get("CK_CHAT_INPUTS")
    if target:
        record: ChatInputs = {
            "schema": 1,
            "files": shared["files"],
            "aggregate": shared["aggregate"],
        }
        atomic(Path(target), json.dumps(record, sort_keys=True).encode(), 0o644)
    shutil.rmtree(destination)


def atomic(path: Path, data: bytes, mode: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=path.parent, prefix=path.name + ".")
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        Path(temporary).chmod(mode)
        Path(temporary).replace(path)
    finally:
        if Path(temporary).exists():
            Path(temporary).unlink()


def accepted_receipt_aggregate(state: AcceptedPointer) -> str | None:
    """The chat-inputs aggregate of the accepted CI receipt: follow `previous` past docs receipts
    to the first CI receipt. None when there is none or it binds nothing (never invented).

    Only `kind`, `chat_inputs` and `previous` are read, so any receipt form of any age answers."""
    path, seen = state.get("receipt"), set()
    while path and path not in seen:
        seen.add(path)
        try:
            receipt = json.loads(Path(path).read_bytes())
        except (OSError, ValueError):
            return None
        if not isinstance(receipt, dict):
            return None
        if receipt.get("kind") == "ci":
            record = receipt.get("chat_inputs")
            return record["aggregate"] if valid_record(record) else None
        if receipt.get("kind") != "docs":
            return None
        previous = receipt.get("previous")  # a restore receipt has none
        path = previous if isinstance(previous, str) else None
    return None


def apply(stage_path: Path) -> None:  # noqa: PLR0912,PLR0915  # linear workflow kept whole so failure ordering stays visible
    if Path.home() / "work/codekarl" != ROOT:
        raise ValueError(
            "Install from the synchronized root tools/chat/install.py, after acceptance"
        )
    state = read_pointer(ROOT)
    manifest_path = stage_path / "tested.json"
    manifest = load_stage_manifest(json.loads(manifest_path.read_text()), str(manifest_path))
    if set(manifest["source"]) != set(SOURCES):
        raise ValueError("Unexpected staged source paths")
    root = shared_root()
    shared_record = manifest.get("shared")
    staged = shared_record["aggregate"] if shared_record else None
    if not staged or aggregate(shared_inputs(root)) != staged:
        raise ValueError("Shared chat inputs changed since staging; restage and retest")
    paths = [
        *manifest["source"],
        "tools/chat/install.py",
        "tools/ck-chat",
        "skills/ck-chat/SKILL.md",
    ]
    for name in paths:
        if digest(ROOT / name) != state["fingerprint"]["files"][name]["sha256"]:
            raise ValueError(f"Source is not accepted: {name}")
    for name, expected in manifest["source"].items():
        if digest(ROOT / name) != expected:
            raise ValueError("Source changed after staging; restage and retest")
    if set(manifest["baseline"]) != set(TRANSFORMS) | set(EXTRA):
        raise ValueError("Unexpected staged installation paths")
    skill = root / "ck-chat"
    claude_skill = Path.home() / ".claude/skills/ck-chat"
    skill_target = ROOT / "skills/ck-chat"
    if (skill.exists() or skill.is_symlink()) and not (
        skill.is_symlink() and skill.resolve() == skill_target
    ):
        raise ValueError("An existing ck-chat skill needs reconciliation; nothing installed")
    if (claude_skill.exists() or claude_skill.is_symlink()) and not (
        claude_skill.is_symlink() and claude_skill.resolve() == skill_target
    ):
        raise ValueError("An existing Claude ck-chat skill needs reconciliation; nothing installed")
    for name, expected in manifest["baseline"].items():
        if digest(root / "chat/scripts" / name) != expected:
            raise ValueError(
                f"Shared file changed while testing: {name}; restage, preserving those edits"
            )
        if digest(stage_path / "chat/scripts" / name) != manifest["installed"][name]:
            raise ValueError(f"Staged file changed after tests: {name}")
        expected_data = (
            TRANSFORMS[name]((root / "chat/scripts" / name).read_text()).encode()
            if name in TRANSFORMS
            else (ROOT / EXTRA[name]).read_bytes()
        )
        if expected_data != (stage_path / "chat/scripts" / name).read_bytes():
            raise ValueError(f"Staged output does not match the accepted adapter: {name}")
    accepted = accepted_receipt_aggregate(state)
    print(f"Staged shared chat inputs: {staged}")
    print(f"Accepted batch receipt chat inputs: {accepted or 'unavailable'}")
    if accepted and accepted != staged:
        print(
            "These differ: shared chat code changed between publication and this stage "
            "(reported, not refused; the stage tested the exact combination being installed)."
        )
    backup = Path.home() / ".local/state/chat/codekarl-install" / str(time.time_ns())
    backup.mkdir(parents=True, mode=0o700)
    for name in manifest["baseline"]:
        target = root / "chat/scripts" / name
        if target.exists():
            saved = backup / name
            saved.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(target, saved)
        mode = target.stat().st_mode & 0o777 if target.exists() else 0o644
        atomic(target, (stage_path / "chat/scripts" / name).read_bytes(), mode)
    if skill.is_symlink() and skill.resolve() == skill_target:
        pass
    else:
        skill.symlink_to(skill_target, target_is_directory=True)
    if not claude_skill.is_symlink():
        claude_skill.parent.mkdir(parents=True, exist_ok=True)
        claude_skill.symlink_to(skill_target, target_is_directory=True)
    print(f"Installed accepted adapter and ck-chat skill. Backup: {backup}")
    print("Bridge restart and Codekarl project registration are separate rollout steps.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("stage")
    commands.add_parser("ci", help="the batch CI step: stage, test, print and record the inputs")
    install = commands.add_parser("apply")
    install.add_argument("stage", type=Path)
    args = parser.parse_args()
    if args.command == "stage":
        stage()
    elif args.command == "ci":
        ci()
    else:
        apply(args.stage)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, RecordError) as error:
        print(f"ck-chat install: {error}", file=sys.stderr)
        sys.exit(1)
