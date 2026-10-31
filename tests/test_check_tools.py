"""S5: pinned tools, doctor enforcement, ShellCheck discovery and the Clang static analyzer.

Real processes where stated: doctor runs in a driver against a fixture hub whose tools are small
scripts (so a wrong or missing version is a fixture, not an uninstall), the ShellCheck step runs on a
disposable repository, and the analyzer runs in a real `cargo build` of a copy of the crate.
"""

import contextlib
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import textwrap
import time
import tomllib
import unittest
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import ClassVar, Protocol, cast
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import check

REPO = Path(__file__).resolve().parents[1]
TOOLS = REPO / "tools"
INSTALL = "tools/bootstrap"
BRIDGE_DEFECT = "\nint ck_analyzer_probe(void) {\n    int *p = NULL;\n    return *p;\n}\n"

DoctorProcessRow = tuple[int, int, int, str, str, str]
DOCTOR_POLL = 0.05
DOCTOR_PS_MIN_FIELDS = 9
DOCTOR_SAMPLE_LIMIT = 8


class _WaitIdInfo(Protocol):
    si_pid: int


class _WaitIdOS(Protocol):
    def waitid(self, idtype: int, id: int, options: int) -> _WaitIdInfo | None: ...


_waitid = cast(_WaitIdOS, os).waitid


@dataclass(slots=True)
class DoctorHarnessHooks:
    """Test seams for the timeout cleanup schedules; production runs use no hooks."""

    before_deadline: Callable[[int], None] | None = None
    after_discovery: Callable[[int, list[DoctorProcessRow]], None] | None = None
    during_wait: Callable[[int], None] | None = None
    before_force: Callable[[int], None] | None = None
    before_reap: Callable[[int], None] | None = None
    cooperate: bool = True


def _doctor_wait_unreaped(pid: int, timeout: float) -> _WaitIdInfo | None:
    """Wait for a child without reaping it, keeping its PID pinned as its session id."""
    deadline = time.monotonic() + timeout
    while True:
        info = _waitid(os.P_PID, pid, os.WEXITED | os.WNOWAIT | os.WNOHANG)
        if info is not None and info.si_pid:
            return info
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None
        time.sleep(min(DOCTOR_POLL, remaining))


def _doctor_session_rows(session: int) -> list[DoctorProcessRow]:
    """List one pinned session with PID identity, group, state and command diagnostics."""
    output = subprocess.run(
        ["ps", "-A", "-o", "pid=,ppid=,pgid=,lstart=,stat=,command="],
        capture_output=True,
        text=True,
        check=True,
        timeout=5,
    ).stdout
    rows: list[DoctorProcessRow] = []
    for line in output.splitlines():
        fields = line.split(None, 9)
        if len(fields) < DOCTOR_PS_MIN_FIELDS:
            continue
        pid, parent, group = (int(field) for field in fields[:3])
        try:
            owned = os.getsid(pid) == session
        except (ProcessLookupError, PermissionError):
            continue
        if owned:
            started = " ".join(fields[3:8])
            state = fields[8]
            command = fields[9] if len(fields) > DOCTOR_PS_MIN_FIELDS else ""
            rows.append((pid, parent, group, started, state, command))
    return sorted(rows)


def _doctor_stable_session(session: int) -> tuple[list[DoctorProcessRow], bool]:
    """Require two consecutive session-membership snapshots to agree, at most ten times."""
    previous: tuple[tuple[int, str], ...] | None = None
    rows: list[DoctorProcessRow] = []
    for _ in range(10):
        rows = _doctor_session_rows(session)
        identity = tuple((row[0], row[3]) for row in rows)
        if identity == previous:
            return rows, True
        previous = identity
        time.sleep(DOCTOR_POLL)
    return rows, False


def _doctor_is_live(row: DoctorProcessRow) -> bool:
    return not row[4].startswith("Z")


def _doctor_signal_owned(pid: int, session: int, sig: signal.Signals) -> str | None:
    """Signal one PID only after an immediately preceding session-ownership check."""
    try:
        if os.getsid(pid) != session:
            return f"pid {pid}: session changed before {sig.name}; not signalled"
        os.kill(pid, sig)
    except ProcessLookupError:
        return None
    except PermissionError as error:
        return f"pid {pid}: {sig.name} denied: {error}"
    return None


