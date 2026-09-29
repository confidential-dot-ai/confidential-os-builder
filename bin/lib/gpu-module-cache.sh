#!/usr/bin/env bash
# Unsigned artifacts only. Callers must include every compilation input in
# the key, and must copy/re-sign restored modules before staging an image.

gpu_module_cache_key() {
    sha256sum "$@" | sha256sum | cut -d' ' -f1
}

gpu_module_cache_restore() {
    local cache="$1" destination="$2" ko
    shift 2
    for ko in "$@"; do
        [[ -s "$cache/$ko" && -f "$cache/$ko.sha256" ]] || return 1
        [[ "$(sha256sum "$cache/$ko" | cut -d' ' -f1)" == "$(cat "$cache/$ko.sha256")" ]] || return 1
    done
    mkdir -p "$destination"
    for ko in "$@"; do cp "$cache/$ko" "$destination/$ko"; done
}

gpu_module_cache_store() {
    local cache="$1" source="$2" ko
    shift 2
    mkdir -p "$cache"
    for ko in "$@"; do
        [[ -s "$source/$ko" ]] || return 1
        install -m0644 "$source/$ko" "$cache/$ko"
        sha256sum "$source/$ko" | cut -d' ' -f1 > "$cache/$ko.sha256"
    done
}
