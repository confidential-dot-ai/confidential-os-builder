#!/usr/bin/env bash

set -euo pipefail

TEST_SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
readonly TEST_SCRIPT_DIR
readonly PREPARE_RELEASE="$TEST_SCRIPT_DIR/../prepare-release"
readonly RELEASE_TAG="$TEST_SCRIPT_DIR/../release-tag"
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
  local dir=$1
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
    printf '# Changelog\n\nIntro.\n\n## [Unreleased]\n\n'
    printf '### Changed\n- New thing.\n\n'
    printf '## [0.6.0] — 2026-11-02\n\n### Fixed\n- Old fix.\n\n'
    printf '[Unreleased]: %s/compare/v0.6.0...HEAD\n' "$REPO_URL"
    printf '[0.6.0]: %s/compare/v0.5.0...v0.6.0\n' "$REPO_URL"
  } >"$dir/CHANGELOG.md"
}

case_number=0
new_case() {
  case_number=$((case_number + 1))
  case_dir="$test_dir/case-$case_number"
  write_tree "$case_dir"
}

# Edit FILE of the current case in place with a sed expression.
mutate() {
  sed "$2" "$case_dir/$1" >"$test_dir/tmp"
  mv "$test_dir/tmp" "$case_dir/$1"
}

# A fresh tree, optionally mutated, must be refused and left untouched.
expect_failure() { # VERSION DATE MESSAGE [FILE SED_EXPR]; env passes through
  local version=$1 date=$2 message=$3
  new_case
  if [ "$#" -eq 5 ]; then mutate "$4" "$5"; fi
  cp -R "$case_dir" "$case_dir.before"
  if bash "$PREPARE_RELEASE" "$version" "$date" "$case_dir" \
    >/dev/null 2>"$test_dir/stderr"; then
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

# Puts a hand-written lead under [Unreleased]. Backslash-newline is the
# portable way to add lines in a sed replacement.
readonly WITH_LEAD=$'s/^## \\[Unreleased\\]$/&\\\n\\\nHand-written lead. Alters measurements./'

# The lead paragraph. The fixture's entry is unmarked, so the tree above got
# none; a marked entry gets the standard one, between heading and entries.
lead_of() { # print the lines between the 0.7.0 heading and its first subsection
  awk '/^## \[0.7.0\]/ { on = 1; next } /^###/ { on = 0 } on && NF' "$case_dir/CHANGELOG.md"
}

new_case
mutate CHANGELOG.md 's/^- New thing\.$/- New thing. **Alters measurements** of x./'
bash "$PREPARE_RELEASE" 0.7.0 2026-11-16 "$case_dir" >/dev/null
lead_of | head -1 | grep -q '^\*\*Changes measurements\.\*\* Images built from this release have new' ||
  fail "a marked entry should get the standard lead"
grep -A1 '^## \[0.7.0\]' "$case_dir/CHANGELOG.md" | tail -1 | grep -q '^$' ||
  fail "the lead should be set off from the heading by a blank line"
bash "$RELEASE_TAG" "$test_dir/case-1/Cargo.toml" "$case_dir/Cargo.toml" \
  "$case_dir/CHANGELOG.md" >/dev/null || fail "release-tag should accept a section with a lead"

# RELEASE_SUMMARY replaces the standard wording, marked entry or not, and
# is reflowed into one paragraph.
new_case
RELEASE_SUMMARY=$'**Changes measurements.**  Kernel moves\nto 6.18.62. '"$(printf 'word %.0s' {1..20})" \
  bash "$PREPARE_RELEASE" 0.7.0 2026-11-16 "$case_dir" >/dev/null
lead_of | head -1 | grep -q '^\*\*Changes measurements\.\*\* Kernel moves to 6.18.62. word' ||
  fail "RELEASE_SUMMARY should become the lead"
[ "$(lead_of | wc -l)" -eq 2 ] || fail "a long summary should wrap onto a second line"
if lead_of | grep -q '.\{79\}'; then fail "lead lines should fit in 78 columns"; fi

# A lead already written under [Unreleased] is kept as it is.
new_case
mutate CHANGELOG.md "$WITH_LEAD"
bash "$PREPARE_RELEASE" 0.7.0 2026-11-16 "$case_dir" >/dev/null
[ "$(lead_of)" = 'Hand-written lead. Alters measurements.' ] ||
  fail "an existing lead should be left alone"

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

expect_failure 0.6.0 2026-11-16 'not newer than 0.6.0'
expect_failure 0.5.9 2026-11-16 'not newer than 0.6.0'
expect_failure v0.7.0 2026-11-16 'invalid version'
expect_failure 0.7 2026-11-16 'invalid version'
expect_failure 0.7.0-rc.1 2026-11-16 'invalid version'
expect_failure 0.07.0 2026-11-16 'invalid version'
expect_failure 0.7.0 16-11-2026 'invalid date'
expect_failure 0.7.0 2026-11-16 'nothing under \[Unreleased\]' \
  CHANGELOG.md '/^### Changed$/d; /^- New thing\.$/d'
expect_failure 0.7.0 2026-11-16 'no \[Unreleased\] section' \
  CHANGELOG.md 's/^## \[Unreleased\]$/## Unreleased/'
expect_failure 0.7.0 2026-11-16 'compare link' \
  CHANGELOG.md 's/^\[Unreleased\]: .*$//'
expect_failure 0.7.0 2026-11-16 'starts at v0.5.0, not v0.6.0' \
  CHANGELOG.md 's|/compare/v0.6.0...HEAD$|/compare/v0.5.0...HEAD|'
RELEASE_SUMMARY='Another lead.' expect_failure 0.7.0 2026-11-16 'already opens with a lead' \
  CHANGELOG.md "$WITH_LEAD"
expect_failure 0.7.0 2026-11-16 'Cargo.lock' \
  Cargo.lock 's/^name = "confos"$/name = "renamed"/'

# release-tag: OLD_CARGO_TOML is an untouched tree's, the rest the case's.
new_case
readonly before="$case_dir/Cargo.toml"

expect_tag() { # EXPECTED
  local actual
  actual=$(bash "$RELEASE_TAG" "$before" "$case_dir/Cargo.toml" "$case_dir/CHANGELOG.md") ||
    fail "release-tag should print '$1'"
  [ "$actual" = "$1" ] || fail "release-tag printed '$actual', want '$1'"
}

expect_no_tag() { # MESSAGE [OLD_CARGO_TOML]
  if bash "$RELEASE_TAG" "${2:-$before}" "$case_dir/Cargo.toml" "$case_dir/CHANGELOG.md" \
    >/dev/null 2>"$test_dir/stderr"; then
    fail "release-tag unexpectedly succeeded, want: $1"
  fi
  grep -q -- "$1" "$test_dir/stderr" || {
    cat "$test_dir/stderr" >&2
    fail "release-tag did not report: $1"
  }
}

# What prepare-release writes is what release-tag accepts.
new_case
bash "$PREPARE_RELEASE" 0.7.0 2026-11-16 "$case_dir" >/dev/null
expect_tag v0.7.0

# A Cargo.toml change that leaves the version alone is not a release.
new_case
mutate Cargo.toml 's/^edition = .*/edition = "2024"/'
expect_tag ''

new_case
mutate Cargo.toml 's/^version = "0.6.0"/version = "0.7.0"/'
expect_no_tag 'no dated \[0.7.0\] section'
new_case
mutate Cargo.toml 's/^version = "0.6.0"/version = "0.5.0"/'
expect_no_tag 'went backwards, from 0.6.0 to 0.5.0'
new_case
mutate Cargo.toml 's/^version = "0.6.0"/version = "0.7.0-rc.1"/'
expect_no_tag 'tag it by hand'
# An unreadable version is an error on either side, never "unchanged".
new_case
mutate Cargo.toml 's/^version = "0.6.0"/version="0.6.0"/'
expect_no_tag 'no plain X.Y.Z'
expect_no_tag 'no plain X.Y.Z' "$case_dir/Cargo.toml"

echo "prepare-release tests passed"
