"""The typed face of `tools/pipeline.py` that the gate modules use.

Since S7c types `pipeline.py`, assigning the module to this Protocol is a real mypy check: a
signature here that `pipeline.py` does not have, or the reverse, is a type error (the probe in
`tests/test_check.py::MypyStep.test_08` shows it), and `tests/test_pipeline_api.py` still calls each
real function in a fixture repository and checks what it returns against the declared types. Only the
functions the gate actually calls are listed; the gate reaches the module through `load()` because
`pipeline` imports `gate.cli` (an import cycle at module load).
"""

from __future__ import annotations

from collections.abc import Sequence
from pathlib import Path
from typing import Protocol

from .records import FileEntry, Fingerprint, PointerRecord


class LegacyPipeline(Protocol):
    """`pipeline.Pipeline`: the pre-activation controller, used by the bootstrap."""

    def verified_state(self) -> PointerRecord:
        """The legacy `accepted.json` state, after its checkpoint was checked clean and unedited."""
        ...

    def blocked(self) -> set[str]:
        """The ids of legacy-blocked submissions."""
        ...


class PipelineModule(Protocol):
    def pijul(self, repo: Path, *arguments: str | Path) -> str: ...

    def hashes(self, repo: Path) -> set[str]: ...

    def clean(self, repo: Path) -> None:
        """Raise RuntimeError unless the repository has no pending changes."""
        ...

    def files(self, repo: Path) -> dict[str, FileEntry]: ...

    def fingerprint(self, repo: Path) -> Fingerprint: ...

    def clone(self, source: Path, destination: Path, hub: Path) -> None: ...

    def pull(self, repo: Path, source: Path, changes: Sequence[str]) -> list[object]:
        """Pull `changes` from `source`; the conflicts Pijul reports (empty when none)."""
        ...

    def doc_path(self, name: str) -> bool:
        """True for ordinary documentation: README.md and docs/**/*.md (not AGENTS/CLAUDE/SKILL)."""
        ...

    def Pipeline(self, hub: Path) -> LegacyPipeline: ...  # the class, called as a factory


def load() -> PipelineModule:
    """The `pipeline` module, imported when first needed (it imports `gate.cli`: an import cycle)."""
    import pipeline  # noqa: PLC0415  # lazy: pipeline imports gate.cli (import cycle)

    module: PipelineModule = pipeline
    return module
