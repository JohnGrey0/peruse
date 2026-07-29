//! The core of Peruse. It opens a data file, it queries the file, and it
//! describes the file.
//!
//! This crate has no dependency on a terminal and no dependency on a window
//! system. The terminal user interface (TUI) is one caller of this crate. A
//! front-end with a graphical user interface (GUI) can use the same API and
//! the same themes.
//!
//! ```no_run
//! use peruse_core::{engine::OpenOptions, worker::{Worker, Request}, query::View};
//!
//! let (worker, opened) = Worker::spawn("trips.parquet", OpenOptions::default())?;
//! println!("{} columns", opened.schema.len());
//! worker.send(Request::SetView { epoch: 1, view: View::default(), limit: 50 });
//! # Ok::<(), anyhow::Error>(())
//! ```

pub mod ddl;
pub mod engine;
pub mod filter;
pub mod meta;
pub mod model;
pub mod query;
pub mod source;
pub mod sql_guard;
pub mod stats;
pub mod theme;
pub mod worker;

pub use ddl::{Dialect, TableProfile};
pub use engine::{Engine, OpenOptions};
pub use filter::{Condition, FilterSet, Join, Op, Term};
pub use model::{Align, CellKind, Column, RowCount, RowPage, Schema};
pub use query::{Base, SortDir, SortKey, View};
pub use source::{Format, Source};
pub use theme::Theme;
pub use worker::{Opened, Request, Response, Worker};

/// The version of Peruse. The option `--version` and the help overlay show it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
