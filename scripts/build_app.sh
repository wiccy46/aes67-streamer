#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: ./scripts/build_app.sh [debug|release]

Build the AES67 desktop application and its platform-native bundles.
The build mode defaults to release.

Examples:
  ./scripts/build_app.sh
  ./scripts/build_app.sh debug
  ./scripts/build_app.sh release
USAGE
}

fail() {
    echo "Error: $*" >&2
    exit 1
}

if [[ "${1:-}" = "-h" || "${1:-}" = "--help" ]]; then
    usage
    exit 0
fi

[[ $# -le 1 ]] || fail "Expected one build mode: debug or release"

BUILD_MODE="${1:-release}"
case "$BUILD_MODE" in
    debug|release)
        ;;
    *)
        fail "Unknown build mode '$BUILD_MODE'; expected debug or release"
        ;;
esac

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DESKTOP_DIR="$ROOT_DIR/apps/aes67-desktop"

command -v cargo >/dev/null 2>&1 || fail "cargo is required to build the native application"
command -v node >/dev/null 2>&1 || fail "Node.js is required to build the frontend"
command -v npm >/dev/null 2>&1 || fail "npm is required to install and build frontend dependencies"

[[ -f "$DESKTOP_DIR/package.json" ]] || fail "Desktop package not found at $DESKTOP_DIR"
[[ -f "$DESKTOP_DIR/package-lock.json" ]] || fail "Desktop package lock is missing"

if [[ ! -x "$DESKTOP_DIR/node_modules/.bin/tauri" ]]; then
    echo "Installing locked desktop dependencies..."
    npm --prefix "$DESKTOP_DIR" ci
fi

echo "Building AES67 desktop application ($BUILD_MODE)..."
if [[ "$BUILD_MODE" = "debug" ]]; then
    npm --prefix "$DESKTOP_DIR" run desktop:build -- --debug
else
    npm --prefix "$DESKTOP_DIR" run desktop:build
fi

echo "Application bundle build complete."
