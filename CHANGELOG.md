# Changes

The versions follow [Semantic Versioning](https://semver.org). Peruse is
below version 1.0, so a minor version can still change the keys or the
settings. Each such change is in this file.

## Unreleased

## 0.1.0

The first release.

### The grid

- Reads one page of rows at a time, so a file larger than the memory opens
  in the time that a key press takes.
- A colour for each family of values, and a NULL that does not look like an
  empty text.
- Sort on a column, search each column, hide a column, fit the widths.
- Copy a cell or a row, through OSC 52, so it works over SSH.

### The record view (`r`)

- One row from the top to the bottom, one column on each line. A file with
  300 columns therefore needs no 300 presses of a key.
- A value that holds other values opens: a structure and a list both drill
  down, at any depth.
- A find box that reaches a field three levels down and opens the way to it.
- Filter on a value inside a structure, from its path.

### The filter

- A builder (`f`) that asks for a column, an operator and a value. No SQL.
- A `WHERE` prompt (`E`) for a user who prefers to type.
- `=` and `!` filter on the value under the cursor.
- The three build one list of conditions, so they work together.
- `u` and `U` go back and forward through the filters, the sorts and the
  statements.

### Reading only

- Two layers stop a write. The engine holds no connection that can write,
  and a guard refuses each statement from a user that could change anything.
- The guard also refuses a statement that would install an extension or
  reach the network.

### Formats

- Parquet, CSV, TSV, PSV, JSON, NDJSON, and the compressed forms of the text
  ones.
- An Arrow IPC file gives a message that says what to do, because the build
  of DuckDB inside Peruse holds no reader for it.

### The other parts

- A chooser (`peruse` with no file) that lists the data files that are near,
  and the files that the user opened before.
- A settings page (`,`) that keeps the theme, the threads, the memory limit,
  the sample size, the index and the panels. Each change keeps itself.
- The metadata panel and the column statistics, one above the other, and a
  metadata panel that opens the fields of a structure.
- `--ddl` writes a `CREATE TABLE` statement for Oracle, MySQL, PostgreSQL,
  Snowflake, BigQuery, SQL Server, DuckDB or DynamoDB, with a primary key and
  index candidates from the data itself.
- Nine themes, and themes of your own from TOML files.
- The rest of a name after the cursor, in each prompt that has a known list
  of answers.
