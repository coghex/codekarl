"""CK-51: the guide helper (`tools/guide/ck_guide.py`), against a temporary fixture hub.

The fixture is a hub's `.pipeline` records (batches, a docs journal and receipts, queue manifests, an
accepted pointer and its checkpoint), a metadata directory, a docs copy, a review copy and a log file.
No real hub is touched, and the helper's boundary is tested directly: every fixture file is hashed
before and after each command, and `subprocess` and `os.system` are patched to fail.
"""

import ast
import contextlib
import functools
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from collections.abc import Callable
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools" / "guide"))
import ck_guide

REPO = Path(__file__).resolve().parents[1]
VISION = (
    "# Codekarl vision\n\n## Economy\n\n- **Firm.** The economy comes first.\n\n"
    "## Responsiveness\n\n- **Firm.** The UI never stutters.\n"
)
QUOTE = "The economy comes first."
ECONOMY = "# Economy\n\n## Prices\n\n- **Firm.** Prices follow the market.\n"
MILITARY = "# Military\n\n## Levies\n\n- **Firm.** Levies are raised by the crown.\n"
PRICES_QUOTE = "Prices follow the market."
LEVIES_QUOTE = "Levies are raised by the crown."
AUTHOR = {"identity": "ck-manager", "family": "claude", "role": "manager"}
CLOCK = 1_792_000_000.0


def sha(text: str | bytes) -> str:
    return hashlib.sha256(text.encode() if isinstance(text, str) else text).hexdigest()


def write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    return path


def objects(value: object) -> list[dict[str, object]]:
    """A JSON array of objects, checked, so the tests can index it under strict typing."""
    assert isinstance(value, list)
    found: list[dict[str, object]] = []
    for item in value:
        assert isinstance(item, dict)
        found.append(item)
    return found


def replace_key(key: str, value: object, coverage: dict[str, object]) -> None:
    coverage[key] = value


def tree_digests(*roots: Path) -> dict[str, str]:
    """The SHA-256 of every file under the roots, keyed by path."""
    found: dict[str, str] = {}
    for root in roots:
        for path in sorted(root.rglob("*")):
            if path.is_file():
                found[str(path)] = sha(path.read_bytes())
    return found


class World:
    """A fixture hub with a chain of publications, a metadata directory and the agent's copies."""

    def __init__(self, base: Path) -> None:
        self.base = base
        self.hub = base / "hub"
        self.meta = base / "meta"
        self.docs = base / "docscopy"
        self.copy = base / "copy"
        self.checkpoint = self.hub / ".pipeline" / "checkpoints" / "head"
        self.files = {"docs/vision.md": VISION, "src/lib.rs": "pub fn a() {}\n"}
        self.executable: set[str] = set()
        self.log = ["PREBASELINE1", "PREBASELINE2"]
        self.submissions: list[str] = []
        self.pointers = [self.pointer_text("genesis")]
        self.ids: list[str] = []
        self.sequence = 0
        (self.docs / "docs" / "guide").mkdir(parents=True)

    # --- pointers and publications -----------------------------------------------------------

    def pointer_text(self, label: str, batch: str | None = None) -> str:
        fingerprint = {
            "files": {
                name: {"executable": name in self.executable, "sha256": sha(text)}
                for name, text in self.files.items()
            },
            "hashes": list(self.log),
        }
        return json.dumps(
            {
                "fingerprint": fingerprint,
                "integration": {} if batch is None else {"batch": batch},
                "previous": None,
                "receipt": f"/receipts/{label}",
                "repo": str(self.checkpoint),
                "schema": 2,
                "submissions": list(self.submissions),
            },
            sort_keys=True,
            separators=(",", ":"),
        )

    @property
    def head(self) -> str:
        return self.pointers[-1]

    def manifest(self, member: str, patches: list[str], issues: tuple[str, ...]) -> Path:
        return write(
            self.hub / ".pipeline" / "queue" / f"{member}.json",
            json.dumps(
                {
                    "id": member,
                    "hashes": patches,
                    "issues": [{"id": i, "revision": "r1", "spec_sha256": "s"} for i in issues],
                },
                sort_keys=True,
            ),
        )

    def batch(
        self,
        identifier: str,
        patches: list[str],
        issues: tuple[str, ...] = ("CK-1",),
        rejected: tuple[list[str], tuple[str, ...]] | None = None,
    ) -> None:
        """A published batch: frozen.json, validated.json, publishing.json and published.json.

        `rejected` is a second submission the batch froze and then dropped at validation (a split
        batch): it is in frozen.json and its queue manifest, not in validated.json or the pointer.
        """
        previous = self.head
        member = f"{identifier}1"
        self.submissions.append(member)
        self.log.extend(patches)
        self.pointers.append(self.pointer_text(identifier, batch=identifier))
        manifest = self.manifest(member, patches, issues)
        frozen = [
            {
                "id": member,
                "manifest_sha256": sha(manifest.read_bytes()),
                "fingerprint_sha256": "f",
            }
        ]
        if rejected is not None:
            dropped = f"{identifier}2"
            dropped_manifest = self.manifest(dropped, rejected[0], rejected[1])
            frozen.append(
                {
                    "id": dropped,
                    "manifest_sha256": sha(dropped_manifest.read_bytes()),
                    "fingerprint_sha256": "f",
                }
            )
        directory = self.hub / ".pipeline" / "batches" / identifier
        write(
            directory / "frozen.json",
            json.dumps(
                {
                    "batch": identifier,
                    "base_pointer_sha256": sha(previous),
                    "members": frozen,
                }
            ),
        )
        write(
            directory / "validated.json",
            json.dumps(
                {
                    "base_pointer_sha256": sha(previous),
                    "members": [member],
                    "membership_manifest_sha256": [sha(manifest.read_bytes())],
                }
            ),
        )
        write(
            directory / "publishing.json",
            json.dumps({"pointer": self.head, "sha256": sha(self.head)}),
        )
        write(
            directory / "published.json",
            json.dumps({"basis": "single-submission", "pointer_sha256": sha(self.head)}),
        )
        self.ids.append(identifier)

    def docs_publication(
        self,
        identifier: str,
        queued: list[str],
        published: list[str],
        files: tuple[str, ...] = ("docs/note.md",),
    ) -> None:
        previous = self.head
        self.log.extend(published)
        self.pointers.append(self.pointer_text(identifier))
        fingerprint = json.loads(self.head)["fingerprint"]
        pipeline = self.hub / ".pipeline" / "docs"
        write(
            pipeline / "receipts" / f"{identifier}.json",
            json.dumps(
                {
                    "kind": "docs",
                    "id": identifier,
                    "fingerprint": fingerprint,
                    "hashes": published,
                    "previous": None,
                }
            ),
        )
        write(
            pipeline / f"{identifier}.json",
            json.dumps(
                {"id": identifier, "hashes": queued, "files": list(files), "state": "published"}
            ),
        )
        self.sequence += 1
        write(
            pipeline / "journal" / f"{self.sequence:08d}.json",
            json.dumps(
                {
                    "seq": self.sequence,
                    "entries": [identifier],
                    "prev": previous,
                    "prev_sha256": sha(previous),
                    "next": self.head,
                }
            ),
        )
        self.ids.append(identifier)

    def standard_chain(self) -> None:
        """B1 -> D1 -> B2 -> D2, the chain most tests walk."""
        self.batch("B1", ["PATCHB1"], ("CK-1", "CK-2"))
        self.docs_publication("D1", ["PATCHD1"], ["PATCHD1"], ("docs/vision.md",))
        self.batch("B2", ["PATCHB2"], ("CK-2",))
        self.docs_publication("D2", ["PATCHD2"], ["PATCHD2"])

    # --- the head, the copies and the metadata -------------------------------------------------

    def publish_head(self) -> None:
        """Write the accepted pointer, the checkpoint, the review copy and the log."""
        write(self.hub / ".pipeline" / "accepted-v2.json", self.head)
        for root in (self.checkpoint, self.copy):
            for name, text in self.files.items():
                write(root / name, text).chmod(0o755 if name in self.executable else 0o644)
        write(self.base / "log.txt", "\n".join(reversed(self.log)) + "\n")

    @property
    def log_file(self) -> Path:
        return self.base / "log.txt"

    def issue(
        self, number: int, title: str, state: str = "open", depends: list[str] | None = None
    ) -> None:
        identifier = f"CK-{number}"
        root = self.meta / "issues" / identifier
        spec = write(root / "spec" / "r1.md", f"# spec of {identifier}\n")
        fields = {
            "depends": depends or [],
            "priority": "P2",
            "state": state,
            "title": title,
            "type": "feature",
        }
        write(
            root / "0001-created.json",
            json.dumps(
                {
                    "at": 1,
                    "authors": [AUTHOR],
                    "fields": fields,
                    "id": identifier,
                    "kind": "created",
                    "spec_sha256": sha(spec.read_bytes()),
                },
                sort_keys=True,
            ),
        )

    def edit(self, number: int, fields: dict[str, object], kind: str = "edit") -> None:
        root = self.meta / "issues" / f"CK-{number}"
        count = len(list(root.glob("[0-9]*-*.json"))) + 1
        write(
            root / f"{count:04d}-{kind}.json",
            json.dumps(
                {
                    "at": count,
                    "authors": [AUTHOR],
                    "fields": fields,
                    "id": f"CK-{number}",
                    "kind": kind,
                },
                sort_keys=True,
            ),
        )

    def comment(self, number: int, text: str) -> None:
        root = self.meta / "issues" / f"CK-{number}" / "comments"
        write(root / f"{len(list(root.glob('*.md'))) + 1}.md", text)

    def spec(self, number: int, revision: int, text: str) -> None:
        write(self.meta / "issues" / f"CK-{number}" / "spec" / f"r{revision}.md", text)

    def review(self, identifier: str, number: int, verdict: str) -> None:
        write(
            self.meta / "reviews" / f"{identifier}.json",
            json.dumps(
                {
                    "id": identifier,
                    "kind": "spec",
                    "verdict": verdict,
                    "issues": [{"id": f"CK-{number}", "revision": "r1", "spec_sha256": "s"}],
                }
            ),
        )

    def standard_world(self) -> None:
        self.issue(1, "First", depends=[])
        self.issue(2, "Second", state="closed")
        self.standard_chain()
        self.publish_head()


def run_main(args: list[str], world: World, *, at: float = CLOCK) -> tuple[int, str, str]:
    """Run the helper's `main` on the world, returning (status, stdout, stderr)."""
    out, err = io.StringIO(), io.StringIO()
    full = ["--hub", str(world.hub), "--meta", str(world.meta), *args]
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        status = ck_guide.main(full, now=lambda: at)
    return status, out.getvalue(), err.getvalue()


