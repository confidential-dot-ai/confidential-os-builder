DefinitionBlock ("", "DSDT", 2, "HOSTXX", "UNTRUST", 0xFFFFFFFF)
{
    Name (\_S5, Package () { 0, 0, 0, 0 })
    Scope (\_SB)
    {
        Device (HOST)
        {
            Name (_HID, "CFA0002")
            Method (_STA, 0, NotSerialized) { Return (0x0F) }
        }
    }
}
