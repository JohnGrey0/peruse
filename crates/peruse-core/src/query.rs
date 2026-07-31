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

use crate::model::{CellKind, Column, Schema, MAX_CELL_CHARS};

/// The largest number of characters that the detail band keeps for the smallest
/// value and the largest value of a column.
///
/// The band draws these two values inside the width of a column of the grid,
/// and no column is wider than 60 screen columns. A cut at 64 characters
/// therefore loses nothing that the user can see, and one value of a megabyte
/// cannot arrive in full. The projection of the grid follows the same rule with
/// a larger limit. Refer to [`display_projection`].
pub const BAND_VALUE_CHARS: usize = 64;

/// Puts an identifier in double quotation marks, for a generated statement.
/// A double quotation mark in the name becomes two double quotation marks.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// One step in a path to a value inside a nested column.
///
/// The record view drills into a structure and into a list. A path such as
/// `payload.commits[0].sha` is a list of these steps. Peruse needs the path in
/// two forms: the text that it shows to the user, and the SQL that reads that
/// value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// A field of a structure, by its name.
    Field(String),
    /// An item of a list, by its position from 0.
    Index(usize),
}

/// Writes a path in the form that a person reads, such as
/// `payload.commits[0].sha`.
pub fn path_text(steps: &[Step]) -> String {
    let mut out = String::new();
    for s in steps {
        match s {
            Step::Field(n) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(n);
            }
            Step::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

/// Writes a path in the form that a statement needs, such as
/// `"payload"."commits"[1]."sha"`.
///
/// Each name goes in quotation marks, so a field with a full stop in its name
/// still works. DuckDB counts the items of a list from 1. Peruse counts each
/// position from 0, as it does everywhere else, so this function adds the one.
pub fn quote_path(steps: &[Step]) -> String {
    let mut out = String::new();
    for s in steps {
        match s {
            Step::Field(n) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(&quote_ident(n));
            }
            Step::Index(i) => out.push_str(&format!("[{}]", i + 1)),
        }
    }
    out
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

/// The statement that the SQL prompt starts from, over the open file.
///
/// The prompt opens with this text, and the cursor comes after the last
/// character. The user of a viewer asks for a part of the file more frequently
/// than for anything else, so the text ends with the word `WHERE` and a space.
/// The key `^U` removes the whole line for a user who wants another statement.
///
/// The keywords are in upper case, as in each statement that Peruse builds, and
/// the prompt gives a color to a keyword.
///
/// The text lives here, beside the other SQL, because it holds the name `src`.
/// The engine gives that name to the open file, and the name must have one home.
pub const PROMPT_START: &str = "SELECT * FROM src WHERE ";

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

    /// Gives `FROM <relation> WHERE (...)`, with no `ORDER BY` part.
    ///
    /// A caller that builds its own list of aggregates needs the same rows
    /// that the grid shows. The profile of a file for a `CREATE TABLE`
    /// statement is one such caller. The part `ORDER BY` is not here, because
    /// an aggregate does not need it and a sort of each row is slow.
    pub fn scan_from(&self) -> String {
        format!("FROM {}{}", self.relation(), self.where_clause())
    }

    /// Gives ` WHERE (...)`, or an empty text when the view has no filter.
    fn where_clause(&self) -> String {
        match &self.filter {
            Some(f) if !f.trim().is_empty() => format!(" WHERE ({})", f.trim()),
            _ => String::new(),
        }
    }

    /// Gives `ORDER BY ...` with no space in front, or an empty text when the
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

    /// Builds `SELECT <projection> FROM ... WHERE ... ORDER BY ...`.
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
    ///
    /// The LIMIT part comes inside, and the projection comes outside. The
    /// database therefore takes the page first, and it changes the values of
    /// those rows only. With the projection beside the LIMIT part, the database
    /// changes the values of a whole block of rows, and it shares that work
    /// across each of its threads. One page of 50 rows then costs 8.3
    /// milliseconds. With the projection outside, the same page costs 5.3. A
    /// sorted page costs 12.4 and 8.5.
    ///
    /// The order of the rows comes from the LIMIT part inside. The setting
    /// `preserve_insertion_order` is true, so the projection outside keeps that
    /// order. The statement for the record view depends on the same rule.
    ///
    /// The LIMIT part starts on a new line. A filter that ends with the comment
    /// marker `--` then hides only the remainder of its own line. The LIMIT
    /// part stays active, and the engine reads one page and not the full file.
    /// The function [`crate::sql_guard::ensure_safe_predicate`] refuses such a
    /// filter, and this new line is the second protection.
    pub fn page_sql(&self, schema: &Schema, limit: u32, offset: u64) -> String {
        format!(
            "SELECT {} FROM ({}\nLIMIT {limit} OFFSET {offset}) AS page",
            display_projection(schema),
            self.select("*")
        )
    }

    /// Builds the statement that reads one complete row as JSON.
    ///
    /// The record view drills into a structure and into a list. The text of a
    /// structure that DuckDB writes, such as `{'id': 1, 'login': a}`, cannot
    /// carry that job: it has no reliable rule for a value that holds a
    /// quotation mark, and Peruse would have to write a parser for it. JSON
    /// has one rule, and the database writes it.
    ///
    /// One row of the usual file is some hundred bytes, so one statement for
    /// the complete row costs less than one statement for each field.
    ///
    /// Two families of values do not go into the JSON as they are:
    ///
    /// * A BLOB becomes its size, as it does in the grid. A value of 10 MB
    ///   must not move to the user interface.
    /// * A long text stops at [`MAX_CELL_CHARS`], as it does in the grid.
    pub fn row_json_sql(&self, schema: &Schema, offset: u64) -> String {
        if schema.is_empty() {
            return "SELECT NULL WHERE false".to_string();
        }
        let fields: Vec<String> = schema
            .columns
            .iter()
            .map(|c| {
                let id = quote_ident(&c.name);
                let value = match c.kind {
                    CellKind::Binary => format!("('blob ' || octet_length({id}) || ' B')"),
                    CellKind::Text => format!("substr({id}, 1, {MAX_CELL_CHARS})"),
                    _ => id,
                };
                format!("{}: {value}", quote_str(&c.name))
            })
            .collect();
        format!(
            "SELECT CAST(to_json({{{}}}) AS VARCHAR) FROM ({}) AS t\nLIMIT 1 OFFSET {offset}",
            fields.join(", "),
            self.select("*")
        )
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
    ///
    /// A column of numbers gets two more values at the end: the smallest value
    /// and the largest value as a DOUBLE. The histogram needs those two edges.
    /// They come from this statement, which reads the column one time already.
    /// A separate statement for the edges would read the column a second time.
    /// Refer to [`View::histogram_sql`].
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
                parts.push(format!("min({c})::DOUBLE AS lo"));
                parts.push(format!("max({c})::DOUBLE AS hi"));
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

    /// Builds the statement that measures each column of the detail band, over
    /// each row of the view.
    ///
    /// One statement measures every column. The band draws the columns that fit
    /// the width of the terminal, so the list is short, and the database reads
    /// the view one time for all of them. One statement for each column would
    /// read the view again for each column.
    ///
    /// The statement gives the count of the rows one time, and then the count of
    /// the values that are not NULL for each column.
    ///
    /// With `values`, each column also gives the count of the different values,
    /// the smallest value and the largest value, in that order. The detailed band
    /// draws those three, and the compact band does not.
    /// [`crate::Engine::column_band`] reads the values by position, so the two
    /// functions must agree about the number of values for each column.
    ///
    /// The count of the different values is an estimate.
    /// `approx_count_distinct` reads the column one time and holds little
    /// memory. An exact count of a large column needs much more of both, and
    /// the band has room for a number of two or three digits only.
    ///
    /// The two edges of the range go through `substr`, as
    /// [`display_projection`] does. One huge value can therefore not arrive in
    /// full. Refer to [`BAND_VALUE_CHARS`].
    ///
    /// The statement has no `ORDER BY` part. An aggregate does not need one, and
    /// a sort of every row is slow.
    pub fn band_sql(&self, columns: &[Column], values: bool) -> String {
        if columns.is_empty() {
            return String::new();
        }
        let mut parts = vec!["count(*) AS n_total".to_string()];
        for (i, c) in columns.iter().enumerate() {
            let id = quote_ident(&c.name);
            parts.push(format!("count({id}) AS n_present_{i}"));
            // The compact band draws the share of NULL values and nothing else,
            // so it needs the two counts above and no more. The three parts below
            // each read the whole column: on a CSV file of 2.7 GB they cost 795
            // ms against 99 ms for the counts alone. A band that draws two facts
            // must not pay for five.
            if !values {
                continue;
            }
            parts.push(format!("approx_count_distinct({id}) AS n_distinct_{i}"));
            // The smallest value and the largest value have a meaning for a
            // type that has an order. A structure, a list and a BLOB have no
            // order, and min() over one of them gives an error and not a NULL.
            match c.kind {
                CellKind::Nested | CellKind::Binary => {
                    parts.push(format!("NULL AS v_min_{i}"));
                    parts.push(format!("NULL AS v_max_{i}"));
                }
                _ => {
                    parts.push(format!(
                        "substr(CAST(min({id}) AS VARCHAR), 1, {BAND_VALUE_CHARS}) AS v_min_{i}"
                    ));
                    parts.push(format!(
                        "substr(CAST(max({id}) AS VARCHAR), 1, {BAND_VALUE_CHARS}) AS v_max_{i}"
                    ));
                }
            }
        }
        format!("SELECT {} {}", parts.join(", "), self.scan_from())
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
    /// value of one column, as a DOUBLE.
    ///
    /// The column inspector does not use this statement. It takes the two
    /// edges from [`View::stats_sql`], which reads the column one time for the
    /// statistics and the edges together. This statement stays for a caller
    /// that wants the two edges alone.
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
    ///
    /// The test on each column is `contains(lower(value), lower(needle))`. The
    /// function `contains` looks for a plain part of a text. `ILIKE` reads a
    /// pattern first, and it costs about three times as much for the same
    /// answer. Over a scan of 250,000 rows and nine columns, `ILIKE` needs 265
    /// milliseconds and `contains` needs 91.
    ///
    /// The database puts the needle in lower case one time, because the needle
    /// is a constant. Both sides of the test therefore follow the same rule for
    /// a letter, also for a letter that English does not use.
    ///
    /// The needle needs no escape character. A search for `50%` looks for the
    /// three characters, and `%` is not a wildcard here.
    pub fn search_sql(
        &self,
        schema: &Schema,
        needle: &str,
        from_row: u64,
        scan_rows: u64,
        limit: u32,
    ) -> String {
        let low = format!("lower({})", quote_str(needle));
        let any: Vec<String> = schema
            .columns
            .iter()
            .map(|c| {
                let id = quote_ident(&c.name);
                match c.kind {
                    // The column is text already. A cast would copy each
                    // value and give no gain.
                    CellKind::Text => format!("contains(lower({id}), {low})"),
                    // The grid shows the size of a BLOB value, and not its
                    // bytes. A match on those bytes would confuse the user.
                    CellKind::Binary => String::new(),
                    _ => format!("contains(lower(CAST({id} AS VARCHAR)), {low})"),
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
    fn the_statement_that_the_prompt_starts_from_reads_and_never_writes() {
        // The prompt puts this text in the line and checks the line at each
        // key. The guard must therefore accept the text while the statement is
        // still incomplete. Without that, the prompt would report an error
        // before the user types one character.
        assert!(crate::sql_guard::ensure_read_only(PROMPT_START).is_ok());
        // The keywords are in upper case, as in each statement that Peruse
        // builds, and the prompt gives a color to a keyword.
        assert!(PROMPT_START.starts_with("SELECT "), "got {PROMPT_START:?}");
        assert!(PROMPT_START.contains(" WHERE "), "got {PROMPT_START:?}");
        // The text ends after the space, so the user types the condition alone.
        assert!(PROMPT_START.ends_with(' '), "got {PROMPT_START:?}");
        // The name of the relation is the name that each generated statement
        // reads from. The user interface holds no second copy of it.
        let relation = View::default().relation();
        let name = relation.split(" AS ").next().unwrap();
        assert!(
            PROMPT_START.contains(&format!("FROM {name} ")),
            "the prompt names another relation than {name:?}: {PROMPT_START:?}"
        );
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
    fn paging_takes_the_page_before_it_changes_the_values() {
        // The LIMIT part comes inside, and the projection comes outside. The
        // database therefore changes the values of the page only.
        let sql = View::default().page_sql(&schema(), 50, 1000);
        assert!(sql.ends_with("\nLIMIT 50 OFFSET 1000) AS page"), "got {sql}");
        let (outer, inner) = sql.split_once(" FROM (").expect("no inner statement");
        assert!(outer.contains("substr(CAST(\"name\""), "no projection outside: {sql}");
        assert!(!inner.contains("substr("), "the projection is inside: {sql}");
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
        assert!(last.starts_with("LIMIT 50 OFFSET 0"), "got {last}");
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
    fn stats_carry_the_histogram_edges_for_a_column_of_numbers() {
        // The histogram needs the smallest and the largest value as a DOUBLE.
        // They come from this statement, so the column is read one time only.
        let v = View::default();
        let numeric = v.stats_sql("id", CellKind::Number);
        assert!(numeric.contains("min(\"id\")::DOUBLE AS lo"), "got {numeric}");
        assert!(numeric.contains("max(\"id\")::DOUBLE AS hi"), "got {numeric}");
        // No other family gets a histogram, so no other family needs the edges.
        for kind in [CellKind::Text, CellKind::Bool, CellKind::Temporal, CellKind::Binary] {
            let sql = v.stats_sql("c", kind);
            assert!(!sql.contains("AS lo"), "{kind:?} does not need the edges: {sql}");
        }
    }

    #[test]
    fn the_band_measures_every_column_in_one_statement() {
        // The band draws the columns that fit the terminal. One statement for
        // each column would read the view again for each column.
        let s = schema();
        let sql = View::default().band_sql(&s.columns, true);
        assert_eq!(sql.matches(" FROM ").count(), 1, "more than one scan: {sql}");
        assert!(sql.starts_with("SELECT count(*) AS n_total,"), "got {sql}");
        for i in 0..3 {
            assert!(sql.contains(&format!("AS n_present_{i}")), "column {i}: {sql}");
            assert!(sql.contains(&format!("AS n_distinct_{i}")), "column {i}: {sql}");
        }
        // The count of the different values is an estimate, as it is in the
        // statement of the column inspector.
        assert!(sql.contains("approx_count_distinct(\"id\")"), "got {sql}");
        // A long value is cut in the database, so it cannot arrive in full.
        assert!(
            sql.contains(&format!("substr(CAST(min(\"name\") AS VARCHAR), 1, {BAND_VALUE_CHARS})")),
            "got {sql}"
        );
        // A BLOB column has no order, so it has no smallest value.
        assert!(sql.contains("NULL AS v_min_2"), "got {sql}");
        assert!(!sql.contains("min(\"payload\")"), "no min() over a BLOB: {sql}");
        // A sort cannot change an aggregate, and a sort of every row is slow.
        assert!(!sql.contains("ORDER BY"), "got {sql}");
    }

    #[test]
    fn the_compact_band_asks_for_the_two_counts_and_nothing_more() {
        // The compact band draws the share of NULL values alone. Each of the
        // three other facts reads the whole column: on a CSV file of 2.7 GB the
        // full statement costs 795 ms and the two counts cost 99 ms. A band that
        // draws two facts must not pay for five.
        let s = schema();
        let sql = View::default().band_sql(&s.columns, false);
        assert_eq!(sql.matches(" FROM ").count(), 1, "more than one scan: {sql}");
        assert!(sql.starts_with("SELECT count(*) AS n_total,"), "got {sql}");
        for i in 0..3 {
            assert!(sql.contains(&format!("AS n_present_{i}")), "column {i}: {sql}");
        }
        for part in [
            "approx_count_distinct",
            "n_distinct_",
            "v_min_",
            "v_max_",
            "min(",
            "max(",
        ] {
            assert!(!sql.contains(part), "compact must not ask for {part:?}: {sql}");
        }
        // The reader takes the values by position, so the two must agree about
        // the number of values for each column. The count below leaves out the
        // name of the relation, which also holds the word AS.
        assert_eq!(
            sql.matches(" AS n_").count(),
            1 + s.columns.len(),
            "one count of rows and one count for each column: {sql}"
        );
    }

    #[test]
    fn the_band_of_the_view_reads_the_rows_of_the_view() {
        // The band describes what the grid shows now. A filter must therefore
        // reach the statement, exactly as it reaches the page and the count.
        let v = View {
            base: Base::Source,
            filter: Some("amount > 100".into()),
            sort: vec![SortKey { column: "id".into(), dir: SortDir::Asc }],
        };
        let sql = v.band_sql(&schema().columns, true);
        assert!(sql.contains("FROM src AS q WHERE (amount > 100)"), "got {sql}");
    }

    #[test]
    fn the_band_statement_is_read_only() {
        // The statement goes to the engine, and every statement that reaches
        // the engine must pass the guard.
        let s = schema();
        for v in [
            View::default(),
            View {
                filter: Some("name = 'o''brien'".into()),
                ..Default::default()
            },
            View {
                base: Base::Sql("SELECT * FROM src".into()),
                filter: Some("id > 1".into()),
                sort: vec![SortKey { column: "id".into(), dir: SortDir::Desc }],
            },
        ] {
            let sql = v.band_sql(&s.columns, true);
            assert!(
                crate::sql_guard::ensure_read_only(&sql).is_ok(),
                "the guard refused {sql}"
            );
        }
    }

    #[test]
    fn a_band_of_no_columns_has_no_statement() {
        assert_eq!(View::default().band_sql(&[], true), "");
    }

    #[test]
    fn search_treats_a_wildcard_as_a_plain_character() {
        // The test uses contains() and not LIKE, so `%` and `_` need no escape
        // character. A search for "50%_off" looks for those seven characters.
        let sql = View::default().search_sql(&schema(), "50%_off", 0, 1000, 10);
        assert!(sql.contains("lower('50%_off')"), "got {sql}");
        assert!(!sql.contains("ESCAPE"), "no pattern, so no escape: {sql}");
        assert!(!sql.contains("\\%"), "the needle must not be escaped: {sql}");
    }

    #[test]
    fn search_quotes_a_needle_that_holds_a_quotation_mark() {
        let sql = View::default().search_sql(&schema(), "o'brien", 0, 1000, 10);
        assert!(sql.contains("lower('o''brien')"), "got {sql}");
    }

    #[test]
    fn search_lowers_both_sides_of_the_test() {
        // The database must fold the case of the needle and of the value with
        // the same rule. It therefore calls lower() on both.
        let sql = View::default().search_sql(&schema(), "Carol", 0, 1000, 10);
        assert!(sql.contains("contains(lower(\"name\"), lower('Carol'))"), "got {sql}");
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
        assert!(sql.contains("contains(lower(\"name\")"), "VARCHAR should not be cast: {sql}");
        assert!(
            sql.contains("contains(lower(CAST(\"id\" AS VARCHAR))"),
            "BIGINT needs a cast: {sql}"
        );
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
