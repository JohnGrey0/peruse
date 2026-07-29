# peruse-core

The engine behind [Peruse](https://github.com/JohnGrey0/peruse), a fast
read-only viewer for Parquet, CSV, TSV and JSON files.

This crate holds the parts that have nothing to do with a terminal:

| Module | What it does |
|---|---|
| `engine` | Opens a file with DuckDB, reads pages, counts rows, builds the index |
| `query` | Turns one `View` into every statement that Peruse runs |
| `filter` | A list of conditions, and the `WHERE` expression it compiles to |
| `sql_guard` | Refuses any statement that could write |
| `worker` | Runs the engine on its own thread, with request coalescing and epochs |
| `meta` | Parquet footer facts and the CSV dialect |
| `stats` | Column statistics and a histogram |
| `ddl` | A `CREATE TABLE` statement for another database |
| `config` | The settings Peruse keeps, and the resources of the machine |
| `theme` | The colour model and the built-in themes |

It has no dependency on ratatui and none on crossterm, so a front end with a
graphical user interface can use the same API.

**If you want the tool, install [`peruse-tui`](https://crates.io/crates/peruse-tui)
instead.** This crate is the library it is built on.

```rust,no_run
use peruse_core::{OpenOptions, Request, View, Worker};

let (worker, opened) = Worker::spawn("trips.parquet", OpenOptions::default())?;
println!("{} columns", opened.schema.len());
worker.send(Request::SetView { epoch: 1, view: View::default(), limit: 50 });
# Ok::<(), anyhow::Error>(())
```

## Reading only

The crate never opens a data file for write access. It reaches files through
`read_parquet`, `read_csv` and `read_json_auto`, which can read but cannot
write, and `sql_guard` refuses any statement from a user that could change
anything. See
[docs/read-only-guard.md](https://github.com/JohnGrey0/peruse/blob/main/docs/read-only-guard.md).

## Building

The build compiles DuckDB from source, so the first build takes some minutes
and needs a C++ compiler. The builds after it do not.

Licensed under the Apache License 2.0.
