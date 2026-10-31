"""Immutable patch provenance for gate-managed Pijul records."""

from __future__ import annotations

import hashlib
import json
import os
from datetime import UTC, datetime
from pathlib import Path

from . import classes, lease, pipeline_api, sessions
from .pointer import canonical
from .records import (
    Author,
    ProvenancePayload,
    RecordError,
    UnknownProvenance,
    is_agent,
    load_provenance,
    patch_descriptions,
)

PREFIX = "ck-provenance/1 sha256="


def payload_digest(payload: object) -> str:
    return hashlib.sha256(canonical(payload)).hexdigest()


def issue_ids(value: str) -> list[str]:
    result = sorted({part.strip().upper() for part in value.split(",") if part.strip()})
    if not result or any(not item.startswith("CK-") for item in result):
        raise RuntimeError("At least one valid CK issue ID is required")
    return result


def make_payload(issues: list[str], role: str | None = None) -> ProvenancePayload:
    session = sessions.resolve()
    if not is_agent(session) and not role:
        raise RuntimeError("ck record requires verifiable agent session evidence")
    declared = sessions.effective_role(session, role)
    if declared is None or not is_agent(session):
        raise RuntimeError("ck record requires verifiable agent session evidence")
    mapping = classes.readiness(
        declared, session["client"], session["models"], session.get("current")
    )
    return {
        "schema": "ck-provenance/1",
        "issues": issues,
        "identity": session["identity"],
        "client": session["client"],
        "session": session["session"],
        "role": declared,
        "families": mapping.get("families", [mapping.get("family", "unknown")]),
        "mapping": {k: v for k, v in mapping.items() if k != "families"},
        "models": sessions.compress_history(session["models"]),
        "evidence": session["evidence"],
        "recorded": datetime.now(UTC).isoformat(),
    }


def record(  # noqa: PLR0913  # explicit arguments; a parameter object would obscure the call sites
    root: Path, hub: Path, message: str, issues: str, paths: list[str], role: str | None = None
) -> ProvenancePayload:
    payload = make_payload(issue_ids(issues), role)
    digest = payload_digest(payload)
    description = PREFIX + digest + "\n" + canonical(payload).decode()
    author = payload["identity"]
    lease.run(
        [
            "pijul",
            "record",
            "-a",
            *paths,
            "-m",
            message,
            "--author",
            author,
            "--description",
            description,
        ],
        cwd=root,
        check=True,
        env={**os.environ, "PIJUL_CONFIG_DIR": str(hub / ".tools/pijul-config")},
    )
    return payload


def parse_description(description: str) -> dict[str, object] | UnknownProvenance | None:
    """The digest-checked payload of a description (shape not yet validated), `unknown` when the
    digest or schema fails, None when it is not a gate record. Validation is `load_provenance_or_unknown`."""
    if not description.startswith(PREFIX):
        return None
    digest, _, raw = description[len(PREFIX) :].partition("\n")
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError:
        return {"schema": "unknown", "digest": digest}
    if (
        not isinstance(payload, dict)
        or payload_digest(payload) != digest
        or payload.get("schema") != "ck-provenance/1"
    ):
        return {"schema": "unknown", "digest": digest}
    return payload


FAMILIES = ("claude", "codex")


def load_provenance_or_unknown(
    payload: object, digest: str
) -> ProvenancePayload | UnknownProvenance:
    """A payload whose digest already matched, but whose shape can still be wrong.

    Such a payload is treated as unknown: it never narrows coverage and never raises."""
    try:
        valid = load_provenance(payload, "patch provenance")
    except RecordError:
        return {"schema": "unknown", "digest": digest}
    if not all(item in FAMILIES for item in valid["families"]):
        return {"schema": "unknown", "digest": digest}
    return valid


def from_capture(repo: Path, hashes: list[str]) -> dict[str, ProvenancePayload | None]:
    """{hash: valid payload | None} read from an immutable capture's own patch descriptions.

    A missing description, another schema, a digest mismatch or a malformed payload is None. A
    requested hash the capture does not contain is an error: it does not match its manifest."""
    pipeline = pipeline_api.load()
    described = patch_descriptions(
        json.loads(pipeline.pijul(repo, "log", "--description", "--output-format", "json")),
        f"{repo}: pijul log",
    )
    result: dict[str, ProvenancePayload | None] = {}
    for patch in hashes:
        if patch not in described:
            raise RuntimeError(f"Capture does not contain patch {patch}")
        payload = parse_description(described[patch])
        checked = None if payload is None else load_provenance_or_unknown(payload, "")
        result[patch] = None if checked is None or checked["schema"] == "unknown" else checked
    return result


class _Entry:
    """One author while their patches are being merged."""

    def __init__(self, payload: ProvenancePayload | None) -> None:
        self.payload = payload
        self.families: set[str] = set()
        self.hashes: set[str] = set()

    def author(self) -> Author:
        """The merged entry, its keys in the order the stored manifests have always had."""
        families = sorted(self.families)
        family = families[0] if len(families) == 1 else "unknown"
        payload = self.payload
        if payload is None:
            return {
                "identity": "unknown",
                "families": families,
                "hashes": sorted(self.hashes),
                "family": family,
            }
        return {
            "identity": payload["identity"],
            "client": payload["client"],
            "session": payload["session"],
            "role": payload.get("role"),
            "families": families,
            "hashes": sorted(self.hashes),
            "family": family,
        }


def authors(payloads: dict[str, ProvenancePayload | None]) -> list[Author]:
    """The authors a set of patches proves: one entry per (identity, session), unknown for any
    patch without valid provenance. Entries keep the union of their hashes and families, and a
    union of more than one family is itself `unknown`, so coverage needs both families."""
    merged: dict[tuple[str, str], _Entry] = {}
    for patch, payload in sorted(payloads.items()):
        if payload is None:
            key, families = ("unknown", ""), ["unknown"]
        else:
            key, families = (payload["identity"], payload["session"]), list(payload["families"])
        entry = merged.setdefault(key, _Entry(payload))
        entry.families.update(families)
        entry.hashes.add(patch)
    return [entry.author() for entry in merged.values()]
