#!/usr/bin/python3
"""Behavioral regressions; run through immutable-root.sh's disposable chroot."""

import errno
import http.server
import os
from pathlib import Path
import shutil
import stat
import subprocess
import threading
import unittest
from unittest import mock

from cloudinit import helpers, sources, stages, url_helper
from cloudinit.sources import DataSourceNoCloud

SEED = Path("/var/lib/cloud/seed/nocloud")
DISABLED = Path("/etc/cloud/cloud-init.disabled")
BAKED = b"#cloud-config\nruncmd: [echo baked]\n"
HOSTILE = b"#cloud-config\nruncmd: [echo host-supplied]\n"


def run(*args, **kwargs):
    return subprocess.run(
        args, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        timeout=30, **kwargs
    )


class CloudInitPolicyTests(unittest.TestCase):
    def setUp(self):
        self.reset_image()
        self.cfg = stages.Init().cfg

    def reset_image(self):
        for directory in (Path("/var/lib/cloud"), Path("/run/cloud-init")):
            if directory.exists():
                shutil.rmtree(directory)
        DISABLED.unlink(missing_ok=True)
        Path("/etc/confai").unlink(missing_ok=True)
        SEED.mkdir(parents=True)

    def seed(self, files=("user-data", "meta-data"), empty=False):
        contents = {
            "user-data": BAKED,
            "meta-data": b"instance-id: confos-sealed\nlocal-hostname: confos\n",
        }
        for name in files:
            (SEED / name).write_bytes(b"" if empty else contents[name])

    def test_only_nocloud_is_selected_despite_configuration_disks(self):
        # The real detector sees both cidata and config-2 via the blkid shim.
        self.seed()
        detected = run(
            "/usr/lib/cloud-init/ds-identify", "--force",
            env={**os.environ, "SYSTEMD_VIRTUALIZATION": "kvm"},
        )
        self.assertEqual(detected.returncode, 0, detected.stdout)
        cfg = stages.Init().cfg
        for deps, expected in (
            ([sources.DEP_FILESYSTEM], DataSourceNoCloud.DataSourceNoCloud),
            ([sources.DEP_FILESYSTEM, sources.DEP_NETWORK],
             DataSourceNoCloud.DataSourceNoCloudNet),
        ):
            with self.subTest(dependencies=deps):
                selected = sources.list_sources(
                    cfg["datasource_list"], deps, ["cloudinit.sources"]
                )
                self.assertEqual(selected, [expected])

    def test_dmi_urls_and_cidata_cannot_supply_payloads(self):
        requests = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                requests.append(self.path)
                body = {
                    "/user-data": HOSTILE,
                    "/meta-data": b"instance-id: host-supplied\n",
                    "/vendor-data": b"",
                    "/network-config": b"version: 2\n",
                }.get(self.path)
                self.send_response(200 if body is not None else 404)
                self.end_headers()
                if body is not None:
                    self.wfile.write(body)

            def log_message(self, *args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(thread.join)
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        url = f"http://127.0.0.1:{server.server_port}/"
        disk_payload = {
            "meta-data": "instance-id: host-disk\n",
            "user-data": HOSTILE,
        }

        for seeded in (False, True):
            self.reset_image()
            if seeded:
                self.seed()
            for serial in ("", f"ds=nocloud;s={url}", f"ds=nocloud-net;s={url}"):
                for cls in (
                    DataSourceNoCloud.DataSourceNoCloud,
                    DataSourceNoCloud.DataSourceNoCloudNet,
                ):
                    with self.subTest(seeded=seeded, serial=serial, source=cls):
                        requests.clear()
                        source = cls(self.cfg, mock.Mock(), helpers.Paths({}))
                        with (
                            mock.patch.object(
                                DataSourceNoCloud.dmi, "read_dmi_data",
                                return_value=serial,
                            ),
                            mock.patch.object(
                                DataSourceNoCloud.util, "find_devs_with",
                                return_value=["/dev/host-cidata"],
                            ) as devices,
                            mock.patch.object(
                                DataSourceNoCloud.util, "mount_cb",
                                return_value=disk_payload,
                            ) as disk_mount,
                        ):
                            try:
                                found = source._get_data()
                            except url_helper.UrlError:
                                # A missing pinned local seed is rejected by
                                # cloud-init's datasource finder.
                                if seeded:
                                    raise
                                found = False
                        expected = seeded and cls is DataSourceNoCloud.DataSourceNoCloud
                        self.assertEqual(found, expected)
                        if found:
                            self.assertEqual(source.userdata_raw, BAKED)
                        else:
                            self.assertIsNone(source.userdata_raw)
                        self.assertEqual(requests, [], "host URL was fetched")
                        devices.assert_not_called()
                        disk_mount.assert_not_called()

    def test_finalization_controls_activation_and_preserves_disable_marker(self):
        # DMI and disks remain hostile for every activation case.
        Path("/sys/class/dmi/id/product_serial").write_text(
            "ds=nocloud;s=http://127.0.0.1:9/\n"
        )
        cases = (
            ("no-seed", (), False, False, False),
            ("user-data-only", ("user-data",), False, False, False),
            ("meta-data-only", ("meta-data",), False, False, False),
            ("complete", ("user-data", "meta-data"), False, False, True),
            ("empty-files", ("user-data", "meta-data"), True, False, True),
            ("profile-disabled", ("user-data", "meta-data"), False, True, False),
        )
        for name, files, empty, disabled, enabled in cases:
            with self.subTest(case=name):
                self.reset_image()
                self.seed(files, empty)
                if disabled:
                    DISABLED.write_text("disabled by downstream profile\n")
                finalized = run(
                    "/bin/bash", "/finalize-under-test",
                    env={**os.environ, "BUILDROOT": "/"},
                )
                self.assertEqual(finalized.returncode, 0, finalized.stdout)
                self.assertEqual(DISABLED.exists(), not enabled)
                if disabled:
                    self.assertEqual(
                        DISABLED.read_text(), "disabled by downstream profile\n"
                    )
                detected = run(
                    "/usr/lib/cloud-init/ds-identify", "--force",
                    env={**os.environ, "SYSTEMD_VIRTUALIZATION": "kvm"},
                )
                self.assertEqual(
                    detected.returncode, 0 if enabled else 2, detected.stdout
                )


class WritableStateTests(unittest.TestCase):
    def test_colliding_profile_paths_have_independent_writable_state(self):
        image = Path("/image")
        state = image / "usr/lib/confai/state.d"
        shutil.copytree("/default-state.d", state)
        (state / "50-one.conf").write_text("opt/a-b\n")
        # A duplicate across files, CRLF, and a final unterminated line.
        (state / "60-two.conf").write_bytes(b"opt/a/b\r\nopt/a-b")
        directories = ("var", "home", "root", "tmp", "opt/a-b", "opt/a/b")
        for directory in (*directories, "run", "etc"):
            (image / directory).mkdir(parents=True, exist_ok=True)
        (image / "root").chmod(0o700)
        (image / "tmp").chmod(0o1777)
        (image / "opt/a-b").chmod(0o750)
        os.chown(image / "opt/a-b", 123, 456)

        booted = run("/bin/bash", "/init-under-test")
        self.assertEqual(booted.returncode, 0, booted.stdout)
        self.assertEqual(
            Path("/switch-root-args").read_text(), "/sysroot\n/sbin/init\n"
        )
        mounts = [
            line.split()[-1] for line in Path("/mount-calls").read_text().splitlines()
            if line.startswith("-t overlay overlay ")
        ]
        self.assertCountEqual(mounts, [f"/sysroot/{d}" for d in directories])
        first, second = Path("/sysroot/opt/a-b"), Path("/sysroot/opt/a/b")
        (first / "first-marker").write_text("first")
        self.assertFalse((second / "first-marker").exists())
        (second / "second-marker").write_text("second")
        self.assertFalse((first / "second-marker").exists())
        self.assertFalse((image / "opt/a-b/first-marker").exists())
        self.assertFalse((image / "opt/a/b/second-marker").exists())
        for directory in ("root", "tmp", "opt/a-b"):
            lower = (image / directory).stat()
            merged = Path(f"/sysroot/{directory}").stat()
            self.assertEqual(stat.S_IMODE(merged.st_mode), stat.S_IMODE(lower.st_mode))
            self.assertEqual((merged.st_uid, merged.st_gid), (lower.st_uid, lower.st_gid))
        with self.assertRaises(OSError) as denied:
            Path("/sysroot/etc/undeclared").write_text("must remain read-only")
        self.assertEqual(denied.exception.errno, errno.EROFS)


if __name__ == "__main__":
    if not Path("/confos-test-root").is_file():
        raise SystemExit("Run tests/immutable-root.sh; these tests need its isolated root")
    unittest.main(verbosity=2)
