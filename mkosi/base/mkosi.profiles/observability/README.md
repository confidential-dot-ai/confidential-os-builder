# Observability profile

This profile provides local CPU and VM metrics without selecting a customer
telemetry destination. `prometheus-node-exporter` listens on
`127.0.0.1:9100`. The stock OpenTelemetry Collector Contrib image is pinned
by digest and, when Docker is available, scrapes the local exporters and
serves an aggregated Prometheus endpoint on `127.0.0.1:8889`.

No remote-write URL, OTLP destination, certificate, API key, or customer label
is included. A later image or provisioning update can add an outbound exporter
once the customer destination is defined.

The collector is limited to one CPU, 512 MiB RAM with no additional swap,
and 128 processes. Its Go memory target is 256 MiB and the collector memory
limiter uses a 384 MiB hard threshold with a 96 MiB spike allowance.
Docker's local log driver rotates three 10 MB files. These limits bound
telemetry overhead, so sustained overload can drop metrics rather than
consume the workload's memory. They do not cap the native GPU exporter.

Filesystem metrics include writable OS overlays and the user-data disk.
Virtual filesystems and individual Docker container mounts are excluded.
All endpoints are unauthenticated and must remain on loopback unless a
protected access path is explicitly configured.

Run `python3 tests/observability_config.py` on a Docker host to validate
both collector configurations against the pinned container before building.
