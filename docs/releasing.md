# How to make a release

Peruse goes out in three forms. A user chooses the one that fits the machine
that they have.

| Form | The command | Who it is for |
|---|---|---|
| crates.io | `cargo install peruse-tui` | A user with Rust, who accepts a build of some minutes |
| A release file | Download and run | A user with no Rust |
| binstall | `cargo binstall peruse-tui` | A user with Rust, who wants the release file |

The command is `peruse` in each case. The crate is `peruse-tui` because the
name `peruse` on crates.io belongs to another project, a parser library. A
crate and its program do not need the same name: `ripgrep` gives `rg`.

## The two crates, and the order

The program is two crates, and the order matters:

1. `peruse-core` — the library. It must go first.
2. `peruse-tui` — the program. It names `peruse-core` by version, and that
   version is on crates.io only after the first step.

A check of the second crate before the first one fails with "no matching
package named `peruse-core`". That message is correct, and it is not a fault.

## Before a release

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo package -p peruse-core --no-verify
```

Then look at the two things that a reader of crates.io sees first: the
description and the README of each crate.

## The steps

1. Set the version in `Cargo.toml`, in `[workspace.package]`. The two crates
   read it from there.
2. Set the same version in the dependency line of `crates/peruse-tui/Cargo.toml`:
   `peruse-core = { path = "../peruse-core", version = "X.Y.Z" }`.
3. Write the changes in `CHANGELOG.md`.
4. Commit, and make the tag: `git tag vX.Y.Z && git push --tags`.
5. Publish the library, and then the program:

   ```sh
   cargo publish -p peruse-core
   cargo publish -p peruse-tui
   ```

   The second command needs the first crate to be on the index. That takes a
   few seconds, and sometimes a minute.

The tag starts the workflow `.github/workflows/release.yml`. That workflow
builds the program for Linux, for macOS on the two processors, and for
Windows, and it puts each archive and its checksum on the release.

## A publish cannot go back

crates.io keeps each version for ever. A version can be yanked, which stops a
new project from taking it, but the files stay. The name of a crate also stays
with its first publisher. Read the manifest one more time before the command.

## The name of a release file

The workflow and the settings for binstall must agree on the names. They are:

```
peruse-{version}-{target}.tar.gz     Linux and macOS
peruse-{version}-{target}.zip        Windows
```

Each archive holds one directory of the same name, and that directory holds
the program, the README and the licence. A user who opens the archive in a
download folder therefore gets one directory, and not three loose files.

The settings for binstall are in `crates/peruse-tui/Cargo.toml`, under
`[package.metadata.binstall]`. A change to the names in the workflow needs the
same change there.

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
