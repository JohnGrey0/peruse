//! The read layer of Peruse. It uses the embedded database DuckDB.
//!
//! Each function in this module blocks the thread that calls it. The module
//! [`crate::worker`] calls these functions on a background thread. The user
//! interface thread therefore does not block.
//!
//! The engine never opens a data file for write access. It reads the data only
//! through the table functions `read_parquet` and `read_csv`, which can read
//! but cannot write. The engine keeps its catalog in memory.
//!
//! This module uses the term "sniffer" for the part of DuckDB that examines a
//! CSV file and finds the delimiter, the quote character and the column types.

use anyhow::{bail, Context, Result};
use duckdb::types::Value;
use duckdb::{Connection, InterruptHandle};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::ddl::{ColumnProfile, TableProfile};
use crate::meta::{ColumnFooterStats, CsvMeta, FileEntry, FileMeta, ParquetMeta};
use crate::model::{CellKind, Column, RowPage, Schema};
use crate::query::{quote_ident, quote_str, View};
use crate::source::{self, Format, Source};
use crate::stats::{CellKindWrapper, ColumnStats, Histogram};

/// The name of the table in memory that holds an indexed CSV file. The
/// function [`Engine::materialize`] makes this table.
const MAT_TABLE: &str = "__peruse_indexed";

/// The deepest level that the reader of a JSON file examines.
///
/// The reader is a C++ function that calls itself one time for each level. A
/// file that nests deeper than the stack of the thread allows therefore stops
/// the program, and Rust cannot catch that fault.
///
/// A real file of data nests some levels, and not a thousand. This limit is
/// far above each such file, and far below the level that fills the stack.
const MAX_JSON_DEPTH: u32 = 128;

/// The options that control how the engine opens a file.
#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    /// The number of threads that DuckDB can use. The default is one thread
    /// for each processor core.
    pub threads: Option<usize>,
    /// The quantity of memory that DuckDB can use before it writes temporary
    /// data to the disk.
    pub memory_limit: Option<String>,
    /// Read each CSV column as text. Use this option when the type detection
    /// of DuckDB gives a wrong result.
    pub all_varchar: bool,
    /// Skip each CSV row that DuckDB cannot read.
    pub ignore_errors: bool,
    /// The CSV delimiter. This value replaces the delimiter that the sniffer
    /// finds and the delimiter that the file extension gives.
    pub delimiter: Option<char>,
    /// Set this option to `true` when the first CSV row holds the column
    /// names, and to `false` when it holds data. `None` lets the sniffer
    /// decide.
    pub header: Option<bool>,
    /// The number of rows that the CSV sniffer examines. The value `-1` makes
    /// the sniffer read the full file.
    pub sample_size: Option<i64>,
}

/// One open connection to DuckDB, and the file or the files behind it.
pub struct Engine {
    conn: Connection,
    /// The file or the files that the engine reads.
    pub source: Source,
    /// The `read_parquet(...)` call or `read_csv(...)` call behind the view
    /// `src`.
    read_expr: String,
    /// True after the engine copies a CSV file into a table.
    pub indexed: bool,
}

/// Changes each backslash in a path to a forward slash.
///
/// DuckDB accepts a forward slash on each platform. In a glob pattern, DuckDB
/// reads a backslash as an escape character and then removes it.
fn db_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Writes the paths as a SQL list, for a table function that reads many files.
fn file_list(files: &[PathBuf]) -> String {
    let items: Vec<String> = files.iter().map(|f| quote_str(&db_path(f))).collect();
    format!("[{}]", items.join(", "))
}

/// Changes a DuckDB value into a `u64`. Gives `None` for a value that does not
/// hold a number.
fn to_u64(v: &Value) -> Option<u64> {
    Some(match v {
        Value::TinyInt(x) => (*x).try_into().ok()?,
        Value::SmallInt(x) => (*x).try_into().ok()?,
        Value::Int(x) => (*x).try_into().ok()?,
        Value::BigInt(x) => (*x).try_into().ok()?,
        Value::HugeInt(x) => (*x).try_into().ok()?,
        Value::UTinyInt(x) => *x as u64,
        Value::USmallInt(x) => *x as u64,
        Value::UInt(x) => *x as u64,
        Value::UBigInt(x) => *x,
        Value::UHugeInt(x) => (*x).try_into().ok()?,
        Value::Double(x) => *x as u64,
        Value::Float(x) => *x as u64,
        _ => return None,
    })
}

/// Changes a DuckDB value into an `f64`. Gives `None` for a value that does not
/// hold a number.
fn to_f64(v: &Value) -> Option<f64> {
    Some(match v {
        Value::Float(x) => *x as f64,
        Value::Double(x) => *x,
        Value::TinyInt(x) => *x as f64,
        Value::SmallInt(x) => *x as f64,
        Value::Int(x) => *x as f64,
        Value::BigInt(x) => *x as f64,
        Value::UTinyInt(x) => *x as f64,
        Value::USmallInt(x) => *x as f64,
        Value::UInt(x) => *x as f64,
        Value::UBigInt(x) => *x as f64,
        _ => return None,
    })
}

impl Engine {
    /// Opens one file, or the set of files that a glob pattern selects.
    ///
    /// This function starts DuckDB, finds the format of the data, and makes
    /// the view `src`. It then reads the schema. A bad path or a bad option
    /// therefore gives an error here, and not at the first page request.
    pub fn open(input: &str, opts: &OpenOptions) -> Result<Engine> {
        let conn = Connection::open_in_memory().context("starting DuckDB")?;
        configure(&conn, opts)?;

        let files = resolve_files(&conn, input)?;
        if files.is_empty() {
            bail!("no files matched {input:?}");
        }
        let (format, ext_delim, compressed) = source::detect(&files[0]);
        let bytes = files
            .iter()
            .filter_map(|f| std::fs::metadata(f).ok())
            .map(|m| m.len())
            .sum();
        let label = files[0]
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| input.to_string());

        let src = Source {
            input: input.to_string(),
            label,
            format,
            files,
            bytes,
            delimiter: opts.delimiter.or(ext_delim),
            compressed,
        };

        // The build of DuckDB inside Peruse holds the readers for Parquet,
        // for text and for JSON. It holds no reader for an Arrow IPC file:
        // the functions `arrow_scan` take a pointer to data in memory, and
        // not the name of a file. Say so, and say what to do about it. The
        // message from the database alone would name a function that the
        // user never wrote.
        if src.format == Format::Arrow {
            bail!(
                "{}: Peruse cannot read an Arrow IPC file yet.\n\
                 Change it to Parquet first, for example:\n  \
                 python -c \"import pyarrow.feather as f, pyarrow.parquet as q; \
                 q.write_table(f.read_table('{}'), 'out.parquet')\"",
                src.label,
                src.label
            );
        }

