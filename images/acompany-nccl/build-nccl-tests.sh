#!/usr/bin/env bash
set -euo pipefail

if [[ -n "${BUILDROOT:-}" && "${ACOMPANY_IN_CHROOT:-0}" != 1 ]]; then
    install -m0755 "$0" "$BUILDROOT/tmp/build-acompany-nccl-tests"
    chroot "$BUILDROOT" /usr/bin/env ACOMPANY_IN_CHROOT=1 \
        /tmp/build-acompany-nccl-tests
    rm -f "$BUILDROOT/tmp/build-acompany-nccl-tests"
    exit 0
fi

readonly ROOT=/opt/acompany/nccl-tests
readonly SRC="$ROOT/src"
readonly MPI_HOME=/usr/lib/x86_64-linux-gnu/openmpi

export PATH=/usr/local/cuda/bin:$PATH
export LD_LIBRARY_PATH=/usr/local/cuda/lib:/usr/lib/x86_64-linux-gnu

# Fail the measured-image build if the rootful OCI runtime or its checked-in
# configuration is missing/invalid. Runtime GPU execution is exercised on a
# confidential GPU VM after publication; this catches packaging drift early.
for binary in containerd docker dockerd runc; do
    command -v "$binary" >/dev/null
done
dockerd --validate --config-file=/etc/docker/daemon.json
containerd --config /etc/containerd/config.toml config dump >/dev/null

make -C "$SRC" clean
make -C "$SRC" -j"$(nproc)" MPI=0 CUDA_HOME=/usr/local/cuda
mv "$SRC/build" "$ROOT/build-nompi"
test -x "$ROOT/build-nompi/all_reduce_perf"

make -C "$SRC" clean
make -C "$SRC" -j"$(nproc)" MPI=1 MPI_HOME="$MPI_HOME" \
    CUDA_HOME=/usr/local/cuda
mv "$SRC/build" "$ROOT/build-mpi"
test -x "$ROOT/build-mpi/all_reduce_perf"

ln -sfn build-nompi "$ROOT/build"

# nvcc embeds PID-derived tmpxft paths in debug/symbol tables, and ld derives
# its build ID from those bytes. Normalize release binaries so the measured
# image is independent of build process IDs.
find "$ROOT/build-nompi" "$ROOT/build-mpi" -type f -perm /111 -print0 |
while IFS= read -r -d '' binary; do
    strip --strip-all "$binary"
    objcopy --remove-section .note.gnu.build-id "$binary"
    chmod 0755 "$binary"
done
find "$ROOT/build-nompi" "$ROOT/build-mpi" -type f \
    \( -name '*.o' -o -name '*.a' \) -delete

{
    /usr/local/cuda/bin/nvcc --version
    find /usr/lib -type f -name 'libnccl.so*' -print
    mpirun --version | sed -n '1p'
} > /opt/acompany/BUILD-VERSIONS
