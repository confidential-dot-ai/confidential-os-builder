#!/usr/bin/env bash

set -euo pipefail

TEST_SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
readonly TEST_SCRIPT_DIR
readonly PREPARE_RELEASE="$TEST_SCRIPT_DIR/../prepare-release"
readonly REPO_URL=https://github.com/confidential-dot-ai/confidential-os-builder

fail() {
  echo "test-prepare-release: $*" >&2
  exit 1
}

test_dir=$(mktemp -d "${TMPDIR:-/tmp}/test-prepare-release.XXXXXXXX")
trap 'rm -rf "$test_dir"' EXIT

# A tree shaped like the repo's: a dependency that shares the package's
# version, so only a scoped edit leaves it alone.
write_tree() {
  local dir=$1 unreleased=$2
  mkdir -p "$dir"
  cat >"$dir/Cargo.toml" <<'EOF'
[package]
name = "confos"
version = "0.6.0"
edition = "2021"

[dependencies]
other = { version = "0.6.0" }
EOF
  cat >"$dir/Cargo.lock" <<'EOF'
version = 4

[[package]]
name = "aaa"
version = "0.6.0"

[[package]]
name = "confos"
version = "0.6.0"
dependencies = [
 "aaa",
]

[[package]]
name = "confos-extra"
version = "0.6.0"
EOF
  {
    printf '# Changelog\n\nIntro.\n\n## [Unreleased]\n'
    printf '%s' "$unreleased"
    printf '## [0.6.0] — 2026-11-02\n\n### Fixed\n- Old fix.\n\n'
    printf '[Unreleased]: %s/compare/v0.6.0...HEAD\n' "$REPO_URL"
    printf '[0.6.0]: %s/compare/v0.5.0...v0.6.0\n' "$REPO_URL"
  } >"$dir/CHANGELOG.md"
}

readonly ENTRIES=$'\n### Changed\n- New thing.\n\n'

case_number=0
new_case() {
  case_number=$((case_number + 1))
  case_dir="$test_dir/case-$case_number"
  write_tree "$case_dir" "${1-$ENTRIES}"
}

expect_failure() {
  local version=$1 date=$2 message=$3
  cp -R "$case_dir" "$case_dir.before"
  if bash "$PREPARE_RELEASE" "$version" "$date" "$case_dir" \
    >"$test_dir/stdout" 2>"$test_dir/stderr"; then
    fail "$version on $date unexpectedly succeeded"
  fi
  grep -q -- "$message" "$test_dir/stderr" || {
    cat "$test_dir/stderr" >&2
    fail "$version on $date did not report: $message"
  }
  diff -r "$case_dir.before" "$case_dir" >/dev/null ||
    fail "$version on $date failed but still changed files"
}

# The happy path, compared byte for byte.
new_case
bash "$PREPARE_RELEASE" 0.7.0 2026-11-16 "$case_dir" >/dev/null ||
  fail "0.7.0 should prepare"

expected="$test_dir/expected"
mkdir -p "$expected"
cat >"$expected/Cargo.toml" <<'EOF'
[package]
name = "confos"
version = "0.7.0"
edition = "2021"

[dependencies]
other = { version = "0.6.0" }
EOF
cat >"$expected/Cargo.lock" <<'EOF'
version = 4

[[package]]
name = "aaa"
version = "0.6.0"

[[package]]
name = "confos"
version = "0.7.0"
dependencies = [
 "aaa",
]

[[package]]
name = "confos-extra"
version = "0.6.0"
EOF
{
  printf '# Changelog\n\nIntro.\n\n## [Unreleased]\n\n'
  printf '## [0.7.0] — 2026-11-16\n\n### Changed\n- New thing.\n\n'
  printf '## [0.6.0] — 2026-11-02\n\n### Fixed\n- Old fix.\n\n'
  printf '[Unreleased]: %s/compare/v0.7.0...HEAD\n' "$REPO_URL"
  printf '[0.7.0]: %s/compare/v0.6.0...v0.7.0\n' "$REPO_URL"
  printf '[0.6.0]: %s/compare/v0.5.0...v0.6.0\n' "$REPO_URL"
} >"$expected/CHANGELOG.md"
diff -ru "$expected" "$case_dir" || fail "0.7.0 produced an unexpected tree"

# A patch release is as valid as a minor one.
new_case
bash "$PREPARE_RELEASE" 0.6.1 2026-11-16 "$case_dir" >/dev/null ||
  fail "0.6.1 should prepare"
grep -q '^## \[0.6.1\] — 2026-11-16$' "$case_dir/CHANGELOG.md" ||
  fail "0.6.1 heading missing"

# Components compare as numbers, not strings.
new_case
bash "$PREPARE_RELEASE" 0.10.0 2026-11-16 "$case_dir" >/dev/null ||
  fail "0.10.0 should be newer than 0.6.0"

new_case
expect_failure 0.6.0 2026-11-16 'not newer than 0.6.0'
new_case
expect_failure 0.5.9 2026-11-16 'not newer than 0.6.0'
new_case
expect_failure v0.7.0 2026-11-16 'invalid version'
new_case
expect_failure 0.7 2026-11-16 'invalid version'
new_case
expect_failure 0.7.0-rc.1 2026-11-16 'invalid version'
new_case
expect_failure 0.07.0 2026-11-16 'invalid version'
new_case
expect_failure 0.7.0 16-11-2026 'invalid date'
new_case $'\n'
expect_failure 0.7.0 2026-11-16 'nothing under \[Unreleased\]'
new_case
sed 's/^## \[Unreleased\]$/## Unreleased/' "$case_dir/CHANGELOG.md" >"$test_dir/tmp"
mv "$test_dir/tmp" "$case_dir/CHANGELOG.md"
expect_failure 0.7.0 2026-11-16 'no \[Unreleased\] section'
new_case
sed 's/^\[Unreleased\]: .*$//' "$case_dir/CHANGELOG.md" >"$test_dir/tmp"
mv "$test_dir/tmp" "$case_dir/CHANGELOG.md"
expect_failure 0.7.0 2026-11-16 'compare link'
new_case
sed 's|/compare/v0.6.0...HEAD$|/compare/v0.5.0...HEAD|' "$case_dir/CHANGELOG.md" >"$test_dir/tmp"
mv "$test_dir/tmp" "$case_dir/CHANGELOG.md"
expect_failure 0.7.0 2026-11-16 'starts at v0.5.0, not v0.6.0'
new_case
sed 's/^name = "confos"$/name = "renamed"/' "$case_dir/Cargo.lock" >"$test_dir/tmp"
mv "$test_dir/tmp" "$case_dir/Cargo.lock"
expect_failure 0.7.0 2026-11-16 'Cargo.lock'

echo "prepare-release tests passed"
