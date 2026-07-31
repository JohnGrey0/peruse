//! The data that the user interface draws: the schema, the kinds of value, and
//! a page of rows.

/// The largest number of characters that Peruse keeps for one cell.
///
/// DuckDB changes each value into text with `CAST(... AS VARCHAR)`. This limit
/// then cuts the text. One JSON value of 40 megabytes can therefore not stop a
/// redraw. The cell inspector asks the engine for the complete value.
pub const MAX_CELL_CHARS: usize = 4096;

/// A family of values. Peruse uses the family to select a color and to select
/// the alignment. It reads the family one time for each column, from the SQL
/// type of that column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    /// An integer, a decimal or a floating-point value.
    Number,
    /// Text, or a type that Peruse shows as text.
    Text,
    /// A value that is true or false.
    Bool,
    /// A date, a time, a timestamp or an interval.
    Temporal,
    /// A value of bytes, such as a BLOB.
    Binary,
    /// A list, a structure, a map or a union.
    Nested,
}

/// The side of a cell where Peruse puts the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// The value starts at the left side.
    Left,
    /// The value ends at the right side.
    Right,
}

impl CellKind {
    /// Gives the family of values for a SQL type.
    pub fn from_sql_type(ty: &str) -> CellKind {
        let u = ty.trim().to_ascii_uppercase();
        // Test the container types first. The type `INTEGER[]` is a nested
        // type, and not a number.
        if u.contains('[')
            || u.starts_with("STRUCT")
            || u.starts_with("MAP")
            || u.starts_with("UNION")
        {
            return CellKind::Nested;
        }
        if u == "BOOLEAN" || u == "BOOL" {
            return CellKind::Bool;
        }
        if u.starts_with("DECIMAL")
            || u.starts_with("NUMERIC")
            || matches!(
                u.as_str(),
                "TINYINT"
                    | "SMALLINT"
                    | "INTEGER"
                    | "INT"
                    | "BIGINT"
                    | "HUGEINT"
                    | "UHUGEINT"
                    | "UTINYINT"
                    | "USMALLINT"
                    | "UINTEGER"
                    | "UBIGINT"
                    | "FLOAT"
                    | "REAL"
                    | "DOUBLE"
            )
        {
            return CellKind::Number;
        }
        if u.starts_with("TIMESTAMP") || u.starts_with("DATE") || u.starts_with("TIME") || u == "INTERVAL"
        {
            return CellKind::Temporal;
        }
        if matches!(u.as_str(), "BLOB" | "BYTEA" | "BINARY" | "VARBINARY" | "BIT") {
            return CellKind::Binary;
        }
        CellKind::Text
    }

    /// Gives the side of the cell where Peruse puts a value of this family.
    pub fn align(self) -> Align {
        match self {
            // Put the numbers at the right side. The user can then compare
            // the sizes of two numbers with the eye.
            CellKind::Number => Align::Right,
            _ => Align::Left,
        }
    }

    /// Gives the character that shows this family in a column header.
    ///
    /// No mark is a letter or a digit, and that rule is not free to break. The
    /// mark comes after the name of the column, so a letter reads as the last
    /// letter of that name: the mark for text was `a`, and a column called
    /// `customer_name` then showed as `customer_name a`, which a user reads as a
    /// spelling mistake and not as a type. A mark of punctuation cannot be read
    /// in that way. The test `no_mark_is_a_letter_or_a_digit` holds the rule.
    pub fn badge(self) -> char {
        match self {
            CellKind::Number => '#',
            // A pair of quotation marks is the sign for text in every language
            // that Peruse reads, so one of them says "text" with no legend.
            CellKind::Text => '"',
            CellKind::Bool => '?',
            CellKind::Temporal => '@',
            CellKind::Binary => '~',
            CellKind::Nested => '{',
        }
    }
}

/// One column of the schema.
#[derive(Clone, Debug)]
pub struct Column {
    /// The name of the column.
    pub name: String,
    /// The SQL type of the column, as DuckDB writes it.
    pub sql_type: String,
    /// The family of values, from the SQL type.
    pub kind: CellKind,
    /// `true` when the column can hold NULL.
    pub nullable: bool,
}

impl Column {
    /// Makes a column and reads its family of values from the SQL type.
    pub fn new(name: impl Into<String>, sql_type: impl Into<String>, nullable: bool) -> Column {
        let sql_type = sql_type.into();
        let kind = CellKind::from_sql_type(&sql_type);
        Column {
            name: name.into(),
            sql_type,
            kind,
            nullable,
        }
    }

