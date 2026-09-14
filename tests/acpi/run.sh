#!/usr/bin/env bash
# Boot the builder's patched kernel source under QEMU, inside the pinned
# kernel tools tree. `confos kernel-source` fetches, verifies, patches and
# compiles the trusted DSDT exactly as `confos kernel` does; the harness
# then builds test kernels from that tree with the same toolchain.
#
# Usage: tests/acpi/run.sh [run.py options, e.g. --jobs 4]
# Output: output/acpi/results/{results.json,*.serial.log,*.config}
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

OUT="$ROOT/output/acpi"
bin/confos kernel-source --output "$OUT"
. kernel/version

exec sudo systemd-nspawn --quiet --register=no --keep-unit --ephemeral \
    --directory mkosi/kernel-builder/mkosi.output/image \
    --bind-ro "$ROOT/tests/acpi:/harness" \
    --bind "$OUT:/work" \
    python3 /harness/run.py --source "/work/linux-$LINUX_VERSION" --output /work/results "$@"
