#!/usr/bin/env python3
"""The same checks run locally and in the acceptance candidate.

One ordered catalog of check groups (`STEPS`); each group has an id, a category (`floor`, `affected` or
`optional`), the path patterns that affect it and the exact commands it runs. Each level selects groups
and runs each selected one once, in order:

- no option: the **full** level, every group. Batch validation runs this and records it; it is the only
  evidence a batch accepts.
- `--fast`: every group like full, without the build slot and the full-spec variable; `--quick`: the
  three static floor groups (the record hook).
- `--changed` and/or `--group ID`: a **planned** level. `--changed` compares the copy with the accepted
  base and selects the floor groups, every affected group a changed path matches and the groups named by
  `--group`; the plan says, for every group, whether it runs and why. An unclaimed changed path or a
  failure to compute the change set selects every affected group. A planned run names itself `planned`
  and is never batch evidence. `--plan` prints the plan without running it.

Only batch validation holds `ci.lock` (through the inherited `CK_CI_LOCK_FD`); manual runs hold no
project lock.
"""

from __future__ import annotations

import argparse
import contextlib
import ctypes
import fcntl
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Iterable, Iterator, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import FrameType
from typing import IO

from environment import ci_descriptor, configure, signing
from gate import lease
from gate.lease import Handler
from gate.pointer import read_pointer

ROOT = Path(__file__).resolve().parent.parent
LEVELS = ("quick", "fast", "full", "planned")
SLOT_LIMIT = 64
GRACE = 3.0
KILL_PATIENCE = 10.0
POLL = 0.05  # longest a wait on an owned child is allowed to block before the causes are looked at
# One step of a fast or full run may take this long (seconds); the slowest step of a batch run is
# measured in the S4 report and this stays at least ten times that.
WATCHDOG = 45 * 60
EX_TEMPFAIL = 75  # sysexits.h: a temporary failure, which batch prepare classes as infrastructure


@dataclass(frozen=True)
class Step:
    """One recipe step: the commands it runs (or an action), and the levels that include it."""

    name: str
    levels: tuple[str, ...]
    commands: Callable[[], list[list[str]]] = list
    action: Callable[[], None] | None = None
    full_spec: bool = False  # sets CK_REQUIRE_FULL_SPEC=1 in full runs (no effect until S10)
    analyze: bool = False  # sets CK_NATIVE_ANALYZE=1: the Clang analyzer runs in the build script
    env: tuple[
        tuple[str, str], ...
    ] = ()  # (name, value) pairs this step's commands always run with
    env_remove: tuple[
        str, ...
    ] = ()  # names removed from the inherited environment, even one the caller exported
    description: str = (
        ""  # one line: what the group checks (the plan and the catalog tests show it)
    )
    category: str = "affected"  # floor | affected | optional (CATEGORIES)
    paths: tuple[str, ...] = ()  # the patterns of the paths that affect the group (see claims())
    needs: str = "plain"  # plain | gpu | window: what any command the group starts may need (NEEDS)


class StepTimeout(Exception):
    """A step outlived the watchdog; its process group was killed and reaped."""

    def __init__(self, step: str | None, elapsed: float, limit: float) -> None:
        super().__init__(step, elapsed, limit)
        self.step, self.elapsed, self.limit = step, elapsed, limit
        # Whether the step's process group was killed and reaped (None: the step had no group).
        self.cleaned: bool | None = None


class State:
    """How the current run executes its children: environment, descriptors, process groups."""

    def __init__(  # noqa: PLR0913  # explicit arguments; a parameter object would obscure the call sites
        self,
        root: Path,
        hub: Path,
        level: str,
        *,
        batch: bool,
        target: Path | str,
        slot: tuple[int, int] | None = None,
    ) -> None:
        self.root, self.hub, self.level = root, hub, level
        self.batch, self.target, self.slot = batch, Path(target), slot
        # The CI lock, the build slot and every checkpoint lease: all inherited by each step's children.
        self.descriptors = tuple(
            fd
            for fd in (
                ci_descriptor(hub) if batch else None,
                slot[1] if slot else None,
                *lease.descriptors(),
            )
            if fd is not None
        )
        # The current step and its start (the run state only adds the SIGALRM watchdog).
        self.step: str | None = None
        self.step_started: float | None = None

    def environment(self, step: Step | None) -> dict[str, str]:
        env = os.environ.copy()
        # One directory for artifacts and intermediates; the legacy and config forms cannot redirect it.
        env["CARGO_TARGET_DIR"] = env["CARGO_BUILD_BUILD_DIR"] = str(self.target)
        env.pop("CARGO_BUILD_TARGET_DIR", None)
        if not self.batch:
            # Only a batch writes CK-6's chat-inputs record.
            env.pop("CK_CHAT_INPUTS", None)
            env.pop("CK_CI_LOCK_FD", None)
        if step is not None and step.full_spec and self.level in ("full", "planned"):
            env["CK_REQUIRE_FULL_SPEC"] = "1"
        # The analyzer runs once, in the build-script unit shared by doc, test and build; Clippy's
        # own unit must not see the variable, and neither may a value the caller exported.
        env.pop("CK_NATIVE_ANALYZE", None)
        if step is not None and step.analyze:
            env["CK_NATIVE_ANALYZE"] = "1"
        if step is not None:
            for name in step.env_remove:
                env.pop(name, None)
            env.update(step.env)
        if lease.descriptors():
            env[lease.ANNOUNCE] = (
                lease.announcement()
            )  # a Python step adopts the leases it inherits
        return env


_state: State | None = None


class Ownership:
    """Who owns a running child, shared by every `run()` (with or without run state).

    While a child is owned (`owned`), and for the rest of the process once any cause exists, the
    SIGALRM, SIGTERM and SIGINT handlers only append their cause to the append-only `causes` list
    (its first element is the first cause) and SIGKILL the child's group once `group` is known.
    They raise nothing, so no cleanup can be bypassed and no later signal can change the status."""

    def __init__(self) -> None:
        self.owned = False
        self.group: int | None = None
        self.causes: list[str] = []

    def reset(self) -> None:
        """No cause or ownership carries over from an earlier run in this process."""
        self.owned = False
        self.group = None
        self.causes = []


_own = Ownership()


def group_members(group: int) -> list[tuple[int, str]]:
    """(pid, state) of every process in the group, from ps (a zombie shows state Z)."""
    output = subprocess.run(
        ["ps", "-axo", "pid=,pgid=,stat="], capture_output=True, text=True, check=True
    ).stdout
    members: list[tuple[int, str]] = []
    for line in output.splitlines():
        fields = line.split()
        if len(fields) == 3 and fields[1] == str(group):  # noqa: PLR2004  # a ps row has three fields: pid, pgid, state
            members.append((int(fields[0]), fields[2]))
    return members


def live_members(group: int) -> list[int]:
    """Group members that are not zombies; a zombie cannot be signalled and is not running."""
    return [pid for pid, state in group_members(group) if not state.startswith("Z")]


def end_group(process: subprocess.Popen[bytes]) -> bool:
    """Kill the step's process group and reap its direct child; True only if both are done.

    The leader is our child, so it stays a zombie until `process.wait` returns its status: every
    return path here has either reaped it that way or reports that it could not. Descendants that
    outlive the leader are signalled until `ps` shows no live member. A signal that is denied for a
    live process is reported, never hidden; EPERM for a group whose members are all zombies is not a
    denial (macOS answers that way), and is recognised from the ps rows, not assumed."""
    group = process.pid
    denied: list[str] = []

    def signal_group(sig: signal.Signals) -> None:
        try:
            os.killpg(group, sig)
        except ProcessLookupError:
            pass  # already gone
        except PermissionError:
            rows = group_members(group)
            if any(not state.startswith("Z") for _, state in rows):
                denied.append(f"{sig.name} denied (EPERM); group members {rows}")

    def reap(timeout: float) -> bool:
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            return False
        return True

    reaped = process.poll() is not None
    if not reaped:
        signal_group(signal.SIGTERM)
        reaped = reap(GRACE)
    if not reaped:
        signal_group(signal.SIGKILL)
        reaped = reap(KILL_PATIENCE)
    for sig, patience in ((signal.SIGTERM, GRACE), (signal.SIGKILL, KILL_PATIENCE)):
        if not live_members(group):
            break
        signal_group(sig)
        deadline = time.monotonic() + patience
        while live_members(group) and time.monotonic() < deadline:
            time.sleep(0.05)
    remaining = live_members(group)
    for message in denied:
        print(f"check: {message}", file=sys.stderr)
    if not reaped:
        print(f"check: step process {group} was not reaped", file=sys.stderr)
    if remaining:
        print(f"check: step processes still running: {remaining}", file=sys.stderr)
    return reaped and not remaining and not denied


def record_cause(own: Ownership, cause: str) -> None:
    """The handlers' whole job while a child is owned: append the cause, kill the group if known."""
    own.causes.append(cause)
    if own.group is not None:
        with contextlib.suppress(ProcessLookupError, PermissionError):
            os.killpg(own.group, signal.SIGKILL)


def wait_bounded(process: subprocess.Popen[bytes], own: Ownership) -> int:
    """Wait for the owned child in short bounded waits and return its status.

    A signal that arrives after the interpreter's last check but before a blocking syscall starts is
    flagged and not acted on until the syscall returns, so no wait on an owned child is unbounded:
    after every wait of at most POLL seconds the causes are looked at, and the group is killed if one
    exists. A missed signal is therefore acted on within one poll."""
    while True:
        try:
            return process.wait(timeout=POLL)
        except subprocess.TimeoutExpired:
            if own.causes:
                with contextlib.suppress(ProcessLookupError, PermissionError):
                    os.killpg(process.pid, signal.SIGKILL)


def decide(own: Ownership, cleaned: bool | None) -> None:
    """After the group is ended: the first recorded cause decides the status, ahead of the command's
    own exit status and of a cleanup that could not be confirmed. Handlers only append meanwhile."""
    first = own.causes[0] if own.causes else None
    if first == "terminated":
        raise SystemExit(128 + signal.SIGTERM)
    if first == "interrupted":
        raise KeyboardInterrupt
    if first == "expired":
        state = _state
        started = state.step_started if state and state.step_started is not None else None
        elapsed = time.monotonic() - started if started is not None else 0.0
        error = StepTimeout(state.step if state else None, elapsed, WATCHDOG)
        error.cleaned = cleaned
        raise error


def capture(arguments: Sequence[str | Path], cwd: Path | None = None) -> str:
    """Run a command under the same group, watchdog and echo as `run`; return its stdout."""
    with tempfile.TemporaryFile("w+") as output:
        # Outside a check run (`ck doctor`) the tools resolve from the caller's directory, as before.
        run(arguments, stdout=output, cwd=cwd or (None if _state else Path.cwd()))
        output.seek(0)
        return output.read()


