//! The description of the file or the file set that Peruse shows. This module
//! also finds the format of the data.

use std::fmt;
use std::path::{Path, PathBuf};

/// The format of the data in a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// The Parquet format: a column format with a footer.
    Parquet,
    /// A text format with one row on each line, such as CSV or TSV.
    Csv,
    /// JSON: one object for each row, or one list of objects. The form with
    /// one object on each line also has the names NDJSON and JSON Lines.
    Json,
    /// The Arrow IPC format, which also has the name Feather version 2.
    Arrow,
    /// A DuckDB database file. Such a file holds many tables, so Peruse
    /// attaches it and reads one table of it.
    DuckDb,
    /// A SQLite database file. Peruse cannot read one, and it gives the format
    /// a name so that the message can say why.
    Sqlite,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Format::Parquet => "parquet",
            Format::Csv => "csv",
            Format::Json => "json",
            Format::Arrow => "arrow",
            Format::DuckDb => "duckdb",
            Format::Sqlite => "sqlite",
        })
    }
}

/// A set of files that Peruse shows as one table.
#[derive(Clone, Debug)]
pub struct Source {
    /// The text that the user typed. The title bar and the error messages use
    /// it.
    pub input: String,
    /// The name of the first file, without the directory.
    pub label: String,
    /// The format of the data.
    pub format: Format,
    /// The paths of the files, in order.
    pub files: Vec<PathBuf>,
    /// The sum of the sizes of the files, in bytes.
    pub bytes: u64,
    /// The delimiter, when the file extension gives one. The extension `.tsv`
    /// gives a tab. The sniffer then does not need to look for a delimiter
    /// that Peruse knows.
    pub delimiter: Option<char>,
    /// `true` when the file name ends with `.gz`, `.zst` or `.bz2`.
    pub compressed: bool,
    /// The table that the grid shows, for a database source.
    ///
    /// A file holds one table, and that table has no name of its own, so the
    /// value is `None` for a file.
    pub table: Option<String>,
}

impl Source {
    /// Gives `true` when the source holds more than one file.
    pub fn is_multi(&self) -> bool {
        self.files.len() > 1
    }

    /// Gives the text for the title bar: the name of the file, or a name and
    /// the number of the other files, such as `name.parquet +3`.
    ///
    /// A database holds many tables, so the title also names the table that the
    /// grid shows. The name of the file alone would not say which rows these
    /// are.
    pub fn title(&self) -> String {
        if let Some(table) = &self.table {
            return format!("{} · {table}", self.label);
        }
        if self.is_multi() {
            format!("{} +{}", self.label, self.files.len() - 1)
        } else {
            self.label.clone()
        }
    }
}

/// Removes the extension `.gz`, `.zst` or `.bz2` from the name of a file, and
/// gives the extension below it.
///
/// The function also gives `true` when it removes one of these extensions.
fn effective_extension(path: &Path) -> (Option<String>, bool) {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let compressed = name.ends_with(".gz") || name.ends_with(".zst") || name.ends_with(".bz2");
    let stem = if compressed {
        name.rsplit_once('.').map(|(a, _)| a).unwrap_or(&name).to_string()
    } else {
        name
    };
    let ext = stem.rsplit_once('.').map(|(_, b)| b.to_string());
    (ext, compressed)
}

/// The number of bytes that [`sniff_magic`] reads from the start of a file.
const MAGIC_HEAD_LEN: usize = 4096;

/// The byte order mark of UTF-8.
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// Gives `true` when a JSON file starts with a byte order mark and then a list.
///
/// The JSON reader of DuckDB does not see the list behind such a mark. It reads
/// the whole file as one row, with one column `json` that holds the list. The
/// caller then tells the reader that the file holds a list. Refer to
/// `build_read_expr` in [`crate::engine`].
///
/// A program on Windows writes such a file often: PowerShell 5 writes the mark
/// for each file that it writes as UTF-8.
pub fn json_list_behind_bom(path: &Path) -> bool {
    use std::io::Read;
    let Ok(f) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = Vec::with_capacity(MAGIC_HEAD_LEN);
    if f.take(MAGIC_HEAD_LEN as u64).read_to_end(&mut head).is_err() {
        return false;
    }
    let Some(text) = head.strip_prefix(UTF8_BOM) else {
        return false;
    };
    text.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'[')
}

