# The engine

The engine is the layer that uses DuckDB. It opens a file, it reads a page of
rows, it counts the rows, it calculates the statistics of a column, it measures
the columns for the detail band, and it reads the metadata. Each function of the
engine blocks the thread that calls it, so the worker calls these functions on a
background thread. The engine never opens a data file for write access. The code
is in `crates/peruse-core/src/engine.rs`.

## The set-up of DuckDB

The engine calls `Connection::open_in_memory`. The database therefore holds no
file on the disk. The function `configure` then sends these settings:

| Setting | Value | Reason |
|---|---|---|
| `autoinstall_known_extensions` | `false` | Peruse must never download an extension. |
| `autoload_known_extensions` | `false` | Peruse must never load one. |
| `threads` | The option `--threads`, or one thread for each core | It controls the parallel work. |
| `enable_progress_bar` | `false` | A progress bar would write to the terminal and break the frame. |
| `preserve_insertion_order` | `true` | The rows must always come back in the order of the file. |
| `temp_directory` | The temporary directory of the system, plus `peruse` | DuckDB writes here when the data does not fit in memory. |
| `memory_limit` | The option `--memory-limit`, or the setting `memory_limit`, or one half of the memory of the machine, and three tenths of a machine of 8 GB or less | It limits the memory before DuckDB writes to the disk. |
| `parquet_metadata_cache` | `true` | DuckDB then reads the footer of a Parquet file one time and keeps the result. |

The two extension settings are necessary for the read-only promise. The bundled
DuckDB build sets them to `true`. A statement that names an `https://` path or
an `s3://` path is a legal read, and the guard correctly accepts it. DuckDB
would then download the `httpfs` extension, write about 28 MB into the home
directory of the user, load that machine code, and send a request to the
network. With the two settings at `false`, the same statement stops with the
message "Missing Extension Error".

The metadata cache is safe for a file that changes on the disk. DuckDB keeps the
size and the time of the last change with the footer, and it reads the footer
again when either one changes. With the cache, a page request costs 8
milliseconds and not 12, and a count costs 1 and not 2. The setting arrived in a
later version of DuckDB than the oldest one that Peruse builds against, so a
failure there is not an error.

## How Peruse opens a file

The function `Engine::open` does these steps:

1. It starts DuckDB and applies the options.
2. It finds the files. The function `resolve_files` accepts one path, or it
   expands a glob pattern with the DuckDB function `glob()`. The set of files
   therefore agrees with the set that `read_parquet` reads.
3. It finds the format with `source::detect`.
4. It refuses a SQLite file, a glob of databases and an Arrow IPC file, each
   with a message that says what to do.
