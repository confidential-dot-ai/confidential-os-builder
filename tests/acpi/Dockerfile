FROM ubuntu:resolute
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    gcc-x86-64-linux-gnu libc6-dev-amd64-cross make flex bison bc libelf-dev libssl-dev \
    acpica-tools qemu-system-x86 python3 cpio xz-utils ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends gcc gcc-15-plugin-dev-x86-64-linux-gnu \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /work
ENTRYPOINT ["python3", "/repo/tests/acpi/run.py"]
