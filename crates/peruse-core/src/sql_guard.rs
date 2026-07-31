//! Makes sure that SQL from the user can read but cannot write.
//!
//! Peruse gives the user a promise: it does not change the data. Two layers
//! keep that promise, and each layer works alone:
//!
//! 1. The structure of the connection. The database is always in memory. The
//!    engine reads the user's files only through the table functions
//!    `read_parquet` and `read_csv`. These two functions can read but cannot
//!    write, and DuckDB never opens the files for write access.
//! 2. The words of the statement. This module does that work. It rejects each
//!    statement that is not a plain query. The statements that can still write
//!    to the disk are `COPY ... TO`, `EXPORT DATABASE`, `ATTACH` and `INSTALL`.
//!    None of them reaches the engine.
//!
//! This module removes the comments, the values in quotation marks and the
//! identifiers in quotation marks before it looks for a keyword. A column with
//! the true name `"update"`, or a value `'drop table'`, is therefore safe.

use std::fmt;

/// The reason that the guard rejects a statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardError {
    /// The text holds no statement.
    Empty,
    /// The text holds more than one statement.
    MultipleStatements,
    /// The statement does not start with a keyword that only reads. The value
    /// is the first word.
    NotAQuery(String),
    /// A keyword that writes, or that changes the state, is in the statement.
    /// The value is that keyword.
    Forbidden(String),
    /// A value or an identifier starts with a quotation mark, but no quotation
    /// mark closes it.
    UnterminatedLiteral,
    /// A filter expression has a different number of `(` and `)` characters.
    /// Such an expression can close the condition that Peruse builds and
    /// change the remainder of the statement.
    UnbalancedParens,
}

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GuardError::Empty => write!(f, "empty query"),
            GuardError::MultipleStatements => {
                write!(f, "only one statement at a time (found a `;` separator)")
            }
            GuardError::NotAQuery(k) => write!(
                f,
                "`{k}` is not a query — Peruse is read-only, start with SELECT / WITH / FROM / DESCRIBE / SUMMARIZE"
            ),
            GuardError::Forbidden(k) => {
                write!(f, "`{k}` would modify data or state — Peruse is read-only")
            }
            GuardError::UnterminatedLiteral => write!(f, "unterminated string or quoted identifier"),
            GuardError::UnbalancedParens => {
                write!(f, "unbalanced parentheses — check the ( and ) in the filter")
            }
        }
    }
}

impl std::error::Error for GuardError {}

/// The first words that a statement can have. A statement that starts with one
/// of these words only reads.
const ALLOWED_LEADING: &[&str] = &[
    "SELECT",
    "WITH",
    "FROM",
    "DESCRIBE",
    "DESC",
    "SHOW",
    "SUMMARIZE",
    "EXPLAIN",
    "VALUES",
    "TABLE",
    "PIVOT",
    "UNPIVOT",
];

/// The keywords that must not occur in a statement, at any position.
///
/// A check of the first word alone is not sufficient. The statements
/// `WITH ... INSERT` and `EXPLAIN COPY ...` both start with an allowed word, but
/// both of them write.
const FORBIDDEN_ANYWHERE: &[&str] = &[
    "INSERT",
    "INTO",
    "UPDATE",
    "DELETE",
    "DROP",
    "CREATE",
    "ALTER",
    "REPLACE",
    "TRUNCATE",
    "ATTACH",
    "DETACH",
    "COPY",
    "EXPORT",
    "IMPORT",
    "INSTALL",
    "LOAD",
    "VACUUM",
    "CHECKPOINT",
    "CALL",
    "SET",
    "RESET",
    "PRAGMA",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "TRANSACTION",
    "PREPARE",
    "EXECUTE",
    "DEALLOCATE",
    "GRANT",
    "REVOKE",
    "USE",
];

