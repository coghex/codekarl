"""Frozen CI, independent integration approval, and atomic publication."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import math
import os
import re
import subprocess
import sys
import time
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import Final, Protocol, TypedDict

from . import docsqueue, lease, locks, meta, pipeline_api, pointer, reviews
from .pointer import atomic_bytes, canonical, file_digest
from .records import (
    BatchApproved,
    BatchCancelled,
    BatchCIReceipt,
    BatchFailed,
    BatchFrozen,
    BatchPublished,
    BatchPublishing,
    BatchRejected,
    BatchSubject,
    BatchValidated,
    BatchValidating,
    ChatInputs,
    CIReceipt,
    Integration,
    ManifestInfo,
    ManifestV2,
    PointerSchema1,
    PointerV2,
    RecordError,
    ReviewRow,
    SingleSubmissionTiming,
    is_batch_ci_receipt,
    is_manifest_v2,
    load_batch_approved,
    load_batch_cancelled,
    load_batch_failed,
    load_batch_frozen,
    load_batch_published,
    load_batch_publishing,
    load_batch_rejected,
    load_batch_validated,
    load_batch_validating,
    load_chat_inputs,
    load_ci_receipt,
    load_manifest,
    read_json,
)

EX_TEMPFAIL = 75  # tools/check.py: a temporary failure (sysexits.h), not a patch failure

# CK-36: a step must finish within a tenth of the watchdog, so that a loaded machine cannot time out
# a healthy run. The single-submission path enforces it from the run's own log (CK-38).
WATCHDOG_MARGIN = 10
SINGLE: Final = "single-submission"
INTEGRATION: Final = "integration-review"
# The three kinds of protocol line `check.py` writes, each matched against the whole line.
_RECIPE = re.compile(r"check: recipe (\w+); watchdog ([0-9]+(?:\.[0-9]+)?) s; steps (\S*)")
_TOOK = re.compile(r"check: step (\S+) took ([0-9]+(?:\.[0-9]+)?) s")
_SUMMARY = re.compile(r"check \w+: .* => (?:PASS|FAIL)")


def _protocol(text: str) -> tuple[list[re.Match[str]], list[re.Match[str]], list[str]] | str:
    """The recipe headers, step timings and summaries of a CI log, or why it is not clean.

    A line that begins like one of those records but is not exactly that record (a trailing space,
    a `NaN` value, an unknown shape) is an error, never silently skipped."""
    headers: list[re.Match[str]] = []
    timings: list[re.Match[str]] = []
    summaries: list[str] = []
    for line in text.split("\n"):
        if line.startswith("check: recipe"):
            matched = _RECIPE.fullmatch(line)
            if matched is None:
                return f"the CI log has a malformed recipe line: {line!r}"
            headers.append(matched)
        elif line.startswith("check: step"):
            matched = _TOOK.fullmatch(line)
            if matched is None:
                return f"the CI log has a malformed step line: {line!r}"
            timings.append(matched)
        elif re.match(r"check \w+:", line):
            if _SUMMARY.fullmatch(line) is None:
                return f"the CI log has a malformed summary line: {line!r}"
            summaries.append(line)
    return headers, timings, summaries


Hook = Callable[..., None]  # a test seam: called with the stage name and keyword details
Validator = Callable[[Path, Path, int], bool]  # (candidate, log, ci lock descriptor) -> passed


class ChatInputsModule(Protocol):
    """`tools/chat/inputs.py`, which is migrated in a later S7 slice (the chat package)."""

    InputError: type[Exception]

    def valid_record(self, record: object) -> bool: ...

    def aggregate(self, files: Mapping[str, str]) -> str: ...

    def shared_inputs(self, root: Path | None = None) -> dict[str, str]: ...

    def shared_root(self) -> Path: ...


def _inputs() -> ChatInputsModule:
    """The one definition of the shared chat inputs, from this gate's own tree."""
    path = Path(__file__).resolve().parents[1] / "chat/inputs.py"
    spec = importlib.util.spec_from_file_location("ck_chat_inputs", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load the shared chat inputs from {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def batch_path(hub: Path, bid: str) -> Path:
    return hub / ".pipeline/batches" / bid


def _write(path: Path, value: Mapping[str, object]) -> None:
    atomic_bytes(path, canonical(value))


class OwnerLabels(TypedDict, total=False):
    owner_signoff: list[str]


def owner_labels(rows: Sequence[ReviewRow]) -> OwnerLabels:
    """`{"owner_signoff": [receipt ids]}` for owner receipts among `rows`; empty when there are none,
    so a record without an owner sign-off is unchanged. An owner sign-off is never an AI verdict."""
    ids = [row["id"] for row in rows if reviews.is_owner_receipt(row)]
    return {"owner_signoff": ids} if ids else {}


def read_frozen(directory: Path) -> BatchFrozen:
    return load_batch_frozen(read_json(directory / "frozen.json"), str(directory / "frozen.json"))


def read_validating(directory: Path) -> BatchValidating:
    path = directory / "validating.json"
    return load_batch_validating(read_json(path), str(path))


def read_validated(directory: Path) -> BatchValidated:
    path = directory / "validated.json"
    return load_batch_validated(read_json(path), str(path))


def read_approved(directory: Path) -> BatchApproved:
    path = directory / "approved.json"
    return load_batch_approved(read_json(path), str(path))


def read_publishing(directory: Path) -> BatchPublishing:
    path = directory / "publishing.json"
    return load_batch_publishing(read_json(path), str(path))


def read_published(directory: Path) -> BatchPublished:
    path = directory / "published.json"
    return load_batch_published(read_json(path), str(path))


def read_failed(directory: Path) -> BatchFailed:
    path = directory / "failed.json"
    return load_batch_failed(read_json(path), str(path))


def read_rejected(directory: Path) -> BatchRejected:
    path = directory / "rejected.json"
    return load_batch_rejected(read_json(path), str(path))


def read_cancelled(directory: Path) -> BatchCancelled:
    path = directory / "cancelled.json"
    return load_batch_cancelled(read_json(path), str(path))


def read_ci_receipt(path: Path) -> CIReceipt:
    return load_ci_receipt(read_json(path), str(path))


def subject(hub: Path, bid: str) -> BatchSubject:
    directory = batch_path(hub, bid)
    validated = read_validated(directory)
    ci_path = Path(validated["ci_receipt"])
    return {
        "batch": bid,
        "validated_sha256": file_digest(directory / "validated.json"),
        "candidate_fingerprint_sha256": validated["candidate_fingerprint_sha256"],
        "ci_receipt_sha256": file_digest(ci_path),
        "membership_manifest_sha256": validated["membership_manifest_sha256"],
    }


def timing_from_log(  # noqa: PLR0911  # one early return per refused condition, each naming its reason
    text: str, log_sha256: str
) -> SingleSubmissionTiming | str:
    """The CK-36 timing evidence of one CI run, read from its own log (CK-38 1.8), or the reason it
    cannot be used. The watchdog and the steps come from the run's one header line, not from the
    accepted controller. The evidence must be unambiguous: every listed step has exactly one timing
    line (and no other step has one), every number is finite, and the log's last summary is the
    run's own passing full summary naming exactly the header's steps."""
    records = _protocol(text)
    if isinstance(records, str):
        return records
    header_rows, timing_rows, summaries = records
    headers = [row.groups() for row in header_rows]
    if len(headers) != 1:
        return (
            "the CI log has no recipe header line"
            if not headers
            else "the CI log has more than one recipe header line"
        )
    level, watchdog_text, listed = headers[0]
    watchdog = float(watchdog_text)
    steps = listed.split(",") if listed else []
    if level != "full" or not steps or "" in steps or len(set(steps)) != len(steps):
        return f"the CI log's recipe is `{level}` with steps [{listed}], not a full run of distinct, non-empty steps"
    if not math.isfinite(watchdog) or watchdog <= 0:
        return "the CI log's watchdog is not a finite positive number"
    lines = [(row.group(1), float(row.group(2))) for row in timing_rows]
    took: dict[str, float] = {}
    for name, seconds in lines:
        if name in took:
            return f"the CI log has more than one timing line for step {name}"
        took[name] = seconds
    missing = [name for name in steps if name not in took]
    if missing:
        return f"the CI log has no timing line for step {', '.join(missing)}"
    extra = [name for name in took if name not in steps]
    if extra:
        return f"the CI log has a timing line for step {', '.join(extra)}, which the header does not list"
    if not all(math.isfinite(seconds) and seconds >= 0 for seconds in took.values()):
        return "the CI log has a timing that is not a finite number"
    expected = f"check full: {', '.join(f'{name} ok' for name in steps)} => PASS"
    if not summaries or summaries[-1] != expected:
        return "the CI log does not end with the full run's passing summary of the header's steps"
    slowest = max(steps, key=lambda name: took[name])
    seconds = took[slowest]
    if seconds * WATCHDOG_MARGIN > watchdog:
        return (
            f"step {slowest} took {seconds:g} s, over a tenth of the {watchdog:g} s watchdog "
            f"(margin {watchdog / seconds:.2f}x, {WATCHDOG_MARGIN}x required)"
        )
    margin = watchdog / seconds if seconds else watchdog
    if not math.isfinite(margin):
        return f"the CI log's margin for step {slowest} is not a finite number"
    return {
        "log_sha256": log_sha256,
        "slowest_step": slowest,
        "slowest_seconds": seconds,
        "margin": margin,
    }


def _intent_basis(intent: BatchPublishing) -> str:
    """The basis a publication intent was written on: its pointer text names it."""
    written = json.loads(intent["pointer"])
    integration = written.get("integration") if isinstance(written, dict) else None
    basis = integration.get("basis") if isinstance(integration, dict) else None
    return basis if isinstance(basis, str) else INTEGRATION


class SinglePlan(TypedDict):
    """What the single-submission path decided on (CK-38): the sole member and its receipts."""

    submission: str
    submission_receipts: list[str]


class InfrastructureFailure(RuntimeError):
    """The batch check could not finish for a reason that is not the patches' fault (exit 75)."""


class _Rows:
    """Coverage rows by submission id, built on first use.

    Authors are recomputed from each capture's patch provenance, never read from the manifest,
    and that includes every incremental parent a review may be counted through."""

    def __init__(self, build: Callable[[str], ManifestInfo | None]) -> None:
        self._built: dict[str, ManifestInfo | None] = {}
        self._build = build

    def get(self, key: str) -> ManifestInfo | None:
        if key not in self._built:
            self._built[key] = self._build(key)
        return self._built[key]


class Manager:
    def __init__(
        self, hub: Path, validator: Validator | None = None, *, hook: Hook | None = None
    ) -> None:
        self.hub = hub
        self.validator = validator or self._full_check
        self.store = meta.Store(hub)
        self.hook = hook
        self._accepted_log: Path | None = None
        self._last_candidate: Path = hub

    def _point(self, name: str, **details: object) -> None:
        if self.hook is not None:
            self.hook(name, **details)

    @staticmethod
    def _inputs_sidecar(log: Path) -> Path:
        return log.with_suffix(".chat-inputs.json")

    def _full_check(self, candidate: Path, log: Path, fd: int) -> bool:
        env = os.environ.copy()
        env["CK_CI_LOCK_FD"] = str(fd)
        # The recipe's chat step records the shared inputs it tested beside this run's log.
        sidecar = self._inputs_sidecar(log)
        sidecar.unlink(missing_ok=True)
        env["CK_CHAT_INPUTS"] = str(sidecar)
        result = lease.run(
            [sys.executable, str(candidate / "tools/check.py")],
            check=False,
            cwd=candidate,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            pass_fds=(fd,),
        )
        log.write_text(result.stdout)
        if result.returncode == EX_TEMPFAIL:
            raise InfrastructureFailure(
                f"the batch check exited {EX_TEMPFAIL} (a step watchdog or another temporary "
                f"infrastructure failure); see {log}"
            )
        return result.returncode == 0

    def manifests(self) -> list[ManifestV2]:
        result: list[ManifestV2] = []
        for path in sorted((self.hub / ".pipeline/queue").glob("*.json")):
            raw = read_json(path)
            if isinstance(raw, dict) and raw.get("schema") == 2:  # noqa: PLR2004  # schema version 2
                manifest = load_manifest(raw, str(path))
                if is_manifest_v2(manifest):
                    result.append(manifest)
        return result

    def _rows(self, manifests: Sequence[ManifestV2]) -> _Rows:
        """Coverage rows for these manifests (and any incremental parent among them)."""
        by_id = {item["id"]: item for item in manifests}

        def build_row(identifier: str) -> ManifestInfo | None:
            item = by_id.get(identifier)
            if item is None:
                return None
            derived = reviews.derive(item)
            return {
                "hashes": item["hashes"],
                "subject": reviews.subject_for_submission(
                    self.hub / ".pipeline/queue" / f"{identifier}.json", item
                ),
                "authors": derived["authors"] if derived else [],
                "submitter": derived["submitter"] if derived else None,
                "unreadable": derived is None,
            }

        return _Rows(build_row)

    def eligible(self) -> list[ManifestV2]:
        state = pointer.read_pointer(self.hub)
        receipts = reviews.receipt_rows(self.store) if (self.store.repo / ".pijul").exists() else []
        accepted = set(state.get("submissions", []))
        manifests = self.manifests()
        manifest_rows = self._rows(manifests)
        quarantined = {path.stem for path in (self.hub / ".pipeline/quarantine").glob("*.json")}
        events = self.store.events() if (self.store.repo / ".pijul").exists() else []
        dead = (
            accepted
            | quarantined
            | {
                event["submission"]
                for event in events
                if event.get("kind") in ("withdrawn", "supersession") and "submission" in event
            }
        )
        result: list[ManifestV2] = []
        for manifest in manifests:
            if manifest["id"] in dead:
                continue
            row = manifest_rows.get(manifest["id"])
            if row is None or row.get("unreadable"):
                continue
            issues_ok = True
            for bound in manifest.get("issues", []):
                issue = self.store.show(bound["id"])
                if (
                    issue is None
                    or issue.get("revision") != bound.get("revision")
                    or issue.get("spec_sha256") != bound.get("spec_sha256")
                    or not meta.ready(self.store, issue)
                ):
                    issues_ok = False
                    break
            if not issues_ok:
                continue
            subject_value = reviews.subject_for_submission(
                self.hub / ".pipeline/queue" / f"{manifest['id']}.json", manifest
            )
            submitter = row.get("submitter")
            row_authors = row.get("authors", [])
            if not reviews.coverage(
                row_authors,
                receipts,
                subject_value,
                manifest_rows,
                submitters=[submitter] if submitter else [],
            ):
                continue
            if reviews.blocking(manifest, manifests, receipts, accepted, dead):
                continue
            eligible_row: ManifestV2 = {**manifest, "authors": row_authors}
            if submitter:
                eligible_row["submitter"] = submitter
            result.append(eligible_row)
        return result

    def _groups(self, manifests: list[ManifestV2]) -> list[list[ManifestV2]]:
        groups: list[list[ManifestV2]] = []
        for manifest in manifests:
            hashes = set(manifest["hashes"])
            touching = [
                group for group in groups if hashes & {h for item in group for h in item["hashes"]}
            ]
            if touching:
                merged = [manifest]
                for group in touching:
                    merged.extend(group)
                    groups.remove(group)
                groups.append(merged)
            else:
                groups.append([manifest])
        return groups

    def prepare(self, no_split: bool = False) -> str:  # noqa: PLR0915  # linear workflow kept whole so failure ordering stays visible
        pipeline = pipeline_api.load()
        with locks.ci(self.hub) as ci_fd:
            with locks.state(self.hub), locks.meta(self.store.repo):
                for old in (self.hub / ".pipeline/batches").glob("*/"):
                    if (old / "frozen.json").exists() and not any(
                        (old / f"{name}.json").exists()
                        for name in ("validated", "failed", "rejected", "cancelled", "published")
                    ):
                        _write(old / "failed.json", {"reason": "interrupted", "at": time.time_ns()})
                        docsqueue.Queue(self.hub).flush_locked()
                if any(
                    not any(
                        (path / f"{state}.json").exists()
                        for state in ("published", "failed", "rejected", "cancelled")
                    )
                    for path in (self.hub / ".pipeline/batches").glob("*/")
                ):
                    raise RuntimeError("A batch is already active")
                docsqueue.Queue(self.hub).flush_locked()
                eligible = self.eligible()
                if not eligible:
                    raise RuntimeError("No eligible submissions")
                state = pointer.read_pointer(self.hub)
                bid = str(time.time_ns())
                directory = batch_path(self.hub, bid)
                base_digest = file_digest(Path(state["_pointer_path"]))
                snapshot = {
                    path.name: file_digest(path)
                    for path in sorted((self.hub / ".pipeline/queue").glob("*.json"))
                }
                frozen: BatchFrozen = {
                    "batch": bid,
                    "base_pointer_sha256": base_digest,
                    "members": [
                        {
                            "id": item["id"],
                            "manifest_sha256": file_digest(
                                self.hub / ".pipeline/queue" / f"{item['id']}.json"
                            ),
                            "fingerprint_sha256": hashlib.sha256(
                                canonical(item["fingerprint"])
                            ).hexdigest(),
                        }
                        for item in eligible
                    ],
                    "queue_snapshot": snapshot,
                    "authors": [author for item in eligible for author in item.get("authors", [])],
                    "owner": {"pid": os.getpid(), "pid_start": time.time_ns()},
                }
                _write(directory / "frozen.json", frozen)
                locks.annotate(ci_fd, batch=bid)  # a busy ci.lock then names this batch
                self._point("after_frozen", bid=bid, directory=directory)
            with locks.state(self.hub):
                _write(directory / "validating.json", {"at": time.time_ns()})
            self._point("after_validating", bid=bid, directory=directory)
            base = Path(state["repo"])
            try:
                passing, failures, log_paths = self._validate_groups(
                    bid, base, eligible, no_split, ci_fd
                )
            except InfrastructureFailure as error:
                # Not a patch failure: no split, no quarantine, no receipt; every member stays
                # eligible for the next prepare and queued docs drain as for any failure.
                with locks.state(self.hub):
                    _write(
                        directory / "failed.json",
                        {
                            "reason": "infrastructure",
                            "exit": EX_TEMPFAIL,
                            "detail": str(error),
                            "at": time.time_ns(),
                        },
                    )
                    docsqueue.Queue(self.hub).flush_locked()
                raise RuntimeError(
                    f"Batch {bid} hit an infrastructure failure: {error}. No member was "
                    "quarantined or split; run `ck batch prepare` again"
                ) from error
            accepted_candidate = (
                self.hub / ".pipeline/checkpoints" / f"{bid}-combined"
                if len(passing) == len(eligible)
                else self.hub / ".pipeline/checkpoints" / f"{bid}-split-final"
            )
            if len(passing) != len(eligible):
                accepted_candidate = Path(self._last_candidate)
            receipts_dir = self.hub / ".pipeline/ci"
            receipts_dir.mkdir(parents=True, exist_ok=True)
            ci_receipt = receipts_dir / f"{bid}.json"
            # Only the accepted candidate's own run is bound; a stale or other group's is ignored.
            chat_inputs = self._chat_inputs(self._accepted_log) if passing else None
            unrecorded = bool(passing) and chat_inputs is None
            receipt: BatchCIReceipt = {
                "kind": "ci",
                "passed": bool(passing) and not unrecorded,
                "fingerprint": pipeline.fingerprint(accepted_candidate),
                "submissions": [item["id"] for item in passing],
                "failed": [item["id"] for item in failures],
                "logs": log_paths,
                "recorded": time.time_ns(),
            }
            if chat_inputs is not None:
                receipt["chat_inputs"] = chat_inputs
            if unrecorded:
                receipt["reason"] = "chat inputs not recorded"
            _write(ci_receipt, receipt)
            with locks.state(self.hub):
                quarantine = self.hub / ".pipeline/quarantine"
                quarantine.mkdir(parents=True, exist_ok=True)
                for item in failures:
                    _write(
                        quarantine / f"{item['id']}.json",
                        {"submission": item["id"], "batch": bid, "reason": "validation failed"},
                    )
                if not passing:
                    _write(
                        directory / "failed.json",
                        {"reason": "validation failed", "failed": receipt["failed"]},
                    )
                    docsqueue.Queue(self.hub).flush_locked()
                    raise RuntimeError("No submission group passed full CI")
                if unrecorded:
                    # A recording failure, not a patch failure: the passing members stay eligible.
                    _write(
                        directory / "failed.json",
                        {"reason": "chat inputs not recorded", "failed": receipt["failed"]},
                    )
                    docsqueue.Queue(self.hub).flush_locked()
                    raise RuntimeError(
                        "Batch CI passed but its chat inputs were not recorded; "
                        "no member was quarantined for it"
                    )
                validated_record: dict[str, object] = {
                    "candidate": str(accepted_candidate),
                    "candidate_fingerprint_sha256": hashlib.sha256(
                        canonical(receipt["fingerprint"])
                    ).hexdigest(),
                    "ci_receipt": str(ci_receipt),
                    "ci_receipt_sha256": file_digest(ci_receipt),
                    "members": [item["id"] for item in passing],
                    "membership_manifest_sha256": [
                        file_digest(self.hub / ".pipeline/queue" / f"{item['id']}.json")
                        for item in passing
                    ],
                    "base_pointer_sha256": base_digest,
                    "authors": [author for item in passing for author in item.get("authors", [])],
                    "submitters": [item["submitter"] for item in passing if item.get("submitter")],
                }
                if self._accepted_log is not None and self._accepted_log.is_file():
                    # The run's own log, bound here so the single-submission path can trust its
                    # timing lines later (CK-38 1.8).
                    validated_record["ci_log_sha256"] = file_digest(self._accepted_log)
                _write(directory / "validated.json", validated_record)
            self._point("after_validated", bid=bid, directory=directory)
            print(bid)
            return bid

    def _chat_inputs(self, log: Path | None) -> ChatInputs | None:
        """The validated record the accepted candidate's own run left beside its log, or None."""
        if log is None:
            return None
        sidecar = self._inputs_sidecar(log)
        try:
            record = load_chat_inputs(read_json(sidecar), str(sidecar))
        except RecordError:
            return None  # unreadable, or not the shape of a record: the run did not record it
        return record if _inputs().valid_record(record) else None

    def _require_bound(self, ci_receipt: CIReceipt) -> None:
        """Before a publication intent exists: the receipt binds the shared inputs, and they
        are still the ones it tested. Completing an existing intent never calls this."""
        inputs = _inputs()
        record = ci_receipt.get("chat_inputs") if is_batch_ci_receipt(ci_receipt) else None
        if record is None or not inputs.valid_record(record):
            raise RuntimeError("Batch CI receipt does not bind shared chat inputs; prepare again")
        try:
            live = inputs.aggregate(inputs.shared_inputs(inputs.shared_root()))
        except inputs.InputError as error:
            raise RuntimeError(str(error)) from error
        if live != record["aggregate"]:
            raise RuntimeError(
                "Shared chat inputs changed after batch validation; cancel and prepare again"
            )

    def _validate_groups(
        self, bid: str, base: Path, eligible: list[ManifestV2], no_split: bool, ci_fd: int
    ) -> tuple[list[ManifestV2], list[ManifestV2], list[str]]:
        pipeline = pipeline_api.load()
        combined = self.hub / ".pipeline/checkpoints" / f"{bid}-combined"
        pipeline.clone(base, combined, self.hub)
        conflicts = []
        for manifest in eligible:
            conflicts.extend(pipeline.pull(combined, Path(manifest["repo"]), manifest["hashes"]))
        log = self.hub / ".pipeline/checkpoints" / f"{bid}-combined.log"
        logs = [str(log)]
        self._accepted_log = None
        valid = not conflicts and self.validator(combined, log, ci_fd)
        try:
            pipeline.clean(combined)
        except RuntimeError:
            valid = False
        if valid:
            self._last_candidate = combined
            self._accepted_log = log
            return eligible, [], logs
        if no_split:
            self._last_candidate = base
            return [], eligible, logs
        accepted_candidate = base
        passing, failures = [], []
        for index, group in enumerate(self._groups(eligible)):
            candidate = self.hub / ".pipeline/checkpoints" / f"{bid}-split-{index}"
            pipeline.clone(accepted_candidate, candidate, self.hub)
            group_conflicts = []
            for manifest in group:
                group_conflicts.extend(
                    pipeline.pull(candidate, Path(manifest["repo"]), manifest["hashes"])
                )
            group_log = self.hub / ".pipeline/checkpoints" / f"{bid}-split-{index}.log"
            logs.append(str(group_log))
            group_valid = not group_conflicts and self.validator(candidate, group_log, ci_fd)
            try:
                pipeline.clean(candidate)
            except RuntimeError:
                group_valid = False
            if group_valid:
                accepted_candidate = candidate
                self._accepted_log = group_log
                passing.extend(group)
            else:
                failures.extend(group)
        self._last_candidate = accepted_candidate
        return passing, failures, logs

    def _single_plan(  # noqa: PLR0911, PLR0912  # one early return per refused condition, in the spec's order
        self,
        bid: str,
        validated: BatchValidated,
        state: PointerV2 | PointerSchema1,
        receipts: Sequence[ReviewRow],
        *,
        recovering: bool,
    ) -> SinglePlan | str:
        """CK-38 behaviour 1.1-1.5 and 1.7, from the records on disk: the plan for publishing this
        batch on its sole member's submission review alone, or the reason it cannot be. Reads only;
        the caller holds the locks. `recovering` (the pointer is already written) skips the one
        check that needs the base repository, the candidate's patch list over it."""
        directory = batch_path(self.hub, bid)
        frozen = read_frozen(directory)
        if len(frozen["members"]) != 1 or len(validated["members"]) != 1:
            return "the batch does not have exactly one member"
        member = frozen["members"][0]
        if validated["members"] != [member["id"]]:
            return "the validated member is not the frozen member"
        if member["fingerprint_sha256"] != validated["candidate_fingerprint_sha256"]:
            return (
                "the candidate differs from the member's capture (the base advanced before prepare)"
            )
        for name in ("rejected", "cancelled", "failed"):
            if (directory / f"{name}.json").exists():
                return f"the batch has {name}.json"
        batch_subject = subject(self.hub, bid)
        for row in receipts:
            if row.get("subject") == batch_subject and row.get("verdict") != "approved":
                return f"batch review receipt {row['id']} is `{row.get('verdict')}`, not approved"
        path = self.hub / ".pipeline/queue" / f"{member['id']}.json"
        manifest = load_manifest(read_json(path), str(path))
        if not is_manifest_v2(manifest):
            return "the member is a legacy submission"
        if (
            manifest["base"]["pointer_sha256"] != frozen["base_pointer_sha256"]
            or validated["base_pointer_sha256"] != frozen["base_pointer_sha256"]
        ):
            return "the submission was made on a different accepted base than the batch's"
        if not recovering:
            pipeline = pipeline_api.load()
            added = pipeline.hashes(Path(validated["candidate"])) - pipeline.hashes(
                Path(state["repo"])
            )
            if added != set(manifest["hashes"]):
                return "the candidate does not add exactly the member's patches over the base"
        derived = reviews.derive(manifest)
        if derived is None:
            return "the member's provenance cannot be read"
        submitters = [derived["submitter"]] if derived["submitter"] else []
        member_subject = reviews.subject_for_submission(path, manifest)
        if not reviews.coverage(
            derived["authors"],
            receipts,
            member_subject,
            self._rows(self.manifests()),
            submitters=submitters,
            owner=False,
        ):
            return (
                "the member's submission review is not satisfied by AI review receipts alone "
                "(an owner receipt covers only its exact subject and is not reused here)"
            )
        covering = [
            row["id"]
            for row in receipts
            if row.get("verdict") == "approved"
            and row.get("subject") == member_subject
            and row.get("revoked") is not True
            and not reviews.is_owner_receipt(row)
            and row["reviewer"].get("role") == "reviewer"
            and row["reviewer"].get("class") == "A"
            and row["reviewer"].get("effort") == "high"
            and row["reviewer"].get("family") in ("claude", "codex")
            and reviews.independent(row["reviewer"], derived["authors"], submitters)
        ]
        return {"submission": member["id"], "submission_receipts": covering}

    def _single_timing(
        self, validated: BatchValidated, ci_receipt: CIReceipt
    ) -> SingleSubmissionTiming | str:
        """CK-38 behaviour 1.8: the run's own log, bound at prepare, shows every listed step timed
        and the slowest within the CK-36 margin of the watchdog the run itself printed."""
        digest = validated.get("ci_log_sha256")
        if digest is None:
            return "the batch was prepared without a bound CI log digest"
        logs = ci_receipt["logs"] if is_batch_ci_receipt(ci_receipt) else []
        if len(logs) != 1:
            return "the CI receipt does not name exactly one log"
        log = Path(logs[0])
        if not log.is_file():
            return "the CI log is missing"
        if file_digest(log) != digest:
            return "the CI log changed after prepare"
        return timing_from_log(log.read_text(errors="replace"), digest)

    def _refusal(self, bid: str, reason: str) -> RuntimeError:
        return RuntimeError(
            f"Batch requires fresh integration review before publication ({reason}); "
            f"request an integration review: `ck review batch {bid} …`"
        )

    def _publication_basis(self, bid: str, directory: Path) -> dict[str, object]:
        """The basis `publish` would use now, for `status`: advisory and read-only; `publish`
        always decides again under its locks."""
        if (directory / "published.json").exists():
            return {"publication_basis": read_published(directory).get("basis", INTEGRATION)}
        if (directory / "approved.json").exists():
            return {"publication_basis": INTEGRATION}
        try:
            validated = read_validated(directory)
            state = pointer.read_pointer(self.hub)
            receipts = reviews.receipt_rows(self.store)
            plan = self._single_plan(bid, validated, state, receipts, recovering=False)
            if isinstance(plan, str):
                return {"publication_basis": INTEGRATION, "publication_basis_reason": plan}
            ci_receipt = read_ci_receipt(Path(validated["ci_receipt"]))
            timing = self._single_timing(validated, ci_receipt)
        except (RuntimeError, RecordError, OSError) as error:
            return {"publication_basis": INTEGRATION, "publication_basis_reason": str(error)}
        if isinstance(timing, str):
            return {"publication_basis": INTEGRATION, "publication_basis_reason": timing}
        return {"publication_basis": SINGLE}

    def status(self, bid: str) -> dict[str, object]:
        """Every record of the batch by name, plus owner sign-offs and collected paths, for display."""
        directory = batch_path(self.hub, bid)
        result: dict[str, object] = {}
        for path in sorted(directory.glob("*.json")):
            result[path.stem] = self._record(path)
        validated = self._validated_for_status(directory)
        if validated and (self.store.repo / ".pijul").exists():
            # Owner-signed subjects (the batch itself and its members), labelled next to the receipt
            # ids so they cannot be mistaken for AI verdicts.
            rows = reviews.receipt_rows(self.store)
            subjects = {json.dumps(subject(self.hub, bid), sort_keys=True): "batch"}
            for member in validated["members"]:
                path = self.hub / ".pipeline/queue" / f"{member}.json"
                if path.exists():
                    manifest = load_manifest(read_json(path), str(path))
                    key = json.dumps(reviews.subject_for_submission(path, manifest), sort_keys=True)
                    subjects[key] = f"submission {member}"
            signed = [
                {"receipt": row["id"], "subject": subjects[key], "owner_signoff": True}
                for row in rows
                if reviews.is_owner_receipt(row)
                and (key := json.dumps(row.get("subject"), sort_keys=True)) in subjects
            ]
            if signed:
                result["owner_signoff"] = signed
        # A receipt or record whose candidate was collected by `ck gc` stays valid evidence of what
        # was validated; its missing path is shown as "collected" rather than left to look broken.
        gone = {
            value: "collected"
            for record in result.values()
            if isinstance(record, dict)
            for key in ("candidate", "repo")
            if isinstance(value := record.get(key), str) and not Path(value).exists()
        }
        if gone:
            result["collected"] = dict(sorted(gone.items()))
        if validated is not None and not any(
            (directory / f"{name}.json").exists() for name in ("rejected", "cancelled", "failed")
        ):
            result.update(self._publication_basis(bid, directory))
        return result

    @staticmethod
    def _record(path: Path) -> object:
        """One record file of a batch directory, validated by its kind (an unknown name is only
        required to be a JSON object, as before)."""
        loaders: dict[str, Callable[[object, str], object]] = {
            "frozen": load_batch_frozen,
            "validating": load_batch_validating,
            "validated": load_batch_validated,
            "approved": load_batch_approved,
            "publishing": load_batch_publishing,
            "published": load_batch_published,
            "failed": load_batch_failed,
            "rejected": load_batch_rejected,
            "cancelled": load_batch_cancelled,
        }
        raw = read_json(path)
        load = loaders.get(path.stem)
        if load is not None:
            return load(raw, str(path))
        if not isinstance(raw, dict):
            raise RecordError(f"{path}: expected a JSON object; inspect the file")
        return raw

    @staticmethod
    def _validated_for_status(directory: Path) -> BatchValidated | None:
        path = directory / "validated.json"
        return read_validated(directory) if path.exists() else None

    def cancel(self, bid: str, reason: str) -> None:
        directory = batch_path(self.hub, bid)
        if (directory / "publishing.json").exists():
            raise RuntimeError("Cannot cancel after publication intent; rerun publish recovery")
        if (directory / "validating.json").exists() and not (directory / "validated.json").exists():
            raise RuntimeError("Cannot cancel a validating batch")
        with locks.ci(self.hub), locks.state(self.hub):
            if (directory / "validating.json").exists() and not (
                directory / "validated.json"
            ).exists():
                raise RuntimeError("Cannot cancel a validating batch")
            _write(directory / "cancelled.json", {"reason": reason, "at": time.time_ns()})
            docsqueue.Queue(self.hub).flush_locked()

    def approve(self, bid: str) -> None:
        directory = batch_path(self.hub, bid)
        with locks.state(self.hub), locks.meta(self.store.repo):
            validated = read_validated(directory)
            all_receipts = reviews.receipt_rows(self.store)
            subject_value = subject(self.hub, bid)
            approvals = [
                row
                for row in all_receipts
                if row.get("kind") == "batch" and row.get("subject") == subject_value
            ]
            if not reviews.coverage(
                validated["authors"],
                approvals,
                subject_value,
                submitters=validated.get("submitters", []),
            ):
                raise RuntimeError("Batch does not have fresh independent integration approval")
            approved: BatchApproved = {
                "subject": subject_value,
                "receipts": [r["id"] for r in approvals],
                **owner_labels(approvals),
            }
            _write(directory / "approved.json", approved)

    def reject(self, bid: str, blame: list[str]) -> None:
        directory = batch_path(self.hub, bid)
        with locks.state(self.hub), locks.meta(self.store.repo):
            validated = read_validated(directory)
            if not set(blame) <= set(validated["members"]):
                raise RuntimeError("Blame must name only members of this batch")
            rejected: BatchRejected = {
                "blame": blame,
                "members": validated["members"],
                "at": time.time_ns(),
            }
            _write(directory / "rejected.json", rejected)
            quarantine = self.hub / ".pipeline/quarantine"
            quarantine.mkdir(parents=True, exist_ok=True)
            for identifier in blame:
                _write(
                    quarantine / f"{identifier}.json",
                    {"submission": identifier, "batch": bid, "reason": "integration review blame"},
                )
            docsqueue.Queue(self.hub).flush_locked()

    def _integration(  # noqa: PLR0913  # explicit arguments; the publication's facts are all distinct
        self,
        bid: str,
        plan: SinglePlan | None,
        validated: BatchValidated,
        ci_receipt: CIReceipt,
        receipts: Sequence[ReviewRow],
        recovering: bool,
    ) -> Integration:
        """The `integration` the new pointer records: the batch-subject receipts, or the
        single-submission basis with its submission receipts and CI timing evidence."""
        if plan is None:
            return {
                "batch": bid,
                "receipts": [
                    r["id"] for r in receipts if r.get("subject") == subject(self.hub, bid)
                ],
            }
        if recovering:  # the evidence is in the intent already; recovery keeps its exact bytes
            timing: SingleSubmissionTiming | str = {
                "log_sha256": "",
                "slowest_step": "",
                "slowest_seconds": 0.0,
                "margin": 0.0,
            }
        else:
            timing = self._single_timing(validated, ci_receipt)
        if isinstance(timing, str):
            raise self._refusal(bid, timing)
        return {
            "batch": bid,
            "basis": SINGLE,
            "submission": plan["submission"],
            "submission_receipts": plan["submission_receipts"],
            "receipts": [],
            "timing": timing,
        }

    def publish(self, bid: str) -> None:  # noqa: PLR0912, PLR0915  # linear workflow kept whole so failure ordering stays visible
        pipeline = pipeline_api.load()
        directory = batch_path(self.hub, bid)
        with locks.state(self.hub), locks.meta(self.store.repo):
            validated = read_validated(directory)
            state = pointer.read_pointer(self.hub)
            intent_path = directory / "publishing.json"
            intent = read_publishing(directory) if intent_path.exists() else None
            # The basis is decided once and is part of the intent's pointer bytes: an interrupted
            # publication is finished on the basis it started with, never switched (CK-38 7).
            single_intent = intent is not None and _intent_basis(intent) == SINGLE
            approved = (directory / "approved.json").exists()
            if not approved and intent is not None and not single_intent:
                raise RuntimeError("Batch requires fresh integration review before publication")
            current_bytes = Path(state["_pointer_path"]).read_bytes()
            recovering = bool(
                intent
                and docsqueue.is_docs_descendant(
                    self.hub, current_bytes, intent["pointer"].encode()
                )
            )
            if (
                not recovering
                and file_digest(Path(state["_pointer_path"])) != validated["base_pointer_sha256"]
            ):
                raise RuntimeError("Accepted base advanced after batch validation")
            candidate = Path(validated["candidate"])
            pipeline.clean(candidate)
            if (
                hashlib.sha256(canonical(pipeline.fingerprint(candidate))).hexdigest()
                != validated["candidate_fingerprint_sha256"]
            ):
                raise RuntimeError("Validated candidate fingerprint changed")
            receipts = reviews.receipt_rows(self.store)
            # The single-submission path replaces the batch-subject review by the sole member's
            # submission review (CK-38). An existing approved.json is honoured on any batch.
            plan: SinglePlan | None = None
            if single_intent or (not approved and intent is None):
                decided = self._single_plan(bid, validated, state, receipts, recovering=recovering)
                if isinstance(decided, str):
                    raise self._refusal(bid, decided)
                plan = decided
            elif not reviews.coverage(
                validated["authors"],
                receipts,
                subject(self.hub, bid),
                submitters=validated.get("submitters", []),
            ):
                raise RuntimeError("Batch lacks fresh independent integration review coverage")
            if [
                file_digest(self.hub / ".pipeline/queue" / f"{item}.json")
                for item in validated["members"]
            ] != validated["membership_manifest_sha256"]:
                raise RuntimeError("Batch membership manifest changed")
            ci_receipt = read_ci_receipt(Path(validated["ci_receipt"]))
            if not ci_receipt.get("passed"):
                raise RuntimeError("Batch CI receipt did not pass")
            if file_digest(Path(validated["ci_receipt"])) != validated["ci_receipt_sha256"]:
                raise RuntimeError("Batch CI receipt changed")
            if not recovering:
                current_eligible = {item["id"] for item in self.eligible()}
                if not set(validated["members"]) <= current_eligible:
                    raise RuntimeError("A batch member is no longer eligible")
            integration = self._integration(bid, plan, validated, ci_receipt, receipts, recovering)
            new_pointer: PointerV2 = {
                "schema": 2,
                "repo": str(candidate),
                "fingerprint": pipeline.fingerprint(candidate),
                "submissions": [*state.get("submissions", []), *validated["members"]],
                "receipt": validated["ci_receipt"],
                "previous": state.get("receipt"),
                "integration": integration,
            }
            raw = intent["pointer"].encode() if recovering and intent else canonical(new_pointer)
            if intent and intent.get("pointer") != raw.decode():
                raise RuntimeError("Publication intent does not match the validated result")
            if not intent:
                self._require_bound(ci_receipt)
                self._point("before_publish_intent", bid=bid, directory=directory)
                _write(
                    intent_path,
                    {"pointer": raw.decode(), "sha256": hashlib.sha256(raw).hexdigest()},
                )
                self._point("after_publish_intent", bid=bid, directory=directory)
            if not recovering:
                pointer.write_accepted(Path(state["_pointer_path"]), raw)
                self._point("after_pointer_write", bid=bid, directory=directory)
            if not (directory / "published.json").exists():
                _write(
                    directory / "published.json",
                    {
                        "pointer_sha256": hashlib.sha256(raw).hexdigest(),
                        "basis": SINGLE if plan is not None else INTEGRATION,
                        **owner_labels(
                            [r for r in receipts if r.get("subject") == subject(self.hub, bid)]
                        ),
                    },
                )
            docsqueue.Queue(self.hub).flush_locked()
