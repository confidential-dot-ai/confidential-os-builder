#!/usr/bin/env python3
"""Exercise unsigned-module caching without a GPU or private signing key."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
LIB = ROOT / "bin/lib/gpu-module-cache.sh"


class CacheTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.source = self.root / "source"
        self.source.mkdir()
        for module in ("nvidia.ko", "nvidia-uvm.ko"):
            (self.source / module).write_bytes(b"unsigned-module-" + module.encode())
        self.env = {**os.environ, "CACHE": str(self.root / "cache"), "SOURCE": str(self.source), "DEST": str(self.root / "destination")}

    def shell(self, script):
        return subprocess.run(["bash", "-c", 'set -euo pipefail; source "$1"; ' + script, "test", str(LIB)], env=self.env, text=True, capture_output=True, timeout=30)

    def store(self):
        result = self.shell('gpu_module_cache_store "$CACHE" "$SOURCE" nvidia.ko nvidia-uvm.ko')
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_hit_copies_unsigned_bytes_without_caching_signing_mutations(self):
        self.store()
        result = self.shell('gpu_module_cache_restore "$CACHE" "$DEST" nvidia.ko nvidia-uvm.ko')
        self.assertEqual(result.returncode, 0, result.stderr)
        dest = self.root / "destination/nvidia.ko"
        dest.write_bytes(dest.read_bytes() + b"signature")
        cached = self.root / "cache/nvidia.ko"
        self.assertEqual(cached.read_bytes(), (self.source / "nvidia.ko").read_bytes())
        self.assertNotIn(b"signature", cached.read_bytes())

    def test_corrupt_or_partial_entry_is_a_miss_without_partial_restore(self):
        self.store()
        (self.root / "cache/nvidia-uvm.ko").write_bytes(b"corrupt")
        result = self.shell('gpu_module_cache_restore "$CACHE" "$DEST" nvidia.ko nvidia-uvm.ko')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "destination").exists())
        (self.root / "cache/nvidia.ko.sha256").unlink()
        self.assertNotEqual(self.shell('gpu_module_cache_restore "$CACHE" "$DEST" nvidia.ko nvidia-uvm.ko').returncode, 0)

    def test_input_change_invalidates_key(self):
        command = 'gpu_module_cache_key "$SOURCE/nvidia.ko" "$SOURCE/nvidia-uvm.ko"'
        first = self.shell(command)
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(first.stdout, self.shell(command).stdout)
        (self.source / "nvidia.ko").write_bytes(b"new-input")
        self.assertNotEqual(first.stdout, self.shell(command).stdout)


if __name__ == "__main__":
    unittest.main()
