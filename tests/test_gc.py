"""`ck gc` (S12b, CK-25): the dry run writes nothing, `--apply` removes only the closed list.

Every GC run here uses a fixture hub in a temporary directory; none touches a real hub.
"""

import contextlib
import fcntl
import hashlib
import io
import json
import os
import shutil
import signal
import stat
import subprocess
import sys
import textwrap
import time
import unittest
from collections.abc import Callable, Iterable, Mapping
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import pipeline
import test_batch
from gate import batch, cli, docsqueue, lease, locks, owned, pointer
from gate import gc as collector
from test_lease import (
    BOUND,
    TOOLS,
    LeaseFixture,
    clean_environment,
    copy_tools,
    exclusive_refused,
    wait_until,
)

DAY = 24 * 3600 * 10**9


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


Entry = tuple[str, int, int, int, str | None]


def snapshot(*roots: Path, exclude: Iterable[Path] = ()) -> dict[str, Entry]:
    """Every path with its type, size, inode, mtime and (for files) content hash.

    `exclude` names exact paths to leave out (see EntryPoint: the one file the trampoline's own
    verification rewrites); nothing else is ever skipped."""
    found: dict[str, Entry] = {}
    skipped = {str(path) for path in exclude}
    for root in roots:
        for path in [root, *sorted(root.rglob("*"))]:
            if str(path) in skipped:
                continue
            info = path.lstat()
            kind = (
                "link"
                if stat.S_ISLNK(info.st_mode)
                else "dir"
                if stat.S_ISDIR(info.st_mode)
                else "file"
            )
            found[str(path)] = (
                kind,
                info.st_size,
                info.st_ino,
                info.st_mtime_ns,
                digest(path) if kind == "file" else None,
            )
    return found


class GcFixture(LeaseFixture):
    """A fixture hub: `.pipeline` with the locks and records GC reads, a metadata directory, and a
    current checkpoint named by a schema-2 pointer."""

    def setUp(self) -> None:
        super().setUp()
        self.hub = self.base / "hub"
        self.pipe = self.hub / ".pipeline"
        self.checkpoints = self.pipe / "checkpoints"
        self.checkpoints.mkdir(parents=True)
        for name in ("batches", "bootstrap", "docs/journal", "docs/captures", "submissions"):
            (self.pipe / name).mkdir(parents=True)
        self.meta = self.base / "codekarl-meta"
        self.meta.mkdir()
        for lock in (self.pipe / "state.lock", self.pipe / "ci.lock", self.meta / ".lock"):
            lock.write_text('{"pid": 1, "command": "ck earlier", "started": 1.0}')
        self.now = time.time_ns()
        self.current = self.checkpoint(f"docs-D{self.now - 30 * DAY}", target=True)
        GcFixture.publish(self, self.current)

    # --- builders ----------------------------------------------------------------------------

    def checkpoint(
        self, name: str, *, capable: bool = True, target: bool = True, content: str = "source\n"
    ) -> Path:
        path = self.checkpoints / name
        (path / "tools/gate").mkdir(parents=True)
        (path / "tools/gate/cli.py").write_text(content)
        if capable:
            (path / "tools/gate/lease.py").write_text("# lease\n")
        if target:
            (path / "target/debug").mkdir(parents=True)
            (path / "target/debug/artifact.bin").write_bytes(b"x" * 4096)
        return path

    def old(
        self,
        kind: str,
        days: float,
        *,
        capable: bool = True,
        target: bool = True,
        content: str = "source\n",
    ) -> Path:
        """A checkpoint of the given kind created `days` ago."""
        created = self.now - int(days * DAY)
        names = {
            "docs": f"docs-D{created}",
            "combined": f"{created}-combined",
            "split": f"{created}-split-0",
            "restore": f"restore-{created}-abc-{created}",
            "legacy": f"{created}-0123abcd",
        }
        return self.checkpoint(names[kind], capable=capable, target=target, content=content)

    def publish(self, repo: Path) -> None:
        state = {
            "schema": 2,
            "repo": str(repo),
            "fingerprint": {"hashes": [], "files": {}},
            "submissions": [],
        }
        pointer.atomic_bytes(self.pipe / "accepted-v2.json", pointer.canonical(state))
        pointer.atomic_bytes(
            self.pipe / "accepted.json", pointer.canonical({"schema": "tombstone"})
        )

    def batch(self, bid: int | str, marker: str | None = None, **extra: object) -> Path:
        directory = self.pipe / "batches" / str(bid)
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "frozen.json").write_text("{}")
        if marker:
            (directory / f"{marker}.json").write_text("{}")
        for name in extra:
            (directory / f"{name}.json").write_text("{}")
        return directory

    def record_file(self, name: str, content: str = "record\n") -> Path:
        path = self.checkpoints / name
        path.write_text(content)
        return path

    # --- running -----------------------------------------------------------------------------

    def gc(self, apply: bool = False) -> tuple[str, RuntimeError | None]:
        buffer = io.StringIO()
        try:
            with contextlib.redirect_stdout(buffer):
                collector.command(self.hub, apply=apply)
        except RuntimeError as error:
            return buffer.getvalue(), error
        return buffer.getvalue(), None

    def dry(self) -> str:
        text, error = self.gc()
        self.assertIsNone(error, text)
        return text

    def apply(self) -> str:
        text, error = self.gc(apply=True)
        self.assertIsNone(error, text)
        return text

    def everything(self, *, exclude: Iterable[Path] = ()) -> dict[str, Entry]:
        return snapshot(self.hub, self.meta, exclude=exclude)

    def no_spawn(self) -> mock.Mock:
        """A spy on every way of starting a process: GC's own code must run no command at all."""
        spy = mock.Mock(side_effect=AssertionError("gc started a process"))
        patches = [
            mock.patch.object(subprocess, "Popen", spy),
            mock.patch.object(subprocess, "run", spy),
            mock.patch.object(subprocess, "check_output", spy),
            mock.patch.object(lease, "run", spy),
            mock.patch.object(lease, "check_output", spy),
            mock.patch.object(os, "system", spy),
        ]
        stack = contextlib.ExitStack()
        for patch in patches:
            stack.enter_context(patch)
        self.addCleanup(stack.close)
        return spy

    def line(self, text: str, fragment: str) -> str:
        matches = [line for line in text.splitlines() if fragment in line]
        self.assertEqual(len(matches), 1, f"{fragment!r} in {text}")
        return matches[0]


