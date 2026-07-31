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

pub mod config;
pub mod ddl;
pub mod dirs;
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

pub use config::{Config, Resources};
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

#[cfg(test)]
mod style_tests {
    /// Each comment in the workspace holds ASCII characters only.
    ///
    /// This project writes its comments in Simplified Technical English, and that
    /// rule covers the characters as well as the words. A comment that holds an
    /// ellipsis character or an arrow is hard to read in an editor with a font
    /// that has no glyph for it, and it is hard to type.
    ///
    /// A text that the program PRINTS is a different thing and keeps its
    /// characters: the screen shows an arrow for the direction of a sort and an
    /// ellipsis for a value that is cut, and that is correct.
    ///
    /// The test therefore looks at a line that STARTS with a comment mark, which
    /// is the form that this project uses for each comment of more than a few
    /// words. A comment after code on the same line is not covered: a test that
    /// covered it would have to know where each text of Rust starts and ends.
    ///
    /// The rule went wrong three times before this test existed, so the test is
    /// here to hold it.
    #[test]
    fn every_comment_holds_ascii_characters_only() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("the crates sit two levels below the workspace")
            .join("crates");
        // A crate that comes from crates.io holds its own source and no
        // workspace. There is nothing to walk there, and that is not a fault.
        let Ok(crates) = std::fs::read_dir(&root) else {
            return;
        };

        let mut files = Vec::new();
        for entry in crates.flatten() {
            for sub in ["src", "examples"] {
                let Ok(entries) = std::fs::read_dir(entry.path().join(sub)) else {
                    continue;
                };
                files.extend(
                    entries
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| p.extension().is_some_and(|x| x == "rs")),
                );
            }
        }
        // A walk that finds nothing would make a test that cannot fail.
        assert!(
            files.len() > 10,
            "the walk found {} source files, so it looked in the wrong place",
            files.len()
        );

        let mut bad = Vec::new();
        for path in &files {
            let text = std::fs::read_to_string(path).expect("a source file that cannot be read");
            for (n, line) in text.lines().enumerate() {
                if !line.trim_start().starts_with("//") {
                    continue;
                }
                if let Some(c) = line.chars().find(|c| !c.is_ascii()) {
                    bad.push(format!(
                        "{}:{}: U+{:04X} in a comment",
                        path.display(),
                        n + 1,
                        c as u32
                    ));
                }
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }
}
