"""S6: the Ruff configuration and the suppressions policy.

Bare `ruff check` discovers the entry points and skips historical clones and installed packages;
every `# noqa` is coded and explained. CK-151: no tracked file holds the current user's home path.
"""

import os
import re
import subprocess
import tempfile
import tokenize
import tomllib
import unittest
from collections.abc import Iterable, Iterator
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
NEW_GROUPS = ["W", "B", "UP", "SIM", "RUF", "PTH", "PL", "C4", "PIE", "RET", "PGH", "ANN"]


VISIBLE = ["tools/ck", "tools/ck-chat", "tools/visible.py", "tests/visible_test.py"]
SENTINELS = [".pipeline/hidden.py", ".tools/hidden.py", "target/hidden.py"]
UNFORMATTED = (
    "import os\nx=1\n"  # a lint error and not formatted: only an excluded file may hold it
)


def table(value: object) -> dict[str, object]:
    """A TOML table, checked rather than assumed."""
    assert isinstance(value, dict)
    return value


def names(value: object) -> list[str]:
    """A TOML array of strings, checked rather than assumed."""
    assert isinstance(value, list)
    assert all(isinstance(item, str) for item in value)
    return value


def make_layout(base: Path, config: str) -> None:
    for name in VISIBLE:
        path = base / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("VALUE: int = 1\n")
    for name in SENTINELS:
        path = base / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(UNFORMATTED)
    (base / "pyproject.toml").write_text(config)


def ruff_files(base: Path) -> set[str]:
    output = subprocess.run(
        [ruff(), "check", "--show-files"], cwd=base, capture_output=True, text=True, check=True
    ).stdout
    return {Path(line).resolve().relative_to(base).as_posix() for line in output.splitlines()}


def ruff() -> str:
    hub = Path(os.environ.get("CK_HUB") or (REPO / ".ck-hub").read_text().strip())
    return str(hub / ".tools/python/bin/ruff")


def comments(path: Path) -> Iterator[tuple[int, str]]:
    with path.open("rb") as stream:
        for token in tokenize.tokenize(stream.readline):
            if token.type == tokenize.COMMENT:
                yield token.start[0], token.string


