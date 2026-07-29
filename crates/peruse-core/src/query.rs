//! Builds SQL from the state of the view.
//!
//! One [`View`] gives each part of what the grid shows:
//!
//! * a page of rows
//! * the row count
//! * the schema
//! * the statistics of a column
//!
//! Each of these statements comes from the same relation, the same filter and
//! the same sort. A filter from the prompt and a sort from the key `s`
//! therefore work together, and the code needs no special case for them.

use crate::model::{CellKind, Schema, MAX_CELL_CHARS};

/// Puts an identifier in double quotation marks, for a generated statement.
/// A double quotation mark in the name becomes two double quotation marks.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Puts a value in single quotation marks, for a generated statement. A single
/// quotation mark in the value becomes two single quotation marks.
pub fn quote_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The direction of a sort.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    /// From the smallest value to the largest value.
    Asc,
    /// From the largest value to the smallest value.
    Desc,
}

impl SortDir {
    /// Gives the character that the column header shows for this direction.
    pub fn arrow(self) -> char {
        match self {
            SortDir::Asc => '▲',
            SortDir::Desc => '▼',
        }
    }
    /// Gives the SQL word for this direction.
    fn sql(self) -> &'static str {
        match self {
            SortDir::Asc => "ASC",
            SortDir::Desc => "DESC",
        }
    }
}

/// One column of a sort, and the direction of that sort.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortKey {
    /// The name of the column to sort on.
    pub column: String,
    /// The direction of the sort.
    pub dir: SortDir,
}

/// The relation that the view reads from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Base {
    /// The open file or files. The engine gives them the name `src`.
    #[default]
    Source,
    /// A statement from the user. The module [`crate::sql_guard`] checks the
    /// statement before it comes here.
    Sql(String),
}

/// The complete description of what the grid shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// The relation to read from.
    pub base: Base,
    /// A `WHERE` expression from the filter prompt, with no `WHERE` word.
    pub filter: Option<String>,
    /// The sort columns, in order of precedence.
    pub sort: Vec<SortKey>,
}

/// The name that each generated statement gives to its one relation.
const ALIAS: &str = "q";

impl View {
    /// Gives `true` when the view reads the file, and not a statement from the
    /// user.
    pub fn is_source(&self) -> bool {
        matches!(self.base, Base::Source)
    }

    /// Gives `true` when the view reads the full file with no filter.
    ///
    /// A caller can then use a faster method that reads the metadata only. For
    /// example, DuckDB counts the rows of a Parquet file from the footer.
    pub fn is_unfiltered_source(&self) -> bool {
        self.is_source() && self.filter.is_none()
    }

    /// Gives the one relation that each generated statement reads from.
    fn relation(&self) -> String {
        match &self.base {
            Base::Source => format!("src AS {ALIAS}"),
            Base::Sql(sql) => format!("({}) AS {ALIAS}", sql.trim().trim_end_matches(';')),
        }
    }

    /// Gives `FROM <relation> WHERE (…)`, with no `ORDER BY` part.
    ///
    /// A caller that builds its own list of aggregates needs the same rows
    /// that the grid shows. The profile of a file for a `CREATE TABLE`
    /// statement is one such caller. The part `ORDER BY` is not here, because
    /// an aggregate does not need it and a sort of each row is slow.
    pub fn scan_from(&self) -> String {
        format!("FROM {}{}", self.relation(), self.where_clause())
    }

    /// Gives ` WHERE (…)`, or an empty text when the view has no filter.
    fn where_clause(&self) -> String {
        match &self.filter {
            Some(f) if !f.trim().is_empty() => format!(" WHERE ({})", f.trim()),
            _ => String::new(),
        }
    }

    /// Gives `ORDER BY …` with no space in front, or an empty text when the
    /// view has no sort.
    fn order_by(&self) -> String {
        if self.sort.is_empty() {
            return String::new();
        }
        let keys: Vec<String> = self
            .sort
            .iter()
            .map(|k| format!("{} {}", quote_ident(&k.column), k.dir.sql()))
            .collect();
        format!("ORDER BY {}", keys.join(", "))
    }

    /// Gives the same text as [`View::order_by`], with a space in front.
    fn order_clause(&self) -> String {
        match self.order_by().as_str() {
            "" => String::new(),
            s => format!(" {s}"),
        }
    }

    /// Builds `SELECT <projection> FROM … WHERE … ORDER BY …`.
    ///
    /// The statement has no `LIMIT` part and no `OFFSET` part.
    pub fn select(&self, projection: &str) -> String {
        format!(
            "SELECT {projection} FROM {}{}{}",
            self.relation(),
            self.where_clause(),
            self.order_clause()
        )
    }

