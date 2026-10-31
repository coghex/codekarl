"""S7a: the typed record loaders accept every valid on-disk form and reject the rest by name.

Each record kind and variant has a hermetic fixture here; the report adds a read-only run over the
real hub. A loader returns the parsed object itself, so unknown additive fields survive.
"""

from __future__ import annotations

import copy
import json
import sys
import tempfile
import tomllib
import typing
import unittest
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any, ClassVar, cast

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from gate import pointer, records, sessions
from gate.records import JsonObject, RecordError
from shapes import conforms, overlay, sample

SHA = "0" * 64
FINGERPRINT: JsonObject = {
    "hashes": ["AAAA", "BBBB"],
    "files": {"a.txt": {"sha256": SHA, "executable": False}},
}
AUTHOR: JsonObject = {
    "identity": "ck-worker-2",
    "client": "claude",
    "session": "4864d6a5",
    "role": "worker",
    "family": "claude",
    "families": ["claude"],
    "hashes": ["AAAA"],
}
ISSUE: JsonObject = {"id": "CK-26", "revision": "r2", "spec_sha256": SHA}


def pointer_v2() -> JsonObject:
    return {
        "schema": 2,
        "repo": "/hub/.pipeline/checkpoints/x",
        "fingerprint": copy.deepcopy(FINGERPRINT),
        "submissions": ["s1", "s2"],
        "receipt": "/hub/.pipeline/ci/x.json",
        "previous": None,
    }


def single_integration() -> JsonObject:
    """The `integration` a single-submission publication records (CK-38)."""
    return {
        "batch": "b1",
        "basis": "single-submission",
        "submission": "s1",
        "submission_receipts": ["r1", "r2"],
        "receipts": [],
        "timing": {
            "log_sha256": SHA,
            "slowest_step": "unittest-check",
            "slowest_seconds": 96.0,
            "margin": 28.125,
        },
    }


def accepted_pointer() -> records.AcceptedPointer:
    record = records.load_pointer(pointer_v2(), "fixture")
    assert pointer.is_accepted(record)
    return record


def manifest_v2() -> JsonObject:
    return {
        "schema": 2,
        "id": "1792174130567425000",
        "repo": "/hub/.pipeline/submissions/1792174130567425000",
        "spec": "CK-32@r1 fix 1",
        "hashes": ["AAAA"],
        "fingerprint": copy.deepcopy(FINGERPRINT),
        "files": ["AGENTS.md"],
        "issues": [copy.deepcopy(ISSUE)],
        "provenance": {"AAAA": SHA, "BBBB": None},
        "authors": [copy.deepcopy(AUTHOR)],
        "base": {"fingerprint_sha256": SHA, "pointer_sha256": SHA},
        "supersedes": [],
    }


def receipt(kind: str = "spec") -> JsonObject:
    subjects: dict[str, JsonObject] = {
        "spec": {"issue": "CK-26", "revision": "r2", "spec_sha256": SHA},
        "submission": {"submission": "s1", "manifest_sha256": SHA, "fingerprint_sha256": SHA},
        "batch": {
            "batch": "b1",
            "candidate_fingerprint_sha256": SHA,
            "ci_receipt_sha256": SHA,
            "membership_manifest_sha256": [SHA],
            "validated_sha256": SHA,
        },
    }
    return {
        "schema": "ck-review/1",
        "id": "ff393d2e77ed7764b922",
        "kind": kind,
        "verdict": "approved",
        "recorded": 1792113325877099000,
        "report_sha256": SHA,
        "issues": [copy.deepcopy(ISSUE)],
        "reviewer": {
            "identity": "ck-reviewer-1",
            "family": "codex",
            "role": "reviewer",
            "client": "codex",
            "session": "01a13b66",
            "model": "gpt-6.1-sol",
            "effort": "high",
            "class": "A",
            "evidence": {"path": "/x/session.jsonl", "offset": 7, "sha256": SHA},
        },
        "subject": subjects[kind],
        "base_examined": "/hub/.pipeline/checkpoints/x",
        "incremental_over": None,
    }


def provenance_payload() -> JsonObject:
    return {
        "schema": "ck-provenance/1",
        "issues": ["CK-26"],
        "identity": "ck-worker-2",
        "client": "claude",
        "session": "4864d6a5",
        "role": "worker",
        "families": ["claude"],
        "mapping": {"family": "claude", "model": "claude-sonnet-5-5"},
        "models": [{"model": "claude-sonnet-5-5", "effort": "high"}],
        "evidence": {"path": "/x", "offset": 1, "sha256": SHA},
        "recorded": "2026-10-16T10:00:00+00:00",
    }


def issue_event(kind: str = "created") -> JsonObject:
    fields: dict[str, JsonObject] = {
        "created": {"title": "T", "type": "task", "priority": "P2", "state": "open", "depends": []},
        "edit": {"depends": ["CK-1"]},
        "spec": {"revision": "r1"},
        "close": {"state": "closed", "reason": "done"},
    }
    event: JsonObject = {
        "id": "CK-26",
        "kind": kind,
        "at": 1792117962835760000,
        "fields": fields[kind],
        "authors": [{"identity": "human", "family": "human", "role": "owner"}],
    }
    if kind in ("created", "spec"):
        event["spec_sha256"] = SHA
    return event


Loader = Callable[[object, str], object]


class LoaderCase(unittest.TestCase):
    def accepts(self, loader: Loader, value: JsonObject, label: str) -> None:
        """Valid, the very same object comes back, and an unknown additive field survives."""
        with self.subTest(label):
            value["added_by_a_later_slice"] = {"anything": 1}
            loaded = loader(value, label)
            self.assertIs(loaded, value)
            self.assertEqual(value["added_by_a_later_slice"], {"anything": 1})

    def rejects(self, loader: Loader, value: object, label: str, mentions: str) -> None:
        with self.subTest(label), self.assertRaises(RecordError) as caught:
            loader(value, label)
        self.assertIn(mentions, str(caught.exception))
        self.assertIn(label, str(caught.exception))  # names where it came from
        self.assertNotIn("delete", str(caught.exception).replace("do not delete", ""))


class Fingerprints(LoaderCase):
    def test_01_a_fingerprint_loads_and_unknown_fields_survive(self) -> None:
        self.accepts(records.load_fingerprint, copy.deepcopy(FINGERPRINT), "fp")
        self.accepts(records.load_fingerprint, {"hashes": [], "files": {}}, "empty")

    def test_02_a_malformed_fingerprint_is_named(self) -> None:
        self.rejects(records.load_fingerprint, [], "fp", "JSON object")
        self.rejects(records.load_fingerprint, {"files": {}}, "fp", "hashes")
        self.rejects(records.load_fingerprint, {"hashes": [1], "files": {}}, "fp", "hashes")
        self.rejects(records.load_fingerprint, {"hashes": [], "files": []}, "fp", "files")
        bad = {"hashes": [], "files": {"a": {"sha256": SHA}}}
        self.rejects(records.load_fingerprint, bad, "fp", "executable")
        bad = {"hashes": [], "files": {"a": {"sha256": 3, "executable": True}}}
        self.rejects(records.load_fingerprint, bad, "fp", "sha256")