def run(
    arguments: Sequence[str | Path],
    step: Step | None = None,
    *,
    cwd: Path | None = None,
    stdout: IO[str] | IO[bytes] | int | None = None,
) -> None:
    """Echo and run one command in its own process group, ending that group before returning.

    The command owns its child from just before `Popen` (group not yet known) until the group has
    been reaped, with or without run state (the standalone doctor has none; run state only adds the
    watchdog). While owned, SIGTERM and Ctrl-C (and the watchdog) only record the first cause and
    kill the group; this function reaps the whole group on every exit, and the first cause decides
    what is raised. Nothing is masked: the child starts with the parent's normal signal state."""
    print("+ " + " ".join(map(str, arguments)), flush=True)
    state = _state
    own = _own
    env = state.environment(step) if state else None
    if env is None and lease.descriptors():
        env = {**os.environ, lease.ANNOUNCE: lease.announcement()}
    descriptors = tuple(
        dict.fromkeys([*(state.descriptors if state else ()), *lease.descriptors()])
    )
    if own.causes:
        decide(own, None)  # a cause recorded earlier stops the step before anything is spawned
    previous: dict[int, Handler] = {
        signal.SIGTERM: signal.signal(signal.SIGTERM, terminate),
        signal.SIGINT: signal.signal(signal.SIGINT, interrupt),
    }
    process: subprocess.Popen[bytes] | None = None
    ended, cleaned = False, None
    try:
        own.owned = True
        process = subprocess.Popen(
            arguments,
            cwd=cwd or (state.root if state else ROOT),
            env=env,
            pass_fds=descriptors,
            process_group=0,
            stdout=stdout,
        )
        own.group = process.pid
        if own.causes:  # a signal arrived before the group was known: end it now
            with contextlib.suppress(ProcessLookupError, PermissionError):
                os.killpg(own.group, signal.SIGKILL)
        code = wait_bounded(process, own)
        cleaned = end_group(process)
        ended = True
        own.group = None  # reaped: a later kill would find no (or someone else's) group
        if not cleaned:
            raise RuntimeError(f"could not end the step's process group {process.pid}; see above")
        if code:
            raise subprocess.CalledProcessError(code, arguments)
    except BaseException:
        # Whenever a child exists, every exit ends and reaps its group first.
        if process is not None and not ended:
            cleaned = end_group(process)
        raise
    finally:
        # State first, decision after: ownership is cleared and only then are the causes tested, so a
        # signal before the clear is in the list when tested, and one after it takes the no-child
        # path, which records its cause before raising. The handlers stay record-only meanwhile.
        own.group = None
        own.owned = False
        if own.causes:
            # The one exit decision, after the cleanup: the first cause decides (75, 143 or 130) and
            # replaces any exception or return in flight.
            decide(own, cleaned)  # deliberately replaces whatever is in flight
        for number, handler in previous.items():
            signal.signal(number, handler)


INSTALL = "tools/bootstrap"  # installs every tool a recipe step needs; it only adds what is missing
SHELLCHECK_VERSION = (
    "0.11.0"  # `shellcheck --version` reports the program, not the shellcheck-py wheel
)
MYPY_VERSION = (
    "2.4.0"  # the newest release supporting the venv Python; tools/requirements.txt pins it
)
RUSTC_VERSION, PIJUL_VERSION, RUFF_VERSION = "1.91.1", "1.0.0-beta.24", "0.14.0"


def hub_tool(name: str) -> str:
    return str(Path(os.environ["CK_HUB"]) / ".tools/python/bin" / name)


# CK-86: ripgrep is a required tool. Agents search the repository with it, and `.rgignore` hides the archived
# and guide records from that search; `tools/bootstrap` installs it with Homebrew.
RIPGREP_PLACES = ("/opt/homebrew/bin/rg", "/usr/local/bin/rg")


def ripgrep() -> str:
    """The `rg` to run: the one on PATH, else Homebrew's. Raises, naming the installation command, when
    there is none (nothing is silently skipped)."""
    found = shutil.which("rg")
    if found:
        return found
    for place in RIPGREP_PLACES:
        if os.access(place, os.X_OK):
            return place
    raise RuntimeError(
        f"ripgrep (rg) is missing: not on PATH and not in {', '.join(RIPGREP_PLACES)}; "
        f"install it with: {INSTALL}"
    )


def probe(label: str, arguments: Sequence[str | Path], install: str = INSTALL) -> str:
    """A tool's output, or an error naming the installation command (nothing is silently skipped)."""
    try:
        return capture(arguments).strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise RuntimeError(
            f"{label} is missing or not runnable ({arguments[0]}): {error}; install it with: {install}"
        ) from error


GLFW_MINIMUM = (
    3,
    4,
)  # GLFW 3.4 or any newer 3.x; Homebrew's `glfw` is 3.5.1 at the time of writing
GRAPHICS_INSTALL = (
    "tools/bootstrap installs GLFW (Homebrew `glfw`) and the shader compiler glslc (Homebrew `shaderc`); the "
    "Vulkan loader and the Khronos validation layer come from the LunarG Vulkan SDK for macOS (or: brew "
    "install vulkan-loader vulkan-validationlayers)"
)
# Where `crates/gpu-vulkan/build.rs` looks for `glslc`, after `$VULKAN_SDK/bin` when that variable is set: by
# explicit path, never by PATH alone. The two lists are the same, in the same order (a catalog test compares them).
GLSLC_DIRECTORIES = ("/usr/local/bin", "/opt/homebrew/bin")
VULKAN_LIBRARY_DIRECTORIES = ("/usr/local/lib", "/opt/homebrew/lib")
VULKAN_LAYER_DIRECTORIES = (
    "/usr/local/share/vulkan/explicit_layer.d",
    "/opt/homebrew/share/vulkan/explicit_layer.d",
    "/usr/share/vulkan/explicit_layer.d",
)
VALIDATION_LAYER = "VkLayer_khronos_validation.json"


def glslc_search_directories() -> list[Path]:
    """The directories `build.rs` searches for `glslc`: `$VULKAN_SDK/bin` first when that is set, then the fixed ones."""
    sdk = os.environ.get("VULKAN_SDK")
    return ([Path(sdk) / "bin"] if sdk else []) + [Path(d) for d in GLSLC_DIRECTORIES]


def glslc_doctor() -> None:
    """`glslc` exists in one of the directories the build searches and runs (`glslc --version`). A missing or
    failing one raises, naming expected and found and the install command; it is never skipped."""
    searched = glslc_search_directories()
    expected = f"glslc in {' or '.join(map(str, searched))}"
    found = next((d / "glslc" for d in searched if (d / "glslc").is_file()), None)
    if found is None:
        raise RuntimeError(f"Expected {expected}, found none; {GRAPHICS_INSTALL}")
    # Through `capture`, like every other tool probe: the compiler runs in its own process group, which is ended
    # and reaped on every exit (a normal return, a nonzero exit, SIGTERM or Ctrl-C, the step's watchdog), and the
    # candidate leases are passed on while it lives.
    try:
        output = capture([str(found), "--version"]).strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise RuntimeError(
            f"Expected {expected}, but {found} does not run ({error}); {GRAPHICS_INSTALL}"
        ) from error
    print(
        f"glslc {output.splitlines()[0] if output else '(no version text)'} ({found})", flush=True
    )


def glfw_keg() -> Path:
    """The Homebrew keg GLFW is linked from: the one rule `crates/window-glfw/build.rs` also follows."""
    return Path(os.environ.get("HOMEBREW_PREFIX", "/opt/homebrew")) / "opt/glfw"


def glfw_header_version(header: Path) -> tuple[int, int, int]:
    """The `GLFW_VERSION_MAJOR/MINOR/REVISION` a `glfw3.h` declares."""
    text = header.read_text()
    found = [
        re.search(rf"^#define GLFW_VERSION_{part}\s+(\d+)", text, re.MULTILINE)
        for part in ("MAJOR", "MINOR", "REVISION")
    ]
    if not all(found):
        raise RuntimeError(f"{header} does not declare GLFW_VERSION_MAJOR/MINOR/REVISION")
    major, minor, revision = (int(match.group(1)) for match in found if match)
    return major, minor, revision


def glfw_runtime_version(library: Path) -> tuple[int, int, int]:
    """What `glfwGetVersion` of the library reports (it needs no initialisation)."""
    glfw = ctypes.CDLL(str(library))
    parts = [ctypes.c_int(0) for _ in range(3)]
    glfw.glfwGetVersion(*(ctypes.byref(part) for part in parts))
    major, minor, revision = (part.value for part in parts)
    return major, minor, revision


def graphics_doctor() -> None:
    """GLFW 3.4 or newer 3.x (library and header agree), the Vulkan loader and the Khronos validation
    layer manifest. A missing piece raises, naming expected and found and the install command; it is never
    skipped. The groups that need them select this check (`doctor.graphics`), and so does the full level."""
    keg = glfw_keg()
    header, library = keg / "include/GLFW/glfw3.h", keg / "lib/libglfw.dylib"
    expected = f"GLFW {GLFW_MINIMUM[0]}.{GLFW_MINIMUM[1]} or newer {GLFW_MINIMUM[0]}.x"
    for needed in (header, library):
        if not needed.exists():
            raise RuntimeError(f"Expected {expected}, found no {needed}; {GRAPHICS_INSTALL}")
    try:
        declared, running = glfw_header_version(header), glfw_runtime_version(library)
    except (OSError, RuntimeError) as error:
        raise RuntimeError(
            f"Expected {expected}, but GLFW cannot be read ({error}); {GRAPHICS_INSTALL}"
        ) from error
    found = ".".join(map(str, running))
    if declared != running or running[0] != GLFW_MINIMUM[0] or running[:2] < GLFW_MINIMUM:
        raise RuntimeError(
            f"Expected {expected}, found the library {found} with a header declaring "
            f"{'.'.join(map(str, declared))} ({keg}); {GRAPHICS_INSTALL}"
        )
    print(f"GLFW {found} ({keg}; expected {expected})", flush=True)
    loader = next(
        (
            Path(d) / "libvulkan.1.dylib"
            for d in VULKAN_LIBRARY_DIRECTORIES
            if (Path(d) / "libvulkan.1.dylib").exists()
        ),
        None,
    )
    if loader is None:
        raise RuntimeError(
            f"Expected the Vulkan loader libvulkan.1.dylib in {' or '.join(VULKAN_LIBRARY_DIRECTORIES)}, "
            f"found none; {GRAPHICS_INSTALL}"
        )
    version = ctypes.c_uint32(0)
    status = ctypes.CDLL(str(loader)).vkEnumerateInstanceVersion(ctypes.byref(version))
    api = (version.value >> 22, (version.value >> 12) & 0x3FF, version.value & 0xFFF)
    if status != 0 or api < (1, 3, 0):
        raise RuntimeError(
            f"Expected a Vulkan loader of 1.3 or newer, found {'.'.join(map(str, api))} "
            f"(status {status}, {loader}); {GRAPHICS_INSTALL}"
        )
    print(
        f"Vulkan loader {'.'.join(map(str, api))} ({loader}; programs load it by this absolute path and "
        "give its vkGetInstanceProcAddr to GLFW)",
        flush=True,
    )
    manifests = [Path(d) / VALIDATION_LAYER for d in VULKAN_LAYER_DIRECTORIES]
    manifest = next((path for path in manifests if path.exists()), None)
    if manifest is None:
        raise RuntimeError(
            f"Expected the Khronos validation layer manifest {VALIDATION_LAYER} in "
            f"{' or '.join(VULKAN_LAYER_DIRECTORIES)}, found none; {GRAPHICS_INSTALL}"
        )
    try:
        name = json.loads(manifest.read_text())["layer"]["name"]
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise RuntimeError(
            f"The validation layer manifest {manifest} cannot be read ({error}); {GRAPHICS_INSTALL}"
        ) from error
    if name != "VK_LAYER_KHRONOS_validation":
        raise RuntimeError(
            f"Expected the layer VK_LAYER_KHRONOS_validation in {manifest}, found {name!r}; {GRAPHICS_INSTALL}"
        )
    print(f"Vulkan validation layer {name} ({manifest})", flush=True)
    glslc_doctor()


