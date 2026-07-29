//! The metadata of the file, for the metadata panel.
//!
//! For a Parquet file, each fact comes from the footer. The cost is one short
//! read for each file, and the size of the data does not change that cost.
//!
//! For a CSV file, the facts are the results of the DuckDB sniffer. The
//! sniffer finds the delimiter, the quote character and the date formats. A
//! wrong delimiter is the most frequent cause of a CSV file that looks wrong,
//! so the panel shows what the sniffer found.

use crate::source::{human_bytes, human_count, Format};

/// The facts from the footer of a Parquet file.
#[derive(Clone, Debug, Default)]
pub struct ParquetMeta {
    /// The name of the program that wrote the file.
    pub created_by: String,
    /// The version of the Parquet format.
    pub format_version: String,
    /// The number of rows in the file.
    pub num_rows: u64,
    /// The number of row groups in the file.
    pub num_row_groups: u64,
    /// The size of the data after compression, in bytes.
    pub total_compressed: u64,
    /// The size of the data before compression, in bytes.
    pub total_uncompressed: u64,
    /// The name of each codec, with the number of column chunks that use it.
    pub compression: Vec<(String, u64)>,
    /// The name of each encoding in the file.
    pub encodings: Vec<String>,
    /// The size of each row group before compression, in bytes.
    pub row_group_bytes: Vec<u64>,
    /// The key/value pairs in the footer. Examples are an Arrow schema and the
    /// index data of pandas.
    pub kv: Vec<(String, String)>,
}

impl ParquetMeta {
    /// Gives the size before compression divided by the size after
    /// compression. Gives `None` when the size after compression is 0.
    pub fn compression_ratio(&self) -> Option<f64> {
        if self.total_compressed == 0 {
            return None;
        }
        Some(self.total_uncompressed as f64 / self.total_compressed as f64)
    }

    /// Gives the mean number of rows in a row group. Gives 0 when the file has
    /// no row group.
    pub fn avg_row_group_rows(&self) -> u64 {
        self.num_rows.checked_div(self.num_row_groups).unwrap_or(0)
    }
}

/// The dialect of a CSV file, as the DuckDB sniffer reads it.
#[derive(Clone, Debug, Default)]
pub struct CsvMeta {
    /// The character between two values.
    pub delimiter: String,
    /// The character around a value that holds a delimiter.
    pub quote: String,
    /// The character that protects a quote character in a value.
    pub escape: String,
    /// The character or the characters at the end of a row.
    pub new_line: String,
    /// The number of rows before the first row of data.
    pub skip_rows: i64,
    /// `true` when the first row holds the column names.
    pub has_header: bool,
    /// The format of a date value.
    pub date_format: String,
    /// The format of a timestamp value.
    pub timestamp_format: String,
    /// The `read_csv(...)` call that reads the file with these settings.
    pub prompt: String,
}

/// The facts about one column, from the footer of a Parquet file.
///
/// DuckDB reads no data page for these facts. The cost is therefore small,
/// also on a file of some gigabytes.
#[derive(Clone, Debug, Default)]
pub struct ColumnFooterStats {
    /// The name of the column.
    pub name: String,
    /// The number of NULL values in the column.
    pub null_count: Option<u64>,
    /// The smallest value, if the footer holds it.
    pub min: Option<String>,
    /// The largest value, if the footer holds it.
    pub max: Option<String>,
    /// The size of the column after compression, in bytes.
    pub compressed: u64,
    /// The size of the column before compression, in bytes.
    pub uncompressed: u64,
    /// The names of the codecs of the column.
    pub compression: String,
    /// The names of the encodings of the column.
    pub encodings: String,
}

impl ColumnFooterStats {
    /// Gives the size before compression divided by the size after
    /// compression. Gives `None` when the size after compression is 0.
    pub fn ratio(&self) -> Option<f64> {
        if self.compressed == 0 {
            return None;
        }
        Some(self.uncompressed as f64 / self.compressed as f64)
    }
}

/// One file of the set, with its size.
#[derive(Clone, Debug)]
pub struct FileEntry {
    /// The path of the file.
    pub path: String,
    /// The size of the file on the disk, in bytes.
    pub bytes: u64,
}

/// The complete metadata of the file or the file set.
#[derive(Clone, Debug)]
pub struct FileMeta {
    /// The format of the data.
    pub format: Format,
    /// The files behind the view.
    pub files: Vec<FileEntry>,
    /// The sum of the sizes of the files, in bytes.
    pub bytes_on_disk: u64,
    /// The footer facts, for a Parquet file.
    pub parquet: Option<ParquetMeta>,
    /// The dialect, for a CSV file.
    pub csv: Option<CsvMeta>,
    /// The footer facts of each column, for a Parquet file.
    pub columns: Vec<ColumnFooterStats>,
}