class QuietLocks(GcFixture):
    def test_01_a_quiet_acquisition_takes_the_kernel_lock_and_writes_nothing(self) -> None:
        before = self.everything()
        with locks.state(self.hub, quiet=True), locks.meta(self.meta, quiet=True):
            descriptor = os.open(self.pipe / "state.lock", os.O_RDWR)
            try:
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            finally:
                os.close(descriptor)
        self.assertEqual(self.everything(), before)

    def test_02_a_recording_acquisition_still_records_the_holder(self) -> None:
        with locks.state(self.hub):
            pass
        record = (self.pipe / "state.lock").read_text()
        self.assertIn(str(os.getpid()), record)

    def test_03_a_missing_lock_file_is_refused_and_never_created(self) -> None:
        (self.pipe / "state.lock").unlink()
        with self.assertRaises(RuntimeError) as caught, locks.state(self.hub, quiet=True):
            pass
        self.assertIn("state.lock", str(caught.exception))
        self.assertFalse((self.pipe / "state.lock").exists())


class DryRun(GcFixture):
    def populate(self) -> None:
        self.old("docs", 40)
        self.old("docs", 41)
        self.old("docs", 42)  # with the current one, the 3 newest are 30, 40 and 41 days old
        self.old("combined", 2)
        self.batch(self.now - 2 * DAY, "published")
        self.old("legacy", 50, capable=False)
        self.record_file("1-combined.log")
        (self.pipe / "target").mkdir()
        (self.pipe / "target/cache.bin").write_bytes(b"y" * 100)
        (self.pipe / "submissions/c1").mkdir()
        (self.pipe / "submissions/c1/manifest.json").write_text("{}")

    def test_01_it_deletes_and_writes_nothing_and_holds_both_locks_while_planning(self) -> None:
        self.populate()
        before = self.everything()
        seen: list[str] = []

        def hook(stage: str) -> None:
            if stage == "planned":
                for path in (self.pipe / "state.lock", self.meta / ".lock"):
                    descriptor = os.open(path, os.O_RDWR)
                    try:
                        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        seen.append(f"free: {path}")
                    except BlockingIOError:
                        seen.append("held")
                    finally:
                        os.close(descriptor)

        with mock.patch.object(collector, "HOOK", hook):
            text = self.dry()
        self.assertEqual(seen, ["held", "held"])
        self.assertEqual(self.everything(), before)  # lock files, holder metadata, mtimes included
        self.assertIn("gc itself deletes and writes nothing and runs no Pijul command", text)

    def test_02_a_missing_state_lock_refuses_naming_it_and_creates_nothing(self) -> None:
        self.populate()
        (self.pipe / "state.lock").unlink()
        before = self.everything()
        _text, error = self.gc()
        self.assertIsNotNone(error)
        self.assertIn("state.lock", str(error))
        self.assertEqual(self.everything(), before)

    def test_03_a_missing_meta_lock_refuses_naming_it_and_creates_nothing(self) -> None:
        self.populate()
        (self.meta / ".lock").unlink()
        before = self.everything()
        _text, error = self.gc()
        self.assertIsNotNone(error)
        self.assertIn(".lock", str(error))
        self.assertEqual(self.everything(), before)

    def test_04_it_reports_every_item_with_its_kind_size_and_rule(self) -> None:
        self.populate()
        text = self.dry()
        oldest = [
            line
            for line in text.splitlines()
            if line.startswith("CANDIDATE") and line.split()[1] == "checkpoint"
        ]
        self.assertEqual(len(oldest), 1, text)
        self.assertIn(f"docs-D{self.now - 42 * DAY}", oldest[0])
        self.assertIn("B", oldest[0])  # a size
        self.assertIn("superseded docs checkpoint", oldest[0])
        self.assertIn("rule 1", self.line(text, f"docs-D{self.now - 30 * DAY} "))
        self.assertIn("rule 8", self.line(text, f"{self.now - 2 * DAY}-combined "))
        self.assertIn("rule 7", self.line(text, "-0123abcd "))
        self.assertIn("rule 7", self.line(text, str(self.pipe / "target ")))  # the legacy cache
        self.assertIn("rule 3", self.line(text, "submissions/c1"))
        self.assertIn("records", text)
        # the three kept checkpoints keep their source, but their unleased build output is collectible
        targets = [
            line
            for line in text.splitlines()
            if line.startswith("CANDIDATE") and line.split()[1] == "target"
        ]
        self.assertEqual(len(targets), 4, text)  # three kept docs/combined ones and the current one
        self.assertIn("summary: 5 candidate(s)", text)


