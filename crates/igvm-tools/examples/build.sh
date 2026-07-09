#!/usr/bin/env bash
# Build an IGVM file from an OVMF firmware + UKI you supply.
#
# Usage:
#   FIRMWARE=path/to/OVMF.fd KERNEL=path/to/uki.efi ./examples/build.sh
#   ... ./examples/build.sh --smp 2            # SMP 2
#   ... ./examples/build.sh -o my-guest.igvm   # custom output path
#
# See examples/README.md for how to build both inputs.

set -euo pipefail

FIRMWARE="${FIRMWARE:?set FIRMWARE to a patched OVMF.fd (see examples/README.md)}"
KERNEL="${KERNEL:?set KERNEL to a UKI (see examples/README.md)}"

if [ ! -f "$FIRMWARE" ]; then
    echo "ERROR: OVMF not found: $FIRMWARE" >&2
    echo "See examples/README.md for how to build it." >&2
    exit 1
fi

if [ ! -f "$KERNEL" ]; then
    echo "ERROR: UKI not found: $KERNEL" >&2
    echo "See examples/README.md for how to build it." >&2
    exit 1
fi

exec igvm-tools build --firmware "$FIRMWARE" --kernel "$KERNEL" "$@"
