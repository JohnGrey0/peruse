# Chocolatey downloads the archive of the release and opens it here.
#
# Chocolatey asks a package to check what it downloads. The checksum below is
# the one that `release.yml` wrote beside the archive, so a file that changed
# on the way stops the install.
#
# `Install-ChocolateyZipPackage` puts the files in the directory of the
# package. Chocolatey then finds `peruse.exe` inside it and makes a shim on
# the path, so the user types `peruse` and nothing else.
$ErrorActionPreference = 'Stop'

$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition

$packageArgs = @{
  packageName    = 'peruse'
  unzipLocation  = $toolsDir
  url64bit       = 'https://github.com/JohnGrey0/peruse/releases/download/v__VERSION__/peruse-__VERSION__-x86_64-pc-windows-msvc.zip'
  checksum64     = '__SHA_WINDOWS_X64__'
  checksumType64 = 'sha256'
}

Install-ChocolateyZipPackage @packageArgs
