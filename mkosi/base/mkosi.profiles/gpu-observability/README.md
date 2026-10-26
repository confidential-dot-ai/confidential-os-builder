# GPU observability profile

This profile adds `127.0.0.1:9400` to the OpenTelemetry Collector's scrape
targets. The native NVIDIA DCGM Exporter is supplied by `general-purpose-gpu`,
including its measured binaries, counter set and systemd service. This profile
does not install another exporter or override that service.

The counter set intentionally excludes profiling fields and NVLink data-link
fields that are not available from Blackwell confidential-compute guests.
`DCGM_FI_DEV_CC_MODE` is retained so the metrics stream reports CC state.

Compose this profile after `observability` with `general-purpose-gpu` and
its GPU dependencies. CPU-only recipes use `observability` without this profile.