/// Reads the first bytes of the file and finds the format from them.
///
/// A Parquet file starts with `PAR1`, and an Arrow IPC file starts with
/// `ARROW1`. A JSON file starts with `{` or `[`, after any spaces. These marks
/// are sufficient to tell such a file from a text file with rows.
fn sniff_magic(path: &Path) -> Option<Format> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    // Read what the file holds, up to the limit. A file of eight bytes is
    // short, but it is still a file, and `read_exact` would give an error for
    // it. One call to `read` can give fewer bytes than the caller asked for,
    // so `read_to_end` over a reader with a limit fills the head.
    //
    // The head is long enough for the spaces and the empty lines that can
    // come before the first `{` of a JSON file.
    let mut head = Vec::with_capacity(MAGIC_HEAD_LEN);
    f.take(MAGIC_HEAD_LEN as u64).read_to_end(&mut head).ok()?;
    if head.starts_with(b"PAR1") {
        return Some(Format::Parquet);
    }
    if head.starts_with(b"ARROW1") {
        return Some(Format::Arrow);
    }
    // A program on Windows often writes a byte order mark in front of UTF-8
    // text. The mark is not a part of the data.
    let text = head.strip_prefix(UTF8_BOM).unwrap_or(&head);
    match text.iter().find(|b| !b.is_ascii_whitespace()) {
        Some(b'{') | Some(b'[') => Some(Format::Json),
        _ => None,
    }
}

/// The position of the mark `DUCK` in a DuckDB database file.
///
/// The first eight bytes of the file hold a checksum of the header, and the
/// mark follows them.
const DUCKDB_MARK_AT: usize = 8;

/// The mark of a DuckDB database file.
const DUCKDB_MARK: &[u8] = b"DUCK";

/// The mark of a SQLite database file. The zero byte is part of it.
const SQLITE_MARK: &[u8] = b"SQLite format 3\0";

/// The number of bytes that [`sniff_database`] needs for both marks.
const DB_HEAD_LEN: usize = if DUCKDB_MARK_AT + DUCKDB_MARK.len() > SQLITE_MARK.len() {
    DUCKDB_MARK_AT + DUCKDB_MARK.len()
} else {
    SQLITE_MARK.len()
};

/// Reads the first bytes of the file and finds a database format.
///
/// The name of a database file says nothing certain. A file `.db` can hold a
/// DuckDB database, a SQLite database or something else, and a user can give a
/// database any name at all. The first bytes are therefore the only honest
/// test, and [`detect`] makes this test before it reads the extension.
///
/// A DuckDB file holds `DUCK` after its checksum. A SQLite file starts with
/// `SQLite format 3` and one zero byte.
fn sniff_database(path: &Path) -> Option<Format> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    // One read can give fewer bytes than the caller asked for. The function
    // `read_to_end` over a reader with a limit therefore reads until the head is
    // full or the file ends. A file that is shorter than the marks is not a
    // database, and `read_exact` would give an error for it.
    let mut head = Vec::with_capacity(DB_HEAD_LEN);
    f.take(DB_HEAD_LEN as u64).read_to_end(&mut head).ok()?;
    if head.starts_with(SQLITE_MARK) {
        return Some(Format::Sqlite);
    }
    let end = DUCKDB_MARK_AT + DUCKDB_MARK.len();
    if head.len() >= end && &head[DUCKDB_MARK_AT..end] == DUCKDB_MARK {
        return Some(Format::DuckDb);
    }
    None
}

