# peruse

**A fast, read-only viewer for data files, in your terminal.**

Peruse opens Parquet, CSV, TSV and JSON files and DuckDB databases, and shows
you the data. It is one program file. It needs no runtime, no plugin, no
notebook and no editor. Give it a file, and look at the data.

```sh
peruse trips.parquet
peruse 'data/*.parquet'
peruse events.ndjson
peruse shop.duckdb --table orders
peruse big.csv --filter "amount > 100"
peruse sales.csv -q "SELECT region, sum(amount) FROM src GROUP BY 1"
```

Peruse never writes to your data. It refuses each query that would change
anything, and it refuses the query before the query reaches the database. A
DuckDB database is stronger still: Peruse opens it read-only, so DuckDB itself
refuses a write.

---

## Contents

- [Why Peruse exists](#why-peruse-exists)
- [Install](#install)
- [The first five minutes](#the-first-five-minutes)
- [What Peruse does](#what-peruse-does)
  - [The grid](#the-grid)
  - [The detail band](#the-detail-band)
  - [The record view](#the-record-view)
  - [The filter builder](#the-filter-builder)
  - [The metadata panel](#the-metadata-panel)
  - [The column statistics](#the-column-statistics)
  - [SQL](#sql)
  - [Search](#search)
  - [DuckDB databases](#duckdb-databases)
  - [The mouse](#the-mouse)
  - [Themes](#themes)
- [Completion](#completion)
- [Settings](#settings)
- [Generate a table](#generate-a-table)
- [Every key](#every-key)
- [Every option](#every-option)
- [File formats](#file-formats)
- [Examples](#examples)
- [Peruse only reads](#peruse-only-reads)
- [Speed](#speed)
- [How it is built](#how-it-is-built)
- [Troubleshooting](#troubleshooting)

---

## Why Peruse exists

To look at a Parquet file, you usually install an extension for an editor, or
start a notebook, or write five lines of Python. Each of those needs a runtime
and a moment of your attention.

Peruse is one program file of about 30 MB. It opens the file in about the time
that you need to press Enter. It works on Windows, on macOS and on Linux, and
it works through SSH.

It does one job: it shows you what is in a data file, quickly, without any risk
to the file.

---

## Install

The command is `peruse` however you install it. Nothing below needs Rust
except the last two.

**With the tool you use already:**

```powershell
scoop bucket add peruse https://github.com/JohnGrey0/scoop-peruse
scoop install peruse          # Windows

winget install JohnGrey0.Peruse   # Windows
choco install peruse              # Windows
```

```sh
brew tap JohnGrey0/peruse
brew install peruse           # macOS, Linux
```

**A prebuilt program, with nothing to compile.** Take the archive for your machine
from the [releases](https://github.com/JohnGrey0/peruse/releases), unpack it,
and put `peruse` somewhere on your `PATH`. Each archive has a `.sha256` file
beside it.

**With Rust, and no wait:**

```sh
cargo binstall peruse-tui
```

This downloads the same prebuilt program instead of building one.

**With Rust, from source:**

```sh
cargo install --locked peruse-tui
```

This compiles DuckDB into the program, so the first build takes several
minutes and needs a C++ compiler. There is no runtime dependency afterwards:
the result is one file.

- **Windows**: the Visual Studio Build Tools, with the C++ workload.
- **macOS**: `xcode-select --install`.
- **Linux**: `build-essential` (Debian, Ubuntu) or `gcc-c++` (Fedora).

**From a clone:**

```sh
git clone https://github.com/JohnGrey0/peruse.git
cd peruse
cargo build --release
./target/release/peruse yourfile.parquet
cargo test --workspace
```

Peruse needs Rust 1.88 or later to build. A job in CI builds on exactly that
version, so the number is true and not a guess.

### Licences

Peruse is Apache-2.0. The program also holds DuckDB and the 26 C and C++
libraries inside it, and every one of those carries a permissive licence.
[THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) names each one, and it
ships beside the program in every release archive.

If a tool at your work reports the GPL, a TLS library or a copyleft licence
in Peruse, read the
[notes for a licence scan](THIRD-PARTY-LICENSES.md#notes-for-a-licence-scan).
All three of those reports are false, and that section says why.

### The two crates

| Crate | What it is |
|---|---|
| [`peruse-tui`](https://crates.io/crates/peruse-tui) | The program. Install this one. |
| [`peruse-core`](https://crates.io/crates/peruse-core) | The engine, as a library, with no terminal code. |

The crate is `peruse-tui` because the name `peruse` on crates.io belongs to a
parser library. The command it installs is still `peruse`.

---

## The first five minutes

Open a file:

```sh
peruse trips.parquet
```

Then:

| Press | And you get |
|---|---|
| `?` | the full list of keys |
| `j` `k` `h` `l` or the arrow keys | move around the grid |
| `d` | column details under the names: press once for the type and the NULL share, twice for four rows of facts |
| `r` | the current row as a vertical record, one column on each line |
| `f` | build a filter from menus. No SQL needed |
| `i` | the statistics of the column under the cursor |
| `m` | what the file itself says: row groups, codecs, the writer |
| `:` or `p` | run any command by name |
| `q` | quit |

The mouse works too: the wheel scrolls, `Ctrl` or `Shift` with the wheel moves
across the columns, and a click puts the cursor on that cell.

### Or start with no file at all

```sh
peruse
```

With no file, Peruse shows you what is around instead of a page of help:

```
 peruse  ~/work/data

  recent
  trips.parquet     ~/warehouse/2024        parquet   1.20 GB    2h ago
  sales.csv         ~/work/data                 csv    4.10 MB    1d ago

  ~/work/data
  ../
  archive/
  events.ndjson                              json    88.0 MB   just now
  sales.csv                                   csv     4.10 MB    1d ago
  trips.parquet                           parquet     1.20 GB    2h ago

  j/k move · Enter open · h up · / find · a every file · ~ home · q quit
```

Files you have opened before come first. `/` filters as you type, `h` goes up
a directory, `~` goes home, and `a` shows every file, and not only the files
whose extension Peruse knows. See
[`docs/chooser.md`](https://github.com/JohnGrey0/peruse/blob/main/docs/chooser.md).

`peruse --help` still prints the help, and so does a bare `peruse` in a
pipeline, where there is no terminal to draw on.

Peruse draws a full screen, so it needs a terminal. `peruse data.csv > out.txt`
tells you that plainly instead of writing escape sequences into your file. To
get text out of Peruse, use [`--ddl`](#generate-a-table).

---

## What Peruse does

### The grid

The grid reads one page at a time, so it can show a file that is larger than
your memory. The cost of a page follows the size of your terminal, and not the
size of the file.

- Each family of values gets its own colour: numbers, text, dates, true/false,
  bytes and nested values.
- A dim mark after each column name gives that family: `#` a number, `"` text,
  `?` true/false, `@` a date or a time, `~` bytes, `{` a structure, a list or a
  map. Press `?` for the legend.
- A NULL value looks different from an empty text, because the two are not the
  same thing.
- The key `Enter` opens the cell inspector, which shows a value that is too
  wide or too long for the grid.
- The keys `x` and `X` hide a column and show every column again. The keys `<`,
  `>` and `w` change the widths.

### The detail band

Press `d`.

The first question about a new file is "what is in each column?". The band
answers it for every column on the screen at once, between the names and the
first row of data. The key moves through three modes: off, compact, detailed.

Compact is one row: the type at the left, and the share of NULL values at the
right, so the shares line up down the grid:

```
 peruse  wide.csv  5 × 5  247 B  csv                                 peruse-dark
      order_id customer_name amount_paid ordered_at        @ region              ›
            0% VARCHAR    0% DOUBLE  20% TIMESTAMP        0%     0%
    1     1001 alice                10.5 2024-01-01 09:15:00 EU
    2     1002 bob                  NULL 2024-01-02 11:20:00 US
```

A column too narrow for both keeps the share, as `order_id` does above. The
header already shows the family of the values with its own mark.

Detailed is four rows, and every column gives the same fact on the same row:

```
      order_id customer_name amount_paid ordered_at        @ region
      BIGINT   VARCHAR       DOUBLE      TIMESTAMP           VARCH…
      0% null  0% null       20% null    0% null             0%
      ~5       ~5 distinct   ~3 distinct ~5 distinct         ~3
      1… → 10… alice → erin  7.0 → 4000… 2024-01… → 2024-01… … → US
    1     1001 alice                10.5 2024-01-01 09:15:00 EU
    2     1002 bob                  NULL 2024-01-02 11:20:00 US
```

The `~` says that a count is an estimate. A column with no facts yet shows a
dim `·`, never a blank and never a stale number.

**It is cheap on Parquet.** A footer already holds the row count and the NULL
count of each column, so the compact band over a plain Parquet file runs no
query at all, also on a file of some gigabytes. Peruse asks the engine in five
cases: a source that is not a plain Parquet file, such as a file of text or a
database; a filtered view; your own SQL; a column that the footer cannot name,
such as a structure; and the detailed mode. One query then measures every column
on the screen.

On a short terminal the band gives its rows back to the data, so the grid
always keeps half its rows. The setting `band` keeps your choice for the next
start.

### The record view

Press `r`.

A grid reads from the left to the right. A file with 300 columns therefore
needs 300 presses of a key to read one row. The record view puts the columns
under each other instead:

```
┌ record 12,481 of 3,102,556 ────────────────────────────────────────┐
│   id              BIGINT     88134221                              │
│   pickup_time     TIMESTAMP  2024-03-01 08:14:22                   │
│   vendor          VARCHAR    CMT                                   │
│   passenger_cnt   INTEGER    2                                     │
│   fare_amount     DOUBLE     12.50                                 │
│   tip_amount      DOUBLE     NULL                                  │
│   store_and_fwd   VARCHAR    (empty)                               │
└ 6/62 · Enter full value · a open all · z hides empty · n/p row · / ┘
```

A field that holds other values opens, so you can drill into a JSON object or
a Parquet structure:

```
┌ record 1 of 11,351 ────────────────────────────────────────────────┐
│   id                VARCHAR   2489651045                           │
│   type              VARCHAR   CreateEvent                          │
│ ▾ actor             struct    {5 fields}                           │
│     id              number    665991                               │
│     login           text      petroav                              │
│     gravatar_id     text      (empty)                              │
│ ▸ repo              struct    {3 fields}                           │
│ ▸ payload           struct    {5 of 20 fields}                     │
│   org               null      NULL                                 │
└ 3/9 · l/h open · a open all · z hides empty · n/p row · / find · y ┘
```

Inside the record view:

| Key | Action |
|---|---|
| `j` `k`, `↑` `↓` | move to another line |
| `PgUp` `PgDn`, `g` `G`, `Home` `End` | move by ten lines, or to the ends |
| `l` `→`, `h` `←` | open a field, close a field |
| `Space` | open a field, or close it |
| `Enter` | open a field, or show one value in full |
| `a` `c` | open every level, close every level |
| `z` | show the fields that hold no value, or hide them |
| `n` `p` | the next record, the previous record. What you opened stays open, so you can follow one field over some rows |
| `/` | find a field by name or by value, at any level |
| `y` | copy this value |
| `Y` | copy the whole record as JSON |
| `P` | copy the path, such as `"payload"."commits"[1]."sha"` |
| `=` `!` | keep, or remove, the rows with this value. This works on a value inside a structure too |
| `Esc` `q` `r` | close. The cursor of the grid moves to the column you were in |

It shows a hidden column too, in a dim colour: you open this view to see the
complete row, and a column that the grid hides is exactly the column that you
cannot see in the grid.

### Nested data

A JSON file usually holds a list of objects, and an object holds other objects.
The grid can only show such a value as one long text:

```
{'id': 665991, 'login': petroav, 'gravatar_id': '', 'url': 'https://api.git…
```

The record view is the way into it. Press `r`, then `l` on any field with a
`▸` mark. The key `Enter` on the cell in the grid does the same, and it opens the
record view with that column already open:

```
┌ record 1 of 11,351 ────────────────────────────────────────────────┐
│   id                BIGINT      2489651045                         │
│ ▾ actor             struct      {5 fields}                         │
│     id              number      665991                             │
│     login           text        petroav                            │
│     gravatar_id     text        (empty)                            │
│     url             text        https://api.github.com/user…       │
└ 3/9 · l/h open · a open all · z hides empty · n/p row · / find · y ┘
```

`Enter` on a cell that holds one value still opens the cell inspector.

**About the fields that hold no value.** DuckDB gives one type to a column, so
for a JSON file it joins the fields of every row into one structure. The
`payload` of a file of GitHub events therefore holds 20 fields, and one row
holds a value in five of them. Those 15 NULLs are the absence of a field in
that row, not 15 missing values, so the record view hides them and says how
many: `{5 of 20 fields}`. The key `z` shows them.

A column of the row is different. The schema declares it, so a NULL there is a
real missing value, and the view always shows it.

**Filtering on a nested value.** Move to a field at any depth and press `=`.
Peruse builds the condition from the path:

```sql
WHERE ("actor"."login" = 'petroav')
```

Press `P` to copy that path, which is what you need to write your own SQL
against the file.

For more, see [`docs/nested-values.md`](https://github.com/JohnGrey0/peruse/blob/main/docs/nested-values.md).

### The filter builder

Press `f`.

The builder asks three questions: which column, which test, and which value.
It then shows the list of your conditions and the expression that they compile
to.

```
┌ filter ───────────────────────────────────────────┐
│     amount        >          100                  │
│ AND region        =          EU                   │
│ OR  (SQL)                    ts > now() - 7       │
│                                                   │
│ WHERE ((("amount" > 100) AND ("region" = 'EU'))   │
│        OR (ts > now() - 7))                       │
└ a add · e edit · d delete · o AND/OR · Enter apply┘
```

| Key | Action |
|---|---|
| `j` `k`, `↑` `↓` | move to another condition |
| `a` or `+` | add a condition |
| `e` | edit the selected condition |
| `d`, `Delete`, `Backspace` | delete the selected condition |
| `o` | change the word in front of a condition between `AND` and `OR` |
| `c` | remove every condition |
| `r` | type one condition as a `WHERE` expression |
| `Enter` | apply the filter |
| `Esc` | leave, and put the old filter back |

The operators follow the type of the column. A number offers `=`, `<>`, `>`,
`>=`, `<`, `<=`, `between`, `is one of`, `is null` and `is not null`. Text also
offers `contains`, `starts with`, `ends with` and `does not contain`. A BLOB
column offers the two tests for NULL only, because the grid shows the size of
such a value and not its bytes.

The list compiles from the top to the bottom, and each step goes in
parentheses. The list `a OR b AND c` therefore gives `((a) OR (b)) AND (c)`,
and not the SQL order in which `AND` binds first. What you read is what runs.

**Three ways to filter, one filter.** All of them build the same list:

- `f`: the builder, described above.
- `E`: a `WHERE` expression that you type. It checks the expression while you
  type it, colours the SQL, remembers your history on `↑` and `↓`, and writes
  the rest of a column name after the cursor as you type. See
  [Completion](#completion).
- `=` and `!`: the quickest way. They keep, or remove, the rows that hold the
  value in the cell under the cursor. A missing value gives `IS NULL` or
  `IS NOT NULL`, not a comparison against the word "NULL".

An expression that you type with `E` becomes the whole filter, as one condition
of the same list. The builder can then show it, and `=` or `!` adds a condition
beside it instead of replacing it. The key `F` clears the filter.

### The metadata panel

Press `m`. For a Parquet file the panel shows:

- the number of rows and the number of row groups
- the compression ratio and the codecs
- the encodings
- the name of the program that wrote the file
- the key/value pairs in the footer
- the number of NULL values in each column, as the footer records it

Each fact comes from the footer, so the panel costs almost no time on a file of
50 GB.

For a CSV file the panel shows what the DuckDB sniffer found: the delimiter, the
quote character, the header row and the date formats. A wrong delimiter is the
usual reason for a CSV file that looks wrong. Peruse asks the sniffer once when
it opens the file, so the panel costs nothing.

The panel also shows the read call behind the view: `read_parquet`, `read_csv`,
`read_json_auto`, or the two `ATTACH` statements of a database. You can copy that
text into a script, or into the DuckDB command line, and get the same rows
outside Peruse.

The column under the cursor opens if it holds a structure, so the panel shows
its fields one level deep. The list follows the cursor, so `h` and `l` scroll the
panel and the grid together. A file with 400 columns therefore needs no keys of
its own.

### The column statistics

Press `i` for the column under the cursor:

- the percentage of NULL values
- the number of different values, and what that number says about the column
- the smallest value, the largest value, the mean and the standard deviation
- a small chart of the distribution, for a column of numbers
- the most frequent values, with a bar for each count

Above 100,000 rows, a column where almost every value is different gets no list
of frequent values. That query groups every row of the view, and a list where
each value occurs one time says nothing.

The statistics follow the filter. If a filter is active, the panel says so.

The panel describes one column. Press `d` for a band that describes every column
on the screen at once. See [The detail band](#the-detail-band).

### SQL

Press `e`. The file is the view `src`, so each statement that DuckDB
understands works:

```sql
SELECT region, count(*), avg(amount)
FROM src
WHERE amount > 0
GROUP BY 1
ORDER BY 2 DESC
```

The prompt opens with `SELECT * FROM src WHERE `, with the cursor after the
space, because a part of the file is what you usually want. Type the condition
and press `Enter`. `^U` clears the line if you want something else, and `Esc`
leaves the grid as it was. Over a statement, the prompt opens with that
statement, so you correct it instead of writing it again.

The prompt colours the SQL, checks it while you type, and completes a column
name on `Tab`. The key `R` goes back to the whole file.

A statement replaces the grid. A filter and a sort on the old columns would
then be wrong, so Peruse removes them.

### Search

Press `/`. Peruse searches every column, and the keys `n` and `N` walk the
matches.

The search works one part of the file at a time, so a match near the cursor
comes back immediately and `Esc` can stop a search that finds nothing. The
status line shows the progress. Each part after the first is twice the size of
the one before, so a large file needs about six parts and not forty.

### DuckDB databases

```sh
peruse shop.duckdb
```

A database holds many tables, so Peruse asks which one before it draws the grid.
A database with one table asks nothing, and `--table orders` also skips the
question:

```
 peruse  shop.duckdb · which table?

 tables
  main.customers                                             table          ~12,480 rows
 views
  main.recent_orders                                          view


 j/k move · Enter open · / find a table · q quit
```

That list comes from the catalog of the database, so it costs no scan. The `~`
says that the database estimated the count.

From there everything works as it does for a file: the filter, the sort, the
search, the statistics, the band, the record view and `--ddl`. Jumps are
instant, and there is nothing to index.

Peruse attaches the file with `READ_ONLY`, so DuckDB itself refuses a write. The
metadata panel shows the two statements it ran, and your own SQL can reach a
second table through the same alias:

```sql
SELECT o.id, c.name FROM src o JOIN __peruse_db.main.customers c ON c.id = o.customer
```

A SQLite file is not a DuckDB database. Peruse knows one from its first bytes
and says how to write a table out with `sqlite3` instead of failing on binary
data.

### The mouse

The wheel scrolls the rows. `Ctrl` or `Shift` with the wheel moves across the
columns, and so does a wheel that turns sideways. Some terminals keep `Ctrl` and
the wheel for the size of the text, and Peruse then never sees it; `Shift` is
the form that always works. A click puts the cursor on that cell, and
a click on a column name moves to that column. A click never sorts, because a
sort of a large file costs seconds and a click can land on the wrong column. A
click on the cell that the cursor is on opens that row in the record view, as
the key `r` does, and so does a double click. The first click on a cell only
chooses it, so a user who wants to read another cell never gets a box on top of
the data.

In an overlay the wheel moves the selection, a click selects the line under the
pointer, and a double click opens it, runs it or applies it, as `Enter` does. A
click outside the box closes the overlay, as `Esc` does. The chooser takes the
mouse too: the wheel moves the list, a click selects an entry, and a double
click goes into a directory or chooses a file.

A terminal that gives the mouse to a program does not select text with the
mouse in the usual way. If you copy out of the grid with the mouse, start with
`--no-mouse` or set `mouse = false`. That covers the chooser as well.

### Themes

Peruse holds 25 themes, 16 dark and 9 light. The key `t` moves to the next one,
and `T` opens the picker, which previews each theme as you move through it. The
choice keeps itself, so you set it once.

Peruse reads your own themes from TOML files in `<config>/peruse/themes`. Run
`peruse --list-themes` to see the names and the directory. A theme needs about
fifteen lines: a background, a text colour and a few accents. Peruse
calculates the other roles from them.

```toml
name = "midnight"
extends = "peruse-dark"
bg = "#0b0f14"
accent = "#7aa2f7"
number = "#e0af68"
```

Peruse uses 24-bit colour when the terminal can show it, and the 256 colours of
the xterm set when it cannot.

---

## Completion

Any prompt with a known set of answers writes the rest of the answer after your
cursor, in a dim colour. Type `am` and you see `amount` straight away:

```
filter › amount
           ▲ you typed "am"; "ount" is the suggestion
```

Press `Tab` to take it. At the end of a line, `→` takes it too.

It follows a full stop into a structure, so nested fields complete as well:

```
filter › actor.login
              ▲ you typed "actor.log"
```

You get suggestions in the filter prompt, the SQL prompt, the filter builder's
SQL step, the record view's find box, the chooser's find box, and the value of a
setting. The search prompt has none, because Peruse cannot know what you look
for. A prompt that takes a number gets none for the same reason.

The shortest matching name wins, so a file with `amount` and `amount_tax` gives
you `amount` for `am`. One more character reaches the longer one.

---

## Settings

Press `,`.

Peruse keeps your settings in `<config>/peruse/config.toml` and writes each
change as soon as you make it. There is no key to press to save. The theme
keys `t` and `T`, and the band key `d`, write their choice too, so you set each
one once.

| On the page | In `config.toml` | What it does | With no value |
|---|---|---|---|
| theme | `theme` | the colours | `peruse-dark` |
| threads | `threads` | threads for DuckDB | one for each core |
| memory limit | `memory_limit` | whole gigabytes, nothing else | half of your machine, three tenths of a machine of 8 GB or less |
| sample size | `sample_size` | rows the sniffer reads. `-1` reads the whole file | 20,480 |
| index at open | `no_index` | index a file of text at the start | yes, below 64 MB and 256 columns |
| panels | `panels` | `none`, `meta`, `stats` or `both` | `none` |
| column details | `band` | the band under the names: `off`, `compact` or `detailed` | `off` |
| step | `step` | rows or columns that `J`, `K`, `H` and `L` move, 1 to 1000 | 10 |

The row `index at open` takes `yes` or `no`, and the file holds `no_index` the
other way round: the page asks the question that you have, and the file carries
the name of the option `--no-index`.

Two more settings live in the file and not on the page:

| In `config.toml` | What it does | With no value |
|---|---|---|
| `mouse = false` | make Peruse ignore the mouse | the mouse is on |
| `recent` | the files you opened, for the chooser. Peruse writes it | empty |

The page also shows what your machine gives: the cores, the processor, the free
and the total memory, and the spill directory. It also shows what DuckDB is using
**right now**. A memory limit is a guess without those numbers.

Inside the page: `j` `k` move, `Enter` or `e` edits, `d` goes back to the
built-in value, `m` takes the value of your machine for the threads and the
memory, `T` opens the theme picker, and `Esc` closes.

Six settings take effect immediately. The sample size and the index apply to
the next file you open, and the page says so.

Command-line options always win over the file, so you can try something once
without changing anything permanently.

### The side panels

`m` adds the metadata, `i` adds the column statistics, and each keeps the
other, so pressing both gives you both, stacked with the metadata on top. `M`
cycles all four states. Set `panels = "both"` to get them at every start.

With both open, the line between them moves. The statistics take exactly the
rows their own content needs, and the metadata keeps the rest, because it holds
the list of columns and that list has no end. A column of numbers therefore asks
for four more rows than a column of text, for the chart.

The statistics of a column cost a scan, so Peruse remembers the answer for
every column of the current view and asks at most once per frame. Holding `l`
down across a wide file therefore does not queue a scan per column. The detail
band works the same way.

---

## Generate a table

A frequent job with a data file is to load it into a warehouse, and that job
needs a table. The file already holds the answer to most of the questions: the
types, which columns are never empty, how long the text gets, and which column
identifies a row. The option `--ddl` reads those answers and writes the
statement.

```sh
peruse orders.parquet --ddl postgres
```

```sql
-- Generated by peruse from 2,410,551 rows.
-- The types, the NULL rules and the sizes come from the data itself.
-- Read this before you run it: the data of today cannot promise
-- the data of tomorrow.

CREATE TABLE "orders" (
  "order_id"     bigint       NOT NULL,  -- 2,410,551 distinct
  "customer_id"  bigint       NOT NULL,  -- 88,204 distinct
  "placed_at"    timestamp    NOT NULL,  -- 2,265,109 distinct
  "status"       varchar(16)  NOT NULL,  -- 5 distinct, longest 9
  "total"        numeric(10,2),          -- 41,220 distinct, 3% null
  CONSTRAINT "pk_orders" PRIMARY KEY ("order_id")
);

-- The key (order_id) is unique over the 2,410,551 rows of the file, by an exact count.

-- Index candidates, from the shape of the data. Your queries decide
-- which of these earn their cost. Each index makes a write slower.
CREATE INDEX "ix_orders_placed_at" ON "orders" ("placed_at");  -- holds a time, so a query asks for a period
CREATE INDEX "ix_orders_customer_id" ON "orders" ("customer_id");  -- the name shows a reference to another table
```

The databases that `--ddl` knows:

`oracle`, `mysql` (also `mariadb`), `postgres` (also `postgresql`),
`snowflake`, `bigquery`, `sqlserver` (also `mssql`), `duckdb`, `dynamodb`.

### What it works out

- **The type.** It maps the type of the file to the type of the database. A
  `DECIMAL(18,3)` keeps its precision and its scale. A timestamp keeps its time
  zone, which is a different type in every database.
- **The size of a text column.** It measures the longest value and rounds up,
  so a value that grows a little needs no change to the table. Oracle gets a
  `CLOB` above 4000, and MySQL gets a `TEXT`.
- **`NOT NULL`.** A column with no missing value in the whole file gets it.
- **The primary key.** It looks for one column that is unique. If no single
  column is unique, it looks at pairs of columns. It then counts the winner
  exactly, so it never writes a key it has not proved.
- **The indexes.** It suggests at most five: a column that holds a time, a
  column whose name points at another table, and a column with few values
  against many rows.

### What it cannot know

The generator reads the data, and the data does not hold everything:

- **Uniqueness today is not a key.** A column can be unique by accident,
  especially in a small file. Below a thousand rows the output says so.
- **A measure is not a key.** Peruse never proposes a price or a quantity as a
  primary key, even when that column happens to be unique.
- **Foreign keys.** It sees the name `customer_id` on a column. It cannot see
  the table that the column points to.
- **Your queries decide your indexes.** The suggestions come from the shape of
  the data alone.

Read the statement before you run it. It is a first draft that saves you the
typing, not a design.

### It follows the view

`--ddl` uses the same view as the grid, so `--query` and `--filter` change what
it measures. You can therefore build a table for the result of a statement, and
not for the file alone:

```sh
peruse trips.parquet \
  -q "SELECT vendor, date_trunc('day', pickup) AS day, sum(fare) AS fare FROM src GROUP BY 1, 2" \
  --ddl snowflake --table trip_daily
```

For a file, `--table` gives the name of the new table, and the default is the
name of the file. For a database, `--table` chooses which table to read, and the
statement takes the name of that table in the spelling that the catalog holds. A
database with more than one table therefore needs `--table` for `--ddl`: Peruse
asks rather than pick a table for you.

### DynamoDB

DynamoDB takes no SQL. The option writes the JSON request for
`aws dynamodb create-table`, with the partition key, a sort key when a column
holds a time, and a note about which columns would need a secondary index.

```sh
peruse events.ndjson --ddl dynamodb > create-table.json
aws dynamodb create-table --cli-input-json file://create-table.json
```

---

## Every key

Press `?` inside Peruse for this list. Press `:` or `p` to run any command by
its name, so no command is behind a key that you must know first.

### Move

| Keys | Action |
|---|---|
| `j`, `↓` | next row |
| `k`, `↑` | previous row |
| `PgDn`, `Ctrl-F` | page down |
| `PgUp`, `Ctrl-B` | page up |
| `Ctrl-D` | down half a page |
| `Ctrl-U` | up half a page |
| `J` | down one step of rows |
| `K` | up one step of rows |
| `L` | right one step of columns |
| `H` | left one step of columns |
| `g` | first row |
| `G` | last row |
| `l`, `→`, `Tab` | next column |
| `h`, `←`, `Shift-Tab` | previous column |
| `a`, `0`, `^`, `Home` | first column of this row |
| `z`, `$`, `End` | last column of this row |
| `o`, `Ctrl-Home` | back to the start: first row and first column |
| `O`, `Ctrl-End` | the far corner: last row and last column |
| `#` | jump to a row number |

Many laptops have no `Home`, `End`, `PgUp` or `PgDn` key, so every movement
also has a letter or a chord. `Home` and `End` reach the two ends of the row,
as they do in a spreadsheet; `a` and `z` do the same and need no shift key. One
key, `o`, comes all the way back to the first row and the first column. A step
is 10 rows or columns, and the setting `step` changes it.

### Query

| Keys | Action |
|---|---|
| `s` | sort by this column (up, then down, then off) |
| `S` | clear the sort |
| `f` | build a filter from menus |
| `E` | filter with a `WHERE` expression |
| `=` | keep only the rows with the value in this cell |
| `!` | remove the rows with the value in this cell |
| `F` | clear the filter |
| `e` | edit the SQL behind the grid |
| `u` | undo the last filter, sort or query |
| `U` | redo what `u` undid |
| `R` | reset to the whole file |
| `/` | search every column |
| `n` | next match |
| `N` | previous match |

### Inspect

| Keys | Action |
|---|---|
| `m` | the file metadata panel |
| `i` | the statistics of this column |
| `M` | cycle the side panels: none, metadata, statistics, both |
| `d` | column details under the headers: off, compact, detailed |
| `Enter` | show this cell in full |
| `r` | show this row as a vertical record |

### Columns

| Keys | Action |
|---|---|
| `>` | make this column wider |
| `<` | make this column narrower |
| `w` | fit every width to what is on the screen |
| `x` | hide this column |
| `X` | show every hidden column |

### Other

| Keys | Action |
|---|---|
| `y` | copy this cell |
| `Y` | copy this row as TSV |
| `I` | index this file now, so that jumps are instant |
| `t` | next theme |
| `T` | choose a theme |
| `,` | settings, and what this machine gives |
| `?`, `F1` | the help |
| `:`, `p` | run a command by name |
| `Esc` | stop the query that runs now |
| `q`, `Ctrl-C` | quit |

`Ctrl-P` is deliberately not the palette: Visual Studio Code takes that chord
for its own file finder, and Peruse never sees it inside that terminal.

### The mouse

| Event | Action |
|---|---|
| wheel | up and down the rows |
| `Shift` or `Ctrl` and wheel | across the columns |
| wheel sideways | the same |
| click | put the cursor on that cell |
| click that cell again | open that row in the record view, as `r` does |
| double click | the same |
| click a column name | go to that column. A click never sorts. |
| wheel in an overlay | move the selection |
| click in an overlay | select that line |
| click that line again | on a value, open or close it |
| double click in an overlay | open, run or apply, as `Enter` does |
| click outside an overlay | close it, as `Esc` does |

`--no-mouse`, or `mouse = false`, turns all of that off, in the chooser too.

### In any prompt

| Keys | Action |
|---|---|
| `Enter` | apply |
| `Esc` | cancel |
| `↑` `↓` | the previous or the next entry from the history |
| `Tab`, `→` | take the ghost completion |
| `Ctrl-W` | delete the word in front of the cursor |
| `Ctrl-U` `Ctrl-K` | delete to the start, or to the end |
| `Ctrl-A` `Ctrl-E` | go to the start, or to the end |
| `Ctrl-←` `Ctrl-→` | move one word |
| `Alt-←` `Alt-→` | the same, for the Option key of a Mac |
| `Alt-B` `Alt-F` | the same |

Copying uses OSC 52, so it works through SSH.

---

## Every option

```
peruse [OPTIONS] [FILE]
```

`FILE` is a path or a glob, such as `data.parquet` or `'part-*.csv'`. Put a
glob in quotation marks, so that your shell gives the pattern to Peruse and
does not expand it first. With no `FILE`, Peruse opens the
[chooser](#or-start-with-no-file-at-all).

| Option | Argument | What it does |
|---|---|---|
| `-q`, `--query` | `SQL` | Start with this statement instead of the whole file. The file is the view `src`. Peruse checks the statement before it opens the file, so a mistake is one message on the command line. |
| `-f`, `--filter` | `EXPR` | Start with this `WHERE` expression. It becomes the first condition in the filter list, so the builder can show it and edit it. |
| `-t`, `--theme` | `NAME` | The name of a theme, or the path of a `.toml` theme file. The default is `peruse-dark`. |
| `--list-themes` | | Print the theme names and the directory for your own themes, then exit. |
| `--ddl` | `DIALECT` | Print a `CREATE TABLE` statement for this file and exit. See [Generate a table](#generate-a-table). |
| `--table` | `NAME` | For a database: which table to read, as `orders` or `main.orders`. For a file: the table name that `--ddl` writes. The default for `--ddl` is the table, or the name of the file. |
| `--delimiter` | `CHAR` | The CSV delimiter. The words `tab` and `space`, and the two characters `\t`, also work. Without it, the DuckDB sniffer finds the delimiter. |
| `--no-header` | | Read the first CSV row as data, and not as the column names. |
| `--all-varchar` | | Read every CSV column as text. Use this when the type detection guesses wrong, or when you want to see the raw text of the file. |
| `--ignore-errors` | | Skip each CSV row that DuckDB cannot read, instead of stopping. |
| `--sample-size` | `N` | The number of rows that the CSV or JSON sniffer examines before it decides the types. The value `-1` reads the whole file, which is slower but always correct. |
| `--threads` | `N` | The number of worker threads. The default is one for each core. |
| `--memory-limit` | `SIZE` | The memory ceiling before DuckDB writes to the disk, for example `4GB`. |
| `--no-index` | | Never index a file of text when Peruse opens it. See [File formats](#file-formats). |
| `--no-mouse` | | Ignore the mouse, so the terminal selects text with it as usual. |
| `-h`, `--help` | | Print the help. |
| `-V`, `--version` | | Print the version. |

---

## File formats

Peruse finds the format in four steps: the first bytes for a database, then the
extension of the file, then the first bytes again for Parquet, Arrow or JSON,
then CSV as the last choice. A file `data.dat` that holds values with commas
therefore opens correctly, and a database called `sales.db` gets a message about
a database and not a parse failure.

| Format | Extensions | Notes |
|---|---|---|
| Parquet | `.parquet` `.parq` `.pq` | Full footer metadata. Jumps are instant. |
| CSV and friends | `.csv` `.tsv` `.tab` `.psv` | The extension gives the delimiter. The sniffer finds the rest. |
| JSON | `.json` `.ndjson` `.jsonl` | One object for each row, one list of objects, or one object with a list inside it. The reader finds the form. |
| DuckDB database | `.duckdb` `.ddb`, or any name | Opened read-only. Peruse shows one table of it. Jumps are instant. See [DuckDB databases](#duckdb-databases). |

Add `.gz`, `.zst` or `.bz2` to any text format: `events.ndjson.gz` works.

A glob opens many files as one table. When the files hold their columns in a
different order, Peruse matches them by name. A glob cannot open two databases:
name the one database file instead.

### Formats that Peruse does not read yet

**Arrow IPC** (`.arrow`, `.ipc`, `.feather`, `.arrows`). Peruse knows the format
from the first bytes of the file, and it says what to do:

```
$ peruse data.arrow
Error: data.arrow: Peruse cannot read an Arrow IPC file yet.
Change it to Parquet first, for example:
  python -c "import pyarrow.feather as f, pyarrow.parquet as q; q.write_table(f.read_table('data.arrow'), 'out.parquet')"
```

The DuckDB build inside Peruse holds no reader that takes the name of an Arrow
file. Its `arrow_scan` functions take a pointer to data that is in memory
already, which is a different thing. A future version can read the file in Rust
and give the blocks to DuckDB.

**SQLite** (any name). No extension puts a file in this group: Peruse knows the
format from the first bytes only. The reader is a DuckDB extension, and Peruse
refuses `INSTALL` and `LOAD` on purpose, so it cannot fetch one. Peruse therefore
says what to do:

```
$ peruse shop.db
Error: shop.db: this is a SQLite database, and Peruse cannot read one yet.
Write a table out first, for example:
  sqlite3 shop.db -header -csv "SELECT * FROM your_table" > out.csv
```

**XML.** DuckDB has no XML reader in its core, and Peruse refuses `INSTALL` and
`LOAD` on purpose, so it cannot fetch one. XML also has no one row shape, so a
viewer needs the name of the element that is a row. Change the file first, for
example with `xq`, and then open the result.

### Indexing a file of text

A Parquet file, an Arrow file and a DuckDB table hold their rows in blocks, and
each block knows its own count. A jump to the last row is therefore instant.

A CSV file and a JSON file have no such structure. A jump to row 8,000,000
must read each row in front of it. Peruse therefore copies such a file into a
table in memory when it opens the file, if the file is below 64 MB and has 256
columns or fewer. Above either limit the footer shows a note, and Peruse waits
for the key `I`. You never wait for a scan that you did not ask for.

Both limits come from a measurement. The index takes about one and a third times
the size of the file, so 64 MB is about 85 MB of memory, which is one percent of
a machine with 8 GB. And a file of 170 MB with 10,000 columns is under a size
limit but costs 21 seconds and 2.7 GB to index, so the columns need their own
limit.

Peruse does not write to your file. The index is a table in memory, and DuckDB
writes the remainder to a temporary directory when the table does not fit.
Use `--no-index` to turn this off.

---

## Examples

**Look at a file.**

```sh
peruse trips.parquet
```

**Look at a set of files as one table.**

```sh
peruse 'year=2024/month=*/part-*.parquet'
```

**Look at one table of a DuckDB database.**

```sh
peruse shop.duckdb                    # Peruse asks which table
peruse shop.duckdb --table main.orders
```

**Start with a filter.**

```sh
peruse trips.parquet --filter "fare_amount > 100 AND tip_amount IS NULL"
```

**Start with a query.**

```sh
peruse trips.parquet -q "
  SELECT vendor, count(*) AS trips, avg(fare_amount) AS avg_fare
  FROM src GROUP BY 1 ORDER BY 2 DESC"
```

**Open a CSV with a semicolon delimiter and no header row.**

```sh
peruse export.csv --delimiter ';' --no-header
```

**Open a CSV whose types the sniffer guesses wrong, as plain text.**

```sh
peruse messy.csv --all-varchar --ignore-errors
```

**Read the whole file before deciding the types.**

```sh
peruse sparse.csv --sample-size -1
```

**Open compressed JSON lines.**

```sh
peruse logs.ndjson.gz
```

**Keep the memory in check on a small machine.**

```sh
peruse huge.parquet --memory-limit 2GB --threads 4
```

**Use your own theme.**

```sh
peruse data.parquet --theme ~/.config/peruse/themes/midnight.toml
```

---

## Peruse only reads

Peruse gives you one promise: it does not change your data. Two layers keep
that promise.

**The structure of the connection.** The database is always in memory. Peruse
reaches your data files only through the table functions `read_parquet`,
`read_csv`, `read_json` and `read_json_auto`. These functions read, and they
cannot write.
Peruse never opens a data file for write access, and it never installs or loads
an extension, so it cannot reach the network.

A DuckDB database is the one file that DuckDB itself opens, and Peruse attaches
it with `ATTACH … (READ_ONLY)`. The promise is stronger there than anywhere
else: the storage engine refuses the write, so the promise does not rest on the
words of a statement at all. A test writes through the connection, past the
guard, and the database refuses it.

**The words of the statement.** A statement from you must be one statement. It
must start with a word that only reads, and it must hold no word that writes,
at any position. This second rule is what stops `WITH … INSERT`,
`EXPLAIN COPY …`, `COPY … TO`, `ATTACH`, `EXPORT DATABASE` and `INSTALL`.
Peruse removes the comments, the values in quotation marks and the identifiers
in quotation marks before it looks for a word, so a column with the real name
`"update"`, or a value `'drop table'`, still works.

A `WHERE` expression gets one more test: its parentheses must balance. Without
that test, an expression could close the condition that Peruse builds and
change the rest of the statement.

```
$ peruse data.parquet -q "COPY src TO 'out.csv'"
Error: --query: `COPY` is not a query — Peruse is read-only, start with SELECT / WITH / FROM / DESCRIBE / SUMMARIZE
```

The filter builder writes its own SQL. Each column name and each value goes
through a quoting function, and the parentheses always balance, so a value that
you type cannot change the shape of the statement. A test feeds injection
strings through every operator and every type to prove it.

**One thing to know.** The promise is about writing. A statement that you type
yourself can still *read* another file on your machine, for example with
`read_csv('/etc/passwd')` inside a subquery. Peruse runs your SQL with your own
permissions. That matters only if you paste a statement that somebody else
wrote.

---

## Speed

These times come from a file of 10 million rows and 9 columns, with a release
build, on a desktop with 16 cores and 93 GB of memory. A machine with fewer
cores gives larger numbers. Read the table as a comparison between the
operations, and not as a promise about your machine.

| | 67 MB Parquet | 1.27 GB CSV |
|---|---|---|
| open, until the schema arrives | 13.7 ms | 55.5 ms |
| the first screen, 50 rows | 15.1 ms | 10.3 ms |
| `count(*)` | 2.6 ms | 373.5 ms |
| index the file of text | - | 1389.0 ms |
| move to the last screen | 17.0 ms | 6.5 ms *(after the index)* |
| filter, and count the filtered rows | 15.3 / 14.3 ms | 4.8 / 2.5 ms |
| sort on a column | 25.9 ms | 7.6 ms |
| statistics of a column of numbers | 42.4 ms | 23.4 ms |
| search every column | 15.0 ms | 7.4 ms |
| the metadata panel | 35.5 ms | 0.1 ms |

To measure the times on your own machine:

```sh
cargo run --release -p peruse-core --example make-sample -- ./sample 10000000
cargo run --release -p peruse-core --example bench -- ./sample/sample.parquet
```

The times come from eight decisions. These four matter most, and the document
below holds all eight.

**Peruse reads only the rows that it shows.** A page is a `LIMIT` and an
`OFFSET` against the file. The cost therefore follows the size of your
terminal, and not the size of the file. A count of a Parquet file comes from
the footer.

**Peruse examines a file of text once.** A CSV file holds no schema, so DuckDB
looks for the delimiter and the types, and it did that again for every
statement. Peruse now asks once, at open, and writes the answer into the read
call. On a 258 MB CSV file one page of 50 rows cost 100 ms, and 92 of those were
the examination; the same page now costs 8 ms.

**The user interface thread never blocks.** The engine runs on its own thread.
A group of scroll events becomes one request for the newest page. Each response
carries an epoch, so a slow count from a filter that you already changed cannot
replace the current count. The key `Esc` stops a query that runs now.

**Peruse indexes a file of text.** See
[Indexing a file of text](#indexing-a-file-of-text).

The search got two changes of its own. It reads a window of a few thousand rows
in front of the cursor before the rest of its part, and a match there answers
the usual search: over 250,000 rows that scan cost 90 ms, and over the window it
costs 2 ms. And the test on each column is now `contains(lower(…), …)` and not
`ILIKE`, which needed 265 ms for the same answer that `contains` gives in 91.

[`docs/performance.md`](https://github.com/JohnGrey0/peruse/blob/main/docs/performance.md)
gives the whole list, with the cost of the detail band on files up to 2.7 GB.

---

## How it is built

```
crates/peruse-core   the engine, the queries, the filter model, the metadata,
                     the statistics and the themes. No terminal code.
crates/peruse-tui    the terminal front end
```

`peruse-core` has no dependency on ratatui and no dependency on crossterm. The
themes are also in that crate. A front end with a graphical user interface can
therefore use the same API, and the engine needs no change.

The engine is DuckDB, compiled into the program file. Peruse builds every
statement from one `View` value: the relation, the filter and the sort. A
filter from the builder and a sort from the key `s` therefore work together,
and the code needs no special case for them.

The directory [`docs/`](https://github.com/JohnGrey0/peruse/blob/main/docs/README.md) holds one document for each part of the
system: the architecture, the engine, the query generation, the filter, the
table generator, the nested values, the read-only guard, the worker and the
concurrency, the user interface, the keys and the commands, the chooser, the
settings, the themes, the performance and the releases.

```sh
cargo test --workspace
```

The tests include end-to-end tests that open a real file, run a real engine
thread, draw a real frame, and then examine the characters on the screen.

---

## Troubleshooting

**The CSV looks wrong: every row is in one column.**
The sniffer guessed the delimiter wrong. Press `m` to see what it decided, then
reopen with `--delimiter ';'` or whichever character the file uses.

**A column that should be a number is text, or the reverse.**
The sniffer reads a sample of the rows. Use `--sample-size -1` to read the
whole file, or `--all-varchar` to read everything as text.

**A jump to the end of a large CSV or JSON file is slow.**
Press `I` to index the file. See
[Indexing a file of text](#indexing-a-file-of-text).

**The filter finds nothing and I expected rows.**
Press `f` to see the conditions. A comparison never matches a NULL, so use `is
null` for missing values. A text comparison is exact; use `contains` for a
part of a value.

**Colours look wrong or missing.**
Your terminal may not report 24-bit colour. Peruse falls back to 256 colours.
Try another theme with `t`, or set `COLORTERM=truecolor` if your terminal does
support it.

**I cannot select text with the mouse any more.**
A terminal that gives the mouse to a program stops selecting text in the usual
way. Start with `--no-mouse`, or put `mouse = false` in the settings file.

**A `.db` file will not open.**
Look at the message. Peruse reads the first bytes, so it knows a DuckDB database
from a SQLite one and says which it found. It reads the first, and it tells you
how to write a table out of the second.

**The band shows `·` and nothing else.**
The facts are on their way. For a filtered view or a file of text they need a
scan of the view, and on a file of some gigabytes that takes a moment. Press `d`
twice to turn the band off.

**The build fails on the first try.**
It is almost always the C++ toolchain. See [Install](#install).

---

Licensed under the Apache License 2.0. See [LICENSE](https://github.com/JohnGrey0/peruse/blob/main/LICENSE).
