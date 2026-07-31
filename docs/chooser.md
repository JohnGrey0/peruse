# The chooser of files

A call with no file opens a screen that lists the data files that are near. The
same screen also picks the table of a database. The code is in
`crates/peruse-tui/src/browser.rs`.

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

The picker of tables has the same shape, and it gives one table back:

```rust
pub fn choose_table(path: &Path, tables: &[DbTable], theme: &Theme)
    -> Result<Option<DbTable>>
```

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
the file, because a directory can hold thousands of them. A DuckDB database with
the extension `.duckdb` or `.ddb` is therefore in the list; a database with
another name is not, and the user names it on the command line.

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
| `PgUp` `PgDn`, `g` `G`, `Home` `End` | Move by ten, or to the ends |
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

## The mouse

| Event | Action | The key that it copies |
|---|---|---|
| The wheel | Move three lines | `j` / `k` |
| A click on an entry | Select that entry | `j` / `k` |
| A double click on an entry | Go into the directory, or choose the file or the table | `Enter` |
| A click on the title bar, the find box, a heading or the row of keys | Nothing | none |

Only the code that draws the list knows which entry is at a row of the
terminal, so `browser::draw` writes one pair for each entry into
`Browser::lines`. A screen with no list clears that table first, or a click
would use the positions of an older frame.

The second press of a double click opens the entry that the first press chose,
and not the entry under the pointer by then: the window keeps the selected
entry on its last row, so the first press moves the window.

A click on an entry takes the focus out of the find box and keeps the text, as
`Enter` does there. The wheel never becomes an arrow key, because an arrow key
in the find box walks the history of the box: `Browser::on_mouse` moves the
selection itself.

The chooser runs in front of the application, so the options of the command
line do not reach it. The function `browser::mouse_wanted` therefore reads
`--no-mouse` from `std::env::args` and the setting `mouse` from the settings
file. The chooser calls the same `enable_mouse` as the application, so the hook
that a panic needs stays in one place.

## The picker of the tables of a database

A DuckDB database holds many tables, and the grid shows one of them. The picker
asks which one, in front of the grid, in the same way as the chooser asks which
file.

```text
 peruse  shop.duckdb · which table?

 tables
  main.customers                                             table          ~12,480 rows
 views
  main.recent_orders                                          view


 j/k move · Enter open · / find a table · q quit
```

The picker is the chooser and not a third list. It uses the same rows, the same
find box with the ghost completion, the same scroll and the same terminal loop.
Only the entries differ: `Browser::of_database` fills the list from the tables of
the database, and the keys `h`, `~` and `a` do nothing there, because a database
has no directory above it and holds no other kind of file.

The tables come first and the views after them, each group under a heading. A
view shows the word `view` and no row count.

The row count comes from `estimated_size` in the catalog of the database. The
character `~` says that the database estimated it. Peruse never counts the rows
of a table to fill a list: a database can hold hundreds of tables, and one
`count(*)` on each of them would read the whole file before the first frame.

Three calls need no picker:

| The call | What happens |
|---|---|
| `peruse shop.duckdb` with one table | The engine opens that table. |
| `peruse shop.duckdb --table orders` | The user named the table already. |
| `peruse shop.duckdb -q "SELECT ..."` | The statement says what the user wants. The view `src` reads the first table of the list, and the statement can name any other table through the alias `__peruse_db`. |

The function `main::pick_table` reads the tables before the engine opens the
file, with `engine::database_tables`. That read attaches the database read-only
and asks its catalog, so it costs no scan of the data. A database that a newer
DuckDB wrote therefore gives its message on the command line, and not behind a
terminal that started already. See [engine.md](engine.md).
