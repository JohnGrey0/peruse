//! Measures the time of the operations that make Peruse fast or slow.
//!
//!     cargo run --release -p peruse-core --example bench -- sample/sample.parquet
//!
//! Each number is a time that a user waits. The operations are these:
//!
//! * The program opens the file.
//! * The program draws the first screen.
//! * The program moves to the last row.
//! * The program applies a filter.
//! * The program calculates the statistics of a column.
//!
//! A second argument names the table of a database file. The grid asks for one
//! window of rows at a time: the screen and 250 rows on each side of it. The
//! lines with `window` measure that request.

use anyhow::Result;
use std::time::Instant;

use peruse_core::model::CellKind;
use peruse_core::query::{SortDir, SortKey, View};
use peruse_core::{Engine, OpenOptions};

/// The number of rows of one request of the grid: a screen of 50 rows and 250
/// rows on each side. Refer to `PREFETCH` in the user interface.
const WINDOW: u32 = 550;

/// Calls a function, measures the time that it needs, and prints the time.
fn time<T>(label: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let t0 = Instant::now();
    let out = f()?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("{label:<34} {ms:>9.1} ms");
    Ok(out)
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "sample/sample.parquet".into());
    let table = std::env::args().nth(2);
    println!("{path}\n");

    let opts = OpenOptions {
        table,
        ..Default::default()
    };
    let mut engine = time("open (to schema)", || Engine::open(&path, &opts))?;
    let view = View::default();
    let schema = engine.describe(&view)?;
    println!(
        "{:<34} {} cols, {}\n",
        "shape",
        schema.len(),
        peruse_core::source::human_bytes(engine.source.bytes)
    );

    time("first screen (50 rows)", || engine.page(&view, &schema, 50, 0))?;
    time("first window (550 rows)", || engine.page(&view, &schema, WINDOW, 0))?;
    // A page of a wide file holds the columns near the screen only. Refer to
    // `WIDE_COLUMNS` in the user interface.
    if schema.len() > 64 {
        time("first window, 64 columns", || {
            engine.page_columns(&view, &schema, Some((0, 64)), WINDOW, 0)
        })?;
    }
    let total = time("count(*)", || engine.count(&view))?;
    println!("{:<34} {}", "rows", peruse_core::source::human_count(total));

    if !engine.is_seekable() {
        time("index CSV", || engine.materialize())?;
    }

    let deep = total.saturating_sub(50);
    time("jump to last screen", || engine.page(&view, &schema, 50, deep))?;
    time("jump to middle", || {
        engine.page(&view, &schema, 50, total / 2)
    })?;
    time("window at middle", || {
        engine.page(&view, &schema, WINDOW, total / 2)
    })?;

    // The filter needs a column of whole numbers. The sample files name it
    // `id`, and a wide file starts with one.
    let num = schema
        .columns
        .iter()
        .find(|c| c.kind == CellKind::Number)
        .map(|c| peruse_core::query::quote_ident(&c.name))
        .unwrap_or_else(|| "1".into());
    let filtered = View {
        filter: Some(format!("{num} % 97 = 0")),
        ..Default::default()
    };
    time("filtered first screen", || {
        engine.page(&filtered, &schema, 50, 0)
    })?;
    let ftotal = time("filtered count", || engine.count(&filtered))?;
    time("filtered window at middle", || {
        engine.page(&filtered, &schema, WINDOW, ftotal / 2)
    })?;

    let sorted = View {
        sort: vec![SortKey {
            column: schema.columns[0].name.clone(),
            dir: SortDir::Desc,
        }],
        ..Default::default()
    };
    time("sorted first screen", || engine.page(&sorted, &schema, 50, 0))?;
    time("sorted first window (550 rows)", || {
        engine.page(&sorted, &schema, WINDOW, 0)
    })?;
    time("sorted window at middle", || {
        engine.page(&sorted, &schema, WINDOW, total / 2)
    })?;
    // The user scrolls on, and the grid asks for the next window.
    time("sorted, next window at middle", || {
        engine.page(&sorted, &schema, WINDOW, total / 2 + u64::from(WINDOW))
    })?;
    time("sorted, jump to last screen", || {
        engine.page(&sorted, &schema, 50, deep)
    })?;
    // A column of text holds repeated values, and the end of the sort puts the
    // rows of one value in order. Refer to `View::order_by`.
    if let Some(col) = schema.columns.iter().find(|c| c.kind == CellKind::Text) {
        let ties = View {
            sort: vec![SortKey {
                column: col.name.clone(),
                dir: SortDir::Desc,
            }],
            ..Default::default()
        };
        time("sorted on text, first window", || {
            engine.page(&ties, &schema, WINDOW, 0)
        })?;
        time("sorted on text, window at middle", || {
            engine.page(&ties, &schema, WINDOW, total / 2)
        })?;
        time("sorted on text, next window", || {
            engine.page(&ties, &schema, WINDOW, total / 2 + u64::from(WINDOW))
        })?;
    }

    if let Some(col) = schema.columns.iter().find(|c| c.kind == CellKind::Number) {
        time(&format!("stats + histogram ({})", col.name), || {
            engine.column_stats(&view, col, 8)
        })?;
    }
    if let Some(col) = schema.columns.iter().find(|c| c.kind == CellKind::Text) {
        time(&format!("stats + top values ({})", col.name), || {
            engine.column_stats(&view, col, 8)
        })?;
    }

    time("search all columns", || {
        engine.search(&view, &schema, "carol", 0, 250_000, 100)
    })?;
    // A search for a word that the file does not hold reads each row of its
    // part of the view.
    time("search, no match", || {
        engine.search(&view, &schema, "quokka", 0, 250_000, 100)
    })?;
    time("file metadata", || engine.file_meta())?;

    // The band measures the columns that fit the screen: about twenty.
    let drawn: Vec<_> = schema.columns.iter().take(20).cloned().collect();
    time("band, compact (20 columns)", || {
        engine.column_band(&view, &drawn, false)
    })?;
    time("band, detailed (20 columns)", || {
        engine.column_band(&view, &drawn, true)
    })?;
    time("record view (one row as JSON)", || {
        engine.row_json(&view, &schema, total / 2)
    })?;
    Ok(())
}
