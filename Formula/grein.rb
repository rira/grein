class Grein < Formula
  desc "Terminal workspace for branching AI sessions, git worktrees, and shells"
  homepage "https://github.com/rira/grein"
  url "https://github.com/rira/grein/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "ed4275756f15e7b6c3fc7bea9b1c2eb98ef625403a00fee8d58c995e51535754"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/grein-cli")
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/grein --version")
  end
end