def doctor(graphics: bool = True) -> None:
    """The toolchain and environment checks. `graphics` adds GLFW, the Vulkan loader and the validation
    layer (`ck doctor` does; the always-run floor group does not, its own group `doctor.graphics` does)."""
    if graphics:
        graphics_doctor()
    tools: list[tuple[str, list[str], Callable[[str], bool], str]] = [
        ("rustc", ["rustc", "--version"], lambda out: RUSTC_VERSION in out.split(), RUSTC_VERSION),
        ("pijul", ["pijul", "--version"], lambda out: PIJUL_VERSION in out.split(), PIJUL_VERSION),
        (
            "ruff",
            [hub_tool("ruff"), "--version"],
            lambda out: RUFF_VERSION in out.split(),
            RUFF_VERSION,
        ),
        (
            "ShellCheck",
            [hub_tool("shellcheck"), "--version"],
            lambda out: f"version: {SHELLCHECK_VERSION}" in out.splitlines(),
            SHELLCHECK_VERSION,
        ),
        (
            "mypy",
            [hub_tool("mypy"), "--version"],
            lambda out: out.split()[1:2] == [MYPY_VERSION],
            MYPY_VERSION,
        ),
        (
            "ripgrep",
            [ripgrep(), "--version"],
            lambda out: out.startswith("ripgrep "),
            "any version",
        ),
    ]
    for label, arguments, accepted, expected in tools:
        output = probe(label, arguments)
        print(output, flush=True)
        if not accepted(output):
            found = output.splitlines()[0] if output else "no output"
            raise RuntimeError(
                f"Expected {label} {expected}, found {found!r} ({arguments[0]}); "
                f"install it with: {INSTALL}"
            )
    # Xcode is an owner-managed application and is not pinned: its versions are recorded in every
    # log, and the analyzer needs Apple's clang from the selected Xcode.
    print(probe("Xcode", ["xcodebuild", "-version"], "install Xcode and select it"), flush=True)
    clang = probe(
        "Apple clang", ["xcrun", "--sdk", "macosx", "clang", "--version"], "install Xcode"
    )
    print(clang, flush=True)
    if "Apple clang" not in clang:
        raise RuntimeError(
            "Expected Apple clang from Xcode (the Clang static analyzer needs it); install Xcode "
            "and select it with DEVELOPER_DIR or xcode-select"
        )
    print(
        probe(
            "Metal toolchain",
            ["xcrun", "--sdk", "macosx", "metal", "--version"],
            "xcodebuild -downloadComponent MetalToolchain",
        ),
        flush=True,
    )
    print(f"Python {sys.version.split()[0]}", flush=True)
    if sys.version_info < (3, 11):  # noqa: UP036  # runtime guard: the doctor reports an old interpreter
        raise RuntimeError("Python 3.11+ required")


# ShellCheck discovery (plan section 4.4): the recipe finds shell sources itself and keeps no list.
SHELL_EXCLUDED = (".pipeline/", ".tools/", "target/", "tests/fixtures/legacy/")
SHELL_NAMES = frozenset({"sh", "bash", "dash", "ksh"})
SHELL_GUARD = "tools/bootstrap"  # the discovery must always match this file


ENV_FLAGS = frozenset("0iv")  # macOS env(1): [-0iv] [-C altwd] [-P altpath] [-S string] [-u name]
ENV_OPERANDS = frozenset("CPSu")


def env_utility(words: Iterable[str]) -> str | None:
    """The utility `env` would run, given the words after `env` on a shebang line; ValueError if the
    options cannot be read (the caller fails closed rather than skip the file).

    Follows macOS env: bundled flags (-i -v -0, and a lone `-`), operands of -C, -P and -u (given
    attached or as the next word), -S splitting its string into further words, `--`, and NAME=VALUE
    assignments before the utility."""
    words, options = list(words), True
    while words:
        word = words.pop(0)
        if options and word == "--":
            options = False
        elif options and word == "-":
            continue  # a lone dash is -i
        elif options and word.startswith("-"):
            for position, letter in enumerate(word[1:], start=1):
                if letter in ENV_FLAGS:
                    continue
                if letter not in ENV_OPERANDS:
                    raise ValueError(f"unrecognised env option {word!r}")
                operand = word[position + 1 :] or (words.pop(0) if words else None)
                if operand is None:
                    raise ValueError(f"env option -{letter} has no operand")
                if letter == "S":
                    if any(character in operand for character in "$\\'\""):
                        raise ValueError(f"cannot read the -S string {operand!r}")
                    words[0:0] = operand.split()  # -S: the string is split into further words
                break
        elif "=" in word:
            continue  # NAME=VALUE: an assignment, not the utility
        else:
            return word
    return None


def shell_interpreter(first_line: str) -> str | None:
    """The shell a `#!` line names directly or through env, else None; ValueError if it cannot be read."""
    if not first_line.startswith("#!"):
        return None
    words = first_line[2:].split()
    if not words:
        return None
    command: str | None = words[0]
    if Path(words[0]).name == "env":
        command = env_utility(words[1:])
    name = Path(command).name if command else ""
    return name if name in SHELL_NAMES else None


def shell_sources(root: Path, tracked: Iterable[str]) -> list[str]:
    """The tracked files that are shell: .sh/.bash, or a first line naming sh, bash, dash or ksh."""
    excluded, found = SHELL_EXCLUDED, []
    for name in sorted(tracked):
        path = root / name
        if name.startswith(excluded) or path.is_symlink() or not path.is_file():
            continue
        with path.open("rb") as stream:
            first = stream.readline(512).decode("utf-8", "replace").rstrip("\r\n")
        try:
            interpreter = shell_interpreter(first)
        except (
            ValueError
        ) as error:  # fail closed: a file ShellCheck might have to lint is never skipped
            raise RuntimeError(
                f"{name}: cannot tell whether the shebang {first!r} names a shell ({error})"
            ) from error
        if path.suffix in {".sh", ".bash"} or interpreter:
            found.append(name)
    return found


def shellcheck_commands() -> list[list[str]]:
    """One ShellCheck run over the discovered sources; the coverage guard fails if the bootstrap is missed."""
    root = _state.root if _state else ROOT
    shellcheck = hub_tool("shellcheck")
    if not Path(shellcheck).exists():
        raise RuntimeError(
            f"ShellCheck is not installed ({shellcheck}); install it with: {INSTALL}"
        )
    tracked = capture(["pijul", "list"]).splitlines()
    sources = shell_sources(root, tracked)
    if SHELL_GUARD not in sources:
        raise RuntimeError(
            f"ShellCheck discovery did not match {SHELL_GUARD} (matched: {sources}); "
            "the discovery rules or exclusions are wrong"
        )
    return [[shellcheck, "--severity=style", *sources]]


def ruff_commands() -> list[list[str]]:
    ruff = str(Path(os.environ["CK_HUB"]) / ".tools/python/bin/ruff")
    # Bare commands: file discovery (including the extensionless entry points) comes from pyproject.toml.
    return [[ruff, "check"], [ruff, "format", "--check"]]


QUICK, FAST, FULL = ("quick", "fast", "full"), ("fast", "full"), ("full",)
CATEGORIES = ("floor", "affected", "optional")
NEEDS = (
    "plain",
    "gpu",
    "window",
)  # a group's commands need nothing special, the GPU, or a real window
CARGO_FLAGS = ["--all-features", "--locked"]


# The scale acceptance builds as its recipe fixes it: release with the Metal feature, locked. It does not take
# `CARGO_FLAGS`, whose `--all-features` also compiles in the test-only seams and is not the measured build.
SCALE_CARGO_FLAGS = ["--locked", "--features", "metal"]


def scale_test(package: str, target: str) -> list[str]:
    """The scale harness `target` of `package` in release, as the recipe builds it."""
    return ["cargo", "test", "--release", "-p", package, "--test", target, *SCALE_CARGO_FLAGS]


def cargo_test(selection: Sequence[str], after: Sequence[str] = ()) -> list[str]:
    """`cargo test` over the units `selection` names (`-p`, `--lib`, `--bin`, `--test`, `--doc`), with
    every feature, locked; `after` goes to the test runners (a split runner's `--set`)."""
    return ["cargo", "test", *selection, *CARGO_FLAGS, *(["--", *after] if after else [])]


# The Python test suite runs as several bounded `unittest-<group>` steps instead of one long step, so
# the step watchdog stays at least 10x the slowest step without raising it again (CK-36). Each
# module is loaded by its own `unittest discover -s tests -p <module>.py` exactly as the single
# `discover` did (same working directory, environment and loader: the test modules import their
# siblings by bare name, which a plain `python -m unittest <module>` could not resolve); a group
# runs its modules one after another. The groups were sized from the per-module times of a quiet
# full run (every group well under 120 s, 1/20 of the watchdog); `test_check` carries most of the
# `check` group because `RealToolchain` builds a copy of the workspace, so that group grows with the
# workspace's dependencies, and the groups are regrouped when one nears the target. The guard below
# makes a module in no group, in two groups, or missing a failure before any step runs.
UNITTEST_GROUPS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "batch",
        (
            "test_batch",
            "test_gate",
            "test_gate_criteria",
            "test_hashing",
            "test_lock_holders",
        ),
    ),
    # CK-84: the catalog tests read Rust sources, native files and manifests of the crates, so they are
    # selected by those inputs (UNITTEST_PATHS) and sit apart from the batch tests, which are not.
    ("catalog", ("test_check_catalog",)),
    (
        "check",
        (
            "test_check",
            "test_lint_policy",
            "test_repo_docs_isolation",
            "test_rgignore",
            "test_procscan",
        ),
    ),
    ("lints", ("test_rust_lints",)),
    (
        "bootstrap",
        (
            "test_bootstrap",
            "test_pipeline",
            "test_owner_signoff",
            "test_chat",
            "test_chat_ci",
            "test_chat_records",
            "test_chatfix",
            "test_check_tools",
        ),
    ),
    (
        "lease",
        (
            "test_lease",
            "test_gc",
            "test_records",
            "test_pipeline_api",
            "test_rollout",
            "test_cli_retired",
            "test_guide",
        ),
    ),
)


