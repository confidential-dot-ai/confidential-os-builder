#!/usr/bin/python3
"""Behavioral regressions; run through immutable-root.sh's disposable chroot."""

import errno
import http.server
import os
from pathlib import Path
import shutil
import socket
import stat
import subprocess
import tempfile
import threading
import unittest
from unittest import mock

from cloudinit import distros, helpers, sources, stages, url_helper
from cloudinit.config import cc_locale
from cloudinit.distros import debian
from cloudinit.sources import DataSourceGCE, DataSourceNoCloud

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

    def test_image_locale_lets_the_locale_module_run_without_changes(self):
        # Ubuntu links /etc/default/locale to the image's /etc/locale.conf,
        # and cloud-init parses it as shell assignments. A bare value there
        # made the module raise, so every cloud-init boot ended in error.
        # /etc is read-only on the booted image: nothing may be regenerated.
        distro = distros.fetch("ubuntu")("ubuntu", {}, helpers.Paths({}))
        cloud = mock.Mock(distro=distro, get_locale=distro.get_locale)
        with mock.patch.object(debian.subp, "subp") as subp:
            cc_locale.handle("locale", self.cfg, cloud, [])
        subp.assert_not_called()
        self.assertEqual(distro.get_locale(), "C.UTF-8")

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


class GceDatasourceTests(unittest.TestCase):
    def test_gcp_profile_reaches_metadata_without_name_resolution(self):
        # cloud-init's GCE datasource skips a NIC when the metadata URL does
        # not resolve, which is always the case in init-local.
        profile = Path("/etc/cloud/cloud.cfg.d/99-zz-gcp.cfg")
        no_dns = mock.patch.object(socket, "getaddrinfo", side_effect=socket.gaierror("no resolver"))

        def metadata_url():
            distro = distros.fetch("ubuntu")("ubuntu", {}, helpers.Paths({}))
            source = DataSourceGCE.DataSourceGCELocal(stages.Init().cfg, distro, helpers.Paths({}))
            return source.metadata_address

        with no_dns:
            self.assertFalse(DataSourceGCE.util.is_resolvable_url(metadata_url()))
        shutil.copy("/gcp.cfg", profile)
        self.addCleanup(profile.unlink)
        with no_dns:
            self.assertTrue(DataSourceGCE.util.is_resolvable_url(metadata_url()))
        self.assertTrue(metadata_url().endswith("/computeMetadata/v1/"))


# The harness disk layout (tests/immutable-root.sh): the real partition UUIDs
# systemd-repart wrote for roothash 58ada214... (Azure image 0.1.2), on NVMe.
ROOTHASH = "58ada21469e0c4fe0cf5f9f77e4295f82d510b566286aa0b09a4c044faf842ba"
_booted = None


def boot():
    """Run the init under test once against the fixture image; tests share it."""
    global _booted
    if _booted is None:
        image = Path("/image")
        state = image / "usr/lib/confai/state.d"
        shutil.copytree("/default-state.d", state)
        (state / "50-one.conf").write_text("opt/a-b\n")
        # A duplicate across files, CRLF, and a final unterminated line.
        (state / "60-two.conf").write_bytes(b"opt/a/b\r\nopt/a-b")
        for directory in ("var", "home", "root", "tmp", "opt/a-b", "opt/a/b", "run", "etc"):
            (image / directory).mkdir(parents=True, exist_ok=True)
        (image / "root").chmod(0o700)
        (image / "tmp").chmod(0o1777)
        (image / "opt/a-b").chmod(0o750)
        os.chown(image / "opt/a-b", 123, 456)
        _booted = run("/bin/bash", "/init-under-test")
    return _booted


# Sysfs paths of disks as the clouds present them (Azure DC2es_v6 and GCE
# c3-standard-4, 2026-10-12). Azure: OS disk controller ...781a, data disk
# controller ...781b; the device link ends in the SCSI address H:C:T:L.
AZURE_VMBUS = "LNXSYSTM:00/LNXSYBUS:00/ACPI0004:00/MSFT1000:00"
AZURE_OS_LUN0 = f"{AZURE_VMBUS}/f8b3781a-1e82-4818-a1c3-63d806ec15bb/host0/target0:0:0/0:0:0:0"
AZURE_DATA = f"{AZURE_VMBUS}/f8b3781b-1e82-4818-a1c3-63d806ec15bb/host1/target1:0:0"
GCE_NVME = "pci0000:00/0000:00:04.0/nvme/nvme0"
RESULTS = ("/cryptsetup-args", "/mount-calls", "/switch-root-args", "/veritysetup-args")


