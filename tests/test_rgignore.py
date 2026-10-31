"""CK-86: archived and guide records are hidden from agents' search, and Pijul's ignore file is untouched.

`.rgignore` is ripgrep's own ignore file. Pijul reads `.ignore` and not `.rgignore`, so the rules that
hide `docs/archive/` and the guide's reports live in the second, and `.ignore` must name no path under
`docs/` (Pijul would stop tracking it, and `pijul add --recursive` would skip new files there).
"""

import os
import subprocess
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import check

REPO = Path(__file__).resolve().parents[1]

# The rules, exactly (comment lines aside).
RULES = ["/docs/archive/", "/docs/guide/*", "!/docs/guide/CURSOR.md"]
# AGENTS.md's rule, as one paragraph (the file wraps it; whitespace is compared normalised).
AGENTS_RULE = (
    "`docs/archive/` is history: closed records, finished plans and review verdicts. Read it only when "
    "asked or when tracing why something was decided, and never treat it as current behavior. "
    "`.rgignore` hides it and the guide's reports from search (`rg --no-ignore` reaches them). Move a "
    "document there with `tools/ck docs` once its work is accepted. Never list docs paths in `.ignore`: "
    "Pijul would stop tracking them."
)


def rules_of(name: str) -> list[str]:
    """The non-comment, non-blank lines of an ignore file in the repository root."""
    lines = (REPO / name).read_text().splitlines()
    return [line.strip() for line in lines if line.strip() and not line.lstrip().startswith("#")]


class Rules(unittest.TestCase):
    def test_01_rgignore_holds_the_archive_and_guide_rules_and_nothing_else(self) -> None:
        self.assertEqual(rules_of(".rgignore"), RULES)

    def test_02_ignore_is_pijuls_and_excludes_no_path_under_docs(self) -> None:
        rules = rules_of(".ignore")
        self.assertTrue(rules, "the ignore file has its rules")
        for rule in rules:
            name = rule.lstrip("!").lstrip("/")
            self.assertFalse(
                name.startswith("docs") or name.endswith("docs/") or "/docs" in rule,
                f".ignore excludes a docs path: {rule}",
            )
        self.assertNotIn(".rgignore", rules, "Pijul must keep tracking .rgignore")

    def test_02b_applied_as_an_ignore_file_it_hides_no_file_of_docs(self) -> None:
        """Not only no literal spelling: the patterns themselves, applied by ripgrep as a gitignore-syntax
        file to the real `docs/` tree, hide nothing from it."""
        every = Search.files("--no-ignore", "docs")
        self.assertGreater(len(every), 10)
        self.assertEqual(Search.files("--no-ignore", "--ignore-file", ".ignore", "docs"), every)

    def test_03_agents_md_carries_the_rule_beside_the_documentation_paragraph(self) -> None:
        text = " ".join((REPO / "AGENTS.md").read_text().split())
        self.assertIn(AGENTS_RULE, text)
        docs = text.index("Ordinary documentation is ungated.")
        rule = text.index("`docs/archive/` is history")
        self.assertGreater(rule, docs)
        self.assertLess(rule - docs, 1500, "the rule follows the ordinary-documentation paragraph")


class Search(unittest.TestCase):
    """The real ripgrep (the doctor's lookup, a required tool), run in the repository with no user
    configuration or global ignore file in play (the repository's own `.rgignore` is the parent of `docs`)."""

    @staticmethod
    def files(*arguments: str) -> list[str]:
        rg = (
            check.ripgrep()
        )  # the doctor's own lookup: it raises, naming tools/bootstrap, and nothing skips
        env = {name: value for name, value in os.environ.items() if not name.startswith("RIPGREP")}
        result = subprocess.run(
            [rg, "--no-config", "--no-ignore-global", "--files", *arguments],
            cwd=REPO,
            env=env,
            capture_output=True,
            text=True,
            check=False,
            timeout=60,
        )
        if result.returncode not in (0, 1):
            raise AssertionError(result.stderr)
        return sorted(result.stdout.splitlines())

    def listing(self, *arguments: str) -> list[str]:
        return self.files(*arguments)

    def test_01_a_search_of_docs_lists_nothing_archived_and_only_the_guide_cursor(self) -> None:
        shown = self.listing("docs")
        self.assertEqual([name for name in shown if name.startswith("docs/archive/")], [])
        self.assertEqual(
            [name for name in shown if name.startswith("docs/guide/")], ["docs/guide/CURSOR.md"]
        )
        self.assertIn("docs/vision.md", shown, "the current documents are still listed")

    def test_02_the_hidden_files_exist_and_a_no_ignore_search_reaches_them(self) -> None:
        reached = self.listing("--no-ignore", "docs")
        self.assertTrue([name for name in reached if name.startswith("docs/archive/")])
        self.assertGreater(len([name for name in reached if name.startswith("docs/guide/")]), 1)


class Lookup(unittest.TestCase):
    """ripgrep is a required tool: the doctor's lookup finds it on PATH or in Homebrew's directories and
    otherwise fails naming the installation command; `tools/bootstrap` installs it."""

    def test_01_the_lookup_prefers_path_then_homebrew(self) -> None:
        with mock.patch("shutil.which", return_value="/somewhere/rg"):
            self.assertEqual(check.ripgrep(), "/somewhere/rg")
        with (
            mock.patch("shutil.which", return_value=None),
            mock.patch(
                "os.access",
                side_effect=lambda place, _mode: place == check.RIPGREP_PLACES[1],
            ),
        ):
            self.assertEqual(check.ripgrep(), check.RIPGREP_PLACES[1])

    def test_02_a_missing_ripgrep_is_an_error_naming_the_bootstrap_not_a_skip(self) -> None:
        with (
            mock.patch("shutil.which", return_value=None),
            mock.patch("os.access", return_value=False),
            self.assertRaises(RuntimeError) as raised,
        ):
            check.ripgrep()
        self.assertIn("tools/bootstrap", str(raised.exception))
        self.assertIn("ripgrep", str(raised.exception))

    def test_03_the_doctor_fails_without_ripgrep_naming_the_bootstrap(self) -> None:
        with (
            mock.patch("shutil.which", return_value=None),
            mock.patch("os.access", return_value=False),
            self.assertRaises(RuntimeError) as raised,
        ):
            check.doctor(graphics=False)
        self.assertIn("tools/bootstrap", str(raised.exception))

    def test_04_the_bootstrap_installs_ripgrep_with_homebrew_when_missing(self) -> None:
        text = (REPO / "tools/bootstrap").read_text()
        self.assertIn("if ! brew list --versions ripgrep >/dev/null 2>&1; then", text)
        self.assertIn("    brew install ripgrep\n", text)


if __name__ == "__main__":
    unittest.main()
