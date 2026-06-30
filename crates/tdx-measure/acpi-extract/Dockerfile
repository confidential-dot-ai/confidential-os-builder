# ACPI table extraction from QEMU for TDX measurement computation.
#
# Patches QEMU to dump ACPI tables (acpi_tables.bin), RSDP (rsdp.bin),
# and the table loader script (table_loader.bin) to /output/ at
# acpi_build_update() time, then exits immediately.
#
# Supports both KVM and TCG acceleration. ACPI table generation happens
# during machine init, not in the accelerator, so the tables are
# identical between KVM and TCG for the same machine configuration.
#
# Build:
#   docker build -t acpi-extract .
#
# Run (TCG -- works anywhere):
#   docker run --rm -v /path/to/output:/output \
#     -v /path/to/OVMF.fd:/firmware.fd:ro \
#     acpi-extract -accel tcg -m 2G -smp 2 \
#     -machine q35,kernel-irqchip=split,hpet=off,smm=off,pic=off \
#     -bios /firmware.fd -nographic -nodefaults -serial stdio
#
# Run (KVM -- faster startup):
#   docker run --rm --device /dev/kvm -v /path/to/output:/output \
#     -v /path/to/OVMF.fd:/firmware.fd:ro \
#     acpi-extract -accel kvm -cpu host -m 2G -smp 2 \
#     -machine q35,kernel-irqchip=split,hpet=off,smm=off,pic=off \
#     -bios /firmware.fd -nographic -nodefaults -serial stdio

# Pin the base image by tag (LTS short-lived release window). A digest
# pin would be stricter but Canonical re-publishes 25.04 periodically
# with libc / kernel-headers updates; for an ACPI extractor whose output
# depends on QEMU's source and machine-type emission rather than the
# base userland, the tag is acceptable. Revisit if extraction output
# starts drifting between rebuilds.
FROM ubuntu:25.04

ENV DEBIAN_FRONTEND=noninteractive

# Install build dependencies for QEMU (minimal set for x86_64-softmmu).
RUN apt-get update && \
    apt-get install --no-install-recommends --yes \
        build-essential \
        git \
        ubuntu-dev-tools \
        libglib2.0-dev \
        meson \
        python3-venv && \
    apt-get clean && \
    rm -rf /var/lib/apt/lists/*

# Download QEMU source from Canonical's TDX PPA. This gives us the
# exact same QEMU version that ships in Ubuntu 25.04 TDX packages,
# ensuring the ACPI tables match what a real TDX VM would see.
ARG QEMU_VERSION="1:9.2.1+ds-1ubuntu4+tdx2.0~ppa2"
RUN pull-ppa-source --ppa ppa:kobuk-team/tdx-release qemu ${QEMU_VERSION}

# Symlink the extracted source to a stable path, then create a non-root
# user that owns the source tree. Building and running the patched QEMU
# under this user limits the blast radius if the (untrusted) PPA-fetched
# source ships a compromised build script. Mirrors the upstream
# tdx-measure repo's commit `5e7e883` ("Run QEMU ACPI Dump container as
# non-root user").
RUN QEMU_WRK_DIR=$(find / -maxdepth 1 -type d -name "qemu*" | head -n 1) && \
    echo "Found QEMU directory: $QEMU_WRK_DIR" && \
    ln -s "$QEMU_WRK_DIR" /qemu-source && \
    useradd -m -s /sbin/nologin qemu-user && \
    chown -R qemu-user:qemu-user "$QEMU_WRK_DIR"

WORKDIR /qemu-source
USER qemu-user

# ---------------------------------------------------------------------------
# Patch QEMU to dump all three ACPI fw_cfg blobs and exit.
#
# 1. In acpi_build_update(), after acpi_build() completes, write:
#      /output/acpi_tables.bin  -- concatenated ACPI tables
#      /output/rsdp.bin         -- RSDP structure
#      /output/table_loader.bin -- QEMU table-loader command script
#    then call exit(0) so QEMU terminates immediately.
#
# 2. Force eoi_intercept_unsupported = 1 (true) to match TDX behavior.
#    TDX does not support EOI intercept, which changes the MADT interrupt
#    source overrides from level-triggered to edge-triggered.
#
# 3. Comment out pc-bios and tests subdirectories in meson.build to
#    speed up the build (we only need the emulator binary).
# ---------------------------------------------------------------------------

ARG acpi_build_c="hw/i386/acpi-build.c"
ARG acpi_common_c="hw/i386/acpi-common.c"
ARG meson_build="meson.build"

# The dump code block injected after acpi_build() in acpi_build_update().
# We write all three fw_cfg blobs that OVMF reads during boot:
#   - tables.table_data  -> etc/acpi/tables    (ACPI tables blob)
#   - tables.rsdp        -> etc/acpi/rsdp      (RSDP structure)
#   - tables.table_loader -> etc/table-loader   (linker/loader script)
ARG acpi_tables_dump_loc='\n\n\
    /* --- ACPI dump patch: write tables, rsdp, and loader to /output/ --- */\n\
    {\n\
        int fd;\n\
        ssize_t ret;\n\
        \n\
        fd = open("/output/acpi_tables.bin", O_CREAT | O_WRONLY | O_TRUNC, 0644);\n\
        if (fd != -1) {\n\
            ret = write(fd, tables.table_data->data, tables.table_data->len);\n\
            if (ret < (ssize_t)tables.table_data->len)\n\
                printf("Error: failed to write acpi_tables.bin\\n");\n\
            else\n\
                printf("Wrote acpi_tables.bin (%d bytes)\\n", (int)tables.table_data->len);\n\
            close(fd);\n\
        } else {\n\
            printf("Error: cannot create /output/acpi_tables.bin\\n");\n\
        }\n\
        \n\
        fd = open("/output/rsdp.bin", O_CREAT | O_WRONLY | O_TRUNC, 0644);\n\
        if (fd != -1) {\n\
            ret = write(fd, tables.rsdp->data, tables.rsdp->len);\n\
            if (ret < (ssize_t)tables.rsdp->len)\n\
                printf("Error: failed to write rsdp.bin\\n");\n\
            else\n\
                printf("Wrote rsdp.bin (%d bytes)\\n", (int)tables.rsdp->len);\n\
            close(fd);\n\
        } else {\n\
            printf("Error: cannot create /output/rsdp.bin\\n");\n\
        }\n\
        \n\
        fd = open("/output/table_loader.bin", O_CREAT | O_WRONLY | O_TRUNC, 0644);\n\
        if (fd != -1) {\n\
            ret = write(fd, tables.linker->cmd_blob->data, tables.linker->cmd_blob->len);\n\
            if (ret < (ssize_t)tables.linker->cmd_blob->len)\n\
                printf("Error: failed to write table_loader.bin\\n");\n\
            else\n\
                printf("Wrote table_loader.bin (%d bytes)\\n", (int)tables.linker->cmd_blob->len);\n\
            close(fd);\n\
        } else {\n\
            printf("Error: cannot create /output/table_loader.bin\\n");\n\
        }\n\
        \n\
        printf("ACPI extraction complete, exiting.\\n");\n\
        exit(0);\n\
    }'

