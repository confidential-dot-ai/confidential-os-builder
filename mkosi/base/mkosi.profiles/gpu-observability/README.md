# GPU observability profile

This profile adds the pinned NVIDIA DCGM Exporter as a guest service. It uses
the NVIDIA CDI device exposed by the guest GPU profile and listens on
`127.0.0.1:9400`. The exporter sees only the GPUs passed through to this CVM.

The counter set intentionally excludes profiling fields and NVLink data-link
fields that are not available from Blackwell confidential-compute guests.
`DCGM_FI_DEV_CC_MODE` is retained so the metrics stream reports CC state.

Compose this profile with `observability`, `gpu`, and `attest-gpu`.
