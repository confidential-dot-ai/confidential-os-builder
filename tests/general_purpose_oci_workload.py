#!/usr/bin/env python3
"""The general-purpose profile's launch-bound OCI workload, run offline
against a stand-in for docker."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
EXTRA = ROOT / "mkosi/base/mkosi.profiles/general-purpose/mkosi.extra"
RUNNER = EXTRA / "usr/lib/confai/general-purpose-oci-workload"
UNITS = EXTRA / "usr/lib/systemd/system"
PRESET = EXTRA / "usr/lib/systemd/system-preset/20-general-purpose.preset"

KEY = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIN3rqB042NenyM+fGqjpQ19oA+lo1FMJ+R4K9hwwfyw/ alice@example.com"
IMAGE = "ghcr.io/stefanprodan/podinfo@sha256:" + "a" * 64


class RunnerTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name)
        self.pubkey = self.dir / "operator-pubkey"
        self.daemon = self.dir / "etc/docker/daemon.json"
        self.daemon.parent.mkdir(parents=True)
        self.daemon.write_text('{"data-root": "/var/lib/user-data/docker", "mtu": 1420}\n')
        self.calls = self.dir / "calls"
        self.docker = self.dir / "docker"
        self.docker.write_text(f'#!/bin/sh\nprintf "%s\\n" "$*" >> {self.calls}\n[ "$1" != pull ] || [ ! -e {self.dir}/pull-fails ]\n')
        self.docker.chmod(0o755)
        self.gpu = self.dir / "nvidia.yaml"

    def bundle(self, *workloads, keys=(KEY,)):
        lines = list(keys) + ["# confos-oci-workload/v1 " + (w if isinstance(w, str) else json.dumps(w)) for w in workloads]
        self.pubkey.write_text("".join(line + "\n" for line in lines))

    def run_step(self, step):
        return subprocess.run(
            [sys.executable, "-I", str(RUNNER), step, "--pubkey", str(self.pubkey), "--daemon-json", str(self.daemon),
             "--docker", str(self.docker), "--gpu-cdi-spec", str(self.gpu)],
            capture_output=True, text=True)

    def docker_calls(self):
        return self.calls.read_text().splitlines() if self.calls.exists() else []

    def test_no_workload_changes_nothing(self):
        for keys in ((KEY,), ()):
            self.bundle(keys=keys)
            for step in ("prepare", "start"):
                self.assertEqual(self.run_step(step).returncode, 0)
        self.pubkey.unlink()
        self.assertEqual(self.run_step("start").returncode, 0)
        self.assertEqual(self.docker_calls(), [])
        self.assertIn("/var/lib/user-data/docker", self.daemon.read_text())

    def test_prepare_moves_only_the_data_root(self):
        self.bundle({"image": IMAGE})
        self.assertEqual(self.run_step("prepare").returncode, 0, self.run_step("prepare").stderr)
        self.assertEqual(json.loads(self.daemon.read_text()), {"data-root": "/var/lib/docker-scratch", "mtu": 1420})
        self.assertEqual(self.docker_calls(), [])

    def test_start_pulls_by_digest_and_runs_on_the_host_network(self):
        self.bundle({"image": IMAGE, "command": ["serve", "--port", "8080"], "env": {"B": "2", "A": "x y"}})
        result = self.run_step("start")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.docker_calls(), [
            "pull " + IMAGE,
            "rm --force confos-workload",
            "run --detach --name confos-workload --network host --restart unless-stopped --env A=x y --env B=2 -- " + IMAGE + " serve --port 8080",
        ])

    def test_start_passes_gpus_when_the_guest_has_them(self):
        self.gpu.write_text("cdiVersion: 0.5.0\n")
        self.bundle({"image": IMAGE})
        self.assertEqual(self.run_step("start").returncode, 0)
        self.assertIn("--device nvidia.com/gpu=all", self.docker_calls()[-1])

    def test_a_failed_pull_fails_the_unit(self):
        (self.dir / "pull-fails").touch()
        self.bundle({"image": IMAGE})
        self.assertNotEqual(self.run_step("start").returncode, 0)
        self.assertEqual(self.docker_calls(), ["pull " + IMAGE])

    def test_refuses_what_is_not_bound_exactly(self):
        bad = [
            {"image": "ghcr.io/stefanprodan/podinfo:6.9.2"},
            {"image": "podinfo@sha256:" + "a" * 64},
            {"image": "ghcr.io/Stefan/podinfo@sha256:" + "a" * 64},
            {"image": IMAGE + "; rm -rf /"},
            {"image": IMAGE, "command": "serve"},
            {"image": IMAGE, "env": {"1A": "x"}},
            {"image": IMAGE, "privileged": True},
            "not json",
        ]
        for workload in bad:
            with self.subTest(workload=workload):
                self.bundle(workload)
                for step in ("prepare", "start"):
                    self.assertNotEqual(self.run_step(step).returncode, 0)
        self.bundle({"image": IMAGE}, {"image": IMAGE})
        self.assertNotEqual(self.run_step("start").returncode, 0)
        self.assertEqual(self.docker_calls(), [])

    def test_accepts_registry_ports_and_nested_repositories(self):
        for image in ("localhost:5000/a/b@sha256:" + "b" * 64, "registry.example.com:443/org/team/app@sha256:" + "c" * 64,
                      "docker.io/library/nginx@sha256:" + "d" * 64):
            with self.subTest(image=image):
                self.bundle({"image": image})
                self.assertEqual(self.run_step("start").returncode, 0)


class UnitTests(unittest.TestCase):
    def test_units_are_ordered_around_docker_and_enabled(self):
        prepare = (UNITS / "general-purpose-oci-workload-prepare.service").read_text()
        start = (UNITS / "general-purpose-oci-workload.service").read_text()
        self.assertIn("Before=docker.service", prepare)
        self.assertIn("After=general-purpose-oci-workload-prepare.service docker.service", start)
        for unit in (prepare, start):
            self.assertIn("ConditionPathExists=/run/confai/operator-pubkey", unit)
        preset = PRESET.read_text().splitlines()
        self.assertIn("enable general-purpose-oci-workload-prepare.service", preset)
        self.assertIn("enable general-purpose-oci-workload.service", preset)
        self.assertTrue(RUNNER.stat().st_mode & 0o111)


if __name__ == "__main__":
    unittest.main()
