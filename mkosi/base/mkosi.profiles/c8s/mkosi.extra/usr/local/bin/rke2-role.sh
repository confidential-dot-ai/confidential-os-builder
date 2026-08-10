#!/bin/bash
# Role dispatch, run by rke2-role.service before any role-gated unit.
# Reads an optional disk labelled `joindata` (files: role, server) and
# writes /run/confos/role-{server,agent} plus join.env for rke2-join.
# No disk => server, the single-node default.
#
# joindata is host-controlled: worst case is DoS; a malformed disk fails
# this unit and every role-gated unit stays down (docs/C8S-MULTINODE.md).
set -euo pipefail

RUN=/run/confos
DEV=/dev/disk/by-label/joindata

# Unless the disk already enumerated, drain the udev queue so a late one
# can't default an agent boot to server; a settle timeout fails the unit.
[[ -e "$DEV" ]] || udevadm settle --timeout=10

if [[ ! -e "$DEV" ]]; then
    : > "$RUN/role-server"
    echo "rke2-role: no joindata disk, defaulting to server"
    exit 0
fi

MNT="$RUN/joindata"
mkdir -p "$MNT"
# Host-controlled device: pin the fs parser, bound a wedged mount.
timeout 10 mount -t iso9660 -o ro,nodev,nosuid,noexec "$DEV" "$MNT"
trap 'umount "$MNT" 2>/dev/null || true' EXIT

# Edge-trim only: interior whitespace survives and fails validation
# below rather than being silently repaired.
role=""
IFS=$' \t\r\n' read -r role < "$MNT/role" || true
case "$role" in
server)
    : > "$RUN/role-server"
    ;;
agent)
    server_addr=""
    IFS=$' \t\r\n' read -r server_addr < "$MNT/server" || true
    if [[ -z "$server_addr" ]]; then
        echo "rke2-role: agent role but no server address" >&2
        exit 1
    fi
    printf 'JOIN_SERVER=%s\n' "$server_addr" > "$RUN/join.env"
    : > "$RUN/role-agent"
    ;;
*)
    echo "rke2-role: invalid role '${role}'" >&2
    exit 1
    ;;
esac
echo "rke2-role: role=${role}"