        // A JSON file that nests very deep stops the program. The reader of
        // DuckDB is a C++ function that calls itself one time for each
        // level, and a stack that is full is not a fault that Rust can
        // catch. Peruse therefore counts the levels with a loop of its own
        // before it gives the file to that reader.
        if src.format == Format::Json
            && source::json_depth_over(&src.files[0], MAX_JSON_DEPTH as usize)
        {
            bail!(
                "{}: this file nests deeper than {MAX_JSON_DEPTH} levels.\n\
                 A file that deep stops the reader of DuckDB, so Peruse does not open it.",
                src.label
            );
        }

        let read_expr = build_read_expr(&src, opts);
        conn.execute_batch(&format!("CREATE OR REPLACE VIEW src AS SELECT * FROM {read_expr}"))
            .with_context(|| format!("opening {}", src.input))?;

        let engine = Engine {
            conn,
            source: src,
            read_expr,
            indexed: false,
        };
        // Read the schema now. An error message here is more useful to the
        // user than an error message at the first scroll.
        engine
            .describe(&View::default())
            .with_context(|| format!("reading schema of {}", engine.source.input))?;
        Ok(engine)
    }

    /// Gives a handle that stops the query that runs now.
    ///
    /// Another thread can hold this handle and use it safely.
    pub fn interrupt_handle(&self) -> Arc<InterruptHandle> {
        self.conn.interrupt_handle()
    }

    /// Gives the name and the type of each column. This function reads no rows.
    pub fn describe(&self, view: &View) -> Result<Schema> {
        let sql = view.describe_sql();
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let mut columns = Vec::new();
        while let Some(r) = rows.next()? {
            let name: String = r.get(0)?;
            let ty: String = r.get(1)?;
            let nullable: Option<String> = r.get(2)?;
            // The statement DESCRIBE puts "YES" or "NO" in the column `null`.
            let nullable = nullable.as_deref() != Some("NO");
            columns.push(Column::new(name, ty, nullable));
        }
        Ok(Schema { columns })
    }

    /// Reads one page of rows: `limit` rows that start at the row `offset`.
    ///
    /// DuckDB changes each value into text, so the grid can draw the page
    /// without more work.
    pub fn page(&self, view: &View, schema: &Schema, limit: u32, offset: u64) -> Result<RowPage> {
        if schema.is_empty() {
            return Ok(RowPage::new(offset, 0, Vec::new()));
        }
        let ncols = schema.len();
        let sql = view.page_sql(schema, limit, offset);
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let mut cells: Vec<Option<String>> = Vec::with_capacity(limit as usize * ncols);
        while let Some(r) = rows.next()? {
            for i in 0..ncols {
                cells.push(r.get::<_, Option<String>>(i)?);
            }
        }
        Ok(RowPage::new(offset, ncols, cells))
    }

    /// Counts the rows in the view.
    pub fn count(&self, view: &View) -> Result<u64> {
        // DuckDB reads count(*) of a Parquet file directly from the footer. A
        // view with no filter therefore needs no scan and no special case.
        let sql = view.count_sql();
        let v: Value = self.conn.query_row(&sql, [], |r| r.get(0))?;
        Ok(to_u64(&v).unwrap_or(0))
    }

    /// Reads one cell in full, for the cell inspector.
    ///
    /// A page holds a short form of a long value. This function gives the
    /// complete value.
    pub fn cell(&self, view: &View, column: &str, row: u64) -> Result<Option<String>> {
        let sql = view.cell_sql(column, row);
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        match rows.next()? {
            Some(r) => Ok(r.get::<_, Option<String>>(0)?),
            None => Ok(None),
        }
    }

    /// Applies the settings that DuckDB can change while it runs.
    ///
    /// The settings page uses this function. Without it, a change to the
    /// threads or to the memory limit would need a restart of the program,
    /// and the user could not see the result of the change.
    ///
    /// The statement `SET` is one that [`crate::sql_guard`] refuses. That
    /// guard reads the statements of the **user**. This statement comes from
    /// Peruse, it names one setting of the engine, and it writes no data.
    pub fn apply_settings(&self, threads: Option<usize>, memory_limit: Option<&str>) -> Result<()> {
        let mut sql = String::new();
        if let Some(n) = threads {
            // A value of zero would stop DuckDB from running any query.
            let n = n.max(1);
            sql.push_str(&format!("SET threads TO {n};\n"));
        }
        if let Some(m) = memory_limit {
            let m = m.trim();
            if !m.is_empty() {
                sql.push_str(&format!("SET memory_limit = {};\n", quote_str(m)));
            }
        }
        if sql.is_empty() {
            return Ok(());
        }
        self.conn.execute_batch(&sql)?;
        Ok(())
    }

    /// Gives the value that DuckDB uses now for one of its settings.
    pub fn current_setting(&self, name: &str) -> Option<String> {
        self.conn
            .query_row(
                &format!("SELECT CAST(current_setting({}) AS VARCHAR)", quote_str(name)),
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
    }

    /// Reads one complete row as JSON, for the record view.
    ///
    /// The function gives `None` when the view holds no row at that offset.
    pub fn row_json(&self, view: &View, schema: &Schema, row: u64) -> Result<Option<String>> {
        let sql = view.row_json_sql(schema, row);
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        match rows.next()? {
            Some(r) => Ok(r.get::<_, Option<String>>(0)?),
            None => Ok(None),
        }
    }

    /// Finds the rows that contain `needle` in one column or more.
    ///
    /// The scan starts at the row `from_row` and examines `scan_rows` rows.
    /// The function gives the offset of each row that matches, and it gives
    /// `limit` offsets at the most. For the reason that the scan has a limit,
    /// refer to [`View::search_sql`].
    pub fn search(
        &self,
        view: &View,
        schema: &Schema,
        needle: &str,
        from_row: u64,
        scan_rows: u64,
        limit: u32,
    ) -> Result<Vec<u64>> {
        let sql = view.search_sql(schema, needle, from_row, scan_rows, limit);
        if sql.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            if let Some(n) = to_u64(&r.get::<_, Value>(0)?) {
                out.push(n);
            }
        }
        Ok(out)
    }

    /// Calculates the statistics of one column, for the column inspector.
    ///
    /// The value `top_k` gives the number of frequent values to collect. The
    /// value 0 collects none. A column of numbers also gets a histogram.
    pub fn column_stats(&self, view: &View, column: &Column, top_k: u32) -> Result<ColumnStats> {
        let sql = view.stats_sql(&column.name, column.kind);
        let mut stats: ColumnStats = self.conn.query_row(&sql, [], |r| {
            Ok(ColumnStats {
                column: column.name.clone(),
                sql_type: column.sql_type.clone(),
                kind: Some(CellKindWrapper(column.kind)),
                n_total: to_u64(&r.get::<_, Value>(0)?).unwrap_or(0),
                n_present: to_u64(&r.get::<_, Value>(1)?).unwrap_or(0),
                n_distinct: to_u64(&r.get::<_, Value>(2)?).unwrap_or(0),
                min: r.get::<_, Option<String>>(3)?,
                max: r.get::<_, Option<String>>(4)?,
                avg: r.get::<_, Option<String>>(5)?,
                std: r.get::<_, Option<String>>(6)?,
                top: Vec::new(),
                histogram: None,
            })
        })?;

        // The function approx_count_distinct gives an estimate, and the
        // estimate can be too large on a small input. A column cannot have
        // more distinct values than it has rows that are not NULL.
        stats.n_distinct = stats.n_distinct.min(stats.n_present);

        if top_k > 0 {
            stats.top = self.top_values(view, &column.name, top_k)?;
        }
        if column.kind == CellKind::Number {
            stats.histogram = self.histogram(view, &column.name, 24)?;
        }
        Ok(stats)
    }

    /// Reads the `k` most frequent values of one column.
    fn top_values(&self, view: &View, column: &str, k: u32) -> Result<Vec<(Option<String>, u64)>> {
        let sql = view.top_values_sql(column, k);
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            let v: Option<String> = r.get(0)?;
            let n = to_u64(&r.get::<_, Value>(1)?).unwrap_or(0);
            out.push((v, n));
        }
        Ok(out)
    }

    /// Counts the values of one column in `bins` buckets of equal width.
    fn histogram(&self, view: &View, column: &str, bins: u32) -> Result<Option<Histogram>> {
        let (lo, hi): (Option<f64>, Option<f64>) =
            self.conn.query_row(&view.bounds_sql(column), [], |r| {
                Ok((
                    r.get::<_, Value>(0).ok().as_ref().and_then(to_f64),
                    r.get::<_, Value>(1).ok().as_ref().and_then(to_f64),
                ))
            })?;
        let (Some(lo), Some(hi)) = (lo, hi) else {
            return Ok(None); // each value in the column is NULL
        };
        // A column with one value only has no distribution to draw. A column
        // of infinite values gives a bucket width that is not a number.
        if !lo.is_finite() || !hi.is_finite() || hi <= lo {
            return Ok(None);
        }

        let sql = view.histogram_sql(column, lo, hi, bins);
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let mut buckets = vec![0u64; bins as usize];
        while let Some(r) = rows.next()? {
            let b = to_u64(&r.get::<_, Value>(0)?).unwrap_or(0) as usize;
            let n = to_u64(&r.get::<_, Value>(1)?).unwrap_or(0);
            if b < buckets.len() {
                buckets[b] = n;
            }
        }
        Ok(Some(Histogram { lo, hi, buckets }))
    }

    /// Reads the metadata of the file.
    ///
    /// For a Parquet file, the answers come from the footer. The size of the
    /// file therefore does not change the cost of this function.
    pub fn file_meta(&self) -> Result<FileMeta> {
        let files: Vec<FileEntry> = self
            .source
            .files
            .iter()
            .map(|p| FileEntry {
                path: db_path(p),
                bytes: std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
            })
            .collect();

        let mut meta = FileMeta {
            format: self.source.format,
            files,
            bytes_on_disk: self.source.bytes,
            parquet: None,
            csv: None,
            columns: Vec::new(),
        };

        match self.source.format {
            Format::Parquet => {
                meta.parquet = Some(self.parquet_meta()?);
                meta.columns = self.parquet_column_stats().unwrap_or_default();
            }
            Format::Csv => {
                // A failure of the sniffer must not remove the full panel.
                // The other metadata is still useful to the user.
                meta.csv = self.csv_meta().ok();
            }
            // A JSON file and an Arrow file hold no footer and no dialect.
            // The panel shows the sizes, the columns and the types only.
            Format::Json | Format::Arrow => {}
        }
        Ok(meta)
    }

    /// Reads the footer facts of a Parquet file: the row count, the row group
    /// count, the sizes, the codecs, the encodings and the writer.
    fn parquet_meta(&self) -> Result<ParquetMeta> {
        let list = file_list(&self.source.files);
        let mut m = ParquetMeta::default();

        let row = self.conn.query_row(
            &format!(
                "SELECT coalesce(any_value(created_by), ''), \
                        coalesce(any_value(format_version)::VARCHAR, ''), \
                        coalesce(sum(num_rows), 0)::BIGINT, \
                        coalesce(sum(num_row_groups), 0)::BIGINT \
                 FROM parquet_file_metadata({list})"
            ),
            [],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )?;
        m.created_by = row.0;
        m.format_version = row.1;
        m.num_rows = row.2.max(0) as u64;
        m.num_row_groups = row.3.max(0) as u64;

        let sizes = self.conn.query_row(
            &format!(
                "SELECT coalesce(sum(total_compressed_size), 0)::BIGINT, \
                        coalesce(sum(total_uncompressed_size), 0)::BIGINT \
                 FROM parquet_metadata({list})"
            ),
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )?;
        m.total_compressed = sizes.0.max(0) as u64;
        m.total_uncompressed = sizes.1.max(0) as u64;

        {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT compression, count(*)::BIGINT FROM parquet_metadata({list}) \
                 GROUP BY 1 ORDER BY 2 DESC"
            ))?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let codec: Option<String> = r.get(0)?;
                let n: i64 = r.get(1)?;
                m.compression
                    .push((codec.unwrap_or_else(|| "?".into()), n.max(0) as u64));
            }
        }

        {
            // The column `encodings` holds one string for each chunk, and a
            // slash separates the names in that string. Split the string, so
            // that the panel shows each name one time.
            let mut stmt = self.conn.prepare(&format!(
                "SELECT DISTINCT encodings FROM parquet_metadata({list})"
            ))?;
            let mut rows = stmt.query([])?;
            let mut seen: Vec<String> = Vec::new();
            while let Some(r) = rows.next()? {
                let e: Option<String> = r.get(0)?;
                for part in e.unwrap_or_default().split(['/', ',']) {
                    let part = part.trim();
                    if !part.is_empty() && !seen.iter().any(|s| s == part) {
                        seen.push(part.to_string());
                    }
                }
            }
            seen.sort();
            m.encodings = seen;
        }

        {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT coalesce(sum(total_uncompressed_size), 0)::BIGINT \
                 FROM parquet_metadata({list}) GROUP BY file_name, row_group_id \
                 ORDER BY file_name, row_group_id"
            ))?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                m.row_group_bytes.push(r.get::<_, i64>(0)?.max(0) as u64);
            }
        }

        m.kv = self.parquet_kv().unwrap_or_default();
        Ok(m)
    }

    /// Reads the key/value pairs from the Parquet footer.
    fn parquet_kv(&self) -> Result<Vec<(String, String)>> {
        let list = file_list(&self.source.files);
        // The keys and the values have the type BLOB, and the function
        // decode() reads them as UTF-8 text. An embedded schema from pandas or
        // from Arrow can be tens of kilobytes long. Keep the first part only.
        let mut stmt = self.conn.prepare(&format!(
            "SELECT DISTINCT coalesce(try_cast(decode(key) AS VARCHAR), '?'), \
                    substr(coalesce(try_cast(decode(value) AS VARCHAR), '<binary>'), 1, 2000) \
             FROM parquet_kv_metadata({list}) ORDER BY 1"
        ))?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push((r.get::<_, String>(0)?, r.get::<_, String>(1)?));
        }
        Ok(out)
    }

    /// Reads the footer facts of each column of a Parquet file.
    ///
    /// The NULL counts of the row groups add together correctly. A Parquet
    /// file therefore gives an exact NULL count for each column, with no scan.
    ///
    /// This function does not combine the minimum and the maximum. The footer
    /// keeps them as text, so a comparison across two row groups compares
    /// `"9"` with `"10"` and gives a wrong result. The column inspector
    /// calculates the true minimum and maximum with a query that keeps the
    /// type of the column.
    fn parquet_column_stats(&self) -> Result<Vec<ColumnFooterStats>> {
        let list = file_list(&self.source.files);
        let mut stmt = self.conn.prepare(&format!(
            "SELECT path_in_schema, \
                    sum(stats_null_count)::BIGINT, \
                    coalesce(sum(total_compressed_size), 0)::BIGINT, \
                    coalesce(sum(total_uncompressed_size), 0)::BIGINT, \
                    string_agg(DISTINCT compression, '/'), \
                    string_agg(DISTINCT encodings, '/') \
             FROM parquet_metadata({list}) GROUP BY 1"
        ))?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(ColumnFooterStats {
                name: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                null_count: r.get::<_, Option<i64>>(1)?.map(|v| v.max(0) as u64),
                min: None,
                max: None,
                compressed: r.get::<_, i64>(2)?.max(0) as u64,
                uncompressed: r.get::<_, i64>(3)?.max(0) as u64,
                compression: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                encodings: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// Asks the DuckDB sniffer for the dialect of a CSV file.
    fn csv_meta(&self) -> Result<CsvMeta> {
        let path = quote_str(&db_path(&self.source.files[0]));
        self.conn
            .query_row(
                &format!(
                    "SELECT coalesce(Delimiter, ''), coalesce(Quote, ''), coalesce(Escape, ''), \
                            coalesce(NewLineDelimiter, ''), coalesce(SkipRows, 0)::BIGINT, \
                            coalesce(HasHeader, false), coalesce(DateFormat, ''), \
                            coalesce(TimestampFormat, ''), coalesce(Prompt, '') \
                     FROM sniff_csv({path})"
                ),
                [],
                |r| {
                    Ok(CsvMeta {
                        delimiter: r.get(0)?,
                        quote: r.get(1)?,
                        escape: r.get(2)?,
                        new_line: r.get(3)?,
                        skip_rows: r.get(4)?,
                        has_header: r.get(5)?,
                        date_format: r.get(6)?,
                        timestamp_format: r.get(7)?,
                        prompt: r.get(8)?,
                    })
                },
            )
            .map_err(Into::into)
    }

    /// Copies a file of text into a table, so that the engine can go directly
    /// to any row.
    ///
    /// Without this table, a jump to row 8,000,000 of a CSV file must parse
    /// each row before it. That cost makes a jump to the end of a large file
    /// too slow to use. A JSON file has the same cost, and the same table
    /// removes it. If the data does not fit in memory, DuckDB writes the
    /// remainder to its temporary directory. The function is then slower, but
    /// it does not fail. The engine does not write to the user's file.
    pub fn materialize(&mut self) -> Result<()> {
        if self.indexed || self.is_seekable() {
            return Ok(());
        }
        self.conn.execute_batch(&format!(
            "CREATE OR REPLACE TABLE {MAT_TABLE} AS SELECT * FROM {};\n\
             CREATE OR REPLACE VIEW src AS SELECT * FROM {MAT_TABLE};",
            self.read_expr
        ))?;
        self.indexed = true;
        Ok(())
    }

    /// Gives `true` when the engine can go directly to any row.
    ///
    /// A Parquet file and an Arrow file hold their rows in blocks, and each
    /// block has its own count. The reader can therefore go past a block that
    /// the query does not need. A file of text has no such structure, so the
    /// reader must read each row in front of the row that it needs. Such a
    /// file gives direct access after the engine indexes it.
    pub fn is_seekable(&self) -> bool {
        matches!(self.source.format, Format::Parquet | Format::Arrow) || self.indexed
    }

    /// Measures the file, so that [`crate::ddl`] can write a `CREATE TABLE`
    /// statement for another database.
    ///
    /// The function makes two queries at the most:
    ///
    /// 1. One query that measures each column. It holds four aggregates for
    ///    each column, and it reads the file one time.
    /// 2. One query that looks for a key of two columns, and only when no
    ///    single column is unique.
    ///
    /// The count of the different values is close, and not exact.
    /// `approx_count_distinct` reads the file one time and uses little
    /// memory. An exact count of each column of a large file needs much more
    /// of both. The function counts the key exactly at the end, because a key
    /// that is wrong is worse than a key that arrives slowly.
    pub fn profile(&self, view: &View, table: &str) -> Result<TableProfile> {
        let schema = self.describe(view)?;
        let rows = self.count(view)?;
        if schema.is_empty() {
            return Ok(TableProfile {
                table: table.to_string(),
                rows,
                columns: Vec::new(),
                key: Vec::new(),
                key_is_exact: true,
            });
        }

        let mut parts: Vec<String> = Vec::new();
        for c in &schema.columns {
            let id = quote_ident(&c.name);
            parts.push(format!("count({id})"));
            parts.push(format!("approx_count_distinct({id})"));
            // The length has a meaning for a column of text only. A cast of
            // a large column of numbers would cost time and give nothing.
            if matches!(c.kind, CellKind::Text | CellKind::Nested) {
                parts.push(format!("max(length(CAST({id} AS VARCHAR)))"));
            } else {
                parts.push("NULL".to_string());
            }
        }
        let sql = format!(
            "SELECT {} {}",
            parts.join(", "),
            view.scan_from()
        );

        let mut columns: Vec<ColumnProfile> = Vec::new();
        self.conn.query_row(&sql, [], |r| {
            for (i, c) in schema.columns.iter().enumerate() {
                let present = r.get::<_, Value>(i * 3).ok().and_then(|v| to_u64(&v)).unwrap_or(0);
                let distinct = r
                    .get::<_, Value>(i * 3 + 1)
                    .ok()
                    .and_then(|v| to_u64(&v))
                    .unwrap_or(0);
                let max_len = r.get::<_, Value>(i * 3 + 2).ok().and_then(|v| to_u64(&v));
                columns.push(ColumnProfile {
                    name: c.name.clone(),
                    sql_type: c.sql_type.clone(),
                    kind: c.kind,
                    nulls: rows.saturating_sub(present),
                    // The count is close. It can give one more value than
                    // the file holds, and a count above the number of rows
                    // would confuse the reader.
                    distinct: distinct.min(rows),
                    max_len,
                });
            }
            Ok(())
        })?;

        let (key, key_is_exact) = self.find_key(view, &schema, &columns, rows)?;
        Ok(TableProfile {
            table: table.to_string(),
            rows,
            columns,
            key,
            key_is_exact,
        })
    }

    /// Looks for a group of columns that is unique over each row.
    ///
    /// The function tries one column first. If no column is unique, it tries
    /// each pair of the columns that come nearest to unique. A key of three
    /// columns or more is rare, and the search for one costs much more, so
    /// the function stops at two.
    fn find_key(
        &self,
        view: &View,
        schema: &Schema,
        cols: &[ColumnProfile],
        rows: u64,
    ) -> Result<(Vec<usize>, bool)> {
        // A file with no row has no key to find.
        if rows == 0 {
            return Ok((Vec::new(), true));
        }
        let usable = |i: usize| -> bool {
            // A key holds no NULL, and no value that the database cannot
            // compare in an index. A measure is also not a key: a price or a
            // count can be unique by accident, and it is still the wrong
            // column to identify a row by.
            cols[i].nulls == 0
                && !matches!(cols[i].kind, CellKind::Binary | CellKind::Nested)
                && !crate::ddl::is_measure(&cols[i].sql_type)
        };

        // Try the columns that look like a key first, and then the columns
        // from the left. Each true key holds one different value for each
        // row, so the count of the values cannot put one candidate in front
        // of another. The name and the position can.
        let mut singles: Vec<usize> = (0..cols.len()).filter(|i| usable(*i)).collect();
        singles.sort_by_key(|i| (!crate::ddl::is_key_name(&cols[*i].name), *i));

        // The count of the values is close, so test each candidate exactly
        // before Peruse writes it into a statement.
        let mut tried = 0;
        for &i in &singles {
            // A column that is far from unique cannot become unique.
            if cols[i].distinct * 100 < rows * 99 {
                continue;
            }
            if self.exact_distinct(view, &[&schema.columns[i].name])? == rows {
                return Ok((vec![i], true));
            }
            tried += 1;
            if tried >= 4 {
                break;
            }
        }

        // Two columns. A pair is unique only when its two columns hold many
        // values between them, so the columns with the most values are the
        // candidates here. One query measures each pair, so the file is read
        // one time and not one time for each pair.
        singles.sort_by_key(|i| std::cmp::Reverse(cols[*i].distinct));
        let cand: Vec<usize> = singles.into_iter().take(6).collect();
        if cand.len() < 2 {
            return Ok((Vec::new(), true));
        }
        let pairs: Vec<(usize, usize)> = (0..cand.len())
            .flat_map(|a| (a + 1..cand.len()).map(move |b| (a, b)))
            .map(|(a, b)| (cand[a], cand[b]))
            .collect();
        let parts: Vec<String> = pairs
            .iter()
            .map(|(a, b)| {
                // Join the two values with a character that no value holds,
                // so that the pair ("ab", "c") and the pair ("a", "bc") do
                // not look the same.
                format!(
                    "approx_count_distinct(concat_ws(chr(31), CAST({} AS VARCHAR), CAST({} AS VARCHAR)))",
                    quote_ident(&schema.columns[*a].name),
                    quote_ident(&schema.columns[*b].name)
                )
            })
            .collect();
        let sql = format!(
            "SELECT {} {}",
            parts.join(", "),
            view.scan_from()
        );
        let mut best: Option<(usize, usize)> = None;
        let mut best_n = 0u64;
        self.conn.query_row(&sql, [], |r| {
            for (i, pair) in pairs.iter().enumerate() {
                let n = r.get::<_, Value>(i).ok().and_then(|v| to_u64(&v)).unwrap_or(0);
                if n > best_n {
                    best_n = n;
                    best = Some(*pair);
                }
            }
            Ok(())
        })?;

        // The count is close, so a pair that is truly unique can come back a
        // little below the number of rows. Test the best pair exactly.
        if let Some((a, b)) = best
            && best_n * 100 >= rows * 99
        {
            let names = [
                schema.columns[a].name.as_str(),
                schema.columns[b].name.as_str(),
            ];
            if self.exact_distinct(view, &names)? == rows {
                return Ok((vec![a, b], true));
            }
        }
        Ok((Vec::new(), true))
    }

    /// Counts the different values of one group of columns, exactly.
    fn exact_distinct(&self, view: &View, names: &[&str]) -> Result<u64> {
        let list: Vec<String> = names.iter().map(|n| quote_ident(n)).collect();
        let sql = format!(
            "SELECT count(*) FROM (SELECT DISTINCT {} {})",
            list.join(", "),
            view.scan_from()
        );
        let n: i64 = self.conn.query_row(&sql, [], |r| r.get(0))?;
        Ok(n.max(0) as u64)
    }

    /// Gives the `read_parquet` call or `read_csv` call that reads this file.
    ///
    /// The metadata panel shows this text. The user can copy the text into a
    /// script and get the same rows outside Peruse.
    pub fn read_expr(&self) -> &str {
        &self.read_expr
    }
}

/// Applies the options to the connection before the engine reads any data.
fn configure(conn: &Connection, opts: &OpenOptions) -> Result<()> {
    let threads = opts.threads.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    });
    let tmp = std::env::temp_dir().join("peruse");
    // The two extension options are necessary for the read-only promise.
    //
    // The bundled DuckDB build sets `autoinstall` and `autoload` to true. A
    // statement that names an `https://` or `s3://` path is a legal read, and
    // the guard in `crate::sql_guard` correctly accepts it. DuckDB then
    // downloads the `httpfs` extension, writes about 28 MB into the home
    // directory of the user, loads that machine code, and sends a request to
    // the network. Peruse must not do any of these three things.
    //
    // With the two options set to false, the same statement stops with the
    // message "Missing Extension Error", and DuckDB downloads nothing.
    let mut setup = format!(
        "SET autoinstall_known_extensions = false;\n\
         SET autoload_known_extensions = false;\n\
         SET threads TO {threads};\n\
         SET enable_progress_bar = false;\n\
         SET preserve_insertion_order = true;\n\
         SET temp_directory = {};\n",
        quote_str(&tmp.to_string_lossy().replace('\\', "/"))
    );
    if let Some(limit) = &opts.memory_limit {
        setup.push_str(&format!("SET memory_limit = {};\n", quote_str(limit)));
    }
    conn.execute_batch(&setup).context("configuring DuckDB")?;
    Ok(())
}

