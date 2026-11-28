class Grein < Formula
  desc "Terminal workspace for branching AI sessions, git worktrees, and shells"
  homepage "https://github.com/rira/grein"
  url "https://github.com/rira/grein/archive/refs/tags/v0.2.0.tar.gz"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/grein-cli")
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/grein --version")
  end
end