class Fixture(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ck-guide-test-")
        self.addCleanup(self.temp.cleanup)
        self.world = World(Path(self.temp.name).resolve())
        self.baseline = sha(self.world.pointers[0])

    def pin(
        self, *extra: str, at: float = CLOCK, cursor: bool = False, baseline: str | None = None
    ) -> str:
        """Phase one of `start`: pins the accepted pointer and returns the run stem."""
        given = [] if cursor else ["--baseline", baseline or self.baseline]
        status, out, err = run_main(["start", *given, *extra], self.world, at=at)
        self.assertEqual(status, 0, err)
        found = re.search(r"pinned (\S+) at", out)
        stem = found.group(1) if found else ""
        self.assertRegex(stem, r"^[0-9]{8}t[0-9]{6}z-[0-9a-f]{8}$")
        self.assertIn(f"tools/ck worker guide-{stem}", out)
        return stem

    def resume(self, stem: str) -> tuple[int, str, str]:
        copy, log = str(self.world.copy), str(self.world.log_file)
        return run_main(["start", "--resume", stem, "--copy", copy, "--log", log], self.world)

    def start(
        self, *extra: str, at: float = CLOCK, cursor: bool = False, baseline: str | None = None
    ) -> tuple[str, Path]:
        """Both phases of `start`; returns the stem and the run directory."""
        stem = self.pin(*extra, at=at, cursor=cursor, baseline=baseline)
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 0, err)
        return stem, self.world.hub / ".pipeline" / "coordination" / "guide" / stem

    def refused(self, args: list[str], needle: str) -> str:
        status, _out, err = run_main(args, self.world)
        self.assertEqual(status, 1, err)
        self.assertIn(needle, err)
        return err

    def coverage(self, run: Path) -> dict[str, object]:
        data = json.loads((run / "coverage.json").read_text())
        assert isinstance(data, dict)
        return data

    def save(self, run: Path, coverage: dict[str, object]) -> None:
        (run / "coverage.json").write_text(json.dumps(coverage, indent=2, sort_keys=True))

    def judge(
        self,
        run: Path,
        verdicts: dict[str, str],
        *,
        findings: list[str] | None = None,
        principles: dict[str, object] | None = None,
    ) -> None:
        """Set every publication's verdict (and issues reviewed), as the agent would."""
        coverage = self.coverage(run)
        publications = coverage["publications"]
        assert isinstance(publications, list)
        for item in publications:
            assert isinstance(item, dict)
            verdict = verdicts.get(str(item["id"]), "reviewed")
            if item["verdict"] == "unreviewed" or str(item["id"]) in verdicts:
                item["verdict"] = verdict
                item["depth"] = "code/tests" if verdict == "reviewed" else ""
                item["note"] = "not looked at" if verdict == "gap" else item.get("note", "")
        issues = coverage["issues"]
        assert isinstance(issues, list)
        for item in issues:
            assert isinstance(item, dict)
            if item["verdict"] == "unreviewed":
                item.update({"verdict": "reviewed", "depth": "effective spec"})
        coverage["findings"] = findings or []
        coverage["principles"] = principles or {}
        self.save(run, coverage)

    def report(self, stem: str, headings: list[str] | None = None) -> None:
        body = "# Guide review\n\nStatus legend\n\n## Findings\n\n"
        for heading in headings or []:
            body += f"### {heading}\n\nA finding.\n\n"
        write(self.world.docs / "docs" / "guide" / f"{stem}.md", body)

    def finish(self, stem: str, *extra: str) -> tuple[int, str, str]:
        return run_main(
            ["finish", "--run", stem, "--docs-copy", str(self.world.docs), *extra],
            self.world,
            at=CLOCK + 100,
        )

    def publish_guide(self) -> None:
        """Stand-in for a docs publication: the docs copy's guide files reach the checkpoint."""
        for root in (self.world.checkpoint, self.world.copy):
            shutil.copytree(
                self.world.docs / "docs" / "guide", root / "docs" / "guide", dirs_exist_ok=True
            )

    def complete(self, stem: str, run: Path, verdicts: dict[str, str] | None = None) -> None:
        """Judge a run, write its report and finish it."""
        self.judge(run, verdicts or {})
        self.report(stem)
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 0, err)

    @property
    def cursor_path(self) -> Path:
        return self.world.docs / "docs" / "guide" / "CURSOR.md"


class Chain(Fixture):
    def setUp(self) -> None:
        super().setUp()
        self.world.standard_world()

    def test_01_a_correct_chain_is_walked_from_the_pin_to_the_baseline(self) -> None:
        stem, run = self.start()
        inventory = json.loads((run / "inventory.json").read_text())
        steps = [(item["kind"], item["id"]) for item in inventory["publications"]]
        self.assertEqual(steps, [("batch", "B1"), ("docs", "D1"), ("batch", "B2"), ("docs", "D2")])
        self.assertTrue(all(item["verdict"] == "unreviewed" for item in inventory["publications"]))
        self.assertEqual(inventory["pin"]["publication"], "D2")
        self.assertEqual(inventory["pin"]["pointer_sha256"], sha(self.world.head))
        self.assertEqual(inventory["baseline"]["pointer_sha256"], self.baseline)
        self.assertEqual(stem.split("-")[1], sha(self.world.head)[:8])
        # Patches from before the baseline have no publication record and are never refused.
        self.assertIn("PREBASELINE1", self.world.log_file.read_text())

    def test_02_a_missing_link_is_refused(self) -> None:
        (self.world.hub / ".pipeline/docs/journal/00000001.json").unlink()
        self.assert_start_refused("missing link")

    def test_03_two_records_producing_the_same_pointer_are_refused(self) -> None:
        journal = self.world.hub / ".pipeline/docs/journal"
        write(journal / "00000099.json", (journal / "00000001.json").read_text())
        self.assert_start_refused("two records produce")

    def test_04_a_stored_hash_that_does_not_match_its_bytes_is_refused(self) -> None:
        publishing = self.world.hub / ".pipeline/batches/B2/publishing.json"
        record = json.loads(publishing.read_text())
        record["pointer"] = record["pointer"].replace("B2", "B9")
        publishing.write_text(json.dumps(record))
        self.assert_start_refused("stored hash mismatch")

    def test_04b_a_manifest_whose_bytes_changed_is_refused(self) -> None:
        manifest = self.world.hub / ".pipeline/queue/B11.json"
        manifest.write_text(manifest.read_text() + " ")
        self.assert_start_refused("stored hash mismatch")

    def test_04c_a_docs_journal_row_with_a_wrong_previous_hash_is_refused(self) -> None:
        row = self.world.hub / ".pipeline/docs/journal/00000001.json"
        record = json.loads(row.read_text())
        record["prev_sha256"] = "0" * 64
        row.write_text(json.dumps(record))
        self.assert_start_refused("stored hash mismatch")

    def test_05_a_publication_patch_absent_from_the_log_is_refused(self) -> None:
        self.world.log_file.write_text(self.world.log_file.read_text().replace("PATCHB2\n", ""))
        self.assert_start_refused("PATCHB2 is absent from the given log")

    def test_06_an_unreachable_baseline_is_refused(self) -> None:
        self.assert_start_refused("is neither a recorded publication", baseline="f" * 64)
        stray_record = json.loads(self.world.pointer_text("stray"))
        stray_record["submissions"].append("X11")
        stray_record["integration"] = {"batch": "X1"}
        stray = json.dumps(stray_record, sort_keys=True, separators=(",", ":"))
        manifest = sha(self.world.manifest("X11", ["PATCHX1"], ("CK-1",)).read_bytes())
        directory = self.world.hub / ".pipeline/batches/X1"
        write(
            directory / "frozen.json",
            json.dumps(
                {
                    "batch": "X1",
                    "base_pointer_sha256": sha("nowhere"),
                    "members": [{"id": "X11", "manifest_sha256": manifest}],
                }
            ),
        )
        write(
            directory / "validated.json",
            json.dumps(
                {
                    "base_pointer_sha256": sha("nowhere"),
                    "members": ["X11"],
                    "membership_manifest_sha256": [manifest],
                }
            ),
        )
        write(directory / "publishing.json", json.dumps({"pointer": stray, "sha256": sha(stray)}))
        self.assert_start_refused("never reached", baseline=sha(stray))

    def assert_start_refused(self, needle: str, baseline: str | None = None) -> None:
        """Pin, then try to build the inventory with the fixture's baseline (or `baseline`)."""
        self.salted = getattr(self, "salted", 0) + 1
        chosen = baseline or self.baseline
        status, out, err = run_main(
            ["start", "--baseline", chosen], self.world, at=CLOCK + self.salted
        )
        self.assertEqual(status, 0, err)
        found = re.search(r"pinned (\S+) at", out)
        self.assertIsNotNone(found)
        stem = found.group(1) if found else ""
        self.refused(
            [
                "start",
                "--resume",
                stem,
                "--copy",
                str(self.world.copy),
                "--log",
                str(self.world.log_file),
            ],
            needle,
        )


class SplitBatch(Fixture):
    """A batch that froze two submissions and published one, as the real batch 1792958797582485000."""

    def setUp(self) -> None:
        super().setUp()
        self.world.issue(1, "Published")
        self.world.issue(3, "Rejected", state="closed")
        self.world.batch("S1", ["PATCHS1"], ("CK-1",), rejected=(["PATCHREJ"], ("CK-3",)))
        self.directory = self.world.hub / ".pipeline/batches/S1"

    def edit(self, name: str, change: Callable[[dict[str, object]], None]) -> None:
        path = self.directory / f"{name}.json"
        record = json.loads(path.read_text())
        change(record)
        path.write_text(json.dumps(record))

    def repoint(self, change: Callable[[dict[str, object]], None]) -> None:
        """Rewrite the published pointer (and every record of its hash) as a different pointer."""
        record = json.loads(self.world.head)
        change(record)
        text = json.dumps(record, sort_keys=True, separators=(",", ":"))
        self.world.pointers[-1] = text
        write(
            self.directory / "publishing.json", json.dumps({"pointer": text, "sha256": sha(text)})
        )
        self.edit("published", lambda marker: marker.update(pointer_sha256=sha(text)))

    def refuse(self, needle: str, *, drop: str = "") -> None:
        self.world.publish_head()
        if drop:
            self.world.log_file.write_text(self.world.log_file.read_text().replace(drop, ""))
        stem = self.pin()
        copy, log = str(self.world.copy), str(self.world.log_file)
        self.refused(["start", "--resume", stem, "--copy", copy, "--log", log], needle)

    def test_a_only_the_published_member_is_inventoried(self) -> None:
        self.world.publish_head()
        self.assertEqual(
            sorted(path.name for path in self.directory.glob("*.json")),
            ["frozen.json", "published.json", "publishing.json", "validated.json"],
        )
        frozen = json.loads((self.directory / "frozen.json").read_text())
        self.assertEqual([m["id"] for m in frozen["members"]], ["S11", "S12"])
        stem, run = self.start()
        inventory = json.loads((run / "inventory.json").read_text())
        (batch,) = inventory["publications"]
        self.assertEqual(batch["id"], "S1")
        self.assertEqual([m["id"] for m in batch["members"]], ["S11"])
        self.assertEqual(batch["hashes"], ["PATCHS1"])
        self.assertEqual([m["issues"] for m in batch["members"]], [["CK-1"]])
        self.assertNotIn("PATCHREJ", (run / "inventory.json").read_text())
        self.assertNotIn("CK-3", [issue["id"] for issue in inventory["issues"]])
        self.assertTrue(stem)

    def test_b_the_log_has_only_the_published_members_hashes(self) -> None:
        self.world.publish_head()
        self.assertNotIn("PATCHREJ", self.world.log_file.read_text())
        self.start()

    def test_c_removing_a_genuinely_published_patch_still_fails(self) -> None:
        self.refuse("PATCHS1 is absent from the given log", drop="PATCHS1\n")

    def test_d_a_rejected_patch_present_in_the_log_is_not_inventoried(self) -> None:
        self.world.publish_head()
        self.world.log_file.write_text(self.world.log_file.read_text() + "PATCHREJ\n")
        _stem, run = self.start()
        self.assertNotIn("PATCHREJ", (run / "inventory.json").read_text())

    def test_e_a_batch_without_validated_json_is_refused(self) -> None:
        (self.directory / "validated.json").unlink()
        self.refuse("validated.json is missing")

    def test_f_validated_names_a_submission_the_batch_never_froze(self) -> None:
        def change(record: dict[str, object]) -> None:
            record["members"] = ["S19"]

        self.edit("validated", change)
        self.refuse("S19, which frozen.json did not freeze")

    def test_g_validated_binds_another_manifest_than_the_frozen_one(self) -> None:
        self.edit("validated", lambda record: record.update(membership_manifest_sha256=["0" * 64]))
        self.refuse("differs from the frozen one")

    def test_h_validated_manifest_hashes_must_match_the_members(self) -> None:
        self.edit("validated", lambda record: record.update(membership_manifest_sha256=[]))
        self.refuse("1 members but 0 manifest hashes")

    def test_i_validated_for_another_base_is_refused(self) -> None:
        self.edit("validated", lambda record: record.update(base_pointer_sha256="1" * 64))
        self.refuse("validated.json is for base")

    def test_j_validated_must_list_a_member_once_and_at_least_one(self) -> None:
        self.edit(
            "validated", lambda record: record.update(members=[], membership_manifest_sha256=[])
        )
        self.refuse("lists no published member")

    def test_k_malformed_validated_membership_is_refused(self) -> None:
        self.edit("validated", lambda record: record.update(members="S11"))
        self.refuse("expected a JSON array")

    def test_l_the_pointer_must_end_with_the_validated_members(self) -> None:
        self.repoint(lambda record: record.update(submissions=["S12"]))
        self.refuse("do not end with the validated members S11")

    def test_m_a_rejected_member_in_the_published_pointer_is_refused(self) -> None:
        self.repoint(lambda record: record.update(submissions=["S12", "S11"]))
        self.refuse("submission S12 is in the published pointer but was not validated")

    def test_n_a_pointer_that_is_not_schema_2_is_refused(self) -> None:
        self.repoint(lambda record: record.update(schema=1))
        self.refuse("not a schema 2 pointer")

    def test_o_a_frozen_member_listed_twice_is_refused(self) -> None:
        def change(record: dict[str, object]) -> None:
            members = record["members"]
            assert isinstance(members, list)
            members.append(members[0])

        self.edit("frozen", change)
        self.refuse("lists submission S11 twice")

    def test_p_a_rejected_members_patches_published_later_belong_to_the_later_batch(self) -> None:
        self.world.batch("S2", ["PATCHREJ"], ("CK-3",))
        self.world.publish_head()
        self.assertIn("PATCHREJ", self.world.log_file.read_text())
        _stem, run = self.start()
        inventory = json.loads((run / "inventory.json").read_text())
        found = {item["id"]: item for item in inventory["publications"]}
        self.assertEqual(list(found), ["S1", "S2"])
        self.assertEqual(found["S1"]["hashes"], ["PATCHS1"])
        self.assertEqual([m["issues"] for m in found["S1"]["members"]], [["CK-1"]])
        self.assertEqual(found["S2"]["hashes"], ["PATCHREJ"])
        self.assertEqual([m["issues"] for m in found["S2"]["members"]], [["CK-3"]])
        self.assertIn("CK-3", [issue["id"] for issue in inventory["issues"]])

    def test_q_the_rejected_issue_is_not_in_range_before_the_later_batch(self) -> None:
        self.world.batch("S2", ["PATCHREJ"], ("CK-3",))
        self.world.publish_head()
        _stem, run = self.start("--until", "S1")
        inventory = json.loads((run / "inventory.json").read_text())
        self.assertNotIn("CK-3", [issue["id"] for issue in inventory["issues"]])

    def test_r_a_pointer_integration_that_names_another_batch_is_refused(self) -> None:
        self.repoint(lambda record: record.update(integration={"batch": "S9"}))
        self.refuse("integration names batch 'S9', not S1")


