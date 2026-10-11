#!/usr/bin/env python3
"""bin/cloud-image packaging."""

import gzip
import os
from pathlib import Path
import struct
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
MIB = 1 << 20


def write_disk(directory, size):
    disk = bytearray(size)
    disk[:9] = b"confos-gpt"[:9]
    disk[size // 2:size // 2 + 4] = b"data"
    disk[-3:] = b"end"
    (directory / "disk.raw").write_bytes(disk)
    return bytes(disk)


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.out = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def package(self, cloud):
        subprocess.run([str(ROOT / "bin/cloud-image"), cloud, str(self.out)], check=True, capture_output=True)

    def test_gce_tarball_holds_only_disk_raw_in_oldgnu_format(self):
        disk = write_disk(self.out, 3 * MIB)
        self.package("gcp")
        archive = self.out / "cloud/confos-gcp.tar.gz"
        raw = gzip.decompress(archive.read_bytes())
        self.assertEqual(raw[257:265], b"ustar  \0")  # GNU (oldgnu) header magic
        with tarfile.open(archive) as tar:
            self.assertEqual(tar.getnames(), ["disk.raw"])
            self.assertEqual(tar.extractfile("disk.raw").read(), disk)

    def test_azure_vhd_is_fixed_mib_aligned_with_a_valid_footer(self):
        disk = write_disk(self.out, MIB + 1000)
        self.package("azure")
        vhd = (self.out / "cloud/confos-azure.vhd").read_bytes()
        size = 2 * MIB
        self.assertEqual(len(vhd), size + 512)
        self.assertEqual(vhd[:len(disk)], disk)
        self.assertEqual(vhd[len(disk):size].count(0), size - len(disk))
        footer = vhd[size:]
        cookie, _, version, data_offset = struct.unpack_from(">8sIIQ", footer)
        original, current = struct.unpack_from(">QQ", footer, 40)
        disk_type, checksum = struct.unpack_from(">II", footer, 60)
        self.assertEqual((cookie, version, data_offset), (b"conectix", 0x00010000, 0xFFFFFFFFFFFFFFFF))
        self.assertEqual((original, current, disk_type), (size, size, 2))  # 2 = fixed
        self.assertEqual(checksum, ~(sum(footer) - sum(footer[64:68])) & 0xFFFFFFFF)
        listed = (self.out / "cloud/confos-azure.vhd.sha256").read_text().split()
        self.assertEqual(listed[1], "confos-azure.vhd")


if __name__ == "__main__":
    unittest.main(verbosity=2)
