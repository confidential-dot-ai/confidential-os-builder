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
OUTPUT="$REPO_ROOT/output/demo/container"
SOURCE_IMAGE="https://cloud-images.ubuntu.com/resolute/current/resolute-server-cloudimg-amd64v3.img"
IMAGE="steep-demo-container:latest"
PORT=8081

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

# Build local container image (always, to pick up changes to examples/container/)
echo "==> Building local container image..."
podman build -t "$IMAGE" "$SCRIPT_DIR"

# Check if CVM is stale (missing or built from a different container image)
NEW_IMAGE_ID="$(podman image inspect --format '{{.Id}}' "$IMAGE")"
IMAGE_ID_FILE="$OUTPUT/.container-image-id"
CVM_STALE=0
if [[ ! -f "$OUTPUT/manifest.json" ]]; then
    CVM_STALE=1
elif [[ ! -f "$IMAGE_ID_FILE" ]] || [[ "$(cat "$IMAGE_ID_FILE")" != "$NEW_IMAGE_ID" ]]; then
    echo "==> Container image changed; rebuilding CVM..."
    rm -rf "$OUTPUT"
    CVM_STALE=1
fi

if [[ $CVM_STALE -eq 1 ]]; then
    echo "==> Building container CVM image..."
    "$STEEP" container "$IMAGE" \
        --kernel "$KERNEL" \
        --firmware "$FIRMWARE" \
        --base-image "$BASE_IMAGE" \
        --service-port 80 \
        -o "$OUTPUT"
    echo "$NEW_IMAGE_ID" > "$IMAGE_ID_FILE"
fi

echo ""
echo "==> Container demo ready."
echo "    URL: http://localhost:$PORT"
echo "    (caddy takes ~10-30s to start after the VM boots)"
echo ""

sudo "$STEEP" run --port-forward "${PORT}:80" "$OUTPUT"
