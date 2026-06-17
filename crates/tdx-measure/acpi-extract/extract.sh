#!/bin/bash
# ACPI table extraction for TDX measurement computation.
#
# Builds a patched QEMU in Docker, runs it to generate the ACPI tables
# matching a given VM configuration, and copies out the three fw_cfg
# blobs needed for RTMR[0] measurement:
#   - acpi_tables.bin  : concatenated ACPI tables
#   - rsdp.bin         : RSDP structure
#   - table_loader.bin : QEMU table-loader command script
#
# Works on any machine with Docker. Uses TCG (software emulation) by
# default, so no KVM or TDX hardware is required. With --kvm, uses KVM
# acceleration for faster QEMU startup (tables are identical either way
# since ACPI generation happens during machine init, not in the
# accelerator).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
readonly IMAGE_NAME="tdx-acpi-extract"
readonly CONTAINER_NAME="tdx-acpi-extract-$$"

# Defaults
CPUS=2
MEMORY="2G"
FIRMWARE=""
OUTPUT_DIR="."
USE_KVM=false
FORCE_BUILD=false
MACHINE_OPTS="q35,kernel-irqchip=split,hpet=off,smm=off,pic=off"
EXTRA_QEMU_ARGS=""

# Color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info()    { echo -e "${BLUE}[INFO]${NC} $*" >&2; }
log_success() { echo -e "${GREEN}[OK]${NC} $*" >&2; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $*" >&2; }
log_error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; }

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Extract ACPI tables from a patched QEMU for TDX measurement computation.

REQUIRED:
    --firmware PATH    Path to OVMF/TDVF firmware binary

OPTIONS:
    --cpus N           Number of vCPUs (default: $CPUS)
    --memory SIZE      Memory size, e.g. 2G, 4096M (default: $MEMORY)
    --output DIR       Output directory (default: current directory)
    --machine OPTS     QEMU machine options (default: $MACHINE_OPTS)
    --kvm              Use KVM acceleration (faster, requires /dev/kvm)
    --force-build      Rebuild Docker image even if it exists
    --extra-args ARGS  Additional QEMU arguments (quoted string)
    -h, --help         Show this help message

OUTPUT FILES:
    acpi_tables.bin    Concatenated ACPI tables (etc/acpi/tables)
    rsdp.bin           RSDP structure (etc/acpi/rsdp)
    table_loader.bin   Table loader commands (etc/table-loader)

NOTE:
    ACPI tables are identical between KVM and TCG for the same machine
    configuration. TCG is the default because it works on any machine.
    Use --kvm on hosts with /dev/kvm for faster (~1s vs ~3s) execution.

EXAMPLES:
    # Basic extraction (TCG, works anywhere)
    $(basename "$0") --firmware /path/to/OVMF.fd --cpus 2 --memory 2G

    # With KVM acceleration
    $(basename "$0") --firmware /path/to/OVMF.fd --kvm

    # Custom output directory and machine config
    $(basename "$0") --firmware /path/to/OVMF.fd --output ./tables \\
        --cpus 4 --memory 8G
EOF
}

# Parse command-line arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --cpus)       CPUS="$2"; shift 2 ;;
        --memory)     MEMORY="$2"; shift 2 ;;
        --firmware)   FIRMWARE="$2"; shift 2 ;;
        --output)     OUTPUT_DIR="$2"; shift 2 ;;
        --machine)    MACHINE_OPTS="$2"; shift 2 ;;
        --kvm)        USE_KVM=true; shift ;;
        --force-build) FORCE_BUILD=true; shift ;;
        --extra-args) EXTRA_QEMU_ARGS="$2"; shift 2 ;;
        -h|--help)    usage; exit 0 ;;
        *)            log_error "Unknown option: $1"; usage; exit 1 ;;
    esac
done

# Validate arguments
if [[ -z "$FIRMWARE" ]]; then
    log_error "Missing required argument: --firmware"
    usage
    exit 1
fi

if [[ ! -f "$FIRMWARE" ]]; then
    log_error "Firmware file not found: $FIRMWARE"
    exit 1
fi

FIRMWARE="$(realpath "$FIRMWARE")"
mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR="$(realpath "$OUTPUT_DIR")"

# Check Docker is available
if ! command -v docker &>/dev/null; then
    log_error "Docker is not installed or not in PATH"
    exit 1
fi

# Build Docker image if needed
build_image() {
    if [[ "$FORCE_BUILD" == "true" ]] || ! docker image inspect "$IMAGE_NAME" &>/dev/null; then
        log_info "Building Docker image '$IMAGE_NAME' (this takes a few minutes on first run)..."
        if ! docker build \
            --progress plain \
            --tag "$IMAGE_NAME" \
            --file "$SCRIPT_DIR/Dockerfile" \
            "$SCRIPT_DIR"; then
            log_error "Docker build failed"
            exit 1
        fi
        log_success "Docker image built successfully"
    else
        log_info "Using existing Docker image '$IMAGE_NAME' (use --force-build to rebuild)"
    fi
}

