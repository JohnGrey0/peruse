# Packaging

A user who does not have Rust should still be able to install Peruse with the
tool that they use already. This directory holds the file that each of those
tools asks for.

| Tool | System | The file here |
|---|---|---|
| Scoop | Windows | `scoop/peruse.json` |
| WinGet | Windows | `winget/*.yaml` |
| Chocolatey | Windows | `chocolatey/` |
| Homebrew | macOS, Linux | `homebrew/peruse.rb` |

Each file is a **template**. It holds marks of the form `__VERSION__` and
`__SHA_LINUX_X64__`. The marks are not valid input for the tools. Render them
first.

## Render

The commands below read a release of the project, take the checksum of each
archive, and write the finished files to `packaging/out/`.

There is one for each shell. They do the same work and give the same files,
so use the one that your shell likes. Neither needs the other, and neither
needs WSL.

```powershell
.\packaging\render.ps1 0.1.0
```

```sh
./packaging/render.sh 0.1.0
```

`render.ps1` runs on Windows PowerShell 5.1 and needs nothing else.
`render.sh` needs `curl`, and runs on Git Bash as well as on Linux and macOS.

The release must exist first, because the checksums come from the files on
it. Make the release before you render:

```sh
git tag v0.1.0 && git push origin v0.1.0     # release.yml builds and uploads
./packaging/render.sh 0.1.0                  # then render
```

To render from archives that you downloaded, or on a machine with no way out
to the network, give the directory that holds the `.sha256` files:

```powershell
.\packaging\render.ps1 0.1.0 -ChecksumDir .\downloads
```

## Publish

Each tool wants the finished file in a different place. None of them takes a
file from this repository, so each one is a step by hand, or a pull request.

### Scoop

The quickest one. A bucket is a repository of JSON files:

1. Make a repository with the name `scoop-peruse`.
2. Put `out/scoop/peruse.json` in a directory `bucket/`.
3. A user then runs:

```powershell
scoop bucket add peruse https://github.com/JohnGrey0/scoop-peruse
scoop install peruse
```

The template holds `checkver` and `autoupdate`, so the bucket keeps itself up
to date after a new release. It needs no work for the release after this one.

### Homebrew

A tap is a repository of Ruby files:

1. Make a repository with the name `homebrew-peruse`.
2. Put `out/homebrew/peruse.rb` in a directory `Formula/`.
3. A user then runs:

```sh
brew tap JohnGrey0/peruse
brew install peruse
```

The main Homebrew repository asks a project for 30 stars, 30 forks or 75
watchers before it accepts a formula. A tap has no such condition, and it
works the same way for the user.

### WinGet

WinGet takes a pull request to one repository:

1. Fork `microsoft/winget-pkgs`.
2. Copy the three files from `out/winget/` to
   `manifests/j/JohnGrey0/Peruse/<version>/`.
3. Test them first. This step finds most faults:

```powershell
winget validate --manifest manifests/j/JohnGrey0/Peruse/0.1.0
winget install --manifest manifests/j/JohnGrey0/Peruse/0.1.0
```

4. Open the pull request. A robot reviews it, and a person then accepts it.

The first submission takes some days. Use `wingetcreate update` for the ones
after it.

### Chocolatey

Chocolatey takes a package, and not a pull request:

```powershell
cd out/chocolatey
choco pack
choco apikey --key <your-key> --source https://push.chocolatey.org/
choco push peruse.<version>.nupkg --source https://push.chocolatey.org/
```

A person reviews the first version. Chocolatey asks a package to check the
file that it downloads, and `chocolateyinstall.ps1` holds a checksum for that
reason.

## The order to do this in

Do the quick ones first. Scoop and Homebrew need a repository of your own and
no review, so a user can install Peruse on the same day. WinGet and Chocolatey
both wait for a person, so start them and do not wait.

## After you change the archives

Both scripts know the names that `release.yml` writes. If you change a name, a
target or the layout inside an archive, change **both** with it. The templates
name the directory inside the archive, and an archive that holds a different
directory makes every one of these tools fail.

To confirm that the two still agree, render with each one and compare:

```sh
diff -r out-from-powershell out-from-sh
```
