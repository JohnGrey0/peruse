//! Runs the engine on a background thread. The user interface never blocks.
//!
//! Two rules keep the scroll fast on a file that is larger than the memory:
//!
//! * **The worker combines the requests.** Each press of a scroll key adds one
//!   page request to the queue. Only the newest request is useful. The worker
//!   therefore reads the full queue and removes the requests that a newer
//!   request replaces. The engine never falls behind the cursor.
//! * **The worker keeps the epoch.** Each change of the view increases a
//!   counter, the epoch. Each response carries the epoch of its request. The
//!   user interface discards a response with an old epoch. A slow count from
//!   the previous filter can therefore not replace the current count.

use anyhow::Result;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use duckdb::InterruptHandle;
use std::sync::Arc;
use std::thread;

use crate::engine::{ColumnBrief, Engine, OpenOptions};
use crate::meta::FileMeta;
use crate::model::{Column, RowPage, Schema};
use crate::query::{Base, View};
use crate::source::Source;
use crate::sql_guard;
use crate::stats::ColumnStats;

/// A unit of work for the worker thread.
#[derive(Clone, Debug)]
pub enum Request {
    /// Use a new view. The worker answers with the schema, then the first
    /// page, then the row count, in that order.
    SetView {
        epoch: u64,
        view: View,
        limit: u32,
    },
    /// Read one page of rows.
    Page {
        epoch: u64,
        view: View,
        schema: Schema,
        offset: u64,
        limit: u32,
    },
    /// Calculate the statistics of one column.
    Stats {
        epoch: u64,
        view: View,
        column: Column,
        top_k: u32,
    },
    /// Measure each column that the detail band draws. One request covers every
    /// column, because the band draws them all at the same time.
    Band {
        epoch: u64,
        view: View,
        columns: Vec<Column>,
        /// `true` to measure the count of the different values and the range as
        /// well as the count of the NULL values.
        ///
        /// The compact band draws the share of NULL values alone. The three
        /// other facts each read the whole column, so the mode has to travel
        /// with the request. Without this, the compact band pays the price of
        /// the detailed band on every file that has no Parquet footer.
        values: bool,
    },
    /// Read one cell in full.
    Cell {
        epoch: u64,
        view: View,
        column: String,
        row: u64,
    },
    /// Apply the settings that DuckDB can change while it runs, and give the
    /// values that it uses after the change.
    Configure {
        epoch: u64,
        threads: Option<usize>,
        memory_limit: Option<String>,
    },
    /// Read one complete row as JSON, for the record view.
    RowJson {
        epoch: u64,
        view: View,
        schema: Schema,
        row: u64,
    },
    /// Find the rows that hold a value.
    Search {
        epoch: u64,
        view: View,
        schema: Schema,
        needle: String,
        from: u64,
        /// The number of rows to examine in this call. The limit keeps the
        /// user interface live, and it lets the user stop a search that finds
        /// nothing. The caller moves away from the cursor one part at a time.
        scan: u64,
        limit: u32,
    },
    /// Read the metadata of the file.
    Meta {
        epoch: u64,
    },
    /// Copy a CSV file into a table, so that the engine can go directly to
    /// any row.
    Index {
        epoch: u64,
    },
    /// Stop the worker thread.
    Shutdown,
}

impl Request {
    /// Gives the epoch of the request. A shutdown request gets the largest
    /// possible epoch, so that no other request can remove it.
    fn epoch(&self) -> u64 {
        match self {
            Request::SetView { epoch, .. }
            | Request::Page { epoch, .. }
            | Request::Stats { epoch, .. }
            | Request::Band { epoch, .. }
            | Request::Cell { epoch, .. }
            | Request::RowJson { epoch, .. }
            | Request::Configure { epoch, .. }
            | Request::Search { epoch, .. }
            | Request::Meta { epoch }
            | Request::Index { epoch } => *epoch,
            Request::Shutdown => u64::MAX,
        }
    }

