# Steep Concepts Guide

A ground-up explanation of every concept behind steep's image building and measured boot pipeline.

---

## 1. What happens when a computer turns on?

When you press the power button, the CPU has no OS, no Linux, nothing. It starts executing **firmware** — code burned into a chip on the motherboard. On physical machines this is BIOS/UEFI. On VMs, this is **OVMF** (an open-source UEFI implementation for virtual machines).

The firmware's only job: find something bootable and hand off control to it.

---

## 2. BIOS vs UEFI

**BIOS** (Basic Input/Output System) is the original firmware from the 1980s. It's extremely simple: it looks at the first 512 bytes of a disk (the "master boot record"), loads that into memory, and jumps to it. That's it. 512 bytes. The bootloader had to fit in 512 bytes or chain-load something bigger. It could only address 1MB of memory at startup, only worked with MBR partition tables (max 2TB disks, max 4 partitions), and had no concept of filesystems — it just read raw disk sectors.

**UEFI** (Unified Extensible Firmware Interface) — the U is "Unified" because it was originally Intel's EFI spec, then the industry unified around it as UEFI. It's a complete replacement for BIOS, designed in the late 90s/2000s. The key differences:

- UEFI understands the **FAT32 filesystem**. It can navigate directories and read files by name.
- UEFI loads **`.efi` executables** — actual structured binaries, not raw machine code at a disk offset.
- It supports GPT partition tables (128 partitions, disks >2TB).
- It has a shell, drivers, network stack, secure boot (signature verification of what it loads).

The boot difference:

```
BIOS:  read first 512 bytes of disk -> jump to it -> hope for the best
UEFI:  read FAT32 partition -> find .efi file -> load and execute a proper binary
```

---

## 3. What is an EFI binary?

An `.efi` file is a **PE (Portable Executable)** binary — the same format as Windows `.exe` files. Microsoft designed PE, and when Intel designed EFI they picked PE as the executable format (Intel and Microsoft were close collaborators on this).

So an `.efi` file is literally a PE executable that targets the UEFI runtime environment instead of Windows. The firmware knows how to parse the PE headers, load the sections into memory, and jump to the entry point.

When we build a UKI, we're taking:
- A PE stub (`linuxx64.efi.stub` — a tiny EFI program that knows how to unpack and boot a Linux kernel)
- The kernel, initrd, and cmdline as data sections stuffed into that PE binary

The result is a single `.efi` file that the firmware loads like any other EFI application, but internally it unpacks and boots Linux.

---

## 4. What is vmlinuz?

`vmlinuz` = **V**irtual **M**emory **linux** compre**z**sed.

History:
- Original Unix kernel was just called `unix`
- When virtual memory support was added, it became `vmunix`
- Linux copied the naming: `vmlinux` = the uncompressed kernel ELF binary
- `vmlinuz` = the compressed version (the `z` = zlib/gzip compression)

`vmlinuz` is the compressed Linux kernel binary. On a system it lives at something like `/boot/vmlinuz-6.17.0-14-generic`. It's ~14MB compressed, ~40MB uncompressed. It contains the core kernel code: scheduler, memory management, filesystem VFS layer, network stack, and whatever drivers were compiled in (vs loaded as modules).

---

## 5. Kernel modules and .ko files

The kernel has thousands of possible drivers — every filesystem, every network card, every USB device, every crypto algorithm. Compiling them all into vmlinuz would make it enormous and waste memory (why load the Bluetooth driver on a server?).

So most drivers are compiled as **modules** — separate files that can be loaded on demand.

`.ko` = **K**ernel **O**bject. It's basically a shared library (`.so`) but for the kernel. The format is ELF (same as regular Linux binaries) but it links against kernel symbols instead of libc.

```
/usr/lib/modules/6.17.0-14-generic/
  kernel/
    drivers/
      md/
        dm-verity.ko.zst       <-- dm-verity driver, zstd-compressed
        dm-bufio.ko.zst
    fs/
      overlay/
        overlay.ko.zst         <-- overlayfs driver
```

You load them with `insmod /path/to/module.ko` or `modprobe module-name` (which also handles dependencies). Once loaded, the driver is part of the running kernel — it can register device types, filesystems, crypto algorithms, whatever it provides.

