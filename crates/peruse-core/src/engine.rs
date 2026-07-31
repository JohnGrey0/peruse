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
//! A database file is the one source that DuckDB itself opens. The engine
//! attaches it with the flag `READ_ONLY`, so the storage engine refuses each
//! write to the file. The promise is therefore stronger there than for a data
//! file: the database enforces it, and not the guard over the words of a
//! statement.
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

/// The name that Peruse gives to an attached database.
///
/// The name starts with two low lines, in the same way as the table of an
/// indexed file. A table of the user cannot take the name away, because the
/// name belongs to the database and not to a table inside it. The metadata
/// panel shows the name, so a user can read a second table of the same
/// database in the SQL prompt.
const DB_ALIAS: &str = "__peruse_db";

/// The number of table names that a message names before it stops.
///
/// A database can hold hundreds of tables, and a message of hundreds of names
/// helps nobody.
const NAMES_IN_MESSAGE: usize = 12;

/// The deepest level that the reader of a JSON file examines.
///
/// The reader is a C++ function that calls itself one time for each level. A
/// file that nests deeper than the stack of the thread allows therefore stops
/// the program, and Rust cannot catch that fault.
///
/// A real file of data nests some levels, and not a thousand. This limit is
/// far above each such file, and far below the level that fills the stack.
const MAX_JSON_DEPTH: u32 = 128;

/// The number of rows above which the statistics leave out the most frequent
/// values of a column where almost each value is different.
///
/// That query groups every row of the view, and it is the slow part of the
/// panel: on ten million rows it costs 300 milliseconds, and each other query
/// of the panel costs 30. Below this number of rows the query costs nothing,
/// and the list is still worth a look.
const TOP_VALUES_MIN_ROWS: u64 = 100_000;

/// The number of rows that a search reads in front of the cursor before it reads
/// the remainder of its part.
///
/// The value covers some screens of rows. A match inside it therefore answers
/// the usual search, and the search does not read the rest of the part. Refer to
/// [`Engine::search`].
const SEARCH_WINDOW: u64 = 8192;

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
    /// The table of a database that the view `src` reads.
    ///
    /// A database holds many tables, and the grid shows one of them. The
    /// option `--table` gives the name, as `sales` or as `main.sales`. The
    /// value has no meaning for a file, because a file holds one table.
    pub table: Option<String>,
}

/// One table or one view of a database that Peruse can open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbTable {
    /// The schema that holds it, such as `main`.
    pub schema: String,
    /// The name of the table or the view.
    pub name: String,
    /// `true` for a view.
    pub is_view: bool,
    /// The number of rows, when the catalog of the database holds that number
    /// already.
    ///
    /// The value is `None` for a view, and for a table that the catalog does
    /// not measure. Peruse never counts the rows of a table to fill a list: a
    /// database can hold hundreds of tables, and one count of each of them
    /// would read the whole database before the first frame.
    pub rows: Option<u64>,
}

impl DbTable {
    /// Gives the name that a person reads, such as `main.sales`.
    pub fn label(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }

    /// Gives the name that a statement needs: the database, the schema and the
    /// table, each in quotation marks.
    ///
    /// The marks make a name with a space, with a capital letter or with a
    /// full stop work in the same way as a plain name.
    pub fn qualified(&self) -> String {
        format!(
            "{}.{}.{}",
            quote_ident(DB_ALIAS),
            quote_ident(&self.schema),
            quote_ident(&self.name)
        )
    }

    /// Gives `true` when the text names this table.
    ///
    /// The text is `sales` or `main.sales`. DuckDB reads an identifier without
    /// regard to the case of its letters, and so does this test.
    fn is_named(&self, want: &str) -> bool {
        let want = want.trim();
        self.name.eq_ignore_ascii_case(want) || self.label().eq_ignore_ascii_case(want)
    }
}

/// What the detail band of the grid knows about one column.
///
/// The band draws a few short rows under the row of column names. It needs
/// fewer facts than the column inspector, and it needs them for every column
/// that the grid draws, so the facts of all the columns arrive in one answer.
/// Refer to [`Engine::column_band`].
///
/// The facts describe the current view. A filter or a statement of the user
/// therefore changes them, exactly as it changes the rows.
#[derive(Clone, Debug, Default)]
pub struct ColumnBrief {
    /// The name of the column.
    pub column: String,
    /// The number of rows in the view.
    pub n_total: u64,
    /// The number of rows where the value is not NULL.
    pub n_present: u64,
    /// The estimated number of different values.
    ///
    /// The value is `None` when nothing measured it. The footer of a Parquet
    /// file gives the count of the rows and the count of the NULL values, and it
    /// gives no count of the different values. Refer to [`footer_briefs`].
    pub n_distinct: Option<u64>,
    /// The smallest value, as text. The value is `None` when the type of the
    /// column has no order, and when each value of the column is NULL.
    pub min: Option<String>,
    /// The largest value, as text.
    pub max: Option<String>,
}

impl ColumnBrief {
    /// Gives the number of rows where the value is NULL.
    pub fn null_count(&self) -> u64 {
        self.n_total.saturating_sub(self.n_present)
    }

    /// Gives the percentage of the rows where the value is NULL.
    pub fn null_pct(&self) -> f64 {
        if self.n_total == 0 {
            0.0
        } else {
            self.null_count() as f64 * 100.0 / self.n_total as f64
        }
    }
}

/// Builds the facts of the detail band from the footer of a Parquet file.
///
/// The footer holds the number of rows of the file and the number of NULL values
/// of each column, and [`Engine::file_meta`] reads them with no scan of the
/// data. The compact band shows the type and the NULL share only, so those facts
/// are all that it needs. The band then costs no query at all, also on a file of
/// some gigabytes.
///
/// The function gives `None` when the footer cannot answer for every column:
///
/// * The file is not a Parquet file, so it has no footer.
/// * The footer names a value inside a structure by its path, such as
///   `actor.login`. It therefore holds no row for the column `actor`.
/// * The writer of the file left the NULL count out.
///
/// The caller then measures the columns with [`Engine::column_band`]. The facts
/// of the footer describe the whole file, so a caller must not use them for a
/// view with a filter or with a statement of the user.
pub fn footer_briefs(meta: &FileMeta, columns: &[Column]) -> Option<Vec<ColumnBrief>> {
    let n_total = meta.parquet.as_ref()?.num_rows;
    let mut out = Vec::with_capacity(columns.len());
    for c in columns {
        let f = meta.columns.iter().find(|f| f.name == c.name)?;
        let nulls = f.null_count?;
        out.push(ColumnBrief {
            column: c.name.clone(),
            n_total,
            n_present: n_total.saturating_sub(nulls),
            n_distinct: None,
            min: None,
            max: None,
        });
    }
    Some(out)
}

/// One open connection to DuckDB, and the file or the files behind it.
pub struct Engine {
    conn: Connection,
    /// The file or the files that the engine reads.
    pub source: Source,
    /// The `read_parquet(...)` call or `read_csv(...)` call behind the view
    /// `src`. Each statement of Peruse reads the file through this call.
    ///
    /// For a file of text, the call names the dialect and every column, so that
    /// DuckDB does not examine the file again for each statement.
    scan_expr: String,
    /// The short form of the read call, for the metadata panel.
    ///
    /// The text of `scan_expr` runs to some kilobytes on a wide file of text. A
    /// panel cannot show such a text, and a redraw must not wrap it. This field
    /// holds the call that finds the columns for itself. It reads the same rows,
    /// because Peruse takes the columns of `scan_expr` from that same reader.
    read_expr: String,
    /// True after the engine copies a CSV file into a table.
    pub indexed: bool,
    /// The name and the type of each column of the whole file.
    ///
    /// Each statement against a file of text makes DuckDB read the file with
    /// its sniffer again, and that sniffer is the slow part of an open. On a
    /// file of 1000 columns one `DESCRIBE` costs four seconds. The open
    /// operation reads the schema one time and keeps it here, so a caller
    /// that asks for the schema of the whole file waits for nothing.
    ///
    /// A filter and a sort do not change the columns, so this value serves
    /// each view that reads the file itself. A view that holds a statement
    /// of the user has its own columns, and it reads them each time.
    base_schema: Schema,
    /// The dialect of a CSV file, from the sniffer call of the open operation.
    ///
    /// The metadata panel shows these values. With this copy, the panel needs
    /// no second sniffer call, and that call costs 50 milliseconds on a file of
    /// 258 MB. The value is `None` for a format that has no dialect, and for a
    /// set of CSV files.
    csv_dialect: Option<CsvMeta>,
    /// The table that the view `src` reads, for a database source.
    ///
    /// The value is `None` for a file. A file holds one table, and that table
    /// has no name of its own.
    table: Option<DbTable>,
}

/// What the DuckDB CSV sniffer found about a file.
struct CsvSniff {
    /// The dialect, in the form that the metadata panel shows. The text of each
    /// character is the text of the sniffer, and not a normal form.
    meta: CsvMeta,
    /// The character that starts a comment row, as the sniffer writes it.
    /// [`CsvMeta`] does not hold this value, because no panel shows it.
    comment: String,
    /// The name and the SQL type of each column, in the order of the file.
    columns: Vec<(String, String)>,
}

/// Changes each backslash in a path to a forward slash.
///
/// DuckDB accepts a forward slash on each platform. In a glob pattern, DuckDB
/// reads a backslash as an escape character and then removes it.
fn db_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Turns the message of a failed open into a message that names the options of
/// Peruse.
///
/// DuckDB writes a long message when its sniffer cannot read a file of text. That
/// message is correct, and it lists about ten fixes in the words of DuckDB:
/// `ignore_errors=true`, `strict_mode=false`, `delimiter=','`. Peruse has an
/// option for each fix that a user of Peruse can apply, and the message of DuckDB
/// names none of them. This function therefore says what to type.
///
/// The words of DuckDB stay at the end. The first lines of that message name the
/// delimiters and the quotation marks that the sniffer tried, and a reader who
/// knows DuckDB wants to see them.
///
/// Each other failure keeps its own message with the name of the file, because
/// only the sniffer has this list of fixes.
fn open_error(input: &str, message: &str) -> anyhow::Error {
    let lower = message.to_ascii_lowercase();
    let from_the_sniffer = lower.contains("automatically detect")
        || lower.contains("sniffing")
        || lower.contains("dialect");
    if !from_the_sniffer {
        return anyhow::anyhow!("{message}").context(format!("opening {input}"));
    }
    anyhow::anyhow!(
        "Peruse cannot work out how {input} is written.\n\n\
         The reader could not find the character between two columns, the \
         quotation mark and the types together. These options tell it what to \
         use:\n\n  \
         --delimiter ';'    the character between two columns. Also tab or space.\n  \
         --no-header        the first row holds data, and not the names.\n  \
         --all-varchar      read every column as text, and guess no type.\n  \
         --ignore-errors    leave out a row that does not fit the others.\n  \
         --sample-size -1   read the whole file before deciding. This is slower.\n\n\
         Two other things give this message. A file of text must use one kind of \
         line ending, and this file can hold both kinds. The file can also be in \
         an encoding that is not UTF-8.\n\n\
         The reader said:\n{}",
        first_lines(message, 3)
    )
}