def _doctor_cleanup(  # noqa: PLR0912, PLR0915  # preserve the ordered cleanup protocol as one auditable state machine
    process: subprocess.Popen[bytes],
    *,
    diagnose: bool,
    hooks: DoctorHarnessHooks | None,
    run_hooks: bool = True,
) -> tuple[str, list[str]]:
    """Stop, verify and reap every live member while the direct child pins its session id."""
    session = process.pid
    notes: list[str] = []
    ended_by: dict[int, str] = {}
    child_exited = _doctor_wait_unreaped(session, 0)

    if child_exited is None:
        note = _doctor_signal_owned(session, session, signal.SIGSTOP)
        if note:
            notes.append(note)
        stop_deadline = time.monotonic() + 2
        while time.monotonic() < stop_deadline:
            rows = _doctor_session_rows(session)
            child = next((row for row in rows if row[0] == session), None)
            if child is None or "T" in child[4]:
                break
            time.sleep(DOCTOR_POLL)

    first_rows, stable = _doctor_stable_session(session)
    snapshots: list[list[DoctorProcessRow]] = [first_rows]
    if not stable:
        notes.append("session membership did not stabilize within ten snapshots")

    if diagnose:
        notes.append("initial stable session snapshot: " + repr(first_rows))
    if run_hooks and hooks is not None and hooks.after_discovery is not None:
        try:
            hooks.after_discovery(session, first_rows)
        except BaseException as error:
            notes.append(f"after-discovery hook failed: {type(error).__name__}: {error}")
        refreshed, refreshed_stable = _doctor_stable_session(session)
        snapshots.append(refreshed)
        if diagnose:
            notes.append("refreshed session snapshot: " + repr(refreshed))
        if not refreshed_stable:
            notes.append("refreshed session membership did not stabilize within ten snapshots")

    rows = snapshots[-1]
    if diagnose:
        sampled = 0
        sample_output: list[str] = []
        for row in sorted(rows, key=lambda item: (item[0] != session, item[0])):
            if not _doctor_is_live(row) or sampled >= DOCTOR_SAMPLE_LIMIT:
                continue
            sampled += 1
            try:
                result = subprocess.run(
                    ["/usr/bin/sample", str(row[0]), "1"],
                    capture_output=True,
                    text=True,
                    check=False,
                    timeout=10,
                )
                sample_output.append(
                    f"sample pid {row[0]} exit={result.returncode}:\n{result.stdout}{result.stderr}"
                )
            except (OSError, subprocess.TimeoutExpired) as error:
                sample_output.append(f"sample pid {row[0]} failed: {error!r}")
        notes.extend(sample_output)

    if child_exited is None and (hooks is None or hooks.cooperate):
        for sig in (signal.SIGTERM, signal.SIGCONT):
            note = _doctor_signal_owned(session, session, sig)
            if note:
                notes.append(note)
        _doctor_wait_unreaped(session, 5)
        after_cooperation = _doctor_session_rows(session)
        live_after_cooperation = {row[0] for row in after_cooperation if _doctor_is_live(row)}
        for snapshot in snapshots:
            for row in snapshot:
                if row[0] not in live_after_cooperation:
                    ended_by.setdefault(row[0], "cooperative SIGTERM/check.run cleanup")

    if run_hooks and hooks is not None and hooks.before_force is not None:
        try:
            hooks.before_force(session)
        except BaseException as error:
            notes.append(f"before-force hook failed: {type(error).__name__}: {error}")

    for round_number in range(1, 11):
        current = _doctor_session_rows(session)
        live = [row for row in current if _doctor_is_live(row)]
        if not live:
            break
        for row in live:
            note = _doctor_signal_owned(row[0], session, signal.SIGKILL)
            if note:
                notes.append(note)
        time.sleep(DOCTOR_POLL)
        remaining = {row[0] for row in _doctor_session_rows(session) if _doctor_is_live(row)}
        for row in live:
            if row[0] not in remaining:
                ended_by.setdefault(row[0], f"forced SIGKILL round {round_number}")

    verify_deadline = time.monotonic() + 5
    remaining_rows = _doctor_session_rows(session)
    while (
        any(_doctor_is_live(row) for row in remaining_rows) and time.monotonic() < verify_deadline
    ):
        time.sleep(DOCTOR_POLL)
        remaining_rows = _doctor_session_rows(session)
    remaining_live = [row for row in remaining_rows if _doctor_is_live(row)]
    if remaining_live:
        notes.append("cleanup verification found live session members: " + repr(remaining_live))
    elif diagnose:
        notes.append("cleanup verification: no live process remains in the child's session")
    if diagnose:
        notes.append("process termination steps: " + repr(ended_by))
        notes.append(
            "zombie session rows before reap: "
            + repr([r for r in remaining_rows if not _doctor_is_live(r)])
        )

    if run_hooks and hooks is not None and hooks.before_reap is not None:
        try:
            hooks.before_reap(session)
        except BaseException as error:
            notes.append(f"before-reap hook failed: {type(error).__name__}: {error}")

    exited = _doctor_wait_unreaped(session, 5)
    if exited is None:
        notes.append(f"child session leader {session} did not exit before bounded reap")
    else:
        deadline = time.monotonic() + 2
        reaped = False
        while time.monotonic() < deadline:
            try:
                child_pid, status = os.waitpid(session, os.WNOHANG)
            except ChildProcessError as error:
                notes.append(f"child {session} was reaped outside the harness: {error}")
                break
            if child_pid == session:
                process.returncode = os.waitstatus_to_exitcode(status)
                reaped = True
                break
            time.sleep(DOCTOR_POLL)
        if not reaped and not any("reaped outside" in note for note in notes):
            notes.append(f"child session leader {session} was not reaped within two seconds")
    return "\n".join(notes), notes


