"""S3: check levels, lock-free manual full checks and build isolation (real processes where stated)."""

import ast
import contextlib
import fcntl
import inspect
import io
import itertools
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
import tomllib
import unittest
from collections.abc import Iterable, Mapping, Sequence
from pathlib import Path
from typing import NoReturn
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import chatfix
import check
import environment
from gate import locks

TOOLS = Path(__file__).resolve().parents[1] / "tools"
REPO = Path(__file__).resolve().parents[1]

ALL = ("quick", "fast", "full")
UNITTEST_STEPS = (
    "unittest-batch",
    "unittest-catalog",
    "unittest-check",
    "unittest-lints",
    "unittest-bootstrap",
    "unittest-lease",
)
RUST_STEPS = (
    "rust.core",
    "rust.app",
    "rust.app-default",
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
)
FULL_SPEC_STEPS = {
    "rust.core",
    "rust.app",
    "metal.gpu",
    "native.window",
    "metal.window-core",
    "metal.window-scene",
    "metal.window-compat",
    "metal.window-instanced",
}  # the all-features groups
ANALYZER_STEPS = {
    "doc",
    *FULL_SPEC_STEPS,
    "metal.isolation",
    "input.core",
    "vulkan.isolation",
    "render2d.core",
    "render2d.vulkan-window",
    "resources.core",
    "glfw.core",
    "glfw.window",
    "vulkan.gpu",
    "vulkan.window",
    "vulkan.app",
}  # they share the build-script unit
# The only real `check.py` runs of this suite are planned runs over the focus-free groups: a real run of
# the full level would start the native window tests (CK-77), and the full level is the batch's.
REAL_CHECK_ARGUMENTS = ["--group", *check.REAL_TOOLCHAIN_GROUPS]
BOUND = 10.0  # seconds any handshake with a driver may take before the test fails

# Driver-side wait for the test's release byte. A blocking `sys.stdin.readline()` can lose a signal
# that arrives just before the read starts (the handler is flagged, but nothing wakes the read), which
# hung the handshake; polling with a short select returns to the interpreter so handlers always run.
RELEASE = (
    "import os, select, sys\n"
    "def emit(*words):\n"
    "    # os.write, never print: a handler (or the wrapper it runs) that prints while the main thread is inside\n"
    "    # print() on the same buffered stdout raises 'reentrant call inside <_io.BufferedWriter>' into the\n"
    "    # interrupted code (CK-89), and one write per line cannot interleave within a line.\n"
    "    os.write(1, (' '.join(map(str, words)) + '\\n').encode())\n"
    "def wait_released():\n"
    "    while not select.select([sys.stdin], [], [], 0.05)[0]:\n"
    "        pass\n"
    "    sys.stdin.readline()\n"
)


class Session:
    """A driver process with lossless, bounded line reads and a cleanup that always ends it.

    Every read has a deadline and fails the test with what the driver had printed, so a missed
    handshake can no longer hang the suite; the driver is killed in a cleanup whatever happens."""

    def __init__(
        self,
        test: unittest.TestCase,
        code: str,
        *,
        env: Mapping[str, str] | None = None,
        stdin: bool = False,
    ) -> None:
        self.test = test
        self.seen: list[str] = []
        self.pending = b""
        self.process = subprocess.Popen(
            [sys.executable, "-c", code],
            stdin=subprocess.PIPE if stdin else None,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=0,
            env=env or clean_environment(),
        )
        test.addCleanup(self.close)

    def send(self, number: int) -> None:
        self.process.send_signal(number)

    def read_line(self, seconds: float = BOUND, wanted: str = "a line") -> str:
        stdout = self.process.stdout
        assert stdout is not None
        deadline = time.monotonic() + seconds
        while b"\n" not in self.pending:
            left = deadline - time.monotonic()
            if left <= 0 or not select.select([stdout], [], [], left)[0]:
                self.give_up(f"no {wanted} within {seconds:g} s")
            chunk = os.read(stdout.fileno(), 4096)
            if not chunk:
                self.give_up(f"the driver ended before {wanted}")
            self.pending += chunk
        line, _, self.pending = self.pending.partition(b"\n")
        self.seen.append(line.decode().strip())
        return self.seen[-1]

    def until(self, marker: str, seconds: float = BOUND) -> list[str]:
        """Read lines until `marker` has been printed; returns the lines read."""
        lines: list[str] = []
        while not lines or lines[-1] != marker:
            lines.append(self.read_line(seconds, repr(marker)))
        return lines

    def release(self) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(b"\n")
        self.process.stdin.flush()

    def give_up(self, why: str) -> NoReturn:
        self.process.kill()
        try:
            _, errors = self.process.communicate(timeout=3)
        except subprocess.TimeoutExpired:  # the step's own processes still hold the pipes
            errors = b"(not available: the step's processes still hold the pipe)"
        self.test.fail(f"{why}; the driver had printed {self.seen}; stderr: {errors.decode()}")

    def finish(self, seconds: float = 30) -> tuple[str, str]:
        """The driver's remaining stdout and its stderr (both text) once it has ended."""
        try:
            output, errors = self.process.communicate(timeout=seconds)
        except subprocess.TimeoutExpired:
            self.give_up(f"the driver did not end within {seconds:g} s")
        return (self.pending + output).decode(), errors.decode()

    @property
    def returncode(self) -> int | None:
        return self.process.returncode

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait()
        for pipe in (self.process.stdin, self.process.stdout, self.process.stderr):
            if pipe:
                pipe.close()


def table(value: object) -> dict[str, object]:
    """A TOML table, checked rather than assumed."""
    assert isinstance(value, dict)
    return value


def names(value: object) -> list[str]:
    """A TOML array of strings, checked rather than assumed."""
    assert isinstance(value, list)
    assert all(isinstance(item, str) for item in value)
    return value


def clean_environment(**extra: str) -> dict[str, str]:
    """The caller's environment with every variable that would make a run a batch removed."""
    env = {
        key: value
        for key, value in os.environ.items()
        if key not in {"CK_CI_LOCK_FD", "CK_CHAT_INPUTS", "CK_REQUIRE_FULL_SPEC"}
        and not key.startswith("CARGO_")
    }
    env.update(extra)
    return env


class CheckFixture(unittest.TestCase):
    """A fixture hub, copies and a driver that runs `check.execute` with cheap fixture steps."""

    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ck-check-levels-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(os.path.realpath(self.temp.name))
        self.hub = self.base / "hub"
        for name in ("checkpoints", "ci"):
            (self.hub / ".pipeline" / name).mkdir(parents=True)
        (self.hub / ".pipeline/ci.lock").touch()
        self.copy = self.base / "copy"
        self.copy.mkdir()

    def driver(  # noqa: PLR0913  # test helper with explicit scenario arguments
        self,
        steps: str,
        level: str = "full",
        root: Path | None = None,
        watchdog: float | None = None,
        prelude: str = "",
        timeout: float = 60,
        env: Mapping[str, str] | None = None,
        pass_fds: Sequence[int] = (),
    ) -> subprocess.CompletedProcess[str]:
        """Run check.execute in a real process; `steps` is Python source for a list of Step.
        `watchdog` shortens the per-step watchdog (test-only: it is a module constant); `prelude`
        is extra Python run in the driver before the steps are built."""
        code = self.build_driver_source(steps, level, root, watchdog, prelude)
        # Never the caller's environment: batch CI runs the suite with CK_CI_LOCK_FD, CK_CHAT_INPUTS
        # and CK_REQUIRE_FULL_SPEC inherited, and a nested check would refuse or change behaviour.
        return subprocess.run(
            [sys.executable, "-c", code],
            check=False,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env or clean_environment(),
            pass_fds=pass_fds,
        )

    def build_driver_source(
        self,
        steps: str,
        level: str = "full",
        root: Path | None = None,
        watchdog: float | None = None,
        prelude: str = "",
    ) -> str:
        prelude = textwrap.indent(prelude, " " * 12).lstrip()  # multi-line preludes stay aligned
        return textwrap.dedent(
            f"""
            import signal, sys
            from pathlib import Path
            sys.path.insert(0, {str(TOOLS)!r})
            import check
            from check import Step
            {"check.WATCHDOG = " + repr(watchdog) if watchdog is not None else ""}
            {prelude}
            steps = {steps}
            signal.signal(signal.SIGTERM, check.terminate)
            sys.exit(check.execute(Path({str(root or self.copy)!r}), Path({str(self.hub)!r}),
                                   {level!r}, steps=steps))
            """
        )

    def listing(self) -> dict[str, list[str]]:
        return {
            name: sorted(path.name for path in (self.hub / ".pipeline" / name).iterdir())
            for name in ("checkpoints", "ci")
        }

    def step_source(self, body: str, name: str = "probe", levels: Sequence[str] = ALL) -> str:
        """A one-command fixture step that runs `python -c body` with the step's environment."""
        return f"[Step({name!r}, {levels!r}, lambda: [[sys.executable, '-c', {body!r}]])]"


