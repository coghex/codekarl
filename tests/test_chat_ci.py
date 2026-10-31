"""RC-1: the shared chat inputs are one defined set, and batch CI runs the staged chat suite."""

import contextlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import types
import unittest
from collections.abc import Callable, Mapping
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import chatfix
import check
from chatfix import FAILING, PASSING, REPO, expected_aggregate, expected_inputs, record_for

MARKER = (
    "import os\nimport unittest\nfrom pathlib import Path\n\n\nclass T(unittest.TestCase):\n"
    "    def test_ran(self):\n"
    "        Path(os.environ['TMPDIR'], 'ran-marker').write_text('ran')\n"
)


IDENTITY = {
    "CHAT_AS": "ck-manager",
    "CHAT_AGENT_ID": "ck-manager",
    "CHAT_ROLE": "manager",
    "CHAT_PROJECT": "codekarl",
    "CMUX_SURFACE_ID": "fixture",
    "CMUX_WORKSPACE_ID": "fixture",
}
ENV_PROBE = """import os
import unittest


class T(unittest.TestCase):
    def test_no_caller_identity(self) -> None:
        names = {"CHAT_AS", "CHAT_AGENT_ID", "CHAT_ROLE", "CHAT_PROJECT"}
        seen = [n for n in os.environ if n in names or n.startswith("CMUX_")]
        self.assertEqual(seen, [])
        self.assertTrue(os.environ.get("CK_CHAT_SOURCE_ROOT"))
"""


class AdapterWorkerRole(unittest.TestCase):
    """CK-76: the identities edits accept a shared skill that already lists `worker` (as it does since the
    designer role was added), insert it into the old form, and still refuse any other shape."""

    OLD = "ROLES = ('manager', 'solver', 'reviewer', 'guide', 'assistant', 'owner')\nNUMBERED = {'solver', 'reviewer'}\n"
    NEW = (
        "ROLES = ('manager', 'solver', 'worker', 'reviewer', 'designer', 'guide', 'assistant', 'owner')\n"
        "NUMBERED = {'solver', 'worker', 'reviewer', 'designer'}\n"
    )

    def setUp(self) -> None:
        self.adapter = chatfix.load(REPO / "tools/chat/adapter.py", "ck_adapter_worker_role")

    def roles_lines(self, source: str) -> str:
        """Run identities() with every edit other than the two worker insertions treated as already done."""
        real = self.adapter.replace
        worker = ("ROLES = (", "NUMBERED = {")

        def only_worker(text: str, old: str, new: str) -> str:
            return real(text, old, new) if old.startswith(worker) else text

        with mock.patch.object(self.adapter, "replace", only_worker):
            result: str = self.adapter.identities(source)
        return result

    def test_the_old_form_gets_worker_inserted(self) -> None:
        self.assertEqual(
            self.roles_lines(self.OLD),
            "ROLES = ('manager', 'solver', 'worker', 'reviewer', 'guide', 'assistant', 'owner')\n"
            "NUMBERED = {'solver', 'worker', 'reviewer'}\n",
        )

    def test_a_shared_skill_that_already_lists_worker_is_left_as_it_is(self) -> None:
        self.assertTrue(self.adapter.has_worker_role(self.NEW))
        self.assertEqual(self.roles_lines(self.NEW), self.NEW)

    def test_any_other_shape_still_fails(self) -> None:
        other = "ROLES = ('manager', 'solver', 'reviewer', 'designer')\nNUMBERED = {'solver', 'reviewer', 'designer'}\n"
        self.assertFalse(self.adapter.has_worker_role(other))
        with self.assertRaisesRegex(ValueError, "Shared chat changed"):
            self.roles_lines(other)

    def test_worker_in_a_comment_is_not_membership(self) -> None:
        commented = (
            "ROLES = ('manager', 'solver', 'reviewer', 'designer', 'guide', 'assistant', 'owner')  # formerly ('worker')\n"
            "NUMBERED = {'solver', 'reviewer', 'designer'}  # formerly {'worker'}\n"
        )
        self.assertFalse(self.adapter.has_worker_role(commented))
        with self.assertRaisesRegex(ValueError, "Shared chat changed"):
            self.roles_lines(commented)

    def test_a_computed_element_is_not_membership(self) -> None:
        computed = (
            "ROLES = ('manager', 'solver', 'reviewer', 'worker' + '-retired')\n"
            "NUMBERED = {'solver', 'reviewer', 'worker' + '-retired'}\n"
        )
        self.assertFalse(self.adapter.has_worker_role(computed))
        with self.assertRaisesRegex(ValueError, "Shared chat changed"):
            self.roles_lines(computed)

    def test_inserting_worker_twice_changes_nothing(self) -> None:
        once = self.roles_lines(self.OLD)
        self.assertEqual(self.roles_lines(once), once)

    def test_an_unrelated_missing_anchor_still_fails(self) -> None:
        stub = chatfix.adapter_stubs()["identities.py"]
        self.adapter.identities(stub)  # every edit already present: accepted
        # The worker-role edit for spawned sessions, removed whole: neither its old nor its new text is left.
        edit = "    return 'worker' if project == 'codekarl' else 'solver'\n\n\ndef manager_surface"
        self.assertIn(edit, stub)
        with self.assertRaisesRegex(ValueError, "Shared chat changed"):
            self.adapter.identities(stub.replace(edit, ""))

    def test_worker_in_only_one_line_is_not_accepted(self) -> None:
        half = "ROLES = ('manager', 'solver', 'worker', 'reviewer')\nNUMBERED = {'solver', 'reviewer', 'designer'}\n"
        self.assertFalse(self.adapter.has_worker_role(half))
        with self.assertRaisesRegex(ValueError, "Shared chat changed"):
            self.roles_lines(half)


