//! Makes the data for the hard cases of the benchmark: a wide file, a set of
//! many small files, and a large database.
//!
//!     cargo run --release -p peruse-core --example make-stress -- ./sample
//!
//! The example writes these things into the directory:
//!
//! * `wide.parquet` and `wide.csv`: 1000 columns.
//! * `many/part-NNNN.parquet` and `many/part-NNNN.csv`: a set of small files
//!   for a glob pattern.
//! * `big.duckdb`: a database with one table of 10 million rows and some small
//!   tables beside it.
//!
//! Give the number of rows of the wide file and of the database as the second
//! and the third argument. The defaults are 100,000 and 10,000,000.

use anyhow::Result;
use duckdb::Connection;

/// The number of columns of the wide file.
const WIDE_COLS: usize = 1000;
/// The number of files of each format in the set.
const MANY_FILES: usize = 500;
/// The number of rows of each file in the set.
const MANY_ROWS: usize = 2000;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| "sample".into());
    let parse = |s: Option<String>, d: u64| -> Result<u64> {
        Ok(s.map(|s| s.replace('_', "").parse()).transpose()?.unwrap_or(d))
    };
    let wide_rows = parse(args.next(), 100_000)?;
    let db_rows = parse(args.next(), 10_000_000)?;
    std::fs::create_dir_all(format!("{dir}/many"))?;
    let d = dir.replace('\\', "/");

    let conn = Connection::open_in_memory()?;

    // Each column of the wide file gets one of four types in turn, so the
    // file holds numbers, text, a decimal and a time stamp.
    let cols: Vec<String> = (0..WIDE_COLS)
        .map(|k| match k % 4 {
            0 => format!("(i * {k}) % 100000 AS c{k}"),
            1 => format!("'v' || ((i + {k}) % 977) AS c{k}"),
            2 => format!("round(((i * 7 + {k}) % 10000) / 3.0, 2)::DECIMAL(12,2) AS c{k}"),
            _ => format!("TIMESTAMP '2024-01-01' + INTERVAL ((i + {k}) % 100000) SECOND AS c{k}"),
        })
        .collect();
    conn.execute_batch(&format!(
        "CREATE TABLE wide AS SELECT {} FROM range({wide_rows}) t(i);
         COPY wide TO '{d}/wide.parquet' (FORMAT parquet, COMPRESSION zstd);
         COPY wide TO '{d}/wide.csv' (FORMAT csv, HEADER);
         DROP TABLE wide;",
        cols.join(", ")
    ))?;

    conn.execute_batch(&format!(
        "CREATE TABLE part AS
         SELECT i AS id, ['alice','bob','carol','dave'][i % 4 + 1] AS name,
                (i * 37.5) % 1000 AS amount, i % 3 = 0 AS flagged
         FROM range({MANY_ROWS}) t(i);"
    ))?;
    for n in 0..MANY_FILES {
        conn.execute_batch(&format!(
            "COPY (SELECT id + {n} * {MANY_ROWS} AS id, name, amount, flagged FROM part)
               TO '{d}/many/part-{n:04}.parquet' (FORMAT parquet);
             COPY (SELECT id + {n} * {MANY_ROWS} AS id, name, amount, flagged FROM part)
               TO '{d}/many/part-{n:04}.csv' (FORMAT csv, HEADER);"
        ))?;
    }

    let db = format!("{d}/big.duckdb");
    let _ = std::fs::remove_file(&db);
    conn.execute_batch(&format!(
        "ATTACH '{db}' AS big;
         CREATE TABLE big.events AS
         SELECT i AS id,
                ['alice','bob','carol','dave', NULL][i % 5 + 1] AS name,
                round((i * 37.5) % 10000, 2) AS amount,
                TIMESTAMP '2024-01-01' + INTERVAL (i) SECOND AS occurred_at,
                ['EU','US','APAC'][i % 3 + 1] AS region,
                repeat('long text ', (i % 12) + 1) AS notes
         FROM range({db_rows}) t(i);
         CREATE TABLE big.small_a AS SELECT * FROM range(10) t(i);
         CREATE TABLE big.small_b AS SELECT * FROM range(10) t(i);
         DETACH big;"
    ))?;

    for f in ["wide.parquet", "wide.csv", "big.duckdb"] {
        let path = format!("{dir}/{f}");
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        println!("{path}  {}", peruse_core::source::human_bytes(bytes));
    }
    println!("{dir}/many  {MANY_FILES} Parquet files and {MANY_FILES} CSV files");
    Ok(())
}
