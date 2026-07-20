#!/bin/sh
# Create the NVIDIA IMEX channel-0 device node.
#
# nvidia-modprobe -c 0 registers the nvidia-caps-imex-channels chardev; its major
# is dynamically allocated and appears in /proc/devices. Create /dev/.../channel0
# at that major so B200/NVSwitch multi-GPU can init the fabric (without it CUDA
# fails "error 802: system not yet initialized"). Idempotent.
set -eu

CHANNEL=/dev/nvidia-caps-imex-channels/channel0

/usr/bin/nvidia-modprobe -c 0

[ -e "$CHANNEL" ] && exit 0

maj=$(awk '/nvidia-caps-imex-channels/ {print $1}' /proc/devices)
[ -n "$maj" ] || { echo "nvidia-imex-channel: no imex major in /proc/devices" >&2; exit 1; }

mkdir -p /dev/nvidia-caps-imex-channels
mknod "$CHANNEL" c "$maj" 0
chmod 0666 "$CHANNEL"
