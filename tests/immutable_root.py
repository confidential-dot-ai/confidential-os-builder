#!/usr/bin/python3
"""Behavioral regressions; run through immutable-root.sh's disposable chroot."""

import errno
import hashlib
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


TDX_MEASUREMENTS = Path("/sys/devices/virtual/misc/tdx_guest/measurements")
RTMR3 = TDX_MEASUREMENTS / "rtmr3:sha384"
SEV_GUEST = Path("/dev/sev-guest")
DEV_NULL = os.makedev(1, 3)
DEV_FULL = os.makedev(1, 7)
TSM_REPORT = Path("/sys/kernel/config/tsm/report/opkey")


def clear_platform():
    shutil.rmtree("/sys/devices", ignore_errors=True)
    shutil.rmtree("/sys/kernel", ignore_errors=True)
    SEV_GUEST.unlink(missing_ok=True)


def launch_tdx():
    """Present a TDX guest's measurement sysfs without an rtmr3 node."""
    clear_platform()
    TDX_MEASUREMENTS.mkdir(parents=True)


def snp_report(host_data):
    """An SNP attestation report: HOSTDATA at offset 192 of 1184 bytes."""
    return bytes(192) + host_data + bytes(1184 - 192 - len(host_data))


def launch_snp(host_data):
    """Present an SNP guest whose next ConfigFS-TSM report carries host_data."""
    clear_platform()
    os.mknod(SEV_GUEST, stat.S_IFCHR | 0o600, DEV_NULL)
    TSM_REPORT.mkdir(parents=True)
    (TSM_REPORT / "outblob").write_bytes(snp_report(host_data))


def make_image():
    shutil.rmtree("/image", ignore_errors=True)
    image = Path("/image")
    shutil.copytree("/default-state.d", image / "usr/lib/confai/state.d")
    for directory in ("var", "home", "root", "tmp", "run", "etc"):
        (image / directory).mkdir(parents=True)


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


class ScratchDiscoveryTests(unittest.TestCase):
    """The host picks how many disks it attaches and in what order."""

    CRYPTSETUP = Path("/cryptsetup-calls")
    MOUNTS = Path("/mount-calls")

    def setUp(self):
        make_image()
        self.CRYPTSETUP.write_text("")
        self.MOUNTS.unlink(missing_ok=True)

    def tearDown(self):
        shutil.rmtree("/image")
        shutil.rmtree("/sys/block")
        self.CRYPTSETUP.unlink()
        self.MOUNTS.unlink(missing_ok=True)

    def attach(self, disks):
        """Publish virtio disks in sysfs the way the kernel does."""
        for name, serial in disks.items():
            sysfs = Path("/sys/block", name)
            sysfs.mkdir(parents=True)
            if serial is not None:
                (sysfs / "serial").write_text(serial)

    def boot(self):
        # The initrd's mounts stay in their own namespace, so each boot starts
        # from the fixture rather than the previous boot's overlays.
        booted = run("/usr/bin/unshare", "--mount", "--propagation", "private",
                     "/bin/bash", "/init-under-test")
        self.assertEqual(booted.returncode, 0, booted.stdout)

    def test_scratch_is_found_after_the_other_launch_disks(self):
        self.attach({"vda": None, "vdb": None, "vdc": "confai-models",
                     "vdd": "", "vde": "confai-scratch"})
        self.boot()
        self.assertIn("/dev/vde scratch", self.CRYPTSETUP.read_text())
        self.assertIn("/dev/mapper/scratch /state", self.MOUNTS.read_text())

    def test_the_image_disk_is_never_taken_for_scratch(self):
        self.attach({"vda": "confai-scratch", "vdb": "confai-models"})
        self.boot()
        self.assertEqual(self.CRYPTSETUP.read_text(), "")
        self.assertIn("-t tmpfs tmpfs /state", self.MOUNTS.read_text())


