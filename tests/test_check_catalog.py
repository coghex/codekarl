"""CK-77: the check catalog, its ownership of every test unit, and the planned check.

Three kinds of proof, none of them from labels alone:

- the **commands** of every group are parsed into the test units they run (a package, a target, a
  feature variant and, for the two split runners, a case set) and compared with the units of the whole
  workspace, read from `cargo metadata`: the groups are disjoint and together exactly the full run;
- what each group **needs** (nothing, the GPU, or the desktop's focus) is derived from those units by an
  oracle written here, and must equal the group's own classification, so a group that is not classified
  as taking focus cannot start a window test, directly or through a nested recipe;
- the **planner** and the **change set** are run on fixture catalogs and on a disposable repository.

No test here opens a window or touches the GPU.
"""

import hashlib
import json
import os
import posixpath
import re
import subprocess
import sys
import tempfile
import unittest
from collections.abc import Sequence
from dataclasses import dataclass
from io import StringIO
from pathlib import Path, PurePosixPath
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import check

REPO = Path(__file__).resolve().parents[1]

# A unit of the full run: (package, kind, target, feature variant, case set).
Unit = tuple[str, str, str, str, str]

# The two custom runners that take an explicit case set, and the sets each declares.
SPLIT_RUNNERS = {
    ("ck-gpu-metal", "presenter"): (
        "gpu",
        "window-core",
        "window-scene",
        "window-compat",
        "window-instanced",
    ),
    ("codekarl", "cli_spec"): ("plain", "gpu"),
}

ALL_FEATURES, DEFAULT_FEATURES = "all-features", "default"
# CK-130: the scale acceptance builds as its recipe fixes it, release with `--features metal`; it is not one of the full run's
# variants, so its units are compared with the full run's by target.
RELEASE_METAL = "release-metal"
SCALE_TARGETS = {("ck-render-2d", "test", "scale"), ("codekarl", "test", "scale_lua")}
# CK-162: the same on Vulkan: release with `--features vulkan` (optional group `vulkan.scale`), never `--all-features`.
RELEASE_VULKAN = "release-vulkan"
VULKAN_SCALE_TARGETS = {
    ("ck-render-2d", "test", "vulkan_scale"),
    ("codekarl", "test", "vulkan_scale_lua"),
}


def workspace_targets() -> dict[str, list[tuple[str, str, bool]]]:
    """Every package of the workspace and its test targets: (kind, name, has doctests)."""
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"],
        cwd=REPO,
        check=True,
        capture_output=True,
        text=True,
    )
    targets: dict[str, list[tuple[str, str, bool]]] = {}
    for package in json.loads(result.stdout)["packages"]:
        found = targets.setdefault(package["name"], [])
        for target in package["targets"]:
            kind = target["kind"][0]
            if target["test"] and kind in ("lib", "bin", "test"):
                found.append((kind, target["name"], bool(target.get("doctest"))))
    return targets


def full_units(targets: dict[str, list[tuple[str, str, bool]]]) -> set[Unit]:
    """The units of today's full run: `cargo test --workspace --all-features`, and `codekarl` again
    without its features (CK-44). A split runner's units are its case sets (all-features only)."""
    units: set[Unit] = set()
    for package, found in targets.items():
        for kind, name, doctests in found:
            sets = SPLIT_RUNNERS.get((package, name), ("*",))
            for case in sets:
                units.add((package, kind, name, ALL_FEATURES, case))
            if kind == "lib" and doctests:
                units.add((package, "doc", name, ALL_FEATURES, "*"))
            if package == "codekarl":
                units.add((package, kind, name, DEFAULT_FEATURES, "*"))
                if (
                    kind == "lib" and doctests
                ):  # CK-131: the application is a library now, with doctests
                    units.add((package, "doc", name, DEFAULT_FEATURES, "*"))
    return units


def parse_cargo_test(
    command: Sequence[str], targets: dict[str, list[tuple[str, str, bool]]]
) -> tuple[list[str], list[tuple[str, str]], str, str | None]:
    """A `cargo test` command's packages, explicit target selectors, feature variant and case set. Anything
    it does not understand is an error, so a new kind of selection cannot slip past the ownership proof."""
    assert list(command[:2]) == ["cargo", "test"], command
    words = list(command[2:])
    after: list[str] = []
    if "--" in words:
        split = words.index("--")
        words, after = words[:split], words[split + 1 :]
    packages: list[str] = []
    explicit: list[tuple[str, str]] = []
    variant = DEFAULT_FEATURES
    position = 0
    while position < len(words):
        word = words[position]
        position += 1
        if word == "-p":
            packages.append(words[position])
            position += 1
        elif word == "--workspace":
            packages = list(targets)
        elif word == "--all-features":
            variant = ALL_FEATURES
        elif word in ("--bin", "--test"):
            explicit.append((word[2:], words[position]))
            position += 1
        elif word in ("--lib", "--doc"):
            explicit.append((word[2:], ""))
        elif word == "--features":  # CK-130, CK-162: only the scale recipe builds, in release
            assert words[position] in ("metal", "vulkan") and "--release" in words, command
            variant = RELEASE_METAL if words[position] == "metal" else RELEASE_VULKAN
            position += 1
        elif word not in ("--locked", "--release"):  # CK-130: the scale acceptance runs in release
            raise AssertionError(f"unrecognised cargo test argument {word!r} in {command}")
    chosen_set: str | None = None
    if after:
        assert after[0] == "--set", after
        chosen_set = after[1]
        assert after == ["--set", chosen_set], after
    return packages, explicit, variant, chosen_set


def units_of(command: Sequence[str], targets: dict[str, list[tuple[str, str, bool]]]) -> set[Unit]:
    """The test units one `cargo test` command runs."""
    packages, explicit, variant, chosen_set = parse_cargo_test(command, targets)
    units: set[Unit] = set()
    for package in packages:
        for kind, name, doctests in targets[package]:
            selected = (
                not explicit
                or (kind, name) in explicit
                or (kind == "lib" and ("lib", "") in explicit)
            )
            if selected:
                units |= expand(package, kind, name, variant, chosen_set)
            if kind == "lib" and doctests and (not explicit or ("doc", "") in explicit):
                units.add((package, "doc", name, variant, "*"))
    return units


def expand(package: str, kind: str, name: str, variant: str, chosen: str | None) -> set[Unit]:
    """The units of one target: a split runner under all features runs the sets it is asked for (all of
    them without `--set`); every other target is one unit, and takes no `--set`."""
    sets = SPLIT_RUNNERS.get((package, name))
    if sets and variant == ALL_FEATURES:
        assert chosen is None or chosen in sets, (package, name, chosen)
        return {(package, kind, name, variant, case) for case in ((chosen,) if chosen else sets)}
    assert chosen is None, f"{package} {name} takes no case set"
    return {(package, kind, name, variant, "*")}


def unit_need(unit: Unit) -> str:
    """What one unit needs, from this oracle and not from any group's label: the Cocoa backend's tests
    and the presenter's windowed set and the application's real-window test open windows; the rest of the
    Metal crate and the application's GPU case need the GPU (or its toolchain) but no window."""
    package, _, name, variant, case = unit
    # CK-130: the scale acceptance's two harnesses open a 1920 x 1080 window (optional group `metal.scale`).
    windowed = {
        ("ck-window-glfw", "window_glfw"),
        ("ck-render-2d", "metal_window"),
        ("ck-render-2d", "scale"),
        (
            "ck-render-2d",
            "vulkan_scale",
        ),  # CK-162: the same recipe on a real GLFW window (optional group `vulkan.scale`)
        ("ck-render-2d", "vulkan_window"),  # CK-159: the resource path on a real GLFW window
    }
    if package == "ck-window-cocoa" or (package, name) in windowed:
        return "window"
    if package == "ck-gpu-vulkan":
        return "window" if name == "window" else "gpu"
    if package == "ck-gpu-metal":
        return "window" if (name == "presenter" and case != "gpu") else "gpu"
    if package == "codekarl" and variant in (ALL_FEATURES, RELEASE_METAL, RELEASE_VULKAN):
        if name in ("windowed", "vulkan_app", "scale_lua", "vulkan_scale_lua"):
            return "window"
        if name == "cli_spec" and case == "gpu":
            return "gpu"
    return "plain"


def need_of(unit: Unit) -> str:
    """`unit_need`, except that a hook-isolation target (CK-84), which only reads source files, is plain in the
    Metal and Vulkan crates alike."""
    return "plain" if unit[2] == "hook_isolation" else unit_need(unit)


def presenter_modules() -> set[str]:
    """Follow the presenter's external `#[path]` modules, including nested scene modules."""
    root = REPO / "crates/gpu-metal/tests/presenter.rs"
    pending = [root]
    found: set[str] = set()
    path_pattern = re.compile(r'#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]')
    while pending:
        source = pending.pop()
        relative = source.relative_to(REPO).as_posix()
        if relative in found:
            continue
        found.add(relative)
        for target in path_pattern.findall(source.read_text()):
            pending.append((source.parent / target).resolve())
    return found


def presenter_case_names(source: str) -> list[str]:
    """Names from the presenter's typed test-case arrays, without scanning unrelated strings."""
    declaration = re.compile(
        r"pub\(crate\)\s+const\s+\w+\s*:\s*\[\s*\(&str,\s*fn\(\)\);\s*\d+\s*\]\s*=\s*\["
    )
    names: list[str] = []
    for match in declaration.finditer(source):
        start = match.end()
        depth = 1
        end = start
        while end < len(source) and depth:
            if source[end] == "[":
                depth += 1
            elif source[end] == "]":
                depth -= 1
            end += 1
        if depth:
            continue
        names.extend(re.findall(r'\(\s*"([^"]+)"\s*,', source[start : end - 1]))
    return names


def group_units(step: check.Step, targets: dict[str, list[tuple[str, str, bool]]]) -> set[Unit]:
    """Every test unit the group's commands run."""
    units: set[Unit] = set()
    for command in step.commands() if step.action is None else []:
        if command[:2] == ["cargo", "test"]:
            units |= units_of(command, targets)
    return units


TARGETS = workspace_targets()


class Ownership(unittest.TestCase):
    """Every unit of the full run belongs to exactly one group, derived from the commands."""

    def test_01_the_groups_run_disjoint_units_that_together_are_exactly_the_full_run(self) -> None:
        owners: dict[Unit, list[str]] = {}
        for step in check.STEPS:
            for unit in group_units(step, TARGETS):
                owners.setdefault(unit, []).append(step.name)
        shared = {unit: names for unit, names in owners.items() if len(names) > 1}
        self.assertEqual(shared, {}, "a unit is run by two groups")
        full = full_units(TARGETS)
        # CK-130: `metal.scale` runs the two scale harnesses as the recipe builds them (release, `--features metal`), which is
        # none of the full run's variants. Those units are matched by target: they are exactly the scale harnesses, and the full
        # run's all-features instances of those targets (no-ops without `CK_SCALE_RUN`) are the ones they stand for.
        scale_build = {unit for unit in owners if unit[3] == RELEASE_METAL}
        self.assertEqual({unit[:3] for unit in scale_build}, SCALE_TARGETS)
        self.assertEqual({owners[unit][0] for unit in scale_build}, {"metal.scale"})
        # CK-162: and `vulkan.scale` runs its two as the Vulkan recipe builds them (release, `--features vulkan`), by target the same way.
        vulkan_scale_build = {unit for unit in owners if unit[3] == RELEASE_VULKAN}
        self.assertEqual({unit[:3] for unit in vulkan_scale_build}, VULKAN_SCALE_TARGETS)
        self.assertEqual({owners[unit][0] for unit in vulkan_scale_build}, {"vulkan.scale"})
        scale_build |= vulkan_scale_build
        every_scale_target = SCALE_TARGETS | VULKAN_SCALE_TARGETS
        stood_for = {
            unit for unit in full if unit[:3] in every_scale_target and unit[3] == ALL_FEATURES
        }
        self.assertEqual({unit[:3] for unit in stood_for}, every_scale_target)
        self.assertEqual(
            sorted(full - owners.keys() - stood_for),
            [],
            "a unit of the full run belongs to no group",
        )
        self.assertEqual(
            sorted(owners.keys() - full - scale_build),
            [],
            "a group runs something the full run does not",
        )

    def test_02_the_oracle_matches_every_groups_classification(self) -> None:
        rank = {"plain": 0, "gpu": 1, "window": 2}
        for step in check.STEPS:
            units = group_units(step, TARGETS)
            needed = max((need_of(unit) for unit in units), default="plain", key=rank.__getitem__)
            self.assertEqual(
                step.needs, needed, f"{step.name} is classified {step.needs}, runs {needed}"
            )
            if step.needs == "plain":
                self.assertEqual([u for u in units if need_of(u) != "plain"], [], step.name)

    def test_03_only_the_window_groups_run_a_window_unit_and_only_the_gpu_groups_a_gpu_unit(
        self,
    ) -> None:
        by_need: dict[str, set[str]] = {"gpu": set(), "window": set()}
        for step in check.STEPS:
            for unit in group_units(step, TARGETS):
                if need_of(unit) != "plain":
                    by_need[need_of(unit)].add(step.name)
        self.assertEqual(
            by_need["window"],
            {
                "native.window",
                "metal.window-core",
                "metal.window-scene",
                "metal.window-compat",
                "metal.window-instanced",
                "metal.scale",
                "vulkan.scale",
                "glfw.window",
                "vulkan.window",
                "render2d.vulkan-window",
                "vulkan.app",
            },
        )
        self.assertEqual(by_need["gpu"], {"metal.gpu", "vulkan.gpu"})

    def test_04_the_floor_and_python_groups_run_no_cargo_test_and_nothing_nested_that_does(
        self,
    ) -> None:
        for step in check.STEPS:
            if (
                step.category == "floor"
                or step.name.startswith("unittest-")
                or step.name == "chat-ci"
            ):
                self.assertEqual(group_units(step, TARGETS), set(), step.name)
                self.assertEqual(step.needs, "plain", step.name)
        # `check.py` is started by one Python test only: the real-toolchain lease test, over these groups.
        self.assertEqual(check.REAL_TOOLCHAIN_GROUPS, ("rust.core",))
        for name in check.REAL_TOOLCHAIN_GROUPS:
            step = next(step for step in check.STEPS if step.name == name)
            self.assertEqual(step.needs, "plain")
            self.assertTrue(group_units(step, TARGETS), "the real workload runs real Cargo tests")

    def test_05_the_real_check_py_runs_of_the_suite_are_planned_runs_over_the_focus_free_groups(
        self,
    ) -> None:
        """A real run of the full level would start the native window tests. The suite starts `check.py`
        for real in exactly two places (the real-toolchain lease test), both with the planned arguments."""
        source = (REPO / "tests/test_check.py").read_text()
        self.assertIn('REAL_CHECK_ARGUMENTS = ["--group", *check.REAL_TOOLCHAIN_GROUPS]', source)
        starts = source.count('str(copy / "tools/check.py")')
        guarded = source.count('str(copy / "tools/check.py"), *REAL_CHECK_ARGUMENTS')
        self.assertEqual((starts, guarded), (2, 2))
        for path in sorted((REPO / "tests").glob("test_*.py")):
            if path.name in ("test_check.py", "test_check_catalog.py"):
                continue
            self.assertNotIn('check.py"), cwd', path.read_text(), path.name)

    def test_06_each_split_runner_declares_exactly_the_sets_the_groups_use(self) -> None:
        declared = {
            ("ck-gpu-metal", "presenter"): re.findall(
                r'\("([\w-]+)", CaseSet::\w+\)',
                section((REPO / "crates/gpu-metal/tests/presenter.rs").read_text(), "const SETS"),
            ),
            ("codekarl", "cli_spec"): re.findall(
                r'\("(\w+)", Self::\w+\)',
                section(
                    (REPO / "apps/codekarl/tests/cli_spec/support.rs").read_text(),
                    "pub(crate) const ALL",
                ),
            ),
        }
        self.assertEqual({key: tuple(value) for key, value in declared.items()}, SPLIT_RUNNERS)
        used: dict[tuple[str, str], list[str]] = {key: [] for key in SPLIT_RUNNERS}
        for step in check.STEPS:
            for unit in group_units(step, TARGETS):
                if (
                    unit[:1] + unit[2:3] in [(p, n) for (p, n) in SPLIT_RUNNERS]
                    and unit[3] == ALL_FEATURES
                ):
                    used[(unit[0], unit[2])].append(unit[4])
        for key, sets in SPLIT_RUNNERS.items():
            self.assertEqual(sorted(used[key]), sorted(sets), f"{key}: each set runs once")

    def test_07_the_default_variant_of_the_real_window_target_is_empty(self) -> None:
        text = (REPO / "apps/codekarl/tests/windowed.rs").read_text()
        self.assertIn('#![cfg(all(feature = "metal", target_os = "macos"))]', text)


def section(text: str, marker: str) -> str:
    """The text from `marker` to the end of its statement (the next line ending in `;`)."""
    start = text.index(marker)
    return text[start : text.index(";\n", start) + 1] if ";\n" in text[start:] else text[start:]


