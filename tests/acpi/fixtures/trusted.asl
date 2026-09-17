DefinitionBlock ("", "DSDT", 2, "CONFAI", "TRUSTED", 1)
{
    Name (\_S5, Package () { 0, 0, 0, 0 })
    Scope (\_SB)
    {
        Device (TRST)
        {
            Name (_HID, "CFA0001")
            Method (_STA, 0, NotSerialized) { Return (0x0F) }
        }
    }
}
