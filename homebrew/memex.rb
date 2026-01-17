# Homebrew Formula for Memex
#
# To publish:
# 1. Create repo: github.com/vimo-ai/homebrew-tap
# 2. Copy this file to: Formula/memex.rb
# 3. Update version and sha256 after each release
#
# Usage: brew install vimo-ai/tap/memex

class Memex < Formula
  desc "Zero-dependency CLI for searching AI CLI session history"
  homepage "https://github.com/vimo-ai/memex"
  version "0.0.1-beta.1"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/vimo-ai/memex/releases/download/lite-v#{version}/memex-darwin-arm64.tar.gz"
      sha256 "82bc41fe441ebdab4e7e38c19aea652c961c8d3227fc71ef22543b6c51bdb05a"
    else
      url "https://github.com/vimo-ai/memex/releases/download/lite-v#{version}/memex-darwin-x64.tar.gz"
      sha256 "cc85c4181643dfe28f716250446f51459fcf1c34911075aebbdb11d2cfbc569d"
    end
  end

  on_linux do
    url "https://github.com/vimo-ai/memex/releases/download/lite-v#{version}/memex-linux-x64.tar.gz"
    sha256 "1060fcc6fba46de267de650c93abd1a4d3367ca9721d937885cdbeaf3ef3f76b"
  end

  def install
    bin.install "memex"
  end

  test do
    assert_match "memex", shell_output("#{bin}/memex --version")
  end
end
