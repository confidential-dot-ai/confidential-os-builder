# Observability profile

This profile provides local CPU and VM metrics without selecting a customer
telemetry destination. `prometheus-node-exporter` listens on
`127.0.0.1:9100`. The stock OpenTelemetry Collector Contrib image is pinned
by digest and, when Docker is available, scrapes the local exporters and
serves an aggregated Prometheus endpoint on `127.0.0.1:8889`.

No remote-write URL, OTLP destination, certificate, API key, or customer label
is included. A later image or provisioning update can add an outbound exporter
once the customer destination is defined.