class Catalog(unittest.TestCase):
    """The catalog's own rules."""

    def test_01_the_real_catalog_is_valid(self) -> None:
        self.assertEqual(check.catalog_problems(check.STEPS), [])
        check.verify_catalog()

    def test_02_every_tracked_path_is_claimed_and_focus_groups_are_selected_only_by_their_own_paths(
        self,
    ) -> None:
        listing = subprocess.run(
            ["pijul", "list"], cwd=REPO, check=True, capture_output=True, text=True
        )
        paths = [name for name in listing.stdout.splitlines() if (REPO / name).is_file()]
        self.assertGreater(len(paths), 100)
        focus = {step.name for step in check.STEPS if step.needs != "plain"}
        native_paths = (
            "crates/gpu-vulkan/",
            "crates/window-glfw/",
            "crates/window-cocoa/",
            "crates/gpu-metal/",
            *check.NATIVE_APP_FILES,  # CK-131: the windowed tests, their build script and native source
            *check.RENDER2D_WINDOW_FILES,  # CK-92: the Metal backend and its real-window test
            "crates/render-2d/src/vulkan",  # CK-159: the Vulkan sink side, which its real-window test drives
            "crates/render-2d/tests/vulkan_window.rs",
            "apps/codekarl/tests/supervisor/",  # CK-84: compiled into the GLFW and Vulkan test targets
            "apps/codekarl/src/vulkan",  # CK-80: vulkan.rs and vulkan/ (the path's own files)
            "apps/codekarl/tests/vulkan_app.rs",
            "config/demo-glfw-vulkan",
            "config/demo-quad-vulkan.toml",  # CK-99: run by the headless case of vulkan_app.rs
            "apps/codekarl/Cargo.toml",  # CK-99: carries the `vulkan` feature's wiring
            "specs/bootstrap.json",
        )
        for path in paths:
            chosen = check.plan(check.STEPS, [path])
            self.assertFalse(
                [choice for choice in chosen.choices if "claimed by no group" in choice.why],
                f"{path} is claimed by no group: it would fail wide",
            )
            taking_focus = [
                choice.step.name
                for choice in chosen.choices
                if choice.selected and choice.step.name in focus
            ]
            if taking_focus:
                self.assertTrue(path.startswith(native_paths), f"{path} selects {taking_focus}")

    def test_03_shared_and_other_backend_paths_never_select_a_focus_group(self) -> None:
        for path in (
            "crates/window/src/main_loop.rs",
            "crates/log/src/lib.rs",
            "crates/lua/src/lib.rs",
            "crates/runtime/src/lib.rs",
            "apps/codekarl/src/main.rs",
            "apps/codekarl/src/windowed.rs",
            "apps/codekarl/tests/modes.rs",
            "config/codekarl.toml",
            "scripts/ui/demo.lua",
            "Cargo.toml",
            "Cargo.lock",
            "tools/check.py",
            "tests/test_check.py",
            "AGENTS.md",
            "docs/tooling.md",
        ):
            chosen = [c.step for c in check.plan(check.STEPS, [path]).choices if c.selected]
            self.assertEqual([s.name for s in chosen if s.needs != "plain"], [], path)

    def test_04_a_new_crate_is_unclaimed_until_a_group_registers_it(self) -> None:
        # (A crate of the future: `crates/gpu-vulkan/` itself is registered since CK-79.)
        chosen = check.plan(check.STEPS, ["crates/gpu-future/src/lib.rs"])
        affected = [c for c in chosen.choices if c.step.category == "affected"]
        self.assertTrue(all(c.selected and "claimed by no group" in c.why for c in affected))
        future = check.Step(
            "future.window",
            check.FAST,
            list,
            description="The future crate's window tests.",
            paths=("crates/gpu-future/",),
            needs="window",
        )
        catalog = [*check.STEPS, future]
        self.assertEqual(check.catalog_problems(catalog), [])
        selected = [
            c.step.name
            for c in check.plan(catalog, ["crates/gpu-future/src/lib.rs"]).choices
            if c.selected
        ]
        floor = [s.name for s in check.STEPS if s.category == "floor"]
        self.assertEqual(selected, [*floor, "future.window"])
        self.assertNotIn("native.window", selected)
        self.assertNotIn("metal.gpu", selected)

    def test_05_invalid_catalogs_are_refused_naming_the_group(self) -> None:
        def step(name: str = "g.one", **fields: object) -> check.Step:
            base: dict[str, object] = {
                "description": "A group.",
                "category": "affected",
                "paths": ("a/",),
            }
            base.update(fields)
            return check.Step(name, check.FAST, list, **base)  # type: ignore[arg-type]

        cases = {
            "id": [step("Bad_Id")],
            "twice": [step(), step()],
            "description": [step(description=" ")],
            "category": [step(category="local-only")],
            "needs": [step(needs="focus")],
            "paths": [step(paths=())],
            "pattern": [step(paths=("a/*",))],
        }
        for label, catalog in cases.items():
            with self.subTest(label):
                problems = check.catalog_problems(catalog)
                self.assertTrue(problems, label)
                self.assertTrue(any("g.one" in p or "Bad_Id" in p for p in problems), problems)
        self.assertEqual(check.catalog_problems([step(category="floor", paths=())]), [])
        self.assertEqual(check.catalog_problems([step(category="optional", paths=())]), [])

    def test_06_patterns_follow_the_moskophoros_rules(self) -> None:
        for bad in (
            "",
            " a/",
            "a/ ",
            "/a",
            "a\\b",
            "a*",
            "a?",
            "a[b]",
            "a//b",
            "./a",
            "a/./b",
            "a/../b",
            "../a",
            ".",
        ):
            self.assertIsNotNone(check.pattern_problem(bad), repr(bad))
        for good in ("a", "a/", "a/b.rs", "a/b/", ".cargo/", "Cargo.toml"):
            self.assertIsNone(check.pattern_problem(good), repr(good))
        self.assertIsNotNone(check.pattern_problem(3))
        # A directory pattern claims everything beneath it and not look-alikes; a file pattern one file.
        self.assertTrue(check.claims("src/", "src/a.py"))
        self.assertTrue(check.claims("src/", "src/x/y/z.py"))
        self.assertFalse(check.claims("src/", "srcs/a.py"))
        self.assertFalse(check.claims("src/", "lib/src/a.py"))
        self.assertFalse(check.claims("src/", "src"))
        self.assertTrue(check.claims("pyproject.toml", "pyproject.toml"))
        self.assertFalse(check.claims("pyproject.toml", "pyproject.toml.bak"))
        self.assertFalse(check.claims("pyproject.toml", "sub/pyproject.toml"))
        self.assertFalse(check.claims("src", "src/a.py"))


def fixture_catalog() -> list[check.Step]:
    def step(
        name: str, category: str, paths: tuple[str, ...] = (), needs: str = "plain"
    ) -> check.Step:
        return check.Step(
            name,
            check.FAST,
            list,
            description=f"{name}.",
            category=category,
            paths=paths,
            needs=needs,
        )

    return [
        step("lint", "floor"),
        step("core", "affected", ("core/", "Cargo.toml")),
        step("native", "affected", ("native/", "core/native.rs"), "window"),
        step("slow", "optional", ("slow/",)),
        step("last", "floor"),
    ]


class Planner(unittest.TestCase):
    """Category selection, patterns and the explanation of every group, on a fixture catalog."""

    def chosen(
        self, paths: Sequence[str] | None, requested: Sequence[str] = (), failure: str | None = None
    ) -> dict[str, tuple[bool, str]]:
        built = check.plan(fixture_catalog(), paths, requested, failure)
        return {choice.step.name: (choice.selected, choice.why) for choice in built.choices}

    def test_01_floor_always_runs_and_affected_groups_follow_their_paths(self) -> None:
        result = self.chosen(["core/a.rs"])
        self.assertEqual(
            {n: s for n, (s, _) in result.items()},
            {"lint": True, "core": True, "native": False, "slow": False, "last": True},
        )
        self.assertIn("core/a.rs matches core/", result["core"][1])
        self.assertEqual(result["lint"][1], "floor: always runs")
        self.assertIn("no changed path matches", result["native"][1])
        self.assertIn("optional", result["slow"][1])

    def test_02_every_group_gets_a_reason_in_the_printed_plan(self) -> None:
        built = check.plan(fixture_catalog(), ["core/native.rs"])
        lines = built.lines()
        self.assertEqual(len(lines), 5)
        for choice, line in zip(built.choices, lines, strict=True):
            self.assertIn(choice.step.name, line)
            self.assertIn(choice.why, line)
            self.assertIn("run " if choice.selected else "skip", line)
        self.assertEqual([s.name for s in built.steps()], ["lint", "core", "native", "last"])

    def test_03_an_empty_change_set_runs_the_floor_only(self) -> None:
        result = self.chosen([])
        self.assertEqual([n for n, (s, _) in result.items() if s], ["lint", "last"])

    def test_04_an_unclaimed_path_selects_every_affected_group_and_says_which(self) -> None:
        result = self.chosen(["core/a.rs", "mystery/new.txt", "other.txt"])
        self.assertEqual(
            [n for n, (s, _) in result.items() if s], ["lint", "core", "native", "last"]
        )
        self.assertIn("mystery/new.txt", result["native"][1])
        self.assertIn("claimed by no group", result["native"][1])
        self.assertFalse(result["slow"][0], "an optional group is never selected by fail-wide")

    def test_05_a_failure_to_compute_the_change_set_fails_wide_too(self) -> None:
        result = self.chosen(None, failure="no accepted pointer")
        self.assertEqual(
            [n for n, (s, _) in result.items() if s], ["lint", "core", "native", "last"]
        )
        self.assertIn("no accepted pointer", result["core"][1])
        self.assertIn("could not be computed", result["core"][1])

    def test_06_named_groups_run_and_an_optional_one_only_when_named(self) -> None:
        result = self.chosen(["core/a.rs"], ["native", "slow"])
        self.assertEqual(result["native"], (True, "requested by name"))
        self.assertEqual(result["slow"], (True, "requested by name"))
        only = self.chosen(None, ["core"])
        self.assertEqual([n for n, (s, _) in only.items() if s], ["lint", "core", "last"])
        self.assertIn("no change set was asked for", only["native"][1])

    def test_07_an_unknown_group_name_is_an_error_listing_the_groups(self) -> None:
        with self.assertRaises(ValueError) as caught:
            check.plan(fixture_catalog(), [], ["nope"])
        self.assertIn("nope", str(caught.exception))
        self.assertIn("native", str(caught.exception))

    def test_08_more_than_one_matching_path_is_counted_and_many_unclaimed_paths_are_abbreviated(
        self,
    ) -> None:
        result = self.chosen(["core/a.rs", "core/b.rs", "Cargo.toml"])
        self.assertIn("(and 2 more)", result["core"][1])
        many = self.chosen([f"z/{n}.txt" for n in range(5)])
        self.assertIn("and 2 more", many["core"][1])

    def test_09_the_real_plan_for_a_window_crate_change_selects_the_neutral_groups_only(
        self,
    ) -> None:
        names = {
            c.step.name: c.selected
            for c in check.plan(check.STEPS, ["crates/window/src/lib.rs"]).choices
        }
        self.assertTrue(names["rust.core"] and names["rust.app"] and names["rust.app-default"])
        self.assertFalse(names["native.window"] or names["metal.gpu"])
        self.assertFalse(names["chat-ci"])
        names = {
            c.step.name: c.selected
            for c in check.plan(check.STEPS, ["crates/gpu-metal/src/presenter.rs"]).choices
        }
        self.assertTrue(names["native.window"] and names["metal.gpu"])
        names = {
            c.step.name: c.selected
            for c in check.plan(check.STEPS, ["crates/gpu-metal/src/lib.rs"]).choices
        }
        self.assertTrue(names["metal.gpu"])
        self.assertFalse(names["native.window"])
        self.assertFalse(names["rust.core"] or names["rust.app"])
        names = {
            c.step.name: c.selected
            for c in check.plan(check.STEPS, ["apps/codekarl/src/native.rs"]).choices
        }
        self.assertTrue(names["native.window"] and names["metal.gpu"] and names["rust.app"])


class DirectoryEntries(unittest.TestCase):
    """An untracked directory is a changed-path entry ending in `/`: it is not walked, so it selects
    every group with a pattern inside it or a directory pattern containing it, and fails wide unless it
    lies wholly inside one group's directory pattern."""

    def chosen(
        self, paths: Sequence[str], catalog: Sequence[check.Step] | None = None
    ) -> dict[str, bool]:
        built = check.plan(fixture_catalog() if catalog is None else catalog, paths)
        return {choice.step.name: choice.selected for choice in built.choices}

    def test_01_a_directory_inside_one_groups_directory_pattern_selects_that_group_only(
        self,
    ) -> None:
        self.assertEqual(
            self.chosen(["core/new/"]),
            {"lint": True, "core": True, "native": False, "slow": False, "last": True},
        )
        self.assertEqual(
            self.chosen(["native/deep/er/"]),
            {"lint": True, "core": False, "native": True, "slow": False, "last": True},
        )

    def test_02_a_directory_spanning_several_groups_selects_each_and_unclaimed_ones_fail_wide(
        self,
    ) -> None:
        # `core/` is the pattern of one group and holds the file pattern of another: both run, and it
        # does not fail wide, since it lies entirely within one group's directory pattern.
        result = check.plan(fixture_catalog(), ["core/"])
        by_name = {c.step.name: c for c in result.choices}
        self.assertTrue(by_name["core"].selected and by_name["native"].selected)
        self.assertFalse(by_name["slow"].selected)
        self.assertFalse([c for c in result.choices if "claimed by no group" in c.why])
        # `a/` holds two groups' directories but lies within none: it spans them, and fails wide.
        two = [
            check.Step("one", check.FAST, list, description="1.", paths=("a/b/",)),
            check.Step("two", check.FAST, list, description="2.", paths=("a/c/",)),
            check.Step("other", check.FAST, list, description="3.", paths=("z/",)),
        ]
        spanning = check.plan(two, ["a/"])
        self.assertTrue(all(c.selected for c in spanning.choices))
        self.assertIn("a/ is claimed by no group", spanning.choices[0].why)
        # Not under any directory pattern: fail wide, naming the directory.
        wide = check.plan(fixture_catalog(), ["brand-new/"])
        self.assertTrue(all(c.selected for c in wide.choices if c.step.category == "affected"))
        core = next(c for c in wide.choices if c.step.name == "core")
        self.assertIn("brand-new/", core.why)

    def test_03_a_file_pattern_inside_the_directory_selects_its_group(self) -> None:
        result = check.plan(fixture_catalog(), ["core/"])
        by_name = {c.step.name: c for c in result.choices}
        self.assertIn(
            "matches", by_name["native"].why + "matches"
        )  # selected through fail-wide or touch
        only = [
            check.Step("a.file", check.FAST, list, description="a.", paths=("pkg/one.txt",)),
            check.Step("b.dir", check.FAST, list, description="b.", paths=("pkg/sub/",)),
            check.Step("c.dir", check.FAST, list, description="c.", paths=("pkg/sub/inner/",)),
            check.Step("whole", check.FAST, list, description="w.", paths=("pkg/",)),
        ]
        # `pkg/sub/` is wholly inside `pkg/` (claimed) and touches the group that has a pattern inside it.
        result = check.plan(only, ["pkg/sub/"])
        self.assertEqual(
            {c.step.name: c.selected for c in result.choices},
            {"a.file": False, "b.dir": True, "c.dir": True, "whole": True},
        )
        self.assertFalse([c for c in result.choices if "claimed by no group" in c.why])

    def test_04_the_real_catalog_for_new_crates_and_directories(self) -> None:
        wide = check.plan(check.STEPS, ["crates/gpu-future/"])
        self.assertTrue(
            all(c.selected for c in wide.choices if c.step.category == "affected"),
            "an unregistered new crate fails wide",
        )
        future = check.Step(
            "future.window",
            check.FAST,
            list,
            description="v.",
            paths=("crates/gpu-future/",),
            needs="window",
        )
        narrow = self.chosen(["crates/gpu-future/"], [*check.STEPS, future])
        self.assertTrue(narrow["future.window"])
        for other in ("native.window", "metal.gpu", "rust.core", "rust.app", "unittest-batch"):
            self.assertFalse(narrow[other], other)
        # A new directory inside the application's directory pattern selects the groups that claim it.
        inside = self.chosen(["apps/codekarl/src/new_module/"], check.STEPS)
        self.assertTrue(
            inside["rust.app"] and inside["rust.app-default"] and inside["unittest-lints"]
        )
        self.assertFalse(inside["native.window"] or inside["metal.gpu"] or inside["rust.core"])
        # The whole of `crates/` spans every crate: everything it holds is selected and it fails wide.
        everything = self.chosen(["crates/"], check.STEPS)
        self.assertTrue(
            everything["native.window"] and everything["metal.gpu"] and everything["rust.core"]
        )

    def test_05_a_directory_entry_is_not_a_file_pattern_match_of_a_look_alike(self) -> None:
        self.assertFalse(check.touches("src/", "srcs/"))
        self.assertFalse(check.touches("src/a.py", "src/b/"))
        self.assertTrue(check.touches("src/a.py", "src/"))
        self.assertTrue(check.touches("src/", "src/x/"))
        self.assertTrue(check.touches("src/", "src/"))
        self.assertFalse(check.touches("lib/", "src/"))
        self.assertTrue(check.is_claimed(fixture_catalog(), "core/x/"))
        self.assertTrue(check.is_claimed(fixture_catalog(), "core/"))
        self.assertFalse(check.is_claimed(fixture_catalog(), "brand-new/"))
        self.assertFalse(check.is_claimed(fixture_catalog(), "Cargo.toml/"))


