//! A filter that the user builds one condition at a time.
//!
//! The filter prompt takes a `WHERE` expression as text. That prompt is quick
//! for a user who knows SQL. This module holds the other form: a list of
//! conditions that the user picks from menus. Peruse compiles the list into
//! one `WHERE` expression, and puts it in [`crate::query::View::filter`]. The
//! two forms therefore share each part of the code after this point.
//!
//! The compiled text is safe because of the way that this module builds it.
//! Each column name goes through [`crate::query::quote_ident`], and each value
//! goes through [`crate::query::quote_str`]. A value can therefore not close
//! its own quotation marks and change the statement. The parentheses are also
//! always balanced. The function [`crate::sql_guard::ensure_safe_predicate`]
//! accepts each text that this module gives, and a test proves it.
//!
//! A text that the user types is one [`Term::Raw`] condition in the same list.
//! The two forms can therefore stand together, and a quick filter from the
//! grid can add a condition to a filter that the user typed.

use crate::model::CellKind;
use crate::query::{quote_ident, quote_str};

/// The word between two conditions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    /// The two conditions must both be true.
    And,
    /// One of the two conditions must be true.
    Or,
}

impl Join {
    /// Gives the SQL word for the join.
    pub fn word(self) -> &'static str {
        match self {
            Join::And => "AND",
            Join::Or => "OR",
        }
    }
    /// Gives the other join. The key `o` in the builder uses it.
    pub fn other(self) -> Join {
        match self {
            Join::And => Join::Or,
            Join::Or => Join::And,
        }
    }
}

/// The test that one condition makes on one column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// The value is the same.
    Eq,
    /// The value is not the same.
    Ne,
    /// The value is smaller.
    Lt,
    /// The value is smaller or the same.
    Le,
    /// The value is larger.
    Gt,
    /// The value is larger or the same.
    Ge,
    /// The text holds the value.
    Contains,
    /// The text does not hold the value.
    NotContains,
    /// The text starts with the value.
    StartsWith,
    /// The text ends with the value.
    EndsWith,
    /// The value is NULL.
    IsNull,
    /// The value is not NULL.
    IsNotNull,
    /// The value is between two values.
    Between,
    /// The value is one of a list of values.
    In,
}

impl Op {
    /// Gives the name that the builder shows for the operator.
    pub fn label(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "<>",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Contains => "contains",
            Op::NotContains => "does not contain",
            Op::StartsWith => "starts with",
            Op::EndsWith => "ends with",
            Op::IsNull => "is null",
            Op::IsNotNull => "is not null",
            Op::Between => "between",
            Op::In => "is one of",
        }
    }

    /// Gives a longer description, for the list of operators.
    pub fn help(self) -> &'static str {
        match self {
            Op::Eq => "the value is the same",
            Op::Ne => "the value is not the same (a NULL never matches)",
            Op::Lt => "the value is smaller",
            Op::Le => "the value is smaller or the same",
            Op::Gt => "the value is larger",
            Op::Ge => "the value is larger or the same",
            Op::Contains => "the text holds this, in any position",
            Op::NotContains => "the text does not hold this (a NULL matches)",
            Op::StartsWith => "the text starts with this",
            Op::EndsWith => "the text ends with this",
            Op::IsNull => "the value is missing",
            Op::IsNotNull => "the value is present",
            Op::Between => "from the first value to the second value",
            Op::In => "one of a list, separated by commas",
        }
    }

    /// Gives `true` when the operator needs a value from the user.
    pub fn needs_value(self) -> bool {
        !matches!(self, Op::IsNull | Op::IsNotNull)
    }

    /// Gives `true` when the operator needs a second value.
    pub fn needs_second_value(self) -> bool {
        matches!(self, Op::Between)
    }

    /// Gives the word that the value prompt shows in front of the text.
    pub fn value_hint(self) -> &'static str {
        match self {
            Op::In => "values, separated by commas",
            Op::Between => "the first value",
            Op::Contains | Op::NotContains | Op::StartsWith | Op::EndsWith => "text",
            _ => "value",
        }
    }

    /// Gives the operators that have a meaning for a family of values, with
    /// the most useful operator first.
    ///
    /// A comparison of two BLOB values has no meaning for a user, because the
    /// grid shows the size of such a value and not its bytes. That family
    /// therefore gets the two tests for NULL only.
    pub fn for_kind(kind: CellKind) -> &'static [Op] {
        use Op::*;
        match kind {
            CellKind::Number | CellKind::Temporal => &[
                Eq, Ne, Gt, Ge, Lt, Le, Between, In, IsNull, IsNotNull,
            ],
            CellKind::Text => &[
                Contains, Eq, Ne, StartsWith, EndsWith, NotContains, In, Gt, Lt, IsNull,
                IsNotNull,
            ],
            CellKind::Bool => &[Eq, Ne, IsNull, IsNotNull],
            CellKind::Binary => &[IsNull, IsNotNull],
            CellKind::Nested => &[Contains, NotContains, IsNull, IsNotNull],
        }
    }
}

