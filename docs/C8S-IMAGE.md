# c8s node image

The c8s node-image definition (profile, kernel fragments, build entry
point, validation scenarios) lives in the c8s repo:
[`node-guest-image/`](https://github.com/confidential-dot-ai/c8s/tree/main/node-guest-image)
([c8s#264](https://github.com/confidential-dot-ai/c8s/issues/264)).

This repo is consumed purely as the builder: the profile is staged via
`confos build --profile-dir`, the kernel fragment via
`--kernel-config-fragment`, and consumer inputs (`c8s-ref`,
`c8s-registry`) via `--sync-input`.

The guest root is immutable at runtime (`/usr` and `/etc` are the read-only
verity mount; only `/var`, `/home`, `/root`, `/tmp` are writable by
default). A node image whose services write elsewhere — rke2 under
`/etc/rancher`, for instance — must declare each such directory, one per
line, in its profile's `mkosi.extra/usr/lib/confai/state.d/<NN-name>.conf`;
the initrd then gives it an ephemeral writable overlay. Each listed
directory must already exist in the built image (baked by the profile), or
the initrd fails the boot rather than silently skipping it.
