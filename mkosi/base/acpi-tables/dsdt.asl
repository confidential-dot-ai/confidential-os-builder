/*
 * Trusted DSDT for steep confidential VMs.
 *
 * Shipped in the initrd at /kernel/firmware/acpi/dsdt.aml. The Linux
 * CONFIG_ACPI_TABLE_UPGRADE feature scans the initrd for that path early
 * in boot and replaces the firmware-supplied DSDT with this one. The
 * VMM's DSDT — the highest-leverage attack surface in the ACPI tables,
 * because the kernel executes AML at kernel privilege ("BadAML") — is
 * never trusted.
 *
 * This file is the ONLY AML the kernel executes. Keep it minimal:
 *
 *   - PCI root bridge: required so the kernel enumerates PCI. The window
 *     resources are the standard QEMU q35 layout; the kernel reads the
 *     ECAM base from MCFG separately, so _CRS here is only the resource
 *     map.
 *   - _S5 method: clean shutdown via ACPI PM_CNT writes. QEMU uses
 *     sleep type 0 for S5.
 *
 * Deliberately absent:
 *
 *   - Processor objects: CPU enumeration is via MADT (which we don't
 *     replace). Omitting Processor() here makes the DSDT SMP-invariant
 *     across all vCPU counts at runtime.
 *   - Memory hotplug stubs: memory enumeration is via E820 / TD HOB.
 *     Omitting them makes the DSDT memory-invariant.
 *   - Embedded controller, lid, batteries, thermal zones, GPEs: the
 *     guest is a server, not a laptop.
 *
 * The result is a single AML blob whose hash is constant across all
 * (vCPU, memory) topologies — so the manifest needs one TDX entry, not
 * one per cross product.
 */

DefinitionBlock ("dsdt.aml", "DSDT", 1, "LUNAL", "STEEP", 0x00000001)
{
    Scope (\_SB)
    {
        /*
         * PCI root bridge.
         *
         * _HID PNP0A08 = PCI Express Root Bridge (q35).
         * _CID PNP0A03 = legacy PCI compatible (fallback).
         *
         * _CRS describes the bus number range and host-bridge resource
         * windows. The numeric ranges match the QEMU q35 default ECAM /
         * MMIO layout. The kernel uses MCFG (not _CRS) to find the ECAM
         * base — _CRS exists so the PCI core's resource accounting has
         * a valid window map.
         */
        Device (PCI0)
        {
            Name (_HID, EisaId ("PNP0A08"))
            Name (_CID, EisaId ("PNP0A03"))
            Name (_UID, 0x00)
            Name (_BBN, 0x00)
            Name (_CRS, ResourceTemplate ()
            {
                /* Bus numbers 0x00..0xFF. */
                WordBusNumber (ResourceProducer, MinFixed, MaxFixed, PosDecode,
                    0x0000, 0x0000, 0x00FF, 0x0000, 0x0100)

                /* PCI config mechanism #1 ports. */
                IO (Decode16, 0x0CF8, 0x0CF8, 0x01, 0x08)

                /* Low I/O window. */
                WordIO (ResourceProducer, MinFixed, MaxFixed, PosDecode, EntireRange,
                    0x0000, 0x0000, 0x0CF7, 0x0000, 0x0CF8)

                /* High I/O window. */
                WordIO (ResourceProducer, MinFixed, MaxFixed, PosDecode, EntireRange,
                    0x0000, 0x0D00, 0xFFFF, 0x0000, 0xF300)

                /* 32-bit MMIO window (PCI hole below 4GiB). */
                DWordMemory (ResourceProducer, PosDecode, MinFixed, MaxFixed,
                    Cacheable, ReadWrite,
                    0x00000000, 0xC0000000, 0xFEBFFFFF, 0x00000000, 0x3EC00000)

                /* 64-bit MMIO window. */
                QWordMemory (ResourceProducer, PosDecode, MinFixed, MaxFixed,
                    Cacheable, ReadWrite,
                    0x0000000000000000, 0x0000000800000000, 0x000000FFFFFFFFFF,
                    0x0000000000000000, 0x000000F800000000)
            })
        }
    }

    /*
     * S5 (soft-off / shutdown) sleep package.
     *
     * Field order: PM1a_CNT.SLP_TYP, PM1b_CNT.SLP_TYP, reserved, reserved.
     * QEMU's q35 ACPI implementation maps S5 to sleep type 0.
     */
    Name (\_S5, Package (0x04)
    {
        0x00,
        0x00,
        0x00,
        0x00
    })
}