class PublicPaths(Fixture):
    """The reports are tracked and mirrored: they record no absolute path into the user's home."""

    def setUp(self) -> None:
        super().setUp()
        self.world.standard_world()
        # The fixture's copies, log and hub sit under this temporary "home"
        self.real_home = Path.home()
        patch = mock.patch.dict(os.environ, {"HOME": str(self.world.base)})
        patch.start()
        self.addCleanup(patch.stop)

    def test_a_the_inventory_and_coverage_record_hub_relative_and_home_relative_paths(self) -> None:
        _stem, run = self.start()
        inventory = json.loads((run / "inventory.json").read_text())
        self.assertEqual(inventory["pin"]["repo"], ".pipeline/checkpoints/head")
        self.assertEqual(inventory["review_copy"]["path"], "~/copy")
        self.assertEqual(inventory["review_copy"]["log"], "~/log.txt")
        self.assertEqual(self.coverage(run)["pin"], inventory["pin"])
        self.assertEqual(self.coverage(run)["review_copy"], inventory["review_copy"])

    def test_b_a_finished_report_holds_the_home_path_nowhere(self) -> None:
        stem, run = self.start()
        self.complete(stem, run)
        report = self.world.docs / "docs" / "guide" / f"{stem}.coverage.md"
        text = report.read_text()
        self.assertIn(".pipeline/checkpoints/head", text)
        self.assertNotIn(str(self.world.base), text)
        self.assertNotIn(str(self.real_home), text)

    def test_c_a_path_outside_the_hub_and_the_home_is_kept_as_given(self) -> None:
        hub = ck_guide.Hub(self.world.hub, self.world.meta)
        with tempfile.TemporaryDirectory(prefix="ck-guide-outside-") as outside:
            elsewhere = Path(outside).resolve() / "log.txt"
            with mock.patch.dict(os.environ, {"HOME": str(Path(outside).resolve() / "other")}):
                self.assertEqual(ck_guide.display_path(hub, elsewhere), str(elsewhere))
                self.assertEqual(ck_guide.display_path(hub, self.world.hub), ".")

    def test_d_a_path_that_leaves_the_home_is_recorded_resolved_never_as_given(self) -> None:
        hub = ck_guide.Hub(self.world.hub, self.world.meta)
        home = self.world.base
        outside = home.parent / f"{home.name}-outside"
        (outside / "copy").mkdir(parents=True)
        (home / "link").symlink_to(outside)
        for given in (home / ".." / outside.name / "copy", home / "link" / "copy"):
            shown = ck_guide.display_path(hub, given)
            self.assertEqual(shown, str(outside / "copy"))
            self.assertNotIn("..", shown)
            self.assertNotIn(f"{home}/", shown)
        shutil.rmtree(outside)


class Replay(Fixture):
    """Docs publications record queued hashes, and the receipt the hashes actually published."""

    def build(self, queued: list[str], published: list[str]) -> None:
        self.world.issue(1, "First")
        self.world.batch("B1", ["PATCHB1"])
        self.world.docs_publication("D1", queued, published)
        self.world.publish_head()

    def test_a_replayed_publication_uses_the_receipts_hashes(self) -> None:
        self.build(["QUEUEDHASH"], ["REPLAYEDHASH"])
        _stem, run = self.start()
        inventory = json.loads((run / "inventory.json").read_text())
        docs = inventory["publications"][1]
        self.assertEqual(docs["hashes"], ["REPLAYEDHASH"])
        self.assertEqual(docs["queued_hashes"], ["QUEUEDHASH"])
        self.assertNotIn("QUEUEDHASH", self.world.log_file.read_text())

    def test_b_a_published_hash_absent_from_the_log_is_refused(self) -> None:
        self.build(["QUEUEDHASH"], ["REPLAYEDHASH"])
        self.world.log_file.write_text(
            self.world.log_file.read_text().replace("REPLAYEDHASH\n", "")
        )
        stem = self.pin()
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 1)
        self.assertIn("REPLAYEDHASH is absent from the given log", err)

    def test_c_a_missing_receipt_is_refused(self) -> None:
        self.build(["Q"], ["Q"])
        (self.world.hub / ".pipeline/docs/receipts/D1.json").unlink()
        stem = self.pin()
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 1)
        self.assertIn("receipt of D1 is missing", err)

    def test_d_a_receipt_without_hashes_is_refused(self) -> None:
        self.build(["Q"], ["Q"])
        path = self.world.hub / ".pipeline/docs/receipts/D1.json"
        record = json.loads(path.read_text())
        del record["hashes"]
        path.write_text(json.dumps(record))
        stem = self.pin()
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 1)
        self.assertIn("missing `hashes`", err)

    def test_e_a_receipt_fingerprint_that_differs_from_the_journal_is_refused(self) -> None:
        self.build(["Q"], ["Q"])
        path = self.world.hub / ".pipeline/docs/receipts/D1.json"
        record = json.loads(path.read_text())
        record["fingerprint"] = {"files": {}, "hashes": ["OTHER"]}
        path.write_text(json.dumps(record))
        stem = self.pin()
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 1)
        self.assertIn("fingerprint that differs", err)


class Issues(Fixture):
    """Issue fingerprints: which changes make an issue reviewable again, and which keep its verdict."""

    def setUp(self) -> None:
        super().setUp()
        self.world.standard_world()
        stem, run = self.start()
        self.complete(stem, run)
        self.publish_guide()
        self.stems = [stem]

    def second(self) -> dict[str, dict[str, object]]:
        """A new run over the same pin, continuing from the cursor: its issue entries by ID."""
        stem, run = self.start(at=CLOCK + 1000 * (len(self.stems) + 1), cursor=True)
        self.stems.append(stem)
        inventory = json.loads((run / "inventory.json").read_text())
        self.assertEqual(inventory["publications"], [])
        return {str(item["id"]): item for item in inventory["issues"]}

    def test_a_an_unchanged_issue_keeps_its_verdict_and_hashes_are_stable(self) -> None:
        first = self.second()["CK-1"]
        second = self.second()["CK-1"]
        self.assertEqual(first["verdict"], "inherited")
        self.assertEqual(first["change"], "unchanged")
        self.assertEqual(first["definition_sha256"], second["definition_sha256"])
        self.assertEqual(first["state_sha256"], second["state_sha256"])
        self.assertRegex(str(first["definition_sha256"]), r"^[0-9a-f]{64}$")

    def test_b_a_new_spec_revision_is_reviewed_again(self) -> None:
        self.world.spec(1, 2, "# a different spec\n")
        entry = self.second()["CK-1"]
        self.assertEqual((entry["change"], entry["verdict"]), ("changed", "unreviewed"))

    def test_c_a_comment_alone_is_reviewed_again(self) -> None:
        self.world.comment(1, "an owner clarification\n")
        entry = self.second()["CK-1"]
        self.assertEqual((entry["change"], entry["verdict"]), ("changed", "unreviewed"))

    def test_d_a_title_alone_is_reviewed_again(self) -> None:
        self.world.edit(1, {"title": "Renamed"})
        entry = self.second()["CK-1"]
        self.assertEqual((entry["change"], entry["verdict"]), ("changed", "unreviewed"))

    def test_e_a_dependency_alone_is_reviewed_again(self) -> None:
        self.world.edit(1, {"depends": ["CK-2"]})
        entry = self.second()["CK-1"]
        self.assertEqual((entry["change"], entry["verdict"]), ("changed", "unreviewed"))

    def test_f_a_state_only_change_keeps_the_verdict(self) -> None:
        self.world.edit(1, {"priority": "P1"})
        entry = self.second()["CK-1"]
        self.assertEqual((entry["change"], entry["verdict"]), ("unchanged", "inherited"))
        self.assertIs(entry["state_changed"], True)

    def test_g_a_review_receipt_is_state_not_definition(self) -> None:
        before = self.second()["CK-1"]
        self.world.review("R1", 1, "approved")
        after = self.second()["CK-1"]
        self.assertEqual(before["definition_sha256"], after["definition_sha256"])
        self.assertNotEqual(before["state_sha256"], after["state_sha256"])
        self.assertEqual(after["verdict"], "inherited")

    def test_h_a_closed_issue_is_listed_only_when_the_range_touched_it(self) -> None:
        self.assertNotIn("CK-2", self.second())
        self.world.batch("B3", ["PATCHB3"], ("CK-2",))
        self.world.publish_head()
        stem = self.pin(at=CLOCK + 9000, cursor=True)
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 0, err)
        inventory = json.loads(
            (self.world.hub / ".pipeline/coordination/guide" / stem / "inventory.json").read_text()
        )
        self.assertEqual([item["id"] for item in inventory["publications"]], ["B3"])
        self.assertIn("CK-2", [item["id"] for item in inventory["issues"]])


