# Formula for a tap. See packaging/README.md.
#
# The class name must match the file name: peruse.rb holds class Peruse.
class Peruse < Formula
  desc "Fast, read-only viewer for Parquet, CSV, TSV and JSON data files"
  homepage "https://github.com/JohnGrey0/peruse"
  version "__VERSION__"
  license "Apache-2.0"

  # The archives hold a program that is built already. Homebrew does not
  # compile DuckDB again, so the install takes seconds.
  on_macos do
    on_arm do
      url "https://github.com/JohnGrey0/peruse/releases/download/v__VERSION__/peruse-__VERSION__-aarch64-apple-darwin.tar.gz"
      sha256 "__SHA_MACOS_ARM64__"
    end
    on_intel do
      url "https://github.com/JohnGrey0/peruse/releases/download/v__VERSION__/peruse-__VERSION__-x86_64-apple-darwin.tar.gz"
      sha256 "__SHA_MACOS_X64__"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/JohnGrey0/peruse/releases/download/v__VERSION__/peruse-__VERSION__-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "__SHA_LINUX_X64__"
    end
  end

  def install
    bin.install "peruse"
    doc.install "README.md", "LICENSE", "THIRD-PARTY-LICENSES.md"
  end

  test do
    # The program must give its version and stop. This finds a build that
    # does not run at all, which is what a test of a binary formula is for.
    assert_match version.to_s, shell_output("#{bin}/peruse --version")
  end
end
