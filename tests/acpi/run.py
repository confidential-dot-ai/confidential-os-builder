#!/usr/bin/env python3
"""Boot kernels built from the builder's patched source under QEMU TCG.

Run through tests/acpi/run.sh: `confos kernel-source --acpi-harness`
prepares the tree and runs this script inside its tools tree. Never needs
/dev/kvm.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import struct
import re
import subprocess
import sys
import threading

HERE = Path(__file__).resolve().parent
# Native inside the x86-64 tools tree; a cross prefix only elsewhere.
CROSS = "" if platform.machine() in ("x86_64", "amd64") else "x86_64-linux-gnu-"


def command(args, *, cwd=None, log=None):
    args = [str(x) for x in args]
    print("+", " ".join(args), flush=True)
    if log:
        with log.open("w") as output:
            subprocess.run(args, cwd=cwd, check=True, stdout=output, stderr=subprocess.STDOUT)
    else:
        subprocess.run(args, cwd=cwd, check=True)


def aml(source, output):
    # The kernel's standard custom-DSDT contract uses this exact prefix.
    directory = output.parent / (output.name + "-iasl")
    directory.mkdir(exist_ok=True)
    command(["iasl", "-ve", "-tc", "-p", directory / "dsdt", source])
    for suffix in (".aml", ".hex"):
        shutil.copyfile(directory / ("dsdt" + suffix), output.with_suffix(suffix))
    if "unsigned char dsdt_aml_code[]" not in output.with_suffix(".hex").read_text():
        raise RuntimeError("iasl did not generate the required dsdt_aml_code symbol")
    return output.with_suffix(".aml")


def checksum(data, at=9):
    """Rewrite the ACPI checksum byte so the table sums to zero."""
    data[at] = 0
    data[at] = -sum(data) & 255
    return data


def variant(source, output, signature=None, revision=None, oem=None):
    data = bytearray(source.read_bytes())
    if signature:
        data[:4] = signature.encode("ascii")
    if revision is not None:
        data[24:28] = revision.to_bytes(4, "little")
    if oem:
        data[10:16] = oem.encode("ascii")
    output.write_bytes(checksum(data))
    return output


def c_array(name, data, static=False):
    return ("static " if static else "") + "unsigned char " + name + "[] = {" + ",".join(str(x) for x in data) + "};\n"


def corrupt(source, defect, *, signature, length):
    """Break one header field; a "checksum" defect leaves the sum wrong."""
    data = bytearray(source.read_bytes())
    if defect == "signature":
        data[:4] = signature
    elif defect == "length":
        struct.pack_into("<I", data, 4, length)
    if defect == "checksum":
        data[9] ^= 1
        return data
    return checksum(data)


def firmware_root(templates, primary, secondary, output, secondary_first=False):
    """Build a checksummed root in reserved guest RAM; FADT points at our DSDT.

    QEMU -acpitable only appends a DSDT, leaving FADT's original DSDT intact.
    Copy the control boot's non-AML tables and replace that actual pointer.
    `secondary_first` lists the secondary tables ahead of the FADT, which is
    the order ACPICA verifies them in.
    """
    base = 0x08000000
    memory = bytearray(65536)
    cursor = 256

    def store(data):
        nonlocal cursor
        address = base + cursor
        if cursor + len(data) > len(memory):
            raise RuntimeError("ACPI test region overflow")
        memory[cursor:cursor + len(data)] = data
        cursor = (cursor + len(data) + 15) & ~15
        return address

    dsdt_address = store(primary.read_bytes())
    fadt = bytearray(templates["FACP"])
    struct.pack_into("<I", fadt, 40, dsdt_address)
    if len(fadt) >= 148:
        struct.pack_into("<Q", fadt, 140, dsdt_address)
    pointers = [store(table.read_bytes()) for table in secondary] if secondary_first else []
    pointers.append(store(checksum(fadt)))
    for name in ("APIC", "HPET", "MCFG"):
        if name in templates:
            pointers.append(store(templates[name]))
    if not secondary_first:
        pointers.extend(store(table.read_bytes()) for table in secondary)
    rsdt = bytearray(struct.pack("<4sIBB6s8sI4sI", b"RSDT", 36 + 4 * len(pointers), 1, 0, b"CONFAI", b"AMLTEST ", 1, b"TEST", 1))
    rsdt.extend(struct.pack("<" + "I" * len(pointers), *pointers))
    rsdt_address = store(checksum(rsdt))
    rsdp = bytearray(struct.pack("<8sB6sBI", b"RSD PTR ", 0, b"CONFAI", 0, rsdt_address))
    checksum(rsdp, at=8)
    memory[:len(rsdp)] = rsdp
    output.write_bytes(memory)
    return ["-device", f"loader,file={output},addr={base}"], f" acpi_rsdp={base:x} memmap=64K$0x{base:x}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True, help="Disposable patched Linux source tree")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=min(os.cpu_count() or 2, 8))
    parser.add_argument("--timeout", type=int, default=45)
    args = parser.parse_args()
    source, output = args.source.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if "ACPI_TRUSTED_AML" not in (source / "drivers/acpi/Kconfig").read_text():
        parser.error("source must already contain the repository trusted AML patch")
    builder_header = source / "include/confos-trusted-dsdt.h"
    if not builder_header.exists():
        parser.error("source lacks the builder-generated trusted DSDT header; prepare it with tests/acpi/run.sh")
    for binary in ("iasl", CROSS + "gcc", "make", "qemu-system-x86_64", "cpio"):
        if not shutil.which(binary):
            parser.error(f"missing {binary}; run inside the kernel tools tree via tests/acpi/run.sh")
    # Test kernels overwrite this header in place; keep the builder's copy.
    production = output / "production.hex"
    shutil.copyfile(builder_header, production)
    trusted = aml(HERE / "fixtures/trusted.asl", output / "trusted")
    host = aml(HERE / "fixtures/host.asl", output / "host")
    secondary = aml(HERE / "fixtures/secondary.asl", output / "secondary")
    method = aml(HERE / "fixtures/method.asl", output / "method")
    tables = {sig: variant(secondary, output / f"{sig}.aml", signature=sig) for sig in ("SSDT", "PSDT", "OSDT")}
    initdir = output / "initramfs"
    initdir.mkdir(exist_ok=True)
    command([CROSS + "gcc", "-static", "-Os", "-Wall", "-Wextra", "-Werror", HERE / "init.c", "-o", initdir / "init"])
    with (output / "initramfs.cpio").open("wb") as archive:
        subprocess.run(["cpio", "-o", "-H", "newc", "--quiet"], cwd=initdir, input=b"init\n", stdout=archive, check=True)
    build = output / "build"
    build.mkdir(exist_ok=True)
    make = ["make", "-C", source, f"O={build}", "ARCH=x86_64", f"CROSS_COMPILE={CROSS}"]
    command([*make, "allnoconfig"], log=output / "configure.log")
    config = source / "scripts/config"
    enabled = ["64BIT", "SMP", "KEXEC", "KALLSYMS", "PRINTK", "MULTIUSER", "SYSFS", "PROC_FS", "TMPFS", "PCI", "ACPI", "PM", "TTY", "SERIAL_8250", "SERIAL_8250_CONSOLE", "BLK_DEV_INITRD", "BINFMT_ELF", "DEVTMPFS", "X86_LOCAL_APIC", "X86_IO_APIC"]
    # The alternate table loaders the gate's Kconfig refuses to coexist with.
    disabled = ["ACPI_TABLE_UPGRADE", "ACPI_CONFIGFS", "EFI_CUSTOM_SSDT_OVERLAYS", "ACPI_DEBUGGER"]
    command([config, "--file", build / ".config", "--set-val", "NR_CPUS", "8",
             *[item for name in enabled for item in ("-e", name)], *[item for name in disabled for item in ("-d", name)]])
    command([*make, "olddefconfig"], log=output / "olddefconfig.log")

    # Boots of a built kernel are independent, so they run concurrently; a
    # kernel build waits for them first to keep the CPUs and the results.
    pool = ThreadPoolExecutor(max_workers=args.jobs)
    pending = []
    results = []
    results_lock = threading.Lock()

    def drain():
        while pending:
            pending.pop().result()

    def kernel(name, header=None, hardened=False):
        drain()
        switches = ["-d", "STANDALONE", "-e" if header else "-d", "ACPI_CUSTOM_DSDT", "-e" if hardened else "-d", "ACPI_TRUSTED_AML"]
        if header:
            shutil.copyfile(header, source / "include/confos-trusted-dsdt.h")
            switches += ["--set-str", "ACPI_CUSTOM_DSDT_FILE", "confos-trusted-dsdt.h"]
        else:
            switches += ["--set-str", "ACPI_CUSTOM_DSDT_FILE", ""]
        command([config, "--file", build / ".config", *switches])
        command([*make, "olddefconfig"], log=output / f"{name}-configure.log")
        resolved = (build / ".config").read_text()
        if hardened and "CONFIG_ACPI_TRUSTED_AML=y" not in resolved:
            raise RuntimeError("trusted AML gate did not resolve on")
        command([*make, f"-j{args.jobs}", "bzImage"], log=output / f"{name}-build.log")
        image = output / f"{name}-bzImage"
        shutil.copyfile(build / "arch/x86/boot/bzImage", image)
        shutil.copyfile(build / ".config", output / f"{name}.config")
        return image

    def run_boot(order, name, image, acpi_tables, required, forbidden, cmdline, fatal, primary, cpus, memory, secondary_first):
        extra = []
        if primary is not None:
            extra, root_cmdline = firmware_root(templates, primary, acpi_tables, output / f"{name}-firmware.bin", secondary_first)
            cmdline += root_cmdline
            acpi_tables = ()
        qemu = ["qemu-system-x86_64", "-machine", "q35,accel=tcg", "-cpu", "max", "-m", f"{memory}M", "-smp", str(cpus), "-nodefaults", "-display", "none", "-serial", "stdio", "-monitor", "none", "-no-reboot", "-kernel", str(image), "-initrd", str(output / "initramfs.cpio"), "-append", "console=ttyS0 rdinit=/init panic=-1 " + cmdline, *extra]
        for table in acpi_tables:
            qemu += ["-acpitable", "file=" + str(table)]
        print(f"BOOT {name}", flush=True)
        timed_out = False
        return_code = None
        try:
            proc = subprocess.run(qemu, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=args.timeout)
            log = proc.stdout.decode(errors="replace")
            return_code = proc.returncode
        except subprocess.TimeoutExpired as exc:
            timed_out = True
            log = (exc.stdout or b"").decode(errors="replace")
        (output / f"{name}.serial.log").write_text(log)
        need = tuple(required) + (() if fatal else ("AMLTEST: USERSPACE_REACHED", f"AMLTEST: CPUS {cpus}"))
        if primary is not None:
            need += ("ACPI: RSDP 0x0000000008000000",)
        deny = tuple(forbidden) + (("AMLTEST: USERSPACE_REACHED",) if fatal else ())
        errors = [f"missing {s}" for s in need if s not in log] + [f"unexpected {s}" for s in deny if s in log]
        if not fatal:
            if timed_out or return_code != 0:
                errors.append(f"guest did not exit cleanly (timeout={timed_out}, code={return_code})")
            found_memory = re.search(r"AMLTEST: MEMORY_MB (\d+)", log)
            if not found_memory or int(found_memory[1]) < memory * 0.7:
                errors.append("guest did not see expected RAM")
        with results_lock:
            results.append({"order": order, "case": name, "passed": not errors, "errors": errors, "command": qemu})
            results.sort(key=lambda item: item["order"])
            print(f"{'FAIL' if errors else 'PASS'} {name}: {errors}", flush=True)
            (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        return log

    def boot(name, image, acpi_tables=(), required=(), forbidden=(), cmdline="", fatal=False, primary=None, cpus=1, memory=256, secondary_first=False):
        """Schedule a boot; the returned future yields the serial log."""
        future = pool.submit(run_boot, len(pending) + len(results), name, image, tuple(acpi_tables), tuple(required), tuple(forbidden), cmdline, fatal, primary, cpus, memory, secondary_first)
        pending.append(future)
        return future

    control = kernel("control")
    # The stock boot's exported non-AML tables seed every firmware_root below.
    templates = {sig: bytes.fromhex(data) for sig, data in re.findall(
        r"AMLTEST: TABLE (\w+) ([0-9a-f]+)", boot("control-stock", control, required=["AMLTEST: DEVICE PNP0A08"]).result())}
    if "FACP" not in templates:
        raise RuntimeError("control boot did not export its FADT")
    boot("control-host", control, primary=host, required=["AMLTEST: DEVICE CFA0002"])
    for sig, table in tables.items():
        boot("control-" + sig, control, [table], primary=host, required=["AMLTEST: DEVICE CFA0003"])
    hardened = kernel("hardened", trusted.with_suffix(".hex"), True)
    markers = ["AMLTEST: DEVICE CFA0002", "AMLTEST: DEVICE CFA0003"]
    # A healthy enforcing boot logs no ACPI error: host definition blocks are
    # dropped while the root table is parsed, not failed at namespace load.
    denied = markers + ["ACPI Error"]
    accepted = ["ACPI: Trusted AML: built-in DSDT loaded", "AMLTEST: DEVICE CFA0001"]
    boot("hardened-default-firmware", hardened, required=accepted, forbidden=denied)
    for sig, table in tables.items():
        boot("hardened-" + sig, hardened, [table], primary=host, required=accepted + [f"Trusted AML: ignoring host {sig}"], forbidden=denied)
    duplicate = variant(secondary, output / "duplicate-DSDT.aml", signature="DSDT")
    # Either order relative to the FADT: ACPICA verifies root entries in
    # order, and a byte-identical host DSDT listed first would otherwise
    # displace the FADT's slot as its duplicate.
    for sig, table in (("DSDT", duplicate), ("SSDT", tables["SSDT"])):
        for suffix, first in (("", False), ("-first", True)):
            boot(f"hardened-duplicate-{sig}{suffix}", hardened, [table], primary=host, secondary_first=first,
                 required=accepted + [f"Trusted AML: ignoring host {sig}"], forbidden=denied)
    for revision in (0, 1, 0xFFFFFFFF):
        table = variant(host, output / f"revision-{revision}.aml", revision=revision, oem="CONFAI" if revision == 1 else "OTHERX")
        boot(f"hardened-revision-{revision}", hardened, primary=table, required=accepted, forbidden=denied)
    for defect in ("signature", "length", "checksum"):
        bad_host = output / f"host-invalid-{defect}.aml"
        bad_host.write_bytes(corrupt(host, defect, signature=b"XXXX", length=20))
        fatal_host = defect == "signature"
        boot("hardened-host-invalid-" + defect, hardened, primary=bad_host,
             required=["Kernel panic", "Trusted AML:"] if fatal_host else accepted,
             forbidden=markers if fatal_host else denied, fatal=fatal_host)
    boot("hardened-copy-dsdt", hardened, primary=host, cmdline="acpi=copy_dsdt", required=accepted, forbidden=denied)
    boot("hardened-acpi-disabled", hardened, cmdline="acpi=off", required=["Kernel panic", "Trusted AML:"], fatal=True)
    boot("hardened-acpi-init-skipped", hardened, cmdline="initcall_blacklist=acpi_init", required=["Kernel panic", "Trusted AML: required built-in DSDT unavailable"], fatal=True)
    dynamic_asl = output / "dynamic.asl"
    buffer = ", ".join(f"0x{byte:02X}" for byte in secondary.read_bytes())
    dynamic_asl.write_text((HERE / "fixtures/trusted.asl").read_text().replace(
        "    Scope", f"    Name (DYNA, Buffer () {{ {buffer} }})\n"
        "    Method (LDBF, 0, NotSerialized) { Load (DYNA, Local0) Return (Local0) }\n"
        '    Method (LDTB, 0, NotSerialized) { Return (LoadTable ("SSDT", "", "", "", "", 0)) }\n    Scope', 1))
    dynamic = aml(dynamic_asl, output / "dynamic")
    # These two kernels contain explicit diagnostic instrumentation. Every other
    # kernel is built from the unmodified repository patch and header alone.
    acpi_makefile = source / "drivers/acpi/Makefile"
    original_makefile = acpi_makefile.read_bytes()
    probe = source / "drivers/acpi/confos-aml-probe.c"
    probe_data = source / "include/confos-aml-test-data.h"
    if probe.exists() or probe_data.exists():
        raise RuntimeError("diagnostic paths already exist in disposable source")
    try:
        shutil.copyfile(HERE / "api-probe.c", probe)
        probe_data.write_text("".join(
            c_array(name, table.read_bytes(), static=True)
            for name, table in (("test_aml", secondary), ("test_method_aml", method))))
        acpi_makefile.write_bytes(original_makefile + b"\nobj-y += confos-aml-probe.o\n")
        method_control = kernel("diagnostic-method-control", dynamic.with_suffix(".hex"))
        boot("control-method-install", method_control, required=[
            "AMLTEST: acpi_install_method=AE_OK", "AMLTEST: installed-method=AE_OK",
            "AMLTEST: installed-method-value=0xcfa132"])
        dynamic_image = kernel("diagnostic-dynamic", dynamic.with_suffix(".hex"), True)
        # The probe's rejected Load/LoadTable methods abort with an ACPI error
        # by design, so only the markers are forbidden here.
        boot("hardened-dynamic-load", dynamic_image, [tables["SSDT"]], required=accepted + [
            "Trusted AML: ignoring host SSDT",
            "AMLTEST: acpi_load_table=AE_ACCESS", "AMLTEST: acpi_install_method=AE_ACCESS",
            "AMLTEST: installed-method=AE_NOT_FOUND",
            "AMLTEST: Load(buffer)=AE_ACCESS", "AMLTEST: LoadTable=AE_ACCESS",
            "AMLTEST: repeated-primary=AE_ACCESS", "AMLTEST: unload-primary=AE_ACCESS",
            "AMLTEST: reload-primary=AE_ACCESS"], forbidden=markers + ["AMLTEST: installed-method-value="])
    finally:
        drain()
        acpi_makefile.write_bytes(original_makefile)
        probe.unlink(missing_ok=True)
        probe_data.unlink(missing_ok=True)
    for defect in ("signature", "length", "checksum"):
        invalid_header = output / f"invalid-{defect}.hex"
        invalid = corrupt(trusted, defect, signature=b"SSDT", length=len(trusted.read_bytes()) + 1)
        invalid_header.write_text(c_array("dsdt_aml_code", invalid))
        invalid_image = kernel("invalid-" + defect, invalid_header, True)
        boot("hardened-invalid-" + defect, invalid_image, required=["Kernel panic", "Trusted AML:"], fatal=True)
    image = kernel("production-dsdt", production, True)
    production_required = accepted[:1] + ["AMLTEST: DEVICE PNP0A08", "AMLTEST: PCI 0000:00:00.0"]
    boot("production-q35", image, required=production_required, forbidden=denied)
    boot("production-q35-smp", image, required=production_required, forbidden=denied, cpus=4, memory=4096)
    drain()
    pool.shutdown()
    hashes = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in output.glob("*-bzImage")}
    (output / "kernel-sha256.json").write_text(json.dumps(hashes, indent=2) + "\n")
    if any(not item["passed"] for item in results):
        return 1
    print(f"All {len(results)} QEMU cases passed; this does not validate TDX/SNP hardware.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
