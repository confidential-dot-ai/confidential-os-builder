## What problem does this change solve?

## Why is this solution the best option to solve that problem?

## Checklist

- [ ] `bin/test` passes
- [ ] `bin/lint` is warning-free
- [ ] Commit messages follow [conventional commits](https://www.conventionalcommits.org/)
- [ ] CLI flag changes are reflected in the README's matching table
- [ ] If this changes what a build produces: does it alter measurements of an
      unchanged config? Note it in `CHANGELOG.md` (see
      [docs/VERSIONING.md](../docs/VERSIONING.md))
- [ ] If this touches the build pipeline: output stays deterministic
      (no timestamps, randomness, or unstable ordering — see the invariants
      in [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md))
- [ ] If this changes the kernel config: `kernel/config-x86_64.snapshot`
      diff is committed and reviewed
