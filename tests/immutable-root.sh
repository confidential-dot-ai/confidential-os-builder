#!/bin/bash
# Run with root on Linux, with the image's cloud-init package installed.
# All production scripts and absolute cloud-init paths run in a disposable
# chroot; only actual hardware setup and the final switch_root are replaced.
set -euo pipefail
test_script=$(realpath "${BASH_SOURCE[0]}")
cd "$(dirname "$test_script")/.."

if [ "$EUID" -ne 0 ] || [ "$(uname -s)" != Linux ]; then
    echo "tests/immutable-root.sh requires root on Linux" >&2
    exit 1
fi
if [ "${1:-}" != --private-mount-namespace ]; then
    exec unshare --mount --propagation private /bin/bash "$test_script" --private-mount-namespace
fi

fixture_root=$(mktemp -d)
cleanup() {
    local status=$?
    trap - EXIT
    # Unmount everything before removing the empty directory. Never recursively
    # delete a fixture containing bind mounts of host paths.
    if mountpoint -q "$fixture_root"; then
        if ! umount -R "$fixture_root"; then
            echo "Failed to unmount test root: $fixture_root" >&2
            exit 1
        fi
    fi
    rmdir "$fixture_root"
    exit "$status"
}
trap cleanup EXIT
mount -t tmpfs tmpfs "$fixture_root"
mkdir -p "$fixture_root"/{usr,dev,proc,sys/class/dmi/id,run,tmp,etc,var/lib}
mount --bind /usr "$fixture_root/usr"
mount -o remount,bind,ro "$fixture_root/usr"
# Keep the host /usr immutable while supplying hardware boundary shims.
mount -t tmpfs tmpfs "$fixture_root/usr/sbin"
for directory in bin sbin lib; do
    ln -s "usr/$directory" "$fixture_root/$directory"
done
if [ -d /usr/lib64 ]; then
    ln -s usr/lib64 "$fixture_root/lib64"
fi
touch "$fixture_root/confos-test-root" "$fixture_root/dev/null" "$fixture_root/dev/urandom"
mount --bind /dev/null "$fixture_root/dev/null"
mount --bind /dev/urandom "$fixture_root/dev/urandom"
# Root disk on NVMe (as on GCE), a SCSI disk without an Azure data-disk
# address and the scratch disk on virtio-blk (serial confai-scratch). Tests
# that need other disks boot again with their own /sys (boot_with_disks).
mknod "$fixture_root/dev/nvme0n1" b 259 0
mknod "$fixture_root/dev/sda" b 8 0
mknod "$fixture_root/dev/vdb" b 253 16
mkdir -p "$fixture_root"/sys/block/{nvme0n1/nvme0n1p2,sda,vdb} "$fixture_root/sys/class/block"
echo confai-scratch > "$fixture_root/sys/block/vdb/serial"
ln -s ../../block/nvme0n1/nvme0n1p2 "$fixture_root/sys/class/block/nvme0n1p2"
printf 'roothash=58ada21469e0c4fe0cf5f9f77e4295f82d510b566286aa0b09a4c044faf842ba\n' > "$fixture_root/proc/cmdline"
printf '0.00 0.00\n' > "$fixture_root/proc/uptime"
# A malformed test boot must never write the real host's sysrq-trigger.
touch "$fixture_root/proc/sysrq-trigger"
cp -a /etc/cloud "$fixture_root/etc/"
cp /etc/os-release "$fixture_root/etc/os-release"
mkdir "$fixture_root/etc/default"
cp mkosi/base/mkosi.extra/etc/locale.conf "$fixture_root/etc/locale.conf"
ln -s ../locale.conf "$fixture_root/etc/default/locale"
cp mkosi/base/mkosi.extra/etc/cloud/cloud.cfg.d/99-steep.cfg \
    "$fixture_root/etc/cloud/cloud.cfg.d/99-steep.cfg"
mkdir "$fixture_root/default-state.d"
cp mkosi/base/mkosi.extra/usr/lib/confai/state.d/*.conf "$fixture_root/default-state.d/"
cp mkosi/base/mkosi.finalize "$fixture_root/finalize-under-test"
cp mkosi/initrd/mkosi.extra/init "$fixture_root/init-under-test"
cp mkosi/base/mkosi.profiles/gcp/mkosi.extra/etc/cloud/cloud.cfg.d/99-zz-gcp.cfg "$fixture_root/gcp.cfg"
cp tests/immutable_root.py "$fixture_root/tests.py"

cat > "$fixture_root/usr/sbin/veritysetup" <<'SHIM'
#!/bin/bash
printf '%s\n' "$@" > /veritysetup-args
SHIM
cat > "$fixture_root/usr/sbin/cryptsetup" <<'SHIM'
#!/bin/bash
printf '%s\n' "$@" > /cryptsetup-args
SHIM
cat > "$fixture_root/usr/sbin/mkfs.ext4" <<'SHIM'
#!/bin/bash
exit 0
SHIM
cat > "$fixture_root/usr/sbin/blkid" <<'SHIM'
#!/bin/bash
# The partition UUIDs systemd-repart wrote for the cmdline roothash (read from
# the real image with sfdisk), on an NVMe disk.
case "$*" in
    "-l -o device -t PARTUUID=58ada214-69e0-c4fe-0cf5-f9f77e4295f8") echo /dev/nvme0n1p2; exit 0 ;;
    "-l -o device -t PARTUUID=2d510b56-6286-aa0b-09a4-c044faf842ba") echo /dev/nvme0n1p3; exit 0 ;;
esac
# No operator-key disk. Expose both NoCloud and ConfigDrive disks to detection.
if [ "${1:-}" = -L ]; then
    exit 1
fi
printf 'DEVNAME=/dev/vdb\nLABEL=cidata\nTYPE=iso9660\n\nDEVNAME=/dev/vdc\nLABEL=config-2\nTYPE=iso9660\n'
SHIM
cat > "$fixture_root/usr/sbin/mount" <<'SHIM'
#!/bin/bash
set -euo pipefail
printf '%s\n' "$*" >> /mount-calls
case "$*" in
    "-t proc proc /proc"|"-t sysfs sysfs /sys"|"-t devtmpfs devtmpfs /dev")
        exit 0 ;;
    "/dev/mapper/scratch /state")
        exec /usr/bin/mount -t tmpfs tmpfs /state ;;
    "-o ro /dev/mapper/root /sysroot")
        /usr/bin/mount --bind /image /sysroot
        exec /usr/bin/mount -o remount,bind,ro /sysroot ;;
    *)
        exec /usr/bin/mount "$@" ;;
esac
SHIM
cat > "$fixture_root/usr/sbin/switch_root" <<'SHIM'
#!/bin/bash
printf '%s\n' "$@" > /switch-root-args
SHIM
chmod +x "$fixture_root"/usr/sbin/*
chroot "$fixture_root" /usr/bin/python3 /tests.py
