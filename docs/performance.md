# The performance

Peruse opens a file of 10 million rows in some milliseconds, and it draws the
first screen immediately after that. The times do not come from a fast loop.
They come from four decisions: Peruse reads only the rows that it shows, the
database does the work of the format, the engine runs on its own thread, and
each request has a limit. This document gives the measured times and the
decision behind each one.

## The measured times

These times come from a file of 10 million rows and 9 columns, with a release
build, on a usual laptop:

| Operation | 67 MB Parquet | 1.27 GB CSV |
|---|---|---|
| open, until the schema arrives | 16 ms | 131 ms |
| the first screen | 20 ms | 89 ms |
| `count(*)` | 5 ms | 437 ms |
| move to the last row | 22 ms | 4 ms *(after the index)* |
| filter and count again | 21 / 18 ms | 5 / 3 ms |
| sort on a column | 34 ms | 11 ms |
| column statistics and histogram | 247 ms | 214 ms |
| search each column | 225 ms | 226 ms |

To make the sample file and to measure the times again, run these two commands:

```sh
cargo run --release -p peruse-core --example make-sample -- ./sample 10000000
cargo run --release -p peruse-core --example bench -- ./sample/sample.parquet
```

The example `bench` measures each operation that a user waits for.

## Decision 1: read only the rows on the screen

A page is a `LIMIT` and an `OFFSET` against the file. The cost therefore
follows the height of the terminal, and not the size of the file. A file of 50
GB and a file of 5 MB give the same time for one page.

The state `App` also asks for 250 rows before the first row of the viewport and
250 rows after the last row. A usual scroll therefore needs no request, and the
grid shows the new rows with no wait.

The grid draws one character for each screen position. The cost of one frame
follows the number of cells on the screen, and not the size of the file.

## Decision 2: read the metadata, and not the data

DuckDB reads `count(*)` of a Parquet file directly from the footer. The count
of 10 million rows therefore needs 5 ms, and it needs no scan.

The metadata panel reads the footer only. The row count, the row group count,
the sizes, the codecs, the encodings and the NULL count of each column all come
from that one read. The panel therefore costs almost no time on a large file.

A CSV file has no footer. A count of a CSV file needs a full scan, and that
scan needs 437 ms in the measurement above.

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

## Decision 4: keep the user interface thread free

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

## Decision 5: limit each request

Two constants keep each request short:

| Constant | Value | Reason |
|---|---|---|
| `SEARCH_CHUNK` | 250,000 rows | The worker answers quickly, so the key `Esc` can stop the search. The number is also large, so a full file does not need thousands of requests. |
| `SEARCH_HITS` | 500 offsets | One part of the view gives 500 matches at the most. |

The search also has a design reason. The row numbers must agree with the
numbers of a page, so the statement needs `row_number()`. A window over the
full table would number each row before the database could report the first
match. On ten million rows, the user would wait some seconds and see nothing.
The statement therefore numbers one part of the view, and the caller moves away
from the cursor one part at a time.

## Decision 6: index a CSV file

A CSV file is a stream. To read row 8,000,000, DuckDB must parse each row
before it. A move to the end of a large file is therefore too slow to use.

The function `Engine::materialize` copies the file into a table in memory.
After that operation, a move to the last row needs 4 ms.

Peruse indexes a file below 256 MB when it opens the file. That scan is quick,
and the user does not notice it. For a larger file, the footer shows a note,
and Peruse waits for the key `I`. The user therefore never waits for a scan
that the user did not ask for.

Peruse does not write to the file of the user. The table is in memory, and
DuckDB writes the remainder to its temporary directory when the table does not
fit. The index therefore becomes slower, but it does not fail.

## The costs that stay

Two operations need a full scan, and no decision can remove that cost:

- The statistics of a column need 247 ms on 10 million rows. The panel shows
  the text `computing…` until the result arrives.
- A search of the full view needs 225 ms for each part of 250,000 rows. The
  status line shows the percentage, and the key `Esc` stops the search.
