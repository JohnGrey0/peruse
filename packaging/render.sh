#!/usr/bin/env sh
#
# Fills the templates in this directory and writes the result to
# packaging/out/.
#
# Usage:
#   ./packaging/render.sh 0.1.0
#
# The checksums come from the release itself. `release.yml` writes a file
# `<archive>.sha256` beside each archive, and this script reads those. The
# release must therefore exist before you run this.
#
# The script needs `curl` and nothing else. It runs on any shell that follows
# POSIX, and on Git Bash on Windows.
set -eu

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
    echo "usage: $0 <version>            for example: $0 0.1.0" >&2
    exit 2
fi

# A tag carries `v`, a version does not. Take either and use the right one in
# each place.
VERSION="${VERSION#v}"

REPO="JohnGrey0/peruse"
BASE="https://github.com/$REPO/releases/download/v$VERSION"
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
OUT="$HERE/out"

# Reads the checksum of one archive from the release.
#
# The file holds the output of `sha256sum`, which is the checksum, then a
# space or two, then the name of the file. Take the first word.
checksum() {
    name="$1"
    url="$BASE/$name.sha256"
    body=$(curl -fsSL "$url" 2>/dev/null) || {
        echo "" >&2
        echo "Could not read $url" >&2
        echo "" >&2
        echo "The release v$VERSION must exist and must hold $name.sha256." >&2
        echo "Push the tag first and wait for release.yml to finish:" >&2
        echo "    git tag v$VERSION && git push origin v$VERSION" >&2
        exit 1
    }
    # The first field of the first line.
    echo "$body" | head -n 1 | tr -s ' ' | cut -d' ' -f1 | tr -d '\r\n'
}

echo "Reading the checksums of release v$VERSION ..."
SHA_LINUX_X64=$(checksum "peruse-$VERSION-x86_64-unknown-linux-gnu.tar.gz")
SHA_MACOS_X64=$(checksum "peruse-$VERSION-x86_64-apple-darwin.tar.gz")
SHA_MACOS_ARM64=$(checksum "peruse-$VERSION-aarch64-apple-darwin.tar.gz")
SHA_WINDOWS_X64=$(checksum "peruse-$VERSION-x86_64-pc-windows-msvc.zip")

# A checksum must be 64 characters of hexadecimal. Anything else means that
# the file held something other than a checksum, and a manifest that carries
# it would fail later, in a place that is harder to understand.
for pair in \
    "linux:$SHA_LINUX_X64" \
    "macos-x64:$SHA_MACOS_X64" \
    "macos-arm64:$SHA_MACOS_ARM64" \
    "windows:$SHA_WINDOWS_X64"; do
    name=${pair%%:*}
    value=${pair#*:}
    if [ "${#value}" -ne 64 ]; then
        echo "The checksum for $name is not 64 characters: '$value'" >&2
        exit 1
    fi
done

# WinGet asks for the checksum in capitals.
SHA_WINDOWS_X64_UPPER=$(echo "$SHA_WINDOWS_X64" | tr 'a-f' 'A-F')

# WinGet asks for the date of the release, as year-month-day.
DATE=$(date -u +%Y-%m-%d)

# Writes one file, with every mark replaced.
render() {
    src="$1"
    dst="$2"
    mkdir -p "$(dirname "$dst")"
    sed \
        -e "s|__VERSION__|$VERSION|g" \
        -e "s|__SHA_LINUX_X64__|$SHA_LINUX_X64|g" \
        -e "s|__SHA_MACOS_X64__|$SHA_MACOS_X64|g" \
        -e "s|__SHA_MACOS_ARM64__|$SHA_MACOS_ARM64|g" \
        -e "s|__SHA_WINDOWS_X64_UPPER__|$SHA_WINDOWS_X64_UPPER|g" \
        -e "s|__SHA_WINDOWS_X64__|$SHA_WINDOWS_X64|g" \
        -e "s|__DATE__|$DATE|g" \
        "$src" > "$dst"
    echo "  $dst"
}

rm -rf "$OUT"
echo "Writing:"
render "$HERE/scoop/peruse.json" "$OUT/scoop/peruse.json"
render "$HERE/homebrew/peruse.rb" "$OUT/homebrew/peruse.rb"
render "$HERE/chocolatey/peruse.nuspec" "$OUT/chocolatey/peruse.nuspec"
render "$HERE/chocolatey/tools/chocolateyinstall.ps1" "$OUT/chocolatey/tools/chocolateyinstall.ps1"
for f in "$HERE"/winget/*.yaml; do
    render "$f" "$OUT/winget/$(basename "$f")"
done

# A mark that is still in a finished file means that this script and the
# templates do not agree. Say so now.
if grep -rl '__[A-Z_]*__' "$OUT" 2>/dev/null | grep -q .; then
    echo "" >&2
    echo "These files still hold a mark that was not replaced:" >&2
    grep -rl '__[A-Z_]*__' "$OUT" >&2
    exit 1
fi

echo ""
echo "Done. The finished files are in packaging/out/."
echo "packaging/README.md says where each one goes."
