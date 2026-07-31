# How to make a release

Peruse goes out in five forms. A user chooses the one that fits the machine
that they have.

| Form | The command | Who it is for |
|---|---|---|
| crates.io | `cargo install peruse-tui` | A user with Rust, who accepts a build of some minutes |
| A release file | Download and run | A user with no Rust |
| binstall | `cargo binstall peruse-tui` | A user with Rust, who wants the release file |
| Scoop, WinGet, Chocolatey | `scoop install peruse` and the rest | A user on Windows |
| Homebrew | `brew install peruse` | A user on macOS or Linux |

The command is `peruse` in each case. The crate is `peruse-tui` because the
name `peruse` on crates.io belongs to another project, a parser library. A
crate and its program do not need the same name: `ripgrep` gives `rg`.

## One command

The scripts `packaging/release.ps1` and `packaging/release.sh` do a whole
release, in the right order, and they stop at the first thing that is wrong.
The two do the same eight steps with the same flags, so use the one that your
shell likes.

```powershell
.\packaging\release.ps1 0.2.0            # look; this changes nothing
.\packaging\release.ps1 0.2.0 -Execute   # do it
```

```sh
./packaging/release.sh 0.2.0             # look; this changes nothing
./packaging/release.sh 0.2.0 --execute   # do it
```

Without the flag the script is a dry run. It reads the repository, the release
and crates.io, prints each step that a real run would take, and changes not one
file. With the flag it prints the same plan and then asks a person to type the
tag, because a tag on the remote, a release and a version on crates.io can none
of them be taken back.

The eight steps are `preflight`, `bump`, `tag`, `workflow`, `verify`, `render`,
`publish` and `handoff`. Each step looks first to see whether it is done
already, so a release that stopped part way takes up where it left off:

```sh
./packaging/release.sh 0.2.0 --execute --from workflow --skip-tests
```

The document [`packaging/README.md`](../packaging/README.md) describes each
step, each flag and the work that a person still does after step 8. The steps
below say what the script does, so a person can do the same work by hand.

## The two crates, and the order

The program is two crates, and the order matters:

1. `peruse-core`: the library. It must go first.
2. `peruse-tui`: the program. It names `peruse-core` by version, and that
   version is on crates.io only after the first step.

A check of the second crate before the first one fails with "no matching
package named `peruse-core`". That message is correct, and it is not a fault.
The release script waits for the index of crates.io between the two.

## Before a release

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo package -p peruse-core --no-verify
```

These are the checks that `ci.yml` runs, and the release script runs the same
ones. There is no check of the format among them: this project writes its code by
hand, it holds no rustfmt settings, and `cargo fmt --check` reports 259 places
that no recent change touched.

Then look at the two things that a reader of crates.io sees first: the
description and the README of each crate.

## The steps

1. Set the version in `Cargo.toml`, in `[workspace.package]`. The two crates
   read it from there.
2. Set the same version in the dependency line of `crates/peruse-tui/Cargo.toml`:
   `peruse-core = { path = "../peruse-core", version = "X.Y.Z" }`.
3. Write the changes in `CHANGELOG.md`.
4. Commit, and make the tag: `git tag -a vX.Y.Z && git push origin main` and
   then `git push origin refs/tags/vX.Y.Z`. Push the branch before the tag.
5. Wait for the workflow, and check that the release holds its eight files.
6. Fill the package manifests: `./packaging/render.sh X.Y.Z`. The checksums
   come from the files on the release, so this step needs step 5.
7. Publish the library, and then the program:

   ```sh
   cargo publish -p peruse-core --locked
   cargo publish -p peruse-tui --locked
   ```

   The second command needs the first crate to be on the index. That takes a
   few seconds, and sometimes a minute.
8. Take each rendered file to its package repository. See
   [`packaging/README.md`](../packaging/README.md).

The tag starts the workflow `.github/workflows/release.yml`. That workflow
builds the program for Linux, for macOS on the two processors, and for
Windows, and it puts each archive and its checksum on the release.

## A publish cannot go back

crates.io keeps each version for ever. An author can yank a version, which stops
a new project from taking it, but the files stay. The name of a crate also stays
with its first publisher. Read the manifest one more time before the command.

A tag on the remote and a GitHub release are the same kind of decision. The
release script therefore asks a person to type the tag before step 2, and it
stops rather than move a tag that is at another commit.

## The name of a release file

The workflow, the settings for binstall and the four scripts in `packaging/`
must agree on the names. They are:

```
peruse-{version}-{target}.tar.gz     Linux and macOS
peruse-{version}-{target}.zip        Windows
```

The four targets are `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin`,
`aarch64-apple-darwin` and `x86_64-pc-windows-msvc`. Each archive holds one
directory of the same name, and that directory holds the program, the README,
the licence and the third party licences. A user who opens the archive in a
download folder therefore gets one directory, and not four loose files.

The settings for binstall are in `crates/peruse-tui/Cargo.toml`, under
`[package.metadata.binstall]`. A change to the names in the workflow needs the
same change there, and in `render.ps1`, `render.sh`, `release.ps1` and
`release.sh`.

## What a security team asks

The project answers these questions in the open, because a user who wants to
run Peruse at work has to answer them for somebody else:

- **Who wrote it?** The repository, in the open, with the history.
- **What does it depend on?** `Cargo.lock` holds each version. The large one
  is DuckDB, which the build compiles from source into the program.
- **Does it write to my data?** No. See
  [read-only-guard.md](read-only-guard.md).
- **Does it reach the network?** No. The engine turns off the two settings of
  DuckDB that would download an extension, and a test proves it.
- **Did the file change on the way?** Each release file has a `.sha256`
  beside it.

The workflows use actions from `actions/*` only, and the command line tool
that the runner holds already. A review of the pipeline therefore has one
publisher to check.
