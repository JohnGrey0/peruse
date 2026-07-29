# The chooser of files

A call with no file opens a screen that lists the data files that are near. The
code is in `crates/peruse-tui/src/browser.rs`.

## Why the screen exists

A user who types `peruse` alone wants to look at some data. The name of the
program is the one thing that such a user remembers, and a page of help is not
an answer to that wish. The user then has to leave the program, find the path
of a file, and type it.

The screen removes those steps. It shows the data files of the directory, the
files that the user opened before, and a way to each other directory.

## The shape of the code

The chooser holds no engine and no worker. It reads a directory with the
functions of the standard library, and it gives one path back:

```rust
pub fn choose(config: &Config, theme: &Theme) -> Result<Option<PathBuf>>
```

The program then opens that path exactly as it opens a path from the command
line. The chooser therefore knows nothing about DuckDB, and the rest of the
program knows nothing about the chooser. The two meet at one path.

The function starts the terminal and stops it again. The main loop of the
program starts a second terminal after it, and that is correct: the two screens
never run at the same time.

## When the screen opens

| The call | What happens |
|---|---|
| `peruse` in a terminal | The chooser opens. |
| `peruse` in a pipeline | Peruse prints the help. |
| `peruse --ddl postgres` | Peruse gives an error and asks for a file. |
| `peruse --help` | Peruse prints the help. |

A terminal that is not there cannot hold a chooser. A call inside a pipeline
must not wait for a key, so `std::io::IsTerminal` decides. The option `--ddl`
writes to the standard output for a script, and a script names its own file.

## What the list holds

| Part | Rule |
|---|---|
| The files that the user opened | At the top, and only when the find box is empty. |
| `..` | One line, so a user needs no key for the directory above. |
| The directories | Before the files, in the order of the alphabet. |
| The data files | After the directories, in the order of the alphabet. |

A file with an extension that Peruse knows is a data file. The function
`source::by_extension` gives the format. It reads the name only and never opens
the file, because a directory can hold thousands of them.

Two rules keep the list short:

- A name that starts with a full stop is not in the list. A viewer of data has
  no use for the files that the system holds for itself.
- A file that Peruse cannot read is not in the list. The key `a` shows each
  file, because a file named `data.dat` can still hold CSV.

Each file shows its format, its size and the time of its last change. A user
who looks for "the file from this morning" then needs no other program.

## The files that the user opened

The setting `recent` holds the paths, the newest first. `Config::remember_file`
puts a path at the top and removes it from its old place, so the list holds no
name two times. The list stops at eight names, because a longer one would push
the directory off the screen.

Peruse writes the list after it opens a file, and not before. A path that
cannot open therefore never joins the list.

A file that the user moved or deleted is not on the screen. The list would
offer a file that cannot open, and the user would see an error for a choice
that looked correct.

Each entry shows the directory beside the name. The name by itself does not say
which `data.csv` it is.

## The keys

| Key | Action |
|---|---|
| `j` `k`, `↑` `↓` | Move |
| `PgUp` `PgDn`, `g` `G` | Move by ten, or to the ends |
| `Enter`, `l`, `→` | Open a file, or go into a directory |
| `h`, `←`, `Backspace` | Go to the directory above |
| `/` | Find by name |
| `a` | Show each file, or the data files only |
| `~` | Go to the home directory |
| `q`, `Esc` | Leave |

The find box gets the same ghost completion as the prompts of the grid. See
[user-interface.md](user-interface.md).

The key `q` clears the find box before it leaves. A user who typed a name and
then pressed `q` wants the list back, and not the end of the program.
