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

use anyhow::Result;
use std::time::Instant;

use peruse_core::model::CellKind;
use peruse_core::query::{SortDir, SortKey, View};
use peruse_core::{Engine, OpenOptions};

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
    println!("{path}\n");

    let mut engine = time("open (to schema)", || {
        Engine::open(&path, &OpenOptions::default())
    })?;
    let view = View::default();
    let schema = engine.describe(&view)?;
    println!(
        "{:<34} {} cols, {}\n",
        "shape",
        schema.len(),
        peruse_core::source::human_bytes(engine.source.bytes)
    );

    time("first screen (50 rows)", || engine.page(&view, &schema, 50, 0))?;
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

    let filtered = View {
        filter: Some("id % 97 = 0".into()),
        ..Default::default()
    };
    time("filtered first screen", || {
        engine.page(&filtered, &schema, 50, 0)
    })?;
    time("filtered count", || engine.count(&filtered))?;

    let sorted = View {
        sort: vec![SortKey {
            column: schema.columns[0].name.clone(),
            dir: SortDir::Desc,
        }],
        ..Default::default()
    };
    time("sorted first screen", || engine.page(&sorted, &schema, 50, 0))?;

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
    time("file metadata", || engine.file_meta())?;
    Ok(())
}