5. For a DuckDB database, it goes to `Engine::open_database`. See
   [A database file](#a-database-file).
6. For a JSON file, it counts the levels of the file first. See
   [A file that nests too deep](#a-file-that-nests-too-deep).
7. It builds the read expression with `build_read_expr`.
8. It runs `CREATE OR REPLACE VIEW src AS SELECT * FROM <read expression>`.
9. It reads the schema, and it keeps that schema. An error therefore comes
   here, and not at the first scroll.
10. For a CSV file or a JSON file of one file, it writes the columns into the
    read call. See [One examination of a file of
    text](#one-examination-of-a-file-of-text).

The engine changes each backslash in a path to a forward slash. DuckDB accepts
a forward slash on each platform. In a glob pattern, DuckDB reads a backslash
as an escape character.

### The formats

| Format | Extensions | What the engine does |
|---|---|---|
| Parquet | `.parquet` `.parq` `.pq` | `read_parquet([...])` |
| CSV and friends | `.csv` `.tsv` `.tab` `.psv` | `read_csv([...], auto_detect = true)` |
| JSON | `.json` `.ndjson` `.jsonl` | `read_json_auto([...], maximum_depth = 128)` |
| DuckDB database | `.duckdb` `.ddb` | `ATTACH … (READ_ONLY)`, and a view over one table |
| Arrow IPC | `.arrow` `.ipc` `.feather` `.arrows` | It refuses the file and says how to change it |
| SQLite database | any name | It refuses the file and says how to write a table out |

Add `.gz`, `.zst` or `.bz2` to any text format.

The function `source::detect` uses four tests, in this order:

1. The first bytes, for a database file.
2. The extension of the file.
3. The first bytes, for Parquet, Arrow or JSON.
4. CSV, as the last choice.

A database comes first, because a database can carry any name. A file `sales.db`
that holds a SQLite database therefore gets a message about SQLite, and not a
parse failure from the CSV reader. The marks are `DUCK` after the eight bytes of
the checksum for DuckDB, and `SQLite format 3` and one zero byte for SQLite.
For the third test the marks are `PAR1` for Parquet, `ARROW1` for Arrow, and `{`
or `[` for JSON, after any spaces.

The function reads the head of the file with a limit and `read_to_end`, and not
with one call to `read`. One call can give fewer bytes than the caller asked
for, on a network share or on a mounted file system, and a database would then
go to the CSV reader.

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

### A file that nests too deep

The JSON reader of DuckDB is a C++ function that calls itself one time for each
level. A file that nests a thousand levels deep therefore fills the stack of the
thread and stops the whole program, and Rust cannot catch that fault.

Two rules stop it:

- The function `source::json_depth_over` counts the levels with a loop and a
  number, so it can never fill the stack itself. It stops at the first level
  above 128, so a file that is made to be deep costs almost nothing to refuse.
  It reads 8 MB of the file at the most: a file that nests too deep does so at
  its start, because each level is one character.
- The read call also holds `maximum_depth = 128`. The reader then stops at that
  level and gives the value below it as text.

## One examination of a file of text

A Parquet file holds its schema. A CSV file and a JSON file hold none, so DuckDB
examines the file to find the delimiter, the quote character and the column
types, and it does that again for every statement. That examination is the slow
part of each request. On a CSV file of 258 MB, one page of 50 rows cost 100
milliseconds, and 92 of those were the examination.

The engine therefore examines the file one time and writes the answer into the
read call:

| Function | What it does |
|---|---|
| `Engine::pin_csv` | Calls the DuckDB function `sniff_csv` one time, and writes the dialect and the columns into a `read_csv` call with `auto_detect = false`. |
| `Engine::pin_json` | Reads the columns with the plain `read_json_auto` call, and writes them into a `read_json` call with a `columns` list. |

The same page then costs 8 milliseconds. Two more gains come from that one call:

- The metadata panel needs no second call to `sniff_csv`. The engine keeps the
  dialect in `csv_dialect`, and that call cost 50 milliseconds on a file of 258
  MB.
- The open operation reads the schema one time and keeps it in `base_schema`.
  One `DESCRIBE` of a file with 1000 columns cost four seconds, and the engine
  made that call for each request. A filter and a sort do not change the
  columns, so this one copy serves each view that reads the file itself. A view
  that holds a statement of the user has its own columns, and it reads them each
  time.

Both functions test their own work. `pin_csv` gives `None` when the sniffer
fails, when the new call fails, or when the new call gives columns that are not
the columns of the sniffer. `pin_json` points the view back at the call that
worked in the same cases. The engine then uses the call with `auto_detect`,
which is the call that Peruse used before this step existed.

A set of files keeps the call with `auto_detect`. A set needs `union_by_name`,
and two files of one set can hold different columns, so one column list cannot
serve them all.

### The two read expressions

The engine holds two texts:

| Field | Contents | Who reads it |
|---|---|---|
| `scan_expr` | The call that the view `src` uses. It can hold the full list of columns. | DuckDB |
| `read_expr` | The short call, which finds the columns for itself. | The metadata panel |

The pinned call runs to some kilobytes on a wide file of text. A panel cannot
show such a text, and a redraw must not wrap it. The two calls read the same
rows, because Peruse takes the columns of `scan_expr` from that same reader.

## A database file

A DuckDB database file is the one source that DuckDB itself opens. The function
`Engine::open_database` runs two statements:

```sql
ATTACH 'C:/data/shop.duckdb' AS "__peruse_db" (READ_ONLY);
CREATE OR REPLACE VIEW src AS SELECT * FROM "__peruse_db"."main"."sales";
```

From there, the pages, the filter, the sort, the search, the statistics, the
detail band, the record view and `--ddl` all read the view `src`, and none of
them knows what the view holds.

The flag `READ_ONLY` makes the promise of Peruse stronger here than anywhere
else. The storage engine of DuckDB refuses each write to the file, so the
promise does not rest on the guard over the words of a statement. The test
`the_read_only_flag_stops_a_write_to_the_database` writes through the connection,
past the guard, and the database refuses it.

The guard still refuses a typed `ATTACH`, a `DETACH`, an `INSTALL` and a `LOAD`,
so a user cannot attach a second database and write to that one. See
[read-only-guard.md](read-only-guard.md).

### Which table

A database holds many tables, and the grid shows one of them. The function
`read_tables` asks the catalog, and it scans no table:

```sql
SELECT schema_name, table_name, false, estimated_size::BIGINT
  FROM duckdb_tables() WHERE database_name = '__peruse_db' AND NOT internal
UNION ALL
SELECT schema_name, view_name, true, NULL::BIGINT
  FROM duckdb_views()  WHERE database_name = '__peruse_db' AND NOT internal
ORDER BY 3, 1, 2;
```

The function `choose_table` then picks one:

- The option `--table` names it, as `sales` or as `main.sales`. DuckDB reads an
  identifier without regard to the case of its letters, and so does this test.
- A database with one table needs no option.
- A name that two schemas both hold gives a message that asks for the schema.
- A database with no table and no view says so.

The function `database_tables` runs the ATTACH and the catalog query on a
connection of its own, so the table picker can list the tables before the engine
opens the file. It takes the same `OpenOptions`, so the threads and the memory
limit of the user hold there too. The picker is in
[chooser.md](chooser.md).

A database that a newer DuckDB wrote cannot attach. The function
`attach_message` reads the words of DuckDB and says which program needs the
change, because the words of DuckDB name a storage version and say nothing about
Peruse. A file of an older version also fails to attach, and a newer Peruse
would not read it either, so the words of DuckDB then stand alone.

### The read expression of a database

```text
ATTACH 'C:/data/shop.duckdb' AS "__peruse_db" (READ_ONLY); FROM "__peruse_db"."main"."sales"
```

The metadata panel shows that text. It is short, it names the true alias, and a
user can paste it into the DuckDB command line. The SQL prompt can also join a
second table of the same file through that alias:

```sql
SELECT o.id, c.name FROM src o JOIN __peruse_db.main.customers c ON c.id = o.customer
```

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
`Engine::is_seekable` then gives `true`. A Parquet file, an Arrow file and a
table of a database always give `true`: each of the three holds its rows in
blocks, and each block knows its own count.

Two rules protect the user:

- Peruse does not write to the file of the user. The table is in memory, and
  DuckDB writes the remainder to its temporary directory.
- Peruse indexes a file of text at the open operation only below 64 MB and with
  256 columns or fewer. Above either limit, the footer shows a note and Peruse
  waits for the key `I`. See [performance.md](performance.md).

A second call to `materialize` makes no change.

## The metadata

The function `Engine::file_meta` gives the metadata of the file. For a Parquet
file, the facts come from these DuckDB functions:

| Function | Facts |
|---|---|
| `parquet_file_metadata` | The writer, the format version, the row count and the row group count |
| `parquet_metadata` | The codecs, the sizes of the row groups, and the size and the NULL count of each column |
| `parquet_kv_metadata` | The key/value pairs in the footer |

The facts of each column come first, from `Engine::parquet_column_stats`. The
sizes of the whole file are the sums of those facts, and the encodings of the
file are the encodings of its columns, so the panel makes two statements fewer
over the footer.

Each fact comes from the footer, so the size of the file does not change the
cost. The NULL counts of the row groups add together, so a Parquet file gives a
NULL count for each column with no scan.

The engine does not combine the minimum and the maximum from the footer. The
footer keeps them as text, so a comparison across two row groups compares `"9"`
with `"10"` and gives a wrong result. The statistics panel calculates the true
minimum and maximum with a query that keeps the type of the column.

The keys and the values of the pairs have the type BLOB. The engine reads them
as UTF-8 text with `decode()`, and it keeps the first 2000 characters. An
embedded schema from pandas or from Arrow can be tens of kilobytes long.

For a CSV file, the engine gives the dialect that the open operation found:
the delimiter, the quote character, the escape character, the line end, the
header row and the date formats. A file that the engine did not pin gets a call
to `sniff_csv` here. A failure of the sniffer does not remove the full panel,
because the other metadata is still useful.

A JSON file, an Arrow file and a database hold no footer and no dialect. The
panel then shows the sizes, the columns and the types. For a database it also
shows the two statements that open the table.

## The statistics of a column

The function `Engine::column_stats` reads the statistics in one statement. It
then reads the most frequent values, and it reads a histogram for a column of
numbers. Three rules keep the results correct and quick:

- The function `approx_count_distinct` gives an estimate, and the estimate can
  be too large on a small input. The engine therefore limits the estimate to
  the number of rows that are not NULL.
- The engine gives no histogram when each value is NULL, when the column holds
  one value only, or when a value is not finite. Each of these columns would
  give a bucket width of zero or a bucket width that is not a number.
- Above `TOP_VALUES_MIN_ROWS` = 100,000 rows, the engine leaves out the most
  frequent values of a column where almost each value is different. That query
  groups every row of the view, and it is the slow part of the panel: on ten
  million rows it costs 300 milliseconds, and each other query of the panel
  costs 30. A list of values that each occur one time says nothing in any case.

## The facts of the detail band

The function `Engine::column_band` measures each column that the grid draws, in
one statement. The statement holds four values for each column: the count of the
rows that are not NULL, the count of the different values, the smallest value
and the largest value. See `View::band_sql` in
[query-generation.md](query-generation.md).

The function `engine::footer_briefs` gives the same facts from a Parquet footer,
with no query. It answers the compact band only, because the footer holds the
NULL count and not the count of the different values. It gives `None` for a
column that the footer cannot name: the footer names the leaves of a structure,
such as `actor.login`, and not `actor`.

## The cancellation

The function `Engine::interrupt_handle` gives a handle. Another thread can hold
that handle and stop the query that runs now. The key `Esc` uses it.
