#!/usr/bin/env python3
"""bin/cloud-image packaging and bin/cloud-upload-azure against a local page-blob server."""

import gzip
import http.server
import os
from pathlib import Path
import struct
import subprocess
import tarfile
import tempfile
import threading
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


class PageBlobServer(http.server.BaseHTTPRequestHandler):
    def do_PUT(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        self.server.requests.append((self.path, self.headers, body))
        self.send_response(201)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, *args):
        pass


class UploadTests(unittest.TestCase):
    def test_blob_equals_the_file_and_zero_chunks_are_skipped(self):
        chunk = 4 * MIB
        data = bytearray(2 * chunk + 1024)
        data[10:14] = b"head"
        data[2 * chunk + 5:2 * chunk + 9] = b"tail"
        server = http.server.HTTPServer(("127.0.0.1", 0), PageBlobServer)
        server.requests = []
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        with tempfile.NamedTemporaryFile() as vhd:
            vhd.write(data)
            vhd.flush()
            url = f"http://127.0.0.1:{server.server_port}/c/x.vhd?sig=s"
            subprocess.run([str(ROOT / "bin/cloud-upload-azure"), url, vhd.name], check=True, capture_output=True)
        create, *pages = server.requests
        self.assertEqual(create[0], "/c/x.vhd?sig=s")
        self.assertEqual(create[1]["x-ms-blob-type"], "PageBlob")
        self.assertEqual(int(create[1]["x-ms-blob-content-length"]), len(data))
        blob = bytearray(len(data))
        for path, headers, body in pages:
            self.assertEqual(path, "/c/x.vhd?sig=s&comp=page")
            start, end = map(int, headers["x-ms-range"].removeprefix("bytes=").split("-"))
            self.assertEqual(end - start + 1, len(body))
            blob[start:end + 1] = body
        self.assertEqual(blob, data)
        self.assertEqual(len(pages), 2, "the all-zero middle chunk must be skipped")

    def test_refuses_a_file_that_is_not_whole_sectors(self):
        with tempfile.NamedTemporaryFile() as vhd:
            vhd.write(b"x" * 1000)
            vhd.flush()
            result = subprocess.run([str(ROOT / "bin/cloud-upload-azure"), "http://127.0.0.1:9/x?s", vhd.name],
                                    capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