/// One test in the filter.
#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    /// A test that the user built from the menus.
    Cmp {
        /// The name of the column, as the schema holds it.
        column: String,
        /// The family of values of the column. It selects the form of the
        /// value in the statement.
        kind: CellKind,
        /// The test.
        op: Op,
        /// The value, as the user typed it.
        value: String,
        /// The second value. Only `Op::Between` uses it.
        value2: String,
    },
    /// A `WHERE` expression that the user typed in the prompt.
    Raw(String),
}

impl Term {
    /// Gives `true` when the term makes no test. The compiler leaves it out.
    ///
    /// A `between` condition needs its two values. With one value only, the
    /// statement would hold an empty text, and the database would give an
    /// error about a type that it cannot read.
    pub fn is_blank(&self) -> bool {
        match self {
            Term::Raw(s) => s.trim().is_empty(),
            Term::Cmp { op, value, value2, .. } => {
                (op.needs_value() && value.trim().is_empty())
                    || (op.needs_second_value() && value2.trim().is_empty())
            }
        }
    }

    /// Builds the SQL for this term. The result is always in parentheses.
    pub fn to_sql(&self) -> String {
        match self {
            Term::Raw(s) => format!("({})", s.trim()),
            Term::Cmp {
                column,
                kind,
                op,
                value,
                value2,
            } => {
                let c = quote_ident(column);
                // A text column is text already. A cast would copy each value
                // and give no gain. Each other family needs the cast, because
                // ILIKE takes text only.
                let as_text = if *kind == CellKind::Text {
                    c.clone()
                } else {
                    format!("CAST({c} AS VARCHAR)")
                };
                let lit = |v: &str| literal(*kind, v);
                match op {
                    Op::IsNull => format!("({c} IS NULL)"),
                    Op::IsNotNull => format!("({c} IS NOT NULL)"),
                    Op::Eq => format!("({c} = {})", lit(value)),
                    Op::Ne => format!("({c} <> {})", lit(value)),
                    Op::Lt => format!("({c} < {})", lit(value)),
                    Op::Le => format!("({c} <= {})", lit(value)),
                    Op::Gt => format!("({c} > {})", lit(value)),
                    Op::Ge => format!("({c} >= {})", lit(value)),
                    Op::Between => {
                        format!("({c} BETWEEN {} AND {})", lit(value), lit(value2))
                    }
                    Op::In => {
                        let items: Vec<String> = split_list(value).iter().map(|v| lit(v)).collect();
                        if items.is_empty() {
                            // An empty list is not legal SQL. A test that no
                            // row passes is the nearest safe meaning.
                            "(false)".to_string()
                        } else {
                            format!("({c} IN ({}))", items.join(", "))
                        }
                    }
                    Op::Contains => format!("({})", like(&as_text, value, true, true)),
                    Op::StartsWith => format!("({})", like(&as_text, value, false, true)),
                    Op::EndsWith => format!("({})", like(&as_text, value, true, false)),
                    // A row with NULL holds no text, so it does not hold the
                    // value. `NOT (NULL ILIKE …)` gives NULL, and the row
                    // would go away. The call to coalesce keeps it.
                    Op::NotContains => format!(
                        "(NOT coalesce({}, false))",
                        like(&as_text, value, true, true)
                    ),
                }
            }
        }
    }

