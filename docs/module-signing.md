# Module signing

The guest kernel loads exactly two out-of-tree modules — `nvidia.ko` and
`nvidia-uvm.ko` — and `CONFIG_LOCK_DOWN_KERNEL_FORCE_CONFIDENTIALITY`
refuses to load them unsigned. That is the only reason signing exists here.

Integrity does **not** rest on these signatures (GPU-IMAGE-PLAN.md D4): the
module bytes live in the dm-verity-measured rootfs, and the boot sequence
latches `kernel.modules_disabled=1` once they are loaded. The signature is
lockdown plumbing.

## How it is split

- **Public certificate — `kernel/module-signing.crt`, committed.** The build
  stages it into the kernel tree and `CONFIG_SYSTEM_TRUSTED_KEYS` builds it
  into the system keyring, so it is part of the measurement. This is what
  makes vmlinuz reproducible: a fixed repo input replaced the per-build
  generated certificate that used to roll every measurement on a cache miss
  (#85).
- **Private key — never in this repo.** It exists only where modules are
  signed: `bin/steep-fetch-gpu` reads it from `MODULE_SIG_KEY` (a path) or
  `MODULE_SIG_KEY_PEM` (contents, for CI). `CONFIG_MODULE_SIG_ALL` is off,
  so the kernel build itself never needs it.

The build fails closed if the private key is missing, and refuses to sign if
its public half does not match the committed certificate — a mismatch would
otherwise surface as a GPU node that boots without a driver.

## Generating the keypair

Run once; keep the private key in a password manager or KMS as well as the CI
secret, because losing it means re-issuing the certificate and rolling the
measurement.

```sh
cat > /tmp/x509.genkey <<'EOF'
[ req ]
default_bits = 4096
distinguished_name = req_distinguished_name
prompt = no
string_mask = utf8only
x509_extensions = myexts

[ req_distinguished_name ]
O = <your org>
CN = confos guest module signing
emailAddress = <owner>

[ myexts ]
basicConstraints = critical,CA:FALSE
keyUsage = digitalSignature
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
EOF

openssl req -new -nodes -utf8 -sha512 -days 36500 -batch -x509 \
  -config /tmp/x509.genkey -outform PEM \
  -out kernel/module-signing.crt \
  -keyout /tmp/module-signing.key
```

Then:

1. Commit `kernel/module-signing.crt` (public — safe to publish).
2. Store the **key + certificate concatenated** as one PEM in the GitHub
   Actions secret `MODULE_SIG_KEY_PEM`, for the workflows that build GPU
   images:
   ```sh
   cat /tmp/module-signing.key kernel/module-signing.crt   # -> paste as the secret
   ```
3. Shred `/tmp/module-signing.key` and `/tmp/x509.genkey`.

`-days 36500` avoids an expiry that would silently stop module loading on a
long-lived image; the certificate is not a revocation-managed PKI.

## Rotation

Rotating the certificate changes the kernel measurement, so it is a reviewed
PR plus an attestation reference-value refresh — the same procedure as any
pin bump. Rotating **only** the private key is not possible: the certificate
carries its public half, so both move together.