class ChatCiFixture(unittest.TestCase):
    def setUp(self) -> None:
        chatfix.guard(self)
        self.temp = tempfile.TemporaryDirectory(prefix="ck-chat-ci-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(os.path.realpath(self.temp.name))
        self.home = self.base / "home"
        self.tmp = self.base / "tmp"
        self.tmp.mkdir()
        self.shared = chatfix.build_shared(self.base / "shared")
        self.checkout = chatfix.build_checkout(self.home / "work/codekarl")
        self.record = self.base / "record.json"

    def rebuild(self, *, shared_test: str = PASSING, chat_test: str = PASSING) -> None:
        shutil.rmtree(self.shared, ignore_errors=True)
        shutil.rmtree(self.checkout)
        for leftover in ("outside", "pm-elsewhere"):
            shutil.rmtree(self.base / leftover, ignore_errors=True)
        self.shared = chatfix.build_shared(self.base / "shared")
        (self.shared / "chat/scripts/tests/test_shared.py").write_text(shared_test)
        self.checkout = chatfix.build_checkout(self.home / "work/codekarl", chat_test=chat_test)

    def env(self, **extra: str) -> dict[str, str]:
        # The caller's own identity must not leak into a fixture run: only `extra` sets it.
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(("CK_", "CMUX_", "CHAT_"))
        }
        env.update(HOME=str(self.home), TMPDIR=str(self.tmp), CK_CHAT_SHARED_ROOT=str(self.shared))
        env.update(extra)
        return env

    def ci(self, **extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(self.checkout / "tools/chat/install.py"), "ci"],
            check=False,
            env=self.env(**extra),
            cwd=self.checkout,
            capture_output=True,
            text=True,
        )

    def stage_command(self, **extra: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(self.checkout / "tools/chat/install.py"), "stage"],
            check=False,
            env=self.env(**extra),
            cwd=self.checkout,
            capture_output=True,
            text=True,
        )

    def staged(self) -> list[Path]:
        return sorted(self.tmp.glob("ck-chat-tested-*"))

    def install_module(self) -> types.ModuleType:
        """The fixture checkout's install.py, in-process, with its sibling modules importable."""
        self.enterContext(
            mock.patch.object(sys, "path", [str(self.checkout / "tools/chat"), *sys.path])
        )
        self.enterContext(mock.patch.object(tempfile, "tempdir", str(self.tmp)))
        self.enterContext(
            mock.patch.dict(
                os.environ, {"HOME": str(self.home), "CK_CHAT_SHARED_ROOT": str(self.shared)}
            )
        )
        module = chatfix.load(self.checkout / "tools/chat/install.py", "ck_install_fixture")
        quiet_run = chatfix.quiet_run(subprocess.run)

        self.enterContext(mock.patch.object(module.subprocess, "run", quiet_run))
        return module

    def inputs_module(self) -> types.ModuleType:
        return chatfix.load(REPO / "tools/chat/inputs.py", "ck_inputs_fixture")