class GlfwGroups(unittest.TestCase):
    """CK-78: the GLFW backend's groups, in the real catalog."""

    def selected(self, path: str) -> set[str]:
        return {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_01_the_crates_paths_select_its_groups_and_never_the_native_ones(self) -> None:
        chosen = self.selected("crates/window-glfw/src/backend.rs")
        self.assertTrue({"glfw.core", "glfw.window", "doctor.graphics", "unittest-lints"} <= chosen)
        for other in ("native.window", "metal.gpu", "rust.core", "rust.app", "rust.app-default"):
            self.assertNotIn(other, chosen, other)

    def test_02_the_window_group_is_selected_only_by_the_crates_own_paths(self) -> None:
        for path in (
            "crates/window/src/main_loop.rs",
            "crates/window-cocoa/src/backend.rs",
            "crates/gpu-metal/src/lib.rs",
            "apps/codekarl/src/main.rs",
            "Cargo.lock",
            "Cargo.toml",
            "tools/check.py",
            "tools/bootstrap",
        ):
            self.assertNotIn("glfw.window", self.selected(path), path)
        self.assertIn("glfw.window", self.selected("crates/window-glfw/tests/window_glfw.rs"))
        # The plain group follows the window core and the toolchain files; it opens no window.
        self.assertIn("glfw.core", self.selected("crates/window/src/lib.rs"))
        self.assertIn("glfw.core", self.selected("Cargo.lock"))

    def test_03_the_bootstrap_selects_the_graphics_doctor(self) -> None:
        self.assertIn("doctor.graphics", self.selected("tools/bootstrap"))
        self.assertNotIn("doctor.graphics", self.selected("crates/window/src/lib.rs"))
        step = next(step for step in check.STEPS if step.name == "doctor")
        with mock.patch.object(check, "doctor") as doctor:
            assert step.action is not None
            step.action()
        doctor.assert_called_once_with(graphics=False)  # the floor group never needs GLFW


class InputGroup(unittest.TestCase):
    """CK-120: the portable key mapper is pure and owns only its library and doc units."""

    def selected(self, path: str) -> set[str]:
        return {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_01_crate_paths_select_the_pure_group_and_lints_only(self) -> None:
        for path in ("crates/input/src/lib.rs", "crates/input/Cargo.toml"):
            chosen = self.selected(path)
            self.assertTrue({"input.core", "unittest-lints"} <= chosen, path)
            for other in (
                "metal.gpu",
                "native.window",
                "glfw.window",
                "vulkan.window",
                "vulkan.gpu",
                "vulkan.app",
            ):
                self.assertNotIn(other, chosen, f"{path}: {other}")

    def test_02_workspace_toolchain_changes_select_the_pure_group_only(self) -> None:
        chosen = self.selected("Cargo.lock")
        self.assertIn("input.core", chosen)
        self.assertNotIn("metal.gpu", chosen)
        self.assertNotIn("native.window", chosen)

    def test_03_it_runs_exactly_the_plain_library_and_doc_units(self) -> None:
        step = next(step for step in check.STEPS if step.name == "input.core")
        units = group_units(step, TARGETS)
        self.assertEqual(
            {(kind, name) for _, kind, name, _, _ in units},
            {("lib", "ck_input"), ("doc", "ck_input")},
        )
        self.assertEqual({unit[0] for unit in units}, {"ck-input"})
        self.assertEqual({need_of(unit) for unit in units}, {"plain"})
        self.assertEqual(step.needs, "plain")
        self.assertEqual(step.category, "affected")


class WindowBuildsOnInput(unittest.TestCase):
    """CK-121: `ck-window` depends on `ck-input`, so a change to the mapper reruns what builds on the window core."""

    def selected(self, path: str) -> set[str]:
        return {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_01_the_mapper_selects_the_groups_that_build_on_the_window_core(self) -> None:
        for path in (
            "crates/input/src/lib.rs",
            "crates/input/Cargo.toml",
            "crates/input/src/tests.rs",
        ):
            chosen = self.selected(path)
            self.assertTrue(
                {
                    "input.core",
                    "rust.core",
                    "rust.app",
                    "rust.app-default",
                    "render2d.core",
                    "glfw.core",
                }
                <= chosen,
                f"{path}: {sorted(chosen)}",
            )
            for window_group in (
                "glfw.window",
                "vulkan.window",
                "vulkan.gpu",
                "vulkan.app",
                "metal.gpu",
            ):
                self.assertNotIn(window_group, chosen, f"{path}: {window_group}")

    def test_02_the_window_core_still_selects_the_same_groups_and_input_is_one_of_the_neutral_crates(
        self,
    ) -> None:
        self.assertIn("crates/input/", check.NEUTRAL_CRATES)
        chosen = self.selected("crates/window/src/main_loop.rs")
        self.assertTrue(
            {"rust.core", "rust.app", "glfw.core", "render2d.core"} <= chosen, sorted(chosen)
        )
        self.assertIn("native.window", self.selected("crates/window-cocoa/native/window.m"))


class Render2dGroup(unittest.TestCase):
    """CK-90: the 2D renderer's core group, in the real catalog."""

    def selected(self, path: str) -> set[str]:
        return {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_01_the_crates_paths_select_its_group_and_never_a_window_or_gpu_group(self) -> None:
        for path in (
            "crates/render-2d/src/lib.rs",
            "crates/render-2d/src/budget.rs",
            "crates/render-2d/src/tests/resources.rs",
        ):
            chosen = self.selected(path)
            self.assertTrue({"render2d.core", "unittest-lints"} <= chosen, path)
            for other in (
                "native.window",
                "metal.gpu",
                "glfw.window",
                "vulkan.window",
                "vulkan.gpu",
                "vulkan.app",
                "rust.core",
                "rust.app",
            ):
                self.assertNotIn(other, chosen, f"{path}: {other}")

    def test_02_what_it_builds_on_and_the_toolchain_select_it_and_no_native_group(self) -> None:
        for path in (
            "crates/window/src/lib.rs",
            "crates/log/src/lib.rs",
            "crates/resources/src/lib.rs",  # CK-105: it builds on the resource component
            "Cargo.lock",
        ):
            chosen = self.selected(path)
            self.assertIn("render2d.core", chosen, path)
            self.assertNotIn("native.window", chosen, path)
            self.assertNotIn("metal.gpu", chosen, path)
        self.assertNotIn("render2d.core", self.selected("crates/window-glfw/src/backend.rs"))
        # CK-92: its Metal backend builds on the GPU crate and the Cocoa backend, so their changes select it.
        for path in ("crates/gpu-metal/src/lib.rs", "crates/window-cocoa/src/backend.rs"):
            self.assertIn("render2d.core", self.selected(path), path)

    def test_03_it_runs_every_test_target_of_the_crate_and_none_needs_a_window_or_the_gpu(
        self,
    ) -> None:
        step = next(step for step in check.STEPS if step.name == "render2d.core")
        units = group_units(step, TARGETS)
        targets = {(kind, name) for _, kind, name, _, _ in units}
        # The library's unit tests and the doctests: the decoder and allocation binaries moved to `ck-resources`
        # (CK-105, `ResourcesGroup`).
        self.assertEqual(
            targets,
            {
                ("lib", "ck_render_2d"),
                ("doc", "ck_render_2d"),
                ("test", "scale_pure"),  # CK-130: the scale acceptance's pure layout and gate tests
            },
        )
        self.assertEqual({unit[0] for unit in units}, {"ck-render-2d"})
        self.assertEqual({need_of(unit) for unit in units}, {"plain"})
        self.assertEqual(step.needs, "plain")
        self.assertEqual(step.category, "affected")

    def test_04_the_metal_window_case_belongs_to_the_window_group_and_only_it(self) -> None:
        """CK-92: `Renderer<MetalBackend>` on a real window takes the desktop's focus."""
        window = next(step for step in check.STEPS if step.name == "native.window")
        core = next(step for step in check.STEPS if step.name == "render2d.core")
        unit = ("ck-render-2d", "test", "metal_window", ALL_FEATURES, "*")
        self.assertIn(unit, group_units(window, TARGETS))
        self.assertNotIn(unit, group_units(core, TARGETS))
        self.assertEqual(need_of(unit), "window")
        for path in check.RENDER2D_WINDOW_FILES:
            self.assertIn("native.window", self.selected(path), path)
            self.assertIn("render2d.core", self.selected(path), path)


class MetalWindowFiles(unittest.TestCase):
    """CK-111: only the Metal crate's real-window inputs select its window group."""

    def selected(self, path: str) -> set[str]:
        return {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_01_window_files_and_nested_directory_files_select_both_metal_groups(self) -> None:
        paths = list(check.METAL_WINDOW_FILES)
        paths.extend(
            (
                "crates/gpu-metal/tests/presenter_quad/cases.rs",
                "crates/gpu-metal/tests/native/testing.m",
            )
        )
        for path in paths:
            chosen = self.selected(path)
            self.assertTrue(
                {
                    "native.window",
                    "metal.window-core",
                    "metal.window-scene",
                    "metal.window-compat",
                    "metal.window-instanced",
                    "metal.gpu",
                }
                <= chosen,
                path,
            )

    def test_02_shared_gpu_and_offscreen_files_do_not_select_the_window_group(self) -> None:
        for path in (
            "crates/gpu-metal/src/lib.rs",
            "crates/gpu-metal/Cargo.toml",
            "crates/gpu-metal/native/bridge.h",
            "crates/gpu-metal/native/bridge.m",
            "crates/gpu-metal/shaders/probe.metal",
            "crates/gpu-metal/tests/gpu_probe.rs",
            "crates/gpu-metal/tests/shader_debug.rs",
            "crates/gpu-metal/tests/hook_isolation.rs",
            "crates/gpu-metal/src/textures.rs",
            "crates/gpu-metal/native/textures.m",
            "crates/gpu-metal/tests/textures.rs",
        ):
            chosen = self.selected(path)
            self.assertIn("metal.gpu", chosen, path)
            self.assertNotIn("native.window", chosen, path)

    def test_02a_the_presenter_build_script_is_a_window_input(self) -> None:
        chosen = self.selected("crates/gpu-metal/build.rs")
        self.assertIn("metal.gpu", chosen)
        for group in (
            "native.window",
            "metal.window-core",
            "metal.window-scene",
            "metal.window-compat",
            "metal.window-instanced",
        ):
            self.assertIn(group, chosen)

    def test_03_the_window_groups_keep_the_presenter_window_cases_and_only_those(self) -> None:
        """CK-142: `native.window` no longer runs the presenter; each `metal.window-*` group runs exactly its own set."""
        window = next(step for step in check.STEPS if step.name == "native.window")
        metal = next(step for step in check.STEPS if step.name == "metal.gpu")
        presenter = {
            case: ("ck-gpu-metal", "test", "presenter", ALL_FEATURES, case)
            for case in ("gpu", "window-core", "window-scene", "window-compat", "window-instanced")
        }
        textures_gpu = ("ck-gpu-metal", "test", "textures", ALL_FEATURES, "*")
        self.assertEqual(
            group_units(window, TARGETS),
            {
                ("ck-render-2d", "test", "metal_window", ALL_FEATURES, "*"),
                ("ck-window-cocoa", "doc", "ck_window_cocoa", ALL_FEATURES, "*"),
                ("ck-window-cocoa", "lib", "ck_window_cocoa", ALL_FEATURES, "*"),
                ("ck-window-cocoa", "test", "window_cocoa", ALL_FEATURES, "*"),
                ("codekarl", "test", "windowed", ALL_FEATURES, "*"),
            },
        )
        for case in ("window-core", "window-scene", "window-compat", "window-instanced"):
            step = next(step for step in check.STEPS if step.name == f"metal.{case}")
            self.assertEqual(group_units(step, TARGETS), {presenter[case]}, step.name)
            self.assertNotIn(presenter[case], group_units(window, TARGETS))
            self.assertNotIn(presenter[case], group_units(metal, TARGETS))
        self.assertIn(presenter["gpu"], group_units(metal, TARGETS))
        self.assertIn(textures_gpu, group_units(metal, TARGETS))
        self.assertNotIn(textures_gpu, group_units(window, TARGETS))

    def test_04_a_presenter_scene_only_change_selects_every_group_that_runs_its_cases(
        self,
    ) -> None:
        presenter_window_groups = {
            step.name
            for step in check.STEPS
            if any(
                unit[:4] == ("ck-gpu-metal", "test", "presenter", ALL_FEATURES)
                and need_of(unit) == "window"
                for unit in group_units(step, TARGETS)
            )
        }
        self.assertTrue(presenter_window_groups, "the presenter catalog has no window case group")
        for path in (
            "crates/gpu-metal/tests/presenter_scene/cases.rs",
            "crates/gpu-metal/tests/presenter_scene/compat.rs",
            "crates/gpu-metal/tests/presenter_scene/instanced.rs",
            "crates/gpu-metal/tests/presenter_metrics/cases.rs",
        ):
            chosen = self.selected(path)
            self.assertIn("metal.gpu", chosen, path)
            self.assertTrue(
                presenter_window_groups <= chosen,
                f"{path}: missing presenter window groups {sorted(presenter_window_groups - chosen)}",
            )
            self.assertIn("native.window", chosen, path)

    def test_05_every_presenter_module_directory_is_classified(self) -> None:
        modules = presenter_modules()
        expected_nested_modules = {
            "crates/gpu-metal/tests/presenter_quad/cases.rs",
            "crates/gpu-metal/tests/presenter_instanced/cases.rs",
            "crates/gpu-metal/tests/presenter_bindless/cases.rs",
            "crates/gpu-metal/tests/presenter_scene/cases.rs",
            "crates/gpu-metal/tests/presenter_scene/compat.rs",
            "crates/gpu-metal/tests/presenter_scene/instanced.rs",
            "crates/gpu-metal/tests/presenter_metrics/cases.rs",
        }
        self.assertGreaterEqual(len(modules), 7, f"presenter modules found: {sorted(modules)}")
        self.assertTrue(
            expected_nested_modules <= modules,
            f"missing nested modules: {expected_nested_modules - modules}",
        )

        classifications = (*check.METAL_WINDOW_FILES, *check.METAL_PRESENTER_GPU_ONLY)

        def contains(entry: str, path: str) -> bool:
            return path.startswith(entry) if entry.endswith("/") else path == entry

        for path in sorted(modules):
            matches = [entry for entry in classifications if contains(entry, path)]
            self.assertEqual(
                len(matches),
                1,
                f"{path} must be in exactly one of METAL_WINDOW_FILES={check.METAL_WINDOW_FILES} "
                f"or METAL_PRESENTER_GPU_ONLY={check.METAL_PRESENTER_GPU_ONLY}; matched {matches}",
            )

        for directory in check.METAL_PRESENTER_GPU_ONLY:
            module_paths = [path for path in modules if contains(directory, path)]
            self.assertTrue(module_paths, f"{directory} contains no presenter module")
            for path in module_paths:
                names = presenter_case_names((REPO / path).read_text())
                self.assertTrue(names, f"{path} contains no typed presenter case array")
                for name in names:
                    self.assertTrue(
                        name.startswith("readback"),
                        f"{path}: GPU-only presenter case {name!r} is not a readback case",
                    )


class NativeWindowGroupsShareOneSelection(unittest.TestCase):
    """CK-142: `native.window` and the presenter's `metal.window-*` groups take one path tuple, so a planned check selects all
    five or none, exactly where it selected the one `native.window` before the split."""

    GROUPS = (
        "native.window",
        "metal.window-core",
        "metal.window-scene",
        "metal.window-compat",
        "metal.window-instanced",
    )

    def steps(self) -> dict[str, check.Step]:
        return {step.name: step for step in check.STEPS if step.name in self.GROUPS}

    def selected(self, path: str) -> set[str]:
        return {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_01_the_five_groups_have_identical_paths_equal_to_the_shared_tuple(self) -> None:
        steps = self.steps()
        self.assertEqual(sorted(steps), sorted(self.GROUPS))
        for name, step in steps.items():
            self.assertEqual(tuple(step.paths), check.NATIVE_WINDOW_PATHS, name)
        self.assertIn("crates/window-cocoa/", check.NATIVE_WINDOW_PATHS)

    def test_02_a_window_path_selects_all_five_and_a_non_window_path_none(self) -> None:
        for path in (
            "crates/window-cocoa/src/lib.rs",
            "crates/gpu-metal/src/presenter.rs",
            "crates/render-2d/src/metal.rs",
            "apps/codekarl/tests/windowed.rs",
        ):
            self.assertTrue(set(self.GROUPS) <= self.selected(path), path)
        for path in (
            "crates/gpu-metal/src/textures.rs",
            "crates/render-2d/src/vulkan.rs",
            "assets/demo-quad.png",
        ):
            self.assertEqual(set(self.GROUPS) & self.selected(path), set(), path)

    def test_03_every_new_window_group_is_a_fast_full_spec_analyzed_window_group_after_native_window(
        self,
    ) -> None:
        names = [step.name for step in check.STEPS]
        start = names.index("native.window")
        self.assertEqual(
            names[start : start + 5],
            [
                "native.window",
                "metal.window-core",
                "metal.window-scene",
                "metal.window-compat",
                "metal.window-instanced",
            ],
        )
        for name, step in self.steps().items():
            self.assertEqual(step.needs, "window", name)
            self.assertEqual(step.levels, check.FAST, name)
            self.assertTrue(step.full_spec and step.analyze, name)


class OptionalScaleGroup(unittest.TestCase):
    """CK-130: the milestone 5 scale acceptance is an optional group. It opens a 1920 x 1080 window and holds the GPU for minutes, so
    it runs only when named, at no level, with its two harnesses told to run (`CK_SCALE_RUN=1`) and in release."""

    NAME = "metal.scale"
    HARNESS_FILES = (
        "crates/render-2d/tests/scale.rs",
        "crates/render-2d/tests/scale/rig.rs",
        "crates/render-2d/tests/scale/layout.rs",
        "apps/codekarl/tests/scale_lua.rs",
        "config/scale-lua.toml",
        "scripts/scale/lua_moving.lua",
    )

    def step(self) -> check.Step:
        return {step.name: step for step in check.STEPS}[self.NAME]

    def test_01_the_group_is_optional_window_needing_and_in_no_level(self) -> None:
        step = self.step()
        self.assertEqual((step.category, step.needs, step.levels), ("optional", "window", ()))
        for level in ("quick", "fast", "full"):
            self.assertNotIn(self.NAME, check.select(level), level)
        self.assertIn(
            self.NAME, check.select("planned", [step])
        )  # a planned check runs what the plan chose

    def test_02_it_runs_the_two_harnesses_in_release_told_to_run(self) -> None:
        step = self.step()
        self.assertEqual(dict(step.env), {"CK_SCALE_RUN": "1"})
        flags = ["--locked", "--features", "metal"]
        self.assertEqual(check.SCALE_CARGO_FLAGS, flags)
        self.assertEqual(
            step.commands(),
            [
                ["cargo", "test", "--release", "-p", "ck-render-2d", "--test", "scale", *flags],
                ["cargo", "test", "--release", "-p", "codekarl", "--test", "scale_lua", *flags],
            ],
        )

    def test_02b_it_builds_the_recipe_not_all_features(self) -> None:
        # The recipe fixes release with `--features metal`; `--all-features` (the catalog's usual flag) is a different build.
        for command in self.step().commands():
            self.assertNotIn("--all-features", command)
            self.assertIn("--release", command)
            self.assertIn("--locked", command)
            self.assertEqual(command.count("--features"), 1)
            self.assertEqual(command[command.index("--features") + 1], "metal")

    def test_03_no_other_group_sets_the_run_variable(self) -> None:
        for other in check.STEPS:
            if other.name not in (
                self.NAME,
                "vulkan.scale",
            ):  # CK-162: the Vulkan scale group sets it too
                self.assertNotIn("CK_SCALE_RUN", dict(other.env), other.name)

    def test_04_a_plan_never_selects_it_unless_it_is_named(self) -> None:
        for path in self.HARNESS_FILES:
            chosen = {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}
            self.assertNotIn(self.NAME, chosen, path)
            named = check.plan(check.STEPS, [path], requested=[self.NAME])
            self.assertIn(self.NAME, {c.step.name for c in named.choices if c.selected}, path)
        by_name = {
            c.step.name: c for c in check.plan(check.STEPS, ["config/scale-lua.toml"]).choices
        }
        self.assertEqual(by_name[self.NAME].why, "optional: runs only when requested by name")

    def test_05_the_harness_files_select_plain_groups_and_never_a_window_or_gpu_group(self) -> None:
        needs = {step.name: step.needs for step in check.STEPS}
        for path in self.HARNESS_FILES:
            chosen = {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}
            self.assertEqual({name for name in chosen if needs[name] != "plain"}, set(), path)
            self.assertFalse(
                path in ("crates/render-2d/tests/scale.rs", "crates/render-2d/tests/scale/rig.rs")
                and "render2d.core" not in chosen,
                path,
            )

    def test_06_every_harness_file_is_claimed_by_a_group(self) -> None:
        for path in (*self.HARNESS_FILES, "crates/render-2d/tests/scale_pure.rs"):
            self.assertTrue(check.is_claimed(check.STEPS, path), path)

    def test_07_the_pure_tests_run_in_the_plain_renderer_group(self) -> None:
        by_name = {step.name: step for step in check.STEPS}
        flags = list(check.CARGO_FLAGS)
        self.assertIn(
            ["cargo", "test", "-p", "ck-render-2d", "--test", "scale_pure", *flags],
            by_name["render2d.core"].commands(),
        )


class OptionalVulkanScaleGroup(unittest.TestCase):
    """CK-162: the milestone 5.5 scale run on Vulkan is an optional group too, with the same rules: window-needing, in no level, its two
    harnesses told to run (`CK_SCALE_RUN=1`), in release with `--features vulkan` (never `--all-features`), and never selected by a file
    change. Its paths are exactly its two targets, its own directory, the three shared platform-free modules, its configuration and the
    script directory."""

    NAME = "vulkan.scale"
    HARNESS_FILES = (
        "crates/render-2d/tests/vulkan_scale.rs",
        "crates/render-2d/tests/vulkan_scale/rig.rs",
        "crates/render-2d/tests/vulkan_scale/evidence.rs",
        "crates/render-2d/tests/scale/fixtures.rs",
        "crates/render-2d/tests/scale/layout.rs",
        "crates/render-2d/tests/scale/json.rs",
        "apps/codekarl/tests/vulkan_scale_lua.rs",
        "config/scale-lua-vulkan.toml",
        "scripts/scale/lua_moving.lua",
    )

    def step(self) -> check.Step:
        return {step.name: step for step in check.STEPS}[self.NAME]

    def test_01_the_group_is_optional_window_needing_and_in_no_level(self) -> None:
        step = self.step()
        self.assertEqual((step.category, step.needs, step.levels), ("optional", "window", ()))
        for level in ("quick", "fast", "full"):
            self.assertNotIn(self.NAME, check.select(level), level)
        self.assertIn(self.NAME, check.select("planned", [step]))

    def test_02_it_runs_the_two_harnesses_in_release_with_the_vulkan_feature_told_to_run(
        self,
    ) -> None:
        step = self.step()
        self.assertEqual(dict(step.env), {"CK_SCALE_RUN": "1"})
        flags = ["--locked", "--features", "vulkan"]
        self.assertEqual(check.VULKAN_SCALE_CARGO_FLAGS, flags)
        self.assertEqual(
            step.commands(),
            [
                [
                    "cargo",
                    "test",
                    "--release",
                    "-p",
                    "ck-render-2d",
                    "--test",
                    "vulkan_scale",
                    *flags,
                ],
                [
                    "cargo",
                    "test",
                    "--release",
                    "-p",
                    "codekarl",
                    "--test",
                    "vulkan_scale_lua",
                    *flags,
                ],
            ],
        )
        # Metal's group is unchanged by it.
        self.assertEqual(check.SCALE_CARGO_FLAGS, ["--locked", "--features", "metal"])

    def test_02b_it_never_builds_all_features(self) -> None:
        for command in self.step().commands():
            self.assertNotIn("--all-features", command)
            self.assertIn("--release", command)
            self.assertIn("--locked", command)
            self.assertEqual(command.count("--features"), 1)
            self.assertEqual(command[command.index("--features") + 1], "vulkan")

    def test_03_its_paths_are_exactly_the_two_targets_its_directory_the_three_shared_modules_and_its_inputs(
        self,
    ) -> None:
        self.assertEqual(
            sorted(self.step().paths),
            sorted(
                (
                    "crates/render-2d/tests/vulkan_scale.rs",
                    "crates/render-2d/tests/vulkan_scale/",
                    "crates/render-2d/tests/scale/fixtures.rs",
                    "crates/render-2d/tests/scale/layout.rs",
                    "crates/render-2d/tests/scale/json.rs",
                    "apps/codekarl/tests/vulkan_scale_lua.rs",
                    "config/scale-lua-vulkan.toml",
                    "scripts/scale/",
                )
            ),
        )
        # Not the Metal directory and not its evidence module, which imports the Metal timing types.
        self.assertNotIn("crates/render-2d/tests/scale/", self.step().paths)
        self.assertNotIn("crates/render-2d/tests/scale/evidence.rs", self.step().paths)

    def test_04_a_plan_never_selects_it_unless_it_is_named(self) -> None:
        for path in self.HARNESS_FILES:
            chosen = {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}
            self.assertNotIn(self.NAME, chosen, path)
            named = check.plan(check.STEPS, [path], requested=[self.NAME])
            self.assertIn(self.NAME, {c.step.name for c in named.choices if c.selected}, path)
        by_name = {
            c.step.name: c
            for c in check.plan(check.STEPS, ["config/scale-lua-vulkan.toml"]).choices
        }
        self.assertEqual(by_name[self.NAME].why, "optional: runs only when requested by name")

    def test_05_the_harness_files_select_no_window_or_gpu_group(self) -> None:
        needs = {step.name: step.needs for step in check.STEPS}
        for path in self.HARNESS_FILES:
            chosen = {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}
            self.assertEqual({name for name in chosen if needs[name] != "plain"}, set(), path)
        # The renderer's pure group judges the harness's sources (they are compiled by its clippy and build checks).
        for path in (
            "crates/render-2d/tests/vulkan_scale.rs",
            "crates/render-2d/tests/vulkan_scale/rig.rs",
        ):
            chosen = {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}
            self.assertIn("render2d.core", chosen, path)

    def test_06_every_harness_file_is_claimed_by_a_group(self) -> None:
        for path in self.HARNESS_FILES:
            self.assertTrue(check.is_claimed(check.STEPS, path), path)

    def test_07_the_metal_group_does_not_run_the_vulkan_targets_and_the_reverse(self) -> None:
        metal = {step.name: step for step in check.STEPS}["metal.scale"]
        for command in metal.commands():
            self.assertNotIn("vulkan_scale", " ".join(command))
            self.assertNotIn("vulkan", command)
        for command in self.step().commands():
            self.assertNotIn("metal", command)
            self.assertNotIn("scale", command)


class ResourcesGroup(unittest.TestCase):
    """CK-105: the resource component's core group, in the real catalog."""

    def selected(self, path: str) -> set[str]:
        return {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_01_the_crates_paths_select_its_group_and_the_renderer_group_and_never_a_window_or_gpu_group(
        self,
    ) -> None:
        for path in (
            "crates/resources/src/lib.rs",
            "crates/resources/tests/decoder/budgets.rs",
            "crates/resources/tests/fixtures/generate.py",
        ):
            chosen = self.selected(path)
            self.assertTrue({"resources.core", "render2d.core", "unittest-lints"} <= chosen, path)
            for other in (
                "native.window",
                "metal.gpu",
                "glfw.window",
                "vulkan.window",
                "vulkan.gpu",
                "vulkan.app",
                "rust.core",
                "rust.app",
            ):
                self.assertNotIn(other, chosen, f"{path}: {other}")

    def test_02_a_renderer_path_does_not_select_it(self) -> None:
        for path in ("crates/render-2d/src/lib.rs", "crates/render-2d/src/tests/resources.rs"):
            self.assertNotIn("resources.core", self.selected(path), path)

    def test_03_it_runs_every_test_target_of_the_crate_and_all_are_plain(self) -> None:
        step = next(step for step in check.STEPS if step.name == "resources.core")
        units = group_units(step, TARGETS)
        self.assertEqual(
            {(kind, name) for _, kind, name, _, _ in units},
            {
                ("lib", "ck_resources"),
                ("test", "decoder"),
                ("test", "allocation"),
                ("doc", "ck_resources"),
            },
        )
        self.assertEqual({unit[0] for unit in units}, {"ck-resources"})
        self.assertEqual({need_of(unit) for unit in units}, {"plain"})
        self.assertEqual(step.needs, "plain")
        self.assertEqual(step.category, "affected")


class ApplicationLibrary(unittest.TestCase):
    """CK-131: the application is a library and a thin `main`, and its windowed tests run in process behind a test-only
    keyboard filter that a build script compiles for them."""

    def selected(self, path: str) -> set[str]:
        return {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_01_the_windowed_tests_and_their_filter_select_the_window_group_and_no_other_window_group(
        self,
    ) -> None:
        for path in (
            "apps/codekarl/tests/windowed.rs",
            "apps/codekarl/build.rs",
            "apps/codekarl/tests/native/testing.m",
        ):
            chosen = self.selected(path)
            self.assertIn("native.window", chosen, path)
            for other in ("glfw.window", "vulkan.window", "vulkan.gpu"):
                self.assertNotIn(other, chosen, f"{path}: {other}")
        # CK-159: the build script and the native support are also the `glfw-vulkan` tests' (they run behind the same filter),
        # so they select `vulkan.app` as well; the native tests alone do not.
        self.assertNotIn("vulkan.app", self.selected("apps/codekarl/tests/windowed.rs"))
        for path in ("apps/codekarl/build.rs", "apps/codekarl/tests/native/testing.m"):
            self.assertIn("vulkan.app", self.selected(path), path)

    def test_02_the_libraries_unit_tests_and_doctests_and_the_thin_binary_belong_to_rust_app(
        self,
    ) -> None:
        step = next(step for step in check.STEPS if step.name == "rust.app")
        units = group_units(step, TARGETS)
        for kind in ("lib", "doc", "bin"):
            self.assertIn(("codekarl", kind, "codekarl", ALL_FEATURES, "*"), units, kind)
        self.assertNotIn("windowed", {unit[2] for unit in units})

    def test_03_the_windowed_target_is_a_supervisor_and_worker_executable_that_needs_the_metal_feature(
        self,
    ) -> None:
        manifest = (REPO / "apps/codekarl/Cargo.toml").read_text()
        windowed = manifest[manifest.index('name = "windowed"') :]
        self.assertIn("harness = false", windowed.split("[", 1)[0])
        self.assertIn('required-features = ["metal"]', windowed.split("[[", 1)[0])


class Render2dVulkanSink(unittest.TestCase):
    """CK-98: the Vulkan sink side of `ck-render-2d` is covered by the plain core group and never by a Metal or
    native-window group."""

    def selected(self, path: str) -> set[str]:
        return {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_01_the_vulkan_module_its_tests_the_manifest_and_the_lockfile_select_the_core_group_only(
        self,
    ) -> None:
        for path in (
            "crates/render-2d/src/vulkan.rs",
            "crates/render-2d/src/vulkan/convert.rs",  # a directory, if the module ever needs one
            "crates/render-2d/src/tests/vulkan_sink.rs",
            "crates/render-2d/src/tests/vulkan_refusal.rs",
            "crates/render-2d/Cargo.toml",
            "Cargo.lock",
        ):
            chosen = self.selected(path)
            self.assertIn("render2d.core", chosen, path)
            for forbidden in ("metal.gpu", "native.window"):
                self.assertNotIn(forbidden, chosen, f"{path}: {forbidden}")

    def test_02_a_change_to_the_vulkan_gpu_crate_reruns_the_conversion_tests_and_no_metal_or_native_group(
        self,
    ) -> None:
        core = next(step for step in check.STEPS if step.name == "render2d.core")
        self.assertIn("crates/gpu-vulkan/", core.paths)
        for path in ("crates/gpu-vulkan/src/lib.rs", "crates/gpu-vulkan/src/quad.rs"):
            chosen = self.selected(path)
            self.assertIn("render2d.core", chosen, path)
            self.assertIn("vulkan.gpu", chosen, path)
            for forbidden in ("metal.gpu", "native.window"):
                self.assertNotIn(forbidden, chosen, f"{path}: {forbidden}")

    def test_03_the_window_files_of_the_crate_are_still_only_the_metal_ones(self) -> None:
        self.assertEqual(
            check.RENDER2D_WINDOW_FILES,
            (
                "crates/render-2d/src/metal.rs",
                "crates/render-2d/tests/metal_window.rs",
                "crates/render-2d/build.rs",
                "crates/render-2d/tests/native/testing.m",
                "crates/window/tests/support/activation_watch.rs",
            ),
        )
        for path in check.RENDER2D_WINDOW_FILES:
            self.assertNotIn("vulkan", path)

    def test_04_the_core_groups_description_names_the_vulkan_sink_conversion(self) -> None:
        core = next(step for step in check.STEPS if step.name == "render2d.core")
        self.assertIn("Vulkan sink", core.description)


class AppRenderGroup(unittest.TestCase):
    """CK-93: the application's render API, its demonstration files and its source guard, in the real catalog."""

    def selected(self, path: str) -> set[str]:
        return {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_01_the_demonstration_files_select_the_plain_app_groups_and_never_a_window_group(
        self,
    ) -> None:
        for path in (
            "config/demo-quad.toml",
            "scripts/ui/demo_quad.lua",
            "assets/demo-quad.png",
            "assets/generate_demo_quad.py",
            # CK-126: the live-scene demonstration (the owner's hand-run command: config/demo-scene.toml)
            "config/demo-scene.toml",
            "scripts/ui/demo_scene.lua",
            "assets/demo-scene/tile-01-stripes-h.png",
            "assets/generate_demo_scene.py",
        ):
            chosen = self.selected(path)
            self.assertTrue({"rust.app", "rust.app-default"} <= chosen, path)
            for other in (
                "native.window",
                "metal.gpu",
                "vulkan.window",
                "vulkan.app",
                "glfw.window",
            ):
                self.assertNotIn(other, chosen, f"{path}: {other}")

    def test_02_the_new_test_targets_run_in_rust_app_and_need_nothing(self) -> None:
        step = next(step for step in check.STEPS if step.name == "rust.app")
        units = group_units(step, TARGETS)
        targets = {(kind, name) for _, kind, name, _, _ in units if kind == "test"}
        self.assertLessEqual({("test", "render"), ("test", "source_guard")}, targets)
        for unit in units:
            if unit[2] in {"render", "source_guard"}:
                self.assertEqual(need_of(unit), "plain", unit)

    def test_03_the_source_guard_and_the_render_tests_are_registered_as_guards(self) -> None:
        registered = {guard.file: guard for guard in GUARDS}
        guard = registered["apps/codekarl/tests/source_guard.rs"]
        self.assertEqual(guard.group, "rust.app")
        self.assertEqual(set(guard.reads), set(APP_SOURCES))
        for path in ("apps/codekarl/src/render_api.rs", "apps/codekarl/tests/source_guard.rs"):
            self.assertIn("rust.app", self.selected(path), path)
        render = registered["apps/codekarl/tests/render.rs"]
        self.assertEqual(render.group, "rust.app")
        for read in render.reads:
            self.assertIn("rust.app", self.selected(read), read)
        # The real-window case reads the demonstration's image; replacing it opens no window by itself.
        windowed = registered["apps/codekarl/tests/windowed.rs"]
        self.assertIn("assets/", windowed.reads)
        self.assertIn("assets/", dict(windowed.unselected))
        self.assertNotIn("native.window", self.selected("assets/demo-quad.png"))
        self.assertIn("native.window", self.selected("apps/codekarl/src/native.rs"))


class Bootstrap(unittest.TestCase):
    def test_01_the_bootstrap_installs_glfw_shaderc_and_ripgrep_when_missing_and_nothing_else_new(
        self,
    ) -> None:
        text = (REPO / "tools/bootstrap").read_text()
        installs = re.findall(r"^\s*brew install (\S+)", text, re.MULTILINE)
        self.assertEqual(installs, ["rustup", "glfw", "shaderc", "ripgrep"])
        self.assertIn("if ! brew list --versions glfw >/dev/null 2>&1; then", text)
        self.assertIn("if ! brew list --versions ripgrep >/dev/null 2>&1; then", text)
        self.assertIn("tools/ck doctor", text.splitlines()[-1], "the doctor runs last")


class VulkanGroups(unittest.TestCase):
    """CK-79: the Vulkan presenter's groups, in the real catalog."""

    def selected(self, path: str) -> set[str]:
        return {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_00_the_quad_shaders_and_their_tests_select_the_vulkan_groups_and_the_doctor_only(
        self,
    ) -> None:
        """CK-95: `glslc` compiles the shaders in `build.rs`, so a shader selects the graphics doctor and the
        groups that build the crate; none of it selects a Metal or native-window group."""
        for path in (
            "crates/gpu-vulkan/shaders/quad.vert",
            "crates/gpu-vulkan/shaders/quad.frag",
            "crates/gpu-vulkan/src/draw.rs",
            "crates/gpu-vulkan/src/quad.rs",
            "crates/gpu-vulkan/src/readback_quad.rs",
            "crates/gpu-vulkan/tests/gpu_quad/mod.rs",
        ):
            chosen = self.selected(path)
            self.assertTrue(
                {"vulkan.gpu", "vulkan.isolation", "doctor.graphics", "unittest-lints"} <= chosen,
                path,
            )
            for other in ("native.window", "metal.gpu", "metal.isolation", "glfw.window"):
                self.assertNotIn(other, chosen, f"{path}: {other}")
        # The new test module is part of the `gpu` target, which `vulkan.gpu` runs.
        step = next(step for step in check.STEPS if step.name == "vulkan.gpu")
        self.assertIn(
            ("ck-gpu-vulkan", "test", "gpu"),
            {(package, kind, name) for package, kind, name, _, _ in group_units(step, TARGETS)},
        )

    def test_01_the_crates_paths_select_its_groups_and_never_the_native_ones(self) -> None:
        for path in ("crates/gpu-vulkan/src/render.rs", "crates/gpu-vulkan/tests/window.rs"):
            chosen = self.selected(path)
            self.assertTrue(
                {"vulkan.gpu", "vulkan.window", "doctor.graphics", "unittest-lints"} <= chosen, path
            )
            for other in (
                "native.window",
                "metal.gpu",
                "glfw.window",
                "glfw.core",
                "rust.core",
                "rust.app",
                "rust.app-default",
            ):
                self.assertNotIn(other, chosen, f"{path}: {other}")

    def test_02_the_vulkan_groups_are_selected_only_by_the_crates_own_paths(self) -> None:
        for path in (
            "crates/window/src/main_loop.rs",
            "crates/window-glfw/src/backend.rs",
            "crates/window-glfw/tests/window_glfw.rs",
            "crates/window-cocoa/src/backend.rs",
            "crates/gpu-metal/src/lib.rs",
            "apps/codekarl/src/main.rs",
            "Cargo.lock",
            "Cargo.toml",
            "tools/check.py",
            "tools/bootstrap",
        ):
            chosen = self.selected(path)
            self.assertNotIn("vulkan.window", chosen, path)
            self.assertNotIn("vulkan.gpu", chosen, path)

    def test_03_the_groups_are_classified_by_what_they_run(self) -> None:
        by_name = {step.name: step for step in check.STEPS}
        self.assertEqual(by_name["vulkan.gpu"].needs, "gpu")
        self.assertEqual(by_name["vulkan.window"].needs, "window")
        units = group_units(by_name["vulkan.window"], TARGETS)
        self.assertEqual({(unit[0], unit[2]) for unit in units}, {("ck-gpu-vulkan", "window")})
        gpu_units = group_units(by_name["vulkan.gpu"], TARGETS)
        self.assertEqual({unit[2] for unit in gpu_units}, {"ck_gpu_vulkan", "gpu"})
        # CK-84: the static guard is its own plain group.
        static_units = group_units(by_name["vulkan.isolation"], TARGETS)
        self.assertEqual({unit[2] for unit in static_units}, {"hook_isolation"})
        self.assertEqual(by_name["vulkan.isolation"].needs, "plain")
        # Every unit of the crate is run by exactly one of the three groups.
        crate = {unit for unit in full_units(TARGETS) if unit[0] == "ck-gpu-vulkan"}
        self.assertEqual(crate, units | gpu_units | static_units)


class VulkanAppGroups(unittest.TestCase):
    """CK-80: the application's glfw-vulkan path, in the real catalog."""

    OWN = (
        "apps/codekarl/src/vulkan.rs",
        "apps/codekarl/src/vulkan/real.rs",
        "apps/codekarl/src/vulkan/tests.rs",
        "apps/codekarl/src/vulkan/fake.rs",
        "apps/codekarl/src/vulkan/quad_tests.rs",
        "apps/codekarl/src/vulkan/logging.rs",
        "apps/codekarl/tests/vulkan_app.rs",
        "apps/codekarl/Cargo.toml",
        "config/demo-glfw-vulkan.toml",
        "config/demo-glfw-vulkan-decline.toml",
        "config/demo-glfw-vulkan-callback-error.toml",
        "config/demo-quad-vulkan.toml",
    )

    def selected(self, path: str) -> set[str]:
        return {c.step.name for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_01_the_paths_of_the_path_select_the_vulkan_app_group_and_never_the_native_window_ones(
        self,
    ) -> None:
        for path in self.OWN:
            chosen = self.selected(path)
            self.assertIn("vulkan.app", chosen, path)
            self.assertIn("doctor.graphics", chosen, path)
            for other in (
                "native.window",
                "metal.gpu",
                "glfw.window",
                "vulkan.window",
                "vulkan.gpu",
            ):
                self.assertNotIn(other, chosen, f"{path}: {other}")

    def test_01b_the_build_script_and_the_native_support_select_the_vulkan_app_group_and_the_native_window_groups(
        self,
    ) -> None:
        """CK-159: the Vulkan application's tests run in process behind the keyboard filter that `build.rs` compiles from
        `tests/native/`, so those paths are theirs; they stay the native windowed tests' too (`NATIVE_APP_FILES`), so a
        change to them selects the five shared native window groups as well, and never the GPU groups."""
        for path in ("apps/codekarl/build.rs", "apps/codekarl/tests/native/testing.m"):
            chosen = self.selected(path)
            self.assertIn("vulkan.app", chosen, path)
            self.assertIn("doctor.graphics", chosen, path)
            for group in (
                "native.window",
                "metal.window-core",
                "metal.window-scene",
                "metal.window-compat",
                "metal.window-instanced",
            ):
                self.assertIn(group, chosen, f"{path}: {group}")
            for other in ("metal.gpu", "glfw.window", "vulkan.window", "vulkan.gpu"):
                self.assertNotIn(other, chosen, f"{path}: {other}")

    def test_01c_the_vulkan_app_target_is_a_supervisor_and_worker_executable_that_needs_the_vulkan_feature(
        self,
    ) -> None:
        manifest = (REPO / "apps/codekarl/Cargo.toml").read_text()
        target = manifest[manifest.index('name = "vulkan_app"') :]
        self.assertIn("harness = false", target.split("[", 1)[0])
        self.assertIn('required-features = ["vulkan"]', target.split("[[", 1)[0])

    def test_02_the_group_is_selected_only_by_the_paths_own_files(self) -> None:
        for path in (
            "apps/codekarl/src/main.rs",
            "apps/codekarl/src/native.rs",
            "apps/codekarl/src/windowed.rs",
            "apps/codekarl/tests/windowed.rs",
            "apps/codekarl/tests/modes.rs",
            "config/codekarl.toml",
            "config/demo-decline.toml",
            "crates/window/src/main_loop.rs",
            "crates/window-glfw/src/backend.rs",
            "crates/gpu-vulkan/src/render.rs",
            "Cargo.lock",
            "tools/check.py",
        ):
            self.assertNotIn("vulkan.app", self.selected(path), path)

    def test_02b_the_manifest_and_the_quad_demonstration_alone_select_the_group_and_no_metal_or_native_one(
        self,
    ) -> None:
        """CK-99: `apps/codekarl/Cargo.toml` carries the `vulkan` feature's wiring (it was once asserted NOT to
        select this group), and `config/demo-quad-vulkan.toml` is run by its headless case. Each alone selects
        `vulkan.app`; neither selects a Metal or native-window group."""
        for path in ("apps/codekarl/Cargo.toml", "config/demo-quad-vulkan.toml"):
            chosen = self.selected(path)
            self.assertIn("vulkan.app", chosen, path)
            for forbidden in ("metal.gpu", "native.window"):
                self.assertNotIn(forbidden, chosen, f"{path}: {forbidden}")
        # The Metal demonstration is still the native group's alone.
        self.assertNotIn("vulkan.app", self.selected("config/demo-quad.toml"))

    def test_03_the_group_runs_exactly_the_vulkan_app_test_target_and_needs_a_window(self) -> None:
        by_name = {step.name: step for step in check.STEPS}
        units = group_units(by_name["vulkan.app"], TARGETS)
        self.assertEqual({(unit[0], unit[2]) for unit in units}, {("codekarl", "vulkan_app")})
        self.assertEqual({need_of(unit) for unit in units}, {"window"})
        self.assertEqual(by_name["vulkan.app"].needs, "window")

    def test_04_no_other_group_runs_the_vulkan_app_target_and_the_native_reverse_override_is_native(
        self,
    ) -> None:
        """A Vulkan-only plan cannot open the native backend, and the reverse override (which does) is in
        `windowed.rs`, run by the native window group and by the full run only."""
        owners: dict[str, set[str]] = {}
        for step in check.STEPS:
            for unit in group_units(step, TARGETS):
                if unit[3] == ALL_FEATURES:  # the default-feature build compiles these files empty
                    owners.setdefault(unit[2], set()).add(step.name)
        self.assertEqual(owners["vulkan_app"], {"vulkan.app"})
        self.assertEqual(owners["windowed"], {"native.window"})
        self.assertNotIn(
            "windowed",
            {
                unit[2]
                for unit in group_units(
                    next(step for step in check.STEPS if step.name == "vulkan.app"), TARGETS
                )
            },
        )
        # The Vulkan-only plan of each of the path's files runs no native window group.
        for path in self.OWN:
            plan = check.plan(check.STEPS, [path])
            commands = [
                command
                for choice in plan.choices
                if choice.selected
                for command in (choice.step.commands() if choice.step.action is None else [])
            ]
            self.assertFalse(
                [
                    command
                    for command in commands
                    if "windowed" in command or "ck-window-cocoa" in command
                ],
                path,
            )
        text = (REPO / "apps/codekarl/tests/windowed.rs").read_text()
        self.assertIn(
            "fn backend_native_metal_over_a_glfw_vulkan_configuration_opens_the_native_window", text
        )
        self.assertNotIn(
            '"native-metal"]', (REPO / "apps/codekarl/tests/vulkan_app.rs").read_text()
        )  # no case here passes `--backend native-metal`: that direction is native coverage


@dataclass(frozen=True)
class Guard:
    """A test or build step that reads files outside the paths of the group that runs it (CK-84).

    `group` is the group the catalog must select when one of `reads` changes; `unselected` lists reads that are
    deliberately not registered, each with the reason (the test asserts the group stays unselected, so the
    exclusion is a stated rule and not an oversight)."""

    file: str
    group: str
    reads: tuple[str, ...]
    unselected: tuple[tuple[str, str], ...] = ()
    after: str = (
        ""  # an issue whose submission adds this file: it is not checked for existing before then
    )


APP_SOURCES = ("apps/codekarl/src/", "apps/codekarl/tests/")
SUPERVISOR = "apps/codekarl/tests/supervisor/mod.rs"
ACTIVATION_WATCH = "crates/window/tests/support/activation_watch.rs"
TOOLCHAIN_FILES = ("Cargo.toml", "Cargo.lock", "clippy.toml", "rust-toolchain.toml")
OWNER_RULE = "the owner's rule: no change to the shared configuration or scripts selects a native window group"
DEMO_RULE = (
    "the demonstration's files never open a window by themselves (CK-93, CK-126): `rust.app` runs the script headless "
    "and every full run covers it on the glfw-vulkan window"
)
FIXTURE_COPY = (
    "a fixture copy of the file: its content is not asserted, only that a toolchain file exists"
)

# The inventory. Every test or build step of the repository that reads files outside the paths of its own group,
# found by searching the Rust tests and build scripts for include/`#[path]`/manifest-relative reads and the
# Python tests for repository-relative reads (see `Scanner`, which keeps this list complete for those idioms).
GUARDS: tuple[Guard, ...] = (
    # The hook-isolation guards: each scans the application's sources and tests and its own crate.
    Guard(
        "crates/gpu-metal/tests/hook_isolation.rs",
        "metal.isolation",
        (*APP_SOURCES, "crates/gpu-metal/native/", "crates/gpu-metal/Cargo.toml"),
    ),
    Guard(
        "crates/gpu-vulkan/tests/hook_isolation.rs",
        "vulkan.isolation",
        (*APP_SOURCES, "crates/gpu-vulkan/src/", "crates/gpu-vulkan/Cargo.toml"),
    ),
    # The catalog tests: the GLFW crate (sources and every file), the case-set declarations, the manifests
    # and the test directories `cargo metadata` derives the units from.
    Guard(
        "tests/test_check_catalog.py",
        "unittest-catalog",
        (
            "crates/window-glfw/",
            "crates/gpu-metal/tests/presenter.rs",
            "crates/gpu-metal/tests/",
            "apps/codekarl/tests/cli_spec/support.rs",
            "apps/codekarl/tests/windowed.rs",
            "apps/codekarl/tests/vulkan_app.rs",
            "crates/log/src/lib.rs",
            "crates/gpu-vulkan/build.rs",
            "apps/codekarl/src/main.rs",
            *TOOLCHAIN_FILES[:2],
            "apps/codekarl/Cargo.toml",
            "crates/log/Cargo.toml",
            "crates/window/tests/",
            "crates/gpu-vulkan/tests/",
        ),
    ),
    # CK-105: the renderer's re-export test checks its window fixture against the resource component's copy.
    Guard(
        "crates/render-2d/src/tests/resources.rs",
        "render2d.core",
        ("crates/resources/tests/fixtures/rgba8.png",),
    ),
    # Native and Rust test support compiled into another crate's test targets.
    Guard(
        "crates/gpu-vulkan/build.rs",
        "vulkan.window",
        ("crates/window-glfw/tests/native/testing.m", "crates/window-glfw/tests/native/testing.h"),
    ),
    Guard(
        "crates/gpu-vulkan/tests/support/mod.rs",  # includes the info binding by path
        "vulkan.window",
        ("crates/window-glfw/tests/support/",),
        after="CK-83",
    ),
    # The info binding's own check that no other file binds the native info function, which reads the test
    # directories of both crates (compiled into the GLFW and the Vulkan window tests).
    Guard(
        "crates/window-glfw/tests/support/info_abi.rs",
        "glfw.window",
        ("crates/window-glfw/tests/", "crates/gpu-vulkan/tests/"),
        unselected=(
            (
                "crates/gpu-vulkan/tests/",
                "also compiled into vulkan.window, which the crate's own directory selects",
            ),
        ),
        after="CK-83",
    ),
    Guard("crates/gpu-vulkan/tests/window.rs", "vulkan.window", (SUPERVISOR, ACTIVATION_WATCH)),
    Guard(
        "crates/gpu-metal/build.rs",
        "metal.window-core",
        ("crates/window-cocoa/tests/native/activation_watch.inc",),
    ),
    Guard("crates/gpu-metal/tests/presenter.rs", "metal.window-core", (ACTIVATION_WATCH,)),
    Guard("crates/render-2d/tests/metal_window.rs", "native.window", (ACTIVATION_WATCH,)),
    Guard(
        "crates/render-2d/tests/scale.rs",
        "metal.scale",
        (ACTIVATION_WATCH,),
        unselected=(
            (ACTIVATION_WATCH, "metal.scale is optional and only runs when the owner schedules it"),
        ),
    ),
    Guard("crates/window-cocoa/tests/window_cocoa.rs", "native.window", (ACTIVATION_WATCH,)),
    # CK-159: `ck-render-2d`'s Vulkan window test compiles `ck-window-glfw`'s native filter (its build script) and includes the
    # info binding and the shared supervisor by path.
    Guard(
        "crates/render-2d/build.rs",
        "render2d.vulkan-window",
        ("crates/window-glfw/tests/native/testing.m", "crates/window-glfw/tests/native/testing.h"),
    ),
    Guard(
        "crates/render-2d/tests/vulkan_window.rs",
        "render2d.vulkan-window",
        (SUPERVISOR, "crates/window-glfw/tests/support/", ACTIVATION_WATCH),
    ),
    Guard(
        "crates/window-glfw/build.rs",
        "glfw.window",
        ("crates/window-cocoa/tests/native/activation_watch.inc",),
    ),
    Guard("crates/gpu-vulkan/tests/gpu.rs", "vulkan.gpu", (SUPERVISOR,)),
    Guard("crates/window-glfw/tests/window_glfw.rs", "glfw.window", (SUPERVISOR, ACTIVATION_WATCH)),
    # The application's tests and the shipped configuration, scripts and CLI spec.
    # CK-99: the Vulkan real-window case also draws the demonstration's image, and the headless case runs the quad
    # demonstration. Replacing the image is a file change that must not open a window by itself (`rust.app`
    # decodes the shipped image headless).
    Guard(
        "apps/codekarl/tests/vulkan_app.rs",
        "vulkan.app",
        (
            "config/demo-glfw-vulkan.toml",
            "config/demo-glfw-vulkan-decline.toml",
            "config/demo-glfw-vulkan-callback-error.toml",
            "config/demo-quad-vulkan.toml",
            # CK-159: the common demonstration it runs on `glfw-vulkan`, unchanged.
            "config/demo-scene.toml",
            ACTIVATION_WATCH,
            "scripts/ui/demo_scene.lua",
            "scripts/gameplay/main.lua",
            "assets/",
        ),
        unselected=(
            (
                "assets/",
                "the demonstration's image: the owner's own replaces it as a file change, which must not open a "
                "window by itself; `rust.app` decodes the shipped image headless",
            ),
            ("config/demo-scene.toml", DEMO_RULE),
            ("scripts/ui/demo_scene.lua", DEMO_RULE),
            ("scripts/gameplay/main.lua", DEMO_RULE),
        ),
    ),
    Guard(
        "apps/codekarl/tests/windowed.rs",
        "native.window",
        ("config/codekarl.toml", "scripts/ui/main.lua", "assets/", ACTIVATION_WATCH),
        unselected=(
            ("config/codekarl.toml", OWNER_RULE),
            ("scripts/ui/main.lua", OWNER_RULE),
            (
                "assets/",
                "the demonstration's image: the owner's own replaces it as a file change, which must not open a "
                "window by itself; `rust.app` decodes the shipped image headless",
            ),
        ),
    ),
    # CK-162: the Vulkan Lua profile reads the same two shared modules; its group `vulkan.scale` is optional and never selected by a file change.
    Guard(
        "apps/codekarl/tests/vulkan_scale_lua.rs",
        "vulkan.scale",
        (
            "crates/render-2d/tests/scale/fixtures.rs",
            "crates/render-2d/tests/scale/layout.rs",
            ACTIVATION_WATCH,
        ),
        unselected=(
            (
                "crates/render-2d/tests/scale/fixtures.rs",
                "the Vulkan scale group is optional and owner-scheduled: no file change selects it, and the pure tests "
                "in `render2d.core` judge the fixtures",
            ),
            (
                "crates/render-2d/tests/scale/layout.rs",
                "the Vulkan scale group is optional and owner-scheduled: no file change selects it, and the pure tests "
                "in `render2d.core` judge the layout",
            ),
            (
                ACTIVATION_WATCH,
                "vulkan.scale is optional and only runs when the owner schedules it",
            ),
        ),
    ),
    # CK-162: the Vulkan scale harness's workers install the activation watch (CK-164) like the Metal harness's.
    Guard(
        "crates/render-2d/tests/vulkan_scale.rs",
        "vulkan.scale",
        (ACTIVATION_WATCH,),
        unselected=(
            (
                ACTIVATION_WATCH,
                "vulkan.scale is optional and only runs when the owner schedules it",
            ),
        ),
    ),
    # CK-130: the Lua profile of the scale acceptance writes the render-2d harness's fixtures with its layout. The optional
    # group `metal.scale` is never selected by a file change.
    Guard(
        "apps/codekarl/tests/scale_lua.rs",
        "metal.scale",
        (
            "crates/render-2d/tests/scale/fixtures.rs",
            "crates/render-2d/tests/scale/layout.rs",
            ACTIVATION_WATCH,
        ),
        unselected=(
            (
                "crates/render-2d/tests/scale/fixtures.rs",
                "the scale group is optional and owner-scheduled: no file change selects it, and the pure tests "
                "in `render2d.core` judge the layout",
            ),
            (
                "crates/render-2d/tests/scale/layout.rs",
                "the scale group is optional and owner-scheduled: no file change selects it, and the pure tests "
                "in `render2d.core` judge the layout",
            ),
            (ACTIVATION_WATCH, "metal.scale is optional and only runs when the owner schedules it"),
        ),
    ),
    Guard(
        "apps/codekarl/tests/lua_vms.rs",
        "rust.app",
        ("config/codekarl.toml", "scripts/ui/main.lua"),
    ),
    Guard("apps/codekarl/tests/cli_spec/support.rs", "rust.app", ("specs/bootstrap.json",)),
    # CK-93: the render API's headless tests run the shipped demonstration configuration and decode its image;
    # `rust.app` claims `config/`, `scripts/` and `assets/`.
    Guard(
        "apps/codekarl/tests/render.rs",
        "rust.app",
        (
            "config/demo-quad.toml",
            "config/demo-scene.toml",
            "scripts/ui/demo_scene.lua",
            "scripts/gameplay/main.lua",
            "assets/",
        ),
    ),
    # CK-126: the headless driver's cases run the shipped live-scene demonstration script over the shipped assets.
    Guard(
        "apps/codekarl/src/demo_tests.rs",
        "rust.app",
        ("scripts/ui/demo_scene.lua", "assets/"),
    ),
    # CK-93: the demonstration script over the fake native backend, and the real-window case with the
    # placeholder image. The shared configuration and scripts never select a native window group, and the
    # demonstration's own files belong to the window case that runs them (`native.window`).
    Guard(
        "apps/codekarl/src/windowed/render_tests.rs",
        "rust.app",
        ("scripts/ui/demo_quad.lua", "assets/"),
    ),
    # CK-93: the source guard reads the application's own sources and tests (no outside path).
    Guard("apps/codekarl/tests/source_guard.rs", "rust.app", APP_SOURCES),
    # Guards that read only files their own group already claims (listed so the inventory is complete).
    Guard("apps/codekarl/src/vulkan/tests.rs", "rust.app", ("apps/codekarl/src/vulkan/real.rs",)),
    Guard(
        "crates/runtime/tests/no_waiting_log_calls.rs", "rust.core", ("crates/runtime/src/lib.rs",)
    ),
    Guard(
        "crates/window/tests/window/records.rs", "rust.core", ("crates/window/src/main_loop.rs",)
    ),
    Guard(
        "tests/test_rust_lints.py",
        "unittest-lints",
        (
            "apps/codekarl/src/main.rs",
            "crates/window-glfw/src/backend.rs",
            "crates/window-glfw/build.rs",
            "clippy.toml",
            "Cargo.toml",
        ),
    ),
    # The analyzer build of a copy of the workspace's Metal crate, with its real bridge source edited.
    Guard(
        "tests/test_check_tools.py",
        "unittest-bootstrap",
        ("crates/gpu-metal/native/bridge.m", "crates/gpu-metal/build.rs", *TOOLCHAIN_FILES),
    ),
    Guard(
        "tests/test_bootstrap.py",
        "unittest-bootstrap",
        ("rust-toolchain.toml",),
    ),
    Guard(
        "tests/test_cli_retired.py",
        "unittest-lease",
        ("rust-toolchain.toml",),
        unselected=(("rust-toolchain.toml", FIXTURE_COPY),),
    ),
    Guard(
        "tests/test_rollout.py",
        "unittest-lease",
        ("rust-toolchain.toml",),
        unselected=(("rust-toolchain.toml", FIXTURE_COPY),),
    ),
    # The real-toolchain lease test builds and checks a copy of the whole workspace as its workload; a failure
    # there is the floor's clippy or doc, or `rust.core`, which any Rust edit selects anyway.
    # CK-86: the search-hiding rules and Pijul's ignore file, listed by name in the test (so the idiom scan
    # does not see them); AGENTS.md is a Python path.
    Guard(
        "tests/test_rgignore.py",
        "unittest-check",
        (
            ".rgignore",
            ".ignore",
            "AGENTS.md",
            # The real-ripgrep search runs over this tree and requires these files of it.
            "docs/",
            "docs/vision.md",
            "docs/guide/CURSOR.md",
            "docs/archive/",
        ),
    ),
    Guard(
        "tests/test_check.py",
        "unittest-check",
        (
            "Cargo.toml",
            "Cargo.lock",
            "clippy.toml",
            "rust-toolchain.toml",
            ".ignore",
            "specs/bootstrap.json",
        ),
        unselected=(
            (
                "specs/bootstrap.json",
                "workload copy: the floor and rust.core fail on its own changes",
            ),
        ),
    ),
)

QUOTE = '"'
PYTHON_OWN = ("tools", "tests", "stubs", "skills", "target")
PYTHON_OWN_FILES = ("pijul.toml", "pyproject.toml", "AGENTS.md", "CLAUDE.md", ".ck-hub")


def normalized(base: str, relative: str) -> str:
    """The repository-relative path of `relative` taken from the directory `base` (`.` is the root)."""
    return posixpath.normpath(str(PurePosixPath(base) / relative))


def rust_references(path: str, text: str) -> set[str]:
    """The repository-relative paths a Rust test, source or build script names outside its own crate, by the
    idioms the repository uses: `include_str!`/`include_bytes!` and `#[path]` (relative to the file), a path
    joined onto `CARGO_MANIFEST_DIR` and, in a build script, a `"../..."` literal (relative to the crate)."""
    crate = "/".join(path.split("/")[:2])
    found: set[str] = set()
    here = str(PurePosixPath(path).parent)
    for pattern in (r'include_(?:str|bytes)!\(\s*"([^"]+)"', r'#\[path\s*=\s*"([^"]+)"\]'):
        found |= {normalized(here, name) for name in re.findall(pattern, text)}
    manifest = r'CARGO_MANIFEST_DIR"\s*\)\s*\)?\s*\.join\(\s*"([^"]+)"'
    found |= {normalized(crate, name) for name in re.findall(manifest, text)}
    if path.endswith("/build.rs"):
        found |= {normalized(crate, name) for name in re.findall(r'"(\.\./[^"]+)"', text)}
    return {name for name in found if name != crate and not name.startswith(f"{crate}/")}


def python_references(text: str) -> set[str]:
    """The repository-relative paths a Python test reads from outside the Python trees, by the idioms the suite
    uses: the `REPO` or `ROOT` path, or `parents[1]` of the test file, divided by a string literal."""
    names = re.findall(r'\b(?:REPO|ROOT)\s*/\s*"([^"]+)"', text)
    names += re.findall(r'parents\[1\]\s*/\s*"([^"]+)"', text)
    return {
        name.rstrip("/")
        for name in names
        if name.split("/")[0] not in PYTHON_OWN and name not in PYTHON_OWN_FILES
    }


def covered(target: str, reads: Sequence[str]) -> bool:
    """Whether a referenced path is one of a guard's declared reads, or contains or lies inside one."""
    if target == ".":
        return True  # the repository root, joined with names the guard declares individually
    return any(
        target == read.rstrip("/")
        or read.startswith(f"{target}/")
        or target.startswith(read if read.endswith("/") else f"{read}/")
        for read in reads
    )


def unregistered(texts: dict[str, str], guards: Sequence[Guard]) -> list[str]:
    """What a set of source files reads outside its own crate (or the Python trees) that no guard declares."""
    by_file = {guard.file: guard for guard in guards}
    problems = []
    for path, text in sorted(texts.items()):
        references = (
            python_references(text) if path.endswith(".py") else rust_references(path, text)
        )
        guard = by_file.get(path)
        for target in sorted(references):
            if guard is None:
                problems.append(f"{path} reads {target} and is in no guard")
            elif not covered(target, guard.reads):
                problems.append(f"{path} reads {target}, which its guard does not declare")
    return problems


def repository_texts() -> dict[str, str]:
    """The Rust files below the catalog's crate directories (`check.RUST_SOURCE_DIRS`, which `unittest-catalog`
    registers: this scan and its selection are one definition) and the Python tests, by repository-relative
    path."""
    texts: dict[str, str] = {}
    for directory in check.RUST_SOURCE_DIRS:
        for path in sorted((REPO / directory).rglob("*.rs")):
            relative = path.relative_to(REPO)
            if "target" not in relative.parts:
                texts[str(relative)] = path.read_text()
    for path in sorted((REPO / "tests").glob("test_*.py")):
        texts[f"tests/{path.name}"] = path.read_text()
    return texts


def sample(entry: str) -> str:
    """A tracked file the entry names: the entry itself, or the first file below a directory entry."""
    if not entry.endswith("/"):
        return entry
    files = sorted(
        str(path.relative_to(REPO))
        for path in (REPO / entry).rglob("*")
        if path.is_file() and "target" not in path.relative_to(REPO).parts
    )
    return files[0]


class GuardInventory(unittest.TestCase):
    """CK-84: a guard that reads files outside its group's paths is run when those files change."""

    def selected(self, path: str) -> dict[str, str]:
        return {c.step.name: c.why for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_01_every_input_of_every_guard_selects_the_group_that_runs_it_naming_the_input(
        self,
    ) -> None:
        for guard in GUARDS:
            skipped = {path for path, _ in guard.unselected}
            for read in guard.reads:
                if read in skipped:
                    continue
                for changed in {sample(read), read}:
                    chosen = self.selected(changed)
                    self.assertIn(guard.group, chosen, f"{changed} (read by {guard.file})")
                    self.assertIn(changed, chosen[guard.group], f"{changed}: {chosen[guard.group]}")

    def test_02_the_deliberate_exclusions_are_stated_and_stay_unselected(self) -> None:
        stated = 0
        for guard in GUARDS:
            for read, reason in guard.unselected:
                stated += 1
                self.assertIn(read, guard.reads)
                self.assertGreater(len(reason), 20, (guard.file, read))
                self.assertNotIn(guard.group, self.selected(sample(read)), (guard.file, read))
        self.assertGreaterEqual(stated, 4)

    def test_03_the_inventory_names_real_files_a_group_runs(self) -> None:
        names = {step.name for step in check.STEPS}
        for guard in GUARDS:
            self.assertIn(guard.group, names, guard.file)
            if guard.after and not (REPO / guard.file).exists():
                continue  # described ahead of the issue that adds it
            self.assertTrue((REPO / guard.file).is_file(), guard.file)
            for read in guard.reads:
                self.assertTrue((REPO / read).exists(), f"{guard.file} reads {read}")
        self.assertEqual(len({guard.file for guard in GUARDS}), len(GUARDS), "one entry per file")

    def test_04_the_guide_probe_rows_select_their_guards(self) -> None:
        """GUARDS-2's rows, which selected no guard in the accepted catalog."""
        rows = {
            "crates/window-glfw/src/backend.rs": "unittest-catalog",
            "crates/window-glfw/native/activation.m": "unittest-catalog",
            "apps/codekarl/src/vulkan/real.rs": "vulkan.isolation",
            "apps/codekarl/tests/vulkan_app.rs": "vulkan.isolation",
        }
        for path, group in rows.items():
            self.assertIn(group, self.selected(path), path)
        for path in ("apps/codekarl/src/vulkan/real.rs", "apps/codekarl/tests/vulkan_app.rs"):
            self.assertIn("metal.isolation", self.selected(path), path)

    def test_05_the_shared_glfw_test_support_selects_the_vulkan_window_tests_and_the_glfw_ones(
        self,
    ) -> None:
        for path in (
            "crates/window-glfw/tests/native/testing.h",
            "crates/window-glfw/tests/native/testing.m",
        ):
            chosen = self.selected(path)
            self.assertIn("vulkan.window", chosen, path)
            self.assertIn("glfw.window", chosen, path)
            self.assertNotIn("vulkan.gpu", chosen, path)
        # Nothing else of the GLFW crate reaches the Vulkan groups.
        for path in (
            "crates/window-glfw/src/backend.rs",
            "crates/window-glfw/tests/window_glfw.rs",
        ):
            self.assertNotIn("vulkan.window", self.selected(path), path)

    def test_06_the_application_and_the_glfw_crate_never_select_the_metal_gpu_or_native_window_groups(
        self,
    ) -> None:
        listing = subprocess.run(
            ["pijul", "list"], cwd=REPO, check=True, capture_output=True, text=True
        )
        paths = [
            name
            for name in listing.stdout.splitlines()
            if name.startswith(("apps/codekarl/", "crates/window-glfw/"))
            and (REPO / name).is_file()
        ]
        self.assertGreater(len(paths), 30)
        native_application_files = (
            "apps/codekarl/src/native",
            "apps/codekarl/tests/windowed.rs",
            "apps/codekarl/build.rs",
            "apps/codekarl/tests/native/",
        )
        for path in paths:
            if path.startswith(native_application_files):
                continue
            chosen = self.selected(path)
            self.assertFalse(
                {"metal.gpu", "native.window"} & chosen.keys(), f"{path}: {sorted(chosen)}"
            )

    def test_07_the_static_guards_run_in_plain_groups_and_each_target_runs_once(self) -> None:
        by_name = {step.name: step for step in check.STEPS}
        for name, package in (
            ("metal.isolation", "ck-gpu-metal"),
            ("vulkan.isolation", "ck-gpu-vulkan"),
        ):
            units = group_units(by_name[name], TARGETS)
            self.assertEqual({(unit[0], unit[2]) for unit in units}, {(package, "hook_isolation")})
            self.assertEqual(by_name[name].needs, "plain")
            self.assertEqual({need_of(unit) for unit in units}, {"plain"})
        for name in ("metal.gpu", "vulkan.gpu"):
            self.assertNotIn(
                "hook_isolation", {unit[2] for unit in group_units(by_name[name], TARGETS)}
            )
        owners: dict[Unit, list[str]] = {}
        for step in check.STEPS:
            for unit in group_units(step, TARGETS):
                owners.setdefault(unit, []).append(step.name)
        self.assertEqual({unit: names for unit, names in owners.items() if len(names) > 1}, {})
        for package in ("ck-gpu-metal", "ck-gpu-vulkan"):
            unit = (package, "test", "hook_isolation", ALL_FEATURES, "*")
            self.assertEqual(len(owners[unit]), 1, unit)

    def test_08_a_unit_test_target_added_to_a_crate_selects_the_catalog_tests(self) -> None:
        # `cargo metadata` derives the units from the manifests and the test directories.
        for path in (
            "crates/log/Cargo.toml",
            "crates/window/tests/a_new_test.rs",
            "apps/codekarl/tests/a_new_test.rs",
            "apps/codekarl/src/bin/tool.rs",
            "Cargo.toml",
        ):
            self.assertIn("unittest-catalog", self.selected(path), path)
        self.assertNotIn("unittest-batch", self.selected("crates/window-glfw/src/backend.rs"))

    def test_09_the_python_tests_are_in_exactly_one_group_and_the_catalog_tests_have_their_own(
        self,
    ) -> None:
        groups = dict(check.UNITTEST_GROUPS)
        self.assertEqual(groups["catalog"], ("test_check_catalog",))
        self.assertNotIn("test_check_catalog", groups["batch"])
        check.verify_groups(REPO / "tests")

    def test_10_every_reference_outside_the_crate_in_the_repository_is_in_the_inventory(
        self,
    ) -> None:
        texts = repository_texts()
        self.assertGreater(len(texts), 100)
        self.assertEqual(unregistered(texts, GUARDS), [])
        # The scan is not vacuous: the inventory's own examples are among what it finds.
        self.assertIn(
            "crates/window-glfw/tests/native/testing.m",
            rust_references("crates/gpu-vulkan/build.rs", texts["crates/gpu-vulkan/build.rs"]),
        )
        self.assertIn(
            "apps/codekarl/tests/supervisor/mod.rs",
            rust_references(
                "crates/gpu-vulkan/tests/gpu.rs", texts["crates/gpu-vulkan/tests/gpu.rs"]
            ),
        )
        self.assertIn(
            "crates/window-glfw/src",
            python_references(texts["tests/test_check_catalog.py"]),
        )

    def test_11_a_guard_that_reads_outside_its_group_and_is_not_registered_fails_the_scan(
        self,
    ) -> None:
        reading = {
            "crates/a/tests/guard.rs": 'let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../b/src");',
            "crates/a/build.rs": 'const SOURCE: &str = "../c/native/x.m";',
            "crates/a/tests/include.rs": 'const T: &str = include_str!("../../d/src/lib.rs");',
            "crates/a/tests/path.rs": '#[path = "../../../apps/x/tests/supervisor/mod.rs"]\nmod supervisor;',
            "tests/test_new.py": f"text = (REPO / {QUOTE}crates/z/src/lib.rs{QUOTE}).read_text()",
        }
        problems = unregistered(reading, GUARDS)
        self.assertEqual(len(problems), 5, problems)
        for path in reading:
            self.assertTrue(any(problem.startswith(path) for problem in problems), path)
        # Inside the crate nothing is a read of someone else's files.
        inside = {
            "crates/a/tests/guard.rs": 'include_str!("../src/lib.rs"); Path::new(env!("CARGO_MANIFEST_DIR")).join("src")',
            "crates/a/tests/other/x.rs": '#[path = "y.rs"]\nmod y;',
            "tests/test_other.py": f"text = (REPO / {QUOTE}tools/check.py{QUOTE}).read_text()",
        }
        self.assertEqual(unregistered(inside, GUARDS), [])
        # Registered, it passes; with the wrong read declared, it does not.
        guard = Guard("crates/a/tests/guard.rs", "g", ("crates/b/src/",))
        text = {"crates/a/tests/guard.rs": reading["crates/a/tests/guard.rs"]}
        self.assertEqual(unregistered(text, [guard]), [])
        wrong = Guard("crates/a/tests/guard.rs", "g", ("crates/q/",))
        self.assertEqual(len(unregistered(text, [wrong])), 1)


class ScannerInputs(unittest.TestCase):
    """CK-84 fix 1: the inventory scan's own input domain selects the group that runs it."""

    def selected(self, path: str) -> dict[str, str]:
        return {c.step.name: c.why for c in check.plan(check.STEPS, [path]).choices if c.selected}

    def test_01_every_file_the_scan_reads_selects_the_catalog_tests_naming_it(self) -> None:
        texts = repository_texts()
        self.assertGreater(len(texts), 100)
        for path in texts:
            chosen = self.selected(path)
            self.assertIn("unittest-catalog", chosen, path)
            self.assertIn(path, chosen["unittest-catalog"], path)

    def test_02_a_new_reader_and_a_build_script_select_the_catalog_tests(self) -> None:
        # A new recognized reader under an existing `src/` directory, a new test file and a build script.
        for path in (
            "crates/log/src/a_new_reader.rs",
            "crates/runtime/src/deep/er/a_new_reader.rs",
            "apps/codekarl/src/new_module/reader.rs",
            "apps/codekarl/tests/a_new_test.rs",
            "crates/gpu-metal/build.rs",
            "crates/window-cocoa/build.rs",
            "crates/window-glfw/build.rs",
        ):
            self.assertIn("unittest-catalog", self.selected(path), path)
        # A crate nobody registered is unclaimed: its plan fails wide, which selects the catalog tests too.
        for path in ("crates/gpu-future/build.rs", "crates/gpu-future/src/lib.rs"):
            self.assertIn("claimed by no group", self.selected(path)["unittest-catalog"], path)

    def test_03_the_scan_and_the_selection_share_one_definition_of_the_crates(self) -> None:
        by_name = {step.name: step for step in check.STEPS}
        self.assertTrue(set(check.RUST_SOURCE_DIRS) <= set(by_name["unittest-catalog"].paths))
        # Every crate directory of the workspace is one of them, so the scan cannot miss a crate.
        manifests = {
            str(path.parent.relative_to(REPO)) + "/"
            for root in ("apps", "crates")
            for path in (REPO / root).glob("*/Cargo.toml")
        }
        self.assertEqual(manifests, set(check.RUST_SOURCE_DIRS))
        self.assertEqual(
            {
                path.split("/")[1]
                for path in repository_texts()
                if path.startswith(("apps/", "crates/"))
            },
            {directory.split("/")[1] for directory in check.RUST_SOURCE_DIRS},
        )


class LicenseGroups(unittest.TestCase):
    """CK-153: the root `LICENSE` is claimed by the doctor group, so it selects the floor and no window, GPU or Vulkan group."""

    def test_01_license_is_claimed_and_selects_only_the_floor(self) -> None:
        by_name = {step.name: step for step in check.STEPS}
        self.assertIn("LICENSE", by_name["doctor"].paths)
        self.assertTrue(check.is_claimed(check.STEPS, "LICENSE"))
        plan = check.plan(check.STEPS, ["LICENSE"])
        self.assertFalse([c for c in plan.choices if "claimed by no group" in c.why])
        selected = {c.step.name: c.step.category for c in plan.choices if c.selected}
        self.assertEqual(set(selected.values()), {"floor"}, selected)


class RgIgnoreGroups(unittest.TestCase):
    """CK-86: `.rgignore` (ripgrep's ignore file) is claimed, and the test that reads it is selected by it."""

    def chosen(self, path: str) -> dict[str, str]:
        plan = check.plan(check.STEPS, [path])
        self.assertFalse(
            [c for c in plan.choices if "claimed by no group" in c.why], f"{path} would fail wide"
        )
        return {c.step.name: c.why for c in plan.choices if c.selected}

    def test_01_rgignore_is_claimed_by_the_doctor_group_beside_ignore(self) -> None:
        by_name = {step.name: step for step in check.STEPS}
        self.assertIn(".rgignore", by_name["doctor"].paths)
        self.assertIn(".ignore", by_name["doctor"].paths)
        # Claimed (the plan does not fail wide, asserted by `chosen`), and its reader is selected by name.
        self.assertTrue(check.is_claimed(check.STEPS, ".rgignore"))
        self.assertIn(".rgignore", self.chosen(".rgignore")["unittest-check"])

    def test_02_a_change_to_either_ignore_file_or_agents_md_selects_the_test_that_reads_them(
        self,
    ) -> None:
        for path in (".rgignore", ".ignore", "AGENTS.md"):
            self.assertIn("unittest-check", self.chosen(path), path)
            self.assertNotIn("native.window", self.chosen(path), path)
            self.assertNotIn("metal.gpu", self.chosen(path), path)

    def test_02b_a_change_below_docs_selects_the_search_test_naming_it(
        self,
    ) -> None:
        for path in (
            "docs/guide/CURSOR.md",  # deleting or editing the guide's cursor
            "docs/guide/a_new_report.md",  # a new file under the guide's directory
            "docs/archive/a_new_record.md",  # a new archived record
            "docs/archive/reviews/a_new_review.md",
            "docs/vision.md",
        ):
            chosen = self.chosen(path)
            self.assertIn("unittest-check", chosen, path)
            self.assertIn(path, chosen["unittest-check"], path)
            self.assertNotIn("native.window", chosen, path)
            self.assertNotIn("metal.gpu", chosen, path)
        # A directory entry (a new untracked directory below docs) selects it as well.
        self.assertIn("unittest-check", self.chosen("docs/archive/new_directory/"))

    def test_03_the_test_module_is_in_a_group_and_inventoried(self) -> None:
        groups = dict(check.UNITTEST_GROUPS)
        self.assertIn("test_rgignore", groups["check"])
        self.assertIn("tests/test_rgignore.py", {guard.file for guard in GUARDS})


class NoSelfDesignatedMainThread(unittest.TestCase):
    def test_01_the_glfw_crate_has_no_api_that_lets_a_thread_designate_itself_the_main_thread(
        self,
    ) -> None:
        """CK-78 fix 3: the main thread is told by the system (pthread_main_np, gettid == getpid), never by
        the first caller of a capture function."""
        sources = sorted((REPO / "crates/window-glfw/src").rglob("*.rs"))
        self.assertGreater(len(sources), 4)
        found = [
            f"{path.relative_to(REPO)}: {line.strip()}"
            for path in sources
            for line in path.read_text().splitlines()
            if "capture_main_thread" in line or "MAIN_THREAD" in line
        ]
        self.assertEqual(found, [])


class NoLoaderVariable(unittest.TestCase):
    def test_01_no_test_or_check_of_the_glfw_crate_or_the_doctor_names_a_dynamic_loader_variable(
        self,
    ) -> None:
        """CK-78 r5: the program is handed the Vulkan loader's function and GLFW never searches, so nothing
        sets or suggests DYLD_FALLBACK_LIBRARY_PATH or another dynamic-loader variable."""
        files = [
            path for path in sorted((REPO / "crates/window-glfw").rglob("*")) if path.is_file()
        ]
        files.append(REPO / "tools/check.py")
        self.assertGreater(len(files), 8)
        found = [
            f"{path.relative_to(REPO)}: {line.strip()}"
            for path in files
            for line in path.read_text(errors="replace").splitlines()
            if "DYLD_" in line or "LD_LIBRARY_PATH" in line
        ]
        self.assertEqual(found, [])


class GraphicsDoctor(unittest.TestCase):
    """The doctor's GLFW, Vulkan loader and validation-layer checks, against a fixture installation."""

    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ck-graphics-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(os.path.realpath(self.temp.name))
        self.keg = self.base / "prefix/opt/glfw"
        (self.keg / "include/GLFW").mkdir(parents=True)
        (self.keg / "lib").mkdir()
        (self.keg / "lib/libglfw.dylib").write_text("")
        self.header(3, 5, 1)
        self.libs = self.base / "usr/local/lib"
        self.libs.mkdir(parents=True)
        (self.libs / "libvulkan.1.dylib").write_text("")
        self.layers = self.base / "usr/local/share/vulkan/explicit_layer.d"
        self.layers.mkdir(parents=True)
        self.layer(json.dumps({"layer": {"name": "VK_LAYER_KHRONOS_validation"}}))
        self.bin = self.base / "usr/local/bin"
        self.bin.mkdir(parents=True)
        self.glslc(self.bin, "shaderc v2025.1 v2025.1\nspirv-tools v2025.1")

    def glslc(self, directory: Path, text: str | None, exit_status: int = 0) -> Path:
        """A fake `glslc` in `directory` that prints `text` and exits with `exit_status` (none when `text` is None)."""
        path = directory / "glslc"
        if text is None:
            path.unlink(missing_ok=True)
            return path
        path.write_text(f"#!/bin/sh\ncat <<'EOF'\n{text}\nEOF\nexit {exit_status}\n")
        path.chmod(0o755)
        return path

    def header(self, major: int, minor: int, revision: int) -> None:
        (self.keg / "include/GLFW/glfw3.h").write_text(
            f"#define GLFW_VERSION_MAJOR          {major}\n"
            f"#define GLFW_VERSION_MINOR          {minor}\n"
            f"#define GLFW_VERSION_REVISION       {revision}\n"
        )

    def layer(self, text: str) -> None:
        (self.layers / check.VALIDATION_LAYER).write_text(text)

    sdk = ""

    def run_doctor(
        self,
        runtime: tuple[int, int, int] = (3, 5, 1),
        loader_version: tuple[int, int, int] = (1, 3, 296),
        status: int = 0,
    ) -> str:
        """Runs the check against the fixture; returns what it printed or raises its RuntimeError."""
        packed = (loader_version[0] << 22) | (loader_version[1] << 12) | loader_version[2]

        def vulkan_version(pointer: object) -> int:
            ctypes_value = getattr(pointer, "_obj", None)
            assert ctypes_value is not None
            ctypes_value.value = packed
            return status

        library = mock.MagicMock()
        library.vkEnumerateInstanceVersion.side_effect = vulkan_version
        printed: list[str] = []
        with (
            mock.patch.dict(os.environ, {"HOMEBREW_PREFIX": str(self.base / "prefix")}),
            mock.patch.dict(os.environ, {"VULKAN_SDK": self.sdk}),
            mock.patch.object(check, "GLSLC_DIRECTORIES", (str(self.bin),)),
            mock.patch.object(check, "glfw_runtime_version", return_value=runtime),
            mock.patch.object(check, "VULKAN_LIBRARY_DIRECTORIES", (str(self.libs),)),
            mock.patch.object(check, "VULKAN_LAYER_DIRECTORIES", (str(self.layers),)),
            mock.patch("ctypes.CDLL", return_value=library),
            mock.patch(
                "builtins.print", side_effect=lambda *a, **k: printed.append(" ".join(map(str, a)))
            ),
        ):
            check.graphics_doctor()
        return "\n".join(printed)

    def refused(self, *args: object, **kwargs: object) -> str:
        with self.assertRaises(RuntimeError) as caught:
            self.run_doctor(*args, **kwargs)  # type: ignore[arg-type]
        return str(caught.exception)

    def test_01_a_complete_installation_passes_and_names_what_it_found(self) -> None:
        shown = self.run_doctor()
        self.assertIn("GLFW 3.5.1", shown)
        self.assertIn("expected GLFW 3.4 or newer 3.x", shown)
        self.assertIn("Vulkan loader 1.3.296", shown)
        self.assertIn(str(self.libs / "libvulkan.1.dylib"), shown, "the loader's absolute path")
        self.assertNotIn("DYLD_", shown, "no dynamic-loader variable is suggested")
        self.assertIn("VK_LAYER_KHRONOS_validation", shown)
        self.header(3, 4, 0)
        self.assertIn("GLFW 3.4.0", self.run_doctor(runtime=(3, 4, 0)))

    def test_02_missing_glfw_names_expected_and_found_and_the_install_command(self) -> None:
        (self.keg / "include/GLFW/glfw3.h").unlink()
        message = self.refused()
        self.assertIn("GLFW 3.4 or newer 3.x", message)
        self.assertIn("found no", message)
        self.assertIn("tools/bootstrap", message)
        (self.keg / "lib/libglfw.dylib").unlink()
        self.header(3, 5, 1)
        self.assertIn("libglfw.dylib", self.refused())

    def test_03_a_version_other_than_3_4_or_newer_3_x_is_refused(self) -> None:
        for declared, running in (
            ((3, 3, 4), (3, 3, 4)),
            ((4, 0, 0), (4, 0, 0)),
            ((2, 9, 0), (2, 9, 0)),
        ):
            with self.subTest(running):
                self.header(*declared)
                message = self.refused(runtime=running)
                self.assertIn("Expected GLFW 3.4 or newer 3.x", message)
                self.assertIn(".".join(map(str, running)), message)
        # The header and the library must agree.
        self.header(3, 5, 1)
        message = self.refused(runtime=(3, 4, 0))
        self.assertIn("header declaring 3.5.1", message)
        self.assertIn("found the library 3.4.0", message)
        self.header(3, 4, 0)
        self.header(3, 5, 1)
        (self.keg / "include/GLFW/glfw3.h").write_text("no version here\n")
        self.assertIn("GLFW_VERSION_MAJOR", self.refused())

    def test_04_the_vulkan_loader_is_required_and_must_be_1_3_or_newer(self) -> None:
        self.assertIn("Vulkan loader 1.4.0", self.run_doctor(loader_version=(1, 4, 0)))
        message = self.refused(loader_version=(1, 2, 198))
        self.assertIn("1.3 or newer", message)
        self.assertIn("1.2.198", message)
        self.assertIn("status", self.refused(status=-3))
        (self.libs / "libvulkan.1.dylib").unlink()
        message = self.refused()
        self.assertIn("libvulkan.1.dylib", message)
        self.assertIn("found none", message)

    def test_05_the_validation_layer_manifest_is_required_and_must_name_the_layer(self) -> None:
        self.layer(json.dumps({"layer": {"name": "VK_LAYER_other"}}))
        self.assertIn("VK_LAYER_other", self.refused())
        self.layer("not json")
        self.assertIn("cannot be read", self.refused())
        self.layer(json.dumps({"layer": {}}))
        self.assertIn("cannot be read", self.refused())
        (self.layers / check.VALIDATION_LAYER).unlink()
        message = self.refused()
        self.assertIn(check.VALIDATION_LAYER, message)
        self.assertIn("found none", message)

    def test_07_glslc_is_found_by_explicit_path_and_run(self) -> None:
        shown = self.run_doctor()
        self.assertIn("glslc shaderc v2025.1", shown)
        self.assertIn(str(self.bin / "glslc"), shown, "the path it was found at")

    def test_08_a_missing_glslc_names_the_directories_searched_and_the_install_command(
        self,
    ) -> None:
        self.glslc(self.bin, None)
        message = self.refused()
        self.assertIn("glslc", message)
        self.assertIn(str(self.bin), message, "the directory that was searched")
        self.assertIn("found none", message)
        self.assertIn("shaderc", message, "the install command")
        self.assertIn("tools/bootstrap", message)

    def test_09_a_glslc_that_does_not_run_is_refused_naming_it(self) -> None:
        self.glslc(self.bin, "boom", exit_status=3)
        message = self.refused()
        self.assertIn(str(self.bin / "glslc"), message)
        self.assertIn("does not run", message)
        self.assertIn("shaderc", message)

    def test_10_vulkan_sdk_is_searched_first_and_a_missing_one_falls_through(self) -> None:
        sdk = self.base / "sdk"
        (sdk / "bin").mkdir(parents=True)
        self.glslc(sdk / "bin", "shaderc from the SDK")
        self.sdk = str(sdk)
        shown = self.run_doctor()
        self.assertIn("shaderc from the SDK", shown)
        self.assertIn(str(sdk / "bin" / "glslc"), shown)
        # An SDK directory without glslc falls through to the fixed directories.
        self.glslc(sdk / "bin", None)
        self.assertIn("shaderc v2025.1", self.run_doctor())
        # And with none anywhere, the message names the SDK's bin first, then the fixed one.
        self.glslc(self.bin, None)
        message = self.refused()
        self.assertLess(message.index(str(sdk / "bin")), message.index(str(self.bin)))

    def test_11_the_search_order_is_the_build_scripts_and_the_bootstrap_installs_when_none_has_it(
        self,
    ) -> None:
        build = (REPO / "crates/gpu-vulkan/build.rs").read_text()
        listed = re.search(r"const GLSLC_DIRECTORIES: \[&str; \d+\] = \[([^\]]*)\];", build)
        assert listed is not None, "build.rs lists its directories in GLSLC_DIRECTORIES"
        self.assertEqual(tuple(re.findall(r'"([^"]+)"', listed.group(1))), check.GLSLC_DIRECTORIES)
        self.assertIn("VULKAN_SDK", build)
        bootstrap = (REPO / "tools/bootstrap").read_text()
        self.assertIn("brew install shaderc", bootstrap)
        for place in check.GLSLC_DIRECTORIES:
            self.assertIn(f"{place}/glslc", bootstrap)
        self.assertIn("VULKAN_SDK", bootstrap)

    def test_12_the_command_asks_for_the_graphics_check_by_default_and_the_floor_does_not(
        self,
    ) -> None:
        with (
            mock.patch.object(check, "graphics_doctor") as graphics,
            mock.patch.object(
                check, "probe", side_effect=RuntimeError("stop after the graphics check")
            ),
        ):
            with self.assertRaises(RuntimeError):
                check.doctor()
            graphics.assert_called_once_with()
            graphics.reset_mock()
            with self.assertRaises(RuntimeError):
                check.doctor(graphics=False)
            graphics.assert_not_called()


class Quick(unittest.TestCase):
    def test_01_quick_and_fast_keep_their_meaning_through_the_catalog(self) -> None:
        self.assertEqual(check.select("quick"), ["fmt", "ruff", "shellcheck"])
        self.assertEqual(check.select("fast"), check.select("full"))
        # CK-130: an optional step (`metal.scale`) has no level, so no level selects it.
        everything = [step.name for step in check.STEPS if step.levels]
        self.assertEqual(check.select("full"), everything)
        self.assertEqual(
            check.select("planned", [check.STEPS[0], check.STEPS[3]]),
            [check.STEPS[0].name, check.STEPS[3].name],
        )
        self.assertEqual(check.select("planned", []), [])


class ChangeSet(unittest.TestCase):
    """The change set against the accepted base, on a disposable repository (pijul init/add, no record)."""

    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ck-changeset-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(os.path.realpath(self.temp.name)) / "copy"
        self.root.mkdir()
        self.hub = self.root.parent / "hub"
        for path, text in {
            "a.txt": "a\n",
            "b.txt": "b\n",
            "script.sh": "#!/bin/sh\n",
            "d/c.txt": "c\n",
            "m.txt": "move me\n",
        }.items():
            (self.root / path).parent.mkdir(parents=True, exist_ok=True)
            (self.root / path).write_text(text)
        for arguments in (["init"], ["add", "--recursive", "."]):
            subprocess.run(["pijul", *arguments], cwd=self.root, check=True, capture_output=True)
        self.write_ignore("/target\n__pycache__\n*.o\n")
        self.base = self.fingerprint()

    def write_ignore(self, text: str) -> None:
        (self.root / ".ignore").write_text(text)

    def write(self, files: dict[str, str]) -> None:
        for name, text in files.items():
            (self.root / name).parent.mkdir(parents=True, exist_ok=True)
            (self.root / name).write_text(text)

    def listing(self) -> list[str]:
        shown = subprocess.run(
            ["pijul", "list"], cwd=self.root, check=True, capture_output=True, text=True
        )
        return shown.stdout.splitlines()

    def pijul_would_add(self, *paths: str) -> list[str]:
        """The files Pijul itself adds below `paths` (the oracle for the ignore rules): it tracks them
        in this disposable repository, and the files it now tracks, minus the ones before, are listed."""
        before = set(self.listing())
        subprocess.run(
            ["pijul", "add", "--recursive", *paths], cwd=self.root, check=True, capture_output=True
        )
        return sorted(name for name in set(self.listing()) - before if (self.root / name).is_file())

    def fingerprint(self) -> dict[str, dict[str, object]]:
        found: dict[str, dict[str, object]] = {}
        for path in sorted(self.root.rglob("*")):
            if path.is_file() and ".pijul" not in path.parts:
                found[path.relative_to(self.root).as_posix()] = {
                    "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                    "executable": bool(path.stat().st_mode & 0o111),
                }
        return found

    def changed(self, base: dict[str, dict[str, object]] | None = None) -> list[str]:
        pointer = {"fingerprint": {"files": self.base if base is None else base}}
        with mock.patch.object(check, "read_pointer", return_value=pointer):
            return check.changed_paths(self.root, self.hub)

    def test_01_a_copy_equal_to_its_base_has_no_changes(self) -> None:
        self.assertEqual(self.changed(), [])

    def test_02_edits_additions_deletions_moves_and_mode_changes_are_all_seen(self) -> None:
        # Recorded-but-unaccepted patches and unrecorded edits both differ from the accepted bytes, so the
        # comparison needs no record to tell them apart; here none is recorded (a record signs).
        (self.root / "a.txt").write_text("a edited\n")  # an edit of a tracked file
        (self.root / "b.txt").unlink()  # a deletion that is still tracked
        (self.root / "n.txt").write_text("new\n")  # an addition that was never added to pijul
        (self.root / "script.sh").chmod(0o755)  # a mode change
        (self.root / "m.txt").rename(self.root / "moved.txt")  # a move: the old and the new path
        self.assertEqual(
            self.changed(),
            ["a.txt", "b.txt", "m.txt", "moved.txt", "n.txt", "script.sh"],
        )

    def test_03_an_added_but_unrecorded_file_is_seen(self) -> None:
        (self.root / "added.txt").write_text("x\n")
        subprocess.run(
            ["pijul", "add", "added.txt"], cwd=self.root, check=True, capture_output=True
        )
        self.assertEqual(self.changed(), ["added.txt"])

    def test_04_a_file_the_base_lacks_and_one_the_copy_lacks_are_both_changes(self) -> None:
        shorter = {k: v for k, v in self.base.items() if k != "d/c.txt"}
        self.assertEqual(self.changed(shorter), ["d/c.txt"])
        extra = {**self.base, "gone.txt": {"sha256": "0" * 64, "executable": False}}
        self.assertEqual(self.changed(extra), ["gone.txt"])

    def test_05_a_failure_to_compare_raises_and_is_never_an_empty_change_set(self) -> None:
        with (
            mock.patch.object(
                check, "read_pointer", side_effect=RuntimeError("no accepted pointer")
            ),
            self.assertRaises(check.ChangeSetError) as caught,
        ):
            check.changed_paths(self.root, self.hub)
        self.assertIn("no accepted pointer", str(caught.exception))
        (self.root / "link").symlink_to("a.txt")
        subprocess.run(["pijul", "add", "link"], cwd=self.root, check=False, capture_output=True)
        with self.assertRaises(check.ChangeSetError) as linked:
            self.changed()
        self.assertIn("symbolic link", str(linked.exception))
        (self.root / "link").unlink()
        with (
            mock.patch.object(check, "read_pointer", return_value={"fingerprint": {}}),
            self.assertRaises(check.ChangeSetError),
        ):
            check.changed_paths(self.root, self.hub)
        with (
            mock.patch.dict(os.environ, {"PATH": "/nonexistent"}),
            mock.patch.object(
                check, "read_pointer", return_value={"fingerprint": {"files": self.base}}
            ),
            self.assertRaises(check.ChangeSetError),
        ):
            check.changed_paths(self.root, self.hub)

    def test_07_an_untracked_directory_is_one_directory_entry_below_a_tracked_parent_and_at_the_root(
        self,
    ) -> None:
        # Pijul reports an untracked directory as one entry; it is neither walked nor interpreted.
        self.write({"d/new/src/lib.rs": "x\n", "d/new/Cargo.toml": "y\n", "fresh/x/y/z.rs": "z\n"})
        (self.root / "d/n.txt").write_text("a file stays a file\n")
        self.assertEqual(self.changed(), ["d/n.txt", "d/new/", "fresh/"])

    def test_08_whatever_pijul_would_ignore_the_directory_is_still_reported(self) -> None:
        # Ignored contents can only make the plan wider: the directory is the change, not its files.
        self.write(
            {
                "only/__pycache__/x.pyc": "p\n",
                "only/a.o": "o\n",
                "mix/keep.rs": "k\n",
                "mix/__pycache__/y.pyc": "p\n",
            }
        )
        (self.root / "mix/__pycache__/lnk").symlink_to("../keep.rs")  # never looked at
        self.assertEqual(self.changed(), ["mix/", "only/"])

    def test_09_a_symbolic_link_as_an_untracked_entry_is_refused_as_before(self) -> None:
        (self.root / "linked").symlink_to("d")  # an untracked symlinked directory
        with self.assertRaises(check.ChangeSetError) as top:
            self.changed()
        self.assertIn("symbolic link", str(top.exception))
        (self.root / "linked").unlink()
        (self.root / "alias.txt").symlink_to("a.txt")  # an untracked symlinked file
        with self.assertRaises(check.ChangeSetError) as file:
            self.changed()
        self.assertIn("alias.txt", str(file.exception))

    def test_10_an_unreadable_untracked_directory_is_still_a_change(self) -> None:
        self.write({"new/locked/keep.rs": "k\n", "new/open.rs": "o\n"})
        locked = self.root / "new/locked"
        locked.chmod(0o000)
        self.addCleanup(locked.chmod, 0o755)
        found = self.changed()
        self.assertEqual(found, ["new/"])
        # Never a floor-only plan: the unclaimed directory selects every affected group.
        chosen = check.plan(check.STEPS, found)
        self.assertTrue(all(c.selected for c in chosen.choices if c.step.category == "affected"))
        locked.chmod(0o755)

    def test_11_the_rules_pijul_reads_differently_from_other_tools_cannot_hide_a_file(self) -> None:
        # The review's cases: a rule with a leading space, and a caret class. Real Pijul adds the file
        # (the oracle); we never evaluate the rules, so the directory is reported whatever they say.
        for number, (rules, name) in enumerate(((" secret\n", "secret"), ("[^x].rs\n", "x.rs"))):
            with self.subTest(rules):
                self.write_ignore(rules)
                directory = f"tree{number}"
                self.write({f"{directory}/{name}": "s\n"})
                self.assertIn(f"{directory}/", self.changed())
                self.assertEqual(self.pijul_would_add(directory), [f"{directory}/{name}"])

    def test_12_a_new_crate_before_pijul_add_is_a_directory_the_planner_widens_or_attributes(
        self,
    ) -> None:
        self.write({"d/gpu-vulkan/src/lib.rs": "pub fn f() {}\n", "ck/x/lib.rs": "y\n"})
        self.assertEqual(self.changed(), ["ck/", "d/gpu-vulkan/"])

    def test_06_planned_prints_the_change_set_and_fails_wide_when_it_cannot_be_computed(
        self,
    ) -> None:
        with (
            mock.patch.object(check, "verify_groups"),
            mock.patch.object(check, "changed_paths", side_effect=check.ChangeSetError("boom")),
            mock.patch("builtins.print") as printed,
        ):
            built = check.planned(self.root, self.hub, True, [])
        self.assertTrue(all(c.selected for c in built.choices if c.step.category == "affected"))
        shown = "\n".join(str(call.args[0]) for call in printed.call_args_list)
        self.assertIn("boom", shown)
        self.assertEqual(shown.count("check: plan:"), len(check.STEPS))


class Command(unittest.TestCase):
    """The command line: which options make a planned check, and what they refuse."""

    def run_main(
        self, arguments: Sequence[str], env: dict[str, str] | None = None
    ) -> tuple[int, str, list[tuple[str, list[str] | None]]]:
        started: list[tuple[str, list[str] | None]] = []

        def execute(
            root: Path, hub: Path, level: str, steps: Sequence[check.Step] | None = None
        ) -> int:
            started.append((level, None if steps is None else [s.name for s in steps]))
            return 0

        errors = StringIO()
        with (
            mock.patch.object(sys, "argv", ["check.py", *arguments]),
            mock.patch.object(check, "enroll", return_value=True),
            mock.patch.object(check, "configure", return_value=REPO),
            mock.patch.object(check, "signing"),
            mock.patch.object(check, "execute", side_effect=execute),
            mock.patch.object(check, "changed_paths", return_value=["crates/window/src/lib.rs"]),
            mock.patch.dict(os.environ, env or {}),
            mock.patch("sys.stderr", errors),
            mock.patch("builtins.print"),
        ):
            # Batch validation runs this suite with the CI lock descriptor exported, and a planned check
            # refuses that environment: a test gets it only when it sets it on purpose.
            if "CK_CI_LOCK_FD" not in (env or {}):
                os.environ.pop("CK_CI_LOCK_FD", None)
            try:
                code = check.main()
            except SystemExit as error:
                code = int(error.code) if isinstance(error.code, int) else 1
        return code, errors.getvalue(), started

    def test_01_the_plain_levels_are_unchanged(self) -> None:
        self.assertEqual(self.run_main([])[2], [("full", None)])
        self.assertEqual(self.run_main(["--fast"])[2], [("fast", None)])
        self.assertEqual(self.run_main(["--quick"])[2], [("quick", None)])

    def test_02_changed_runs_a_planned_recipe_of_the_selected_groups(self) -> None:
        code, _, started = self.run_main(["--changed"])
        self.assertEqual(code, 0)
        ((level, names),) = started
        self.assertEqual(level, "planned")
        assert names is not None
        self.assertIn("rust.core", names)
        self.assertNotIn("native.window", names)
        self.assertNotIn("metal.gpu", names)
        self.assertEqual(names[:6], ["fmt", "ruff", "shellcheck", "doctor", "clippy", "doc"])

    def test_03_group_names_a_group_and_runs_no_native_group_unless_named(self) -> None:
        _, _, started = self.run_main(["--group", "rust.core"])
        ((level, names),) = started
        self.assertEqual(level, "planned")
        assert names is not None
        self.assertIn("rust.core", names)
        self.assertNotIn("rust.app", names)
        self.assertNotIn("native.window", names)
        _, _, started = self.run_main(
            ["--changed", "--group", "native.window", "--group", "metal.gpu"]
        )
        assert started[0][1] is not None
        self.assertIn("native.window", started[0][1])
        self.assertIn("metal.gpu", started[0][1])

    def test_04_plan_prints_and_runs_nothing(self) -> None:
        code, _, started = self.run_main(["--changed", "--plan"])
        self.assertEqual((code, started), (0, []))

    def test_05_refusals(self) -> None:
        for arguments, message in (
            (["--plan"], "--plan needs"),
            (["--changed", "--quick"], "do not mix"),
            (["--group", "rust.core", "--fast"], "do not mix"),
            (["--group", "no.such.group"], "unknown group"),
        ):
            with self.subTest(arguments):
                code, errors, started = self.run_main(arguments)
                self.assertEqual(code, 2)
                self.assertIn(message, errors)
                self.assertEqual(started, [])

    def test_06_a_planned_check_is_never_batch_evidence_and_refuses_the_batch_environment(
        self,
    ) -> None:
        code, errors, started = self.run_main(["--changed"], {"CK_CI_LOCK_FD": "9"})
        self.assertEqual(code, 2)
        self.assertIn("never batch evidence", errors)
        self.assertEqual(started, [])
        # The full level still honours the batch environment (execute decides).
        self.assertEqual(self.run_main([], {"CK_CI_LOCK_FD": "9"})[2], [("full", None)])

    def test_07_a_planned_run_names_itself_and_never_uses_the_full_label(self) -> None:
        printed: list[str] = []
        with (
            mock.patch(
                "builtins.print", side_effect=lambda *a, **k: printed.append(" ".join(map(str, a)))
            ),
            mock.patch.object(
                check, "acquire_slot", return_value=(0, os.open(os.devnull, os.O_RDONLY))
            ),
            mock.patch.object(check, "checks"),
            mock.patch.dict(os.environ),
        ):
            os.environ.pop("CK_CI_LOCK_FD", None)
            code = check.execute(REPO, REPO, "planned", steps=[check.STEPS[0]])
        self.assertEqual(code, 0)
        self.assertTrue(any(line.startswith("check: recipe planned;") for line in printed), printed)
        self.assertFalse(any("recipe full" in line for line in printed))
        self.assertTrue(
            any(line.startswith("check planned:") and line.endswith("PASS") for line in printed),
            printed,
        )

    def test_08_a_planned_run_requires_full_spec_for_the_groups_that_set_it(self) -> None:
        state = check.State(REPO, REPO, "planned", batch=False, target=REPO / "target")
        by_name = {step.name: step for step in check.STEPS}
        with mock.patch.dict(os.environ):
            os.environ.pop("CK_REQUIRE_FULL_SPEC", None)
            for name in ("metal.gpu", "native.window"):
                self.assertEqual(
                    state.environment(by_name[name]).get("CK_REQUIRE_FULL_SPEC"), "1", name
                )
            self.assertNotIn("CK_REQUIRE_FULL_SPEC", state.environment(by_name["rust.app-default"]))


class BatchEnvironment(unittest.TestCase):
    """The command tests do not depend on the caller's environment: batch validation exports
    `CK_CI_LOCK_FD` (and the other batch variables) to the process that runs the suite."""

    def test_01_the_command_tests_pass_with_the_batch_variables_exported(self) -> None:
        env = {
            **os.environ,
            "CK_CI_LOCK_FD": "9",
            "CK_REQUIRE_FULL_SPEC": "1",
            "CK_CHAT_INPUTS": "/nonexistent/chat-inputs.json",
        }
        result = subprocess.run(
            [sys.executable, "-m", "unittest", "-b", "test_check_catalog.Command"],
            cwd=Path(__file__).resolve().parent,
            env=env,
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("OK", result.stderr)

    def test_02_the_helper_clears_the_lock_variable_unless_a_test_sets_it_and_restores_it(
        self,
    ) -> None:
        with mock.patch.dict(os.environ, {"CK_CI_LOCK_FD": "9"}):
            code, _, started = Command("test_01_the_plain_levels_are_unchanged").run_main(
                ["--changed"]
            )
            self.assertEqual((code, [level for level, _ in started]), (0, ["planned"]))
            refused, errors, none = Command("test_01_the_plain_levels_are_unchanged").run_main(
                ["--changed"], {"CK_CI_LOCK_FD": "9"}
            )
            self.assertEqual((refused, none), (2, []))
            self.assertIn("never batch evidence", errors)
            self.assertEqual(os.environ["CK_CI_LOCK_FD"], "9", "the caller's value is restored")


if __name__ == "__main__":
    unittest.main()
