<#
.SYNOPSIS
    Fills the templates in this directory and writes the result to
    packaging/out/.

.DESCRIPTION
    This is render.sh, for PowerShell. The two do the same work and give the
    same files, so use the one that your shell likes. Neither needs the
    other, and neither needs WSL.

    The checksums come from the release itself. release.yml writes a file
    `<archive>.sha256` beside each archive, and this script reads those. The
    release must therefore exist before you run this.

.PARAMETER Version
    The version to render, with or without the leading `v`.

.PARAMETER ChecksumDir
    A directory that holds the `.sha256` files already. Give this to render
    from archives that you downloaded, or when the machine has no way out to
    the network. Without it, the script reads the files from the release.

.EXAMPLE
    .\packaging\render.ps1 0.1.0

.EXAMPLE
    .\packaging\render.ps1 0.1.0 -ChecksumDir .\downloads
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string] $Version,

    [string] $ChecksumDir
)

$ErrorActionPreference = 'Stop'

# Windows PowerShell 5.1 offers an old TLS by default, and GitHub refuses it.
# Without this line every download fails, and the message does not say why.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

# A tag carries `v`, a version does not. Take either.
$Version = $Version -replace '^v', ''

$repo = 'JohnGrey0/peruse'
$base = "https://github.com/$repo/releases/download/v$Version"
$here = $PSScriptRoot
$out = Join-Path $here 'out'

# Reads the checksum of one archive.
#
# The file holds the output of `sha256sum`: the checksum, then a space or
# two, then the name of the file. Take the first word.
function Get-Checksum {
    param([string] $Name)

    $body = $null
    if ($ChecksumDir) {
        $path = Join-Path $ChecksumDir "$Name.sha256"
        if (-not (Test-Path $path)) {
            throw "There is no $path."
        }
        $body = Get-Content $path -Raw
    }
    else {
        $url = "$base/$Name.sha256"
        try {
            $body = (Invoke-WebRequest -Uri $url -UseBasicParsing).Content
        }
        catch {
            Write-Host ''
            Write-Host "Could not read $url" -ForegroundColor Red
            Write-Host ''
            Write-Host "The release v$Version must exist and must hold $Name.sha256."
            Write-Host 'Push the tag first and wait for release.yml to finish:'
            Write-Host "    git tag v$Version; git push origin v$Version"
            throw "The release v$Version does not hold $Name.sha256."
        }
    }

    $first = ($body -split "`n")[0].Trim()
    return ($first -split '\s+')[0]
}

Write-Host "Reading the checksums of release v$Version ..."

$sha = [ordered] @{
    LINUX_X64   = Get-Checksum "peruse-$Version-x86_64-unknown-linux-gnu.tar.gz"
    MACOS_X64   = Get-Checksum "peruse-$Version-x86_64-apple-darwin.tar.gz"
    MACOS_ARM64 = Get-Checksum "peruse-$Version-aarch64-apple-darwin.tar.gz"
    WINDOWS_X64 = Get-Checksum "peruse-$Version-x86_64-pc-windows-msvc.zip"
}

# A checksum must be 64 characters of hexadecimal. Anything else means that
# the file held something other than a checksum, and a manifest that carries
# it would fail later, in a place that is harder to understand.
foreach ($name in $sha.Keys) {
    if ($sha[$name] -notmatch '^[0-9a-fA-F]{64}$') {
        throw "The checksum for $name is not 64 characters of hexadecimal: '$($sha[$name])'"
    }
}

$marks = @{
    '__VERSION__'               = $Version
    '__SHA_LINUX_X64__'         = $sha.LINUX_X64
    '__SHA_MACOS_X64__'         = $sha.MACOS_X64
    '__SHA_MACOS_ARM64__'       = $sha.MACOS_ARM64
    # WinGet asks for the checksum in capitals.
    '__SHA_WINDOWS_X64_UPPER__' = $sha.WINDOWS_X64.ToUpperInvariant()
    '__SHA_WINDOWS_X64__'       = $sha.WINDOWS_X64
    # WinGet asks for the date of the release, as year-month-day.
    '__DATE__'                  = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd')
}

# Writes one file, with every mark replaced.
#
# The file is read and written whole, and not line by line, so the line
# endings of the template stay as they are. It is written with no byte order
# mark: a mark at the start of a Ruby formula or a YAML manifest makes the
# tool that reads it fail.
function Write-Rendered {
    param([string] $Source, [string] $Target)

    $dir = Split-Path -Parent $Target
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }

    $text = [System.IO.File]::ReadAllText($Source)
    # The longer mark goes first, so a shorter one that starts the same way
    # cannot take part of it.
    foreach ($mark in ($marks.Keys | Sort-Object -Property Length -Descending)) {
        $text = $text.Replace($mark, $marks[$mark])
    }

    $utf8NoBom = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::WriteAllText($Target, $text, $utf8NoBom)
    Write-Host "  $Target"
}

if (Test-Path $out) {
    Remove-Item $out -Recurse -Force
}

Write-Host 'Writing:'
Write-Rendered (Join-Path $here 'scoop\peruse.json') (Join-Path $out 'scoop\peruse.json')
Write-Rendered (Join-Path $here 'homebrew\peruse.rb') (Join-Path $out 'homebrew\peruse.rb')
Write-Rendered (Join-Path $here 'chocolatey\peruse.nuspec') (Join-Path $out 'chocolatey\peruse.nuspec')
Write-Rendered (Join-Path $here 'chocolatey\tools\chocolateyinstall.ps1') (Join-Path $out 'chocolatey\tools\chocolateyinstall.ps1')
foreach ($f in Get-ChildItem (Join-Path $here 'winget') -Filter *.yaml) {
    Write-Rendered $f.FullName (Join-Path $out "winget\$($f.Name)")
}

# A mark that is still in a finished file means that this script and the
# templates do not agree. Say so now.
$left = Get-ChildItem $out -Recurse -File |
    Where-Object { (Get-Content $_.FullName -Raw) -match '__[A-Z_]+__' }
if ($left) {
    Write-Host ''
    Write-Host 'These files still hold a mark that was not replaced:' -ForegroundColor Red
    $left | ForEach-Object { Write-Host "  $($_.FullName)" }
    throw 'A template and this script do not agree.'
}

Write-Host ''
Write-Host 'Done. The finished files are in packaging/out/.'
Write-Host 'packaging/README.md says where each one goes.'
