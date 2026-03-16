#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

FORCE=0
for arg in "$@"; do
    [[ "$arg" == "--force" ]] && FORCE=1
done

STEEP="$REPO_ROOT/target/release/steep"
IGVM_PREBUILT="$(cd "$REPO_ROOT/../igvm-tools/examples/prebuilt" && pwd)"
KERNEL="$IGVM_PREBUILT/uki.efi"
FIRMWARE="$IGVM_PREBUILT/OVMF.fd"
BASE_IMAGE="$REPO_ROOT/output/demo/base/base.raw"
OUTPUT="$REPO_ROOT/output/demo/cloud-init"
SOURCE_IMAGE="https://cloud-images.ubuntu.com/resolute/current/resolute-server-cloudimg-amd64v3.img"
PORT=8080

# Build steep if not already built
if [[ ! -x "$STEEP" ]]; then
    (cd "$REPO_ROOT" && cargo build --release --quiet)
fi

# Remove output dir if --force
if [[ $FORCE -eq 1 ]]; then
    rm -rf "$OUTPUT"
fi

# Build base image if not present
if [[ ! -f "$BASE_IMAGE" ]]; then
    echo "==> Building base image..."
    "$STEEP" base \
        --source-image "$SOURCE_IMAGE" \
        -o "$REPO_ROOT/output/demo/base"
fi

# Build cloud-init image if not present
if [[ ! -f "$OUTPUT/manifest.json" ]]; then
    echo "==> Building cloud-init CVM image..."
    "$STEEP" cloud-init "$SCRIPT_DIR" \
        --kernel "$KERNEL" \
        --firmware "$FIRMWARE" \
        --base-image "$BASE_IMAGE" \
        --service-port 80 \
        -o "$OUTPUT"
fi

echo ""
echo "==> Cloud-init demo ready."
echo "    URL: http://localhost:$PORT"
echo "    (caddy takes ~10-30s to start after the VM boots)"
echo ""

sudo "$STEEP" run --port-forward "${PORT}:80" "$OUTPUT"