    /// Gives a number for the kind of the request. A newer request of one kind
    /// replaces an older request of the same kind.
    fn kind(&self) -> u8 {
        match self {
            Request::SetView { .. } => 0,
            Request::Page { .. } => 1,
            Request::Stats { .. } => 2,
            Request::Cell { .. } => 3,
            Request::Search { .. } => 4,
            Request::Meta { .. } => 5,
            Request::Index { .. } => 6,
            Request::RowJson { .. } => 7,
            Request::Configure { .. } => 8,
            Request::Band { .. } => 9,
            Request::Shutdown => 10,
        }
    }
}

/// An answer from the worker thread. Each answer carries the epoch of its
/// request.
#[derive(Clone, Debug)]
pub enum Response {
    /// The name and the type of each column.
    Schema { epoch: u64, schema: Schema },
    /// One page of rows.
    Page { epoch: u64, page: RowPage },
    /// The number of rows in the view.
    Count { epoch: u64, total: u64 },
    /// The statistics of one column.
    Stats { epoch: u64, stats: Box<ColumnStats> },
    /// The facts of each column that the detail band draws.
    Band { epoch: u64, briefs: Vec<ColumnBrief> },
    /// The complete value of one cell.
    Cell { epoch: u64, row: u64, column: String, value: Option<String> },
    /// One complete row as JSON.
    RowJson { epoch: u64, row: u64, json: Option<String> },
    /// The values that DuckDB uses after a change to its settings.
    Configured { epoch: u64, threads: Option<String>, memory_limit: Option<String> },
    /// The offsets of the rows that match, and the part of the view that the
    /// worker examined.
    Search { epoch: u64, hits: Vec<u64>, from: u64, scan: u64 },
    /// The metadata of the file.
    Meta { epoch: u64, meta: Box<FileMeta> },
    /// The worker copied the CSV file into a table.
    Indexed { epoch: u64 },
    /// `true` while the worker has work to do, and `false` when it has none.
    Busy(bool),
    /// A request failed. The worker stays alive.
    Error { epoch: u64, context: String, message: String },
}

/// What the caller learns when the file opens, before Peruse draws a frame.
#[derive(Clone, Debug)]
pub struct Opened {
    /// The name and the type of each column.
    pub schema: Schema,
    /// The file or the files behind the view.
    pub source: Source,
    /// `true` when the engine can go directly to any row.
    pub seekable: bool,
    /// The `read_parquet` call or `read_csv` call that reads the file.
    pub read_expr: String,
}

