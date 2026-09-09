#!/usr/bin/env bash

set -euo pipefail

TEST_SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
readonly TEST_SCRIPT_DIR
readonly KERNEL_RELEASE="$TEST_SCRIPT_DIR/../kernel-release"

fail() {
  echo "test-kernel-release: $*" >&2
  exit 1
}

test_dir=$(mktemp -d "${TMPDIR:-/tmp}/test-kernel-release.XXXXXXXX")
trap 'rm -rf "$test_dir"' EXIT

case_number=0
expect_release() {
  local pinned=$1 expected=$2 metadata=$3 manifest actual
  case_number=$((case_number + 1))
  manifest="$test_dir/case-${case_number}.json"
  printf '%s\n' "$metadata" >"$manifest"
  if ! actual=$(bash "$KERNEL_RELEASE" "$manifest" "$pinned"); then
    fail "$pinned should resolve to $expected"
  fi
  [ "$actual" = "$expected" ] || fail "$pinned resolved to $actual, want $expected"
}

expect_failure() {
  local pinned=$1 message=$2 metadata=$3 manifest
  case_number=$((case_number + 1))
  manifest="$test_dir/case-${case_number}.json"
  printf '%s\n' "$metadata" >"$manifest"
  if bash "$KERNEL_RELEASE" "$manifest" "$pinned" \
    >"$test_dir/unexpected.stdout" 2>"$test_dir/unexpected.stderr"; then
    fail "$pinned unexpectedly resolved"
  fi
  grep -q "$message" "$test_dir/unexpected.stderr" || {
    cat "$test_dir/unexpected.stderr" >&2
    fail "$pinned did not report $message"
  }
}

expect_release 6.18.4 6.18.4 \
  '{"releases":[{"version":"6.18.4","moniker":"longterm","iseol":false}]}'
expect_release 6.18.3 6.18.4 \
  '{"releases":[{"version":"6.18.4","moniker":"longterm","iseol":false}]}'
expect_release 7.3 7.3 \
  '{"releases":[{"version":"7.3","moniker":"stable","iseol":false}]}'

expect_failure 6.16.12 'is EOL on kernel.org' \
  '{"releases":[{"version":"6.16.12","moniker":"stable","iseol":true}]}'
expect_failure 6.18.4 'invalid iseol metadata' \
  '{"releases":[{"version":"6.18.4","moniker":"longterm"}]}'
expect_failure 6.17.9 'is EOL or absent' \
  '{"releases":[{"version":"6.18.4","moniker":"longterm","iseol":false}]}'
expect_failure 6.18.3 'returned 2 stable records' \
  '{"releases":[{"version":"6.18.4","moniker":"longterm","iseol":false},{"version":"6.18.3","moniker":"stable","iseol":false}]}'
expect_failure '6.18.*' 'invalid pinned kernel version' \
  '{"releases":[]}'

echo "kernel release metadata tests passed"