/// Finds the format from the extension of the file, and the delimiter that
/// the extension gives.
///
/// The function reads the name only, and it never opens the file. The chooser
/// of files calls it for each entry of a directory, and a directory can hold
/// thousands of them.
///
/// The function gives `None` for an extension that Peruse does not know. The
/// full function [`detect`] then looks at the first bytes of the file.
pub fn by_extension(path: &Path) -> Option<(Format, Option<char>)> {
    let (ext, _) = effective_extension(path);
    match ext.as_deref() {
        Some("parquet" | "parq" | "pq") => Some((Format::Parquet, None)),
        Some("tsv" | "tab") => Some((Format::Csv, Some('\t'))),
        // The name `.csv` does not give the delimiter. A spreadsheet program
        // in a country that writes a decimal comma saves a `.csv` file with a
        // semicolon between the values, so the sniffer must decide. With a
        // comma here, each such file showed as one column.
        Some("csv") => Some((Format::Csv, None)),
        Some("psv") => Some((Format::Csv, Some('|'))),
        Some("json" | "ndjson" | "jsonl") => Some((Format::Json, None)),
        Some("arrow" | "ipc" | "feather" | "arrows") => Some((Format::Arrow, None)),
        // The chooser of files lists a database with these two extensions. The
        // first bytes decide the format of a file that Peruse opens, so a
        // database with another name still opens. Refer to [`detect`].
        Some("duckdb" | "ddb") => Some((Format::DuckDb, None)),
        _ => None,
    }
}

/// Finds the format of a file, the delimiter and the compression.
///
/// The function uses four tests, in this order:
///
/// 1. The first bytes, for a database file.
/// 2. The extension of the file.
/// 3. The first four bytes of the file.
/// 4. CSV, as the last choice.
///
/// A file `data.dat` that holds values with commas therefore opens correctly.
///
/// A database comes first, because a database can carry any name. A file
/// `sales.db` that holds a SQLite database therefore gets a message about
/// SQLite, and not a parse failure from the CSV reader.
pub fn detect(path: &Path) -> (Format, Option<char>, bool) {
    let (_, compressed) = effective_extension(path);
    // A database file holds its data in blocks that the storage engine reads.
    // No such file arrives compressed, so the answer here says `false`.
    if let Some(fmt) = sniff_database(path) {
        return (fmt, None, false);
    }
    if let Some((fmt, delim)) = by_extension(path) {
        return (fmt, delim, compressed);
    }
    match sniff_magic(path) {
        Some(fmt) => (fmt, None, compressed),
        // The extension is unknown and the file is not a Parquet file.
        // Give the file to the CSV sniffer.
        None => (Format::Csv, None, compressed),
    }
}

/// The number of records that the JSON reader of DuckDB examines when the
/// user gives no `sample_size`.
///
/// The reader finds the columns from these records, and that step is the part
/// that calls itself one time for each level. A record after them goes to a
/// step that respects `maximum_depth`, so a deep record there gives an error
/// message and not a crash.
pub const JSON_SAMPLE_RECORDS: u64 = 20_480;