class ApplyNeverRemoves(GcFixture):
    def test_01_only_the_closed_list_is_ever_removed(self) -> None:
        self.old("docs", 60)
        self.old("docs", 61)
        self.old("docs", 62)
        victim = self.old(
            "docs", 63
        )  # with the current one, the 3 newest are 30, 60 and 61 days old
        kept = {
            "current source": self.current / "tools/gate/cli.py",
        }
        # a non-terminal batch: validating, never published, no state.lock needed
        bid = self.now - 80 * DAY
        self.batch(bid)
        for days in (
            10,
            11,
            12,
        ):  # three newer, terminal ones: the open batch is not among the newest
            newer = self.now - days * DAY
            self.batch(newer, "published")
            self.checkpoint(f"{newer}-combined")
        kept["non-terminal batch candidate"] = (
            self.checkpoint(f"{bid}-combined") / "tools/gate/cli.py"
        )
        kept["non-terminal batch target"] = (
            self.checkpoints / f"{bid}-combined/target/debug/artifact.bin"
        )
        # captures of every kind and the bootstrap helper's
        for relative in ("submissions/1", "docs/captures/D1", "docs/captures/D1-resolve"):
            (self.pipe / relative).mkdir(parents=True)
            kept[relative] = self.pipe / relative / "manifest.json"
            kept[relative].write_text("{}")
        # records
        for name in ("a.json", "a.log", "a-combined.chat-inputs.json"):
            kept[name] = self.record_file(name)
        # pre-S12 checkpoint and its target; the legacy cache; the bootstrap checkpoint
        legacy = self.old("legacy", 90, capable=False)
        kept["pre-S12 source"] = legacy / "tools/gate/cli.py"
        kept["pre-S12 target"] = legacy / "target/debug/artifact.bin"
        (self.pipe / "target").mkdir()
        kept["legacy cache"] = self.pipe / "target/cache.bin"
        kept["legacy cache"].write_bytes(b"cache")
        bootstrap = self.checkpoint(f"bootstrap-{self.now - 95 * DAY}")
        kept["bootstrap"] = bootstrap / "tools/gate/cli.py"
        for days in (
            20,
            21,
            22,
        ):  # three newer bootstrap checkpoints: this one is not among the newest
            self.checkpoint(f"bootstrap-{self.now - days * DAY}")
        (self.pipe / "bootstrap" / "b1").mkdir()
        (self.pipe / "bootstrap/b1/published.json").write_text("{}")
        before = {name: path.read_bytes() for name, path in kept.items()}
        for path in kept.values():
            self.assertTrue(path.exists(), path)
        text = self.apply()
        self.assertFalse(victim.exists(), "the superseded checkpoint should be collected")
        for name, path in kept.items():
            self.assertEqual(path.read_bytes(), before[name], name)
        self.assertEqual(list((self.pipe / "trash").iterdir()), [])  # and trash was emptied
        self.assertIn("moved to trash/", text)

    def test_02_a_collectible_target_is_removed_but_its_checkpoint_stays(self) -> None:
        # a terminal batch checkpoint inside the window keeps its source; its unleased target goes
        bid = self.now - 2 * DAY
        self.batch(bid, "failed")
        candidate = self.checkpoint(f"{bid}-combined")
        self.apply()
        self.assertTrue((candidate / "tools/gate/cli.py").exists())
        self.assertFalse((candidate / "target").exists())

    def test_03_the_work_survives_a_second_apply(self) -> None:
        for days in (60, 61, 62, 63):
            self.old("docs", days)
        self.apply()
        before = self.everything()
        text = self.apply()
        self.assertIn("0 candidate(s)", text)

        def keep(snap: Mapping[str, Entry]) -> dict[str, Entry]:
            return {k: v for k, v in snap.items() if ".lock" not in k}

        self.assertEqual(keep(self.everything()), keep(before))

    def test_04_the_owners_post_publish_hook_is_never_listed_or_removed(self) -> None:
        for days in (60, 61, 62):
            self.old("docs", days)
        victim = self.old("docs", 63)  # so that the apply has something real to remove
        hooks = self.pipe / "hooks"
        hooks.mkdir()
        (hooks / "post-publish").write_text("#!/bin/sh\n")
        (hooks / "post-publish").chmod(0o755)
        (hooks / "post-publish.log").write_text("log\n")
        (hooks / "notes").write_text("owner's file\n")

        def mine(snap: Mapping[str, Entry]) -> dict[str, Entry]:
            return {k: v for k, v in snap.items() if str(hooks) in k}

        before = mine(self.everything())
        self.assertEqual(len(before), 4)
        self.assertNotIn("hooks", self.dry())
        self.assertNotIn("hooks", self.apply())
        self.assertFalse(victim.exists())
        self.assertEqual(mine(self.everything()), before)


class CurrentCheckpoint(GcFixture):
    """The current checkpoint is recognized by identity, never by how its path is spelled."""

    def make_current_collectible_by_every_other_rule(self) -> None:
        for days in (
            1,
            2,
            3,
        ):  # three newer docs checkpoints: the current one is not among the newest
            self.old("docs", days)

    def test_01_a_pointer_spelled_through_a_symlink_still_protects_the_current_checkpoint(
        self,
    ) -> None:
        self.make_current_collectible_by_every_other_rule()
        alias = self.base / "alias"
        alias.symlink_to(self.hub)
        spelled = alias / ".pipeline/checkpoints" / self.current.name
        self.publish(spelled)  # the same directory, a different spelling
        text = self.apply()
        self.assertTrue((self.current / "tools/gate/cli.py").exists(), text)
        self.assertIn("rule 1", self.line(text, f"{self.current.name} "))

    def test_02_the_current_checkpoint_is_protected_without_the_other_rules(self) -> None:
        self.make_current_collectible_by_every_other_rule()
        text = self.apply()
        self.assertTrue((self.current / "tools/gate/cli.py").exists(), text)

    def test_03_the_rename_step_refuses_the_current_checkpoint_on_its_own(self) -> None:
        trash = self.pipe / "trash"
        trash.mkdir()
        info = self.current.lstat()
        item = collector.Item(
            "checkpoint",
            self.current,
            1,
            "candidate",
            "forced",
            self.current.name,
            lease_path=self.current,
        )
        outcome = collector._to_trash(item, self.hub, trash, (info.st_dev, info.st_ino))
        self.assertTrue(outcome.startswith("refused") and "current" in outcome, outcome)
        self.assertTrue(self.current.exists())
        unknown = collector._to_trash(item, self.hub, trash, None)
        self.assertTrue(unknown.startswith("refused"), unknown)

    def test_04_a_pointer_to_a_missing_repository_plans_nothing_for_removal(self) -> None:
        self.old("docs", 90)
        self.publish(self.checkpoints / "docs-D1")
        text, error = self.gc(apply=True)
        self.assertIsNotNone(error)
        self.assertIn("pointer cannot be used", str(error))
        self.assertEqual(sum(line.startswith("CANDIDATE") for line in text.splitlines()), 0, text)


class ProtectingRecordShapes(GcFixture):
    """A protecting record that is valid JSON but not what the gate writes is a recovery condition,
    exactly like one that does not parse: apply refuses and collects nothing."""

    def setUp(self) -> None:
        super().setUp()
        for days in (60, 61, 62):
            self.old("docs", days)
        self.victim = self.old("docs", 63)

    def refuses(self, expected: str = "unexpected shape") -> None:
        dry = self.dry()
        self.assertIn("RECOVERY CONDITION", dry)
        self.assertIn(expected, dry)
        text, error = self.gc(apply=True)
        self.assertIsNotNone(error)
        self.assertIn(expected, text + str(error))
        self.assertTrue(self.victim.exists())
        self.assertFalse((self.pipe / "trash").exists())

    def test_01_a_journal_record_that_is_not_an_object(self) -> None:
        for index, text in enumerate(("[]", "null", '"text"', "7")):
            with self.subTest(text):
                path = self.pipe / "docs/journal" / f"{index:08d}.json"
                path.write_text(text)
                self.refuses()
                path.unlink()

    def test_02_a_journal_record_whose_entries_are_not_a_list_of_ids(self) -> None:
        for index, text in enumerate(('{"entries": "D1"}', '{"entries": [1]}', '{"entries": {}}')):
            with self.subTest(text):
                path = self.pipe / "docs/journal" / f"{index:08d}.json"
                path.write_text(text)
                self.refuses()
                path.unlink()

    def test_03_a_docs_entry_that_is_not_an_object(self) -> None:
        (self.pipe / "docs/journal/00000001.json").write_text('{"entries": ["D1"]}')
        (self.pipe / "docs/D1.json").write_text("[]")
        self.refuses()

    def test_04_a_terminal_marker_that_is_not_an_object(self) -> None:
        for marker in ("published", "failed", "rejected", "cancelled"):
            with self.subTest(marker):
                bid = self.now - 95 * DAY
                self.batch(bid, marker)
                (self.pipe / f"batches/{bid}/{marker}.json").write_text("[]")
                self.refuses()
                shutil.rmtree(self.pipe / f"batches/{bid}")

    def test_05_a_frozen_record_of_an_open_batch_that_is_not_an_object(self) -> None:
        bid = self.now - 95 * DAY
        self.batch(bid)
        (self.pipe / f"batches/{bid}/frozen.json").write_text("null")
        self.refuses()

    def test_06_a_bootstrap_marker_that_is_not_an_object(self) -> None:
        (self.pipe / "bootstrap/b1").mkdir()
        (self.pipe / "bootstrap/b1/published.json").write_text("[]")
        self.refuses()

    def test_07_a_pointer_that_is_not_an_object(self) -> None:
        (self.pipe / "accepted-v2.json").write_text("[]")
        dry = self.dry()
        self.assertIn("pointer cannot be used", dry)
        self.assertEqual(sum(line.startswith("CANDIDATE") for line in dry.splitlines()), 0, dry)
        _text, error = self.gc(apply=True)
        self.assertIsNotNone(error)
        self.assertTrue(self.victim.exists())