    /// Builds the statement that reads one page of rows as text.
    /// The LIMIT part starts on a new line. A filter that ends with the comment
    /// marker `--` then hides only the remainder of its own line. The LIMIT
    /// part stays active, and the engine reads one page and not the full file.
    /// The function [`crate::sql_guard::ensure_safe_predicate`] refuses such a
    /// filter, and this new line is the second protection.
    pub fn page_sql(&self, schema: &Schema, limit: u32, offset: u64) -> String {
        let projection = display_projection(schema);
        format!("{}\nLIMIT {limit} OFFSET {offset}", self.select(&projection))
    }

    /// Builds the statement that counts the rows in the view.
    pub fn count_sql(&self) -> String {
        // The statement has no ORDER BY part. A sort cannot change a count,
        // but it makes the database sort all of the rows.
        format!(
            "SELECT count(*) FROM {}{}",
            self.relation(),
            self.where_clause()
        )
    }

    /// Builds the statement that gives the schema. It reads no rows.
    pub fn describe_sql(&self) -> String {
        format!("DESCRIBE SELECT * FROM {}", self.relation())
    }

    /// Builds the statement that reads one cell for the cell inspector.
    ///
    /// The statement gives the complete value. The statement for a page gives
    /// a short form of a long value.
    pub fn cell_sql(&self, column: &str, offset: u64) -> String {
        format!(
            "SELECT CAST({} AS VARCHAR) FROM {}{}{} LIMIT 1 OFFSET {offset}",
            quote_ident(column),
            self.relation(),
            self.where_clause(),
            self.order_clause()
        )
    }

    /// Builds the statement that gives the statistics of one column, over each
    /// row of the view.
    pub fn stats_sql(&self, column: &str, kind: CellKind) -> String {
        let c = quote_ident(column);
        let mut parts = vec![
            "count(*) AS n_total".to_string(),
            format!("count({c}) AS n_present"),
            format!("approx_count_distinct({c}) AS n_distinct"),
        ];
        // The minimum and the maximum have a meaning for each type that has
        // an order. The mean and the deviation do not. A call to avg() on a
        // VARCHAR column gives an error, and not a NULL.
        match kind {
            CellKind::Nested | CellKind::Binary => {
                parts.push("NULL AS v_min".into());
                parts.push("NULL AS v_max".into());
                parts.push("NULL AS v_avg".into());
                parts.push("NULL AS v_std".into());
            }
            CellKind::Number => {
                parts.push(format!("CAST(min({c}) AS VARCHAR) AS v_min"));
                parts.push(format!("CAST(max({c}) AS VARCHAR) AS v_max"));
                parts.push(format!("CAST(avg({c}) AS VARCHAR) AS v_avg"));
                parts.push(format!("CAST(stddev_samp({c}) AS VARCHAR) AS v_std"));
            }
            _ => {
                parts.push(format!("CAST(min({c}) AS VARCHAR) AS v_min"));
                parts.push(format!("CAST(max({c}) AS VARCHAR) AS v_max"));
                parts.push("NULL AS v_avg".into());
                parts.push("NULL AS v_std".into());
            }
        }
        format!(
            "SELECT {} FROM {}{}",
            parts.join(", "),
            self.relation(),
            self.where_clause()
        )
    }

    /// Builds the statement that gives the `k` most frequent values of one
    /// column, for the column inspector.
    pub fn top_values_sql(&self, column: &str, k: u32) -> String {
        let c = quote_ident(column);
        format!(
            "SELECT CAST({c} AS VARCHAR) AS v, count(*) AS n FROM {}{} GROUP BY 1 ORDER BY n DESC, v LIMIT {k}",
            self.relation(),
            self.where_clause()
        )
    }

    /// Builds the statement that gives the smallest value and the largest
    /// value of one column. The histogram uses them as its two edges.
    pub fn bounds_sql(&self, column: &str) -> String {
        let c = quote_ident(column);
        format!(
            "SELECT min({c})::DOUBLE, max({c})::DOUBLE FROM {}{}",
            self.relation(),
            self.where_clause()
        )
    }

