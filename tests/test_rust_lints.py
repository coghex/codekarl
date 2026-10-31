"""S8: the workspace lint configuration and the suppression policy for Rust sources.

The behaviour of each rule is proved by controlled violations (recorded with the slice); these
tests pin the configuration so a rule cannot be dropped, weakened or silently suppressed.
"""

import re
import tomllib
import unittest
from collections.abc import Callable
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
RUST_SOURCES = sorted(
    path
    for root in ("apps", "crates")
    for path in (REPO / root).rglob("*.rs")
    if "target" not in path.parts
)


def table(value: object) -> dict[str, object]:
    """A TOML table of a manifest, checked rather than assumed."""
    assert isinstance(value, dict)
    return value


def manifest(name: str) -> dict[str, object]:
    return tomllib.loads((REPO / name).read_text())


class WorkspaceLints(unittest.TestCase):
    def lints(self) -> dict[str, object]:
        return table(table(manifest("Cargo.toml")["workspace"])["lints"])

    def test_01_clippy_has_pedantic_below_the_individual_rules_and_the_safety_rules_denied(
        self,
    ) -> None:
        clippy = table(self.lints()["clippy"])
        self.assertEqual(clippy["pedantic"], {"level": "warn", "priority": -1})
        for rule in (
            "undocumented_unsafe_blocks",
            "multiple_unsafe_ops_per_block",
            "allow_attributes",
            "allow_attributes_without_reason",
            "unwrap_used",
            "dbg_macro",
            "todo",
        ):
            self.assertEqual(clippy[rule], "deny", rule)

    def test_02_expect_used_and_blanket_as_conversions_stay_off(self) -> None:
        clippy = table(self.lints()["clippy"])
        self.assertNotIn("expect_used", clippy)
        self.assertNotIn("as_conversions", clippy)

    def test_03_the_rustc_lints_are_denied(self) -> None:
        rust = table(self.lints()["rust"])
        for lint in (
            "unsafe_op_in_unsafe_fn",
            "linker_messages",
            "missing_docs",
            "unreachable_pub",
            "unused_qualifications",
        ):
            self.assertEqual(rust[lint], "deny", lint)

    def test_04_unwraps_are_allowed_in_tests_by_configuration_not_by_suppression(self) -> None:
        self.assertEqual(manifest("clippy.toml"), {"allow-unwrap-in-tests": True})

    def test_05_every_member_inherits_the_workspace_lints(self) -> None:
        members = table(manifest("Cargo.toml")["workspace"])["members"]
        assert isinstance(members, list)
        self.assertTrue(members)
        for member in members:
            self.assertEqual(manifest(f"{member}/Cargo.toml")["lints"], {"workspace": True}, member)


# CK-105: the resource component's boundary (docs/scene-design.md §3), read from the members' manifests.
RESOURCES = "ck-resources"
FEATURE = "test-support"
# The crates that must never depend on the resource component: the GPU and window crates.
NOT_ON_RESOURCES = (
    "ck-gpu-metal",
    "ck-gpu-vulkan",
    "ck-window",
    "ck-window-cocoa",
    "ck-window-glfw",
)
DEPENDENCY_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")


def dependency_tables(document: dict[str, object]) -> list[tuple[str, dict[str, object]]]:
    """Every dependency table of a manifest, including the target-specific ones, with its kind."""
    found = [(kind, table(document[kind])) for kind in DEPENDENCY_TABLES if kind in document]
    for target in table(document.get("target", {})).values():
        found += [
            (kind, table(table(target)[kind]))
            for kind in DEPENDENCY_TABLES
            if kind in table(target)
        ]
    return found


def dependencies(
    document: dict[str, object], kinds: tuple[str, ...] = DEPENDENCY_TABLES
) -> dict[str, dict[str, object]]:
    """The package names a manifest depends on (a renamed dependency by its real package), each with the
    spec's table (a bare version string is an empty one)."""
    found: dict[str, dict[str, object]] = {}
    for kind, entries in dependency_tables(document):
        if kind not in kinds:
            continue
        for key, spec in entries.items():
            spec_table = spec if isinstance(spec, dict) else {}
            name = spec_table.get("package", key)
            assert isinstance(name, str)
            found[name] = spec_table
    return found


def features_reaching(features: dict[str, object], start: str) -> set[str]:
    """The feature names (and `dep/feature` entries) a feature switches on, itself included."""
    seen: set[str] = set()
    todo = [start]
    while todo:
        name = todo.pop()
        if name in seen:
            continue
        seen.add(name)
        entry = features.get(name, [])
        assert isinstance(entry, list)
        todo += [item for item in entry if isinstance(item, str)]
    return seen