# Run QEMU inside Docker to extract tables
extract_tables() {
    log_info "Extracting ACPI tables..."
    log_info "  Firmware: $FIRMWARE"
    log_info "  CPUs: $CPUS, Memory: $MEMORY"
    log_info "  Machine: $MACHINE_OPTS"

    # Build QEMU arguments
    local qemu_args=(
        "-m" "$MEMORY"
        "-smp" "$CPUS"
        "-machine" "$MACHINE_OPTS"
        "-bios" "/firmware.fd"
        "-nographic"
        "-nodefaults"
        "-serial" "stdio"
    )

    # Docker run arguments
    local docker_args=(
        "--rm"
        "--name" "$CONTAINER_NAME"
        "-v" "$FIRMWARE:/firmware.fd:ro"
        "-v" "$OUTPUT_DIR:/output"
    )

    if [[ "$USE_KVM" == "true" ]]; then
        # Check for /dev/kvm
        if [[ ! -c /dev/kvm ]]; then
            log_warn "/dev/kvm not found, falling back to TCG"
            qemu_args+=("-accel" "tcg")
            qemu_args+=("-cpu" "qemu64")
        else
            docker_args+=("--device" "/dev/kvm:/dev/kvm")
            # Grant access to the kvm group
            local kvm_gid
            kvm_gid=$(getent group kvm 2>/dev/null | cut -d: -f3 || true)
            if [[ -n "$kvm_gid" ]]; then
                docker_args+=("--group-add" "$kvm_gid")
            fi
            qemu_args+=("-accel" "kvm")
            qemu_args+=("-cpu" "host")
            log_info "  Accelerator: KVM"
        fi
    else
        qemu_args+=("-accel" "tcg")
        qemu_args+=("-cpu" "qemu64")
        log_info "  Accelerator: TCG (software emulation)"
    fi

    # Add any extra QEMU arguments
    if [[ -n "$EXTRA_QEMU_ARGS" ]]; then
        # shellcheck disable=SC2206
        qemu_args+=($EXTRA_QEMU_ARGS)
    fi

    log_warn "You may see ROM file errors (kvmvapic.bin, linuxboot_dma.bin) -- these are safe to ignore."

    # Run Docker -- the patched QEMU will write files to /output/ and exit(0)
    if ! docker run "${docker_args[@]}" "$IMAGE_NAME" "${qemu_args[@]}"; then
        # The patched QEMU calls exit(0), but QEMU may also return non-zero
        # before reaching acpi_build_update if there are option errors.
        # Check if the files were actually created.
        if [[ ! -f "$OUTPUT_DIR/acpi_tables.bin" ]]; then
            log_error "QEMU execution failed and no ACPI tables were generated"
            exit 1
        fi
        log_warn "QEMU exited with non-zero status but tables were generated"
    fi

    # Verify output files
    local missing=false
    for f in acpi_tables.bin rsdp.bin table_loader.bin; do
        if [[ -f "$OUTPUT_DIR/$f" ]]; then
            local size
            size=$(stat -c%s "$OUTPUT_DIR/$f" 2>/dev/null || stat -f%z "$OUTPUT_DIR/$f" 2>/dev/null)
            log_success "$f ($size bytes)"
        else
            log_error "Missing output file: $f"
            missing=true
        fi
    done

    if [[ "$missing" == "true" ]]; then
        log_error "Some output files are missing. Check QEMU output above."
        exit 1
    fi

    echo ""
    log_success "ACPI tables extracted to: $OUTPUT_DIR/"
    log_info "Files:"
    log_info "  $OUTPUT_DIR/acpi_tables.bin  -- ACPI tables (etc/acpi/tables)"
    log_info "  $OUTPUT_DIR/rsdp.bin         -- RSDP (etc/acpi/rsdp)"
    log_info "  $OUTPUT_DIR/table_loader.bin -- Table loader (etc/table-loader)"
    echo ""
    log_info "Use with tdx-measure:"
    log_info "  tdx-measure measure --firmware /path/to/OVMF.fd --uki /path/to/uki.efi \\"
    log_info "    --acpi-tables $OUTPUT_DIR/acpi_tables.bin \\"
    log_info "    --acpi-rsdp $OUTPUT_DIR/rsdp.bin \\"
    log_info "    --acpi-loader $OUTPUT_DIR/table_loader.bin"
}

# Cleanup on exit
cleanup() {
    docker rm -f "$CONTAINER_NAME" 2>/dev/null || true
}
trap cleanup EXIT

# Main
build_image
extract_tables
