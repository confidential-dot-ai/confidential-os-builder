#!/usr/bin/env python3
"""The general-purpose profile's public attestation endpoint, run offline
against a stand-in for attestation-api."""

import base64
import hashlib
import http.server
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
PROFILES = ROOT / "mkosi/base/mkosi.profiles"
COMMON = PROFILES / "general-purpose"
EXTRA = COMMON / "mkosi.extra"
SERVICE = EXTRA / "usr/lib/confai/general-purpose-attest"
UNIT = EXTRA / "usr/lib/systemd/system/general-purpose-attest.service"

# One fixed case of the commitment, which confai's verifier pins too.
NONCE = bytes(range(32))
ED25519 = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIN3rqB042NenyM+fGqjpQ19oA+lo1FMJ+R4K9hwwfyw/"
ECDSA = ("ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBNn3/UBrZYJDQZ6RtyscBySI"
         "R9iNQQ0/vw+wpoxwoq81z+kze/vHSjCCxZ0yTKlYjWqiTfsgdVxtBoBRfNt+Tqs=")
REPORT_DATA = "95fa7453248f041d7d2ef55323a87d9a5ee4a175b5c2c6641811732dbedccfca9381a9309489c85a878de5c85dbefcf5"


def lp(field: bytes) -> bytes:
    return struct.pack(">I", len(field)) + field


class Backend(http.server.BaseHTTPRequestHandler):
    """attestation-api's /attest: records each request and answers as told."""

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.server.requests.append((self.path, body))
        status, answer = self.server.answer
        data = json.dumps(answer).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


class EndpointTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.keys = Path(self.tmp.name) / "ssh"
        self.keys.mkdir()
        (self.keys / "ssh_host_ed25519_key.pub").write_text(ED25519 + " root@guest\n")
        (self.keys / "ssh_host_ecdsa_key.pub").write_text(ECDSA + " root@guest\n")
        (self.keys / "ssh_host_ed25519_key").write_text("a private key, which the endpoint never opens\n")

        self.backend = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Backend)
        self.backend.requests = []
        self.backend.answer = (200, {"platform": "tdx", "evidence": {"quote": "c2lnbmVk"}})
        threading.Thread(target=self.backend.serve_forever, daemon=True).start()
        self.addCleanup(self.backend.server_close)
        self.addCleanup(self.backend.shutdown)
        self.url = self.start()

    def start(self, *extra):
        process = subprocess.Popen(
            [sys.executable, str(SERVICE), "--bind", "127.0.0.1:0",
             "--backend", f"http://127.0.0.1:{self.backend.server_address[1]}/attest",
             "--host-keys", str(self.keys / "ssh_host_*_key.pub"), "--backend-timeout", "5", *extra],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
        )
        self.addCleanup(process.wait, 10)
        self.addCleanup(process.terminate)
        line = process.stdout.readline()
        self.addCleanup(process.stdout.close)
        self.assertTrue(line.startswith("listening on 127.0.0.1:"), line)
        return "http://" + line.split()[-1]

    def post(self, body, path="/attest", url=None):
        if not isinstance(body, bytes):
            body = json.dumps(body).encode()
        request = urllib.request.Request((url or self.url) + path, data=body, method="POST")
        try:
            with urllib.request.urlopen(request, timeout=20) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as err:
            return err.code, json.load(err)

    def nonce(self, raw=NONCE):
        return base64.b64encode(raw).decode()

    def test_the_commitment_is_the_documented_layout(self):
        blobs = [base64.b64decode(line.split()[1]) for line in sorted([ED25519, ECDSA])]
        encoded = lp(b"confos/ssh-host-keys/v1") + lp(NONCE) + b"".join(lp(blob) for blob in blobs)
        self.assertEqual(hashlib.sha384(encoded).hexdigest(), REPORT_DATA)

    def test_a_nonce_gets_the_host_keys_and_a_quote_over_both(self):
        status, answer = self.post({"nonce": self.nonce()})
        self.assertEqual(status, 200)
        self.assertEqual(answer, {
            "version": "confos/ssh-host-keys/v1",
            "ssh_host_keys": [ECDSA, ED25519],
            "attestation": {"platform": "tdx", "evidence": {"quote": "c2lnbmVk"}},
        })
        # attestation-api is asked for exactly the commitment, and for no GPU.
        self.assertEqual(self.backend.requests, [
            ("/attest", {"report_data": base64.b64encode(bytes.fromhex(REPORT_DATA)).decode()}),
        ])

    def test_another_nonce_or_another_key_is_another_commitment(self):
        self.post({"nonce": self.nonce(bytes(32))})
        (self.keys / "ssh_host_ed25519_key.pub").write_text(ED25519[:-1] + "+\n")
        self.post({"nonce": self.nonce()})
        asked = [body["report_data"] for _, body in self.backend.requests]
        expected = base64.b64encode(bytes.fromhex(REPORT_DATA)).decode()
        self.assertEqual(len(set(asked + [expected])), 3)

    def test_gpu_evidence_is_passed_through_when_asked_for(self):
        self.assertEqual(self.post({"nonce": self.nonce(), "nvidia_gpu": True})[0], 200)
        self.assertEqual(self.post({"nonce": self.nonce(), "nvidia_gpu": False})[0], 200)
        self.assertEqual([body.get("nvidia_gpu") for _, body in self.backend.requests], [True, None])

    def test_a_caller_cannot_name_what_the_quote_covers(self):
        chosen = base64.b64encode(bytes(48)).decode()
        for body in (
            {"report_data": chosen},
            {"nonce": self.nonce(), "report_data": chosen},
            {"nonce": self.nonce(), "platform": "tdx"},
            {"nonce": self.nonce(NONCE[:31])},
            {"nonce": self.nonce(bytes(48))},
            {"nonce": NONCE.hex()},
            {"nonce": 7},
            {"nonce": self.nonce(), "nvidia_gpu": "yes"},
            {},
            [self.nonce()],
            b"nonce=" + self.nonce().encode(),
        ):
            with self.subTest(body=body):
                status, answer = self.post(body)
                self.assertEqual(status, 400, answer)
        self.assertEqual(self.backend.requests, [])
        # Software inside the guest that still posts report_data here learns
        # where attestation-api went.
        self.assertIn("127.0.0.1:8401", self.post({"report_data": chosen})[1]["error"])

    def test_only_the_one_route_answers(self):
        self.assertEqual(self.post({"nonce": self.nonce()}, path="/verify")[0], 404)
        self.assertEqual(self.post({"nonce": self.nonce()}, path="/attest/")[0], 404)
        with urllib.request.urlopen(self.url + "/healthz", timeout=10) as response:
            self.assertEqual(json.load(response), {"status": "ok"})
        with self.assertRaises(urllib.error.HTTPError) as get:
            urllib.request.urlopen(self.url + "/attest", timeout=10)
        self.assertEqual(get.exception.code, 405)
        self.assertEqual(get.exception.headers["Allow"], "POST")
        self.assertIn("POST", json.load(get.exception)["error"])
        get.exception.close()
        self.assertEqual(self.backend.requests, [])

    def test_a_large_body_is_refused_unread(self):
        status, _ = self.post({"nonce": self.nonce(), "nvidia_gpu": False, "pad": "x" * 2000})
        self.assertEqual(status, 413)
        self.assertEqual(self.backend.requests, [])

    def test_no_evidence_is_a_502_that_tells_the_caller_nothing(self):
        self.backend.answer = (500, {"error": "tdx: quote generation failed at /dev/tdx_guest"})
        status, answer = self.post({"nonce": self.nonce()})
        self.assertEqual((status, answer), (502, {"error": "could not obtain attestation evidence"}))

        dead = self.start("--max-quotes", "1")
        self.backend.shutdown()
        self.backend.server_close()
        self.assertEqual(self.post({"nonce": self.nonce()}, url=dead)[0], 502)

    def test_a_guest_without_usable_host_keys_gives_no_quote(self):
        (self.keys / "ssh_host_ecdsa_key.pub").write_text("ssh-rsa " + ECDSA.split()[1] + "\n")
        self.assertEqual(self.post({"nonce": self.nonce()})[0], 502)
        (self.keys / "ssh_host_ecdsa_key.pub").write_text("not a key\n")
        self.assertEqual(self.post({"nonce": self.nonce()})[0], 502)
        for path in self.keys.glob("*.pub"):
            path.unlink()
        self.assertEqual(self.post({"nonce": self.nonce()})[0], 502)
        self.assertEqual(self.backend.requests, [])


