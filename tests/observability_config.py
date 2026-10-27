#!/usr/bin/env python3
"""Validate both configurations with the exact collector shipped by the profile."""

import configparser
from pathlib import Path
import shlex
import subprocess


ROOT = Path(__file__).resolve().parents[1]
PROFILES = ROOT / "mkosi/base/mkosi.profiles"


def main():
    unit = configparser.ConfigParser(interpolation=None, strict=False)
    unit.read(PROFILES / "observability/mkosi.extra/etc/systemd/system/otelcol.service")
    args = shlex.split(unit["Service"]["ExecStart"])
    images = [arg for arg in args if arg.startswith("docker.io/otel/")]
    if len(images) != 1 or "@sha256:" not in images[0]:
        raise ValueError("Expected one digest-pinned collector image")
    for profile in ("observability", "gpu-observability"):
        config = PROFILES / profile / "mkosi.extra/etc/confos/observability/otelcol.yaml"
        print(f"Validating {profile}", flush=True)
        subprocess.run([
            "docker", "run", "--rm", "--network=none", "--read-only",
            "--cap-drop=ALL", "--security-opt=no-new-privileges=true",
            "--memory=512m", "--cpus=1", "--pids-limit=128",
            "-v", f"{config}:/etc/otelcol/config.yaml:ro", images[0],
            "validate", "--config=/etc/otelcol/config.yaml",
        ], check=True)


if __name__ == "__main__":
    main()
