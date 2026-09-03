#!/usr/bin/env bash

set -euo pipefail

TEST_SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
readonly TEST_SCRIPT_DIR
# shellcheck source=.github/scripts/kernel-checksum
source "$TEST_SCRIPT_DIR/../kernel-checksum"

fail() {
  echo "test-kernel-checksum: $*" >&2
  exit 1
}

make_signing_key() {
  local home="$1" identity="$2"

  mkdir -m 700 "$home"
  GNUPGHOME="$home" gpg --batch --no-options --pinentry-mode loopback \
    --passphrase '' --quick-generate-key "$identity" ed25519 sign 0 >/dev/null 2>&1
  GNUPGHOME="$home" gpg --batch --no-options --with-colons --list-keys |
    awk -F: '$1 == "fpr" { print $10; exit }'
}

sign_payload() {
  local home="$1" fingerprint="$2" payload="$3" manifest="$4"

  GNUPGHOME="$home" gpg --batch --no-options --yes --armor \
    --local-user "$fingerprint" --output "$manifest" --clearsign "$payload"
}

expect_failure() {
  local description="$1"
  shift

  if "$@" > "$test_dir/unexpected.stdout" 2> "$test_dir/unexpected.stderr"; then
    fail "$description unexpectedly succeeded"
  fi
}

test_dir=$(mktemp -d "${TMPDIR:-/tmp}/test-kernel-checksum.XXXXXXXX")
trap 'rm -rf "$test_dir"' EXIT

readonly test_version="6.17.3"
readonly test_artifact="linux-${test_version}.tar.xz"
readonly test_checksum="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
readonly test_other_checksum="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

signer_home="$test_dir/signer"
signer_fingerprint=$(make_signing_key "$signer_home" "Checksum Test <checksum-test@example.invalid>")
signer_key="$test_dir/signer.asc"
GNUPGHOME="$signer_home" gpg --batch --no-options --armor \
  --export "$signer_fingerprint" > "$signer_key"

inspection_home="$test_dir/inspection"
mkdir -m 700 "$inspection_home"
production_fingerprints=$(
  GNUPGHOME="$inspection_home" gpg --batch --no-options --with-colons \
    --show-keys "$KERNEL_ORG_CHECKSUM_KEY" |
    awk -F: '
      $1 == "pub" { want_fingerprint = 1; next }
      want_fingerprint && $1 == "fpr" { print $10; want_fingerprint = 0 }
    '
)
[ "$production_fingerprints" = "$KERNEL_ORG_CHECKSUM_FINGERPRINT" ] ||
  fail "the committed kernel.org key does not match the pinned fingerprint"

valid_payload="$test_dir/valid.payload"
valid_manifest="$test_dir/valid.asc"
printf '%s  %s\n' "$test_checksum" "$test_artifact" > "$valid_payload"
sign_payload "$signer_home" "$signer_fingerprint" "$valid_payload" "$valid_manifest"
if ! actual=$(verify_kernel_checksum \
  "$valid_manifest" "$test_version" "$signer_key" "$signer_fingerprint" \
  2> "$test_dir/valid.stderr"); then
  cat "$test_dir/valid.stderr" >&2
  fail "valid manifest verification failed"
fi
[ "$actual" = "$test_checksum" ] || fail "valid manifest returned the wrong checksum"

appended_manifest="$test_dir/appended-unsigned.asc"
cp "$valid_manifest" "$appended_manifest"
printf '%s  %s\n' "$test_other_checksum" "$test_artifact" >> "$appended_manifest"
if ! appended_actual=$(verify_kernel_checksum \
  "$appended_manifest" "$test_version" "$signer_key" "$signer_fingerprint" \
  2> "$test_dir/appended-unsigned.stderr"); then
  cat "$test_dir/appended-unsigned.stderr" >&2
  fail "valid signed payload with trailing unsigned data was rejected"
fi
[ "$appended_actual" = "$test_checksum" ] ||
  fail "unsigned data appended after the signed message influenced the checksum"

tampered_manifest="$test_dir/tampered.asc"
sed "s/$test_checksum/$test_other_checksum/" "$valid_manifest" > "$tampered_manifest"
expect_failure "tampered manifest" verify_kernel_checksum \
  "$tampered_manifest" "$test_version" "$signer_key" "$signer_fingerprint"

expect_failure "wrong trusted fingerprint" verify_kernel_checksum \
  "$valid_manifest" "$test_version" "$signer_key" \
  "0000000000000000000000000000000000000000"

duplicate_payload="$test_dir/duplicate.payload"
duplicate_manifest="$test_dir/duplicate.asc"
printf '%s  %s\n%s  %s\n' \
  "$test_checksum" "$test_artifact" "$test_other_checksum" "$test_artifact" > "$duplicate_payload"
sign_payload "$signer_home" "$signer_fingerprint" "$duplicate_payload" "$duplicate_manifest"
expect_failure "duplicate checksum entries" verify_kernel_checksum \
  "$duplicate_manifest" "$test_version" "$signer_key" "$signer_fingerprint"

malformed_payload="$test_dir/malformed.payload"
malformed_manifest="$test_dir/malformed.asc"
printf 'not-a-sha256  %s\n' "$test_artifact" > "$malformed_payload"
sign_payload "$signer_home" "$signer_fingerprint" "$malformed_payload" "$malformed_manifest"
expect_failure "malformed checksum entry" verify_kernel_checksum \
  "$malformed_manifest" "$test_version" "$signer_key" "$signer_fingerprint"

missing_payload="$test_dir/missing.payload"
missing_manifest="$test_dir/missing.asc"
printf '%s  linux-6.17.2.tar.xz\n' "$test_checksum" > "$missing_payload"
sign_payload "$signer_home" "$signer_fingerprint" "$missing_payload" "$missing_manifest"
expect_failure "missing checksum entry" verify_kernel_checksum \
  "$missing_manifest" "$test_version" "$signer_key" "$signer_fingerprint"

other_home="$test_dir/other-signer"
other_fingerprint=$(make_signing_key "$other_home" "Other Test <other-test@example.invalid>")
untrusted_manifest="$test_dir/untrusted.asc"
sign_payload "$other_home" "$other_fingerprint" "$valid_payload" "$untrusted_manifest"
expect_failure "untrusted signer" verify_kernel_checksum \
  "$untrusted_manifest" "$test_version" "$signer_key" "$signer_fingerprint"

other_key="$test_dir/other.asc"
GNUPGHOME="$other_home" gpg --batch --no-options --armor \
  --export "$other_fingerprint" > "$other_key"
combined_key="$test_dir/combined.asc"
cat "$signer_key" "$other_key" > "$combined_key"
expect_failure "a trust file containing an extra key" verify_kernel_checksum \
  "$valid_manifest" "$test_version" "$combined_key" "$signer_fingerprint"

expect_failure "invalid version" verify_kernel_checksum \
  "$valid_manifest" "6.17.*" "$signer_key" "$signer_fingerprint"

echo "kernel checksum verification tests passed"