class ClosedIssues(Fixture):
    """CK-54: a closed issue an earlier report covered re-enters the inventory when its definition
    changes, even though no publication in the range touches it."""

    def setUp(self) -> None:
        super().setUp()
        self.world.standard_world()
        stem, run = self.start()
        self.complete(stem, run)  # covers CK-1 (open) and CK-2 (closed, touched by B1 and B2)
        self.publish_guide()
        self.calls = 0

    def status(self) -> str:
        status, out, err = run_main(["status"], self.world)
        self.assertEqual(status, 0, err)
        return out

    def next_run(self) -> tuple[str, Path]:
        self.calls += 1
        return self.start(at=CLOCK + 1000 * (self.calls + 1), cursor=True)

    def inventory(self, run: Path) -> dict[str, dict[str, object]]:
        data = json.loads((run / "inventory.json").read_text())
        return {str(item["id"]): item for item in data["issues"]}

    def test_a_each_kind_of_amendment_makes_the_closed_issue_pending_and_part_of_the_next_inventory(
        self,
    ) -> None:
        amendments: dict[str, Callable[[], None]] = {
            "comment": lambda: self.world.comment(2, "The owner changed the requirement.\n"),
            "spec": lambda: self.world.spec(2, 2, "# a new revision of the spec\n"),
            "title": lambda: self.world.edit(2, {"title": "Second, renamed"}),
            "dependency": lambda: self.world.edit(2, {"depends": ["CK-1"]}),
        }
        for kind, amend in amendments.items():
            with self.subTest(amendment=kind):
                self.restore_ck2()
                amend()
                self.assertIn("CK-2 (definition changed since it was covered)", self.status())
                _stem, run = self.next_run()
                entry = self.inventory(run)["CK-2"]
                self.assertEqual((entry["change"], entry["verdict"]), ("changed", "unreviewed"))
                self.assertEqual(entry["state"], "closed")

    def restore_ck2(self) -> None:
        """Back to the state after the first run, for the next subtest."""
        shutil.rmtree(self.world.meta / "issues" / "CK-2")
        self.world.issue(2, "Second", state="closed")

    def test_b_controls_unchanged_state_only_and_never_covered_stay_out(self) -> None:
        # Unchanged.
        self.assertNotIn("CK-2", self.status())
        self.assertNotIn("CK-2", self.inventory(self.next_run()[1]))
        # A state-only change (a close reason) is not a re-review.
        self.world.edit(2, {"reason": "done"})
        self.assertNotIn("CK-2", self.status())
        self.assertNotIn("CK-2", self.inventory(self.next_run()[1]))
        # A closed issue that was never covered.
        self.world.issue(3, "Third", state="closed")
        self.world.comment(3, "late words\n")
        self.assertNotIn("CK-3", self.status())
        self.assertNotIn("CK-3", self.inventory(self.next_run()[1]))

    def test_c_reviewing_it_clears_status_and_changes_nothing_else_or_outside_the_boundary(
        self,
    ) -> None:
        first = {
            path.name: path.read_bytes()
            for path in (self.world.docs / "docs" / "guide").glob("*.coverage.md")
        }
        self.world.comment(2, "The owner changed the requirement.\n")
        stem, run = self.next_run()
        before = tree_digests(self.world.hub, self.world.meta, self.world.copy)
        before = {k: v for k, v in before.items() if "/coordination/guide/" not in k}
        entries = self.inventory(run)
        self.assertEqual(entries["CK-1"]["verdict"], "inherited")
        self.judge(run, {})
        self.report(stem)
        self.assertEqual(self.finish(stem)[0], 0)
        after = tree_digests(self.world.hub, self.world.meta, self.world.copy)
        after = {k: v for k, v in after.items() if "/coordination/guide/" not in k}
        self.assertEqual(after, before, "nothing outside the run directory and the docs copy")
        self.publish_guide()
        self.assertNotIn("CK-2", self.status())
        # The earlier coverage records are untouched, and CK-1's entry in the new one is the old one's.
        for name, data in first.items():
            self.assertEqual((self.world.docs / "docs" / "guide" / name).read_bytes(), data)
        coverage = json.loads((run / "coverage.final.json").read_text())
        new = {item["id"]: item for item in coverage["issues"]}
        self.assertEqual(new["CK-2"]["verdict"], "reviewed")
        self.assertEqual(new["CK-1"]["verdict"], "inherited")
        old = next(iter(first.values())).decode()
        self.assertIn(str(new["CK-1"]["definition_sha256"]), old)
        self.assertIn(str(new["CK-1"]["state_sha256"]), old)
        # The next run does not list it again.
        self.assertNotIn("CK-2", self.inventory(self.next_run()[1]))

    def test_d_the_guide_1_probe_scenario(self) -> None:
        """ck-guide's closed-issue-probe.py: status flagged CK-2 but the next inventory left it out."""
        self.world.comment(2, "Owner changes the accepted requirement after closure.\n")
        out = self.status()
        entries = self.inventory(self.next_run()[1])
        self.assertIn("CK-2 (definition changed since it was covered)", out)
        self.assertIn("CK-2", entries)

    def test_f_a_covered_closed_issue_reopened_during_the_run_is_a_state_notice_only(self) -> None:
        """The reviewer's differential case: CK-2 is omitted at start (unchanged), then reopened with no
        definition change. `recheck` must still report it, as a state notice, not a definition change."""
        stem, run = self.next_run()
        self.assertNotIn("CK-2", self.inventory(run), "unchanged and closed at start, so omitted")
        self.world.edit(2, {"state": "open"})
        status, out, err = run_main(["recheck", "--run", stem], self.world)
        self.assertEqual(status, 0, err)
        pending = [(item["id"], item["change"]) for item in objects(self.coverage(run)["pending"])]
        self.assertIn(("CK-2", "state"), pending)
        self.assertNotIn(("CK-2", "definition"), pending)
        self.assertIn("1 item(s) moved since the pin", out)
        # The next inventory includes it (it is open), keeps its inherited verdict (the definition is
        # unchanged) and records that its state changed: no forced re-review.
        entry = self.inventory(self.next_run()[1])["CK-2"]
        self.assertEqual((entry["change"], entry["verdict"]), ("unchanged", "inherited"))
        self.assertIs(entry["state_changed"], True)

    def test_g_a_still_closed_state_only_change_and_a_never_covered_open_issue(self) -> None:
        stem, run = self.next_run()
        self.world.edit(2, {"reason": "done"})  # still closed: state only, not reported
        self.world.issue(4, "Fourth")  # open and never covered: new
        status, _out, err = run_main(["recheck", "--run", stem], self.world)
        self.assertEqual(status, 0, err)
        pending = [(item["id"], item["change"]) for item in objects(self.coverage(run)["pending"])]
        self.assertNotIn("CK-2", [identifier for identifier, _change in pending])
        self.assertIn(("CK-4", "new"), pending)

    def test_e_recheck_reports_a_covered_closed_issue_that_changes_during_the_run(self) -> None:
        stem, run = self.next_run()
        self.assertNotIn("CK-2", self.inventory(run), "unchanged at start, so omitted")
        self.world.comment(2, "A change during the run.\n")
        self.world.edit(1, {"priority": "P1"})  # state only: not a pending definition change
        self.world.issue(3, "Third", state="closed")  # never covered
        status, out, err = run_main(["recheck", "--run", stem], self.world)
        self.assertEqual(status, 0, err)
        pending = [(item["id"], item["change"]) for item in objects(self.coverage(run)["pending"])]
        self.assertIn(("CK-2", "definition"), pending)
        self.assertIn(("CK-1", "state"), pending)
        self.assertNotIn("CK-3", [identifier for identifier, _change in pending])
        self.assertIn("CK-2", out)


class Runs(Fixture):
    """Full, partial, bounded and review-only runs, and what each does to the cursor."""

    def setUp(self) -> None:
        super().setUp()
        self.world.standard_world()

    def resume_line(self) -> str:
        found = re.search(r"^Resume after: .*$", self.cursor_path.read_text(), re.M)
        return found.group(0) if found else ""

    def test_a_a_full_run_moves_the_boundary_to_the_pin(self) -> None:
        stem, run = self.start()
        self.complete(stem, run)
        line = self.resume_line()
        self.assertIn("`D2`", line)
        self.assertIn("patch `PATCHD2`", line)
        self.assertIn(f"pointer `{sha(self.world.head)}`", line)
        self.assertIn(f"[{stem}]({stem}.md)", line)
        coverage = (self.world.docs / "docs/guide" / f"{stem}.coverage.md").read_text()
        self.assertIn("```json", coverage)

    def test_b_a_gap_stops_the_boundary_and_is_listed_beyond_it(self) -> None:
        stem, run = self.start()
        self.complete(stem, run, {"B2": "gap"})
        text = self.cursor_path.read_text()
        self.assertIn("Resume after: `D1`", text)
        self.assertIn("Covered beyond the boundary: D2 (", text)
        self.assertIn("publication B2 is after the boundary", text)

    def test_c_until_marks_later_publications_outside_the_range(self) -> None:
        stem, run = self.start("--until", "B1")
        coverage = self.coverage(run)
        verdicts = {str(item["id"]): item["verdict"] for item in objects(coverage["publications"])}
        self.assertEqual(
            verdicts,
            {
                "B1": "unreviewed",
                "D1": "outside-range",
                "B2": "outside-range",
                "D2": "outside-range",
            },
        )
        self.complete(stem, run)
        self.assertIn("Resume after: `B1`", self.cursor_path.read_text())

    def test_d_until_must_name_a_publication_in_the_range(self) -> None:
        stem = self.pin("--until", "NOPE")
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 1)
        self.assertIn("--until NOPE is not a publication", err)

    def test_e_a_review_only_run_leaves_the_cursor_byte_identical_and_unmoved(self) -> None:
        stem, run = self.start()
        self.complete(stem, run)
        self.publish_guide()
        before = self.cursor_path.read_bytes()
        self.world.batch("B3", ["PATCHB3"])
        self.world.publish_head()
        self.publish_guide()
        stem2, run2 = self.start("--mode", "review-only", at=CLOCK + 5000, cursor=True)
        self.judge(run2, {})
        self.report(stem2)
        status, out, err = self.finish(stem2, "--review-only")
        self.assertEqual(status, 0, err)
        self.assertIn("CURSOR.md is untouched", out)
        self.assertEqual(self.cursor_path.read_bytes(), before)
        self.publish_guide()
        # A later full run still lists B3: review-only coverage is not inherited.
        _stem3, run3 = self.start(at=CLOCK + 9000, cursor=True)
        inventory = json.loads((run3 / "inventory.json").read_text())
        self.assertEqual(
            [(i["id"], i["verdict"]) for i in inventory["publications"]], [("B3", "unreviewed")]
        )

    def test_f_a_scoped_run_covers_only_its_scope(self) -> None:
        stem, run = self.start("--mode", "scoped", "--scope", "CK-1")
        self.judge(run, {})
        self.report(stem)
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 1)
        self.assertIn("a scoped run covers only its scope", err)
        coverage = self.coverage(run)
        for kind in ("publications", "issues"):
            for item in objects(coverage[kind]):
                if item["id"] != "CK-1":
                    item.update({"verdict": "outside-range", "depth": ""})
        self.save(run, coverage)
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 0, err)
        # It placed no publication, so the boundary stays at the baseline.
        self.assertIn(self.baseline, self.cursor_path.read_text())

    def test_g_a_scoped_run_needs_a_scope(self) -> None:
        status, _out, err = run_main(
            ["start", "--baseline", self.baseline, "--mode", "scoped"], self.world
        )
        self.assertEqual(status, 1)
        self.assertIn("needs --scope", err)

    def test_h_start_without_a_cursor_or_baseline_is_refused(self) -> None:
        stem = self.pin(cursor=True)
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 1)
        self.assertIn("no cursor boundary and no --baseline", err)

    def test_i_status_reports_the_cursor_and_pending_publications(self) -> None:
        status, out, err = run_main(["status"], self.world)
        self.assertEqual(status, 0, err)
        self.assertIn("cursor: none", out)
        stem, run = self.start("--until", "B1")
        self.complete(stem, run)
        self.publish_guide()
        status, out, err = run_main(["status"], self.world)
        self.assertEqual(status, 0, err)
        self.assertIn("cursor boundary: B1", out)
        self.assertIn("pending publications after the boundary: 3", out)
        self.assertIn("docs D1", out)


class Concurrency(Fixture):
    """The pin never moves; what changed after it is recorded as pending."""

    def setUp(self) -> None:
        super().setUp()
        self.world.standard_world()

    def test_recheck_records_later_publications_and_issue_changes_without_moving_the_pin(
        self,
    ) -> None:
        stem, run = self.start()
        self.judge(run, {})
        pin = json.loads((run / "run.json").read_text())["pointer_sha256"]
        judged = json.dumps(self.coverage(run)["publications"], sort_keys=True)
        self.world.batch("B3", ["PATCHB3"])
        self.world.publish_head()
        self.world.issue(3, "Third")
        self.world.edit(1, {"title": "Renamed"})
        status, out, err = run_main(["recheck", "--run", stem], self.world)
        self.assertEqual(status, 0, err)
        pending = self.coverage(run)["pending"]
        kinds = {(item["kind"], item.get("id")) for item in objects(pending)}
        self.assertIn(("publication", "B3"), kinds)
        self.assertIn(("issue", "CK-3"), kinds)
        self.assertIn(("issue", "CK-1"), kinds)
        self.assertEqual(json.loads((run / "run.json").read_text())["pointer_sha256"], pin)
        self.assertEqual(json.dumps(self.coverage(run)["publications"], sort_keys=True), judged)
        self.assertIn("recorded as pending; the pin is unchanged", out)
        self.report(stem)
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 0, err)
        self.assertIn("publication B3 is after the boundary", self.cursor_path.read_text())


