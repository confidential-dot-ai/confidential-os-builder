#!/bin/bash
# Role dispatch, run by rke2-role.service before any role-gated unit.
# Reads an optional disk labelled `joindata` (files: role, server) and
# writes /run/confos/role-{server,agent} plus join.env for rke2-join.
# No disk => server, the single-node default.
#
# joindata is host-controlled: worst case is DoS; a malformed disk fails
# this unit and every role-gated unit stays down.
set -euo pipefail

RUN=/run/confos
DEV=/dev/disk/by-label/joindata
AGENT_TOKEN_FILE=$RUN/rke2-agent-token

ensure_agent_token() {
    local token tmp

    if [[ -e "$AGENT_TOKEN_FILE" || -L "$AGENT_TOKEN_FILE" ]]; then
        if [[ -L "$AGENT_TOKEN_FILE" || ! -f "$AGENT_TOKEN_FILE" ]]; then
            echo "rke2-role: agent token path is not a regular file" >&2
            return 1
        fi
        token=$(<"$AGENT_TOKEN_FILE")
        if [[ ! "$token" =~ ^[0-9a-f]{64}$ ]]; then
            echo "rke2-role: existing agent token is malformed" >&2
            return 1
        fi
        return
    fi

    tmp=$(mktemp "$RUN/.rke2-agent-token.XXXXXX")
    if ! od -An -N32 -tx1 /dev/urandom | tr -d ' \n' > "$tmp"; then
        rm -f "$tmp"
        return 1
    fi
    token=$(<"$tmp")
    if [[ ! "$token" =~ ^[0-9a-f]{64}$ ]]; then
        echo "rke2-role: generated agent token is malformed" >&2
        rm -f "$tmp"
        return 1
    fi
    chmod 0600 "$tmp"
    mv -f "$tmp" "$AGENT_TOKEN_FILE"
}

set_server_role() {
    # RKE2 otherwise aliases agent-token to its privileged server token.
    # Generate a boot-local, agent-only secret before rke2-server starts.
    ensure_agent_token
    : > "$RUN/role-server"
}

# Unless the disk already enumerated, drain the udev queue so a late one
# can't default an agent boot to server; a settle timeout fails the unit.
[[ -e "$DEV" ]] || udevadm settle --timeout=10

if [[ ! -e "$DEV" ]]; then
    set_server_role
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
    set_server_role
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
