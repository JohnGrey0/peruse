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

The structure `Config` holds each setting. Each field is optional: a field with
no value takes the value that Peruse builds in.

| Setting | Meaning | With no value |
|---|---|---|
| `theme` | The name of a theme, or the path of a `.toml` theme file | `peruse-dark` |
| `threads` | The threads that DuckDB can use | one for each core |
| `memory_limit` | The memory that DuckDB can use, in whole gigabytes | one half of the machine, and three tenths of a machine of 8 GB or less |
| `sample_size` | The rows that the sniffer of a file of text reads. The value `-1` reads the whole file | 20,480 rows |
| `no_index` | `true` to never index a file of text at the start | Peruse indexes a file below 64 MB with 256 columns or fewer |
| `panels` | The panels that stay at the side of the grid: `none`, `meta`, `stats` or `both` | `none` |
| `band` | The rows of facts under the column names: `off`, `compact` or `detailed` | `off` |
| `step` | The rows or columns that the keys `J`, `K`, `H` and `L` move, from 1 to 1000 | 10 |
| `mouse` | `false` to make Peruse ignore the mouse | `true` |
| `recent` | The files that the user opened, the newest first | an empty list |

Two of those ten are not on the page:

- `recent` is a list that Peruse writes for the chooser of files. A user does
  not edit it by hand. See [chooser.md](chooser.md).
- `mouse` is on the command line as `--no-mouse` and in the file. A user who
  turns the mouse off wants it off at each start, and a page that the mouse
  itself can reach is a poor place for that switch.

The page therefore shows eight settings, from the enumeration `Setting`, in
this order:

| Row | Setting | What the page says |
|---|---|---|
| `theme` | `Setting::Theme` | the colors. T also opens the picker |
| `threads` | `Setting::Threads` | threads for DuckDB. Empty means one for each core |
| `memory limit` | `Setting::MemoryLimit` | such as 4GB, before DuckDB writes to the disk |
| `sample size` | `Setting::SampleSize` | rows the sniffer reads. -1 reads the whole file |
| `index at open` | `Setting::NoIndex` | index a file of text at the start, for instant jumps |
| `panels` | `Setting::Panels` | keep at the side: none, meta, stats or both |
| `column details` | `Setting::Band` | rows under the headers: off, compact or detailed. d cycles |
| `step` | `Setting::Step` | rows or columns that J, K, H and L move |

The row `index at open` takes `yes` or `no`, and it is the setting `no_index`
the other way round. The page asks the question that the user has, and the file
holds the name of the option `--no-index`.

## The keys of the page

| Key | Action |
|---|---|
| `j` `k`, `↑` `↓` | Move to another setting |
| `Enter`, `e` | Edit the value of the setting |
| `d`, `Delete`, `Backspace` | Remove the value, and go back to the built-in one |
| `m` | Take the value of this machine, for the threads and the memory limit |
| `T` | Open the theme picker |
| `Esc`, `q` | Close the page |

Inside the editor, `Enter` keeps the value and `Esc` leaves it. A setting with
a known set of answers gets the ghost completion, so `de` in the row `column
details` writes `detailed` after the cursor. See
[user-interface.md](user-interface.md).

## The rules that the page follows

**Each change goes into the file at once.** A second key to keep a change is a
key that a user forgets to press. The title of the page says so.

**A change to the theme also goes into the file at once**, from the key `t` and
from the picker. That change writes the theme by itself: it reads the file
again and changes one line of it, so a setting that the user is testing in this
session does not go into the file without a request. The key `d` in the grid
writes the band in the same way.

**Six settings work now, and two work at the next file.** DuckDB changes its
threads and its memory limit while it runs, so the page shows the result of a
change with no restart. The theme, the panels, the band and the step are all in
the front end, and they change the next frame. The sniffer and the index do
their work when a file opens, so the page says "takes effect at the next file"
for the sample size and for the index. The function `Setting::at_next_file`
holds that rule.

**A setting shows the built-in value when it has none.** The page writes that
value in a dim color, so the user sees what happens without a setting.

**A bad value gives a message and keeps the old value.** The page names what to
write: "panels: write none, meta, stats or both", "column details: write off,
compact or detailed", "step: write a number from 1 to 1000". A setting is not a
reason to lose the value that works.

## The memory limit

The unit is always the gigabyte, and the number is always a whole number. A
choice of units gives the user one more thing to get wrong, and a value of
`512MB` read as 512 gigabytes would be a bad surprise. The page therefore
refuses each unit that is not the gigabyte, and it says what to write.

With no setting, DuckDB gets **one half of the memory of the machine**. Without
a limit, DuckDB takes 80 percent of the memory for itself, and a viewer of data
is not the only program that a user runs.

A machine of 8 GB or less gets three tenths instead of one half. The limit is
the point where DuckDB starts to write to the disk, and that is the graceful way
to run out of memory. A limit near the size of the machine takes that way away:
the operating system starts to write to the disk instead, and that is much
slower. A measurement of an index of a file of 1.36 GB shows the trade: a limit
of 1 GiB made the work 17 percent slower, and it halved the memory that the
program held.

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

The same rule holds for the other names in the file. A `panels` value or a
`band` value that Peruse does not know leaves that part off, and the file still
opens.

**Peruse refuses a setting name that it does not know.** The structure
`Config` uses `deny_unknown_fields`, so `thread = 4` gives "unknown field". A
name with a spelling mistake must not pass in silence: the user would then look
for the reason that the setting does nothing.

**A test never writes the settings of the user who runs it.** The functions
`Config::load_from` and `Config::save_to` take a path, and the field
`App::config_path` holds the one that the program uses. A test points that
value at a file beside its own data.
