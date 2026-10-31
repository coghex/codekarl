"""Bootstrap transaction used only from an immutable reviewed capture."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
from collections.abc import Callable, Mapping
from pathlib import Path

from . import (
    classes,
    docsqueue,
    lease,
    locks,
    meta,
    pipeline_api,
    pointer,
    procscan,
    reviews,
    sessions,
)
from .pointer import atomic_bytes, canonical, file_digest
from .records import (
    AcceptedPointer,
    Author,
    AuthorsLedger,
    BootstrapCancelled,
    BootstrapCIReceipt,
    BootstrapFailed,
    BootstrapFrozen,
    BootstrapIntegrationSubject,
    BootstrapPublished,
    BootstrapPublishing,
    BootstrapReceipt,
    BootstrapRestored,
    BootstrapRestoring,
    BootstrapSpecSubject,
    BootstrapValidated,
    DocsRestoreReceipt,
    EvidenceIndex,
    EvidenceRow,
    GateConfig,
    Manifest,
    MetaEvent,
    PointerSchema1,
    PointerV2,
    RestoreConflict,
    RestoreEntry,
    RestoreLink,
    SubmissionSubject,
    is_accepted_pointer,
    load_accepted_pointer,
    load_authors_ledger,
    load_bootstrap_frozen,
    load_bootstrap_publishing,
    load_bootstrap_receipt,
    load_bootstrap_restoring,
    load_bootstrap_validated,
    load_ci_receipt,
    load_docs_entry,
    load_docs_receipt,
    load_evidence_index,
    load_manifest,
    load_pointer,
    read_json,
)

Hook = Callable[..., None]  # a test seam: called with the stage name and keyword details
Checker = Callable[[Path, Path, int], bool]  # (candidate, log, ci lock descriptor) -> passed


def _write(path: Path, value: Mapping[str, object]) -> None:
    payload = canonical(value)
    if path.exists() and path.read_bytes() != payload:
        raise RuntimeError(f"Immutable bootstrap record changed: {path}")
    if not path.exists():
        atomic_bytes(path, payload)


def _frozen(directory: Path) -> BootstrapFrozen:
    return load_bootstrap_frozen(
        read_json(directory / "frozen.json"), str(directory / "frozen.json")
    )


def _validated(directory: Path) -> BootstrapValidated:
    path = directory / "validated.json"
    return load_bootstrap_validated(read_json(path), str(path))


def _publishing(directory: Path) -> BootstrapPublishing:
    path = directory / "publishing.json"
    return load_bootstrap_publishing(read_json(path), str(path))


def _restoring(directory: Path) -> BootstrapRestoring:
    path = directory / "restoring.json"
    return load_bootstrap_restoring(read_json(path), str(path))


def _manifest(hub: Path, sid: str) -> tuple[Path, Manifest]:
    path = hub / ".pipeline/queue" / f"{sid}.json"
    return path, load_manifest(read_json(path), str(path))


def _evidence_rows(
    index: EvidenceIndex, evidence_root: Path, evidence_copy: Path
) -> list[EvidenceRow]:
    rows = [index["spec"], index["authors"], *index["receipts"]]
    index_copy = (evidence_copy / "index.json").resolve()
    for row in rows:
        name = row["path"]
        path = Path(name)
        if path.is_absolute() or ".." in path.parts or not path.parts:
            raise RuntimeError(f"Unsafe bootstrap evidence path: {name}")
        for root in (evidence_root, evidence_copy):
            resolved_root = root.resolve()
            target = (root / path).resolve()
            if not target.is_relative_to(resolved_root):
                raise RuntimeError(f"Unsafe bootstrap evidence path: {name}")
        if (evidence_copy / path).resolve() == index_copy:
            raise RuntimeError(f"Unsafe bootstrap evidence path collides with index copy: {name}")
    return rows


class Bootstrap:
    def __init__(  # explicit test seams
        self,
        hub: Path,
        capture: Path,
        checker: Checker | None = None,
        *,
        hook: Hook | None = None,
        process_lister: procscan.Lister | None = None,
    ) -> None:
        self.hub = hub
        self.capture = capture
        self.sid = capture.name
        self.directory = hub / ".pipeline/bootstrap"
        self.pipeline = pipeline_api.load()
        self.pointer_module = pointer
        self.checker = checker or self._full_check
        self.hook = hook
        self.process_lister = process_lister

    def _point(self, name: str, **details: object) -> None:
        if self.hook is not None:
            self.hook(name, **details)

    def _require_quiescent(self) -> None:
        if self.process_lister is None:
            procscan.require_quiescent(self.hub)
        else:
            procscan.require_quiescent(self.hub, self.process_lister)

    def _full_check(self, candidate: Path, log: Path, ci_fd: int) -> bool:
        env = os.environ.copy()
        env["CK_CI_LOCK_FD"] = str(ci_fd)
        result = lease.run(
            [sys.executable, str(candidate / "tools/check.py")],
            check=False,
            cwd=candidate,
            env=env,
            pass_fds=(ci_fd,),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        log.write_text(result.stdout)
        return result.returncode == 0

    def prepare(self, sid: str, expected_manifest: str) -> str:  # noqa: PLR0912,PLR0915  # linear workflow kept whole so failure ordering stays visible
        if sid != self.sid:
            raise RuntimeError("prepare submission must be this helper's immutable capture")
        manifest_path, manifest = _manifest(self.hub, sid)
        if file_digest(manifest_path) != expected_manifest:
            raise RuntimeError("submission manifest digest differs from --manifest-sha256")
        bid = str(time.time_ns())
        directory = self.directory / bid
        with locks.ci(self.hub) as ci_fd:
            if (self.hub / ".pipeline/accepted-v2.json").exists():
                raise RuntimeError("accepted-v2.json already exists")
            self._recover_previous()
            docsqueue.Queue(self.hub).flush()
            verified = self.pipeline.Pipeline(self.hub).verified_state()
            if not is_accepted_pointer(verified):
                raise RuntimeError("accepted.json is already tombstoned")
            state = verified
            self._require_quiescent()
            self.pipeline.clean(self.capture)
            capture_fingerprint = self.pipeline.fingerprint(self.capture)
            if capture_fingerprint != manifest.get("fingerprint"):
                raise RuntimeError("submission capture fingerprint changed")
            if sid in self.pipeline.Pipeline(self.hub).blocked():
                raise RuntimeError("submission is legacy-blocked")
            if sid in state.get("submissions", []):
                raise RuntimeError("submission is already accepted")
            if set(manifest.get("hashes", [])) != set(capture_fingerprint["hashes"]) - set(
                state["fingerprint"]["hashes"]
            ):
                raise RuntimeError("submission hashes do not match the capture closure")
            queue_snapshot = {
                path.name: file_digest(path)
                for path in sorted((self.hub / ".pipeline/queue").glob("*.json"))
            }
            pending = [
                load_manifest(read_json(path), str(path))
                for path in sorted((self.hub / ".pipeline/queue").glob("*.json"))
            ]
            superseded: list[str] = []
            for other in pending:
                if other["id"] == sid or other["id"] in state.get("submissions", []):
                    continue
                if not set(other.get("hashes", [])) <= set(manifest.get("hashes", [])):
                    raise RuntimeError(f"Unrelated pending legacy submission: {other['id']}")
                superseded.append(other["id"])
            with locks.state(self.hub):
                base_path = self.hub / ".pipeline/accepted.json"
                base_bytes = base_path.read_bytes()
                frozen: BootstrapFrozen = {
                    "bootstrap": bid,
                    "submission": sid,
                    "base_pointer_sha256": hashlib.sha256(base_bytes).hexdigest(),
                    "queue_snapshot": queue_snapshot,
                    "manifest_sha256": file_digest(manifest_path),
                    "fingerprint_sha256": hashlib.sha256(
                        canonical(capture_fingerprint)
                    ).hexdigest(),
                    "superseded_legacy": superseded,
                    "owner": {
                        "pid": os.getpid(),
                        "pid_start": procscan.process_start(os.getpid()),
                    },
                }
                _write(directory / "frozen.json", frozen)
                self._point("after_frozen", bid=bid, directory=directory)
                atomic_bytes(directory / "legacy-pointer.json", base_bytes)
                self._point("after_legacy_pointer", bid=bid, directory=directory)
                hold = pointer.tombstone(self.hub, state, bootstrap=bid)
                hold_bytes = canonical(hold)
                atomic_bytes(directory / "hold.json", hold_bytes)
                self._point("after_hold_record", bid=bid, directory=directory)
                atomic_bytes(base_path, hold_bytes)
                self._point("after_hold_pointer", bid=bid, directory=directory)
            candidate = self.hub / ".pipeline/checkpoints" / f"bootstrap-{bid}"
            self.pipeline.clone(Path(state["repo"]), candidate, self.hub)
            conflicts = self.pipeline.pull(candidate, self.capture, manifest["hashes"])
            expected_hashes = set(state["fingerprint"]["hashes"]) | set(manifest["hashes"])
            if set(self.pipeline.hashes(candidate)) != expected_hashes:
                raise RuntimeError("bootstrap candidate hashes differ from base plus submission")
            self.pipeline.clean(candidate)
            before_check = self.pipeline.fingerprint(candidate)
            log = self.hub / ".pipeline/checkpoints" / f"bootstrap-{bid}.log"
            passed = not conflicts and self.checker(candidate, log, ci_fd)
            if passed:
                self.pipeline.clean(candidate)
                if self.pipeline.fingerprint(candidate) != before_check:
                    raise RuntimeError("CI changed the bootstrap candidate fingerprint")
                receipt_path = self.hub / ".pipeline/ci" / f"bootstrap-{bid}.json"
                ci_receipt: BootstrapCIReceipt = {
                    "kind": "ci",
                    "passed": True,
                    "fingerprint": self.pipeline.fingerprint(candidate),
                    "submissions": [sid],
                    "bootstrap": bid,
                    "log": str(log),
                }
                _write(receipt_path, ci_receipt)
                validated: BootstrapValidated = {
                    "bootstrap": bid,
                    "submission": sid,
                    "base_pointer_sha256": frozen["base_pointer_sha256"],
                    "manifest_sha256": frozen["manifest_sha256"],
                    "fingerprint_sha256": frozen["fingerprint_sha256"],
                    "candidate": str(candidate),
                    "candidate_fingerprint": ci_receipt["fingerprint"],
                    "candidate_fingerprint_sha256": hashlib.sha256(
                        canonical(ci_receipt["fingerprint"])
                    ).hexdigest(),
                    "ci_receipt": str(receipt_path),
                    "ci_receipt_sha256": file_digest(receipt_path),
                    "helper_sha256": file_digest(self.capture / "tools/gate/bootstrap.py"),
                    "superseded_legacy": superseded,
                }
                with locks.state(self.hub):
                    _write(directory / "validated.json", validated)
                self._point("after_validated", bid=bid, directory=directory)
                print(bid)
                return bid
            with locks.state(self.hub):
                failed: BootstrapFailed = {
                    "reason": "validation failed",
                    "conflicts": conflicts,
                    "at": time.time_ns(),
                }
                _write(directory / "failed.json", failed)
            self._point("after_failed", bid=bid, directory=directory)
            self.restore(bid, ci_held=True)
            raise RuntimeError(f"Bootstrap validation failed; restored legacy state. See {log}")

    def _recover_previous(self) -> None:
        for directory in sorted(self.directory.glob("*/")) if self.directory.exists() else []:
            if (directory / "published.json").exists() or (directory / "restored.json").exists():
                continue
            if (directory / "validated.json").exists() and not any(
                (directory / name).exists() for name in ("failed.json", "cancelled.json")
            ):
                raise RuntimeError(f"Bootstrap {directory.name} is validated and awaiting review")
            if (directory / "frozen.json").exists():
                with locks.state(self.hub):
                    if not any(
                        (directory / name).exists() for name in ("failed.json", "cancelled.json")
                    ):
                        _write(
                            directory / "failed.json",
                            {"reason": "interrupted", "at": time.time_ns()},
                        )
                self.restore(directory.name, ci_held=True)

    def restore(self, bid: str, *, ci_held: bool = False) -> None:
        if ci_held:
            with locks.state(self.hub):
                self._restore_locked(bid)
        else:
            with locks.ci(self.hub), locks.state(self.hub):
                self._restore_locked(bid)

    def _build_restore_chain(self, bid: str, base_bytes: bytes) -> BootstrapRestoring:
        pipeline = self.pipeline
        current_bytes = base_bytes
        current: AcceptedPointer = load_accepted_pointer(
            json.loads(current_bytes), "legacy pointer"
        )
        queue_rows = docsqueue._queue(self.hub)
        virtual_rows = [row.copy() for row in queue_rows]
        live_rows = [row for row in virtual_rows if row.get("state") in ("pending", "blocked")]
        entries: list[RestoreEntry] = []
        links: list[RestoreLink] = []
        conflicts: list[RestoreConflict] = []
        for row in live_rows:
            entry_path = self.hub / ".pipeline/docs" / f"{row['id']}.json"
            entry_sha = file_digest(entry_path)
            blocked_ids = docsqueue._blocked_entries(virtual_rows)
            if row["id"] in blocked_ids or row.get("state") == "blocked":
                conflicts.append(
                    {
                        "id": row["id"],
                        "entry_sha256": entry_sha,
                        "state": "blocked",
                        "entry": row,
                    }
                )
                continue
            if set(row.get("hashes", [])) <= set(current["fingerprint"]["hashes"]):
                entries.append(
                    {
                        "id": row["id"],
                        "entry_sha256": entry_sha,
                        "state": "published",
                        "already_present": True,
                        "entry": row,
                    }
                )
                continue
            name = f"restore-{bid}-{row['id']}-{time.time_ns()}"
            candidate = self.hub / ".pipeline/checkpoints" / name
            pipeline.clone(Path(current["repo"]), candidate, self.hub)
            pipeline.clean(Path(row["repo"]))
            missing = sorted(set(row.get("hashes", [])) - set(current["fingerprint"]["hashes"]))
            found = pipeline.pull(candidate, Path(row["repo"]), missing)
            if found:
                row["state"] = "conflict"
                row["conflicts"] = found
                conflicts.append(
                    {
                        "id": row["id"],
                        "entry_sha256": entry_sha,
                        "state": "conflict",
                        "conflicts": found,
                        "entry": row,
                    }
                )
                continue
            pipeline.clean(candidate)
            fingerprint = pipeline.fingerprint(candidate)
            if set(fingerprint["hashes"]) != set(current["fingerprint"]["hashes"]) | set(
                row.get("hashes", [])
            ):
                raise RuntimeError(f"Docs restore candidate has unexpected hashes: {row['id']}")
            old_code = {
                name: value
                for name, value in current["fingerprint"]["files"].items()
                if not pipeline.doc_path(name)
            }
            new_code = {
                name: value
                for name, value in fingerprint["files"].items()
                if not pipeline.doc_path(name)
            }
            if old_code != new_code:
                raise RuntimeError(f"Docs restore changed executable files: {row['id']}")
            receipt_path = candidate.parent / f"{candidate.name}.json"
            receipt: DocsRestoreReceipt = {
                "kind": "docs",
                "passed": True,
                "inherits": current.get("receipt"),
            }
            atomic_bytes(receipt_path, canonical(receipt))
            next_state: PointerSchema1 = {
                "submissions": current["submissions"],
                "repo": str(candidate),
                "fingerprint": fingerprint,
                "receipt": str(receipt_path),
                "previous": current.get("receipt"),
            }
            next_bytes = canonical(next_state)
            link: RestoreLink = {
                "id": row["id"],
                "entry_sha256": entry_sha,
                "candidate": str(candidate),
                "fingerprint": fingerprint,
                "receipt": str(receipt_path),
                "receipt_sha256": file_digest(receipt_path),
                "hashes": missing,
                "previous_pointer_sha256": hashlib.sha256(current_bytes).hexdigest(),
                "pointer": next_bytes.decode(),
            }
            links.append(link)
            entries.append(
                {
                    "id": row["id"],
                    "entry_sha256": entry_sha,
                    "state": "published",
                    "entry": row,
                }
            )
            current, current_bytes = next_state, next_bytes
            self._point("after_restore_link", bid=bid, entry=row, link=link)
        return {
            "mode": "restored",
            "pointer": current_bytes.decode(),
            "pointer_sha256": hashlib.sha256(current_bytes).hexdigest(),
            "entries": entries,
            "links": links,
            "conflicts": conflicts,
        }

    def _verify_restore_chain(self, directory: Path, restoring: BootstrapRestoring) -> None:
        pipeline = self.pipeline
        previous = (directory / "legacy-pointer.json").read_bytes()
        previous_state = load_accepted_pointer(json.loads(previous), "legacy pointer")
        for link in restoring["links"]:
            if hashlib.sha256(previous).hexdigest() != link["previous_pointer_sha256"]:
                raise RuntimeError("Bootstrap restore chain previous pointer digest changed")
            candidate = Path(link["candidate"])
            pipeline.clean(candidate)
            if pipeline.fingerprint(candidate) != link["fingerprint"]:
                raise RuntimeError(f"Bootstrap restore checkpoint changed: {candidate}")
            if set(link["fingerprint"]["hashes"]) != set(
                previous_state["fingerprint"]["hashes"]
            ) | set(link["hashes"]):
                raise RuntimeError(f"Bootstrap restore chain hashes changed: {candidate}")
            old_code = {
                name: value
                for name, value in previous_state["fingerprint"]["files"].items()
                if not pipeline.doc_path(name)
            }
            new_code = {
                name: value
                for name, value in link["fingerprint"]["files"].items()
                if not pipeline.doc_path(name)
            }
            if old_code != new_code:
                raise RuntimeError(f"Bootstrap restore chain changed code: {candidate}")
            receipt_path = Path(link["receipt"])
            if file_digest(receipt_path) != link["receipt_sha256"]:
                raise RuntimeError(f"Bootstrap restore receipt changed: {receipt_path}")
            receipt = load_docs_receipt(read_json(receipt_path), str(receipt_path))
            if (
                receipt.get("kind") != "docs"
                or receipt.get("passed") is not True
                or receipt.get("inherits") != previous_state.get("receipt")
            ):
                raise RuntimeError(f"Bootstrap restore receipt is not legacy docs: {receipt_path}")
            next_bytes = link["pointer"].encode()
            next_state = load_accepted_pointer(json.loads(next_bytes), f"{directory}: restore link")
            if (
                next_state.get("repo") != str(candidate)
                or next_state.get("fingerprint") != link["fingerprint"]
            ):
                raise RuntimeError("Bootstrap restore chain pointer differs from its checkpoint")
            previous, previous_state = next_bytes, next_state
        final_bytes = restoring["pointer"].encode()
        if previous != final_bytes:
            raise RuntimeError("Bootstrap restore final pointer differs from recorded chain")
        live = {
            row["id"]: file_digest(self.hub / ".pipeline/docs" / f"{row['id']}.json")
            for row in docsqueue._queue(self.hub)
            if row.get("state") in ("pending", "blocked")
        }
        recorded = {
            row["id"]: row["entry_sha256"]
            for row in [*restoring["entries"], *restoring["conflicts"]]
        }
        for identifier, digest in recorded.items():
            entry_path = self.hub / ".pipeline/docs" / f"{identifier}.json"
            if not entry_path.exists() or file_digest(entry_path) != digest:
                raise RuntimeError(f"Bootstrap restore docs entry changed: {identifier}")
        if any(recorded.get(identifier) != digest for identifier, digest in live.items()):
            raise RuntimeError("Bootstrap restore intent omits or mismatches a live docs entry")

    def _verify_restore_base(self, directory: Path, frozen: BootstrapFrozen) -> bytes:
        legacy_pointer = directory / "legacy-pointer.json"
        if not legacy_pointer.exists():
            raise RuntimeError("Installed hold has no saved legacy pointer")
        saved = legacy_pointer.read_bytes()
        if file_digest(legacy_pointer) != frozen["base_pointer_sha256"]:
            raise RuntimeError("Saved legacy pointer digest does not match freeze")
        base_state = load_accepted_pointer(json.loads(saved), str(legacy_pointer))
        base_repo = Path(base_state["repo"])
        self.pipeline.clean(base_repo)
        if self.pipeline.fingerprint(base_repo) != base_state["fingerprint"]:
            raise RuntimeError("Saved legacy checkpoint fingerprint changed")
        return saved

    def _ack_restore(self, directory: Path, restoring: BootstrapRestoring) -> None:
        rows: list[RestoreEntry | RestoreConflict] = [
            *restoring["entries"],
            *restoring["conflicts"],
        ]
        statuses = {row["id"]: row for row in rows}
        for identifier, outcome in statuses.items():
            path = self.hub / ".pipeline/docs" / f"{identifier}.json"
            if not path.exists():
                raise RuntimeError(f"Bootstrap restore docs entry changed: {identifier}")
            entry = load_docs_entry(read_json(path), str(path))
            expected = outcome["entry"].copy()
            expected["state"] = outcome["state"]
            found = outcome.get("conflicts")
            if found and isinstance(found, list):
                expected["conflicts"] = found
            if file_digest(path) == outcome["entry_sha256"]:
                atomic_bytes(path, canonical(expected))
            elif entry != expected:
                raise RuntimeError(f"Bootstrap restore docs entry changed: {identifier}")
        restored: BootstrapRestored = {
            "mode": "restored",
            "pointer_sha256": restoring["pointer_sha256"],
            "conflicts": restoring["conflicts"],
        }
        _write(directory / "restored.json", restored)

    def _restore_locked(self, bid: str) -> None:
        directory = self.directory / bid
        accepted_path = self.hub / ".pipeline/accepted.json"
        accepted_bytes = accepted_path.read_bytes()
        if (directory / "restored.json").exists():
            return
        frozen = _frozen(directory)
        if not any((directory / name).exists() for name in ("failed.json", "cancelled.json")):
            interrupted: BootstrapFailed = {"reason": "interrupted", "at": time.time_ns()}
            _write(directory / "failed.json", interrupted)
        if (directory / "restoring.json").exists():
            restoring = _restoring(directory)
            final_bytes = restoring["pointer"].encode()
            if accepted_bytes == final_bytes:
                self._ack_restore(directory, restoring)
                return
            if (
                not (directory / "hold.json").exists()
                or accepted_bytes != (directory / "hold.json").read_bytes()
            ):
                raise RuntimeError(
                    "Bootstrap restore incident: pointer matches neither hold nor restoring intent"
                )
            self._verify_restore_base(directory, frozen)
            self._verify_restore_chain(directory, restoring)
            atomic_bytes(accepted_path, final_bytes)
            self._point("after_restore_pointer", bid=bid, directory=directory)
            self._ack_restore(directory, restoring)
            return
        accepted = load_pointer(json.loads(accepted_bytes), str(accepted_path))
        if is_accepted_pointer(accepted):
            never_held: BootstrapRestored = {
                "mode": "never-held",
                "pointer_sha256": file_digest(accepted_path),
            }
            _write(directory / "restored.json", never_held)
            return
        if (
            not (directory / "hold.json").exists()
            or accepted_bytes != (directory / "hold.json").read_bytes()
        ):
            raise RuntimeError("Bootstrap restore incident: accepted pointer is not the saved hold")
        saved = self._verify_restore_base(directory, frozen)
        restoring = self._build_restore_chain(bid, saved)
        _write(directory / "restoring.json", restoring)
        self._point("after_restoring", bid=bid, directory=directory, restoring=restoring)
        self._verify_restore_chain(directory, restoring)
        atomic_bytes(accepted_path, restoring["pointer"].encode())
        self._point("after_restore_pointer", bid=bid, directory=directory)
        self._ack_restore(directory, restoring)

    def cancel(self, bid: str, reason: str) -> None:
        directory = self.directory / bid
        with locks.ci(self.hub), locks.state(self.hub):
            if (directory / "publishing.json").exists():
                raise RuntimeError("Cannot cancel after publication intent")
            if not (directory / "cancelled.json").exists():
                cancelled: BootstrapCancelled = {"reason": reason, "at": time.time_ns()}
                _write(directory / "cancelled.json", cancelled)
            self._point("after_cancelled", bid=bid, directory=directory)
            self._restore_locked(bid)

    def publish(self, bid: str, evidence: Path, expected: str) -> None:  # noqa: PLR0912,PLR0915  # linear workflow kept whole so failure ordering stays visible
        directory = self.directory / bid
        if (directory / "published.json").exists():
            publishing = _publishing(directory)
            accepted_path = self.hub / ".pipeline/accepted.json"
            v2_path = self.hub / ".pipeline/accepted-v2.json"
            intended_tombstone = publishing["tombstone"].encode()
            intended_v2 = publishing["v2"].encode()
            if accepted_path.read_bytes() != intended_tombstone:
                raise RuntimeError("Published bootstrap final tombstone changed")
            if not v2_path.exists():
                raise RuntimeError("Published bootstrap v2 pointer is missing")
            v2_bytes = v2_path.read_bytes()
            if v2_bytes != intended_v2 and not docsqueue.is_docs_descendant(
                self.hub, v2_bytes, intended_v2
            ):
                raise RuntimeError(
                    "Published bootstrap v2 pointer is not the intended docs descendant"
                )
            self._activation_tail(bid, publishing["evidence_sha256"])
            return
        run_tail = False
        with locks.ci(self.hub), locks.state(self.hub), locks.meta(meta.Store(self.hub).repo):
            validated = _validated(directory)
            if any((directory / name).exists() for name in ("failed.json", "cancelled.json")):
                raise RuntimeError("Cannot publish a failed or cancelled bootstrap bid")
            evidence_root = self.hub / ".pipeline/coordination/gate-bootstrap"
            evidence_copy = directory / "evidence"
            frozen = _frozen(directory)
            base_bytes = (directory / "legacy-pointer.json").read_bytes()
            hold_bytes = (directory / "hold.json").read_bytes()
            accepted_path = self.hub / ".pipeline/accepted.json"
            accepted_bytes = accepted_path.read_bytes()
            existing_intent = (
                _publishing(directory) if (directory / "publishing.json").exists() else None
            )
            recovering = False
            published_recovery = False
            if existing_intent:
                intended_final = existing_intent["tombstone"].encode()
                intended_v2 = existing_intent["v2"].encode()
                v2_path = self.hub / ".pipeline/accepted-v2.json"
                was_published = (directory / "published.json").exists()
                v2_ok = not v2_path.exists() or v2_path.read_bytes() == intended_v2
                if was_published and v2_path.exists():
                    v2_ok = docsqueue.is_docs_descendant(
                        self.hub, v2_path.read_bytes(), intended_v2
                    )
                if accepted_bytes == intended_final and v2_ok:
                    recovering = True
                    published_recovery = True
                elif accepted_bytes != hold_bytes:
                    raise RuntimeError(
                        "Bootstrap pointer state does not match publication recovery intent"
                    )
            elif accepted_bytes != hold_bytes:
                raise RuntimeError("Bootstrap hold tombstone changed")
            if published_recovery:
                if existing_intent is None:  # unreachable: recovery is only set from an intent
                    raise RuntimeError("Publication intent is missing")
                if existing_intent["evidence_sha256"] != expected:
                    raise RuntimeError("Publication intent evidence digest changed")
                copied_index = evidence_copy / "index.json"
                if not copied_index.exists() or file_digest(copied_index) != expected:
                    raise RuntimeError("Copied evidence index changed")
                evidence_raw = copied_index.read_bytes()
                index = load_evidence_index(json.loads(evidence_raw), str(copied_index))
                listed = _evidence_rows(index, evidence_root, evidence_copy)
                for row in listed:
                    copied = evidence_copy / row["path"]
                    if not copied.exists() or file_digest(copied) != row["sha256"]:
                        raise RuntimeError(f"Copied evidence changed: {row['path']}")
            else:
                evidence_raw = evidence.read_bytes()
                if hashlib.sha256(evidence_raw).hexdigest() != expected:
                    raise RuntimeError("Evidence index digest mismatch")
                index = load_evidence_index(json.loads(evidence_raw), str(evidence))
                listed = _evidence_rows(index, evidence_root, evidence_copy)
                for row in listed:
                    path = evidence_root / row["path"]
                    if file_digest(path) != row["sha256"]:
                        raise RuntimeError(f"Evidence changed: {row['path']}")
            submission_path, manifest = _manifest(self.hub, frozen["submission"])
            if file_digest(submission_path) != frozen["manifest_sha256"]:
                raise RuntimeError("Bootstrap submission manifest changed")
            self.pipeline.clean(self.capture)
            if self.pipeline.fingerprint(self.capture) != manifest.get("fingerprint"):
                raise RuntimeError("Bootstrap submission capture changed")
            if hashlib.sha256(base_bytes).hexdigest() != frozen["base_pointer_sha256"]:
                raise RuntimeError("Saved base pointer changed")
            base_state = load_accepted_pointer(
                json.loads(base_bytes), str(directory / "legacy-pointer.json")
            )
            self.pipeline.clean(Path(base_state["repo"]))
            if self.pipeline.fingerprint(Path(base_state["repo"])) != base_state["fingerprint"]:
                raise RuntimeError("Saved base checkpoint fingerprint changed")
            if not published_recovery:
                expected_queue = {
                    path.name: file_digest(path)
                    for path in sorted((self.hub / ".pipeline/queue").glob("*.json"))
                }
                if expected_queue != frozen["queue_snapshot"]:
                    raise RuntimeError("Queue changed after bootstrap freeze")
                self._require_quiescent()
            candidate = Path(validated["candidate"])
            self.pipeline.clean(candidate)
            if (
                hashlib.sha256(canonical(self.pipeline.fingerprint(candidate))).hexdigest()
                != validated["candidate_fingerprint_sha256"]
            ):
                raise RuntimeError("Bootstrap candidate changed")
            if file_digest(Path(validated["ci_receipt"])) != validated["ci_receipt_sha256"]:
                raise RuntimeError("Bootstrap CI receipt changed")
            ci_receipt = load_ci_receipt(
                read_json(Path(validated["ci_receipt"])), validated["ci_receipt"]
            )
            if ci_receipt.get("passed") is not True:
                raise RuntimeError("Bootstrap CI receipt did not pass")
            if ci_receipt.get("fingerprint") != validated["candidate_fingerprint"]:
                raise RuntimeError("Bootstrap CI receipt fingerprint differs from validation")
            expected_candidate_hashes = set(base_state["fingerprint"]["hashes"]) | set(
                manifest["hashes"]
            )
            if set(self.pipeline.hashes(candidate)) != expected_candidate_hashes:
                raise RuntimeError("Bootstrap candidate hashes differ from base plus submission")
            helper_path = self.capture / "tools/gate/bootstrap.py"
            if file_digest(helper_path) != validated["helper_sha256"]:
                raise RuntimeError("Bootstrap helper changed after review")
            if file_digest(candidate / "tools/gate/bootstrap.py") != file_digest(helper_path):
                raise RuntimeError("Bootstrap helper differs from candidate helper")
            receipt_root = evidence_copy if published_recovery else evidence_root
            receipts = [
                load_bootstrap_receipt(
                    json.loads((receipt_root / row["path"]).read_bytes()),
                    str(receipt_root / row["path"]),
                )
                for row in index["receipts"]
                if row["path"].endswith(".json")
            ]
            authors_path = receipt_root / index["authors"]["path"]
            authors = load_authors_ledger(json.loads(authors_path.read_bytes()), str(authors_path))
            self._verify_receipts(
                bid,
                validated,
                authors,
                receipts,
                index,
                manifest,
                verify_sessions=not published_recovery,
            )
            if not published_recovery:
                evidence_copy.mkdir(parents=True, exist_ok=True)
                for row in listed:
                    source = evidence_root / row["path"]
                    atomic_bytes(evidence_copy / row["path"], source.read_bytes())
                atomic_bytes(evidence_copy / "index.json", evidence_raw)
            final_tombstone = pointer.tombstone(self.hub, base_state, bootstrap=None)
            next_pointer: PointerV2 = {
                "schema": 2,
                "repo": str(candidate),
                "fingerprint": self.pipeline.fingerprint(candidate),
                "submissions": [*base_state.get("submissions", []), frozen["submission"]],
                "receipt": validated["ci_receipt"],
                "previous": base_state.get("receipt"),
                "integration": {"bootstrap": bid, "evidence_sha256": expected},
                "superseded_legacy": frozen["superseded_legacy"],
            }
            v2_bytes = canonical(next_pointer)
            final_bytes = canonical(final_tombstone)
            if existing_intent and (
                existing_intent["tombstone"] != final_bytes.decode()
                or existing_intent["v2"] != v2_bytes.decode()
                or existing_intent["evidence_sha256"] != expected
            ):
                raise RuntimeError(
                    "Publication intent differs from recomputed candidate and evidence"
                )
            if not existing_intent:
                self._point("before_publishing", bid=bid, directory=directory)
                intent: BootstrapPublishing = {
                    "tombstone": final_bytes.decode(),
                    "v2": v2_bytes.decode(),
                    "evidence_sha256": expected,
                }
                _write(directory / "publishing.json", intent)
                self._point("after_publishing", bid=bid, directory=directory)
            if not recovering:
                atomic_bytes(accepted_path, final_bytes)
                self._point("after_final_tombstone", bid=bid, directory=directory)
            if not (self.hub / ".pipeline/accepted-v2.json").exists():
                pointer.write_accepted(self.hub / ".pipeline/accepted-v2.json", v2_bytes)
                self._point("after_v2_pointer", bid=bid, directory=directory)
            if not (directory / "published.json").exists():
                published: BootstrapPublished = {
                    "v2_sha256": hashlib.sha256(v2_bytes).hexdigest(),
                    "at": time.time_ns(),
                }
                _write(directory / "published.json", published)
            run_tail = True
        if run_tail:
            self._activation_tail(bid, expected)

    def _verify_receipts(  # noqa: PLR0913  # explicit arguments; a parameter object would obscure the call sites
        self,
        bid: str,
        validated: BootstrapValidated,
        authors: AuthorsLedger,
        receipts: list[BootstrapReceipt],
        index: EvidenceIndex,
        manifest: Manifest,
        *,
        verify_sessions: bool = True,
    ) -> None:
        spec_path = Path(validated["candidate"]) / "docs/review-gate-plan.md"
        if file_digest(spec_path) != index["spec"]["sha256"]:
            raise RuntimeError("Approved specification digest is stale")
        submission_path = self.hub / ".pipeline/queue" / f"{validated['submission']}.json"
        patch_authors = authors["patches"]
        if set(patch_authors) != set(manifest.get("hashes", [])):
            raise RuntimeError("Authors ledger patches must map every submission patch exactly")
        author_count = len(authors["submission_authors"])
        for patch_hash, indexes in patch_authors.items():
            if any(position < 0 or position >= author_count for position in indexes):
                raise RuntimeError(f"Authors ledger has an invalid author index for {patch_hash}")
        subjects: dict[
            str, BootstrapSpecSubject | SubmissionSubject | BootstrapIntegrationSubject
        ] = {
            "spec": {"path": "docs/review-gate-plan.md", "sha256": file_digest(spec_path)},
            "submission": reviews.subject_for_submission(submission_path, manifest),
            "integration": {
                "bootstrap": bid,
                "validated_sha256": file_digest(self.directory / bid / "validated.json"),
                "candidate_fingerprint_sha256": validated["candidate_fingerprint_sha256"],
                "ci_receipt_sha256": validated["ci_receipt_sha256"],
                "membership_manifest_sha256": [validated["manifest_sha256"]],
            },
        }
        author_groups: dict[str, list[Author]] = {
            "spec": authors["spec_authors"],
            "submission": authors["submission_authors"],
            "integration": authors["submission_authors"],
        }
        for kind in ("spec", "submission", "integration"):
            matching = [
                row
                for row in receipts
                if row.get("kind") == kind and row.get("subject") == subjects[kind]
            ]
            if any(row.get("verdict") != "approved" for row in matching):
                raise RuntimeError(f"{kind} approval contains a non-approved verdict")
            for receipt in matching:
                reviewer = receipt["reviewer"]
                if not reviews.independent(reviewer, author_groups[kind]):
                    raise RuntimeError(f"{kind} reviewer authored this subject")
                if (
                    reviewer["role"] != "reviewer"
                    or reviewer["class"] != "A"
                    or reviewer["effort"] != "high"
                ):
                    raise RuntimeError(f"{kind} reviewer does not match Class A/high")
                if verify_sessions:
                    session_path = sessions._session_log(reviewer["client"], reviewer["session"])
                    models, _, _ = sessions.history(session_path, reviewer["client"])
                    mapping = classes.verify("reviewer", reviewer["client"], models)
                    master, _ = classes.master()
                    receipt_model = classes.normalize_model(
                        master, reviewer["client"], reviewer["model"]
                    )
                    if (
                        mapping.get("family") != reviewer["family"]
                        or mapping.get("model") != receipt_model
                        or mapping.get("effort") != reviewer["effort"]
                    ):
                        raise RuntimeError(
                            f"{kind} reviewer session evidence does not match its receipt"
                        )
            if not reviews.coverage(author_groups[kind], matching, subjects[kind]):
                raise RuntimeError(f"Insufficient opposite-family coverage for {kind}")

    def _activation_tail(self, bid: str, evidence_sha: str) -> None:
        gate_file = self.hub / ".pipeline/gate.json"
        if not gate_file.exists():
            config: GateConfig = {"schema": 2, "meta_repo": str(self.hub.parent / "codekarl-meta")}
            atomic_bytes(gate_file, canonical(config))
        self._point("after_gate_file", bid=bid)
        store = meta.Store(self.hub)
        store.ensure()
        event_rows = store.events()
        if not any(
            row.get("kind") == "bootstrap"
            and row.get("bootstrap") == bid
            and row.get("evidence_sha256") == evidence_sha
            for row in event_rows
        ):
            event = store.repo / "events" / f"{time.time_ns()}-bootstrap.json"
            bootstrap_event: MetaEvent = {
                "kind": "bootstrap",
                "bootstrap": bid,
                "evidence_sha256": evidence_sha,
            }
            store.write(event, canonical(bootstrap_event), "Import gate bootstrap evidence")
        self._point("after_bootstrap_event", bid=bid)
        self._sync_hub_root()
        self._point("after_root_sync", bid=bid)
        docsqueue.Queue(self.hub, hook=self.hook).flush()
        self._point("after_tail_docs", bid=bid)

    def _sync_hub_root(self) -> None:
        root = self.hub
        if not (root / ".pijul").exists():
            return
        state = pointer.read_pointer(root)
        self.pipeline.clean(root)
        missing = set(state["fingerprint"]["hashes"]) - self.pipeline.hashes(root)
        conflicts = self.pipeline.pull(root, Path(state["repo"]), sorted(missing))
        if conflicts:
            raise RuntimeError(f"Activation root sync has conflicts: {conflicts}")
        self.pipeline.clean(root)


def execute(hub: Path, capture: Path, argv: list[str]) -> None:
    parser = argparse.ArgumentParser(prog="bootstrap.py")
    parser.add_argument("command", choices=("prepare", "status", "publish", "cancel", "docs"))
    parser.add_argument("--submission")
    parser.add_argument("--manifest-sha256")
    parser.add_argument("--evidence", type=Path)
    parser.add_argument("--evidence-sha256")
    parser.add_argument("--reason")
    parser.add_argument("--from", dest="from_copy", type=Path)
    parser.add_argument("-m", "--message")
    parser.add_argument("paths", nargs="*")
    args = parser.parse_args(argv)
    helper = Bootstrap(hub, capture)
    if args.command == "prepare":
        if not args.submission or not args.manifest_sha256:
            raise RuntimeError("prepare requires --submission and --manifest-sha256")
        helper.prepare(args.submission, args.manifest_sha256)
    elif args.command == "status":
        if len(args.paths) != 1:
            raise RuntimeError("status requires a bootstrap BID")
        directory = helper.directory / args.paths[0]
        frozen = _frozen(directory)
        subject: dict[str, object] = {
            "submission": frozen["submission"],
            "manifest_sha256": frozen["manifest_sha256"],
            "fingerprint_sha256": frozen["fingerprint_sha256"],
        }
        if (directory / "validated.json").exists():
            validated = _validated(directory)
            subject["integration"] = {
                "bootstrap": args.paths[0],
                "validated_sha256": file_digest(directory / "validated.json"),
                "candidate_fingerprint_sha256": validated["candidate_fingerprint_sha256"],
                "ci_receipt_sha256": validated["ci_receipt_sha256"],
                "membership_manifest_sha256": [validated["manifest_sha256"]],
            }
        print(canonical(subject).decode())
    elif args.command == "publish":
        if not args.evidence or not args.evidence_sha256 or not args.paths:
            raise RuntimeError("publish BID requires --evidence FILE --evidence-sha256 HEX")
        helper.publish(args.paths[0], args.evidence, args.evidence_sha256)
    elif args.command == "cancel":
        if not args.reason or not args.paths:
            raise RuntimeError("cancel BID requires --reason")
        helper.cancel(args.paths[0], args.reason)
    else:
        if not args.from_copy or not args.paths:
            raise RuntimeError("docs requires --from COPY -m MSG PATHS")
        # Bootstrap captures share the same durable docs queue, but remain under the hold.
        docsqueue.Queue(hub).capture(
            args.from_copy, args.message or "Update documentation", args.paths, bootstrap=True
        )
