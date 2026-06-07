#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: bash scripts/update_homebrew_formula.sh <formula-path> <version> <macos-arm64-checksum-file> <linux-x86_64-checksum-file>

Updates the Homebrew formula metadata for supported aes67-tools release targets.
USAGE
}

fail() {
    echo "$*" >&2
    exit 1
}

[[ $# -eq 4 ]] || {
    usage >&2
    exit 1
}

FORMULA="$1"
VERSION="$2"
MACOS_ARM64_CHECKSUM_FILE="$3"
LINUX_X86_64_CHECKSUM_FILE="$4"

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]] \
    || fail "Version must be valid SemVer, got '$VERSION'"
[[ -f "$MACOS_ARM64_CHECKSUM_FILE" ]] || fail "macOS ARM64 checksum file not found: $MACOS_ARM64_CHECKSUM_FILE"
[[ -f "$LINUX_X86_64_CHECKSUM_FILE" ]] || fail "Linux x86_64 checksum file not found: $LINUX_X86_64_CHECKSUM_FILE"

read_sha256() {
    local checksum_file="$1"
    local sha256
    sha256="$(awk '{ print $1; exit }' "$checksum_file")"
    [[ "$sha256" =~ ^[0-9a-fA-F]{64}$ ]] || fail "Checksum file does not start with a SHA256 hex digest: $checksum_file"
    echo "$sha256"
}

MACOS_ARM64_SHA256="$(read_sha256 "$MACOS_ARM64_CHECKSUM_FILE")"
LINUX_X86_64_SHA256="$(read_sha256 "$LINUX_X86_64_CHECKSUM_FILE")"

mkdir -p "$(dirname "$FORMULA")"
cat > "$FORMULA" <<RUBY
class Aes67Tools < Formula
  desc "AES67-oriented RTP audio streamer and player"
  homepage "https://github.com/wiccy46/aes67-tools"
  version "$VERSION"
  license "GPL-3.0-only"

  on_macos do
    depends_on arch: :arm64

    url "https://github.com/wiccy46/aes67-tools/releases/download/v#{version}/aes67-tools-#{version}-aarch64-apple-darwin.tar.gz"
    sha256 "$MACOS_ARM64_SHA256"
  end

  on_linux do
    url "https://github.com/wiccy46/aes67-tools/releases/download/v#{version}/aes67-tools-#{version}-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "$LINUX_X86_64_SHA256"
  end

  def install
    bin.install "bin/aes67-streamer"
    bin.install "bin/aes67-player"
    doc.install "README.md"
    doc.install "LICENSE"
    pkgshare.install "examples"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/aes67-streamer --version")
    assert_match version.to_s, shell_output("#{bin}/aes67-player --version")
  end
end
RUBY

if command -v ruby >/dev/null 2>&1; then
    ruby -c "$FORMULA" >/dev/null
fi

echo "Updated $FORMULA to aes67-tools $VERSION"
