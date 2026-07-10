# Verifying a Steep Build

This guide is for the **relying party**: someone who wants to confirm that a
VM they are talking to (or an image they were handed) is exactly what a
trusted `steep build` produced. It covers three independent layers of
verification, from cheapest to most thorough:

1. [Artifact verification](#1-artifact-verification-offline) — offline hash
   checks against `manifest.json`.
2. [Launch attestation](#2-launch-attestation-runtime) — comparing a
   hardware-signed attestation report from a *running* guest against the
   manifest's reference measurements.
3. [Build reproduction](#3-build-reproduction-audit) — rebuilding the image
   from source to confirm the publisher's toolchain is honest.

## What you need, and what you must already trust

Every check below compares something against `manifest.json`. The manifest is
**reference data, not proof** — you must obtain it through a channel you
trust (the publisher's repository, a registry you trust, or your own build).
If an attacker can substitute the manifest, they can substitute the expected
values too. Steep does not currently sign published manifests; if you
republish builds, sign the manifest with your own infrastructure (e.g.
cosign) so downstream verifiers get a stronger anchor.

See [MANIFEST.md](MANIFEST.md) for the full schema. The fields for verification are:

| Field | What it pins |
|---|---|
| `outputs.uki.sha256`, `outputs.disk_image.sha256` | The exact boot artifact bytes |
| `snp_variants[].measurement.snp_launch_digest` | SEV-SNP launch digest, one per vCPU count |
| `snp_variants[].igvm.sha256` | The IGVM file that produces that digest |
| `tdx.mrtd`, `tdx.rtmr1`, `tdx.rtmr2` | TDX reference registers |
| `tdx.firmware.sha256` | File hash of the TDVF/OVMF binary that `mrtd` was computed from (the two values are different: `mrtd` measures the firmware's measured regions, not the whole file) |

## 1. Artifact verification (offline)

Confirms that files you received match the manifest. No hardware needed.

```bash
cd output/myimage   # or wherever `steep pull` placed the artifacts

# Compare each artifact hash against the manifest
jq -r '.outputs.uki.sha256 + "  uki.efi",
       .outputs.disk_image.sha256 + "  disk.raw"' manifest.json | sha256sum -c
```

You can also recompute the measurements from the artifacts, without trusting
the manifest's precomputed values:

```bash
# SNP: recompute the launch digest from the IGVM file itself
igvm-tools measure guest-smp4.igvm
# → must equal .snp_variants[] entry with smp == 4 → .measurement.snp_launch_digest

# TDX: recompute MRTD/RTMR values from firmware + UKI + disk
tdx-measure measure --firmware OVMF.tdx.fd --uki uki.efi --disk disk.raw
```

Both tools live in this repository (`crates/igvm-tools`,
`crates/tdx-measure`) and can be installed with
`cargo install --path crates/<name>`, so a verifier needs only the Rust
toolchain — not a full steep build host.

## 2. Launch attestation (runtime)

This is the real guarantee: the CPU signs a report of what was actually
loaded and measured at launch. A verifier compares the signed measurement
against the manifest and validates the signature chain up to the CPU vendor.

### AMD SEV-SNP

**Inside the guest**, fetch an attestation report. Steep's kernel builds the
guest driver in (`CONFIG_SEV_GUEST=y`), so `/dev/sev-guest` is always
available. Using [`snpguest`](https://github.com/virtee/snpguest):

The anti-replay property depends on the **verifier** choosing the nonce:
generate 64 random bytes on the verifier, send them to the guest, and later
check they round-tripped through the signed report. A nonce the guest picks
for itself proves nothing about freshness to you.

```bash
# On the verifier: generate the nonce and get it to the guest
head -c 64 /dev/urandom > request.bin
scp request.bin guest:   # or however you talk to your workload

# In the guest: request a report over the verifier's nonce
snpguest report report.bin request.bin
```

**On the verifier side**, validate and compare:

```bash
# 1. Fetch the AMD certificate chain (ARK → ASK → VCEK) for the chip
snpguest fetch ca pem ./certs --report report.bin   # or: snpguest fetch ca pem ./certs milan
snpguest fetch vcek pem ./certs report.bin

# 2. Verify the chain and the report signature
snpguest verify certs ./certs
snpguest verify attestation ./certs report.bin

# 3. Check the nonce round-tripped (REPORT_DATA == your request.bin)
# 4. Compare the measurement against the manifest
snpguest display report report.bin | grep -A2 "Measurement"
jq -r '.snp_variants[] | select(.smp == 4) | .measurement.snp_launch_digest' manifest.json
```

The two 48-byte SHA-384 values must be identical. **Match the `smp` variant
to the guest's actual vCPU count** — SMP count is part of the SNP launch
measurement, so a 4-vCPU guest only ever matches the `smp: 4` entry.

Also inspect the report's **policy** field (debug bit must be off for
production) and **platform info** (verify TSME/SMT state matches your
requirements). A correct measurement with `DEBUG=1` in the policy is not a
trustworthy guest.

### Intel TDX

**Inside the guest**, obtain a TD quote. Steep's kernel enables
`CONFIG_TDX_GUEST_DRIVER`, exposing `/dev/tdx_guest`; on 6.7+ kernels the
generic `configfs-tsm` report interface also works. Any quote-generation
client works (e.g. Intel's `trustauthority-cli`, `go-tdx-guest`, or a
`configfs-tsm` reader), as long as it passes a fresh verifier nonce as
`REPORTDATA`.

**On the verifier side:**

1. Verify the quote's signature chain with Intel DCAP's Quote Verification
   Library, Intel Trust Authority, or an equivalent QVL service. This proves
   the quote came from a genuine TDX module on genuine hardware.
2. Compare the quote's measurement registers against the manifest:
   - `MRTD` must equal `tdx.mrtd` — the measurement of the TDVF firmware's
     measured regions (`OVMF.tdx.fd`; the *file's* SHA-256 is recorded
     separately in `tdx.firmware.sha256`).
   - `RTMR[1]` must equal `tdx.rtmr1` — UKI PE image identity + GPT + boot
     service constants.
   - `RTMR[2]` must equal `tdx.rtmr2` — the UKI section measurement chain
     (kernel, cmdline, initrd, os-release).
3. Check the debug attributes in the quote body are cleared.

**RTMR[0] is deliberately not pinned.** It mixes VMM-supplied data (TD-HOB
and ACPI tables) that varies with memory size and vCPU topology; pinning it
would force one manifest entry per (smp × memory) combination. The security
gap this would normally leave — the VMM controls the ACPI DSDT, whose AML
bytecode the kernel executes at full privilege — is closed differently: the
initrd carries a trusted DSDT that *overrides* the VMM's copy at boot, and
that initrd is itself measured into RTMR[2] (and into the SNP launch digest).
So verifying RTMR[2] transitively verifies the executable ACPI content. See
the "Trusted DSDT" section of the [README](../README.md) for the mechanism.

You can additionally cross-check from inside a booted TDX guest:

```bash
# Replay the CC event log against a TDREPORT and check UKI digests
tdx-measure verify --ccel /sys/firmware/acpi/tables/data/CCEL \
                   --tdreport tdreport.bin --uki uki.efi

# Confirm the trusted-DSDT override actually fired ("override" is the
# important word — "install" alone means the VMM's DSDT is still live)
dmesg | grep "Table Upgrade: override"
```

## 3. Build reproduction (audit)

The strongest check: rebuild the image yourself and confirm the publisher
isn't shipping a tampered toolchain.

A caveat on scope first. What [REPRODUCIBILITY.md](REPRODUCIBILITY.md)
establishes today is that the **base image** (same roothash and UKI SHA-256)
is bit-identical across **consecutive builds with the same pinned
toolchain**. Reproduction across independent hosts is the goal and requires
matching the publisher's environment exactly — same steep commit, same
`bin/setup`-installed tool versions (mkosi in particular), same Ubuntu
package snapshot state. It has not been validated as broadly as the
consecutive-build case, so treat a mismatch as a signal to bisect, not as
proof of tampering.

```bash
git clone https://github.com/confidential-dot-ai/steep.git && cd steep
git checkout <the commit the publisher built from>
bin/setup
bin/steep build
diff <(jq -S 'del(.build.timestamp)' output/base/manifest.json) \
     <(jq -S 'del(.build.timestamp)' /path/to/published/manifest.json)
```

If everything matches (`outputs.*.sha256` and the measurement values), the
published artifacts are exactly what the source at that commit produces. If
something differs, compare the `inputs.*` hashes in the two manifests first
— they bisect the divergence to a specific input (kernel binary, config
fragments, initrd, firmware, base image) rather than leaving you staring at
two different launch digests.

## Verification checklist

For a production deployment, all of these should hold:

- [ ] `manifest.json` obtained via a trusted channel
- [ ] Attestation report signature chain validates to the CPU vendor root
- [ ] Fresh nonce round-tripped through the report/quote
- [ ] Measurement matches the manifest (correct `smp` variant on SNP)
- [ ] Debug policy bits are off
- [ ] Image was **not** built with `--profile dev`
- [ ] (TDX) `dmesg` shows the DSDT `override` fired