Our initrd carries specific `.ko` files for dm-verity, dm-bufio (buffer I/O layer that dm-verity needs), and overlay. The init script loads them with `insmod` before trying to set up verity or overlayfs.

---

## 6. Device naming: /dev, /dev/vda, /dev/mapper

### /dev

`/dev` is where Linux exposes hardware as files. Everything in Unix is a file — including disks, partitions, terminals, random number generators:

```
/dev/sda      <-- first SCSI/SATA disk
/dev/sda1     <-- first partition of sda
/dev/sda2     <-- second partition
/dev/nvme0n1  <-- first NVMe drive
/dev/ttyS0    <-- first serial port
/dev/null     <-- the void
/dev/random   <-- random bytes
```

### Why vda specifically?

The prefix tells you the driver/bus type:

```
sd   = SCSI disk      (also SATA, USB)     -> /dev/sda, /dev/sdb
hd   = IDE disk       (legacy)             -> /dev/hda, /dev/hdb
nvme = NVMe           (PCIe SSDs)          -> /dev/nvme0n1
vd   = virtio disk    (paravirtualized)    -> /dev/vda, /dev/vdb
```

**virtio** is a standard for VM I/O. Instead of emulating a real SATA controller (slow — the guest thinks it's talking to real hardware, every register access traps to the hypervisor), virtio provides a simplified interface designed for VMs. The guest knows it's in a VM and uses a purpose-built driver that batches I/O efficiently.

So `vda` = "first **v**irtio **d**isk, device **a**". `vda2` = second partition of that disk.

In our QEMU command we have:

```
-drive file=disk.raw,format=raw,if=virtio
```

The `if=virtio` part means "attach this disk via the virtio bus" — which is why it shows up as `/dev/vda` inside the guest rather than `/dev/sda`.

### /dev/mapper

`/dev/mapper/` is where the **device mapper** subsystem creates virtual block devices. Device mapper is a kernel framework that sits between the filesystem and the real disk, transforming I/O in some way:

- **dm-crypt** -> encrypts/decrypts blocks -> `/dev/mapper/my-encrypted-vol`
- **dm-verity** -> verifies block integrity -> `/dev/mapper/root`
- **dm-linear** -> concatenates disks (LVM uses this) -> `/dev/mapper/vg-lv`
- **dm-cache** -> SSD caching for HDDs
- **dm-raid** -> software RAID

When our init script runs:

```bash
veritysetup open /dev/vda2 root /dev/vda3 "$ROOTHASH"
```

This tells device mapper: "Create a new virtual device called `root` that reads data from `/dev/vda2`, checks it against hashes on `/dev/vda3`, using this root hash." The result appears at `/dev/mapper/root`.

From the filesystem's perspective, `/dev/mapper/root` is just another block device — you mount it like any disk. But under the hood, every read goes through the verity verification layer before reaching userspace.

```
App reads /etc/hostname
    |
ext4 on /dev/mapper/root -> "I need block 12345"
    |
dm-verity -> reads block 12345 from /dev/vda2
          -> reads hash for block 12345 from /dev/vda3
          -> verifies hash matches
          -> if good: return data
          -> if bad: return I/O error (corruption detected)
```

---

## 7. What is an initrd?

The **initrd** (initial ramdisk) solves a chicken-and-egg problem:

- The kernel needs drivers to read the disk that the root filesystem is on
- Those drivers might be kernel modules stored on the root filesystem
- You can't read the modules without mounting the filesystem first

The initrd is a small compressed archive (cpio.gz format) that gets loaded into RAM alongside the kernel. It contains:

- A mini userspace (just enough to set up the real root)
- The kernel modules needed to access the real disk
- An `/init` script that does the setup work

The boot chain:

```
Firmware -> loads kernel + initrd into RAM -> kernel runs -> kernel unpacks initrd -> kernel executes /init from initrd
```

The initrd's `/init` script does whatever prep is needed (load drivers, decrypt disks, set up dm-verity), then **switches to the real root filesystem** and hands off to systemd.

---

## 8. Why steep builds TWO things — an initrd AND an image

Because they serve completely different purposes.

**The initrd** (`mkosi/initrd/`) is a tiny throwaway environment. It exists only during the first few seconds of boot. Ours contains:
- `cryptsetup` / `veritysetup` — the tool that sets up dm-verity
- `kmod` — for loading kernel modules (dm-verity, ext4, virtio drivers)
- `bash` — to run the init script
- Our custom `/init` script

That's it. ~50MB compressed. It does its job and is discarded.

**The image** (`mkosi/base/`) is the actual operating system — Ubuntu with systemd, cloud-init, networking, SSH, everything. This becomes the root filesystem that runs for the lifetime of the VM.

They're built separately because:
1. The initrd needs to be **embedded inside the UKI** (more on that next)
2. The image needs the initrd to be **passed to mkosi at build time** so mkosi can bundle it into the UKI
3. They have totally different package sets — you don't want `vim` and `openssh-server` in your initrd

---

## 9. What is a UKI?

A **Unified Kernel Image** bundles three things into a single `.efi` binary:

1. The Linux kernel
2. The initrd
3. The kernel command line (boot parameters)

```
UKI (.efi) = kernel + initrd + "roothash=abc123 console=ttyS0 ..."
```

Why bundle them? Two reasons:

1. **Single file to boot** — the firmware loads one `.efi` file and everything is there. No bootloader needed, no separate files to manage.
2. **Measurement** — for confidential computing, we need to measure (hash) everything the VM will execute. One file = one measurement. If the kernel, initrd, or command line changes, the measurement changes.

The `roothash=...` in the kernel command line is critical — it binds the UKI to a specific root filesystem. Our init script parses it and passes it to `veritysetup`. Change one file in the root filesystem -> different roothash -> different UKI -> different IGVM measurement.

---

## 10. What is dm-verity?

A Linux kernel feature that provides transparent integrity checking of block devices. It creates a hash tree over the entire root filesystem — every 4K block has a hash, and those hashes roll up to a single **root hash**. At read time, every block is verified against the hash tree. If a single byte is tampered with, the read fails.

The 3-partition layout of our disk:

```
Partition 1 (ESP):         EFI boot files (FAT32)
Partition 2 (root-data):   The ext4 root filesystem, read-only
Partition 3 (root-verity): The hash tree for partition 2
```

When the init script runs `veritysetup open /dev/vda2 root /dev/vda3 <roothash>`, it creates `/dev/mapper/root` — a virtual block device where every read from the data partition is checked against the hash partition.

---

## 11. What is overlayfs and copy-on-write?

### The problem

dm-verity makes the root filesystem **physically read-only**. Not just "please don't write" — the kernel will refuse any write operation to `/dev/mapper/root`. If systemd tries to write to `/var/log/syslog`, it gets an error. If cloud-init tries to write a config file, error. The system can't even boot properly because systemd needs to write to dozens of places during startup.

So we need writes to work, but we can't write to the real disk.

### The naive solution (and why it's wrong)

You might think: "Just mount a tmpfs at `/var/log` and another at `/tmp` and another at `/run`..."

But that doesn't scale. Cloud-init needs to write to `/etc/`. Package managers write to `/usr/`. Random programs write to random places. You'd need a tmpfs mount for every possible writable path, and you'd miss some.

### What overlayfs actually does

Overlayfs merges two directories into one view:

```
/sysroot-lower/     (the dm-verity root, read-only)
    etc/
        hostname        -> "steep"
        resolv.conf     -> "nameserver 10.0.2.3"
    usr/
        bin/
            bash
            vim
    var/
        log/

/sysroot-upper/     (empty tmpfs, writable)
    (nothing yet)

/sysroot/           (the merged overlay -- this is what the OS actually uses)
    etc/
        hostname        -> comes from lower (steep)
        resolv.conf     -> comes from lower (nameserver 10.0.2.3)
    usr/
        bin/
            bash        -> comes from lower
            vim         -> comes from lower
    var/
        log/
```

At boot, the upper layer is **empty**. Every file the OS reads comes from the lower layer (dm-verity verified). The system looks and behaves exactly like the image we built.

### What happens on a write (new file)

Say cloud-init runs and writes a new `/etc/myapp.conf`:

```
Write: /sysroot/etc/myapp.conf -> "some config"
```

Overlayfs sees this write and puts the file in the **upper layer**:

```
/sysroot-upper/
    etc/
        myapp.conf      -> "some config"      <-- NEW, lives in tmpfs (RAM)

/sysroot-lower/
    etc/
        hostname        -> "steep"             <-- untouched
        resolv.conf     -> "nameserver 10.0.2.3"  <-- untouched
```

When anything reads `/sysroot/etc/myapp.conf`, overlayfs finds it in the upper layer and returns it. When anything reads `/sysroot/etc/hostname`, overlayfs doesn't find it in upper, so it reads from lower (dm-verity verified).

### Copy-on-write: modifying an existing file

This is where "copy-on-write" comes in. Say systemd-resolved wants to update `/etc/resolv.conf` which already exists in the lower layer:

```
Write: /sysroot/etc/resolv.conf -> "nameserver 8.8.8.8"
```

Overlayfs **cannot modify the lower layer** (it's read-only). So it:

1. **Copies** the entire file from lower to upper
2. **Writes** the modification to the upper copy

```
/sysroot-upper/
    etc/
        myapp.conf      -> "some config"
        resolv.conf     -> "nameserver 8.8.8.8"   <-- COPIED UP then modified

/sysroot-lower/
    etc/
        hostname        -> "steep"
        resolv.conf     -> "nameserver 10.0.2.3"  <-- still the original, untouched
```

Now when anything reads `/sysroot/etc/resolv.conf`, overlayfs finds the upper copy first and returns "nameserver 8.8.8.8". The lower layer's original is still there, still verified by dm-verity, just hidden — "shadowed" by the upper copy.

That's the "copy on write" — the file isn't modified in place, it's copied up to the writable layer on the first write.

### Deleting a file

What if you `rm /sysroot/usr/bin/vim`? Overlayfs can't remove it from the read-only lower layer. Instead it creates a **whiteout file** in the upper layer — a special marker that says "pretend this doesn't exist":

```
/sysroot-upper/
    usr/
        bin/
            .wh.vim     <-- whiteout: hides vim from the merged view

/sysroot-lower/
    usr/
        bin/
            vim         <-- still physically here, still verified
```

Now `ls /sysroot/usr/bin/vim` returns "no such file". But the original is untouched on disk.

### Why this matters for security

The lower layer — the dm-verity verified root — is **never modified**. Not by writes, not by deletes, not by anything. Every block that gets read from the real disk is still verified against the hash tree.

If an attacker somehow got code execution in the VM and modified `/usr/bin/bash` — that modification lives in the tmpfs upper layer (RAM). The real `/usr/bin/bash` on disk is still intact and verified. And on reboot, the tmpfs is gone — the upper layer starts empty again, and the system is back to the exact verified state.

```
Reboot cycle:

Boot 1:  upper = empty    -> OS runs from verified lower
         (runtime writes accumulate in upper)

Boot 2:  upper = empty    -> everything from boot 1 is gone
         (fresh start from verified lower)
```

The tradeoff: **nothing persists across reboots**. Logs, config changes, installed packages — all gone. That's by design for a confidential VM where you want a known-good state every boot. If you need persistence, you'd attach a separate data disk that isn't part of the verified root.

---

## 12. What is mkosi?

mkosi is a tool for building OS images declaratively. Think of it like a Dockerfile but for bare-metal/VM disk images. You write `mkosi.conf` with a distro, packages, and output format, and it produces a disk image.

Steep uses mkosi twice:

1. **`mkosi/initrd/`** — Builds the minimal cpio initrd (just cryptsetup, kmod, bash, util-linux). Output: `image.cpio.gz`.
2. **`mkosi/base/`** — Builds the full Ubuntu disk image with 3 partitions (ESP + root-data + root-verity-hash). Output: `image.raw`, `image.efi` (UKI), `image.roothash`.

Key mkosi directories:
- `mkosi.conf` — the build config (distro, packages, output format)
- `mkosi.extra/` — files overlaid into the image (like `etc/hostname`, systemd presets)
- `mkosi.repart/` — partition definitions (using systemd-repart)

---

## 13. What is IGVM?

IGVM (Independent Guest Virtual Machine) is a format that bundles firmware (OVMF) + UKI + VM configuration into a single measured package for confidential VMs (SEV-SNP).

When QEMU loads an IGVM file, the hypervisor measures every page into the SNP launch digest. A remote verifier can check this digest to prove the VM is running exactly the expected code. Nobody — not the cloud provider, not the hypervisor — can tamper with it without the measurement changing.

---

## 14. Image vs kernel vs guest OS vs UKI — glossary

These terms describe different layers:

**Kernel** (`vmlinuz`) — Just the Linux kernel binary. ~14MB. Manages hardware and runs processes but has no userspace — no bash, no systemd, no files. Useless on its own.

**Root filesystem** (rootfs) — The directory tree of the OS: `/bin`, `/etc`, `/usr`, `/var`. Contains all the programs, config files, libraries. This is what you interact with.

**Disk image** (`image.raw`) — A file that contains a complete disk: partition table + partitions. Ours has 3 partitions (ESP + root + verity hash). Think of it as a virtual hard drive saved to a file.

**Guest OS** — The complete operating system running inside a VM. It's the kernel + rootfs + all the boot infrastructure. Not a file, just a concept.

**initrd** — The temporary mini-filesystem loaded into RAM at boot, used only to set up the real root. Discarded after boot.

**UKI** (`image.efi`) — kernel + initrd + boot parameters bundled into one EFI binary. It's the thing the firmware actually loads.

**IGVM** (`guest.igvm`) — firmware (OVMF) + UKI bundled into a measured package for confidential VMs. The outermost layer.

The nesting:

```
IGVM contains:
  +-- OVMF firmware
  +-- UKI (.efi) contains:
        +-- kernel (vmlinuz)
        +-- initrd (cpio.gz) contains:
        |     +-- /init script
        |     +-- veritysetup, kernel modules
        +-- cmdline ("roothash=abc123...")

Disk image (.raw) contains:
  +-- Partition 1: ESP (copy of UKI for firmware to find)
  +-- Partition 2: root filesystem (ext4, the actual OS)
  +-- Partition 3: verity hash tree
```

The IGVM and the disk image are separate files. QEMU loads the IGVM (which boots the kernel), and the kernel mounts the disk image as its storage.

---

## 15. The full boot sequence in steep

```
1. QEMU loads IGVM
      |
2. IGVM contains OVMF firmware + UKI
      |
3. Firmware starts, finds UKI on ESP, executes it
      |
4. Kernel starts with initrd in RAM
      |
5. /init runs:
   a. Load kernel modules (dm-verity, ext4, virtio)
   b. Parse roothash from kernel cmdline
   c. Wait for /dev/vda2 to appear
   d. veritysetup open /dev/vda2 root /dev/vda3 <roothash>
   e. Mount /dev/mapper/root read-only -> /sysroot-lower
   f. Mount tmpfs overlay -> /sysroot (writes go to RAM)
   g. Switch root to /sysroot
      |
6. systemd starts from the verified root
      |
7. cloud-init runs (if configured), networking comes up, services start
```

The whole point: step 2 is **measured** into the SNP hardware. A remote verifier can check the launch digest and know with certainty what firmware, kernel, initrd, command line (including roothash), and therefore what root filesystem is running inside the VM. Nobody — not the cloud provider, not the hypervisor — can tamper with it without the measurement changing.

---

## 16. The measurement chain

This is what makes the whole system trustworthy for remote attestation:

```
cloud-init YAML  (included in image)
       |
       v
ext4 root filesystem  (contains all OS files + cloud-init config)
       |
       v
dm-verity root hash   (single hash representing entire root filesystem)
       |
       v
UKI kernel cmdline    (roothash=<hash> baked into the UKI)
       |
       v
IGVM launch digest    (measurement of firmware + UKI by SNP hardware)
```

Change one file in the root -> different roothash -> different UKI -> different IGVM launch digest. A remote verifier checks the launch digest and can trust the entire stack.