    /// Builds the statement that counts the values in `bins` buckets of equal
    /// width, between the two edges `lo` and `hi`.
    ///
    /// The caller gives the two edges. A common table expression could
    /// calculate them, but then the statement would read the column two times
    /// and join it to itself. With the edges as numbers, one pass is enough.
    pub fn histogram_sql(&self, column: &str, lo: f64, hi: f64, bins: u32) -> String {
        debug_assert!(lo.is_finite() && hi.is_finite() && hi > lo && bins > 0);
        let c = quote_ident(column);
        let width = (hi - lo) / bins as f64;
        let wh = self.where_clause();
        let conj = if wh.is_empty() { " WHERE" } else { " AND" };
        format!(
            "SELECT least({bins} - 1, floor(({c}::DOUBLE - {lo:?}) / {width:?}))::BIGINT AS b, \
             count(*)::BIGINT AS n FROM {}{wh}{conj} {c} IS NOT NULL AND isfinite({c}::DOUBLE) \
             GROUP BY b ORDER BY b",
            self.relation()
        )
    }

    /// Builds the statement that finds the matches in one part of the view.
    ///
    /// The caller needs a row offset that it can move the viewport to. The row
    /// numbers must therefore agree with the numbers that [`View::page_sql`]
    /// gives. Only the SQL function `row_number()` can do this.
    ///
    /// A window over the full table would number each row before the database
    /// could report the first match. On ten million rows, the user would wait
    /// some seconds and see nothing.
    ///
    /// This statement numbers the rows of one part of the view instead. The
    /// cost of each call is proportional to `scan_rows`, and not to the size
    /// of the file. The caller moves away from the cursor one part at a time.
    /// A match near the cursor is the usual case, and it comes back
    /// immediately. The user can also stop a search that finds nothing.
    pub fn search_sql(
        &self,
        schema: &Schema,
        needle: &str,
        from_row: u64,
        scan_rows: u64,
        limit: u32,
    ) -> String {
        let escaped = needle
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pat = quote_str(&format!("%{escaped}%"));
        let any: Vec<String> = schema
            .columns
            .iter()
            .map(|c| {
                let id = quote_ident(&c.name);
                match c.kind {
                    // The column is text already. A cast would copy each
                    // value and give no gain.
                    CellKind::Text => format!("{id} ILIKE {pat} ESCAPE '\\'"),
                    // The grid shows the size of a BLOB value, and not its
                    // bytes. A match on those bytes would confuse the user.
                    CellKind::Binary => String::new(),
                    _ => format!("CAST({id} AS VARCHAR) ILIKE {pat} ESCAPE '\\'"),
                }
            })
            .filter(|s| !s.is_empty())
            .collect();
        if any.is_empty() {
            return String::new();
        }
        format!(
            "SELECT {from_row} + off AS off FROM (\
               SELECT row_number() OVER () - 1 AS off, * FROM ({} LIMIT {scan_rows} OFFSET {from_row})\
             ) WHERE {} ORDER BY off LIMIT {limit}",
            self.select("*"),
            any.join(" OR ")
        )
    }
}

