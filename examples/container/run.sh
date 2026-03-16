#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

FORCE=0
for arg in "$@"; do
    [[ "$arg" == "--force" ]] && FORCE=1
done

STEEP="$REPO_ROOT/target/release/steep"
KERNEL="/boot/vmlinuz-$(uname -r)"
INITRD="/boot/initrd.img-$(uname -r)"
FIRMWARE="$HOME/.local/share/steep/OVMF.fd"
BASE_IMAGE="$REPO_ROOT/output/demo/base/base.raw"
OUTPUT="$REPO_ROOT/output/demo/container"
SOURCE_IMAGE="https://cloud-images.ubuntu.com/resolute/current/resolute-server-cloudimg-amd64v3.img"
IMAGE="steep-demo-container:latest"
PORT=8081

# Build steep if not already built
(cd "$REPO_ROOT" && cargo build --release --quiet)

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

# Build local container image (always, to pick up changes to examples/container/)
echo "==> Building local container image..."
podman build -t "$IMAGE" "$SCRIPT_DIR"

# Build container CVM image if not present
if [[ ! -f "$OUTPUT/manifest.json" ]]; then
    echo "==> Building container CVM image..."
    "$STEEP" container "$IMAGE" \
        --kernel "$KERNEL" \
        --initrd "$INITRD" \
        --firmware "$FIRMWARE" \
        --base-image "$BASE_IMAGE" \
        --service-port 80 \
        -o "$OUTPUT"
fi

echo ""
echo "==> Container demo ready."
echo "    URL: http://localhost:$PORT"
echo "    (caddy takes ~10-30s to start after the VM boots)"
echo ""

sudo "$STEEP" run --port-forward "${PORT}:80" "$OUTPUT"
