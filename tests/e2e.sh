#!/bin/bash
# E2E test for steep seal pipeline.
#
# Tests:
#   1. Seal with boot-time cloud-init (--skip-igvm)
#   2. Artifact existence and manifest validation
#   3. Seal with IGVM (if STEEP_FIRMWARE + STEEP_IGVM_TOOLS set)
#   4. Boot VM and verify cloud-init applied (if QEMU + firmware available)
#
# Usage: sudo ./tests/e2e.sh
#
# Env vars:
#   STEEP_FIRMWARE   - path to OVMF.fd (required for IGVM + boot tests)
#   STEEP_IGVM_TOOLS - path to igvm-tools binary (required for IGVM test)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_DIR"

if [ -n "${SUDO_USER:-}" ]; then
    REAL_HOME=$(getent passwd "$SUDO_USER" | cut -d: -f6)
    [ -d "$REAL_HOME/.local/bin" ] && export PATH="$REAL_HOME/.local/bin:$PATH"
    export HOME="$REAL_HOME"
fi

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
NC='\033[0m'

MARKER="STEEP_E2E_OK"
HOST_PORT=19522
GUEST_PORT=18080

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
    [ -d "${IGVM_OUT2:-}" ] && rm -rf "$IGVM_OUT2" 2>/dev/null || true
}
trap cleanup EXIT

BOOT_FW="${STEEP_FIRMWARE:-}"
if [ -z "$BOOT_FW" ] && [ -f /usr/share/OVMF/OVMF_CODE_4M.fd ]; then
    BOOT_FW=/usr/share/OVMF/OVMF_CODE_4M.fd
fi

STEEP="$REPO_DIR/target/debug/steep"
if [ ! -x "$STEEP" ]; then
    echo "ERROR: $STEEP not found. Run 'cargo build' first (before sudo)."
    exit 1
fi
echo -e "${BOLD}Using $STEEP${NC}"

# ── Cloud-init test config ────────────────────────────────────────────────────
CI_FILE=$(mktemp --suffix=.yaml)

cat > "$CI_FILE" <<USERDATA
#cloud-config
write_files:
  - path: /etc/steep-e2e-marker
    permissions: '0644'
    content: |
      ${MARKER}

runcmd:
  - |
    exec > /dev/hvc0 2>&1
    set -ex
    echo "=== steep e2e: starting ==="
    cat /etc/steep-e2e-marker
    python3 -c "
    from http.server import HTTPServer, BaseHTTPRequestHandler
    class H(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'${MARKER}')
        def log_message(self, *a): pass
    HTTPServer(('', ${GUEST_PORT}), H).serve_forever()
    " &
USERDATA

# ── Test 1: Seal (--skip-igvm) ───────────────────────────────────────────────
OUT="$REPO_DIR/output/e2e-test"
rm -rf "$OUT"

echo -e "\n${BOLD}Test 1: Seal (boot-time cloud-init, --skip-igvm)${NC}"
$STEEP seal --skip-igvm --cloud-init "$CI_FILE" -o "$OUT" 2>&1 | tail -20

# ── Test 2: Artifact checks ──────────────────────────────────────────────────
echo -e "\n${BOLD}Test 2: Artifact checks${NC}"

for f in disk.raw uki.efi roothash manifest.json; do
    [ -f "$OUT/$f" ] && pass "$f exists" || fail "$f missing"
done

[ ! -f "$OUT/guest.igvm" ] && pass "no guest.igvm (--skip-igvm)" || fail "unexpected guest.igvm"

python3 -c "
import json, sys
m = json.load(open('$OUT/manifest.json'))
ok = True
if m['version'] != 1: print('bad version'); ok = False
if m['build']['platform'] != 'generic': print('bad platform'); ok = False
if 'measurement' in m and m['measurement'] is not None: print('unexpected measurement'); ok = False
for section in ['inputs', 'outputs']:
    for key, entry in m[section].items():
        if entry is None: continue
        h = entry.get('sha256', '')
        if len(h) != 64 or not all(c in '0123456789abcdef' for c in h):
            print(f'bad sha256 in {section}.{key}: {h}'); ok = False
sys.exit(0 if ok else 1)
" && pass "manifest: valid structure, hashes, platform=generic" \
  || fail "manifest: invalid"

RH=$(cat "$OUT/roothash")
echo "$RH" | grep -qE '^[0-9a-f]{64}$' \
    && pass "roothash valid (${RH:0:16}...)" \
    || fail "roothash invalid: $RH"

[ ! -d "$REPO_DIR/mkosi/base/mkosi.extra/var/lib/cloud" ] \
    && pass "cloud-init seed cleaned up" \
    || fail "cloud-init seed leaked"

# ── Test 3: Seal with IGVM ───────────────────────────────────────────────────
echo -e "\n${BOLD}Test 3: IGVM seal${NC}"

if [ -n "${STEEP_IGVM_TOOLS:-}" ] && [ -n "${STEEP_FIRMWARE:-}" ]; then
    IGVM_SEAL_ARGS=(--cloud-init "$CI_FILE" --firmware "$STEEP_FIRMWARE" --igvm-tools "$STEEP_IGVM_TOOLS")

    IGVM_OUT="$REPO_DIR/output/e2e-igvm"
    rm -rf "$IGVM_OUT"

    $STEEP seal "${IGVM_SEAL_ARGS[@]}" -o "$IGVM_OUT" 2>&1 | tail -20

    [ -f "$IGVM_OUT/guest.igvm" ] && pass "IGVM: guest.igvm built" || fail "IGVM: guest.igvm missing"
    [ -f "$IGVM_OUT/manifest.json" ] && pass "IGVM: manifest.json built" || fail "IGVM: manifest.json missing"

    python3 -c "
