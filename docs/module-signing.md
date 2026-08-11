# Module signing

Images whose kernel fragment enables `CONFIG_MODULE_SIG` must supply their
own signing material — confos ships none, because the trust anchor for the
modules an image loads belongs to whoever owns that image.

Two inputs, both caller-owned:

| Input | Passed as | Role |
|---|---|---|
| Public certificate | `confos build --module-signing-cert <path>` | Staged into the kernel tree as `certs/confos-module-signing.crt` and built into the system keyring by the fragment's `CONFIG_SYSTEM_TRUSTED_KEYS`. Measured — its sha256 is a kernel fingerprint input, so rotating it changes the image measurement. |
| Private key | `MODULE_SIG_KEY=<path>` or `MODULE_SIG_KEY_PEM=<contents>` in `bin/steep-fetch-gpu`'s environment, alongside `MODULE_SIG_CERT=<same certificate>` | Signs the out-of-tree NVIDIA modules via `scripts/sign-file`. Never enters this repo or the kernel build. |

`CONFIG_MODULE_SIG_ALL` must stay **off** in consumer fragments: after
`mod2yesconfig` there are no in-tree modules to sign, and leaving it on is
what would force a private key into the kernel build — the situation that
made vmlinuz non-reproducible (#85).

`steep-fetch-gpu` refuses to sign if the key's public half does not match
the certificate, since that mismatch would otherwise appear as a node
booting without its GPU driver.

For a worked example — key generation, CI secret wiring, and rotation — see
the c8s repo's `node-guest-image/MODULE-SIGNING.md`.