class SharedInputs(ChatCiFixture):
    def test_01_inputs_cover_both_trees_and_aggregate_every_byte(self) -> None:
        pycache = self.shared / "chat/scripts/__pycache__"
        pycache.mkdir()
        (pycache / "x.pyc").write_bytes(b"cache")
        nested = self.shared / "project-manager/scripts/__pycache__"
        nested.mkdir()
        (nested / "y.pyc").write_bytes(b"cache")
        inputs = self.inputs_module()
        files = inputs.shared_inputs(self.shared)
        self.assertEqual(files, expected_inputs(self.shared))
        self.assertIn("chat/scripts/tests/test_shared.py", files)
        self.assertIn("project-manager/scripts/pm", files)
        self.assertFalse([name for name in files if "__pycache__" in name])
        self.assertEqual(inputs.aggregate(files), expected_aggregate(files))
        self.assertEqual(
            inputs.aggregate(dict(reversed(list(files.items())))), inputs.aggregate(files)
        )
        before = inputs.aggregate(files)
        (self.shared / "project-manager/scripts/pm").write_text("#!/usr/bin/env python3\n# edit\n")
        self.assertNotEqual(inputs.aggregate(inputs.shared_inputs(self.shared)), before)
        with mock.patch.dict(os.environ, {"CK_CHAT_SHARED_ROOT": str(self.shared)}):
            self.assertEqual(inputs.shared_root(), self.shared)
        with mock.patch.dict(os.environ, {"HOME": str(self.home)}):
            os.environ.pop("CK_CHAT_SHARED_ROOT", None)
            self.assertEqual(inputs.shared_root(), self.home / ".codex/skills")

    def test_13a_a_record_must_be_internally_consistent(self) -> None:
        inputs = self.inputs_module()
        files = expected_inputs(self.shared)
        self.assertTrue(inputs.valid_record(record_for(files)))
        digest = next(iter(files.values()))
        with_int_key: dict[object, str] = {}
        for key, value in files.items():
            with_int_key[key] = value
        with_int_key[3] = digest  # a non-text key
        for bad in (
            [],
            {"schema": 1},
            {**record_for(files), "schema": 2},
            {**record_for(files), "files": {}},
            {**record_for(files), "aggregate": "0" * 64},
            record_for({**files, "chat/x": "not-hex"}),
            record_for({**files, "chat/x": digest.upper()}),
            record_for({**files, "chat/x": digest[:-1]}),
            {"schema": 1, "files": with_int_key, "aggregate": "0" * 64},
            record_for({**files, "": digest}),
            *(
                record_for({**files, key: digest})
                for key in (
                    "/outside",
                    "/chat/scripts/pchat",
                    "../escape",
                    "chat/../../escape",
                    "chat/../chat/scripts/pchat",
                    "./chat/scripts/pchat",
                    "chat/./scripts/pchat",
                    "chat//scripts/pchat",
                    "chat/scripts/",
                    "..",
                    ".",
                )
            ),
        ):
            with self.subTest(bad=str(bad)[:60]):
                self.assertFalse(inputs.valid_record(bad))


