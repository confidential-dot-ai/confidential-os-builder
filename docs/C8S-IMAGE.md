# c8s node image

The c8s node-image definition (profile, kernel fragments, build entry
point, validation scenarios) lives in the c8s repo:
[`node-guest-image/`](https://github.com/confidential-dot-ai/c8s/tree/main/node-guest-image)
([c8s#264](https://github.com/confidential-dot-ai/c8s/issues/264)).

This repo is consumed purely as the builder: the profile is staged via
`confos build --profile-dir`, the kernel fragment via
`--kernel-config-fragment`, and consumer inputs (`c8s-ref`,
`c8s-registry`) via `--sync-input`.
