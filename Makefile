# steep — composable build entry points.
#
# Underlying commands live in bin/. Profile-specific host-side prep is wired
# through mkosi lifecycle hooks (e.g. `mkosi/base/mkosi.profiles/attest/
# mkosi.sync` pulls the attestation-api binary from GHCR before the build),
# so single `bin/steep build --profile ...` invocations are self-contained.

.PHONY: build clean

# Base image — no profile. Produces output/base/{disk.raw, uki.efi, guest.igvm, ...}.
build:
	bin/steep build

# Wipe per-build staging + the output directory. Safe to run any time.
clean:
	sudo rm -rf mkosi/base/mkosi.local
	rm -rf output/base