class Cursor(Fixture):
    def setUp(self) -> None:
        super().setUp()
        self.world.standard_world()

    def test_a_rebuild_cursor_equals_the_one_finish_wrote(self) -> None:
        stem, run = self.start()
        self.complete(stem, run, {"B2": "gap"})
        written = self.cursor_path.read_text()
        status, _out, err = run_main(
            ["rebuild-cursor", "--docs-copy", str(self.world.docs)], self.world
        )
        self.assertEqual(status, 0, err)
        self.assertEqual(self.cursor_path.read_text(), written)
        self.cursor_path.unlink()
        status, _out, err = run_main(
            ["rebuild-cursor", "--docs-copy", str(self.world.docs)], self.world
        )
        self.assertEqual(status, 0, err)
        self.assertEqual(self.cursor_path.read_text(), written)

    def test_b_an_older_run_finishing_later_does_not_move_the_boundary_back(self) -> None:
        older, older_run = self.start("--until", "B1", at=CLOCK)
        newer, newer_run = self.start(at=CLOCK + 10)
        self.judge(older_run, {})
        self.report(older)
        self.judge(newer_run, {})
        self.report(newer)
        status, _out, err = self.finish(newer)
        self.assertEqual(status, 0, err)
        self.assertIn("Resume after: `D2`", self.cursor_path.read_text())
        status, _out, err = self.finish(older)
        self.assertEqual(status, 0, err)
        self.assertIn("Resume after: `D2`", self.cursor_path.read_text())

    def test_b2_a_later_started_shorter_run_does_not_move_the_boundary_back(self) -> None:
        full, full_run = self.start(at=CLOCK)
        shorter, shorter_run = self.start("--until", "B1", at=CLOCK + 10)
        for stem, run in ((full, full_run), (shorter, shorter_run)):
            self.judge(run, {})
            self.report(stem)
            self.assertEqual(self.finish(stem)[0], 0)
        self.assertIn("Resume after: `D2`", self.cursor_path.read_text())
        # Rebuilt from the reports alone (no kept cursor to fall back on), the furthest boundary wins.
        self.cursor_path.unlink()
        rebuild = ["rebuild-cursor", "--docs-copy", str(self.world.docs)]
        self.assertEqual(run_main(rebuild, self.world)[0], 0)
        self.assertIn("Resume after: `D2`", self.cursor_path.read_text())

    def test_c_a_report_on_a_diverging_chain_is_noted_and_not_used(self) -> None:
        elsewhere = {
            "schema": ck_guide.SCHEMA,
            "stem": "20260101t000000z-deadbeef",
            "mode": "full",
            "completed_at": "2026-01-01T00:00:00Z",
            "boundary": {"id": "X9", "patch": "", "pointer_sha256": "e" * 64},
            "publications": [],
            "issues": [],
            "principles": {},
        }
        write(
            self.world.docs / "docs/guide/20260101t000000z-deadbeef.coverage.md",
            "# old\n\n```json\n" + json.dumps(elsewhere) + "\n```\n",
        )
        stem, run = self.start()
        self.complete(stem, run)
        text = self.cursor_path.read_text()
        self.assertIn("Resume after: `D2`", text)
        self.assertIn("20260101t000000z-deadbeef is on a diverging chain", text)

    def test_d_open_findings_come_from_report_headings(self) -> None:
        stem, run = self.start()
        self.judge(run, {}, findings=["GUIDE-1", "GUIDE-2"])
        self.report(
            stem, ["[#99] GUIDE-1. Tracked elsewhere", "[deferred] GUIDE-2. Waiting for the owner"]
        )
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 0, err)
        text = self.cursor_path.read_text()
        self.assertIn(f"- {stem}/GUIDE-2 — Waiting for the owner — deferred", text)
        self.assertNotIn("GUIDE-1 —", text)

    def test_e_readings_carry_forward_from_the_latest_exercising_run(self) -> None:
        principle = {
            "section": "## Economy",
            "quote": QUOTE,
            "sha256": sha(QUOTE),
            "reading": "drifting",
            "note": "first",
        }
        stem, run = self.start("--until", "B1")
        self.judge(run, {}, principles={"vision.economy": principle})
        self.report(stem)
        self.assertEqual(self.finish(stem)[0], 0)
        self.publish_guide()
        stem2, run2 = self.start(at=CLOCK + 1000, cursor=True)
        later = dict(principle, reading="not exercised", note="")
        self.judge(run2, {}, principles={"vision.economy": later})
        self.report(stem2)
        self.assertEqual(self.finish(stem2)[0], 0)
        self.assertIn("| vision.economy | drifting |", self.cursor_path.read_text())


class Validation(Fixture):
    def setUp(self) -> None:
        super().setUp()
        self.world.standard_world()
        self.stem, self.run_dir = self.start()
        self.principle = {
            "section": "## Economy",
            "quote": QUOTE,
            "sha256": sha(QUOTE),
            "reading": "aligned",
            "note": "",
        }

    def invalid(self, needle: str, *, headings: list[str] | None = None) -> None:
        self.report(self.stem, headings)
        status, _out, err = self.finish(self.stem)
        self.assertEqual(status, 1, err)
        self.assertIn(needle, err)
        self.assertFalse((self.world.docs / "docs/guide/CURSOR.md").exists())
        status, _out, err = run_main(
            ["validate", "--run", self.stem, "--docs-copy", str(self.world.docs)], self.world
        )
        self.assertEqual(status, 1)
        self.assertIn(needle, err)

    def test_a_a_valid_judgment_finishes_and_validates(self) -> None:
        self.judge(self.run_dir, {}, principles={"vision.economy": self.principle})
        self.report(self.stem)
        status, out, err = run_main(
            ["validate", "--run", self.stem, "--docs-copy", str(self.world.docs)], self.world
        )
        self.assertEqual(status, 0, err)
        self.assertIn("coverage is valid", out)

    def test_b_unreviewed_items_are_refused(self) -> None:
        self.invalid("is still unreviewed")

    def test_c_a_gap_needs_a_reason(self) -> None:
        self.judge(self.run_dir, {"B2": "gap"})
        coverage = self.coverage(self.run_dir)
        for item in objects(coverage["publications"]):
            item["note"] = ""
        self.save(self.run_dir, coverage)
        self.invalid("is a gap without a reason")

    def test_d_a_reviewed_item_needs_a_depth(self) -> None:
        self.judge(self.run_dir, {})
        coverage = self.coverage(self.run_dir)
        objects(coverage["publications"])[0]["depth"] = ""
        self.save(self.run_dir, coverage)
        self.invalid("is reviewed without a `depth`")

    def test_e_a_missing_verdict_is_refused(self) -> None:
        self.judge(self.run_dir, {})
        coverage = self.coverage(self.run_dir)
        del objects(coverage["publications"])[0]["verdict"]
        self.save(self.run_dir, coverage)
        self.invalid("has no verdict")

    def test_f_principle_keys_and_pinned_text(self) -> None:
        for change, needle in (
            ({"key": "economy"}, "is not of the form `vision.some-name`"),
            ({"quote": "Invented words."}, "the quote is not in docs/vision.md at the pin"),
            ({"sha256": "0" * 64}, "is not the SHA-256 of its quote"),
            ({"section": "## Nowhere"}, "is not in docs/vision.md at the pin"),
            ({"reading": "excellent"}, "reading must be one of"),
            ({"reading": ""}, "has no pinned `reading`"),
        ):
            with self.subTest(needle=needle):
                entry = dict(self.principle)
                key = str(change.pop("key", "vision.economy"))
                entry.update(change)
                self.judge(self.run_dir, {}, principles={key: entry})
                self.invalid(needle)

    def test_g_finding_ids_and_report_headings_must_agree_both_ways(self) -> None:
        self.judge(self.run_dir, {}, findings=["GUIDE-1"])
        self.invalid("finding GUIDE-1 is in the coverage but has no report heading")
        self.judge(self.run_dir, {}, findings=[])
        self.invalid(
            "has a heading for GUIDE-2 that the coverage does not list",
            headings=["[deferred] GUIDE-2. Something"],
        )

    def test_h_a_run_is_not_started_twice_over_its_judgments(self) -> None:
        status, _out, err = self.resume(self.stem)
        self.assertEqual(status, 1)
        self.assertIn("already has coverage.json", err)

    def test_i_an_unknown_or_malformed_run_stem_is_refused(self) -> None:
        for stem in ("nonsense", "20260101t000000z-abcdef01"):
            status, _out, err = run_main(["recheck", "--run", stem], self.world)
            self.assertEqual(status, 1)
            self.assertIn("ck-guide:", err)


class Binding(Fixture):
    """B1: the coverage is the inventory's, in order; only the judgment fields may differ."""

    def setUp(self) -> None:
        super().setUp()
        self.world.standard_world()

    def edited(self, run: Path, stem: str, edit: Callable[[dict[str, object]], None]) -> None:
        """Start again from the helper's skeleton, judge it, apply one edit and save the result."""
        skeleton = run / "skeleton.json"
        if skeleton.exists():
            (run / "coverage.json").write_text(skeleton.read_text())
        else:
            skeleton.write_text((run / "coverage.json").read_text())
        self.judge(run, {})
        coverage = self.coverage(run)
        edit(coverage)
        self.save(run, coverage)
        self.report(stem)

    def refuse_finish(self, stem: str, needle: str) -> None:
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 1, err)
        self.assertIn(needle, err)
        self.assertFalse(self.cursor_path.exists())
        status, _out, err = run_main(["validate", "--run", stem], self.world)
        self.assertEqual(status, 1)
        self.assertIn(needle, err)

    def test_a_later_verdicts_cannot_be_edited_after_until(self) -> None:
        stem, run = self.start("--until", "B1")

        def reopen(coverage: dict[str, object]) -> None:
            for item in objects(coverage["publications"]):
                item.update({"verdict": "reviewed", "depth": "code/tests"})

        self.edited(run, stem, reopen)
        self.refuse_finish(stem, "is outside the run's range (--until) and cannot be reviewed")

    def test_b_a_gap_cannot_be_removed_from_the_coverage(self) -> None:
        stem, run = self.start()

        def drop_gap(coverage: dict[str, object]) -> None:
            coverage["publications"] = [
                item for item in objects(coverage["publications"]) if item["id"] != "B2"
            ]

        self.edited(run, stem, drop_gap)
        self.refuse_finish(stem, "coverage publications do not match the inventory (missing: B2")

    def test_c_extra_reordered_and_altered_items_are_refused(self) -> None:
        stem, run = self.start()

        def extra(coverage: dict[str, object]) -> None:
            coverage["issues"] = [
                *objects(coverage["issues"]),
                {"id": "CK-99", "verdict": "reviewed", "depth": "x"},
            ]

        self.edited(run, stem, extra)
        self.refuse_finish(stem, "extra: CK-99")

        def reorder(coverage: dict[str, object]) -> None:
            coverage["issues"] = list(reversed(objects(coverage["issues"])))
            coverage["publications"] = list(reversed(objects(coverage["publications"])))

        self.edited(run, stem, reorder)
        self.refuse_finish(stem, "order differs")

        def alter(coverage: dict[str, object]) -> None:
            objects(coverage["publications"])[0]["hashes"] = ["FORGED"]
            objects(coverage["issues"])[0]["definition_sha256"] = "0" * 64

        self.edited(run, stem, alter)
        self.refuse_finish(stem, "mechanical field `hashes` differs from the inventory")
        self.refuse_finish(stem, "mechanical field `definition_sha256` differs from the inventory")

    def test_d_inherited_and_outside_range_need_the_inventorys_evidence(self) -> None:
        stem, run = self.start()

        def claim(coverage: dict[str, object]) -> None:
            objects(coverage["publications"])[0]["verdict"] = "inherited"
            objects(coverage["publications"])[1]["verdict"] = "outside-range"

        self.edited(run, stem, claim)
        self.refuse_finish(stem, "is marked inherited, but no earlier guide report covers it")
        self.refuse_finish(stem, "is marked outside-range but is inside the run's range")

    def test_e_mode_scope_pin_and_documents_are_the_runs(self) -> None:
        stem, run = self.start()
        for key, value in (
            ("mode", "review-only"),
            ("scope", ["CK-1"]),
            ("pin", {"pointer_sha256": "0" * 64}),
            ("until", "B1"),
            ("documents", []),
        ):
            with self.subTest(key=key):
                self.edited(run, stem, functools.partial(replace_key, key, value))
                self.refuse_finish(stem, f"coverage `{key}` differs from the run's recorded value")

    def test_f_review_only_must_match_the_runs_mode_both_ways(self) -> None:
        stem, run = self.start()
        self.judge(run, {})
        self.report(stem)
        status, _out, err = self.finish(stem, "--review-only")
        self.assertEqual(status, 1)
        self.assertIn("--review-only goes with a run started as --mode review-only", err)
        review, review_run = self.start("--mode", "review-only", at=CLOCK + 50)
        self.judge(review_run, {})
        self.report(review)
        status, _out, err = self.finish(review)
        self.assertEqual(status, 1)
        self.assertIn("--review-only goes with a run started as --mode review-only", err)
        self.assertFalse((self.world.docs / "docs/guide" / f"{review}.coverage.md").exists())


