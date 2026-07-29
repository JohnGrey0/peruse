# The engine

The engine is the layer that uses DuckDB. It opens a file, it reads a page of
rows, it counts the rows, it calculates the statistics of a column, and it
reads the metadata. Each function of the engine blocks the thread that calls
it, so the worker calls these functions on a background thread. The engine
never opens a data file for write access. The code is in
`crates/peruse-core/src/engine.rs`.

## The set-up of DuckDB

The engine calls `Connection::open_in_memory`. The database therefore holds no
file on the disk. The function `configure` then sends these settings:

| Setting | Value | Reason |
|---|---|---|
| `threads` | The option `--threads`, or one thread for each core | It controls the parallel work. |
| `enable_progress_bar` | `false` | A progress bar would write to the terminal and break the frame. |
| `preserve_insertion_order` | `true` | The rows must always come back in the order of the file. |
| `temp_directory` | The temporary directory of the system, plus `peruse` | DuckDB writes here when the data does not fit in memory. |
| `memory_limit` | The option `--memory-limit` | It limits the memory before DuckDB writes to the disk. |

## How Peruse opens a file

The function `Engine::open` does these steps:

1. It starts DuckDB and applies the options.
2. It finds the files. The function `resolve_files` accepts one path, or it
   expands a glob pattern with the DuckDB function `glob()`. The set of files
   therefore agrees with the set that `read_parquet` reads.
3. It finds the format with `source::detect`. That function looks at the
   extension, then at the first bytes, and then it selects CSV.
4. It rejects an Arrow IPC file with a message that says what to do. This
   build of DuckDB holds no reader that takes the name of such a file.
5. It builds the read expression with `build_read_expr`.
6. It runs `CREATE OR REPLACE VIEW src AS SELECT * FROM <read expression>`.
7. It reads the schema. An error therefore comes here, and not at the first
   scroll.

The engine changes each backslash in a path to a forward slash. DuckDB accepts
a forward slash on each platform. In a glob pattern, DuckDB reads a backslash
as an escape character.

### The read expression

### The formats

| Format | Extensions | Read expression |
|---|---|---|
| Parquet | `.parquet` `.parq` `.pq` | `read_parquet([...])` |
| CSV and friends | `.csv` `.tsv` `.tab` `.psv` | `read_csv([...], auto_detect = true)` |
| JSON | `.json` `.ndjson` `.jsonl` | `read_json_auto([...])` |

The function `source::detect` finds the format in three steps: the extension of
the file, then the first bytes of the file, then CSV as the last choice. The
first bytes are `PAR1` for Parquet, `ARROW1` for Arrow, and `{` or `[` for
JSON, after any spaces.

Peruse knows the Arrow IPC format, and it refuses it with a message. The
functions `arrow_scan` in this build take a pointer to data that is in memory
already, and not the name of a file. A message that names such a function would
tell the user nothing, so `Engine::open` gives its own message and names a way
to change the file to Parquet.

For a CSV file, the expression takes more options:

- `delim`, when the user or the file extension gives a delimiter
- `header`, when the user gives the option `--no-header`
- `all_varchar`, `ignore_errors` and `sample_size`, from the options
- `union_by_name`, when the source holds more than one file

The option `union_by_name` joins the files by the names of the columns. The
files of one set often hold their columns in a different order.

A tab needs a special form. The function `read_csv` reads an escape sequence in
the option `delim`, so the engine writes a tab as `'\t'`.

## The pages of rows

The function `Engine::page` reads `limit` rows that start at the row `offset`.
The statement uses `LIMIT` and `OFFSET`. The cost therefore follows the height
of the terminal, and not the size of the file.

DuckDB changes each value into text with `CAST(… AS VARCHAR)`. Peruse does not
format the values in Rust code, for two reasons:

- Each type looks the same as it looks in DuckDB. A decimal, an interval, an
  enumeration and a nested structure all work with no special code.
- The cost is small, because the database does the work.

The function `substr` cuts each value at 4096 characters. One JSON value of 40
megabytes can therefore not stop a redraw. For a BLOB column, the statement
gives the size only, and not the bytes. The cell inspector asks for the
complete value of one cell with `Engine::cell`.

## The counts

The function `Engine::count` runs `SELECT count(*)`. The statement has no
`ORDER BY` part, because a sort cannot change a count.

DuckDB reads `count(*)` of a Parquet file directly from the footer. A view with
no filter therefore needs no scan. A view with a filter needs a full scan, and
that scan can take some seconds on a large CSV file. The worker therefore sends
the count last, after the schema and the first page.

## The index of a file of text

A CSV file and a JSON file are streams. To read row 8,000,000, DuckDB must
parse each row before it. A move to the end of a large file is therefore too
slow to use.

The function `Engine::materialize` copies the file into a table:

```sql
CREATE OR REPLACE TABLE __peruse_indexed AS SELECT * FROM read_csv(...);
CREATE OR REPLACE VIEW src AS SELECT * FROM __peruse_indexed;
```

After this operation, the engine can go directly to any row. The function
`Engine::is_seekable` then gives `true`. A Parquet file always gives `true`,
because it holds its rows in row groups and each row group knows its own
count.

Two rules protect the user:

- Peruse does not write to the file of the user. The table is in memory, and
  DuckDB writes the remainder to its temporary directory.
- Peruse indexes a file below 256 MB when it opens the file. For a larger file,
  the footer shows a note, and Peruse waits for the key `I`.

A second call to `materialize` makes no change.

## The metadata

The function `Engine::file_meta` gives the metadata of the file. For a Parquet
file, the facts come from these DuckDB functions:

| Function | Facts |
|---|---|
| `parquet_file_metadata` | The writer, the format version, the row count and the row group count |
| `parquet_metadata` | The sizes, the codecs, the encodings and the NULL count of each column |
| `parquet_kv_metadata` | The key/value pairs in the footer |

Each fact comes from the footer, so the size of the file does not change the
cost. The NULL counts of the row groups add together correctly, so a Parquet
file gives an exact NULL count for each column with no scan.

The engine does not combine the minimum and the maximum from the footer. The
footer keeps them as text, so a comparison across two row groups compares `"9"`
with `"10"` and gives a wrong result. The column inspector calculates the true
minimum and maximum with a query that keeps the type of the column.

The keys and the values of the pairs have the type BLOB. The engine reads them
as UTF-8 text with `decode()`, and it keeps the first 2000 characters. An
embedded schema from pandas or from Arrow can be tens of kilobytes long.

For a CSV file, the engine calls `sniff_csv`. That function gives the
delimiter, the quote character, the escape character, the line end, the header
row and the date formats. A failure of the sniffer does not remove the full
panel, because the other metadata is still useful.

## The statistics of a column

The function `Engine::column_stats` reads the statistics in one statement. It
then reads the most frequent values, and it reads a histogram for a column of
numbers. Two rules keep the results correct:

- The function `approx_count_distinct` gives an estimate, and the estimate can
  be too large on a small input. The engine therefore limits the estimate to
  the number of rows that are not NULL.
- The engine gives no histogram when each value is NULL, when the column holds
  one value only, or when a value is not finite. Each of these columns would
  give a bucket width of zero or a bucket width that is not a number.

## The cancellation

The function `Engine::interrupt_handle` gives a handle. Another thread can hold
that handle and stop the query that runs now. The key `Esc` uses it.
