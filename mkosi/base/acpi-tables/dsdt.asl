/*
 * Trusted DSDT for confos confidential VMs.
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

// Linux's acpi_table_initrd_override (drivers/acpi/tables.c) requires the
// initrd table to match the firmware DSDT on Signature, OEM_ID, AND
// OEM_Table_ID, AND have an OEM_Revision STRICTLY GREATER than the
// firmware's. QEMU emits DSDT with OEM_ID="BOCHS " and OEM_Table_ID="BXPC"
// at OEM_Revision=1, so we must match those exactly and bump revision to
// at least 2 for the override path to fire — otherwise the kernel keeps
// FADT pointing at the VMM-supplied DSDT and our table is installed as
// an unused second DSDT (the "install" path), leaving BadAML unmitigated.
//
// INVARIANT: do not reorder these positional args. The pattern is
//   DefinitionBlock(filename, signature, compliance_rev, OEM_ID,
//                   OEM_Table_ID, OEM_Revision).
// OEM_ID is exactly 6 bytes (space-padded), OEM_Table_ID is 8 bytes.
// OEM_ID is exactly 6 ASCII bytes, OEM_Table_ID is exactly 8 ASCII bytes.
// QEMU/SeaBIOS space-pads short strings; iasl null-pads. memcmp on the
// raw 6/8 bytes is what the kernel match uses, so we must space-pad too
// or the override match fails on a single trailing byte difference.
DefinitionBlock ("dsdt.aml", "DSDT", 1, "BOCHS ", "BXPC    ", 0x00000002)
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

                /* 64-bit MMIO window (low): 32GiB..1TiB. Used by firmware for
                 * device BARs on small-memory guests. NOTE: Linux drops a
                 * host-bridge window ENTIRELY if any part of it overlaps
                 * System RAM, so guests with >=32GiB of RAM lose this window
                 * — the high window below exists for exactly that case. */
                QWordMemory (ResourceProducer, PosDecode, MinFixed, MaxFixed,
                    Cacheable, ReadWrite,
                    0x0000000000000000, 0x0000000800000000, 0x000000FFFFFFFFFF,
                    0x0000000000000000, 0x000000F800000000)

                /* 64-bit MMIO window (high): 2TiB..64TiB. Covers wherever OVMF
                 * places very large device BARs. B200 resizable BAR2 is
                 * 256GiB; one GPU needs a ~384GiB bridge window, so 8 GPUs need
                 * ~3TiB. OVMF's placement base depends on its 64-bit MMIO
                 * aperture: with the default aperture it lands near 56TiB;
                 * raising it (fw_cfg opt/ovmf/X-PciMmio64Mb, needed to fit 4+
                 * of these BARs plus the boot-disk BAR) relocates it toward
                 * ~2TiB. This single wide window covers both. RAM can never
                 * reach 2TiB on supported hosts, so unlike the low window it is
                 * immune to Linux's RAM-conflict drop; 64TiB is the 46-bit
                 * physical-address ceiling of this host's CPUs. Without it,
                 * multi-GPU guests fail driver probe with "BAR0 is 0M @ 0x0"
                 * (kernel can't claim the firmware-placed BARs: "can't claim;
                 * no compatible bridge window"). NOTE: 4+ GPUs ALSO require the
                 * OVMF aperture raise above — the window here is necessary but
                 * not sufficient on its own. */
                QWordMemory (ResourceProducer, PosDecode, MinFixed, MaxFixed,
                    Cacheable, ReadWrite,
                    0x0000000000000000, 0x0000020000000000, 0x00003FFFFFFFFFFF,
                    0x0000000000000000, 0x00003E0000000000)
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