class PublishedCursor(Fixture):
    """B2: a docs copy that is behind the published guide files cannot move the cursor back."""

    def setUp(self) -> None:
        super().setUp()
        self.world.standard_world()

    def stale_copy(self) -> Path:
        stale = self.world.base / "stale-docs"
        (stale / "docs" / "guide").mkdir(parents=True)
        return stale

    def publish_newer(self) -> str:
        newer, newer_run = self.start(at=CLOCK + 10)
        self.judge(
            newer_run,
            {},
            findings=["GUIDE-1"],
            principles={
                "vision.economy": {
                    "section": "## Economy",
                    "quote": QUOTE,
                    "sha256": sha(QUOTE),
                    "reading": "drifting",
                    "note": "newer",
                }
            },
        )
        self.report(newer, ["[deferred] GUIDE-1. Newer finding"])
        self.assertEqual(self.finish(newer)[0], 0)
        self.publish_guide()
        return newer

    def test_a_an_older_run_finishing_from_a_stale_copy_keeps_the_published_boundary(self) -> None:
        older, older_run = self.start("--until", "B1", at=CLOCK)
        newer = self.publish_newer()
        stale = self.stale_copy()
        self.judge(older_run, {})
        write(stale / "docs/guide" / f"{older}.md", "# Older\n\nStatus legend\n")
        status, _out, err = run_main(
            ["finish", "--run", older, "--docs-copy", str(stale)], self.world
        )
        self.assertEqual(status, 0, err)
        text = (stale / "docs/guide/CURSOR.md").read_text()
        self.assertIn("Resume after: `D2`", text)
        self.assertNotIn("Resume after: `B1`", text)
        self.assertIn(f"{newer}/GUIDE-1 — Newer finding — deferred", text)
        self.assertIn("| vision.economy | drifting |", text)

    def test_b_rebuild_from_a_stale_copy_merges_the_published_files(self) -> None:
        older, older_run = self.start("--until", "B1", at=CLOCK)
        newer = self.publish_newer()
        stale = self.stale_copy()
        self.judge(older_run, {})
        write(stale / "docs/guide" / f"{older}.md", "# Older\n\nStatus legend\n")
        self.assertEqual(
            run_main(["finish", "--run", older, "--docs-copy", str(stale)], self.world)[0], 0
        )
        (stale / "docs/guide/CURSOR.md").unlink()
        status, _out, err = run_main(["rebuild-cursor", "--docs-copy", str(stale)], self.world)
        self.assertEqual(status, 0, err)
        text = (stale / "docs/guide/CURSOR.md").read_text()
        self.assertIn("Resume after: `D2`", text)
        self.assertIn(f"{newer}/GUIDE-1", text)

    def test_c_a_cursor_with_no_report_behind_it_is_kept(self) -> None:
        stem, run = self.start("--until", "B1")
        self.complete(stem, run)
        self.publish_guide()
        shutil.rmtree(self.world.checkpoint / "docs/guide")
        # The published cursor alone (the reports are gone) still fixes the boundary.
        write(
            self.world.checkpoint / "docs/guide/CURSOR.md",
            f"# Guide cursor\n\nResume after: `D1` · pointer `{sha(self.world.pointers[2])}`\n",
        )
        status, _out, err = run_main(
            ["rebuild-cursor", "--docs-copy", str(self.world.docs)], self.world
        )
        self.assertEqual(status, 0, err)
        self.assertIn("Resume after: `D1`", self.cursor_path.read_text())


class Provenance(Fixture):
    """B3: the true origin of the vision, its pinned bytes, and the executable flags."""

    def test_a_the_vision_origin_may_precede_the_baseline(self) -> None:
        self.world.standard_world()
        _stem, run = self.start(baseline="D1")
        inventory = json.loads((run / "inventory.json").read_text())
        self.assertEqual([item["id"] for item in inventory["publications"]], ["B2", "D2"])
        vision = next(d for d in inventory["documents"] if d["path"] == "docs/vision.md")
        self.assertEqual(vision["publication"], "D1")

    def test_b_a_vision_no_recorded_publication_changed_has_no_origin(self) -> None:
        self.world.batch("B1", ["PATCHB1"])
        self.world.docs_publication("D1", ["PATCHD1"], ["PATCHD1"], ("docs/other.md",))
        self.world.publish_head()
        _stem, run = self.start()
        vision = next(
            d
            for d in json.loads((run / "inventory.json").read_text())["documents"]
            if d["path"] == "docs/vision.md"
        )
        self.assertEqual(vision["publication"], "")

    def test_c_the_pinned_bytes_are_copied_and_used_for_the_principle_checks(self) -> None:
        self.world.standard_world()
        stem, run = self.start()
        self.assertEqual((run / "pinned/docs/vision.md").read_text(), VISION)
        # The checkpoint's vision disappears after the start: the stored copy still decides.
        (self.world.checkpoint / "docs/vision.md").unlink()
        invented = {
            "section": "## Invented",
            "quote": "A made up promise.",
            "sha256": sha("A made up promise."),
            "reading": "aligned",
            "note": "",
        }
        self.judge(run, {}, principles={"vision.invented": invented})
        self.report(stem)
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 1, err)
        self.assertIn("the quote is not in docs/vision.md at the pin", err)
        self.assertIn("the section `## Invented` is not in docs/vision.md at the pin", err)

    def test_d_a_missing_or_altered_stored_copy_refuses_the_principles(self) -> None:
        self.world.standard_world()
        stem, run = self.start()
        principle = {
            "section": "## Economy",
            "quote": QUOTE,
            "sha256": sha(QUOTE),
            "reading": "aligned",
            "note": "",
        }
        self.judge(run, {}, principles={"vision.economy": principle})
        self.report(stem)
        stored = run / "pinned/docs/vision.md"
        stored.write_text(VISION + "\nA later edit.\n")
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 1)
        self.assertIn("stored copy of docs/vision.md is missing or does not match", err)
        stored.unlink()
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 1)
        self.assertIn("stored copy of docs/vision.md is missing or does not match", err)
        stored.write_text(VISION)
        self.assertEqual(self.finish(stem)[0], 0)

    def test_e_a_checkpoint_vision_that_differs_from_the_pin_is_refused_at_start(self) -> None:
        self.world.standard_world()
        (self.world.checkpoint / "docs/vision.md").write_text(VISION + "\nUnpublished.\n")
        stem = self.pin()
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 1)
        self.assertIn("does not match the pin's fingerprint", err)

    def test_f_the_executable_flag_is_part_of_the_copy_check(self) -> None:
        self.world.executable = {"src/lib.rs"}
        self.world.standard_world()
        self.assertEqual(self.start()[0][:8], self.start(at=CLOCK + 1)[0][:8])
        self.world.copy.joinpath("src/lib.rs").chmod(0o644)
        stem = self.pin(at=CLOCK + 2)
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 1)
        self.assertIn("src/lib.rs has a different executable flag", err)

    def test_g_a_copy_made_executable_that_the_pin_records_as_plain_is_refused(self) -> None:
        self.world.standard_world()
        self.world.copy.joinpath("src/lib.rs").chmod(0o755)
        stem = self.pin()
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 1)
        self.assertIn("src/lib.rs has a different executable flag", err)

    def finish_text(self, stem: str) -> tuple[int, str]:
        status, _out, err = self.finish(stem)
        return status, err

    def test_h_a_vision_the_pin_lists_but_the_checkpoint_cannot_provide_is_refused_at_start(
        self,
    ) -> None:
        self.world.standard_world()
        (self.world.checkpoint / "docs/vision.md").unlink()
        stem = self.pin()
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 1)
        self.assertIn("the pinned docs/vision.md cannot be read from the checkpoint", err)
        self.assertFalse(
            (self.world.hub / ".pipeline/coordination/guide" / stem / "coverage.json").exists()
        )

    def test_i_a_vision_absent_from_the_fingerprint_is_recorded_as_absent(self) -> None:
        del self.world.files["docs/vision.md"]
        self.world.standard_world()
        stem, run = self.start()
        vision = next(
            d
            for d in json.loads((run / "inventory.json").read_text())["documents"]
            if d["path"] == "docs/vision.md"
        )
        self.assertEqual((vision["absent"], vision["sha256"]), (True, ""))
        self.assertFalse((run / "pinned/docs/vision.md").exists())
        self.judge(run, {})
        self.report(stem)
        self.assertEqual(
            self.finish(stem)[0], 0, "no principles: an absent vision is only recorded"
        )

    def test_i2_principles_need_a_vision_the_pin_lists(self) -> None:
        del self.world.files["docs/vision.md"]
        self.world.standard_world()
        stem, run = self.start()
        principle = {
            "section": "## Economy",
            "quote": QUOTE,
            "sha256": sha(QUOTE),
            "reading": "aligned",
            "note": "",
        }
        self.judge(run, {}, principles={"vision.economy": principle})
        self.report(stem)
        status, err = self.finish_text(stem)
        self.assertEqual(status, 1)
        self.assertIn("no usable pinned docs/vision.md", err)

    def test_j_a_missing_or_altered_stored_vision_is_refused_even_with_no_principles(self) -> None:
        self.world.standard_world()
        stem, run = self.start()
        self.judge(run, {})
        self.report(stem)
        stored = run / "pinned/docs/vision.md"
        stored.write_text(VISION + "\nA later edit.\n")
        status, err = self.finish_text(stem)
        self.assertEqual(status, 1)
        self.assertIn("stored copy of docs/vision.md is missing or does not match", err)
        stored.unlink()
        status, err = self.finish_text(stem)
        self.assertEqual(status, 1)
        self.assertIn("stored copy of docs/vision.md is missing or does not match", err)
        status, _out, err = run_main(["validate", "--run", stem], self.world)
        self.assertEqual(status, 1)
        self.assertIn("stored copy of docs/vision.md", err)
        self.assertFalse(self.cursor_path.exists())
        stored.write_text(VISION)
        self.assertEqual(self.finish(stem)[0], 0)

    def contract_world(self) -> None:
        self.world.files["docs/design.md"] = "# design\n"
        self.world.batch("B1", ["PATCHB1"])
        self.world.docs_publication(
            "D1", ["PATCHD1"], ["PATCHD1"], ("docs/design.md", "docs/gone.md")
        )
        self.world.issue(1, "First")
        self.world.publish_head()

    def test_k_a_missing_or_altered_stored_contract_is_refused(self) -> None:
        self.contract_world()
        stem, run = self.start()
        documents = {
            d["path"]: d for d in json.loads((run / "inventory.json").read_text())["documents"]
        }
        self.assertEqual(documents["docs/design.md"]["publication"], "D1")
        self.assertIs(
            documents["docs/gone.md"]["absent"], True, "a deleted contract is recorded as absent"
        )
        self.judge(run, {})
        self.report(stem)
        stored = run / "pinned/docs/design.md"
        stored.write_text("# design\naltered\n")
        status, err = self.finish_text(stem)
        self.assertEqual(status, 1)
        self.assertIn("stored copy of docs/design.md is missing or does not match", err)
        stored.unlink()
        status, err = self.finish_text(stem)
        self.assertEqual(status, 1)
        self.assertIn("stored copy of docs/design.md is missing or does not match", err)
        stored.write_text("# design\n")
        self.assertEqual(self.finish(stem)[0], 0)

    def test_l_a_contract_the_pin_lists_but_the_checkpoint_lacks_is_refused_at_start(self) -> None:
        self.contract_world()
        (self.world.checkpoint / "docs/design.md").unlink()
        stem = self.pin()
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 1)
        self.assertIn("the pinned docs/design.md cannot be read from the checkpoint", err)


