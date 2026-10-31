"""S9: file hashing in fingerprints is streamed in bounded chunks and byte-identical to the old digests."""

import hashlib
import importlib.machinery
import importlib.util
import os
import subprocess
import sys
import tempfile
import tracemalloc
import types
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import pipeline

CHUNK = 1 << 20


def load_trampoline() -> types.ModuleType:
    path = Path(__file__).resolve().parents[1] / "tools/ck"
    loader = importlib.machinery.SourceFileLoader("ck_trampoline_hashing", str(path))
    spec = importlib.util.spec_from_loader("ck_trampoline_hashing", loader)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


class StreamingHash(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="ck-hashing-")
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(os.path.realpath(self.temp.name)) / "repo"
        self.repo.mkdir()
        subprocess.run(["pijul", "init"], cwd=self.repo, check=True, capture_output=True)
        self.ck = load_trampoline()

    def add(
        self,
        name: str,
        data: bytes | None = None,
        *,
        size: int | None = None,
        mode: int | None = None,
    ) -> Path:
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        if size is not None:
            with path.open("wb") as stream:
                stream.truncate(size)  # a sparse file: no disk, reads as zeros
        else:
            assert data is not None
            path.write_bytes(data)
        if mode:
            path.chmod(mode)
        subprocess.run(["pijul", "add", name], cwd=self.repo, check=True, capture_output=True)
        return path

    def expected(self, name: str) -> dict[str, object]:
        path = self.repo / name
        return {
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "executable": bool(path.stat().st_mode & 0o111),
        }

    def test_01_digests_match_the_old_implementation_at_every_chunk_boundary(self) -> None:
        sizes = {
            "empty": b"",
            "small": b"a small file\n",
            "one-byte-short": os.urandom(CHUNK - 1),
            "exactly-one-chunk": os.urandom(CHUNK),
            "one-byte-over": os.urandom(CHUNK + 1),
            "several-chunks": os.urandom(3 * CHUNK + 17),
        }
        for name, data in sizes.items():
            self.add(name, data)
        self.add("tool.sh", b"#!/bin/sh\n", mode=0o755)
        found = pipeline.files(self.repo)
        mapped = self.ck.file_map(self.repo)
        for name in [*sizes, "tool.sh"]:
            with self.subTest(name):
                self.assertEqual(found[name], self.expected(name))
                self.assertEqual(mapped[name], self.expected(name))
        self.assertTrue(found["tool.sh"]["executable"])
        self.assertFalse(found["small"]["executable"])

    def test_02_peak_memory_is_bounded_for_a_large_file(self) -> None:
        self.add("big.bin", size=256 * 1024 * 1024)
        for label, hash_files in (
            ("pipeline.files", lambda: pipeline.files(self.repo)),
            ("ck file_map", lambda: self.ck.file_map(self.repo)),
        ):
            with self.subTest(label):
                tracemalloc.start()
                try:
                    result = hash_files()
                    _, peak = tracemalloc.get_traced_memory()
                finally:
                    tracemalloc.stop()
                self.assertLess(peak, 8 * 1024 * 1024, f"{label} peaked at {peak} bytes")
                digest = hashlib.sha256()
                zeros = bytes(CHUNK)
                for _ in range(256):
                    digest.update(zeros)
                self.assertEqual(result["big.bin"]["sha256"], digest.hexdigest())

    def test_03_the_trampoline_file_map_and_the_fingerprint_agree(self) -> None:
        self.add("a.txt", b"alpha\n")
        self.add("dir/b.bin", os.urandom(CHUNK + 5))
        self.add("dir/run.sh", b"#!/bin/sh\n", mode=0o750)
        self.assertEqual(self.ck.file_map(self.repo), pipeline.files(self.repo))
        self.assertEqual(self.ck.file_map(self.repo), pipeline.fingerprint(self.repo)["files"])

    def test_04_symlinks_are_still_refused(self) -> None:
        # Pijul does not list a symlink it cannot track, so the listing is stubbed to name one.
        (self.repo / "real.txt").write_text("x\n")
        (self.repo / "link.txt").symlink_to("real.txt")
        with (
            mock.patch.object(pipeline, "pijul", return_value="link.txt\n"),
            self.assertRaisesRegex(RuntimeError, "Symlinks are not supported"),
        ):
            pipeline.files(self.repo)
        with (
            mock.patch.object(self.ck.subprocess, "check_output", return_value="link.txt\n"),
            self.assertRaisesRegex(RuntimeError, "Symlinks are not supported"),
        ):
            self.ck.file_map(self.repo)


if __name__ == "__main__":
    unittest.main()
