#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: bash scripts/update_homebrew_formula.sh <formula-path> <version> <checksum-file>

Updates the Apple Silicon macOS Homebrew formula metadata for aes67-tools.
USAGE
}

fail() {
    echo "$*" >&2
    exit 1
}

[[ $# -eq 3 ]] || {
    usage >&2
    exit 1
}

FORMULA="$1"
VERSION="$2"
CHECKSUM_FILE="$3"

[[ -f "$FORMULA" ]] || fail "Formula file not found: $FORMULA"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]] \
    || fail "Version must be valid SemVer, got '$VERSION'"
[[ -f "$CHECKSUM_FILE" ]] || fail "Checksum file not found: $CHECKSUM_FILE"

SHA256="$(awk '{ print $1; exit }' "$CHECKSUM_FILE")"
[[ "$SHA256" =~ ^[0-9a-fA-F]{64}$ ]] || fail "Checksum file does not start with a SHA256 hex digest"

export AES67_FORMULA_VERSION="$VERSION"
export AES67_FORMULA_SHA256="$SHA256"
export AES67_FORMULA_URL='https://github.com/wiccy46/aes67-tools/releases/download/v#{version}/aes67-tools-#{version}-aarch64-apple-darwin.tar.gz'

perl -0pi -e '
    my $version = $ENV{"AES67_FORMULA_VERSION"};
    my $url = $ENV{"AES67_FORMULA_URL"};
    my $sha = $ENV{"AES67_FORMULA_SHA256"};
    s/version\s+"[^"]+"/version "$version"/ or die "version line not found\n";
    s|url\s+"[^"]*aarch64-apple-darwin\.tar\.gz"|url "$url"| or die "Apple Silicon URL line not found\n";
    s/sha256\s+"[0-9a-fA-F]{64}"/sha256 "$sha"/ or die "sha256 line not found\n";
' "$FORMULA"

if command -v ruby >/dev/null 2>&1; then
    ruby -c "$FORMULA" >/dev/null
fi

echo "Updated $FORMULA to aes67-tools $VERSION"
