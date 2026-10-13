#!/usr/bin/env python3
import hashlib
import importlib.machinery
import importlib.util
import lzma
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "bin/verify-apt-cache"
loader = importlib.machinery.SourceFileLoader("verify_apt_cache", str(SCRIPT))
spec = importlib.util.spec_from_loader(loader.name, loader)
module = importlib.util.module_from_spec(spec)
loader.exec_module(module)

DEB = "hello_1%3a2.0-1_amd64.deb"


def gpg(home, *args, **kwargs):
    return subprocess.run(["gpg", "--homedir", str(home), "--batch", *args], check=True, capture_output=True, **kwargs).stdout


class VerifyAptCacheTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.root = self.tmp / "checkout"
        self.keyring = self.tmp / "keyring.gpg"
        self.signer = self.make_key("archive")
        self.keyring.write_bytes(gpg(self.signer, "--export"))

        self.deb = b"genuine package bytes"
        packages = (
            "Package: hello\nVersion: 1:2.0-1\nArchitecture: amd64\n"
            f"Filename: pool/main/h/hello/hello_2.0-1_amd64.deb\nSHA256: {hashlib.sha256(self.deb).hexdigest()}\n\n"
        ).encode()
        dists = self.tmp / "repo/dists/resolute"
        (dists / "main/binary-amd64").mkdir(parents=True)
        (dists / "main/binary-amd64/Packages.xz").write_bytes(lzma.compress(packages))
        release = "Origin: Ubuntu\nSuite: resolute\nSHA256:\n" + "".join(
            f" {hashlib.sha256(data).hexdigest()} {len(data)} main/binary-amd64/{name}\n"
            for name, data in (("Packages", packages), ("Packages.xz", lzma.compress(packages)))
        )
        (dists / "Release").write_text(release)
        self.sign(self.signer)

        uri = (self.tmp / "repo").as_uri()
        sources = self.root / "mkosi/base/mkosi.sandbox/etc/apt/sources.list.d/mkosi.sources"
        sources.parent.mkdir(parents=True)
        sources.write_text(f"Types: deb deb-src\nURIs: {uri}\nSuites: resolute\nComponents: main\n")

        self.lists = self.root / "mkosi/base/mkosi.cache/ubuntu.metadata.cache/lib/apt/lists"
        (self.lists / "partial").mkdir(parents=True)
        (self.lists / "partial/stale").write_text("x")
        prefix = module.apt_filename(f"{uri}/dists/resolute")
        (self.lists / f"{prefix}_InRelease").write_bytes((dists / "InRelease").read_bytes())
        (self.lists / f"{prefix}_main_binary-amd64_Packages").write_bytes(packages)
        self.bin = self.root / "mkosi/base/mkosi.cache/ubuntu.metadata.cache/cache/apt/pkgcache.bin"
        self.bin.parent.mkdir(parents=True)
        self.bin.write_text("binary cache")

        self.archives = self.root / "mkosi/mkosi.pkgcache/cache/apt/archives"
        (self.archives / "partial").mkdir(parents=True)
        (self.archives / DEB).write_bytes(self.deb)

    def make_key(self, name):
        home = self.tmp / name
        home.mkdir(mode=0o700)
        gpg(home, "--passphrase", "", "--quick-gen-key", f"{name}@example.invalid", "ed25519", "sign", "never")
        return home

    def sign(self, home):
        dists = self.tmp / "repo/dists/resolute"
        (dists / "InRelease").unlink(missing_ok=True)
        gpg(home, "--clearsign", "--output", str(dists / "InRelease"), str(dists / "Release"))

    def run_script(self):
        subprocess.run([sys.executable, str(SCRIPT), "--root", str(self.root), "--keyring", str(self.keyring)], check=True)

    def test_good_cache_is_kept(self):
        self.run_script()
        self.assertEqual((self.archives / DEB).read_bytes(), self.deb)
        self.assertEqual(len(list(self.lists.glob("*_*"))), 2)
        self.assertFalse((self.lists / "partial/stale").exists())
        self.assertFalse(self.bin.exists())

    def test_tampered_or_unknown_debs_are_deleted(self):
        (self.archives / DEB).write_bytes(b"tampered package byte")
        (self.archives / "other_1.0_amd64.deb").write_bytes(b"unlisted")
        self.run_script()
        self.assertEqual(sorted(p.name for p in self.archives.iterdir()), ["partial"])

    def test_tampered_list_drops_metadata_but_keeps_verified_debs(self):
        packages = next(self.lists.glob("*_Packages"))
        packages.write_bytes(packages.read_bytes().replace(b"Version: 1:2.0-1", b"Version: 1:2.0-2"))
        self.run_script()
        self.assertFalse(self.lists.exists())
        self.assertEqual((self.archives / DEB).read_bytes(), self.deb)

    def test_bad_signature_deletes_whole_cache(self):
        self.sign(self.make_key("attacker"))
        self.run_script()
        self.assertFalse((self.root / "mkosi/mkosi.pkgcache").exists())
        self.assertFalse(self.lists.exists())

    def test_mirror_list_falls_back_and_names_lists_like_apt(self):
        sandbox = self.root / "mkosi/base/mkosi.sandbox"
        mirror_list = sandbox / "etc/apt/mirrors/ubuntu.list"
        mirror_list.parent.mkdir(parents=True)
        mirror_list.write_text(f"{(self.tmp / 'repo').as_uri()}\tpriority:2\n{(self.tmp / 'empty').as_uri()}\tpriority:1\n")
        sources = sandbox / "etc/apt/sources.list.d/mkosi.sources"
        uri = (self.tmp / "repo").as_uri()
        sources.write_text(sources.read_text().replace(f"URIs: {uri}", "URIs: mirror+file:/etc/apt/mirrors/ubuntu.list"))
        for path in self.lists.glob("*_*"):
            path.rename(path.with_name(path.name.replace(module.apt_filename(uri), "_etc_apt_mirrors_ubuntu.list")))
        self.run_script()
        self.assertEqual((self.archives / DEB).read_bytes(), self.deb)
        self.assertEqual(len(list(self.lists.glob("_etc_apt_mirrors_ubuntu.list_dists_resolute_*"))), 2)


if __name__ == "__main__":
    unittest.main()