class HeadingDocuments(Fixture):
    """CK-87: the heading is `docs/vision.md` plus the `docs/game/*.md` files the pin lists."""

    def setUp(self) -> None:
        super().setUp()
        self.world.files["docs/game/economy.md"] = ECONOMY
        self.world.files["docs/game/military.md"] = MILITARY
        self.world.standard_world()

    def principle(self, quote: str, section: str, **extra: str) -> dict[str, str]:
        return {
            "section": section,
            "quote": quote,
            "sha256": sha(quote),
            "reading": "aligned",
            "note": "",
            **extra,
        }

    def documents(self, run: Path) -> dict[str, dict[str, object]]:
        found = json.loads((run / "inventory.json").read_text())["documents"]
        return {str(item["path"]): item for item in objects(found)}

    def test_a_every_game_document_is_pinned_with_no_in_range_change(self) -> None:
        _stem, run = self.start()
        documents = self.documents(run)
        # Nothing in the range changed the game documents (D2 changed only `docs/note.md`, a contract).
        self.assertEqual(
            sorted(documents),
            ["docs/game/economy.md", "docs/game/military.md", "docs/note.md", "docs/vision.md"],
        )
        for name, text in (("docs/game/economy.md", ECONOMY), ("docs/game/military.md", MILITARY)):
            self.assertEqual(documents[name]["sha256"], sha(text))
            self.assertEqual(documents[name]["publication"], "")
            self.assertEqual((run / "pinned" / name).read_text(), text)

    def test_b_only_direct_game_markdown_files_join_the_heading(self) -> None:
        self.world.files["docs/game/notes.txt"] = "not markdown\n"
        self.world.files["docs/game/deeper/more.md"] = "# deeper\n"
        self.world.files["docs/gamer.md"] = "# not game\n"
        self.world.files["docs/design.md"] = "# a contract\n"
        self.world.batch("B3", ["PATCHB3"])
        self.world.publish_head()
        _stem, run = self.start()
        self.assertEqual(
            sorted(self.documents(run)),
            ["docs/game/economy.md", "docs/game/military.md", "docs/note.md", "docs/vision.md"],
        )

    def test_c_a_principle_may_name_a_game_document_and_validates_against_it(self) -> None:
        stem, run = self.start()
        economy = self.principle(PRICES_QUOTE, "## Prices", document="docs/game/economy.md")
        self.judge(run, {}, principles={"vision.prices": economy})
        self.report(stem)
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 0, err)

    def test_d_a_quote_only_another_heading_document_has_is_refused_naming_the_document(
        self,
    ) -> None:
        stem, run = self.start()
        cases = (
            # The economy's quote claimed against the military document, and the vision's against a game file.
            (
                self.principle(PRICES_QUOTE, "## Prices", document="docs/game/military.md"),
                "military",
            ),
            (self.principle(QUOTE, "## Economy", document="docs/game/economy.md"), "economy"),
            # A game quote with no `document` is checked against the vision.
            (self.principle(PRICES_QUOTE, "## Prices"), "docs/vision.md"),
        )
        for entry, name in cases:
            with self.subTest(name=name, quote=entry["quote"]):
                self.judge(run, {}, principles={"vision.mixed": entry})
                self.report(stem)
                status, _out, err = self.finish(stem)
                self.assertEqual(status, 1, err)
                self.assertIn("the quote is not in docs/", err)
                self.assertIn(name, err)
                self.assertFalse(self.cursor_path.exists())

    def test_e_a_document_outside_the_heading_set_is_refused(self) -> None:
        stem, run = self.start()
        self.world.files["docs/design.md"] = "# design\n"
        for document in (
            "docs/design.md",
            "docs/guide/CURSOR.md",
            "docs/game/deeper/more.md",
            "docs/game/../vision.md",
            "../docs/vision.md",
            "/docs/vision.md",
            "docs/game/economy.txt",
            "docs/game/",
            "src/lib.rs",
        ):
            with self.subTest(document=document):
                entry = self.principle(QUOTE, "## Economy", document=document)
                self.judge(run, {}, principles={"vision.outside": entry})
                self.report(stem)
                status, _out, err = self.finish(stem)
                self.assertEqual(status, 1, err)
                self.assertIn("is not a heading document", err)

    GAME_NAMES = (
        "economy.md",
        "with space.md",
        "_leading_underscore.md",
        "Caf\u00e9 \u7d4c\u6e08.md",
        ".hidden.md",
        "UPPER.MD.md",
        "a'quote.md",
        "dots.in.name.md",
        "-dash.md",
    )

    def test_n_every_direct_game_markdown_name_shape_joins_the_heading_and_is_usable(self) -> None:
        for name in self.GAME_NAMES:
            self.world.files[f"docs/game/{name}"] = (
                f"# {name}\n\n## Part\n\n- **Firm.** Kept in {name}.\n"
            )
        self.world.batch("B3", ["PATCHB3"])
        self.world.publish_head()
        stem, run = self.start()
        documents = self.documents(run)
        for name in self.GAME_NAMES:
            path = f"docs/game/{name}"
            with self.subTest(path=path):
                self.assertIn(path, documents)
                self.assertEqual((run / "pinned" / path).read_text(), self.world.files[path])
        # Each one carries a principle that validates.
        principles: dict[str, object] = {
            f"vision.shape-{index}": self.principle(
                f"Kept in {name}.", "## Part", document=f"docs/game/{name}"
            )
            for index, name in enumerate(self.GAME_NAMES)
        }
        self.judge(run, {}, principles=principles)
        self.report(stem)
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 0, err)

    def test_o_names_that_are_not_direct_markdown_children_stay_out_of_the_heading(self) -> None:
        for path in (
            "docs/game/sub/deeper.md",
            "docs/game/notes.txt",
            "docs/game/noextension",
            "docs/game/.md",
            "docs/game/md",
            "docs/gamer/x.md",
            "docs/game.md",
            "docs/Game/x.md",
            "docs/game/line\nbreak.md",
        ):
            with self.subTest(path=path):
                self.assertFalse(ck_guide.is_heading_document(path), path)
        self.assertTrue(ck_guide.is_heading_document("docs/vision.md"))
        self.assertTrue(ck_guide.is_heading_document("docs/game/a b.md"))
        self.assertEqual(
            ck_guide.heading_documents(
                {"docs/game/x y.md": "1", "docs/game/z/q.md": "2", "docs/game/w.txt": "3"}
            ),
            ["docs/game/x y.md", "docs/vision.md"],
        )

    def test_p_an_explicit_invalid_document_is_refused_and_never_read_as_the_vision(self) -> None:
        stem, run = self.start()
        for value in (
            17,
            None,
            "",
            True,
            0,
            [],
            ["docs/vision.md"],
            {"path": "docs/vision.md"},
            " ",
            "vision",
        ):
            with self.subTest(document=value):
                entry = self.principle(QUOTE, "## Economy")
                entry["document"] = value  # type: ignore[assignment]
                self.judge(run, {}, principles={"vision.economy": entry})
                self.report(stem)
                for command in (
                    ["validate", "--run", stem, "--docs-copy", str(self.world.docs)],
                    ["finish", "--run", stem, "--docs-copy", str(self.world.docs)],
                ):
                    status, _out, err = run_main(command, self.world, at=CLOCK + 100)
                    self.assertEqual(status, 1, (command[0], err))
                    self.assertIn("`document`", err)
                    self.assertIn("is not a heading document", err)
                    self.assertIn(repr(value), err)
                # Nothing was persisted.
                self.assertFalse(self.cursor_path.exists())
                self.assertEqual(
                    list((self.world.docs / "docs" / "guide").glob("*.coverage.md")), []
                )

    def test_q_the_vision_is_the_default_only_when_document_is_absent(self) -> None:
        stem, run = self.start()
        absent = self.principle(QUOTE, "## Economy")
        explicit = self.principle(QUOTE, "## Economy", document="docs/vision.md")
        self.assertNotIn("document", absent)
        for entry in (absent, explicit):
            self.judge(run, {}, principles={"vision.economy": entry})
            self.report(stem)
            status, _out, err = run_main(
                ["validate", "--run", stem, "--docs-copy", str(self.world.docs)], self.world
            )
            self.assertEqual(status, 0, err)

    def test_f_a_game_path_the_pin_does_not_list_is_refused_even_if_it_looks_allowed(self) -> None:
        stem, run = self.start()
        entry = self.principle(PRICES_QUOTE, "## Prices", document="docs/game/unpinned.md")
        self.judge(run, {}, principles={"vision.unpinned": entry})
        self.report(stem)
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 1, err)
        self.assertIn("no usable pinned docs/game/unpinned.md", err)

    def test_g_a_coverage_without_document_validates_exactly_as_before(self) -> None:
        stem, run = self.start()
        self.judge(run, {}, principles={"vision.economy": self.principle(QUOTE, "## Economy")})
        self.report(stem)
        self.assertEqual(self.finish(stem)[0], 0)

    def test_h_missing_or_altered_stored_game_bytes_are_refused_even_with_no_principle(
        self,
    ) -> None:
        stem, run = self.start()
        self.judge(run, {})
        self.report(stem)
        stored = run / "pinned/docs/game/economy.md"
        stored.write_text(ECONOMY + "altered\n")
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 1)
        self.assertIn("stored copy of docs/game/economy.md is missing or does not match", err)
        stored.unlink()
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 1)
        self.assertIn("stored copy of docs/game/economy.md is missing or does not match", err)
        stored.write_text(ECONOMY)
        self.assertEqual(self.finish(stem)[0], 0)

    def test_i_a_principle_on_a_game_document_needs_its_stored_copy(self) -> None:
        stem, run = self.start()
        entry = self.principle(PRICES_QUOTE, "## Prices", document="docs/game/economy.md")
        self.judge(run, {}, principles={"vision.prices": entry})
        self.report(stem)
        (run / "pinned/docs/game/economy.md").write_text("# tampered\n")
        status, _out, err = self.finish(stem)
        self.assertEqual(status, 1)
        self.assertIn("no usable pinned docs/game/economy.md", err)

    def test_j_a_checkpoint_game_file_that_differs_from_the_fingerprint_is_refused_at_start(
        self,
    ) -> None:
        (self.world.checkpoint / "docs/game/economy.md").write_text(ECONOMY + "unpublished\n")
        stem = self.pin()
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 1)
        self.assertIn("the pinned docs/game/economy.md does not match the pin's fingerprint", err)

    def test_k_a_game_file_the_pin_lists_but_the_checkpoint_cannot_provide_is_refused(self) -> None:
        (self.world.checkpoint / "docs/game/military.md").unlink()
        stem = self.pin()
        status, _out, err = self.resume(stem)
        self.assertEqual(status, 1)
        self.assertIn("the pinned docs/game/military.md cannot be read from the checkpoint", err)

    def test_l_recheck_reports_an_edit_an_addition_and_a_deletion_of_a_heading_document(
        self,
    ) -> None:
        stem, run = self.start()
        self.judge(run, {})
        before = json.dumps(self.coverage(run)["documents"], sort_keys=True)
        self.world.files["docs/game/economy.md"] = ECONOMY + "\n- **Open.** Tax.\n"  # edited
        self.world.files["docs/game/maritime.md"] = "# Maritime\n"  # added
        del self.world.files["docs/game/military.md"]  # deleted
        self.world.docs_publication(
            "D3",
            ["PATCHD3"],
            ["PATCHD3"],
            ("docs/game/economy.md", "docs/game/maritime.md", "docs/game/military.md"),
        )
        self.world.publish_head()
        # `publish_head` writes the files that exist; the deleted one must go from the checkpoint too.
        (self.world.checkpoint / "docs/game/military.md").unlink(missing_ok=True)
        status, _out, err = run_main(["recheck", "--run", stem], self.world)
        self.assertEqual(status, 0, err)
        found = {
            (item["path"], item["change"])
            for item in objects(self.coverage(run)["pending"])
            if item["kind"] == "document"
        }
        self.assertEqual(
            found,
            {
                ("docs/game/economy.md", "published since the pin"),
                ("docs/game/maritime.md", "added since the pin"),
                ("docs/game/military.md", "deleted since the pin"),
            },
        )
        # The pin and the pinned inventory do not move.
        self.assertEqual(json.dumps(self.coverage(run)["documents"], sort_keys=True), before)
        self.assertEqual((run / "pinned/docs/game/military.md").read_text(), MILITARY)

    def test_m_recheck_reports_a_vision_edit_and_nothing_for_unchanged_heading_documents(
        self,
    ) -> None:
        stem, run = self.start()
        status, _out, err = run_main(["recheck", "--run", stem], self.world)
        self.assertEqual(status, 0, err)
        self.assertEqual(
            [item for item in objects(self.coverage(run)["pending"]) if item["kind"] == "document"],
            [],
        )
        self.world.files["docs/vision.md"] = VISION + "\nA later edit.\n"
        self.world.docs_publication("D3", ["PATCHD3"], ["PATCHD3"], ("docs/vision.md",))
        self.world.publish_head()
        status, _out, err = run_main(["recheck", "--run", stem], self.world)
        self.assertEqual(status, 0, err)
        found = [
            (item["path"], item["change"])
            for item in objects(self.coverage(run)["pending"])
            if item["kind"] == "document"
        ]
        self.assertEqual(found, [("docs/vision.md", "published since the pin")])