def boot_with_disks(disks):
    """Run the init again with /sys/block holding only the root disk and DISKS.

    DISKS maps a block device name to (sysfs device path or None, serial or
    None). The boot runs in a private mount namespace with its own /sys, and
    the shared boot()'s result files are restored afterwards. Returns the
    init output and the cryptsetup arguments (None when nothing was opened).
    """
    boot()
    saved = {path: Path(path).read_bytes() for path in RESULTS if Path(path).exists()}
    Path("/cryptsetup-args").unlink()
    sysfs = Path(tempfile.mkdtemp(dir="/tmp"))
    try:
        (sysfs / "block/nvme0n1/nvme0n1p2").mkdir(parents=True)
        (sysfs / "class/block").mkdir(parents=True)
        (sysfs / "class/block/nvme0n1p2").symlink_to("../../block/nvme0n1/nvme0n1p2")
        for minor, (name, (device, serial)) in enumerate(disks.items(), start=32):
            block = sysfs / "block" / name
            block.mkdir()
            if serial:
                (block / "serial").write_text(serial + "\n")
            if device:
                (sysfs / "devices" / device).mkdir(parents=True)
                (block / "device").symlink_to(os.path.relpath(sysfs / "devices" / device, block))
            if not Path("/dev", name).exists():
                os.mknod(f"/dev/{name}", stat.S_IFBLK | 0o600, os.makedev(8, minor))
        booted = run(
            "unshare", "--mount", "--propagation", "private", "/bin/bash", "-c",
            f"mount --bind {sysfs} /sys && exec /bin/bash /init-under-test",
        )
        opened = Path("/cryptsetup-args")
        return booted, opened.read_text().split() if opened.exists() else None
    finally:
        shutil.rmtree(sysfs)
        for path in RESULTS:
            Path(path).unlink(missing_ok=True)
        for path, content in saved.items():
            Path(path).write_bytes(content)


class DiskDiscoveryTests(unittest.TestCase):
    def test_verity_partitions_are_found_by_roothash_partuuid(self):
        booted = boot()
        self.assertEqual(booted.returncode, 0, booted.stdout)
        self.assertEqual(
            Path("/veritysetup-args").read_text().split(),
            ["open", "/dev/nvme0n1p2", "root", "/dev/nvme0n1p3", ROOTHASH],
        )

    def test_scratch_disk_is_found_by_serial_when_root_is_not_virtio(self):
        booted = boot()
        self.assertEqual(booted.returncode, 0, booted.stdout)
        self.assertIn("/dev/vdb", Path("/cryptsetup-args").read_text().split())

    def test_azure_data_disk_at_lun_0_is_the_scratch_disk(self):
        booted, opened = boot_with_disks({
            "sda": (AZURE_OS_LUN0, None),
            "sdb": (f"{AZURE_DATA}/1:0:0:1", None),
            "sdc": (f"{AZURE_DATA}/1:0:0:0", None),
        })
        self.assertEqual(booted.returncode, 0, booted.stdout)
        self.assertIn("initrd: scratch disk /dev/sdc (Azure data disk LUN 0)", booted.stdout)
        self.assertIn("/dev/sdc", opened)
        self.assertIn("plain", opened)

    def test_other_cloud_disks_leave_state_in_tmpfs(self):
        # The Azure OS-disk controller's LUN 0, another data LUN and a GCE NVMe
        # data disk (no serial, device name only in NVMe Identify data).
        booted, opened = boot_with_disks({
            "nvme0n2": (GCE_NVME, None),
            "sda": (AZURE_OS_LUN0, None),
            "sdb": (f"{AZURE_DATA}/1:0:0:10", None),
        })
        self.assertEqual(booted.returncode, 0, booted.stdout)
        self.assertIsNone(opened)
        self.assertIn("no scratch disk found", booted.stdout)


class WritableStateTests(unittest.TestCase):
    def test_colliding_profile_paths_have_independent_writable_state(self):
        image = Path("/image")
        directories = ("var", "home", "root", "tmp", "opt/a-b", "opt/a/b")
        booted = boot()
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