class CiStep(ChatCiFixture):
    def test_02_ci_passes_prints_the_aggregate_records_it_and_removes_the_stage(self) -> None:
        result = self.ci(CK_CHAT_INPUTS=str(self.record))
        self.assertEqual(result.returncode, 0, result.stderr)
        files = expected_inputs(self.shared)
        self.assertIn(
            f"chat-inputs {expected_aggregate(files)} files={len(files)}",
            result.stdout.splitlines(),
        )
        self.assertEqual(json.loads(self.record.read_text()), record_for(files))
        self.assertEqual(self.staged(), [])

    def test_02a_ci_without_a_record_path_still_runs(self) -> None:
        result = self.ci()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.record.exists())
        self.assertEqual(self.staged(), [])

    def test_03_a_failing_chat_regression_fails_the_step_and_keeps_the_stage(self) -> None:
        self.rebuild(chat_test=FAILING)
        result = self.ci(CK_CHAT_INPUTS=str(self.record))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Staged integration tests failed", result.stderr)
        self.assertEqual(len(self.staged()), 1)
        self.assertFalse(self.record.exists())
        self.assertNotIn("chat-inputs ", result.stdout)

    def test_03a_a_failing_shared_test_fails_the_step_too(self) -> None:
        self.rebuild(shared_test=FAILING)
        result = self.ci()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(self.staged()), 1)

    def test_03b_the_full_recipe_runs_ci_right_after_unittest_discover(self) -> None:
        with (
            mock.patch.object(check, "run") as run,
            mock.patch.object(check, "doctor"),
            mock.patch.object(check, "capture", return_value="tools/bootstrap\n"),
            mock.patch.object(check, "hub_tool", return_value=sys.executable),
        ):
            check.checks(Path("/hub"), False)
        commands = [[str(part) for part in call.args[0]] for call in run.call_args_list]
        # CK-36: the suite runs as several unittest groups; S7a: the mypy step (the patched hub tool,
        # here the interpreter itself) runs after the last `discover`, and CI follows it.
        discover = max(
            i for i, c in enumerate(commands) if c[1:4] == ["-m", "unittest", "discover"]
        )
        ci = [i for i, c in enumerate(commands) if c[1:] == ["tools/chat/install.py", "ci"]]
        self.assertEqual(commands[discover + 1], [sys.executable])
        self.assertEqual(ci, [discover + 2])
        self.assertEqual(commands[ci[0]][0], sys.executable)
        with (
            mock.patch.object(check, "run") as run,
            mock.patch.object(check, "capture", return_value="tools/bootstrap\n"),
            mock.patch.object(check, "hub_tool", return_value=sys.executable),
        ):
            check.checks(Path("/hub"), True)
        self.assertFalse(
            [
                call
                for call in run.call_args_list
                if "install.py" in " ".join(map(str, call.args[0]))
            ]
        )

    def test_04_a_file_changed_between_snapshot_and_recheck_fails_the_copy(self) -> None:
        install = self.install_module()
        real: Callable[..., object] = shutil.copytree

        def racing(
            source: str | Path, destination: str | Path, *args: object, **kwargs: object
        ) -> object:
            result = real(source, destination, *args, **kwargs)
            if Path(source).name == "chat":
                (self.shared / "chat/scripts/chatlib.py").write_text("VALUE = 2\n")
            return result

        with (
            mock.patch.object(shutil, "copytree", racing),
            self.assertRaisesRegex(ValueError, "shared chat inputs changed while copying"),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            install.ci()

    def test_04a_a_file_changed_before_the_copy_is_not_in_the_copy(self) -> None:
        install = self.install_module()
        real: Callable[..., object] = shutil.copytree

        def racing(
            source: str | Path, destination: str | Path, *args: object, **kwargs: object
        ) -> object:
            if Path(source).name == "project-manager":
                (self.shared / "project-manager/SKILL.md").write_text("# changed\n")
            return real(source, destination, *args, **kwargs)

        with (
            mock.patch.object(shutil, "copytree", racing),
            self.assertRaisesRegex(ValueError, "shared chat inputs changed while copying"),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            install.ci()

    def test_15_the_suite_does_not_depend_on_who_runs_it(self) -> None:
        self.rebuild(shared_test=ENV_PROBE)
        # Control: the probe really fails when an identity variable is visible to it.
        probe = self.base / "probe" / "test_probe.py"
        probe.parent.mkdir()
        probe.write_text(ENV_PROBE)
        control = subprocess.run(
            [sys.executable, "-m", "unittest", "discover", "-s", str(probe.parent)],
            check=False,
            env=self.env(CK_CHAT_SOURCE_ROOT=str(self.checkout), **IDENTITY),
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(control.returncode, 0)
        clean = subprocess.run(
            [sys.executable, "-m", "unittest", "discover", "-s", str(probe.parent)],
            check=False,
            env=self.env(CK_CHAT_SOURCE_ROOT=str(self.checkout)),
            capture_output=True,
            text=True,
        )
        self.assertEqual(clean.returncode, 0, clean.stderr)
        # Under ci and stage, with the caller exporting a manager's identity.
        ci = self.ci(CK_CHAT_INPUTS=str(self.record), **IDENTITY)
        self.assertEqual(ci.returncode, 0, ci.stderr)
        self.assertTrue(self.record.exists())
        self.assertEqual(self.staged(), [])
        stage = self.stage_command(**IDENTITY)
        self.assertEqual(stage.returncode, 0, stage.stderr)
        self.assertEqual(len(self.staged()), 1)
        # The same result as for a caller with no identity at all.
        plain = self.stage_command()
        self.assertEqual(plain.returncode, 0, plain.stderr)

    def test_stage_is_unchanged_but_records_the_shared_inputs(self) -> None:
        result = subprocess.run(
            [sys.executable, str(self.checkout / "tools/chat/install.py"), "stage"],
            check=False,
            env=self.env(),
            cwd=self.checkout,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        destination = Path(result.stdout.strip().splitlines()[-1])
        self.assertTrue(destination.exists())
        tested = json.loads((destination / "tested.json").read_text())
        files = expected_inputs(self.shared)
        self.assertEqual(tested["shared"], {"files": files, "aggregate": expected_aggregate(files)})
        self.assertEqual(set(tested), {"baseline", "installed", "source", "shared"})


class Symlinks(ChatCiFixture):
    def make_links(self, kind: str) -> Path:
        outside = self.base / "outside"
        outside.mkdir()
        (outside / "target.py").write_text("VALUE = 1\n")
        scripts = self.shared / "chat/scripts"
        if kind == "file link":
            (scripts / "linked.py").symlink_to(scripts / "chatlib.py")
        elif kind == "directory link":
            (scripts / "linked").symlink_to(scripts / "tests", target_is_directory=True)
        elif kind == "external link":
            (scripts / "external.py").symlink_to(outside / "target.py")
        else:
            copy = self.base / "pm-elsewhere"
            shutil.move(self.shared / "project-manager", copy)
            (self.shared / "project-manager").symlink_to(copy, target_is_directory=True)
        return outside / "target.py"

    def test_10_every_kind_of_link_is_refused_and_nothing_runs(self) -> None:
        for kind in ("file link", "directory link", "external link", "tree root link"):
            with self.subTest(kind=kind):
                self.rebuild(shared_test=MARKER)
                for leftover in (self.record, self.tmp / "ran-marker"):
                    leftover.unlink(missing_ok=True)
                external = self.make_links(kind)
                inputs = self.inputs_module()
                with self.assertRaisesRegex(ValueError, "symlink in shared chat inputs"):
                    inputs.shared_inputs(self.shared)
                result = self.ci(CK_CHAT_INPUTS=str(self.record))
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("symlink in shared chat inputs", result.stderr)
                self.assertFalse(self.record.exists())
                self.assertFalse((self.tmp / "ran-marker").exists())
                if kind == "external link":
                    external.write_text("VALUE = 2  # the target's bytes changed\n")
                    again = self.ci(CK_CHAT_INPUTS=str(self.record))
                    self.assertNotEqual(again.returncode, 0)
                    self.assertFalse(self.record.exists())

    def test_10a_a_non_regular_entry_is_refused(self) -> None:
        fifo = self.shared / "chat/scripts/pipe"
        os.mkfifo(fifo)
        inputs = self.inputs_module()
        with self.assertRaisesRegex(ValueError, "pipe"):
            inputs.shared_inputs(self.shared)

    def test_10b_a_link_created_between_snapshot_and_copy_is_copied_as_a_link_and_refused(
        self,
    ) -> None:
        install = self.install_module()
        real: Callable[..., object] = shutil.copytree

        def late_link(
            source: str | Path, destination: str | Path, *args: object, **kwargs: object
        ) -> object:
            if Path(source).name == "chat":
                (self.shared / "chat/scripts/late.py").symlink_to(
                    self.shared / "chat/scripts/chatlib.py"
                )
            return real(source, destination, *args, **kwargs)

        with (
            mock.patch.object(shutil, "copytree", late_link),
            self.assertRaisesRegex(ValueError, "symlink in shared chat inputs"),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            install.ci()
        copied = list(self.tmp.glob("ck-chat-tested-*/chat/scripts/late.py"))
        self.assertEqual(len(copied), 1)
        self.assertTrue(copied[0].is_symlink())


class InstallDrift(ChatCiFixture):
    def pointer(self, receipt: Path | None = None) -> types.ModuleType:
        install = self.install_module()
        self.accept(install, receipt)
        return install

    def accept(self, install: types.ModuleType, receipt: Path | None = None) -> None:
        """An accepted pointer whose fingerprint matches the fixture checkout's sources."""
        names = [
            *sorted(install.SOURCES),
            "tools/chat/install.py",
            "tools/ck-chat",
            "skills/ck-chat/SKILL.md",
        ]
        files = {
            name: {"sha256": install.digest(self.checkout / name), "executable": False}
            for name in names
            if (self.checkout / name).exists()
        }
        state: dict[str, object] = {
            "schema": 2,
            "repo": str(self.checkout),
            "fingerprint": {"hashes": [], "files": files},
            "submissions": [],
        }
        if receipt:
            state["receipt"] = str(receipt)
        path = self.checkout / ".pipeline/accepted-v2.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(state))

    def stage(self, install: types.ModuleType) -> Path:
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            install.stage()
        return Path(out.getvalue().strip().splitlines()[-1])

    def apply(self, install: types.ModuleType, stage: Path) -> str:
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            install.apply(stage)
        return out.getvalue()

    def receipts(self, chat_inputs: Mapping[str, object] | None) -> Path:
        ci = self.base / "ci.json"
        ci.write_text(
            json.dumps(
                {
                    "kind": "ci",
                    "passed": True,
                    **({"chat_inputs": chat_inputs} if chat_inputs else {}),
                }
            )
        )
        docs = self.base / "docs.json"
        docs.write_text(json.dumps({"kind": "docs", "previous": str(ci)}))
        return docs

    def test_08_apply_refuses_when_a_non_transformed_shared_file_changed_after_stage(self) -> None:
        install = self.pointer()
        stage = self.stage(install)
        (self.shared / "chat/scripts/pchat").write_text("#!/usr/bin/env python3\n# changed\n")
        with self.assertRaisesRegex(ValueError, "Shared chat inputs changed since staging"):
            self.apply(install, stage)
        self.assertFalse((self.home / ".local/state/chat/codekarl-install").exists())
        self.assertFalse((self.shared / "chat/scripts/codekarl_chat.py").exists())

    def test_08a_an_unchanged_stage_installs_as_before(self) -> None:
        install = self.pointer()
        stage = self.stage(install)
        output = self.apply(install, stage)
        self.assertIn("Installed accepted adapter", output)
        self.assertIn("unavailable", output)
        self.assertEqual(
            (self.shared / "chat/scripts/codekarl_chat.py").read_bytes(),
            (self.checkout / "tools/chat/codekarl_chat.py").read_bytes(),
        )

    def test_08b_a_stage_without_shared_inputs_must_be_restaged(self) -> None:
        install = self.pointer()
        stage = self.stage(install)
        tested = json.loads((stage / "tested.json").read_text())
        del tested["shared"]
        (stage / "tested.json").write_text(json.dumps(tested))
        with self.assertRaisesRegex(ValueError, "Shared chat inputs changed since staging"):
            self.apply(install, stage)

    def assert_nothing_installed(self) -> None:
        self.assertFalse((self.home / ".local/state/chat/codekarl-install").exists())
        self.assertFalse((self.shared / "chat/scripts/codekarl_chat.py").exists())
        self.assertFalse((self.home / ".claude/skills/ck-chat").exists())
        self.assertFalse((self.shared / "ck-chat").exists())

    def test_08f_the_helper_is_a_bound_source_of_the_stage(self) -> None:
        install = self.pointer()
        self.assertIn("tools/chat/inputs.py", install.SOURCES)
        stage = self.stage(install)
        tested = json.loads((stage / "tested.json").read_text())
        self.assertEqual(
            tested["source"]["tools/chat/inputs.py"],
            install.digest(self.checkout / "tools/chat/inputs.py"),
        )

    def test_08g_an_unaccepted_helper_is_refused_before_any_write(self) -> None:
        install = self.pointer()
        helper = self.checkout / "tools/chat/inputs.py"
        helper.write_text(helper.read_text() + "\n# an edit the accepted pointer never saw\n")
        stage = self.stage(install)
        with self.assertRaisesRegex(ValueError, "Source is not accepted: tools/chat/inputs.py"):
            self.apply(install, stage)
        self.assert_nothing_installed()

    def test_08h_a_helper_changed_after_staging_is_refused_before_any_write(self) -> None:
        install = self.pointer()
        stage = self.stage(install)
        helper = self.checkout / "tools/chat/inputs.py"
        helper.write_text(helper.read_text() + "\n# changed after the stage was tested\n")
        self.accept(install)  # even if the changed helper is accepted, it was not the tested one
        with self.assertRaisesRegex(ValueError, "Source changed after staging"):
            self.apply(install, stage)
        self.assert_nothing_installed()

    def test_08i_a_stage_manifest_loads_through_the_loader_apply_uses(self) -> None:
        install = self.pointer()
        stage = self.stage(install)
        manifest = install.load_stage_manifest(
            json.loads((stage / "tested.json").read_text()), "tested.json"
        )
        self.assertEqual(set(manifest["source"]), set(install.SOURCES))
        self.assertEqual(set(manifest["baseline"]), set(manifest["installed"]))
        self.assertEqual(
            manifest["shared"]["aggregate"], expected_aggregate(expected_inputs(self.shared))
        )

    def test_08j_a_damaged_or_old_manifest_is_refused_with_the_next_action_before_any_write(
        self,
    ) -> None:
        install = self.pointer()
        stage = self.stage(install)
        path = stage / "tested.json"
        good = json.loads(path.read_text())

        def table(raw: dict[str, object], key: str) -> dict[str, object]:
            found = raw[key]
            assert isinstance(found, dict)
            return found

        def no_shared(raw: dict[str, object]) -> None:
            raw.pop("shared")

        def wrong_source_digest(raw: dict[str, object]) -> None:
            table(raw, "source").update(x=3)

        def baseline_not_a_map(raw: dict[str, object]) -> None:
            raw.update(baseline=["x"])

        def shared_without_files(raw: dict[str, object]) -> None:
            table(raw, "shared").pop("files")

        changes: list[tuple[str, Callable[[dict[str, object]], None], str]] = [
            ("no shared inputs", no_shared, "restage"),
            ("a source digest of the wrong type", wrong_source_digest, "source"),
            ("baseline not a map", baseline_not_a_map, "baseline"),
            ("shared without files", shared_without_files, "shared"),
        ]
        for label, change, mentions in changes:
            with self.subTest(label):
                raw: dict[str, object] = json.loads(json.dumps(good))
                change(raw)
                path.write_text(json.dumps(raw))
                with self.assertRaisesRegex((ValueError, RuntimeError), mentions):
                    self.apply(install, stage)
                self.assert_nothing_installed()

    def test_08c_the_receipt_lookup_walks_past_a_docs_receipt(self) -> None:
        files = expected_inputs(self.shared)
        install = self.pointer(self.receipts(record_for(files)))
        stage = self.stage(install)
        output = self.apply(install, stage)
        self.assertIn(f"Accepted batch receipt chat inputs: {expected_aggregate(files)}", output)
        self.assertNotIn("differ", output)

    def test_08d_a_differing_receipt_is_reported_not_refused(self) -> None:
        other = record_for({"chat/other": "0" * 64})
        install = self.pointer(self.receipts(other))
        stage = self.stage(install)
        output = self.apply(install, stage)
        self.assertIn(f"Accepted batch receipt chat inputs: {other['aggregate']}", output)
        self.assertIn("reported, not refused", output)
        self.assertIn("Installed accepted adapter", output)

    def test_08e_a_ci_receipt_without_a_record_prints_unavailable(self) -> None:
        install = self.pointer(self.receipts(None))
        stage = self.stage(install)
        output = self.apply(install, stage)
        self.assertIn("Accepted batch receipt chat inputs: unavailable", output)


if __name__ == "__main__":
    unittest.main()