    /// Gives the three parts that the builder shows in its list: the column,
    /// the operator and the value.
    pub fn parts(&self) -> (String, String, String) {
        match self {
            Term::Raw(s) => ("(SQL)".into(), String::new(), s.trim().to_string()),
            Term::Cmp {
                column,
                op,
                value,
                value2,
                ..
            } => {
                let v = if !op.needs_value() {
                    String::new()
                } else if op.needs_second_value() {
                    format!("{value} … {value2}")
                } else {
                    value.clone()
                };
                (column.clone(), op.label().to_string(), v)
            }
        }
    }
}

/// One condition: the word in front of it, and the test.
#[derive(Clone, Debug, PartialEq)]
pub struct Condition {
    /// The word between this condition and the condition in front of it. The
    /// first condition of the list does not use it.
    pub join: Join,
    /// The test.
    pub term: Term,
}

/// The complete filter, as a list of conditions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilterSet {
    /// The conditions, in the order that the user made them.
    pub conditions: Vec<Condition>,
}

impl FilterSet {
    /// Makes a filter that holds one expression that the user typed.
    pub fn from_raw(sql: &str) -> FilterSet {
        FilterSet {
            conditions: vec![Condition {
                join: Join::And,
                term: Term::Raw(sql.to_string()),
            }],
        }
    }

    /// Gives `true` when the filter makes no test.
    pub fn is_empty(&self) -> bool {
        self.conditions.iter().all(|c| c.term.is_blank())
    }

    /// Gives the number of conditions.
    pub fn len(&self) -> usize {
        self.conditions.len()
    }

    /// Adds a condition at the end, with the word `AND` in front of it.
    pub fn push(&mut self, term: Term) {
        self.conditions.push(Condition {
            join: Join::And,
            term,
        });
    }

    /// Removes each condition.
    pub fn clear(&mut self) {
        self.conditions.clear();
    }

    /// Builds the `WHERE` expression, or gives `None` when the filter makes no
    /// test.
    ///
    /// The compiler puts each step in parentheses, from the left to the right.
    /// The list `a OR b AND c` therefore gives `((a) OR (b)) AND (c)`, and not
    /// the SQL order, in which `AND` binds first. The user reads the list from
    /// the top to the bottom, so the result must follow that same order.
    pub fn to_sql(&self) -> Option<String> {
        let mut live = self.conditions.iter().filter(|c| !c.term.is_blank());
        let mut out = live.next()?.term.to_sql();
        for c in live {
            out = format!("({out} {} {})", c.join.word(), c.term.to_sql());
        }
        Some(out)
    }
}

/// Puts one value in the form that a statement needs.
///
/// A number goes in the statement as a number, so a comparison uses the order
/// of the numbers and not the order of the characters. Each other value goes
/// in quotation marks, and DuckDB casts it to the type of the column. The
/// quotation marks are the reason that a value cannot change the statement.
fn literal(kind: CellKind, v: &str) -> String {
    let t = v.trim();
    match kind {
        CellKind::Number if is_plain_number(t) => t.to_string(),
        CellKind::Bool if matches!(t.to_ascii_lowercase().as_str(), "true" | "false") => {
            t.to_ascii_uppercase()
        }
        _ => quote_str(t),
    }
}

/// Gives `true` when the text is a number that a statement can hold directly.
///
/// The test of the characters comes first. Without it, the text `inf` and the
/// text `NaN` would pass, because `f64` reads them. In a statement they are
/// names, and not numbers.
fn is_plain_number(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E'))
        && s.parse::<f64>().is_ok()
}

/// Builds a test with ILIKE, with no parentheses around it. The two flags add
/// the character `%` in front of the value and after it.
///
/// The caller adds the parentheses. The test for `does not contain` needs the
/// bare form, because it puts the test inside a call to `coalesce`.
fn like(target: &str, value: &str, lead: bool, trail: bool) -> String {
    // The characters % and _ have a meaning in a pattern. The user types a
    // value, and not a pattern, so these characters must lose that meaning.
    let esc = value
        .trim()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pat = format!(
        "{}{esc}{}",
        if lead { "%" } else { "" },
        if trail { "%" } else { "" }
    );
    format!("{target} ILIKE {} ESCAPE '\\'", quote_str(&pat))
}

