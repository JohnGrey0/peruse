//! The statistics of one column, for the column inspector panel. Peruse
//! calculates them only when the user opens the panel.

use crate::model::CellKind;
use crate::source::human_count;

/// The counts of the values of a column, in buckets of equal width.
#[derive(Clone, Debug, Default)]
pub struct Histogram {
    /// The smallest value in the column.
    pub lo: f64,
    /// The largest value in the column.
    pub hi: f64,
    /// The count in each bucket, from the smallest bucket to the largest one.
    pub buckets: Vec<u64>,
}

impl Histogram {
    /// Gives the largest count of the buckets.
    pub fn max(&self) -> u64 {
        self.buckets.iter().copied().max().unwrap_or(0)
    }

    /// Draws the histogram as one Unicode block character for each bucket.
    ///
    /// The terminal user interface shows this text today. A graphical user
    /// interface can show the same text as a small chart.
    pub fn sparkline(&self) -> String {
        const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        let max = self.max();
        if max == 0 {
            return String::new();
        }
        self.buckets
            .iter()
            .map(|&n| {
                if n == 0 {
                    ' '
                } else {
                    // A bucket with a count gets the smallest block at the
                    // minimum. A value that occurs one time is then visible,
                    // and it does not look like an empty bucket.
                    let idx = ((n as f64 / max as f64) * 7.0).ceil() as usize;
                    BLOCKS[idx.min(7)]
                }
            })
            .collect()
    }
}

/// The statistics of one column.
#[derive(Clone, Debug, Default)]
pub struct ColumnStats {
    /// The name of the column.
    pub column: String,
    /// The SQL type of the column.
    pub sql_type: String,
    /// The family of values of the column.
    pub kind: Option<CellKindWrapper>,
    /// The number of rows in the view.
    pub n_total: u64,
    /// The number of rows where the value is not NULL.
    pub n_present: u64,
    /// The estimated number of different values.
    pub n_distinct: u64,
    /// The smallest value, as text.
    pub min: Option<String>,
    /// The largest value, as text.
    pub max: Option<String>,
    /// The mean value, as text. Only a column of numbers has one.
    pub avg: Option<String>,
    /// The standard deviation, as text. Only a column of numbers has one.
    pub std: Option<String>,
    /// The most frequent values, with their counts. `None` is the group of the
    /// NULL values.
    pub top: Vec<(Option<String>, u64)>,
    /// The histogram. Only a column of numbers has one.
    pub histogram: Option<Histogram>,
}

/// Holds a [`CellKind`] value inside [`ColumnStats`].
///
/// `CellKind` is `Copy`, but it has no `Default`. This wrapper lets Rust derive
/// `ColumnStats::default()`, and Peruse does not need a default family of
/// values with no meaning.
#[derive(Clone, Copy, Debug)]
pub struct CellKindWrapper(pub CellKind);

impl ColumnStats {
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

    /// Gives the number of different values divided by the number of values
    /// that are not NULL. The value 1.0 shows that each value is different.
    pub fn cardinality_ratio(&self) -> f64 {
        if self.n_present == 0 {
            0.0
        } else {
            (self.n_distinct as f64 / self.n_present as f64).min(1.0)
        }
    }

    /// Gives a short text about the character of the column. The panel shows
    /// this text with the count of the different values.
    pub fn shape_hint(&self) -> &'static str {
        if self.n_present == 0 {
            return "all null";
        }
        if self.n_distinct <= 1 {
            return "constant";
        }
        let r = self.cardinality_ratio();
        if r > 0.99 {
            "unique — key-like"
        } else if self.n_distinct <= 32 {
            "low cardinality — categorical"
        } else if r < 0.05 {
            "repeating"
        } else {
            "mixed"
        }
    }

    /// Gives the statistics as rows of a label and a value.
    ///
    /// This crate builds the rows one time, so each front-end shows the same
    /// information.
    pub fn rows(&self) -> Vec<(String, String)> {
        let mut r = vec![
            ("type".into(), self.sql_type.clone()),
            ("count".into(), human_count(self.n_total)),
            (
                "nulls".into(),
                format!("{} ({:.1}%)", human_count(self.null_count()), self.null_pct()),
            ),
            (
                "distinct".into(),
                format!("~{} ({})", human_count(self.n_distinct), self.shape_hint()),
            ),
        ];
        if let Some(v) = &self.min {
            r.push(("min".into(), v.clone()));
        }
        if let Some(v) = &self.max {
            r.push(("max".into(), v.clone()));
        }
        if let Some(v) = &self.avg {
            r.push(("mean".into(), trim_float(v)));
        }
        if let Some(v) = &self.std {
            r.push(("stddev".into(), trim_float(v)));
        }
        r
    }
}

/// Cuts a floating-point value to six decimal places, and removes the zeros at
/// the end.
///
/// DuckDB writes a floating-point value with each decimal place. A panel with
/// a summary does not need more than six of them.
fn trim_float(s: &str) -> String {
    match s.parse::<f64>() {
        Ok(v) if v.fract() != 0.0 => format!("{v:.6}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string(),
        _ => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(total: u64, present: u64, distinct: u64) -> ColumnStats {
        ColumnStats {
            n_total: total,
            n_present: present,
            n_distinct: distinct,
            ..Default::default()
        }
    }

    #[test]
    fn null_arithmetic() {
        let s = stats(1000, 750, 50);
        assert_eq!(s.null_count(), 250);
        assert!((s.null_pct() - 25.0).abs() < 1e-9);
    }

    #[test]
    fn empty_column_does_not_divide_by_zero() {
        let s = stats(0, 0, 0);
        assert_eq!(s.null_pct(), 0.0);
        assert_eq!(s.cardinality_ratio(), 0.0);
        assert_eq!(s.shape_hint(), "all null");
    }

    #[test]
    fn approx_distinct_can_exceed_present_without_breaking_ratio() {
        // The function approx_count_distinct gives an estimate, and the
        // estimate can be a little too large.
        let s = stats(100, 100, 104);
        assert_eq!(s.cardinality_ratio(), 1.0);
    }

    #[test]
    fn shape_hints() {
        assert_eq!(stats(100, 100, 1).shape_hint(), "constant");
        assert_eq!(stats(100, 100, 100).shape_hint(), "unique — key-like");
        assert_eq!(stats(1000, 1000, 5).shape_hint(), "low cardinality — categorical");
        assert_eq!(stats(10000, 10000, 200).shape_hint(), "repeating");
    }

    #[test]
    fn sparkline_marks_every_non_empty_bucket() {
        let h = Histogram {
            lo: 0.0,
            hi: 10.0,
            buckets: vec![0, 1, 50, 100],
        };
        let s: Vec<char> = h.sparkline().chars().collect();
        assert_eq!(s[0], ' ', "empty bucket is blank");
        assert_ne!(s[1], ' ', "a single row still shows");
        assert_eq!(s[3], '█', "the max bucket is full height");
        assert_eq!(h.max(), 100);
    }

    #[test]
    fn empty_histogram_is_blank() {
        assert_eq!(Histogram::default().sparkline(), "");
    }

    #[test]
    fn floats_are_trimmed_but_integers_are_not() {
        assert_eq!(trim_float("3.14159265358979"), "3.141593");
        assert_eq!(trim_float("42"), "42");
        assert_eq!(trim_float("2.5"), "2.5");
        assert_eq!(trim_float("not a number"), "not a number");
    }
}