import json, sys
m = json.load(open('$IGVM_OUT/manifest.json'))
ok = True
if m['build']['platform'] != 'snp': print('bad platform'); ok = False
meas = m.get('measurement')
if not meas: print('no measurement'); ok = False
elif len(meas.get('snp_launch_digest', '')) < 64: print('bad digest'); ok = False
sys.exit(0 if ok else 1)
" && pass "IGVM: manifest has SNP measurement" || fail "IGVM: manifest invalid"

    # Reproducibility: build again and compare hashes
    IGVM_OUT2="$REPO_DIR/output/e2e-igvm-2"
    rm -rf "$IGVM_OUT2"

    $STEEP seal "${IGVM_SEAL_ARGS[@]}" -o "$IGVM_OUT2" 2>&1 | tail -5

    HASH1=$(sha256sum "$IGVM_OUT/guest.igvm" | cut -d' ' -f1)
    HASH2=$(sha256sum "$IGVM_OUT2/guest.igvm" | cut -d' ' -f1)
    if [ "$HASH1" = "$HASH2" ]; then
        pass "IGVM: reproducible ($HASH1)"
    else
        fail "IGVM: not reproducible (${HASH1:0:16}... vs ${HASH2:0:16}...)"
    fi
else
    skip "IGVM: STEEP_IGVM_TOOLS or STEEP_FIRMWARE not set"
fi

# ── Test 4: Boot + cloud-init verification ────────────────────────────────────
# Uses raw QEMU instead of `steep run` because `steep run` calls exec() and
# cannot be backgrounded. TODO: add a non-exec launch mode to steep run.
echo -e "\n${BOLD}Test 4: Boot VM + verify cloud-init${NC}"

if [ -z "$BOOT_FW" ]; then
    skip "boot: no OVMF firmware available"
elif ! command -v qemu-system-x86_64 &>/dev/null; then
    skip "boot: qemu-system-x86_64 not found"
elif [ ! -e /dev/kvm ]; then
    skip "boot: /dev/kvm not available"
elif [ ! -f "$OUT/uki.efi" ]; then
    skip "boot: seal output not available"
else
    SERIAL_LOG=$(mktemp)

    echo "Launching VM (smp=1, mem=4G, port $HOST_PORT->$GUEST_PORT)..."
    qemu-system-x86_64 \
        -machine q35 \
        -enable-kvm \
        -drive "if=pflash,format=raw,readonly=on,file=$BOOT_FW" \
        -kernel "$OUT/uki.efi" \
        -drive "file=$OUT/disk.raw,format=raw,if=virtio" \
        -smp 1 -m 4G \
        -nographic \
        -chardev "stdio,id=hvc0,signal=off" \
        -device "virtio-serial-pci,id=virtser0" \
        -device "virtconsole,chardev=hvc0,id=console0" \
        -no-reboot \
        -netdev "user,id=net0,hostfwd=tcp::${HOST_PORT}-:${GUEST_PORT}" \
        -device virtio-net-pci,netdev=net0 \
        </dev/null \
        > "$SERIAL_LOG" 2>&1 &
    QEMU_PID=$!

    echo -n "Waiting for boot..."
    BOOTED=false
    for i in $(seq 1 60); do
        if grep -q "login:\|$MARKER" "$SERIAL_LOG" 2>/dev/null; then
            BOOTED=true
            break
        fi
        echo -n "."
        sleep 2
    done
    echo ""

    if $BOOTED; then
        pass "boot: VM booted"
    else
        fail "boot: VM did not boot within 120s"
        tail -30 "$SERIAL_LOG"
    fi

    if grep -q "dm-verity\|verity" "$SERIAL_LOG" 2>/dev/null; then
        pass "boot: dm-verity setup seen in log"
    else
        skip "boot: dm-verity not visible in log"
    fi

    echo -n "Waiting for HTTP health check..."
    HTTP_OK=false
    for i in $(seq 1 30); do
        RESULT=$(curl -sf --connect-timeout 2 "http://localhost:${HOST_PORT}/" 2>/dev/null || true)
        if [ "$RESULT" = "$MARKER" ]; then
            HTTP_OK=true
            break
        fi
        echo -n "."
        sleep 2
    done
    echo ""

    if $HTTP_OK; then
        pass "e2e: cloud-init applied, HTTP health check passed"
    elif grep -q "$MARKER" "$SERIAL_LOG" 2>/dev/null; then
        pass "e2e: cloud-init applied (serial marker), HTTP didn't respond"
    else
        fail "e2e: cloud-init did not complete"
        tail -30 "$SERIAL_LOG"
    fi

    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true
    unset QEMU_PID
fi

# ── Summary ──────────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}==========================================${NC}"
echo -e "  ${GREEN}${PASS} passed${NC}, ${RED}${FAIL} failed${NC}, ${YELLOW}${SKIP} skipped${NC}"
echo -e "${BOLD}==========================================${NC}"

[ "$FAIL" -gt 0 ] && exit 1 || exit 0