def unittest_step(group: str, modules: Sequence[str]) -> Step:
    """The step `unittest-<group>`: one `discover` per module, in the group's order."""
    return Step(
        f"unittest-{group}",
        FAST,
        lambda: [
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
        description=UNITTEST_DESCRIPTIONS.get(group, f"Python tests of the {group} group."),
        paths=UNITTEST_PATHS.get(group, PYTHON_PATHS),
    )


# What each Python test group exercises. The groups run no Rust unit that opens a window or takes the GPU:
# `test_check`'s real-toolchain lease test runs a planned recipe over `rust.core` (REAL_TOOLCHAIN_GROUPS).
UNITTEST_DESCRIPTIONS = {
    "batch": "Python tests of the batch gate, the gate criteria, hashing and the lock holders.",
    "catalog": "Python tests of the check catalog: group ownership of every test unit, the planner, and the "
    "source scans of the crates (GLFW sources, the native and Vulkan test files, the manifests).",
    "check": "Python tests of the check runner, its lint policy, the docs isolation and the process scan.",
    "lints": "The workspace lint configuration and the Rust suppression policy.",
    "bootstrap": "Python tests of the bootstrap, the pipeline, the owner sign-off, the chat and the tools.",
    "lease": "Python tests of the checkout lease, gc, the records, the pipeline API, roll-outs and the guide.",
}
PYTHON_PATHS = (
    "tools/",
    "tests/",
    "skills/",
    "stubs/",
    "pijul.toml",
    "pyproject.toml",
    "AGENTS.md",
    "CLAUDE.md",
)
TOOLCHAIN_PATHS = ("Cargo.toml", "Cargo.lock", "clippy.toml", "rust-toolchain.toml", ".cargo/")
RUST_SOURCE_DIRS = (
    "apps/codekarl/",
    "crates/gpu-metal/",
    "crates/gpu-vulkan/",
    "crates/input/",
    "crates/log/",
    "crates/lua/",
    "crates/render-2d/",
    "crates/resources/",
    "crates/runtime/",
    "crates/window/",
    "crates/window-cocoa/",
    "crates/window-glfw/",
)
# CK-84: the files the catalog tests read besides the Python ones (see `tests/test_check_catalog.py`, GUARDS):
# every Rust source, test and build script of the crates (RUST_SOURCE_DIRS, by directory, which is also what
# the inventory scan reads and where `cargo metadata` derives the test units from), the toolchain files, and
# the whole GLFW crate (every file there is scanned for dynamic-loader variables).
CATALOG_PATHS = (*PYTHON_PATHS, *TOOLCHAIN_PATHS, *RUST_SOURCE_DIRS)
UNITTEST_PATHS = {
    "batch": PYTHON_PATHS,
    "catalog": CATALOG_PATHS,
    # `.rgignore` and `.ignore` are read by `test_rgignore` and copied into the real-toolchain workload;
    # `test_rgignore` also runs ripgrep over the real `docs/` tree. Registering it selects the test when a
    # code change is checked; documentation publication stays ungated (the docs lane runs no check).
    "check": (*PYTHON_PATHS, *TOOLCHAIN_PATHS, ".rgignore", ".ignore", "docs/"),
    # The Rust sources it scans, by directory: a new crate is unclaimed until a group registers it.
    "lints": (*RUST_SOURCE_DIRS, "tests/test_rust_lints.py", *TOOLCHAIN_PATHS),
    # `AnalyzerBuild` builds a copy of the workspace's `ck-gpu-metal` offline with the analyzer on, and edits
    # its real bridge source: the crate's files and the toolchain files it builds with are its inputs.
    "bootstrap": (*PYTHON_PATHS, "crates/gpu-metal/", *TOOLCHAIN_PATHS),
    "lease": PYTHON_PATHS,
}
# The groups the real-toolchain lease test runs (`check.py --group ...`): focus-free, real Cargo work.
REAL_TOOLCHAIN_GROUPS = ("rust.core",)


def discoverable_modules(tests: Path) -> tuple[list[str], list[str]]:
    """The test modules `unittest discover -s tests` loads (default pattern `test*.py`), by name,
    and the packages below `tests` it would also descend into (which the groups cannot name)."""
    modules = sorted(path.stem for path in tests.glob("test*.py") if path.is_file())
    packages = sorted(
        str(path.parent.relative_to(tests)) for path in tests.rglob("__init__.py") if path.is_file()
    )
    return modules, packages


def verify_groups(tests: Path, groups: Sequence[tuple[str, Sequence[str]]] | None = None) -> None:
    """Fail, naming every offending module, unless the groups cover the tests directory exactly."""
    problems = group_problems(tests, groups) if tests.is_dir() else []
    if problems:
        raise RuntimeError("; ".join(problems))


def guard_groups(level: str, steps: Sequence[Step] | None) -> None:
    """The real recipe, fast or full, in a run on a copy: its tests directory must be covered exactly."""
    if steps is None and level in ("fast", "full") and _state is not None:
        verify_catalog()
        verify_groups(_state.root / "tests")


def group_problems(
    tests: Path, groups: Sequence[tuple[str, Sequence[str]]] | None = None
) -> list[str]:
    """What is wrong with the groups for this tests directory, one message per problem: a module in
    no group, a module in two groups, a group naming a module that does not exist, or a package of
    tests the groups cannot select."""
    groups = UNITTEST_GROUPS if groups is None else groups
    modules, packages = discoverable_modules(tests)
    placed: dict[str, list[str]] = {}
    for group, names in groups:
        for name in names:
            placed.setdefault(name, []).append(group)
    problems = [
        f"test module {name} is in no unittest group (add it to UNITTEST_GROUPS in tools/check.py)"
        for name in modules
        if name not in placed
    ]
    problems += [
        f"test module {name} is in more than one unittest group: {', '.join(found)}"
        for name, found in placed.items()
        if len(found) > 1
    ]
    problems += [
        f"unittest group {', '.join(found)} names {name}, which is not a test module in tests/"
        for name, found in placed.items()
        if name not in modules
    ]
    problems += [
        f"tests/{package} is a package: its modules cannot be selected by the unittest groups"
        for package in packages
    ]
    return problems


# The catalog: every check group, in run order. Floor groups always run; affected groups run when a changed
# path matches their patterns (or when the plan fails wide); optional groups only when named. A group's
# `needs` covers every command it starts, nested recipes included. `doc` and `rust.*`/`metal.gpu`/
# `native.window` and `metal.window-*` set `analyze` (they share the build-script unit); Clippy does not.
# CK-121: `ck-window` depends on `ck-input` (the loop's key mapper), so the crates that build on the window core
# are rebuilt and retested when the mapper changes: `crates/input/` is one of them.
NEUTRAL_CRATES = (
    "crates/input/",
    "crates/log/",
    "crates/lua/",
    "crates/runtime/",
    "crates/window/",
)
# CK-131: the application's windowed tests run the program in process behind a test-only keyboard filter, so the
# script that builds the filter and its native source exist for them, like `ck-render-2d`'s (below).
NATIVE_APP_FILES = (
    "apps/codekarl/src/native.rs",
    "apps/codekarl/tests/windowed.rs",
    "apps/codekarl/build.rs",
    "apps/codekarl/tests/native/",
    "crates/window/tests/support/activation_watch.rs",
)
# CK-92: the files of `ck-render-2d` that only the real-window group exercises: its Metal backend and the test
# that drives it on a real window. The rest of the crate selects `render2d.core`, never a window group.
RENDER2D_WINDOW_FILES = (
    "crates/render-2d/src/metal.rs",
    "crates/render-2d/tests/metal_window.rs",
    # The test-only keyboard filter that test installs, and the script that builds it.
    "crates/render-2d/build.rs",
    "crates/render-2d/tests/native/testing.m",
    "crates/window/tests/support/activation_watch.rs",
)
# CK-111: classify every current Metal crate file by whether editing it changes code the windowed presenter
# cases execute. The presenter/quad implementation, quad shader, presenter native pair, unwind cleanup
# helper, presenter runner/cases, and test-only native keyboard support are window inputs. `src/lib.rs`
# holds the probe, module list, shared re-exports and Error/message parser used by both presenter sets;
# `build.rs` compiles both production sources with one shared flag vector; `Cargo.toml` defines their shared
# target/features. Those three are excluded because compile/link/export/flag failures are exercised by
# `metal.gpu`, including the offscreen presenter's GPU set. A shared-flag change with only an on-screen effect
# must be reported to ck-manager before checks run. The bridge header/implementation, probe shader, GPU probe,
# shader debug and hook isolation tests are probe/offscreen inputs. `build.rs` is a window input because it
# compiles the presenter's native test support, which includes the shared activation watch. Future offscreen
# texture modules/native sources/tests remain GPU-only; add future window-only files to this list. The batch
# still runs the complete window group once.
METAL_WINDOW_FILES = (
    # The presenter implementation and the quad slot drawn by its real-window frames.
    "crates/gpu-metal/src/presenter.rs",
    "crates/gpu-metal/src/quad.rs",
    # Conservative: the presenter disposes wake and join payloads through this shared helper.
    "crates/gpu-metal/src/unwind.rs",
    # The presenter's native drawable and presentation interface and implementation.
    "crates/gpu-metal/native/presenter.h",
    "crates/gpu-metal/native/presenter.m",
    # The shader used by the quad rendered in the windowed presenter cases.
    "crates/gpu-metal/shaders/quad.metal",
    # The presenter runner and its windowed quad and scene cases.
    "crates/gpu-metal/tests/presenter.rs",
    "crates/gpu-metal/tests/presenter_quad/",
    "crates/gpu-metal/tests/presenter_scene/",
    # CK-129: the frame-timing window cases (real windows: the marks, the measurement windows and their delayed results).
    "crates/gpu-metal/tests/presenter_metrics/",
    # Test-only keyboard support installed by the windowed cases.
    "crates/gpu-metal/tests/native/",
    "crates/gpu-metal/build.rs",
    "crates/window-cocoa/tests/native/activation_watch.inc",
    "crates/window/tests/support/activation_watch.rs",
)
# CK-144: these presenter modules contain only off-screen `readback*` cases, so `metal.gpu` covers them without
# opening a window. Move a module directory to `METAL_WINDOW_FILES` if it gains a window case.
METAL_PRESENTER_GPU_ONLY = (
    "crates/gpu-metal/tests/presenter_instanced/",
    "crates/gpu-metal/tests/presenter_bindless/",
)
# CK-142: the paths of the native window groups. `native.window` (the Cocoa backend, the application's real-window tests and
# `ck-render-2d`'s Metal backend) and the presenter's four window-case groups (`metal.window-core`, `-scene`, `-compat`, `-instanced`) take
# **one shared tuple**: `gpu-metal` depends on `window-cocoa`, and `render-2d` and the application depend on `gpu-metal`, so the
# window inputs overlap, and a planned check selects all five groups or none, exactly where it selected the one `native.window`
# before the split. No path gains or loses a window run. Target: every window group under 135 s on a batch run (1/20 of the
# watchdog, half the single-submission gate's limit, like the `unittest-*` groups); they are regrouped when one nears it.
NATIVE_WINDOW_PATHS = (
    "crates/window-cocoa/",
    "crates/window-cocoa/tests/native/activation_watch.inc",
    "crates/window/tests/support/activation_watch.rs",
    *METAL_WINDOW_FILES,
    *NATIVE_APP_FILES,
    *RENDER2D_WINDOW_FILES,
)
# CK-80: the `glfw-vulkan` path's own files in the application: the generic run closure and its real stack,
# the binary tests that open real GLFW windows, and the demonstration configurations. Only these select
# `vulkan.app`; the rest of `apps/codekarl/` selects the application's other groups, never this one.
# CK-99: the quad demonstration, and the application's manifest, which carries the `vulkan` feature's wiring
# (`ck-render-2d/vulkan` among it): a change to it alone selects this group. It still selects neither
# `metal.gpu` nor `native.window`, which claim other paths.
# CK-159: the application's windowed Vulkan tests run in process behind the test-only keyboard filter, so the build script and the
# native test support are theirs too (they stay in `NATIVE_APP_FILES` as well: the native windowed tests use them, so a change
# to them also selects the native window groups). The common demonstration they run on `glfw-vulkan` (its configuration, scripts
# and assets) is read too but selects nothing here: the demonstration's files never open a window by themselves.
VULKAN_APP_FILES = (
    "apps/codekarl/Cargo.toml",
    "apps/codekarl/build.rs",
    "apps/codekarl/src/vulkan.rs",
    "apps/codekarl/src/vulkan/",
    "apps/codekarl/tests/native/",
    "apps/codekarl/tests/vulkan_app.rs",
    "crates/window-cocoa/tests/native/activation_watch.inc",
    "crates/window/tests/support/activation_watch.rs",
    "config/demo-glfw-vulkan.toml",
    "config/demo-glfw-vulkan-decline.toml",
    "config/demo-glfw-vulkan-callback-error.toml",
    "config/demo-quad-vulkan.toml",
)
# CK-84: what the static guards and the test targets of other crates read. The hook-isolation guards of the
# two GPU crates scan the application's Rust sources and tests; the supervisor module of the application's
# tests is compiled into the GLFW and Vulkan test targets by path; the Vulkan window tests compile and link
# `ck-window-glfw`'s native test support and include its Rust test support. Only GLFW and Vulkan groups are
# selected by these (never `metal.gpu` or `native.window`).
APP_SOURCE_DIRS = ("apps/codekarl/src/", "apps/codekarl/tests/")
SUPERVISOR_DIR = "apps/codekarl/tests/supervisor/"
GLFW_TEST_SUPPORT = (
    "crates/window-glfw/tests/native/",
    "crates/window-glfw/tests/support/",
    "crates/window-cocoa/tests/native/activation_watch.inc",
    "crates/window/tests/support/activation_watch.rs",
    "crates/window-glfw/build.rs",
)
STEPS = [
    Step(
        "fmt",
        QUICK,
        lambda: [["cargo", "fmt", "--all", "--", "--check"]],
        description="rustfmt over the workspace.",
        category="floor",
    ),
    Step(
        "ruff",
        QUICK,
        ruff_commands,
        description="Ruff lint and format check over the Python tools and tests.",
        category="floor",
    ),
    Step(
        "shellcheck",
        QUICK,
        shellcheck_commands,
        description="ShellCheck over the tracked shell sources.",
        category="floor",
    ),
    Step(
        "doctor",
        FAST,
        action=lambda: doctor(graphics=False),
        description="The toolchain and environment doctor. Claims the documentation, licence and editor files, "
        "which have no test group of their own.",
        category="floor",
        paths=("docs/", "README.md", "LICENSE", ".gitignore", ".ignore", ".rgignore", ".vscode/"),
    ),
    Step(
        "doctor.graphics",
        FAST,
        action=lambda: graphics_doctor(),
        description="GLFW 3.4 or newer 3.x, the Vulkan loader and the Khronos validation layer are installed "
        "(a missing piece fails, naming the install command).",
        paths=("crates/window-glfw/", "crates/gpu-vulkan/", "tools/bootstrap", *VULKAN_APP_FILES),
    ),
    Step(
        "clippy",
        FAST,
        lambda: [
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
        description="Clippy over every workspace target (compiles all tests, runs none).",
        category="floor",
    ),
    Step(
        "doc",
        FAST,
        lambda: [["cargo", "doc", "--workspace", "--no-deps", "--all-features", "--locked"]],
        analyze=True,
        env=(("RUSTDOCFLAGS", "-D warnings"),),  # includes rustdoc::broken_intra_doc_links
        description="rustdoc over the workspace with warnings as errors (compiles, runs no test).",
        category="floor",
    ),
    Step(
        "rust.core",
        FAST,
        lambda: [
            cargo_test(["-p", "ck-log", "-p", "ck-lua", "-p", "ck-runtime", "-p", "ck-window"])
        ],
        full_spec=True,
        analyze=True,
        description="Every Rust test of the backend-neutral crates (log, lua, runtime, window with its fakes).",
        paths=(*NEUTRAL_CRATES, *TOOLCHAIN_PATHS),
    ),
    Step(
        "rust.app",
        FAST,
        lambda: [
            cargo_test(
                [
                    "-p",
                    "codekarl",
                    # CK-131: the program is the library (its unit tests); the binary is a thin `main` with none.
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
                ]
            ),
            cargo_test(["-p", "codekarl", "--test", "cli_spec"], ["--set", "plain"]),
            # Cargo refuses `--doc` beside other target selectors (CK-131: the library has doctests to run).
            cargo_test(["-p", "codekarl", "--doc"]),
        ],
        full_spec=True,
        analyze=True,
        description="The codekarl application's tests without a real window or the GPU, with every feature "
        "(unit and boundary tests over fakes, the CLI spec's plain cases, Lua VMs, modes, signals, runtime).",
        paths=(
            "apps/codekarl/",
            "config/",
            "scripts/",
            "assets/",
            "specs/",
            *NEUTRAL_CRATES,
            *TOOLCHAIN_PATHS,
        ),
    ),
    # CK-44: the application without its `metal` feature. The CLI spec runner's normal mode shows the
    # skipped Metal case without failing it, so this step must not see CK_REQUIRE_FULL_SPEC, not even
    # one the caller exported (`State.environment` copies the parent environment).
    Step(
        "rust.app-default",
        FAST,
        lambda: [
            ["cargo", "test", "-p", "codekarl", "--locked"],
            # CK-162: the application with the `vulkan` feature and no `metal` feature must compile everywhere its
            # sources are (a Linux-only break shows here, on a Mac, as a build error); every target is compiled, none run.
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
        env_remove=("CK_REQUIRE_FULL_SPEC",),
        description="The codekarl application's tests without its `metal` feature (the Metal CLI case is "
        "shown skipped); its real-window test target is empty without the feature; and a build of every "
        "target with the `vulkan` feature alone (CK-162).",
        paths=(
            "apps/codekarl/",
            "config/",
            "scripts/",
            "assets/",
            "specs/",
            *NEUTRAL_CRATES,
            *TOOLCHAIN_PATHS,
        ),
    ),
    # CK-84: the Metal presenter's static guard. It reads files and opens no window and uses no GPU, so it is
    # its own group, selected by what it reads (the crate and the application's sources), and `metal.gpu`
    # no longer runs it.
    Step(
        "metal.isolation",
        FAST,
        lambda: [cargo_test(["-p", "ck-gpu-metal", "--test", "hook_isolation"])],
        analyze=True,
        description="The Metal presenter's hook isolation: nothing in codekarl's sources names the hidden "
        "constructor or the hooks type, and the native sources hold no volatile access (reads files only).",
        paths=("crates/gpu-metal/", *APP_SOURCE_DIRS),
    ),
    Step(
        "metal.gpu",
        FAST,
        lambda: [
            cargo_test(
                [
                    "-p",
                    "ck-gpu-metal",
                    "--lib",
                    "--test",
                    "gpu_probe",
                    "--test",
                    "textures",
                    "--test",
                    "shader_debug",
                ]
            ),
            cargo_test(["-p", "ck-gpu-metal", "--doc"]),
            cargo_test(["-p", "ck-gpu-metal", "--test", "presenter"], ["--set", "gpu"]),
            cargo_test(["-p", "codekarl", "--test", "cli_spec"], ["--set", "gpu"]),
        ],
        full_spec=True,
        analyze=True,
        needs="gpu",
        description="The Metal tests that need the GPU but open no window: the GPU probe, the readback and "
        "the presenter supervisor's own checks, the library tests, and the `--metal-smoke` CLI case "
        "(the hook isolation is `metal.isolation`).",
        paths=(
            "crates/gpu-metal/",
            "crates/window-cocoa/tests/native/activation_watch.inc",
            "crates/window/tests/support/activation_watch.rs",
            "apps/codekarl/src/native.rs",
            "specs/bootstrap.json",
        ),
    ),
    Step(
        "native.window",
        FAST,
        lambda: [
            cargo_test(["-p", "ck-window-cocoa"]),
            cargo_test(["-p", "codekarl", "--test", "windowed"]),
            cargo_test(["-p", "ck-render-2d", "--test", "metal_window"]),
        ],
        full_spec=True,
        analyze=True,
        needs="window",
        description="The native tests that open real windows without taking the desktop's focus, except for explicitly named focus cases: the Cocoa "
        "backend, the application's real-window tests and ck-render-2d's Metal backend on a real "
        "window (the Metal presenter's windowed cases are the `metal.window-*` groups).",
        paths=NATIVE_WINDOW_PATHS,
    ),
    # CK-142: the Metal presenter's windowed cases, split by the module they are listed in so that no window group nears the
    # batch timing gate (the single `native.window` took 289 s against the 270 s gate). Each runs one of the presenter's window sets
    # (`--set window-core|window-scene|window-compat|window-instanced`); the checker control runs in every one.
    Step(
        "metal.window-core",
        FAST,
        lambda: [
            cargo_test(["-p", "ck-gpu-metal", "--test", "presenter"], ["--set", "window-core"]),
        ],
        full_spec=True,
        analyze=True,
        needs="window",
        description="The Metal presenter's windowed cases of `presenter.rs` and `presenter_quad/` (real windows, "
        "under Metal validation and the Main Thread Checker).",
        paths=NATIVE_WINDOW_PATHS,
    ),
    Step(
        "metal.window-scene",
        FAST,
        lambda: [
            cargo_test(["-p", "ck-gpu-metal", "--test", "presenter"], ["--set", "window-scene"]),
        ],
        full_spec=True,
        analyze=True,
        needs="window",
        description="The Metal presenter's windowed scene cases of `presenter_scene/cases.rs` (the "
        "texture driver, tickets, proof and rollback).",
        paths=NATIVE_WINDOW_PATHS,
    ),
    # CK-145: `presenter_scene/compat.rs` split out of `metal.window-scene` (128.2 s against the 135 s target) as CK-142 split
    # the presenter's cases: by module, on the same shared path tuple.
    Step(
        "metal.window-compat",
        FAST,
        lambda: [
            cargo_test(["-p", "ck-gpu-metal", "--test", "presenter"], ["--set", "window-compat"]),
        ],
        full_spec=True,
        analyze=True,
        needs="window",
        description="The Metal presenter's windowed compatibility-quad cases of `presenter_scene/compat.rs` "
        "(the first-frame gate, the held replacement image, and their bindless forms).",
        paths=NATIVE_WINDOW_PATHS,
    ),
    Step(
        "metal.window-instanced",
        FAST,
        lambda: [
            cargo_test(
                ["-p", "ck-gpu-metal", "--test", "presenter"], ["--set", "window-instanced"]
            ),
        ],
        full_spec=True,
        analyze=True,
        needs="window",
        description="The Metal presenter's windowed instanced-scene cases of `presenter_scene/instanced.rs` "
        "(the checker's start-up contract, batching, the instance buffer and the bindless frames).",
        paths=NATIVE_WINDOW_PATHS,
    ),
    # CK-130: the milestone 5 scale acceptance (`docs/scene-design.md` §11.3), an OPTIONAL group: it opens a 1920 x 1080 window and holds the
    # GPU for minutes on the owner's Mac, so it runs only when named (`--group metal.scale`), only when the owner has scheduled it, and never
    # in a batch or at any level (`levels` is empty). Its two harnesses run their phases only because the group sets `CK_SCALE_RUN=1`; run
    # any other way they report that they did not run. They run in release, as the recipe says.
    Step(
        "metal.scale",
        (),
        lambda: [
            scale_test("ck-render-2d", "scale"),
            scale_test("codekarl", "scale_lua"),
        ],
        env=(("CK_SCALE_RUN", "1"),),
        needs="window",
        category="optional",
        description="The milestone 5 scale acceptance, owner-scheduled and never run by a batch: the frozen recipe "
        "(32,768 loaded textures, 65,536 quads, 1920 x 1080 at 60 Hz, CPU and GPU frame p95 under 8 ms) in "
        "`ck-render-2d`'s harness, and the all-quads-moving Lua profile through the application (measured, not gated).",
        paths=(
            "crates/render-2d/tests/scale.rs",
            "crates/render-2d/tests/scale/",
            "apps/codekarl/tests/scale_lua.rs",
            "config/scale-lua.toml",
            "scripts/scale/",
        ),
    ),
    Step(
        "input.core",
        FAST,
        lambda: [
            cargo_test(["-p", "ck-input", "--lib"]),
            cargo_test(["-p", "ck-input", "--doc"]),
        ],
        analyze=True,
        description="The portable physical-key names, binding validation and bounded pure action mapper (no native input, GPU or window).",
        paths=("crates/input/", *TOOLCHAIN_PATHS),
    ),
    # CK-90: the 2D renderer's core (images, the scene bridge and the `Renderer` over a fake backend). It has no
    # GPU feature and opens no window, so every test target of the crate is one plain group, selected by the
    # crate and by what it builds on (the window crate's `Presenter` and the log crate).
    Step(
        "render2d.core",
        FAST,
        lambda: [
            cargo_test(["-p", "ck-render-2d", "--lib"]),
            # CK-130: the scale acceptance's pure layer (the frozen scene's geometry, the identity code and the fixtures' bytes).
            cargo_test(["-p", "ck-render-2d", "--test", "scale_pure"]),
            # Cargo refuses `--doc` beside other target selectors.
            cargo_test(["-p", "ck-render-2d", "--doc"]),
        ],
        analyze=True,
        description="Every Rust test of ck-render-2d that needs no window: points to pixels, the scene bridge, "
        "the renderer over a fake backend, the clear-only backend, the re-exports from ck-resources, the Metal "
        "backend's sink conversion, the Vulkan sink side's scene conversion and refusal, and the scale "
        "acceptance's pure geometry and fixtures (no GPU work, no window; the Metal window cases are the "
        "`metal.window-*` groups).",
        paths=(
            "crates/render-2d/",
            "crates/resources/",
            "crates/window/",
            "crates/input/",
            "crates/log/",
            "crates/gpu-metal/",
            # CK-98: the Vulkan sink converts into the GPU crate's plain types, so a change to that crate reruns
            # the conversion tests (and, as for the Metal crate above, still never selects a window group).
            "crates/gpu-vulkan/",
            "crates/window-cocoa/",
            *TOOLCHAIN_PATHS,
        ),
    ),
    # CK-105: the resource component (`docs/scene-design.md` §3): PNG decoding under budgets, the assets rule. It is
    # CPU-only with no dependency on a `ck-*` crate, so it is its own plain group, selected by the crate alone (and
    # the toolchain); `render2d.core` lists the crate too, because it builds on it.
    Step(
        "resources.core",
        FAST,
        lambda: [
            cargo_test(
                ["-p", "ck-resources", "--lib", "--test", "decoder", "--test", "allocation"]
            ),
            # Cargo refuses `--doc` beside other target selectors.
            cargo_test(["-p", "ck-resources", "--doc"]),
        ],
        analyze=True,
        description="Every Rust test of ck-resources: PNG decoding and its budgets, the decoder's allocation "
        "envelope and the assets rule (no GPU, no window).",
        paths=(
            "crates/resources/",
            *TOOLCHAIN_PATHS,
        ),
    ),
    # CK-78: the GLFW window backend. Its window tests avoid desktop focus except for the explicit focus cases, so they are their own
    # group, selected only by this crate's paths (never by `crates/window/` or another backend's paths).
    Step(
        "glfw.core",
        FAST,
        lambda: [
            cargo_test(["-p", "ck-window-glfw", "--lib"]),
            cargo_test(["-p", "ck-window-glfw", "--doc"]),
        ],
        analyze=True,
        description="The GLFW backend's tests that open no window: the pending-state logic and the docs.",
        paths=("crates/window-glfw/", "crates/window/", "crates/input/", *TOOLCHAIN_PATHS),
    ),
    Step(
        "glfw.window",
        FAST,
        lambda: [cargo_test(["-p", "ck-window-glfw", "--test", "window_glfw"])],
        analyze=True,
        needs="window",
        description="The GLFW backend's window tests: real windows through GLFW and the Vulkan loader, without taking desktop focus "
        "except in the explicitly named focus cases.",
        paths=("crates/window-glfw/", SUPERVISOR_DIR, *GLFW_TEST_SUPPORT),
    ),
    # CK-79: the Vulkan presenter. Two groups, each selected only by this crate's own paths: the one without
    # a window needs the GPU (the ledger's logic, the instance, the loader and the off-screen readback); the
    # one with real GLFW windows avoids desktop focus except in explicitly named focus cases. Neither selects `native.window` or `metal.gpu`,
    # and `crates/window-glfw/` selects neither of them.
    # CK-84: the Vulkan presenter's static guard, as `metal.isolation` is the Metal one.
    Step(
        "vulkan.isolation",
        FAST,
        lambda: [cargo_test(["-p", "ck-gpu-vulkan", "--test", "hook_isolation"])],
        analyze=True,
        description="The Vulkan presenter's hook isolation: no test-only name of the library in codekarl's "
        "sources, the library's dependency boundary and its source rules (reads files only).",
        paths=("crates/gpu-vulkan/", *APP_SOURCE_DIRS),
    ),
    Step(
        "vulkan.gpu",
        FAST,
        lambda: [
            cargo_test(["-p", "ck-gpu-vulkan", "--lib"]),
            cargo_test(["-p", "ck-gpu-vulkan", "--doc"]),
            cargo_test(["-p", "ck-gpu-vulkan", "--test", "gpu"]),
        ],
        analyze=True,
        needs="gpu",
        description="The Vulkan presenter's tests that open no window: the ledger's logic, the loader, the "
        "instance, the off-screen readback under validation and the loader's lifetime (the hook isolation "
        "is `vulkan.isolation`).",
        paths=("crates/gpu-vulkan/", SUPERVISOR_DIR),
    ),
    Step(
        "vulkan.window",
        FAST,
        lambda: [cargo_test(["-p", "ck-gpu-vulkan", "--test", "window"])],
        analyze=True,
        needs="window",
        description="The Vulkan presenter's window tests: real GLFW windows, real presents under validation "
        "and every ledger transition, without taking desktop focus.",
        paths=("crates/gpu-vulkan/", SUPERVISOR_DIR, *GLFW_TEST_SUPPORT),
    ),
    # CK-80: the application's `glfw-vulkan` path end to end. Its binary tests open real GLFW windows without
    # taking desktop focus except in explicitly named focus cases, so they are their own group. The native
    # reverse-override case (`--backend native-metal` over a `glfw-vulkan` configuration) opens the native
    # backend and lives in `windowed.rs`, so `native.window` runs it and this group never does.
    # CK-159 (M5.5-4, part A): the resource path on a real Vulkan window, `ck-render-2d`'s counterpart of its Metal window test
    # (CK-117): a supervisor and a worker over `ck-window-glfw`'s window with the surface's own pixels captured. It opens
    # windows, so it needs the window slot. `render2d.core`'s commands do not run this target.
    Step(
        "render2d.vulkan-window",
        FAST,
        lambda: [cargo_test(["-p", "ck-render-2d", "--test", "vulkan_window"])],
        analyze=True,
        needs="window",
        description="`ck-render-2d`'s Vulkan window test: the real resource path (service, resident driver, ledger, scene "
        "presenter) on a GLFW window under validation, with the surface's pixels captured, and the keyboard filter's isolation "
        "(real windows without taking the desktop's focus).",
        paths=(
            "crates/render-2d/tests/vulkan_window.rs",
            "crates/render-2d/src/vulkan.rs",
            "crates/render-2d/src/vulkan_resident.rs",
            "crates/render-2d/src/vulkan_scene.rs",
            "crates/render-2d/build.rs",
            SUPERVISOR_DIR,
            *GLFW_TEST_SUPPORT,
        ),
    ),
    Step(
        "vulkan.app",
        FAST,
        lambda: [cargo_test(["-p", "codekarl", "--test", "vulkan_app"])],
        analyze=True,
        needs="window",
        description="The application's glfw-vulkan path with a real GLFW window and Vulkan under validation, run in "
        "process behind the keyboard filter: close, signal, the backend override, the shipped demonstration "
        "configurations, the common demonstration, the texture lifecycle and the minimised window.",
        paths=VULKAN_APP_FILES,
    ),
    *(unittest_step(group, modules) for group, modules in UNITTEST_GROUPS),
    # S7: strict mypy over `tools` and `tests` (pyproject.toml); no module is exempt.
    # A failure shows mypy's output.
    Step(
        "mypy",
        FAST,
        lambda: [[hub_tool("mypy")]],
        description="Strict mypy over the tools and tests.",
        category="floor",
    ),
    # RC-1: the Codekarl chat suite runs against the shared chat code (a record only in a batch).
    Step(
        "chat-ci",
        FAST,
        lambda: [[sys.executable, "tools/chat/install.py", "ci"]],
        description="The Codekarl chat suite against the shared chat code.",
        paths=(
            "tools/chat/",
            "tools/ck-chat",
            "skills/ck-chat/",
            "stubs/",
            "tests/chatfix.py",
            "tests/test_chat.py",
            "tests/test_chat_ci.py",
            "tests/test_chat_records.py",
            "tests/test_chatfix.py",
        ),
    ),
]


def select(level: str, steps: Sequence[Step] | None = None) -> list[str]:
    """The names of the steps a level runs, in order. A planned level runs the steps it is given."""
    return [
        step.name
        for step in (STEPS if steps is None else steps)
        if level == "planned" or level in step.levels
    ]


# ---------------------------------------------------------------------------------------------------
# The catalog's rules and the planner (modelled on the owner's moskophoros validation)
# ---------------------------------------------------------------------------------------------------

GROUP_ID = re.compile(r"[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)*")


def pattern_problem(pattern: object) -> str | None:
    """Why `pattern` is not a valid path pattern, or None. A pattern ending in `/` names a directory and
    claims everything beneath it; any other names exactly one file. There are no wildcards."""
    if not isinstance(pattern, str) or not pattern:
        return "must be a non-empty string"
    if pattern != pattern.strip():
        return "has leading or trailing whitespace"
    if pattern.startswith("/"):
        return "must be relative to the repository root"
    if any(character in pattern for character in "\\*?["):
        return "must not contain a backslash or a wildcard (* ? [)"
    if any(segment in ("", ".", "..") for segment in pattern.removesuffix("/").split("/")):
        return "has an empty, `.` or `..` segment"
    return None


def claims(pattern: str, path: str) -> bool:
    """Whether `pattern` claims the repository-relative `path` (case-sensitive, `/` separators)."""
    return path.startswith(pattern) if pattern.endswith("/") else path == pattern


def is_directory_entry(entry: str) -> bool:
    """A changed-path entry that names a directory (an untracked one), not a file: it ends in `/`."""
    return entry.endswith("/")


def touches(pattern: str, entry: str) -> bool:
    """Whether a changed-path entry touches `pattern`'s paths. A file entry is claimed by the pattern. A
    directory entry (a new, untracked directory whose files are not listed) touches a pattern that lies
    inside it (`crates/` touches `crates/gpu-metal/`) and one that contains it (`crates/gpu-vulkan/`
    touches `crates/`), so a group is never missed because its files were not enumerated."""
    if not is_directory_entry(entry):
        return claims(pattern, entry)
    return pattern.startswith(entry) or (pattern.endswith("/") and entry.startswith(pattern))


def is_claimed(steps: Sequence[Step], entry: str) -> bool:
    """Whether some group claims the entry. A directory entry is claimed only when it lies wholly inside
    one group's directory pattern; otherwise it may hold files no group has agreed to cover."""
    patterns = [pattern for step in steps for pattern in step.paths]
    if not is_directory_entry(entry):
        return any(claims(pattern, entry) for pattern in patterns)
    return any(pattern.endswith("/") and entry.startswith(pattern) for pattern in patterns)


def catalog_problems(steps: Sequence[Step]) -> list[str]:
    """What is wrong with a catalog, one message per problem: an invalid or repeated id, an unknown
    category or need, a missing description, an affected group without paths, an invalid pattern."""
    problems: list[str] = []
    seen: set[str] = set()
    for step in steps:
        name = step.name
        if not GROUP_ID.fullmatch(name):
            problems.append(
                f"group {name!r}: the id must be lower-case words (letters, digits, -, .)"
            )
        if name in seen:
            problems.append(f"group {name}: the id is used twice")
        seen.add(name)
        if not step.description.strip():
            problems.append(f"group {name}: it has no description")
        if step.category not in CATEGORIES:
            problems.append(f"group {name}: category {step.category!r} is not one of {CATEGORIES}")
        if step.needs not in NEEDS:
            problems.append(f"group {name}: needs {step.needs!r} is not one of {NEEDS}")
        if step.category == "affected" and not step.paths:
            problems.append(f"group {name}: an affected group needs path patterns")
        problems += [
            f"group {name}: pattern {pattern!r} {why}"
            for pattern in step.paths
            if (why := pattern_problem(pattern))
        ]
    return problems


def verify_catalog(steps: Sequence[Step] | None = None) -> None:
    """Fail, naming every problem, unless the catalog is valid."""
    problems = catalog_problems(STEPS if steps is None else steps)
    if problems:
        raise RuntimeError("the check catalog is invalid: " + "; ".join(problems))


@dataclass(frozen=True)
class Choice:
    """One group of a plan: whether it runs, and why."""

    step: Step
    selected: bool
    why: str


@dataclass(frozen=True)
class Plan:
    """The groups a planned check runs and, for every group, the reason."""

    choices: tuple[Choice, ...]

    def steps(self) -> list[Step]:
        return [choice.step for choice in self.choices if choice.selected]

    def lines(self) -> list[str]:
        return [
            f"check: plan: {'run ' if choice.selected else 'skip'} {choice.step.name} "
            f"[{choice.step.category}]: {choice.why}"
            for choice in self.choices
        ]


def abbreviate(items: Sequence[str], limit: int) -> str:
    """The first `limit` items, comma-separated, and how many more there were."""
    more = len(items) - limit
    return ", ".join(items[:limit]) + (f" and {more} more" if more > 0 else "")


def matched(step: Step, changed: Sequence[str]) -> tuple[bool, str]:
    """Whether a changed path matches one of the group's patterns, and the reason either way."""
    hits = [(path, pattern) for path in changed for pattern in step.paths if touches(pattern, path)]
    if not hits:
        return False, "no changed path matches its patterns"
    path, pattern = hits[0]
    more = f" (and {len(hits) - 1} more)" if len(hits) > 1 else ""
    return True, f"changed path {path} matches {pattern}{more}"


def choose(
    step: Step, changed: Sequence[str] | None, requested: Sequence[str], wide: str
) -> Choice:
    """Whether one group runs in a plan, and why (`wide` is the fail-wide reason, empty when none)."""
    if step.category == "floor":
        selected, why = True, "floor: always runs"
    elif step.name in requested:
        selected, why = True, "requested by name"
    elif step.category == "optional":
        selected, why = False, "optional: runs only when requested by name"
    elif wide:
        selected, why = True, wide
    elif changed is None:
        selected, why = False, "no change set was asked for (--changed) and it was not requested"
    else:
        selected, why = matched(step, changed)
    return Choice(step, selected, why)


def plan(
    steps: Sequence[Step],
    changed: Sequence[str] | None,
    requested: Sequence[str] = (),
    failure: str | None = None,
) -> Plan:
    """Choose the groups of a planned check. Floor groups always run, optional ones only when named,
    affected ones when named or when a changed path matches. `changed` is None when no change set was
    asked for (only the named groups run); `failure` is why asking failed. A changed path that no group
    claims, and a failure to compute the change set, select every affected group (fail wide). An entry
    ending in `/` is an untracked directory whose files are not listed (see `touches` and `is_claimed`)."""
    names = [step.name for step in steps]
    unknown = [name for name in requested if name not in names]
    if unknown:
        raise ValueError(f"unknown group {', '.join(unknown)}; the groups are {', '.join(names)}")
    wide = ""
    if failure is not None:
        wide = f"fail wide: the change set could not be computed ({failure})"
    elif changed is not None:
        unclaimed = [path for path in changed if not is_claimed(steps, path)]
        if unclaimed:
            shown = abbreviate(unclaimed, 3)
            wide = f"fail wide: {shown} is claimed by no group"
    return Plan(tuple(choose(step, changed, requested, wide) for step in steps))


class ChangeSetError(RuntimeError):
    """The copy could not be compared with its accepted base."""


def changed_paths(root: Path, hub: Path) -> list[str]:
    """The paths where the copy differs from the accepted base (the hub's accepted pointer), sorted: edits,
    additions, deletions, mode changes and, for a move, both its old and its new path. It covers recorded
    patches the pointer does not hold yet and unrecorded changes alike, because it compares the files. An
    untracked directory (a new crate before `pijul add`) is reported as the directory itself, with a
    trailing `/`: it is neither walked nor matched against ignore rules, so no unreadable folder and no
    rule Pijul reads differently can hide a file (the planner treats such an entry conservatively; see
    `plan`). Anything that stops the comparison raises ChangeSetError: an unreadable pointer, a failed
    `pijul`, a symbolic link. It is never read as 'nothing changed'. (A copy older than the accepted base
    also shows the paths accepted since; it widens the plan, never narrows it.)"""
    try:
        base = {
            name: (entry["sha256"], entry["executable"])
            for name, entry in read_pointer(hub)["fingerprint"]["files"].items()
        }
        status = json.loads(capture(["pijul", "status", "--output-format", "json"], cwd=root))
        names = set(capture(["pijul", "list"], cwd=root).splitlines())
        names |= {str(change["path"]) for change in status["changes"]}
        directories: list[str] = []
        for untracked in (str(name) for name in status["untracked"]):
            path = root / untracked
            if path.is_symlink():
                raise ChangeSetError(f"{untracked} is a symbolic link")
            if path.is_dir():
                # Not walked, and no ignore rule is evaluated: the directory itself is the change.
                directories.append(untracked.rstrip("/") + "/")
            else:
                names.add(untracked)
        current: dict[str, tuple[str, bool]] = {}
        for name in sorted(names):
            path = root / name
            if path.is_symlink():
                raise ChangeSetError(f"{name} is a symbolic link")
            if path.is_file():
                current[name] = (
                    hashlib.sha256(path.read_bytes()).hexdigest(),
                    bool(path.stat().st_mode & 0o111),
                )
    except ChangeSetError:
        raise
    except (
        OSError,
        KeyError,
        TypeError,
        ValueError,
        RuntimeError,
        subprocess.CalledProcessError,
    ) as error:
        raise ChangeSetError(f"{type(error).__name__}: {error}") from error
    return sorted(
        [name for name in base.keys() | current.keys() if base.get(name) != current.get(name)]
        + directories
    )


def planned(root: Path, hub: Path, changed: bool, requested: Sequence[str]) -> Plan:
    """The plan of a planned check, printed. The tests directory and the catalog are verified first."""
    verify_catalog()
    verify_groups(root / "tests")
    paths: list[str] | None = None
    failure: str | None = None
    if changed:
        try:
            paths = changed_paths(root, hub)
        except ChangeSetError as error:
            failure = str(error)
    chosen = plan(STEPS, paths, requested, failure)
    if paths is not None:
        shown = abbreviate(paths, 10)
        print(f"check: change set: {len(paths)} path(s){': ' + shown if paths else ''}", flush=True)
    for line in chosen.lines():
        print(line, flush=True)
    return chosen


@contextlib.contextmanager
def watchdog(enabled: bool) -> Iterator[None]:
    """Raise StepTimeout in the main thread when the current step outlives WATCHDOG seconds."""
    if not enabled:
        yield
        return

    def fire(signum: int, frame: FrameType | None) -> None:
        if _own.owned or _own.causes:
            record_cause(_own, "expired")  # the first cause decides the exit status; see run()
            return
        _own.causes.append("expired")  # no child to end, but later signals now only append
        state = _state
        started = state.step_started if state and state.step_started is not None else None
        elapsed = time.monotonic() - started if started is not None else 0.0
        raise StepTimeout(state.step if state else None, elapsed, WATCHDOG)

    previous = signal.signal(signal.SIGALRM, fire)
    signal.setitimer(signal.ITIMER_REAL, WATCHDOG)
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        if not _own.causes:  # with a cause recorded the record-only handler stays installed
            signal.signal(signal.SIGALRM, previous)


def checks(
    hub: Path,
    level: str | bool,
    results: list[tuple[str, str]] | None = None,
    steps: Sequence[Step] | None = None,
) -> None:
    """Run the level's steps once each, in order. `level` may also be the legacy quick flag."""
    level = "quick" if level is True else "full" if level is False else level
    guard_groups(level, steps)  # before any step runs
    for step in steps or STEPS:
        if level != "planned" and level not in step.levels:
            continue
        if _own.causes:
            decide(_own, None)  # a recorded cause ends the run: no later step starts
        started = time.monotonic()
        timed = level in ("fast", "full", "planned") and _state is not None
        if _state is not None:
            _state.step, _state.step_started = step.name, started
        try:
            # Fast and full runs put every part of each step (its commands and its in-process
            # actions) under the watchdog; quick runs do not.
            with watchdog(timed):
                if step.action is not None:
                    step.action()
                for arguments in step.commands():
                    run(arguments, step=step)
        except StepTimeout:
            if results is not None:
                results.append((step.name, "TIMEOUT"))
            raise
        except BaseException:
            if results is not None:
                results.append((step.name, "FAILED"))
            raise
        if results is not None:
            results.append((step.name, "ok"))
        print(f"check: step {step.name} took {time.monotonic() - started:.1f} s", flush=True)
    if _own.causes:
        decide(_own, None)  # a cause recorded in the last step's hand-off is not a PASS


def acquire_slot(base: Path) -> tuple[int, int]:
    """Take the lowest `<n>.lock` under `base` without waiting; the lock is private to the copy.

    The descriptor is inheritable: every step's child keeps the slot until it exits."""
    base.mkdir(parents=True, exist_ok=True)
    for number in range(SLOT_LIMIT):
        descriptor = os.open(base / f"{number}.lock", os.O_RDWR | os.O_CREAT, 0o644)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            os.close(descriptor)
            continue
        os.set_inheritable(descriptor, True)
        return number, descriptor
    raise RuntimeError(f"No free build slot under {base}")


def summary(
    level: str, results: Sequence[tuple[str, str]], planned: Sequence[str], ok: bool
) -> str:
    done = dict(results)
    parts = [f"{name} {done.get(name, 'not run')}" for name in planned]
    return f"check {level}: {', '.join(parts)} => {'PASS' if ok else 'FAIL'}"


def execute(root: Path, hub: Path, level: str, steps: Sequence[Step] | None = None) -> int:  # noqa: PLR0912,PLR0915  # linear workflow, kept whole
    """Run one level on `root`; returns the exit code. Batch iff a valid CK_CI_LOCK_FD is inherited."""
    global _state  # noqa: PLW0603  # the run's State, read by run() and reset when the run ends
    planned = select(level, steps)
    # What the run actually uses, for the batch gate to read back from the log (CK-38): the recipe
    # level, the per-step watchdog and the steps that will run.
    print(f"check: recipe {level}; watchdog {WATCHDOG:g} s; steps {','.join(planned)}", flush=True)
    batch = False
    if "CK_CI_LOCK_FD" in os.environ:
        if ci_descriptor(hub) is None:
            print(
                f"check: inherited CK_CI_LOCK_FD does not name {hub / '.pipeline/ci.lock'}; "
                "restart the command as: tools/ck check",
                file=sys.stderr,
            )
            return 1
        batch = True
    print(
        f"check: {'batch' if batch else 'manual'} run; project lock held: "
        f"{'ci.lock, inherited as CK_CI_LOCK_FD' if batch else 'none'}",
        flush=True,
    )
    slot: tuple[int, int] | None = None
    if not batch and level in ("full", "planned"):
        slot = acquire_slot(root / "target/ck-full")
        target = root / "target/ck-full" / str(slot[0])
        print(f"check: build slot {slot[0]} ({target})", flush=True)
    else:
        target = root / "target"
    _state = State(root, hub, level, batch=batch, target=target, slot=slot)
    _own.reset()
    results: list[tuple[str, str]] = []
    code = 1
    # SIGTERM and Ctrl-C end the owned child's group instead of raising while a step owns one.
    previous: dict[int, Handler] = {
        signal.SIGTERM: signal.signal(signal.SIGTERM, terminate),
        signal.SIGINT: signal.signal(signal.SIGINT, interrupt),
    }
    try:
        checks(hub, level, results, steps)
        ok = True
    except StepTimeout as error:
        if error.cleaned is None:
            ended = "the step was interrupted"
        elif error.cleaned:
            ended = "its process group was killed and reaped"
        else:
            ended = "its process group could NOT be fully killed or reaped (see above)"
        print(
            f"check: step {error.step} exceeded the {error.limit:.0f} s watchdog after "
            f"{error.elapsed:.1f} s; {ended}. Exiting {EX_TEMPFAIL} "
            "(an infrastructure failure, not a patch failure)",
            file=sys.stderr,
        )
        ok = False
        code = EX_TEMPFAIL
    except subprocess.CalledProcessError as error:
        print(f"check: {error}", file=sys.stderr)
        ok = False
    except RuntimeError as error:
        print(f"check: {error}", file=sys.stderr)
        ok = False
    except BaseException:
        if not _own.causes:
            raise
        ok = False  # a recorded cause decides below and replaces what was in flight
    finally:
        if slot is not None:
            os.close(slot[1])  # the build slot is released first, once the step's group is reaped
        _state = None
        # The same single decision as run(): a recorded cause decides the exit and replaces whatever
        # is in flight, with the record-only handlers still installed until the process exits.
        if _own.causes:
            first = _own.causes[0]
            if first == "terminated":
                raise SystemExit(128 + signal.SIGTERM)  # replaces what is in flight
            if first == "interrupted":
                raise KeyboardInterrupt  # replaces what is in flight
            ok, code = False, EX_TEMPFAIL  # the expired watchdog's code replaces what is in flight
        else:
            for number, handler in previous.items():
                signal.signal(number, handler)  # last, and only without a cause
    print(summary(level, results, planned, ok), flush=True)
    return 0 if ok else code


def terminate(signum: int, frame: FrameType | None) -> None:
    if _own.owned or _own.causes:
        record_cause(_own, "terminated")  # the first cause decides the exit status; see run()
        return
    _own.causes.append("terminated")  # nothing to end, but later signals now only append
    raise SystemExit(128 + signum)


def interrupt(signum: int, frame: FrameType | None) -> None:
    """SIGINT (Ctrl-C): like `terminate`; with no child owned it is the usual KeyboardInterrupt."""
    if _own.owned or _own.causes:
        record_cause(_own, "interrupted")  # the first cause decides the exit status; see run()
        return
    _own.causes.append("interrupted")  # nothing to end, but later signals now only append
    raise KeyboardInterrupt


def enroll() -> bool:
    """Lease this checkout as the first action, before any subprocess (plan 10.4: candidates self-enroll).

    Every step's children inherit the descriptor, so a collector sees the checkout as in use for as
    long as any of them lives, including after this process and the batch controller have gone."""
    try:
        lease.acquire(ROOT)
    except lease.LeaseUnavailable as error:
        print(f"check: cannot lease this checkout ({error}); no step was run", file=sys.stderr)
        return False
    print(f"check: candidate lease held on {ROOT}", flush=True)
    return True


def main() -> int:
    if not enroll():
        return 1
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--quick", action="store_true", help="the static floor groups only")
    group.add_argument("--fast", action="store_true", help="every group, without full-spec mode")
    parser.add_argument(
        "--changed",
        action="store_true",
        help="a planned check: the floor groups and the groups the copy's changes affect",
    )
    parser.add_argument(
        "--group",
        action="append",
        default=[],
        metavar="ID",
        help="run this group too (a planned check; repeatable)",
    )
    parser.add_argument("--plan", action="store_true", help="print the plan and run nothing")
    args = parser.parse_args()
    is_planned = args.changed or bool(args.group)
    if is_planned and (args.quick or args.fast):
        parser.error("--changed and --group select a planned check; --quick and --fast do not mix")
    if args.plan and not is_planned:
        parser.error("--plan needs --changed or --group")
    if is_planned and "CK_CI_LOCK_FD" in os.environ:
        parser.error("a planned check is never batch evidence; the batch runs the full level")
    level = "planned" if is_planned else "quick" if args.quick else "fast" if args.fast else "full"
    hub = configure(ROOT)
    signal.signal(signal.SIGTERM, terminate)
    steps: Sequence[Step] | None = None
    if is_planned:
        try:
            chosen = planned(ROOT, hub, args.changed, args.group)
        except ValueError as error:
            parser.error(str(error))
        if args.plan:
            return 0
        steps = chosen.steps()
    if level == "quick":
        return execute(ROOT, hub, level)
    with signing(hub):
        return execute(ROOT, hub, level, steps)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (RuntimeError, subprocess.CalledProcessError) as error:
        print(f"check: {error}", file=sys.stderr)
        sys.exit(1)
