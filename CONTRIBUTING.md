# Contributing to steep

## Setup

Follow the [Installation](README.md#installation) section: clone, run
`bin/setup`, and use `bin/steep` to build-and-run the CLI. The build host
requirements (real Linux, sudo, user namespaces) apply to development too.

## Before you open a PR

- `bin/test` — full test suite (cargo-nextest). Must pass.
- `bin/lint` — clippy over all targets. Must be warning-free.

CI runs `bin/test` on Linux (x86 and arm) and macOS, and `bin/lint` on Linux.

Image-building integration tests need the mkosi host capabilities described
in the README; the pure-Rust unit tests run anywhere.

## Conventions

- Use [conventional commit messages](https://www.conventionalcommits.org/).
- Keep docs in sync: if you add or change a CLI flag, update the matching
  table in README.md in the same change.

## License

By contributing you agree that your contributions are licensed under the
[Apache License 2.0](LICENSE).
