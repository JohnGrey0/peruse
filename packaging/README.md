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

## The whole release, in one command

`release.ps1` and `release.sh` do a whole release, in the right order, and
stop at the first thing that is wrong. Rendering is step 6 of them, so on the
day of a release you run one command and not seven.

```powershell
.\packaging\release.ps1 0.2.0            # look; this changes nothing
.\packaging\release.ps1 0.2.0 -Execute   # do it
```

```sh
./packaging/release.sh 0.2.0             # look; this changes nothing
./packaging/release.sh 0.2.0 --execute   # do it
```

Without the flag the script is a **dry run**. It reads the repository, the
release and crates.io, prints each step that a real run would take, and
changes not one file. With the flag it prints the same plan and then asks you
to type the tag, because a tag on the remote, a release and a version on
crates.io can none of them be taken back.

The steps:

| # | Step | What it does |
|---|---|---|
| 1 | `preflight` | The tree is clean, the branch is main, main is level with `origin/main`, the tag is free, `CHANGELOG.md` holds a section for the version, the version is higher than the one on crates.io, the tests pass, clippy is clean, the format is clean, and both crates package. |
| 2 | `bump` | The version goes into `Cargo.toml`, into the `peruse-core` line of `crates/peruse-tui/Cargo.toml`, and into `Cargo.lock`. |
| 3 | `tag` | Commit the bump, tag `vX.Y.Z`, push main, then push the tag. |
| 4 | `workflow` | Wait for `release.yml`. It builds four platforms and compiles DuckDB for each, so 30 to 60 minutes is normal. |
| 5 | `verify` | The release holds four archives and four `.sha256` files. |
| 6 | `render` | Run `render.ps1` or `render.sh`, which fills the manifests in this directory. |
| 7 | `publish` | `peruse-core` first, then `peruse-tui`. |
| 8 | `handoff` | Say which rendered file goes to which package repository, below. |

Step 7 keeps that order because `crates/peruse-tui/Cargo.toml` names
`peruse-core` by version and not with `version.workspace = true`. The front
end cannot resolve its own dependency until crates.io has indexed the engine,
so the script waits for the index between the two, and step 2 writes that one
line as well. A bump that missed it would publish a front end that asks for
an old engine.

The flags have the same names in both scripts:

| PowerShell | Shell | What it means |
|---|---|---|
| `-Execute` | `--execute` | Do the work. Without it, the script is a dry run. |
| `-SkipTests` | `--skip-tests` | Leave out the long checks of the preflight, for a second run a few minutes later. The quick checks still run. |
| `-From <step>` | `--from <step>` | Start at a step, for a release that stopped part way. |
| `-NoPublish` | `--no-publish` | Stop before crates.io. |
| `-Help` | `--help` | The help, with a worked example. |

Each step looks first to see whether it is done already, and says "already
done, going on". A release that stopped at step 4 therefore takes up where it
left off:

```sh
./packaging/release.sh 0.2.0 --execute --from workflow --skip-tests
```

The crates.io token comes from `CARGO_REGISTRY_TOKEN`, or from the file that
`cargo login` wrote. It is never an argument, where the history of the shell
would keep it.

Two things to know before the first release:

- `gh` makes step 4 better, and is not necessary. Without it the script waits
  for the eight files to appear on the release, but it cannot show you the log
  of a build that failed.
- The preflight asks for `cargo fmt --all -- --check` to be clean. No job in
  CI checks the format, so the tree can be some way from it, as it was when
  these scripts arrived. Run `cargo fmt --all` once and commit it, or give
  `-SkipTests`.

**The two scripts must stay in step.** They do the same steps, in the same
order, with the same flag names, and each holds the list of steps in one
comment block at the top. A change to one is a change to both.

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
it. `release.sh` sees to that, and does the two steps in the right order. By
hand, it is:

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

Four files know the names that `release.yml` writes: `render.ps1`,
`render.sh`, `release.ps1` and `release.sh`. The render scripts read the
`.sha256` file of each archive, and step 5 of the release scripts counts the
eight files on the release. If you change a name, a target or the layout
inside an archive, change **all four** with it. The templates name the
directory inside the archive, and an archive that holds a different directory
makes every one of these tools fail.

To confirm that the two render scripts still agree, render with each one and
compare:

```sh
diff -r out-from-powershell out-from-sh
```
