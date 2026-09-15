DefinitionBlock ("", "SSDT", 2, "HOSTXX", "SECOND", 1)
{
    Scope (\_SB)
    {
        Device (SECN)
        {
            Name (_HID, "CFA0003")
            Method (_STA, 0, NotSerialized) { Return (0x0F) }
        }
    }
}
