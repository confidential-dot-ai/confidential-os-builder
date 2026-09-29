#!/usr/bin/env python3
"""Exercise failed runner metadata refreshes without installing host packages."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(sys.platform.startswith("linux"), "installer reads /etc/os-release")
class HostDependencyTests(unittest.TestCase):
    def test_incomplete_metadata_stops_before_package_installation(self):
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            log = temporary / "calls"
            sudo = temporary / "sudo"
            sudo.write_text("""#!/usr/bin/env bash
set -eu
printf '%s\\n' "$*" >> "$HOST_DEPS_TEST_LOG"
if [[ "${!#}" == update ]]; then
    echo 'W: Failed to fetch snapshot index: 503 Service Unavailable'
    if [[ " $* " == *' APT::Update::Error-Mode=any '* ]]; then exit 100; fi
fi
exit 0
""")
            sudo.chmod(0o755)
            environment = {
                **os.environ,
                "PATH": f"{temporary}:{os.environ['PATH']}",
                "HOST_DEPS_TEST_LOG": str(log),
            }
            subprocess.run([str(sudo), "--probe"], env=environment, check=True, timeout=5)
            log.unlink()
            result = subprocess.run(
                ["bash", str(ROOT / "bin/host-deps"), "bash"],
                env=environment,
                text=True, capture_output=True, timeout=15,
            )
            self.assertEqual(result.returncode, 100, result.stdout + result.stderr)
            calls = log.read_text().splitlines()
            self.assertEqual(len(calls), 1, calls)
            self.assertTrue(calls[0].endswith(" update"), calls)


if __name__ == "__main__":
    unittest.main()
