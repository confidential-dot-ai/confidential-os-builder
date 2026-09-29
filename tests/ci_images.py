#!/usr/bin/env python3
import importlib.machinery
import importlib.util
import json
from pathlib import Path
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
loader = importlib.machinery.SourceFileLoader("ci_images", str(ROOT / "bin/ci-images"))
spec = importlib.util.spec_from_loader(loader.name, loader)
module = importlib.util.module_from_spec(spec)
loader.exec_module(module)
RECIPES = json.loads((ROOT / "ci/images.json").read_text())
build_loader = importlib.machinery.SourceFileLoader("build_image", str(ROOT / "bin/build-image"))
build_spec = importlib.util.spec_from_loader(build_loader.name, build_loader)
build_module = importlib.util.module_from_spec(build_spec)
build_loader.exec_module(build_module)


class SelectionTests(unittest.TestCase):
    def check(self, paths, expected):
        self.assertEqual(module.affected(RECIPES, paths), sorted(expected))

    def test_gpu_only_profile_and_kernel_changes(self):
        for path in ("mkosi/base/mkosi.profiles/general-purpose-gpu/mkosi.conf", "kernel/general-purpose-gpu.config", "kernel/gpu.config", "bin/confos-fetch-gpu", "bin/lib/gpu-module-cache.sh", "kernel/config-x86_64-general-purpose-gpu.snapshot"):
            self.check([path], ["general-purpose-gpu"])

    def test_shared_workload_and_ssh(self):
        for profile in ("general-purpose", "ssh"):
            self.check([f"mkosi/base/mkosi.profiles/{profile}/mkosi.extra/file"], ["general-purpose", "general-purpose-gpu"])

    def test_cpu_only(self):
        self.check(["kernel/general-purpose.config"], ["general-purpose", "general-purpose-gpu"])
        self.check(["mkosi/base/mkosi.profiles/attest/mkosi.sync"], ["general-purpose"])

    def test_shared_kernel_and_builder(self):
        for path in ("kernel/hardening.config", "kernel/trusted-dsdt.asl", "kernel/version", "mkosi/initrd/mkosi.conf", "src/commands/build.rs", "ci/images.json", ".github/actions/build-cache/action.yml"):
            self.check([path], RECIPES)

    def test_docs_and_unpublished_dev_profile_do_not_rebuild(self):
        self.check(["docs/KERNEL-SECURITY.md", "README.md", "ci/README.md", "mkosi/base/mkosi.profiles/dev/mkosi.conf"], [])
        for profile in ("general-purpose", "general-purpose-gpu"):
            self.check([f"mkosi/base/mkosi.profiles/{profile}/README.md"], [])

    def test_payload_readme_still_rebuilds(self):
        self.check(["mkosi/base/mkosi.profiles/general-purpose/mkosi.extra/opt/README.md"], ["general-purpose", "general-purpose-gpu"])

    def test_deletion_and_multiple_profiles(self):
        self.check(["mkosi/base/mkosi.profiles/attest/mkosi.sync", "kernel/general-purpose-gpu.config"], ["general-purpose", "general-purpose-gpu"])


class BuildRecipeTests(unittest.TestCase):
    def test_only_gpu_kernel_preparation_requires_tools_on_cache_hit(self):
        for name, recipe in RECIPES.items():
            with self.subTest(recipe=name), patch("sys.argv", ["build-image", name]), patch.object(build_module.subprocess, "run") as run:
                build_module.main()
                commands = [call.args[0] for call in run.call_args_list]
                self.assertEqual("--ensure-tools" in commands[0], "gpu" in recipe["profiles"])
                self.assertEqual(commands[0][1], "kernel")
                self.assertNotIn("--ensure-tools", commands[-1])
                self.assertEqual(commands[-1][1], "build")
                if "gpu" in recipe["profiles"]:
                    self.assertEqual(Path(commands[1][0]).name, "confos-fetch-gpu")


if __name__ == "__main__":
    unittest.main()
