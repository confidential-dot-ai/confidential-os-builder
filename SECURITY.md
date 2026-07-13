# Security Policy

## Reporting a vulnerability

Please report suspected vulnerabilities privately via
[GitHub private vulnerability reporting](https://github.com/confidential-dot-ai/steep/security/advisories/new).
Do not open public issues for security reports.

We will acknowledge reports within a few business days. Please include enough
detail to reproduce the issue (config, commands, and the manifest of an
affected build if relevant).

## Scope

Anything that breaks the measurement/attestation guarantees is in scope —
e.g. builds that are not reproducible when they should be, unmeasured content
reachable in the verity root, IGVM/TDX measurement computation errors, or
`steep run` weakening the documented guest posture. Host-side tooling bugs
without security impact are ordinary issues.
