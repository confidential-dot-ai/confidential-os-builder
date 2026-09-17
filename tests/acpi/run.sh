#!/usr/bin/env bash
# Boot the builder's patched kernel source under QEMU. `confos kernel-source`
# fetches, verifies, patches and compiles the trusted DSDT exactly as
# `confos kernel` does, then runs tests/acpi/run.py inside its tools tree.
#
# Usage: tests/acpi/run.sh [run.py options, e.g. --jobs 4]
# Output: output/acpi/results/{results.json,*.serial.log,*.config}
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
exec bin/confos kernel-source --output output/acpi --acpi-harness -- "$@"
