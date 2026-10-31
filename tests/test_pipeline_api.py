"""The contract `gate.pipeline_api` states for `tools/pipeline.py`, checked at run time as well.

mypy checks the assignment of the module to the Protocol (S7c typed `pipeline.py`); these tests call
each real function the gate uses in a fixture repository and check the value it returns against the
declared return type (`shapes.conforms`), which also covers what a static signature cannot say.
"""

from __future__ import annotations

import json
import sys
import tempfile
import typing
import unittest
from collections.abc import Callable
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from gate import pipeline_api
from gate.pointer import canonical
from shapes import conforms

REAL = pipeline_api.load()


def call(function: Callable[..., object], *arguments: object) -> object:
    """The value a function returns, even when it is declared to return None."""
    return function(*arguments)


def declared(owner: object, name: str) -> object:
    """The return annotation the contract declares for `owner.name`."""
    return typing.get_type_hints(getattr(owner, name))["return"]


class PipelineContract(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ck-pipeline-api-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name).resolve()
        self.hub = self.base / "hub"
        (self.hub / ".pipeline").mkdir(parents=True)
        self.repo = self.base / "repo"
        self.repo.mkdir()
        REAL.pijul(self.repo, "init")
        (self.repo / ".ignore").write_text("/.ck-hub\n")
        (self.repo / "README.md").write_text("first\n")
        (self.repo / "tool.sh").write_text("#!/bin/sh\n")
        (self.repo / "tool.sh").chmod(0o755)
        self.record("first")

    def record(self, message: str) -> None:
        REAL.pijul(self.repo, "add", "--recursive", ".")
        REAL.pijul(self.repo, "record", "-am", message, "--author", "Contract tests")

    def check(self, name: str, value: object) -> None:
        hint = declared(pipeline_api.PipelineModule, name)
        self.assertTrue(conforms(value, hint), f"{name} returned {value!r}, declared {hint}")

    def test_01_the_primitives_return_what_they_declare(self) -> None:
        self.check("pijul", REAL.pijul(self.repo, "log", "--hash-only"))
        self.check("hashes", REAL.hashes(self.repo))
        self.check("files", REAL.files(self.repo))
        self.check("fingerprint", REAL.fingerprint(self.repo))
        self.check("clean", call(REAL.clean, self.repo))
        self.check("doc_path", REAL.doc_path("README.md"))
        self.check("doc_path", REAL.doc_path("src/main.rs"))
        fingerprint = REAL.fingerprint(self.repo)
        self.assertEqual(fingerprint["files"]["tool.sh"]["executable"], True)
        self.assertEqual(fingerprint["files"]["README.md"]["executable"], False)
        self.assertEqual(set(fingerprint["hashes"]), REAL.hashes(self.repo))

    def test_02_clean_refuses_pending_changes_as_declared(self) -> None:
        (self.repo / "README.md").write_text("changed\n")
        with self.assertRaises(RuntimeError):
            REAL.clean(self.repo)

    def test_03_clone_and_pull(self) -> None:
        clone = self.base / "clone"
        self.check("clone", call(REAL.clone, self.repo, clone, self.hub))
        self.assertEqual(REAL.hashes(clone), REAL.hashes(self.repo))
        (self.repo / "README.md").write_text("second\n")
        self.record("second")
        missing = sorted(REAL.hashes(self.repo) - REAL.hashes(clone))
        self.assertEqual(len(missing), 1)
        self.check("pull", REAL.pull(clone, self.repo, missing))
        self.assertEqual(REAL.pull(clone, self.repo, []), [])  # nothing asked, nothing pulled
        self.assertEqual(REAL.hashes(clone), REAL.hashes(self.repo))

    def test_04_the_legacy_controller(self) -> None:
        fingerprint = REAL.fingerprint(self.repo)
        pointer: dict[str, object] = {
            "repo": str(self.repo),
            "fingerprint": fingerprint,
            "submissions": [],
            "receipt": None,
            "previous": None,
        }
        (self.hub / ".pipeline/accepted.json").write_bytes(canonical(pointer))
        legacy = REAL.Pipeline(self.hub)
        state = legacy.verified_state()
        self.assertTrue(conforms(state, declared(pipeline_api.LegacyPipeline, "verified_state")))
        self.assertEqual(state["repo"], str(self.repo))
        (self.hub / ".pipeline/blocked").mkdir()
        (self.hub / ".pipeline/blocked/old-submission.json").write_text(json.dumps({}))
        blocked = legacy.blocked()
        self.assertTrue(conforms(blocked, declared(pipeline_api.LegacyPipeline, "blocked")))
        self.assertEqual(blocked, {"old-submission"})


if __name__ == "__main__":
    unittest.main()