class RunsNoCommand(GcFixture):
    """GC's own code starts no process, so it runs no Pijul command in any checkpoint."""

    def test_01_neither_the_dry_run_nor_apply_starts_a_process(self) -> None:
        for days in (60, 61, 62):
            self.old("docs", days)
        victim = self.old("docs", 63)
        leftover = self.pipe / "trash/1-checkpoint-docs-D1"
        leftover.mkdir(parents=True)
        spy = self.no_spawn()
        self.dry()
        self.apply()
        self.assertFalse(victim.exists())
        self.assertFalse(leftover.exists())
        spy.assert_not_called()

    def test_02_the_spy_would_catch_a_pijul_command(self) -> None:
        spy = self.no_spawn()
        with self.assertRaises(AssertionError):
            lease.run(["pijul", "log"])
        spy.assert_called_once()

    def test_03_the_gc_module_has_no_way_to_start_one(self) -> None:
        source = (TOOLS / "gate/gc.py").read_text()
        for word in ("subprocess", "import pipeline", "lease.run", "check_output", "os.system"):
            self.assertNotIn(word, source)
        self.assertNotRegex(source, r"pijul\s+(log|status|diff|list)")


class Window(GcFixture):
    def test_01_recent_and_newest_are_kept_and_the_rest_collected(self) -> None:
        recent = self.old("docs", 1)
        eight = self.old("docs", 8)
        nine = self.old("docs", 9)
        ten = self.old("docs", 10)
        eleven = self.old("docs", 11)
        self.apply()
        self.assertTrue(recent.exists() and eight.exists() and nine.exists())  # window or 3 newest
        self.assertFalse(ten.exists() or eleven.exists())

    def test_02_an_old_checkpoint_among_the_three_newest_of_its_kind_is_kept(self) -> None:
        first, second = self.old("combined", 20), self.old("combined", 21)
        self.batch(self.now - 20 * DAY, "published")
        self.batch(self.now - 21 * DAY, "published")
        self.apply()
        self.assertTrue(first.exists() and second.exists())

    def test_03_each_kind_has_its_own_three_newest(self) -> None:
        docs = [self.old("docs", 31 + n) for n in range(3)]
        combined = []
        for n in range(4):
            created = self.now - (30 + n) * DAY
            self.batch(created, "published")
            combined.append(self.checkpoint(f"{created}-combined"))
        self.apply()
        # the current checkpoint is a docs checkpoint too: its 30 days, then 31 and 32, are the newest
        self.assertEqual([path.exists() for path in docs], [True, True, False])
        self.assertEqual([path.exists() for path in combined], [True, True, True, False])

    def test_04_a_batch_checkpoint_without_a_batch_record_is_kept(self) -> None:
        orphan = self.old("combined", 50)
        self.old("combined", 51), self.old("combined", 52), self.old("combined", 53)
        text = self.apply()
        self.assertTrue(orphan.exists())
        self.assertIn("no record of batch", text)


if __name__ == "__main__":
    unittest.main()


