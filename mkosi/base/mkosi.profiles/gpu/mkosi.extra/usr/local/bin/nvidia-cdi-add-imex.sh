#!/bin/sh
# Fold the NVIDIA IMEX channel deviceNode into every device of a CDI spec.
#
# nvidia-ctk cdi generate --mode=nvml omits the IMEX channel that B200/NVSwitch
# needs (without /dev/nvidia-caps-imex-channels/channel0 CUDA errors 802 "system
# not yet initialized"). Insert it into each device's containerEdits.deviceNodes
# so a pod requesting nvidia.com/gpu gets the channel with no extra resource.
# Idempotent; no-op on non-NVSwitch hosts (channel absent).
set -eu

SPEC="$1"
CHANNEL=/dev/nvidia-caps-imex-channels/channel0

[ -e "$CHANNEL" ] || exit 0                       # not an NVSwitch box
grep -q "$CHANNEL" "$SPEC" && exit 0              # already folded in

MAJOR=$(awk '/nvidia-caps-imex-channels/ {print $1}' /proc/devices)
[ -n "$MAJOR" ] || { echo "nvidia-cdi-add-imex: no imex major in /proc/devices" >&2; exit 1; }

# After each `deviceNodes:` line, emit the channel entry at the list-item indent
# (the key's indent + 4 spaces, matching nvidia-ctk's own layout).
awk -v ch="$CHANNEL" -v maj="$MAJOR" '
  { print }
  /^[[:space:]]*deviceNodes:[[:space:]]*$/ {
    match($0, /^[[:space:]]*/); ind = substr($0, 1, RLENGTH) "    "
    printf "%s- path: %s\n", ind, ch
    printf "%s  major: %s\n", ind, maj
    printf "%s  fileMode: 438\n", ind
    printf "%s  permissions: rwm\n", ind
  }
' "$SPEC" > "$SPEC.tmp"
mv "$SPEC.tmp" "$SPEC"