/// A handle to the engine thread: a channel for requests, a channel for
/// responses, and a way to stop a query.
pub struct Worker {
    tx: Sender<Request>,
    rx: Receiver<Response>,
    interrupt: Arc<InterruptHandle>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Worker {
    /// Starts the worker thread, opens the file, and waits for the schema.
    ///
    /// To open a file, the engine reads a Parquet footer or a sample of a CSV
    /// file. This work is fast, so the wait is short. A bad path therefore
    /// gives a plain error on the command line. The terminal does not start
    /// and then stop again.
    pub fn spawn(input: &str, opts: OpenOptions) -> Result<(Worker, Opened)> {
        let (req_tx, req_rx) = unbounded::<Request>();
        let (resp_tx, resp_rx) = unbounded::<Response>();
        let (ready_tx, ready_rx) = bounded::<Result<(Opened, Arc<InterruptHandle>)>>(1);

        let input = input.to_string();
        let handle = thread::Builder::new()
            .name("peruse-engine".into())
            .spawn(move || {
                let mut engine = match Engine::open(&input, &opts) {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                // The open operation read the schema already. A second read
                // would run the sniffer of DuckDB again, and on a file of
                // text that is the slow part of an open.
                let opened = Opened {
                    schema: engine.base_schema().clone(),
                    source: engine.source.clone(),
                    seekable: engine.is_seekable(),
                    read_expr: engine.read_expr().to_string(),
                };
                if ready_tx.send(Ok((opened, engine.interrupt_handle()))).is_err() {
                    return;
                }
                run(&mut engine, &req_rx, &resp_tx);
            })?;

        let (opened, interrupt) = ready_rx.recv()??;
        Ok((
            Worker {
                tx: req_tx,
                rx: resp_rx,
                interrupt,
                handle: Some(handle),
            },
            opened,
        ))
    }

    /// Sends one request to the worker thread.
    pub fn send(&self, req: Request) {
        // A closed channel shows that the worker thread stopped. The user
        // interface reports the failure through the request that it waits on.
        let _ = self.tx.send(req);
    }

    /// Gives the channel of responses. The event loop waits on this channel.
    pub fn responses(&self) -> &Receiver<Response> {
        &self.rx
    }

    /// Stops the query that runs now.
    ///
    /// The request that stops gives an error, and the user interface shows a
    /// message about the cancellation.
    pub fn cancel(&self) {
        self.interrupt.interrupt();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Stop a long query first. If it continues, the join below waits for
        // the end of that query.
        self.interrupt.interrupt();
        let _ = self.tx.send(Request::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Removes the requests that Peruse no longer needs, and keeps the others.
fn coalesce(mut batch: Vec<Request>) -> Vec<Request> {
    if batch.iter().any(|r| matches!(r, Request::Shutdown)) {
        return vec![Request::Shutdown];
    }
    // A request from an old view is work with no result, because the user
    // interface discards the response.
    //
    // The index is the one exception. It builds a table of the row positions
    // of the file, and that table has no connection to the view. A start with
    // `--filter` sends the index request and then changes the view, and
    // without this exception the index would never be built at all.
    let newest = batch.iter().map(Request::epoch).filter(|g| *g != u64::MAX).max();
    if let Some(newest) = newest {
        batch.retain(|r| r.epoch() == newest || matches!(r, Request::Index { .. }));
    }
    // In the newest epoch, keep the last request of each kind.
    let mut out: Vec<Request> = Vec::new();
    for req in batch {
        match out.iter().position(|r| r.kind() == req.kind()) {
            Some(i) => out[i] = req,
            None => out.push(req),
        }
    }
    out
}

/// The main loop of the worker thread.
fn run(engine: &mut Engine, rx: &Receiver<Request>, tx: &Sender<Response>) {
    loop {
        // Wait for the first request, then take each request behind it. A key
        // that the user holds down therefore does not build a long queue.
        let Ok(first) = rx.recv() else { return };
        let mut batch = vec![first];
        while let Ok(more) = rx.try_recv() {
            batch.push(more);
        }
        let batch = coalesce(batch);
        if batch.iter().any(|r| matches!(r, Request::Shutdown)) {
            return;
        }

        if tx.send(Response::Busy(true)).is_err() {
            return;
        }
        for req in batch {
            if handle(engine, req, tx).is_err() {
                return;
            }
        }
        if tx.send(Response::Busy(false)).is_err() {
            return;
        }
    }
}

/// Reports a failure to the user interface and keeps the worker alive.
///
/// A bad filter expression is a usual event. The previous view stays on the
/// screen, and the user can correct the expression.
fn fail(tx: &Sender<Response>, epoch: u64, context: &str, e: impl std::fmt::Display) -> Result<(), ()> {
    tx.send(Response::Error {
        epoch,
        context: context.to_string(),
        message: e.to_string(),
    })
    .map_err(|_| ())
}

/// Does the work of one request and sends the response or the error.
///
/// The function gives `Err(())` only when the worker must stop.
fn handle(engine: &mut Engine, req: Request, tx: &Sender<Response>) -> Result<(), ()> {
    match req {
        Request::Shutdown => Err(()),

        Request::SetView { epoch, view, limit } => {
            // The guard runs two times. The user interface checks the text
            // while the user types it. This check makes sure that no view
            // reaches the engine without a check, whatever the caller does.
            //
            // A view holds text of the user in two places. The statement is a
            // complete query, and `query.rs` puts the filter into a
            // `WHERE (...)` part of a statement that Peruse builds. The two
            // places therefore need two different checks, and a check of the
            // statement alone leaves the filter open.
            if let Base::Sql(sql) = &view.base
                && let Err(e) = sql_guard::ensure_read_only(sql) {
                    return fail(tx, epoch, "query", e);
                }
            if let Some(filter) = &view.filter
                && let Err(e) = sql_guard::ensure_safe_predicate(filter) {
                    return fail(tx, epoch, "filter", e);
                }
            let schema = match engine.describe(&view) {
                Ok(s) => s,
                Err(e) => return fail(tx, epoch, "schema", e),
            };
            tx.send(Response::Schema { epoch, schema: schema.clone() }).map_err(|_| ())?;

            match engine.page(&view, &schema, limit, 0) {
                Ok(page) => tx.send(Response::Page { epoch, page }).map_err(|_| ())?,
                Err(e) => return fail(tx, epoch, "rows", e),
            }
            // Count the rows last. The count is the one part that can need a
            // full scan, and the user can use the grid before it arrives.
            match engine.count(&view) {
                Ok(total) => tx.send(Response::Count { epoch, total }).map_err(|_| ())?,
                Err(e) => return fail(tx, epoch, "count", e),
            }
            Ok(())
        }

        Request::Page { epoch, view, schema, offset, limit } => {
            match engine.page(&view, &schema, limit, offset) {
                Ok(page) => tx.send(Response::Page { epoch, page }).map_err(|_| ()),
                Err(e) => fail(tx, epoch, "rows", e),
            }
        }

        Request::Stats { epoch, view, column, top_k } => {
            match engine.column_stats(&view, &column, top_k) {
                Ok(stats) => tx
                    .send(Response::Stats { epoch, stats: Box::new(stats) })
                    .map_err(|_| ()),
                Err(e) => fail(tx, epoch, "column stats", e),
            }
        }

        Request::Band { epoch, view, columns, values } => match engine
            .column_band(&view, &columns, values)
        {
            Ok(briefs) => tx.send(Response::Band { epoch, briefs }).map_err(|_| ()),
            Err(e) => fail(tx, epoch, "column band", e),
        },

        Request::Configure { epoch, threads, memory_limit } => {
            match engine.apply_settings(threads, memory_limit.as_deref()) {
                Ok(()) => tx
                    .send(Response::Configured {
                        epoch,
                        threads: engine.current_setting("threads"),
                        memory_limit: engine.current_setting("memory_limit"),
                    })
                    .map_err(|_| ()),
                Err(e) => fail(tx, epoch, "settings", e),
            }
        }
        Request::RowJson { epoch, view, schema, row } => {
            match engine.row_json(&view, &schema, row) {
                Ok(json) => tx
                    .send(Response::RowJson { epoch, row, json })
                    .map_err(|_| ()),
                Err(e) => fail(tx, epoch, "record", e),
            }
        }
        Request::Cell { epoch, view, column, row } => match engine.cell(&view, &column, row) {
            Ok(value) => tx
                .send(Response::Cell { epoch, row, column, value })
                .map_err(|_| ()),
            Err(e) => fail(tx, epoch, "cell", e),
        },

        Request::Search { epoch, view, schema, needle, from, scan, limit } => {
            match engine.search(&view, &schema, &needle, from, scan, limit) {
                Ok(hits) => tx
                    .send(Response::Search { epoch, hits, from, scan })
                    .map_err(|_| ()),
                Err(e) => fail(tx, epoch, "search", e),
            }
        }

        Request::Meta { epoch } => match engine.file_meta() {
            Ok(meta) => tx
                .send(Response::Meta { epoch, meta: Box::new(meta) })
                .map_err(|_| ()),
            Err(e) => fail(tx, epoch, "metadata", e),
        },

        Request::Index { epoch } => match engine.materialize() {
            Ok(()) => tx.send(Response::Indexed { epoch }).map_err(|_| ()),
            Err(e) => fail(tx, epoch, "indexing", e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(epoch: u64, offset: u64) -> Request {
        Request::Page {
            epoch,
            view: View::default(),
            schema: Schema::default(),
            offset,
            limit: 50,
        }
    }

    #[test]
    fn an_index_request_survives_a_change_of_the_view() {
        // `App::new` asks for the index, and a start with `--filter` then
        // changes the view immediately. The index describes the file and not
        // the view, so it must not go away with the old epoch. Without this
        // rule, the table is never built, no response ever arrives, and the
        // note "press I to index" stays on the screen for the whole session.
        let out = coalesce(vec![
            Request::Index { epoch: 1 },
            Request::SetView {
                epoch: 2,
                view: View::default(),
                limit: 50,
            },
        ]);
        assert!(
            out.iter().any(|r| matches!(r, Request::Index { .. })),
            "the index request went away: {out:?}"
        );
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn a_page_request_from_an_old_view_still_goes_away() {
        let out = coalesce(vec![page(1, 0), page(2, 500)]);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Request::Page { epoch: 2, .. }));
    }

    #[test]
    fn a_burst_of_scrolling_collapses_to_the_last_page() {
        let out = coalesce(vec![page(1, 0), page(1, 50), page(1, 100)]);
        assert_eq!(out.len(), 1);
        match &out[0] {
            Request::Page { offset, .. } => assert_eq!(*offset, 100, "newest wins"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn stale_generations_are_dropped_entirely() {
        let out = coalesce(vec![page(1, 0), page(2, 0), Request::Meta { epoch: 2 }]);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|r| r.epoch() == 2));
    }

    #[test]
    fn different_kinds_in_one_generation_all_survive() {
        let out = coalesce(vec![
            page(3, 0),
            Request::Meta { epoch: 3 },
            Request::Index { epoch: 3 },
        ]);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn ordering_within_a_generation_is_preserved() {
        let out = coalesce(vec![
            Request::Meta { epoch: 1 },
            page(1, 10),
            page(1, 20),
        ]);
        // The metadata request stays first. The newer page request takes the
        // position of the older page request.
        assert!(matches!(out[0], Request::Meta { .. }));
        match &out[1] {
            Request::Page { offset, .. } => assert_eq!(*offset, 20),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_band_request_replaces_only_an_older_band_request() {
        // A scroll to the side gives a new set of columns, and only the newest
        // set is on the screen. The page request must survive beside it: the
        // rows and the band are two different answers.
        let band = |epoch: u64, name: &str| Request::Band {
            epoch,
            view: View::default(),
            columns: vec![Column::new(name, "BIGINT", false)],
            values: false,
        };
        let out = coalesce(vec![page(4, 0), band(4, "a"), band(4, "b")]);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Request::Page { .. }));
        match &out[1] {
            Request::Band { columns, .. } => assert_eq!(columns[0].name, "b", "newest wins"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn shutdown_pre_empts_everything() {
        let out = coalesce(vec![page(9, 0), Request::Shutdown, page(9, 50)]);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Request::Shutdown));
    }

    #[test]
    fn shutdown_alone_does_not_filter_itself_out() {
        let out = coalesce(vec![Request::Shutdown]);
        assert_eq!(out.len(), 1);
    }

    /// Writes a small CSV file and starts a worker on it.
    fn worker_on_csv(tag: &str) -> Worker {
        let dir = std::env::temp_dir().join(format!("peruse-worker-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.csv");
        std::fs::write(&path, "id,name\n1,alice\n2,bob\n").unwrap();
        Worker::spawn(path.to_str().unwrap(), OpenOptions::default())
            .expect("the worker did not open the file")
            .0
    }

    #[test]
    fn a_filter_that_writes_never_reaches_the_engine() {
        // The front end checks the filter while the user types it. This test
        // goes past the front end, as a caller of the library can, and the
        // worker must still refuse the filter. The filter goes into a
        // `WHERE (...)` part, so the guard for a statement does not see it.
        let worker = worker_on_csv("filter-guard");
        worker.send(Request::SetView {
            epoch: 1,
            view: View {
                filter: Some("id IN (DELETE FROM src RETURNING id)".into()),
                ..View::default()
            },
            limit: 50,
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut failure = None;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match worker.responses().recv_timeout(left) {
                Ok(Response::Error { context, message, .. }) => {
                    failure = Some((context, message));
                }
                // The worker answers a view that it accepts with these three.
                // None of them can come, because the guard stops the request
                // before the worker gives the view to the engine.
                Ok(Response::Schema { .. } | Response::Page { .. } | Response::Count { .. }) => {
                    panic!("the worker gave the view to the engine");
                }
                Ok(Response::Busy(false)) => break,
                Ok(_) => {}
                Err(e) => panic!("the worker gave no answer: {e}"),
            }
        }

        // The user sees `context: message` on the status line, so the message
        // must name the word that Peruse refuses.
        let (context, message) = failure.expect("the worker accepted a filter that writes");
        assert_eq!(context, "filter");
        assert!(message.contains("DELETE"), "{message}");
        assert!(message.contains("read-only"), "{message}");
    }
}