/// Gives the first `n` lines of a message, with the empty lines left out.
///
/// The full message of DuckDB runs to about twenty lines. The first lines hold
/// the reason, and the rest is the list of fixes that [`open_error`] replaces.
///
/// A line that ends with a colon introduces the lines below it, and those lines
/// are the ones that this function leaves out. Such a line at the end would
/// promise a list that is not there, so the function drops it.
fn first_lines(message: &str, n: usize) -> String {
    let mut kept: Vec<&str> = message
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(n)
        .collect();
    while kept.last().is_some_and(|l| l.ends_with(':')) {
        kept.pop();
    }
    kept.join("\n")
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
            // `Engine::open_database` names the table as soon as it chooses
            // one. A file holds one table, and it keeps this value empty.
            table: None,
        };

        // A SQLite file is a database, and the reader for it is an extension
        // that this build does not hold. Without this message, the file would
        // go to the CSV reader, and the user would see a parse failure about a
        // line of binary data.
        if src.format == Format::Sqlite {
            bail!(
                "{}: this is a SQLite database, and Peruse cannot read one yet.\n\
                 Write a table out first, for example:\n  \
                 sqlite3 {} -header -csv \"SELECT * FROM your_table\" > out.csv",
                src.label,
                src.label
            );
        }

        // A glob names a set of files, and each file of a set becomes rows of
        // one table. Two databases cannot join in that way, and a database is
        // not a file that a table function reads at all.
        if src.format == Format::DuckDb && (src.is_multi() || source::looks_like_glob(input)) {
            bail!(
                "{input:?}: Peruse opens one database at a time.\n\
                 Name the database file itself, and choose the table inside it."
            );
        }

        // A database is not a file that a table function reads. Peruse
        // attaches it and points the view `src` at one of its tables. Each
        // other part of Peruse then works with no change.
        if src.format == Format::DuckDb {
            return Engine::open_database(conn, src, opts);
        }

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

        let plain = build_read_expr(&src, opts);
        let mut engine = Engine {
            conn,
            source: src,
            scan_expr: String::new(),
            read_expr: plain,
            indexed: false,
            base_schema: Schema::default(),
            csv_dialect: None,
            table: None,
        };

        // A file of text and a file of JSON hold no schema. DuckDB therefore
        // examines the file to find the columns, and it does that again for
        // every statement. That examination is the slow part of each request:
        // on a CSV file of 258 MB, one page of 50 rows costs 100 milliseconds,
        // and 92 of those are the examination.
        //
        // Peruse examines the file one time and writes the columns into the read
        // call. Each later statement then reads the file directly, and the same
        // page costs 8 milliseconds. Refer to [`Engine::pin_csv`] and to
        // [`Engine::pin_json`].
        //
        // A set of files needs `union_by_name`, and two files of one set can
        // hold different columns. One column list cannot serve them all, so a
        // set keeps the call that finds the columns for each file.
        let pinned = if engine.source.format == Format::Csv && !engine.source.is_multi() {
            engine.pin_csv(opts)
        } else {
            None
        };

        // Read the schema now, and keep it. An error message here is more
        // useful to the user than an error message at the first scroll, and
        // each later caller then needs no second read of the file.
        engine.base_schema = match pinned {
            Some(schema) => schema,
            None => {
                let plain = engine.read_expr.clone();
                engine.use_scan_expr(plain)?
            }
        };

        // A file of JSON has no sniffer to ask. Peruse therefore reads the
        // columns with the plain call above, and writes them into the call now.
        if engine.source.format == Format::Json && !engine.source.is_multi() {
            engine.pin_json();
        }
        Ok(engine)
    }

    /// Opens one table of a DuckDB database file.
    ///
    /// The function runs two statements, and nothing else is special about a
    /// database:
    ///
    /// ```text
    /// ATTACH '<path>' AS "__peruse_db" (READ_ONLY);
    /// CREATE OR REPLACE VIEW src AS SELECT * FROM "__peruse_db"."main"."sales";
    /// ```
    ///
    /// From there, paging, the filter, the sort, the search, the statistics,
    /// the record view and `--ddl` all read the view `src`, and none of them
    /// knows what the view holds.
    ///
    /// The flag `READ_ONLY` makes the promise of Peruse stronger here than
    /// anywhere else: the storage engine of DuckDB refuses each write to the
    /// file, so the promise does not rest on the guard over the words of a
    /// statement.
    fn open_database(conn: Connection, mut src: Source, opts: &OpenOptions) -> Result<Engine> {
        attach_database(&conn, &src)?;
        let tables = read_tables(&conn)
            .with_context(|| format!("reading the tables of {}", src.label))?;
        let table = choose_table(&src, &tables, opts.table.as_deref())?;
        // The title bar names the table. The name of the file does not say
        // which rows the grid shows.
        src.table = Some(table.label());

        let scan = table.qualified();
        let mut engine = Engine {
            read_expr: database_read_expr(&src, &table),
            conn,
            source: src,
            scan_expr: String::new(),
            indexed: false,
            base_schema: Schema::default(),
            csv_dialect: None,
            table: Some(table),
        };
        // Read the schema now, in the same way as for a file. A name that the
        // database does not hold therefore gives an error here.
        engine.base_schema = engine.use_scan_expr(scan)?;
        Ok(engine)
    }

    /// Gives the table that the view `src` reads, for a database source.
    ///
    /// The value is `None` for a file. The option `--ddl` uses the name of this
    /// table for its `CREATE TABLE` statement.
    pub fn table(&self) -> Option<&DbTable> {
        self.table.as_ref()
    }

    /// Points the view `src` at `expr`, and gives the columns of that view.
    fn use_scan_expr(&mut self, expr: String) -> Result<Schema> {
        let input = self.source.input.clone();
        self.conn
            .execute_batch(&format!("CREATE OR REPLACE VIEW src AS SELECT * FROM {expr}"))
            .map_err(|e| open_error(&input, &e.to_string()))?;
        let schema = self
            .read_schema(&View::default())
            .map_err(|e| open_error(&input, &e.to_string()))?;
        self.scan_expr = expr;
        Ok(schema)
    }

    /// Asks the CSV sniffer one time, and writes the dialect and the columns
    /// into the `read_csv` call.
    ///
    /// The function gives the columns of the new call, and it keeps the dialect
    /// for the metadata panel. It gives `None` when the sniffer fails, when the
    /// new call fails, or when the new call gives columns that are not the
    /// columns of the sniffer. The caller then uses the call with `auto_detect`,
    /// which is the call that Peruse used before this step existed.
    fn pin_csv(&mut self, opts: &OpenOptions) -> Option<Schema> {
        let sniff = self.sniff_csv(opts).ok()?;
        let expr = pinned_csv_expr(&self.source, opts, &sniff)?;
        let schema = self.use_scan_expr(expr).ok()?;
        // The sniffer and the read call share one piece of code inside DuckDB,
        // so the columns of the sniffer are the columns that `auto_detect`
        // gives. A difference here shows that the dialect did not arrive
        // correctly, and the plain call is then the safe answer.
        let same = schema.len() == sniff.columns.len()
            && schema
                .columns
                .iter()
                .zip(&sniff.columns)
                .all(|(c, (name, ty))| c.name == *name && c.sql_type == *ty);
        if !same {
            return None;
        }
        self.csv_dialect = Some(sniff.meta);
        Some(schema)
    }

    /// Asks the DuckDB sniffer for the dialect and the columns of a CSV file.
    ///
    /// The sniffer gets the same options as the read call. The two therefore
    /// agree about the file.
    fn sniff_csv(&self, opts: &OpenOptions) -> Result<CsvSniff> {
        let mut args = vec![quote_str(&db_path(&self.source.files[0]))];
        if let Some(d) = self.source.delimiter {
            args.push(format!("delim = {}", delim_literal(d)));
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
        let call = format!("sniff_csv({})", args.join(", "));

        // The field `Columns` holds one structure for each column. The call to
        // unnest makes one row for each of them, and the dialect repeats on
        // every row. One statement therefore runs the sniffer one time.
        let mut stmt = self.conn.prepare(&format!(
            "SELECT coalesce(Delimiter, ''), coalesce(Quote, ''), coalesce(Escape, ''), \
                    coalesce(NewLineDelimiter, ''), coalesce(SkipRows, 0)::BIGINT, \
                    coalesce(HasHeader, false), coalesce(DateFormat, ''), \
                    coalesce(TimestampFormat, ''), coalesce(Prompt, ''), \
                    coalesce(Comment, ''), col.name, col.type \
             FROM (SELECT Delimiter, Quote, Escape, NewLineDelimiter, SkipRows, HasHeader, \
                          DateFormat, TimestampFormat, Prompt, Comment, unnest(Columns) AS col \
                   FROM {call})"
        ))?;
        let mut rows = stmt.query([])?;
        let mut out: Option<CsvSniff> = None;
        while let Some(r) = rows.next()? {
            let sniff = match &mut out {
                Some(s) => s,
                None => out.insert(CsvSniff {
                    meta: CsvMeta {
                        delimiter: r.get(0)?,
                        quote: r.get(1)?,
                        escape: r.get(2)?,
                        new_line: r.get(3)?,
                        skip_rows: r.get(4)?,
                        has_header: r.get(5)?,
                        date_format: r.get(6)?,
                        timestamp_format: r.get(7)?,
                        prompt: r.get(8)?,
                    },
                    comment: r.get(9)?,
                    columns: Vec::new(),
                }),
            };
            sniff.columns.push((r.get(10)?, r.get(11)?));
        }
        out.ok_or_else(|| anyhow::anyhow!("the CSV sniffer found no column"))
    }

    /// Writes the columns that Peruse read into the `read_json` call.
    ///
    /// The JSON reader of DuckDB finds the fields and their types for every
    /// statement, in the same way as the CSV sniffer. On a file of 95 MB, one
    /// page of 50 rows costs 74 milliseconds with the plain call, and 9 with the
    /// pinned call.
    ///
    /// The function keeps the plain call when the pinned call fails, or when it
    /// gives different columns.
    fn pin_json(&mut self) {
        let want = self.base_schema.clone();
        let Some(expr) = pinned_json_expr(&self.source, &want) else {
            return;
        };
        let plain = self.scan_expr.clone();
        match self.use_scan_expr(expr) {
            Ok(got) if same_columns(&got, &want) => {}
            // Point the view back at the call that worked. The schema in
            // `base_schema` is the schema of that call already.
            _ => {
                let _ = self.use_scan_expr(plain);
            }
        }
    }

    /// Gives the name and the type of each column of the whole file.
    pub fn base_schema(&self) -> &Schema {
        &self.base_schema
    }

    /// Gives a handle that stops the query that runs now.
    ///
    /// Another thread can hold this handle and use it safely.
    pub fn interrupt_handle(&self) -> Arc<InterruptHandle> {
        self.conn.interrupt_handle()
    }

    /// Gives the name and the type of each column. This function reads no rows.
    ///
    /// A view that reads the file itself gets the schema that the open
    /// operation found. A filter and a sort remove rows, and they do not
    /// change the columns, so that schema is the right one for each of them.
    /// This saves a read of the file, and on a file of text that read runs
    /// the sniffer of DuckDB again.
    pub fn describe(&self, view: &View) -> Result<Schema> {
        if view.is_source() && !self.base_schema.is_empty() {
            return Ok(self.base_schema.clone());
        }
        self.read_schema(view)
    }

    /// Reads the name and the type of each column from the database.
    fn read_schema(&self, view: &View) -> Result<Schema> {
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
        if limit == 0 || scan_rows == 0 {
            return Ok(Vec::new());
        }
        // Read a small window in front of the cursor first.
        //
        // The statement holds `ORDER BY off`, because the caller needs the
        // matches in the order of the view. The database must therefore read
        // every row of the part before it can give the first match, and one
        // search over 250,000 rows costs 90 milliseconds.
        //
        // A match near the cursor is the usual case. With a window of a few
        // thousand rows in front, that search costs 2 milliseconds. A window
        // that fills the limit answers the request, and the remainder stays
        // unread.
        //
        // A window that does not fill the limit costs a scan of the window. The
        // remainder is then one more statement, so a search that finds nothing
        // reads 3 percent more rows than it read before.
        //
        // A sorted view reads its part in one statement. Each window would need
        // its own sort of the whole view, and two sorts cost more than one scan.
        let first = if view.sort.is_empty() {
            SEARCH_WINDOW.min(scan_rows)
        } else {
            scan_rows
        };

        let mut out: Vec<u64> = Vec::new();
        let mut done = 0u64;
        let mut window = first;
        while done < scan_rows && out.len() < limit as usize {
            let take = window.min(scan_rows - done);
            let need = limit - out.len() as u32;
            let sql = view.search_sql(schema, needle, from_row + done, take, need);
            if sql.is_empty() {
                // No column of the view holds text that a search can read.
                return Ok(Vec::new());
            }
            let mut stmt = self.conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                if let Some(n) = to_u64(&r.get::<_, Value>(0)?) {
                    out.push(n);
                }
            }
            done += take;
            // The window in front is small. The remainder is one statement.
            window = scan_rows - done;
        }
        Ok(out)
    }

    /// Calculates the statistics of one column, for the column inspector.
    ///
    /// The value `top_k` gives the number of frequent values to collect. The
    /// value 0 collects none. A column of numbers also gets a histogram.
    pub fn column_stats(&self, view: &View, column: &Column, top_k: u32) -> Result<ColumnStats> {
        let sql = view.stats_sql(&column.name, column.kind);
        // A column of numbers gets two more values: the two edges of the
        // histogram. They arrive with the statistics, so the column is read one
        // time and not two times.
        let numeric = column.kind == CellKind::Number;
        let mut bounds: (Option<f64>, Option<f64>) = (None, None);
        let mut stats: ColumnStats = self.conn.query_row(&sql, [], |r| {
            if numeric {
                bounds = (
                    r.get::<_, Value>(7).ok().as_ref().and_then(to_f64),
                    r.get::<_, Value>(8).ok().as_ref().and_then(to_f64),
                );
            }
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

        // The most frequent values of a column of keys say nothing: each
        // value occurs one time. That query is also the slow part of the
        // panel, because it groups every row of the view. On ten million
        // rows it costs 300 milliseconds, and each other query of the panel
        // costs 30. A column where almost each value is different therefore
        // does not get it.
        //
        // The panel already writes "every sampled value occurs once" for a
        // list of counts of one, and an empty list reads the same way.
        //
        // A small view keeps the list. The query costs nothing there, and a
        // user who looks at a hundred rows still wants to see them.
        let nearly_unique = stats.n_present >= TOP_VALUES_MIN_ROWS
            && stats.n_distinct * 4 > stats.n_present * 3;
        if top_k > 0 && !nearly_unique {
            stats.top = self.top_values(view, &column.name, top_k)?;
        }
        if numeric {
            stats.histogram = self.histogram(view, &column.name, bounds, 24)?;
        }
        Ok(stats)
    }

    /// Measures each column that the detail band draws, in one query.
    ///
    /// One query covers every column. The number of columns is bounded by the
    /// width of the terminal, so the query stays small, and the database reads
    /// the view one time for all of the columns. Refer to [`View::band_sql`].
    ///
    /// A caller that draws the compact band over a plain Parquet file needs no
    /// query at all. Refer to [`footer_briefs`].
    pub fn column_band(
        &self,
        view: &View,
        columns: &[Column],
        values: bool,
    ) -> Result<Vec<ColumnBrief>> {
        if columns.is_empty() {
            return Ok(Vec::new());
        }
        let sql = view.band_sql(columns, values);
        // The count of the rows comes first. Each column then gives one value
        // without `values`, and four with it. Refer to `View::band_sql`.
        let stride = if values { 4 } else { 1 };
        let mut out: Vec<ColumnBrief> = Vec::with_capacity(columns.len());
        self.conn.query_row(&sql, [], |r| {
            let n_total = to_u64(&r.get::<_, Value>(0)?).unwrap_or(0);
            for (i, c) in columns.iter().enumerate() {
                let at = 1 + i * stride;
                let n_present = to_u64(&r.get::<_, Value>(at)?).unwrap_or(0);
                let (n_distinct, min, max) = if values {
                    let distinct = to_u64(&r.get::<_, Value>(at + 1)?).unwrap_or(0);
                    (
                        // The function approx_count_distinct gives an estimate,
                        // and the estimate can be a little too large on a small
                        // input. A column cannot hold more different values than
                        // it holds rows that are not NULL.
                        Some(distinct.min(n_present)),
                        r.get::<_, Option<String>>(at + 2)?,
                        r.get::<_, Option<String>>(at + 3)?,
                    )
                } else {
                    // The compact band draws none of these three. `None` says
                    // that Peruse did not measure them, so the detailed band asks
                    // again instead of drawing a blank.
                    (None, None, None)
                };
                out.push(ColumnBrief {
                    column: c.name.clone(),
                    n_total,
                    n_present,
                    n_distinct,
                    min,
                    max,
                });
            }
            Ok(())
        })?;
        Ok(out)
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
    ///
    /// The caller gives the two edges of the histogram. They come from the
    /// statistics query, which reads the column one time for the statistics and
    /// the edges together.
    fn histogram(
        &self,
        view: &View,
        column: &str,
        bounds: (Option<f64>, Option<f64>),
        bins: u32,
    ) -> Result<Option<Histogram>> {
        let (Some(lo), Some(hi)) = bounds else {
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
                // The facts of each column come first. The sizes of the whole
                // file and the list of the encodings come from them, so the
                // panel needs two statements fewer over the footer.
                meta.columns = self.parquet_column_stats().unwrap_or_default();
                meta.parquet = Some(self.parquet_meta(&meta.columns)?);
            }
            Format::Csv => {
                // A failure of the sniffer must not remove the full panel.
                // The other metadata is still useful to the user.
                meta.csv = self.csv_meta().ok();
            }
            // A JSON file, an Arrow file and a database hold no footer and no
            // dialect. The panel shows the sizes, the columns and the types
            // only. For a database it also shows the two statements that open
            // the table. Refer to [`database_read_expr`].
            Format::Json | Format::Arrow | Format::DuckDb | Format::Sqlite => {}
        }
        Ok(meta)
    }

    /// Reads the footer facts of a Parquet file: the row count, the row group
    /// count, the sizes, the codecs, the encodings and the writer.
    ///
    /// The caller gives the facts of each column, from
    /// [`Engine::parquet_column_stats`]. The sizes of the whole file are the
    /// sums of those facts, and the encodings of the file are the encodings of
    /// its columns. Two statements over the footer are therefore not necessary.
    fn parquet_meta(&self, columns: &[ColumnFooterStats]) -> Result<ParquetMeta> {
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

        // Each column chunk belongs to one column, so the sizes of the columns
        // add up to the sizes of the file.
        if columns.is_empty() {
            // The statement for the columns failed. Ask for the two sums.
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
        } else {
            m.total_compressed = columns.iter().map(|c| c.compressed).sum();
            m.total_uncompressed = columns.iter().map(|c| c.uncompressed).sum();
        }

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

        // The field `encodings` of a column holds one name for each encoding of
        // that column, and a slash separates the names. Split the names, so that
        // the panel shows each one time. Each column chunk belongs to one
        // column, so the columns hold every encoding of the file.
        let mut seen: Vec<String> = Vec::new();
        for c in columns {
            for part in c.encodings.split(['/', ',']) {
                let part = part.trim();
                if !part.is_empty() && !seen.iter().any(|s| s == part) {
                    seen.push(part.to_string());
                }
            }
        }
        seen.sort();
        m.encodings = seen;

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
    /// The NULL counts of the row groups add together, but only when each row
    /// group holds one. A writer is free to leave the statistics out, and it can
    /// leave them out of some row groups and not others. The function `sum` skips
    /// a NULL input, so a partial set of counts adds up to a number that looks
    /// correct and is too small.
    ///
    /// The query therefore counts the row groups that hold the statistic and
    /// compares that with the number of row groups. The NULL count is `None`
    /// unless every row group holds one. A count that Peruse does not know is
    /// better than a count that is wrong: the band and the metadata panel show
    /// the type instead, and the column inspector still measures the true count
    /// with a query.
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
                    CASE WHEN count(*) = count(stats_null_count) \
                         THEN sum(stats_null_count)::BIGINT END, \
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
    ///
    /// The open operation asks the sniffer already, and it keeps the answer. The
    /// panel therefore waits for nothing. A set of CSV files has no such answer,
    /// and this function then asks the sniffer about the first file of the set.
    fn csv_meta(&self) -> Result<CsvMeta> {
        if let Some(m) = &self.csv_dialect {
            return Ok(m.clone());
        }
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
            self.scan_expr
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
    ///
    /// A table of a database is in blocks already, and the database holds the
    /// count of the rows. An index of it would copy the table for nothing, so
    /// the key `I` and the index at open must not run for a database.
    pub fn is_seekable(&self) -> bool {
        matches!(
            self.source.format,
            Format::Parquet | Format::Arrow | Format::DuckDb
        ) || self.indexed
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
    ///
    /// For a file of text, the call that Peruse runs also names the dialect and
    /// every column, so that DuckDB does not examine the file again. That text
    /// is too long for a panel. This function therefore gives the short form,
    /// which finds the same columns and reads the same rows.
    ///
    /// A database has no such call. It gives the ATTACH statement and the
    /// qualified name of the table instead. Refer to [`database_read_expr`].
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

    // DuckDB reads the footer of a Parquet file for each statement. With this
    // cache it reads the footer one time and keeps the result. A page request
    // then costs 8 milliseconds and not 12, and a count costs 1 and not 2.
    //
    // The cache is safe for a file that changes on the disk. DuckDB keeps the
    // size and the time of the last change with the footer, and it reads the
    // footer again when either one changes.
    //
    // The setting arrived in a later version of DuckDB than the oldest one that
    // Peruse builds against. A build without it must still open a file, so a
    // failure here is not an error.
    let _ = conn.execute_batch("SET parquet_metadata_cache = true;");
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

/// Reads the tables and the views of a DuckDB database file.
///
/// The function attaches the file on a connection of its own, for reading only,
/// and it closes that connection when it gives the answer. The table picker of
/// the user interface calls it before the engine opens the file, in the same way
/// as the chooser of files reads a directory.
///
/// A file that a newer DuckDB wrote gives an error here, in front of the
/// terminal. Refer to [`attach_database`].
///
/// The options are the options of the open that follows. Without them, this
/// connection would take 80 percent of the memory of the machine and one thread
/// for each processor, which is what `--memory-limit` and `--threads` exist to
/// stop.
pub fn database_tables(path: &Path, opts: &OpenOptions) -> Result<Vec<DbTable>> {
    let conn = Connection::open_in_memory().context("starting DuckDB")?;
    configure(&conn, opts)?;
    let label = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let src = Source {
        input: path.to_string_lossy().into_owned(),
        label,
        format: Format::DuckDb,
        files: vec![path.to_path_buf()],
        bytes: 0,
        delimiter: None,
        compressed: false,
        table: None,
    };
    attach_database(&conn, &src)?;
    read_tables(&conn).with_context(|| format!("reading the tables of {}", src.label))
}

/// Attaches a database file, for reading only.
///
/// The statement is `ATTACH '<path>' AS "__peruse_db" (READ_ONLY)`. It goes
/// straight to the connection, and it is not a statement of the user, so
/// [`crate::sql_guard`] never sees it. That guard must keep refusing an ATTACH
/// that a user types.
fn attach_database(conn: &Connection, src: &Source) -> Result<()> {
    let sql = format!(
        "ATTACH {} AS {} (READ_ONLY)",
        quote_str(&db_path(&src.files[0])),
        quote_ident(DB_ALIAS)
    );
    conn.execute_batch(&sql)
        .map_err(|e| anyhow::anyhow!("{}", attach_message(&src.label, &e.to_string())))
}

/// Writes the message for a database that Peruse cannot attach.
///
/// A database that a newer DuckDB wrote has a storage version that this build
/// cannot read. The words of DuckDB name that version and say nothing about
/// Peruse, so a user would not know what to do. This function says which
/// program needs the change, and it keeps the words of DuckDB below, because
/// those words hold the version and the path.
fn attach_message(label: &str, duck: &str) -> String {
    // A file of an older storage version also fails to attach, and a newer
    // Peruse would not read it either. The words of DuckDB then stand alone.
    let newer = !duck.contains("older version")
        && (duck.contains("newer version")
            || duck.contains("storage version")
            || duck.contains("not compatible"));
    if !newer {
        return format!("{label}: Peruse cannot attach this database.\n{duck}");
    }
    match storage_version(duck) {
        Some(v) => format!(
            "{label}: this database needs a newer Peruse. \
             DuckDB reports storage version {v}, and this build reads an older one.\n{duck}"
        ),
        None => format!(
            "{label}: this database needs a newer Peruse. \
             A newer DuckDB wrote the file.\n{duck}"
        ),
    }
}

/// Finds the storage version that a message from DuckDB names.
///
/// The message holds a sentence such as "Trying to read a database file with
/// version number 64". The words `version number` name the version of the file,
/// so the function looks for them first. The message also holds the path of the
/// file, and a directory can carry the word `version` with a number after it.
/// The function then looks in a short part of the message only, so that a number
/// far away cannot arrive here. The caller keeps the words of DuckDB, so nothing
/// is lost when the function finds no number.
fn storage_version(duck: &str) -> Option<String> {
    let lower = duck.to_ascii_lowercase();
    let at = match lower.find("version number") {
        Some(i) => i + "version number".len(),
        None => lower.find("version")? + "version".len(),
    };
    let window: String = lower[at..].chars().take(40).collect();
    let digits: String = window
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    (!digits.is_empty()).then_some(digits)
}

/// Reads the tables and the views of the attached database.
///
/// The field `estimated_size` is the number of rows that the catalog holds
/// already, so the list costs no scan of the data. A list of views has no such
/// number, and a view therefore shows none. Peruse must not run `count(*)` over
/// each table of a database to fill a list.
fn read_tables(conn: &Connection) -> Result<Vec<DbTable>> {
    let alias = quote_str(DB_ALIAS);
    let sql = format!(
        "SELECT schema_name, table_name, false, estimated_size::BIGINT \
         FROM duckdb_tables() WHERE database_name = {alias} AND NOT internal \
         UNION ALL \
         SELECT schema_name, view_name, true, NULL::BIGINT \
         FROM duckdb_views() WHERE database_name = {alias} AND NOT internal \
         ORDER BY 3, 1, 2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(DbTable {
            schema: r.get(0)?,
            name: r.get(1)?,
            is_view: r.get(2)?,
            rows: r.get::<_, Option<i64>>(3)?.map(|n| n.max(0) as u64),
        });
    }
    Ok(out)
}

/// Chooses the table that the view `src` reads.
///
/// The rules are:
///
/// * The name that the caller gives wins. A name that two schemas both hold is
///   not an answer, and the message then asks for the schema.
/// * A database with one table opens that table.
/// * A database with many tables needs a name. The user interface asks the user
///   with a picker, and a call with no terminal gets this message.
fn choose_table(src: &Source, tables: &[DbTable], want: Option<&str>) -> Result<DbTable> {
    if tables.is_empty() {
        bail!(
            "{}: this database holds no table and no view, so Peruse has nothing to show.",
            src.label
        );
    }
    if let Some(want) = want.map(str::trim).filter(|w| !w.is_empty()) {
        let hits: Vec<&DbTable> = tables.iter().filter(|t| t.is_named(want)).collect();
        return match hits.as_slice() {
            [one] => Ok((*one).clone()),
            [] => bail!(
                "{}: this database holds no table {want:?}.\n{}",
                src.label,
                table_list(tables)
            ),
            many => bail!(
                "{}: {} schemas hold a table {want:?}. Name the schema too, as {}.\n{}",
                src.label,
                many.len(),
                many[0].label(),
                table_list(tables)
            ),
        };
    }
    if let [one] = tables {
        return Ok(one.clone());
    }
    bail!(
        "{}: this database holds {}. Name one with --table, for example:\n  \
         peruse {} --table {}\n{}",
        src.label,
        how_many(tables),
        src.input,
        tables[0].name,
        table_list(tables)
    )
}

/// Writes how many tables and views a database holds, such as
/// `2 tables and 1 view`.
fn how_many(tables: &[DbTable]) -> String {
    let views = tables.iter().filter(|t| t.is_view).count();
    let plain = tables.len() - views;
    let part = |n: usize, word: &str| {
        if n == 1 {
            format!("{n} {word}")
        } else {
            format!("{n} {word}s")
        }
    };
    match (plain, views) {
        (p, 0) => part(p, "table"),
        (0, v) => part(v, "view"),
        (p, v) => format!("{} and {}", part(p, "table"), part(v, "view")),
    }
}

/// Writes the names of the tables for a message.
fn table_list(tables: &[DbTable]) -> String {
    let mut names: Vec<String> = tables
        .iter()
        .take(NAMES_IN_MESSAGE)
        .map(|t| {
            if t.is_view {
                format!("{} (view)", t.label())
            } else {
                t.label()
            }
        })
        .collect();
    if tables.len() > names.len() {
        names.push(format!("and {} more", tables.len() - names.len()));
    }
    format!("tables: {}", names.join(", "))
}

/// Writes the text that the metadata panel shows for a database.
///
/// The panel shows what Peruse reads, and the user copies that text into
/// another program. For a database the text is therefore the two statements
/// that open the table. It names the true alias, so the same name also works in
/// the SQL prompt of Peruse, where a statement can read a second table of the
/// database.
///
/// The text stays short. A read expression of some kilobytes costs real time at
/// each frame, because the panel measures it again for each redraw.
fn database_read_expr(src: &Source, table: &DbTable) -> String {
    format!(
        "ATTACH {} AS {} (READ_ONLY); FROM {}",
        quote_str(&db_path(&src.files[0])),
        quote_ident(DB_ALIAS),
        table.qualified()
    )
}

/// Gives `true` when two schemas hold the same names and the same types, in the
/// same order.
fn same_columns(a: &Schema, b: &Schema) -> bool {
    a.len() == b.len()
        && a.columns
            .iter()
            .zip(&b.columns)
            .all(|(x, y)| x.name == y.name && x.sql_type == y.sql_type)
}

/// Writes a CSV delimiter as a SQL literal.
///
/// The option reader of DuckDB reads an escape sequence in this value. A tab
/// therefore stays a tab when it goes through SQL as `\t`, and a backslash
/// needs a second backslash.
fn delim_literal(d: char) -> String {
    match d {
        '\t' => "'\\t'".to_string(),
        '\\' => "'\\\\'".to_string(),
        _ => quote_str(&d.to_string()),
    }
}

/// Changes one character field of the CSV sniffer into the text that the
/// matching `read_csv` option needs.
///
/// The sniffer writes the words `(empty)` when the file uses no such character.
/// The option needs an empty text for that case.
fn sniffed_char(v: &str) -> &str {
    if v == "(empty)" { "" } else { v }
}

/// Writes the `read_csv` call that reads the file with the dialect and the
/// columns that the sniffer found.
///
/// The call sets `auto_detect = false`, so DuckDB accepts these values and does
/// not examine the file again. Refer to [`Engine::pin_csv`].
fn pinned_csv_expr(src: &Source, opts: &OpenOptions, s: &CsvSniff) -> Option<String> {
    if s.columns.is_empty() {
        return None;
    }
    let cols: Vec<String> = s
        .columns
        .iter()
        .map(|(name, ty)| format!("{}: {}", quote_str(name), quote_str(ty)))
        .collect();
    let mut args = vec![
        file_list(&src.files),
        "auto_detect = false".to_string(),
        format!("columns = {{{}}}", cols.join(", ")),
        format!("header = {}", s.meta.has_header),
        format!("skip = {}", s.meta.skip_rows.max(0)),
    ];
    // The sniffer gives these four values as the characters themselves. The
    // option reader of DuckDB reads an escape sequence in the value, so a
    // backslash must arrive as two backslashes.
    for (name, value) in [
        ("delim", s.meta.delimiter.as_str()),
        ("quote", s.meta.quote.as_str()),
        ("escape", s.meta.escape.as_str()),
        ("comment", s.comment.as_str()),
    ] {
        let v = sniffed_char(value).replace('\\', "\\\\");
        args.push(format!("{name} = {}", quote_str(&v)));
    }
    // The sniffer gives the line end as an escape sequence, such as `\n`. The
    // option takes `\r`, `\n` and `\r\n` only. A file that mixes them has no
    // value here, so the call leaves the option out and DuckDB finds the line
    // end for itself.
    let nl = sniffed_char(&s.meta.new_line);
    if matches!(nl, "\\r" | "\\n" | "\\r\\n") {
        args.push(format!("new_line = {}", quote_str(nl)));
    }
    // A file with a date in a form that is not the international one needs these
    // two. Without them, the reader would give an error on the first such value.
    for (name, value) in [
        ("dateformat", s.meta.date_format.as_str()),
        ("timestampformat", s.meta.timestamp_format.as_str()),
    ] {
        if !value.is_empty() {
            args.push(format!("{name} = {}", quote_str(value)));
        }
    }
    if opts.ignore_errors {
        args.push("ignore_errors = true".into());
    }
    Some(format!("read_csv({})", args.join(", ")))
}

/// Writes the `read_json` call that reads the file with a fixed column list.
///
/// Refer to [`Engine::pin_json`].
fn pinned_json_expr(src: &Source, schema: &Schema) -> Option<String> {
    if schema.is_empty() {
        return None;
    }
    let cols: Vec<String> = schema
        .columns
        .iter()
        .map(|c| format!("{}: {}", quote_str(&c.name), quote_str(&c.sql_type)))
        .collect();
    Some(format!(
        "read_json({}, maximum_depth = {MAX_JSON_DEPTH}, columns = {{{}}})",
        file_list(&src.files),
        cols.join(", ")
    ))
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
                args.push(format!("delim = {}", delim_literal(d)));
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
        // `Engine::open` refuses an Arrow file and a SQLite file in front of
        // this function, and it opens a database with an ATTACH and a
        // qualified name. None of the three values arrives here.
        Format::Arrow => unreachable!("Engine::open refuses an Arrow file"),
        Format::Sqlite => unreachable!("Engine::open refuses a SQLite file"),
        Format::DuckDb => unreachable!("a database opens with ATTACH"),
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

    /// Reads every cell of a view as text. The tests of the pinned read call
    /// compare the result with the values that the file holds.
    fn all_cells(e: &Engine) -> Vec<Vec<Option<String>>> {
        let view = View::default();
        let schema = e.describe(&view).unwrap();
        let page = e.page(&view, &schema, 1000, 0).unwrap();
        (0..page.nrows)
            .map(|r| {
                (0..page.ncols)
                    .map(|c| page.cell(r, c).map(|s| s.to_string()))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_single_csv_file_gets_a_read_call_with_no_sniffer() {
        // Each statement against a file of text makes DuckDB examine the file
        // again, and that examination is the slow part of a page request. The
        // open operation therefore writes the dialect and the columns into the
        // read call one time.
        let d = tmpdir("pin-csv");
        let p = write_csv(&d, "s.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        assert!(
            e.scan_expr.contains("auto_detect = false"),
            "the read call still examines the file: {}",
            e.scan_expr
        );
        assert!(e.scan_expr.contains("columns = {"), "no column list: {}", e.scan_expr);
        // The panel shows the short call, and not the long one.
        assert!(
            !e.read_expr().contains("columns = {"),
            "the panel would show a column list: {}",
            e.read_expr()
        );
        // The dialect of the panel comes from the same examination.
        let meta = e.file_meta().unwrap();
        assert_eq!(meta.csv.unwrap().delimiter, ",");
    }

    #[test]
    fn a_set_of_csv_files_keeps_the_read_call_that_finds_the_columns() {
        // Two files of one set can hold different columns, and `union_by_name`
        // reads each file with its own columns. One column list cannot serve
        // them all.
        let d = tmpdir("pin-csv-multi");
        write_csv(&d, "a.csv", "id\n1\n2\n");
        write_csv(&d, "b.csv", "id\n3\n");
        let pattern = format!("{}/*.csv", d.to_string_lossy());
        let e = Engine::open(&pattern, &OpenOptions::default()).unwrap();
        assert!(!e.scan_expr.contains("auto_detect = false"), "{}", e.scan_expr);
        assert_eq!(e.count(&View::default()).unwrap(), 3);
    }

    #[test]
    fn the_pinned_read_call_keeps_quotation_marks_and_line_ends() {
        // A quoted value can hold the delimiter, a line end and a quotation
        // mark. The pinned call must carry the quote character and the escape
        // character, or each of these three values would arrive broken.
        let d = tmpdir("pin-quotes");
        let p = write_csv(
            &d,
            "q.csv",
            "a,b\n1,\"hello, world\"\n2,\"line\nbreak\"\n3,\"say \"\"hi\"\"\"\n",
        );
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let rows = all_cells(&e);
        assert_eq!(rows.len(), 3, "a line end inside a value split a row: {rows:?}");
        assert_eq!(rows[0][1].as_deref(), Some("hello, world"));
        assert_eq!(rows[1][1].as_deref(), Some("line\nbreak"));
        assert_eq!(rows[2][1].as_deref(), Some("say \"hi\""));
    }

    #[test]
    fn the_pinned_read_call_keeps_a_delimiter_that_the_sniffer_found() {
        // The extension `.dat` names no delimiter, so the sniffer finds the
        // semicolon. The pinned call must carry what the sniffer found. A file
        // with the extension `.csv` always uses a comma, and it would not test
        // this path.
        let d = tmpdir("pin-semicolon");
        let p = write_csv(&d, "s.dat", "a;b;c\n1;two;3.5\n4;five;6.5\n");
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let schema = e.describe(&View::default()).unwrap();
        assert_eq!(schema.len(), 3, "the semicolon did not split the row");
        assert!(e.scan_expr.contains("delim = ';'"), "{}", e.scan_expr);
        let rows = all_cells(&e);
        assert_eq!(rows[1][1].as_deref(), Some("five"));
    }

    #[test]
    fn the_pinned_read_call_keeps_a_carriage_return_line_end() {
        let d = tmpdir("pin-crlf");
        let p = write_csv(&d, "s.csv", "a,b\r\n1,x\r\n2,y\r\n");
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let rows = all_cells(&e);
        assert_eq!(rows.len(), 2);
        // A carriage return that stays in the value would break this test.
        assert_eq!(rows[0][1].as_deref(), Some("x"));
        assert_eq!(rows[1][1].as_deref(), Some("y"));
    }

    #[test]
    fn the_pinned_read_call_keeps_a_date_format_that_is_not_the_international_one() {
        // The sniffer finds the form of the date. The pinned call must carry
        // that form, or the reader would give an error on the first value.
        let d = tmpdir("pin-dateformat");
        let p = write_csv(&d, "s.csv", "d\n31/12/2024\n01/01/2025\n30/06/2025\n");
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let rows = all_cells(&e);
        assert_eq!(rows.len(), 3, "the reader lost a row: {rows:?}");
        assert_eq!(rows[0][0].as_deref(), Some("2024-12-31"));
        assert_eq!(rows[2][0].as_deref(), Some("2025-06-30"));
    }

    #[test]
    fn the_pinned_read_call_keeps_a_comment_row_out_of_the_data() {
        let d = tmpdir("pin-comment");
        let p = write_csv(&d, "s.csv", "# a note\na,b\n1,x\n# another note\n2,y\n");
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let rows = all_cells(&e);
        assert_eq!(rows.len(), 2, "a comment row became data: {rows:?}");
        assert_eq!(rows[1][1].as_deref(), Some("y"));
    }

    #[test]
    fn the_pinned_read_call_keeps_a_file_that_has_no_header_row() {
        let d = tmpdir("pin-no-header");
        let p = write_csv(&d, "s.csv", "1,alice\n2,bob\n3,carol\n");
        let opts = OpenOptions {
            header: Some(false),
            ..Default::default()
        };
        let e = Engine::open(p.to_str().unwrap(), &opts).unwrap();
        let rows = all_cells(&e);
        assert_eq!(rows.len(), 3, "the first row became a header: {rows:?}");
        assert_eq!(rows[0][1].as_deref(), Some("alice"));
    }

    #[test]
    fn a_json_file_gets_a_read_call_with_a_fixed_column_list() {
        // The JSON reader finds the fields for each statement, in the same way
        // as the CSV sniffer.
        let d = tmpdir("pin-json");
        let p = write_csv(
            &d,
            "e.ndjson",
            "{\"id\": 1, \"user\": \"alice\"}\n{\"id\": 2, \"user\": \"bob\"}\n",
        );
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        assert!(e.scan_expr.contains("columns = {"), "no column list: {}", e.scan_expr);
        let rows = all_cells(&e);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][1].as_deref(), Some("bob"));
    }

    /// Writes a Parquet file with `rows` rows and small row groups.
    ///
    /// The test writes the file with a connection of its own. The engine never
    /// writes a file.
    fn write_parquet(dir: &Path, name: &str, rows: u64) -> PathBuf {
        let p = dir.join(name);
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "COPY (SELECT i AS id, ('name ' || i) AS label FROM range({rows}) t(i)) \
             TO '{}' (FORMAT parquet, ROW_GROUP_SIZE 2048);",
            db_path(&p)
        ))
        .unwrap();
        p
    }

    #[test]
    fn the_parquet_sizes_and_encodings_come_from_the_columns() {
        // The panel reads the facts of each column, and the sizes of the file
        // and the list of the encodings come from those facts. The numbers must
        // agree with the numbers of the footer.
        let d = tmpdir("parquet-meta");
        let p = write_parquet(&d, "t.parquet", 10_000);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let meta = e.file_meta().unwrap();
        let pq = meta.parquet.expect("a Parquet file has a footer");

        assert_eq!(pq.num_rows, 10_000);
        assert_eq!(pq.num_row_groups, 5, "2048 rows in each group");
        assert_eq!(
            pq.row_group_bytes.len() as u64,
            pq.num_row_groups,
            "one size for each row group"
        );
        assert_eq!(meta.columns.len(), 2);

        // The sizes of the file are the sums of the sizes of the columns.
        assert!(pq.total_compressed > 0 && pq.total_uncompressed > 0);
        assert_eq!(
            pq.total_compressed,
            meta.columns.iter().map(|c| c.compressed).sum::<u64>()
        );
        assert_eq!(
            pq.total_uncompressed,
            meta.columns.iter().map(|c| c.uncompressed).sum::<u64>()
        );
        // The sizes of the row groups also add up to the size of the file.
        assert_eq!(
            pq.row_group_bytes.iter().sum::<u64>(),
            pq.total_uncompressed
        );

        // Each name of an encoding arrives one time, and no name holds a
        // separator.
        assert!(!pq.encodings.is_empty(), "no encoding named");
        let mut sorted = pq.encodings.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted, pq.encodings, "an encoding was named two times");
        assert!(pq.encodings.iter().all(|s| !s.contains('/')));
        assert!(!pq.compression.is_empty(), "no codec named");
    }

    #[test]
    fn the_band_measures_each_column_of_the_view_in_one_query() {
        let d = tmpdir("band");
        let p = write_csv(&d, "t.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let schema = e.describe(&View::default()).unwrap();
        let band = e.column_band(&View::default(), &schema.columns, true).unwrap();

        assert_eq!(band.len(), 4, "one brief for each column");
        let by = |n: &str| band.iter().find(|b| b.column == n).unwrap().clone();
        assert_eq!(by("id").n_total, 3);
        assert_eq!(by("id").n_present, 3);
        assert_eq!(by("id").null_count(), 0);
        assert_eq!(by("id").n_distinct, Some(3));
        assert_eq!(by("id").min.as_deref(), Some("1"));
        assert_eq!(by("id").max.as_deref(), Some("3"));
        // The third row of the sample holds no amount.
        assert_eq!(by("amount").null_count(), 1);
        assert!((by("amount").null_pct() - 100.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn the_band_follows_the_filter_of_the_view() {
        // The band describes what the grid shows. A NULL share over the whole
        // file would be a lie under a filter.
        let d = tmpdir("band-filter");
        let p = write_csv(&d, "t.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let schema = e.describe(&View::default()).unwrap();
        let view = View {
            filter: Some("name = 'bob'".into()),
            ..Default::default()
        };
        let band = e.column_band(&view, &schema.columns, true).unwrap();
        let amount = band.iter().find(|b| b.column == "amount").unwrap();
        assert_eq!(amount.n_total, 1);
        assert_eq!(amount.n_present, 0, "the one row of bob holds no amount");
        assert_eq!(amount.null_pct(), 100.0);
        assert_eq!(amount.min, None, "each value of the column is NULL");
    }

    #[test]
    fn the_band_of_a_parquet_file_comes_from_the_footer() {
        // The footer holds the number of rows and the number of NULL values of
        // each column. The compact band needs nothing more, so it costs no scan
        // of the data, also on a file of some gigabytes.
        let d = tmpdir("band-footer");
        let p = d.join("t.parquet");
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "COPY (SELECT i AS id, CASE WHEN i % 4 = 0 THEN NULL ELSE ('n' || i) END AS label \
             FROM range(1000) t(i)) TO '{}' (FORMAT parquet);",
            db_path(&p)
        ))
        .unwrap();

        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let schema = e.describe(&View::default()).unwrap();
        let meta = e.file_meta().unwrap();
        let footer = footer_briefs(&meta, &schema.columns).expect("the footer answers");

        assert_eq!(footer.len(), 2);
        assert_eq!(footer[0].n_total, 1000);
        assert_eq!(footer[0].null_count(), 0);
        assert_eq!(footer[1].null_count(), 250, "one row in four holds no label");
        // The footer holds no count of the different values, so the band knows
        // that it did not measure one.
        assert!(footer.iter().all(|b| b.n_distinct.is_none()));

        // The numbers of the footer must be the numbers of a full scan.
        let scanned = e.column_band(&View::default(), &schema.columns, true).unwrap();
        for (f, s) in footer.iter().zip(&scanned) {
            assert_eq!(f.n_total, s.n_total, "{}", f.column);
            assert_eq!(f.n_present, s.n_present, "{}", f.column);
        }
    }

    #[test]
    fn the_compact_band_measures_the_counts_and_leaves_the_rest_unknown() {
        // The compact band draws the share of NULL values alone, so the engine
        // must not measure the three facts that only the detailed band draws.
        // The counts must still be the counts of a full measurement.
        let d = tmpdir("band-compact");
        let p = d.join("t.csv");
        std::fs::write(
            &p,
            "id,name\n1,alice\n2,\n3,carol\n4,\n5,erin\n",
        )
        .unwrap();

        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let schema = e.describe(&View::default()).unwrap();

        let compact = e.column_band(&View::default(), &schema.columns, false).unwrap();
        let full = e.column_band(&View::default(), &schema.columns, true).unwrap();
        assert_eq!(compact.len(), 2);

        for (c, f) in compact.iter().zip(&full) {
            assert_eq!(c.n_total, f.n_total, "{}", c.column);
            assert_eq!(c.n_present, f.n_present, "{}", c.column);
            // `None` says that Peruse did not measure these, so the detailed
            // band asks again instead of drawing a blank.
            assert_eq!(c.n_distinct, None, "{}", c.column);
            assert_eq!(c.min, None, "{}", c.column);
            assert_eq!(c.max, None, "{}", c.column);
        }
        assert_eq!(compact[1].n_total, 5);
        assert_eq!(compact[1].n_present, 3, "two rows hold no name");
        // The full measurement does give the three facts.
        assert_eq!(full[1].n_distinct, Some(3));
        assert_eq!(full[1].min.as_deref(), Some("alice"));
    }

    #[test]
    fn a_file_of_text_that_the_reader_cannot_read_names_the_options_of_peruse() {
        // DuckDB lists its fixes in its own words: `ignore_errors=true`,
        // `delimiter=','`. A user of Peruse types `--ignore-errors`, and the
        // message of DuckDB never says so.
        //
        // A file with two kinds of line ending is the way to reach this state
        // that a user meets by accident: a file that a program on Windows wrote
        // one row at a time can hold both kinds.
        let d = tmpdir("bad-csv");
        let p = d.join("mixed.csv");
        std::fs::write(&p, "id,name\n1,alice\n2,bob\r\n").unwrap();

        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default())
            .map(|_| ())
            .unwrap_err();
        let msg = format!("{e:#}");

        for option in [
            "--delimiter",
            "--no-header",
            "--all-varchar",
            "--ignore-errors",
            "--sample-size",
        ] {
            assert!(msg.contains(option), "the message does not name {option}:\n{msg}");
        }
        assert!(
            msg.contains("line ending"),
            "the message must name the cause that a user meets:\n{msg}"
        );
        // The list of fixes of DuckDB is gone, and its reason stays.
        assert!(!msg.contains("ignore_errors=true"), "the words of DuckDB stayed:\n{msg}");
        assert!(msg.contains("The reader said:"), "the reason of DuckDB went away:\n{msg}");
        // The message must stay short enough to read on a terminal. The message of
        // DuckDB by itself runs to more than twenty lines.
        assert!(msg.lines().count() <= 16, "the message is too long:\n{msg}");
        // A line that ends with a colon promises a list. The last line of the
        // part from DuckDB must therefore not end with one.
        let last = msg.lines().last().unwrap_or("").trim().to_string();
        assert!(
            !last.ends_with(':'),
            "the message ends with a heading and no list under it: {last:?}"
        );
    }

    #[test]
    fn a_failure_that_is_not_the_sniffer_keeps_its_own_message() {
        // Only the sniffer has that list of fixes. A file that is not there must
        // not get advice about a delimiter.
        let d = tmpdir("no-such-file");
        let p = d.join("not-there.parquet");
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default())
            .map(|_| ())
            .unwrap_err();
        let msg = format!("{e:#}");
        assert!(!msg.contains("--delimiter"), "advice that does not fit:\n{msg}");
    }

    #[test]
    fn a_partial_set_of_null_counts_in_the_footer_gives_no_count() {
        // A Parquet writer may leave the statistics out, and it may leave them
        // out of some row groups and not others. The function `sum` skips a NULL
        // input, so a partial set of counts adds up to a number that looks
        // correct and is too small. The band and the metadata panel would then
        // show a share of NULL values that is wrong.
        //
        // DuckDB always writes the statistics, so a file with a partial set is
        // not one that this test can write. The test therefore drives the same
        // expression that `parquet_column_stats` uses over a list of values, one
        // list with a hole in it and one list without.
        let conn = Connection::open_in_memory().unwrap();
        let guard = "SELECT CASE WHEN count(*) = count(n) THEN sum(n)::BIGINT END FROM ";

        let partial: Option<i64> = conn
            .query_row(
                &format!("{guard} (VALUES (1), (CAST(NULL AS BIGINT)), (3)) AS t(n)"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(partial, None, "a row group with no statistic must give none");

        let whole: Option<i64> = conn
            .query_row(&format!("{guard} (VALUES (1), (2), (3)) AS t(n)"), [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(whole, Some(6), "a full set must still add up");

        // Without the guard the partial set gives 4, which is the fault.
        let unguarded: Option<i64> = conn
            .query_row(
                "SELECT sum(n)::BIGINT FROM (VALUES (1), (CAST(NULL AS BIGINT)), (3)) AS t(n)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(unguarded, Some(4), "this is the number that looks correct");
    }

    #[test]
    fn the_footer_cannot_answer_for_a_column_that_holds_a_structure() {
        // The footer names each leaf by its path, such as `s.a`. It therefore
        // holds no row for the column `s`, and the band must measure the columns
        // with a query instead of showing a wrong number.
        let d = tmpdir("band-footer-struct");
        let p = d.join("t.parquet");
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "COPY (SELECT i AS id, {{'a': i, 'b': 'x'}} AS s FROM range(10) t(i)) \
             TO '{}' (FORMAT parquet);",
            db_path(&p)
        ))
        .unwrap();

        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let schema = e.describe(&View::default()).unwrap();
        let meta = e.file_meta().unwrap();
        assert!(footer_briefs(&meta, &schema.columns).is_none());

        // The query answers for the same columns. A structure has no order, so
        // it has no smallest value and no largest value.
        let band = e.column_band(&View::default(), &schema.columns, true).unwrap();
        let s = band.iter().find(|b| b.column == "s").unwrap();
        assert_eq!(s.n_total, 10);
        assert_eq!(s.n_present, 10);
        assert_eq!(s.min, None);
        assert!(s.n_distinct.is_some(), "a structure still has a count");
    }

    /// Prints what the detail band costs on a file that the caller names.
    ///
    /// The test does not assert. It measures the two ways that the band gets its
    /// facts, so a person can see the difference between them:
    ///
    /// ```text
    /// PERUSE_BAND_FILE=sample/sample.parquet cargo test --release -p peruse-core \
    ///     the_cost_of_the_band -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "prints times instead of asserting"]
    fn the_cost_of_the_band() {
        let Ok(path) = std::env::var("PERUSE_BAND_FILE") else {
            println!("set PERUSE_BAND_FILE to the path of a file");
            return;
        };
        let e = Engine::open(&path, &OpenOptions::default()).unwrap();
        let schema = e.describe(&View::default()).unwrap();
        let cols = &schema.columns;
        println!("{path}\n{} columns", cols.len());

        let t = std::time::Instant::now();
        let meta = e.file_meta().unwrap();
        let read_footer = t.elapsed();
        let t = std::time::Instant::now();
        let footer = footer_briefs(&meta, cols);
        println!(
            "footer            {:>9.1} ms (+{:.1} ms) {}",
            read_footer.as_secs_f64() * 1000.0,
            t.elapsed().as_secs_f64() * 1000.0,
            if footer.is_some() { "answers" } else { "cannot answer" }
        );

        for (label, view) in [
            ("band, whole file", View::default()),
            (
                "band, filtered",
                View {
                    filter: Some(format!("{} IS NOT NULL", quote_ident(&cols[0].name))),
                    ..Default::default()
                },
            ),
        ] {
            let t = std::time::Instant::now();
            let n = e.column_band(&view, cols, true).unwrap().len();
            println!(
                "{label:<18}{:>9.1} ms ({n} columns)",
                t.elapsed().as_secs_f64() * 1000.0
            );
        }

        // The statistics panel measures one column. The band measures all of
        // them, so the two numbers say what the band costs beside the panel.
        let t = std::time::Instant::now();
        e.column_stats(&View::default(), &cols[0], 8).unwrap();
        println!(
            "stats, one column {:>9.1} ms",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }

    #[test]
    fn a_band_of_no_columns_asks_the_engine_nothing() {
        let d = tmpdir("band-empty");
        let p = write_csv(&d, "t.csv", SAMPLE);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        assert!(e.column_band(&View::default(), &[], true).unwrap().is_empty());
    }

    #[test]
    fn a_page_keeps_the_order_of_its_rows_through_the_projection() {
        // The page statement takes the rows inside and changes their values
        // outside. The rows must arrive in the order of the view. A file with
        // many rows makes DuckDB use each of its threads, so a projection that
        // mixed the rows would show here.
        let d = tmpdir("page-order");
        let p = write_parquet(&d, "t.parquet", 200_000);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let schema = e.describe(&View::default()).unwrap();

        // The order of the file.
        let page = e.page(&View::default(), &schema, 50, 1000).unwrap();
        assert_eq!(page.nrows, 50);
        for r in 0..50 {
            assert_eq!(page.cell(r, 0), Some((1000 + r).to_string().as_str()));
        }

        // The order of a sort.
        let sorted = View {
            sort: vec![crate::query::SortKey {
                column: "id".into(),
                dir: crate::query::SortDir::Desc,
            }],
            ..Default::default()
        };
        let page = e.page(&sorted, &schema, 50, 0).unwrap();
        for r in 0..50 {
            assert_eq!(page.cell(r, 0), Some((199_999 - r).to_string().as_str()));
        }

        // The order of a sort, with a filter.
        let filtered = View {
            filter: Some("id % 3 = 0".into()),
            ..sorted.clone()
        };
        let page = e.page(&filtered, &schema, 20, 0).unwrap();
        // 199,998 is the largest number below 200,000 that 3 divides.
        for r in 0..20 {
            assert_eq!(page.cell(r, 0), Some((199_998 - 3 * r).to_string().as_str()));
        }
    }

    #[test]
    fn a_search_gives_the_first_matches_across_the_window_it_reads_first() {
        // The search reads a window of 8192 rows in front of the cursor, and
        // then the remainder. The matches of the two parts must join into one
        // list that holds the first matches of the view, in order.
        let d = tmpdir("search-window");
        let mut body = String::from("id,note\n");
        for i in 0..20_000u32 {
            let note = if i % 5_000 == 4_999 { "needle" } else { "plain" };
            body.push_str(&format!("{i},{note}\n"));
        }
        let p = write_csv(&d, "s.csv", &body);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let view = View::default();
        let schema = e.describe(&view).unwrap();

        // The first match is inside the window, and the other three are not.
        let hits = e.search(&view, &schema, "needle", 0, 20_000, 10).unwrap();
        assert_eq!(hits, vec![4_999, 9_999, 14_999, 19_999]);

        // A limit that the window fills must not read the remainder, and it must
        // still give the first match.
        let hits = e.search(&view, &schema, "needle", 0, 20_000, 1).unwrap();
        assert_eq!(hits, vec![4_999]);

        // A limit that the window does not fill takes matches from both parts,
        // in order, and it stops at the limit.
        let hits = e.search(&view, &schema, "needle", 0, 20_000, 3).unwrap();
        assert_eq!(hits, vec![4_999, 9_999, 14_999]);

        // A part that starts after the first match rebases the offsets.
        let hits = e.search(&view, &schema, "needle", 10_000, 10_000, 10).unwrap();
        assert_eq!(hits, vec![14_999, 19_999]);

        // A needle that no row holds reads every row of the part and finds none.
        assert!(e.search(&view, &schema, "nothing", 0, 20_000, 10).unwrap().is_empty());

        // A limit of zero rows asks for nothing.
        assert!(e.search(&view, &schema, "needle", 0, 20_000, 0).unwrap().is_empty());
    }

    #[test]
    fn a_search_over_a_sorted_view_gives_the_first_matches_in_the_sorted_order() {
        // A sorted view reads its part in one statement, because each window
        // would need its own sort. The offsets must still follow the sort.
        let d = tmpdir("search-window-sorted");
        let mut body = String::from("id,note\n");
        for i in 0..20_000u32 {
            let note = if i % 5_000 == 4_999 { "needle" } else { "plain" };
            body.push_str(&format!("{i},{note}\n"));
        }
        let p = write_csv(&d, "s.csv", &body);
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let view = View {
            sort: vec![crate::query::SortKey {
                column: "id".into(),
                dir: crate::query::SortDir::Desc,
            }],
            ..Default::default()
        };
        let schema = e.describe(&View::default()).unwrap();
        let hits = e.search(&view, &schema, "needle", 0, 20_000, 10).unwrap();
        // With the identifiers in falling order, row 19,999 comes first.
        assert_eq!(hits, vec![0, 5_000, 10_000, 15_000]);
        // The offset must select a row that matches.
        let page = e.page(&view, &schema, 1, hits[1]).unwrap();
        assert_eq!(page.cell(0, 1), Some("needle"));
    }

    #[test]
    fn a_search_finds_a_needle_that_holds_a_wildcard_character() {
        // The search uses contains() and not LIKE, so `%` and `_` are plain
        // characters. A user who looks for "50%" must not match "509".
        let d = tmpdir("search-wildcard");
        let p = write_csv(&d, "s.csv", "note\n50% off\n509 items\nsale_price\nsaleXprice\n");
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        let view = View::default();
        let schema = e.describe(&view).unwrap();
        assert_eq!(e.search(&view, &schema, "50%", 0, 100, 10).unwrap(), vec![0]);
        assert_eq!(e.search(&view, &schema, "sale_", 0, 100, 10).unwrap(), vec![2]);
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

    // ------------------------------------------------- a database as a source

    /// Writes a DuckDB database file and gives its path.
    ///
    /// The test writes the file with a connection of its own, and it closes
    /// that connection before the engine attaches the file. The engine never
    /// writes a database.
    fn write_db(dir: &Path, name: &str, sql: &str) -> PathBuf {
        let p = dir.join(name);
        let conn = Connection::open(&p).unwrap();
        conn.execute_batch(sql).unwrap();
        drop(conn);
        p
    }

    /// The options of an open that names one table.
    fn with_table(name: &str) -> OpenOptions {
        OpenOptions {
            table: Some(name.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn a_database_with_one_table_opens_straight_away() {
        // One table needs no question. The name of the file says nothing about
        // the format, so this file also proves the test by the first bytes.
        let d = tmpdir("db-one");
        let p = write_db(
            &d,
            "shop.db",
            "CREATE TABLE sales AS SELECT i AS id, ('n' || i) AS name FROM range(5) t(i);",
        );
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        assert_eq!(e.source.format, Format::DuckDb);
        assert_eq!(e.table().map(|t| t.name.clone()), Some("sales".into()));

        let view = View::default();
        let schema = e.describe(&view).unwrap();
        assert_eq!(
            schema.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["id", "name"]
        );
        assert_eq!(e.count(&view).unwrap(), 5);
        assert_eq!(e.page(&view, &schema, 10, 0).unwrap().cell(1, 1), Some("n1"));

        // The panel shows the two statements that open the table. A user can
        // paste them into another program and get the same rows.
        let text = e.read_expr();
        assert!(text.contains("ATTACH"), "{text}");
        assert!(text.contains("READ_ONLY"), "{text}");
        assert!(text.contains("\"sales\""), "{text}");
        assert!(text.len() < 300, "the read expression is long: {text}");

        // The title bar names the table. The name of the file does not say
        // which rows the grid shows.
        assert_eq!(e.source.title(), "shop.db · main.sales");
    }

    #[test]
    fn a_statement_of_the_user_can_read_a_second_table_of_the_database() {
        // The view `src` shows one table. The metadata panel names the alias of
        // the attached database, so a statement in the SQL prompt can join that
        // table with another table of the same file.
        let d = tmpdir("db-second-table");
        let p = write_db(
            &d,
            "shop.duckdb",
            "CREATE TABLE customers AS SELECT 1 AS id, 'ann' AS name;\n\
             CREATE TABLE orders AS SELECT 1 AS customer, 5 AS qty;",
        );
        let e = Engine::open(p.to_str().unwrap(), &with_table("orders")).unwrap();
        let view = View {
            base: crate::query::Base::Sql(
                "SELECT c.name, o.qty FROM src o \
                 JOIN __peruse_db.main.customers c ON c.id = o.customer"
                    .into(),
            ),
            ..Default::default()
        };
        let schema = e.describe(&view).unwrap();
        let page = e.page(&view, &schema, 10, 0).unwrap();
        assert_eq!(page.nrows, 1);
        assert_eq!(page.cell(0, 0), Some("ann"));
        // The alias is in the text that the panel shows, so the user can read
        // it there.
        assert!(e.read_expr().contains("__peruse_db"), "{}", e.read_expr());
    }

    #[test]
    fn a_database_needs_no_index() {
        // A table of a database is in blocks already, and the database holds
        // the count of the rows. An index would copy the table for nothing.
        let d = tmpdir("db-index");
        let p = write_db(&d, "one.duckdb", "CREATE TABLE t AS SELECT 1 AS a;");
        let mut e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();
        assert!(e.is_seekable(), "a database table gives direct access");
        e.materialize().unwrap();
        assert!(!e.indexed, "the engine copied the table for nothing");
    }

    #[test]
    fn the_table_option_chooses_among_the_tables_of_a_database() {
        let d = tmpdir("db-many");
        let p = write_db(
            &d,
            "shop.duckdb",
            "CREATE TABLE customers AS SELECT 1 AS id;\n\
             CREATE TABLE orders AS SELECT i AS id, (i * 2) AS qty FROM range(9) t(i);\n\
             CREATE VIEW big_orders AS SELECT * FROM orders WHERE qty > 10;",
        );

        let e = Engine::open(p.to_str().unwrap(), &with_table("orders")).unwrap();
        assert_eq!(e.count(&View::default()).unwrap(), 9);

        // A view is a table of the database as well.
        let e = Engine::open(p.to_str().unwrap(), &with_table("big_orders")).unwrap();
        assert_eq!(e.count(&View::default()).unwrap(), 3);
        assert!(e.table().unwrap().is_view);

        // The schema is part of the name.
        let e = Engine::open(p.to_str().unwrap(), &with_table("main.customers")).unwrap();
        assert_eq!(e.count(&View::default()).unwrap(), 1);

        // With no name, the message says how to choose and names the tables.
        let err = Engine::open(p.to_str().unwrap(), &OpenOptions::default())
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(err.contains("--table"), "{err}");
        assert!(err.contains("2 tables and 1 view"), "{err}");
        assert!(err.contains("orders"), "{err}");
        assert!(err.contains("big_orders (view)"), "{err}");

        // A name that the database does not hold names the tables too.
        let err = Engine::open(p.to_str().unwrap(), &with_table("nope"))
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(err.contains("no table \"nope\""), "{err}");
        assert!(err.contains("customers"), "{err}");
    }

    #[test]
    fn a_table_name_that_needs_quotation_marks_opens() {
        // A name with a space, and a name with capital letters, both go into
        // the statement in quotation marks. Without the marks, the view would
        // not open at all.
        let d = tmpdir("db-quoting");
        let p = write_db(
            &d,
            "odd.duckdb",
            "CREATE TABLE \"order items\" AS SELECT 1 AS id, 'shoe' AS what;\n\
             CREATE TABLE \"Orders\" AS SELECT i AS id FROM range(4) t(i);",
        );

        let e = Engine::open(p.to_str().unwrap(), &with_table("order items")).unwrap();
        assert_eq!(e.count(&View::default()).unwrap(), 1);
        let schema = e.describe(&View::default()).unwrap();
        assert_eq!(e.page(&View::default(), &schema, 1, 0).unwrap().cell(0, 1), Some("shoe"));

        // DuckDB reads a name without regard to the case of the letters, and
        // the option follows it.
        let e = Engine::open(p.to_str().unwrap(), &with_table("orders")).unwrap();
        assert_eq!(e.count(&View::default()).unwrap(), 4);
        assert_eq!(e.table().map(|t| t.name.clone()), Some("Orders".into()));
    }

    #[test]
    fn the_read_only_flag_stops_a_write_to_the_database() {
        // The promise of Peruse is stronger for a database than for a data
        // file: the storage engine refuses the write, and not the guard over
        // the words of the statement. This test therefore goes around the
        // guard and writes straight to the connection.
        let d = tmpdir("db-readonly");
        let p = write_db(&d, "shop.duckdb", "CREATE TABLE t AS SELECT 1 AS a;");
        let e = Engine::open(p.to_str().unwrap(), &OpenOptions::default()).unwrap();

        for sql in [
            "INSERT INTO \"__peruse_db\".\"main\".\"t\" VALUES (2)",
            "DELETE FROM \"__peruse_db\".\"main\".\"t\"",
            "CREATE TABLE \"__peruse_db\".\"main\".\"u\" AS SELECT 1",
            "DROP TABLE \"__peruse_db\".\"main\".\"t\"",
        ] {
            let err = e.conn.execute_batch(sql).unwrap_err().to_string();
            assert!(
                err.to_lowercase().contains("read-only") || err.to_lowercase().contains("read only"),
                "statement {sql} gave {err}"
            );
        }
        // The rows of the file did not change.
        assert_eq!(e.count(&View::default()).unwrap(), 1);
    }

    #[test]
    fn a_user_cannot_type_an_attach_into_the_query_box() {
        // Peruse attaches a database itself, and that statement goes straight
        // to the connection. The guard reads the statements of the user, and it
        // must keep refusing each of these four words.
        for sql in [
            "ATTACH 'other.duckdb' AS other (READ_ONLY)",
            "DETACH __peruse_db",
            "INSTALL sqlite",
            "LOAD sqlite",
            "SELECT 1; ATTACH 'other.duckdb' AS other",
        ] {
            assert!(
                crate::sql_guard::ensure_read_only(sql).is_err(),
                "the guard accepted {sql}"
            );
        }
    }

    #[test]
    fn a_filter_and_a_sort_over_a_database_give_the_same_rows_as_over_parquet() {
        // The view `src` hides where the rows come from. A database table and
        // a Parquet file of the same rows must therefore answer in the same
        // way, with no special case anywhere.
        let d = tmpdir("db-vs-parquet");
        let db = d.join("shop.duckdb");
        let pq = d.join("shop.parquet");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(&format!(
                "CREATE TABLE sales AS SELECT i AS id, ('n' || (i % 7)) AS name, \
                        (i * 1.5) AS amount FROM range(500) t(i);\n\
                 COPY sales TO '{}' (FORMAT parquet);",
                db_path(&pq)
            ))
            .unwrap();
        }

        let view = View {
            filter: Some("id % 3 = 0 AND name <> 'n1'".into()),
            sort: vec![crate::query::SortKey {
                column: "amount".into(),
                dir: crate::query::SortDir::Desc,
            }],
            ..Default::default()
        };
        let from_db = Engine::open(db.to_str().unwrap(), &with_table("sales")).unwrap();
        let from_pq = Engine::open(pq.to_str().unwrap(), &OpenOptions::default()).unwrap();

        assert_eq!(from_db.count(&view).unwrap(), from_pq.count(&view).unwrap());
        let schema = from_pq.describe(&view).unwrap();
        let a = from_db.page(&view, &schema, 40, 10).unwrap();
        let b = from_pq.page(&view, &schema, 40, 10).unwrap();
        assert_eq!(a.nrows, b.nrows);
        for r in 0..a.nrows {
            for c in 0..schema.len() {
                assert_eq!(a.cell(r, c), b.cell(r, c), "row {r} column {c}");
            }
        }

        // The statistics and the profile read the same view, so they also
        // agree. The profile is what `--ddl` measures.
        let stats_db = from_db.column_stats(&view, &schema.columns[2], 4).unwrap();
        let stats_pq = from_pq.column_stats(&view, &schema.columns[2], 4).unwrap();
        assert_eq!(stats_db.min, stats_pq.min);
        assert_eq!(stats_db.max, stats_pq.max);
    }

    #[test]
    fn the_ddl_of_a_database_table_names_that_table() {
        // The option --ddl measures the view and writes a CREATE TABLE
        // statement. With --table, the view is that table of the database, and
        // the statement carries its name.
        let d = tmpdir("db-ddl");
        let p = write_db(
            &d,
            "shop.duckdb",
            "CREATE TABLE customers AS SELECT 1 AS id;\n\
             CREATE TABLE orders AS SELECT i AS id, ('c' || i) AS code, \
                    (i * 1.25) AS total FROM range(20) t(i);",
        );
        let e = Engine::open(p.to_str().unwrap(), &with_table("orders")).unwrap();
        let name = e.table().unwrap().name.clone();
        let profile = e.profile(&View::default(), &name).unwrap();
        assert_eq!(profile.rows, 20);
        assert_eq!(profile.columns.len(), 3);
        assert_eq!(profile.key, vec![0], "the column id is the key");

        let sql = crate::ddl::render(&profile, crate::ddl::Dialect::parse("postgres").unwrap());
        assert!(sql.contains("orders"), "{sql}");
        assert!(sql.contains("code"), "{sql}");
    }

    #[test]
    fn the_list_of_tables_says_what_each_entry_is_and_costs_no_scan() {
        // The picker of the user interface reads this list before the engine
        // opens the file. The count of the rows comes from the catalog of the
        // database, so the list costs no scan. A view has no such count.
        let d = tmpdir("db-list");
        let p = write_db(
            &d,
            "shop.duckdb",
            "CREATE TABLE orders AS SELECT i AS id FROM range(30) t(i);\n\
             CREATE TABLE customers AS SELECT 1 AS id;\n\
             CREATE VIEW recent AS SELECT * FROM orders WHERE id > 25;",
        );
        let tables = database_tables(&p, &OpenOptions::default()).unwrap();
        assert_eq!(tables.len(), 3);
        // The tables come first, and each group follows the alphabet.
        let names: Vec<String> = tables.iter().map(|t| t.label()).collect();
        assert_eq!(names, ["main.customers", "main.orders", "main.recent"]);
        assert!(!tables[0].is_view && !tables[1].is_view);
        assert!(tables[2].is_view, "the view comes last");

        let orders = tables.iter().find(|t| t.name == "orders").unwrap();
        assert_eq!(orders.rows, Some(30), "the catalog knows the count");
        assert_eq!(tables[2].rows, None, "a view has no count to read");
        assert_eq!(orders.qualified(), "\"__peruse_db\".\"main\".\"orders\"");
    }

    #[test]
    fn a_database_with_no_table_says_so() {
        let d = tmpdir("db-empty");
        // A database file needs one write before DuckDB makes the file. The
        // table goes away again, so the file holds a catalog with nothing in
        // it.
        let p = write_db(
            &d,
            "empty.duckdb",
            "CREATE TABLE gone AS SELECT 1 AS a;\nDROP TABLE gone;",
        );
        let err = Engine::open(p.to_str().unwrap(), &OpenOptions::default())
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(err.contains("no table"), "{err}");
    }

    #[test]
    fn a_glob_that_matches_databases_is_refused() {
        // A set of files becomes the rows of one table. Two databases cannot
        // join in that way, and the message must say what to do instead.
        let d = tmpdir("db-glob");
        write_db(&d, "a.duckdb", "CREATE TABLE t AS SELECT 1 AS a;");
        write_db(&d, "b.duckdb", "CREATE TABLE t AS SELECT 2 AS a;");
        let pattern = format!("{}/*.duckdb", d.to_string_lossy());
        let err = Engine::open(&pattern, &OpenOptions::default())
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(err.contains("one database at a time"), "{err}");

        // A glob that matches one database is still a glob.
        let one = format!("{}/a.*", d.to_string_lossy());
        let err = Engine::open(&one, &OpenOptions::default())
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(err.contains("one database at a time"), "{err}");
    }

    #[test]
    fn a_sqlite_file_is_refused_by_name() {
        // Without this message, the file would go to the CSV reader, and the
        // user would see a parse failure about a line of binary data. The name
        // of the file says nothing, so the test uses a name that misleads.
        let d = tmpdir("sqlite");
        let p = d.join("shop.duckdb");
        let mut body = b"SQLite format 3\0".to_vec();
        body.extend_from_slice(&[0u8; 64]);
        std::fs::write(&p, &body).unwrap();

        let msg = match Engine::open(p.to_str().unwrap(), &OpenOptions::default()) {
            Ok(_) => panic!("a SQLite file must not open"),
            Err(e) => format!("{e:#}"),
        };
        assert!(msg.contains("SQLite"), "{msg}");
        assert!(msg.contains("cannot read one yet"), "{msg}");
        assert!(msg.contains("sqlite3"), "no way forward given: {msg}");
    }

    #[test]
    fn a_database_from_a_newer_duckdb_asks_for_a_newer_peruse() {
        // A file with a newer storage version cannot attach. The words of
        // DuckDB name the version and say nothing about Peruse, so the message
        // must say which program needs the change. The test uses the text of
        // DuckDB, because no test can write a file of a version that does not
        // exist yet.
        let duck = "IO Error: Trying to read a database file with version number 64, \
                    but we can only read version 51. The database file was created with \
                    a newer version of DuckDB.";
        let msg = attach_message("shop.duckdb", duck);
        assert!(msg.contains("needs a newer Peruse"), "{msg}");
        assert!(msg.contains("64"), "the version of the file is missing: {msg}");
        assert!(msg.contains(duck), "the words of DuckDB are missing: {msg}");

        // A file that holds something else gives the words of DuckDB, and no
        // guess about a version.
        let other = "IO Error: The file is not a valid DuckDB database file!";
        let msg = attach_message("odd.duckdb", other);
        assert!(msg.contains("cannot attach"), "{msg}");
        assert!(!msg.contains("newer Peruse"), "{msg}");

        // A file of an older version also fails to attach, and a newer Peruse
        // would not read it either. The message must not send the user after
        // one.
        let older = "IO Error: Trying to read a database file with version number 39, \
                     but we can only read version 64. The database file was created with \
                     an older version of DuckDB.";
        let msg = attach_message("old.duckdb", older);
        assert!(msg.contains("cannot attach"), "{msg}");
        assert!(!msg.contains("newer Peruse"), "{msg}");

        assert_eq!(storage_version(duck).as_deref(), Some("64"));
        assert_eq!(storage_version("no numbers here"), None);

        // The words of DuckDB hold the path of the file, and a directory can
        // carry the word `version` with a number after it. The version of the
        // file is the number after the words `version number`.
        let with_path = "IO Error: Cannot open file \"C:/exports/version3/shop.duckdb\": \
                         Trying to read a database file with version number 66, but we can \
                         only read version 64. The database file was created with a newer \
                         version of DuckDB.";
        assert_eq!(storage_version(with_path).as_deref(), Some("66"));
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

