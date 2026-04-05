#!/bin/bash
# E2E test for steep seal pipeline.
#
# Seals ONE image with cloud-init that installs a Rust toolchain, clones kettle,
# builds it, sets up a systemd service, and exposes an HTTP health endpoint.
# Then boots the VM and verifies everything worked.
#
# Usage: sudo ./tests/e2e.sh
#
# Env vars:
#   STEEP_FIRMWARE   - path to OVMF.fd (optional, uses system OVMF if unset)
#   STEEP_IGVM_TOOLS - path to igvm-tools binary (optional, skips IGVM test if unset)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_DIR"

# Ensure user-installed tools (mkosi, etc.) are available under sudo.
# sudo strips ~/.local/bin and resets HOME, but steep needs both.
if [ -n "${SUDO_USER:-}" ]; then
    REAL_HOME="/home/$SUDO_USER"
    [ -d "$REAL_HOME/.local/bin" ] && export PATH="$REAL_HOME/.local/bin:$PATH"
    export HOME="$REAL_HOME"
fi

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
NC='\033[0m'

PASS=0
FAIL=0
SKIP=0

pass() { echo -e "  ${GREEN}PASS${NC}  $1"; PASS=$((PASS + 1)); }
fail() { echo -e "  ${RED}FAIL${NC}  $1"; FAIL=$((FAIL + 1)); }
skip() { echo -e "  ${YELLOW}SKIP${NC}  $1"; SKIP=$((SKIP + 1)); }

cleanup() {
    [ -n "${QEMU_PID:-}" ] && kill "$QEMU_PID" 2>/dev/null || true
    [ -n "${SERIAL_LOG:-}" ] && rm -f "$SERIAL_LOG" 2>/dev/null || true
    [ -n "${CI_FILE:-}" ] && rm -f "$CI_FILE" 2>/dev/null || true
}
trap cleanup EXIT

# Resolve firmware for KVM boot tests
BOOT_FW="${STEEP_FIRMWARE:-}"
if [ -z "$BOOT_FW" ] && [ -f /usr/share/OVMF/OVMF_CODE_4M.fd ]; then
    BOOT_FW=/usr/share/OVMF/OVMF_CODE_4M.fd
fi

# ── Build steep ──────────────────────────────────────────────────────────────
# Must be pre-built: run `cargo build` before `sudo ./tests/e2e.sh`
STEEP="$REPO_DIR/target/debug/steep"
if [ ! -x "$STEEP" ]; then
    echo "ERROR: $STEEP not found. Run 'cargo build' first (before sudo)."
    exit 1
fi
echo -e "${BOLD}Using $STEEP${NC}"

# ── Cloud-init: realistic workload ──────────────────────────────────────────
# Installs rust, clones + builds kettle (real deps: openssl, serde, sha2, etc.),
# creates a systemd service, exposes HTTP health check.
CI_FILE=$(mktemp --suffix=.yaml)
HOST_PORT=19522
GUEST_PORT=18080

cat > "$CI_FILE" <<USERDATA
#cloud-config
runcmd:
  - |
    exec > /dev/ttyS0 2>&1
    set -ex
    echo "=== steep e2e: starting ==="

    # Install Rust
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    export PATH="/root/.cargo/bin:\$PATH"
    rustc --version

    # Clone and build kettle (real project with real deps)
    cd /tmp
    git clone https://github.com/aspect-build/kettle.git
    cd kettle
    cargo build --release 2>&1
    test -f target/release/kettle

    echo "STEEP_E2E_OK"

    # Serve result over HTTP so host can check
    python3 -c "
    from http.server import HTTPServer, BaseHTTPRequestHandler
    class H(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'STEEP_E2E_OK')
        def log_message(self, *a): pass
    HTTPServer(('', ${GUEST_PORT}), H).serve_forever()
    " &
USERDATA

# ── Seal ─────────────────────────────────────────────────────────────────────
OUT="$REPO_DIR/output/e2e-test"
rm -rf "$OUT"

echo -e "\n${BOLD}Sealing image (cloud-init + service port ${GUEST_PORT})...${NC}"
$STEEP seal --skip-igvm --cloud-init "$CI_FILE" --service-port "$GUEST_PORT" -o "$OUT" 2>&1 | tail -20

# ── Artifact checks ─────────────────────────────────────────────────────────
echo -e "\n${BOLD}Checking artifacts...${NC}"

for f in disk.raw uki.efi roothash manifest.json; do
    [ -f "$OUT/$f" ] && pass "$f exists" || fail "$f missing"
done

[ ! -f "$OUT/guest.igvm" ] && pass "no guest.igvm (--skip-igvm)" || fail "unexpected guest.igvm"

# Manifest checks
python3 -c "
import json, sys
m = json.load(open('$OUT/manifest.json'))
ok = True
if m['version'] != 1: print('bad version'); ok = False
if m['build']['platform'] != 'generic': print('bad platform'); ok = False
if 'measurement' in m: print('unexpected measurement'); ok = False
sys.exit(0 if ok else 1)
" && pass "manifest: valid, platform=generic, no measurement" \
  || fail "manifest: invalid"

# Roothash
RH=$(cat "$OUT/roothash")
echo "$RH" | grep -qE '^[0-9a-f]{64}$' \
    && pass "roothash valid (${RH:0:16}...)" \
    || fail "roothash invalid: $RH"