def doctor_process(
    command: list[str],
    env: dict[str, str],
    *,
    timeout: float = 120,
    hooks: DoctorHarnessHooks | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run a Doctor child in a pinned session and diagnose/clean up timeout descendants safely."""
    with tempfile.TemporaryDirectory(prefix="ck-doctor-session-") as directory:
        stdout_path, stderr_path = Path(directory) / "stdout", Path(directory) / "stderr"
        process: subprocess.Popen[bytes] | None = None
        timed_out = False
        try:
            with stdout_path.open("wb") as stdout_file, stderr_path.open("wb") as stderr_file:
                process = subprocess.Popen(
                    command,
                    env=env,
                    stdout=stdout_file,
                    stderr=stderr_file,
                    start_new_session=True,
                )
                if hooks is not None and hooks.before_deadline is not None:
                    hooks.before_deadline(process.pid)
                deadline = time.monotonic() + timeout
                while True:
                    info = _waitid(
                        os.P_PID,
                        process.pid,
                        os.WEXITED | os.WNOWAIT | os.WNOHANG,
                    )
                    if info is not None and info.si_pid:
                        break
                    if hooks is not None and hooks.during_wait is not None:
                        hooks.during_wait(process.pid)
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        timed_out = True
                        break
                    time.sleep(min(DOCTOR_POLL, remaining))
        except BaseException as error:
            if process is not None and process.returncode is None:
                cleanup, _ = _doctor_cleanup(process, diagnose=False, hooks=None, run_hooks=False)
                if cleanup:
                    error.add_note(cleanup)
            raise

        assert process is not None
        cleanup, cleanup_notes = _doctor_cleanup(
            process,
            diagnose=timed_out,
            hooks=hooks if timed_out else None,
            run_hooks=timed_out,
        )
        stdout = stdout_path.read_text(errors="replace")
        stderr = stderr_path.read_text(errors="replace")
        if timed_out:
            code = 124
            stderr += (
                f"\nDoctor child timed out after {timeout:g}s; session {process.pid}.\n{cleanup}\n"
            )
        else:
            code = process.returncode if process.returncode is not None else 125
        if cleanup_notes and not timed_out:
            code = 125
            stderr += "\nDoctor child cleanup failed:\n" + cleanup + "\n"
        return subprocess.CompletedProcess(command, code, stdout, stderr)


def clean_environment(**extra: str) -> dict[str, str]:
    """The caller's environment without the batch variables (batch CI exports them to the suite)."""
    env = {
        key: value
        for key, value in os.environ.items()
        if key
        not in {"CK_CI_LOCK_FD", "CK_CHAT_INPUTS", "CK_REQUIRE_FULL_SPEC", "CK_NATIVE_ANALYZE"}
        and not key.startswith("CARGO_")
    }
    env.update(extra)
    return env


def table(value: object) -> dict[str, object]:
    """A TOML table, checked rather than assumed."""
    assert isinstance(value, dict)
    return value


class Pins(unittest.TestCase):
    def pins(self) -> dict[str, str]:
        pins: dict[str, str] = {}
        for line in (TOOLS / "requirements.txt").read_text().splitlines():
            match = re.fullmatch(r"([A-Za-z0-9_.-]+)==([0-9][A-Za-z0-9_.]*)", line.strip())
            self.assertIsNotNone(match, f"not an exact pin: {line!r}")
            assert match is not None
            pins[match.group(1).lower().replace("-", "_")] = match.group(2)
        return pins

    def test_01_every_requirement_is_an_exact_pin_and_the_doctor_agrees_with_them(self) -> None:
        pins = self.pins()
        self.assertEqual(pins["ruff"], check.RUFF_VERSION)
        # shellcheck-py's own version has a fourth component; ShellCheck itself reports three.
        self.assertTrue(pins["shellcheck_py"].startswith(check.SHELLCHECK_VERSION + "."))
        self.assertEqual(pins["shellcheck_py"], "0.11.0.1")
        self.assertEqual(pins["mypy"], check.MYPY_VERSION)
        # mypy's dependencies are pinned too, so a new release cannot change the tool silently.
        for dependency in (
            "mypy_extensions",
            "pathspec",
            "typing_extensions",
            "librt",
            "ast_serialize",
        ):
            self.assertIn(dependency, pins)

    def test_02_mypy_keeps_the_python_floor_the_doctor_enforces(self) -> None:
        config = tomllib.loads((REPO / "pyproject.toml").read_text())
        self.assertEqual(table(table(config["tool"])["mypy"])["python_version"], "3.11")

    def test_03_bootstrap_installs_the_pins_with_the_hub_python(self) -> None:
        text = (TOOLS / "bootstrap").read_text()
        self.assertIn("pip install --requirement tools/requirements.txt", text)
        self.assertIn(".tools/python", text)


class Discovery(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ck-shell-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(os.path.realpath(self.temp.name))

    def write(self, name: str, text: str, mode: int = 0o644) -> str:
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        path.chmod(mode)
        return name

    def test_01_matches_shell_by_extension_or_first_line_and_nothing_else(self) -> None:
        names = [
            self.write("tools/bootstrap", "#!/bin/sh\nset -eu\n"),
            self.write("a/run.sh", "echo no shebang\n"),
            self.write("a/run.bash", "echo no shebang\n"),
            self.write("a/env-bash", "#!/usr/bin/env bash\n"),
            self.write("a/env-sh", "#!/usr/bin/env -S sh -e\n"),
            self.write("a/dash", "#!/bin/dash -e\n"),
            self.write("a/ksh", "#!/usr/bin/ksh\n"),
            self.write("a/py", "#!/usr/bin/env python3\nimport os\n"),
            self.write("a/zsh", "#!/bin/zsh\n"),
            self.write("a/none", "just text\n"),
            self.write("a/late", "\n#!/bin/sh\n"),
            self.write(".pipeline/x.sh", "echo excluded\n"),
            self.write(".tools/x.sh", "echo excluded\n"),
            self.write("target/x.sh", "echo excluded\n"),
            self.write("tests/fixtures/legacy/x.sh", "echo excluded\n"),
            self.write("tests/fixtures/other/x.sh", "echo kept\n"),
        ]
        (self.root / "a/binary").write_bytes(b"#!/bin/sh\n\xff\xfe\n")
        (self.root / "a/link.sh").symlink_to(self.root / "a/run.sh")
        found = check.shell_sources(self.root, [*names, "a", "a/binary", "a/link.sh", "gone.sh"])
        self.assertEqual(
            found,
            [
                "a/binary",
                "a/dash",
                "a/env-bash",
                "a/env-sh",
                "a/ksh",
                "a/run.bash",
                "a/run.sh",
                "tests/fixtures/other/x.sh",
                "tools/bootstrap",
            ],
        )

    def test_02_the_real_repository_matches_the_bootstrap(self) -> None:
        listing = subprocess.run(
            ["pijul", "list"],
            cwd=REPO,
            env=clean_environment(),
            capture_output=True,
            text=True,
            check=True,
        ).stdout.splitlines()
        self.assertIn("tools/bootstrap", check.shell_sources(REPO, listing))

    def commands_for(
        self, tracked: list[str], excluded: tuple[str, ...] | None = None
    ) -> list[list[str]]:
        excluded = check.SHELL_EXCLUDED if excluded is None else excluded
        state = mock.Mock(root=self.root)
        with (
            mock.patch.object(check, "_state", state),
            mock.patch.object(check, "capture", return_value="\n".join(tracked)),
            mock.patch.object(check, "hub_tool", return_value=sys.executable),
            mock.patch.object(check, "SHELL_EXCLUDED", excluded),
        ):
            return check.shellcheck_commands()

    def test_03_the_command_runs_the_hub_shellcheck_at_style_severity_with_no_exclusions(
        self,
    ) -> None:
        self.write("tools/bootstrap", "#!/bin/sh\n")
        self.write("x.sh", "true\n")
        [command] = self.commands_for(["tools/bootstrap", "x.sh", "notes.txt"])
        self.assertEqual(command, [sys.executable, "--severity=style", "tools/bootstrap", "x.sh"])
        self.assertFalse([word for word in command if word.startswith(("-e", "--exclude"))])

    def test_04_the_coverage_guard_fails_when_the_bootstrap_is_excluded(self) -> None:
        self.write("tools/bootstrap", "#!/bin/sh\n")
        self.write("x.sh", "true\n")
        with self.assertRaises(RuntimeError) as caught:
            self.commands_for(["tools/bootstrap", "x.sh"], excluded=("tools/",))
        self.assertIn("tools/bootstrap", str(caught.exception))
        # ... or when it is no longer shell, or no longer tracked.
        self.write("tools/bootstrap", "#!/usr/bin/env python3\n")
        with self.assertRaises(RuntimeError):
            self.commands_for(["tools/bootstrap", "x.sh"])
        self.write("tools/bootstrap", "#!/bin/sh\n")
        with self.assertRaises(RuntimeError):
            self.commands_for(["x.sh"])

    def test_05_a_missing_shellcheck_names_the_installation_command(self) -> None:
        self.write("tools/bootstrap", "#!/bin/sh\n")
        missing = self.root / "no-shellcheck"
        state = mock.Mock(root=self.root)
        with (
            mock.patch.object(check, "_state", state),
            mock.patch.object(check, "hub_tool", return_value=str(missing)),
            self.assertRaises(RuntimeError) as caught,
        ):
            check.shellcheck_commands()
        self.assertIn(INSTALL, str(caught.exception))

    def test_06_the_step_is_in_every_level_and_follows_ruff(self) -> None:
        names = [step.name for step in check.STEPS]
        self.assertEqual(names.index("shellcheck"), names.index("ruff") + 1)
        step = next(step for step in check.STEPS if step.name == "shellcheck")
        self.assertEqual(step.levels, ("quick", "fast", "full"))


class EnvShebang(unittest.TestCase):
    """`#!/usr/bin/env ...` lines as macOS env reads them (the kernel splits the line at spaces)."""

    SHELLS: ClassVar[dict[str, str]] = {
        "#!/usr/bin/env bash": "bash",
        "#!/usr/bin/env -u S5_REVIEW bash": "bash",  # the reviewer's probe: -u takes S5_REVIEW
        "#!/usr/bin/env -uS5_REVIEW bash": "bash",
        "#!/usr/bin/env -i bash": "bash",
        "#!/usr/bin/env - bash": "bash",
        "#!/usr/bin/env -iv sh": "sh",
        "#!/usr/bin/env -0 ksh": "ksh",
        "#!/usr/bin/env -S bash -e": "bash",
        "#!/usr/bin/env -S -i dash": "dash",
        "#!/usr/bin/env NAME=VALUE bash": "bash",
        "#!/usr/bin/env -u A -i B=1 C=2 bash": "bash",
        "#!/usr/bin/env -C /tmp bash": "bash",
        "#!/usr/bin/env -P /bin:/usr/bin sh": "sh",
        "#!/usr/bin/env -- bash": "bash",
    }
    NOT_SHELLS: ClassVar[tuple[str, ...]] = (
        "#!/usr/bin/env python3",
        "#!/usr/bin/env -u bash python3",  # bash is the operand of -u, not the utility
        "#!/usr/bin/env -i python3",
        "#!/usr/bin/env NAME=bash python3",
        "#!/usr/bin/env -S python3 -u",
        "#!/usr/bin/env",
        "#!/bin/zsh",
    )
    UNREADABLE: ClassVar[tuple[str, ...]] = (
        "#!/usr/bin/env -z bash",  # not an env option
        "#!/usr/bin/env -u",  # no operand
        "#!/usr/bin/env -S $SHELL",  # needs env's substitution
        "#!/usr/bin/env -S 'bash -e'",  # quotes the kernel's split breaks
        "#!/usr/bin/env -iz bash",
    )

    def test_01_each_form_names_the_utility_env_runs(self) -> None:
        for line, expected in self.SHELLS.items():
            with self.subTest(line):
                self.assertEqual(check.shell_interpreter(line), expected)

    def test_02_forms_that_run_something_else_are_not_shell(self) -> None:
        for line in self.NOT_SHELLS:
            with self.subTest(line):
                self.assertIsNone(check.shell_interpreter(line))

    def test_03_an_option_that_cannot_be_read_fails_closed(self) -> None:
        for line in self.UNREADABLE:
            with self.subTest(line), self.assertRaises(ValueError):
                check.shell_interpreter(line)

    def repository(self, files: Mapping[str, str]) -> Path:
        directory = tempfile.TemporaryDirectory(prefix="ck-env-shebang-")
        self.addCleanup(directory.cleanup)
        root = Path(os.path.realpath(directory.name))
        for name, text in files.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_text(text)
        env = clean_environment()
        for arguments in (["init"], ["add", "--recursive", "."]):
            subprocess.run(
                ["pijul", *arguments], cwd=root, env=env, check=True, capture_output=True
            )
        return root

    def run_step(self, root: Path) -> subprocess.CompletedProcess[str]:
        hub = os.environ["CK_HUB"]
        code = textwrap.dedent(
            f"""
            import sys
            from pathlib import Path
            sys.path.insert(0, {str(TOOLS)!r})
            import check
            steps = [s for s in check.STEPS if s.name == "shellcheck"]
            sys.exit(check.execute(Path({str(root)!r}), Path({hub!r}), "quick", steps=steps))
            """
        )
        return subprocess.run(
            [sys.executable, "-c", code],
            env=clean_environment(CK_HUB=hub),
            capture_output=True,
            text=True,
            check=False,
        )

    DEFECT = "echo $1\n"  # SC2086: unquoted

    def test_04_every_shell_form_in_a_real_tracked_file_reaches_shellcheck(self) -> None:
        files = {"tools/bootstrap": "#!/bin/sh\nset -eu\n"}
        shells = {
            f"forms/shell{index}": f"{line}\n{self.DEFECT}"
            for index, line in enumerate(self.SHELLS)
        }
        others = {
            f"forms/other{index}": f"{line}\nimport os\n"
            for index, line in enumerate(self.NOT_SHELLS)
        }
        root = self.repository({**files, **shells, **others})
        result = self.run_step(root)
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 1, output)
        self.assertIn("check quick: shellcheck FAILED => FAIL", output)
        for name in shells:
            self.assertIn(f"In {name} line", output, f"{name} was not linted")
        for name in others:
            self.assertNotIn(name, output)

    def test_05_the_reviewers_probe_alone_fails_the_step(self) -> None:
        root = self.repository(
            {
                "tools/bootstrap": "#!/bin/sh\nset -eu\n",
                "probe": f"#!/usr/bin/env -u S5_REVIEW bash\n{self.DEFECT}",
            }
        )
        result = self.run_step(root)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("In probe line", result.stdout + result.stderr)

    def test_06_an_unreadable_env_option_fails_the_step_naming_the_file(self) -> None:
        root = self.repository(
            {
                "tools/bootstrap": "#!/bin/sh\nset -eu\n",
                "strange": "#!/usr/bin/env -z bash\ntrue\n",
            }
        )
        result = self.run_step(root)
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 1, output)
        self.assertIn("strange", output)
        self.assertIn("unrecognised env option", output)
        self.assertNotIn("shellcheck --severity", output)  # nothing was linted behind its back


