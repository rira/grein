class Grein < Formula
  desc "Terminal workspace for branching AI sessions, git worktrees, and shells"
  homepage "https://github.com/rira/grein"
  url "https://github.com/rira/grein/archive/refs/tags/v0.1.2.tar.gz"
  sha256 "a95cd4073478f2cdbe88fe70495b0343add9dd96ba4ef176597ca353600ce7e3"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/grein-cli")
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/grein --version")
  end
end
