## What problem does this change solve?

## Why is this solution the best option to solve that problem?

## Checklist

- [ ] `bin/test` passes
- [ ] `bin/lint` is warning-free
- [ ] `cargo deny check` passes (required if `Cargo.toml`/`Cargo.lock`
      changed; CI gates on it regardless)
- [ ] Commit messages follow [conventional commits](https://www.conventionalcommits.org/)
- [ ] CLI flag changes are reflected in the README's matching table
- [ ] If this changes what a build produces: does it alter measurements of an
      unchanged config? Note it in `CHANGELOG.md` (see
      [docs/VERSIONING.md](https://github.com/confidential-dot-ai/confidential-os-builder/blob/main/docs/VERSIONING.md))
- [ ] If this touches the build pipeline: output stays deterministic
      (no timestamps, randomness, or unstable ordering — see the invariants
      in [docs/ARCHITECTURE.md](https://github.com/confidential-dot-ai/confidential-os-builder/blob/main/docs/ARCHITECTURE.md))
- [ ] If this changes the kernel config: `kernel/config-x86_64.snapshot`
      diff is committed and reviewed
- [ ] If this bumps `kernel/version`: re-verify every
      `# CONFIG_X is forced on` forcing chain against the new Kconfig