class Destination(Fixture):
    """B5: `--docs-copy` can never be one of the hub's record trees."""

    def setUp(self) -> None:
        super().setUp()
        self.world.standard_world()
        self.stem, self.run_dir = self.start()
        self.judge(self.run_dir, {})
        self.report(self.stem)

    def protected(self) -> dict[str, Path]:
        pipeline = self.world.hub / ".pipeline"
        alias = self.world.base / "alias"
        alias.symlink_to(self.world.checkpoint)
        return {
            "checkpoint": self.world.checkpoint,
            "checkpoints": pipeline / "checkpoints",
            "queue": pipeline / "queue",
            "batches": pipeline / "batches",
            "docs records": pipeline / "docs",
            "metadata": self.world.meta,
            "inside the metadata": self.world.meta / "issues",
            "hub root": self.world.hub,
            "symlink into the checkpoint": alias,
            "inside the checkpoint": self.world.checkpoint / "src",
        }

    def test_finish_and_rebuild_refuse_protected_destinations_and_write_nothing(self) -> None:
        for name, destination in self.protected().items():
            with self.subTest(destination=name):
                before = tree_digests(self.world.hub, self.world.meta)
                for args in (
                    ["finish", "--run", self.stem, "--docs-copy", str(destination)],
                    ["rebuild-cursor", "--docs-copy", str(destination)],
                ):
                    status, _out, err = run_main(args, self.world)
                    self.assertEqual(status, 1, err)
                    self.assertIn("refusing --docs-copy", err)
                self.assertEqual(tree_digests(self.world.hub, self.world.meta), before)

    def test_a_worker_copy_and_an_outside_copy_are_allowed(self) -> None:
        inside = self.world.hub / ".pipeline" / "workers" / "guide-docs"
        shutil.copytree(self.world.docs, inside)
        status, _out, err = run_main(
            ["finish", "--run", self.stem, "--docs-copy", str(inside)], self.world
        )
        self.assertEqual(status, 0, err)
        self.assertTrue((inside / "docs/guide/CURSOR.md").is_file())
        self.assertEqual(self.finish(self.stem)[0], 0)

    def linked_copy(self) -> Path:
        """A copy of the docs copy that the test then turns into a symlinked layout."""
        linked = self.world.base / "linked"
        shutil.copytree(self.world.docs, linked, symlinks=True)
        return linked

    def refused_without_writing(self, linked: Path, needle: str) -> None:
        trees = (self.world.hub, self.world.meta, linked)
        before = tree_digests(*trees)
        for args in (
            ["finish", "--run", self.stem, "--docs-copy", str(linked)],
            ["rebuild-cursor", "--docs-copy", str(linked)],
        ):
            status, _out, err = run_main(args, self.world)
            self.assertEqual(status, 1, err)
            self.assertIn(needle, err)
        self.assertEqual(tree_digests(*trees), before)

    def test_a_docs_symlink_into_the_checkpoint_is_refused(self) -> None:
        linked = self.linked_copy()
        shutil.rmtree(linked / "docs")
        (linked / "docs").symlink_to(self.world.checkpoint / "docs")
        self.refused_without_writing(linked, "refusing to write")

    def test_a_docs_guide_symlink_into_the_checkpoint_is_refused(self) -> None:
        linked = self.linked_copy()
        (self.world.checkpoint / "docs/guide").mkdir()
        shutil.rmtree(linked / "docs/guide")
        (linked / "docs/guide").symlink_to(self.world.checkpoint / "docs/guide")
        self.refused_without_writing(linked, "refusing to write")

    def test_a_leaf_cursor_symlink_into_the_checkpoint_is_refused(self) -> None:
        linked = self.linked_copy()
        (linked / "docs/guide/CURSOR.md").symlink_to(self.world.checkpoint / "src/lib.rs")
        self.refused_without_writing(linked, "refusing to write")
        self.assertEqual((self.world.checkpoint / "src/lib.rs").read_text(), "pub fn a() {}\n")

    def test_a_leaf_coverage_symlink_into_the_checkpoint_is_refused_by_finish(self) -> None:
        linked = self.linked_copy()
        (linked / f"docs/guide/{self.stem}.coverage.md").symlink_to(
            self.world.checkpoint / "src/lib.rs"
        )
        trees = (self.world.hub, self.world.meta, linked)
        before = tree_digests(*trees)
        status, _out, err = run_main(
            ["finish", "--run", self.stem, "--docs-copy", str(linked)], self.world
        )
        self.assertEqual(status, 1, err)
        self.assertIn("refusing to write", err)
        self.assertEqual(tree_digests(*trees), before)

    def test_symlinks_to_other_outside_trees_and_into_the_metadata_are_refused(self) -> None:
        for target in (self.world.base / "elsewhere", self.world.meta / "issues"):
            with self.subTest(target=target.name):
                target.mkdir(exist_ok=True)
                linked = self.linked_copy()
                shutil.rmtree(linked / "docs/guide")
                (linked / "docs/guide").symlink_to(target)
                self.refused_without_writing(linked, "refusing to write")
                shutil.rmtree(self.world.base / "linked", ignore_errors=True)

    def test_a_symlink_that_stays_inside_the_docs_copy_is_allowed(self) -> None:
        linked = self.linked_copy()
        (linked / "docs/real-guide").mkdir()
        shutil.copy2(linked / f"docs/guide/{self.stem}.md", linked / "docs/real-guide")
        shutil.rmtree(linked / "docs/guide")
        (linked / "docs/guide").symlink_to(linked / "docs/real-guide")
        status, _out, err = run_main(
            ["finish", "--run", self.stem, "--docs-copy", str(linked)], self.world
        )
        self.assertEqual(status, 0, err)
        self.assertTrue((linked / "docs/real-guide/CURSOR.md").is_file())


class Boundary(Fixture):
    """The helper reads and writes only what it should, and never starts a process."""

    def setUp(self) -> None:
        super().setUp()
        self.world.standard_world()

    def watched(self) -> dict[str, str]:
        """Every file the helper must leave alone, except its own run directories."""
        everything = tree_digests(self.world.hub, self.world.meta, self.world.copy)
        everything[str(self.world.log_file)] = sha(self.world.log_file.read_bytes())
        return {k: v for k, v in everything.items() if "/coordination/guide/" not in k}

    def test_no_command_changes_a_watched_file_or_starts_a_process(self) -> None:
        def forbidden(*_args: object, **_kwargs: object) -> None:
            raise AssertionError("the helper started a process")

        stem = ""
        with (
            mock.patch.object(subprocess, "run", forbidden),
            mock.patch.object(subprocess, "Popen", forbidden),
            mock.patch.object(subprocess, "check_output", forbidden),
            mock.patch.object(subprocess, "check_call", forbidden),
            mock.patch.object(subprocess, "call", forbidden),
            mock.patch.object(os, "system", forbidden),
            mock.patch.object(os, "popen", forbidden),
        ):
            before = self.watched()
            run_main(["status"], self.world)
            self.assertEqual(self.watched(), before)
            stem = self.pin()
            self.assertEqual(self.watched(), before)
            self.assertEqual(self.resume(stem)[0], 0)
            self.assertEqual(self.watched(), before)
            run = self.world.hub / ".pipeline/coordination/guide" / stem
            self.judge(run, {})
            self.report(stem)
            for args in (["recheck", "--run", stem], ["validate", "--run", stem]):
                self.assertEqual(run_main(args, self.world)[0], 0)
                self.assertEqual(self.watched(), before)
            self.assertEqual(self.finish(stem)[0], 0)
            self.assertEqual(self.watched(), before)
            self.assertEqual(
                run_main(["rebuild-cursor", "--docs-copy", str(self.world.docs)], self.world)[0], 0
            )
            self.assertEqual(self.watched(), before)
        changed = {path.name for path in (self.world.docs / "docs/guide").iterdir()}
        self.assertEqual(changed, {"CURSOR.md", f"{stem}.md", f"{stem}.coverage.md"})

    def test_the_source_imports_no_process_module(self) -> None:
        tree = ast.parse((REPO / "tools/guide/ck_guide.py").read_text())
        imported = {
            alias.name.split(".")[0]
            for node in ast.walk(tree)
            if isinstance(node, ast.Import)
            for alias in node.names
        } | {
            (node.module or "").split(".")[0]
            for node in ast.walk(tree)
            if isinstance(node, ast.ImportFrom)
        }
        for module in ("subprocess", "os", "shutil", "socket", "multiprocessing", "pty"):
            self.assertNotIn(module, imported)

    def test_run_stems_fit_the_worker_name_rule_read_from_the_gate(self) -> None:
        source = (REPO / "tools/gate/cli.py").read_text()
        found = re.search(r're\.fullmatch\(r"([^"]+)", args\.name\)', source)
        self.assertIsNotNone(found)
        rule = found.group(1) if found else ""
        stem = self.pin()
        self.assertRegex(f"guide-{stem}", f"^(?:{rule})$")

    def test_a_cold_cli_run_writes_no_bytecode_into_its_code_tree(self) -> None:
        code = self.world.base / "code" / "tools"
        code.mkdir(parents=True)
        shutil.copy2(REPO / "tools" / "ck-guide", code / "ck-guide")
        for name in ("guide", "gate"):
            shutil.copytree(
                REPO / "tools" / name, code / name, ignore=shutil.ignore_patterns("__pycache__")
            )
        env = {
            key: value
            for key, value in os.environ.items()
            if key not in ("PYTHONDONTWRITEBYTECODE", "PYTHONPYCACHEPREFIX")
        }
        before = tree_digests(code)
        result = subprocess.run(
            [
                sys.executable,
                str(code / "ck-guide"),
                "--hub",
                str(self.world.hub),
                "--meta",
                str(self.world.meta),
                "status",
            ],
            capture_output=True,
            text=True,
            check=False,
            env=env,
            timeout=60,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sorted(path.name for path in code.rglob("__pycache__")), [])
        self.assertEqual(tree_digests(code), before)

    def test_the_extensionless_shim_runs_the_helper(self) -> None:
        shim = REPO / "tools" / "ck-guide"
        result = subprocess.run(
            [
                sys.executable,
                str(shim),
                "--hub",
                str(self.world.hub),
                "--meta",
                str(self.world.meta),
                "status",
            ],
            capture_output=True,
            text=True,
            check=False,
            timeout=60,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("cursor: none", result.stdout)
        self.assertTrue(os.access(shim, os.X_OK))

    def test_errors_are_one_line_and_exit_one(self) -> None:
        status, out, err = run_main(["recheck", "--run", "20260101t000000z-abcdef01"], self.world)
        self.assertEqual((status, out), (1, ""))
        self.assertEqual(len(err.strip().splitlines()), 1)
        self.assertTrue(err.startswith("ck-guide: "))


if __name__ == "__main__":
    unittest.main()
