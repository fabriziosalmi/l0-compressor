# Homebrew formula for l0-compressor.
#
# Install (no separate tap repo needed — Homebrew accepts an explicit git URL):
#
#   brew tap fabriziosalmi/l0-compressor https://github.com/fabriziosalmi/l0-compressor
#   brew install l0-compressor
#
# Builds from source; Homebrew provides the Rust toolchain as a build-time dep.
# Besides the `l0-compressor` binary (and the `l0-comp` / `t` aliases), this also
# installs the integration helpers: `l0-compressor-claude-hook`,
# `l0-compressor-agent-hook`, `l0-compressor-agent-rules` (the repo's *.sh
# scripts, runnable without cloning).
class L0Compressor < Formula
  desc "CLI proxy that cuts LLM token use via universal output filtering"
  homepage "https://github.com/fabriziosalmi/l0-compressor"
  url "https://github.com/fabriziosalmi/l0-compressor/archive/refs/tags/v0.3.1.tar.gz"
  sha256 "de17195655493e88faa26ecfb24a2a50569eb0e0b08bd4b52bd65e46e6c0f864"
  license "MIT"
  head "https://github.com/fabriziosalmi/l0-compressor.git", branch: "master"

  depends_on "rust" => :build
  # Runtime: the bundled hook managers use jq to edit the agent's settings.json.
  depends_on "jq"

  def install
    system "cargo", "install", *std_cargo_args
    # The short `l0-comp` and `t` aliases, like the project's own installer creates.
    bin.install_symlink bin/"l0-compressor" => "l0-comp"
    bin.install_symlink bin/"l0-compressor" => "t"

    # Standalone integration setup scripts — they depend only on l0-compressor + jq
    # on PATH (no sibling files), so Homebrew users get the transparent-hook
    # tooling without cloning the repo. Exposed under namespaced commands.
    libexec.install "claude-hook.sh", "agent-hook.sh", "agent-rules.sh"
    bin.install_symlink libexec/"claude-hook.sh" => "l0-compressor-claude-hook"
    bin.install_symlink libexec/"agent-hook.sh"  => "l0-compressor-agent-hook"
    bin.install_symlink libexec/"agent-rules.sh" => "l0-compressor-agent-rules"
  end

  test do
    assert_match "l0-compressor", shell_output("#{bin}/l0-compressor --version")
    # End-to-end: filtering a large output truncates with the banner.
    out = shell_output("#{bin}/l0-compressor --no-auto --head 5 --tail 5 --threshold 10 seq 1 100")
    assert_match "Showing 5 head + 5 tail of 100 lines", out
    # The bundled Claude Code hook manager is installed and self-describes
    # (side-effect-free: `help` neither needs jq nor edits any settings).
    assert_match "Claude Code", shell_output("#{bin}/l0-compressor-claude-hook help")
  end
end
