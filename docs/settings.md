# The settings

The key `,` opens the settings page. The page holds the settings that a user
changes, and it shows what the machine gives. The code is in
`crates/peruse-core/src/config.rs` and in `crates/peruse-tui/src/app.rs`.

## Where a value comes from

Three levels give the value of a setting. The level that is nearer to the user
wins:

1. The option on the command line.
2. The setting in the file.
3. The value that Peruse builds in.

The file therefore changes the default, and the command line always wins over
it. A user can test an option one time without a change to the file.

## The file

The file is `<config>/peruse/config.toml`, beside the directory of the themes.
On Windows that is `%APPDATA%\peruse\config\config.toml`.

Peruse writes the file itself, and it writes a note above each setting, because
a person reads it. A setting with no value goes into the file as a note, so the
file also says which settings exist:

```toml
# The name of a theme, or the path of a .toml theme file.
theme = "dracula"

# The number of threads that DuckDB can use.
# With no value, DuckDB uses one thread for each core.
# threads =
```

## The settings

| Setting | Meaning | With no value |
|---|---|---|
| `theme` | The colors | `peruse-dark` |
| `threads` | The threads that DuckDB can use | one for each core |
| `memory_limit` | The memory that DuckDB can use, in whole gigabytes | one half of the machine |
| `sample_size` | The rows that the sniffer of a file of text reads | 20,480 |
| `no_index` | `true` to never index a file of text at the start | Peruse indexes a file below 256 MB |
| `panels` | `none`, `meta`, `stats` or `both` | `none` |

## The rules that the page follows

**Each change goes into the file at once.** A second key to keep a change is a
key that a user forgets to press. The title of the page says so.

**A change to the theme also goes into the file at once**, from the key `t` and
from the picker. That change writes the theme by itself: it reads the file
again and changes one line of it, so a setting that the user is testing in this
session does not go into the file without a request.

**Two settings work now, and two work at the next file.** DuckDB changes its
threads and its memory limit while it runs, so the page shows the result of a
change with no restart. The sniffer and the index do their work when a file
opens, so the page says "takes effect at the next file" for those two.

**A setting shows the built-in value when it has none.** The page writes that
value in a dim color, so the user sees what happens without a setting. The key
`d` removes a value and goes back to it.

**The key `m` takes the value of the machine**, for the threads and for the
memory limit.

## The memory limit

The unit is always the gigabyte, and the number is always a whole number. A
choice of units gives the user one more thing to get wrong, and a value of
`512MB` read as 512 gigabytes would be a bad surprise. The page therefore
refuses each unit that is not the gigabyte, and it says what to write.

With no setting, DuckDB gets **one half of the memory of the machine**. Without
a limit, DuckDB takes 80 percent of the memory for itself, and a viewer of data
is not the only program that a user runs.

The file holds the number, and the text that goes to DuckDB says `GiB`. DuckDB
reads `GB` as 1000 million bytes and `GiB` as 1024 million bytes, and the
memory of a machine is always the second one. A limit of `8GB` would give 7.4
of the gigabytes that the page shows, and the number on the screen would be a
lie.

An older file holds a text such as `"32GB"`. The function `gigabytes` reads
that form too, so a file from an older version still works.

## What the machine gives

The lower half of the page holds these facts:

| Row | Contents |
|---|---|
| `cores` | The threads that the machine can run at the same time, and the name of the processor |
| `memory` | The memory that is free, of the memory that exists |
| `duckdb now` | The threads and the memory limit that DuckDB uses at this moment |
| `spill to` | The directory that DuckDB writes to when a query needs more memory than its limit |
| `file` | The path of the settings file |

A user who sets a memory limit needs to know how much memory the machine has,
and a user who sets the threads needs to know how many cores it has. Without
those numbers the user is guessing.

The row `duckdb now` comes from the engine itself, through `Request::Configure`
and `Engine::current_setting`. It shows what DuckDB uses, and not what the user
asked for. The two are the same only after DuckDB accepts the value.

## Two rules that protect the user

**A theme in the file that no longer exists does not stop the program.** The
function `Config::theme_or_default` gives the built-in theme and the reason. A
user keeps a theme today, a later version removes it, and the only way back
from a refusal would be to find the file and edit it by hand. A theme on the
command line is a different case: the user asked for that name in this call, so
a mistake in it is an error.

**A test never writes the settings of the user who runs it.** The functions
`Config::load_from` and `Config::save_to` take a path, and `App::config_path`
holds the one that the program uses. A test points that value at a file beside
its own data.