/// Builds the `SELECT` list that the grid reads.
///
/// The statement casts each value to VARCHAR in the database. Rust code could
/// format the values instead, but then Peruse would need code for each type.
/// With the cast, a decimal, an interval, an enumeration and a nested
/// structure all look the same as they look in DuckDB.
///
/// The function `substr` limits the length of each value. A wide column of
/// text therefore costs the same as a narrow one.
pub fn display_projection(schema: &Schema) -> String {
    if schema.is_empty() {
        return "*".to_string();
    }
    schema
        .columns
        .iter()
        .map(|c| {
            let id = quote_ident(&c.name);
            match c.kind {
                // Do not move the bytes of a BLOB value to the grid. The
                // grid shows the size of the value only.
                CellKind::Binary => format!("('blob ' || octet_length({id}) || ' B')"),
                _ => format!("substr(CAST({id} AS VARCHAR), 1, {MAX_CELL_CHARS})"),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Column;

    fn schema() -> Schema {
        Schema {
            columns: vec![
                Column::new("id", "BIGINT", false),
                Column::new("name", "VARCHAR", true),
                Column::new("payload", "BLOB", true),
            ],
        }
    }

    #[test]
    fn identifiers_and_literals_are_escaped() {
        assert_eq!(quote_ident("plain"), "\"plain\"");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(quote_str("o'brien"), "'o''brien'");
    }

    #[test]
    fn bare_source_view() {
        let v = View::default();
        assert_eq!(v.select("*"), "SELECT * FROM src AS q");
        assert_eq!(v.count_sql(), "SELECT count(*) FROM src AS q");
        assert!(v.is_unfiltered_source());
    }

    #[test]
    fn filter_and_sort_compose() {
        let v = View {
            base: Base::Source,
            filter: Some("amount > 100".into()),
            sort: vec![
                SortKey { column: "amount".into(), dir: SortDir::Desc },
                SortKey { column: "id".into(), dir: SortDir::Asc },
            ],
        };
        assert_eq!(
            v.select("*"),
            "SELECT * FROM src AS q WHERE (amount > 100) ORDER BY \"amount\" DESC, \"id\" ASC"
        );
        assert!(!v.is_unfiltered_source());
    }

    #[test]
    fn count_ignores_ordering() {
        let v = View {
            base: Base::Source,
            filter: Some("a = 1".into()),
            sort: vec![SortKey { column: "a".into(), dir: SortDir::Asc }],
        };
        assert_eq!(v.count_sql(), "SELECT count(*) FROM src AS q WHERE (a = 1)");
    }

    #[test]
    fn sql_base_is_wrapped_and_desemicoloned() {
        let v = View {
            base: Base::Sql("SELECT 1 AS x;".into()),
            ..Default::default()
        };
        assert_eq!(v.select("*"), "SELECT * FROM (SELECT 1 AS x) AS q");
        assert!(!v.is_source());
    }

    #[test]
    fn projection_truncates_text_and_summarises_blobs() {
        let p = display_projection(&schema());
        assert!(p.contains("substr(CAST(\"name\" AS VARCHAR), 1, 4096)"));
        assert!(p.contains("octet_length(\"payload\")"));
        assert!(!p.contains("CAST(\"payload\" AS VARCHAR)"), "blobs are never cast");
    }

    #[test]
    fn empty_schema_projects_star() {
        assert_eq!(display_projection(&Schema::default()), "*");
    }

    #[test]
    fn paging_appends_limit_offset() {
        let sql = View::default().page_sql(&schema(), 50, 1000);
        assert!(sql.ends_with("\nLIMIT 50 OFFSET 1000"), "got {sql}");
    }

    #[test]
    fn a_comment_in_a_filter_cannot_hide_the_limit_part() {
        // The guard refuses this filter. If a different gap in the guard lets
        // a comment marker through, the new line keeps the LIMIT part active.
        let v = View {
            filter: Some("1 --".into()),
            ..Default::default()
        };
        let sql = v.page_sql(&schema(), 50, 0);
        let last = sql.lines().last().unwrap();
        assert_eq!(last, "LIMIT 50 OFFSET 0");
        assert!(!last.contains("--"));
    }

    #[test]
    fn stats_do_not_average_strings() {
        let v = View::default();
        let numeric = v.stats_sql("id", CellKind::Number);
        assert!(numeric.contains("avg(\"id\")"));
        let text = v.stats_sql("name", CellKind::Text);
        assert!(text.contains("NULL AS v_avg"), "no avg() over VARCHAR");
        assert!(text.contains("min(\"name\")"), "but min/max still apply");
        let blob = v.stats_sql("payload", CellKind::Binary);
        assert!(blob.contains("NULL AS v_min"));
    }

    #[test]
    fn search_escapes_wildcards_in_the_needle() {
        let sql = View::default().search_sql(&schema(), "50%_off", 0, 1000, 10);
        assert!(sql.contains("'%50\\%\\_off%'"), "got {sql}");
        assert!(sql.contains("ESCAPE"));
    }

    #[test]
    fn search_bounds_its_scan_and_rebases_offsets() {
        let sql = View::default().search_sql(&schema(), "x", 5000, 250, 10);
        assert!(sql.contains("LIMIT 250 OFFSET 5000"), "scan not bounded: {sql}");
        assert!(sql.contains("5000 + off"), "offsets not rebased: {sql}");
    }

    #[test]
    fn search_does_not_cast_text_or_probe_blobs() {
        let sql = View::default().search_sql(&schema(), "x", 0, 100, 10);
        assert!(sql.contains("\"name\" ILIKE"), "VARCHAR should not be cast: {sql}");
        assert!(sql.contains("CAST(\"id\" AS VARCHAR) ILIKE"), "BIGINT needs a cast: {sql}");
        assert!(!sql.contains("\"payload\""), "blobs are not searched: {sql}");
    }

    #[test]
    fn search_over_a_sorted_view_numbers_rows_in_that_order() {
        let v = View {
            sort: vec![SortKey { column: "id".into(), dir: SortDir::Desc }],
            ..Default::default()
        };
        let sql = v.search_sql(&schema(), "x", 0, 100, 10);
        assert!(
            sql.contains("ORDER BY \"id\" DESC LIMIT 100 OFFSET 0"),
            "slice must be taken from the sorted view: {sql}"
        );
    }

    #[test]
    fn search_with_no_searchable_columns_is_empty() {
        let blobs = Schema {
            columns: vec![Column::new("payload", "BLOB", true)],
        };
        assert_eq!(View::default().search_sql(&blobs, "x", 0, 100, 10), "");
    }
}
