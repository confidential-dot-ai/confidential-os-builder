#!/bin/bash
# E2E test for the confos build pipeline.
#
# Tests:
#   1. Build with boot-time cloud-init (--skip-igvm)
#   2. Artifact existence and manifest validation
#   3. Build with IGVM (if CONFOS_FIRMWARE + CONFOS_IGVM_TOOLS set)
#   4. Boot VM and verify cloud-init applied (if QEMU + firmware available)
#
# Usage: sudo ./tests/e2e.sh
#
# Env vars:
#   CONFOS_FIRMWARE   - path to OVMF.fd (required for IGVM + boot tests)
#   CONFOS_IGVM_TOOLS - path to igvm-tools binary (required for IGVM test)
#   CONFOS_E2E_IPE=1  - build with kernel/ipe.config and assert the IPE probes
#                       (denied exec outside the root, locked enforcement);
#                       without it the probes must report IPE absent

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

MARKER="CONFOS_E2E_OK"
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

BOOT_FW="${CONFOS_FIRMWARE:-}"
if [ -z "$BOOT_FW" ] && [ -f /usr/share/OVMF/OVMF_CODE_4M.fd ]; then
    BOOT_FW=/usr/share/OVMF/OVMF_CODE_4M.fd
fi

CONFOS="$REPO_DIR/target/debug/confos"
if [ ! -x "$CONFOS" ]; then
    echo "ERROR: $CONFOS not found. Run 'cargo build' first (before sudo)."
    exit 1
fi
echo -e "${BOLD}Using $CONFOS${NC}"

# ── Cloud-init test config ────────────────────────────────────────────────────
CI_FILE=$(mktemp --suffix=.yaml)

# Everything runs from bootcmd: cloud-init feeds it to /bin/sh as a file, so
# the interpreter (on the verity root) is what executes. runcmd would write a
# script and exec it from /var, which an IPE kernel denies — that denial is
# one of the probes below, and the reason user-data must not rely on runcmd.
IPE_FLAGS=""
if [ "${CONFOS_E2E_IPE:-0}" = 1 ]; then
    IPE_FLAGS="--kernel-config-fragment kernel/ipe.config"
fi
cat > "$CI_FILE" <<USERDATA
#cloud-config
bootcmd:
  - |
    exec > /dev/hvc0 2>&1
    set -x
    echo "=== confos e2e: starting ==="
    echo ${MARKER}
    # Emit the root-layout invariant: /etc is erofs and /var is overlayfs.
    findmnt -T /etc -no TARGET,FSTYPE; findmnt -T /var -no TARGET,FSTYPE
    [ "\$(findmnt -T /etc -no FSTYPE)" = erofs ] && [ "\$(findmnt -T /var -no FSTYPE)" = overlay ] \
        && echo CONFOS_E2E_ROOT_IMMUTABLE || echo CONFOS_E2E_ROOT_MUTABLE
    # IPE (only with kernel/ipe.config): a binary copied to writable state
    # and a script written to /run must not execute; enforcement must be
    # locked (CAP_MAC_ADMIN dropped). Without IPE, say so.
    mountpoint -q /sys/kernel/security || mount -t securityfs securityfs /sys/kernel/security 2>/dev/null || true
    if [ -d /sys/kernel/security/ipe ]; then
        cp /usr/bin/true /var/tmp/confos-e2e-true && chmod 0755 /var/tmp/confos-e2e-true
        /var/tmp/confos-e2e-true && echo CONFOS_E2E_IPE_BINARY_RAN || echo CONFOS_E2E_IPE_BINARY_DENIED
        printf '#!/bin/sh\ntrue\n' > /run/confos-e2e.sh && chmod 0755 /run/confos-e2e.sh
        /run/confos-e2e.sh && echo CONFOS_E2E_IPE_SCRIPT_RAN || echo CONFOS_E2E_IPE_SCRIPT_DENIED
        echo 0 > /sys/kernel/security/ipe/enforce && echo CONFOS_E2E_IPE_UNLOCKED || echo CONFOS_E2E_IPE_LOCKED
        echo "CONFOS_E2E_IPE enforce=\$(cat /sys/kernel/security/ipe/enforce) active=\$(cat /sys/kernel/security/ipe/policies/confos/active)"
    else
        echo CONFOS_E2E_IPE_ABSENT
    fi
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

# ── Test 1: Build (--skip-igvm) ──────────────────────────────────────────────
OUT="$REPO_DIR/output/e2e-test"
rm -rf "$OUT"

