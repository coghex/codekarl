"""S12a: checkpoint leases for controllers, candidates, `worker` and `sync`.

Real processes where stated; every wait on a child has a deadline and fails the test with what the child
had printed, and no test orders processes by sleeping.
"""

import ast
import contextlib
import fcntl
import io
import json
import os
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import textwrap
import threading
import time
import unittest
from collections.abc import Callable, Iterable, Mapping, Sequence
from contextlib import AbstractContextManager
from pathlib import Path
from typing import NoReturn
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import pipeline
import test_batch
from gate import batch, cli, lease, owned, pointer

REPO = Path(__file__).resolve().parents[1]
TOOLS = REPO / "tools"
BOUND = 15.0


def clean_environment(**extra: str) -> dict[str, str]:
    """The caller's environment without the batch variables (batch CI exports them to the suite)."""
    env = {
        key: value
        for key, value in os.environ.items()
        if key
        not in {
            "CK_CI_LOCK_FD",
            "CK_CHAT_INPUTS",
            "CK_REQUIRE_FULL_SPEC",
            "CK_NATIVE_ANALYZE",
            "CK_LEASE_FDS",  # a test child adopts only the leases the test hands it
        }
        and not key.startswith("CARGO_")
    }
    env.update(extra)
    return env


def exclusive_refused(path: Path) -> bool:
    """True if an exclusive nonblocking lock on the directory is refused right now."""
    descriptor = os.open(path, os.O_RDONLY)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        return True
    finally:
        os.close(descriptor)
    return False


