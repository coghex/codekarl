"""RC-3: the bootstrap quiescence scan matches the real subcommand and is never run by accident."""

import os
import subprocess
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import procfix
from gate import procscan
from gate.records import ProcessRow

HUB = Path("/fixture/hub")


class ProcessScanSpecification(unittest.TestCase):
    def setUp(self) -> None:
        procfix.isolate(self)

    def scan(self, command: str) -> list[int]:
        rows: list[ProcessRow] = [
            {"pid": 2, "ppid": 1, "command": "bootstrap helper"},
            {"pid": 9, "ppid": 1, "command": command},
        ]
        return [row["pid"] for row in procscan.scan(HUB, self_pid=2, lister=lambda: rows)]

    def test_01_only_the_subcommand_is_matched(self) -> None:
        for command in (
            "tools/ck session --role worker",
            "/x/tools/ck session --role worker",
            "tools/ck session --role status",
            "tools/ck issue show CK-1",
            "python /x/tools/ck check",
            "tools/ck message sync",
        ):
            with self.subTest(command=command):
                self.assertEqual(self.scan(command), [])
        for command in (
            "tools/ck worker x",
            "/abs/tools/ck submit --issue CK-1 --spec s",
            "./ck docs -m m p",
            "tools/ck sync",
            "tools/ck status",
            "ck init",
            "python /x/tools/ck accept",
            "tools/ck retry",
        ):
            with self.subTest(command=command):
                self.assertEqual(self.scan(command), [9])
        # The first `ck` word is the entry point; a later path argument named ck is an argument.
        self.assertEqual(self.scan("tools/ck session /home/ck"), [])

    def test_01a_the_pipeline_rule_and_the_ancestor_exclusion_are_unchanged(self) -> None:
        rows: list[ProcessRow] = [
            {"pid": 2, "ppid": 3, "command": "bootstrap helper"},
            {"pid": 3, "ppid": 1, "command": "/tmp/tools/ck status"},
            {"pid": 7, "ppid": 1, "command": f"python {HUB}/tools/pipeline.py submit"},
        ]
        found = procscan.scan(HUB, self_pid=2, lister=lambda: rows)
        self.assertEqual([row["pid"] for row in found], [7])

    def test_02_the_refusal_message_is_unchanged(self) -> None:
        rows: list[ProcessRow] = [{"pid": 9, "ppid": 1, "command": "tools/ck worker scratch"}]
        with self.assertRaisesRegex(RuntimeError, r"State writers are still running: pid 9: "):
            procscan.require_quiescent(HUB, lambda: rows)

    def test_03a_default_listers_are_resolved_when_called(self) -> None:
        calls: list[str] = []

        def patched() -> list[ProcessRow]:
            calls.append("patched")
            return [{"pid": 2, "ppid": 1, "command": "x"}]

        with mock.patch.object(procscan, "processes", patched):
            procscan.ancestors(2)
            procscan.scan(HUB, self_pid=2)
            procscan.require_quiescent(HUB)
        # ancestors (1), scan (ancestors + listing: 2), require_quiescent -> scan (2)
        self.assertEqual(len(calls), 5)

    def test_04_the_process_table_is_not_read_by_accident(self) -> None:
        with self.assertRaises(procfix.RealProcessTableRead):
            subprocess.check_output(["ps", "-axo", "pid=,ppid=,command="])
        # A default-bound alias of the original lister reaches the same call and is blocked too.
        with self.assertRaises(procfix.RealProcessTableRead):
            procfix.REAL_PROCESSES()
        with self.assertRaises(procfix.RealProcessTableRead):
            procscan.scan(HUB, lister=procfix.REAL_PROCESSES)
        self.assertEqual([row["pid"] for row in procscan.processes()], [os.getpid()])
        # Other commands, and the start-time lookup, are not affected.
        self.assertTrue(procscan.process_start(os.getpid()))

    @procfix.real_process
    def test_04a_a_marked_test_may_read_the_real_table(self) -> None:
        rows = procfix.REAL_PROCESSES()
        self.assertIn(os.getpid(), {row["pid"] for row in rows})


if __name__ == "__main__":
    unittest.main()