def enabling_problems(name: str, document: dict[str, object]) -> list[str]:
    """Where a manifest switches `test-support` on other than from a `[dev-dependencies]` table or by default."""
    problems = []
    for kind, entries in dependency_tables(document):
        for key, spec in entries.items():
            spec_table = spec if isinstance(spec, dict) else {}
            features = spec_table.get("features", [])
            if (
                spec_table.get("package", key) == RESOURCES
                and isinstance(features, list)
                and FEATURE in features
                and kind != "dev-dependencies"
            ):
                problems.append(
                    f"{name}: [{kind}] enables {RESOURCES}'s {FEATURE}; only [dev-dependencies] may"
                )
    for feature, items in table(document.get("features", {})).items():
        assert isinstance(items, list)
        problems += [
            f"{name}: [features] {feature} forwards to {RESOURCES}'s {FEATURE}"
            for item in items
            if isinstance(item, str) and item.replace("?", "").endswith(f"{RESOURCES}/{FEATURE}")
        ]
    return problems


def resources_boundary_problems(manifests: dict[str, dict[str, object]]) -> list[str]:
    """The ways a set of member manifests (by `[package] name`) breaks the resource component's boundary:
    its dependencies, who may depend on it and how `test-support` is switched on."""
    problems = []
    for name, document in sorted(manifests.items()):
        if name == RESOURCES:
            problems += [
                f"{name}: depends on {dependency}, but it depends on no ck-* crate"
                for dependency in dependencies(document)
                if dependency.startswith("ck-")
            ]
            features = table(document.get("features", {}))
            if any(
                item.split("?")[0].endswith(FEATURE)
                for item in features_reaching(features, "default")
            ):
                problems.append(f"{name}: {FEATURE} is switched on by its default features")
        if name == "ck-render-2d" and "png" in dependencies(document):
            problems.append(f"{name}: depends on png, which ck-resources owns")
        if name in NOT_ON_RESOURCES and RESOURCES in dependencies(document):
            problems.append(f"{name}: depends on {RESOURCES}, which GPU and window crates must not")
        problems += enabling_problems(name, document)
    return problems


def member_manifests() -> dict[str, dict[str, object]]:
    members = table(manifest("Cargo.toml")["workspace"])["members"]
    assert isinstance(members, list)
    found = {}
    for member in members:
        document = manifest(f"{member}/Cargo.toml")
        found[str(table(document["package"])["name"])] = document
    return found


