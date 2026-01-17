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
      sha256 "d709301b7882ab56a5c73f42d3fd5f5fe1a8798ad52951090e066f2e9b594047"
    else
      url "https://github.com/vimo-ai/memex/releases/download/lite-v#{version}/memex-darwin-x64.tar.gz"
      sha256 "2ba81dee86ef11611d99e239987216522d695205db76eee3072216987852bc23"
    end
  end

  on_linux do
    url "https://github.com/vimo-ai/memex/releases/download/lite-v#{version}/memex-linux-x64.tar.gz"
    sha256 "86ec662c8676749faa4d0d031c8934341be3cce868212a93af5ac6738c79f1d4"
  end

  def install
    bin.install "memex"
  end

  test do
    assert_match "memex", shell_output("#{bin}/memex --version")
  end
end
