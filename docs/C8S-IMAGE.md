# c8s node image

The c8s node-image definition (profile, kernel fragments, build entry
point, validation scenarios) lives in the c8s repo:
[`node-guest-image/`](https://github.com/confidential-dot-ai/c8s/tree/main/node-guest-image)
([c8s#264](https://github.com/confidential-dot-ai/c8s/issues/264)).

This repo is consumed purely as the builder: the profile is staged via
`confos build --profile-dir`, the kernel fragment via
`--kernel-config-fragment`, and consumer inputs (`c8s-ref`,
`c8s-registry`) via `--sync-input`.

The guest runs directly from the read-only verity root. Writable overlays
cover `/var`, `/home`, `/root`, and `/tmp` by default. A node image must
declare any additional runtime state directory in its profile's
`mkosi.extra/usr/lib/confai/state.d/<NN-name>.conf`, one path per line.
For example, rke2 requires `/etc/rancher`. Each declared directory must
already exist in the built image; the initrd gives it an ephemeral writable
overlay and fails the boot if it is missing.

The IPE execution policy (`kernel/ipe.config`) is opt-in and a node image
must not opt in: it runs containers from the scratch disk, which the policy
would deny. The default kernel has no IPE and the initrd leaves the
capability set alone.