/// Removes the parts of a statement that can hold any text.
///
/// The function replaces each comment, each value in quotation marks and each
/// identifier in quotation marks. The structure of the statement stays, so a
/// search for a keyword is then safe.
///
/// The function gives the clean text. It also gives `true` when a semicolon
/// separates two statements that are not empty.
fn scrub(sql: &str) -> Result<(String, bool), GuardError> {
    let b: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(b.len());
    let mut i = 0;
    let mut statements = 0usize;
    let mut current_has_content = false;

    while i < b.len() {
        let c = b[i];
        // A comment that starts with -- and ends at the end of the line.
        if c == '-' && i + 1 < b.len() && b[i + 1] == '-' {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // A comment between /* and */. DuckDB does not put one of these
        // comments inside another one.
        if c == '/' && i + 1 < b.len() && b[i + 1] == '*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            if i + 1 >= b.len() {
                return Err(GuardError::UnterminatedLiteral);
            }
            i += 2;
            out.push(' ');
            continue;
        }
        // A value between single quotation marks. Two single quotation
        // marks in the value are one quotation mark.
        if c == '\'' {
            i += 1;
            loop {
                if i >= b.len() {
                    return Err(GuardError::UnterminatedLiteral);
                }
                if b[i] == '\'' {
                    if i + 1 < b.len() && b[i + 1] == '\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push_str(" '' ");
            current_has_content = true;
            continue;
        }
        // An identifier between double quotation marks. Two double
        // quotation marks in the identifier are one quotation mark.
        if c == '"' {
            i += 1;
            loop {
                if i >= b.len() {
                    return Err(GuardError::UnterminatedLiteral);
                }
                if b[i] == '"' {
                    if i + 1 < b.len() && b[i + 1] == '"' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push_str(" id ");
            current_has_content = true;
            continue;
        }
        // A value between two pairs of dollar signs.
        if c == '$' && i + 1 < b.len() && b[i + 1] == '$' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '$' && b[i + 1] == '$') {
                i += 1;
            }
            if i + 1 >= b.len() {
                return Err(GuardError::UnterminatedLiteral);
            }
            i += 2;
            out.push_str(" '' ");
            current_has_content = true;
            continue;
        }
        if c == ';' {
            if current_has_content {
                statements += 1;
                current_has_content = false;
            }
            out.push(' ');
            i += 1;
            continue;
        }
        if !c.is_whitespace() {
            current_has_content = true;
        }
        out.push(c);
        i += 1;
    }
    if current_has_content {
        statements += 1;
    }
    Ok((out, statements > 1))
}

/// Gives the first word of the clean text, in capital letters.
fn first_word(scrubbed: &str) -> Option<String> {
    scrubbed
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .find(|w| !w.is_empty())
        .map(|w| w.to_ascii_uppercase())
}

/// Checks a statement from the user.
///
/// The function gives `Ok(())` for one statement that only reads. For each
/// other input, it gives the reason for the refusal.
pub fn ensure_read_only(sql: &str) -> Result<(), GuardError> {
    let (scrubbed, multiple) = scrub(sql)?;
    if scrubbed.trim().is_empty() {
        return Err(GuardError::Empty);
    }
    if multiple {
        return Err(GuardError::MultipleStatements);
    }

    let lead = first_word(&scrubbed).ok_or(GuardError::Empty)?;
    if !ALLOWED_LEADING.contains(&lead.as_str()) {
        return Err(GuardError::NotAQuery(lead));
    }

    for word in scrubbed.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if word.is_empty() {
            continue;
        }
        let up = word.to_ascii_uppercase();
        if FORBIDDEN_ANYWHERE.contains(&up.as_str()) {
            return Err(GuardError::Forbidden(up));
        }
    }
    Ok(())
}

