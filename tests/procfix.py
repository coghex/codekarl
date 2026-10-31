"""Process-table isolation for the bootstrap and batch tests (RC-3).

A test must never read the machine's real process table by accident: another agent's genuine
`tools/ck` writer would make it flaky. Unless a test is marked `real_process`, the underlying
`ps -axo` listing raises, and `procscan.processes` is replaced by a lister that reports only the
test's own process.
"""

import os
import subprocess
import unittest
from collections.abc import Callable
from typing import TypeVar
from unittest import mock

from gate import lease, procscan
from gate.records import ProcessRow

REAL_PROCESSES = procscan.processes


Method = TypeVar("Method", bound=Callable[..., object])


def real_process(method: Method) -> Method:
    """Mark a test that deliberately reads the real process table."""
    method.real_process = True  # type: ignore[attr-defined]  # a marker attribute on the function
    return method


class RealProcessTableRead(AssertionError):
    """A test that is not marked real_process listed the machine's processes."""


def own_process() -> list[ProcessRow]:
    return [{"pid": os.getpid(), "ppid": 1, "command": "python -m unittest"}]


def isolate(test: unittest.TestCase) -> None:
    """Call from setUp. Exempts only tests marked with @real_process."""
    if getattr(getattr(test, test._testMethodName), "real_process", False):
        return
    real_check_output: Callable[..., object] = subprocess.check_output

    def guarded(arguments: object, *args: object, **kwargs: object) -> object:
        if isinstance(arguments, (list, tuple)) and arguments[:1] == ["ps"] and "-axo" in arguments:
            raise RealProcessTableRead(f"test read the real process table: {arguments}")
        return real_check_output(arguments, *args, **kwargs)

    real_lease_output: Callable[..., object] = lease.check_output

    def guarded_lease(arguments: object, *args: object, **kwargs: object) -> object:
        # The controller reads the process table through the spawn helper, not subprocess directly.
        if isinstance(arguments, (list, tuple)) and arguments[:1] == ["ps"] and "-axo" in arguments:
            raise RealProcessTableRead(f"test read the real process table: {arguments}")
        return real_lease_output(arguments, *args, **kwargs)

    for patch in (
        mock.patch.object(subprocess, "check_output", guarded),
        mock.patch.object(lease, "check_output", guarded_lease),
        mock.patch.object(procscan, "processes", own_process),
    ):
        patch.start()
        test.addCleanup(patch.stop)
