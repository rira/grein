class Grein < Formula
  desc "Terminal workspace for branching AI sessions, git worktrees, and shells"
  homepage "https://github.com/rira/grein"
  url "https://github.com/rira/grein/archive/refs/tags/v0.1.3.tar.gz"
  sha256 "35f2fcceab50d5b78a7b9b20110a44a8be344d2e6b4468ccd8b43246e352254e"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/grein-cli")
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/grein --version")
  end
end
