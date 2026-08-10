#!/bin/bash
# Role dispatch, run by rke2-role.service before any role-gated unit.
# Reads an optional disk labelled `joindata` (files: role, server) and
# writes /run/confos/role-{server,agent} plus join.env for rke2-join.
# No disk => server, the single-node default.
#
# The disk is host-controlled and DoS-only: a flipped role fails to join or
# forms an empty cluster; a redirected server fails `c8s join`'s same-image
# verification. A malformed disk fails this unit and every role-gated unit
# stays down.
set -euo pipefail

RUN=/run/confos
DEV=/dev/disk/by-label/joindata
mkdir -p "$RUN"

# Drain the udev queue so a late-enumerating joindata disk can't make an
# agent boot silently default to server.
udevadm settle --timeout=10 || true

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

role=$(tr -d '[:space:]' < "$MNT/role")
case "$role" in
server)
    : > "$RUN/role-server"
    ;;
agent)
    server_addr=$(tr -d '[:space:]' < "$MNT/server")
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
