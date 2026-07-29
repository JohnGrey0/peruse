# peruse

**A fast, read-only viewer for data files, in your terminal.**

Peruse opens Parquet, CSV, TSV and JSON files and shows you the data. It is one
program file. It needs no runtime, no plugin, no notebook and no editor. Give
it a file, and look at the data.

```sh
peruse trips.parquet
peruse 'data/*.parquet'
peruse events.ndjson
peruse big.csv --filter "amount > 100"
peruse sales.csv -q "SELECT region, sum(amount) FROM src GROUP BY 1"
```

Peruse never writes to your data. It refuses each query that would change
anything, and it refuses the query before the query reaches the database.

---

## Contents

- [Why Peruse exists](#why-peruse-exists)
- [Install](#install)
- [The first five minutes](#the-first-five-minutes)
- [What Peruse does](#what-peruse-does)
  - [The grid](#the-grid)
  - [The record view](#the-record-view)
  - [The filter builder](#the-filter-builder)
  - [The metadata panel](#the-metadata-panel)
  - [The column statistics](#the-column-statistics)
  - [SQL](#sql)
  - [Search](#search)
  - [Themes](#themes)
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

You need a Rust toolchain (1.88 or later) and a C++ compiler. The build puts
DuckDB inside the program file, so the result has no runtime dependency.

```sh
git clone https://github.com/JohnGrey0/peruse.git
cd peruse
cargo install --path crates/peruse-tui
```

The first build needs some minutes, because it compiles DuckDB. The builds
after it do not.

To build without installing:

```sh
cargo build --release
./target/release/peruse yourfile.parquet
```

To check that everything works:

```sh
cargo test --workspace
```

**Windows**: install the Visual Studio Build Tools with the C++ workload.
**macOS**: `xcode-select --install`.
**Linux**: install `build-essential` (Debian, Ubuntu) or `gcc-c++` (Fedora).

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
| `r` | the current row as a vertical record, one column on each line |
| `f` | build a filter from menus — no SQL needed |
| `i` | the statistics of the column under the cursor |
| `m` | what the file itself says: row groups, codecs, the writer |
| `q` | quit |

If you type `peruse` with no file, it prints its help.

---

## What Peruse does

### The grid

The grid reads one page at a time, so it can show a file that is larger than
your memory. The cost of a page follows the size of your terminal, and not the
size of the file.

- Each family of values gets its own colour: numbers, text, dates, true/false,
  bytes and nested values.
- A NULL value looks different from an empty text, because the two are not the
  same thing.
- The key `Enter` opens the cell inspector, which shows a value that is too
  wide or too long for the grid.
- The keys `x` and `X` hide a column and show every column again. The keys `<`,
  `>` and `w` change the widths.

### The record view

Press `r`.

A grid reads from the left to the right. A file with 300 columns therefore
needs 300 presses of a key to read one row. The record view puts the columns
under each other instead:

```
┌ record 12,481 of 3,102,556 ───────────────────────┐
│ column          type       value                  │
│ id              BIGINT     88134221               │
│ pickup_time     TIMESTAMP  2024-03-01 08:14:22    │
│ vendor          VARCHAR    CMT                    │
│ passenger_cnt   INTEGER    2                      │
│ fare_amount     DOUBLE     12.50                  │
│ tip_amount      DOUBLE     NULL                   │
│ store_and_fwd   VARCHAR    (empty)                │
└ field 6/62 · j/k move · n/p record · / find ──────┘
```

A field that holds other values opens, so you can drill into a JSON object or
a Parquet structure:

```
┌ record 1 of 11,351 ──────────────────────────────────────────┐
│   id                VARCHAR   2489651045                     │
│   type              VARCHAR   CreateEvent                    │
│ ▾ actor             struct    {5 fields}                     │
│     id              number    665991                         │
│     login           text      petroav                        │
│     gravatar_id     text      (empty)                        │
│ ▸ repo              struct    {3 fields}                     │
│ ▸ payload           struct    {5 of 20 fields}               │
│   org               null      NULL                           │
└ field 3/9 · l open · h close · / find · z shows the empty ───┘
```

Inside the record view:

| Key | Action |
|---|---|
| `j` `k`, `↑` `↓` | move to another line |
| `PgUp` `PgDn`, `g` `G` | move by ten lines, or to the ends |
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
`▸` mark.

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

For more, see [`docs/nested-values.md`](docs/nested-values.md).

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
| `a` | add a condition |
| `e` | edit the selected condition |
| `d` | delete the selected condition |
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

- `f` — the builder, described above.
- `E` — a `WHERE` expression that you type. It checks the expression while you
  type it, colours the SQL, remembers your history on `↑` and `↓`, and
  completes a column name when you press `Tab`.
- `=` and `!` — the quickest way. They keep, or remove, the rows that hold the
  value in the cell under the cursor. A missing value gives `IS NULL` or
  `IS NOT NULL`, not a comparison against the word "NULL".

An expression that you type becomes one condition in the same list, so a quick
filter adds to it and does not replace it. The key `F` clears the filter.

### The metadata panel

Press `m`. For a Parquet file the panel shows:

- the number of rows and the number of row groups
- the compression ratio and the codecs
- the encodings
- the name of the program that wrote the file
- the key/value pairs in the footer
- the exact number of NULL values in each column

Each fact comes from the footer, so the panel costs almost no time on a file of
50 GB.

For a CSV file the panel shows the results of the DuckDB sniffer: the
delimiter, the quote character, the header row and the date formats. A wrong
delimiter is the usual reason for a CSV file that looks wrong.

The panel also shows the `read_parquet` or `read_csv` call behind the view. You
can copy that text into a script and get the same rows outside Peruse.

### The column statistics

Press `i` for the column under the cursor:

- the percentage of NULL values
- the number of different values, and what that number says about the column
- the smallest value, the largest value, the mean and the standard deviation
- a small chart of the distribution, for a column of numbers
- the most frequent values, for each other column

The statistics follow the filter. If a filter is active, the panel says so.

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

The prompt colours the SQL, checks it while you type, and completes a column
name on `Tab`. The key `R` goes back to the whole file.

A statement replaces the grid. A filter and a sort on the old columns would
then be wrong, so Peruse removes them.

### Search

Press `/`. Peruse searches every column, and the keys `n` and `N` walk the
matches.

The search works one part of the file at a time, so a match near the cursor
comes back immediately and `Esc` can stop a search that finds nothing. The
status line shows the progress.

### Themes

Peruse holds nine themes. The key `t` moves to the next one, and `T` opens the
picker, which previews each theme as you move through it.

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

`oracle`, `mysql` (also `mariadb`), `postgres`, `snowflake`, `bigquery`,
`sqlserver` (also `mssql`), `duckdb`, `dynamodb`.

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
- **A measure is not a key.** A price or a quantity is never proposed as a
  primary key, even when it happens to be unique.
- **Foreign keys.** It sees that a column is named `customer_id`. It cannot see
  the table that it points to.
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

Press `?` inside Peruse for this list. Press `:` to run any command by its
name, so no command is behind a key that you must know first.

### Move

| Keys | Action |
|---|---|
| `j`, `↓` | next row |
| `k`, `↑` | previous row |
| `PgDn`, `Ctrl-F` | page down |
| `PgUp`, `Ctrl-B` | page up |
| `g`, `Home` | first row |
| `G`, `End` | last row |
| `l`, `→`, `Tab` | next column |
| `h`, `←`, `Shift-Tab` | previous column |
| `^` | first column |
| `$` | last column |
| `#` | jump to a row number |

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
| `R` | reset to the whole file |
| `/` | search every column |
| `n` | next match |
| `N` | previous match |

### Inspect

| Keys | Action |
|---|---|
| `m` | the file metadata panel |
| `i` | the statistics of this column |
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
| `?`, `F1` | the help |
| `:`, `Ctrl-P` | run a command by name |
| `Esc` | stop the query that runs now |
| `q`, `Ctrl-C` | quit |

### In any prompt

| Keys | Action |
|---|---|
| `Enter` | apply |
| `Esc` | cancel |
| `↑` `↓` | the previous or the next entry from the history |
| `Tab` | complete a column name (filter and SQL prompts) |
| `Ctrl-W` | delete the word in front of the cursor |
| `Ctrl-U` `Ctrl-K` | delete to the start, or to the end |
| `Ctrl-A` `Ctrl-E` | go to the start, or to the end |

Copying uses OSC 52, so it works through SSH.

---

## Every option

```
peruse [OPTIONS] [FILE]
```

`FILE` is a path or a glob, such as `data.parquet` or `'part-*.csv'`. Put a
glob in quotation marks, so that your shell gives the pattern to Peruse and
does not expand it first. With no `FILE`, Peruse prints its help.

| Option | Argument | What it does |
|---|---|---|
| `-q`, `--query` | `SQL` | Start with this statement instead of the whole file. The file is the view `src`. Peruse checks the statement before it opens the file, so a mistake is one message on the command line. |
| `-f`, `--filter` | `EXPR` | Start with this `WHERE` expression. It becomes the first condition in the filter list, so the builder can show it and edit it. |
| `-t`, `--theme` | `NAME` | The name of a theme, or the path of a `.toml` theme file. The default is `peruse-dark`. |
| `--list-themes` | | Print the theme names and the directory for your own themes, then exit. |
| `--ddl` | `DIALECT` | Print a `CREATE TABLE` statement for this file and exit. See [Generate a table](#generate-a-table). |
| `--table` | `NAME` | The table name for `--ddl`. The default is the name of the file. |
| `--delimiter` | `CHAR` | The CSV delimiter. The words `tab` and `space`, and the two characters `\t`, also work. Without it, the DuckDB sniffer finds the delimiter. |
| `--no-header` | | Read the first CSV row as data, and not as the column names. |
| `--all-varchar` | | Read every CSV column as text. Use this when the type detection guesses wrong, or when you want to see the raw text of the file. |
| `--ignore-errors` | | Skip each CSV row that DuckDB cannot read, instead of stopping. |
| `--sample-size` | `N` | The number of rows that the CSV or JSON sniffer examines before it decides the types. The value `-1` reads the whole file, which is slower but always correct. |
| `--threads` | `N` | The number of worker threads. The default is one for each core. |
| `--memory-limit` | `SIZE` | The memory ceiling before DuckDB writes to the disk, for example `4GB`. |
| `--no-index` | | Never index a file of text when Peruse opens it. See [File formats](#file-formats). |
| `-h`, `--help` | | Print the help. |
| `-V`, `--version` | | Print the version. |

---

## File formats

Peruse finds the format in three steps: the extension of the file, then the
first bytes of the file, then CSV as the last choice. A file `data.dat` that
holds values with commas therefore opens correctly.

| Format | Extensions | Notes |
|---|---|---|
| Parquet | `.parquet` `.parq` `.pq` | Full footer metadata. Jumps are instant. |
| CSV and friends | `.csv` `.tsv` `.tab` `.psv` | The extension gives the delimiter. The sniffer finds the rest. |
| JSON | `.json` `.ndjson` `.jsonl` | One object for each row, one list of objects, or one object with a list inside it. The reader finds the form. |

Add `.gz`, `.zst` or `.bz2` to any text format: `events.ndjson.gz` works.

A glob opens many files as one table. When the files hold their columns in a
different order, Peruse matches them by name.

### Formats that Peruse does not read yet

**Arrow IPC** (`.arrow`, `.ipc`, `.feather`). Peruse knows the format from the
first bytes of the file, and it says what to do:

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

**XML.** DuckDB has no XML reader in its core, and Peruse refuses `INSTALL` and
`LOAD` on purpose, so it cannot fetch one. XML also has no one row shape, so a
viewer must be told which element is a row. Change the file first, for example
with `xq`, and then open the result.

### Indexing a file of text

A Parquet file and an Arrow file hold their rows in blocks, and each block
knows its own count. A jump to the last row is therefore instant.

A CSV file and a JSON file have no such structure. A jump to row 8,000,000
must read each row in front of it. Peruse therefore copies such a file into a
table in memory when it opens the file, if the file is below 256 MB. For a
larger file the footer shows a note, and Peruse waits for the key `I`. You
never wait for a scan that you did not ask for.

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

**Open a CSV whose types are guessed wrong, as plain text.**

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
reaches your files only through the table functions `read_parquet`, `read_csv`,
`read_json_auto` and `read_arrow`. These functions read, and they cannot write.
Peruse never opens your files for write access, and it never installs or loads
an extension, so it cannot reach the network.

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
build, on a usual laptop:

| | 67 MB Parquet | 1.27 GB CSV |
|---|---|---|
| open, until the schema arrives | 16 ms | 131 ms |
| the first screen | 20 ms | 89 ms |
| `count(*)` | 5 ms | 437 ms |
| move to the last row | 22 ms | 4 ms *(after the index)* |
| filter and count again | 21 / 18 ms | 5 / 3 ms |
| sort on a column | 34 ms | 11 ms |
| column statistics and histogram | 247 ms | 214 ms |
| search every column | 225 ms | 226 ms |

To measure the times on your own machine:

```sh
cargo run --release -p peruse-core --example make-sample -- ./sample 10000000
cargo run --release -p peruse-core --example bench -- ./sample/sample.parquet
```

Three decisions give these times.

**Peruse reads only the rows that it shows.** A page is a `LIMIT` and an
`OFFSET` against the file. The cost therefore follows the size of your
terminal, and not the size of the file. A count of a Parquet file comes from
the footer.

**The user interface thread never blocks.** The engine runs on its own thread.
A group of scroll events becomes one request for the newest page. Each response
carries an epoch, so a slow count from a filter that you already changed cannot
replace the current count. The key `Esc` stops a query that runs now.

**Peruse indexes a file of text.** See
[Indexing a file of text](#indexing-a-file-of-text).

---

## How it is built

```
crates/peruse-core   the engine, the queries, the filter model, the metadata,
                     the statistics and the themes — no terminal code
crates/peruse-tui    the terminal front end
```

`peruse-core` has no dependency on ratatui and no dependency on crossterm. The
themes are also in that crate. A front end with a graphical user interface can
therefore use the same API, and the engine needs no change.

The engine is DuckDB, compiled into the program file. Peruse builds every
statement from one `View` value: the relation, the filter and the sort. A
filter from the builder and a sort from the key `s` therefore work together,
and the code needs no special case for them.

The directory [`docs/`](docs/README.md) holds one document for each part of the
system: the architecture, the engine, the query generation, the read-only
guard, the worker and the concurrency, the user interface, the keys and the
commands, the themes, and the performance.

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

**The build fails on the first try.**
It is almost always the C++ toolchain. See [Install](#install).

---

Licensed under the Apache License 2.0. See [LICENSE](LICENSE).
