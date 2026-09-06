# Homebrew formula for qdrop. Put this in a tap
# (e.g. kkissdev/homebrew-tap) and `brew install kkissdev/tap/qdrop`.
class Qdrop < Formula
  desc "Peer-to-peer clipboard, file, and link bridge for your own devices"
  homepage "https://github.com/kkissdev/qdropd"
  url "https://github.com/kkissdev/qdropd/archive/refs/tags/v0.0.1.tar.gz"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  license "MIT"
  head "https://github.com/kkissdev/qdropd.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", "--locked", "--root", prefix, "--path", "crates/qdrop"
    system "cargo", "install", "--locked", "--root", prefix, "--path", "crates/qdropd"
    man1.install "packaging/man/qdrop.1" if File.exist?("packaging/man/qdrop.1")
    man1.install "packaging/man/qdropd.1" if File.exist?("packaging/man/qdropd.1")
  end

  service do
    run [opt_bin/"qdropd"]
    keep_alive true
    log_path "#{Dir.home}/Library/Logs/qdropd.log"
    error_log_path "#{Dir.home}/Library/Logs/qdropd.log"
  end

  test do
    assert_match "qdrop", shell_output("#{bin}/qdrop --version")
    assert_match "qdropd", shell_output("#{bin}/qdropd --version")
  end
end
