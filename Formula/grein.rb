class Grein < Formula
  desc "Autonomous development harness and workspace CLI for intelligent coding pipelines"
  homepage "https://github.com/rira/grein"
  url "https://github.com/rira/grein/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  license "Apache-2.0"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/grein-cli")
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/grein --version")
  end
end