class ShellcheckStep(unittest.TestCase):
    """The real step on a disposable repository: a controlled defect fails the quick recipe there."""

    def test_a_controlled_shell_defect_fails_the_quick_recipe_at_shellcheck(self) -> None:
        hub = os.environ.get("CK_HUB")
        if not hub or not (Path(hub) / ".tools/python/bin/shellcheck").exists():
            self.fail("this test needs the hub's ShellCheck (tools/bootstrap installs it)")
        with tempfile.TemporaryDirectory(prefix="ck-shell-step-") as directory:
            root = Path(os.path.realpath(directory))
            (root / "tools").mkdir()
            script = root / "tools/bootstrap"
            script.write_text('#!/bin/sh\nset -eu\necho "$1"\n')
            env = clean_environment(CK_HUB=hub)
            for arguments in (["init"], ["add", "--recursive", "."]):
                subprocess.run(
                    ["pijul", *arguments], cwd=root, env=env, check=True, capture_output=True
                )
            code = textwrap.dedent(
                f"""
                import sys
                from pathlib import Path
                sys.path.insert(0, {str(TOOLS)!r})
                import check
                steps = [s for s in check.STEPS if s.name == "shellcheck"]
                sys.exit(check.execute(Path({str(root)!r}), Path({hub!r}), "quick", steps=steps))
                """
            )

            def quick() -> subprocess.CompletedProcess[str]:
                return subprocess.run(
                    [sys.executable, "-c", code],
                    env=env,
                    capture_output=True,
                    text=True,
                    check=False,
                )

            clean = quick()
            self.assertEqual(clean.returncode, 0, clean.stdout + clean.stderr)
            self.assertIn("check quick: shellcheck ok => PASS", clean.stdout)
            script.write_text(
                "#!/bin/sh\nset -eu\nfor f in $(ls); do echo $f; done\n"
            )  # SC2045, SC2086
            broken = quick()
            self.assertEqual(broken.returncode, 1, broken.stdout + broken.stderr)
            self.assertIn("check quick: shellcheck FAILED => FAIL", broken.stdout)
            self.assertIn("SC2", broken.stdout + broken.stderr)  # ShellCheck's own diagnostic
            self.assertIn("tools/bootstrap", broken.stdout + broken.stderr)