class Pointers(LoaderCase):
    def test_01_schema_two_in_both_forms(self) -> None:
        self.accepts(records.load_pointer, pointer_v2(), "v2")
        full = pointer_v2()
        full["integration"] = {"batch": "b1", "receipts": ["r1"]}
        full["superseded_legacy"] = ["old"]
        full["receipt"] = None
        self.accepts(records.load_pointer, full, "v2 with integration")
        bootstrap = pointer_v2()
        bootstrap["integration"] = {"bootstrap": "b1", "evidence_sha256": SHA}
        self.accepts(records.load_pointer, bootstrap, "v2 published by the bootstrap")
        bad = pointer_v2()
        bad["integration"] = {"bootstrap": "b1"}
        self.rejects(records.load_pointer, bad, "p", "evidence_sha256")

    def test_01a_the_integration_bases_of_ck38(self) -> None:
        reviewed = pointer_v2()
        reviewed["integration"] = {"batch": "b1", "receipts": ["r1"], "basis": "integration-review"}
        self.accepts(records.load_pointer, reviewed, "v2 published on the integration review")
        single = pointer_v2()
        single["integration"] = single_integration()
        self.accepts(records.load_pointer, single, "v2 published on the submission review")
        unknown = pointer_v2()
        unknown["integration"] = {"batch": "b1", "receipts": [], "basis": "guess"}
        self.rejects(records.load_pointer, unknown, "p", "basis")
        for key in ("submission", "submission_receipts", "timing"):
            damaged = pointer_v2()
            damaged["integration"] = single_integration()
            del cast("JsonObject", damaged["integration"])[key]
            self.rejects(records.load_pointer, damaged, "p", key)
        for key, wrong in (("slowest_seconds", "96"), ("margin", None), ("log_sha256", 3)):
            damaged = pointer_v2()
            damaged["integration"] = single_integration()
            cast("JsonObject", cast("JsonObject", damaged["integration"])["timing"])[key] = wrong
            self.rejects(records.load_pointer, damaged, "p", key)

    def test_02_schema_one_and_the_unversioned_form(self) -> None:
        legacy = pointer_v2()
        del legacy["schema"]
        self.accepts(records.load_pointer, legacy, "no schema")
        legacy["schema"] = 1
        self.accepts(records.load_pointer, legacy, "schema 1")

    def test_03_the_hold_and_the_tombstone_forms(self) -> None:
        hold = {
            "schema": "bootstrap-hold",
            "bootstrap": "b1",
            "repo": "/x",
            "fingerprint": {"hashes": [], "files": {}},
        }
        self.accepts(records.load_pointer, hold, "hold")
        tomb = pointer_v2()
        tomb.update(schema="tombstone", upgrade="accepted-v2.json")
        self.accepts(records.load_pointer, tomb, "tombstone")

    def test_04_the_writers_output_loads(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            hub = Path(directory)
            built = pointer.tombstone(hub, accepted_pointer())
            self.assertEqual(records.load_pointer(built, "tombstone()")["schema"], "tombstone")
            held = pointer.tombstone(hub, accepted_pointer(), bootstrap="b1")
            self.assertEqual(records.load_pointer(held, "hold()")["schema"], "bootstrap-hold")
            path = hub / ".pipeline/accepted-v2.json"
            pointer.write_pointer(path, accepted_pointer())
            self.assertEqual(records.load_pointer(records.read_json(path), str(path))["schema"], 2)

    def test_05_malformed_pointers_are_named(self) -> None:
        self.rejects(
            records.load_pointer, {"schema": 7, "repo": "/x"}, "p", "unsupported pointer schema"
        )
        no_repo = pointer_v2()
        del no_repo["repo"]
        self.rejects(records.load_pointer, no_repo, "p", "repo")
        bad = pointer_v2()
        bad["submissions"] = "s1"
        self.rejects(records.load_pointer, bad, "p", "submissions")
        bad = pointer_v2()
        bad["integration"] = {"batch": 1, "receipts": []}
        self.rejects(records.load_pointer, bad, "p", "batch")
        bad = pointer_v2()
        bad["receipt"] = 5
        self.rejects(records.load_pointer, bad, "p", "receipt")
        hold = {
            "schema": "bootstrap-hold",
            "repo": "/x",
            "fingerprint": {"hashes": [], "files": {}},
        }
        self.rejects(records.load_pointer, hold, "p", "bootstrap")


class Manifests(LoaderCase):
    def test_01_the_current_form_with_and_without_a_submitter(self) -> None:
        self.accepts(records.load_manifest, manifest_v2(), "v2")
        with_submitter = manifest_v2()
        with_submitter["submitter"] = copy.deepcopy(AUTHOR)
        self.accepts(records.load_manifest, with_submitter, "v2 with submitter")

    def test_02_the_older_form_without_issues_or_provenance(self) -> None:
        legacy = {
            "id": "1792012152768312000-52be4dd8",
            "repo": "/x",
            "spec": "Route bootstrap docs refusals",
            "hashes": ["AAAA"],
            "fingerprint": copy.deepcopy(FINGERPRINT),
        }
        self.accepts(records.load_manifest, legacy, "legacy")

    def test_03_unknown_authors_and_the_owner_author_load(self) -> None:
        unknown = manifest_v2()
        unknown["authors"] = [
            {
                "identity": "unknown",
                "family": "unknown",
                "families": ["unknown"],
                "hashes": ["AAAA"],
            }
        ]
        self.accepts(records.load_manifest, unknown, "unknown author")
        owner = manifest_v2()
        owner["authors"] = [{"identity": "human", "family": "human", "role": "owner"}]
        self.accepts(records.load_manifest, owner, "owner author")

    def test_04_malformed_manifests_are_named(self) -> None:
        self.rejects(records.load_manifest, [], "m", "JSON object")
        bad = manifest_v2()
        bad["schema"] = 3
        self.rejects(records.load_manifest, bad, "m", "unsupported manifest schema")
        for key in ("issues", "provenance", "authors", "base", "supersedes", "files"):
            gone = manifest_v2()
            del gone[key]
            self.rejects(records.load_manifest, gone, "m", key)
        bad = manifest_v2()
        bad["provenance"] = {"AAAA": 5}
        self.rejects(records.load_manifest, bad, "m", "provenance")
        bad = manifest_v2()
        bad["issues"] = [{"id": "CK-1"}]
        self.rejects(records.load_manifest, bad, "m", "revision")
        bad = manifest_v2()
        bad["authors"] = [{"identity": "x"}]
        self.rejects(records.load_manifest, bad, "m", "family")
        bad = manifest_v2()
        bad["base"] = {"fingerprint_sha256": SHA}
        self.rejects(records.load_manifest, bad, "m", "pointer_sha256")


class Receipts(LoaderCase):
    def test_01_each_subject_kind_loads(self) -> None:
        for kind in ("spec", "submission", "batch"):
            self.accepts(records.load_review_receipt, receipt(kind), kind)

    def test_02_verdicts_and_incremental_parents(self) -> None:
        for verdict in ("approved", "changes-requested", "rejected"):
            value = receipt("submission")
            value["verdict"] = verdict
            self.accepts(records.load_review_receipt, value, verdict)
        incremental = receipt("submission")
        incremental["incremental_over"] = "abcdef"
        self.accepts(records.load_review_receipt, incremental, "incremental")

    def test_03_the_owner_signoff_reviewer_loads(self) -> None:
        owner = receipt("batch")
        owner["reviewer"] = {
            "identity": "human",
            "role": "owner",
            "family": "human",
            "owner_signoff": True,
        }
        self.accepts(records.load_review_receipt, owner, "owner")

    def test_04_malformed_receipts_are_named(self) -> None:
        bad = receipt()
        bad["schema"] = "ck-review/2"
        self.rejects(records.load_review_receipt, bad, "r", "schema")
        bad = receipt()
        bad["kind"] = "revocation"
        self.rejects(records.load_review_receipt, bad, "r", "kind")
        bad = receipt("submission")
        del bad["subject"]["manifest_sha256"]
        self.rejects(records.load_review_receipt, bad, "r", "manifest_sha256")
        bad = receipt("batch")
        bad["subject"]["membership_manifest_sha256"] = SHA  # a string, not the list the gate writes
        self.rejects(records.load_review_receipt, bad, "r", "membership_manifest_sha256")
        bad = receipt()
        del bad["reviewer"]["effort"]
        self.rejects(records.load_review_receipt, bad, "r", "effort")
        bad = receipt()
        bad["reviewer"]["evidence"]["offset"] = "7"
        self.rejects(records.load_review_receipt, bad, "r", "offset")
        bad = receipt()
        bad["recorded"] = True
        self.rejects(records.load_review_receipt, bad, "r", "recorded")


class Provenance(LoaderCase):
    def test_01_a_payload_loads(self) -> None:
        self.accepts(records.load_provenance, provenance_payload(), "payload")

    def test_02_malformed_payloads_are_named(self) -> None:
        bad = provenance_payload()
        bad["schema"] = "unknown"
        self.rejects(records.load_provenance, bad, "p", "schema")
        bad = provenance_payload()
        bad["families"] = []
        self.rejects(records.load_provenance, bad, "p", "families")
        bad = provenance_payload()
        del bad["identity"]
        self.rejects(records.load_provenance, bad, "p", "identity")
        bad = provenance_payload()
        bad["models"] = [{"model": "m"}]
        self.rejects(records.load_provenance, bad, "p", "effort")
        bare = provenance_payload()
        for optional in ("models", "evidence", "mapping", "recorded", "role", "issues"):
            del bare[optional]  # the gate has never required these
        self.accepts(records.load_provenance, bare, "bare")

    def test_03_the_digest_helpers_agree_with_the_loader(self) -> None:
        from gate import provenance  # noqa: PLC0415  # kept next to its only use

        payload = provenance_payload()
        self.assertEqual(provenance.load_provenance_or_unknown(payload, "d"), payload)
        self.assertEqual(
            provenance.load_provenance_or_unknown({**payload, "families": ["other"]}, "d"),
            {"schema": "unknown", "digest": "d"},
        )
        description = (
            provenance.PREFIX
            + provenance.payload_digest(payload)
            + "\n"
            + json.dumps(payload, sort_keys=True, separators=(",", ":"))
        )
        parsed = provenance.parse_description(description)
        self.assertIsNotNone(parsed)
        self.assertEqual(records.load_provenance(parsed, "description")["identity"], "ck-worker-2")


class IssueEvents(LoaderCase):
    def test_01_every_event_kind_loads(self) -> None:
        for kind in ("created", "edit", "spec", "close"):
            self.accepts(records.load_issue_event, issue_event(kind), kind)

    def test_02_malformed_events_are_named(self) -> None:
        bad = issue_event()
        bad["at"] = "now"
        self.rejects(records.load_issue_event, bad, "e", "at")
        bad = issue_event()
        del bad["fields"]
        self.rejects(records.load_issue_event, bad, "e", "fields")
        bad = issue_event()
        bad["authors"] = [{"identity": "x"}]
        self.rejects(records.load_issue_event, bad, "e", "family")
        bad = issue_event()
        bad["spec_sha256"] = 3
        self.rejects(records.load_issue_event, bad, "e", "spec_sha256")


class ReviewFindings(LoaderCase):
    """Each defect the S7a review found in the first loaders, kept as a regression case."""

    def test_01_a_receipt_must_state_base_examined_and_incremental_over(self) -> None:
        for key in ("base_examined", "incremental_over"):
            bad = receipt()
            del bad[key]
            self.rejects(records.load_review_receipt, bad, "r", key)
            explicit = receipt()
            explicit[key] = None
            self.accepts(records.load_review_receipt, explicit, f"{key} null")

    def test_02_schema_values_equal_to_an_integer_are_not_that_integer(self) -> None:
        for impostor in values(True, 1.0, 2.0, "2", None, [], {}):
            with self.subTest(impostor):
                bad = pointer_v2()
                bad["schema"] = impostor
                self.rejects(records.load_pointer, bad, "p", "schema")
        legacy = pointer_v2()
        del legacy["schema"]
        self.accepts(records.load_pointer, legacy, "unversioned pointer")
        for impostor in values(True, 1.0, 2.0, "1", [], {}):
            with self.subTest(f"schema 1 impostor {impostor!r}"):
                bad = pointer_v2()
                bad["schema"] = impostor
                del bad["fingerprint"]["files"]
                self.rejects(records.load_pointer, bad, "p", "")
        explicit_null = pointer_v2()
        explicit_null["schema"] = None
        self.rejects(records.load_pointer, explicit_null, "p", "schema")
        for impostor in values(True, 2.0, "2", None, [], {}):
            with self.subTest(f"manifest {impostor!r}"):
                bad = manifest_v2()
                bad["schema"] = impostor
                self.rejects(records.load_manifest, bad, "m", "schema")

    def test_03_a_tag_that_cannot_be_a_key_is_a_record_error_not_a_type_error(self) -> None:
        for impostor in values([], {}, 1, None, True, ["spec"]):
            with self.subTest(impostor):
                bad = receipt()
                bad["kind"] = impostor
                self.rejects(records.load_review_receipt, bad, "r", "kind")
                pointer_bad = pointer_v2()
                pointer_bad["schema"] = impostor
                if impostor != 1 or impostor is True:  # the integer 1 is the schema-1 pointer
                    with self.assertRaises(RecordError):
                        records.load_pointer(pointer_bad, "p")
                payload = provenance_payload()
                payload["schema"] = impostor
                self.rejects(records.load_provenance, payload, "p", "schema")
                manifest = manifest_v2()
                manifest["schema"] = impostor
                with self.assertRaises(RecordError):
                    records.load_manifest(manifest, "m")

    def test_04_optional_fields_are_checked_where_they_are_declared(self) -> None:
        off_class = {"model": "m", "effort": "high", "events": 1, "family": []}
        author = {"identity": "w", "family": "claude", "off_class": [off_class]}
        self.rejects(records.load_author, author, "a", "family")
        manifest = manifest_v2()
        manifest["authors"] = [author]
        self.rejects(
            records.load_manifest, manifest, "m", "family"
        )  # the tagged branch, not the legacy one
        submitter = manifest_v2()
        submitter["submitter"] = author
        self.rejects(records.load_manifest, submitter, "m", "family")
        moved = pointer_v2()
        moved["_pointer_path"] = []
        self.rejects(records.load_pointer, moved, "p", "_pointer_path")
        good = pointer_v2()
        good["_pointer_path"] = "/hub/.pipeline/accepted-v2.json"
        self.accepts(records.load_pointer, good, "annotated pointer")


class VariantGuards(LoaderCase):
    """S7b review: a form is told apart from its siblings by one tag, and the loader, the audit's
    tag selector and every exported predicate agree on which."""

    CI_BATCH: ClassVar[JsonObject] = {
        "kind": "ci",
        "passed": True,
        "fingerprint": {"hashes": [], "files": {}},
        "submissions": [],
        "failed": [],
        "logs": ["/log"],
        "recorded": 1,
    }
    CI_BOOTSTRAP: ClassVar[JsonObject] = {
        "kind": "ci",
        "passed": True,
        "fingerprint": {"hashes": [], "files": {}},
        "submissions": [],
        "bootstrap": "b",
        "log": "/log",
    }

    def test_01_a_ci_receipt_with_both_forms_tags_is_refused_not_misread(self) -> None:
        ambiguous = {**self.CI_BOOTSTRAP, "logs": []}  # the review's example
        self.rejects(records.load_ci_receipt, ambiguous, "ci", "logs")
        also = {**self.CI_BATCH, "bootstrap": "b", "log": "/log"}
        self.rejects(records.load_ci_receipt, also, "ci", "logs")

    def test_02_the_ci_predicates_say_exactly_what_the_loader_established(self) -> None:
        batch = records.load_ci_receipt(copy.deepcopy(self.CI_BATCH), "ci")
        boot = records.load_ci_receipt(copy.deepcopy(self.CI_BOOTSTRAP), "ci")
        self.assertEqual(
            (records.is_batch_ci_receipt(batch), records.is_bootstrap_ci_receipt(batch)),
            (True, False),
        )
        self.assertEqual(
            (records.is_batch_ci_receipt(boot), records.is_bootstrap_ci_receipt(boot)),
            (False, True),
        )
        both = cast("records.CIReceipt", {**self.CI_BOOTSTRAP, "logs": []})  # never loaded
        self.assertEqual(
            (records.is_batch_ci_receipt(both), records.is_bootstrap_ci_receipt(both)),
            (False, False),
        )
        legacy = records.load_ci_receipt(
            {"submissions": [], "repo": "/r", "started": 1.5, "passed": False}, "ci"
        )
        self.assertEqual(
            (records.is_batch_ci_receipt(legacy), records.is_bootstrap_ci_receipt(legacy)),
            (False, False),
        )

    def test_03_a_docs_receipt_with_both_forms_tags_is_refused(self) -> None:
        both = {"kind": "docs", "passed": True, "inherits": None, "id": "D1", "hashes": []}
        self.rejects(records.load_docs_receipt, both, "docs", "id")

    def test_04_a_bootstrap_receipt_is_never_a_review_row(self) -> None:
        bootstrap = records.read_json(FIXTURES / "evidence-receipt-spec.json")
        assert isinstance(bootstrap, dict)
        loaded = records.load_bootstrap_receipt(copy.deepcopy(bootstrap), "r")
        self.assertFalse(records.is_review_row(loaded))
        with_id = cast("records.BootstrapReceipt", {**bootstrap, "id": "x"})  # an additive id
        self.assertFalse(records.is_review_row(with_id))
        stored = records.load_review_receipt(receipt("spec"), "r")
        self.assertTrue(records.is_review_row(cast("records.ReviewRow", stored)))


def keys_of(value: object, prefix: tuple[object, ...] = ()) -> Iterator[tuple[object, ...]]:
    """Every path to a value inside nested objects and lists."""
    if isinstance(value, dict):
        for key, item in value.items():
            yield (*prefix, key)
            yield from keys_of(item, (*prefix, key))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            yield (*prefix, index)
            yield from keys_of(item, (*prefix, index))


def replaced(value: JsonObject, path: tuple[object, ...], new: object) -> JsonObject:
    result = copy.deepcopy(value)
    holder: Any = result
    for step in path[:-1]:
        holder = holder[step]
    if new is DELETE:
        del holder[path[-1]]
    else:
        holder[path[-1]] = new
    return result


DELETE = object()


def values(*items: object) -> tuple[object, ...]:
    return items


JUNK: tuple[object, ...] = (None, True, False, 0, 1.5, "x", "", [], {}, [1], {"a": 1}, [[]], [{}])


def required_keys(typed: object) -> set[str]:
    """The keys the TypedDict declares required (resolved, so `from __future__` annotations count)."""
    hints = typing.get_type_hints(typed, include_extras=True)
    return {key for key, hint in hints.items() if typing.get_origin(hint) is not typing.NotRequired}


def selected_shapes(loader: Loader, loaded: object) -> list[tuple[object, object]]:  # noqa: PLR0911  # one return per tagged record kind
    """(value, TypedDict) pairs that the loaded record's own tags select, where the declared
    return type is a union that an untagged member could satisfy on its own."""
    if not isinstance(loaded, dict):
        return []
    if loader is records.load_manifest:
        shape = records.ManifestV2 if "schema" in loaded else records.ManifestLegacy
        return [(loaded, shape)]
    if loader is records.load_pointer:
        shapes: dict[object, object] = {
            "bootstrap-hold": records.PointerHold,
            "tombstone": records.PointerTombstone,
            2: records.PointerV2,
        }
        return [(loaded, shapes.get(loaded.get("schema"), records.PointerSchema1))]
    if loader is records.load_review_receipt:
        subjects: dict[object, object] = {
            "spec": records.SpecSubject,
            "submission": records.SubmissionSubject,
            "batch": records.BatchSubject,
        }
        reviewer = (
            records.OwnerReviewer
            if loaded["reviewer"].get("owner_signoff") is True
            else records.AiReviewer
        )
        return [(loaded["subject"], subjects[loaded["kind"]]), (loaded["reviewer"], reviewer)]
    if loader is records.load_ci_receipt:
        if "bootstrap" in loaded:
            return [(loaded, records.BootstrapCIReceipt)]
        return [(loaded, records.BatchCIReceipt if "logs" in loaded else records.LegacyCIReceipt)]
    if loader is records.load_docs_receipt:
        return [
            (
                loaded,
                records.DocsRestoreReceipt if "inherits" in loaded else records.DocsPublishReceipt,
            )
        ]
    if loader is records.load_bootstrap_receipt:
        bootstrap_subjects: dict[object, object] = {
            "spec": records.BootstrapSpecSubject,
            "submission": records.SubmissionSubject,
            "integration": records.BootstrapIntegrationSubject,
        }
        return [(loaded["subject"], bootstrap_subjects[loaded["kind"]])]
    return []


# Keys that belong to a sibling form of some record: added to a record of another form they make
# it ambiguous (rejected) or leave it what it was (accepted), and no guard may then overclaim.
SIBLING_KEYS = (
    "logs",
    "bootstrap",
    "inherits",
    "id",
    "schema",
    "kind",
    "submission",
    "issue",
    "batch",
    "human",
    "owner_signoff",
    "passed",
)


def dict_locations(value: object, prefix: tuple[object, ...] = ()) -> Iterator[tuple[object, ...]]:
    """The path of every object inside `value`, the root included."""
    if isinstance(value, dict):
        yield prefix
        for key, item in value.items():
            yield from dict_locations(item, (*prefix, key))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            yield from dict_locations(item, (*prefix, index))


def guard_postconditions(loader: Loader, loaded: object) -> list[str]:
    """The exported narrowing predicates that claim a loaded record (or part of it), with the claims
    that are false: a guard that returns True must leave a value of the shape it narrows to."""
    claims: list[tuple[str, bool, object, object]] = []
    if not isinstance(loaded, dict):
        return []
    record: Any = loaded  # the predicates are applied to whatever the loader accepted
    if loader is records.load_ci_receipt:
        claims += [
            (
                "is_batch_ci_receipt",
                records.is_batch_ci_receipt(record),
                loaded,
                records.BatchCIReceipt,
            ),
            (
                "is_bootstrap_ci_receipt",
                records.is_bootstrap_ci_receipt(record),
                loaded,
                records.BootstrapCIReceipt,
            ),
        ]
    if loader is records.load_manifest:
        claims.append(
            ("is_manifest_v2", records.is_manifest_v2(record), loaded, records.ManifestV2)
        )
    if loader is records.load_pointer:
        claims += [
            ("is_hold", records.is_hold(record), loaded, records.PointerHold),
            ("is_tombstone", records.is_tombstone(record), loaded, records.PointerTombstone),
            (
                "is_accepted_pointer",
                records.is_accepted_pointer(record),
                loaded,
                records.PointerV2 | records.PointerSchema1,
            ),
        ]
    if loader is records.load_review_receipt:
        reviewer: Any = loaded["reviewer"]
        claims += [
            ("is_review_row", records.is_review_row(record), loaded, records.ReviewReceipt),
            (
                "is_owner_reviewer",
                records.is_owner_reviewer(reviewer),
                reviewer,
                records.OwnerReviewer,
            ),
        ]
    wrong = [name for name, result, value, shape in claims if result and not conforms(value, shape)]
    if loader is records.load_bootstrap_receipt and records.is_review_row(record):
        wrong.append("is_review_row (a bootstrap receipt is not a ck-review/1 receipt)")
    return wrong


# The key that tells a form apart from its sibling forms: without it the record is the other form.
TAG_KEYS = frozenset({"schema", "logs", "inherits", "bootstrap"})
TAGGED = (
    records.PointerV2,
    records.ManifestV2,
    records.BatchCIReceipt,
    records.BootstrapCIReceipt,
    records.DocsRestoreReceipt,
)


class Audit(LoaderCase):
    """Every loader against its declared TypedDict and against malformed shapes at every depth.

    A required key that is absent must be named (a `RecordError`); a key declared `NotRequired`
    must load without it; and no value of the wrong type anywhere in a record may escape as
    anything but a `RecordError` (never a `TypeError`, `KeyError`, `AttributeError`)."""

    def subjects(self) -> list[tuple[str, Loader, JsonObject, object | None]]:
        owner = receipt()
        owner["reviewer"] = {
            "identity": "human",
            "family": "human",
            "role": "owner",
            "owner_signoff": True,
        }
        incremental = receipt("submission")
        incremental["incremental_over"] = "earlier"
        legacy_pointer = pointer_v2()
        del legacy_pointer["schema"]
        legacy_manifest = manifest_v2()
        for key in ("schema", "files", "issues", "provenance", "authors", "base", "supersedes"):
            del legacy_manifest[key]
        base: list[tuple[str, Loader, JsonObject, object | None]] = [
            ("pointer v2", records.load_pointer, pointer_v2(), records.PointerV2),
            ("pointer unversioned", records.load_pointer, legacy_pointer, records.PointerSchema1),
            (
                "pointer (single-submission basis)",
                records.load_pointer,
                {**pointer_v2(), "integration": single_integration()},
                records.PointerV2,
            ),
            (
                "pointer (integration-review basis)",
                records.load_pointer,
                {
                    **pointer_v2(),
                    "integration": {
                        "batch": "b1",
                        "receipts": ["r1"],
                        "basis": "integration-review",
                    },
                },
                records.PointerV2,
            ),
            ("manifest v2", records.load_manifest, manifest_v2(), records.ManifestV2),
            ("manifest legacy", records.load_manifest, legacy_manifest, records.ManifestLegacy),
            ("spec receipt", records.load_review_receipt, receipt("spec"), records.ReviewReceipt),
            ("submission receipt", records.load_review_receipt, receipt("submission"), None),
            ("batch receipt", records.load_review_receipt, receipt("batch"), None),
            ("owner receipt", records.load_review_receipt, owner, None),
            ("incremental receipt", records.load_review_receipt, incremental, None),
            (
                "provenance",
                records.load_provenance,
                provenance_payload(),
                records.ProvenancePayload,
            ),
            ("created event", records.load_issue_event, issue_event("created"), records.IssueEvent),
            ("close event", records.load_issue_event, issue_event("close"), None),
            ("author", records.load_author, copy.deepcopy(AUTHOR), records.Author),
            (
                "fingerprint",
                records.load_fingerprint,
                copy.deepcopy(FINGERPRINT),
                records.Fingerprint,
            ),
            (
                "holder",
                records.load_holder,
                {"pid": 7, "command": "ck", "started": 1.5},
                records.Holder,
            ),
            ("edit event", records.load_issue_event, issue_event("edit"), None),
            (
                "gate config",
                records.load_gate_config,
                {"meta_repo": "/m", "schema": 2},
                records.GateConfig,
            ),
            ("counters", records.load_counters, {"issue": 3, "event": 0, "review": 1}, None),
            ("session roots", records.load_session_roots, {"claude": "/a", "codex": "/b"}, None),
            (
                "revocation by an AI reviewer",
                records.load_meta_event,
                {
                    "kind": "revocation",
                    "review": "r",
                    "reason": "x",
                    "actor": {
                        "identity": "ck-reviewer-1",
                        "family": "codex",
                        "role": "reviewer",
                        "client": "codex",
                        "session": "s",
                        "model": "m",
                        "effort": "high",
                        "class": "A",
                    },
                },
                None,
            ),
            (
                "revocation by the owner",
                records.load_meta_event,
                {
                    "kind": "revocation",
                    "review": "r",
                    "reason": "x",
                    "actor": {
                        "identity": "human",
                        "family": "human",
                        "role": "owner",
                        "owner_signoff": True,
                    },
                },
                None,
            ),
            (
                "model classes",
                records.load_model_classes,
                {
                    "classes": {
                        "A": {"description": "d", "claude": {"model": "m", "effort": "high"}},
                        "B": {"route": "A"},
                    },
                    "clients": {"copilot": {"model_aliases": {"m": "alias"}}},
                },
                None,
            ),
            (
                "meta event",
                records.load_meta_event,
                {"kind": "supersession", "submission": "s", "by": "t", "authors": [AUTHOR]},
                records.MetaEvent,
            ),
        ]
        base += self.workflow_subjects()
        # The same records with every optional field the declared type has, recursively, so the
        # fields a fixture happens not to carry are audited too.
        full = [
            (f"{label} (every optional field)", loader, overlay(value, sample(typed)), typed)
            for label, loader, value, typed in base
            if typed is not None
        ]
        assert all(isinstance(entry[2], dict) for entry in full)
        return [*base, *cast("list[tuple[str, Loader, JsonObject, object | None]]", full)]

    def workflow_subjects(self) -> list[tuple[str, Loader, JsonObject, object | None]]:
        """The batch, CI, docs and bootstrap records: copies of real ones where the hub has the
        kind, hand-built for the forms it does not (an infrastructure failure, a restore)."""

        def real(stem: str) -> JsonObject:
            value = records.read_json(FIXTURES / f"{stem}.json")
            assert isinstance(value, dict)
            return copy.deepcopy(value)

        entry = real("docs-entry")
        restore_entry = {"id": "D1", "entry_sha256": SHA, "state": "published", "entry": entry}
        restore_conflict = {
            "id": "D2",
            "entry_sha256": SHA,
            "state": "conflict",
            "conflicts": [{"path": "a"}],
            "entry": entry,
        }
        link = {
            "id": "D1",
            "entry_sha256": SHA,
            "candidate": "/hub/c",
            "fingerprint": copy.deepcopy(FINGERPRINT),
            "receipt": "/hub/r.json",
            "receipt_sha256": SHA,
            "hashes": ["AAAA"],
            "previous_pointer_sha256": SHA,
            "pointer": "{}",
        }
        return [
            ("batch frozen", records.load_batch_frozen, real("batch-frozen"), records.BatchFrozen),
            (
                "batch validating",
                records.load_batch_validating,
                real("batch-validating"),
                records.BatchValidating,
            ),
            (
                "batch validated",
                records.load_batch_validated,
                real("batch-validated-submitters"),
                records.BatchValidated,
            ),
            (
                "batch validated (bound CI log)",
                records.load_batch_validated,
                {**real("batch-validated-submitters"), "ci_log_sha256": SHA},
                records.BatchValidated,
            ),
            (
                "batch published (single-submission basis)",
                records.load_batch_published,
                {**real("batch-published"), "basis": "single-submission"},
                records.BatchPublished,
            ),
            (
                "batch approved",
                records.load_batch_approved,
                real("batch-approved"),
                records.BatchApproved,
            ),
            (
                "batch publishing",
                records.load_batch_publishing,
                real("batch-publishing"),
                records.BatchPublishing,
            ),
            (
                "batch published",
                records.load_batch_published,
                real("batch-published"),
                records.BatchPublished,
            ),
            (
                "batch failed",
                records.load_batch_failed,
                real("batch-failed-validation"),
                records.BatchFailed,
            ),
            (
                "batch failed (infrastructure)",
                records.load_batch_failed,
                {"reason": "infrastructure", "exit": 75, "detail": "d", "at": 1, "failed": []},
                records.BatchFailed,
            ),
            (
                "batch rejected",
                records.load_batch_rejected,
                real("batch-rejected"),
                records.BatchRejected,
            ),
            (
                "batch cancelled",
                records.load_batch_cancelled,
                real("batch-cancelled"),
                records.BatchCancelled,
            ),
            ("quarantine", records.load_quarantine, real("quarantine"), records.QuarantineRecord),
            (
                "ci receipt (batch)",
                records.load_ci_receipt,
                real("ci-receipt-batch-chat-inputs"),
                records.BatchCIReceipt,
            ),
            (
                "ci receipt (bootstrap)",
                records.load_ci_receipt,
                real("ci-receipt-bootstrap"),
                records.BootstrapCIReceipt,
            ),
            (
                "ci receipt (legacy)",
                records.load_ci_receipt,
                real("checkpoint-legacy"),
                records.LegacyCIReceipt,
            ),
            (
                "ci receipt (legacy, failed)",
                records.load_ci_receipt,
                {"submissions": [], "repo": "/r", "started": 1.5, "passed": False, "error": "x"},
                records.LegacyCIReceipt,
            ),
            (
                "chat inputs",
                records.load_chat_inputs,
                real("checkpoint-chat-inputs"),
                records.ChatInputs,
            ),
            (
                "docs receipt (restore)",
                records.load_docs_receipt,
                real("checkpoint-docs-restore"),
                records.DocsRestoreReceipt,
            ),
            (
                "docs receipt (publish)",
                records.load_docs_receipt,
                real("docs-receipt"),
                records.DocsPublishReceipt,
            ),
            ("docs entry", records.load_docs_entry, entry, records.DocsEntry),
            ("docs journal", records.load_docs_journal, real("docs-journal"), records.DocsJournal),
            (
                "pijul pull",
                records.load_pijul_pull,
                {"conflicts": [{"path": "a"}]},
                records.PijulPull,
            ),
            (
                "queue summary",
                records.load_queue_summary,
                {"schema": 2, "id": "s", "spec": "t"},
                None,
            ),
            (
                "pijul status",
                records.load_pijul_status,
                {"changes": [{"path": "a", "operation": "edit"}], "untracked": ["b"]},
                records.PijulStatus,
            ),
            (
                "bootstrap frozen",
                records.load_bootstrap_frozen,
                real("bootstrap-frozen"),
                records.BootstrapFrozen,
            ),
            (
                "bootstrap validated",
                records.load_bootstrap_validated,
                real("bootstrap-validated"),
                records.BootstrapValidated,
            ),
            (
                "bootstrap publishing",
                records.load_bootstrap_publishing,
                real("bootstrap-publishing"),
                records.BootstrapPublishing,
            ),
            (
                "bootstrap published",
                records.load_bootstrap_published,
                real("bootstrap-published"),
                records.BootstrapPublished,
            ),
            (
                "bootstrap failed",
                records.load_bootstrap_failed,
                {"reason": "r", "at": 1, "conflicts": [{"a": 1}]},
                records.BootstrapFailed,
            ),
            (
                "bootstrap cancelled",
                records.load_bootstrap_cancelled,
                {"reason": "r", "at": 1},
                records.BootstrapCancelled,
            ),
            (
                "bootstrap restoring",
                records.load_bootstrap_restoring,
                {
                    "mode": "restored",
                    "pointer": "{}",
                    "pointer_sha256": SHA,
                    "entries": [{**restore_entry, "already_present": True}],
                    "links": [link],
                    "conflicts": [restore_conflict],
                },
                records.BootstrapRestoring,
            ),
            (
                "bootstrap restored",
                records.load_bootstrap_restored,
                {"mode": "never-held", "pointer_sha256": SHA, "conflicts": [restore_conflict]},
                records.BootstrapRestored,
            ),
            (
                "evidence index",
                records.load_evidence_index,
                real("evidence-index"),
                records.EvidenceIndex,
            ),
            (
                "authors ledger",
                records.load_authors_ledger,
                real("evidence-authors"),
                records.AuthorsLedger,
            ),
            (
                "bootstrap receipt (spec)",
                records.load_bootstrap_receipt,
                real("evidence-receipt-spec"),
                records.BootstrapReceipt,
            ),
            (
                "bootstrap receipt (submission)",
                records.load_bootstrap_receipt,
                real("evidence-receipt-submission"),
                None,
            ),
            (
                "bootstrap receipt (integration)",
                records.load_bootstrap_receipt,
                real("evidence-receipt-integration"),
                None,
            ),
        ]

    def test_01_declared_required_keys_are_required_and_the_rest_are_not(self) -> None:
        for label, loader, value, typed in self.subjects():
            if typed is None:
                continue
            declared = required_keys(typed)
            for key in value:
                if key in TAG_KEYS and typed in TAGGED:
                    continue  # without its tag the record is another valid form
                with self.subTest(f"{label}: delete {key}"):
                    candidate = replaced(value, (key,), DELETE)
                    if key in declared:
                        with self.assertRaises(RecordError) as caught:
                            loader(candidate, label)
                        self.assertIn(key, str(caught.exception))
                    else:
                        loader(candidate, label)  # optional by declaration: absence is valid
            for key in declared:
                self.assertIn(key, value, f"{label}: the fixture lacks the required key {key}")

    @staticmethod
    def problem(label: str, loader: Loader, declared: object, candidate: JsonObject) -> str | None:
        """What is wrong with the way `loader` treated `candidate`, or None: it must be refused with
        a `RecordError`, or accepted as the declared type, the shape its own tag selects, and with no
        exported narrowing predicate claiming more than it is."""
        try:
            loaded = loader(candidate, label)
        except RecordError:
            return None  # named, as required
        if not conforms(loaded, declared):
            return f"accepted, but it does not have the shape {declared}: {loaded!r}"
        for part, shape in selected_shapes(loader, loaded):
            if not conforms(part, shape):
                return f"accepted, but its own tag selects {shape} and it is not one: {part!r}"
        wrong = guard_postconditions(loader, loaded)
        return f"accepted, but {wrong} claim more than the value is: {loaded!r}" if wrong else None

    def test_02_a_value_of_any_wrong_type_at_any_depth_is_a_record_error_or_valid(self) -> None:
        failures: list[str] = []
        for label, loader, value, _ in self.subjects():
            declared = typing.get_type_hints(loader)["return"]
            for path in keys_of(value):
                for junk in (*JUNK, DELETE):
                    found = self.problem(label, loader, declared, replaced(value, path, junk))
                    if found:
                        failures.append(f"{label}: {path} -> {junk!r}: {found}")
        self.assertEqual(failures[:10], [], f"{len(failures)} mutations")

    def test_02a_a_sibling_form_tag_added_to_any_object_never_makes_a_guard_overclaim(self) -> None:
        tagged_loaders = (
            records.load_ci_receipt,
            records.load_docs_receipt,
            records.load_manifest,
            records.load_pointer,
            records.load_review_receipt,
            records.load_bootstrap_receipt,
        )
        failures: list[str] = []
        for label, loader, value, _ in self.subjects():
            if loader not in tagged_loaders:
                continue
            declared = typing.get_type_hints(loader)["return"]
            for location in dict_locations(value):
                if len(location) > 1:
                    continue  # the tags that matter sit on the record and on its subject or reviewer
                for key in SIBLING_KEYS:
                    for junk in values([], "x", True):
                        candidate = replaced(value, (*location, key), junk)
                        found = self.problem(label, loader, declared, candidate)
                        if found:
                            failures.append(f"{label}: {location} + {key} = {junk!r}: {found}")
        self.assertEqual(failures[:10], [], f"{len(failures)} mutations")

    def test_03_a_top_level_value_that_is_not_an_object_is_named(self) -> None:
        for label, loader, _, _ in self.subjects():
            for junk in values(None, True, 1, 1.5, "x", [], [{}]):
                with self.subTest(f"{label}: {junk!r}"), self.assertRaises(RecordError):
                    loader(junk, label)


FIXTURES = Path(__file__).resolve().parent / "fixtures/records"


class RealRecords(unittest.TestCase):
    """One record of each kind, copied read-only from this project's hub and metadata repository
    (paths and the home directory replaced; nothing else changed), loads through its loader.

    The owner sign-off receipt and the lock holder note do not exist at rest on the hub, so those
    kinds are covered by the hand-built fixtures above and by the writers' own tests."""

    LOADERS: ClassVar[dict[str, Loader]] = {
        "pointer-v2": records.load_pointer,
        "pointer-tombstone": records.load_pointer,
        "manifest-legacy": records.load_manifest,
        "manifest-v2": records.load_manifest,
        "manifest-v2-submitter": records.load_manifest,
        "receipt-spec": records.load_review_receipt,
        "receipt-submission": records.load_review_receipt,
        "receipt-submission-incremental": records.load_review_receipt,
        "receipt-batch": records.load_review_receipt,
        "issue-event-created": records.load_issue_event,
        "issue-event-spec": records.load_issue_event,
        "issue-event-edit": records.load_issue_event,
        "issue-event-close": records.load_issue_event,
        "meta-event-supersession": records.load_meta_event,
        "meta-event-revocation": records.load_meta_event,
        "meta-event-bootstrap": records.load_meta_event,
        "provenance-payload": records.load_provenance,
        "batch-frozen": records.load_batch_frozen,
        "batch-validating": records.load_batch_validating,
        "batch-validated": records.load_batch_validated,
        "batch-validated-submitters": records.load_batch_validated,
        "batch-approved": records.load_batch_approved,
        "batch-publishing": records.load_batch_publishing,
        "batch-published": records.load_batch_published,
        "batch-failed-validation": records.load_batch_failed,
        "batch-rejected": records.load_batch_rejected,
        "batch-cancelled": records.load_batch_cancelled,
        "quarantine": records.load_quarantine,
        "ci-receipt-batch": records.load_ci_receipt,
        "ci-receipt-batch-chat-inputs": records.load_ci_receipt,
        "ci-receipt-batch-failed": records.load_ci_receipt,
        "ci-receipt-bootstrap": records.load_ci_receipt,
        "checkpoint-legacy": records.load_ci_receipt,
        "checkpoint-docs-restore": records.load_docs_receipt,
        "checkpoint-chat-inputs": records.load_chat_inputs,
        "docs-entry": records.load_docs_entry,
        "docs-journal": records.load_docs_journal,
        "docs-receipt": records.load_docs_receipt,
        "bootstrap-frozen": records.load_bootstrap_frozen,
        "bootstrap-validated": records.load_bootstrap_validated,
        "bootstrap-publishing": records.load_bootstrap_publishing,
        "bootstrap-published": records.load_bootstrap_published,
        "evidence-index": records.load_evidence_index,
        "evidence-authors": records.load_authors_ledger,
        "evidence-receipt-spec": records.load_bootstrap_receipt,
        "evidence-receipt-submission": records.load_bootstrap_receipt,
        "evidence-receipt-integration": records.load_bootstrap_receipt,
    }

    def test_01_every_copied_record_loads_unchanged(self) -> None:
        stems = sorted(path.stem for path in FIXTURES.glob("*.json") if path.stem != "registry-row")
        self.assertEqual(stems, sorted(self.LOADERS), "every copied record has a loader, and back")
        for stem in stems:
            with self.subTest(stem):
                value = records.read_json(FIXTURES / f"{stem}.json")
                self.assertIs(self.LOADERS[stem](value, stem), value)

    def test_02_the_variants_the_hub_has_are_all_present(self) -> None:
        names = {path.stem for path in FIXTURES.glob("*.json")}
        self.assertTrue(
            {
                "pointer-v2",
                "pointer-tombstone",
                "manifest-legacy",
                "manifest-v2",
                "manifest-v2-submitter",
                "receipt-spec",
                "receipt-submission",
                "receipt-submission-incremental",
                "receipt-batch",
                "issue-event-created",
                "issue-event-spec",
                "issue-event-edit",
                "issue-event-close",
                "meta-event-supersession",
                "meta-event-revocation",
                "meta-event-bootstrap",
                "provenance-payload",
                "registry-row",
            }
            <= names
        )

    def test_03_a_registry_row_and_the_model_class_master_load(self) -> None:
        row = records.registry_row(records.read_json(FIXTURES / "registry-row.json"))
        assert row is not None
        self.assertEqual(row["brand"], "codex")
        master = tomllib.loads((FIXTURES / "model-classes.toml").read_text())
        classes_table = records.load_model_classes(master, "model-classes.toml")
        self.assertEqual(set(classes_table["classes"]), {"S", "A", "B"})
        self.assertIs(classes_table, master)

    def test_04_the_real_receipt_text_is_what_the_gate_reads_back(self) -> None:
        real = records.load_review_receipt(
            records.read_json(FIXTURES / "receipt-submission.json"), "receipt"
        )
        marked = records.review_row(real, revoked=False)
        self.assertIs(marked, real)
        self.assertFalse(marked["revoked"])


class GateReaders(LoaderCase):
    """The smaller boundaries the migrated modules read: holders, rows, the master, issues."""

    def test_01_a_lock_holder_note(self) -> None:
        self.accepts(records.load_holder, {"pid": 4, "command": "ck", "started": 2.5}, "h")
        self.accepts(records.load_holder, {"pid": 4}, "bare")
        for bad in (
            {"pid": True},
            {"pid": "4"},
            {},
            {"pid": 4, "started": "now"},
            {"pid": 4, "batch": 1},
        ):
            self.rejects(records.load_holder, bad, "h", "")
        self.assertEqual(records.frozen_owner_pid({"owner": {"pid": 9}}), 9)
        for odd in values(
            None, [], {"owner": 3}, {"owner": {"pid": True}}, {"owner": {"pid": "9"}}
        ):
            self.assertIsNone(records.frozen_owner_pid(odd))

    def test_02_a_registry_row_with_a_wrong_field_is_skipped_not_fatal(self) -> None:
        good = {"name": "n", "brand": "claude", "session_id": "s", "role": None, "pid": 3}
        self.assertIs(records.registry_row(good), good)
        for key, bad in (("name", 1), ("brand", []), ("pid", True), ("pid", "3"), ("role", 4)):
            self.assertIsNone(records.registry_row({**good, key: bad}), key)
        for odd in values(None, [], "row", 3):
            self.assertIsNone(records.registry_row(odd))

    def test_03_the_model_class_master(self) -> None:
        good = {
            "classes": {"A": {"claude": {"model": "m", "effort": "high"}}, "B": {"route": "A"}},
            "clients": {"copilot": {"model_aliases": {"m": "alias"}}},
            "kanban": {"anything": 1},
        }
        self.accepts(records.load_model_classes, good, "master")
        self.accepts(records.load_model_classes, {}, "empty master")
        for bad in values(
            {"classes": []},
            {"classes": {"A": []}},
            {"classes": {"A": {"route": 1}}},
            {"clients": {"c": {"model_aliases": {"m": 1}}}},
            {"clients": []},
        ):
            self.rejects(records.load_model_classes, bad, "master", "")

    def test_04_issues_fold_from_validated_events_and_keep_unknown_fields(self) -> None:
        created = records.load_issue_event(issue_event("created"), "e1")
        close = records.load_issue_event(issue_event("close"), "e2")
        bare = records.fold_issue("CK-26", [created], None)
        self.assertEqual((bare["title"], bare["state"], bare["depends"]), ("T", "open", []))
        self.assertNotIn("revision", bare)
        spec = records.SpecRevision("r3", SHA, "/spec/r3.md")
        folded = records.fold_issue("CK-26", [created, close], spec)
        self.assertEqual(
            (folded["state"], folded["reason"], folded["revision"]), ("closed", "done", "r3")
        )
        self.assertEqual(folded["spec_authors"], created["authors"])
        extra = issue_event("edit")
        extra["fields"]["added_later"] = {"x": 1}
        kept = records.fold_issue("CK-26", [records.load_issue_event(extra, "e3")], None)
        self.assertEqual(kept["added_later"], {"x": 1})  # type: ignore[typeddict-item]  # an additive field
        no_fields = records.fold_issue("CK-26", [], None)
        self.assertEqual(no_fields["priority"], "P2")

    def test_05_issue_fields_of_the_wrong_type_are_named(self) -> None:
        for key, bad in (("title", 1), ("state", []), ("depends", "CK-1"), ("depends", [1])):
            event = issue_event("created")
            event["fields"][key] = bad
            self.rejects(records.load_issue_event, event, "e", key)

    def test_06_meta_events(self) -> None:
        actor = {
            "identity": "ck-reviewer-1",
            "family": "codex",
            "role": "reviewer",
            "client": "codex",
            "session": "s",
            "model": "m",
            "effort": "high",
            "class": "A",
        }
        revocation = {"kind": "revocation", "review": "r", "reason": "x", "actor": actor}
        self.accepts(records.load_meta_event, copy.deepcopy(revocation), "AI actor")
        owner = {
            **revocation,
            "actor": {
                "identity": "human",
                "family": "human",
                "role": "owner",
                "owner_signoff": True,
            },
        }
        self.accepts(records.load_meta_event, owner, "owner actor")
        for bad_actor in values(
            {}, {"identity": "i"}, {**actor, "identity": 3}, {**actor, "class": None}, []
        ):
            self.rejects(records.load_meta_event, {**revocation, "actor": bad_actor}, "m", "")
        for bad in values(
            {},
            {"kind": 3},
            {"kind": "k", "supersedes": "s"},
            {"kind": "k", "actor": []},
            {"kind": "k", "authors": [1]},
        ):
            self.rejects(records.load_meta_event, bad, "m", "")

    def test_07_a_class_description_must_be_text(self) -> None:
        for bad in values([], 3, None, {}):
            self.rejects(
                records.load_model_classes,
                {"classes": {"A": {"description": bad}}},
                "master",
                "description",
            )

    def test_08_an_event_cannot_overwrite_what_the_issue_derives(self) -> None:
        for key in ("id", "spec_sha256", "spec_path", "spec_authors"):
            for bad in values([], "CK-9", 3):
                event = issue_event("edit")
                event["fields"][key] = bad
                self.rejects(records.load_issue_event, event, "e", key)
                # a caller that skipped the loader still cannot get an unsound Issue out
                with self.assertRaises(RecordError) as caught:
                    records.fold_issue("CK-26", [cast("records.IssueEvent", event)], None)
                self.assertIn(key, str(caught.exception))

    def test_08a_the_folded_issue_is_checked_whatever_the_caller_passed(self) -> None:
        spec = records.SpecRevision("r1", SHA, "/s/r1.md")
        for key, bad in (
            ("title", []),
            ("state", 3),
            ("depends", "CK-1"),
            ("reason", {}),
            ("assign", 1),
        ):
            event = cast("records.IssueEvent", {**issue_event("edit"), "fields": {key: bad}})
            with self.assertRaises(RecordError) as caught:
                records.fold_issue("CK-26", [event], None)
            self.assertIn(key, str(caught.exception))
        unvalidated = cast("records.IssueEvent", {**issue_event("created"), "authors": [1]})
        with self.assertRaises(RecordError):
            records.fold_issue("CK-26", [unvalidated], spec)

    def test_09_a_folded_issue_has_the_declared_shape_and_keeps_additive_fields(self) -> None:
        folded = records.fold_issue(
            "CK-26",
            [
                records.load_issue_event(issue_event("created"), "e1"),
                records.load_issue_event(issue_event("close"), "e2"),
            ],
            records.SpecRevision("r2", SHA, "/s/r2.md"),
        )
        self.assertTrue(conforms(folded, records.Issue))
        self.assertEqual(folded["id"], "CK-26")
        extra = issue_event("edit")
        extra["fields"]["added_later"] = [1]
        kept = records.fold_issue("CK-26", [records.load_issue_event(extra, "e")], None)
        self.assertEqual(kept["added_later"], [1])  # type: ignore[typeddict-item]  # additive
        self.assertTrue(conforms(kept, records.Issue))


class RegistrySweep(unittest.TestCase):
    def test_01_a_registry_row_is_a_row_of_the_declared_shape_or_nothing(self) -> None:
        row: JsonObject = {
            "name": "n",
            "brand": "claude",
            "session_id": "s",
            "role": "worker",
            "pid": 3,
            "pid_start": "t",
            "extra": [1],
        }
        for path in keys_of(row):
            for junk in (*JUNK, DELETE):
                with self.subTest(f"{path} -> {junk!r}"):
                    loaded = records.registry_row(replaced(row, path, junk))
                    self.assertTrue(loaded is None or conforms(loaded, records.RegistryRow), loaded)


class SmallFiles(LoaderCase):
    def test_01_the_patch_log_pairs_each_hash_with_its_description(self) -> None:
        log = [
            {"hash": "H1", "description": "d"},
            {"hash": "H2", "description": None},
            {"hash": "H3"},
        ]
        self.assertEqual(records.patch_descriptions(log, "log"), {"H1": "d", "H2": "", "H3": ""})
        for bad in values({}, [1], [{}], [{"hash": 3}], [{"hash": "H", "description": 3}], "x"):
            with self.subTest(bad), self.assertRaises(RecordError):
                records.patch_descriptions(bad, "log")

    def test_02_a_class_family_entry_must_name_a_model_and_an_effort_as_text(self) -> None:
        from gate import classes  # noqa: PLC0415  # next to its only use

        def master(entry: object) -> records.ModelClasses:
            return cast("records.ModelClasses", {"classes": {"A": {"claude": entry}}})

        ok = classes.resolve_class(master({"model": "m", "effort": "high"}), "A", "claude")
        self.assertEqual((ok["model"], ok["effort"]), ("m", "high"))
        for bad in values(
            {"model": 7, "effort": "high"},
            {"model": "m", "effort": True},
            {"model": "m"},
            {"model": "", "effort": "high"},
            [],
            None,
        ):
            with (
                self.subTest(bad),
                self.assertRaisesRegex(RuntimeError, "has no claude model and effort"),
            ):
                classes.resolve_class(master(bad), "A", "claude")


class SessionLogs(unittest.TestCase):
    """What a native session log says about the model and effort is checked before it becomes a
    `ModelUse` string; a value of another type is refused naming the line, for every client."""

    CLAUDE_OK: ClassVar[JsonObject] = {
        "type": "assistant",
        "message": {"model": "claude-sonnet-5-5"},
        "effort": "high",
    }
    CODEX_OK: ClassVar[JsonObject] = {
        "type": "turn_context",
        "payload": {"model": "gpt-6.1-sol", "effort": "high"},
    }
    COPILOT_START: ClassVar[JsonObject] = {
        "type": "session.start",
        "data": {"sessionId": "s", "selectedModel": "m", "reasoningEffort": "high"},
    }
    COPILOT_REPLY: ClassVar[JsonObject] = {"type": "assistant.message", "data": {}}

    def parse(self, client: str, *events: JsonObject) -> sessions.Parsed:
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "log.jsonl"
            log.write_text("\n".join(json.dumps(event) for event in events) + "\n")
            return sessions.parse(log, client)

    def test_01_every_valid_native_form_still_parses_to_strings(self) -> None:
        for client, events in (
            ("claude", [self.CLAUDE_OK]),
            ("codex", [self.CODEX_OK]),
            ("copilot", [self.COPILOT_START, self.COPILOT_REPLY]),
        ):
            with self.subTest(client):
                models, _, _, current = self.parse(client, *events)
                self.assertEqual(len(models), 1)
                for use in (*models, current):
                    assert use is not None
                    self.assertIsInstance(use["model"], str)
                    self.assertIsInstance(use["effort"], str)
        reasoning = {**self.CLAUDE_OK, "effort": None, "reasoning_effort": "low"}
        self.assertEqual(self.parse("claude", reasoning)[0][0]["effort"], "low")

    def test_02_a_model_or_effort_of_another_type_is_refused_naming_the_line(self) -> None:
        def claude(model: object = "m", effort: object = "high") -> tuple[str, list[JsonObject]]:
            return "claude", [{"type": "assistant", "message": {"model": model}, "effort": effort}]

        def codex(model: object = "m", effort: object = "high") -> tuple[str, list[JsonObject]]:
            return "codex", [
                {"type": "turn_context", "payload": {"model": model, "effort": effort}}
            ]

        def copilot_start(
            model: object = "m", effort: object = "high"
        ) -> tuple[str, list[JsonObject]]:
            start = {
                "type": "session.start",
                "data": {"selectedModel": model, "reasoningEffort": effort},
            }
            return "copilot", [start, self.COPILOT_REPLY]

        def copilot_change(**data: object) -> tuple[str, list[JsonObject]]:
            return "copilot", [
                self.COPILOT_START,
                {"type": "session.model_change", "data": data},
                self.COPILOT_REPLY,
            ]

        def copilot_choice(model: object, effort: object) -> tuple[str, list[JsonObject]]:
            choice = {
                "type": "user.message",
                "data": {"responsesReasoning": {"model": model, "effort": effort}},
            }
            return "copilot", [self.COPILOT_START, choice, self.COPILOT_REPLY]

        def copilot_reply(model: object) -> tuple[str, list[JsonObject]]:
            reply = {"type": "assistant.message", "data": {"model": model}}
            return "copilot", [self.COPILOT_START, reply]

        cases = [
            ("claude model 7", claude(model=7), "model"),
            ("claude effort true", claude(effort=True), "effort"),
            ("claude both (the reviewer's example)", claude(model=7, effort=True), "model"),
            ("claude model list", claude(model=["m"]), "model"),
            (
                "claude reasoning_effort",
                ("claude", [{**self.CLAUDE_OK, "effort": None, "reasoning_effort": 5}]),
                "reasoning_effort",
            ),
            ("codex model 7", codex(model=7), "model"),
            ("codex effort true", codex(effort=True), "effort"),
            ("codex both (the reviewer's example)", codex(model=7, effort=True), "model"),
            ("codex effort object", codex(effort={"a": 1}), "effort"),
            ("copilot selectedModel 7", copilot_start(model=7), "selectedModel"),
            ("copilot reasoningEffort true", copilot_start(effort=True), "reasoningEffort"),
            (
                "copilot both (the reviewer's example)",
                copilot_start(model=7, effort=True),
                "selectedModel",
            ),
            ("copilot newModel", copilot_change(newModel=7), "newModel"),
            (
                "copilot change effort",
                copilot_change(newModel="n", reasoningEffort=3),
                "reasoningEffort",
            ),
            ("copilot choice model", copilot_choice(7, "high"), "responsesReasoning.model"),
            ("copilot choice effort", copilot_choice("m", False), "responsesReasoning.effort"),
            ("copilot reply model", copilot_reply(7), "model"),
        ]
        for label, (client, events), field in cases:
            with self.subTest(label), self.assertRaises(RecordError) as caught:
                self.parse(client, *events)
            message = str(caught.exception)
            self.assertIn("must be a string", message)
            self.assertIn(field, message)
            self.assertRegex(message, r"at line \d+")
            self.assertIn(client, message)

    def test_03_an_event_that_leaves_them_out_keeps_its_old_behaviour(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "Unresolvable Claude effort"):
            self.parse("claude", {"type": "assistant", "message": {"model": "m"}})
        incomplete = {"type": "turn_context", "payload": {"model": "m"}}
        with self.assertRaisesRegex(RuntimeError, "unresolvable"):
            self.parse("codex", incomplete)
        models, _, _, current = self.parse("codex", self.CODEX_OK, incomplete)
        self.assertEqual((len(models), current), (1, None))


class Files(unittest.TestCase):
    def test_01_a_file_that_is_not_json_is_named(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "broken.json"
            path.write_text("{nope")
            with self.assertRaises(RecordError) as caught:
                records.read_json(path)
            self.assertIn(str(path), str(caught.exception))
            with self.assertRaises(RecordError) as missing:
                records.read_json(Path(directory) / "absent.json")
            self.assertIn("absent.json", str(missing.exception))

    def test_02_pointer_reading_names_a_pointer_that_is_not_an_object(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            hub = Path(directory)
            (hub / ".pipeline").mkdir()
            (hub / ".pipeline/accepted-v2.json").write_text("[]")
            with self.assertRaises(RuntimeError) as caught:
                pointer.read_pointer(hub)
            self.assertIn("accepted-v2.json", str(caught.exception))
            self.assertIn("JSON object", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