    /// Gives a short form of the SQL type, for a narrow space.
    ///
    /// A header of 12 characters cannot show `TIMESTAMP WITH TIME ZONE`.
    pub fn short_type(&self) -> String {
        let t = &self.sql_type;
        let short = match t.to_ascii_uppercase().as_str() {
            "TIMESTAMP WITH TIME ZONE" => "TIMESTAMPTZ",
            "TIME WITH TIME ZONE" => "TIMETZ",
            "DOUBLE PRECISION" => "DOUBLE",
            "CHARACTER VARYING" => "VARCHAR",
            _ => t.as_str(),
        };
        // Count the characters, and do not count the bytes. A type name can
        // contain a character of more than one byte. A STRUCT type with a
        // field name in a language other than English is one example. A cut at
        // byte 17 can then fall inside a character. The program stops with an
        // error when this occurs.
        if short.chars().count() > 18 {
            format!("{}…", short.chars().take(17).collect::<String>())
        } else {
            short.to_string()
        }
    }
}

/// Reads the fields of a `STRUCT` type, one level deep.
///
/// DuckDB writes the type of a structure as text:
///
/// ```text
/// STRUCT(id BIGINT, "login" VARCHAR, tags VARCHAR[], inner STRUCT(a INTEGER))
/// ```
///
/// The metadata panel shows those fields, so a user can see what a column
/// holds without opening a row. The text is the one source that each format
/// gives: a Parquet footer names the leaves, and a CSV file and a JSON file
/// name nothing at all.
///
/// The function gives an empty list for a type that is not a structure.
///
/// A field of a list of structures, such as `STRUCT(a INTEGER)[]`, is not a
/// structure at this level. The function gives an empty list for it, and the
/// record view is the place that opens such a value.
pub fn struct_fields(sql_type: &str) -> Vec<Column> {
    let t = sql_type.trim();
    if !t.to_ascii_uppercase().starts_with("STRUCT(") || !t.ends_with(')') {
        return Vec::new();
    }
    let inner = &t["STRUCT(".len()..t.len() - 1];

    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quoted = false;
    let mut part = String::new();
    for c in inner.chars() {
        match c {
            // Two quotation marks inside a name are one quotation mark. The
            // state therefore turns off and on again, which is correct.
            '"' => {
                quoted = !quoted;
                part.push(c);
            }
            '(' | '[' if !quoted => {
                depth += 1;
                part.push(c);
            }
            ')' | ']' if !quoted => {
                depth -= 1;
                part.push(c);
            }
            ',' if !quoted && depth == 0 => {
                push_field(&mut out, &part);
                part.clear();
            }
            _ => part.push(c),
        }
    }
    push_field(&mut out, &part);
    out
}

/// Reads one `name type` pair from the text of a structure type.
fn push_field(out: &mut Vec<Column>, part: &str) {
    let s = part.trim();
    if s.is_empty() {
        return;
    }
    // The name comes first. A name in quotation marks can hold a space, so
    // the split must respect the marks.
    let (name, ty) = if let Some(rest) = s.strip_prefix('"') {
        match rest.find('"') {
            Some(end) => (rest[..end].replace("\"\"", "\""), rest[end + 1..].trim()),
            None => return,
        }
    } else {
        match s.find(' ') {
            Some(i) => (s[..i].to_string(), s[i + 1..].trim()),
            None => return,
        }
    };
    if name.is_empty() || ty.is_empty() {
        return;
    }
    out.push(Column::new(name, ty, true));
}

/// The columns of the view, in the order that the grid shows them.
#[derive(Clone, Debug, Default)]
pub struct Schema {
    /// The columns, in order.
    pub columns: Vec<Column>,
}

impl Schema {
    /// Gives the number of columns.
    pub fn len(&self) -> usize {
        self.columns.len()
    }
    /// Gives `true` when the schema has no column.
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
    /// Finds the position of a column by its name.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }
}

/// A block of rows that follow each other in the view.
///
/// The page holds each cell in one flat list. The grid reads the cells in the
/// order of the rows, and one list needs one allocation. A list of lists needs
/// one allocation for each row.
#[derive(Clone, Debug, Default)]
pub struct RowPage {
    /// The offset of the first row of the page, in the view.
    pub offset: u64,
    /// The number of columns.
    pub ncols: usize,
    /// The number of rows in the page.
    pub nrows: usize,
    cells: Vec<Option<String>>,
}

impl RowPage {
    /// Makes a page from a flat list of cells, in the order of the rows.
    pub fn new(offset: u64, ncols: usize, cells: Vec<Option<String>>) -> RowPage {
        let nrows = cells.len().checked_div(ncols).unwrap_or(0);
        RowPage {
            offset,
            ncols,
            nrows,
            cells,
        }
    }

