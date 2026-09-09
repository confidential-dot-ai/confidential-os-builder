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
touch "$fixture_root/confos-test-root" "$fixture_root/dev/null" "$fixture_root/dev/vda2"
mount --bind /dev/null "$fixture_root/dev/null"
printf 'roothash=%064d\n' 0 > "$fixture_root/proc/cmdline"
printf '0.00 0.00\n' > "$fixture_root/proc/uptime"
# A malformed test boot must never write the real host's sysrq-trigger.
touch "$fixture_root/proc/sysrq-trigger"
cp -a /etc/cloud "$fixture_root/etc/"
cp /etc/os-release "$fixture_root/etc/os-release"
cp mkosi/base/mkosi.extra/etc/cloud/cloud.cfg.d/99-steep.cfg \
    "$fixture_root/etc/cloud/cloud.cfg.d/99-steep.cfg"
mkdir "$fixture_root/default-state.d"
cp mkosi/base/mkosi.extra/usr/lib/confai/state.d/*.conf "$fixture_root/default-state.d/"
cp mkosi/base/mkosi.finalize "$fixture_root/finalize-under-test"
cp mkosi/initrd/mkosi.extra/init "$fixture_root/init-under-test"
cp tests/immutable_root.py "$fixture_root/tests.py"

cat > "$fixture_root/usr/sbin/veritysetup" <<'SHIM'
#!/bin/bash
exit 0
SHIM
cat > "$fixture_root/usr/sbin/blkid" <<'SHIM'
#!/bin/bash
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
