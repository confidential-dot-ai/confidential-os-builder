#!/bin/bash
# Role dispatch, run by rke2-role.service before any role-gated unit.
# Reads an optional `joindata` disk and stages the RKE2 role, tokens, and
# node addresses. No disk => server (the single-node default).
#
# joindata is host-controlled: worst case is DoS. A malformed disk fails this
# unit and every role-gated unit stays down. Disk text is never sourced or
# evaluated; it is validated field by field and rejected on anything unexpected.
#
# joindata contract (v0), all files a single <=256-byte ASCII line:
#   role              server|agent            (both roles)
#   node-ip           routable IPv4           (both roles)
#   node-external-ip  routable IPv4, optional (both roles)
#   server            IPv4, no scheme/port    (agent only; forbidden on server)
#   server-token      64 lowercase hex        (server only; forbidden on agent)
#   agent-token       64 lowercase hex        (both roles)
set -euo pipefail

RUN=/run/confos
DEV=/dev/disk/by-label/joindata
MNT=$RUN/joindata
FRAG=/etc/rancher/rke2/config.yaml.d/50-role.yaml

fail() { echo "rke2-role: $1" >&2; exit 1; }

# read_field FILE — echo the single validated line of MNT/FILE. Rejects a
# missing/symlink/non-regular file, NUL bytes, an over-long file, more than
# one line, and any interior whitespace (edges are trimmed below).
read_field() {
    local path="$MNT/$1" line
    [[ -f "$path" && ! -L "$path" ]] || fail "$1: missing or not a regular file"
    # Bound the read before slurping: a huge host file must not fill RAM.
    (( $(stat -c%s "$path") <= 257 )) || fail "$1: larger than one 256-byte line"
    # tr, not grep: a NUL can't be a grep pattern argument, and busybox grep
    # lacks -P. If stripping NULs shortens the file, one was present.
    (( $(tr -d '\0' < "$path" | wc -c) == $(stat -c%s "$path") )) || fail "$1: contains NUL"
    # A well-formed field is one line plus an optional trailing newline, so at
    # most one newline total; anything more is a multi-line file.
    (( $(tr -cd '\n' < "$path" | wc -c) <= 1 )) || fail "$1: more than one line"
    IFS= read -r line < "$path" || true
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line%"${line##*[![:space:]]}"}"
    (( ${#line} <= 256 )) || fail "$1: line exceeds 256 bytes"
    [[ "$line" == *[[:space:]]* ]] && fail "$1: interior whitespace"
    printf '%s' "$line"
}

is_hex_token() { [[ "$1" =~ ^[0-9a-f]{64}$ ]]; }

is_ipv4() {
    local ip="$1" o IFS=.
    [[ "$ip" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || return 1
    for o in $ip; do (( o <= 255 )) || return 1; done
}

# forbid FILE — the file must not be present for this role.
forbid() { [[ -e "$MNT/$1" ]] && fail "$1: forbidden for this role"; return 0; }

# write_atomic PATH MODE < content
write_atomic() {
    local dest="$1" mode="$2" tmp
    tmp=$(mktemp "${dest%/*}/.${dest##*/}.XXXXXX")
    cat > "$tmp"
    chmod "$mode" "$tmp"
    mv -f "$tmp" "$dest"
}

# Common address fields, validated the same way for both roles.
stage_addresses() {
    local node_ip node_ext
    node_ip=$(read_field node-ip)
    is_ipv4 "$node_ip" || fail "node-ip: not IPv4"
    NODE_IP="$node_ip"
    NODE_EXT=""
    if [[ -e "$MNT/node-external-ip" ]]; then
        node_ext=$(read_field node-external-ip)
        is_ipv4 "$node_ext" || fail "node-external-ip: not IPv4"
        NODE_EXT="$node_ext"
    fi
}

emit_node_addr_lines() {
    printf 'node-ip: %s\n' "$NODE_IP"
    [[ -n "$NODE_EXT" ]] && printf 'node-external-ip: %s\n' "$NODE_EXT"
}

set_server_role() {
    local server_token agent_token
    forbid server
    server_token=$(read_field server-token)
    agent_token=$(read_field agent-token)
    is_hex_token "$server_token" || fail "server-token: not 64 lowercase hex"
    is_hex_token "$agent_token" || fail "agent-token: not 64 lowercase hex"
    [[ "$server_token" != "$agent_token" ]] || fail "server-token equals agent-token"
    stage_addresses

    printf '%s' "$server_token" | write_atomic "$RUN/rke2-server-token" 0600
    printf '%s' "$agent_token" | write_atomic "$RUN/rke2-agent-token" 0600
    {
        printf 'token-file: %s\n' "$RUN/rke2-server-token"
        printf 'agent-token-file: %s\n' "$RUN/rke2-agent-token"
        emit_node_addr_lines
    } | write_atomic "$FRAG" 0600
    : > "$RUN/role-server"
}

set_agent_role() {
    local server_addr agent_token
    forbid server-token
    server_addr=$(read_field server)
    agent_token=$(read_field agent-token)
    is_ipv4 "$server_addr" || fail "server: not IPv4"
    is_hex_token "$agent_token" || fail "agent-token: not 64 lowercase hex"
    stage_addresses

    printf '%s' "$agent_token" | write_atomic "$RUN/rke2-agent-token" 0600
    {
        printf 'token-file: %s\n' "$RUN/rke2-agent-token"
        printf 'server: https://%s:9345\n' "$server_addr"
        emit_node_addr_lines
    } | write_atomic "$FRAG" 0600
    : > "$RUN/role-agent"
}

# No-disk fallback: legacy single-node server. RKE2 otherwise aliases
# agent-token to its privileged server token, so generate a boot-local
# agent-only secret before rke2-server starts.
set_legacy_server_role() {
    local tmp token
    tmp=$(mktemp "$RUN/.rke2-agent-token.XXXXXX")
    od -An -N32 -tx1 /dev/urandom | tr -d ' \n' > "$tmp" || { rm -f "$tmp"; fail "token generation failed"; }
    token=$(<"$tmp")
    is_hex_token "$token" || { rm -f "$tmp"; fail "generated agent token malformed"; }
    chmod 0600 "$tmp"
    mv -f "$tmp" "$RUN/rke2-agent-token"
    : > "$RUN/role-server"
}

# Unless the disk already enumerated, drain the udev queue so a late one
# can't default an agent boot to server; a settle timeout fails the unit.
[[ -e "$DEV" ]] || udevadm settle --timeout=10

if [[ ! -e "$DEV" ]]; then
    set_legacy_server_role
    echo "rke2-role: no joindata disk, defaulting to server"
    exit 0
fi

mkdir -p "$MNT"
# Host-controlled device: pin the fs parser, bound a wedged mount.
timeout 10 mount -t iso9660 -o ro,nodev,nosuid,noexec "$DEV" "$MNT"
trap 'umount "$MNT" 2>/dev/null || true' EXIT

role=$(read_field role)
case "$role" in
server) set_server_role ;;
agent)  set_agent_role ;;
*)      fail "invalid role '${role}'" ;;
esac
echo "rke2-role: role=${role}"
