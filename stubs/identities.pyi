from collections.abc import Mapping
from contextlib import AbstractContextManager
from pathlib import Path
from typing import TypedDict

from identity_conflicts import IdentityConflict as IdentityConflict

Agent = dict[str, object]

class Registry(TypedDict):
    agents: dict[str, Agent]
    counters: dict[str, object]

def registry_path() -> Path: ...
def read_registry() -> Registry: ...
def atomic_json(path: Path, value: object) -> None: ...
def locked() -> AbstractContextManager[None]: ...
def allocate(
    cfg: Mapping[str, object],
    registry: Mapping[str, object],
    *,
    project: str,
    role: str,
    key: str,
    name: str | None = None,
    **details: object,
) -> Agent: ...
def register(
    *, project: str, role: str, key: str, name: str | None = None, **details: object
) -> Agent: ...
def sync(cfg: Mapping[str, object] | None = None) -> list[Agent]: ...
def current(cfg: Mapping[str, object], *, discover: bool = True) -> Agent | None: ...
def resolve(
    cfg: Mapping[str, object], requested: str | None = None, *, discover: bool = True
) -> str | None: ...