class Levels(CheckFixture):
    def test_01_levels_select_the_exact_steps_in_order(self) -> None:
        self.assertEqual(check.select("quick"), ["fmt", "ruff", "shellcheck"])
        self.assertEqual(
            check.select("fast"),
            [
                "fmt",
                "ruff",
                "shellcheck",
                "doctor",
                "doctor.graphics",
                "clippy",
                "doc",
                *RUST_STEPS,
                *UNITTEST_STEPS,
                "mypy",
                "chat-ci",
            ],
        )
        self.assertEqual(
            check.select("full"),
            [
                "fmt",
                "ruff",
                "shellcheck",
                "doctor",
                "doctor.graphics",
                "clippy",
                "doc",
                *RUST_STEPS,
                *UNITTEST_STEPS,
                "mypy",
                "chat-ci",
            ],
        )
        by_name = {step.name: step for step in check.STEPS}
        self.assertEqual(by_name["fmt"].commands(), [["cargo", "fmt", "--all", "--", "--check"]])
        self.assertEqual(
            by_name["clippy"].commands(),
            [
                [
                    "cargo",
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ]
            ],
        )
        self.assertEqual(
            by_name["doc"].commands(),
            [["cargo", "doc", "--workspace", "--no-deps", "--all-features", "--locked"]],
        )
        flags = ["--all-features", "--locked"]
        self.assertEqual(
            by_name["rust.core"].commands(),
            [
                [
                    "cargo",
                    "test",
                    "-p",
                    "ck-log",
                    "-p",
                    "ck-lua",
                    "-p",
                    "ck-runtime",
                    "-p",
                    "ck-window",
                    *flags,
                ]
            ],
        )
        self.assertEqual(
            by_name["rust.app"].commands(),
            [
                [
                    "cargo",
                    "test",
                    "-p",
                    "codekarl",
                    "--lib",
                    "--bin",
                    "codekarl",
                    "--test",
                    "cli_spec_unit",
                    "--test",
                    "lua_vms",
                    "--test",
                    "modes",
                    "--test",
                    "render",
                    "--test",
                    "runtime",
                    "--test",
                    "signals",
                    "--test",
                    "source_guard",
                    *flags,
                ],
                [
                    "cargo",
                    "test",
                    "-p",
                    "codekarl",
                    "--test",
                    "cli_spec",
                    *flags,
                    "--",
                    "--set",
                    "plain",
                ],
                # CK-131: the application is a library now, with doctests (Cargo refuses `--doc` beside other
                # target selectors).
                ["cargo", "test", "-p", "codekarl", "--doc", *flags],
            ],
        )
        # CK-44: the application is also tested without its `metal` feature.
        self.assertEqual(
            by_name["rust.app-default"].commands(),
            [
                ["cargo", "test", "-p", "codekarl", "--locked"],
                # CK-162: the `vulkan` feature alone compiles (every target, none run).
                [
                    "cargo",
                    "check",
                    "-p",
                    "codekarl",
                    "--no-default-features",
                    "--features",
                    "vulkan",
                    "--all-targets",
                    "--locked",
                ],
            ],
        )
        self.assertEqual(by_name["rust.app-default"].levels, ("fast", "full"))
        self.assertFalse(by_name["rust.app-default"].full_spec)
        self.assertFalse(by_name["rust.app-default"].analyze)
        self.assertEqual(
            by_name["metal.gpu"].commands(),
            [
                [
                    "cargo",
                    "test",
                    "-p",
                    "ck-gpu-metal",
                    "--lib",
                    "--test",
                    "gpu_probe",
                    "--test",
                    "textures",
                    "--test",
                    "shader_debug",
                    *flags,
                ],
                ["cargo", "test", "-p", "ck-gpu-metal", "--doc", *flags],
                [
                    "cargo",
                    "test",
                    "-p",
                    "ck-gpu-metal",
                    "--test",
                    "presenter",
                    *flags,
                    "--",
                    "--set",
                    "gpu",
                ],
                [
                    "cargo",
                    "test",
                    "-p",
                    "codekarl",
                    "--test",
                    "cli_spec",
                    *flags,
                    "--",
                    "--set",
                    "gpu",
                ],
            ],
        )
        self.assertEqual(
            by_name["native.window"].commands(),
            [
                ["cargo", "test", "-p", "ck-window-cocoa", *flags],
                ["cargo", "test", "-p", "codekarl", "--test", "windowed", *flags],
                # CK-92: `ck-render-2d`'s Metal backend on a real window.
                ["cargo", "test", "-p", "ck-render-2d", "--test", "metal_window", *flags],
            ],
        )
        # CK-142: the presenter's windowed cases, one group per set (the old `--set window` is an error now).
        for name in ("window-core", "window-scene", "window-compat", "window-instanced"):
            self.assertEqual(
                by_name[f"metal.{name}"].commands(),
                [
                    [
                        "cargo",
                        "test",
                        "-p",
                        "ck-gpu-metal",
                        "--test",
                        "presenter",
                        *flags,
                        "--",
                        "--set",
                        name,
                    ],
                ],
                name,
            )
        # CK-78: the GLFW backend's two groups, and its graphics doctor.
        self.assertEqual(
            by_name["glfw.core"].commands(),
            [
                ["cargo", "test", "-p", "ck-window-glfw", "--lib", *flags],
                ["cargo", "test", "-p", "ck-window-glfw", "--doc", *flags],
            ],
        )
        self.assertEqual(
            by_name["glfw.window"].commands(),
            [["cargo", "test", "-p", "ck-window-glfw", "--test", "window_glfw", *flags]],
        )
        self.assertEqual(
            (by_name["glfw.window"].needs, by_name["glfw.core"].needs), ("window", "plain")
        )
        self.assertEqual(
            by_name["glfw.window"].paths, ("crates/window-glfw/", "apps/codekarl/tests/supervisor/")
        )
        # CK-79: the Vulkan presenter's two groups, each selected only by the crate's own paths.
        self.assertEqual(
            by_name["vulkan.gpu"].commands(),
            [
                ["cargo", "test", "-p", "ck-gpu-vulkan", "--lib", *flags],
                ["cargo", "test", "-p", "ck-gpu-vulkan", "--doc", *flags],
                ["cargo", "test", "-p", "ck-gpu-vulkan", "--test", "gpu", *flags],
            ],
        )
        # CK-84: the two hook-isolation guards are their own plain groups.
        self.assertEqual(
            by_name["metal.isolation"].commands(),
            [["cargo", "test", "-p", "ck-gpu-metal", "--test", "hook_isolation", *flags]],
        )
        self.assertEqual(
            by_name["vulkan.isolation"].commands(),
            [["cargo", "test", "-p", "ck-gpu-vulkan", "--test", "hook_isolation", *flags]],
        )
        self.assertEqual(
            (by_name["metal.isolation"].needs, by_name["vulkan.isolation"].needs),
            ("plain", "plain"),
        )
        self.assertEqual(
            by_name["vulkan.window"].commands(),
            [["cargo", "test", "-p", "ck-gpu-vulkan", "--test", "window", *flags]],
        )
        self.assertEqual(
            (by_name["vulkan.gpu"].needs, by_name["vulkan.window"].needs), ("gpu", "window")
        )
        self.assertEqual(
            by_name["vulkan.gpu"].paths, ("crates/gpu-vulkan/", "apps/codekarl/tests/supervisor/")
        )
        self.assertEqual(
            by_name["vulkan.window"].paths,
            (
                "crates/gpu-vulkan/",
                "apps/codekarl/tests/supervisor/",
                "crates/window-glfw/tests/native/",
                "crates/window-glfw/tests/support/",
            ),
        )
        # CK-80: the application's glfw-vulkan path, a window group selected only by its own files.
        self.assertEqual(
            by_name["vulkan.app"].commands(),
            [["cargo", "test", "-p", "codekarl", "--test", "vulkan_app", *flags]],
        )
        self.assertEqual(by_name["vulkan.app"].needs, "window")
        self.assertEqual(
            by_name["vulkan.app"].paths,
            (
                "apps/codekarl/Cargo.toml",  # CK-99: carries the `vulkan` feature's wiring
                "apps/codekarl/build.rs",  # CK-159: compiles the keyboard filter the tests run behind
                "apps/codekarl/src/vulkan.rs",
                "apps/codekarl/src/vulkan/",
                "apps/codekarl/tests/native/",  # CK-159: its native source
                "apps/codekarl/tests/vulkan_app.rs",
                "config/demo-glfw-vulkan.toml",
                "config/demo-glfw-vulkan-decline.toml",
                "config/demo-glfw-vulkan-callback-error.toml",
                "config/demo-quad-vulkan.toml",  # CK-99
            ),
        )
        self.assertEqual(by_name["doctor.graphics"].category, "affected")
        self.assertEqual(by_name["doctor"].category, "floor")
        self.assertNotIn("test", by_name)  # CK-77: the one `cargo test` became per-unit groups
        self.assertNotIn("test-default", by_name)
        self.assertEqual(by_name["mypy"].commands(), [[check.hub_tool("mypy")]])  # S7a
        self.assertNotIn("unittest", by_name)  # CK-36: the single step became bounded groups
        for group, modules in check.UNITTEST_GROUPS:
            self.assertEqual(
                by_name[f"unittest-{group}"].commands(),
                [
                    [
                        sys.executable,
                        "-m",
                        "unittest",
                        "discover",
                        "-s",
                        "tests",
                        "-b",
                        "-p",
                        f"{module}.py",
                    ]
                    for module in modules
                ],
            )
        self.assertEqual(
            by_name["chat-ci"].commands(), [[sys.executable, "tools/chat/install.py", "ci"]]
        )
        self.assertNotIn("build", by_name)  # Phase B: no separate cargo build step ...
        self.assertNotIn("probes", by_name)  # ... and no Python executable probes

    def test_01a_each_selected_step_runs_once_in_order_without_running_cargo(self) -> None:
        for level in ALL:
            with self.subTest(level), mock.patch.object(check, "run") as run:
                with (
                    mock.patch.object(check, "doctor") as doctor,
                    mock.patch.object(check, "graphics_doctor") as graphics,
                    # The discovery reads the tracked files; nothing here depends on the hub's tools.
                    mock.patch.object(check, "capture", return_value="tools/bootstrap\n"),
                    mock.patch.object(check, "hub_tool", return_value=sys.executable),
                ):
                    check.checks(self.hub, level)
                self.assertEqual(doctor.call_count, 0 if level == "quick" else 1)
                self.assertEqual(graphics.call_count, 0 if level == "quick" else 1)
                names = [call.kwargs["step"].name for call in run.call_args_list]
                expected = [
                    n for n in check.select(level) if n not in {"doctor", "doctor.graphics", "ruff"}
                ]
                # a step with several commands (a unittest group) runs them one after another
                steps = [name for name, _calls in itertools.groupby(names) if name != "ruff"]
                self.assertEqual(steps, expected)
                self.assertEqual(names.count("ruff"), 2)  # check and format --check

    def test_01b_the_run_prints_its_recipe_header_for_the_batch_gate(self) -> None:
        # CK-38: the batch gate reads the watchdog and the steps from the run's own log.
        for level, watchdog in (("fast", 123.0), ("full", 2700.0)):
            with self.subTest(level):
                steps = self.step_source("pass", name="probe", levels=(level,))
                result = self.driver(steps, level, watchdog=watchdog)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                lines = result.stdout.splitlines()
                self.assertEqual(
                    lines[0], f"check: recipe {level}; watchdog {watchdog:g} s; steps probe"
                )
                self.assertIn("check: step probe took ", result.stdout)
                self.assertTrue(lines[-1].endswith("=> PASS"), lines[-1])
        header = f"check: recipe full; watchdog {check.WATCHDOG:g} s; steps {','.join(check.select('full'))}"
        with mock.patch.object(check, "checks"), contextlib.redirect_stdout(io.StringIO()) as out:
            check.execute(self.copy, self.hub, "full")
        self.assertEqual(out.getvalue().splitlines()[0], header)

    def test_02_output_is_quiet_when_passing_and_full_when_failing(self) -> None:
        tests = self.copy / "tests"
        tests.mkdir()
        (tests / "test_fixture.py").write_text(
            "import unittest\n\n\nclass Fixture(unittest.TestCase):\n"
            "    def test_quiet_name(self):\n        print('stdout of a passing test')\n"
        )
        unit = "[check.unittest_step('fixture', ('test_fixture',))]"
        passing = self.driver(unit, "fast", env=clean_environment())
        self.assertEqual(passing.returncode, 0, passing.stderr)
        self.assertNotIn("test_quiet_name", passing.stdout + passing.stderr)
        self.assertNotIn("stdout of a passing test", passing.stdout + passing.stderr)
        self.assertIn("-s tests -b", passing.stdout)
        self.assertNotIn(" -v", passing.stdout)
        self.assertIn("check fast: unittest-fixture ok => PASS", passing.stdout)
        (tests / "test_fixture.py").write_text(
            "import unittest\n\n\nclass Fixture(unittest.TestCase):\n"
            "    def test_loud_failure(self):\n        print('captured output')\n"
            "        self.fail('the assertion text')\n"
        )
        failing = self.driver(unit, "fast", env=clean_environment())
        self.assertEqual(failing.returncode, 1)
        shown = failing.stdout + failing.stderr
        for text in ("test_loud_failure", "the assertion text", "captured output"):
            self.assertIn(text, shown)
        self.assertIn("check fast: unittest-fixture FAILED => FAIL", failing.stdout)

    def test_02b_a_passing_suite_prints_no_nested_suite_summaries(self) -> None:
        # Tests that run sub-suites (the staged chat fixtures) must capture them: only the outer
        # `Ran N tests` summary may appear in a passing run.
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "unittest",
                "discover",
                "-s",
                "tests",
                "-b",
                "-p",
                "test_chat_ci.py",
            ],
            check=False,
            cwd=REPO,
            env=clean_environment(),
            capture_output=True,
            text=True,
            timeout=300,
        )
        self.assertEqual(result.returncode, 0, result.stderr[-2000:])
        self.assertEqual(result.stdout, "")
        self.assertEqual(result.stderr.count("Ran "), 1, result.stderr[-2000:])

    def test_02c_a_failing_nested_suite_shows_its_failure_output_in_full(self) -> None:
        quiet = chatfix.quiet_run(subprocess.run)
        failing = [
            sys.executable,
            "-c",
            "import sys; print('NESTED STDOUT'); print('NESTED FAILURE DIAGNOSTIC', file=sys.stderr);"
            " sys.exit(3)",
        ]
        passing = [
            sys.executable,
            "-c",
            "import sys; print('quiet'); print('quiet', file=sys.stderr)",
        ]
        for label, kwargs in (("exit code", {}), ("check=True", {"check": True})):
            with self.subTest(label):
                out, err = io.StringIO(), io.StringIO()
                with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                    if kwargs:
                        with self.assertRaises(subprocess.CalledProcessError):
                            quiet(failing, **kwargs)
                    else:
                        self.assertEqual(quiet(failing).returncode, 3)
                self.assertIn("NESTED STDOUT", out.getvalue())
                self.assertIn("NESTED FAILURE DIAGNOSTIC", err.getvalue())
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            self.assertEqual(quiet(passing).returncode, 0)
        self.assertEqual((out.getvalue(), err.getvalue()), ("", ""))
        # A caller that routes any stream itself keeps full control of them: capturing on top of
        # stdout=, stderr= or capture_output= would raise ValueError.
        routed = quiet(passing, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        self.assertEqual(sorted(routed.stdout.split()), ["quiet", "quiet"])
        only_stderr = quiet(passing, stderr=subprocess.DEVNULL)
        self.assertEqual(only_stderr.returncode, 0)
        only_stdout = quiet(passing, stdout=subprocess.DEVNULL)
        self.assertEqual(only_stdout.returncode, 0)
        explicit = quiet(passing, capture_output=True, text=True)
        self.assertEqual(sorted(explicit.stdout.split()), ["quiet"])

    def test_02a_the_summary_names_every_step_and_the_ones_not_run(self) -> None:
        steps = (
            "[Step('one', ('full',), lambda: [[sys.executable, '-c', 'pass']]),"
            " Step('two', ('full',), lambda: [[sys.executable, '-c', 'raise SystemExit(3)']]),"
            " Step('three', ('full',), lambda: [[sys.executable, '-c', 'pass']])]"
        )
        result = self.driver(steps, "full", env=clean_environment())
        self.assertEqual(result.returncode, 1)
        self.assertIn("check full: one ok, two FAILED, three not run => FAIL", result.stdout)


class UnittestGroups(CheckFixture):
    """CK-36: the Python suite runs as bounded groups, and no test module can fall out of them."""

    def fixture_tests(self, extra: Iterable[str] = ()) -> Path:
        """A fixture tests directory holding exactly the modules the groups name, plus `extra`."""
        tests = self.copy / "tests"
        tests.mkdir()
        for _group, modules in check.UNITTEST_GROUPS:
            for module in modules:
                (tests / f"{module}.py").write_text("")
        for name in extra:
            (tests / name).write_text("")
        return tests

    def test_01_the_groups_cover_the_real_tests_directory_exactly(self) -> None:
        self.assertEqual(check.group_problems(REPO / "tests"), [])
        names = [module for _group, modules in check.UNITTEST_GROUPS for module in modules]
        self.assertEqual(len(names), len(set(names)))

    def test_02_a_module_in_no_group_fails_naming_it_whatever_its_filename_style(self) -> None:
        for name in ("test_extra.py", "testextra.py"):  # `discover` loads both (pattern test*.py)
            with self.subTest(name):
                tests = self.fixture_tests([name])
                with self.assertRaises(RuntimeError) as caught:
                    check.verify_groups(tests)
                self.assertIn(name[:-3], str(caught.exception))
                self.assertIn("in no unittest group", str(caught.exception))
                (tests / name).unlink()
                shutil.rmtree(tests)

    def test_03_a_module_listed_twice_fails_naming_it(self) -> None:
        tests = self.fixture_tests()
        groups = (*check.UNITTEST_GROUPS, ("again", ("test_gc",)))
        with self.assertRaises(RuntimeError) as caught:
            check.verify_groups(tests, groups)
        self.assertIn("test_gc", str(caught.exception))
        self.assertIn("more than one unittest group", str(caught.exception))

    def test_04_a_group_naming_a_module_that_does_not_exist_fails(self) -> None:
        tests = self.fixture_tests()
        groups = (*check.UNITTEST_GROUPS, ("stale", ("test_gone",)))
        with self.assertRaises(RuntimeError) as caught:
            check.verify_groups(tests, groups)
        self.assertIn("test_gone", str(caught.exception))

    def test_05_a_package_of_tests_cannot_slip_past_the_groups(self) -> None:
        tests = self.fixture_tests()
        (tests / "pkg").mkdir()
        (tests / "pkg/__init__.py").write_text("")
        with self.assertRaises(RuntimeError) as caught:
            check.verify_groups(tests)
        self.assertIn("pkg", str(caught.exception))

    def test_06_a_run_with_an_uncovered_module_fails_before_any_step_runs(self) -> None:
        self.fixture_tests(["test_extra.py"])
        state = check.State(self.copy, self.hub, "fast", batch=False, target=self.copy / "target")
        for level in ("fast", "full"):
            with (
                self.subTest(level),
                mock.patch.object(check, "_state", state),
                mock.patch.object(check, "run") as run,
                self.assertRaises(RuntimeError) as caught,
            ):
                check.checks(self.hub, level)
            self.assertIn("test_extra", str(caught.exception))
            run.assert_not_called()  # no step ran
        # Quick has no unittest step, and a custom step list is not the real recipe: no guard.
        with (
            mock.patch.object(check, "_state", state),
            mock.patch.object(check, "run") as run,
        ):
            check.checks(self.hub, "quick", steps=[check.Step("probe", ALL, action=lambda: None)])
            run.assert_not_called()

    def test_07_the_union_of_the_groups_equals_what_discover_loads(self) -> None:
        # Test ids by loader inspection in a separate process (it imports every test module).
        code = textwrap.dedent(
            f"""
            import json, sys, unittest
            sys.path.insert(0, {str(TOOLS)!r})
            import check

            def ids(suite):
                for item in suite:
                    if isinstance(item, unittest.TestSuite):
                        yield from ids(item)
                    else:
                        yield item.id()

            loader = unittest.TestLoader()
            everything = sorted(ids(loader.discover("tests")))
            grouped = []
            for _group, modules in check.UNITTEST_GROUPS:
                for module in modules:
                    grouped += list(ids(loader.discover("tests", pattern=module + ".py")))
            print(json.dumps({{"all": everything, "grouped": sorted(grouped)}}))
            """
        )
        done = subprocess.run(
            [sys.executable, "-c", code],
            capture_output=True,
            text=True,
            check=False,
            cwd=REPO,
            env=clean_environment(),
            timeout=300,
        )
        self.assertEqual(done.returncode, 0, done.stderr[-2000:])
        found = json.loads(done.stdout.splitlines()[-1])
        self.assertGreater(len(found["all"]), 400)
        self.assertEqual(found["grouped"], found["all"])  # every test, exactly once


class MypyStep(CheckFixture):
    """S7: the strict mypy step and its configuration; the S7 migration is complete, so no module is
    exempt from it and no temporary list remains."""

    def configuration(self) -> dict[str, object]:
        return tomllib.loads((REPO / "pyproject.toml").read_text())

    def test_01_mypy_is_strict_and_covers_tools_and_tests(self) -> None:
        mypy = table(table(self.configuration()["tool"])["mypy"])
        self.assertIs(mypy["strict"], True)
        self.assertEqual(mypy["mypy_path"], "tools:stubs")
        self.assertEqual(mypy["files"], ["tools", "tests", "tools/ck"])
        self.assertEqual(mypy["exclude"], ["tests/fixtures/legacy/"])
        text = (REPO / "pyproject.toml").read_text()
        # the entry points mypy cannot take carry their reason; the temporary list is gone
        self.assertNotIn("S7 migration pending", text)

    def test_02_no_temporary_mypy_list_remains(self) -> None:
        mypy = table(table(self.configuration()["tool"])["mypy"])
        self.assertNotIn("overrides", mypy)  # no module is exempt from the strict check
        self.assertIs(mypy["strict"], True)
        text = (REPO / "pyproject.toml").read_text()
        self.assertNotIn("ignore_errors", text)

    def test_03_ruff_ann_is_ignored_for_nothing_but_the_legacy_fixture(self) -> None:
        ruff = table(table(table(self.configuration()["tool"])["ruff"])["lint"])
        self.assertIn("ANN", names(ruff["select"]))
        ignored = table(ruff["per-file-ignores"])
        self.assertEqual(set(ignored), {"tests/fixtures/legacy/**"})  # the one documented exclusion
        # every test module is covered by the lint: no other path may switch ANN off
        for path, codes in ignored.items():
            if "ANN" in names(codes):
                self.assertEqual(path, "tests/fixtures/legacy/**")

    def test_04_a_type_error_in_any_module_of_tools_or_tests_fails_the_step(
        self,
    ) -> None:
        # A fixture project with the real configuration: the same error fails the step in a module
        # of `tools` and in a module of `tests` (the last of the formerly exempt ones).
        def mypy_in(root: Path) -> subprocess.CompletedProcess[str]:
            return subprocess.run(
                [check.hub_tool("mypy")],
                cwd=root,
                capture_output=True,
                text=True,
                check=False,
                env=clean_environment(),
                timeout=300,
            )

        error = "def broken() -> int:\n    return 'not an int'\n"
        for module in ("tools/gate/pointer", "tests/test_gc"):
            with self.subTest(module):
                root = self.base / f"fixture-{module.replace('/', '-')}"
                (root / "tools/gate").mkdir(parents=True)
                (root / "tests").mkdir()
                (root / "tests/test_placeholder.py").write_text("VALUE = 1\n")  # mypy needs a file
                shutil.copy2(REPO / "pyproject.toml", root / "pyproject.toml")
                (root / "tools/gate/__init__.py").write_text("")
                (root / "tools/ck").write_text("VALUE = 1\n")  # the script the configuration lists
                for name in ("tools/gate/pointer", "tests/test_gc"):
                    (root / f"{name}.py").write_text(
                        "VALUE = 1\n" + (error if name == module else "")
                    )
                result = mypy_in(root)
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertIn(f"{module}.py", result.stdout)
                self.assertIn("Incompatible return value type", result.stdout)

    def test_10_the_toml_helpers_of_the_lint_tests_hand_back_checked_values(self) -> None:
        # The parsed TOML is checked where it is read: a caller that misuses a value fails mypy
        # instead of failing only when the test runs (review of S7e submission 1).
        root = self.base / "toml-helpers"
        shutil.rmtree(root, ignore_errors=True)
        (root / "tools").mkdir(parents=True)
        (root / "tests").mkdir()
        (root / "tools/ck").write_text("VALUE = 1\n")  # the script the configuration lists
        (root / "tools/placeholder.py").write_text("VALUE = 1\n")  # mypy needs a file in tools
        shutil.copy2(REPO / "pyproject.toml", root / "pyproject.toml")
        for name in ("test_rust_lints", "test_lint_policy"):
            shutil.copy2(REPO / f"tests/{name}.py", root / f"tests/{name}.py")
        (root / "tests/probe_toml.py").write_text(
            "from test_lint_policy import Configuration\n"
            "from test_rust_lints import WorkspaceLints, manifest\n\n"
            'manifest("Cargo.toml")["workspace"].does_not_exist()\n'
            'WorkspaceLints().lints()["clippy"].does_not_exist()\n'
            "case = Configuration()\n"
            'case.config["lint"].does_not_exist()\n'
        )
        result = subprocess.run(
            [check.hub_tool("mypy")],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
            env=clean_environment(),
            timeout=300,
        )
        self.assertEqual(result.returncode, 1, result.stdout)
        found = [
            line
            for line in result.stdout.splitlines()
            if line.startswith("tests/probe_toml.py") and ": error:" in line
        ]
        self.assertEqual(len(found), 3, result.stdout)
        for line in found:
            self.assertIn('has no attribute "does_not_exist"', line)

    PROBE = """\
from pathlib import Path

from gate import meta, pointer, reviews, sessions

hub = Path("/hub")
store = meta.Store(hub)
{body}
"""

    def probe(
        self, name: str, body: str, edits: Iterable[tuple[str, str, str]] = ()
    ) -> subprocess.CompletedProcess[str]:
        """mypy over the real tools and one probe module that uses the migrated readers."""
        root = self.base / f"probe-{name}"
        shutil.rmtree(root, ignore_errors=True)  # a subtest may reuse the name
        shutil.copytree(
            REPO / "tools", root / "tools", ignore=shutil.ignore_patterns("__pycache__", "*.pyc")
        )
        shutil.copytree(REPO / "stubs", root / "stubs")
        (root / "tests").mkdir(parents=True)
        for path, old, new in edits:  # a deliberate change to the copied tools
            text = (root / path).read_text()
            self.assertIn(old, text)
            (root / path).write_text(text.replace(old, new, 1))
        (root / "tests/test_probe.py").write_text(self.PROBE.format(body=body))
        shutil.copy2(REPO / "pyproject.toml", root / "pyproject.toml")
        return subprocess.run(
            [check.hub_tool("mypy")],
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
            env=clean_environment(),
            timeout=300,
        )

    @staticmethod
    def probe_errors(result: subprocess.CompletedProcess[str]) -> list[str]:
        """The diagnostics about the probe module only: a type error elsewhere in the tools is the
        mypy step's to report, not this test's."""
        return [
            line for line in result.stdout.splitlines() if line.startswith("tests/test_probe.py")
        ]

    def test_05_the_migrated_readers_return_typed_records_that_reject_misuse(self) -> None:
        # Each body misuses a value a migrated reader returns; mypy must reject it by name, so the
        # field types reach callers (the review's probe: `read_pointer(hub)["repo"]` as an int).
        misuses = {
            "pointer-repo-as-int": (
                'repo: int = pointer.read_pointer(hub)["repo"]',
                "Incompatible types in assignment",
            ),
            "pointer-misspelled-method": (
                "pointer.read_pointer(hub).hashse()",
                'has no attribute "hashse"',
            ),
            "pointer-unknown-field": (
                'pointer.read_pointer(hub)["repo_path"]',
                "TypedDict",
            ),
            "issue-unknown-field": (
                'store.issues()[0]["titel"]',
                "TypedDict",
            ),
            "issue-state-as-int": (
                'state: int = store.issues()[0]["state"]',
                "Incompatible types in assignment",
            ),
            "event-kind-as-int": (
                'kind: int = store.events()[0]["kind"]',
                "Incompatible types in assignment",
            ),
            "receipt-verdict-as-int": (
                'verdict: int = reviews.receipt_rows(store)[0]["verdict"]',
                "Incompatible types in assignment",
            ),
            "bootstrap-fingerprint-hashes-misuse": (
                "from gate.records import load_bootstrap_validated\n"
                "validated = load_bootstrap_validated({}, 'validated.json')\n"
                'validated["candidate_fingerprint"]["hashes"].does_not_exist()',
                'has no attribute "does_not_exist"',
            ),
            "session-client-as-int": (
                "from gate.records import is_agent\n"
                "session = sessions.resolve()\n"
                "if is_agent(session):\n"
                '    n: int = session["client"]',
                "Incompatible types in assignment",
            ),
        }
        for name, (body, message) in misuses.items():
            with self.subTest(name):
                result = self.probe(name, body)
                found = self.probe_errors(result)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertTrue(found, result.stdout)
                self.assertIn(message, "\n".join(found))
        correct = (
            'repo: str = pointer.read_pointer(hub)["repo"]\n'
            'state: str = store.issues()[0]["state"]\n'
            'kind: str = store.events()[0]["kind"]\n'
            'verdict: str = reviews.receipt_rows(store)[0]["verdict"]\n'
            'submissions: list[str] = pointer.read_pointer(hub).get("submissions", [])'
        )
        result = self.probe("correct", correct)
        self.assertEqual(self.probe_errors(result), [], result.stdout + result.stderr)

    def test_06_the_workflow_readers_return_typed_records_that_reject_misuse(self) -> None:
        # The S7b readers: batch state, the eligible rows, the docs queue, the pipeline contract and
        # the subprocess wrapper each hand callers a typed value, so misuse is a mypy error.
        header = (
            "from gate import batch, docsqueue, lease, pipeline_api\n"
            "from gate.records import load_ci_receipt, read_json\n"
            "directory = Path('/d')\n"
        )
        misuses = {
            "validated-members-as-int": (
                "n: int = batch.read_validated(directory)['members']",
                "Incompatible types in assignment",
            ),
            "validated-unknown-field": (
                "batch.read_validated(directory)['member']",
                "TypedDict",
            ),
            "eligible-id-as-int": (
                "n: int = batch.Manager(hub).eligible()[0]['id']",
                "Incompatible types in assignment",
            ),
            "docs-state-as-int": (
                "n: int = docsqueue._queue(hub)[0]['state']",
                "Incompatible types in assignment",
            ),
            "batch-subject-as-int": (
                "n: int = batch.subject(hub, 'b')['batch']",
                "Incompatible types in assignment",
            ),
            "fingerprint-hashes-as-int": (
                "n: int = pipeline_api.load().fingerprint(directory)['hashes']",
                "Incompatible types in assignment",
            ),
            "pull-result-as-str": (
                "n: str = pipeline_api.load().pull(directory, directory, [])",
                "Incompatible types in assignment",
            ),
            "text-output-is-str": (
                "lease.run(['x'], text=True).stdout.decode()",
                'has no attribute "decode"',
            ),
            "bytes-output-is-bytes": (
                "lease.run(['x']).stdout.strip() + 'x'",
                "Unsupported operand types",
            ),
        }
        for name, (body, message) in misuses.items():
            with self.subTest(name):
                result = self.probe("s7b-" + name, header + body)
                found = self.probe_errors(result)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertTrue(found, result.stdout)
                self.assertIn(message, "\n".join(found))
        correct = header + (
            "members: list[str] = batch.read_validated(directory)['members']\n"
            "ident: str = batch.Manager(hub).eligible()[0]['id']\n"
            "state: str = docsqueue._queue(hub)[0]['state']\n"
            "hashes: list[str] = pipeline_api.load().fingerprint(directory)['hashes']\n"
            "out: str = lease.run(['x'], text=True).stdout\n"
            "raw: bytes = lease.run(['x']).stdout\n"
            "passed: bool = load_ci_receipt(read_json(directory / 'x'), 'x')['passed']\n"
        )
        result = self.probe("s7b-correct", correct)
        self.assertEqual(self.probe_errors(result), [], result.stdout + result.stderr)

    def test_08_the_pipeline_contract_is_a_real_check_since_pipeline_is_typed(self) -> None:
        # Changing a return type in the Protocol, or in pipeline.py, is now a mypy error at the
        # assignment in `pipeline_api.load` (it was accepted whatever the signature while
        # pipeline.py was untyped).
        for path, old, new in (
            (
                "tools/gate/pipeline_api.py",
                "def hashes(self, repo: Path) -> set[str]: ...",
                "def hashes(self, repo: Path) -> list[str]: ...",
            ),
            (
                "tools/pipeline.py",
                "def hashes(repo: Path) -> set[str]:",
                "def hashes(repo: Path) -> frozenset[str]:",
            ),
        ):
            with self.subTest(path):
                result = self.probe("contract-" + Path(path).stem, "", edits=[(path, old, new)])
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn("pipeline_api.py", result.stdout)
                self.assertIn("hashes", result.stdout)

    def test_09_a_type_error_in_the_executed_entry_point_logic_fails_mypy(self) -> None:
        # tools/ck is checked as a script: a deliberate error in logic it executes (not in a dead
        # corner) is a mypy failure naming tools/ck, and so is one in the checkpoint-side launcher.
        for path, old, new in (
            (
                "tools/ck",
                "def verify(hub: Path, state: State) -> Path:",
                "def verify(hub: Path, state: State) -> int:",
            ),
            (
                "tools/ck",
                "launch(verify(hub, state), root, args)",
                "launch(verify(hub, state), 7, args)",
            ),
            (
                "tools/launcher.py",
                "                cli.main(copy, hub)",
                "                cli.main(copy, 3)",
            ),
        ):
            with self.subTest(path + " " + old[:30]):
                result = self.probe("entry-" + Path(path).stem, "", edits=[(path, old, new)])
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn(path + ":", result.stdout)

    def test_07_the_subprocess_wrapper_agrees_with_subprocess_on_text_and_bytes(self) -> None:
        # Review of S7b: every option that selects text mode (text, encoding, errors,
        # universal_newlines) gives str, none gives bytes, an option the wrapper does not support
        # is refused, and the paired runtime behaviour is in tests/test_lease.py (TextModes).
        header = "from gate import lease\n"
        misuses = {
            "encoding-is-not-bytes": (
                "raw: bytes = lease.check_output(['echo', 'ok'], encoding='utf-8')",
                "Incompatible types in assignment",
            ),
            "run-encoding-is-not-bytes": (
                "out: bytes = lease.run(['echo'], encoding='utf-8', capture_output=True).stdout",
                "Incompatible types in assignment",
            ),
            "errors-is-not-bytes": (
                "out: bytes = lease.run(['echo'], errors='strict').stdout",
                "Incompatible types in assignment",
            ),
            "universal-newlines-is-not-bytes": (
                "out: bytes = lease.check_output(['echo'], universal_newlines=True)",
                "Incompatible types in assignment",
            ),
            "default-is-not-str": (
                "out: str = lease.run(['echo'], capture_output=True).stdout",
                "Incompatible types in assignment",
            ),
            "check-output-default-is-not-str": (
                "out: str = lease.check_output(['echo'])",
                "Incompatible types in assignment",
            ),
            "empty-encoding-is-not-str": (
                "out: str = lease.check_output(['x'], encoding='')",
                "Incompatible types in assignment",
            ),
            "dynamic-errors-is-not-str": (
                "def f(handler: str) -> str:\n    return lease.check_output(['x'], errors=handler)",
                "Incompatible return value type",
            ),
            "other-codec-is-not-str": (
                "out: str = lease.check_output(['x'], encoding='latin-1')",
                "Incompatible types in assignment",
            ),
            "unsupported-option": (
                "lease.run(['echo'], shell=True)",
                'Unexpected keyword argument "shell"',
            ),
        }
        for name, (body, message) in misuses.items():
            with self.subTest(name):
                result = self.probe("lease-" + name, header + body)
                found = self.probe_errors(result)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertTrue(found, result.stdout)
                self.assertIn(message, "\n".join(found))
        correct = header + (
            "a: str = lease.run(['x'], text=True, input='i').stdout\n"
            "b: str = lease.run(['x'], encoding='utf-8', capture_output=True).stdout\n"
            "c: str = lease.run(['x'], errors='strict').stdout\n"
            "d: str = lease.run(['x'], universal_newlines=True).stdout\n"
            "e: bytes = lease.run(['x'], capture_output=True, input=b'i').stdout\n"
            "f: str = lease.check_output(['x'], encoding='utf-8')\n"
            "g: bytes = lease.check_output(['x'])\n"
            "def either(flag: bool) -> str | bytes:\n"
            "    return lease.run(['x'], text=flag).stdout\n"
            "h: str = lease.check_output(['x'], text=True, cwd='/tmp', pass_fds=(1,))\n"
            "def named(codec: str) -> str | bytes:\n"
            "    return lease.check_output(['x'], encoding=codec)  # may be empty, so not str\n"
        )
        result = self.probe("lease-correct", correct)
        self.assertEqual(self.probe_errors(result), [], result.stdout + result.stderr)


class Locks(CheckFixture):
    def test_03_a_manual_full_check_ignores_a_batch_holding_ci_lock(self) -> None:
        record = self.hub / ".pipeline/checkpoints/leak.chat-inputs.json"
        steps = (
            "[Step('chat-ci', ('full',), lambda: [[sys.executable, '-c', "
            '\'import os; p = os.environ.get("CK_CHAT_INPUTS"); '
            'open(p, "w").write("x") if p else None; print("ran")\']])]'
        )
        before = self.listing()
        with locks.ci(self.hub):
            started = time.monotonic()
            result = self.driver(steps, "full", env=clean_environment(CK_CHAT_INPUTS=str(record)))
            elapsed = time.monotonic() - started
            # A second batch prepare is still rejected while the first holds the slot.
            with (
                self.assertRaisesRegex(RuntimeError, "Already running"),
                locks.ci(self.hub),
            ):
                pass
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertLess(elapsed, 20)
        self.assertIn("ran", result.stdout)
        self.assertFalse(record.exists())
        self.assertEqual(self.listing(), before)
        self.assertIn("check full: chat-ci ok => PASS", result.stdout)
        self.assertIn("check: manual run; project lock held: none", result.stdout)

    def test_03a_manual_runs_never_hold_or_wait_for_ci_lock(self) -> None:
        steps = self.step_source("pass", levels=ALL)
        for level in ALL:
            with self.subTest(level):
                result = self.driver(steps, level, env=clean_environment())
                self.assertEqual(result.returncode, 0, result.stderr)
        # Nothing the runs left behind keeps the slot: a batch can take it at once.
        with locks.ci(self.hub):
            pass

    def test_04_fast_runs_in_two_copies_while_a_batch_holds_the_slot(self) -> None:
        copies = []
        for name in ("one", "two"):
            copy = self.base / name
            copy.mkdir()
            copies.append(copy)
        steps = self.step_source("import time; time.sleep(1)", levels=ALL)
        with locks.ci(self.hub):
            started = time.monotonic()
            processes = [
                subprocess.Popen(
                    [sys.executable, "-c", self.driver_code(steps, "fast", copy)],
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                    env=clean_environment(),
                )
                for copy in copies
            ]
            outputs = [process.communicate(timeout=60) for process in processes]
            elapsed = time.monotonic() - started
        self.assertEqual([process.returncode for process in processes], [0, 0])
        self.assertLess(elapsed, 1.9)  # both ran at once
        for stdout, _ in outputs:
            self.assertIn("check fast: probe ok => PASS", stdout)

    def driver_code(self, steps: str, level: str, root: Path) -> str:
        return textwrap.dedent(
            f"""
            import signal, sys
            from pathlib import Path
            sys.path.insert(0, {str(TOOLS)!r})
            import check
            from check import Step
            signal.signal(signal.SIGTERM, check.terminate)
            sys.exit(check.execute(Path({str(root)!r}), Path({str(self.hub)!r}),
                                   {level!r}, steps={steps}))
            """
        )

    def test_05_an_invalid_inherited_descriptor_fails_at_once(self) -> None:
        steps = self.step_source("raise SystemExit('must not run')")
        for value in ("999", "not-a-number", "-1", "0"):
            with self.subTest(value):
                started = time.monotonic()
                result = self.driver(steps, "full", env=clean_environment(CK_CI_LOCK_FD=value))
                self.assertEqual(result.returncode, 1)
                self.assertLess(time.monotonic() - started, 10)
                self.assertIn("CK_CI_LOCK_FD", result.stderr)
                self.assertIn("restart the command as: tools/ck check", result.stderr)
                self.assertNotIn("must not run", result.stdout + result.stderr)

    def test_05a_a_valid_inherited_descriptor_makes_a_batch(self) -> None:
        facts = self.base / "facts.json"
        body = (
            "import json, os; "
            f"json.dump({{'target': os.environ.get('CARGO_TARGET_DIR'), "
            "'record': os.environ.get('CK_CHAT_INPUTS'), "
            f"'fd': os.environ.get('CK_CI_LOCK_FD')}}, open({str(facts)!r}, 'w'))"
        )
        steps = self.step_source(body)
        descriptor = os.open(self.hub / ".pipeline/ci.lock", os.O_RDWR)
        self.addCleanup(os.close, descriptor)
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        record = self.hub / ".pipeline/checkpoints/batch.chat-inputs.json"
        result = self.driver(
            steps,
            "full",
            env=clean_environment(CK_CI_LOCK_FD=str(descriptor), CK_CHAT_INPUTS=str(record)),
            pass_fds=(descriptor,),
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        seen = json.loads(facts.read_text())
        self.assertEqual(seen["target"], str(self.copy / "target"))  # batch: the candidate's target
        self.assertEqual(seen["record"], str(record))  # a batch keeps CK-6's record path
        self.assertEqual(seen["fd"], str(descriptor))
        self.assertIn("check: batch run; project lock held: ci.lock", result.stdout)
        self.assertFalse((self.copy / "target/ck-full").exists())  # and takes no manual slot


class BuildDirectories(CheckFixture):
    def environment_of(self, level: str, **inherited: str) -> dict[str, str]:
        state = check.State(
            self.copy, self.hub, level, batch=False, target=self.copy / "target/ck-full/0"
        )
        with mock.patch.dict(os.environ, inherited):
            return state.environment(None)

    def test_06_inherited_cargo_variables_are_overridden(self) -> None:
        env = self.environment_of(
            "full",
            CARGO_TARGET_DIR="/elsewhere/a",
            CARGO_BUILD_BUILD_DIR="/elsewhere/b",
            CARGO_BUILD_TARGET_DIR="/elsewhere/c",
        )
        expected = str(self.copy / "target/ck-full/0")
        self.assertEqual((env["CARGO_TARGET_DIR"], env["CARGO_BUILD_BUILD_DIR"]), (expected,) * 2)
        self.assertNotIn("CARGO_BUILD_TARGET_DIR", env)

    def test_06a_the_config_equivalents_are_overridden_by_the_real_cargo(self) -> None:
        crate = self.base / "crate"
        (crate / "src").mkdir(parents=True)
        (crate / "src/main.rs").write_text("fn main() {}\n")
        (crate / "Cargo.toml").write_text('[package]\nname = "fixture"\nversion = "0.0.0"\n')
        (crate / ".cargo").mkdir()
        (crate / ".cargo/config.toml").write_text(
            '[build]\ntarget-dir = "from-config/target"\nbuild-dir = "from-config/build"\n'
        )
        slot = self.base / "slot"
        for label, inherited in (
            ("none", {}),
            ("CARGO_TARGET_DIR", {"CARGO_TARGET_DIR": "/inherited/target"}),
            ("CARGO_BUILD_BUILD_DIR", {"CARGO_BUILD_BUILD_DIR": "/inherited/build"}),
            ("CARGO_BUILD_TARGET_DIR", {"CARGO_BUILD_TARGET_DIR": "/inherited/legacy"}),
        ):
            with self.subTest(label):
                state = check.State(crate, self.hub, "full", batch=False, target=slot)
                with mock.patch.dict(os.environ, inherited):
                    env = state.environment(None)
                result = subprocess.run(
                    ["cargo", "metadata", "--format-version", "1", "--no-deps", "--offline"],
                    check=False,
                    cwd=crate,
                    env=env,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout)["target_directory"], str(slot))

    def test_06b_quick_and_fast_use_the_copy_default_target(self) -> None:
        for level in ("quick", "fast"):
            with self.subTest(level):
                state = check.State(
                    self.copy, self.hub, level, batch=False, target=self.copy / "target"
                )
                with mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": "/elsewhere"}):
                    env = state.environment(None)
                self.assertEqual(env["CARGO_TARGET_DIR"], str(self.copy / "target"))
                self.assertEqual(env["CARGO_BUILD_BUILD_DIR"], str(self.copy / "target"))

    def test_06c_full_spec_is_set_only_for_the_full_steps(self) -> None:
        by_name = {step.name: step for step in check.STEPS}
        state = check.State(self.copy, self.hub, "full", batch=False, target=self.copy / "target")
        fast = check.State(self.copy, self.hub, "fast", batch=False, target=self.copy / "target")
        with mock.patch.dict(os.environ):
            os.environ.pop("CK_REQUIRE_FULL_SPEC", None)  # the caller's own value is not under test
            self.assertEqual(
                [n for n, s in by_name.items() if state.environment(s).get("CK_REQUIRE_FULL_SPEC")],
                [name for name in by_name if name in FULL_SPEC_STEPS],
            )
            self.assertFalse(
                any(fast.environment(s).get("CK_REQUIRE_FULL_SPEC") for s in check.STEPS)
            )

    def test_06c2_the_featureless_test_step_never_sees_the_full_spec_or_analyzer_variables(
        self,
    ) -> None:
        """CK-44: `CK_REQUIRE_FULL_SPEC` and `CK_NATIVE_ANALYZE` are absent from `test-default` even
        when the caller exported both (the environment is copied from the parent), while every other
        step keeps the contract it had."""
        by_name = {step.name: step for step in check.STEPS}
        for level in ("fast", "full"):
            state = check.State(
                self.copy, self.hub, level, batch=False, target=self.copy / "target"
            )
            with mock.patch.dict(
                os.environ, {"CK_REQUIRE_FULL_SPEC": "1", "CK_NATIVE_ANALYZE": "1"}
            ):
                env = state.environment(by_name["rust.app-default"])
                self.assertNotIn("CK_REQUIRE_FULL_SPEC", env, level)
                self.assertNotIn("CK_NATIVE_ANALYZE", env, level)
                # Every other step is unchanged: the caller's full-spec value still reaches it, and
                # the analyzer variable is set only for the steps that share the build-script unit.
                for name, step in by_name.items():
                    if name == "rust.app-default":
                        continue
                    other = state.environment(step)
                    self.assertEqual(other.get("CK_REQUIRE_FULL_SPEC"), "1", (level, name))
                    self.assertEqual(
                        other.get("CK_NATIVE_ANALYZE"),
                        "1" if name in ANALYZER_STEPS else None,
                        (level, name),
                    )
            with mock.patch.dict(os.environ):
                os.environ.pop("CK_REQUIRE_FULL_SPEC", None)
                os.environ.pop("CK_NATIVE_ANALYZE", None)
                clean = state.environment(by_name["rust.app-default"])
                self.assertNotIn("CK_REQUIRE_FULL_SPEC", clean, level)
                self.assertNotIn("CK_NATIVE_ANALYZE", clean, level)

    def test_06d_two_concurrent_manual_runs_take_different_slots_and_neither_waits(self) -> None:
        facts = self.base / "facts"
        facts.mkdir()
        body = (
            "import os, time; "
            f"open({str(facts)!r} + '/' + str(os.getpid()), 'w')"
            ".write(os.environ['CARGO_TARGET_DIR']); time.sleep(2)"
        )
        code = self.driver_source(self.step_source(body, levels=("full",)))
        started = time.monotonic()
        processes = [
            subprocess.Popen(
                [sys.executable, "-c", code],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                env=clean_environment(),
            )
            for _ in range(2)
        ]
        for process in processes:
            process.communicate(timeout=60)
        elapsed = time.monotonic() - started
        self.assertEqual([p.returncode for p in processes], [0, 0])
        self.assertLess(elapsed, 3.8)  # concurrent, not serial
        used = sorted(path.read_text() for path in facts.iterdir())
        self.assertEqual(
            used,
            [str(self.copy / "target/ck-full/0"), str(self.copy / "target/ck-full/1")],
        )

    def driver_source(self, steps: str) -> str:
        return textwrap.dedent(
            f"""
            import signal, sys
            from pathlib import Path
            sys.path.insert(0, {str(TOOLS)!r})
            import check
            from check import Step
            signal.signal(signal.SIGTERM, check.terminate)
            sys.exit(check.execute(Path({str(self.copy)!r}), Path({str(self.hub)!r}),
                                   'full', steps={steps}))
            """
        )


def process_state(pid: int) -> str:
    """The ps state of a pid ('' when it is gone; 'Z' while it is an unreaped zombie)."""
    return subprocess.run(
        ["ps", "-o", "stat=", "-p", str(pid)], check=False, capture_output=True, text=True
    ).stdout.strip()


def slot_free(path: Path) -> bool:
    descriptor = os.open(path, os.O_RDWR | os.O_CREAT)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        return False
    finally:
        os.close(descriptor)
    return True


def wait_for_pids(pidfile: Path, count: int, timeout: float = 30.0) -> list[int]:
    """The PIDs a fixture step wrote, once the file holds exactly `count` integers. The writers create
    the file before they write to it, so existence alone can show an empty file."""
    deadline = time.monotonic() + timeout
    content = None
    while True:
        with contextlib.suppress(FileNotFoundError):
            content = pidfile.read_text()
            fields = content.split()
            # ASCII digits only: `isdigit` alone accepts characters such as "²" that `int` rejects.
            if len(fields) == count and all(
                field.isascii() and field.isdigit() for field in fields
            ):
                return [int(field) for field in fields]
        if time.monotonic() >= deadline:
            raise AssertionError(
                f"{pidfile} did not hold {count} PIDs within {timeout} s; last content: {content!r}"
            )
        time.sleep(0.01)


class PidFiles(unittest.TestCase):
    """`wait_for_pids` waits for the whole content, not just the file (the CK-70 flake)."""

    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.pidfile = Path(directory.name) / "step.pids"

    def test_a_file_created_empty_and_filled_later_gives_its_pids(self) -> None:
        self.pidfile.write_text("")
        timer = threading.Timer(0.1, lambda: self.pidfile.write_text("123 456"))
        timer.start()
        self.addCleanup(timer.cancel)
        self.assertEqual(wait_for_pids(self.pidfile, 2, timeout=10.0), [123, 456])

    def test_a_file_that_stays_empty_fails_with_an_assertion(self) -> None:
        self.pidfile.write_text("")
        started = time.monotonic()
        with self.assertRaisesRegex(AssertionError, "did not hold 1 PIDs.*last content: ''"):
            wait_for_pids(self.pidfile, 1, timeout=0.2)
        self.assertLess(time.monotonic() - started, 5.0)

    def test_the_wrong_number_of_pids_keeps_waiting_and_then_fails(self) -> None:
        self.pidfile.write_text("123")
        with self.assertRaisesRegex(AssertionError, "did not hold 2 PIDs.*last content: '123'"):
            wait_for_pids(self.pidfile, 2, timeout=0.2)

    def test_non_ascii_digits_keep_waiting_for_valid_content(self) -> None:
        self.pidfile.write_text("\u00b2")
        timer = threading.Timer(0.1, lambda: self.pidfile.write_text("789"))
        timer.start()
        self.addCleanup(timer.cancel)
        self.assertEqual(wait_for_pids(self.pidfile, 1, timeout=10.0), [789])

    def test_non_numeric_content_is_not_ready(self) -> None:
        self.pidfile.write_text("abc def")
        with self.assertRaisesRegex(AssertionError, "last content: 'abc def'"):
            wait_for_pids(self.pidfile, 2, timeout=0.2)

    def test_the_source_a_generated_driver_embeds_waits_the_same_way(self) -> None:
        """test_08aa's driver runs the helper from its source with only these imports."""
        namespace: dict[str, object] = {}
        # The helper's own source, run as the generated driver runs it.
        exec(
            "import contextlib, time\nfrom pathlib import Path\n"
            + inspect.getsource(wait_for_pids),
            namespace,
        )
        embedded = namespace["wait_for_pids"]
        assert callable(embedded)
        self.pidfile.write_text("abc def")
        with self.assertRaisesRegex(AssertionError, "last content: 'abc def'"):
            embedded(self.pidfile, 2, timeout=0.2)
        self.pidfile.write_text("123")
        with self.assertRaisesRegex(AssertionError, "last content: '123'"):
            embedded(self.pidfile, 2, timeout=0.2)
        self.pidfile.write_text("0 456")
        self.assertEqual(embedded(self.pidfile, 2, timeout=1.0), [0, 456])

    def test_a_missing_file_fails_with_an_assertion(self) -> None:
        with self.assertRaisesRegex(AssertionError, "last content: None"):
            wait_for_pids(self.pidfile, 1, timeout=0.2)


class Slots(CheckFixture):
    def test_07a_slots_are_private_nonblocking_and_reused(self) -> None:
        base = self.copy / "target/ck-full"
        first = check.acquire_slot(base)
        second = check.acquire_slot(base)
        self.assertEqual((first[0], second[0]), (0, 1))
        os.close(first[1])
        third = check.acquire_slot(base)
        self.assertEqual(third[0], 0)  # the lowest free slot is reused
        for _, descriptor in (second, third):
            os.close(descriptor)

    def test_07b_interruption_reaps_the_direct_child_before_the_slot_is_released(self) -> None:
        """SIGINT and SIGTERM during a step, with and without a grandchild. The leader takes a second to
        die after SIGTERM, so cleanup is observably in progress: while the still-running checker has
        not reaped its direct child the slot stays held, and the moment the slot is free the child and
        grandchild are gone from ps (not even as zombies). The interrupt propagates unchanged."""
        for sent in (signal.SIGINT, signal.SIGTERM):
            for with_grandchild in (False, True):
                name = f"{sent.name}-{'gc' if with_grandchild else 'leader'}"
                with self.subTest(name):
                    pidfile = self.base / f"{name}.pids"
                    body = (
                        "import os, signal, subprocess, time\n"
                        "def stop(*_):\n    time.sleep(1.0)\n    os._exit(0)\n"
                        "signal.signal(signal.SIGTERM, stop)\n"
                        f"grandchild = subprocess.Popen(['sleep', '300']).pid if {with_grandchild} else 0\n"
                        f"open({str(pidfile)!r}, 'w').write(f'{{os.getpid()}} {{grandchild}}')\n"
                        "time.sleep(300)\n"
                    )
                    code = self.driver_source_for(
                        self.step_source(body, name=name, levels=("full",))
                    )
                    process = subprocess.Popen(
                        [sys.executable, "-c", code],
                        stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE,
                        text=True,
                        env=clean_environment(),
                    )
                    leader, grandchild = wait_for_pids(pidfile, 2)
                    time.sleep(0.2)
                    slot = self.copy / "target/ck-full/0.lock"
                    self.assertFalse(slot_free(slot))
                    started = time.monotonic()
                    process.send_signal(sent)
                    held_while_running = released_after_reap = 0
                    while process.poll() is None and time.monotonic() - started < 30:  # noqa: PLR2004  # polling bound in seconds
                        states = {pid: process_state(pid) for pid in (leader, grandchild) if pid}
                        if slot_free(slot):
                            # Released: the owned child must be gone now. Read the states again
                            # after observing the release; a snapshot taken before it may predate
                            # the reap (the slot can be released between the two observations).
                            after = {pid: process_state(pid) for pid in (leader, grandchild) if pid}
                            self.assertEqual(after.get(leader, ""), "", f"{name}: {after}")
                            self.assertEqual(after.get(grandchild, ""), "", f"{name}: {after}")
                            released_after_reap += 1
                            break
                        if any(state for state in states.values()):
                            held_while_running += 1
                        time.sleep(0.005)
                    _, errors = process.communicate(timeout=60)
                    elapsed = time.monotonic() - started
                    self.assertTrue(released_after_reap or slot_free(slot))
                    expected = {signal.SIGINT: (-signal.SIGINT, 130), signal.SIGTERM: (143,)}[sent]
                    self.assertIn(process.returncode, expected, errors)
                    self.assertNotIn("PermissionError", errors)
                    self.assertNotIn("denied", errors)
                    self.assertLess(elapsed, check.GRACE + 2.0, f"cleanup took {elapsed:.1f}s")
                    for pid in (leader, grandchild):
                        if pid:
                            self.assertEqual(process_state(pid), "")

    def test_07d_the_leaders_returncode_is_set_before_the_slot_descriptor_is_closed(self) -> None:
        """In-process ordering: a driver records the step leader's returncode at the instant check.py
        closes the slot descriptor. It must already be set (the leader was reaped), and the slot is
        held while the step is still running."""
        for sent in (signal.SIGINT, signal.SIGTERM):
            with self.subTest(sent.name):
                started = self.base / f"order-{sent.name}.started"
                body = (
                    "import os, time\n"
                    f"open({str(started)!r}, 'w').write(str(os.getpid()))\n"
                    "time.sleep(300)\n"
                )
                steps = self.step_source(body, name="order", levels=("full",))
                driver = textwrap.dedent(
                    f"""
                    import os, signal, subprocess, sys
                    from pathlib import Path
                    sys.path.insert(0, {str(TOOLS)!r})
                    import check
                    from check import Step
                    spawned, slot_fd = [], []
                    class Tracking(subprocess.Popen):
                        def __init__(self, *a, **k):
                            super().__init__(*a, **k)
                            spawned.append(self)
                    real_acquire, real_close = check.acquire_slot, os.close
                    def acquire(base):
                        slot = real_acquire(base)
                        slot_fd.append(slot[1])
                        return slot
                    def close(fd):
                        if slot_fd and fd == slot_fd[0]:
                            print("ORDER slot-close leader_returncode=%r" % spawned[0].returncode,
                                  file=sys.stderr, flush=True)
                        real_close(fd)
                    check.subprocess.Popen = Tracking
                    check.acquire_slot = acquire
                    check.os.close = close
                    signal.signal(signal.SIGTERM, check.terminate)
                    sys.exit(check.execute(Path({str(self.copy)!r}), Path({str(self.hub)!r}),
                                           'full', steps={steps}))
                    """
                )
                process = subprocess.Popen(
                    [sys.executable, "-c", driver],
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                    env=clean_environment(),
                )
                deadline = time.monotonic() + 30
                while not started.exists() and time.monotonic() < deadline:
                    time.sleep(0.05)
                time.sleep(0.2)
                self.assertFalse(slot_free(self.copy / "target/ck-full/0.lock"))  # step running
                process.send_signal(sent)
                _, errors = process.communicate(timeout=60)
                orders = [line for line in errors.splitlines() if line.startswith("ORDER ")]
                self.assertEqual(len(orders), 1, errors)
                self.assertNotIn("leader_returncode=None", orders[0])
                self.assertNotIn("PermissionError", errors)

    def test_07c_end_group_reports_real_denials_and_not_zombie_only_groups(self) -> None:
        quick = mock.patch.multiple(check, GRACE=0.2, KILL_PATIENCE=0.2)
        # A live group that cannot be signalled: reported as denied, not reaped, not hidden.
        victim = subprocess.Popen(["sleep", "300"], process_group=0)

        def reap() -> None:
            victim.kill()
            victim.wait()

        self.addCleanup(reap)
        errors = io.StringIO()
        with (
            quick,
            mock.patch.object(os, "killpg", side_effect=PermissionError(1, "denied")),
            contextlib.redirect_stderr(errors),
        ):
            self.assertFalse(check.end_group(victim))
        self.assertIn("denied (EPERM)", errors.getvalue())
        self.assertIn("was not reaped", errors.getvalue())
        # EPERM for a group whose members are all zombies is not a denial.
        gone = mock.Mock(pid=999999)
        gone.poll.return_value = None
        gone.wait.return_value = 0
        errors = io.StringIO()
        with (
            quick,
            mock.patch.object(os, "killpg", side_effect=PermissionError(1, "zombies")),
            mock.patch.object(check, "group_members", return_value=[(999999, "Z")]),
            contextlib.redirect_stderr(errors),
        ):
            self.assertTrue(check.end_group(gone))
        self.assertEqual(errors.getvalue(), "")
        # A group that is already gone (ESRCH) with a reaped child is clean.
        done = subprocess.Popen(
            [sys.executable, "-c", "pass"], process_group=0, env=clean_environment()
        )
        done.wait()
        with quick, contextlib.redirect_stderr(io.StringIO()):
            self.assertTrue(check.end_group(done))

    def driver_source_for(self, steps: str) -> str:
        return textwrap.dedent(
            f"""
            import signal, sys
            from pathlib import Path
            sys.path.insert(0, {str(TOOLS)!r})
            import check
            from check import Step
            signal.signal(signal.SIGTERM, check.terminate)
            sys.exit(check.execute(Path({str(self.copy)!r}), Path({str(self.hub)!r}),
                                   'full', steps={steps}))
            """
        )


def process_table() -> dict[int, tuple[int, str]]:
    raw = subprocess.check_output(["ps", "-axo", "pid=,ppid=,command="], text=True)
    rows: dict[int, tuple[int, str]] = {}
    for line in raw.splitlines():
        parts = line.strip().split(None, 2)
        if len(parts) == 3:  # noqa: PLR2004  # a ps row has three fields
            rows[int(parts[0])] = (int(parts[1]), parts[2])
    return rows


def descendants(root_pid: int) -> dict[int, str]:
    rows = process_table()
    found: dict[int, str] = {}
    queue = [root_pid]
    while queue:
        parent = queue.pop()
        for pid, (ppid, command) in rows.items():
            if ppid == parent and pid not in found:
                found[pid] = command
                queue.append(pid)
    return found


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


# The real-toolchain copy: the Cargo workspace, the Python the recipe lints and runs, and the hub marker.
# The copy runs the real recipe up to the first Cargo test step (fmt, ruff, shellcheck, doctor, clippy,
# doc, test), so every root file that configures one of those steps must be here, or the copy would
# run them differently from the real tree (CK-48):
# - `clippy.toml` holds `allow-unwrap-in-tests`; without it Clippy rejects `unwrap` in test code that
#   passes in the real tree;
# - `.cargo/config.toml` forces the one MACOSX_DEPLOYMENT_TARGET that the native code, the shaders
#   and the link of the clippy, doc and test builds use;
# - `stubs` is on the strict-mypy path (`tools:stubs`), and the bare `ruff check` and
#   `ruff format --check` lint its `.pyi` files. Strict mypy itself runs after the test step, where
#   this test stops the recipe, so it is not reached.
# `docs/` and the top-level `*.md` files stay out (CK-33): ordinary documentation is not a test input.
TOOLCHAIN_INPUTS = (
    ".cargo",
    ".ck-hub",
    ".ignore",
    "Cargo.lock",
    "Cargo.toml",
    "apps",
    "clippy.toml",
    "crates",
    "pyproject.toml",
    "rust-toolchain.toml",
    "specs",
    "stubs",
    "tests",
    "tools",
)


def write_hub_marker(source: Path, copy: Path) -> None:
    """Give `copy` a `.ck-hub` marker naming the hub `source` resolves to (CK-37): a worker copy's
    marker is passed through, the root (the hub, with no marker) names itself."""
    (copy / ".ck-hub").write_text(f"{environment.hub_for(source)}\n")


SCRATCH = shutil.ignore_patterns("target", ".pijul", ".ruff_cache", "__pycache__", ".git")


def terminate_pids(pids: Sequence[int]) -> None:
    """Independent of the code under test: end these processes (bounded) and fail if any survives."""
    for sig in (signal.SIGTERM, signal.SIGKILL):
        for pid in pids:
            if process_state(pid):
                with contextlib.suppress(ProcessLookupError):
                    os.kill(pid, sig)
        end = time.monotonic() + 5.0
        while time.monotonic() < end and any(process_state(pid) for pid in pids):
            time.sleep(0.05)
    survivors = [pid for pid in pids if process_state(pid)]
    if survivors:
        raise AssertionError(f"fixture processes survived SIGKILL: {survivors}")


class Watchdog(CheckFixture):
    """S4: every part of each step of a fast or full run is under a watchdog; firing exits 75."""

    def hanging_step(
        self, name: str, pidfile: Path, levels: Sequence[str] = ("fast", "full")
    ) -> str:
        body = (
            "import os, subprocess, time\n"
            "child = subprocess.Popen(['sleep', '300'])\n"
            f"open({str(pidfile)!r}, 'w').write(f'{{os.getpid()}} {{child.pid}}')\n"
            "time.sleep(300)\n"
        )
        return self.step_source(body, name=name, levels=levels)

    def fixture_pids(self, pidfile: Path) -> None:
        """Register an independent cleanup for the step's processes, whatever the code under test does."""
        self.addCleanup(
            lambda: terminate_pids(list(map(int, pidfile.read_text().split())))
            if pidfile.exists()
            else None
        )

    def test_08_a_hung_step_is_killed_with_its_group_and_check_exits_75(self) -> None:
        for level in ("fast", "full"):
            with self.subTest(level):
                pidfile = self.base / f"{level}.pids"
                self.fixture_pids(pidfile)
                fast = (
                    "[Step('quick-ok', ('fast', 'full'), lambda: [[sys.executable, '-c', 'pass']])]"
                )
                hang = self.hanging_step("hang", pidfile)
                started = time.monotonic()
                result = self.driver(
                    fast[:-1] + ", " + hang[1:], level=level, watchdog=1.0, timeout=30
                )
                elapsed = time.monotonic() - started
                self.assertEqual(result.returncode, 75, result.stdout + result.stderr)
                self.assertLess(elapsed, 20.0)
                self.assertIn("step hang exceeded the 1 s watchdog after 1.", result.stderr)
                self.assertIn("killed and reaped. Exiting 75", result.stderr)
                self.assertIn("quick-ok ok, hang TIMEOUT", result.stdout)
                self.assertIn("check: step quick-ok took", result.stdout)
                survivors = [
                    pid for pid in map(int, pidfile.read_text().split()) if process_state(pid)
                ]
                self.assertEqual(survivors, [], "the watchdog left step processes running")
                if level == "full":  # the build slot is released after the group is reaped
                    self.assertTrue(slot_free(self.copy / "target/ck-full/0.lock"))

    def test_08b_a_hung_doctor_tool_is_under_the_watchdog(self) -> None:
        """The doctor's version checks run inside one step, so a hanging tool is bounded too."""
        pidfile = self.base / "doctor.pids"
        self.fixture_pids(pidfile)
        tools = self.base / "bin"
        tools.mkdir()
        fake = tools / "rustc"
        fake.write_text(f'#!/bin/bash\nsleep 300 &\necho "$$ $!" > {pidfile}\nwait\n')
        fake.chmod(0o755)
        started = time.monotonic()
        result = self.driver(
            "[Step('doctor', ('fast', 'full'), action=check.doctor)]",
            level="fast",
            watchdog=1.0,
            timeout=30,
            env={
                **clean_environment(),
                "PATH": f"{tools}:{os.environ['PATH']}",
                "CK_HUB": str(self.hub),
            },
        )
        self.assertEqual(result.returncode, 75, result.stdout + result.stderr)
        self.assertLess(time.monotonic() - started, 20.0)
        self.assertIn("step doctor exceeded the 1 s watchdog", result.stderr)
        survivors = [pid for pid in map(int, pidfile.read_text().split()) if process_state(pid)]
        self.assertEqual(survivors, [], "the hung doctor tool's group was left running")

    def test_08c_an_in_process_action_is_under_the_watchdog_too(self) -> None:
        result = self.driver(
            "[Step('action', ('fast',), action=lambda: __import__('time').sleep(30))]",
            level="fast",
            watchdog=0.5,
            timeout=30,
        )
        self.assertEqual(result.returncode, 75, result.stdout + result.stderr)
        self.assertIn("step action exceeded the 0 s watchdog", result.stderr)
        self.assertIn("the step was interrupted", result.stderr)

    def test_08d_a_group_that_cannot_be_ended_is_reported_not_claimed_killed(self) -> None:
        pidfile = self.base / "stubborn.pids"
        self.fixture_pids(pidfile)
        result = self.driver(
            self.hanging_step("hang", pidfile),
            level="fast",
            watchdog=1.0,
            timeout=30,
            prelude="real = check.end_group; check.end_group = lambda process: (real(process), False)[1]",
        )
        self.assertEqual(result.returncode, 75, result.stdout + result.stderr)
        self.assertIn("could NOT be fully killed or reaped", result.stderr)
        self.assertNotIn("killed and reaped. Exiting", result.stderr)

    def test_08e_a_deadline_during_the_spawn_still_reaps_the_new_child(self) -> None:
        """The alarm lands after the child exists but before run() has recorded it: the expiry is
        deferred, the child is recorded, ended and reaped, and nothing is left running."""
        prelude = (
            "import os\n"
            "real = check.subprocess.Popen\n"
            "fired = []\n"
            "class Late(real):\n"
            "    def __init__(self, *args, **kwargs):\n"
            "        super().__init__(*args, **kwargs)\n"
            "        if not fired and kwargs.get('process_group') is not None:\n"
            "            fired.append(self.pid)\n"
            "            print('SPAWNED', self.pid, file=sys.stderr, flush=True)\n"
            "            os.kill(os.getpid(), signal.SIGALRM)\n"
            "check.subprocess.Popen = Late"
        )
        step = self.step_source("import time; time.sleep(30)", name="spawn", levels=("fast",))
        result = self.driver(step, level="fast", watchdog=30.0, prelude=prelude, timeout=30)
        spawned = [
            int(line.split()[1])
            for line in result.stderr.splitlines()
            if line.startswith("SPAWNED")
        ]
        self.assertEqual(len(spawned), 1, result.stdout + result.stderr)
        self.addCleanup(terminate_pids, spawned)
        self.assertEqual(result.returncode, 75, result.stdout + result.stderr)
        self.assertIn("killed and reaped. Exiting 75", result.stderr)
        self.assertEqual(
            process_state(spawned[0]), "", "the child spawned before the alarm survived"
        )

    def test_08f_a_deadline_during_cleanup_stops_the_step_before_the_next_command(self) -> None:
        """The alarm lands while a group is being ended (watchdog held off): the expiry is kept and
        the step's next command never starts, instead of running with the one-shot timer spent."""
        marker = self.base / "second-ran"
        prelude = (
            "import os\n"
            "real = check.end_group\n"
            "calls = []\n"
            "def noisy(process):\n"
            "    if not calls:\n"
            "        calls.append(process.pid)\n"
            "        os.kill(os.getpid(), signal.SIGALRM)\n"
            "    return real(process)\n"
            "check.end_group = noisy\n"
            "popen = check.subprocess.Popen\n"
            "class Counting(popen):\n"
            "    def __init__(self, *args, **kwargs):\n"
            "        if kwargs.get('process_group') is not None:\n"
            "            print('SPAWN', file=sys.stderr, flush=True)\n"
            "        super().__init__(*args, **kwargs)\n"
            "check.subprocess.Popen = Counting"
        )
        second = f"from pathlib import Path; Path({str(marker)!r}).write_text('ran'); import time; time.sleep(30)"
        steps = (
            "[Step('two-commands', ('fast',), lambda: "
            f"[[sys.executable, '-c', 'pass'], [sys.executable, '-c', {second!r}]])]"
        )
        result = self.driver(steps, level="fast", watchdog=30.0, prelude=prelude, timeout=30)
        self.assertEqual(result.returncode, 75, result.stdout + result.stderr)
        self.assertFalse(
            marker.exists(), "the second command started after the deadline had passed"
        )
        spawns = [line for line in result.stderr.splitlines() if line == "SPAWN"]
        self.assertEqual(len(spawns), 1, "a command was spawned after the deadline had passed")
        self.assertIn("step two-commands exceeded", result.stderr)
        self.assertIn("two-commands TIMEOUT", result.stdout)

    def expire_during_first_cleanup(self) -> str:
        return (
            "import os\n"
            "real = check.end_group\n"
            "calls = []\n"
            "def noisy(process):\n"
            "    if not calls:\n"
            "        calls.append(process.pid)\n"
            "        os.kill(os.getpid(), signal.SIGALRM)\n"
            "    return real(process)\n"
            "check.end_group = noisy"
        )

    def test_08g_an_expiry_during_cleanup_beats_a_failing_exit_status(self) -> None:
        """A watchdog expiry is never classified as a patch failure: exit 75, not 1."""
        failing = self.step_source("raise SystemExit(3)", name="failing", levels=("fast",))
        result = self.driver(
            failing,
            level="fast",
            watchdog=30.0,
            prelude=self.expire_during_first_cleanup(),
            timeout=30,
        )
        self.assertEqual(result.returncode, 75, result.stdout + result.stderr)
        self.assertIn("failing TIMEOUT", result.stdout)
        self.assertNotIn("returned non-zero exit status", result.stderr)

    def test_08h_an_action_does_not_continue_after_an_expired_cleanup(self) -> None:
        marker = self.base / "action-continued"
        action = f"lambda: (check.run([sys.executable, '-c', 'pass']), Path({str(marker)!r}).write_text('x'))"
        result = self.driver(
            f"[Step('action', ('fast',), action={action})]",
            level="fast",
            watchdog=30.0,
            prelude=self.expire_during_first_cleanup(),
            timeout=30,
        )
        self.assertEqual(result.returncode, 75, result.stdout + result.stderr)
        self.assertFalse(marker.exists(), "the action kept running after the expired cleanup")

    def test_08i_an_expiry_beats_an_unconfirmed_cleanup_and_the_report_stays_truthful(self) -> None:
        """The alarm lands during cleanup that then reports failure (a real child is reaped, the
        result is forced to False): the expiry still wins over the RuntimeError and the exit
        status, and the message says the group could not be confirmed ended."""
        prelude = (
            "import os\n"
            "real = check.end_group\n"
            "def unconfirmed(process):\n"
            "    os.kill(os.getpid(), signal.SIGALRM)\n"
            "    real(process)\n"
            "    return False\n"
            "check.end_group = unconfirmed"
        )
        for code in (0, 3):
            with self.subTest(exit=code):
                step = self.step_source(f"raise SystemExit({code})", name="s", levels=("fast",))
                result = self.driver(step, level="fast", watchdog=30.0, prelude=prelude, timeout=30)
                self.assertEqual(result.returncode, 75, result.stdout + result.stderr)
                self.assertIn("s TIMEOUT", result.stdout)
                self.assertIn("could NOT be fully killed or reaped", result.stderr)
                self.assertNotIn("could not end the step's process group", result.stderr)

    def test_08j_a_deadline_just_before_the_normal_cleanup_still_ends_the_group(self) -> None:
        """The alarm lands after the step's process exited cleanly but before the normal-path
        cleanup begins (the unprotected boundary): the leftover descendant is still ended, and in a
        full run the build slot is released only after it is gone."""
        prelude = (
            "import os\n"
            "real = check.end_group\n"
            "fired = []\n"
            "def early(process):\n"
            "    if not fired:\n"
            "        fired.append(1)\n"
            "        os.kill(os.getpid(), signal.SIGALRM)\n"
            "    return real(process)\n"
            "check.end_group = early"
        )
        for level in ("fast", "full"):
            with self.subTest(level):
                pidfile = self.base / f"boundary-{level}.pids"
                self.fixture_pids(pidfile)
                body = (
                    "import subprocess, time\n"
                    "child = subprocess.Popen(['sleep', '300'])\n"
                    f"open({str(pidfile)!r}, 'w').write(f'{{child.pid}}')\n"
                )
                step = self.step_source(body, name="boundary", levels=("fast", "full"))
                code = self.build_driver_source(step, level, watchdog=30.0, prelude=prelude)
                process = subprocess.Popen(
                    [sys.executable, "-c", code],
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                    env=clean_environment(),
                )
                slot = self.copy / "target/ck-full/0.lock"
                deadline = time.monotonic() + 30
                (child,) = wait_for_pids(pidfile, 1)
                released_with_child_alive = False
                while process.poll() is None and time.monotonic() < deadline:
                    if level == "full" and slot_free(slot):
                        # Released: the leftover descendant must already be gone (re-read after).
                        released_with_child_alive = bool(process_state(child))
                        break
                    time.sleep(0.005)
                _, errors = process.communicate(timeout=30)
                self.assertEqual(process.returncode, 75, errors)
                self.assertIn("killed and reaped. Exiting 75", errors)
                self.assertEqual(process_state(child), "", "the leftover descendant survived")
                self.assertFalse(released_with_child_alive, "the slot was released before the reap")

    def interrupted_driver(
        self,
        name: str,
        prelude: str,
        signals: Sequence[int],
        hold: bool = False,
        level: str = "fast",
    ) -> tuple[int | None, str, str, list[int]]:
        """A driver whose step hangs; `signals` are sent in order once the step runs. With `hold`,
        the cleanup waits (on a pipe, not a sleep) until every signal's handler has run."""
        pidfile = self.base / f"{name}.pids"
        self.fixture_pids(pidfile)
        code = self.build_driver_source(
            self.hanging_step("hang", pidfile), level, watchdog=30.0, prelude=prelude
        )
        session = Session(self, code, stdin=True)
        wait_for_pids(pidfile, 2)
        time.sleep(0.2)  # the step's process is running; it only has to exist
        if hold:
            session.send(signals[0])
            session.until("CLEANUP-START")  # printed once the cleanup starts
            for number in signals[1:]:
                session.send(number)
            wanted = sum(1 for number in signals[1:] if number != signal.SIGALRM)
            for _ in range(wanted):  # one HANDLED line per SIGTERM or SIGINT sent after the first
                session.until("HANDLED")
            session.release()
        else:
            for number in signals:
                session.send(number)
        output, errors = session.finish()
        survivors = [pid for pid in map(int, pidfile.read_text().split()) if process_state(pid)]
        return session.returncode, output, errors, survivors

    # The cleanup-holding and handler-reporting wrappers shared by `test_08k` to `test_08u` (they need `check`
    # imported and `RELEASE` defined): the hold in `end_group`, a line per handled SIGTERM or SIGINT.
    HOLD_WRAPPERS = (
        "import os\n"
        "real_end = check.end_group\n"
        "def held(process):\n"
        "    emit('CLEANUP-START')\n"
        "    wait_released()\n"
        "    return real_end(process)\n"
        "check.end_group = held\n"
        "real_term, real_int = check.terminate, check.interrupt\n"
        "def term(signum, frame):\n"
        "    real_term(signum, frame)\n"
        "    emit('HANDLED')\n"
        "def intr(signum, frame):\n"
        "    real_int(signum, frame)\n"
        "    emit('HANDLED')\n"
        "check.terminate, check.interrupt = term, intr\n"
    )
    HOLD = RELEASE + HOLD_WRAPPERS

    def test_08k_sigterm_then_the_alarm_exits_143_and_nothing_survives(self) -> None:
        """The first cause wins: a real SIGTERM, then the watchdog alarm delivered from inside the
        SIGTERM handler's own wake-up. Neither handler raises while the child is owned."""
        prelude = (
            "import os\n"
            "real_term = check.terminate\n"
            "def term(signum, frame):\n"
            "    real_term(signum, frame)\n"
            "    os.kill(os.getpid(), signal.SIGALRM)\n"
            "check.terminate = term"
        )
        code, _, errors, survivors = self.interrupted_driver(
            "term-alarm", prelude, [signal.SIGTERM]
        )
        self.assertEqual(code, 143, errors)
        self.assertNotIn("watchdog", errors)
        self.assertEqual(survivors, [])

    def test_08l_the_alarm_then_sigterm_exits_75_and_nothing_survives(self) -> None:
        code, output, errors, survivors = self.interrupted_driver(
            "alarm-term", self.HOLD, [signal.SIGALRM, signal.SIGTERM], hold=True
        )
        self.assertEqual(code, 75, output + errors)
        self.assertIn("step hang exceeded", errors)
        self.assertEqual(survivors, [])

    def test_08m_two_sigterms_exit_143_and_nothing_survives(self) -> None:
        code, output, errors, survivors = self.interrupted_driver(
            "term-term", self.HOLD, [signal.SIGTERM, signal.SIGTERM], hold=True
        )
        self.assertEqual(code, 143, output + errors)
        self.assertEqual(survivors, [])

    def test_08n_repeated_ctrl_c_is_the_interrupt_and_nothing_survives(self) -> None:
        code, output, errors, survivors = self.interrupted_driver(
            "int-int", self.HOLD, [signal.SIGINT, signal.SIGINT], hold=True
        )
        self.assertIn(code, (-signal.SIGINT, 130), output + errors)
        self.assertEqual(survivors, [])

    # CK-89. A driver wrapper that prints from a signal handler races with the main thread's own print on the
    # same buffered stdout: a signal that lands inside that print makes the handler's print raise "reentrant
    # call inside <_io.BufferedWriter>", and the error propagates into the interrupted code. The accepted
    # `test_08n` hit it once in a heavily loaded run (the second Ctrl-C arrived the instant the first
    # CLEANUP-START line was read): the exception left the test's `end_group` wrapper before it reached the
    # real `end_group`, so `run()`'s exception path called the wrapper a second time. `check.py`'s handlers
    # only record; the defect was in the wrapper. The tests below land that signal where it hurts with no
    # sleep, around **the wrappers the suite actually uses** (`HOLD_WRAPPERS`, `NESTED` with the bodies
    # `test_08p` pauses in, and `test_08u`'s standalone doctor), so putting a `print` back in any of them fails.
    #
    # `REENTRANT_STDOUT` replaces the driver's stdout with a buffered writer whose raw layer sends the driver the
    # signal numbered in `armed` from inside the first buffered write after it is set.
    REENTRANT_STDOUT = (
        "import io, signal\n"
        "armed = []\n"
        "class Raw(io.RawIOBase):\n"
        "    def writable(self):\n"
        "        return True\n"
        "    def write(self, data):\n"
        "        if armed:\n"
        "            os.kill(os.getpid(), armed.pop())  # lands inside this buffered write\n"
        "        return os.write(1, bytes(data))\n"
        "sys.stdout = io.TextIOWrapper(io.BufferedWriter(Raw()), line_buffering=True)\n"
    )

    @staticmethod
    def reentrant_trigger(signal_name: str) -> str:
        """Layered over whatever `end_group` wrappers are installed: on its first entry it arms the signal and
        prints, so the signal lands inside this driver print, in the main thread, while the installed signal
        handlers (the wrappers under test) run. `TRIGGERED` counts the entries: a second one means the first
        was torn out of its print by an exception from the handler."""
        return (
            "fired = []\n"
            "inner = check.end_group\n"
            "def trigger(process):\n"
            "    emit('TRIGGERED')\n"
            "    if not fired:\n"
            "        fired.append(1)\n"
            f"        armed.append(signal.{signal_name})\n"
            "    print('IN-PRINT', flush=True)\n"
            "    return inner(process)\n"
            "check.end_group = trigger\n"
        )

    def drive(
        self, session: Session, pidfile: Path, script: Sequence[tuple[str, object]]
    ) -> list[str]:
        """Plays `script` ((`send`, signal), (`until`, marker) or (`release`, None)) against a session whose
        step has started, then returns every line the driver wrote."""
        wait_for_pids(pidfile, 2)
        time.sleep(0.2)
        for action, argument in script:
            if action == "send":
                assert isinstance(argument, int)
                session.send(argument)
            elif action == "until":
                assert isinstance(argument, str)
                session.until(argument)
            else:
                session.release()
        output, _errors = session.finish()
        return session.seen + output.splitlines()

    def survivors_of(self, pidfile: Path) -> list[int]:
        return [pid for pid in map(int, pidfile.read_text().split()) if process_state(pid)]

    def reentrant_driver(self, name: str, prelude: str) -> tuple[Session, Path]:
        pidfile = self.base / f"{name}.pids"
        self.fixture_pids(pidfile)
        code = self.build_driver_source(
            self.hanging_step("hang", pidfile), "fast", watchdog=30.0, prelude=prelude
        )
        return Session(self, code, stdin=True), pidfile

    # The wrappers as `test_08n` first had them: the handler prints. Kept only as the bounded negative control
    # of the race above, never used by a test of the suite.
    OLD_PRINTING_WRAPPERS = (
        "import os\n"
        "real_end = check.end_group\n"
        "def held(process):\n"
        "    print('CLEANUP-START', flush=True)\n"
        "    wait_released()\n"
        "    return real_end(process)\n"
        "check.end_group = held\n"
        "real_int = check.interrupt\n"
        "def intr(signum, frame):\n"
        "    real_int(signum, frame)\n"
        "    print('HANDLED', flush=True)\n"
        "check.interrupt = intr\n"
    )

    def test_08n2_the_real_hold_wrappers_survive_a_second_signal_inside_the_drivers_own_print(
        self,
    ) -> None:
        """`HOLD`, as `test_08n` and `test_08o` use it: a signal lands inside a buffered driver print (SIGINT or
        SIGTERM, the two handler wrappers it has), the handler wrapper still writes its line, the trigger is
        entered once, and the first cause decides the exit."""
        for first, hit, codes in (
            (signal.SIGINT, signal.SIGINT, (-signal.SIGINT, 130)),
            (signal.SIGINT, signal.SIGTERM, (-signal.SIGINT, 130)),
            (signal.SIGTERM, signal.SIGTERM, (143,)),
            (signal.SIGTERM, signal.SIGINT, (143,)),
        ):
            with self.subTest(first=first.name, hit=hit.name):
                prelude = self.HOLD + self.REENTRANT_STDOUT + self.reentrant_trigger(hit.name)
                session, pidfile = self.reentrant_driver(f"reentrant-hold-{first}-{hit}", prelude)
                lines = self.drive(
                    session,
                    pidfile,
                    [("send", first), ("until", "CLEANUP-START"), ("release", None)],
                )
                self.assertEqual(lines.count("TRIGGERED"), 1, lines)
                self.assertEqual(
                    lines.count("HANDLED"), 2, lines
                )  # the test's signal and the one inside the print
                self.assertIn(session.returncode, codes, lines)
                self.assertEqual(self.survivors_of(pidfile), [])

    def test_08n3_negative_control_the_accepted_printing_wrappers_lose_the_race_deterministically(
        self,
    ) -> None:
        """The same trigger over the OLD wrappers (print from the handler): the handler's print raises inside
        the interrupted print, the trigger is torn out before it reaches the cleanup wrapper, and run()'s
        exception path enters it a second time (`TRIGGERED` twice, the second Ctrl-C's `HANDLED` missing),
        the flake's signature. check.py itself ran its handler to completion and still exits with the
        interrupt's status. Proves the trigger really reproduces the race, so the tests above bind to it."""
        prelude = (
            RELEASE
            + "import sys\n"
            + self.OLD_PRINTING_WRAPPERS
            + self.REENTRANT_STDOUT
            + self.reentrant_trigger("SIGINT")
        )
        session, pidfile = self.reentrant_driver("reentrant-old", prelude)
        wait_for_pids(pidfile, 2)
        time.sleep(0.2)
        session.send(signal.SIGINT)
        session.until("CLEANUP-START")
        session.release()
        output, errors = session.finish()
        lines = session.seen + output.splitlines()
        self.assertEqual(lines.count("TRIGGERED"), 2, lines)
        self.assertEqual(lines.count("HANDLED"), 1, lines)
        self.assertIn(session.returncode, (-signal.SIGINT, 130), errors)
        self.assertEqual(self.survivors_of(pidfile), [])

    def test_08n4_the_real_nested_wrappers_survive_a_signal_inside_the_drivers_own_print(
        self,
    ) -> None:
        """`NESTED`, as `test_08p` uses it: a recorded cause is reported by the record wrapper, which runs in
        the handler. SIGTERM is the first cause (from the test), SIGINT the second (inside the print): both
        causes are reported, the trigger is entered once, and the first cause decides the exit."""
        prelude = (
            (self.NESTED % "real_term(signum, frame)")
            + self.REENTRANT_STDOUT
            + self.reentrant_trigger("SIGINT")
        )
        session, pidfile = self.reentrant_driver("reentrant-nested", prelude)
        lines = self.drive(
            session,
            pidfile,
            [("send", signal.SIGTERM), ("until", "CLEANUP-START"), ("release", None)],
        )
        self.assertEqual(lines.count("TRIGGERED"), 1, lines)
        self.assertEqual(
            [line for line in lines if line.startswith("CAUSE")],
            ["CAUSE terminated", "CAUSE interrupted"],
        )
        self.assertEqual(session.returncode, 143, lines)
        self.assertEqual(self.survivors_of(pidfile), [])

    def test_08n5_the_real_paused_handler_bodies_survive_a_signal_inside_the_drivers_own_print(
        self,
    ) -> None:
        """The two SIGTERM handler bodies `test_08p` pauses in: a second SIGTERM lands inside the driver's
        print, runs the paused body again (a `PAUSED` line each time) and the trigger is entered once."""
        for name, body in (("after", self.PAUSED_AFTER), ("before", self.PAUSED_BEFORE)):
            with self.subTest(name):
                prelude = (
                    (self.NESTED % body) + self.REENTRANT_STDOUT + self.reentrant_trigger("SIGTERM")
                )
                session, pidfile = self.reentrant_driver(f"reentrant-paused-{name}", prelude)
                lines = self.drive(
                    session,
                    pidfile,
                    [
                        ("send", signal.SIGTERM),
                        ("until", "PAUSED"),  # the test's SIGTERM, paused in the handler
                        ("release", None),
                        (
                            "until",
                            "PAUSED",
                        ),  # the one inside the print, paused in the handler again
                        ("release", None),
                        ("until", "CLEANUP-START"),
                        ("release", None),
                    ],
                )
                self.assertEqual(lines.count("TRIGGERED"), 1, lines)
                self.assertEqual(lines.count("PAUSED"), 2, lines)
                self.assertEqual(session.returncode, 143, lines)
                self.assertEqual(self.survivors_of(pidfile), [])

    def test_08n6_the_real_standalone_doctor_wrappers_survive_a_signal_inside_the_drivers_own_print(
        self,
    ) -> None:
        """`test_08u`'s wrappers (`HOLD_WRAPPERS`) around the standalone doctor, with no run state: SIGINT
        first, then either signal inside the print."""
        for hit in (signal.SIGINT, signal.SIGTERM):
            with self.subTest(hit=hit.name):
                wrappers = (
                    self.HOLD_WRAPPERS + self.REENTRANT_STDOUT + self.reentrant_trigger(hit.name)
                )
                session, pidfile = self.standalone_doctor_session(
                    f"reentrant-doctor-{hit}", wrappers
                )
                lines = self.drive(
                    session,
                    pidfile,
                    [("send", signal.SIGINT), ("until", "CLEANUP-START"), ("release", None)],
                )
                self.assertEqual(lines.count("TRIGGERED"), 1, lines)
                self.assertEqual(lines.count("HANDLED"), 2, lines)
                self.assertIn(session.returncode, (-signal.SIGINT, 130), lines)
                self.assertEqual(self.survivors_of(pidfile), [])

    def test_08o_the_first_cause_wins_when_ctrl_c_arrives_during_cleanup(self) -> None:
        for first, second, expected in (
            (signal.SIGTERM, signal.SIGINT, (143,)),
            (signal.SIGALRM, signal.SIGINT, (75,)),
            (signal.SIGINT, signal.SIGTERM, (-signal.SIGINT, 130)),
        ):
            with self.subTest(first=first.name, second=second.name):
                code, output, errors, survivors = self.interrupted_driver(
                    f"{first.name}-{second.name}", self.HOLD, [first, second], hold=True
                )
                self.assertIn(code, expected, output + errors)
                self.assertEqual(survivors, [])

    NESTED = RELEASE + (
        "import os\n"
        "real_record = check.record_cause\n"
        "def record(state, cause):\n"
        "    real_record(state, cause)\n"
        "    emit('CAUSE', cause)\n"
        "check.record_cause = record\n"
        "real_term = check.terminate\n"
        "def term(signum, frame):\n"
        "    %s\n"
        "check.terminate = term\n"
        "real_end = check.end_group\n"
        "def held(process):\n"
        "    emit('CLEANUP-START')\n"
        "    wait_released()\n"
        "    return real_end(process)\n"
        "check.end_group = held\n"
    )

    # The bodies of `NESTED`'s SIGTERM handler that `test_08p` pauses in (and the CK-89 regression drives).
    PAUSED_AFTER = "real_term(signum, frame); emit('PAUSED'); wait_released()"
    PAUSED_BEFORE = "emit('PAUSED'); wait_released(); real_term(signum, frame)"

    def nested_signals(self, name: str, body: str) -> tuple[int | None, list[str], str, list[int]]:
        """SIGTERM enters a paused handler; SIGALRM is delivered while it is paused. Returns the
        driver's exit code and the order in which causes were recorded."""
        pidfile = self.base / f"{name}.pids"
        self.fixture_pids(pidfile)
        code = self.build_driver_source(
            self.hanging_step("hang", pidfile), "fast", watchdog=30.0, prelude=self.NESTED % body
        )
        session = Session(self, code, stdin=True)
        wait_for_pids(pidfile, 2)
        time.sleep(0.2)

        def read_until(marker: str) -> None:
            session.until(marker)

        session.send(signal.SIGTERM)
        read_until("PAUSED")
        session.send(signal.SIGALRM)  # delivered while the first handler is paused
        read_until("CAUSE expired")
        session.release()  # release the paused SIGTERM handler
        read_until("CLEANUP-START")
        session.release()  # release the cleanup
        _, errors = session.finish()
        causes = [line.split()[1] for line in session.seen if line.startswith("CAUSE")]
        survivors = [pid for pid in map(int, pidfile.read_text().split()) if process_state(pid)]
        return session.returncode, causes, errors, survivors

    def test_08p_a_nested_handler_cannot_overwrite_the_first_recorded_cause(self) -> None:
        # Paused AFTER recording: SIGTERM is the first cause, the nested alarm the second.
        code, causes, errors, survivors = self.nested_signals("nested-after", self.PAUSED_AFTER)
        self.assertEqual(causes[:2], ["terminated", "expired"], errors)
        self.assertEqual(code, 143, errors)
        self.assertEqual(survivors, [])
        # Paused BEFORE recording: the nested alarm records first, so it is the first cause.
        code, causes, errors, survivors = self.nested_signals("nested-before", self.PAUSED_BEFORE)
        self.assertEqual(causes[:2], ["expired", "terminated"], errors)
        self.assertEqual(code, 75, errors)
        self.assertEqual(survivors, [])

    def test_08r_the_reviewers_handler_entry_replay_keeps_exit_143_and_leaves_nothing(self) -> None:
        """A real SIGTERM, then the alarm delivered at the very next line run() executes (a trace
        hook, no sleeps): in fix 1 that was the first statement of the exception handler, which
        replaced exit 143 with 75 and left the group alive."""
        prelude = (
            "import os\n"
            "state = {'after_term': False, 'fired': False}\n"
            "real_term = check.terminate\n"
            "def term(signum, frame):\n"
            "    state['after_term'] = True\n"
            "    real_term(signum, frame)\n"
            "check.terminate = term\n"
            "def local(frame, event, arg):\n"
            "    if event == 'line' and state['after_term'] and not state['fired']:\n"
            "        state['fired'] = True\n"
            "        os.kill(os.getpid(), signal.SIGALRM)\n"
            "    return local\n"
            "def tracer(frame, event, arg):\n"
            "    return local if frame.f_code is check.run.__code__ else None\n"
            "sys.settrace(tracer)"
        )
        code, _, errors, survivors = self.interrupted_driver("replay", prelude, [signal.SIGTERM])
        self.assertEqual(code, 143, errors)
        self.assertNotIn("watchdog", errors)
        self.assertEqual(survivors, [])

    def test_08s_a_second_signal_at_the_decision_point_cannot_change_the_first_cause(self) -> None:
        """The reviewer's B4: after the group is reaped, a second, different signal arrives exactly
        where the status is decided. Once a cause is recorded the handlers only append, so the first
        cause decides: SIGTERM first gives 143, the alarm first gives 75."""
        for first, second, expected in (
            (signal.SIGTERM, "SIGALRM", 143),
            (signal.SIGALRM, "SIGTERM", 75),
        ):
            with self.subTest(first=first.name, second=second):
                prelude = (
                    "import os\n"
                    "real_decide = check.decide\n"
                    "fired = []\n"
                    "def decide(state, cleaned):\n"
                    "    if not fired:\n"
                    "        fired.append(1)\n"
                    f"        os.kill(os.getpid(), signal.{second})\n"
                    "    return real_decide(state, cleaned)\n"
                    "check.decide = decide"
                )
                code, _, errors, survivors = self.interrupted_driver(
                    f"decide-{first.name}", prelude, [first]
                )
                self.assertEqual(code, expected, errors)
                self.assertEqual(survivors, [])

    def test_08v_a_late_ctrl_c_while_execute_unwinds_cannot_replace_the_first_cause(self) -> None:
        """The reviewer's last B4 window: after run() raised the first cause, execute() is unwinding.
        A real SIGINT delivered at the line after the build slot is closed (a trace hook) must only
        be recorded: the status stays that of the first cause, and the slot was closed first."""
        prelude = (
            "import inspect, os\n"
            "source, first = inspect.getsourcelines(check.execute)\n"
            "target = next(first + i for i, line in enumerate(source) if line.strip() == '_state = None')\n"
            "fired = []\n"
            "def local(frame, event, arg):\n"
            "    if event == 'line' and frame.f_lineno == target and not fired:\n"
            "        fired.append(1)\n"
            "        try:\n"
            "            os.fstat(frame.f_locals['slot'][1])\n"
            "            closed = False\n"
            "        except OSError:\n"
            "            closed = True\n"
            "        print('SLOT-CLOSED', closed, file=sys.stderr, flush=True)\n"
            "        os.kill(os.getpid(), signal.SIGINT)\n"
            "    return local\n"
            "def tracer(frame, event, arg):\n"
            "    return local if frame.f_code is check.execute.__code__ else None\n"
            "sys.settrace(tracer)"
        )
        for first, expected in ((signal.SIGTERM, 143), (signal.SIGALRM, 75)):
            with self.subTest(first=first.name):
                code, _, errors, survivors = self.interrupted_driver(
                    f"late-{first.name}", prelude, [first], level="full"
                )
                self.assertEqual(code, expected, errors)
                self.assertIn("SLOT-CLOSED True", errors)  # closed before the handlers are touched
                self.assertNotIn("KeyboardInterrupt", errors)
                self.assertEqual(survivors, [])

    def test_08w_a_cause_raised_with_no_child_owned_still_makes_later_signals_only_append(
        self,
    ) -> None:
        """An in-process step (no child) interrupted by SIGTERM: the handler raises the first cause
        (exit 143), and a Ctrl-C landing while that unwinds must not replace it."""
        prelude = (
            "import inspect, os, time\n"
            "source, first = inspect.getsourcelines(check.execute)\n"
            "target = next(first + i for i, line in enumerate(source) if line.strip() == '_state = None')\n"
            "fired = []\n"
            "def local(frame, event, arg):\n"
            "    if event == 'line' and frame.f_lineno == target and not fired:\n"
            "        fired.append(1)\n"
            "        os.kill(os.getpid(), signal.SIGINT)\n"
            "    return local\n"
            "def tracer(frame, event, arg):\n"
            "    return local if frame.f_code is check.execute.__code__ else None\n"
            "sys.settrace(tracer)\n"
            "def act():\n"
            "    print('ACTING', flush=True)\n"
            "    time.sleep(300)"
        )
        code = self.build_driver_source(
            "[Step('act', ('fast',), action=act)]", "fast", watchdog=30.0, prelude=prelude
        )
        session = Session(self, code)
        session.until("ACTING")
        session.send(signal.SIGTERM)
        _, errors = session.finish()
        self.assertEqual(session.returncode, 143, errors)
        self.assertNotIn("KeyboardInterrupt", errors)

    def test_08x_a_pending_cause_governs_exceptional_exits_too(self) -> None:
        """The reviewer's B5: a real signal lands after ownership starts but the spawn itself fails
        (a missing executable). The cause decides (75 or 143), nothing survives, the slot is
        released; with no cause the ordinary error propagates unchanged."""
        for level in ("fast", "full"):
            for signal_name, expected in (("SIGALRM", 75), ("SIGTERM", 143), (None, 1)):
                with self.subTest(level=level, signal=signal_name):
                    send = (
                        f"        os.kill(os.getpid(), signal.{signal_name})\n"
                        if signal_name
                        else "        pass\n"
                    )
                    prelude = (
                        "import os\n"
                        "real = check.subprocess.Popen\n"
                        "class Failing(real):\n"
                        "    def __init__(self, *args, **kwargs):\n"
                        + send
                        + "        super().__init__(*args, **kwargs)\n"
                        "check.subprocess.Popen = Failing"
                    )
                    steps = (
                        "[Step('missing', ('fast', 'full'), lambda: [['/nonexistent/tool-xyz']])]"
                    )
                    result = self.driver(
                        steps, level=level, watchdog=30.0, prelude=prelude, timeout=30
                    )
                    self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                    if signal_name is None:
                        self.assertIn("FileNotFoundError", result.stderr)
                    else:
                        self.assertNotIn("FileNotFoundError", result.stderr)
                    if level == "full":
                        self.assertTrue(slot_free(self.copy / "target/ck-full/0.lock"))

    def test_08y_the_same_single_decision_applies_in_execute(self) -> None:
        """An in-process step records a cause and then fails with an ordinary error: the cause
        decides the exit (75), replacing the error in flight; without a cause the error is kept."""
        for cause, expected in (("expired", 75), ("terminated", 143), (None, 1)):
            with self.subTest(cause=cause):
                record = f"check._own.causes.append({cause!r}); " if cause else ""
                prelude = f"def act():\n    {record}raise ValueError('ordinary error')"
                result = self.driver(
                    "[Step('act', ('fast',), action=act)]",
                    level="fast",
                    watchdog=30.0,
                    prelude=prelude,
                    timeout=30,
                )
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                if cause is None:
                    self.assertIn("ValueError", result.stderr)
                else:
                    self.assertNotIn("ValueError", result.stderr)

    def test_08z_a_cause_recorded_at_the_end_of_run_is_not_skipped_by_the_next_step(self) -> None:
        """The reviewer's B6: a real signal lands after run() found no cause but before it released
        ownership, so the handler only records it. The cause must still decide the exit, and the
        next step must not run."""
        for signal_name, expected in (
            ("SIGALRM", 75),
            ("SIGTERM", 143),
            ("SIGINT", -2),
        ):  # Ctrl-C ends the interpreter by SIGINT
            with self.subTest(signal=signal_name):
                prelude = (
                    "import inspect, os\n"
                    "source, first = inspect.getsourcelines(check.run)\n"
                    "target = next(first + i for i, line in enumerate(source)\n"
                    "              if line.strip().startswith('own.owned = False'))\n"
                    "fired = []\n"
                    "def local(frame, event, arg):\n"
                    "    if event == 'line' and frame.f_lineno == target and not fired:\n"
                    "        fired.append(1)\n"
                    f"        os.kill(os.getpid(), signal.{signal_name})\n"
                    "    return local\n"
                    "def tracer(frame, event, arg):\n"
                    "    return local if frame.f_code is check.run.__code__ else None\n"
                    "sys.settrace(tracer)\n"
                    "def second():\n"
                    "    print('SECOND-RAN', flush=True)"
                )
                steps = (
                    "[Step('one', ('fast',), lambda: [['/usr/bin/true']]),"
                    " Step('two', ('fast',), action=second)]"
                )
                result = self.driver(
                    steps, level="fast", watchdog=30.0, prelude=prelude, timeout=30
                )
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                self.assertNotIn("SECOND-RAN", result.stdout)
                self.assertNotIn("PASS", result.stdout)

    def test_08z2_a_cause_recorded_in_the_last_steps_hand_off_is_not_a_pass(self) -> None:
        """The same hand-off in a single-step run: nothing follows to decide, so checks() decides
        after the last step and the run cannot report PASS."""
        for signal_name, expected in (("SIGALRM", 75), ("SIGTERM", 143)):
            with self.subTest(signal=signal_name):
                prelude = (
                    "import inspect, os\n"
                    "source, first = inspect.getsourcelines(check.run)\n"
                    "target = next(first + i for i, line in enumerate(source)\n"
                    "              if line.strip().startswith('own.owned = False'))\n"
                    "fired = []\n"
                    "def local(frame, event, arg):\n"
                    "    if event == 'line' and frame.f_lineno == target and not fired:\n"
                    "        fired.append(1)\n"
                    f"        os.kill(os.getpid(), signal.{signal_name})\n"
                    "    return local\n"
                    "def tracer(frame, event, arg):\n"
                    "    return local if frame.f_code is check.run.__code__ else None\n"
                    "sys.settrace(tracer)"
                )
                steps = "[Step('only', ('fast',), lambda: [['/usr/bin/true']])]"
                result = self.driver(
                    steps, level="fast", watchdog=30.0, prelude=prelude, timeout=30
                )
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                self.assertNotIn("PASS", result.stdout)

    SHIM = (
        "#include <signal.h>\n#include <sys/wait.h>\n#include <unistd.h>\n"
        "int wait_after_signal(int pid, int sig, int flags, int *status) {\n"
        "    kill(getpid(), sig);  /* the C-level handler only flags it; no Python runs before the wait */\n"
        "    return waitpid(pid, status, flags);\n"
        "}\n"
    )

    def test_08aa_an_alarm_that_lands_just_before_the_wait_syscall_is_still_acted_on(self) -> None:
        """The reviewer's wait-entry window, modelled natively: a tiny C shim (compiled here, loaded
        only into the disposable driver) raises the single SIGALRM and then enters waitpid with no
        Python bytecode in between, so the interpreter has flagged the signal but not run its handler.
        An unbounded wait stays blocked with the live child; bounded waits act on it within one poll."""
        source = self.base / "shim.c"
        source.write_text(self.SHIM)
        library = self.base / "shim.dylib"
        subprocess.run(["cc", "-shared", "-o", str(library), str(source)], check=True)
        for level in ("fast", "full"):
            with self.subTest(level=level):
                pidfile = self.base / f"window-{level}.pids"
                self.fixture_pids(pidfile)
                prelude = (
                    "import contextlib, ctypes, os, time\n"
                    "from pathlib import Path\n"
                    # The host side's readiness check, its own source, so both wait the same way.
                    f"{inspect.getsource(wait_for_pids)}"
                    f"lib = ctypes.CDLL({str(library)!r}, use_errno=True)\n"
                    "lib.wait_after_signal.argtypes = [ctypes.c_int] * 3\n"
                    "lib.wait_after_signal.argtypes.append(ctypes.POINTER(ctypes.c_int))\n"
                    "real, fired = os.waitpid, []\n"
                    "def waitpid(pid, flags):\n"
                    "    if fired:\n"
                    "        return real(pid, flags)\n"
                    "    fired.append(1)\n"
                    "    # the step and its child run once both PIDs are written\n"
                    f"    wait_for_pids(Path({str(pidfile)!r}), 2)\n"
                    "    status = ctypes.c_int(0)\n"
                    "    result = lib.wait_after_signal(pid, signal.SIGALRM, flags, ctypes.byref(status))\n"
                    "    if result < 0:\n"
                    "        raise OSError(ctypes.get_errno(), 'waitpid')\n"
                    "    return result, status.value\n"
                    "os.waitpid = waitpid"
                )
                code = self.build_driver_source(
                    self.hanging_step("hang", pidfile), level, watchdog=30.0, prelude=prelude
                )
                session = Session(self, code)
                _, errors = session.finish(seconds=BOUND)  # an unbounded wait fails here
                self.assertEqual(session.returncode, 75, errors)
                self.assertIn("killed and reaped. Exiting 75", errors)
                survivors = [
                    pid for pid in map(int, pidfile.read_text().split()) if process_state(pid)
                ]
                self.assertEqual(survivors, [])

    def test_08t_the_standalone_doctor_interrupted_by_ctrl_c_ends_its_tool_group(self) -> None:
        """The reviewer's B3: `ck doctor` has no run state (no watchdog), yet an interrupted tool and
        its child must still be ended and reaped before the interrupt propagates."""
        pidfile = self.base / "standalone.pids"
        self.fixture_pids(pidfile)
        tools = self.base / "standalone-bin"
        tools.mkdir()
        fake = tools / "rustc"
        fake.write_text(f'#!/bin/bash\nsleep 300 &\necho "$$ $!" > {pidfile}\nwait\n')
        fake.chmod(0o755)
        code = textwrap.dedent(
            f"""
            import sys
            sys.path.insert(0, {str(TOOLS)!r})
            import check
            check.doctor()
            """
        )
        process = subprocess.Popen(
            [sys.executable, "-c", code],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env={
                **clean_environment(),
                "PATH": f"{tools}:{os.environ['PATH']}",
                "CK_HUB": str(self.hub),
            },
        )
        wait_for_pids(pidfile, 2)
        time.sleep(0.2)
        process.send_signal(signal.SIGINT)
        _, errors = process.communicate(timeout=30)
        self.assertIn("KeyboardInterrupt", errors)
        survivors = [pid for pid in map(int, pidfile.read_text().split()) if process_state(pid)]
        self.assertEqual(survivors, [], "the interrupted standalone doctor left its tool running")

    def standalone_doctor_session(self, name: str, wrappers: str) -> tuple[Session, Path]:
        """A driver running `check.doctor()` (no run state) over a fake `rustc` that hangs, with `wrappers`
        (Python source needing `check`, `RELEASE`'s helpers) installed first. Returns it with its pidfile."""
        pidfile = self.base / f"{name}.pids"
        self.fixture_pids(pidfile)
        tools = self.base / f"{name}-bin"
        tools.mkdir()
        fake = tools / "rustc"
        fake.write_text(f'#!/bin/bash\nsleep 300 &\necho "$$ $!" > {pidfile}\nwait\n')
        fake.chmod(0o755)
        # The doctor runs `glslc --version` (owned, and instant) before the toolchain probe: the wrappers are
        # about the probe the fake `rustc` hangs in, so they act only on a process whose program is `rustc`.
        only_the_rustc_probe = (
            "wrapped_end = check.end_group\n"
            "def only_the_rustc_probe(process):\n"
            "    if os.path.basename(str(process.args[0])) != 'rustc':\n"
            "        return unwrapped_end(process)\n"
            "    return wrapped_end(process)\n"
            "check.end_group = only_the_rustc_probe\n"
        )
        code = (
            RELEASE
            + textwrap.dedent(
                f"""
            import sys
            sys.path.insert(0, {str(TOOLS)!r})
            import check
            unwrapped_end = check.end_group
            """
            )
            + wrappers
            + only_the_rustc_probe
            + "\ncheck.doctor()\n"
        )
        session = Session(
            self,
            code,
            stdin=True,
            env={
                **clean_environment(),
                "PATH": f"{tools}:{os.environ['PATH']}",
                "CK_HUB": str(self.hub),
            },
        )
        return session, pidfile

    def glslc_probe_session(self, name: str, fake_body: str) -> tuple[Session, Path]:
        """A driver running `check.glslc_doctor()` (no run state) over a fake `glslc` whose script is
        `fake_body`; it writes its own pid and its background child's to the pidfile (`$PIDFILE`)."""
        pidfile = self.base / f"{name}.pids"
        self.fixture_pids(pidfile)
        tools = self.base / f"{name}-bin"
        tools.mkdir()
        fake = tools / "glslc"
        fake.write_text(
            f'#!/bin/bash\nPIDFILE={pidfile}\nsleep 300 &\necho "$$ $!" > $PIDFILE\n{fake_body}\n'
        )
        fake.chmod(0o755)
        code = (
            RELEASE
            + textwrap.dedent(
                f"""
            import sys
            sys.path.insert(0, {str(TOOLS)!r})
            import check
            os.environ.pop('VULKAN_SDK', None)
            check.GLSLC_DIRECTORIES = ({str(tools)!r},)
            """
            )
            + "\ncheck.glslc_doctor()\nemit('DONE')\n"
        )
        session = Session(
            self,
            code,
            stdin=True,
            env={**clean_environment(), "CK_HUB": str(self.hub)},
        )
        return session, pidfile

    def test_08v_the_glslc_probe_is_an_owned_group_and_leaves_no_descendant(self) -> None:
        """CK-95 review: `glslc --version` goes through `run`/`capture`, so its group is ended and reaped on every
        exit. A fake compiler leaves a sleeping descendant; none may survive a normal return, a nonzero exit, SIGTERM
        or Ctrl-C."""
        # A normal return (the fake prints a version and leaves its child behind).
        session, pidfile = self.glslc_probe_session("glslc-ok", "echo 'shaderc v9'")
        wait_for_pids(pidfile, 2)
        output, errors = session.finish()
        self.assertEqual(session.returncode, 0, errors)
        self.assertIn("glslc shaderc v9", output)
        self.assertEqual(self.survivors_of(pidfile), [], "a normal return left a descendant")
        # A nonzero exit: refused naming the compiler, and nothing survives.
        session, pidfile = self.glslc_probe_session("glslc-fail", "exit 3")
        wait_for_pids(pidfile, 2)
        _, errors = session.finish()
        self.assertNotEqual(session.returncode, 0)
        self.assertIn("does not run", errors)
        self.assertEqual(self.survivors_of(pidfile), [], "a nonzero exit left a descendant")
        # A hang, interrupted by either signal: the first cause decides the exit, and nothing survives.
        for sent, codes in (
            (signal.SIGTERM, (143, -signal.SIGTERM)),
            (signal.SIGINT, (130, -signal.SIGINT)),
        ):
            with self.subTest(sent=sent.name):
                session, pidfile = self.glslc_probe_session(f"glslc-hang-{sent.name}", "wait")
                wait_for_pids(pidfile, 2)
                time.sleep(0.2)
                session.send(sent)
                _, errors = session.finish()
                self.assertIn(session.returncode, codes, errors)
                self.assertEqual(
                    self.survivors_of(pidfile), [], f"{sent.name} left the compiler's descendant"
                )

    def test_08u_a_second_ctrl_c_during_the_standalone_doctors_cleanup_cannot_interrupt_it(
        self,
    ) -> None:
        """No run state, same rule: the first Ctrl-C is recorded and the group killed; a second one
        during the cleanup only appends. The interrupt propagates (exit 130) and nothing survives."""
        session, pidfile = self.standalone_doctor_session("standalone2", self.HOLD_WRAPPERS)
        wait_for_pids(pidfile, 2)
        time.sleep(0.2)
        session.send(signal.SIGINT)
        session.until("CLEANUP-START")
        session.send(signal.SIGINT)  # the second Ctrl-C, during the cleanup
        session.until("HANDLED")
        session.release()
        _, errors = session.finish()
        self.assertIn(session.returncode, (-signal.SIGINT, 130), errors)
        survivors = [pid for pid in map(int, pidfile.read_text().split()) if process_state(pid)]
        self.assertEqual(survivors, [], "the second Ctrl-C interrupted the standalone cleanup")

    def test_08q_a_steps_child_keeps_the_parents_signal_state(self) -> None:
        """The child starts with an empty blocked mask and default SIGTERM/SIGINT handling, and a real
        SIGTERM sent to it ends it with status -15 (nothing is masked or ignored around the spawn)."""
        pidfile, report = self.base / "state.pids", self.base / "state.report"
        self.fixture_pids(pidfile)
        body = (
            "import json, os, signal, time\n"
            "mask = sorted(int(n) for n in signal.pthread_sigmask(signal.SIG_BLOCK, []))\n"
            f"open({str(report)!r}, 'w').write(json.dumps({{'mask': mask,\n"
            "    'term_default': signal.getsignal(signal.SIGTERM) == signal.SIG_DFL,\n"
            "    'int': getattr(signal.getsignal(signal.SIGINT), '__name__', 'other')}))\n"
            f"open({str(pidfile)!r}, 'w').write(str(os.getpid()))\n"
            "time.sleep(300)\n"
        )
        step = self.step_source(body, name="state", levels=("fast",))
        process = subprocess.Popen(
            [sys.executable, "-c", self.build_driver_source(step, "fast", watchdog=30.0)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=clean_environment(),
        )
        (child,) = wait_for_pids(pidfile, 1)
        time.sleep(0.2)
        os.kill(child, signal.SIGTERM)  # straight to the child
        _, errors = process.communicate(timeout=30)
        shown = json.loads(report.read_text())
        self.assertEqual(shown, {"mask": [], "term_default": True, "int": "default_int_handler"})
        self.assertEqual(process.returncode, 1, errors)  # the step's own status, not a signal cause
        self.assertIn("SIGTERM", errors)

    def test_09_quick_runs_have_no_watchdog_and_a_slow_passing_step_is_not_killed(self) -> None:
        slow = self.step_source(
            "import time; time.sleep(1.5)", name="slow", levels=("quick", "fast")
        )
        result = self.driver(slow, level="quick", watchdog=0.3)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        fast = self.driver(slow, level="fast", watchdog=30.0)
        self.assertEqual(fast.returncode, 0, fast.stdout + fast.stderr)
        self.assertRegex(fast.stdout, r"check: step slow took 1\.\d s")

    def test_10_the_watchdog_leaves_ordinary_failures_at_exit_1(self) -> None:
        failing = self.step_source("raise SystemExit(3)", name="bad", levels=("fast",))
        result = self.driver(failing, level="fast", watchdog=30.0)
        self.assertEqual(result.returncode, 1)
        self.assertNotIn("watchdog", result.stderr)

    def test_11_the_production_watchdog_is_forty_five_minutes(self) -> None:
        self.assertEqual(check.WATCHDOG, 45 * 60)
        self.assertEqual(check.EX_TEMPFAIL, 75)


class SpawnEnvironment(CheckFixture):
    """Batch CI runs the whole suite with CK_CI_LOCK_FD, CK_CHAT_INPUTS and CK_REQUIRE_FULL_SPEC
    inherited. A test that spawns Python (a driver or check.py) must therefore build its environment
    with clean_environment(); a missing one failed 25 tests only in batch CI."""

    def test_every_python_spawn_in_this_module_builds_its_environment_with_clean_environment(
        self,
    ) -> None:
        module = Path(__file__).read_text()
        spawns: list[tuple[int, bool]] = []
        for node in ast.walk(ast.parse(module)):
            if not (
                isinstance(node, ast.Call)
                and isinstance(node.func, ast.Attribute)
                and node.func.attr in ("Popen", "run", "check_output", "check_call", "call")
                and isinstance(node.func.value, ast.Name)
                and node.func.value.id == "subprocess"
                and node.args
                and isinstance(node.args[0], ast.List)
                and "sys.executable" in (ast.get_source_segment(module, node.args[0]) or "")
            ):
                continue
            env = next((kw.value for kw in node.keywords if kw.arg == "env"), None)
            ok = env is not None and "clean_environment" in (
                ast.get_source_segment(module, env) or ""
            )
            spawns.append((node.lineno, ok))
        self.assertGreater(len(spawns), 5)  # the scan finds the module's spawns
        self.assertEqual(
            [line for line, ok in spawns if not ok], [], "spawns without clean_environment()"
        )

    def test_the_driver_ignores_the_batch_variables_it_inherits(self) -> None:
        inherited = {
            "CK_CI_LOCK_FD": "9",
            "CK_CHAT_INPUTS": "/nonexistent/record",
            "CK_REQUIRE_FULL_SPEC": "1",
        }
        steps = (
            "[Step('probe', ('fast',), action=lambda: print('SEEN', sorted(k for k in os.environ"
            " if k in ('CK_CI_LOCK_FD', 'CK_CHAT_INPUTS', 'CK_REQUIRE_FULL_SPEC'))))]"
        )
        with mock.patch.dict(os.environ, inherited):
            result = self.driver(steps, "fast", prelude="import os")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("SEEN []", result.stdout)


class HubMarker(unittest.TestCase):
    """CK-37: the real-toolchain fixture's `.ck-hub` marker is written from the hub the source copy
    resolves to, so it works from a worker copy (which has a marker) and from the root (which is the
    hub and has none)."""

    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ck-hub-marker-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(os.path.realpath(self.temp.name))
        self.copy = self.base / "copy"
        self.copy.mkdir()

    def test_01_a_root_shaped_source_without_a_marker_gets_a_marker_naming_the_source(self) -> None:
        source = self.base / "root"
        source.mkdir()
        self.assertFalse((source / ".ck-hub").exists())
        write_hub_marker(source, self.copy)
        self.assertEqual((self.copy / ".ck-hub").read_text().strip(), str(source))

    def test_02_a_worker_shaped_source_passes_its_hub_through(self) -> None:
        hub = self.base / "hub"
        hub.mkdir()
        source = self.base / "worker"
        source.mkdir()
        (source / ".ck-hub").write_text(f"{hub}\n")
        write_hub_marker(source, self.copy)
        self.assertEqual((self.copy / ".ck-hub").read_text().strip(), str(hub))

    def test_03_the_marker_names_the_hub_the_source_resolves_to(self) -> None:
        for source in (REPO, self.base):
            with self.subTest(source=source):
                write_hub_marker(source, self.copy)
                self.assertEqual(
                    Path((self.copy / ".ck-hub").read_text().strip()), environment.hub_for(source)
                )


class RealToolchain(unittest.TestCase):
    """Test 7: the real Cargo recipe on a disposable copy, interrupted with SIGKILL."""

    def kill_during_a_cargo_step(self) -> tuple[Path, set[int]]:
        """Start a real planned check (the focus-free `rust.core` group) on a disposable copy and
        SIGKILL check.py while a real Cargo step is compiling. Returns (copy, orphans still alive)."""
        temp = tempfile.TemporaryDirectory(prefix="ck-check-real-")
        self.addCleanup(temp.cleanup)
        copy = Path(os.path.realpath(temp.name)) / "copy"
        copy.mkdir()
        # Only what the toolchain recipe needs: no docs/ and no top-level *.md (ordinary
        # documentation is not a test input, CK-33).
        for name in TOOLCHAIN_INPUTS:
            if name == ".ck-hub":  # only worker copies have a marker; the root is the hub
                write_hub_marker(REPO, copy)
                continue
            source = REPO / name
            if source.is_dir():
                shutil.copytree(source, copy / name, ignore=SCRATCH)
            else:
                shutil.copy2(source, copy / name)
        self.assertTrue((copy / ".ck-hub").exists())
        # ShellCheck discovery reads the tracked files, so the copy is a (never recorded) repository.
        for arguments in (["init"], ["add", "--recursive", "."]):
            subprocess.run(
                ["pijul", *arguments],
                cwd=copy,
                env=clean_environment(),
                check=True,
                capture_output=True,
            )
        self.assertEqual(
            sorted(path.name for path in copy.iterdir()),
            sorted([*TOOLCHAIN_INPUTS, ".pijul"]),
            "copy inputs",
        )
        process = subprocess.Popen(
            [sys.executable, str(copy / "tools/check.py"), *REAL_CHECK_ARGUMENTS],
            cwd=copy,
            # No ssh-agent: a SIGKILLed check.py could not clean one up.
            env=clean_environment(CK_SIGNING_ACTIVE=(copy / ".ck-hub").read_text().strip()),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        seen: dict[int, str] = {}

        def cleanup() -> None:
            for pid in [*list(seen), process.pid]:
                with contextlib.suppress(ProcessLookupError):
                    os.kill(pid, signal.SIGKILL)

        self.addCleanup(cleanup)
        deadline = time.monotonic() + 600
        while time.monotonic() < deadline:
            self.assertIsNone(process.poll(), "check.py ended before a Cargo step compiled")
            found = descendants(process.pid)
            # A real Cargo build or test step (never clippy): a `cargo test|build` process with rustc
            # compiling a crate, or a test binary running from the slot directory.
            building = any(
                Path(command.split()[0]).name == "cargo"
                and command.split()[1:2] in (["test"], ["build"])
                for command in found.values()
            )
            if building and any(
                (Path(command.split()[0]).name == "rustc" and "--crate-name" in command)
                or "/target/ck-full/0/debug/deps/" in command.split()[0]
                for command in found.values()
            ):
                seen.update(found)
                break
            time.sleep(0.02)
        else:
            self.fail("no Cargo build or test step reached rustc or a test binary")
        os.kill(process.pid, signal.SIGKILL)
        process.wait()
        return copy, {pid for pid in seen if alive(pid)}

    def test_07_sigkill_of_check_py_leaves_the_slot_held_by_the_real_toolchain(self) -> None:
        for _ in range(5):  # the toolchain can finish between detection and the kill
            copy, orphans = self.kill_during_a_cargo_step()
            if orphans:
                break
        self.assertTrue(orphans, "the Cargo descendants always died with check.py")
        base = copy / "target/ck-full"
        # Orphaned descendants keep slot 0 held (they inherited its descriptor): the refusal is
        # the nonblocking flock, and a new real manual run takes slot 1.
        self.assertFalse(slot_free(base / "0.lock"))
        self.assertEqual(self.slot_of_a_manual_run(copy), 1)
        # Slot 0 stays held for as long as any orphaned process lives, then is free again.
        deadline = time.monotonic() + 600
        while True:
            living = [pid for pid in orphans if alive(pid)]
            free = slot_free(base / "0.lock")
            if living:
                self.assertFalse(free, f"slot 0 was free while {living} lived")
            elif free:
                break
            self.assertLess(time.monotonic(), deadline, "the orphaned toolchain never exited")
            time.sleep(0.05)
        self.assertEqual(self.slot_of_a_manual_run(copy), 0)  # a later run reuses slot 0

    def slot_of_a_manual_run(self, copy: Path) -> int:
        """Start a real manual planned check.py on the copy, read the slot it announces, then
        SIGTERM it (it kills and reaps its step group before releasing the slot)."""
        process = subprocess.Popen(
            [sys.executable, str(copy / "tools/check.py"), *REAL_CHECK_ARGUMENTS],
            cwd=copy,
            env=clean_environment(),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        try:
            assert process.stdout is not None
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                line = process.stdout.readline()
                if line.startswith("check: build slot "):
                    return int(line.split()[3])
                if not line and process.poll() is not None:
                    break
            self.fail("the manual run never announced a build slot")
        finally:
            process.send_signal(signal.SIGTERM)
            try:
                process.communicate(timeout=120)
            except subprocess.TimeoutExpired:
                process.kill()
                process.communicate()


if __name__ == "__main__":
    unittest.main()
