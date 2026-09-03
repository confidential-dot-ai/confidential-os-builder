# Kernel.org checksum trust anchor

`kernel-org-checksum-autosigner.asc` is the kernel.org checksum autosigner key
for `autosigner@kernel.org`, retrieved through kernel.org's official WKD. Its
pinned primary fingerprint is:

```text
B886 8C80 BA62 A1FF FAF5  FDA9 632D 3A06 589D A6B1
```

This key authenticates checksum metadata published through kernel.org mirrors.
It does not authenticate a tarball as a kernel developer release and is not a
substitute for verifying developer tarball signatures.

The pin-watch workflow verifies that exact fingerprint before creating an
isolated keyring and accepting a signed checksum manifest. This makes a key
rotation fail closed. To rotate the key, verify the replacement fingerprint
through official kernel.org channels, then update the key and the fingerprint
in `.github/scripts/kernel-checksum` together in a reviewed change. See
[kernel.org's signature documentation](https://www.kernel.org/signature.html).