RUN perl -i -0pe 's/(acpi_build_update\(void \*build_opaque\)[\s\S]*?acpi_build\([^;]*;)/\1'"$acpi_tables_dump_loc"'/gs' "$acpi_build_c" && \
    perl -i -pe 's/x86ms->eoi_intercept_unsupported;/1;/g' "$acpi_build_c" "$acpi_common_c" && \
    perl -i -pe "s/(^subdir\('pc-bios'\)|^subdir\('tests'\))/# \1/g" "$meson_build"

# Configure with minimal features -- we only need enough to run the
# machine init and generate ACPI tables. Both KVM and TCG are enabled
# so the image works on machines with or without /dev/kvm.
RUN ./configure \
    --disable-bsd-user \
    --disable-guest-agent \
    --disable-strip \
    --disable-werror \
    --disable-gcrypt \
    --disable-debug-info \
    --disable-debug-tcg \
    --disable-docs \
    --disable-tcg-interpreter \
    --enable-attr \
    --disable-brlapi \
    --disable-linux-aio \
    --disable-bzip2 \
    --disable-cap-ng \
    --disable-curl \
    --disable-glusterfs \
    --disable-gnutls \
    --disable-nettle \
    --disable-gtk \
    --disable-rdma \
    --disable-libiscsi \
    --disable-vnc-jpeg \
    --enable-kvm \
    --enable-tcg \
    --disable-lzo \
    --disable-curses \
    --disable-libnfs \
    --disable-numa \
    --disable-opengl \
    --disable-rbd \
    --disable-vnc-sasl \
    --disable-sdl \
    --disable-seccomp \
    --disable-smartcard \
    --disable-snappy \
    --disable-spice \
    --disable-libusb \
    --disable-usb-redir \
    --disable-vde \
    --disable-vhost-net \
    --disable-virglrenderer \
    --disable-virtfs \
    --disable-vnc \
    --disable-vte \
    --disable-xen \
    --disable-xen-pci-passthrough \
    --disable-tools \
    --target-list=x86_64-softmmu

RUN make -j$(nproc)

ENTRYPOINT ["build/qemu-system-x86_64"]
