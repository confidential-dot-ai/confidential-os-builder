# Module signing

Images whose kernel fragment enables `CONFIG_MODULE_SIG` need a signing
certificate built into the kernel's system keyring, or lockdown-confidentiality
refuses to load the out-of-tree NVIDIA modules.

Integrity does **not** rest on these signatures (GPU-IMAGE-PLAN.md D4): the
module bytes live in the dm-verity-measured rootfs, and the boot sequence
latches `kernel.modules_disabled=1` once they are loaded. The signature is
lockdown plumbing.

## Certificate and key

| | Where | Notes |
|---|---|---|
| Public certificate | `kernel/module-signing.crt`, committed | The default. Built into the system keyring and shipped inside every image, so it is public by construction. Measured — its sha256 is a kernel fingerprint input, so replacing it changes the image measurement. |
| Private key | `MODULE_SIG_KEY_PEM` (contents) or `MODULE_SIG_KEY` (path) in `bin/confos-fetch-gpu`'s environment | Signs the out-of-tree modules via `scripts/sign-file`. Never committed; lives in the org-level Actions secret. |

A fresh clone builds a reproducible image with no setup: the default
certificate is already here, and only *signing* new modules needs the key.

To sign with your own key instead, pass `confos build --module-signing-cert
<path>` and set `MODULE_SIG_CERT` to the same file for `confos-fetch-gpu`.
Consumers that own their image's trust anchor do this — see the c8s repo's
`node-guest-image/`.

`CONFIG_MODULE_SIG_ALL` must stay **off**: after `mod2yesconfig` there are no
in-tree modules to sign, and enabling it makes the kernel build generate its
own key, which is what broke reproducibility in #85. `confos` enforces this
against the resolved `.config` — it is not just a convention.

`confos-fetch-gpu` refuses to sign if the key's public half does not match the
certificate, since that mismatch would otherwise appear as a node booting
without its GPU driver.

## Generating the default keypair

Run once. Keep the private key in a password manager or KMS as well as the
Actions secret: losing it means issuing a new certificate and rolling the
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
O = Confidential AI
CN = Confidential AI module signing
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
2. Store the **key and certificate concatenated** as one PEM in the
   organisation-level Actions secret `MODULE_SIG_KEY_PEM`, so both this repo's
   and consumers' GPU builds inherit it:
   ```sh
   cat /tmp/module-signing.key kernel/module-signing.crt   # -> paste as the secret
   ```
3. Shred `/tmp/module-signing.key` and `/tmp/x509.genkey`.

`-days 36500` avoids an expiry that would silently stop module loading on a
long-lived image; this is not a revocation-managed PKI.

## What the default certificate does and does not buy

Anyone holding the default key can sign a module that loads in an image built
with the default certificate. That is acceptable here for the reason above —
verity measures the module bytes and `modules_disabled=1` latches after boot,
so the signature is not the trust root. An image that wants a stronger claim
supplies its own certificate and keeps the key to itself.

## Rotation

Replacing the certificate changes the kernel measurement, so it is a reviewed
PR plus an attestation reference-value refresh — the same procedure as any pin
bump. The private key cannot be rotated on its own: the certificate carries its
public half, so both move together.