/// Breaks the text of an `is one of` condition into its values.
///
/// A comma inside quotation marks belongs to the value. The user can therefore
/// write `'a,b', c` for the two values `a,b` and `c`.
fn split_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == ',' => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
            }
            None => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_guard::ensure_safe_predicate;

    fn cmp(column: &str, kind: CellKind, op: Op, value: &str) -> Term {
        Term::Cmp {
            column: column.into(),
            kind,
            op,
            value: value.into(),
            value2: String::new(),
        }
    }

    #[test]
    fn a_number_stays_a_number_and_a_text_gets_quotation_marks() {
        assert_eq!(
            cmp("amount", CellKind::Number, Op::Gt, "100").to_sql(),
            "(\"amount\" > 100)"
        );
        assert_eq!(
            cmp("region", CellKind::Text, Op::Eq, "EU").to_sql(),
            "(\"region\" = 'EU')"
        );
        // A value that is not a number goes in quotation marks, and DuckDB
        // casts it. A bad value then gives an error from the database, and
        // not a broken statement.
        assert_eq!(
            cmp("amount", CellKind::Number, Op::Gt, "abc").to_sql(),
            "(\"amount\" > 'abc')"
        );
    }

    #[test]
    fn the_words_inf_and_nan_do_not_become_bare_names() {
        for v in ["inf", "NaN", "-infinity"] {
            let sql = cmp("x", CellKind::Number, Op::Eq, v).to_sql();
            assert!(sql.contains('\''), "{v} gave {sql}");
        }
    }

    #[test]
    fn a_date_keeps_its_quotation_marks_so_duckdb_casts_it() {
        assert_eq!(
            cmp("ts", CellKind::Temporal, Op::Ge, "2024-01-01").to_sql(),
            "(\"ts\" >= '2024-01-01')"
        );
    }

    #[test]
    fn contains_escapes_the_pattern_characters() {
        let sql = cmp("name", CellKind::Text, Op::Contains, "50%_off").to_sql();
        assert_eq!(
            sql,
            "(\"name\" ILIKE '%50\\%\\_off%' ESCAPE '\\')"
        );
    }

    #[test]
    fn a_text_column_needs_no_cast_and_each_other_family_needs_one() {
        assert!(cmp("name", CellKind::Text, Op::Contains, "a")
            .to_sql()
            .starts_with("(\"name\" ILIKE"));
        assert!(cmp("n", CellKind::Number, Op::Contains, "a")
            .to_sql()
            .starts_with("(CAST(\"n\" AS VARCHAR) ILIKE"));
    }

    #[test]
    fn does_not_contain_keeps_the_rows_that_hold_null() {
        let sql = cmp("name", CellKind::Text, Op::NotContains, "x").to_sql();
        assert_eq!(
            sql,
            "(NOT coalesce(\"name\" ILIKE '%x%' ESCAPE '\\', false))"
        );
    }

    #[test]
    fn is_null_needs_no_value() {
        assert_eq!(
            cmp("c", CellKind::Text, Op::IsNull, "").to_sql(),
            "(\"c\" IS NULL)"
        );
        assert!(!cmp("c", CellKind::Text, Op::IsNull, "").is_blank());
        assert!(cmp("c", CellKind::Text, Op::Eq, "  ").is_blank());
    }

    #[test]
    fn between_uses_the_two_values() {
        let t = Term::Cmp {
            column: "n".into(),
            kind: CellKind::Number,
            op: Op::Between,
            value: "1".into(),
            value2: "9".into(),
        };
        assert_eq!(t.to_sql(), "(\"n\" BETWEEN 1 AND 9)");
    }

    #[test]
    fn between_with_one_value_only_makes_no_test() {
        // Without this rule, the statement would hold `BETWEEN 1 AND ''`, and
        // the database would give an error about the empty text.
        let t = Term::Cmp {
            column: "n".into(),
            kind: CellKind::Number,
            op: Op::Between,
            value: "1".into(),
            value2: "  ".into(),
        };
        assert!(t.is_blank());
        let mut f = FilterSet::default();
        f.push(t);
        assert_eq!(f.to_sql(), None);
    }

    #[test]
    fn a_list_breaks_on_commas_but_not_inside_quotation_marks() {
        assert_eq!(split_list("a, b ,c"), vec!["a", "b", "c"]);
        assert_eq!(split_list("'a,b', c"), vec!["a,b", "c"]);
        assert_eq!(split_list("  ,, "), Vec::<String>::new());
        assert_eq!(
            cmp("r", CellKind::Text, Op::In, "EU, US").to_sql(),
            "(\"r\" IN ('EU', 'US'))"
        );
        // An empty list is not legal SQL. The compiler gives a test that no
        // row passes.
        assert_eq!(cmp("r", CellKind::Text, Op::In, " , ").to_sql(), "(false)");
    }

    #[test]
    fn the_list_compiles_from_the_left_to_the_right() {
        let mut f = FilterSet::default();
        f.push(cmp("a", CellKind::Number, Op::Gt, "1"));
        f.push(cmp("b", CellKind::Number, Op::Gt, "2"));
        f.push(cmp("c", CellKind::Number, Op::Gt, "3"));
        f.conditions[1].join = Join::Or;
        // The user reads the list from the top to the bottom, so the result
        // must follow that order and not the SQL order, in which AND binds
        // before OR.
        assert_eq!(
            f.to_sql().unwrap(),
            "(((\"a\" > 1) OR (\"b\" > 2)) AND (\"c\" > 3))"
        );
    }

    #[test]
    fn an_empty_filter_gives_no_expression() {
        assert_eq!(FilterSet::default().to_sql(), None);
        assert!(FilterSet::default().is_empty());
        assert_eq!(FilterSet::from_raw("   ").to_sql(), None);
    }

    #[test]
    fn a_blank_condition_does_not_reach_the_statement() {
        let mut f = FilterSet::default();
        f.push(cmp("a", CellKind::Number, Op::Gt, "1"));
        f.push(cmp("b", CellKind::Number, Op::Gt, ""));
        assert_eq!(f.to_sql().unwrap(), "(\"a\" > 1)");
    }

    #[test]
    fn a_typed_expression_and_a_built_condition_work_together() {
        let mut f = FilterSet::from_raw("amount > 100");
        f.push(cmp("region", CellKind::Text, Op::Eq, "EU"));
        assert_eq!(
            f.to_sql().unwrap(),
            "((amount > 100) AND (\"region\" = 'EU'))"
        );
    }

    #[test]
    fn each_compiled_filter_passes_the_read_only_guard() {
        // The builder writes the statement, so a value can never close its
        // own quotation marks. This test holds the promise: each value that a
        // user can type must give a text that the guard accepts.
        let nasty = [
            "1) OR (1=1",
            "'; DROP TABLE src; --",
            "x'--",
            "a\"b",
            "%_\\",
            "1); COPY src TO 'out.csv'; --",
            "INSERT",
            ")))",
        ];
        for kind in [
            CellKind::Text,
            CellKind::Number,
            CellKind::Temporal,
            CellKind::Bool,
            CellKind::Nested,
        ] {
            for op in Op::for_kind(kind) {
                for v in nasty {
                    let mut f = FilterSet::default();
                    f.push(Term::Cmp {
                        // The column name is also from the file, so it can
                        // hold a quotation mark.
                        column: format!("col\"{v}"),
                        kind,
                        op: *op,
                        value: v.to_string(),
                        value2: v.to_string(),
                    });
                    let sql = f.to_sql().unwrap();
                    assert!(
                        ensure_safe_predicate(&sql).is_ok(),
                        "{kind:?} {op:?} {v:?} gave {sql}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_operators_of_a_family_all_have_a_meaning_for_it() {
        // A BLOB value is the one family with no comparison. The grid shows
        // the size of such a value, so a comparison would confuse the user.
        assert_eq!(Op::for_kind(CellKind::Binary), &[Op::IsNull, Op::IsNotNull]);
        for kind in [CellKind::Number, CellKind::Text, CellKind::Bool] {
            let ops = Op::for_kind(kind);
            assert!(ops.contains(&Op::Eq), "{kind:?} has no test for equality");
            assert!(ops.contains(&Op::IsNull), "{kind:?} has no test for NULL");
        }
    }

    #[test]
    fn the_list_shows_the_three_parts_of_a_condition() {
        let (c, o, v) = cmp("amount", CellKind::Number, Op::Gt, "100").parts();
        assert_eq!((c.as_str(), o.as_str(), v.as_str()), ("amount", ">", "100"));
        let (c, _, v) = Term::Raw("a = 1".into()).parts();
        assert_eq!((c.as_str(), v.as_str()), ("(SQL)", "a = 1"));
        // An operator with no value shows no value.
        let (_, _, v) = cmp("c", CellKind::Text, Op::IsNull, "x").parts();
        assert_eq!(v, "");
    }
}
