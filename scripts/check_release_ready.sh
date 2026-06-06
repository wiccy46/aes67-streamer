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
[[ -f src/aes67-streamer/Cargo.toml ]] || fail "src/aes67-streamer/Cargo.toml is missing"
[[ -f src/aes67-player/Cargo.toml ]] || fail "src/aes67-player/Cargo.toml is missing"

VERSION="$(tr -d '[:space:]' < VERSION)"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]] \
    || fail "VERSION must be valid SemVer, got '$VERSION'"

read_package_version() {
    local manifest="$1"
    awk '
        /^\[package\]/ { in_package = 1; next }
        /^\[/ { in_package = 0 }
        in_package && /^version[[:space:]]*=/ {
            gsub(/"/, "", $3)
            print $3
            exit
        }
    ' "$manifest"
}

STREAMER_VERSION="$(read_package_version src/aes67-streamer/Cargo.toml)"
PLAYER_VERSION="$(read_package_version src/aes67-player/Cargo.toml)"

[[ "$STREAMER_VERSION" == "$VERSION" ]] \
    || fail "src/aes67-streamer/Cargo.toml version $STREAMER_VERSION does not match VERSION $VERSION"
[[ "$PLAYER_VERSION" == "$VERSION" ]] \
    || fail "src/aes67-player/Cargo.toml version $PLAYER_VERSION does not match VERSION $VERSION"

if ! grep -Eq "^##[[:space:]]+\\[?$VERSION\\]?" CHANGELOG.md; then
    fail "CHANGELOG.md must contain a section for version $VERSION"
fi

echo "Release readiness check passed for $VERSION"
