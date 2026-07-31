# The performance

Peruse opens a file of 10 million rows in some milliseconds, and it draws the
first screen immediately after that. The times do not come from a fast loop.
They come from eight decisions: Peruse reads only the rows that it shows, it
reads the metadata and not the data, the database does the work of the format,
Peruse examines a file of text one time and not at each statement, the engine
runs on its own thread, each request has a limit and starts small, the detail
band reads a footer before it reads the rows, and Peruse indexes a small file of
text. This document gives the measured times and the decision behind each one.

## The measured times

These times come from a file of 10 million rows and 9 columns, with a release
build, on a desktop with 16 cores and 93 GB of memory:

| Operation | 67 MB Parquet | 1.27 GB CSV |
|---|---|---|
| open, until the schema arrives | 13.7 ms | 55.5 ms |
| the first screen, 50 rows | 15.1 ms | 10.3 ms |
| `count(*)` | 2.6 ms | 373.5 ms |
| index the file of text | - | 1389.0 ms |
| move to the last screen | 17.0 ms | 6.5 ms *(after the index)* |
| filter, and count the filtered rows | 15.3 / 14.3 ms | 4.8 / 2.5 ms |
| sort on a column | 25.9 ms | 7.6 ms |
| statistics of a column of numbers | 42.4 ms | 23.4 ms |
| statistics of a column of text | 28.8 ms | 20.5 ms |
| search each column | 15.0 ms | 7.4 ms |
| the metadata panel | 35.5 ms | 0.1 ms |

A machine with fewer cores gives larger numbers. Read the table as a comparison
between the operations, and not as a promise about your machine.

To make the sample file and to measure the times again, run these two commands:

```sh
cargo run --release -p peruse-core --example make-sample -- ./sample 10000000
cargo run --release -p peruse-core --example bench -- ./sample/sample.parquet
```

The example `bench` measures each operation that a user waits for. Give it
`./sample/sample.csv` for the second column of the table.

## Decision 1: read only the rows on the screen

A page is a `LIMIT` and an `OFFSET` against the file. The cost therefore
follows the height of the terminal, and not the size of the file. A file of 50
GB and a file of 5 MB give the same time for one page.

The state `App` also asks for 250 rows before the first row of the viewport and
250 rows after the last row. A usual scroll therefore needs no request, and the
grid shows the new rows with no wait.

The grid draws one character for each screen position. The cost of one frame
follows the number of cells on the screen, and not the size of the file.