class Leases(GcFixture):
    """A leased checkpoint is reported in use, with its target/; GC's own checkpoint is in use by it."""

    HOLDER = """
        import sys
        sys.path.insert(0, {tools!r})
        from gate import lease
        lease.acquire({path!r})
        print('ready', flush=True)
        sys.stdin.read()
    """

    def test_01_a_leased_checkpoint_and_its_target_are_in_use_until_the_child_exits(self) -> None:
        bid = self.now - 2 * DAY
        self.batch(bid, "published")
        leased = self.checkpoint(f"{bid}-combined")
        process = subprocess.Popen(
            [
                sys.executable,
                "-c",
                textwrap.dedent(self.HOLDER.format(tools=str(TOOLS), path=str(leased))),
                str(self.base),
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
        )
        assert process.stdout is not None
        assert process.stdin is not None
        self.addCleanup(process.kill)
        self.addCleanup(process.stdout.close)
        self.addCleanup(process.stdin.close)
        self.assertEqual(process.stdout.readline().strip(), b"ready")
        text = self.dry()
        self.assertTrue(self.line(text, f"{leased}/target").startswith("IN-USE"), text)
        self.apply()
        self.assertTrue(
            (leased / "target/debug/artifact.bin").exists(), "a leased target was removed"
        )
        process.stdin.close()
        process.wait(timeout=BOUND)
        wait_until(lambda: not exclusive_refused(leased), "the child's lease to end")
        self.apply()
        self.assertFalse(
            (leased / "target").exists(), "the target was not collected after the child exited"
        )
        self.assertTrue(
            (leased / "tools/gate/cli.py").exists()
        )  # inside the window: the source stays

    def test_02_the_current_checkpoints_target_is_in_use_by_this_gc_until_the_pointer_moves(
        self,
    ) -> None:
        descriptor = lease.acquire(
            self.current
        )  # what the controller does first, as this gc's own lease
        self.addCleanup(lease.release, descriptor)
        text = self.dry()
        line = self.line(text, f"{self.current}/target")
        self.assertTrue(line.startswith("IN-USE") and "in use by this gc" in line, line)
        self.apply()
        self.assertTrue((self.current / "target").exists())
        lease.release(descriptor)
        self.addCleanup(lambda: None)
        newer = self.checkpoint(f"docs-D{self.now - DAY}")
        self.publish(newer)
        descriptor = lease.acquire(newer)  # a later run is a controller of the newer checkpoint
        self.addCleanup(lease.release, descriptor)
        self.apply()
        self.assertFalse((self.current / "target").exists(), "a later run should collect it")
        self.assertTrue((self.current / "tools/gate/cli.py").exists())  # recent: the source stays


class LeaseAfterPlanning(GcFixture):
    def test_01_a_lease_taken_between_the_plan_and_the_rename_stops_the_rename(self) -> None:
        for days in (60, 61, 62):
            self.old("docs", days)
        victim = self.old("docs", 63)
        held: list[int] = []

        def hook(stage: str) -> None:
            if stage == "planned":
                descriptor = os.open(victim, os.O_RDONLY | os.O_DIRECTORY)
                fcntl.flock(descriptor, fcntl.LOCK_SH)  # a reader enrolls after the plan was made
                held.append(descriptor)

        def release() -> None:
            for descriptor in held:
                os.close(descriptor)

        self.addCleanup(release)
        with mock.patch.object(collector, "HOOK", hook):
            text = self.apply()
        self.assertTrue(victim.exists())
        self.assertIn(f"{victim}: in use", text)


class PathSafety(GcFixture):
    def test_01_a_symlinked_checkpoint_is_refused_and_nothing_outside_is_touched(self) -> None:
        outside = self.base / "outside"
        (outside / "tools/gate").mkdir(parents=True)
        (outside / "tools/gate/lease.py").write_text("x")
        link = self.checkpoints / f"docs-D{self.now - 70 * DAY}"
        link.symlink_to(outside)
        text = self.dry()
        self.assertTrue(self.line(text, str(link)).startswith("REFUSED"), text)
        self.apply()
        self.assertTrue(link.is_symlink() and (outside / "tools/gate/lease.py").exists())

    def test_02_a_symlinked_target_is_refused_and_not_followed(self) -> None:
        bid = self.now - 2 * DAY
        self.batch(bid, "published")
        checkpoint = self.checkpoint(f"{bid}-combined", target=False)
        outside = self.base / "outside-target"
        outside.mkdir()
        (outside / "keep.txt").write_text("keep")
        (checkpoint / "target").symlink_to(outside)
        text = self.dry()
        self.assertTrue(self.line(text, f"{checkpoint}/target").startswith("REFUSED"), text)
        self.apply()
        self.assertTrue((checkpoint / "target").is_symlink() and (outside / "keep.txt").exists())

    def test_03_a_name_outside_the_patterns_is_never_touched(self) -> None:
        odd = self.checkpoints / "something-else"
        (odd / "tools/gate").mkdir(parents=True)
        (odd / "tools/gate/lease.py").write_text("x")
        text = self.dry()
        self.assertTrue(self.line(text, str(odd)).startswith("PROTECTED"), text)
        self.assertIn("outside the checkpoint patterns", text)
        self.apply()
        self.assertTrue(odd.exists())
        crafted = collector.Item(
            "checkpoint", odd, 1, "candidate", "forced", odd.name, lease_path=odd
        )
        (self.pipe / "trash").mkdir(exist_ok=True)
        outcome = collector._to_trash(crafted, self.hub, self.pipe / "trash", (0, 0))
        self.assertTrue(outcome.startswith("refused") and "pattern" in outcome, outcome)
        self.assertTrue(odd.exists())

    def test_04_a_candidate_replaced_by_a_symlink_after_planning_is_refused(self) -> None:
        victim = self.old("docs", 80)
        for days in (81, 82, 83):
            self.old("docs", days)
        outside = self.base / "outside"
        outside.mkdir()
        (outside / "keep.txt").write_text("keep")
        gone = self.old("docs", 90)

        def hook(stage: str) -> None:
            if stage == "planned":
                shutil.rmtree(gone)
                gone.symlink_to(outside)

        with mock.patch.object(collector, "HOOK", hook):
            text = self.apply()
        self.assertRegex(text, rf"{gone.name}: refused: ")
        self.assertTrue(gone.is_symlink() and (outside / "keep.txt").exists())
        self.assertTrue(victim.exists())  # the newest three of the kind stay

    def test_04b_a_candidate_replaced_by_a_symlink_to_a_directory_inside_pipeline_is_refused(
        self,
    ) -> None:
        for days in (81, 82, 83):
            self.old("docs", days)
        gone = self.old("docs", 90)
        inside = self.pipe / "submissions/c1"
        inside.mkdir(parents=True)
        (inside / "manifest.json").write_text("{}")

        def hook(stage: str) -> None:
            if stage == "planned":
                shutil.rmtree(gone)
                gone.symlink_to(inside)  # resolves inside .pipeline, but is not a real directory

        with mock.patch.object(collector, "HOOK", hook):
            text = self.apply()
        self.assertIn("not a real directory", text)
        self.assertTrue(gone.is_symlink() and (inside / "manifest.json").exists())

    def test_05_a_trash_on_another_device_refuses_every_rename(self) -> None:
        for days in (60, 61, 62):
            self.old("docs", days)
        victim = self.old("docs", 63)
        with mock.patch.object(collector, "_same_device", lambda *_: False):
            text = self.apply()
        self.assertIn("trash is on another device", text)
        self.assertTrue(victim.exists())
        self.assertEqual(
            list((self.pipe / "trash").iterdir()) if (self.pipe / "trash").exists() else [], []
        )

    def test_06_an_rmtree_that_cannot_avoid_symlink_attacks_refuses_before_any_rename(self) -> None:
        for days in (60, 61, 62):
            self.old("docs", days)
        victim = self.old("docs", 63)
        before = self.everything()
        with mock.patch.object(shutil.rmtree, "avoids_symlink_attacks", False):
            _text, error = self.gc(apply=True)
        self.assertIsNotNone(error)
        self.assertIn("avoid symlink attacks", str(error))
        self.assertTrue(victim.exists())
        self.assertEqual(self.everything(), before)


class Trash(GcFixture):
    def make_trash(self, name: str = "1-checkpoint-docs-D1") -> Path:
        entry = self.pipe / "trash" / name
        (entry / "sub").mkdir(parents=True)
        (entry / "sub/file.bin").write_bytes(b"z" * 100)
        return entry

    def test_01_trash_left_by_a_crash_is_reported_by_a_dry_run_and_removed_first_by_apply(
        self,
    ) -> None:
        entry = self.make_trash()
        text = self.dry()
        self.assertTrue(self.line(text, str(entry)).startswith("CANDIDATE"), text)
        self.assertTrue(entry.exists())
        self.apply()
        self.assertFalse(entry.exists())

    def test_02a_a_proceeding_apply_removes_leftover_trash_before_it_moves_new_candidates(
        self,
    ) -> None:
        entry = self.make_trash()
        for days in (60, 61, 62):
            self.old("docs", days)
        victim = self.old("docs", 63)
        text = self.apply()
        self.assertFalse(entry.exists())
        self.assertFalse(victim.exists())
        self.assertLess(text.index(f"removed trash/{entry.name}"), text.index("moved to trash/"))

    def test_02b_a_refused_apply_removes_nothing_not_even_leftover_trash(self) -> None:
        entry = self.make_trash()
        before = (entry / "sub/file.bin").read_bytes()
        self.batch(self.now - 90 * DAY, "publishing")  # a recovery condition
        for days in (60, 61, 62):
            self.old("docs", days)
        victim = self.old("docs", 63)
        _text, error = self.gc(apply=True)
        self.assertIsNotNone(error)
        self.assertEqual((entry / "sub/file.bin").read_bytes(), before)
        self.assertTrue(victim.exists())

    def test_02_leftover_trash_is_removed_without_any_lock(self) -> None:
        entry = self.make_trash()
        state = os.open(self.pipe / "state.lock", os.O_RDWR)
        fcntl.flock(state, fcntl.LOCK_EX)  # someone else holds state.lock
        try:
            with mock.patch.object(collector, "delete_trash", wraps=collector.delete_trash) as spy:
                lines = collector.delete_trash(self.pipe / "trash")
        finally:
            os.close(state)
        self.assertFalse(entry.exists())
        self.assertEqual(lines, [f"removed trash/{entry.name}"])
        self.assertTrue(spy.called)

    def test_03_an_entry_still_being_written_is_retried_not_reported_as_a_failure(self) -> None:
        self.make_trash()
        real: Callable[..., object] = owned.remove_tree

        def busy(parent: Path, name: str, **kwargs: object) -> None:
            real(parent, name, **kwargs)  # the contents go ...
            raise OSError(66, "Directory not empty")  # ... but a writer added one meanwhile

        with mock.patch.object(owned, "remove_tree", busy):
            lines = collector.delete_trash(self.pipe / "trash")
        self.assertEqual(len(lines), 1)
        self.assertTrue(lines[0].startswith("retry later"), lines)

    def test_04_two_cleanups_of_the_same_trash_are_harmless(self) -> None:
        entry = self.make_trash()
        real: Callable[..., object] = owned.remove_tree

        def raced(parent: Path, name: str, **kwargs: object) -> None:
            shutil.rmtree(entry)  # the other cleanup finished it first
            real(parent, name, **kwargs)

        with mock.patch.object(owned, "remove_tree", raced):
            lines = collector.delete_trash(self.pipe / "trash")
        self.assertEqual(lines, [f"removed trash/{entry.name}"])

    def test_05_a_symlink_or_stray_file_in_trash_is_kept_and_never_followed(self) -> None:
        outside = self.base / "outside"
        outside.mkdir()
        (outside / "keep.txt").write_text("keep")
        (self.pipe / "trash").mkdir()
        (self.pipe / "trash/1-checkpoint-link").symlink_to(outside)
        (self.pipe / "trash/stray.txt").write_text("x")
        lines = collector.delete_trash(self.pipe / "trash")
        self.assertEqual(sum(line.startswith("refused") for line in lines), 2, lines)
        self.assertTrue((outside / "keep.txt").exists())


class Interruption(GcFixture):
    def test_01_killing_gc_between_the_rename_and_the_deletion_leaves_trash_the_next_run_finishes(
        self,
    ) -> None:
        for days in (60, 61, 62):
            self.old("docs", days)
        victim = self.old("docs", 63)
        ready = self.base / "ready"
        process = self.child(
            f"""
            import sys, time
            from pathlib import Path
            sys.path.insert(0, {str(TOOLS)!r})
            from gate import gc
            def hook(stage: str) -> None:
                if stage == "renamed":
                    Path({str(ready)!r}).write_text("x")
                    time.sleep(600)
            gc.HOOK = hook
            gc.command(Path({str(self.hub)!r}), apply=True)
            """
        )
        wait_until(ready.exists, "gc to rename and stop before deleting")
        process.send_signal(signal.SIGKILL)
        process.wait(timeout=BOUND)
        self.assertFalse(victim.exists())  # renamed out of checkpoints ...
        [entry] = [  # ... and left in trash
            path
            for path in (self.pipe / "trash").iterdir()
            if path.name.endswith(f"-checkpoint-{victim.name}")
        ]
        text = self.dry()
        self.assertTrue(self.line(text, str(entry)).startswith("CANDIDATE"), text)
        self.apply()
        self.assertFalse(entry.exists())


class Workers(GcFixture):
    """Worker copies are out of scope for GC: never listed, never touched."""

    def test_01_apply_and_the_dry_run_never_touch_a_worker_copy(self) -> None:
        worker = self.pipe / "workers/w1"
        (worker / "target").mkdir(parents=True)
        (worker / "target/artifact.bin").write_bytes(b"x" * 100)
        (worker / "notes.txt").write_text("unrecorded work\n")
        before = snapshot(worker)
        text = self.dry()
        self.assertNotIn(str(worker), text)
        self.assertNotIn("WORKER", text)
        self.apply()
        self.assertEqual(snapshot(worker), before)


class EntryPoint(GcFixture):
    """`tools/ck gc` through the real trampoline on a fixture hub whose pointer names a real
    checkpoint: the entry refusals (before any controller runs) and the controller's recovery
    conditions."""

    def setUp(self) -> None:
        super().setUp()
        shutil.rmtree(self.current)
        self.current.mkdir()
        copy_tools(self.current)
        self.late_swap_marker = self.pipe / "swap-pointer-after-lock"
        # Fixture-only code in this test's own copy of the controller (never in the real tools): once
        # the controller holds state.lock, and only if the marker exists, the pointer is replaced by
        # a bootstrap hold, as if another command had changed it after the trampoline read it.
        with (self.current / "tools/gate/gc.py").open("a") as stream:
            stream.write(
                textwrap.dedent(
                    f"""

                    def _late_swap(stage):
                        if stage == "locked" and Path({str(self.late_swap_marker)!r}).exists():
                            Path({str(self.pipe / "accepted-v2.json")!r}).write_text(
                                json.dumps({{"schema": "bootstrap-hold", "bootstrap": "late"}})
                            )


                    HOOK = _late_swap
                    """
                )
            )
        (self.current / ".ignore").write_text("/.ck-hub\n/target\n")
        pipeline.pijul(self.current, "init")
        pipeline.pijul(self.current, "add", "--recursive", ".")
        pipeline.pijul(self.current, "record", "-am", "checkpoint", "--author", "gc tests")
        self.publish(self.current)
        self.caller = self.base / "caller"
        (self.caller / "tools").mkdir(parents=True)
        shutil.copy2(TOOLS / "ck", self.caller / "tools/ck")
        (self.caller / ".ck-hub").write_text(f"{self.hub}\n")

    def publish(self, repo: Path) -> None:
        state = {
            "schema": 2,
            "repo": str(repo),
            "fingerprint": pipeline.fingerprint(repo),
            "submissions": [],
        }
        pointer.atomic_bytes(self.pipe / "accepted-v2.json", pointer.canonical(state))
        pointer.atomic_bytes(
            self.pipe / "accepted.json", pointer.canonical({"schema": "tombstone"})
        )

    def ck(self, *arguments: str) -> tuple[int, str, str]:
        code = (
            "import runpy, sys\n"
            f"sys.argv = ['ck', *{list(arguments)!r}]\n"
            f"runpy.run_path({str(self.caller / 'tools/ck')!r}, run_name='__main__')"
        )
        done = subprocess.run(
            [sys.executable, "-c", code],
            capture_output=True,
            text=True,
            check=False,
            timeout=120,
            env=clean_environment(CK_SIGNING_ACTIVE=str(self.hub)),
        )
        return done.returncode, done.stdout, done.stderr

    def database(self) -> Path:
        """The one file spec test 1 allows to change through the real entry point: the trampoline's
        own verification of the pointer's repository runs Pijul there (measured: `tools/ck status`
        does the same). GC's own code runs no Pijul command (`no_spawn`)."""
        return self.current / ".pijul/pristine/db"

    def everything(self, *, exclude: Iterable[Path] = ()) -> dict[str, Entry]:
        return super().everything(exclude=[self.database(), *exclude])

    def locks_only_by_holder(self, before: Mapping[str, Entry], after: Mapping[str, Entry]) -> None:
        """Everything is equal except the project lock files' bytes and mtime (same inode)."""
        lock_files = {str(self.pipe / "state.lock"), str(self.meta / ".lock")}
        self.assertEqual(set(before), set(after))
        for path in before:
            if path in lock_files:
                self.assertEqual(before[path][0], after[path][0], path)  # still a file
                self.assertEqual(before[path][2], after[path][2], f"{path} changed inode")
            else:
                self.assertEqual(before[path], after[path], path)

    def test_01_a_missing_pointer_a_hold_and_a_tombstone_refuse_before_any_controller_runs(
        self,
    ) -> None:
        cases: dict[str, tuple[Callable[[], object], str]] = {}
        hold = pointer.canonical({"schema": "bootstrap-hold", "bootstrap": "b1"})
        cases["hold"] = (
            lambda: pointer.atomic_bytes(self.pipe / "accepted-v2.json", hold),
            "bootstrap b1 in progress",
        )
        tombstone = pointer.canonical({"schema": "tombstone"})
        cases["tombstone"] = (
            lambda: pointer.atomic_bytes(self.pipe / "accepted-v2.json", tombstone),
            "activation incomplete",
        )

        def missing() -> None:
            (self.pipe / "accepted-v2.json").unlink()
            (self.pipe / "accepted.json").unlink()

        cases["missing"] = (missing, "accepted.json")
        for name, (arrange, message) in cases.items():
            for arguments in (("gc",), ("gc", "--apply")):
                with self.subTest(name, arguments=arguments):
                    arrange()
                    before = GcFixture.everything(self)  # exact: nothing at all may change
                    code, out, err = self.ck(*arguments)
                    self.assertNotEqual(code, 0, (out, err))
                    self.assertIn(message, err)
                    self.assertNotIn("gc dry run", out)  # no dry run is printed
                    self.assertEqual(out.strip(), "")
                    self.assertEqual(GcFixture.everything(self), before)
                    self.publish(self.current)
                    (self.pipe / "accepted.json").write_bytes(
                        pointer.canonical({"schema": "tombstone"})
                    )

    def conditions(self) -> dict[str, tuple[Callable[[], object], str]]:
        return {
            "publishing": (
                lambda: self.batch(self.now - 90 * DAY, "publishing"),
                "tools/ck batch publish",
            ),
            "bootstrap": (
                lambda: (self.pipe / "bootstrap/b9").mkdir(),
                "bootstrap publish recovery",
            ),
            "journal": (
                lambda: (
                    (self.pipe / "docs/journal/00000001.json").write_text(
                        '{"entries": ["D1"], "next": "", "prev": "", "seq": 1}'
                    ),
                    (self.pipe / "docs/D1.json").write_text('{"state": "pending"}'),
                ),
                "tools/ck docs flush",
            ),
            "unparseable": (
                lambda: (
                    self.batch(self.now - 91 * DAY),
                    (self.pipe / f"batches/{self.now - 91 * DAY}/published.json").write_text(
                        "{nope"
                    ),
                ),
                "does not parse",
            ),
            "wrong shape": (
                lambda: (
                    self.batch(self.now - 92 * DAY),
                    (self.pipe / f"batches/{self.now - 92 * DAY}/published.json").write_text("[]"),
                ),
                "unexpected shape",
            ),
        }

    def test_02_each_recovery_condition_refuses_apply_with_its_command_but_prints_the_dry_run(
        self,
    ) -> None:
        for _ in range(3):  # three superseded docs checkpoints that would otherwise be collected
            self.old("docs", 70 + _)
        victim = self.old("docs", 90)
        trash = self.pipe / "trash/1-checkpoint-docs-D1"
        (trash / "sub").mkdir(parents=True)
        (trash / "sub/file.bin").write_bytes(b"leftover")
        for name, (arrange, command) in self.conditions().items():
            with self.subTest(name):
                arrange()
                before = self.everything()
                code, out, err = self.ck("gc", "--apply")
                self.assertNotEqual(code, 0, (out, err))
                self.assertIn(command, err)
                self.assertIn("CANDIDATE", out)  # the dry run is still printed
                self.assertIn("RECOVERY CONDITION", out)
                self.assertTrue(victim.exists())
                self.assertEqual((trash / "sub/file.bin").read_bytes(), b"leftover")  # trash too
                self.locks_only_by_holder(before, self.everything())
                # run as a dry run, the same fixture leaves the exact snapshot, lock files included
                unchanged = self.everything()
                code, out, err = self.ck("gc")
                self.assertEqual(code, 0, (out, err))
                self.assertEqual(self.everything(), unchanged)
                self.assertIn("RECOVERY CONDITION", out)
                shutil.rmtree(self.pipe / "batches")
                (self.pipe / "batches").mkdir()
                shutil.rmtree(self.pipe / "bootstrap")
                (self.pipe / "bootstrap").mkdir()
                shutil.rmtree(self.pipe / "docs/journal")
                (self.pipe / "docs/journal").mkdir()
                (self.pipe / "docs/D1.json").unlink(missing_ok=True)

    def test_02a_a_valid_dry_run_through_the_real_entry_point_changes_only_the_trampolines_file(
        self,
    ) -> None:
        for days in (60, 61, 62):
            self.old("docs", days)
        self.old("docs", 63)
        before = self.everything()
        code, out, err = self.ck("gc")
        self.assertEqual(code, 0, (out, err))
        self.assertIn("gc itself deletes and writes nothing and runs no Pijul command", out)
        self.assertIn("may rewrite that checkpoint's .pijul/pristine/db", out)  # the r9 exception
        self.assertIn("CANDIDATE", out)
        self.assertEqual(self.everything(), before)  # every path, lock files included
        self.assertTrue(self.database().exists())

    def test_03a_through_the_real_entry_point_a_pointer_swapped_after_the_trampoline_read_refuses(
        self,
    ) -> None:
        # The specified hook: the swap happens inside the controller the trampoline started.
        victim = self.old("docs", 90)
        self.late_swap_marker.touch()
        code, out, err = self.ck("gc", "--apply")
        self.assertNotEqual(code, 0, (out, err))
        self.assertIn("bootstrap late in progress", out)  # the pointer state is the reason
        self.assertIn("pointer", err)
        self.assertTrue(victim.exists())
        self.assertFalse((self.pipe / "trash").exists())

    def test_03_a_pointer_swapped_after_the_trampoline_read_is_a_recovery_condition(self) -> None:
        # The defence in depth: the controller re-reads the pointer under state.lock.
        victim = self.old("docs", 90)

        def swap(stage: str) -> None:
            if stage == "locked":
                hold = pointer.canonical({"schema": "bootstrap-hold", "bootstrap": "late"})
                pointer.atomic_bytes(self.pipe / "accepted-v2.json", hold)

        with mock.patch.object(collector, "HOOK", swap):
            text, error = self.gc(apply=True)
        self.assertIsNotNone(error)
        self.assertIn("pointer", str(error))
        self.assertIn(
            "bootstrap late in progress", text
        )  # the pointer state is named as the reason
        self.assertTrue(victim.exists())
        self.assertFalse((self.pipe / "trash").exists())


class AfterCollection(test_batch.BatchGateSpecification):
    """After a full `--apply` the gate's flows still run: prepare, review, publish, docs, worker,
    sync, status; and a receipt whose candidate was collected loads and shows it as collected."""

    def populate_base(self) -> None:
        copy_tools(
            self.base
        )  # the batch candidates then contain tools/gate/lease.py: lease-capable

    def published_batch(self, name: str) -> tuple[batch.Manager, str]:
        self.submission(name)
        manager = batch.Manager(self.hub, self.passing)
        bid = manager.prepare()
        self.integration_receipt(bid, f"integration-{name}")
        manager.approve(bid)
        manager.publish(bid)
        return manager, bid

    def collect(self) -> str:
        """A full --apply, with the retention window and the newest-three rule out of the way."""
        buffer = io.StringIO()
        far = time.time_ns() + 400 * DAY
        with (
            mock.patch.object(collector, "_now", lambda: far),
            mock.patch.object(collector, "NEWEST", 0),
            contextlib.redirect_stdout(buffer),
        ):
            collector.command(self.hub, apply=True)
        return buffer.getvalue()

    def test_01_every_flow_still_works_after_a_full_apply(self) -> None:
        first_manager, first = self.published_batch("one")
        _manager, second = self.published_batch("two")
        checkpoints = self.hub / ".pipeline/checkpoints"
        collected = checkpoints / f"{first}-combined"
        current = Path(pointer.read_pointer(self.hub)["repo"])
        self.assertEqual(current, checkpoints / f"{second}-combined")
        self.assertTrue(collected.exists())
        receipt = batch.batch_path(self.hub, first) / "validated.json"
        before_receipt = receipt.read_bytes()

        text = self.collect()
        self.assertIn("moved to trash/", text)
        self.assertFalse(collected.exists(), "the superseded batch checkpoint was not collected")
        self.assertTrue(current.exists(), text)
        self.assertTrue(
            (current / "tools/gate/lease.py").exists(), text
        )  # the current one is untouched
        self.assertEqual(receipt.read_bytes(), before_receipt)  # no record changed

        # the receipt still loads, and the missing candidate shows as collected
        status = first_manager.status(first)
        self.assertEqual(status["collected"], {str(collected): "collected"})
        self.assertIn("published", status)

        # prepare (with its eligible scan over every capture), review and publish
        _manager, third = self.published_batch("three")
        self.assertIn("three", pointer.read_pointer(self.hub)["submissions"])
        self.assertIn("published", batch.Manager(self.hub, self.passing).status(third))

        # docs capture and flush, a worker, sync and status
        copy = Path(self.temp.name) / "docs-copy"
        pipeline.clone(Path(pointer.read_pointer(self.hub)["repo"]), copy, self.hub)
        (copy / "README.md").write_text("Docs after collection\n")
        did = docsqueue.Queue(self.hub).capture(copy, "Docs after collection", ["README.md"])
        row = json.loads((self.hub / ".pipeline/docs" / f"{did}.json").read_bytes())
        self.assertEqual(row["state"], "published")
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(cli.run(self.base, self.hub, ["worker", "after-gc"]), 0)
            worker = self.hub / ".pipeline/workers/after-gc"
            self.assertTrue((worker / "README.md").read_text().startswith("Docs after"))
            self.assertEqual(cli.run(worker, self.hub, ["status"]), 0)
            self.assertEqual(cli.run(worker, self.hub, ["sync"]), 0)


for _name in dir(test_batch.BatchGateSpecification):
    if _name.startswith("test"):  # the batch tests run once, in test_batch; this class adds its own
        setattr(AfterCollection, _name, None)
