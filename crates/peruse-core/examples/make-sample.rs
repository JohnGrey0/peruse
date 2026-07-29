//! Makes sample data. Use this data to try Peruse.
//!
//!     cargo run -p peruse-core --example make-sample -- ./sample
//!
//! The example writes four things: one Parquet file, one CSV file, one TSV
//! file, and one set of two Parquet files. The data holds the types and the
//! difficult values that Peruse must show correctly:
//!
//! * NULL values
//! * wide text
//! * negative numbers and very large numbers
//! * timestamps
//! * values that are true or false
//! * Unicode characters

use anyhow::Result;
use duckdb::Connection;

fn main() -> Result<()> {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "sample".into());
    let rows: u64 = std::env::args()
        .nth(2)
        .map(|s| s.replace('_', "").parse())
        .transpose()?
        .unwrap_or(200_000);
    std::fs::create_dir_all(&dir)?;
    let d = dir.replace('\\', "/");

    let conn = Connection::open_in_memory()?;
    conn.execute_batch(
        &"CREATE TABLE t AS
         SELECT
           i                                              AS id,
           ['alice','bob','carol','dave', NULL][i % 5 + 1] AS name,
           CASE WHEN i % 7 = 0 THEN NULL
                ELSE round((i * 37.5)::DECIMAL(18,2) % 10000, 2) END AS amount,
           TIMESTAMP '2024-01-01 00:00:00' + INTERVAL (i) MINUTE  AS occurred_at,
           (i % 3 = 0)                                    AS flagged,
           ['EU','US','APAC'][i % 3 + 1]                  AS region,
           repeat('long text ', (i % 12) + 1)             AS notes,
           ['ok','naïve','日本語','emoji 🎉'][i % 4 + 1]   AS unicode_sample,
           (i - 50000) * 1000000                          AS big_signed
         FROM range(ROWS) tbl(i);"
            .replace("ROWS", &rows.to_string()),
    )?;

    conn.execute_batch(&format!(
        "COPY t TO '{d}/sample.parquet' (FORMAT parquet, COMPRESSION zstd, ROW_GROUP_SIZE 50000);
         COPY t TO '{d}/sample.csv'     (FORMAT csv, HEADER);
         COPY t TO '{d}/sample.tsv'     (FORMAT csv, HEADER, DELIMITER '\t');
         COPY (FROM t WHERE id % 2 = 0) TO '{d}/part-0.parquet' (FORMAT parquet);
         COPY (FROM t WHERE id % 2 = 1) TO '{d}/part-1.parquet' (FORMAT parquet);"
    ))?;

    for f in [
        "sample.parquet",
        "sample.csv",
        "sample.tsv",
        "part-0.parquet",
        "part-1.parquet",
    ] {
        let path = format!("{dir}/{f}");
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        println!("{path}  {}", peruse_core::source::human_bytes(bytes));
    }
    println!("\nTry:  peruse {dir}/sample.parquet");
    println!("      peruse '{dir}/part-*.parquet'");
    Ok(())
}