/// Gives `true` when a JSON file nests deeper than `limit`.
///
/// The reader of DuckDB finds the columns with a C++ function that calls
/// itself one time for each level, and the option `maximum_depth` does not
/// stop that function. A file that nests a thousand levels deep therefore
/// fills the stack of the thread and stops the whole program. A stack that is
/// full is not a fault that Rust can catch, so Peruse has to see such a file
/// before it gives the file to the reader.
///
/// The value `records` is the number of records that the reader examines, and
/// `None` means the whole file. The function reads twice that number of
/// records, so a count that is not exact still covers each record that the
/// reader examines. A limit in bytes is not safe: a deep record can come after
/// some megabytes of shallow records, or after one long text.
///
/// The reader goes that deep only in the records that it examines to find the
/// columns, and those are the first `records` records of the file. A record
/// after them goes into a column that is known already, and it does not fill
/// the stack. The function therefore examines the same records, and `None`
/// examines the whole file, as `--sample-size -1` does. A record is a value at
/// the top of the file, or a value inside the list at the top of the file.
///
/// A test of the first 8 MB alone was not sufficient. A file with one record
/// on each line can hold a deep record at line 9,000, and the reader examines
/// that line.
///
/// This function counts the levels with a loop and a number, so it can never
/// fill the stack itself. It stops at the first level above the limit, so a
/// file that is made to be deep costs almost nothing to refuse.
///
/// A brace inside a value in quotation marks is a character of that value,
/// and not a level. The function therefore follows the quotation marks.
pub fn json_depth_over(path: &Path, limit: usize, records: Option<u64>) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = vec![0u8; 64 * 1024];
    let cap = records.map(|n| n.saturating_mul(2).max(1));

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    // A file that is one list holds one record for each item of the list.
    // Each other file holds one record for each value at the top.
    let mut one_list: Option<bool> = None;
    let mut seen = 0u64;
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => return false,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        };
        for &b in &buf[..n] {
            if in_string {
                // A backslash takes the meaning from the character after it,
                // and a quotation mark then ends the value.
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    in_string = false;
                }
                continue;
            }
            match b {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    if depth == 0 && one_list.is_none() {
                        one_list = Some(b == b'[');
                    }
                    depth += 1;
                    if depth > limit {
                        return true;
                    }
                }
                b'}' | b']' => {
                    depth = depth.saturating_sub(1);
                    let record_end = depth == 0 || (depth == 1 && one_list == Some(true));
                    if record_end {
                        seen += 1;
                        if cap.is_some_and(|c| seen >= c) {
                            return false;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Gives `true` when the name of the file ends with `.gz`, `.zst` or `.bz2`.
///
/// DuckDB decompresses a file by its extension, and not by its first bytes. A
/// file of compressed bytes with another name gives a parse failure there.
pub fn is_compressed(path: &Path) -> bool {
    effective_extension(path).1
}

/// Gives `true` when one JSON text nests deeper than `limit`.
///
/// This is the test of [`json_depth_over`] for a text in memory. A compressed
/// file needs it: Peruse cannot decompress the file, so DuckDB reads its
/// records as text, and this function counts the levels of each one.
pub fn json_text_depth_over(text: &[u8], limit: usize) -> bool {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for &b in text {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > limit {
                    return true;
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

/// Writes a path in a form that a settings file can hold, and that the system
/// can open again.
///
/// On Windows, `std::fs::canonicalize` gives a verbatim path, which starts
/// with `\\?\`. Windows gives such a path to its own calls with no change, and
/// it does not read a forward slash inside one. A change of the separators
/// would therefore give `//?/C:/...`, and that path never opens again.
///
/// The function removes that start first, and then changes the separators.
/// A path with no verbatim start accepts a forward slash on Windows.
///
/// On each other system, a backslash is an ordinary character of a name. The
/// function changes nothing there: `/data/back\slash.csv` is one file, and
/// `/data/back/slash.csv` is a different one.
pub fn tidy_path(p: &Path) -> String {
    let text = p.to_string_lossy();
    if !cfg!(windows) {
        return text.into_owned();
    }
    let plain = text
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .unwrap_or_else(|| text.strip_prefix(r"\\?\").unwrap_or(&text).to_string());
    plain.replace('\\', "/")
}

/// Reads a path that a settings file holds.
///
/// A version of Peruse before this one wrote a verbatim path with its
/// separators changed, such as `//?/C:/data/a.csv`. Windows cannot open that
/// path, so each such entry would go away in silence. The function repairs
/// it, and a user keeps the list that they built.
pub fn untidy_path(text: &str) -> PathBuf {
    if cfg!(windows)
        && let Some(rest) = text.strip_prefix("//?/")
    {
        return PathBuf::from(rest);
    }
    PathBuf::from(text)
}

/// Gives `true` when the text is a glob pattern. Peruse must then expand the
/// pattern, and it must not open the text as one path.
pub fn looks_like_glob(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

/// Writes a number of bytes with a unit, such as `1.50 KB`.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[i])
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

/// Writes a number with a comma after each group of three digits. The number
/// 12438201 becomes 12,438,201, which the user can read more quickly.
pub fn human_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_formats() {
        assert_eq!(detect(Path::new("a.parquet")).0, Format::Parquet);
        assert_eq!(detect(Path::new("a.pq")).0, Format::Parquet);
        assert_eq!(detect(Path::new("a.csv")).0, Format::Csv);
        // The sniffer finds the delimiter of a `.csv` file. Refer to
        // `by_extension`.
        assert_eq!(detect(Path::new("a.csv")).1, None);
        assert_eq!(detect(Path::new("a.tsv")).1, Some('\t'));
        assert_eq!(detect(Path::new("a.psv")).1, Some('|'));
        assert_eq!(detect(Path::new("a.json")).0, Format::Json);
        assert_eq!(detect(Path::new("a.ndjson")).0, Format::Json);
        assert_eq!(detect(Path::new("a.jsonl")).0, Format::Json);
        assert_eq!(detect(Path::new("a.arrow")).0, Format::Arrow);
        assert_eq!(detect(Path::new("a.feather")).0, Format::Arrow);
        // The chooser of files reads the name only, and it must list a
        // database.
        assert_eq!(by_extension(Path::new("a.duckdb")).unwrap().0, Format::DuckDb);
        assert_eq!(by_extension(Path::new("a.ddb")).unwrap().0, Format::DuckDb);
    }

    #[test]
    fn the_first_bytes_find_a_database_whatever_the_name_is() {
        // A database can carry any name, so the name is not a test. The mark
        // `DUCK` follows the eight bytes of the checksum.
        let dir = std::env::temp_dir().join("peruse-sniff-db");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut duck = vec![0u8; 8];
        duck.extend_from_slice(b"DUCK");
        duck.extend_from_slice(&[0u8; 32]);
        for name in ["shop.db", "shop.csv", "shop", "shop.parquet"] {
            let p = dir.join(name);
            std::fs::write(&p, &duck).unwrap();
            assert_eq!(detect(&p).0, Format::DuckDb, "file {name}");
        }

        // A SQLite file gets its own format, so that the message can name it.
        let mut lite = b"SQLite format 3\0".to_vec();
        lite.extend_from_slice(&[0u8; 32]);
        for name in ["shop.db", "shop.sqlite", "shop.duckdb"] {
            let p = dir.join(name);
            std::fs::write(&p, &lite).unwrap();
            assert_eq!(detect(&p).0, Format::Sqlite, "file {name}");
        }

        // The head that the function reads must cover both marks. The mark of
        // SQLite is the longer of the two, and a file that holds it and nothing
        // else still gets the name of SQLite.
        let p = dir.join("bare.db");
        std::fs::write(&p, b"SQLite format 3\0").unwrap();
        assert_eq!(detect(&p).0, Format::Sqlite, "the head is too short");

        // The mark must be at the right place. The four letters somewhere else
        // are a value of a file of text.
        let p = dir.join("word.csv");
        std::fs::write(&p, b"name\nDUCK\n").unwrap();
        assert_eq!(detect(&p).0, Format::Csv);

        // A file that is shorter than the marks is not a database, and it must
        // not stop the program either.
        let p = dir.join("tiny.csv");
        std::fs::write(&p, b"a\n1\n").unwrap();
        assert_eq!(detect(&p).0, Format::Csv);
    }

    #[test]
    fn the_first_bytes_give_the_format_when_the_extension_does_not() {
        let dir = std::env::temp_dir().join("peruse-sniff");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cases: [(&str, &[u8], Format); 5] = [
            ("a.bin", b"PAR1\x15\x04\x15", Format::Parquet),
            ("b.bin", b"ARROW1\x00\x00", Format::Arrow),
            ("c.bin", b"  [{\"a\": 1}]", Format::Json),
            ("d.bin", b"{\"a\": 1}\n", Format::Json),
            ("e.bin", b"id,name\n1,a\n", Format::Csv),
        ];
        for (name, body, want) in cases {
            let p = dir.join(name);
            std::fs::write(&p, body).unwrap();
            assert_eq!(detect(&p).0, want, "file {name}");
        }
        // A JSON file can start with many spaces or empty lines, or with the
        // byte order mark that a program on Windows writes. The head of 16
        // bytes saw neither, and such a file went to the CSV reader.
        let cases: [(&str, &[u8]); 2] = [
            ("f.txt", b"                              \r\n\r\n  [{\"a\": 1}]"),
            ("g.txt", b"\xEF\xBB\xBF{\"a\": 1}\n"),
        ];
        for (name, body) in cases {
            let p = dir.join(name);
            std::fs::write(&p, body).unwrap();
            assert_eq!(detect(&p).0, Format::Json, "file {name}");
        }
        let p = dir.join("h.json");
        std::fs::write(&p, b"\xEF\xBB\xBF  [{\"a\": 1}]").unwrap();
        assert!(json_list_behind_bom(&p));
        std::fs::write(&p, b"[{\"a\": 1}]").unwrap();
        assert!(!json_list_behind_bom(&p), "no mark, no change");
        std::fs::write(&p, b"\xEF\xBB\xBF{\"a\": 1}").unwrap();
        assert!(!json_list_behind_bom(&p), "an object is not a list");

        // A file that holds nothing is not a reason to stop the program.
        let empty = dir.join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(detect(&empty).0, Format::Csv);
    }

    #[test]
    fn compression_suffix_is_seen_through() {
        let (fmt, delim, gz) = detect(Path::new("data.tsv.gz"));
        assert_eq!(fmt, Format::Csv);
        assert_eq!(delim, Some('\t'));
        assert!(gz);
    }

    #[test]
    fn unknown_extension_falls_back_to_csv() {
        assert_eq!(detect(Path::new("mystery.dat")).0, Format::Csv);
    }

    #[test]
    fn a_file_that_nests_too_deep_is_seen_before_the_reader_gets_it() {
        let dir = std::env::temp_dir().join("peruse-depth");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // The list around the objects is one level of its own, so a file of
        // `d` objects nests `d + 1` levels deep.
        let nested = |d: usize| format!("[{}1{}]", "{\"a\":".repeat(d), "}".repeat(d));
        for (d, want) in [(5, false), (100, false), (127, false), (128, true), (5000, true)] {
            let p = dir.join(format!("d{d}.json"));
            std::fs::write(&p, nested(d)).unwrap();
            assert_eq!(json_depth_over(&p, 128, Some(JSON_SAMPLE_RECORDS)), want, "depth {d}");
        }
    }

    #[test]
    fn a_deep_record_after_the_first_megabytes_is_seen() {
        // The reader of DuckDB examines the first 20,480 records to find the
        // columns. With records of one kilobyte, that is 20 MB, and a test of
        // the first 8 MB did not see a deep record at line 9,000. The program
        // then stopped.
        let dir = std::env::temp_dir().join("peruse-depth-late");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let line = format!("{{\"a\": \"{}\"}}\n", "x".repeat(1000));
        let mut body = line.repeat(9_000);
        body.push_str(&format!("{{\"a\": {}1{}}}\n", "{\"b\":".repeat(3000), "}".repeat(3000)));
        let p = dir.join("late.ndjson");
        std::fs::write(&p, &body).unwrap();
        assert!(json_depth_over(&p, 128, Some(JSON_SAMPLE_RECORDS)));
        // A record after the sample does not reach the deep part of the
        // reader, so the test stops after twice the sample: the margin covers
        // a count that is not exact.
        assert!(!json_depth_over(&p, 128, Some(4_000)));
        // The value -1 of `--sample-size` examines each record.
        assert!(json_depth_over(&p, 128, None));

        // The records of a file that is one list are the values inside it.
        let list = format!("[{}{{\"a\": {}1{}}}]", "{\"a\": 1},".repeat(10), "[".repeat(200), "]".repeat(200));
        std::fs::write(&p, &list).unwrap();
        assert!(json_depth_over(&p, 128, Some(11)));
        // Twice a sample of five records ends before the deep item.
        assert!(!json_depth_over(&p, 128, Some(5)));
    }

    #[test]
    fn a_brace_inside_a_value_is_not_a_level() {
        // Without this rule, a file of text that holds braces would look
        // deep, and Peruse would refuse a file that is correct.
        let dir = std::env::temp_dir().join("peruse-depth-str");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let p = dir.join("braces.json");
        let body = format!("[{{\"a\": \"{}\"}}]", "{".repeat(500));
        std::fs::write(&p, body).unwrap();
        assert!(!json_depth_over(&p, 128, Some(JSON_SAMPLE_RECORDS)), "a brace in a value counted");

        // A quotation mark that a backslash holds does not end the value.
        let p = dir.join("escaped.json");
        std::fs::write(&p, format!("[{{\"a\": \"\\\"{}\"}}]", "[".repeat(500))).unwrap();
        assert!(!json_depth_over(&p, 128, Some(JSON_SAMPLE_RECORDS)), "an escaped mark ended the value");
    }

    #[test]
    fn a_deep_record_after_many_megabytes_is_still_seen() {
        // The reader of DuckDB examines a number of records, and not a number
        // of bytes. Before this test, the function read 8 MB and then stopped,
        // so a deep record behind 9 MB of shallow records, or behind one long
        // text, went to the reader and stopped the program.
        let dir = std::env::temp_dir().join("peruse-depth-far");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let deep = format!("{}1{}", "{\"a\":".repeat(1000), "}".repeat(1000));

        let line = format!("{{\"a\":\"{}\"}}\n", "p".repeat(9000));
        let p = dir.join("far.ndjson");
        std::fs::write(&p, format!("{}{deep}\n", line.repeat(1000))).unwrap();
        assert!(json_depth_over(&p, 128, Some(JSON_SAMPLE_RECORDS)), "a deep line after 9 MB");

        let p = dir.join("long.json");
        std::fs::write(&p, format!("[{{\"a\":\"{}\"}},{deep}]", "q".repeat(9 << 20))).unwrap();
        assert!(json_depth_over(&p, 128, Some(JSON_SAMPLE_RECORDS)), "a deep item after a long text");

        // A record that the reader does not examine needs no check. The reader
        // gives an error for it, and it does not crash.
        let p = dir.join("late.ndjson");
        std::fs::write(&p, format!("{}{deep}\n", "{\"a\":1}\n".repeat(100))).unwrap();
        assert!(!json_depth_over(&p, 128, Some(10)), "a record after twice the sample");
        assert!(json_depth_over(&p, 128, None), "the whole file must be read for no sample limit");
    }

    #[test]
    fn a_file_that_is_not_there_is_not_too_deep() {
        assert!(!json_depth_over(Path::new("/no/such/file.json"), 128, None));
    }

    #[test]
    fn a_path_for_the_settings_file_opens_again() {
        // On Windows, `canonicalize` gives a verbatim path. A change of the
        // separators inside one gives `//?/C:/...`, and Windows cannot open
        // that. The list of the files that the user opened was therefore
        // always empty on Windows.
        if cfg!(windows) {
            assert_eq!(tidy_path(Path::new(r"\\?\C:\data\a.csv")), "C:/data/a.csv");
            assert_eq!(tidy_path(Path::new(r"C:\data\a.csv")), "C:/data/a.csv");
            // A path on another machine keeps its two first separators.
            assert_eq!(
                tidy_path(Path::new(r"\\?\UNC\host\share\a.csv")),
                "//host/share/a.csv"
            );
        } else {
            // A backslash is an ordinary character of a name here, and the
            // function must not make one file into two.
            assert_eq!(
                tidy_path(Path::new(r"/data/back\slash.csv")),
                r"/data/back\slash.csv"
            );
        }
    }

    #[test]
    fn a_path_from_an_older_settings_file_still_opens() {
        // A version before this one wrote `//?/C:/...`. A user keeps the list
        // of files that they built.
        if cfg!(windows) {
            assert_eq!(
                untidy_path("//?/C:/data/a.csv"),
                PathBuf::from("C:/data/a.csv")
            );
            assert_eq!(untidy_path("C:/data/a.csv"), PathBuf::from("C:/data/a.csv"));
        }
        assert_eq!(untidy_path("/data/a.csv"), PathBuf::from("/data/a.csv"));
    }

    #[test]
    fn a_path_goes_out_and_comes_back_the_same() {
        for p in ["/data/a.csv", "C:/data/a.csv"] {
            let out = tidy_path(Path::new(p));
            assert_eq!(untidy_path(&out), PathBuf::from(p), "path {p}");
        }
    }

    #[test]
    fn globs_are_recognised() {
        assert!(looks_like_glob("data/*.parquet"));
        assert!(looks_like_glob("part-[0-9].csv"));
        assert!(!looks_like_glob("plain.csv"));
    }

    #[test]
    fn humanisers() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.00 KB");
        assert_eq!(human_bytes(1536), "1.50 KB");
        assert_eq!(human_count(12438201), "12,438,201");
        assert_eq!(human_count(0), "0");
        assert_eq!(human_count(999), "999");
    }
}