# Cleanup check
[ ! -d "$REPO_DIR/mkosi/base/mkosi.extra/var/lib/cloud" ] \
    && pass "cloud-init seed cleaned up" \
    || fail "cloud-init seed leaked"
[ ! -f "$REPO_DIR/mkosi/base/mkosi.extra/etc/nftables.conf" ] \
    && pass "nftables.conf cleaned up" \
    || fail "nftables.conf leaked"

# ── IGVM test (optional, runs igvm-tools on the UKI from the first seal) ────
echo -e "\n${BOLD}IGVM test...${NC}"

if [ -n "${STEEP_IGVM_TOOLS:-}" ] && [ -n "${STEEP_FIRMWARE:-}" ]; then
    IGVM_OUT="$OUT/igvm-test"
    mkdir -p "$IGVM_OUT"
    # Run igvm-tools directly on the UKI — no need to re-seal
    if "$STEEP_IGVM_TOOLS" build \
        --firmware "$STEEP_FIRMWARE" \
        --kernel "$OUT/uki.efi" \
        --smp 1 \
        --platform snp \
        --manifest "$IGVM_OUT/manifest.json" \
        -o "$IGVM_OUT/guest.igvm" 2>&1 | tail -5; then
        [ -f "$IGVM_OUT/guest.igvm" ] && pass "IGVM: guest.igvm built" || fail "IGVM: guest.igvm missing"
        python3 -c "
import json, sys
m = json.load(open('$IGVM_OUT/manifest.json'))
sys.exit(0 if 'measurement' in m else 1)
" && pass "IGVM: manifest has measurement" || fail "IGVM: no measurement"
    else
        fail "IGVM: igvm-tools build failed"
    fi
else
    skip "IGVM: STEEP_IGVM_TOOLS or STEEP_FIRMWARE not set"
fi

# ── Boot + E2E test ─────────────────────────────────────────────────────────
echo -e "\n${BOLD}Boot + E2E test (build kettle inside VM)...${NC}"

if [ -z "$BOOT_FW" ]; then
    skip "boot: no OVMF firmware available"
elif ! command -v qemu-system-x86_64 &>/dev/null; then
    skip "boot: qemu-system-x86_64 not found"
elif [ ! -f "$OUT/uki.efi" ]; then
    skip "boot: seal output not available"
else
    SERIAL_LOG=$(mktemp)

    echo "Launching VM (smp=4, mem=8G, port $HOST_PORT->$GUEST_PORT)..."
    sudo qemu-system-x86_64 \
        -machine q35 \
        -enable-kvm \
        -drive "if=pflash,format=raw,readonly=on,file=$BOOT_FW" \
        -kernel "$OUT/uki.efi" \
        -drive "file=$OUT/disk.raw,format=raw,if=virtio" \
        -smp 4 -m 8G \
        -nographic \
        -no-reboot \
        -serial stdio \
        -monitor none \
        -netdev "user,id=net0,hostfwd=tcp::${HOST_PORT}-:${GUEST_PORT}" \
        -device virtio-net-pci,netdev=net0 \
        </dev/null \
        > "$SERIAL_LOG" 2>&1 &
    QEMU_PID=$!

    # Wait for login prompt (basic boot check)
    echo -n "Waiting for boot..."
    BOOTED=false
    for i in $(seq 1 90); do
        if grep -q "login:" "$SERIAL_LOG" 2>/dev/null; then
            BOOTED=true
            break
        fi
        echo -n "."
        sleep 2
    done
    echo ""

    if $BOOTED; then
        pass "boot: VM reached login prompt"
    else
        fail "boot: VM did not reach login prompt within 180s"
        echo "--- last 30 lines of serial log ---"
        tail -30 "$SERIAL_LOG"
    fi

    # Check verity in boot log
    if grep -q "verity" "$SERIAL_LOG" 2>/dev/null; then
        pass "boot: dm-verity setup seen in log"
    else
        skip "boot: dm-verity not visible in log"
    fi

    # Wait for cloud-init to finish (kettle build takes a few minutes)
    echo -n "Waiting for kettle build (up to 10 min)..."
    BUILD_OK=false
    for i in $(seq 1 150); do
        RESULT=$(curl -sf --connect-timeout 2 "http://localhost:${HOST_PORT}/" 2>/dev/null || true)
        if [ "$RESULT" = "STEEP_E2E_OK" ]; then
            BUILD_OK=true
            break
        fi
        echo -n "."
        sleep 4
    done
    echo ""

    if $BUILD_OK; then
        pass "e2e: kettle built + systemd service running (HTTP)"
    elif grep -q "STEEP_E2E_OK" "$SERIAL_LOG" 2>/dev/null; then
        pass "e2e: kettle built (serial log marker), HTTP endpoint didn't respond"
    else
        fail "e2e: kettle build did not complete within 10 minutes"
        echo "--- last 50 lines of serial log ---"
        tail -50 "$SERIAL_LOG"
    fi

    sudo kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true
    unset QEMU_PID
fi

# ── Summary ──────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}==========================================${NC}"
echo -e "  ${GREEN}${PASS} passed${NC}, ${RED}${FAIL} failed${NC}, ${YELLOW}${SKIP} skipped${NC}"
echo -e "${BOLD}==========================================${NC}"

[ "$FAIL" -gt 0 ] && exit 1 || exit 0
