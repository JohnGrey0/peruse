# The architecture of Peruse

Peruse has two crates. The crate `peruse-core` holds the engine and the data
layer, and it has no terminal code. The crate `peruse-tui` holds the terminal
front-end. The program uses three threads: one thread reads the keys, one
thread runs the engine, and the main thread draws the frames. The threads speak
to each other with channels only. The main thread therefore never blocks, and
the grid stays live on a file that is larger than the memory.

## The two crates

The crate `peruse-core` holds these modules:

| Module | Function |
|---|---|
| `engine.rs` | The connection to DuckDB, the open operation, the pages, the counts, the statistics, the metadata and the CSV index |
| `query.rs` | The code that builds SQL from the view |
| `filter.rs` | The list of conditions, and the `WHERE` expression that it compiles to |
| `ddl.rs` | The `CREATE TABLE` statement for another database, and the rules behind it |
| `sql_guard.rs` | The code that rejects a statement that can write |
| `worker.rs` | The background thread, the combination of requests and the epoch |
| `model.rs` | The schema, the families of values, the pages of rows and the row count |
| `meta.rs` | The Parquet footer facts and the CSV dialect |
| `stats.rs` | The statistics of a column and the histogram |
| `source.rs` | The format detection, and the text forms of a size and a count |
| `theme.rs` | The colors, the built-in themes and the theme files |

The crate `peruse-tui` holds these modules:

| Module | Function |
|---|---|
| `main.rs` | The options of the command line, the terminal, and the event loop |
| `app.rs` | The state of the application and each change to that state |
| `grid.rs` | The grid of rows and columns |
| `panels.rs` | The metadata panel and the column statistics panel |
| `overlays.rs` | The help, the palette, the theme picker, the cell inspector, the record view and the filter builder |
| `ui.rs` | The layout of the frame, the title bar, the status line and the footer |
| `commands.rs` | The one table of commands, keys and descriptions |
| `input.rs` | The editor of one line for the prompt |
| `text.rs` | The text functions that know the width of a character |
| `colors.rs`, `paint.rs` | The change from a theme color to a terminal style |
| `sqlhl.rs` | The colors of the SQL prompt |
| `clip.rs` | The clipboard, with the escape sequence OSC 52 |
| `render_test.rs` | The tests of the complete program |

The crate `peruse-core` has no dependency on ratatui and no dependency on
crossterm. The themes are also in that crate. A front-end with a graphical user
interface (GUI) can therefore use the same API.

## The three threads

| Thread | Name | Work |
|---|---|---|
| The main thread | — | It draws the frames and it changes the state. |
| The input thread | `peruse-input` | It reads the terminal events and sends them. |
| The engine thread | `peruse-engine` | It runs the engine and sends the responses. |

A read from the terminal blocks the thread. The input thread therefore does
that read, and it sends each event on a channel. A query can also block for
some seconds. The engine thread therefore does that work.

The main thread waits on the two channels together. It does not examine the
channels again and again, so it uses no processor time between two events.

## The path from a key to a frame

The steps below show what happens after the user presses a key:

1. The input thread reads the event and sends it on the key channel.
2. The main thread receives the event and gives it to `App::on_key`. Peruse
   uses the press of a key only, because Windows also sends the release.
3. The function `on_key` looks at the mode. In the normal mode, the function
   `commands::resolve` finds the command for the key.
4. The function `App::run` does the command. The command can change the cursor,
   or open an overlay, or change the view.
5. A change of the view calls `App::reload`. That function increases the epoch,
   discards the old results, and sends the request `SetView`.
6. The main thread draws the frame. The function `App::ensure_rows` then asks
   for a page when the grid needs rows that the current page does not hold.
7. The worker combines the requests in its queue, does the work, and sends the
   responses.
8. The main thread receives each response and gives it to `App::on_response`.
   That function discards a response with an old epoch.
9. The main thread draws the next frame.

## The state

The structure `App` holds each part of the state. The main thread is the only
thread that touches it. The worker holds the engine, and it sends copies of the
view and the schema with each request. The two threads therefore need no lock.

## Where to read more

- The engine: [engine.md](engine.md)
- The statements: [query-generation.md](query-generation.md)
- The worker: [worker-and-concurrency.md](worker-and-concurrency.md)
- The screen: [user-interface.md](user-interface.md)
