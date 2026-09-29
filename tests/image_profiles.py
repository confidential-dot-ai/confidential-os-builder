#!/usr/bin/env python3
"""Offline profile regressions; hardware acceptance is a separate gate."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
PROFILE = ROOT / "mkosi/base/mkosi.profiles/general-purpose-gpu"
EXTRA = PROFILE / "mkosi.extra"
COMMON = ROOT / "mkosi/base/mkosi.profiles/general-purpose"
COMMON_EXTRA = COMMON / "mkosi.extra"
READY = "CC State : ON\nMulti-GPU Mode : NVLE\nCC GPUs Ready State : Ready\n"


class WorkloadTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.directory = Path(self.tmp.name)
        self.bin = self.directory / "bin"
        self.bin.mkdir()
        self.env = {
            **os.environ,
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "FAKE_GPUS": "2",
            "FAKE_CC": READY,
            "FAKE_PCI": self.pci(2),
        }
        self.env.pop("CONFOS_EXPECTED_GPUS", None)
        self.shim("lspci", '''
[[ "$*" == '-Dn -d 10de:' ]] || {
    echo 'GPU detection requires numeric-only PCI output' >&2
    exit 1
}
printf '%s\\n' "$FAKE_PCI"
''')
        self.shim("nvidia-smi", '''
case "$*" in
  '--query-gpu=index --format=csv,noheader')
    for ((i=0; i<FAKE_GPUS; i++)); do echo "$i"; done ;;
  'conf-compute -q') printf '%s\\n' "$FAKE_CC" ;;
  *) echo 'GPU diagnostics' ;;
esac
''')

    @staticmethod
    def pci(count):
        return "\n".join(
            f"0000:{i+9:02x}:00.0 0302: 10de:{'2901' if i % 2 else '3182'}"
            for i in range(count)
        ) + "\n0000:01:00.0 0604: 10de:1234"

    def shim(self, name, body):
        path = self.bin / name
        path.write_text("#!/bin/bash\nset -eu\n" + body + "\n")
        path.chmod(0o755)

    def run_helper(self, name, *args):
        return subprocess.run(
            ["bash", str(EXTRA / "usr/local/bin" / name), *map(str, args)],
            env=self.env, text=True, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, timeout=30,
        )

    def test_detects_one_two_four_and_eight_blackwell_gpus(self):
        for count in (1, 2, 4, 8):
            with self.subTest(gpus=count):
                self.env.update(FAKE_GPUS=str(count), FAKE_PCI=self.pci(count))
                result = self.run_helper("general-purpose-gpu-check")
                self.assertEqual(result.returncode, 0, result.stdout)
                self.assertIn(f"{count}-GPU visibility", result.stdout)

    def test_uninitialized_or_missing_pci_gpus_fail(self):
        for visible, pci in ((1, self.pci(2)), (0, self.pci(2)), (2, "")):
            with self.subTest(visible=visible, pci=pci):
                self.env.update(FAKE_GPUS=str(visible), FAKE_PCI=pci)
                self.assertNotEqual(self.run_helper("general-purpose-gpu-check").returncode, 0)

    def test_override_is_validated_and_not_a_hardcoded_default(self):
        self.env["CONFOS_EXPECTED_GPUS"] = "2"
        self.env["FAKE_PCI"] = ""
        self.assertEqual(self.run_helper("general-purpose-gpu-check").returncode, 0)
        for invalid in ("0", "-1", "eight", "2;echo injected"):
            self.env["CONFOS_EXPECTED_GPUS"] = invalid
            self.assertNotEqual(self.run_helper("general-purpose-gpu-check").returncode, 0)

    def test_nonconfidential_devtools_and_unencrypted_fabric_fail(self):
        for cc in (
            READY.replace("ON", "OFF"),
            READY.replace("ON", "DEVTOOLS"),
            READY.replace("Ready", "Not Ready"),
            READY.replace("NVLE", "Protected PCIe"),
            READY + "DevTools Mode : ON\n",
            "",
        ):
            with self.subTest(cc=cc):
                self.env["FAKE_CC"] = cc
                self.assertNotEqual(self.run_helper("general-purpose-gpu-check").returncode, 0)

    @unittest.skipIf(os.geteuid() == 0, "NCCL helper intentionally drops root privileges")
    def test_nccl_uses_all_visible_gpus_and_supported_cc_paths(self):
        self.shim("general-purpose-gpu-check", "exit 0")
        self.shim("timeout", '''
printf '%s %s %s %s\\n' "$NCCL_NVLS_ENABLE" "$NCCL_CUMEM_HOST_ENABLE" "$NCCL_ENV_PLUGIN" "$*" >> "$TEST_COMMANDS"
''')
        for count in (2, 4, 8):
            with self.subTest(gpus=count):
                commands = self.directory / f"commands-{count}"
                output = self.directory / f"results-{count}"
                self.env.update(FAKE_GPUS=str(count), TEST_COMMANDS=str(commands))
                result = self.run_helper("run-nccl-smoke", output)
                self.assertEqual(result.returncode, 0, result.stdout)
                calls = commands.read_text().splitlines()
                self.assertEqual(len(calls), 7)
                self.assertTrue(all(call.startswith("0 0 none ") for call in calls))
                self.assertIn(f"-g {count}", calls[0])
                self.assertIn(f"mpirun -np {count}", calls[1])
                self.assertTrue((output / "all-reduce-mpi.txt").exists())

    @unittest.skipIf(os.geteuid() == 0, "NCCL helper intentionally drops root privileges")
    def test_nccl_failure_does_not_report_success(self):
        self.shim("general-purpose-gpu-check", "exit 0")
        self.shim("timeout", "exit 1")
        result = self.run_helper("run-nccl-smoke", self.directory / "failed-results")
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("NCCL smoke matrix completed", result.stdout)

    def test_workload_kernel_keeps_gpu_requirements_without_changing_baseline(self):
        workload = (ROOT / "kernel/general-purpose-gpu.config").read_text().splitlines()
        for line in (ROOT / "kernel/gpu.config").read_text().splitlines():
            if line.startswith("CONFIG_") or (line.startswith("# CONFIG_") and line.endswith(" is not set")):
                self.assertIn(line, workload)
        for symbol in ("BPF_SYSCALL", "BPF_JIT_ALWAYS_ON", "BPF_UNPRIV_DEFAULT_OFF", "CGROUP_BPF", "USER_NS", "VETH", "BRIDGE", "NF_TABLES", "IO_URING"):
            self.assertIn(f"CONFIG_{symbol}=y", workload)
        shared = (ROOT / "kernel/general-purpose.config").read_text()
        self.assertTrue((ROOT / "kernel/general-purpose-gpu.config").read_text().endswith(shared))
        for symbol in ("CHECKPOINT_RESTORE", "KCMP", "USERFAULTFD", "FUSE_FS", "TUN"):
            self.assertIn(f"# CONFIG_{symbol} is not set", shared)
        self.assertIn("CONFIG_MSEAL_SYSTEM_MAPPINGS=y", shared)
        self.assertNotIn("CONFIG_MODULES=y", shared)
        self.assertNotIn("CONFIG_VSOCKETS=y", shared)
        baseline = (ROOT / "kernel/hardening.config").read_text()
        self.assertIn("# CONFIG_BPF_SYSCALL is not set", (ROOT / "kernel/config-x86_64.snapshot").read_text())
        self.assertIn("# CONFIG_IO_URING is not set", baseline)

    def test_mutability_persistence_and_container_networking_are_explicit(self):
        state = (COMMON_EXTRA / "usr/lib/confai/state.d/40-general-purpose-package-manager.conf").read_text()
        self.assertEqual([line for line in state.splitlines() if line and not line.startswith("#")], ["boot", "etc", "opt", "usr"])
        docker = json.loads((COMMON_EXTRA / "etc/docker/daemon.json").read_text())
        self.assertEqual(docker["data-root"], "/var/lib/user-data/docker")
        self.assertTrue(docker["features"]["cdi"])
        self.assertEqual(str(docker["mtu"]), docker["default-network-opts"]["bridge"]["com.docker.network.driver.mtu"])
        network = (COMMON_EXTRA / "etc/systemd/network/70-container-veth.network").read_text()
        self.assertIn("Kind=veth bridge", network)
        self.assertIn("Unmanaged=yes", network)
        for unit in ("docker.service.d/10-general-purpose-persistent-state.conf", "containerd.service.d/10-general-purpose-persistent-state.conf"):
            self.assertIn("RequiresMountsFor=/var/lib/user-data", (COMMON_EXTRA / "etc/systemd/system" / unit).read_text())
        self.assertIn("What=/dev/disk/by-label/user-data", (COMMON_EXTRA / "etc/systemd/system/var-lib-user\\x2ddata.mount").read_text())
        self.assertNotIn("nvidia", (COMMON_EXTRA / "etc/systemd/system/docker.service.d/10-general-purpose-persistent-state.conf").read_text())
        ssh = (COMMON_EXTRA / "etc/ssh/sshd_config.d/10-opkey.conf").read_text()
        self.assertIn("AuthorizedKeysFile /run/confai/operator-pubkey .ssh/authorized_keys", ssh)
        self.assertIn("PasswordAuthentication no", ssh)
        self.assertIn("PermitRootLogin prohibit-password", ssh)
        self.assertIn("build-nompi", (PROFILE / "mkosi.postinst").read_text())

    def test_shell_hooks_and_helpers_parse(self):
        paths = [PROFILE / name for name in ("mkosi.sync", "mkosi.finalize", "mkosi.postinst")]
        paths.extend(COMMON / name for name in ("mkosi.finalize", "mkosi.postinst"))
        paths.extend((EXTRA / "usr/local/bin").iterdir())
        paths.extend((COMMON_EXTRA / "usr/local/bin").iterdir())
        for path in paths:
            with self.subTest(script=path.name):
                result = subprocess.run(["bash", "-n", str(path)], timeout=30)
                self.assertEqual(result.returncode, 0)

    @unittest.skipUnless(sys.platform.startswith("linux"), "finalizer uses Linux sed/chroot")
    def test_finalizer_removes_nested_overlay_and_keeps_test_account_locked(self):
        root = self.directory / "root"
        (root / "etc/ld.so.conf.d").mkdir(parents=True)
        state = root / "usr/lib/confai/state.d"
        state.mkdir(parents=True)
        (state / "50-ssh.conf").write_text("etc/ssh\n")
        (root / "etc/shadow").write_text("nccl-test:!:0:0:99999:7:::\n")
        self.shim("chroot", "exit 0")
        result = subprocess.run(
            ["bash", str(PROFILE / "mkosi.finalize")],
            env={**self.env, "BUILDROOT": str(root)}, timeout=30,
        )
        self.assertEqual(result.returncode, 0)
        result = subprocess.run(
            ["bash", str(COMMON / "mkosi.finalize")],
            env={**self.env, "BUILDROOT": str(root)}, timeout=30,
        )
        self.assertEqual(result.returncode, 0)
        self.assertFalse((state / "50-ssh.conf").exists())
        self.assertIn("nccl-test:!:19800:", (root / "etc/shadow").read_text())

    def test_cpu_profile_has_runtime_without_gpu_dependencies(self):
        conf = (COMMON / "mkosi.conf").read_text()
        for package in ("docker.io", "containerd", "runc", "git", "python3-venv"):
            self.assertIn(package, conf)
        packages = conf.split("Packages=", 1)[1].lower()
        self.assertNotIn("nvidia", packages)
        self.assertNotIn("cuda", packages)
        sysctl = (COMMON_EXTRA / "etc/sysctl.d/99-general-purpose.conf").read_text()
        self.assertIn("kernel.unprivileged_bpf_disabled=1", sysctl.replace(" ", ""))
        self.assertIn("net.core.bpf_jit_harden=2", sysctl.replace(" ", ""))
        workflow = (ROOT / ".github/workflows/general-purpose.yml").read_text()
        self.assertIn("fromJSON(needs.validate.outputs.images)", workflow)
        self.assertIn("inputs.publish == true", workflow)
        recipes = json.loads((ROOT / "ci/images.json").read_text())
        self.assertIn("general-purpose", recipes["general-purpose"]["profiles"])

    def test_snapshot_translation_fix_does_not_change_package_pins(self):
        for tree in ("base", "initrd", "kernel-builder"):
            sandbox = ROOT / "mkosi" / tree / "mkosi.sandbox/etc/apt"
            self.assertIn('Acquire::Languages "none";', (sandbox / "apt.conf.d/81-no-translations").read_text())
            self.assertIn("https://snapshot.ubuntu.com/ubuntu/", (sandbox / "sources.list.d/mkosi.sources").read_text())

    def test_cache_directories_match_shared_actions_paths(self):
        action = (ROOT / ".github/actions/build-cache/action.yml").read_text()
        self.assertIn("mkosi/mkosi.pkgcache", action)
        self.assertNotIn("mkosi/base/mkosi.pkgcache", action)
        for tree in ("base", "initrd", "kernel-builder"):
            conf = (ROOT / "mkosi" / tree / "mkosi.conf").read_text()
            self.assertIn("PackageCacheDirectory=../mkosi.pkgcache", conf)
            self.assertIn("CacheDirectory=mkosi.cache", conf)
            self.assertIn("Incremental=false", conf)
        for tree in ("base", "initrd"):
            self.assertIn("OutputDirectory=mkosi.output", (ROOT / "mkosi" / tree / "mkosi.conf").read_text())


if __name__ == "__main__":
    unittest.main()
