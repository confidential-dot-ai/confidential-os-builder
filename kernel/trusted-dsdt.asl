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

                /* 64-bit MMIO window (high): 2TiB..256TiB. Firmware placement
                 * depends on guest physical-address width and the OVMF MMIO
                 * aperture. Observed placements include 56TiB on a 256GiB
                 * guest and 224TiB on a 1TiB guest. The former 64TiB ceiling
                 * excluded the latter's virtio BARs, preventing boot-disk
                 * discovery even without GPUs.
                 *
                 * This is a resource-accounting window, not a declaration of
                 * available physical-address bits. It must remain disjoint
                 * from guest System RAM; configurations reaching this window
                 * need a different layout. Large GPU BARs still require a
                 * sufficient OVMF aperture (fw_cfg opt/ovmf/X-PciMmio64Mb);
                 * widening _CRS alone does not size that aperture. */
                QWordMemory (ResourceProducer, PosDecode, MinFixed, MaxFixed,
                    Cacheable, ReadWrite,
                    0x0000000000000000, 0x0000020000000000, 0x0000FFFFFFFFFFFF,
                    0x0000000000000000, 0x0000FE0000000000)
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
