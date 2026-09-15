/*
 * The only AML admitted by the confos kernel namespace-load gate.
 * Compiled with the pinned kernel tools tree and included in vmlinuz.
 * Host DSDT identifiers and revision do not select this trusted table.
 * Keep topology windows compatible with supported CPU/memory/GPU shapes.
 */

DefinitionBlock ("dsdt.aml", "DSDT", 1, "CONFAI", "TRUSTED ", 0x00000002)
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