class ProfileTests(unittest.TestCase):
    def finalize(self, root):
        return subprocess.run(["bash", str(COMMON / "mkosi.finalize")],
                              env={"PATH": "/usr/bin:/bin", "BUILDROOT": str(root)},
                              capture_output=True, text=True, timeout=30)

    def test_attestation_api_moves_to_loopback_in_both_recipes(self):
        for profile in ("attest", "attest-gpu"):
            with self.subTest(profile=profile), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                baked = PROFILES / profile / "mkosi.extra/etc/attestation-api/config.toml"
                config = root / "etc/attestation-api/config.toml"
                config.parent.mkdir(parents=True)
                shutil.copy(baked, config)
                self.assertEqual(self.finalize(root).returncode, 0)
                before, after = baked.read_text().splitlines(), config.read_text().splitlines()
                changed = [(old, new) for old, new in zip(before, after, strict=True) if old != new]
                self.assertEqual(changed, [('bind = "0.0.0.0:8400"', 'bind = "127.0.0.1:8401"')])
                self.assertTrue(config.read_text().endswith("\n"))

    def test_a_config_that_binds_elsewhere_stops_the_build(self):
        with tempfile.TemporaryDirectory() as tmp:
            config = Path(tmp) / "etc/attestation-api/config.toml"
            config.parent.mkdir(parents=True)
            config.write_text('[server]\nbind = "0.0.0.0:9000"\n')
            result = self.finalize(Path(tmp))
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("does not bind 0.0.0.0:8400 exactly once", result.stderr)
            self.assertEqual(config.read_text(), '[server]\nbind = "0.0.0.0:9000"\n')

    def test_an_image_without_attestation_api_still_builds(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(self.finalize(Path(tmp)).returncode, 0)

    def test_the_unit_starts_after_the_keys_and_the_attestation_service(self):
        unit = UNIT.read_text()
        self.assertIn("After=general-purpose-ssh-host-keys.service attestation-api.service", unit)
        self.assertIn("ConditionPathExists=/usr/local/bin/attestation-api", unit)
        self.assertIn("ExecStart=/usr/lib/confai/general-purpose-attest\n", unit)
        self.assertIn("DynamicUser=yes", unit)
        preset = (EXTRA / "usr/lib/systemd/system-preset/20-general-purpose.preset").read_text()
        self.assertIn("enable general-purpose-attest.service\n", preset)

    def test_the_ports_agree_across_the_profile(self):
        service = SERVICE.read_text()
        self.assertIn('default="0.0.0.0:8400"', service)
        self.assertIn('default="http://127.0.0.1:8401/attest"', service)
        self.assertIn('bind = "127.0.0.1:8401"', (COMMON / "mkosi.finalize").read_text())
        self.assertTrue(SERVICE.stat().st_mode & 0o111, "the service file must be executable")
        self.assertEqual(list(EXTRA.rglob("__pycache__")), [])


if __name__ == "__main__":
    unittest.main()