impl FileMeta {
    /// Gives the metadata as rows of a label and a value.
    ///
    /// This function selects the rows for the format. The user interface
    /// therefore does not need to know which fields belong to which format.
    pub fn summary_rows(&self) -> Vec<(String, String)> {
        let mut r = vec![
            ("format".into(), self.format.to_string()),
            ("files".into(), human_count(self.files.len() as u64)),
            ("size on disk".into(), human_bytes(self.bytes_on_disk)),
        ];
        if let Some(p) = &self.parquet {
            r.push(("rows".into(), human_count(p.num_rows)));
            r.push(("row groups".into(), human_count(p.num_row_groups)));
            r.push((
                "avg rows/group".into(),
                human_count(p.avg_row_group_rows()),
            ));
            r.push(("uncompressed".into(), human_bytes(p.total_uncompressed)));
            if let Some(ratio) = p.compression_ratio() {
                r.push(("compression".into(), format!("{ratio:.2}×")));
            }
            let codecs: Vec<String> = p
                .compression
                .iter()
                .map(|(c, n)| format!("{c} ({n})"))
                .collect();
            if !codecs.is_empty() {
                r.push(("codecs".into(), codecs.join(", ")));
            }
            if !p.encodings.is_empty() {
                r.push(("encodings".into(), p.encodings.join(", ")));
            }
            if !p.format_version.is_empty() {
                // The footer holds an integer only, and "1" alone has no
                // meaning for the user. The letter v makes it a version.
                r.push(("version".into(), format!("v{}", p.format_version)));
            }
            if !p.created_by.is_empty() {
                r.push(("created by".into(), p.created_by.clone()));
            }
        }
        if let Some(c) = &self.csv {
            r.push(("delimiter".into(), describe_char(&c.delimiter)));
            r.push(("quote".into(), describe_char(&c.quote)));
            r.push(("escape".into(), describe_char(&c.escape)));
            r.push(("line ending".into(), describe_char(&c.new_line)));
            r.push(("header row".into(), if c.has_header { "yes" } else { "no" }.into()));
            if c.skip_rows > 0 {
                r.push(("skipped rows".into(), c.skip_rows.to_string()));
            }
            if !c.date_format.is_empty() {
                r.push(("date format".into(), c.date_format.clone()));
            }
            if !c.timestamp_format.is_empty() {
                r.push(("timestamp format".into(), c.timestamp_format.clone()));
            }
        }
        r
    }

    /// Gives the key/value pairs from the footer. Only a Parquet file has
    /// these pairs.
    pub fn kv_rows(&self) -> &[(String, String)] {
        self.parquet.as_ref().map(|p| p.kv.as_slice()).unwrap_or(&[])
    }
}

/// Gives the name of a character that the user cannot see.
///
/// A tab, a space and a line end are invisible in a panel. This function gives
/// each of them a name.
pub fn describe_char(s: &str) -> String {
    match s {
        "\t" | "\\t" => "\\t (tab)".into(),
        " " => "' ' (space)".into(),
        "\n" | "\\n" => "\\n (LF)".into(),
        "\r\n" | "\\r\\n" => "\\r\\n (CRLF)".into(),
        "\r" | "\\r" => "\\r (CR)".into(),
        "" => "(none)".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invisible_delimiters_get_named() {
        assert_eq!(describe_char("\t"), "\\t (tab)");
        assert_eq!(describe_char("\\t"), "\\t (tab)");
        assert_eq!(describe_char("\r\n"), "\\r\\n (CRLF)");
        assert_eq!(describe_char(""), "(none)");
        assert_eq!(describe_char(","), ",");
    }

    #[test]
    fn compression_ratio_guards_zero() {
        let mut p = ParquetMeta::default();
        assert_eq!(p.compression_ratio(), None);
        p.total_compressed = 100;
        p.total_uncompressed = 400;
        assert_eq!(p.compression_ratio(), Some(4.0));
    }

    #[test]
    fn avg_row_group_rows_guards_zero() {
        let mut p = ParquetMeta::default();
        assert_eq!(p.avg_row_group_rows(), 0);
        p.num_rows = 1000;
        p.num_row_groups = 4;
        assert_eq!(p.avg_row_group_rows(), 250);
    }

    #[test]
    fn summary_covers_parquet_and_csv_separately() {
        let base = FileMeta {
            format: Format::Parquet,
            files: vec![FileEntry { path: "a.parquet".into(), bytes: 100 }],
            bytes_on_disk: 100,
            parquet: Some(ParquetMeta {
                num_rows: 10,
                num_row_groups: 2,
                total_compressed: 100,
                total_uncompressed: 300,
                ..Default::default()
            }),
            csv: None,
            columns: vec![],
        };
        let rows = base.summary_rows();
        let keys: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"row groups"));
        assert!(keys.contains(&"compression"));
        assert!(!keys.contains(&"delimiter"), "no CSV fields for parquet");
    }
}
