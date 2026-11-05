"""RC-3: the bootstrap quiescence scan matches the real subcommand and is never run by accident."""

import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import procfix
from gate import lease, procscan
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

    def test_05_open_paths_reads_cwd_and_open_file_records(self) -> None:
        raw = (
            "p12\0\nfcwd\0n/fixture/worker\0\nf3\0n/fixture/worker/target/a\0"
            "\np13\0\nf0\0n/dev/null\0\n"
        )
        completed = subprocess.CompletedProcess(["lsof"], 0, stdout=raw, stderr="")
        with mock.patch.object(lease, "run", return_value=completed):
            self.assertEqual(
                procscan.open_paths(),
                {
                    12: [Path("/fixture/worker"), Path("/fixture/worker/target/a")],
                    13: [Path("/dev/null")],
                },
            )

    def test_05b_open_paths_with_parents_reads_ppid_records(self) -> None:
        raw = "p12\0R10\0\nfcwd\0n/fixture/worker\0\n"
        completed = subprocess.CompletedProcess(["lsof"], 0, stdout=raw, stderr="")
        with mock.patch.object(lease, "run", return_value=completed):
            self.assertEqual(
                procscan.open_paths_with_parents(),
                ({12: [Path("/fixture/worker")]}, {12: 10}),
            )

    @procfix.real_process
    def test_05a_real_lsof_finds_a_child_running_inside_a_fixture_copy(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            copy = Path(directory) / "worker-fixture"
            copy.mkdir()
            child = subprocess.Popen(
                [sys.executable, "-c", "import time; time.sleep(30)"],
                cwd=copy,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            try:
                time.sleep(0.1)
                users = procscan.users_of(copy)
                self.assertTrue(
                    any(pid == child.pid for pid, _path in users),
                    f"real lsof did not report child {child.pid} in {copy}: {users}",
                )
            finally:
                child.terminate()
                child.wait(timeout=5)

    def test_06_open_path_scan_fails_closed_on_unreadable_output(self) -> None:
        for output in ("", "n/orphan\0", "pnot-a-pid\0"):
            completed = subprocess.CompletedProcess(["lsof"], 0, stdout=output, stderr="")
            with (
                mock.patch.object(lease, "run", return_value=completed),
                self.assertRaises(RuntimeError),
            ):
                procscan.open_paths()
        with (
            mock.patch.object(
                lease,
                "run",
                side_effect=subprocess.CalledProcessError(1, ["lsof"]),
            ),
            self.assertRaisesRegex(RuntimeError, "cannot inspect process"),
        ):
            procscan.open_paths()
        unreadable = subprocess.CompletedProcess(
            ["lsof"], 0, stdout="p12\0n/fixture\0", stderr="permission denied\n"
        )
        with (
            mock.patch.object(lease, "run", return_value=unreadable),
            self.assertRaisesRegex(RuntimeError, "permission denied"),
        ):
            procscan.open_paths()

    def test_07_users_of_matches_any_process_path_inside_the_copy(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            copy = Path(directory) / "worker"
            (copy / "target").mkdir(parents=True)
            inside = copy / "target/output"
            outside = Path(directory) / "elsewhere"
            self.assertEqual(procscan.users_of(copy, {12: [inside], 13: [outside]}), [(12, inside)])

    def test_08_collector_exclusion_is_descendants_only(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            copy = Path(directory) / "worker"
            (copy / "target").mkdir(parents=True)
            inside = copy / "target/output"
            observed = {
                10: [inside],  # the collector's ancestor must still protect the copy
                12: [inside],  # the collector itself is ignored
                13: [inside],  # a child is ignored
                14: [inside],  # and so is a deeper descendant
                20: [inside],  # an unrelated process still protects the copy
            }
            parents = {10: 1, 12: 10, 13: 12, 14: 13, 20: 1}
            self.assertEqual(
                procscan.users_of(copy, observed, ignore_pid=12, parents=parents),
                [(10, inside), (20, inside)],
            )

    def test_09_missing_collector_ancestry_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            copy = Path(directory) / "worker"
            copy.mkdir()
            with self.assertRaisesRegex(RuntimeError, "omitted collector pid"):
                procscan.users_of(copy, {12: []}, ignore_pid=12, parents={20: 10})


if __name__ == "__main__":
    unittest.main()
