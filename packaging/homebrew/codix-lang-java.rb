# The Java plugin of codix: its grammar as a shared library, its descriptor
# and its queries. An example, not a published formula.
class CodixLangJava < Formula
  desc "Java support for codix"
  homepage "https://github.com/imgn-dev/codix"
  url "https://github.com/imgn-dev/codix/archive/refs/tags/v0.4.0.tar.gz"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  license "Apache-2.0"

  depends_on "rust" => :build
  depends_on "codix"

  def install
    system "cargo", "xtask", "plugins", buildpath/"plugins", "--only", "java"
    (lib/"codix/plugins/language").install buildpath/"plugins/language/java"
  end

  test do
    (testpath/"A.java").write "class A { void run() {} }\n"
    system Formula["codix"].opt_bin/"codix", "init"
    assert_match "A.run", shell_output("#{Formula["codix"].opt_bin}/codix find run")
  end
end
