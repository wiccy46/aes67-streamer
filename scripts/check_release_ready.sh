#!/usr/bin/env bash
set -euo pipefail

fail() {
    echo "$*" >&2
    exit 1
}

ROOT_DIR="${AES67_RELEASE_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$ROOT_DIR"

[[ -f VERSION ]] || fail "VERSION file is missing"
[[ -f CHANGELOG.md ]] || fail "CHANGELOG.md is missing"

VERSION="$(tr -d '[:space:]' < VERSION)"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]] \
    || fail "VERSION must be valid SemVer, got '$VERSION'"

if ! grep -Eq "^##[[:space:]]+\[?$VERSION\]?" CHANGELOG.md; then
    fail "CHANGELOG.md must contain a section for version $VERSION"
fi

echo "Release readiness check passed for $VERSION"
