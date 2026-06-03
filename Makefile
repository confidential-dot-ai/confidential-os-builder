# steep — composable build entry points.
#
# Underlying commands live in bin/. The Makefile chains together host-side
# prep (e.g. fetching binaries from registries for `--profile` builds) with
# the actual mkosi build, so day-to-day use is a single `make <target>`.

.PHONY: build build-attest fetch-attest clean

# Base image — no profile. Produces output/base/{disk.raw, uki.efi, guest.igvm, ...}.
build:
	bin/steep-safe build

# Base image + attest profile: pulls the attestation-api binary first,
# then builds with --profile attest. The systemd unit + config live in
# mkosi/base/mkosi.profiles/attest/; the binary is staged into
# mkosi.local/ by bin/steep-fetch-attest.
build-attest: fetch-attest
	bin/steep-safe build --profile attest

# Stage the attestation-api binary into mkosi.local/ for the attest profile.
# Idempotent — re-runs are cheap (one nerdctl pull, cached locally).
fetch-attest:
	bin/steep-fetch-attest

# Wipe per-build staging + the output directory. Safe to run any time.
clean:
	sudo rm -rf mkosi/base/mkosi.local
	rm -rf output/base
