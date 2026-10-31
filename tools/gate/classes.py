"""Role-to-class verification against the shared model-class master."""

from __future__ import annotations

import hashlib
import os
import tomllib
from pathlib import Path

from .records import (
    ClassRow,
    ModelClasses,
    ModelUse,
    OffClass,
    RoleMapping,
    load_model_classes,
)

ROLE_CLASS = {"manager": "S", "worker": "B", "reviewer": "A"}
Pair = tuple[str, str | None]  # a model and the effort it ran at


def master_path() -> Path:
    return Path(os.environ.get("CK_MODEL_CLASSES", Path.home() / ".config/model-classes.toml"))


def master() -> tuple[ModelClasses, str]:
    path = master_path()
    raw = path.read_bytes()
    return load_model_classes(tomllib.loads(raw.decode()), str(path)), hashlib.sha256(
        raw
    ).hexdigest()


def _family_entry(row: ClassRow, family: str) -> object:
    if family == "claude":
        return row.get("claude")
    if family == "codex":
        return row.get("codex")
    return None  # the master names only these two families


def resolve_class(data: ModelClasses, class_name: str, family: str) -> RoleMapping:
    seen = set()
    while True:
        if class_name in seen:
            raise RuntimeError(f"Cyclic class route: {class_name}")
        seen.add(class_name)
        row = data.get("classes", {}).get(class_name)
        if row is None:
            raise RuntimeError(f"Unknown model class {class_name}")
        if "route" in row:
            class_name = row["route"]
            continue
        value = _family_entry(row, family)
        model = value.get("model") if isinstance(value, dict) else None
        effort = value.get("effort") if isinstance(value, dict) else None
        if not isinstance(model, str) or not model or not isinstance(effort, str) or not effort:
            raise RuntimeError(f"Class {class_name} has no {family} model and effort")
        return {"class": class_name, "family": family, "model": model, "effort": effort}


def family_for_model(data: ModelClasses, client: str, model: str) -> str:
    normalized = normalize_model(data, client, model)
    families = set()
    for class_name in data.get("classes", {}):
        for family in ("claude", "codex"):
            try:
                if resolve_class(data, class_name, family)["model"] == normalized:
                    families.add(family)
            except RuntimeError:
                pass
    return next(iter(families)) if len(families) == 1 else "unknown"


def normalize_model(data: ModelClasses, client: str, model: str) -> str:
    aliases = data.get("clients", {}).get(client, {}).get("model_aliases", {})
    return next((key for key, value in aliases.items() if value == model), model)


def _pair_of(data: ModelClasses, client: str, item: ModelUse | None) -> Pair:
    if item is None:  # an unresolved selection has neither field
        return normalize_model(data, client, ""), None
    return normalize_model(data, client, item.get("model", "")), item.get("effort")


def verify(role: str, client: str, observed: list[ModelUse]) -> RoleMapping:
    if role not in ROLE_CLASS:
        raise RuntimeError(f"Unknown agent role {role!r}")
    if not observed:
        raise RuntimeError("Agent model history is empty")
    data, master_sha = master()
    pairs = {_pair_of(data, client, item) for item in observed}
    families = {family_for_model(data, client, model) for model, _ in pairs}
    if len(families) != 1 or "unknown" in families:
        family = "multi-family" if len(families) > 1 else "unknown"
        return {"class": ROLE_CLASS[role], "family": family, "families": sorted(families)}
    family = next(iter(families))
    expected = resolve_class(data, ROLE_CLASS[role], family)
    expected_pair = (expected["model"], expected["effort"])
    if pairs != {expected_pair}:
        raise RuntimeError(
            f"Session does not match role {role} Class {ROLE_CLASS[role]} at high effort: "
            f"observed {sorted(pairs, key=str)!r}, expected {expected_pair!r}"
        )
    return {**expected, "master_sha256": master_sha, "families": [family]}


def _require_current(role: str, current_pair: Pair, expected_pair: Pair) -> None:
    if current_pair != expected_pair:
        raise RuntimeError(
            f"Session does not match role {role} Class {ROLE_CLASS[role]} at high effort: "
            f"current {current_pair!r}, expected {expected_pair!r}"
        )


def _mixed_readiness(  # noqa: PLR0913  # explicit arguments; a parameter object would obscure the call sites
    role: str,
    data: ModelClasses,
    master_sha: str,
    client: str,
    counts: dict[Pair, int],
    families: set[str],
    current_pair: Pair,
) -> RoleMapping:
    """A mixed or unknown history is judged by its current selection like any other.

    The history classification stays exactly what `verify` reports, so authorship still needs
    reviews from both families and a mixed-history reviewer still cannot review; only the
    current model and effort must now be on the role's class for the current model's family."""
    current_family = family_for_model(data, client, current_pair[0])
    if current_family == "unknown":
        raise RuntimeError(f"Session current model family is unresolved for role {role}")
    expected = resolve_class(data, ROLE_CLASS[role], current_family)
    expected_pair = (expected["model"], expected["effort"])
    _require_current(role, current_pair, expected_pair)
    off_class: list[OffClass] = [
        {
            "model": model,
            "effort": effort or "",
            "events": events,
            "family": family_for_model(data, client, model),
        }
        for (model, effort), events in sorted(
            counts.items(), key=lambda i: (i[0][0], i[0][1] or "")
        )
        if (model, effort) != expected_pair
    ]
    return {
        **expected,
        "master_sha256": master_sha,
        "family": "multi-family" if len(families) > 1 else "unknown",
        "families": sorted(families),
        "current_family": current_family,
        "off_class": off_class,
    }


def readiness(
    role: str, client: str, observed: list[ModelUse], current: ModelUse | None
) -> RoleMapping:
    """Return the role mapping for a session that runs its role's class now.

    `verify` judges the whole history. This judges the current selection and records the
    earlier off-class contributions instead of refusing them, for managers and workers.
    """
    if role not in ROLE_CLASS:
        raise RuntimeError(f"Unknown agent role {role!r}")
    if not observed:
        raise RuntimeError("Agent model history is empty")
    if current is None:
        raise RuntimeError("Session current model selection is unresolved")
    data, master_sha = master()
    counts: dict[Pair, int] = {}
    for item in observed:
        pair = _pair_of(data, client, item)
        counts[pair] = counts.get(pair, 0) + 1
    families = {family_for_model(data, client, model) for model, _ in counts}
    current_pair = _pair_of(data, client, current)
    if len(families) != 1 or "unknown" in families:
        return _mixed_readiness(role, data, master_sha, client, counts, families, current_pair)
    if role == "reviewer":
        result = verify(role, client, observed)
    else:
        family = next(iter(families))
        expected = resolve_class(data, ROLE_CLASS[role], family)
        result = {**expected, "master_sha256": master_sha, "families": [family]}
    expected_pair = (result["model"], result["effort"])
    _require_current(role, current_pair, expected_pair)
    off_class: list[OffClass] = [
        {"model": model, "effort": effort or "", "events": events}
        for (model, effort), events in sorted(
            counts.items(), key=lambda i: (i[0][0], i[0][1] or "")
        )
        if (model, effort) != expected_pair
    ]
    if off_class:
        result["off_class"] = off_class
    return result
