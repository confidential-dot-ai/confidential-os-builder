# Module signing

Images whose kernel fragment enables `CONFIG_MODULE_SIG` need a signing
certificate built into the kernel's system keyring, or lockdown-confidentiality
refuses to load the out-of-tree NVIDIA modules.

Integrity does **not** rest on these signatures: the
module bytes live in the dm-verity-measured rootfs, and the boot sequence
latches `kernel.modules_disabled=1` once they are loaded. SSH and the
container runtimes are ordered after that latch, and the guest powers off if
it cannot be set. The signature is lockdown plumbing.

If the latch command fails, or has not returned after 30 seconds, the guest
powers off at once, with no retry. No failed unit is left to inspect and SSH
never starts. If the latch unit is missing or cannot be loaded, nothing
powers off: the guest stays up, and SSH and the container runtimes do not
start at boot.

The ordering covers the boot. It does not stop a process that is already
root from starting those units by hand or stopping the latch unit. The image
has no console login, and cloud-init, which is not ordered against the latch,
runs only the seed baked into the measured image.

After a power-off the journal is lost with the guest, and a production image
copies it to no console. Reading the cause takes a separate debug build whose
journal reaches a serial port the host captures from before the boot. The
[`dev` profile](../mkosi/base/mkosi.profiles/dev/mkosi.conf) is the starting
point, but no such build of a GPU image is documented or has been exercised.
That profile gives a passwordless root login on the serial port, so use it
only for a disposable guest that holds no secrets. The capture is best
effort: a forced power-off can cut the last lines.

`attestation-api.service` does not wait for
the latch: it gives no shell and runs no workload, and a verifier may need
its GPU evidence before the GPU is marked ready.

## Certificate and key

| | Where | Notes |
|---|---|---|
| Public certificate | `kernel/module-signing.crt`, committed | The default. Built into the system keyring and shipped inside every image, so it is public by construction. Measured — its sha256 is a kernel fingerprint input, so replacing it changes the image measurement. |
| Private key | `MODULE_SIG_KEY_PEM` (contents) or `MODULE_SIG_KEY` (path) in `bin/confos-fetch-gpu`'s environment | Signs the out-of-tree modules via `scripts/sign-file`. Never committed; lives in the org-level Actions and Dependabot secrets. |

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
   and consumers' GPU builds inherit it. Store the same PEM as the
   organisation-level Dependabot secret of the same name: runs triggered by
   Dependabot read only Dependabot secrets, and their GPU builds fail without
   it. Set both from a file, which keeps the line breaks intact. The file
   holds the private key, so it goes in a directory only you can read:
   ```sh
   org=your-org
   pem_dir=$(umask 077 && mktemp -d)
   (umask 077 && cat /tmp/module-signing.key kernel/module-signing.crt > "$pem_dir/module-signing.pem")
   gh secret set MODULE_SIG_KEY_PEM --org "$org" < "$pem_dir/module-signing.pem"
   gh secret set MODULE_SIG_KEY_PEM --org "$org" --app dependabot < "$pem_dir/module-signing.pem"
   rm -rf "$pem_dir"
   ```
   The key must be PEM (`BEGIN PRIVATE KEY`), not an OpenSSH-format key.
3. Shred `/tmp/module-signing.key` and `/tmp/x509.genkey`.

`-days 36500` avoids an expiry that would silently stop module loading on a
long-lived image; this is not a revocation-managed PKI.

## What the default certificate does and does not buy

Anyone holding the default key can sign a module that loads in an image built
with the default certificate. That is acceptable here for the reason above —
verity measures the module bytes and the boot sequence sets
`kernel.modules_disabled=1` once the driver is loaded, so the signature is not
the trust root. Until that write succeeds, a correctly signed module still
loads. An image that wants a stronger claim
supplies its own certificate and keeps the key to itself.

### Exposure in Dependabot runs

The Dependabot copy of the key is read by runs on Dependabot branches. Those
runs execute freshly bumped third-party actions before anyone has reviewed
them, in the same job that later reads the key, so a malicious release of one
of those actions could capture it. This is accepted for the default key, for
the reason above. Its holder can sign a module, but an image rebuilt with
that module has a different measurement and no longer matches the published
reference values, and a running image refuses the module once
`kernel.modules_disabled=1` is set.

A project that supplies its own certificate and relies on the key staying
private should leave the key out of Dependabot secrets. If no repository or
organisation Dependabot secret supplies `MODULE_SIG_KEY_PEM`, its Dependabot
runs stop at the GPU pre-check with `Missing MODULE_SIG_KEY_PEM secret`.

## Rotation

Replacing the certificate changes the kernel measurement, so it is a reviewed
PR plus an attestation reference-value refresh — the same procedure as any pin
bump. The private key cannot be rotated on its own: the certificate carries its
public half, so both move together.
