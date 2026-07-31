# The worker and the concurrency

The worker runs the engine on a background thread. The user interface thread
sends a request, and it continues immediately. The answer comes back later as a
response on a second channel. Two rules keep the screen correct and fast: the
worker combines the requests in its queue, and each request carries an epoch.
The code is in `crates/peruse-core/src/worker.rs`.

## The start

The function `Worker::spawn` starts the thread and waits for the schema. It
makes three channels:

| Channel | Direction | Contents |
|---|---|---|
| The request channel | To the worker | The requests |
| The response channel | From the worker | The responses |
| The ready channel | From the worker | The result of the open operation |

The new thread opens the file, reads the schema, and sends the result on the
ready channel. The function then gives a `Worker` and an `Opened`.

To open a file, the engine reads a Parquet footer or asks the sniffer about a
CSV file one time. This work is fast, so the wait is short. A bad path therefore
gives a plain error on the command line. The terminal does not start and then
stop again.

## The requests

| Request | Kind | Work | Responses |
|---|---|---|---|
| `SetView` | 0 | Use a new view | `Schema`, then `Page`, then `Count` |
| `Page` | 1 | Read one page of rows | `Page` |
| `Stats` | 2 | Calculate the statistics of a column | `Stats` |
| `Cell` | 3 | Read one cell in full | `Cell` |
| `Search` | 4 | Find the rows that hold a value | `Search` |
| `Meta` | 5 | Read the metadata of the file | `Meta` |
| `Index` | 6 | Copy a file of text into a table | `Indexed` |
| `RowJson` | 7 | Read one complete row as JSON, for the record view | `RowJson` |
| `Configure` | 8 | Change the settings that DuckDB accepts while it runs | `Configured` |
| `Band` | 9 | Measure each column that the detail band draws | `Band` |
| `Shutdown` | 10 | Stop the thread | None |

The worker also sends `Busy(true)` before a group of requests and `Busy(false)`
after it, and `Error` when one request fails.

The number in the column `Kind` is the value of `Request::kind`. A newer request
of one kind replaces an older request of the same kind, and it replaces no
request of another kind. A new band request therefore never removes the page
request that the grid waits for.

The request `SetView` gives three responses, in this order: the schema, the
first page, and then the count. The count is the one part that can need a full
scan, and the user can use the grid before it arrives.

The request `Band` covers each column that the grid draws, in one request. One
request for each column would read the view again for each column, and the width
of the terminal already bounds the number of columns.

## How the worker combines the requests

The main loop of the worker does these steps:

1. It waits for the first request.
2. It takes each request that is behind the first one in the queue.
3. It calls the function `coalesce` on that group.
4. It sends `Busy(true)`, does the work, and sends `Busy(false)`.

The function `coalesce` uses four rules:

- A shutdown request removes each other request. It carries the largest possible
  epoch, so no other request can remove it.
- The function keeps the requests with the newest epoch only. A request from an
  old view is work with no result, because the user interface discards the
  response.
- The index request is the one exception to that rule. See
  [Work that has no view](#work-that-has-no-view).
- Of each kind, the function keeps the last request only. The newer request
  takes the position of the older one, so the order of the kinds does not
  change.

A key that the user holds down sends one page request for each press. Only the
newest page is useful. The engine therefore never falls behind the cursor.

A dropped request needs its work again. `App::band_asked` therefore holds the
columns of the request that is in flight, and not the columns of every request
of this view. Without that rule, a user who scrolls past the first screen of
columns before the first answer arrives would see a row of points on the first
columns until the view changed.

## The epoch

The state `App` holds a counter, the epoch. The function `App::reload`
increases the counter at each change of the view. Each request carries the
current epoch, and each response carries the epoch of its request.

The function `App::on_response` reads the epoch of the response. If the epoch
is not the current epoch, the function discards the response.

This rule stops a real fault. A count of a filtered CSV file can take some
seconds. The user can change the filter in that time. Without the epoch, the
old count would replace the new count, and the title bar would show a wrong
number of rows.

The state `App` also discards the match offsets of a search at each change of
the view. An offset is a position in the old view, and a new sort makes each
offset wrong. It empties `stats_cache` and `band_cache` for the same reason: the
numbers of those two describe the rows of one view.

## Work that has no view

Two operations describe the file, and not the view: the index and the file
metadata. The epoch must not touch them.

`App::new` sends the index request at the start. A start with `--filter` or
with `--query` then changes the view immediately, and the epoch goes from 1 to
2. Without an exception, two things would go wrong at the same time:

- `coalesce` would remove the index request from the group, so the table would
  never be built.
- `App::on_response` would discard the response `Indexed`, so the value
  `App::indexing` would stay `true` for the whole session. The footer would
  show the note "press I to index" for ever, and the key `I` would answer
  "already indexing" for ever.

Two rules stop this:

- `coalesce` keeps a request `Index` whatever its epoch is.
- `App::on_response` applies the responses `Indexed` and `Meta` in front of the
  test of the epoch.

Peruse asks for the metadata one time in a session, and it keeps the answer. The
metadata panel and the compact detail band both read that one answer. A read of
the footer that fails is not final: `App::after_panel_change` clears the latch
when the last request failed, so the key `m` gives a second try. A file that was
truncated or that changed on the disk therefore needs no restart.

## The cancellation

The handle from `Engine::interrupt_handle` stops the query that runs now.
Another thread can hold this handle and use it safely.

The function `Worker::cancel` calls that handle. The key `Esc` starts the
command `Cancel`, and that command calls `Worker::cancel` when the worker is
busy. The request that stops gives an error, and the user interface shows a
message about the cancellation.

The search also uses this rule. The first search request examines 250,000 rows
at the most, and each request after it examines two times the rows of the one
before, up to 4 million. The worker therefore answers quickly at the start, and
the key `Esc` can stop the search between two parts. See
[performance.md](performance.md).

## The errors

A failure of one request does not stop the worker. The function `fail` sends a
response `Error` with a context and a message, and the loop continues. A bad
filter expression is a usual event, and the previous view stays on the screen.

The user interface keeps the first line of the message. DuckDB adds the full
statement after it, and that text is too long for a status line of one row.

## The end

The `Drop` code of the `Worker` does these steps:

1. It stops the query that runs now. Without this step, the join below would
   wait for the end of a long query.
2. It sends the request `Shutdown`.
3. It waits for the end of the thread.