/// Checks a `WHERE` expression from the filter prompt.
///
/// Peruse puts this expression into a statement that it builds. The expression
/// must therefore not close the condition and start a new part.
pub fn ensure_safe_predicate(expr: &str) -> Result<(), GuardError> {
    if expr.trim().is_empty() {
        return Err(GuardError::Empty);
    }
    // Use the same clean-up code and the same list of forbidden keywords. The
    // word SELECT in front gives the text a legal first word.
    ensure_read_only(&format!("SELECT 1 WHERE {expr}"))?;

    // Count the parentheses of the expression.
    //
    // Peruse puts the expression into `WHERE (<expr>)`, and then adds
    // `ORDER BY` and `LIMIT` after it. An expression that contains one more
    // `)` than `(` closes the condition too early. The remainder of the
    // expression then becomes part of the statement. For example, the
    // expression `1) ORDER BY 1--` makes the comment marker delete the
    // `LIMIT` clause, and the engine reads the full file into one page.
    //
    // The clean-up code removes the comments and the text in quotation marks.
    // A parenthesis inside a text value therefore does not count.
    let (scrubbed, _) = scrub(expr)?;
    let mut depth: i32 = 0;
    for c in scrubbed.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return Err(GuardError::UnbalancedParens);
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(GuardError::UnbalancedParens);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_queries_pass() {
        for q in [
            "SELECT * FROM src",
            "  with a as (select 1) select * from a",
            "FROM src SELECT count(*)",
            "DESCRIBE src",
            "SUMMARIZE src",
            "EXPLAIN SELECT 1",
            "select * from src where name = 'drop table x'",
            "SELECT \"update\", \"delete\" FROM src",
            "SELECT * FROM src -- copy this later\n",
            "SELECT * /* insert */ FROM src",
        ] {
            assert!(ensure_read_only(q).is_ok(), "should allow: {q}");
        }
    }

    #[test]
    fn writes_are_rejected() {
        for q in [
            "DROP TABLE src",
            "INSERT INTO src VALUES (1)",
            "UPDATE src SET a = 1",
            "DELETE FROM src",
            "CREATE TABLE t AS SELECT 1",
            "ATTACH 'x.db'",
            "INSTALL httpfs",
            "COPY src TO 'out.csv'",
            "EXPORT DATABASE 'dir'",
            "PRAGMA database_list",
            "SET threads TO 1",
        ] {
            assert!(ensure_read_only(q).is_err(), "should reject: {q}");
        }
    }

    #[test]
    fn allowed_leading_keyword_does_not_smuggle_a_write() {
        // These statements start with SELECT, WITH or EXPLAIN, but they write.
        assert_eq!(
            ensure_read_only("WITH a AS (SELECT 1) INSERT INTO t SELECT * FROM a"),
            Err(GuardError::Forbidden("INSERT".into()))
        );
        assert_eq!(
            ensure_read_only("EXPLAIN COPY src TO 'out.parquet'"),
            Err(GuardError::Forbidden("COPY".into()))
        );
    }

    #[test]
    fn statement_stacking_is_rejected() {
        assert_eq!(
            ensure_read_only("SELECT 1; DROP TABLE src"),
            Err(GuardError::MultipleStatements)
        );
        // One semicolon at the end is usual, and the guard accepts it.
        assert!(ensure_read_only("SELECT 1;").is_ok());
        assert!(ensure_read_only("SELECT 1;  \n ").is_ok());
    }

    #[test]
    fn comments_cannot_hide_a_statement_break() {
        assert!(ensure_read_only("SELECT 1 -- ; DROP TABLE src").is_ok());
    }

    #[test]
    fn unterminated_literals_are_rejected() {
        assert_eq!(
            ensure_read_only("SELECT 'abc"),
            Err(GuardError::UnterminatedLiteral)
        );
    }

    #[test]
    fn empty_input() {
        assert_eq!(ensure_read_only("   \n\t "), Err(GuardError::Empty));
        assert_eq!(ensure_read_only("-- just a comment"), Err(GuardError::Empty));
    }

    #[test]
    fn a_filter_cannot_close_the_condition_that_peruse_builds() {
        // Peruse writes WHERE (<expr>) and then adds ORDER BY and LIMIT. An
        // expression with one more `)` than `(` closes the condition early and
        // changes the parts that follow.
        for expr in [
            "1) ORDER BY 1--",
            "1) LIMIT 999999999 --",
            "1)) OR ((1=1",
            "(1",
        ] {
            assert_eq!(
                ensure_safe_predicate(expr),
                Err(GuardError::UnbalancedParens),
                "should refuse: {expr}"
            );
        }
    }

    #[test]
    fn a_parenthesis_in_a_text_value_does_not_count() {
        // The clean-up code removes the text in quotation marks. A filter that
        // looks for a bracket is therefore correct.
        assert!(ensure_safe_predicate("name = 'a)b'").is_ok());
        assert!(ensure_safe_predicate("name LIKE '%(%'").is_ok());
        assert!(ensure_safe_predicate("(a > 1 AND b < 2) OR c = 3").is_ok());
    }

    #[test]
    fn select_into_is_refused() {
        // A subquery makes SELECT INTO fail in DuckDB, but the guard refuses it
        // before the engine sees it.
        assert_eq!(
            ensure_read_only("SELECT * INTO other FROM src"),
            Err(GuardError::Forbidden("INTO".into()))
        );
    }

    #[test]
    fn predicates() {
        assert!(ensure_safe_predicate("amount > 100 AND region = 'EU'").is_ok());
        assert!(ensure_safe_predicate("name ILIKE '%o''brien%'").is_ok());
        // The user must not be able to close the condition and add a
        // second statement.
        assert!(ensure_safe_predicate("1=1; DROP TABLE src").is_err());
        assert!(ensure_safe_predicate("").is_err());
    }
}