echo -e "\n${BOLD}Test 1: Build (boot-time cloud-init, --skip-igvm)${NC}"
# shellcheck disable=SC2086
$CONFOS build --skip-igvm $IPE_FLAGS --cloud-init "$CI_FILE" "$(basename "$OUT")" 2>&1 | tail -20

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
want_ipe = '${CONFOS_E2E_IPE:-0}' == '1'
if m['inputs']['kernel']['ipe'] != want_ipe: print(f'kernel.ipe should be {want_ipe}'); ok = False
for section in ['inputs', 'outputs']:
    for key, entry in m[section].items():
        if entry is None or key == 'kernel': continue
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

# ── Test 3: Build with IGVM ──────────────────────────────────────────────────
echo -e "\n${BOLD}Test 3: IGVM build${NC}"

if [ -n "${CONFOS_IGVM_TOOLS:-}" ] && [ -n "${CONFOS_FIRMWARE:-}" ]; then
    IGVM_BUILD_ARGS=(--cloud-init "$CI_FILE" --firmware "$CONFOS_FIRMWARE")

    IGVM_OUT="$REPO_DIR/output/e2e-igvm"
    rm -rf "$IGVM_OUT"

    $CONFOS build "${IGVM_BUILD_ARGS[@]}" "$(basename "$IGVM_OUT")" 2>&1 | tail -20

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

    $CONFOS build "${IGVM_BUILD_ARGS[@]}" "$(basename "$IGVM_OUT2")" 2>&1 | tail -5

    HASH1=$(sha256sum "$IGVM_OUT/guest.igvm" | cut -d' ' -f1)
    HASH2=$(sha256sum "$IGVM_OUT2/guest.igvm" | cut -d' ' -f1)
    if [ "$HASH1" = "$HASH2" ]; then
        pass "IGVM: reproducible ($HASH1)"
    else
        fail "IGVM: not reproducible (${HASH1:0:16}... vs ${HASH2:0:16}...)"
    fi
else
    skip "IGVM: CONFOS_IGVM_TOOLS or CONFOS_FIRMWARE not set"
fi

# ── Test 4: Boot + cloud-init verification ────────────────────────────────────
# Uses raw QEMU instead of `confos run` because `confos run` calls exec() and
# cannot be backgrounded. TODO: add a non-exec launch mode to confos run.
echo -e "\n${BOLD}Test 4: Boot VM + verify cloud-init${NC}"

if [ -z "$BOOT_FW" ]; then
    skip "boot: no OVMF firmware available"
elif ! command -v qemu-system-x86_64 &>/dev/null; then
    skip "boot: qemu-system-x86_64 not found"
elif [ ! -e /dev/kvm ]; then
    skip "boot: /dev/kvm not available"
elif [ ! -f "$OUT/uki.efi" ]; then
    skip "boot: build output not available"
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
        -display none \
        -serial none \
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

    # The probes print from bootcmd, so they are only checked once the
    # HTTP/marker wait above has given cloud-init time to run. A missing
    # token here is a failure, not a skip: it means the default build did
    # not come up immutable or enforcing, or the probe never ran.
    # probe <good token> <bad token> <subject>: pass on the good token, fail
    # on the bad one, fail if neither was printed.
    probe() {
        if grep -q "$1" "$SERIAL_LOG" 2>/dev/null; then
            pass "e2e: $3"
        elif grep -q "$2" "$SERIAL_LOG" 2>/dev/null; then
            fail "e2e: $3 (guest reported $2)"
            grep -A2 "confos e2e: starting" "$SERIAL_LOG" | tail -5
        else
            fail "e2e: $3 (probe never reported)"
        fi
    }
    probe CONFOS_E2E_ROOT_IMMUTABLE CONFOS_E2E_ROOT_MUTABLE \
        "immutable root layout (/etc erofs, /var overlay)"
    if [ "${CONFOS_E2E_IPE:-0}" = 1 ]; then
        probe CONFOS_E2E_IPE_BINARY_DENIED CONFOS_E2E_IPE_BINARY_RAN \
            "IPE denies executing a binary outside the verity root"
        probe CONFOS_E2E_IPE_SCRIPT_DENIED CONFOS_E2E_IPE_SCRIPT_RAN \
            "IPE denies executing a script outside the verity root"
    else
        probe CONFOS_E2E_IPE_ABSENT CONFOS_E2E_IPE_BINARY_ "IPE absent on the default kernel"
    fi
    if [ "${CONFOS_E2E_IPE:-0}" != 1 ]; then
        :
    elif grep -q "CONFOS_E2E_IPE_LOCKED" "$SERIAL_LOG" 2>/dev/null \
        && grep -q "CONFOS_E2E_IPE enforce=1 active=1" "$SERIAL_LOG" 2>/dev/null; then
        pass "e2e: IPE enforcing the confos policy and root cannot switch it off"
    else
        fail "e2e: IPE enforcement is not locked"
        grep "CONFOS_E2E_IPE" "$SERIAL_LOG" | tail -5
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