    /// Gives the value of one cell of the page.
    ///
    /// `None` is the SQL value NULL. The user interface gives NULL a different
    /// color from an empty text, because the two are not the same.
    pub fn cell(&self, row: usize, col: usize) -> Option<&str> {
        if row >= self.nrows || col >= self.ncols {
            return None;
        }
        self.cells[row * self.ncols + col].as_deref()
    }

    /// Gives `true` when the text in the page is at the limit. The true value
    /// is then longer than the text.
    pub fn is_truncated(&self, row: usize, col: usize) -> bool {
        self.cell(row, col)
            .is_some_and(|s| s.chars().count() >= MAX_CELL_CHARS)
    }

    /// Gives the offset of a row in the view. The first row of the view has
    /// the offset 0.
    pub fn abs_row(&self, row: usize) -> u64 {
        self.offset + row as u64
    }

    /// Gives `true` when the page holds the row at the offset `abs`.
    pub fn contains(&self, abs: u64) -> bool {
        abs >= self.offset && abs < self.offset + self.nrows as u64
    }
}

/// The number of rows in the view, if Peruse knows it.
///
/// To count the rows of a CSV file with a filter, DuckDB scans the file. The
/// user interface must therefore be able to show a message about the count.
/// It must not block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowCount {
    /// Peruse did not ask for the count.
    Unknown,
    /// The worker counts the rows now.
    Counting,
    /// The exact number of rows.
    Exact(u64),
}

