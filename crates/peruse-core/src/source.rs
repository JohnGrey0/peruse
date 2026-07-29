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
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Format::Parquet => "parquet",
            Format::Csv => "csv",
            Format::Json => "json",
            Format::Arrow => "arrow",
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
}

impl Source {
    /// Gives `true` when the source holds more than one file.
    pub fn is_multi(&self) -> bool {
        self.files.len() > 1
    }

    /// Gives the text for the title bar: the name of the file, or a name and
    /// the number of the other files, such as `name.parquet +3`.
    pub fn title(&self) -> String {
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

/// Reads the first bytes of the file and finds the format from them.
///
/// A Parquet file starts with `PAR1`, and an Arrow IPC file starts with
/// `ARROW1`. A JSON file starts with `{` or `[`, after any spaces. These marks
/// are sufficient to tell such a file from a text file with rows.
fn sniff_magic(path: &Path) -> Option<Format> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 16];
    // Read what the file holds. A file of eight bytes is short, but it is
    // still a file, and `read_exact` would give an error for it.
    let n = f.read(&mut buf).ok()?;
    let head = &buf[..n];
    if head.starts_with(b"PAR1") {
        return Some(Format::Parquet);
    }
    if head.starts_with(b"ARROW1") {
        return Some(Format::Arrow);
    }
    match head.iter().find(|b| !b.is_ascii_whitespace()) {
        Some(b'{') | Some(b'[') => Some(Format::Json),
        _ => None,
    }
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
        Some("csv") => Some((Format::Csv, Some(','))),
        Some("psv") => Some((Format::Csv, Some('|'))),
        Some("json" | "ndjson" | "jsonl") => Some((Format::Json, None)),
        Some("arrow" | "ipc" | "feather" | "arrows") => Some((Format::Arrow, None)),
        _ => None,
    }
}

/// Finds the format of a file, the delimiter and the compression.
///
/// The function uses three tests, in this order:
///
/// 1. The extension of the file.
/// 2. The first four bytes of the file.
/// 3. CSV, as the last choice.
///
/// A file `data.dat` that holds values with commas therefore opens correctly.
pub fn detect(path: &Path) -> (Format, Option<char>, bool) {
    let (_, compressed) = effective_extension(path);
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

/// Writes a path in a form that a settings file can hold, and that the system
/// can open again.
///
/// On Windows, `std::fs::canonicalize` gives a verbatim path, which starts
/// with `\\?\`. Windows gives such a path to its own calls with no change, and
/// it does not read a forward slash inside one. A change of the separators
/// would therefore give `//?/C:/…`, and that path never opens again.
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
        assert_eq!(detect(Path::new("a.csv")).1, Some(','));
        assert_eq!(detect(Path::new("a.tsv")).1, Some('\t'));
        assert_eq!(detect(Path::new("a.psv")).1, Some('|'));
        assert_eq!(detect(Path::new("a.json")).0, Format::Json);
        assert_eq!(detect(Path::new("a.ndjson")).0, Format::Json);
        assert_eq!(detect(Path::new("a.jsonl")).0, Format::Json);
        assert_eq!(detect(Path::new("a.arrow")).0, Format::Arrow);
        assert_eq!(detect(Path::new("a.feather")).0, Format::Arrow);
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
    fn a_path_for_the_settings_file_opens_again() {
        // On Windows, `canonicalize` gives a verbatim path. A change of the
        // separators inside one gives `//?/C:/…`, and Windows cannot open
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
        // A version before this one wrote `//?/C:/…`. A user keeps the list
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
