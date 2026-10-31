"""No test reads the repository's own docs/: they are ordinary documentation, published by `ck docs`.

A static scan: any path built from the test file's location (`__file__`) or the repository root
(a name assigned from `__file__`, or REPO/ROOT) that has a `docs` component is a read of the real
documentation, and so is a whole-repository `copytree` that does not exclude `docs`. Fixture hubs and
temporary copies (`self.hub / "docs"`) are allowed.
"""

import ast
import unittest
from collections.abc import Callable
from pathlib import Path

TESTS = Path(__file__).resolve().parent
REPOSITORY = TESTS.parent
ROOT_NAMES = {"REPO", "ROOT", "REPO_ROOT", "REPOSITORY"}


def mentions_file(node: ast.AST) -> bool:
    return any(isinstance(item, ast.Name) and item.id == "__file__" for item in ast.walk(node))


def has_docs_component(node: ast.AST) -> bool:
    for item in ast.walk(node):
        if (
            isinstance(item, ast.Constant)
            and isinstance(item.value, str)
            and "docs" in item.value.replace("\\", "/").split("/")
        ):
            return True
    return False


def violations(source: str, filename: str = "<test>") -> list[tuple[int, str]]:
    """(line, text) of every path expression rooted at the repository that names a docs component."""
    tree = ast.parse(source, filename)
    roots = set(ROOT_NAMES)
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign) and mentions_file(node.value):
            roots.update(t.id for t in node.targets if isinstance(t, ast.Name))

    def rooted(node: ast.AST) -> bool:
        if mentions_file(node):
            return True
        for item in ast.walk(node):
            if isinstance(item, ast.Name) and item.id in roots:
                return True
            if isinstance(item, ast.Attribute) and item.attr in ROOT_NAMES:
                return True
        return False

    found: dict[int, str] = {}
    for node in ast.walk(tree):
        if not isinstance(node, ast.BinOp | ast.Call):
            continue  # only these can build a path or copy a tree
        builds_path = (isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div)) or (
            isinstance(node, ast.Call)
            and (
                (isinstance(node.func, ast.Attribute) and node.func.attr in ("joinpath", "join"))
                or (isinstance(node.func, ast.Name) and node.func.id in ("Path", "open"))
            )
        )
        if builds_path and rooted(node) and has_docs_component(node):
            found.setdefault(node.lineno, ast.unparse(node)[:120])
        if (
            isinstance(node, ast.Call)
            and copies_whole_repository(node, rooted)
            and not ignores_docs(node)
        ):
            found.setdefault(node.lineno, ast.unparse(node)[:120])
    return sorted(found.items())


def copies_whole_repository(node: ast.AST, rooted: Callable[[ast.AST], bool]) -> bool:
    """A copytree whose source is the repository root itself (not a path below it)."""
    if not isinstance(node, ast.Call) or not node.args:
        return False
    name = getattr(node.func, "attr", getattr(node.func, "id", ""))
    if name not in ("copytree", "copy_tree"):
        return False
    source = node.args[0]
    below_root = any(
        isinstance(item, ast.BinOp) and isinstance(item.op, ast.Div) for item in ast.walk(source)
    )
    return rooted(source) and not below_root


def ignores_docs(call: ast.Call) -> bool:
    """The copy passes shutil.ignore_patterns(...) that names docs."""
    for keyword in call.keywords:
        if (
            keyword.arg == "ignore"
            and isinstance(keyword.value, ast.Call)
            and any(
                isinstance(item, ast.Constant) and item.value == "docs"
                for item in ast.walk(keyword.value)
            )
        ):
            return True
    return False


class NoRepositoryDocsReads(unittest.TestCase):
    def scanned(self) -> list[Path]:
        paths = sorted(TESTS.rglob("*.py")) + sorted((REPOSITORY / "tools/chat").glob("test_*.py"))
        self.assertTrue(paths)
        return paths

    def test_01_no_test_module_reads_the_repository_docs(self) -> None:
        offences: dict[str, list[tuple[int, str]]] = {}
        for path in self.scanned():
            found = violations(path.read_text(), str(path))
            if found:
                offences[path.relative_to(REPOSITORY).as_posix()] = found
        self.assertEqual(offences, {}, "tests must not read the repository's own docs/")

    def test_02_the_scan_flags_a_deliberate_read_and_allows_fixture_hubs(self) -> None:
        deliberate = (
            "from pathlib import Path\n"
            "def f():\n"
            "    return (Path(__file__).resolve().parents[1] / 'docs/review-gate-plan.md').read_bytes()\n"
        )
        self.assertEqual([line for line, _ in violations(deliberate)], [3])
        via_name = (
            "from pathlib import Path\n"
            "REPO = Path(__file__).resolve().parents[1]\n"
            "def f():\n"
            "    return open(REPO / 'docs' / 'x.md', 'rb')\n"
        )
        self.assertTrue(violations(via_name))
        joined = (
            "import os\n"
            "def f(here=__file__):\n"
            "    return os.path.join(os.path.dirname(__file__), '..', 'docs', 'x.md')\n"
        )
        self.assertTrue(violations(joined))
        fixture = (
            "def f(self):\n"
            "    (self.hub / 'docs').mkdir()\n"
            "    (self.hub / 'docs/review-gate-plan.md').write_text('x')\n"
        )
        self.assertEqual(violations(fixture), [])
        whole = (
            "import shutil\n"
            "from pathlib import Path\n"
            "REPO = Path(__file__).resolve().parents[1]\n"
            "def f(dest):\n"
            "    shutil.copytree(REPO, dest, ignore=shutil.ignore_patterns('target'))\n"
        )
        self.assertEqual([line for line, _ in violations(whole)], [5])
        bare = "import shutil\ndef f(dest):\n    shutil.copytree(__file__, dest)\n"
        self.assertTrue(violations(bare))
        excluded = whole.replace("'target'", "'target', 'docs'")
        self.assertEqual(violations(excluded), [])
        below = (
            "import shutil\n"
            "from pathlib import Path\n"
            "REPO = Path(__file__).resolve().parents[1]\n"
            "def f(dest, name):\n"
            "    shutil.copytree(REPO / name, dest)\n"
        )
        self.assertEqual(violations(below), [])
        spec = "from pathlib import Path\nSPEC = Path(__file__).resolve().parent / 'fixtures/bootstrap/spec.md'\n"
        self.assertEqual(violations(spec), [])


if __name__ == "__main__":
    unittest.main()