/// Expands a glob pattern, or accepts a path to one file.
///
/// This function calls the DuckDB function `glob()`. The set of files
/// therefore agrees with the set that `read_parquet` reads from the same
/// pattern.
fn resolve_files(conn: &Connection, input: &str) -> Result<Vec<PathBuf>> {
    let literal = PathBuf::from(input);
    if !source::looks_like_glob(input) {
        if literal.is_file() {
            return Ok(vec![literal]);
        }
        if literal.is_dir() {
            bail!(
                "{input:?} is a directory — point at a file, or use a glob like {input}/*.parquet"
            );
        }
        bail!("{input:?} not found");
    }

    let pattern = input.replace('\\', "/");
    let mut stmt = conn.prepare("SELECT file FROM glob(?)")?;
    let mut rows = stmt.query([&pattern])?;
    let mut files = Vec::new();
    while let Some(r) = rows.next()? {
        files.push(PathBuf::from(r.get::<_, String>(0)?));
    }
    files.sort();
    Ok(files)
}

/// Builds the `read_parquet` call or the `read_csv` call for this source.
fn build_read_expr(src: &Source, opts: &OpenOptions) -> String {
    let list = file_list(&src.files);
    match src.format {
        Format::Parquet => {
            let mut args = vec![list];
            if src.is_multi() {
                // The files of one set often hold their columns in a
                // different order.
                args.push("union_by_name = true".into());
            }
            format!("read_parquet({})", args.join(", "))
        }
        Format::Csv => {
            let mut args = vec![list, "auto_detect = true".into()];
            if let Some(d) = src.delimiter {
                // The function read_csv reads an escape sequence in the
                // option `delim`. A tab therefore stays a tab when it goes
                // through SQL as `\t`.
                let lit = if d == '\t' {
                    "'\\t'".to_string()
                } else {
                    quote_str(&d.to_string())
                };
                args.push(format!("delim = {lit}"));
            }
            if let Some(h) = opts.header {
                args.push(format!("header = {h}"));
            }
            if opts.all_varchar {
                args.push("all_varchar = true".into());
            }
            if opts.ignore_errors {
                args.push("ignore_errors = true".into());
            }
            if let Some(n) = opts.sample_size {
                args.push(format!("sample_size = {n}"));
            }
            if src.is_multi() {
                args.push("union_by_name = true".into());
            }
            format!("read_csv({})", args.join(", "))
        }
        // The JSON reader finds the form of the file itself: one object for
        // each row, one list of objects, or one object with a list inside it.
        // A nested value stays nested, and the grid shows it as text.
        Format::Json => {
            // The reader of DuckDB walks a value with one call for each
            // level, in C++. A file that nests a thousand levels deep
            // therefore fills the stack of the thread and stops the whole
            // program, and no Rust code can catch that.
            //
            // With a limit, the reader stops at that level and gives the
            // value below it as text. A deep file then opens, and the record
            // view shows the remainder as one JSON value. That is a poor
            // view of a strange file, and it is much better than a program
            // that goes away.
            let mut args = vec![list, format!("maximum_depth = {MAX_JSON_DEPTH}")];
            if let Some(n) = opts.sample_size {
                // The reader looks at this number of rows before it decides
                // the types. The value -1 reads the whole file.
                args.push(format!("sample_size = {n}"));
            }
            if src.is_multi() {
                args.push("union_by_name = true".into());
            }
            format!("read_json_auto({})", args.join(", "))
        }
        // `Engine::open` refuses this format in front of this function, so
        // the value never arrives here.
        Format::Arrow => unreachable!("Engine::open refuses an Arrow file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("peruse-test-{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_csv(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    const SAMPLE: &str = "id,name,amount,when\n\
                          1,alice,10.5,2024-01-01\n\
                          2,bob,,2024-01-02\n\
                          3,carol,30.25,2024-01-03\n";

    #[test]
    fn the_profile_measures_each_column() {
        let d = tmpdir("profile");
        let p = write_csv(&d, "orders.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let prof = e.profile(&View::default(), "orders").unwrap();

        assert_eq!(prof.rows, 3);
        assert_eq!(prof.columns.len(), 4);
        let by = |n: &str| prof.columns.iter().find(|c| c.name == n).unwrap().clone();
        assert_eq!(by("id").nulls, 0);
        assert_eq!(by("id").distinct, 3);
        // The third row of the sample holds no amount.
        assert_eq!(by("amount").nulls, 1);
        // A length has a meaning for a column of text only.
        assert_eq!(by("name").max_len, Some(5));
        assert_eq!(by("id").max_len, None);
    }

    #[test]
    fn the_profile_finds_a_key_of_one_column() {
        let d = tmpdir("profile-key");
        let p = write_csv(&d, "t.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let prof = e.profile(&View::default(), "t").unwrap();
        assert_eq!(prof.key, vec![0], "the column `id` is the key");
        assert!(prof.key_is_exact);
    }

    #[test]
    fn the_profile_finds_a_key_of_two_columns() {
        // No column of this file is unique, and the pair (store, day) is.
        let d = tmpdir("profile-composite");
        let p = write_csv(
            &d,
            "sales.csv",
            "store,day,units\n\
             A,2024-01-01,5\n\
             A,2024-01-02,5\n\
             B,2024-01-01,3\n\
             B,2024-01-02,3\n",
        );
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let prof = e.profile(&View::default(), "sales").unwrap();
        assert_eq!(prof.key, vec![0, 1], "the pair (store, day) is the key");
    }

    #[test]
    fn a_column_that_holds_a_null_is_not_a_key() {
        // The column `amount` has three different values over three rows in
        // the eye of a count, because a NULL counts for nothing. A key must
        // still not hold a NULL.
        let d = tmpdir("profile-null-key");
        let p = write_csv(
            &d,
            "t.csv",
            "a,b\n1,x\n2,\n3,z\n",
        );
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let prof = e.profile(&View::default(), "t").unwrap();
        assert!(!prof.key.contains(&1), "a column with a NULL became a key");
    }

    #[test]
    fn the_profile_follows_the_filter_of_the_view() {
        let d = tmpdir("profile-filter");
        let p = write_csv(&d, "t.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let view = View {
            filter: Some("id > 1".into()),
            ..Default::default()
        };
        let prof = e.profile(&view, "t").unwrap();
        assert_eq!(prof.rows, 2, "the profile must measure the filtered rows");
    }

    #[test]
    fn a_file_with_no_row_gives_a_profile_with_no_key() {
        let d = tmpdir("profile-empty");
        let p = write_csv(&d, "t.csv", "a,b\n");
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let prof = e.profile(&View::default(), "t").unwrap();
        assert_eq!(prof.rows, 0);
        assert!(prof.key.is_empty());
    }

    #[test]
    fn opens_json_lines_and_reads_schema_and_rows() {
        let d = tmpdir("ndjson");
        let p = write_csv(
            &d,
            "events.ndjson",
            "{\"id\": 1, \"user\": \"alice\", \"score\": 10.5}\n\
             {\"id\": 2, \"user\": \"bob\", \"score\": null}\n\
             {\"id\": 3, \"user\": \"carol\", \"score\": 30.25}\n",
        );
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        assert_eq!(e.source.format, Format::Json);

        let schema = e.describe(&View::default()).unwrap();
        let names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "user", "score"]);
        assert_eq!(schema.columns[2].kind, CellKind::Number);

        let page = e.page(&View::default(), &schema, 10, 0).unwrap();
        assert_eq!(page.nrows, 3);
        assert_eq!(page.cell(0, 1), Some("alice"));
        // A JSON null is a SQL NULL, and not the text "null".
        assert_eq!(page.cell(1, 2), None);
    }

    #[test]
    fn opens_a_json_file_that_holds_one_list_of_objects() {
        let d = tmpdir("json-array");
        let p = write_csv(
            &d,
            "rows.json",
            "[{\"a\": 1, \"b\": \"x\"}, {\"a\": 2, \"b\": \"y\"}]",
        );
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let schema = e.describe(&View::default()).unwrap();
        let page = e.page(&View::default(), &schema, 10, 0).unwrap();
        assert_eq!(page.nrows, 2);
        assert_eq!(page.cell(1, 1), Some("y"));
    }

    #[test]
    fn a_json_file_is_not_seekable_until_it_is_indexed() {
        // A file of text has no block structure, so a jump to a far row must
        // read each row in front of it. The index removes that cost.
        let d = tmpdir("json-index");
        let p = write_csv(&d, "a.ndjson", "{\"a\": 1}\n{\"a\": 2}\n");
        let mut e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        assert!(!e.is_seekable());
        e.materialize().unwrap();
        assert!(e.is_seekable());
        let schema = e.describe(&View::default()).unwrap();
        assert_eq!(e.page(&View::default(), &schema, 10, 0).unwrap().nrows, 2);
    }

    #[test]
    fn an_arrow_file_gives_a_message_that_says_what_to_do() {
        // This build of DuckDB holds no reader for an Arrow IPC file. The
        // message must say so in words that the user can act on, and it must
        // not name a SQL function that the user never wrote.
        let d = tmpdir("arrow");
        let p = d.join("data.arrow");
        std::fs::write(&p, b"ARROW1\x00\x00rest of the file").unwrap();
        let msg = match Engine::open(p.to_str().unwrap(), &OpenOptions::default()) {
            Ok(_) => panic!("an Arrow file must not open"),
            Err(e) => format!("{e:#}"),
        };
        assert!(msg.contains("Arrow"), "{msg}");
        assert!(msg.contains("parquet"), "no way forward given: {msg}");
    }

    #[test]
    fn opens_csv_and_reads_schema_and_rows() {
        let d = tmpdir("open-csv");
        let p = write_csv(&d, "s.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        assert_eq!(e.source.format, Format::Csv);

        let view = View::default();
        let schema = e.describe(&view).unwrap();
        assert_eq!(
            schema.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["id", "name", "amount", "when"]
        );
        assert_eq!(schema.columns[0].kind, CellKind::Number);
        assert_eq!(schema.columns[1].kind, CellKind::Text);
        assert_eq!(schema.columns[3].kind, CellKind::Temporal, "date sniffed");

        assert_eq!(e.count(&view).unwrap(), 3);
        let page = e.page(&view, &schema, 10, 0).unwrap();
        assert_eq!(page.nrows, 3);
        assert_eq!(page.cell(0, 1), Some("alice"));
        assert_eq!(page.cell(1, 2), None, "empty CSV field is NULL");
    }

    #[test]
    fn paging_offsets_line_up() {
        let d = tmpdir("paging");
        let mut body = String::from("i\n");
        for i in 0..500 {
            body.push_str(&format!("{i}\n"));
        }
        let p = write_csv(&d, "n.csv", &body);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let view = View::default();
        let schema = e.describe(&view).unwrap();

        let page = e.page(&view, &schema, 10, 250).unwrap();
        assert_eq!(page.offset, 250);
        assert_eq!(page.cell(0, 0), Some("250"));
        assert_eq!(page.cell(9, 0), Some("259"));
        assert_eq!(page.abs_row(9), 259);

        // A page after the last row is empty. It is not an error.
        let tail = e.page(&view, &schema, 10, 9999).unwrap();
        assert_eq!(tail.nrows, 0);
    }

    #[test]
    fn filter_and_sort_change_what_pages_return() {
        let d = tmpdir("filter");
        let p = write_csv(&d, "s.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let schema = e.describe(&View::default()).unwrap();

        let filtered = View {
            filter: Some("amount IS NOT NULL".into()),
            sort: vec![crate::query::SortKey {
                column: "amount".into(),
                dir: crate::query::SortDir::Desc,
            }],
            ..Default::default()
        };
        assert_eq!(e.count(&filtered).unwrap(), 2);
        let page = e.page(&filtered, &schema, 10, 0).unwrap();
        assert_eq!(page.cell(0, 1), Some("carol"), "highest amount first");
        assert_eq!(page.cell(1, 1), Some("alice"));
    }

    #[test]
    fn column_stats_are_computed() {
        let d = tmpdir("stats");
        let p = write_csv(&d, "s.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let view = View::default();
        let schema = e.describe(&view).unwrap();

        let amount = &schema.columns[2];
        let s = e.column_stats(&view, amount, 5).unwrap();
        assert_eq!(s.n_total, 3);
        assert_eq!(s.n_present, 2);
        assert_eq!(s.null_count(), 1);
        assert_eq!(s.min.as_deref(), Some("10.5"));
        assert_eq!(s.max.as_deref(), Some("30.25"));
        assert!(s.histogram.is_some(), "numeric columns get a histogram");

        // The query must not use avg() on a column of text.
        let name = &schema.columns[1];
        let s = e.column_stats(&view, name, 5).unwrap();
        assert_eq!(s.n_distinct, 3);
        assert!(s.avg.is_none());
        assert_eq!(s.top.len(), 3);
    }

    #[test]
    fn distinct_estimate_never_exceeds_row_count() {
        let d = tmpdir("distinct");
        let p = write_csv(&d, "s.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let view = View::default();
        let schema = e.describe(&view).unwrap();
        for c in &schema.columns {
            let s = e.column_stats(&view, c, 0).unwrap();
            assert!(s.n_distinct <= s.n_present, "column {}", c.name);
        }
    }

    #[test]
    fn search_returns_offsets_that_match_paging() {
        let d = tmpdir("search");
        let p = write_csv(&d, "s.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let view = View::default();
        let schema = e.describe(&view).unwrap();

        let hits = e.search(&view, &schema, "carol", 0, 1000, 10).unwrap();
        assert_eq!(hits, vec![2]);
        // The offset must select the same row when the pager reads it.
        let page = e.page(&view, &schema, 1, hits[0]).unwrap();
        assert_eq!(page.cell(0, 1), Some("carol"));

        assert!(e.search(&view, &schema, "nobody", 0, 1000, 10).unwrap().is_empty());
        // The search ignores the case of the letters.
        assert_eq!(e.search(&view, &schema, "ALICE", 0, 1000, 10).unwrap(), vec![0]);
    }

    #[test]
    fn search_offsets_respect_the_active_sort() {
        let d = tmpdir("search-sorted");
        let p = write_csv(&d, "s.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let schema = e.describe(&View::default()).unwrap();
        let view = View {
            sort: vec![crate::query::SortKey {
                column: "id".into(),
                dir: crate::query::SortDir::Desc,
            }],
            ..Default::default()
        };
        let hits = e.search(&view, &schema, "alice", 0, 1000, 10).unwrap();
        assert_eq!(hits, vec![2], "alice is last when id descends");
        let page = e.page(&view, &schema, 1, hits[0]).unwrap();
        assert_eq!(page.cell(0, 1), Some("alice"));
    }

    #[test]
    fn tsv_delimiter_survives_sql_quoting() {
        let d = tmpdir("tsv");
        let p = write_csv(&d, "s.tsv", "a\tb\n1\thello world\n");
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        assert_eq!(e.source.delimiter, Some('\t'));
        let schema = e.describe(&View::default()).unwrap();
        assert_eq!(schema.len(), 2, "tab split into two columns");
        let page = e.page(&View::default(), &schema, 5, 0).unwrap();
        assert_eq!(page.cell(0, 1), Some("hello world"));
    }

    #[test]
    fn csv_indexing_preserves_contents_and_enables_seeking() {
        let d = tmpdir("materialize");
        let p = write_csv(&d, "s.csv", SAMPLE);
        let mut e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        assert!(!e.is_seekable(), "raw CSV is a stream");

        let view = View::default();
        let before = e.describe(&view).unwrap();
        e.materialize().unwrap();
        assert!(e.is_seekable());

        let after = e.describe(&view).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(e.count(&view).unwrap(), 3);
        let page = e.page(&view, &after, 10, 1).unwrap();
        assert_eq!(page.cell(0, 1), Some("bob"));

        // A second call makes no change.
        e.materialize().unwrap();
        assert_eq!(e.count(&view).unwrap(), 3);
    }

    #[test]
    fn the_engine_does_not_download_an_extension() {
        // The read-only promise depends on this test.
        //
        // The bundled DuckDB build starts with `autoinstall` and `autoload`
        // set to true. A statement that names an `https://` path is a legal
        // read, and the guard accepts it. DuckDB then downloads about 28 MB
        // into the home directory of the user, loads that machine code, and
        // sends a request to the network.
        //
        // The engine sets both options to false. The statement must therefore
        // stop with a message about a missing extension, and not with a
        // message from the network.
        let d = tmpdir("no-extension");
        let p = write_csv(&d, "s.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();

        for (name, value) in [
            ("autoinstall_known_extensions", false),
            ("autoload_known_extensions", false),
        ] {
            let got: bool = e
                .conn
                .query_row(&format!("SELECT current_setting('{name}')"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(got, value, "setting {name}");
        }

        let view = View {
            base: crate::query::Base::Sql(
                "SELECT * FROM read_csv('https://example.com/nope.csv')".into(),
            ),
            ..Default::default()
        };
        let err = e.describe(&view).unwrap_err().to_string();
        assert!(
            err.contains("Extension") || err.contains("extension"),
            "expected a missing extension error, got: {err}"
        );
        assert!(
            !err.contains("HTTP"),
            "the engine reached the network: {err}"
        );
    }

    #[test]
    fn missing_file_reports_clearly() {
        let err = Engine::open("definitely-not-here.parquet", &OpenOptions::default())
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(err.contains("not found"), "got {err}");
    }

    #[test]
    fn glob_matching_nothing_reports_clearly() {
        let d = tmpdir("empty-glob");
        let pattern = format!("{}/*.parquet", d.to_string_lossy());
        let err = Engine::open(&pattern, &OpenOptions::default())
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(err.contains("no files matched"), "got {err}");
    }

    #[test]
    fn glob_unions_multiple_files() {
        let d = tmpdir("glob-multi");
        write_csv(&d, "a.csv", "id\n1\n2\n");
        write_csv(&d, "b.csv", "id\n3\n");
        let pattern = format!("{}/*.csv", d.to_string_lossy());
        let e = Engine::open(&pattern, &OpenOptions::default()).unwrap();
        assert_eq!(e.source.files.len(), 2);
        assert!(e.source.is_multi());
        assert_eq!(e.count(&View::default()).unwrap(), 3);
    }

    #[test]
    fn all_varchar_disables_type_inference() {
        let d = tmpdir("all-varchar");
        let p = write_csv(&d, "s.csv", SAMPLE);
        let opts = OpenOptions {
            all_varchar: true,
            ..Default::default()
        };
        let e = Engine::open(p.to_str().unwrap(), &opts).unwrap();
        let schema = e.describe(&View::default()).unwrap();
        assert!(schema.columns.iter().all(|c| c.kind == CellKind::Text));
    }

    #[test]
    fn cell_inspector_returns_the_full_value() {
        let d = tmpdir("cell");
        let long = "x".repeat(10_000);
        let p = write_csv(&d, "s.csv", &format!("a,b\n1,{long}\n"));
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let view = View::default();
        let schema = e.describe(&view).unwrap();

        let page = e.page(&view, &schema, 1, 0).unwrap();
        assert_eq!(
            page.cell(0, 1).unwrap().chars().count(),
            crate::model::MAX_CELL_CHARS,
            "grid truncates"
        );
        assert!(page.is_truncated(0, 1));

        let full = e.cell(&view, "b", 0).unwrap().unwrap();
        assert_eq!(full.chars().count(), 10_000, "inspector does not");
    }

    #[test]
    fn blob_columns_are_summarised_not_transferred() {
        let d = tmpdir("blob");
        let p = write_csv(&d, "s.csv", "a\n1\n");
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        // Make a BLOB value with SQL. A binary test file is not necessary.
        let view = View {
            base: crate::query::Base::Sql(
                "SELECT 'hello'::BLOB AS payload, 1 AS n".into(),
            ),
            ..Default::default()
        };
        let schema = e.describe(&view).unwrap();
        assert_eq!(schema.columns[0].kind, CellKind::Binary);
        let page = e.page(&view, &schema, 1, 0).unwrap();
        assert_eq!(page.cell(0, 0), Some("blob 5 B"));
    }

    #[test]
    fn sql_base_views_page_and_count() {
        let d = tmpdir("sqlbase");
        let p = write_csv(&d, "s.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let view = View {
            base: crate::query::Base::Sql(
                "SELECT name, amount * 2 AS doubled FROM src WHERE amount IS NOT NULL".into(),
            ),
            ..Default::default()
        };
        let schema = e.describe(&view).unwrap();
        assert_eq!(schema.len(), 2);
        assert_eq!(e.count(&view).unwrap(), 2);
        let page = e.page(&view, &schema, 10, 0).unwrap();
        assert_eq!(page.cell(0, 1), Some("21.0"));
    }

    #[test]
    fn csv_metadata_reports_the_sniffed_dialect() {
        let d = tmpdir("csvmeta");
        let p = write_csv(&d, "s.tsv", "a\tb\n1\t2\n");
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let meta = e.file_meta().unwrap();
        assert_eq!(meta.format, Format::Csv);
        assert!(meta.parquet.is_none());
        let csv = meta.csv.expect("sniffer ran");
        assert!(csv.has_header);
        assert!(csv.delimiter.contains('\t') || csv.delimiter.contains("\\t"));
    }

    #[test]
    fn nested_types_render_without_special_casing() {
        let d = tmpdir("nested");
        let p = write_csv(&d, "s.csv", "a\n1\n");
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let view = View {
            base: crate::query::Base::Sql(
                "SELECT [1,2,3] AS xs, {'k': 'v'} AS rec".into(),
            ),
            ..Default::default()
        };
        let schema = e.describe(&view).unwrap();
        assert!(schema.columns.iter().all(|c| c.kind == CellKind::Nested));
        let page = e.page(&view, &schema, 1, 0).unwrap();
        assert_eq!(page.cell(0, 0), Some("[1, 2, 3]"));
        assert_eq!(page.cell(0, 1), Some("{'k': v}"));
    }
}