class Configuration(unittest.TestCase):
    def setUp(self) -> None:
        self.config = table(
            table(tomllib.loads((REPO / "pyproject.toml").read_text())["tool"])["ruff"]
        )

    def test_01_rules_discovery_and_the_historical_fixture(self) -> None:
        lint = table(self.config["lint"])
        selected = names(lint["select"])
        for group in ["E4", "E7", "E9", "F", "I", *NEW_GROUPS]:
            self.assertIn(group, selected)
        self.assertNotIn("S", selected)  # subprocess orchestration is the project's purpose
        self.assertTrue({"tools/ck", "tools/ck-chat"} <= set(names(self.config["extend-include"])))
        ignores = table(lint["per-file-ignores"])
        # Only the new groups are off for the byte-pinned fixture; the existing rules still run.
        self.assertEqual(sorted(names(ignores["tests/fixtures/legacy/**"])), sorted(NEW_GROUPS))
        # The S7 migration is complete: the fixture is the only per-file ignore left.
        self.assertEqual(set(ignores), {"tests/fixtures/legacy/**"})

    def test_02_bare_ruff_discovers_the_entry_points_and_skips_clones(self) -> None:
        """Behaviour on a disposable layout with real sentinel files, at a root and inside a nested
        .pipeline/workers/<name>/ copy (where the copy itself must still be linted)."""
        config = (REPO / "pyproject.toml").read_text()
        with tempfile.TemporaryDirectory() as directory:
            for location in ("root", ".pipeline/workers/example"):
                with self.subTest(location):
                    base = Path(directory).resolve() / location.replace("/", "_") / location
                    make_layout(base, config)
                    listed = ruff_files(base)
                    for name in VISIBLE:
                        self.assertIn(name, listed)
                    for name in SENTINELS:
                        self.assertNotIn(name, listed)
                    # The sentinels are unformatted and have lint errors: bare commands still pass.
                    for command in (["check"], ["format", "--check"]):
                        result = subprocess.run(
                            [ruff(), *command],
                            cwd=base,
                            capture_output=True,
                            text=True,
                            check=False,
                        )
                        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_02b_the_layout_check_detects_a_broken_exclusion(self) -> None:
        """A leading slash is an absolute filesystem path in Ruff and excludes nothing: the layout
        check must see the sentinels then (a control that the test is not vacuous)."""
        broken = (REPO / "pyproject.toml").read_text()
        broken = broken.replace(
            '"./.pipeline", "./.tools", "./target"', '"/.pipeline", "/.tools", "/target"'
        )
        self.assertIn('"/.pipeline"', broken)
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory).resolve() / "root"
            make_layout(base, broken)
            listed = ruff_files(base)
            self.assertTrue(set(SENTINELS) <= listed)
            result = subprocess.run(
                [ruff(), "check"], cwd=base, capture_output=True, text=True, check=False
            )
            self.assertNotEqual(result.returncode, 0)

    def test_03_every_noqa_is_coded_and_explained(self) -> None:
        paths = [*sorted((REPO / "tools").rglob("*.py")), *sorted((REPO / "tests").rglob("*.py"))]
        paths += [REPO / "tools/ck", REPO / "tools/ck-chat"]
        self.assertGreater(len(paths), 20)
        offenders = [entry for path in paths for entry in noqa_offenders(path)]
        self.assertEqual(offenders, [])

    def test_04_the_scan_rejects_bare_and_unexplained_noqa(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            sample = Path(directory) / "sample.py"
            sample.write_text(
                "import os  # noqa\n"
                "import sys  # noqa: F401\n"
                "import re  # noqa: F401  # imported for its side effect\n"
                "import json  # noqa: F401,E402  # two codes, one reason\n"
                "x = '# noqa inside a string is not a directive'\n"
            )
            lines = [line for _, line, _ in noqa_offenders(sample)]
        self.assertEqual(lines, [1, 2])


class HomePath(unittest.TestCase):
    """CK-151: tracked files may be mirrored publicly, so none holds the current user's home path."""

    def test_01_no_tracked_file_holds_the_home_path(self) -> None:
        listing = subprocess.run(
            ["pijul", "list"], cwd=REPO, capture_output=True, text=True, check=True
        ).stdout
        self.assertEqual(
            home_path_offenders(REPO, listing.splitlines(), str(Path.home())),
            [],
            "tracked files hold the home path; write it as ~ or relative to the hub",
        )

    def test_02_the_guard_names_each_offending_tracked_file(self) -> None:
        home = str(Path.home())
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            files = {
                "docs/leak.md": f"see {home}/work/codekarl/x\n",
                "name with spaces.txt": f"{home}\n",
                "clean.md": "see ~/work/codekarl/x\n",
                "lookalike.md": f"{home}-other and {home}x/y\n",
            }
            for name, text in files.items():
                (base / name).parent.mkdir(parents=True, exist_ok=True)
                (base / name).write_text(text)
            (base / "binary.bin").write_bytes(b"\x00\xff" + home.encode() + b"/x\x00")
            (base / "link").symlink_to(f"{home}/target")
            (base / "untracked.md").write_text(f"{home}\n")
            tracked = [*files, "binary.bin", "link", "docs", "missing.md"]
            self.assertEqual(
                home_path_offenders(base, tracked, home),
                ["binary.bin", "docs/leak.md", "link", "name with spaces.txt"],
            )

    def test_03_a_home_that_would_match_everything_is_refused(self) -> None:
        for home in ("", "/", "relative"):
            with self.assertRaises(ValueError):
                home_path_offenders(REPO, [], home)


def home_path_offenders(root: Path, tracked: Iterable[str], home: str) -> list[str]:
    """The tracked names (literal, so spaces are fine) whose bytes hold `home` as a path prefix.

    A symbolic link is judged by its target text; directories and missing names are skipped.
    """
    prefix = home.rstrip("/")
    if not prefix.startswith("/"):
        raise ValueError(f"not a home path: {home!r}")
    pattern = re.compile(re.escape(prefix.encode()) + rb"(?![\w.-])")
    found: list[str] = []
    for name in sorted(set(tracked)):
        path = root / name
        if path.is_symlink():
            data = str(path.readlink()).encode()
        elif path.is_file():
            data = path.read_bytes()
        else:
            continue
        if pattern.search(data):
            found.append(name)
    return found


def noqa_offenders(path: Path) -> list[tuple[Path, int, str]]:
    """(path, line, comment) for every comment that mentions noqa without a code and a reason."""
    found: list[tuple[Path, int, str]] = []
    for line, text in comments(path):
        if "noqa" not in text:
            continue
        head, _, reason = text.lstrip("# ").partition("  # ")
        codes = head[len("noqa: ") :].split(",") if head.startswith("noqa: ") else []
        coded = bool(codes) and all(
            code.strip()[:1].isalpha() and code.strip()[-1:].isdigit() for code in codes
        )
        if not (coded and reason.strip()):
            found.append((path, line, text))
    return found


if __name__ == "__main__":
    unittest.main()
