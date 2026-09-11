# Tell whoever logs in why a write under /usr or /etc fails with
# "Read-only file system": the root is the verity image, and only the
# directories the image declares in /usr/lib/confai/state.d are writable.
# Reads the declarations live so profiles' additions show up. Sourced by
# /etc/profile for login shells; silent otherwise.
case $- in *i*) ;; *) return 0 2>/dev/null || exit 0 ;; esac
_confai_state=""
for _f in /usr/lib/confai/state.d/*.conf; do
    [ -e "$_f" ] || continue
    while read -r _d || [ -n "$_d" ]; do
        _d="${_d%$(printf '\r')}"
        case "$_d" in "" | \#*) continue ;; esac
        _confai_state="$_confai_state /${_d#/}"
    done < "$_f"
done
printf 'Immutable root: /usr and /etc are the read-only verity image; a write there fails with "Read-only file system".\nWritable (ephemeral state overlays):%s\nDeclare more in /usr/lib/confai/state.d — see docs/THREAT_MODEL.md in confidential-os-builder.\n' "$_confai_state"
unset _confai_state _f _d
