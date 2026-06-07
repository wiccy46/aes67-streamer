class Aes67Tools < Formula
  desc "AES67-oriented RTP audio streamer and player"
  homepage "https://github.com/wiccy46/aes67-tools"
  version "REPLACE_WITH_VERSION"
  license "GPL-3.0-only"

  on_macos do
    depends_on arch: :arm64

    url "https://github.com/wiccy46/aes67-tools/releases/download/v#{version}/aes67-tools-#{version}-aarch64-apple-darwin.tar.gz"
    sha256 "REPLACE_WITH_AARCH64_APPLE_DARWIN_SHA256"
  end

  on_linux do
    url "https://github.com/wiccy46/aes67-tools/releases/download/v#{version}/aes67-tools-#{version}-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "REPLACE_WITH_X86_64_UNKNOWN_LINUX_GNU_SHA256"
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
