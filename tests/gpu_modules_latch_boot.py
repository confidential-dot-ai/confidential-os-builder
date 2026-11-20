#!/usr/bin/env python3
"""Boot systemd with the gpu profile's latch unit and drop-ins around stubs.

Checks what the unit text cannot show: that ssh, containerd and docker wait
for the modules latch, do not start without it, and that a failed latch
powers the guest off. It runs systemd as PID 1 in a container, with stub
services writing marks in start order.

The latch's own ExecStart never runs here. Writing kernel.modules_disabled
would latch the host kernel until reboot, so every case replaces the command,
and the container is not privileged, which keeps /proc/sys read-only. The
real sysctl write and real GPU bring-up belong to hardware acceptance.
"""

from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
UNITS = ROOT / "mkosi/base/mkosi.profiles/gpu/mkosi.extra/etc/systemd/system"
LATCH = "nvidia-modules-latch.service"
PROTECTED = ("ssh", "containerd", "docker")
IMAGE = "confos-gpu-latch-test"
# ubuntu:26.04, the release the images are built from.
BASE = "ubuntu@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db"
DOCKERFILE = f"""FROM {BASE}
RUN apt-get update -qq \\
 && apt-get install -y -qq --no-install-recommends systemd >/dev/null \\
 && rm -rf /var/lib/apt/lists/*
"""
# systemd needs its own cgroup subtree and unfiltered mount syscalls to boot.
# SYS_BOOT lets a poweroff end the container. None of this makes /proc/sys
# writable.
RUN_FLAGS = [
    "--cap-add", "SYS_ADMIN", "--cap-add", "SYS_BOOT",
    "--security-opt", "seccomp=unconfined", "--security-opt", "apparmor=unconfined",
    "--cgroupns=host", "-v", "/sys/fs/cgroup:/sys/fs/cgroup:rw",
    "--tmpfs", "/run", "--tmpfs", "/run/lock", "--tmpfs", "/tmp",
]
BRING_UP_SECONDS = 3
FINISH_SECONDS = 8


def mark(name, then=""):
    return f"/bin/sh -c 'echo {name} >> /marks{then}'"


def stub(exec_start, after=""):
    return f"[Unit]\n{after}[Service]\nType=oneshot\nRemainAfterExit=yes\nExecStart={exec_start}\n"


def docker(*args, **kwargs):
    return subprocess.run(["docker", *args], capture_output=True, text=True, **kwargs)


class LatchBootTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not shutil.which("docker") or docker("info").returncode:
            raise unittest.SkipTest("needs a running Docker daemon")
        build = docker("build", "-q", "-t", IMAGE, "-", input=DOCKERFILE)
        if build.returncode:
            raise RuntimeError(build.stderr)

    def boot(self, latch):
        """Boot with the latch succeeding ("ok"), failing ("fail") or absent (None)."""
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        system = Path(tmp.name) / "etc/systemd/system"
        wants = system / "multi-user.target.wants"
        wants.mkdir(parents=True)

        def install(name, text):
            (system / name).write_text(text)
            (wants / name).symlink_to(f"../{name}")

        for name in PROTECTED:
            install(f"{name}.service", stub(mark(name)))
            drop_in = UNITS / f"{name}.service.d"
            if drop_in.is_dir():
                shutil.copytree(drop_in, system / drop_in.name)
        install("nvidia-cc-ready.service", stub(mark("cc-ready", f"; sleep {BRING_UP_SECONDS}")))
        if latch:
            install(LATCH, (UNITS / LATCH).read_text())
            command = mark("latch") if latch == "ok" else mark("latch-attempt", "; exit 1")
            override = system / f"{LATCH}.d"
            override.mkdir()
            (override / "99-test.conf").write_text(f"[Service]\nExecStart=\nExecStart={command}\n")
        # Ends the boot once the protected units have started or failed. The
        # delay leaves a failed latch time to power off first.
        after = "After=" + " ".join(f"{name}.service" for name in PROTECTED) + f" {LATCH}\n"
        install("finish.service", stub(
            mark("finish", "; systemctl poweroff --force --force").replace("echo", f"sleep {FINISH_SECONDS}; echo"), after))
        (system / "default.target").symlink_to("/usr/lib/systemd/system/multi-user.target")
        (system / "systemd-modules-load.service").symlink_to("/dev/null")

        container = docker("create", *RUN_FLAGS, IMAGE, "/lib/systemd/systemd").stdout.strip()
        self.assertTrue(container)
        self.addCleanup(docker, "rm", "-f", container)
        self.assertEqual(0, docker("cp", f"{tmp.name}/.", f"{container}:/").returncode)
        try:
            docker("start", "-a", container, timeout=90)
        except subprocess.TimeoutExpired:
            self.fail("the container did not power off")
        marks = Path(tmp.name) / "marks"
        docker("cp", f"{container}:/marks", str(marks))
        return marks.read_text().split() if marks.exists() else []

    def test_protected_units_start_after_a_slow_bring_up_and_the_latch(self):
        marks = self.boot("ok")
        self.assertEqual(["cc-ready", "latch"], marks[:2])
        self.assertEqual(sorted(PROTECTED), sorted(marks[2:5]))
        self.assertEqual(["finish"], marks[5:])

    def test_a_failed_latch_powers_off_before_anything_starts(self):
        self.assertEqual(["cc-ready", "latch-attempt"], self.boot("fail"))

    def test_protected_units_do_not_start_without_the_latch_unit(self):
        self.assertEqual(["cc-ready", "finish"], self.boot(None))


if __name__ == "__main__":
    unittest.main()
