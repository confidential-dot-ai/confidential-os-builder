# SNP boot diagnostics

Use the manual `SNP boot diagnostics` workflow when a guest is Running but its
attestation endpoint never starts. It uses GitHub-hosted x86 runners and the
existing `confos`, mkosi, dependency and cache tooling. It does not launch VMs or
change host, cluster, catalog or production measurement configuration.

The existing `Test` workflow can also call it from a reviewed branch: enable
`snp_boot_diagnostics`, provide `source_revision`, and optionally enable
`publish_diagnostic_candidate`. Other tests retain their existing behavior.
This permits testing the new workflow before it is merged; normal push and pull
request runs do not build or publish a diagnostic image.

Supply a full reviewed builder commit. The default is the source tag of the
accepted general-purpose image used for the failing launch. The workflow records
that source separately from the workflow revision and composes the existing
general-purpose and dev kernel fragments. It builds one 2-vCPU, 8-GiB SNP variant
with the existing dev profile. These are diagnostic parameters, not fleet limits.

The dev profile enables the serial console and passwordless root access through
that console. Use the result only for an isolated disposable test with generated
test keys and no customer secrets. Keep its exact manifest, launch measurement
and source receipt separate from production pins. A diagnostic success is not
acceptance of its console policy for customers.

With publication disabled, the run retains the image and build inputs as short-lived
workflow artifacts. Optional publication uses only unique `diagnostic-snp-` tags,
including source commit, run ID and attempt. It never advances ordinary image
aliases or modifies deployment configuration. The CDI provenance identifies the
workflow invocation; `diagnostic-build.json` also records the selected build source.

Publication uses the reviewed workflow revision's existing CLI, independently
of the selected guest-build source. This retains native OCI packaging fixes
when diagnosing an older guest source. A native Docker pull must unpack the
published CDI image before the publication is accepted.

To retry publication without rebuilding a kernel, supply `artifact_run_id`
(`diagnostic_artifact_run_id` through `Test`) for attempt 1 of a retained build
in this repository. The workflow verifies its original source receipt,
reference manifest and disk, UKI and IGVM hashes before using the current
publisher. It records build and publisher revisions separately and creates
new unique tags; guest measurements and production aliases remain unchanged.

Run the maintained Cloud minimal-launch fixture with the candidate's separately
reviewed manifest, then read its native serial log to locate the boot failure.
Keep the original non-debug image's failing case. Correct the owning image or
launch component and repeat hardware acceptance with the production profile
before opening customer capacity.
