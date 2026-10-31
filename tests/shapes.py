"""Runtime shape checks shared by the tests: does a value have the shape a type declares?

`conforms` walks resolved annotations (TypedDicts, unions, Literals, lists, sets, dicts, NotRequired);
`sample` builds a valid value with every optional field filled in; `overlay` adds to a fixture the
fields it lacks. Extra keys in an object are the additive fields a loader preserves, so they are
allowed; every declared key is checked.
"""

from __future__ import annotations

import types
import typing


def typeddict_conforms(value: object, hint: object) -> bool:
    if not isinstance(value, dict):
        return False
    declared = typing.get_type_hints(hint, include_extras=True)
    total = bool(getattr(hint, "__total__", True))
    for name, option in declared.items():
        marker = typing.get_origin(option)
        required = marker is not typing.NotRequired if total else marker is typing.Required
        if name not in value:
            if required:
                return False
        elif not conforms(value[name], option):
            return False
    return True


def conforms(value: object, hint: object) -> bool:
    """Whether `value` has the shape the declared type promises (extra keys in an object are the
    additive fields a loader preserves, so they are allowed; every declared key is checked)."""
    origin = typing.get_origin(hint)
    arguments = typing.get_args(hint)
    verdict: bool
    if hint is typing.Any or hint is object:
        verdict = True
    elif hint is None or hint is type(None):
        verdict = value is None
    elif origin in (typing.NotRequired, typing.Required):
        verdict = conforms(value, arguments[0])
    elif origin is typing.Literal:
        verdict = any(type(value) is type(o) and value == o for o in arguments)
    elif origin in (typing.Union, types.UnionType):
        verdict = any(conforms(value, option) for option in arguments)
    elif origin is list:
        verdict = isinstance(value, list) and all(conforms(e, arguments[0]) for e in value)
    elif origin in (set, frozenset):
        verdict = isinstance(value, set | frozenset) and all(
            conforms(e, arguments[0]) for e in value
        )
    elif origin is dict:
        verdict = isinstance(value, dict) and all(
            conforms(k, arguments[0]) and conforms(v, arguments[1]) for k, v in value.items()
        )
    elif typing.is_typeddict(hint):
        verdict = typeddict_conforms(value, hint)
    elif hint is float:
        verdict = type(value) in (int, float)
    elif isinstance(hint, type):
        verdict = type(value) is hint
    else:
        raise AssertionError(f"conforms() cannot judge {hint!r}")
    return verdict


SCALARS: dict[object, object] = {bool: True, int: 1, float: 1.5, str: "text"}


def sample(hint: object) -> object:
    """A valid value of the declared type, with every optional field filled in, recursively."""
    origin = typing.get_origin(hint)
    arguments = typing.get_args(hint)
    result: object
    if hint is typing.Any or hint is object:
        result = "any"
    elif hint is None or hint is type(None):
        result = None
    elif origin in (typing.NotRequired, typing.Required):
        result = sample(arguments[0])
    elif origin is typing.Literal:
        result = arguments[0]
    elif origin in (typing.Union, types.UnionType):
        result = sample(next(option for option in arguments if option is not type(None)))
    elif origin is list:
        result = [sample(arguments[0])]
    elif origin is dict:
        result = {"key": sample(arguments[1])}
    elif typing.is_typeddict(hint):
        result = {
            name: sample(option)
            for name, option in typing.get_type_hints(hint, include_extras=True).items()
        }
    elif hint in SCALARS:
        result = SCALARS[hint]
    else:
        raise AssertionError(f"sample() cannot build {hint!r}")
    return result


def overlay(fixture: object, filler: object) -> object:
    """The fixture, plus every field the filler has that the fixture lacks (lists pairwise)."""
    if isinstance(fixture, dict) and isinstance(filler, dict):
        merged = {name: overlay(value, filler.get(name)) for name, value in fixture.items()}
        merged.update({name: value for name, value in filler.items() if name not in fixture})
        return merged
    if isinstance(fixture, list) and isinstance(filler, list) and filler:
        return [overlay(item, filler[0]) for item in fixture]
    return fixture