class ResourcesBoundary(unittest.TestCase):
    """CK-105: the resource component stays CPU-only and below the renderer, and its test-only constructors
    reach only test builds."""

    def test_01_the_real_workspace_keeps_the_boundary(self) -> None:
        manifests = member_manifests()
        self.assertIn(RESOURCES, manifests)
        self.assertEqual(resources_boundary_problems(manifests), [])
        # The scan is not vacuous: the renderer does depend on the component, through [dev-dependencies] with the feature.
        renderer = manifests["ck-render-2d"]
        self.assertIn(RESOURCES, dependencies(renderer, ("dependencies",)))
        enabled = dependencies(renderer, ("dev-dependencies",))[RESOURCES]
        self.assertEqual(enabled.get("features"), [FEATURE])
        self.assertEqual(manifests[RESOURCES]["features"], {"default": [], FEATURE: []})
        self.assertEqual(list(dependencies(manifests[RESOURCES])), ["png"])

    def test_02_synthetic_manifests_that_break_a_rule_are_refused_naming_the_manifest_and_the_rule(
        self,
    ) -> None:
        def good() -> dict[str, dict[str, object]]:
            return {
                RESOURCES: {
                    "features": {"default": [], FEATURE: []},
                    "dependencies": {"png": "0.18"},
                },
                "ck-render-2d": {
                    "dependencies": {RESOURCES: {"path": "../resources"}},
                    "dev-dependencies": {
                        RESOURCES: {"path": "../resources", "features": [FEATURE]}
                    },
                    "features": {"metal": ["dep:ck-gpu-metal"]},
                },
                "ck-gpu-metal": {"dependencies": {"ck-log": {"path": "../log"}}},
            }

        self.assertEqual(resources_boundary_problems(good()), [])

        def broken(change: Callable[[dict[str, dict[str, object]]], None]) -> list[str]:
            manifests = good()
            change(manifests)
            return resources_boundary_problems(manifests)

        def enable_in(kind: str) -> Callable[[dict[str, dict[str, object]]], None]:
            def change(manifests: dict[str, dict[str, object]]) -> None:
                manifests["codekarl"] = {kind: {RESOURCES: {"path": "x", "features": [FEATURE]}}}

            return change

        def in_default(manifests: dict[str, dict[str, object]]) -> None:
            manifests[RESOURCES]["features"] = {"default": [FEATURE], FEATURE: []}

        def through_a_feature(manifests: dict[str, dict[str, object]]) -> None:
            manifests[RESOURCES]["features"] = {"default": ["more"], "more": [FEATURE], FEATURE: []}

        def forwarded(manifests: dict[str, dict[str, object]]) -> None:
            manifests["codekarl"] = {"features": {"metal": [f"{RESOURCES}/{FEATURE}"]}}

        def forwarded_weakly(manifests: dict[str, dict[str, object]]) -> None:
            manifests["codekarl"] = {"features": {"metal": [f"{RESOURCES}?/{FEATURE}"]}}

        def gpu_depends(manifests: dict[str, dict[str, object]]) -> None:
            manifests["ck-gpu-metal"]["dependencies"] = {RESOURCES: {"path": "../resources"}}

        def window_depends_renamed(manifests: dict[str, dict[str, object]]) -> None:
            manifests["ck-window"] = {
                "dependencies": {"images": {"package": RESOURCES, "path": "x"}}
            }

        def resources_depends_on_a_crate(manifests: dict[str, dict[str, object]]) -> None:
            manifests[RESOURCES]["dependencies"] = {"png": "0.18", "ck-log": {"path": "../log"}}

        def renderer_keeps_png(manifests: dict[str, dict[str, object]]) -> None:
            manifests["ck-render-2d"]["dependencies"] = {"png": "0.18"}

        cases: list[tuple[str, Callable[[dict[str, dict[str, object]]], None], str, str]] = [
            (
                "[dependencies] enable",
                enable_in("dependencies"),
                "codekarl",
                "[dependencies] enables",
            ),
            (
                "[build-dependencies] enable",
                enable_in("build-dependencies"),
                "codekarl",
                "[build-dependencies] enables",
            ),
            ("default", in_default, RESOURCES, "default features"),
            ("default through a feature", through_a_feature, RESOURCES, "default features"),
            ("forwarded", forwarded, "codekarl", "forwards to"),
            ("forwarded weakly", forwarded_weakly, "codekarl", "forwards to"),
            ("a GPU crate depends", gpu_depends, "ck-gpu-metal", "GPU and window crates must not"),
            (
                "a window crate depends, renamed",
                window_depends_renamed,
                "ck-window",
                "GPU and window crates must not",
            ),
            (
                "it depends on a ck-* crate",
                resources_depends_on_a_crate,
                RESOURCES,
                "depends on no ck-* crate",
            ),
            ("the renderer keeps png", renderer_keeps_png, "ck-render-2d", "depends on png"),
        ]
        for label, change, manifest_name, rule in cases:
            problems = broken(change)
            self.assertTrue(
                any(
                    problem.startswith(f"{manifest_name}:") and rule in problem
                    for problem in problems
                ),
                f"{label}: {problems}",
            )


class Suppressions(unittest.TestCase):
    def test_01_the_rust_sources_were_found(self) -> None:
        self.assertGreaterEqual(len(RUST_SOURCES), 3)

    def test_02_no_lint_is_allowed_only_expected(self) -> None:
        pattern = re.compile(r"#!?\[\s*allow\s*\(|#!?\[\s*cfg_attr\([^\]]*\ballow\b")
        found = [
            f"{path.relative_to(REPO)}:{number}"
            for path in RUST_SOURCES
            for number, line in enumerate(path.read_text().splitlines(), start=1)
            if pattern.search(line)
        ]
        self.assertEqual(found, [], 'use #[expect(lint, reason = "...")], never allow')

    def test_03_every_expect_carries_a_reason(self) -> None:
        found: list[str] = []
        for path in RUST_SOURCES:
            text = path.read_text()
            for match in re.finditer(r"#!?\[\s*expect\s*\(([^\]]*)\)\s*\]", text, flags=re.DOTALL):
                if "reason" not in match.group(1):
                    found.append(f"{path.relative_to(REPO)}: {match.group(0)}")
        self.assertEqual(found, [])

    def test_04_every_unsafe_block_has_a_safety_comment(self) -> None:
        found: list[str] = []
        for path in RUST_SOURCES:
            lines = path.read_text().splitlines()
            for number, line in enumerate(lines):
                if re.search(r"\bunsafe\s*\{", line) and not line.lstrip().startswith("//"):
                    above = [entry.strip() for entry in lines[max(0, number - 4) : number]]
                    if not any(entry.startswith("// SAFETY:") for entry in above):
                        found.append(f"{path.relative_to(REPO)}:{number + 1}")
        self.assertEqual(found, [])


if __name__ == "__main__":
    unittest.main()