The detail band under the column names follows the same rule. One request
covers each column that the grid draws, and the width of the terminal bounds
that number. See [Decision 7](#decision-7-the-detail-band-reads-the-footer-first).

## Decision 2: read the metadata, and not the data

DuckDB reads `count(*)` of a Parquet file directly from the footer. The count
of 10 million rows therefore needs 2.6 ms, and it needs no scan.

The metadata panel reads the footer only. The row count, the row group count,
the sizes, the codecs, the encodings and the NULL count of each column all come
from that one read. The panel therefore costs almost no time on a large file.

The facts of each column come first, and the sizes of the whole file and the
list of the encodings are the sums of those facts. The panel therefore makes
two statements fewer over the footer than it made before.

A CSV file has no footer. A count of a CSV file needs a full scan, and that
scan needs 373 ms in the measurement above.

## Decision 3: give the format work to the database

DuckDB changes each value into text with `CAST(… AS VARCHAR)`. Rust code could
format the values, but then Peruse would need code for each type. The cast also
runs in parallel inside the database.

Two limits control the quantity of data that moves to the grid:

- The function `substr` cuts each value at 4096 characters. A wide column of
  text therefore costs the same as a narrow one. The cell inspector asks for
  the complete value of one cell.
- A BLOB column gives its size only, and not its bytes.

The search uses the same rule. A text column needs no cast, because a cast
would copy each value for no gain.

## Decision 4: examine a file of text one time

A Parquet file holds its schema. A CSV file and a JSON file hold none, so
DuckDB examines the file to find the delimiter and the columns, and it does
that again for every statement. That examination is the slow part of each
request. On a CSV file of 258 MB, one page of 50 rows cost 100 milliseconds,
and 92 of those were the examination.

The engine now asks the sniffer one time, at the open operation, and writes the
answer into the read call:

```sql
read_csv(['big.csv'], auto_detect = false,
         columns = {'id': 'BIGINT', 'name': 'VARCHAR', …},
         header = true, skip = 0, delim = ',', quote = '"', …)
```

Each later statement reads the file directly, and the same page costs 8
milliseconds. The functions are `Engine::pin_csv` and `Engine::pin_json`. See
[engine.md](engine.md).

Two more gains come from that one call:

- **The metadata panel needs no second sniffer call.** The engine keeps the
  dialect from the open operation. That call cost 50 milliseconds on a file of
  258 MB, and the panel now costs 0.1 ms.
- **A wide file of text opens.** One `DESCRIBE` of a file with 1000 columns
  cost four seconds, and the engine made that call for each request. The open
  operation reads the schema one time and keeps it in `base_schema`.

A set of files keeps the call with `auto_detect`. A set needs `union_by_name`,
and two files of one set can hold different columns, so one column list cannot
serve them all.

## Decision 5: keep the user interface thread free

The engine runs on its own thread. The user interface thread sends a request
and continues immediately. Three rules keep the screen correct:

- **The worker combines the requests.** A key that the user holds down sends
  one page request for each press. The worker keeps the newest request of each
  kind, so the engine never falls behind the cursor.
- **Each response carries an epoch.** The user interface discards a response
  from an old view. A slow count from a filter that the user changed can
  therefore not replace the current count.
- **The user can stop a query.** The key `Esc` stops the query that runs now.

The request `SetView` gives the schema first, then the first page, and then the
count. The count is the one part that can need a full scan, and the user can
use the grid before it arrives.

## Decision 6: limit each request, and start small

Three constants keep each request short:

| Constant | Value | Reason |
|---|---|---|
| `SEARCH_CHUNK` | 250,000 rows | The number of rows of the first search request. It is small, so the first answer arrives at once and the key `Esc` can stop a search that finds nothing. |
| `SEARCH_CHUNK_MAX` | 4,000,000 rows | Each request after the first covers two times the rows of the one before, up to this limit. |
| `SEARCH_HITS` | 500 offsets | One part of the view gives 500 matches at the most. |

The parts double in size for a reason. Each request reads the view from its
start and then skips to its part, so a request that starts late costs more than
a request that starts early. A search of ten million rows in parts of 250,000
reads the file forty times, and the cost then grows with the square of the size.
With parts that double, the number of parts falls from forty to about six, and
the first part is still small.

Inside one request, the engine reads a window of `SEARCH_WINDOW` = 8192 rows in
front of the cursor first. The statement holds `ORDER BY off`, because the
caller needs the matches in the order of the view, so the database must read
each row of the part before it gives the first match. One search over 250,000
rows costs 90 milliseconds, and the same search over a window of 8192 rows
costs 2 milliseconds. A match near the cursor is the usual case, and the
remainder of the part then stays unread.

A search that finds nothing in the window costs one more statement for the
remainder, so it reads 3 percent more rows than it read before. A sorted view
reads its part in one statement, because each window would need its own sort of
the whole view.

The search also has a design reason for `row_number()`. The row numbers must
agree with the numbers of a page, so the statement needs that function. A
window over the full table would number each row before the database could
report the first match. On ten million rows, the user would wait some seconds
and see nothing.

## Decision 7: the detail band reads the footer first

The key `d` puts a band of facts under the column names. The compact band shows
the type and the share of NULL values of each column. A Parquet footer holds
the row count and the NULL count of each column already, so the compact band
over a plain Parquet file runs **no query at all**. The cost then follows the
number of row groups, and not the number of rows.

The band asks the engine in five cases: a source that is not a plain Parquet
file, such as a file of text or a database; a filtered view; a view that holds a
statement of the user; a column that the footer cannot name, such as a structure;
and the detailed mode. The function `footer_can_answer` holds the first three
tests. One statement then measures each column that the grid draws.

These times come from the same machine, with the ignored test
`the_cost_of_the_band` in `engine.rs`. That test prints times instead of
asserting:

```sh
PERUSE_BAND_FILE=sample/sample.parquet cargo test --release -p peruse-core \
    the_cost_of_the_band -- --ignored --nocapture
```

| File | Rows | Compact, from the footer | The band query, 9 columns | The statistics panel, 1 column |
|---|---|---|---|---|
| 13 MB Parquet | 200,000 | 16.4 ms | 10.4 ms | 8.2 ms |
| 14 MB Parquet | 2 million | 21.0 ms | 44.4 ms | 22.3 ms |
| 141 MB Parquet | 20 million | 62.0 ms | 222.5 ms | 77.3 ms |
| 270 MB CSV | 2 million | not possible | 99.2 ms | 162.7 ms |
| 2.7 GB CSV | 20 million | not possible | 794.6 ms | 1602.8 ms |

The footer path costs one read of the footer, which the metadata panel shares
and Peruse asks for one time in a session. The query path grows with the number
of rows.

The band also gives its rows back to the data on a short terminal. A grid with
no room for a band row makes no request, because a query for facts that nothing
draws reads the whole file for nothing.

## Decision 8: index a file of text

A CSV file is a stream. To read row 8,000,000, DuckDB must parse each row
before it. A move to the end of a large file is therefore too slow to use.

The function `Engine::materialize` copies the file into a table in memory.
After that operation, a move to the last screen needs 6.5 ms.

Peruse indexes a file of text when it opens the file, and two limits control
that choice:

| Constant | Value | Reason |
|---|---|---|
| `AUTO_INDEX_BYTES` | 64 MB | The index holds the file in memory, and it takes about one and a third times the size of the file. This limit therefore also limits what Peruse spends without asking: about 85 MB. |
| `AUTO_INDEX_COLUMNS` | 256 columns | The size in bytes is the wrong measure by itself. A file of 170 MB with 10,000 columns is below the limit above, and the index of it costs 21 seconds and 2.7 GB. |

Above either limit, the footer shows a note and Peruse waits for the key `I`.
The user therefore never waits for a scan that the user did not ask for.

Peruse does not write to the file of the user. The table is in memory, and
DuckDB writes the remainder to its temporary directory when the table does not
fit. The index therefore becomes slower, but it does not fail.

A table of a DuckDB database needs no index. The database holds its rows in
blocks already, so `Engine::is_seekable` gives `true` for it, as it does for a
Parquet file.

## The costs that stay

Two operations need a full scan, and no decision can remove that cost:

- **The statistics of a column.** The panel shows the text `computing…` until
  the result arrives. Above 100,000 rows the engine leaves out the most
  frequent values of a column where almost each value is different, because
  that query groups every row of the view and it is the slow part of the panel.
- **A search that finds nothing.** The search then reads each part of the view.
  The status line shows the percentage, and the key `Esc` stops the search.