class LaunchBindingTests(unittest.TestCase):
    """The host picks the opkeydata disk and SNP HOSTDATA; the guest binds the key or powers off."""

    PUBKEY = b"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOperatorKeyBytesForTheTest operator\n"
    DISK = Path("/opkey-disk")
    SYSRQ = Path("/proc/sysrq-trigger")
    SWITCH_ROOT = Path("/switch-root-args")
    STAGED = Path("/switch-root-pubkey")
    FATAL = "FATAL: launch binding failed: "

    def setUp(self):
        make_image()

    def tearDown(self):
        shutil.rmtree("/image")
        shutil.rmtree(self.DISK, ignore_errors=True)
        clear_platform()

    def attach_key(self):
        shutil.rmtree(self.DISK, ignore_errors=True)
        self.DISK.mkdir()
        (self.DISK / "pubkey").write_bytes(self.PUBKEY)

    def boot(self):
        self.SYSRQ.write_text("")
        self.SWITCH_ROOT.unlink(missing_ok=True)
        self.STAGED.unlink(missing_ok=True)
        return run("/usr/bin/unshare", "--mount", "--propagation", "private",
                   "/bin/bash", "/init-under-test")

    def assert_boots(self, staged):
        booted = self.boot()
        self.assertEqual(booted.returncode, 0, booted.stdout)
        self.assertNotIn("FATAL", booted.stdout)
        self.assertEqual(self.SYSRQ.read_text(), "")
        self.assertEqual(self.SWITCH_ROOT.read_text(), "/sysroot\n/sbin/init\n")
        if staged is None:
            self.assertFalse(self.STAGED.exists())
        else:
            self.assertEqual(self.STAGED.read_bytes(), staged)

    def assert_powers_off(self):
        booted = self.boot()
        self.assertNotEqual(booted.returncode, 0, booted.stdout)
        self.assertIn(self.FATAL, booted.stdout)
        self.assertEqual(self.SYSRQ.read_text(), "o\n")
        self.assertFalse(self.SWITCH_ROOT.exists(), booted.stdout)

    def record_extends(self):
        """Make rtmr3 a FIFO; return a reader of every byte written to it."""
        os.mkfifo(RTMR3)
        # Holding both ends keeps the init's open from blocking and its writes queued.
        fd = os.open(RTMR3, os.O_RDWR | os.O_NONBLOCK)
        self.addCleanup(os.close, fd)

        def extends():
            try:
                return os.read(fd, 4096)
            except BlockingIOError:
                return b""
        return extends

    def test_tdx_key_disk_extends_rtmr3_once_with_the_staged_key_digest(self):
        launch_tdx()
        extends = self.record_extends()
        self.attach_key()
        self.assert_boots(staged=self.PUBKEY)
        self.assertEqual(extends(), hashlib.sha384(self.PUBKEY).digest())

    def test_tdx_without_key_disk_boots_unbound_without_extending(self):
        launch_tdx()
        extends = self.record_extends()
        self.assert_boots(staged=None)
        self.assertEqual(extends(), b"")

    def test_tdx_rtmr3_rejecting_the_extend_powers_off(self):
        launch_tdx()
        # Writes to /dev/full fail with ENOSPC.
        os.mknod(RTMR3, stat.S_IFCHR | 0o600, DEV_FULL)
        self.attach_key()
        self.assert_powers_off()

    def test_snp_key_matching_hostdata_is_staged(self):
        launch_snp(hashlib.sha256(self.PUBKEY).digest())
        self.attach_key()
        self.assert_boots(staged=self.PUBKEY)

    def test_snp_key_not_matching_hostdata_powers_off(self):
        self.attach_key()
        for name, host_data in (("other key", hashlib.sha256(b"other key").digest()),
                                ("zero", bytes(32))):
            with self.subTest(host_data=name):
                launch_snp(host_data)
                self.assert_powers_off()

    def test_snp_without_key_disk_boots_unbound_only_with_all_zero_hostdata(self):
        with self.subTest(host_data="zero"):
            launch_snp(bytes(32))
            self.assert_boots(staged=None)
        with self.subTest(host_data="key digest"):
            launch_snp(hashlib.sha256(self.PUBKEY).digest())
            self.assert_powers_off()

    def test_outside_a_tee_boots_unbound_only_without_key_disk(self):
        clear_platform()
        with self.subTest(key_disk=False):
            self.assert_boots(staged=None)
        with self.subTest(key_disk=True):
            self.attach_key()
            self.assert_powers_off()


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
