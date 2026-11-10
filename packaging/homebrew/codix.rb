# The codix core, modular flavour: git compiled in, every other provider a
# plugin formula. An example, not a published formula.
class Codix < Formula
  desc "Headless code index for LLM agents"
  homepage "https://github.com/imgn-dev/codix"
  url "https://github.com/imgn-dev/codix/archive/refs/tags/v0.4.0.tar.gz"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  license "Apache-2.0"

  depends_on "rust" => :build

  def install
    ENV["CODIX_INSTALL_HINT"] = "brew install codix-{kind}-{name}"
    system "cargo", "build", "--release", "--locked", "--package", "codix-cli",
           "--no-default-features", "--features", "plugins,vcs-git"
    bin.install "target/release/codix"
  end

  test do
    system bin/"codix", "--version"
  end
end