def wait_until(condition: Callable[[], object], what: str, seconds: float = BOUND) -> None:
    deadline = time.monotonic() + seconds
    while not condition():
        if time.monotonic() >= deadline:
            raise AssertionError(f"timed out after {seconds:g} s waiting for {what}")
        time.sleep(0.02)


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    state = subprocess.run(
        ["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True, check=False
    ).stdout.strip()
    return bool(state) and not state.startswith("Z")


class Lines:
    """Lossless, bounded line reads from a child's stdout."""

    def __init__(self, test: unittest.TestCase, process: subprocess.Popen[bytes]) -> None:
        self.test, self.process, self.pending = test, process, b""
        self.seen: list[str] = []

    def read(self, seconds: float = BOUND) -> str:
        assert self.process.stdout is not None
        deadline = time.monotonic() + seconds
        while b"\n" not in self.pending:
            left = deadline - time.monotonic()
            if left <= 0 or not select.select([self.process.stdout], [], [], left)[0]:
                self.test.fail(f"no line within {seconds:g} s; the child had printed {self.seen}")
            chunk = os.read(self.process.stdout.fileno(), 4096)
            if not chunk:
                self.process.wait(timeout=BOUND)
                assert self.process.stderr is not None
                errors = self.process.stderr.read().decode(errors="replace")[-1500:]
                self.test.fail(f"the child ended; it had printed {self.seen}; stderr: {errors}")
            self.pending += chunk
        line, _, self.pending = self.pending.partition(b"\n")
        self.seen.append(line.decode().strip())
        return self.seen[-1]


class LeaseFixture(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ck-lease-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(os.path.realpath(self.temp.name))
        self.pids: list[int] = []
        self.addCleanup(self.reap)
        self.addCleanup(self.kill_everything_under_the_fixture)

    def reap(self) -> None:
        for pid in self.pids:
            if alive(pid):
                with contextlib.suppress(ProcessLookupError):
                    os.kill(pid, signal.SIGKILL)

    @staticmethod
    def end_group(process: subprocess.Popen[bytes]) -> None:
        with contextlib.suppress(ProcessLookupError, PermissionError):
            os.killpg(process.pid, signal.SIGKILL)
        process.wait()

    def kill_everything_under_the_fixture(self) -> None:
        """Whatever a failed test left running names this fixture's directory on its command line."""
        found = subprocess.run(
            ["pgrep", "-f", str(self.base)], capture_output=True, text=True, check=False
        ).stdout.split()
        for pid in map(int, found):
            if pid != os.getpid():
                with contextlib.suppress(ProcessLookupError, PermissionError):
                    os.kill(pid, signal.SIGKILL)

    def directory(self, name: str = "checkpoint") -> Path:
        path = self.base / name
        path.mkdir()
        return path

    def child(
        self,
        code: str,
        *,
        pass_fds: Sequence[int] = (),
        env: Mapping[str, str] | None = None,
        stdin: int | None = None,
    ) -> subprocess.Popen[bytes]:
        process = subprocess.Popen(
            [sys.executable, "-c", textwrap.dedent(code)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env or clean_environment(),
            pass_fds=pass_fds,
            start_new_session=True,  # its own group, so a failed test cannot leave descendants behind
            stdin=stdin,
        )
        self.addCleanup(self.end_group, process)
        self.addCleanup(
            lambda: [s.close() for s in (process.stdin, process.stdout, process.stderr) if s]
        )
        return process


class Primitive(LeaseFixture):
    def test_01_an_exclusive_lock_is_refused_while_a_shared_lease_is_held(self) -> None:
        path = self.directory()
        self.assertFalse(exclusive_refused(path))
        descriptor = lease.acquire(path)
        self.addCleanup(lease.release, descriptor)
        self.assertTrue(exclusive_refused(path))  # another descriptor in this process
        probe = self.child(
            f"""
            import fcntl, os, sys
            fd = os.open({str(path)!r}, os.O_RDONLY | os.O_DIRECTORY)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                sys.exit(0)
            sys.exit(7)
            """
        )
        self.assertEqual(probe.wait(timeout=BOUND), 0)  # and a separate process
        lease.release(descriptor)
        self.assertFalse(exclusive_refused(path))

    def test_02_a_rename_keeps_the_inode_and_the_check_detects_a_replacement(self) -> None:
        path = self.directory()
        descriptor = lease.acquire(path)
        self.addCleanup(lease.release, descriptor)
        moved = self.base / "moved"
        path.rename(moved)
        self.assertEqual(os.fstat(descriptor).st_ino, moved.stat().st_ino)
        self.assertTrue(exclusive_refused(moved))  # still the leased directory
        path.mkdir()  # a different directory under the old name
        self.assertNotEqual(os.fstat(descriptor).st_ino, path.stat().st_ino)
        # The check inside acquire: the path names a different inode than the one opened.
        other = path.stat()
        with (
            mock.patch.object(Path, "stat", return_value=other),
            self.assertRaises(lease.LeaseUnavailable) as caught,
        ):
            lease.acquire(moved)
        self.assertIn("replaced", str(caught.exception))
        # Nothing leaked on failure: the registry holds the one lease taken above (and whatever this
        # process inherited, e.g. the checkout lease of the check.py that runs the suite).
        self.assertEqual([entry[1] for entry in lease.held()].count(path), 1)
        self.assertNotIn(moved, [entry[1] for entry in lease.held()])

    def test_03_a_missing_directory_and_a_file_are_unavailable(self) -> None:
        with self.assertRaises(lease.LeaseUnavailable):
            lease.acquire(self.base / "nothing")
        (self.base / "file").write_text("x")
        with self.assertRaises(lease.LeaseUnavailable):
            lease.acquire(self.base / "file")

    def test_04_an_exclusive_lock_held_by_another_process_makes_acquire_give_up(self) -> None:
        path = self.directory()
        blocker = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
        self.addCleanup(os.close, blocker)
        fcntl.flock(blocker, fcntl.LOCK_EX | fcntl.LOCK_NB)
        started = time.monotonic()
        with self.assertRaises(lease.LeaseUnavailable) as caught:
            lease.acquire(path, wait=0.4)
        self.assertGreaterEqual(time.monotonic() - started, 0.35)  # it polled before giving up
        self.assertIn("exclusively locked", str(caught.exception))
        fcntl.flock(blocker, fcntl.LOCK_UN)
        self.addCleanup(
            lease.release, lease.acquire(path, wait=0.4)
        )  # and succeeds once it is free

    def test_05_a_lease_inherited_by_a_child_and_grandchild_outlives_the_killed_parent(
        self,
    ) -> None:
        path = self.directory()
        parent = self.child(
            f"""
            import os, subprocess, sys
            sys.path.insert(0, {str(TOOLS)!r})
            from gate import lease
            fd = lease.acquire({str(path)!r})
            middle = (
                "import os, subprocess, sys, time\\n"
                "grand = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(600)'],"
                " pass_fds=(int(sys.argv[1]),))\\n"
                "print('descendants', os.getpid(), grand.pid, flush=True)\\n"
                "time.sleep(600)"
            )
            child = subprocess.Popen([sys.executable, "-c", middle, str(fd)], pass_fds=(fd,))
            print("parent", os.getpid(), flush=True)
            child.wait()
            """
        )
        lines = Lines(self, parent)
        parent_pid = int(lines.read().split()[1])
        _, middle_pid, grand_pid = lines.read().split()
        survivors = [int(middle_pid), int(grand_pid)]
        self.pids.extend([parent_pid, *survivors])
        self.assertTrue(exclusive_refused(path))
        os.kill(parent_pid, signal.SIGKILL)
        parent.wait(timeout=BOUND)
        self.assertTrue(exclusive_refused(path))  # held by the descendants after the parent died
        os.kill(survivors[0], signal.SIGKILL)  # the middle process; the grandchild still holds it
        wait_until(lambda: not alive(survivors[0]), "the middle process to exit")
        self.assertTrue(exclusive_refused(path))
        os.kill(survivors[1], signal.SIGKILL)
        wait_until(lambda: not alive(survivors[1]), "the grandchild to exit")
        wait_until(lambda: not exclusive_refused(path), "the lease to be released")


def copy_tools(destination: Path) -> Path:
    """A fixture checkpoint's `tools` directory: the controller code under test, nothing else."""
    shutil.copytree(TOOLS, destination / "tools", ignore=shutil.ignore_patterns("__pycache__"))
    return destination


def tree(path: Path) -> list[str]:
    return sorted(str(entry.relative_to(path)) for entry in path.rglob("*"))


class TextModes(LeaseFixture):
    """The runtime half of the review's pairs: the type each option set promises is the type the
    child's output really has (the static half is MypyStep.test_07)."""

    COMMAND = (sys.executable, "-c", "print('ok')")

    def test_01_every_option_that_selects_text_mode_gives_str(self) -> None:
        for options in (
            {"text": True},
            {"encoding": "utf-8"},
            {"errors": "strict"},
            {"universal_newlines": True},
            {"text": True, "encoding": "utf-8"},
        ):
            with self.subTest(options):
                out = lease.run(self.COMMAND, capture_output=True, **options).stdout
                self.assertIsInstance(out, str)
                self.assertEqual(out, "ok\n")
                self.assertIsInstance(lease.check_output(self.COMMAND, **options), str)

    def test_02_no_option_gives_bytes(self) -> None:
        for options in ({}, {"text": False}, {"universal_newlines": False}, {"text": None}):
            with self.subTest(options):
                out = lease.run(self.COMMAND, capture_output=True, **options).stdout
                self.assertIsInstance(out, bytes)
                self.assertEqual(out, b"ok\n")
                self.assertIsInstance(lease.check_output(self.COMMAND, **options), bytes)

    def test_03_input_must_match_the_mode_and_an_unsupported_option_is_refused(self) -> None:
        cat = [sys.executable, "-c", "import sys; sys.stdout.write(sys.stdin.read())"]
        self.assertEqual(lease.run(cat, input="in", text=True, capture_output=True).stdout, "in")
        self.assertEqual(lease.run(cat, input=b"in", capture_output=True).stdout, b"in")
        with self.assertRaises(TypeError):
            lease.run(cat, input=b"in", text=True, capture_output=True)
        with self.assertRaises(TypeError):
            lease.run(cat, input="in", capture_output=True)
        with self.assertRaises(TypeError):
            lease.run(self.COMMAND, shell=True)  # type: ignore[call-overload]  # the refusal under test


class TextModeRefusals(LeaseFixture):
    """What the text-mode options refuse, before any child exists (S7b review, B1 and B3)."""

    def marker_command(self) -> tuple[Path, tuple[str, str, str]]:
        marker = self.base / "child-ran"
        return marker, (sys.executable, "-c", f"open({str(marker)!r}, 'w').close()")

    def test_01_contradictory_text_aliases_are_refused_as_subprocess_refuses_them(self) -> None:
        for options in (
            {"text": False, "universal_newlines": True},
            {"text": True, "universal_newlines": False},
        ):
            for call in (
                lambda command, o: lease.run(command, capture_output=True, **o),
                lambda command, o: lease.check_output(command, **o),
            ):
                with self.subTest(options):
                    marker, command = self.marker_command()
                    marker.unlink(missing_ok=True)
                    with self.assertRaises(subprocess.SubprocessError):
                        call(command, options)
                    self.assertFalse(marker.exists(), "a child ran although the call was refused")
        marker, command = self.marker_command()
        lease.run(command, text=True, universal_newlines=True, capture_output=True)  # agreeing
        self.assertTrue(marker.exists())

    def test_02_an_empty_encoding_or_errors_still_reads_bytes_as_subprocess_does(self) -> None:
        # The types do not promise str for these (only the literal "utf-8" and "strict" select
        # the str overloads; MypyStep.test_07 pairs this), and the runtime keeps subprocess's rule.
        for options in ({"encoding": ""}, {"errors": ""}):
            with self.subTest(options):
                self.assertIsInstance(
                    lease.run(  # type: ignore[call-overload]  # an empty option is not a selecting literal
                        [sys.executable, "-c", "print(1)"], capture_output=True, **options
                    ).stdout,
                    bytes,
                )
                self.assertIsInstance(
                    lease.check_output(  # type: ignore[call-overload]  # the same, for check_output
                        [sys.executable, "-c", "print(1)"], **options
                    ),
                    bytes,
                )
        self.assertEqual(
            lease.check_output([sys.executable, "-c", "print('é')"], encoding="utf-8"), "é\n"
        )
        self.assertEqual(
            lease.check_output([sys.executable, "-c", "print(1)"], errors="strict"), "1\n"
        )


class Controller(LeaseFixture):
    def driver(self, checkpoint: Path, body: str) -> tuple[subprocess.Popen[bytes], Path]:
        """A controller process the way the launcher runs one: `<checkpoint>/tools` first on sys.path."""
        hub = self.base / "hub"
        (hub / ".pipeline").mkdir(parents=True, exist_ok=True)
        code = (
            "import os, sys\n"
            "from pathlib import Path\n"
            f"sys.path.insert(0, {str(checkpoint / 'tools')!r})\n"
            "from gate import cli\n"
            f"root, hub = Path({str(self.base)!r}), Path({str(hub)!r})\n"
            "sys.argv = ['ck', 'status']\n" + textwrap.dedent(body)
        )
        process = self.child(code, stdin=subprocess.PIPE)
        return process, hub

    def test_01_a_checkpoint_renamed_before_the_lease_exits_with_the_rerun_message_and_no_state(
        self,
    ) -> None:
        checkpoint = copy_tools(self.directory("cp1"))
        gone = self.base / "cp1.collected"
        process, hub = self.driver(
            checkpoint,
            f"""
            os.rename({str(checkpoint)!r}, {str(gone)!r})  # collected after the imports, before the lease
            cli.main(root, hub)
            """,
        )
        before = tree(hub)
        _, errors = process.communicate(timeout=BOUND)
        self.assertEqual(process.returncode, 1, errors)
        self.assertIn("ck: controller checkpoint was collected; rerun the command", errors.decode())
        self.assertEqual(tree(hub), before)  # no lock, record or pointer write
        self.assertFalse(list(hub.rglob("*.lock")))

    def test_02_a_controller_holds_its_lease_through_a_lazy_import_and_completes(self) -> None:
        checkpoint = copy_tools(self.directory("cp1"))
        process, _ = self.driver(
            checkpoint,
            """
            def blocked(root, hub, argv=None):
                print("LEASED", flush=True)
                sys.stdin.readline()  # the hook: held between the imports and a lazy import
                import gate.provenance  # a lazy import after the hold
                print("IMPORTED", gate.provenance.__file__, flush=True)
                return 0
            cli.run = blocked
            cli.main(root, hub)
            """,
        )
        lines = Lines(self, process)
        self.assertEqual(lines.read(), "LEASED")
        self.assertTrue(exclusive_refused(checkpoint))  # the controller holds it
        assert process.stdin is not None
        process.stdin.write(b"\n")
        process.stdin.flush()
        self.assertTrue(lines.read().startswith("IMPORTED"))  # the lazy import still finds its file
        self.assertEqual(process.wait(timeout=BOUND), 0)
        self.assertFalse(exclusive_refused(checkpoint))  # released when the controller exits

    def test_03_the_controller_leases_the_directory_that_contains_its_real_tools_path(self) -> None:
        checkpoint = copy_tools(self.directory("cp1"))
        link = self.base / "alias"
        link.symlink_to(checkpoint)  # reached through a symlink: the real path is leased
        process, _ = self.driver(
            link,
            """
            def blocked(root, hub, argv=None):
                print("LEASED", flush=True)
                sys.stdin.readline()
                return 0
            cli.run = blocked
            cli.main(root, hub)
            """,
        )
        lines = Lines(self, process)
        self.assertEqual(lines.read(), "LEASED")
        self.assertTrue(exclusive_refused(checkpoint))
        assert process.stdin is not None
        process.stdin.write(b"\n")
        process.stdin.flush()
        self.assertEqual(process.wait(timeout=BOUND), 0)


# --- the spawn-helper guard -----------------------------------------------------------------------

CONTROLLER_MODULES = (
    "tools/pipeline.py",
    "tools/environment.py",
    # tools/gate/lease.py is the helper itself: the one module that calls `subprocess`.
    *sorted(
        f"tools/gate/{path.name}"
        for path in (TOOLS / "gate").glob("*.py")
        if path.name != "lease.py"
    ),
)
SPAWNERS = {"run", "Popen", "check_output", "check_call", "call", "getoutput", "getstatusoutput"}
OS_SPAWNERS = {
    "system",
    "popen",
    "execv",
    "execve",
    "execvp",
    "execl",
    "spawnv",
    "spawnl",
    "posix_spawn",
}

# Every direct use of a process-starting call in a controller module must be listed here with the
# reason it is outside the rule. The helper itself (tools/gate/lease.py) is the one place that calls
# `subprocess`. The trampoline (tools/ck) is not a controller module: it runs before `exec` and holds
# no lease. tools/check.py is not scanned either, for two reasons that both hold: as a candidate's or a
# caller copy's script it leases its own checkout in `main` and passes that descriptor (announced in
# CK_LEASE_FDS) to every step; and when `ck doctor` imports it inside a controller, that controller's
# lease is already held, `check.run` merges every held lease descriptor into each child's `pass_fds`
# and environment, and the controller's enrollment covers the doctor's processes.
# The one exception is the owner's post-publish hook (CK-152): it must outlive the command in its own
# session and is never waited for, which the waiting helper cannot do, and it inherits no descriptors
# (`close_fds`) and no lease announcement (the environment drops it), so it holds no lease.
ALLOWED_DIRECT_SPAWNS: dict[tuple[str, str], str] = {
    ("tools/gate/postpublish.py", "subprocess.Popen("): "the detached post-publish hook, no leases",
}  # (module, line text): reason; every other spawn uses the helper


def direct_spawns(source: str, filename: str) -> list[tuple[int, str]]:
    """(line, text) of every `subprocess.<spawner>` or `os.<spawner>` use in the module source."""
    tree = ast.parse(source, filename)
    lines = source.splitlines()
    found: list[tuple[int, str]] = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name):
            module, name = node.value.id, node.attr
            if (module == "subprocess" and name in SPAWNERS) or (
                module == "os" and name in OS_SPAWNERS
            ):
                found.append((node.lineno, lines[node.lineno - 1].strip()))
        if isinstance(node, ast.ImportFrom) and node.module == "subprocess":
            for alias in node.names:
                if alias.name in SPAWNERS:
                    found.append((node.lineno, lines[node.lineno - 1].strip()))
    return found


class SpawnHelper(unittest.TestCase):
    def offences(self, modules: Iterable[str], root: Path = REPO) -> dict[str, str]:
        return {
            f"{module}:{line}": text
            for module in modules
            for line, text in direct_spawns((root / module).read_text(), module)
            if (module, text) not in ALLOWED_DIRECT_SPAWNS
        }

    def test_01_no_controller_module_starts_a_process_except_through_the_helper(self) -> None:
        self.assertGreaterEqual(len(CONTROLLER_MODULES), 12)
        self.assertEqual(self.offences(CONTROLLER_MODULES), {})

    def test_02_the_scan_covers_the_modules_that_start_processes_today(self) -> None:
        # A sanity check on the scan itself: the helper module is the one that does spawn.
        self.assertTrue(direct_spawns((TOOLS / "gate/lease.py").read_text(), "lease.py"))
        started = {
            module
            for module in ("tools/pipeline.py", "tools/environment.py", "tools/gate/batch.py")
            if "lease.run(" in (REPO / module).read_text()
            or "lease.check_output(" in (REPO / module).read_text()
        }
        self.assertEqual(
            started, {"tools/pipeline.py", "tools/environment.py", "tools/gate/batch.py"}
        )

    def test_04_the_only_ungrouped_children_are_the_named_exceptions(self) -> None:
        # Behaviour 3's explicit exception (`ssh-agent -s`) and the legacy interactive passthrough: a
        # call with `group=False` must be one of the named cases, exactly once, and nothing else may
        # use it (a second call in the same module is an offence, not an overwrite).
        found: list[tuple[str, str]] = []
        for module in CONTROLLER_MODULES:
            tree = ast.parse((REPO / module).read_text(), module)
            for node in ast.walk(tree):
                if isinstance(node, ast.Call) and any(
                    keyword.arg == "group"
                    and isinstance(keyword.value, ast.Constant)
                    and keyword.value.value is False
                    for keyword in node.keywords
                ):
                    first = node.args[0]
                    words = (
                        [
                            e.value
                            for e in first.elts[:2]
                            if isinstance(e, ast.Constant) and isinstance(e.value, str)
                        ]
                        if isinstance(first, ast.List)
                        else []
                    )
                    found.append((module, " ".join(words) or "interactive passthrough"))
        self.assertEqual(
            sorted(found),
            [
                ("tools/environment.py", "ssh-agent -s"),
                ("tools/pipeline.py", "interactive passthrough"),
            ],
        )
        self.assertEqual(set(lease.UNGROUPED), {label for _module, label in found})

    def test_03_the_guard_fails_on_a_deliberately_added_direct_call(self) -> None:
        # Negative proof: a copy of a controller module with one added direct `subprocess.run`.
        with tempfile.TemporaryDirectory(prefix="ck-guard-") as directory:
            root = Path(directory)
            (root / "tools/gate").mkdir(parents=True)
            source = (TOOLS / "gate/meta.py").read_text()
            (root / "tools/gate/meta.py").write_text(
                source + "\n\ndef sneaky():\n    return subprocess.run(['true'], check=False)\n"
            )
            (root / "tools/gate/other.py").write_text(
                (TOOLS / "gate/provenance.py").read_text()
                + "\nfrom subprocess import check_output\n"
            )
            found = self.offences(["tools/gate/meta.py", "tools/gate/other.py"], root)
            self.assertEqual(len(found), 2, found)
            self.assertTrue(any("subprocess.run" in text for text in found.values()))
            self.assertTrue(any("from subprocess import" in text for text in found.values()))

    def test_04_the_helper_passes_every_held_lease_and_keeps_the_callers_descriptors(self) -> None:
        with tempfile.TemporaryDirectory(prefix="ck-helper-") as directory:
            path = Path(directory)
            first = lease.acquire(path)
            self.addCleanup(lease.release, first)
            read_end, write_end = os.pipe()
            self.addCleanup(os.close, read_end)
            self.addCleanup(os.close, write_end)
            code = "import os, sys\nfor fd in sys.argv[1:]:\n    os.fstat(int(fd))\nprint('open', *sys.argv[1:])"
            result = lease.run(
                [sys.executable, "-c", code, str(first), str(write_end)],
                capture_output=True,
                text=True,
                check=True,
                pass_fds=(write_end,),
            )
            self.assertEqual(result.stdout.split()[0], "open")
            # With inherit=False the lease is withheld (for a daemon that would hold it for ever).
            withheld = lease.run(
                [sys.executable, "-c", code, str(first)],
                capture_output=True,
                text=True,
                inherit=False,
            )
            self.assertNotEqual(withheld.returncode, 0)

    def test_05_an_interrupted_helper_call_kills_the_childs_process_group(self) -> None:
        descriptor, name = tempfile.mkstemp(prefix="ck-group-")
        os.close(descriptor)
        marker = Path(name)
        self.addCleanup(marker.unlink)
        code = (
            "import os, subprocess, sys, time\n"
            "grand = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(600)'])\n"
            f"open({str(marker)!r}, 'w').write(f'{{os.getpid()}} {{grand.pid}}')\n"
            "time.sleep(600)"
        )

        class Interrupt(Exception):
            pass

        def interrupt() -> NoReturn:
            wait_until(lambda: marker.read_text() != "", "the child and grandchild pids")
            raise Interrupt

        with (
            mock.patch.object(
                subprocess.Popen, "communicate", side_effect=lambda *a, **k: interrupt()
            ),
            self.assertRaises(Interrupt),
        ):
            lease.run([sys.executable, "-c", code])
        child, grandchild = map(int, marker.read_text().split())
        wait_until(
            lambda: not alive(child) and not alive(grandchild), "the group to be killed and reaped"
        )


# --- real Pijul, hubs and candidates ---------------------------------------------------------------

HOLD_SHIM = """#!/bin/sh
# Test hook: when this is the held command, announce, wait for the release file, then run the real tool.
if [ "$1" = "$CK_HOLD_ON" ] || [ -z "$CK_HOLD_ON" ]; then
    echo "$$" > "$CK_HOLD/pid.$$"
    : > "$CK_HOLD/ready"
    while [ ! -e "$CK_HOLD/release" ]; do sleep 0.05; done
fi
exec "$CK_REAL" "$@"
"""

# Leaves a detached writer behind (a double fork keeps every inherited descriptor, the lease included):
# it appends to `$CK_WRITE` every 50 ms until the finish file appears.
DETACH = """import os, sys, time
if os.fork():
    sys.exit(0)
os.setsid()
if os.fork():
    os._exit(0)
null = os.open(os.devnull, os.O_RDWR)  # let go of the step's output pipe; keep every other descriptor
for descriptor in (0, 1, 2):
    os.dup2(null, descriptor)
hold, write = sys.argv[1:3]
open(hold + "/descendant.pid", "w").write(str(os.getpid()))
os.makedirs(os.path.dirname(write) or ".", exist_ok=True)
while not os.path.exists(hold + "/finish"):
    with open(write, "a") as stream:
        stream.write("x")
    time.sleep(0.05)
"""


# Stands in for the hub's ruff: the first call announces itself and waits for the release file, then
# leaves a detached writer behind (when `$CK_WRITE` is set) and runs the real ruff.
RUFF_HOOK = """#!/bin/sh
if [ ! -e "$CK_HOLD/ready" ]; then
    : > "$CK_HOLD/ready"
    while [ ! -e "$CK_HOLD/release" ]; do sleep 0.05; done
    if [ -n "$CK_WRITE" ]; then python3 -c "$CK_DETACH" "$CK_HOLD" "$CK_WRITE"; fi
fi
exec "$CK_REAL" "$@"
"""


def real_tool(name: str) -> str:
    found = shutil.which(name)
    if not found:
        raise AssertionError(f"{name} is required on PATH for these tests")
    return found


class HubFixture(LeaseFixture):
    def setUp(self) -> None:
        super().setUp()
        self.pipeline = pipeline
        self.hub = self.base / "hub"
        (self.hub / ".pipeline").mkdir(parents=True)
        self.hold = self.base / "hold"
        self.hold.mkdir()
        self.addCleanup(lambda: (self.hold / "release").touch())  # never leave a hook waiting
        self.addCleanup(lambda: (self.hold / "finish").touch())

    def repo(self, name: str, files: Mapping[str, str]) -> Path:
        path = self.base / name
        path.mkdir()
        self.pipeline.pijul(path, "init")
        files = {".ignore": "/.ck-hub\n", **files}  # the marker a clone writes is not a change
        for relative, text in files.items():
            (path / relative).parent.mkdir(parents=True, exist_ok=True)
            (path / relative).write_text(text)
        self.record(path, f"record {name}")
        return path

    def record(self, path: Path, message: str) -> None:
        self.pipeline.pijul(path, "add", "--recursive", ".")
        self.pipeline.pijul(path, "record", "-am", message, "--author", "Lease tests")

    def publish(self, repo: Path) -> None:
        state = {
            "schema": 2,
            "repo": str(repo),
            "fingerprint": self.pipeline.fingerprint(repo),
            "submissions": ["accepted"],
        }
        pointer.atomic_bytes(self.hub / ".pipeline/accepted-v2.json", pointer.canonical(state))
        pointer.atomic_bytes(
            self.hub / ".pipeline/accepted.json", pointer.canonical({"schema": "tombstone"})
        )

    def shim(self, directory: str, name: str, real: str) -> Path:
        shim = self.base / directory
        shim.mkdir(exist_ok=True)
        (shim / name).write_text(HOLD_SHIM)
        (shim / name).chmod(0o755)
        return shim

    def environment(
        self, shim_dir: Path, real: str, hold_on: str | None = None, **extra: str
    ) -> dict[str, str]:
        return clean_environment(
            PATH=f"{shim_dir}:{os.environ['PATH']}",
            CK_HOLD=str(self.hold),
            CK_REAL=real,
            CK_HOLD_ON=hold_on or "",
            **extra,
        )

    def held_command_pids(self) -> list[int]:
        return [int(path.name.split(".")[1]) for path in self.hold.glob("pid.*")]

    def gone(self, *paths: Path) -> None:
        wait_until(lambda: not any(exclusive_refused(path) for path in paths), "the lease to end")


class InheritedLeases(HubFixture):
    """Kill a controller with SIGKILL while its Pijul child reads a checkpoint: the child keeps the lease."""

    def setUp(self) -> None:
        super().setUp()
        self.accepted = self.repo("accepted", {"a.txt": "one\n"})
        self.caller = self.base / "caller"
        self.pipeline.clone(self.accepted, self.caller, self.hub)
        (self.accepted / "b.txt").write_text("two\n")
        self.record(self.accepted, "second change")
        self.publish(self.accepted)
        self.checkpoint = copy_tools(self.directory("cp1"))
        self.shim_dir = self.shim("shim", "pijul", real_tool("pijul"))

    def controller(
        self, hold_on: str | None, body: str, checkpoint: Path | None = None
    ) -> subprocess.Popen[bytes]:
        checkpoint = checkpoint or self.checkpoint
        code = (
            "import sys\nfrom pathlib import Path\n"
            f"sys.path.insert(0, {str(checkpoint / 'tools')!r})\n"
            "from gate import cli\n"
            f"root, hub = Path({str(self.caller)!r}), Path({str(self.hub)!r})\n"
            + textwrap.dedent(body)
        )
        return self.child(code, env=self.environment(self.shim_dir, real_tool("pijul"), hold_on))

    def kill_while_held(
        self, process: subprocess.Popen[bytes], sources: Sequence[Path], own: Path | None = None
    ) -> None:
        own = own or self.checkpoint
        wait_until((self.hold / "ready").exists, "the Pijul child to be held open")
        for path in (*sources, own):
            self.assertTrue(exclusive_refused(path), f"{path} is not leased while the child runs")
        os.kill(process.pid, signal.SIGKILL)
        process.wait(timeout=BOUND)
        self.assertEqual(process.returncode, -signal.SIGKILL)
        for path in (*sources, own):  # the controller's own checkpoint as well as the source
            self.assertTrue(
                exclusive_refused(path), f"{path} was released with the killed controller"
            )
        pids = self.held_command_pids()
        self.assertTrue(pids)
        (self.hold / "release").touch()
        wait_until(lambda: not any(alive(pid) for pid in pids), "the held child to finish")
        self.gone(*sources, own)  # and now an exclusive attempt succeeds

    def test_01_a_worker_clone_child_keeps_the_source_lease_after_the_controller_is_killed(
        self,
    ) -> None:
        process = self.controller(
            "clone", "sys.argv = ['ck', 'worker', 'w1']\ncli.main(root, hub)\n"
        )
        self.kill_while_held(process, [self.accepted])

    def test_02_a_sync_pull_child_keeps_the_source_lease_after_the_controller_is_killed(
        self,
    ) -> None:
        process = self.controller("pull", "sys.argv = ['ck', 'sync']\ncli.main(root, hub)\n")
        self.kill_while_held(process, [self.accepted])

    def test_03_a_controller_pijul_log_child_keeps_the_leases_after_the_controller_is_killed(
        self,
    ) -> None:
        # The controller reads its own checkpoint, so this checkpoint is a Pijul repository.
        own = copy_tools(self.directory("cp2"))
        (own / ".ignore").write_text("/.ck-hub\n")
        self.pipeline.pijul(own, "init")
        self.record(own, "controller checkpoint")
        process = self.controller(
            "log",
            f"""
            import pipeline
            def log_only(root, hub, argv=None):
                print(len(pipeline.hashes(Path({str(own)!r}))), flush=True)
                return 0
            cli.run = log_only
            cli.main(root, hub)
            """,
            checkpoint=own,
        )
        self.kill_while_held(process, [], own=own)

    def test_04_the_real_pijul_binary_keeps_an_inherited_lease_while_it_runs(self) -> None:
        # A change with a 300 kB description makes `pijul log` write more than a pipe holds, so the real
        # binary blocks on its unread stdout while the controller is killed. No shim is involved.
        (self.accepted / "c.txt").write_text("three\n")
        self.pipeline.pijul(self.accepted, "add", "c.txt")
        self.pipeline.pijul(
            self.accepted,
            "record",
            "-am",
            "big",
            "--author",
            "Lease tests",
            "--description",
            "d" * 300_000,
        )
        # The test holds the read end of the output pipe and never reads it, so the blocked binary
        # outlives the controller instead of dying on a broken pipe.
        read_end, write_end = os.pipe()
        self.addCleanup(os.close, read_end)
        process = self.child(
            f"""
            import os, subprocess, sys, time
            sys.path.insert(0, {str(TOOLS)!r})
            from gate import lease
            fd = lease.acquire({str(self.accepted)!r})
            log = subprocess.Popen(["pijul", "log", "--description"], cwd={str(self.accepted)!r},
                                   stdout={write_end}, pass_fds=(*lease.descriptors(), {write_end}))
            print(os.getpid(), log.pid, flush=True)
            time.sleep(600)
            """,
            pass_fds=(write_end,),
        )
        os.close(write_end)
        controller_pid, log_pid = map(int, Lines(self, process).read().split())
        self.pids.extend([controller_pid, log_pid])
        wait_until(lambda: alive(log_pid), "pijul log to start")
        os.kill(controller_pid, signal.SIGKILL)
        process.wait(timeout=BOUND)
        deadline = time.monotonic() + 1.0
        while time.monotonic() < deadline:  # the blocked binary stays alive, and so does the lease
            self.assertTrue(alive(log_pid), "pijul log finished: the output did not fill the pipe")
            self.assertTrue(exclusive_refused(self.accepted))
            time.sleep(0.05)
        os.kill(log_pid, signal.SIGKILL)
        wait_until(lambda: not alive(log_pid), "pijul log to be killed")
        self.gone(self.accepted)


class Readers(HubFixture):
    """`worker` and `sync` whose source is renamed away before their lease."""

    def setUp(self) -> None:
        super().setUp()
        self.first = self.repo("first", {"a.txt": "one\n"})
        self.second = self.base / "second"
        self.pipeline.clone(self.first, self.second, self.hub)  # the pointer's next repository
        self.publish(self.first)
        self.cli = cli

    def run_cli(self, root: Path, argv: list[str]) -> int:
        with contextlib.redirect_stdout(io.StringIO()):  # `worker` prints the new directory
            return self.cli.run(root, self.hub, argv)

    def racing(self, republish: bool) -> tuple[mock._patch[Callable[..., int]], list[Path]]:
        real = lease.acquire
        calls: list[Path] = []

        def acquire(path: Path, wait: float = lease.WAIT) -> int:
            calls.append(path)
            if len(calls) == 1:
                Path(path).rename(self.base / "first.collected")  # collected just before the lease
                if republish:
                    self.publish(self.second)
            return real(path, wait)

        return mock.patch.object(lease, "acquire", acquire), calls

    def test_01_a_worker_retries_from_the_current_pointer_after_its_source_is_collected(
        self,
    ) -> None:
        patch, calls = self.racing(republish=True)
        with patch:
            self.run_cli(self.base, ["worker", "w1"])
        self.assertEqual([Path(call) for call in calls], [self.first, self.second])
        destination = self.hub / ".pipeline/workers/w1"
        self.assertEqual(self.pipeline.hashes(destination), self.pipeline.hashes(self.second))

    def test_02_a_worker_fails_cleanly_when_the_source_never_comes_back(self) -> None:
        patch, calls = self.racing(republish=False)
        with patch, self.assertRaises(RuntimeError) as caught:
            self.run_cli(self.base, ["worker", "w1"])
        self.assertIn("could not be leased after 3 attempts", str(caught.exception))
        self.assertEqual(len(calls), 3)
        self.assertFalse(
            (self.hub / ".pipeline/workers/w1").exists()
        )  # no half-written destination
        self.assertFalse(list((self.hub / ".pipeline/workers").glob("*")))

    def test_03_a_failed_clone_removes_only_the_destination_it_created(self) -> None:
        workers = self.hub / ".pipeline/workers"
        workers.mkdir(parents=True)
        other = workers / "someone-else"
        other.mkdir()
        (other / "keep.txt").write_text("keep\n")
        with (
            mock.patch.object(self.pipeline, "clone", side_effect=RuntimeError("clone failed")),
            self.assertRaises(RuntimeError),
        ):
            self.run_cli(self.base, ["worker", "w1"])
        self.assertFalse((workers / "w1").exists())
        self.assertEqual((other / "keep.txt").read_text(), "keep\n")

    def test_04_a_sync_retries_from_the_current_pointer_after_its_source_is_collected(self) -> None:
        caller = self.base / "caller"
        self.pipeline.clone(self.first, caller, self.hub)
        (self.second / "b.txt").write_text("two\n")
        self.record(self.second, "second change")
        patch, calls = self.racing(republish=True)
        with patch:
            self.run_cli(caller, ["sync"])
        self.assertEqual([Path(call) for call in calls], [self.first, self.second])
        self.assertEqual(self.pipeline.hashes(caller), self.pipeline.hashes(self.second))

    def test_05_a_sync_fails_cleanly_when_the_source_never_comes_back(self) -> None:
        caller = self.base / "caller"
        self.pipeline.clone(self.first, caller, self.hub)
        before = self.pipeline.hashes(caller)
        patch, calls = self.racing(republish=False)
        with patch, self.assertRaises(RuntimeError):
            self.run_cli(caller, ["sync"])
        self.assertEqual(len(calls), 3)
        self.assertEqual(self.pipeline.hashes(caller), before)


class StaleTrampoline(HubFixture):
    """A `tools/ck` from a copy synced before S12a runs a post-S12a checkpoint."""

    def test_01_the_checkpoint_controller_takes_its_lease_whatever_the_trampolines_age(
        self,
    ) -> None:
        # The checkpoint the pointer names holds the controller code; the hub's ruff is the hook that
        # holds the controller (the doctor runs it) after the trampoline has verified and exec'd.
        checkpoint = copy_tools(self.directory("accepted"))
        (checkpoint / ".ignore").write_text("/.ck-hub\n")
        self.pipeline.pijul(checkpoint, "init")
        self.record(checkpoint, "post-S12a checkpoint")
        self.publish(checkpoint)
        bin_directory = self.hub / ".tools/python/bin"
        bin_directory.mkdir(parents=True)
        (bin_directory / "ruff").write_text(HOLD_SHIM)
        (bin_directory / "ruff").chmod(0o755)
        caller = self.base / "caller"
        (caller / "tools").mkdir(parents=True)
        # tools/ck is untouched by S12a, so the one in this tree is exactly the stale trampoline.
        shutil.copy2(TOOLS / "ck", caller / "tools/ck")
        (caller / ".ck-hub").write_text(f"{self.hub}\n")
        process = self.child(
            "import runpy, sys\n"
            f"sys.argv = ['ck', 'doctor']\n"
            f"runpy.run_path({str(caller / 'tools/ck')!r}, run_name='__main__')",
            env=self.environment(
                self.base / "no-shim",
                "/bin/echo",
                hold_on="--version",
                CK_SIGNING_ACTIVE=str(self.hub),
            ),
        )
        wait_until((self.hold / "ready").exists, "the controller's doctor to reach the hub's ruff")
        self.assertTrue(
            exclusive_refused(checkpoint), "the post-S12a controller did not lease its checkpoint"
        )
        (self.hold / "release").touch()
        process.communicate(
            timeout=BOUND
        )  # the doctor goes on to fail at ShellCheck: not under test
        self.gone(checkpoint)


# The gate's CI launcher as installed before S12a (`Manager._full_check` and the bootstrap helper's):
# the candidate's check.py is started with the CI descriptor and nothing else.
PRE_S12A_LAUNCHER = """import os, subprocess, sys
candidate, hub, log, *arguments = sys.argv[1:]
fd = os.open(hub + "/.pipeline/ci.lock", os.O_RDWR)
env = os.environ.copy()
env["CK_CI_LOCK_FD"] = str(fd)
result = subprocess.run(
    [sys.executable, candidate + "/tools/check.py", *arguments], check=False, cwd=candidate, env=env,
    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, pass_fds=(fd,),
)
open(log, "w").write(result.stdout)
print("exit", result.returncode, flush=True)
"""


class Candidates(HubFixture):
    """A candidate's own check.py takes the lease, whichever controller started it."""

    def setUp(self) -> None:
        super().setUp()
        self.ci_hub = self.base / "ci-hub"
        tools_bin = self.ci_hub / ".tools/python/bin"
        tools_bin.mkdir(parents=True)
        (self.ci_hub / ".pipeline").mkdir()
        (self.ci_hub / ".pipeline/ci.lock").touch()
        real_hub = Path(os.environ["CK_HUB"])
        for name in ("shellcheck", "mypy"):
            (tools_bin / name).symlink_to(real_hub / ".tools/python/bin" / name)
        (tools_bin / "ruff").write_text(RUFF_HOOK)
        (tools_bin / "ruff").chmod(0o755)
        # The hook is not here to lint: it only has to hold the step, so the real ruff is `true`.
        self.real_ruff = "/usr/bin/true"

    def candidate(self, bootstrap: bool = True) -> Path:
        path = self.directory("candidate")
        copy_tools(path)
        for relative in ("pyproject.toml", "rust-toolchain.toml"):
            shutil.copy2(REPO / relative, path / relative)
        (path / "Cargo.toml").write_text('[workspace]\nresolver = "3"\nmembers = ["m"]\n')
        (path / "m/src").mkdir(parents=True)
        (path / "m/Cargo.toml").write_text(
            '[package]\nname = "m"\nversion = "0.1.0"\nedition = "2024"\n'
        )
        (path / "m/src/lib.rs").write_text("//! A fixture crate.\n")
        (path / ".ignore").write_text("/.ck-hub\n/target\n")
        (path / ".ck-hub").write_text(f"{self.ci_hub}\n")
        if not bootstrap:
            (path / "tools/bootstrap").unlink()
        self.pipeline.pijul(path, "init")
        self.record(path, "candidate")
        (path / "target").mkdir()
        return path

    def hook_environment(self, writer: Path) -> dict[str, str]:
        return clean_environment(
            CK_HOLD=str(self.hold),
            CK_REAL=self.real_ruff,
            CK_WRITE=str(writer),
            CK_DETACH=DETACH,
        )

    def descendant_pid(self) -> int:
        wait_until((self.hold / "descendant.pid").exists, "the detached writer to start")
        wait_until(lambda: (self.hold / "descendant.pid").read_text() != "", "its pid")
        pid = int((self.hold / "descendant.pid").read_text())
        self.pids.append(pid)
        return pid

    def assert_writer_keeps_the_lease_then_ends(self, candidate: Path, writer: Path) -> None:
        pid = self.descendant_pid()
        self.assertTrue(alive(pid))
        self.assertTrue(exclusive_refused(candidate))  # check.py and its controller have gone
        first = writer.stat().st_size
        wait_until(lambda: writer.stat().st_size > first, "the descendant to keep writing")
        (self.hold / "finish").touch()
        wait_until(lambda: not alive(pid), "the detached writer to exit")
        self.gone(candidate)

    def test_01_a_candidate_started_by_the_pre_s12a_controller_holds_its_lease_to_the_last_descendant(
        self,
    ) -> None:
        candidate = self.candidate()
        writer = candidate / "target/progress"
        log = self.base / "ci.log"
        code = f"import sys\nsys.argv = ['x', {str(candidate)!r}, {str(self.ci_hub)!r}, {str(log)!r}, '--quick']\n"
        launcher = self.child(code + PRE_S12A_LAUNCHER, env=self.hook_environment(writer))
        wait_until((self.hold / "ready").exists, "check.py to reach its second step")
        self.assertTrue(
            exclusive_refused(candidate)
        )  # held from the first step on (see the log below)
        (self.hold / "release").touch()
        self.assertEqual(Lines(self, launcher).read(), "exit 0")  # the candidate passed
        output = log.read_text()
        self.assertIn(f"check: candidate lease held on {candidate}", output)
        self.assertLess(
            output.index("candidate lease held"),
            output.index("+ cargo fmt"),
            "leased before step 1",
        )
        self.assert_writer_keeps_the_lease_then_ends(candidate, writer)

    def test_02_a_candidate_renamed_before_check_starts_exits_nonzero_before_any_step(self) -> None:
        candidate = self.candidate()
        gone = self.base / "candidate.collected"
        driver = self.child(
            f"""
            import os, sys
            sys.path.insert(0, {str(candidate / "tools")!r})
            sys.argv = ['check.py', '--quick']
            import check
            os.rename({str(candidate)!r}, {str(gone)!r})  # collected before main() can lease it
            sys.exit(check.main())
            """,
            env=clean_environment(),
        )
        output, errors = driver.communicate(timeout=BOUND)
        self.assertEqual(driver.returncode, 1, errors)
        self.assertIn(b"cannot lease this checkout", errors)
        self.assertNotIn(b"+ ", output)  # no step ran
        self.assertNotIn(b"check: step", output)

    def test_03_a_failed_candidate_check_leaves_its_descendants_holding_the_lease(self) -> None:
        candidate = self.candidate(bootstrap=False)  # ShellCheck's coverage guard fails the run
        writer = candidate / "target/progress"
        log = self.base / "ci.log"
        outcome: dict[str, object] = {}
        manager = batch.Manager(self.ci_hub)

        def run_check() -> None:
            fd = os.open(self.ci_hub / ".pipeline/ci.lock", os.O_RDWR)
            try:
                outcome["passed"] = manager._full_check(candidate, log, fd)
            except BaseException as error:  # reported to the test thread
                outcome["error"] = error
            finally:
                os.close(fd)

        with mock.patch.dict(os.environ, self.hook_environment(writer)):
            thread = threading.Thread(target=run_check)
            thread.start()
            wait_until(
                (self.hold / "ready").exists, "the candidate's check to reach its second step"
            )
            (self.hold / "release").touch()
            thread.join(timeout=BOUND * 4)
        self.assertFalse(thread.is_alive())
        self.assertEqual(outcome, {"passed": False})  # the controller records `failed` and moves on
        self.assertIn("check: candidate lease held", log.read_text())
        self.assert_writer_keeps_the_lease_then_ends(candidate, writer)


class WorkerOwnership(HubFixture):
    """A `worker` builds in a private container and takes the destination name only by an exclusive
    rename; it never creates, opens or removes the destination any other way."""

    def setUp(self) -> None:
        super().setUp()
        self.first = self.repo("first", {"a.txt": "one\n"})
        self.publish(self.first)
        self.workers = self.hub / ".pipeline/workers"
        self.destination = self.workers / "w1"

    def run_worker(self) -> int:
        with contextlib.redirect_stdout(io.StringIO()):
            return cli.run(self.base, self.hub, ["worker", "w1"])

    def competitor_after_the_clone(
        self, populate: Callable[[Path], object]
    ) -> AbstractContextManager[object]:
        """The real clone runs, then a competing operation makes the destination (the race)."""
        real = self.pipeline.clone

        def racing(source: Path, target: Path, hub: Path) -> None:
            real(source, target, hub)  # the real Pijul clone, inside this call's private container
            self.destination.mkdir(parents=True)
            populate(self.destination)

        return mock.patch.object(self.pipeline, "clone", racing)

    def failing_hooks(
        self, before: Callable[[], object] | None = None
    ) -> AbstractContextManager[object]:
        real: Callable[..., str] = self.pipeline.pijul

        def failing(repo: Path, *arguments: str | Path) -> str:
            if arguments[:1] == ("hooks",):
                if before:
                    before()
                raise RuntimeError("hooks approve failed")
            return real(repo, *arguments)

        return mock.patch.object(self.pipeline, "pijul", failing)

    def test_01_a_directory_made_by_a_competitor_survives_with_its_contents(self) -> None:
        patch = self.competitor_after_the_clone(
            lambda path: (path / "other-owner.txt").write_text("someone else's work\n")
        )
        with patch, self.assertRaises(RuntimeError) as caught:
            self.run_worker()
        self.assertIn("already exists", str(caught.exception))
        self.assertEqual(
            (self.destination / "other-owner.txt").read_text(), "someone else's work\n"
        )
        self.assertEqual(
            [path.name for path in self.workers.iterdir()], ["w1"]
        )  # no container left

    def test_02_an_empty_directory_made_by_a_competitor_is_not_replaced(self) -> None:
        # os.rename would silently replace an empty directory; the exclusive rename refuses.
        with self.competitor_after_the_clone(lambda path: None), self.assertRaises(RuntimeError):
            self.run_worker()
        self.assertEqual(list(self.destination.iterdir()), [])
        self.assertEqual([path.name for path in self.workers.iterdir()], ["w1"])

    def test_03_a_failure_in_setup_removes_the_container_and_never_creates_the_destination(
        self,
    ) -> None:
        other = self.workers / "someone-else"
        other.mkdir(parents=True)
        (other / "keep.txt").write_text("keep\n")
        with self.failing_hooks(), self.assertRaises(RuntimeError):
            self.run_worker()
        self.assertFalse(self.destination.exists())
        self.assertEqual((other / "keep.txt").read_text(), "keep\n")
        self.assertEqual(sorted(path.name for path in self.workers.iterdir()), ["someone-else"])

    def test_04_a_clone_that_fails_midway_leaves_nothing_behind(self) -> None:
        def partial(source: Path, target: Path, hub: Path) -> None:
            target.mkdir(parents=True)
            (target / "half-written").write_text("x")
            raise RuntimeError("clone failed")

        with mock.patch.object(self.pipeline, "clone", partial), self.assertRaises(RuntimeError):
            self.run_worker()
        self.assertEqual(list(self.workers.iterdir()), [])

    def test_05_a_destination_that_exists_beforehand_is_refused_untouched(self) -> None:
        self.destination.mkdir(parents=True)
        (self.destination / "mine.txt").write_text("mine\n")
        with self.assertRaises(RuntimeError) as caught:
            self.run_worker()
        self.assertIn("already exists", str(caught.exception))
        self.assertEqual((self.destination / "mine.txt").read_text(), "mine\n")

    def test_06_a_dangling_symlink_at_the_destination_is_refused_untouched(self) -> None:
        self.workers.mkdir(parents=True)
        self.destination.symlink_to(self.base / "nowhere")
        with self.assertRaises(RuntimeError):
            self.run_worker()
        self.assertTrue(self.destination.is_symlink())
        self.assertEqual([path.name for path in self.workers.iterdir()], ["w1"])

    def test_07_an_empty_directory_made_at_the_destination_during_cleanup_survives(self) -> None:
        # The reviewer's final-name probe: the visible name is replaced by an EMPTY directory exactly
        # when the failure cleanup runs. No code path removes the destination name, so it survives.
        def competitor() -> None:
            self.destination.mkdir()

        with self.failing_hooks(before=competitor), self.assertRaises(RuntimeError):
            self.run_worker()
        self.assertTrue(self.destination.is_dir())
        self.assertEqual(list(self.destination.iterdir()), [])
        self.assertEqual([path.name for path in self.workers.iterdir()], ["w1"])

    def test_08_all_setup_happens_in_the_private_container_before_the_name_exists(self) -> None:
        seen: dict[str, object] = {}
        real: Callable[..., str] = self.pipeline.pijul

        def watching(repo: Path, *arguments: str | Path) -> str:
            if arguments[:1] == ("hooks",):
                seen["parent"] = Path(repo).parent.name
                seen["destination_exists"] = self.destination.exists()
                seen["container_mode"] = Path(repo).parent.stat().st_mode & 0o777
            return real(repo, *arguments)

        with mock.patch.object(self.pipeline, "pijul", watching):
            self.run_worker()
        parent = seen["parent"]
        assert isinstance(parent, str)
        self.assertTrue(parent.startswith(".w1-"), seen)
        self.assertFalse(seen["destination_exists"])
        self.assertEqual(seen["container_mode"], 0o700)
        self.assertEqual([path.name for path in self.workers.iterdir()], ["w1"])  # container gone

    def test_09_the_promoted_worker_is_complete_and_its_hooks_stay_approved(self) -> None:
        self.run_worker()
        self.assertEqual((self.destination / "a.txt").read_text(), "one\n")
        self.assertEqual((self.destination / ".ck-hub").read_text().strip(), str(self.hub))
        log = subprocess.run(
            ["pijul", "log"], cwd=self.destination, capture_output=True, text=True, check=True
        )
        self.assertTrue(log.stdout.strip())
        shown = subprocess.run(
            ["pijul", "hooks", "show"],
            cwd=self.destination,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotIn("not approved", (shown.stdout + shown.stderr).lower())

    def test_10_the_threat_model_is_stated_where_the_container_is_implemented(self) -> None:
        text = " ".join((owned.__doc__ or "").split())
        self.assertIn("same-user process that deliberately races on the private", text)
        self.assertIn("out of scope", text)


class ContainerPrimitive(LeaseFixture):
    """The private container: contents removed through its descriptor, promotion by exclusive rename."""

    def container(self) -> owned.Container:
        container = owned.Container(self.base, ".c-")
        self.addCleanup(container.close)
        return container

    def test_01_a_tree_is_removed_through_the_descriptor_without_following_symlinks(self) -> None:
        outside = self.directory("outside")
        (outside / "precious.txt").write_text("keep\n")
        container = self.container()
        tree = container.path / "tree"
        (tree / "a/b").mkdir(parents=True)
        (tree / "a/b/file.txt").write_text("x")
        (tree / "link").symlink_to(outside)
        (tree / "dirlink").symlink_to(outside, target_is_directory=True)
        container.discard()
        self.assertFalse(container.path.exists())
        self.assertEqual((outside / "precious.txt").read_text(), "keep\n")  # never followed

    def test_02_the_container_is_private_and_unpredictable(self) -> None:
        first, second = self.container(), self.container()
        self.assertEqual(first.path.stat().st_mode & 0o777, 0o700)
        self.assertNotEqual(first.path, second.path)

    def test_03_promotion_claims_only_a_free_name(self) -> None:
        container = self.container()
        (container.path / "repo").mkdir()
        (container.path / "repo/file.txt").write_text("built\n")
        container.promote("repo", "result")
        self.assertEqual((self.base / "result/file.txt").read_text(), "built\n")
        (container.path / "repo").mkdir()
        with self.assertRaises(FileExistsError):
            container.promote("repo", "result")
        self.assertTrue((container.path / "repo").is_dir())  # a refused rename moves nothing
        (self.base / "empty").mkdir()
        with self.assertRaises(FileExistsError):  # even an empty directory is not replaced
            container.promote("repo", "empty")

    def test_04_a_container_whose_name_was_taken_over_is_emptied_but_the_name_is_left(self) -> None:
        container = self.container()
        (container.path / "stuff.txt").write_text("x")
        moved = self.base / "moved"
        container.path.rename(moved)
        container.path.mkdir()  # empty: only the identity check keeps the final rmdir off it
        container.discard()
        self.assertTrue(container.path.is_dir())
        self.assertEqual(list(moved.iterdir()), [])  # what we held was emptied wherever it went

    def test_05_closing_twice_is_harmless(self) -> None:
        container = self.container()
        container.discard()
        container.close()
        container.close()
        container.discard()

    def test_06_the_exclusive_rename_claims_only_a_free_name(self) -> None:
        source, target = self.base / "src", self.base / "dst"
        source.mkdir()
        lease.rename_exclusive(source, target)
        self.assertTrue(target.is_dir() and not source.exists())
        source.mkdir()
        with self.assertRaises(FileExistsError):
            lease.rename_exclusive(source, target)
        self.assertTrue(source.is_dir())


class AcrossExec(LeaseFixture):
    """B2: leases survive exec and import, so every generation passes them on."""

    def test_01_a_lease_reaches_a_grandchild_started_through_the_helper_twice(self) -> None:
        path = self.directory()
        mark = f"# {self.base}\n"  # lets the fixture find every generation by its command line
        grand = (
            mark + "import os, sys, time\n"
            f"sys.path.insert(0, {str(TOOLS)!r})\n"
            "from gate import lease\n"
            "print('grandchild', os.getpid(), [str(p) for _, p in lease.held()], flush=True)\n"
            "time.sleep(600)"
        )
        middle = (
            mark + "import os, sys\n"
            f"sys.path.insert(0, {str(TOOLS)!r})\n"
            "from gate import lease\n"
            "print('middle', os.getpid(), [str(p) for _, p in lease.held()], flush=True)\n"
            f"lease.run([sys.executable, '-c', {grand!r}])"
        )
        parent = self.child(
            f"""
            import os, sys
            sys.path.insert(0, {str(TOOLS)!r})
            from gate import lease
            lease.acquire({str(path)!r})
            print('parent', os.getpid(), flush=True)
            lease.run([sys.executable, '-c', {middle!r}])
            """
        )
        lines = Lines(self, parent)
        parent_pid = int(lines.read().split()[1])
        middle_line = lines.read()
        grand_line = lines.read()
        self.assertTrue(middle_line.startswith("middle"), middle_line)
        self.assertIn(str(path), middle_line)  # the middle Python reports the inherited lease
        self.assertIn(str(path), grand_line)  # and so does the grandchild, a generation later
        middle_pid, grand_pid = int(middle_line.split()[1]), int(grand_line.split()[1])
        self.pids.extend([parent_pid, middle_pid, grand_pid])
        self.assertTrue(exclusive_refused(path))
        os.kill(parent_pid, signal.SIGKILL)
        os.kill(middle_pid, signal.SIGKILL)
        wait_until(lambda: not alive(parent_pid) and not alive(middle_pid), "the ancestors to die")
        self.assertTrue(alive(grand_pid))
        self.assertTrue(exclusive_refused(path), "the lease was lost with the ancestors")
        os.kill(grand_pid, signal.SIGKILL)
        wait_until(lambda: not alive(grand_pid), "the grandchild to exit")
        wait_until(lambda: not exclusive_refused(path), "the lease to be released")

    def test_02_an_announcement_that_does_not_match_its_descriptor_is_ignored(self) -> None:
        path, other = self.directory("one"), self.directory("two")
        descriptor = lease.acquire(path)
        self.addCleanup(lease.release, descriptor)
        info = path.stat()
        probe = f"""
            import sys
            sys.path.insert(0, {str(TOOLS)!r})
            from gate import lease
            print(len(lease.held()), flush=True)
            """
        for label, entry in {
            "right inode": [descriptor, info.st_dev, info.st_ino, str(path)],
            "wrong inode": [descriptor, info.st_dev, other.stat().st_ino, str(path)],
            "closed descriptor": [descriptor + 40, info.st_dev, info.st_ino, str(path)],
            "not a list entry": "garbage",
        }.items():
            with self.subTest(label):
                process = self.child(
                    probe,
                    pass_fds=(descriptor,),
                    env=clean_environment(CK_LEASE_FDS=json.dumps([entry])),
                )
                output, _ = process.communicate(timeout=BOUND)
                self.assertEqual(int(output), 1 if label == "right inode" else 0, label)

    def test_03_a_daemon_launch_withholds_the_announcement_as_well_as_the_descriptors(self) -> None:
        path = self.directory()
        descriptor = lease.acquire(path)
        self.addCleanup(lease.release, descriptor)
        code = "import os; print(os.environ.get('CK_LEASE_FDS', 'none'))"
        shown = lease.check_output([sys.executable, "-c", code], text=True, inherit=False).strip()
        self.assertEqual(shown, "none")
        announced = lease.check_output([sys.executable, "-c", code], text=True).strip()
        self.assertIn(str(path), announced)


class GroupCleanup(LeaseFixture):
    """B3: cleanup ends the whole group, even a member that ignores SIGTERM, before it returns."""

    # The pids are published only after the grandchild is ignoring SIGTERM (it says so with a
    # `.ready` file): a test that signals the driver at once must not race the grandchild's start-up.
    LEADER = (
        "import os, subprocess, sys, time\n"
        "ready = sys.argv[1] + '.ready'\n"
        "grandchild = subprocess.Popen([sys.executable, '-c', "
        "'# ' + sys.argv[1] + '\\nimport signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); "
        "open(' + repr(ready) + ', \"w\").close(); time.sleep(600)'],"
        " pass_fds=[int(fd) for fd in sys.argv[2:]])\n"
        "while not os.path.exists(ready):\n"
        "    time.sleep(0.01)\n"
        "open(sys.argv[1], 'w').write(f'{os.getpid()} {grandchild.pid}')\n"
        "time.sleep(600)"
    )

    def setUp(self) -> None:
        super().setUp()
        self.path = self.directory()
        self.marker = self.base / "pids"
        patch = mock.patch.multiple(lease, GRACE=0.3, KILL_WAIT=5.0)
        patch.start()
        self.addCleanup(patch.stop)
        self.descriptor = lease.acquire(self.path)
        self.addCleanup(lease.release, self.descriptor)

    def pids_of_the_group(self) -> tuple[int, int]:
        wait_until(self.marker.exists, "the leader and its grandchild")
        wait_until(lambda: self.marker.read_text().count(" ") == 1, "both pids")
        leader, grandchild = map(int, self.marker.read_text().split())
        self.pids.extend([leader, grandchild])
        return leader, grandchild

    def assert_group_gone(self, leader: int, grandchild: int, started: float) -> None:
        self.assertLess(time.monotonic() - started, 10, "cleanup took too long")
        self.assertFalse(alive(leader))
        self.assertFalse(alive(grandchild), "the SIGTERM-ignoring member survived the cleanup")
        self.assertFalse(lease.group_alive(leader))
        lease.release(self.descriptor)  # the test's own copy; the lease must now be free
        self.assertFalse(exclusive_refused(self.path), "a descendant still holds the lease")

    def command(self) -> list[str]:
        return [sys.executable, "-c", self.LEADER, str(self.marker), str(self.descriptor)]

    def test_01_a_run_timeout_kills_a_member_that_ignores_sigterm(self) -> None:
        started = time.monotonic()
        with self.assertRaises(subprocess.TimeoutExpired):
            lease.run(self.command(), timeout=1, pass_fds=(self.descriptor,))
        self.assert_group_gone(*self.pids_of_the_group(), started)

    def test_02_a_check_output_timeout_follows_the_same_contract(self) -> None:
        started = time.monotonic()
        with self.assertRaises(subprocess.TimeoutExpired):
            lease.check_output(self.command(), timeout=1, pass_fds=(self.descriptor,))
        self.assert_group_gone(*self.pids_of_the_group(), started)

    def test_03_an_interruption_ends_the_whole_group_before_it_propagates(self) -> None:
        def interrupted(*arguments: object, **keywords: object) -> NoReturn:
            self.pids_of_the_group()
            raise KeyboardInterrupt

        started = time.monotonic()
        with (
            mock.patch.object(subprocess.Popen, "communicate", interrupted),
            self.assertRaises(KeyboardInterrupt),
        ):
            lease.run(self.command(), pass_fds=(self.descriptor,))
        self.assert_group_gone(*self.pids_of_the_group(), started)

    def test_04_a_leader_that_exits_first_does_not_end_the_cleanup_early(self) -> None:
        # The leader dies on SIGTERM at once; the member that ignores it must still be killed.
        started = time.monotonic()
        with self.assertRaises(subprocess.TimeoutExpired):
            lease.run(self.command(), timeout=1, pass_fds=(self.descriptor,))
        leader, grandchild = self.pids_of_the_group()
        self.assertFalse(alive(grandchild))
        self.assert_group_gone(leader, grandchild, started)

    def test_05_a_member_that_cannot_be_killed_is_an_error_not_a_silent_success(self) -> None:
        with (
            mock.patch.multiple(lease, GRACE=0.1, KILL_WAIT=0.3),
            mock.patch.object(lease, "group_alive", return_value=True),
            self.assertRaises(RuntimeError) as caught,
        ):
            lease.run(self.command(), timeout=1, pass_fds=(self.descriptor,))
        self.assertIn("could not end the process group", str(caught.exception))
        for pid in self.pids_of_the_group():
            with contextlib.suppress(ProcessLookupError):  # the real kill had already reached them
                os.kill(pid, signal.SIGKILL)


class SignalSafeCleanup(LeaseFixture):
    """R2: while a child is owned, signals are recorded; waits are bounded; the group always dies first."""

    LEADER = GroupCleanup.LEADER

    def setUp(self) -> None:
        super().setUp()
        self.path = self.directory()
        self.marker = self.base / "pids"

    def driver(self, grace: float, body: str) -> subprocess.Popen[bytes]:
        """A driver process that owns the leader and its SIGTERM-ignoring leased grandchild."""
        header = "\n".join(
            [
                f"# {self.base}",
                "import os, signal, sys",
                f"sys.path.insert(0, {str(TOOLS)!r})",
                "from gate import lease",
                f"lease.GRACE = {grace}",
                f"descriptor = lease.acquire({str(self.path)!r})",
                "print('ready', os.getpid(), flush=True)",
            ]
        )
        return self.child(header + "\n" + body)

    def start(self, grace: float = 3.0) -> tuple[subprocess.Popen[bytes], Lines, int, int, int]:
        run = "\n".join(
            [
                "try:",
                f"    lease.run([sys.executable, '-c', {GroupCleanup.LEADER!r}, {str(self.marker)!r}, str(descriptor)])",
                "except BaseException as error:",
                "    print('raised', type(error).__name__, flush=True)",
            ]
        )
        process = self.driver(grace, run)
        lines = Lines(self, process)
        driver_pid = int(lines.read().split()[1])
        self.pids.append(driver_pid)
        wait_until(
            lambda: self.marker.exists() and self.marker.read_text().count(" ") == 1, "the pids"
        )
        leader, grandchild = map(int, self.marker.read_text().split())
        self.pids.extend([leader, grandchild])
        return process, lines, driver_pid, leader, grandchild

    def test_01_a_second_sigint_during_the_cleanup_cannot_leave_the_leased_grandchild_alive(
        self,
    ) -> None:
        process, lines, driver_pid, leader, grandchild = self.start(grace=3.0)
        os.kill(driver_pid, signal.SIGINT)
        wait_until(
            lambda: not alive(leader), "the first interrupt to take effect (cleanup running)"
        )
        self.assertTrue(alive(grandchild))  # ignores SIGTERM: the grace period is still running
        os.kill(driver_pid, signal.SIGINT)  # the second Ctrl-C, during the cleanup
        self.assertEqual(lines.read(), "raised KeyboardInterrupt")  # the first cause, after cleanup
        self.assertEqual(process.wait(timeout=BOUND), 0)
        self.assertFalse(
            alive(grandchild), "the second interrupt bypassed the remaining group kill"
        )
        wait_until(lambda: not exclusive_refused(self.path), "the lease to be released")

    def test_02_a_sigterm_ends_the_group_and_then_exits_with_the_terminated_status(self) -> None:
        process, lines, driver_pid, leader, grandchild = self.start(grace=0.3)
        os.kill(driver_pid, signal.SIGTERM)
        self.assertEqual(lines.read(), "raised SystemExit")
        self.assertEqual(process.wait(timeout=BOUND), 0)
        self.assertFalse(alive(leader) or alive(grandchild))
        wait_until(lambda: not exclusive_refused(self.path), "the lease to be released")

    def test_03_a_live_leader_that_cannot_be_killed_is_a_bounded_error_naming_it(self) -> None:
        # The reviewer's fault injection: the signalling attempts fail, so the real child stays alive.
        started = time.monotonic()
        with (
            mock.patch.multiple(lease, GRACE=0.1, KILL_WAIT=0.2),
            mock.patch.object(os, "killpg", side_effect=PermissionError),
            self.assertRaises(RuntimeError) as caught,
        ):
            lease.run(
                [sys.executable, "-c", f"# {self.base}\nimport time; time.sleep(8)"], timeout=0.1
            )
        self.assertLess(time.monotonic() - started, 5, "the cleanup waited on a live leader")
        self.assertIn("could not end the process group", str(caught.exception))
        self.assertIn("leader still running", str(caught.exception))

    def test_04_the_signal_handlers_are_restored_and_a_worker_thread_still_runs_children(
        self,
    ) -> None:
        before = (signal.getsignal(signal.SIGINT), signal.getsignal(signal.SIGTERM))
        result = lease.run([sys.executable, "-c", "print('ok')"], capture_output=True, text=True)
        self.assertEqual(result.stdout.strip(), "ok")
        self.assertEqual(
            (signal.getsignal(signal.SIGINT), signal.getsignal(signal.SIGTERM)), before
        )
        outcome = []
        thread = threading.Thread(
            target=lambda: outcome.append(lease.run([sys.executable, "-c", "pass"], check=True))
        )
        thread.start()
        thread.join(timeout=BOUND)
        self.assertEqual(
            len(outcome), 1
        )  # handlers cannot be installed off the main thread; no error


class CauseLifecycle(LeaseFixture):
    """One lifecycle for the owned group, the recorded cause and the handlers' restoration: a cause
    recorded at any point is raised, after the whole group is ended, and never lost."""

    LEADER = GroupCleanup.LEADER

    def setUp(self) -> None:
        super().setUp()
        self.path = self.directory()
        self.marker = self.base / "pids"

    def driver(self, body: str, grace: float = 0.3) -> tuple[subprocess.Popen[bytes], Lines]:
        header = "\n".join(
            [
                f"# {self.base}",
                "import os, signal, sys",
                f"sys.path.insert(0, {str(TOOLS)!r})",
                "from gate import lease",
                f"lease.GRACE = {grace}",
                f"descriptor = lease.acquire({str(self.path)!r})",
                "print('ready', os.getpid(), flush=True)",
            ]
        )
        process = self.child(header + "\n" + textwrap.dedent(body))
        lines = Lines(self, process)
        self.pids.append(int(lines.read().split()[1]))
        return process, lines

    def leader_call(self) -> str:
        return (
            "lease.run([sys.executable, '-c', "
            f"{GroupCleanup.LEADER!r}, {str(self.marker)!r}, str(descriptor)])"
        )

    def grandchild(self) -> tuple[int, int]:
        wait_until(
            lambda: self.marker.exists() and self.marker.read_text().count(" ") == 1, "the pids"
        )
        leader, grandchild = map(int, self.marker.read_text().split())
        self.pids.extend([leader, grandchild])
        return leader, grandchild

    def test_01_a_signal_between_the_last_decision_and_the_restoration_is_raised(self) -> None:
        for number, expected in (("SIGINT", "KeyboardInterrupt"), ("SIGTERM", "SystemExit")):
            with self.subTest(number):
                process, lines = self.driver(
                    f"""
                    real = lease._leave
                    def hooked():  # a real signal arrives after the decision, before restoring
                        os.kill(os.getpid(), signal.{number})
                        real()
                    lease._leave = hooked
                    try:
                        result = lease.run([sys.executable, '-c', 'pass'])
                        print('returned', result.returncode, flush=True)
                    except BaseException as error:
                        print('raised', type(error).__name__, flush=True)
                    """
                )
                self.assertEqual(lines.read(), f"raised {expected}")
                self.assertEqual(process.wait(timeout=BOUND), 0)

    def test_02_a_signal_after_the_leader_exited_still_ends_the_leased_grandchild(self) -> None:
        for hook in ("lease._close", "lease.end_group"):
            with self.subTest(hook):
                self.marker.unlink(missing_ok=True)
                name = hook.split(".")[1]
                process, lines = self.driver(
                    f"""
                    real = {hook}
                    def hooked(*args, **kwargs):  # a real SIGINT once the leader has completed
                        os.kill(os.getpid(), signal.SIGINT)
                        return real(*args, **kwargs)
                    lease.{name} = hooked
                    leader = (
                        "import os, subprocess, sys; "
                        "g = subprocess.Popen([sys.executable, '-c', 'import signal, time; "
                        "signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(600)'], "
                        "pass_fds=[int(sys.argv[2])], stdin=subprocess.DEVNULL, "
                        "stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); "
                        "open(sys.argv[1], 'w').write(f'{{os.getpid()}} {{g.pid}}')"
                    )
                    try:
                        lease.run([sys.executable, '-c', leader, {str(self.marker)!r}, str(descriptor)],
                                  stdout=-3, stderr=-3)
                    except BaseException as error:
                        print('raised', type(error).__name__, flush=True)
                    """
                )
                self.assertEqual(lines.read(), "raised KeyboardInterrupt")
                self.assertEqual(process.wait(timeout=BOUND), 0)
                _leader, grandchild = self.grandchild()
                self.assertFalse(alive(grandchild), f"the group was not ended ({hook})")
                wait_until(lambda: not exclusive_refused(self.path), "the lease to be released")

    def test_03_a_leader_that_exits_normally_does_not_leave_a_member_behind(self) -> None:
        process, lines = self.driver(
            f"""
            leader = (
                "import os, subprocess, sys; "
                "g = subprocess.Popen([sys.executable, '-c', 'import signal, time; "
                "signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(600)'], "
                "pass_fds=[int(sys.argv[2])], stdin=subprocess.DEVNULL, "
                "stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL); "
                "open(sys.argv[1], 'w').write(f'{{os.getpid()}} {{g.pid}}')"
            )
            result = lease.run([sys.executable, '-c', leader, {str(self.marker)!r}, str(descriptor)])
            print('returned', result.returncode, flush=True)
            """
        )
        self.assertEqual(lines.read(), "returned 0")
        self.assertEqual(process.wait(timeout=BOUND), 0)
        _leader, grandchild = self.grandchild()
        self.assertFalse(alive(grandchild))
        wait_until(lambda: not exclusive_refused(self.path), "the lease to be released")

    def test_04_a_cause_recorded_before_the_spawn_starts_nothing(self) -> None:
        process, lines = self.driver(
            """
            real = lease._install
            def hooked():  # a real SIGINT as the handlers go in, before the spawn
                real()
                os.kill(os.getpid(), signal.SIGINT)
            lease._install = hooked
            started = lease.subprocess.Popen
            def spawning(*args, **kwargs):
                print('spawned', flush=True)
                return started(*args, **kwargs)
            lease.subprocess.Popen = spawning
            try:
                lease.run([sys.executable, '-c', 'pass'])
            except BaseException as error:
                print('raised', type(error).__name__, flush=True)
            """
        )
        self.assertEqual(lines.read(), "raised KeyboardInterrupt")  # no 'spawned' line before it
        self.assertEqual(process.wait(timeout=BOUND), 0)

    def test_05_after_a_cause_the_handlers_stay_record_only_and_the_first_cause_wins(self) -> None:
        process, lines = self.driver(
            """
            real = lease._leave
            def hooked():
                os.kill(os.getpid(), signal.SIGTERM)
                real()
            lease._leave = hooked
            try:
                lease.run([sys.executable, '-c', 'pass'])
            except BaseException as error:
                print('raised', type(error).__name__, flush=True)
            lease._leave = real
            print('record-only', signal.getsignal(signal.SIGINT) is lease._on_interrupt, flush=True)
            os.kill(os.getpid(), signal.SIGINT)  # unwinding is not interrupted by a later signal
            print('survived', lease._causes, flush=True)
            print('cleanup child', lease.run([sys.executable, '-c', 'pass']).returncode, flush=True)
            """
        )
        self.assertEqual(lines.read(), "raised SystemExit")
        self.assertEqual(lines.read(), "record-only True")
        self.assertEqual(lines.read(), "survived ['terminated', 'interrupted']")
        self.assertEqual(lines.read(), "cleanup child 0")  # cleanup tools still run
        self.assertEqual(process.wait(timeout=BOUND), 0)

    def test_05b_a_signal_inside_the_restoration_is_held_until_the_handlers_are_settled(
        self,
    ) -> None:
        # The reviewer's probe: a real SIGTERM inside the first restoring call. Masked, it stays
        # pending and meets the previous (default) handler once they are restored: the process ends
        # with the terminated status, so a later SIGINT cannot replace it.
        process, lines = self.driver(
            """
            real = lease.signal.signal
            fired = []
            def watching(number, handler):
                if not fired and handler not in (lease._on_interrupt, lease._on_terminate):
                    fired.append(1)  # the first call that restores a previous handler
                    os.kill(os.getpid(), signal.SIGTERM)
                return real(number, handler)
            lease.signal.signal = watching
            try:
                lease.run([sys.executable, '-c', 'pass'])
            except BaseException as error:
                print('raised', type(error).__name__, flush=True)
            lease.signal.signal = real
            print('unwinding', flush=True)
            os.kill(os.getpid(), signal.SIGINT)
            print('survived', flush=True)
            """
        )
        self.assertEqual(
            process.wait(timeout=BOUND), -signal.SIGTERM
        )  # 143, not a KeyboardInterrupt
        self.assertEqual(lines.seen, ["ready " + str(self.pids[-1])])

    def test_05c_a_signal_inside_the_restoration_reaches_the_previous_handler_exactly_once(
        self,
    ) -> None:
        process, lines = self.driver(
            """
            got = []
            signal.signal(signal.SIGTERM, lambda *a: got.append('terminated'))
            real = lease.signal.signal
            fired = []
            def watching(number, handler):
                if not fired and handler not in (lease._on_interrupt, lease._on_terminate):
                    fired.append(1)
                    os.kill(os.getpid(), signal.SIGTERM)
                return real(number, handler)
            lease.signal.signal = watching
            lease.run([sys.executable, '-c', 'pass'])
            lease.signal.signal = real
            print('handler calls', got, lease._causes, flush=True)
            print('mask', sorted(signal.pthread_sigmask(signal.SIG_BLOCK, [])), flush=True)
            """
        )
        self.assertEqual(lines.read(), "handler calls ['terminated'] []")
        self.assertEqual(lines.read(), "mask []")  # the signal mask is exactly as it was
        self.assertEqual(process.wait(timeout=BOUND), 0)

    def test_06_the_first_of_two_causes_during_the_cleanup_decides(self) -> None:
        process, lines = self.driver(
            f"""
            try:
                {self.leader_call()}
            except BaseException as error:
                print('raised', type(error).__name__, flush=True)
            """,
            grace=3.0,
        )
        _leader, grandchild = self.grandchild()
        driver_pid = self.pids[-3]
        os.kill(driver_pid, signal.SIGTERM)  # first
        wait_until(lambda: not alive(_leader), "the cleanup to start")
        os.kill(driver_pid, signal.SIGINT)  # second, during the cleanup
        self.assertEqual(lines.read(), "raised SystemExit")
        self.assertEqual(process.wait(timeout=BOUND), 0)
        self.assertFalse(alive(grandchild))

    def test_07_without_a_cause_the_previous_handlers_are_restored_exactly(self) -> None:
        process, lines = self.driver(
            """
            def mine(signum, frame): pass
            signal.signal(signal.SIGINT, mine)
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            lease.run([sys.executable, '-c', 'pass'])
            print(signal.getsignal(signal.SIGINT) is mine,
                  signal.getsignal(signal.SIGTERM) is signal.SIG_IGN, flush=True)
            """
        )
        self.assertEqual(lines.read(), "True True")
        self.assertEqual(process.wait(timeout=BOUND), 0)


class SurvivingCiDescendants(test_batch.BatchGateSpecification):
    """Test 6 (CK-24@r2): the real `batch prepare` failure path with a real flocked `ci.lock`."""

    DETACHING_RUFF = """#!/bin/sh
if [ ! -e "$CK_HOLD/started" ]; then
    : > "$CK_HOLD/started"
    python3 -c "$CK_DETACH" "$CK_HOLD" "$CK_WRITE"
fi
exec /usr/bin/true "$@"
"""

    def populate_base(self) -> None:
        # The accepted base is a realistic tree, so the candidate has the real check.py and tools.
        copy_tools(self.base)
        for relative in ("pyproject.toml", "rust-toolchain.toml"):
            shutil.copy2(REPO / relative, self.base / relative)
        (self.base / "Cargo.toml").write_text('[workspace]\nresolver = "3"\nmembers = ["m"]\n')
        (self.base / "m/src").mkdir(parents=True)
        (self.base / "m/Cargo.toml").write_text(
            '[package]\nname = "m"\nversion = "0.1.0"\nedition = "2024"\n'
        )
        (self.base / "m/src/lib.rs").write_text("//! A fixture crate.\n")
        (self.base / "tools/bootstrap").unlink()  # ShellCheck's coverage guard then fails the run
        (self.base / ".ignore").write_text("/.pipeline\n/.ck-hub\n/target\n")

    def test_01_a_descendant_that_escaped_the_step_keeps_both_locks_after_the_batch_fails(
        self,
    ) -> None:
        hold = Path(self.temp.name) / "hold"
        hold.mkdir()
        tools_bin = self.hub / ".tools/python/bin"
        tools_bin.mkdir(parents=True)
        real_hub = Path(os.environ["CK_HUB"])
        for name in ("shellcheck", "mypy"):
            (tools_bin / name).symlink_to(real_hub / ".tools/python/bin" / name)
        (tools_bin / "ruff").write_text(self.DETACHING_RUFF)
        (tools_bin / "ruff").chmod(0o755)
        self.addCleanup((hold / "finish").touch)
        self.submission("probe")
        environment = {
            "CK_HOLD": str(hold),
            "CK_DETACH": DETACH,
            "CK_WRITE": "target/progress",  # relative to the candidate, the step's working directory
        }
        with (
            mock.patch.dict(os.environ, environment),
            contextlib.redirect_stdout(io.StringIO()),  # prepare reports what it is checking
            self.assertRaises(RuntimeError) as caught,
        ):
            batch.Manager(self.hub).prepare()
        self.assertIn("No submission group passed full CI", str(caught.exception))
        # The controller recorded the terminal failure and closed its own ci.lock descriptor ...
        [failed] = list((self.hub / ".pipeline/batches").glob("*/failed.json"))
        self.assertEqual(json.loads(failed.read_text())["reason"], "validation failed")
        [candidate] = list((self.hub / ".pipeline/checkpoints").glob("*-combined"))
        ci_lock = self.hub / ".pipeline/ci.lock"
        # ... but the descendant inherited CK_CI_LOCK_FD and the candidate lease, so both stay held.
        wait_until((hold / "descendant.pid").exists, "the escaped descendant")
        wait_until(lambda: (hold / "descendant.pid").read_text() != "", "its pid")
        pid = int((hold / "descendant.pid").read_text())
        self.assertTrue(alive(pid))
        self.assertTrue(exclusive_refused(ci_lock), "ci.lock was free while a descendant lived")
        self.assertTrue(
            exclusive_refused(candidate), "the candidate was free while a descendant lived"
        )
        self.assertTrue(candidate.is_dir())  # nothing is collected meanwhile
        writer = candidate / "target/progress"
        first = writer.stat().st_size
        wait_until(lambda: writer.stat().st_size > first, "the descendant to keep writing")
        (hold / "finish").touch()
        wait_until(lambda: not alive(pid), "the descendant to exit")
        wait_until(lambda: not exclusive_refused(ci_lock), "ci.lock to be released")
        wait_until(lambda: not exclusive_refused(candidate), "the candidate lease to be released")


# The fixture is inherited for its hub, base and submissions; its own tests run in test_batch.py.
for _name in dir(test_batch.BatchGateSpecification):
    if _name.startswith("test") and _name not in SurvivingCiDescendants.__dict__:
        setattr(SurvivingCiDescendants, _name, None)


if __name__ == "__main__":
    unittest.main()