class Doctor(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ck-doctor-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(os.path.realpath(self.temp.name))
        self.bin = self.base / "hub/.tools/python/bin"
        self.bin.mkdir(parents=True)

    def tool(self, name: str, output: str) -> None:
        path = self.bin / name
        path.write_text(f"#!/bin/sh\ncat <<'EOF'\n{output}\nEOF\n")
        path.chmod(0o755)

    def good_tools(self) -> None:
        self.tool("ruff", "ruff 0.14.0")
        self.tool(
            "shellcheck", "ShellCheck - shell script analysis tool\nversion: 0.11.0\nlicense: GPL"
        )
        self.tool("mypy", "mypy 2.4.0 (compiled: yes)")

    def doctor(
        self,
        path_first: Path | str | None = None,
        *,
        graphics: bool = True,
        timeout: float = 120,
        hooks: DoctorHarnessHooks | None = None,
    ) -> subprocess.CompletedProcess[str]:
        # As `ck doctor` and the recipe report it: a RuntimeError is printed and the exit status is 1.
        code = (
            f"import sys; sys.path.insert(0, {str(TOOLS)!r}); import check\n"
            f"try:\n    check.doctor(graphics={graphics!r})\n"
            "except RuntimeError as error:\n    print(f'check: {error}', file=sys.stderr); sys.exit(1)"
        )
        path = os.environ["PATH"] if path_first is None else f"{path_first}:{os.environ['PATH']}"
        return doctor_process(
            [sys.executable, "-c", code],
            clean_environment(CK_HUB=str(self.base / "hub"), PATH=path),
            timeout=timeout,
            hooks=hooks,
        )

    def test_01_correct_tools_pass_and_xcode_and_apple_clang_are_recorded(self) -> None:
        self.good_tools()
        result = self.doctor()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for text in ("version: 0.11.0", "mypy 2.4.0", "Xcode ", "Apple clang"):
            self.assertIn(text, result.stdout)

    def test_02_a_wrong_shellcheck_fails_naming_the_installation_command(self) -> None:
        self.good_tools()
        self.tool("shellcheck", "ShellCheck - shell script analysis tool\nversion: 0.10.0")
        result = self.doctor()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("Expected ShellCheck 0.11.0", result.stderr)
        self.assertIn(f"install it with: {INSTALL}", result.stderr)

    def test_03_a_wrong_mypy_fails_naming_the_installation_command(self) -> None:
        self.good_tools()
        self.tool("mypy", "mypy 2.3.1 (compiled: yes)")
        result = self.doctor()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("Expected mypy 2.4.0", result.stderr)
        self.assertIn(f"install it with: {INSTALL}", result.stderr)

    def test_04_a_missing_tool_fails_naming_the_installation_command(self) -> None:
        self.good_tools()
        (self.bin / "shellcheck").unlink()
        result = self.doctor()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("ShellCheck is missing", result.stderr)
        self.assertIn(f"install it with: {INSTALL}", result.stderr)
        self.assertNotIn("Traceback", result.stderr)

    def test_05_a_wrong_rustc_fails_too(self) -> None:
        self.good_tools()
        shim = self.base / "shim"
        shim.mkdir()
        (shim / "rustc").write_text("#!/bin/sh\necho 'rustc 1.90.0 (fake)'\n")
        (shim / "rustc").chmod(0o755)
        result = self.doctor(path_first=str(shim))
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("Expected rustc 1.91.1", result.stderr)

    def test_06_a_non_apple_clang_is_refused(self) -> None:
        self.good_tools()
        shim = self.base / "shim"
        shim.mkdir()
        (shim / "xcrun").write_text(
            "#!/bin/sh\ncase \"$*\" in *clang*) echo 'clang version 19 (not Apple)';; "
            '*) exec /usr/bin/xcrun "$@";; esac\n'
        )
        (shim / "xcrun").chmod(0o755)
        result = self.doctor(path_first=str(shim))
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("Apple clang", result.stderr)


class SessionHarness(unittest.TestCase):
    """The timeout harness owns the complete session created by the real doctor driver."""

    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ck-doctor-session-test-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(os.path.realpath(self.temp.name))
        self.bin = self.base / "hub/.tools/python/bin"
        self.bin.mkdir(parents=True)
        self.shim = self.base / "shim"
        self.shim.mkdir()
        self.pids = self.base / "pids"
        self.pids.mkdir()
        self.fifo = self.base / "release"
        os.mkfifo(self.fifo)
        for name, output in {
            "rustc": "rustc 1.91.1 (fake)\n",
            "rg": "ripgrep 14.1.1 (fake)\n",
            "xcodebuild": "Xcode 26.0\nBuild version 17A123\n",
            "xcrun": "Apple clang version 17.0.0\nMetal compiler version 32023\n",
        }.items():
            self.write_tool(self.shim / name, f"#!/bin/sh\nprintf '%s' {output!r}\n")
        self.write_tool(self.bin / "ruff", "#!/bin/sh\necho 'ruff 0.14.0'\n")
        self.write_tool(
            self.bin / "shellcheck",
            "#!/bin/sh\nprintf '%s\\n' 'ShellCheck - shell script analysis tool' 'version: 0.11.0' 'license: GPL'\n",
        )
        self.write_tool(self.bin / "mypy", "#!/bin/sh\necho 'mypy 2.4.0 (compiled: yes)'\n")

    @staticmethod
    def write_tool(path: Path, content: str) -> None:
        path.write_text(content)
        path.chmod(0o755)

    def doctor_command(self) -> tuple[list[str], dict[str, str]]:
        code = (
            f"import sys; sys.path.insert(0, {str(TOOLS)!r}); import check\n"
            "check.doctor(graphics=False)"
        )
        env = clean_environment(
            CK_HUB=str(self.base / "hub"), PATH=f"{self.shim}:{os.environ['PATH']}"
        )
        return [sys.executable, "-c", code], env

    def write_blocking_pijul(self, *, ignore_term: bool = False) -> Path:
        pidfile = self.pids / "pijul"
        temporary_pidfile = pidfile.with_name(f"{pidfile.name}.tmp.$$")
        trap = "trap '' TERM\n" if ignore_term else ""
        self.write_tool(
            self.shim / "pijul",
            f"#!/bin/sh\nprintf '%s\\n' \"$$\" > {temporary_pidfile}\nmv {temporary_pidfile} {pidfile}\n{trap}exec /bin/sleep 3600\n",
        )
        return pidfile

    def pid_from(self, path: Path, timeout: float = 3) -> int:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                contents = path.read_text().strip()
            except FileNotFoundError:
                contents = ""
            if contents:
                try:
                    return int(contents)
                except ValueError:
                    # A fixture may have created/truncated the file before its PID write lands.
                    pass
            time.sleep(DOCTOR_POLL)
        raise AssertionError(f"process did not write its PID file: {path}")

    def wait_for_fixtures(self, *pidfiles: Path) -> Callable[[int], None]:
        """Reach every blocked fixture before a Doctor timeout used by its control starts."""

        def wait(_session: int) -> None:
            for pidfile in pidfiles:
                self.pid_from(pidfile)

        return wait

    @staticmethod
    def live(pid: int) -> bool:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        return True

    def assert_gone(self, pid: int, timeout: float = 3) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline and self.live(pid):
            try:
                if os.getsid(pid) != pid:
                    pass
            except ProcessLookupError:
                return
            time.sleep(DOCTOR_POLL)
        if self.live(pid):
            try:
                state = subprocess.run(
                    ["ps", "-p", str(pid), "-o", "stat=,command="],
                    capture_output=True,
                    text=True,
                    check=False,
                    timeout=2,
                ).stdout.strip()
            except subprocess.SubprocessError:
                state = "unknown"
            self.assertTrue(state.startswith("Z"), f"pid {pid} still lives: {state}")

    def process_row(self, pid: int) -> DoctorProcessRow:
        for row in _doctor_session_rows(os.getsid(pid)):
            if row[0] == pid:
                return row
        raise AssertionError(f"pid {pid} is absent from its session")

    def test_blocked_probe_is_cooperatively_ended_and_verified(self) -> None:
        pidfile = self.write_blocking_pijul()
        command, env = self.doctor_command()
        observed_by_session: dict[int, list[DoctorProcessRow]] = {}

        def observe(session: int, rows: list[DoctorProcessRow]) -> None:
            observed_by_session[session] = rows

        result = doctor_process(
            command,
            env,
            timeout=2,
            hooks=DoctorHarnessHooks(
                before_deadline=self.wait_for_fixtures(pidfile), after_discovery=observe
            ),
        )
        probe = self.pid_from(pidfile)
        self.assertEqual(result.returncode, 124, result.stdout + result.stderr)
        self.assertIn("cleanup verification: no live process remains", result.stderr)
        self.assertEqual(len(observed_by_session), 1)
        ((session, observed),) = observed_by_session.items()
        self.assertTrue(any(row[0] == probe for row in observed), observed)
        probe_row = next(row for row in observed if row[0] == probe)
        self.assertNotEqual(probe_row[2], session)
        self.assertRegex(
            result.stderr,
            rf"{probe}: 'cooperative SIGTERM/check\.run cleanup'",
        )
        self.assertIn(str(probe), result.stderr)
        self.assert_gone(probe)

    def test_sigterm_ignoring_probe_is_force_killed(self) -> None:
        pidfile = self.write_blocking_pijul(ignore_term=True)
        command, env = self.doctor_command()
        observed: list[DoctorProcessRow] = []

        def observe(session: int, rows: list[DoctorProcessRow]) -> None:
            observed.extend(rows)

        result = doctor_process(
            command,
            env,
            timeout=2,
            hooks=DoctorHarnessHooks(
                before_deadline=self.wait_for_fixtures(pidfile), after_discovery=observe
            ),
        )
        probe = self.pid_from(pidfile)
        self.assertEqual(result.returncode, 124, result.stdout + result.stderr)
        self.assertTrue(
            "forced SIGKILL" in result.stderr
            or "cooperative SIGTERM/check.run cleanup" in result.stderr,
            result.stderr,
        )
        self.assertTrue(any(row[0] == probe for row in observed), observed)
        self.assert_gone(probe)

    def test_second_probe_started_after_discovery_is_found_by_session(self) -> None:
        pijul_pid = self.pids / "pijul"
        pijul_temporary_pid = pijul_pid.with_name(f"{pijul_pid.name}.tmp.$$")
        self.write_tool(
            self.shim / "pijul",
            f"#!/bin/sh\nprintf '%s\\n' \"$$\" > {pijul_temporary_pid}\nmv {pijul_temporary_pid} {pijul_pid}\nread release < {self.fifo}\necho 'pijul 1.0.0-beta.24'\n",
        )
        ruff_pid = self.pids / "ruff"
        ruff_temporary_pid = ruff_pid.with_name(f"{ruff_pid.name}.tmp.$$")
        self.write_tool(
            self.bin / "ruff",
            f"#!/bin/sh\nprintf '%s\\n' \"$$\" > {ruff_temporary_pid}\nmv {ruff_temporary_pid} {ruff_pid}\nexec /bin/sleep 3600\n",
        )
        command, env = self.doctor_command()
        released = False

        def after_discovery(session: int, rows: list[DoctorProcessRow]) -> None:
            nonlocal released
            os.kill(session, signal.SIGCONT)
            with self.fifo.open("w") as stream:
                stream.write("continue\n")
            self.pid_from(ruff_pid)
            os.kill(session, signal.SIGSTOP)
            released = True

        result = doctor_process(
            command,
            env,
            timeout=2,
            hooks=DoctorHarnessHooks(
                before_deadline=self.wait_for_fixtures(pijul_pid),
                after_discovery=after_discovery,
                cooperate=False,
            ),
        )
        self.assertTrue(released, result.stderr)
        probe = self.pid_from(pijul_pid)
        ruff = self.pid_from(ruff_pid)
        self.assertEqual(result.returncode, 124, result.stdout + result.stderr)
        self.assertIn(str(ruff), result.stderr)
        self.assertIn("cleanup verification: no live process remains", result.stderr)
        self.assert_gone(probe)
        self.assert_gone(ruff)

    def test_reparented_process_keeps_session_ownership_after_probe_leader_exits(self) -> None:  # noqa: PLR0915  # keep the full leader-exit safety trace together
        child_pidfile = self.pids / "orphan"
        leader_pidfile = self.pids / "pijul"
        script = textwrap.dedent(
            f"""\
            #!/usr/bin/env python3
            import os
            import time
            def write_pid(path, pid):
                temporary = f"{{path}}.tmp.{{os.getpid()}}"
                with open(temporary, "x") as output:
                    output.write(f"{{pid}}\\n")
                os.replace(temporary, path)
            write_pid({str(leader_pidfile)!r}, os.getpid())
            child = os.fork()
            if child == 0:
                os.setpgid(0, 0)
                write_pid({str(child_pidfile)!r}, os.getpid())
                os.execl("/bin/sleep", "sleep", "3600")
            with open({str(self.fifo)!r}) as release:
                release.readline()
            print("pijul 1.0.0-beta.24", flush=True)
            """
        )
        self.write_tool(self.shim / "pijul", script)
        command, env = self.doctor_command()
        child_pid: int | None = None
        child_session: int | None = None
        child_row: DoctorProcessRow | None = None
        parent_control_missed = False

        def after_discovery(session: int, rows: list[DoctorProcessRow]) -> None:
            nonlocal child_pid, child_session
            os.kill(session, signal.SIGCONT)
            with self.fifo.open("w") as stream:
                stream.write("continue\n")
            child_pid = self.pid_from(child_pidfile)
            child_session = os.getsid(child_pid)
            self.pid_from(leader_pidfile)
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                try:
                    leader = _doctor_session_rows(session)
                except (OSError, subprocess.SubprocessError):
                    leader = []
                leader_row = next(
                    (row for row in leader if row[0] == int(leader_pidfile.read_text())), None
                )
                if leader_row is None or leader_row[4].startswith("Z"):
                    break
                time.sleep(DOCTOR_POLL)
            os.kill(session, signal.SIGSTOP)

        def check_parent_control(session: int) -> None:
            nonlocal child_row, parent_control_missed
            assert child_pid is not None
            rows = _doctor_session_rows(session)
            child_row = next((row for row in rows if row[0] == child_pid), None)
            all_rows = subprocess.run(
                ["ps", "-A", "-o", "pid=,ppid="], capture_output=True, text=True, check=True
            ).stdout
            parents = {
                int(line.split()[0]): int(line.split()[1])
                for line in all_rows.splitlines()
                if line.split()
            }
            owned_by_parent: set[int] = {session}
            while True:
                children = {pid for pid, parent in parents.items() if parent in owned_by_parent}
                if children <= owned_by_parent:
                    break
                owned_by_parent |= children
            parent_control_missed = child_pid not in owned_by_parent

        result = doctor_process(
            command,
            env,
            timeout=2,
            hooks=DoctorHarnessHooks(
                before_deadline=self.wait_for_fixtures(leader_pidfile, child_pidfile),
                after_discovery=after_discovery,
                before_force=check_parent_control,
                cooperate=False,
            ),
        )
        self.assertIsNotNone(child_pid)
        self.assertIsNotNone(child_session)
        assert child_pid is not None and child_session is not None
        self.assertEqual(child_session, int(result.stderr.split("session ", 1)[1].split(".", 1)[0]))
        self.assertIsNotNone(child_row)
        assert child_row is not None
        self.assertNotEqual(child_row[2], child_session)
        self.assertTrue(parent_control_missed)
        self.assertIn(str(child_pid), result.stderr)
        self.assertIn("forced SIGKILL", result.stderr)
        self.assertIn("cleanup verification: no live process remains", result.stderr)
        self.assert_gone(child_pid)

    def test_exited_child_stays_unreaped_through_force_cleanup(self) -> None:
        command, env = self.doctor_command()
        observations: list[tuple[str, bool, int | None]] = []

        pidfile = self.write_blocking_pijul()

        def exit_during_cleanup(session: int, rows: list[DoctorProcessRow]) -> None:
            self.pid_from(pidfile)
            os.kill(session, signal.SIGTERM)
            os.kill(session, signal.SIGCONT)

        def observe_pin(stage: str) -> Callable[[int], None]:
            def observe(session: int) -> None:
                info = _waitid(os.P_PID, session, os.WEXITED | os.WNOWAIT | os.WNOHANG)
                self.assertIsNotNone(info)
                assert info is not None
                self.assertEqual(info.si_pid, session)
                zombie = subprocess.run(
                    ["ps", "-p", str(session), "-o", "stat="],
                    capture_output=True,
                    text=True,
                    check=True,
                    timeout=2,
                ).stdout.strip()
                self.assertTrue(zombie.startswith("Z"), zombie)
                observations.append((stage, True, session))

            return observe

        hooks = DoctorHarnessHooks(
            before_deadline=self.wait_for_fixtures(pidfile),
            after_discovery=exit_during_cleanup,
            before_force=observe_pin("force"),
            before_reap=observe_pin("reap"),
        )
        command, env = self.doctor_command()
        result = doctor_process(
            command,
            env,
            timeout=2,
            hooks=hooks,
        )
        self.assertEqual(result.returncode, 124, result.stdout + result.stderr)
        self.assertEqual([item[0] for item in observations], ["force", "reap"], result.stderr)
        self.assertFalse(self.live(observations[-1][2] or -1))

    def test_process_outside_the_session_survives_cleanup(self) -> None:
        outside = subprocess.Popen(["/bin/sleep", "3600"], start_new_session=True)

        def cleanup_outside() -> None:
            if outside.poll() is None:
                outside.kill()

        self.addCleanup(cleanup_outside)
        command, env = self.doctor_command()
        pidfile = self.write_blocking_pijul()
        result = doctor_process(
            command,
            env,
            timeout=2,
            hooks=DoctorHarnessHooks(before_deadline=self.wait_for_fixtures(pidfile)),
        )
        self.assertEqual(result.returncode, 124, result.stdout + result.stderr)
        self.assertIsNone(outside.poll())
        self.assertTrue(self.live(outside.pid))
        outside.kill()
        outside.wait(timeout=3)
        self.assertFalse(self.live(outside.pid))

    def test_exception_in_wait_hook_cleans_session_and_preserves_error(self) -> None:
        pidfile = self.write_blocking_pijul()
        command, env = self.doctor_command()
        failure = RuntimeError("injected wait failure")

        def raise_failure(session: int) -> None:
            raise failure

        with self.assertRaises(RuntimeError) as caught:
            doctor_process(
                command,
                env,
                timeout=5,
                hooks=DoctorHarnessHooks(
                    before_deadline=self.wait_for_fixtures(pidfile), during_wait=raise_failure
                ),
            )
        self.assertIs(caught.exception, failure)
        self.assertFalse(
            any(
                "cleanup verification found live" in note
                for note in getattr(caught.exception, "__notes__", [])
            )
        )
        self.assert_gone(self.pid_from(pidfile))

    def test_old_timeout_path_leaves_the_probe_until_the_control_kills_it(self) -> None:
        pidfile = self.write_blocking_pijul()
        command, env = self.doctor_command()
        # The old-timeout control is about orphan cleanup, not rustup discovery. Keep a loaded
        # machine from spending its two-second timeout in the real rustc proxy before doctor
        # reaches the blocking pijul fixture.
        self.write_tool(self.shim / "rustc", "#!/bin/sh\necho 'rustc 1.91.1'\n")
        original_communicate = subprocess.Popen.communicate
        child_pid: int | None = None

        def wait_for_fixture(
            process: subprocess.Popen[bytes],
            input: bytes | None = None,
            timeout: float | None = None,
        ) -> tuple[bytes | None, bytes | None]:
            nonlocal child_pid
            if process.args == command:
                child_pid = process.pid
                try:
                    # subprocess.run starts its timeout inside communicate().
                    # Hold that point until the fixture proves that the probe is actually blocked,
                    # so the old timeout control tests cleanup rather than startup scheduling.
                    self.pid_from(pidfile, timeout=5)
                except BaseException:
                    if process.poll() is None:
                        with contextlib.suppress(ProcessLookupError):
                            process.kill()
                    process.wait()
                    raise
            return original_communicate(process, input=input, timeout=timeout)

        with (
            mock.patch.object(subprocess.Popen, "communicate", new=wait_for_fixture),
            self.assertRaises(subprocess.TimeoutExpired),
        ):
            subprocess.run(command, env=env, timeout=2, check=False)
        assert child_pid is not None
        probe = self.pid_from(pidfile)
        self.assertEqual(os.getsid(probe), os.getsid(os.getpid()))
        probe_group = os.getpgid(probe)
        self.assertNotEqual(probe_group, child_pid)
        self.assertTrue(self.live(probe))
        self.assertEqual(os.getsid(probe), os.getsid(os.getpid()))
        self.assertEqual(os.getpgid(probe), probe_group)
        os.killpg(probe_group, signal.SIGKILL)
        self.assert_gone(probe)


class AnalyzerEnvironment(unittest.TestCase):
    def state(self, level: str, root: Path) -> check.State:
        return check.State(root, root, level, batch=False, target=root / "target")

    def test_01_only_the_steps_that_share_the_build_script_unit_set_the_variable(self) -> None:
        root = Path(tempfile.gettempdir())
        by_name = {step.name: step for step in check.STEPS}
        for level in ("fast", "full"):
            state = self.state(level, root)
            with mock.patch.dict(os.environ):
                os.environ.pop("CK_NATIVE_ANALYZE", None)
                setting = {
                    name: state.environment(step).get("CK_NATIVE_ANALYZE")
                    for name, step in by_name.items()
                }
            self.assertEqual(
                {name for name, value in setting.items() if value == "1"},
                {
                    "doc",
                    "rust.core",
                    "rust.app",
                    "metal.isolation",
                    "metal.gpu",
                    "native.window",
                    "metal.window-core",
                    "metal.window-scene",
                    "metal.window-compat",
                    "metal.window-instanced",
                    "input.core",
                    "render2d.core",
                    "resources.core",
                    "glfw.core",
                    "glfw.window",
                    "vulkan.isolation",
                    "vulkan.gpu",
                    "vulkan.window",
                    "render2d.vulkan-window",
                    "vulkan.app",
                },
                level,
            )
            self.assertEqual([v for v in setting.values() if v not in (None, "1")], [])
            self.assertIsNone(setting["clippy"])

    def test_01b_rustdoc_warnings_are_errors_for_the_doc_step_only(self) -> None:
        by_name = {step.name: step for step in check.STEPS}
        for level in ("fast", "full"):
            state = self.state(level, Path(tempfile.gettempdir()))
            with mock.patch.dict(os.environ):
                os.environ.pop("RUSTDOCFLAGS", None)
                setting = {
                    name: state.environment(step).get("RUSTDOCFLAGS")
                    for name, step in by_name.items()
                }
            self.assertEqual({n: v for n, v in setting.items() if v}, {"doc": "-D warnings"}, level)

    def test_02_a_value_the_caller_exported_never_reaches_the_other_steps(self) -> None:
        by_name = {step.name: step for step in check.STEPS}
        state = self.state("fast", Path(tempfile.gettempdir()))
        with mock.patch.dict(os.environ, {"CK_NATIVE_ANALYZE": "1"}):
            self.assertNotIn("CK_NATIVE_ANALYZE", state.environment(by_name["clippy"]))
            self.assertNotIn("CK_NATIVE_ANALYZE", state.environment(None))
            self.assertEqual(state.environment(by_name["rust.app"])["CK_NATIVE_ANALYZE"], "1")
            # CK-44: the feature-less test step neither analyzes nor inherits the caller's value.
            self.assertNotIn("CK_NATIVE_ANALYZE", state.environment(by_name["rust.app-default"]))


class AnalyzerBuild(unittest.TestCase):
    """A real `cargo build` of a copy of the crate: the analyzer runs on the real bridge arguments."""

    INPUTS = ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "apps", "crates")

    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ck-analyzer-")
        self.addCleanup(self.temp.cleanup)
        self.copy = Path(os.path.realpath(self.temp.name)) / "copy"
        self.copy.mkdir()
        for name in self.INPUTS:
            source = REPO / name
            if source.is_dir():
                shutil.copytree(source, self.copy / name, ignore=shutil.ignore_patterns("target"))
            else:
                shutil.copy2(source, self.copy / name)
        self.target = Path(os.path.realpath(self.temp.name)) / "target"
        self.bridge = self.copy / "crates/gpu-metal/native/bridge.m"
        self.clean = self.bridge.read_text()

    def build(self, analyze: str | None) -> subprocess.CompletedProcess[str]:
        env = clean_environment(
            CARGO_TARGET_DIR=str(self.target), CARGO_BUILD_BUILD_DIR=str(self.target)
        )
        if analyze is not None:
            env["CK_NATIVE_ANALYZE"] = analyze
        return subprocess.run(
            ["cargo", "build", "-p", "ck-gpu-metal", "--locked", "--offline", "-vv"],
            cwd=self.copy,
            env=env,
            capture_output=True,
            text=True,
            check=False,
            timeout=300,
        )

    def test_01_the_analyzer_runs_before_the_compile_with_the_shared_arguments(self) -> None:
        result = self.build("1")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        lines = [
            line
            for line in (result.stdout + result.stderr).splitlines()
            if "ck-gpu-metal: xcrun" in line
        ]
        analysis = next(line for line in lines if "--analyze" in line)
        compile_ = next(line for line in lines if " -c " in line and "bridge.m" in line)
        self.assertLess(lines.index(analysis), lines.index(compile_))
        self.assertIn("-Xclang -analyzer-werror", analysis)
        self.assertIn("bridge.analysis", analysis)
        # Everything but the output mode is shared with the real compile, deployment target included.
        shared = analysis.split("--analyze")[0].split("ck-gpu-metal: ")[1].strip()
        self.assertIn("-mmacosx-version-min=", shared)
        self.assertTrue(compile_.split("ck-gpu-metal: ")[1].startswith(shared))
        self.assertNotIn(" -c ", analysis)

    def test_02_a_controlled_finding_fails_the_build_with_the_analyzers_diagnostic(self) -> None:
        self.bridge.write_text(self.clean + BRIDGE_DEFECT)
        result = self.build("1")
        output = result.stdout + result.stderr
        self.assertNotEqual(result.returncode, 0, output)
        self.assertIn("Clang static analyzer reported findings", output)
        self.assertIn("core.NullDereference", output)
        self.assertIn("bridge.m", output)

    def test_03_the_analyzer_is_opt_in_and_reruns_when_the_variable_changes(self) -> None:
        self.bridge.write_text(self.clean + BRIDGE_DEFECT)
        off = self.build(None)  # the defect compiles: it is only an analyzer finding
        self.assertEqual(off.returncode, 0, off.stdout + off.stderr)
        zero = self.build("0")
        self.assertEqual(zero.returncode, 0, zero.stdout + zero.stderr)
        on = self.build("1")  # same sources, only the variable changed
        self.assertNotEqual(on.returncode, 0, on.stdout + on.stderr)

    def test_04_any_other_value_is_an_error_not_a_skipped_analysis(self) -> None:
        result = self.build("yes")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("CK_NATIVE_ANALYZE must be 1 or 0", result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
