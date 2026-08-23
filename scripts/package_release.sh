#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: bash scripts/package_release.sh [--dry-run]

Builds and packages the aes67 application.

Environment overrides:
  AES67_RELEASE_OUTPUT_DIR     Output directory, default target/release-packages
  AES67_RELEASE_TARGET         Rust target triple, default current rustc host
  AES67_RELEASE_VERSION_FILE   Version file, default VERSION
  AES67_RELEASE_BINARY_DIR     Binary directory override, default Cargo release dir
  AES67_RELEASE_SKIP_BUILD     Set to 1 to package existing binaries without cargo build
USAGE
}

fail() {
    echo "$*" >&2
    exit 1
}

DRY_RUN=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run)
            DRY_RUN=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            fail "Unknown argument: $1"
            ;;
    esac
done

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

VERSION_FILE="${AES67_RELEASE_VERSION_FILE:-VERSION}"
[[ "$VERSION_FILE" = /* ]] || VERSION_FILE="$ROOT_DIR/$VERSION_FILE"
[[ -f "$VERSION_FILE" ]] || fail "Version file not found: $VERSION_FILE"

VERSION="$(tr -d '[:space:]' < "$VERSION_FILE")"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]]; then
    fail "Release version must be valid SemVer, got '$VERSION'"
fi

TARGET="${AES67_RELEASE_TARGET:-}"
if [[ -z "$TARGET" ]]; then
    TARGET="$(rustc -vV | awk '/^host:/ { print $2 }')"
fi
[[ -n "$TARGET" ]] || fail "Could not determine Rust target triple"

OUTPUT_DIR="${AES67_RELEASE_OUTPUT_DIR:-target/release-packages}"
[[ "$OUTPUT_DIR" = /* ]] || OUTPUT_DIR="$ROOT_DIR/$OUTPUT_DIR"

PACKAGE_NAME="aes67-tools-${VERSION}-${TARGET}"
STAGING_ROOT="$OUTPUT_DIR/staging"
PACKAGE_DIR="$STAGING_ROOT/$PACKAGE_NAME"
ARCHIVE="$OUTPUT_DIR/$PACKAGE_NAME.tar.gz"
CHECKSUM="$ARCHIVE.sha256"

if [[ -n "${AES67_RELEASE_BINARY_DIR:-}" ]]; then
    BINARY_DIR="$AES67_RELEASE_BINARY_DIR"
    [[ "$BINARY_DIR" = /* ]] || BINARY_DIR="$ROOT_DIR/$BINARY_DIR"
elif [[ -n "${AES67_RELEASE_TARGET:-}" ]]; then
    BINARY_DIR="$ROOT_DIR/target/$TARGET/release"
else
    BINARY_DIR="$ROOT_DIR/target/release"
fi

print_configuration() {
    cat <<CONFIG
Release package configuration
  Version:     $VERSION
  Target:      $TARGET
  Package:     $PACKAGE_NAME
  Output dir:  $OUTPUT_DIR
  Archive:     $ARCHIVE
  Checksum:    $CHECKSUM
  Binary dir:  $BINARY_DIR
  Contents:
    bin/aes67
    README.md
    LICENSE
    VERSION
    examples/send-file.toml
    examples/example.sdp
CONFIG
}

if [[ "$DRY_RUN" -eq 1 ]]; then
    print_configuration
    exit 0
fi

if [[ "${AES67_RELEASE_SKIP_BUILD:-0}" != "1" ]]; then
    cargo_args=(build --release -p aes67)
    if [[ -n "${AES67_RELEASE_TARGET:-}" ]]; then
        cargo_args+=(--target "$TARGET")
    fi
    cargo "${cargo_args[@]}"
fi

AES67_BIN="$BINARY_DIR/aes67"
[[ -x "$AES67_BIN" ]] || fail "Missing executable release binary: $AES67_BIN"

mkdir -p "$OUTPUT_DIR" "$STAGING_ROOT"
rm -rf "$PACKAGE_DIR" "$ARCHIVE" "$CHECKSUM"
mkdir -p "$PACKAGE_DIR/bin" "$PACKAGE_DIR/examples"

cp "$AES67_BIN" "$PACKAGE_DIR/bin/aes67"
cp "$ROOT_DIR/README.md" "$PACKAGE_DIR/README.md"
cp "$ROOT_DIR/LICENSE" "$PACKAGE_DIR/LICENSE"
cp "$ROOT_DIR/VERSION" "$PACKAGE_DIR/VERSION"
cp "$ROOT_DIR/tests/example.sdp" "$PACKAGE_DIR/examples/example.sdp"

cat > "$PACKAGE_DIR/examples/send-file.toml" <<'TOML'
[audio]
file = "audio.wav"
loop = false
duration_seconds = 30
gain_db = 0.0

[stream]
name = "AES67 Stream"
address = "239.69.83.1"
port = 5004
interface = "192.168.1.100"
sdp_output = "stream.sdp"
packet_time_ms = 1
payload_type = 97
ssrc = 305419896
ttl = 32
sap = true
ptp_domain = 0

[runtime]
verbose = false
TOML

tar -C "$STAGING_ROOT" -czf "$ARCHIVE" "$PACKAGE_NAME"

if command -v sha256sum >/dev/null 2>&1; then
    (cd "$OUTPUT_DIR" && sha256sum "$(basename "$ARCHIVE")" > "$(basename "$CHECKSUM")")
elif command -v shasum >/dev/null 2>&1; then
    (cd "$OUTPUT_DIR" && shasum -a 256 "$(basename "$ARCHIVE")" > "$(basename "$CHECKSUM")")
else
    fail "Neither sha256sum nor shasum is available for checksum generation"
fi

print_configuration
echo "Release package created"
