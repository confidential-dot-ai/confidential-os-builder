# Contributing to confos

Participation in this project is covered by our
[Code of Conduct](CODE_OF_CONDUCT.md). A map of the codebase and the design
invariants to preserve is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Setup

Follow the [Installation](README.md#installation) section: clone, run
`bin/setup`, and use `bin/confos` to build-and-run the CLI. The build host
requirements (real Linux, sudo, user namespaces) apply to development too.

## Before you open a PR

- `bin/test` — full test suite (cargo-nextest). Must pass.
- `bin/lint` — rustfmt + clippy over all targets. Must be warning-free.
- `cargo deny check` — dependency licenses and advisories (`deny.toml`).
  Only needed when you've touched `Cargo.toml`/`Cargo.lock`, but CI gates
  on it either way.

CI runs `bin/test` on Linux (x86 and arm) and macOS, and `bin/lint`,
`cargo deny check`, and a release build on Linux.

Image-building integration tests need the mkosi host capabilities described
in the README; the pure-Rust unit tests run anywhere.

## Conventions

- Use [conventional commit messages](https://www.conventionalcommits.org/).
- Keep docs in sync: if you add or change a CLI flag, update the matching
  table in README.md in the same change.

## Releasing

Run the **prepare release** workflow from `main` with the version to cut
(`gh workflow run prepare-release.yml -f version=X.Y.Z`). It opens a
`release: vX.Y.Z` PR that bumps the package version and moves the
changelog's `[Unreleased]` entries into a dated section. Add a lead
paragraph to that section saying whether the release changes measurements
([docs/VERSIONING.md](docs/VERSIONING.md)), then merge.

Merging is the release: the **tag release** workflow tags the merge commit
`vX.Y.Z`, and the tag starts the cargo-dist workflow that builds the
binaries and publishes the GitHub release. Nothing is tagged unless the
merge raises the version in `Cargo.toml`.

## License

By contributing you agree that your contributions are licensed under the
[GNU Affero General Public License v3.0](LICENSE).