impl RowCount {
    /// Gives the number of rows, or `None` when Peruse does not know it.
    pub fn value(self) -> Option<u64> {
        match self {
            RowCount::Exact(n) => Some(n),
            _ => None,
        }
    }
    /// Gives the text that the user interface shows for this count.
    pub fn label(self) -> String {
        match self {
            RowCount::Unknown => "? rows".into(),
            RowCount::Counting => "counting…".into(),
            RowCount::Exact(n) => format!("{} rows", crate::source::human_count(n)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_types() {
        use CellKind::*;
        let cases = [
            ("INTEGER", Number),
            ("BIGINT", Number),
            ("DECIMAL(18,3)", Number),
            ("DOUBLE", Number),
            ("VARCHAR", Text),
            ("UUID", Text),
            ("BOOLEAN", Bool),
            ("DATE", Temporal),
            ("TIMESTAMP WITH TIME ZONE", Temporal),
            ("INTERVAL", Temporal),
            ("BLOB", Binary),
            ("INTEGER[]", Nested),
            ("STRUCT(a INTEGER)", Nested),
            ("MAP(VARCHAR, INTEGER)", Nested),
        ];
        for (ty, want) in cases {
            assert_eq!(CellKind::from_sql_type(ty), want, "type {ty}");
        }
    }

    #[test]
    fn a_long_type_name_with_multibyte_characters_does_not_stop_the_program() {
        // A STRUCT type can have a field name in a language other than
        // English. A cut at byte 17 then falls inside a character.
        for ty in [
            "STRUCT(aaaaaaaa\u{8a9e}xxxx INTEGER)",
            "STRUCT(\u{3b1}\u{3b1}\u{3b1}\u{3b1}\u{3b1}\u{3b1}\u{3b1}\u{3b1}\u{3b1}\u{3b1}\u{3b1} INTEGER)",
            "ENUM('aaaaaaaaaa\u{8a9e}xx','b')",
            "VARCHAR",
        ] {
            let short = Column::new("c", ty, true).short_type();
            assert!(short.chars().count() <= 18, "type {ty} gave {short}");
        }
    }

    #[test]
    fn a_short_type_name_is_not_cut() {
        assert_eq!(Column::new("c", "BIGINT", true).short_type(), "BIGINT");
        assert_eq!(
            Column::new("c", "TIMESTAMP WITH TIME ZONE", true).short_type(),
            "TIMESTAMPTZ"
        );
    }

    #[test]
    fn arrays_of_numbers_are_nested_not_numeric() {
        assert_eq!(CellKind::from_sql_type("BIGINT[3]"), CellKind::Nested);
    }

    #[test]
    fn no_mark_is_a_letter_or_a_digit() {
        // The mark comes after the name of the column. A letter therefore reads
        // as the last letter of that name: the mark for text was `a`, and a
        // column called `customer_name` showed as `customer_name a`. A user
        // reads that as a spelling mistake and not as a type, and one did.
        use CellKind::*;
        for kind in [Number, Text, Bool, Temporal, Binary, Nested] {
            let mark = kind.badge();
            assert!(
                !mark.is_alphanumeric(),
                "the mark {mark:?} of {kind:?} is a letter or a digit, so it reads \
                 as part of the name of the column"
            );
            assert!(
                !mark.is_whitespace() && !mark.is_control(),
                "the mark of {kind:?} must be a character that the user can see"
            );
        }
        // Two families with the same mark would say nothing.
        let marks: Vec<char> = [Number, Text, Bool, Temporal, Binary, Nested]
            .iter()
            .map(|k| k.badge())
            .collect();
        for (i, a) in marks.iter().enumerate() {
            assert!(
                !marks[i + 1..].contains(a),
                "the mark {a:?} belongs to more than one family"
            );
        }
    }

    #[test]
    fn only_numbers_right_align() {
        assert_eq!(CellKind::Number.align(), Align::Right);
        assert_eq!(CellKind::Text.align(), Align::Left);
        assert_eq!(CellKind::Temporal.align(), Align::Left);
    }

    #[test]
    fn page_indexing_is_row_major() {
        let page = RowPage::new(
            100,
            2,
            vec![
                Some("a".into()),
                None,
                Some("c".into()),
                Some("d".into()),
            ],
        );
        assert_eq!(page.nrows, 2);
        assert_eq!(page.cell(0, 0), Some("a"));
        assert_eq!(page.cell(0, 1), None, "SQL NULL");
        assert_eq!(page.cell(1, 1), Some("d"));
        assert_eq!(page.cell(9, 0), None, "out of range");
        assert_eq!(page.abs_row(1), 101);
        assert!(page.contains(101));
        assert!(!page.contains(102));
    }

    #[test]
    fn the_fields_of_a_structure_come_from_its_type() {
        let f = struct_fields("STRUCT(id BIGINT, login VARCHAR, avatar_url VARCHAR)");
        let names: Vec<&str> = f.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "login", "avatar_url"]);
        assert_eq!(f[0].sql_type, "BIGINT");
        assert_eq!(f[0].kind, CellKind::Number);
        assert_eq!(f[1].kind, CellKind::Text);
    }

    #[test]
    fn a_structure_inside_a_structure_stays_one_field() {
        // The comma inside the inner structure does not break the outer one.
        let f = struct_fields("STRUCT(a INTEGER, b STRUCT(c INTEGER, d VARCHAR), e DATE)");
        let names: Vec<&str> = f.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "e"]);
        assert_eq!(f[1].sql_type, "STRUCT(c INTEGER, d VARCHAR)");
        assert_eq!(f[1].kind, CellKind::Nested);
        // The inner structure opens in the same way.
        let inner = struct_fields(&f[1].sql_type);
        assert_eq!(inner.len(), 2);
        assert_eq!(inner[1].name, "d");
    }

    #[test]
    fn a_list_inside_a_structure_stays_one_field() {
        let f = struct_fields("STRUCT(tags VARCHAR[], commits STRUCT(sha VARCHAR)[])");
        let names: Vec<&str> = f.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["tags", "commits"]);
        assert_eq!(f[0].kind, CellKind::Nested);
        assert_eq!(f[1].sql_type, "STRUCT(sha VARCHAR)[]");
    }

    #[test]
    fn a_field_name_in_quotation_marks_keeps_its_spaces() {
        let f = struct_fields("STRUCT(\"first name\" VARCHAR, \"a,b\" INTEGER, plain DATE)");
        let names: Vec<&str> = f.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["first name", "a,b", "plain"]);
        assert_eq!(f[1].sql_type, "INTEGER");
    }

    #[test]
    fn a_type_that_is_not_a_structure_gives_no_field() {
        for ty in ["VARCHAR", "BIGINT", "INTEGER[]", "STRUCT(a INTEGER)[]", "", "MAP(VARCHAR, INTEGER)"] {
            assert!(struct_fields(ty).is_empty(), "type {ty:?} gave fields");
        }
    }

    #[test]
    fn a_type_that_is_cut_short_does_not_stop_the_program() {
        // A type of some thousand characters can arrive cut short. The
        // function must give what it can read, and must not panic.
        let full = "STRUCT(a INTEGER, b VARCHAR, c DATE)";
        for n in 1..full.len() {
            let _ = struct_fields(&full[..n]);
        }
    }

    #[test]
    fn row_count_labels() {
        assert_eq!(RowCount::Counting.label(), "counting…");
        assert_eq!(RowCount::Exact(1234).label(), "1,234 rows");
        assert_eq!(RowCount::Exact(5).value(), Some(5));
        assert_eq!(RowCount::Unknown.value(), None);
    }
}
