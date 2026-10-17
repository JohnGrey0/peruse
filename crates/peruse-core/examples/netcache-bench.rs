//! Measures a file on a network drive: read in place, read from a local copy,
//! and read into a table in memory.
//!
//!     cargo run -p peruse-core --example netcache-bench -- gen <dir>
//!     cargo run -p peruse-core --example netcache-bench -- run <file>
//!     cargo run -p peruse-core --example netcache-bench -- run <file> --copy <cache dir>
//!     cargo run -p peruse-core --example netcache-bench -- mem <file>
//!
//! The option `--no-index` makes `run` measure a file of text with no index,
//! as Peruse opens a file of text of more than 64 MB until the user presses
//! `I`. Each step then reads the file again.
//!
//! The command `gen` writes three files that do not compress well: a Parquet
//! file of about 1 GB, a CSV file of about 500 MB and a file of JSON lines of
//! about 300 MB. The command `run` measures the operations that a user waits
//! for. With `--copy`, it first copies the file into the cache directory, in
//! the same way as Peruse does, and it measures the copy too. A second `run`
//! with the same cache directory reads the copy of the first. The command
//! `mem` loads the file into a table in memory, and measures the same steps.
//!
//! The steps use the first column of text for the sort and the first column of
//! numbers for the statistics, so each file works.

use anyhow::{Context, Result};
use std::time::Instant;

use peruse_core::model::CellKind;
use peruse_core::query::{SortDir, SortKey, View};
use peruse_core::{Engine, OpenOptions};

/// Calls a function, measures the time that it needs, and prints the time.
fn time<T>(label: &str, f: impl FnOnce() -> Result<T>) -> Result<(T, f64)> {
    let t0 = Instant::now();
    let out = f()?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("{label:<34} {ms:>10.1} ms");
    Ok((out, ms))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("gen") => generate(args.get(1).context("gen needs a directory")?),
        Some("mem") => memory(args.get(1).context("mem needs a file")?),
        Some("run") => {
            let path = args.get(1).context("run needs a file")?;
            let copy = args
                .iter()
                .position(|a| a == "--copy")
                .and_then(|i| args.get(i + 1));
            let no_index = args.iter().any(|a| a == "--no-index");
            run(path, copy.map(String::as_str), no_index)
        }
        _ => anyhow::bail!(
            "use: gen <dir> | run <file> [--copy <cache dir>] [--no-index] | mem <file>"
        ),
    }
}

/// Writes the three test files.
fn generate(dir: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let d = dir.replace('\\', "/");
    let conn = duckdb::Connection::open_in_memory()?;
    let select = |rows: u64| {
        format!(
            "SELECT i AS id, md5(i::VARCHAR) AS h1, md5((i * 7)::VARCHAR) AS h2, \
                    random() AS x, (random() * 1e6)::BIGINT AS n, \
                    TIMESTAMP '2024-01-01' + INTERVAL (i) SECOND AS ts, \
                    ['EU', 'US', 'APAC'][i % 3 + 1] AS region \
             FROM range({rows}) t(i)"
        )
    };
    conn.execute_batch(&format!(
        "COPY ({}) TO '{d}/wide.parquet' (FORMAT parquet, COMPRESSION zstd);\n\
         COPY ({}) TO '{d}/big.csv' (FORMAT csv, HEADER);\n\
         COPY ({}) TO '{d}/big.json' (FORMAT json);",
        select(20_000_000),
        select(4_000_000),
        select(1_500_000)
    ))?;
    for f in ["wide.parquet", "big.csv", "big.json"] {
        let p = std::path::Path::new(dir).join(f);
        println!("{} {}", p.display(), std::fs::metadata(&p)?.len());
    }
    Ok(())
}

/// Gives the read call of DuckDB for a file, from its extension.
fn reader(path: &str) -> String {
    let p = path.replace('\\', "/");
    let lower = p.to_ascii_lowercase();
    if lower.ends_with(".parquet") {
        format!("read_parquet('{p}')")
    } else if lower.ends_with(".json") || lower.ends_with(".ndjson") || lower.ends_with(".jsonl") {
        format!("read_json_auto('{p}')")
    } else {
        format!("read_csv_auto('{p}')")
    }
}

/// Loads a file into a table in memory, and measures the same steps on that
/// table. This is the other way to make a file fast: DuckDB holds the rows,
/// and no step reads the file again.
fn memory(path: &str) -> Result<()> {
    println!("{path} (into memory)\n");
    let t_start = Instant::now();
    let conn = duckdb::Connection::open_in_memory()?;
    time("load into a table in memory", || {
        conn.execute_batch(&format!("CREATE TABLE t AS SELECT * FROM {}", reader(path)))
            .map_err(Into::into)
    })?;
    let q = |sql: &str| -> Result<()> {
        let mut st = conn.prepare(sql)?;
        let mut rows = st.query([])?;
        while rows.next()?.is_some() {}
        Ok(())
    };
    time("first screen (50 rows)", || q("SELECT * FROM t LIMIT 50"))?;
    println!(
        "{:<34} {:>10.1} ms",
        "= time to the first frame",
        t_start.elapsed().as_secs_f64() * 1000.0
    );
    let rows: i64 = conn.query_row("SELECT count(*) FROM t", [], |r| r.get(0))?;
    // The first column of text and the first column of numbers.
    let mut text_col = None;
    let mut num_col = None;
    {
        let mut st = conn.prepare("SELECT column_name, column_type FROM (DESCRIBE t)")?;
        let mut r = st.query([])?;
        while let Some(row) = r.next()? {
            let name: String = row.get(0)?;
            let ty: String = row.get(1)?;
            if text_col.is_none() && ty == "VARCHAR" {
                text_col = Some(name.clone());
            }
            if num_col.is_none() && (ty == "DOUBLE" || ty == "BIGINT" || ty == "INTEGER") {
                num_col = Some(name);
            }
        }
    }
    time("jump to the last row", || {
        q(&format!("SELECT * FROM t LIMIT 50 OFFSET {}", (rows - 50).max(0)))
    })?;
    if let Some(c) = &text_col {
        time(&format!("sort on a text column ({c})"), || {
            q(&format!("SELECT * FROM t ORDER BY \"{c}\" DESC LIMIT 50"))
        })?;
    }
    if let Some(c) = &num_col {
        time(&format!("statistics of a column ({c})"), || {
            q(&format!(
                "SELECT min(\"{c}\"), max(\"{c}\"), avg(\"{c}\"), stddev(\"{c}\"), \
                 approx_count_distinct(\"{c}\"), count(\"{c}\") FROM t"
            ))
        })?;
    }
    let mem: String = conn.query_row(
        "SELECT sum(memory_usage_bytes)::VARCHAR FROM duckdb_memory()",
        [],
        |r| r.get(0),
    )?;
    println!("{:<34} {:>10} bytes", "memory that DuckDB holds", mem);
    Ok(())
}

/// Measures one file, in place or from a local copy.
fn run(path: &str, copy: Option<&str>, no_index: bool) -> Result<()> {
    println!("{path}\n");
    let t_start = Instant::now();
    let mut _keep = None;
    let input = match copy {
        None => path.to_string(),
        Some(dir) => {
            use peruse_core::cache::{Plan, Policy, Settings, plan};
            let settings = Settings {
                policy: Policy::Always,
                min_bytes: 0,
                max_bytes: u64::MAX / 4,
                dir: Some(dir.into()),
            };
            let probe = peruse_core::remote::System;
            let local = match plan(path, &settings, &probe) {
                Plan::Ready(l) => {
                    println!("{:<34} {:>13}", "copy to the local disk", "an earlier one");
                    l
                }
                Plan::Copy(job) => {
                    let total = job.total_bytes();
                    let cancel = std::sync::atomic::AtomicBool::new(false);
                    let (l, ms) = time("copy to the local disk", || {
                        job.run(&mut |_| {}, &cancel, &probe)
                            .map_err(|e| anyhow::anyhow!("{e}"))
                    })?;
                    println!(
                        "{:<34} {:>10.1} MB/s",
                        "copy rate",
                        total as f64 / 1e6 / (ms / 1000.0)
                    );
                    l
                }
                Plan::InPlace(why) => anyhow::bail!("no copy: {why:?}"),
            };
            let input = local.input.clone();
            _keep = Some(local);
            input
        }
    };

    let (mut engine, _) = time("open (to schema)", || {
        Engine::open(&input, &OpenOptions::default())
    })?;
    let view = View::default();
    let schema = engine.describe(&view)?;
    time("first screen (50 rows)", || engine.page(&view, &schema, 50, 0))?;
    println!(
        "{:<34} {:>10.1} ms",
        "= time to the first frame",
        t_start.elapsed().as_secs_f64() * 1000.0
    );
    let (total, _) = time("count(*)", || engine.count(&view))?;
    if !engine.is_seekable() && !no_index {
        time("index the file of text", || engine.materialize())?;
    }
    time("jump to the last row", || {
        engine.page(&view, &schema, 50, total.saturating_sub(50))
    })?;
    if let Some(col) = schema.columns.iter().find(|c| c.kind == CellKind::Text) {
        let sorted = View {
            sort: vec![SortKey {
                column: col.name.clone(),
                dir: SortDir::Desc,
            }],
            ..Default::default()
        };
        time(&format!("sort on a text column ({})", col.name), || {
            engine.page(&sorted, &schema, 50, 0)
        })?;
    }
    if let Some(col) = schema.columns.iter().find(|c| c.kind == CellKind::Number) {
        time(&format!("statistics of a column ({})", col.name), || {
            engine.column_stats(&view, col, 8)
        })?;
    }
    time("metadata panel", || engine.file_meta())?;
    println!(
        "{:<34} {:>10.1} ms",
        "= all of the above",
        t_start.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}
