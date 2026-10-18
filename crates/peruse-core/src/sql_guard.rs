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

/// The functions that the guard refuses, each only when a `(` follows the name.
///
/// A keyword in [`FORBIDDEN_ANYWHERE`] cannot hide inside a value, because the
/// clean-up blanks each value first. A function is different: a user can call
/// one in a `FROM` part or in a `SELECT` part with no keyword at all, and some
/// functions of DuckDB write a file, change the state of the database, or run
/// SQL that comes from a text value. The list holds the name of each such
/// function.
///
/// The guard matches a name only when a `(` follows it, so a column with the
/// name `query` still passes. Refer to [`contains_forbidden_call`].
///
/// The list comes from the catalogue of DuckDB. The test
/// `every_catalogue_function_is_classified` reads `duckdb_functions()` and the
/// pragma list and fails when a later version of DuckDB adds a table function,
/// a table macro or a pragma that this module does not classify.
///
/// | Function | Why it is refused |
/// |---|---|
/// | `enable_logging`, `disable_logging`, `write_log` | Write a log file. |
/// | `enable_profiling`, `disable_profiling` | Write a profile file. |
/// | `checkpoint`, `force_checkpoint` | Write the pages of a database to the disk. |
/// | `truncate_duckdb_logs` | Change the state of the database. |
/// | `query`, `query_table` | Run SQL that comes from a text value. |
/// | `json_execute_serialized_sql` | Run SQL that comes from a text value. |
/// | `copy_database`, `import_database` | Write files, or run SQL from files. |
const FORBIDDEN_FUNCTIONS: &[&str] = &[
    "enable_logging",
    "disable_logging",
    "write_log",
    "enable_profiling",
    "disable_profiling",
    "checkpoint",
    "force_checkpoint",
    "truncate_duckdb_logs",
    "query",
    "query_table",
    "json_execute_serialized_sql",
    "copy_database",
    "import_database",
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
        // DuckDB ends the comment at a carriage return as well as at a line
        // feed, so the guard must end it at both. A guard that ended it at the
        // line feed alone would read `-- x\r; DROP TABLE t` as one comment,
        // but DuckDB would read a second statement after the carriage return.
        if c == '-' && i + 1 < b.len() && b[i + 1] == '-' {
            while i < b.len() && b[i] != '\n' && b[i] != '\r' {
                i += 1;
            }
            continue;
        }
        // A comment between /* and */. DuckDB nests these comments, so the
        // guard counts the depth. A guard that stopped at the first */ would
        // read `/* /* */ ... */` as a comment and then the text, but DuckDB
        // reads the whole span as one comment, and a keyword inside it would
        // then be hidden from one of the two and not the other.
        if c == '/' && i + 1 < b.len() && b[i + 1] == '*' {
            i += 2;
            let mut depth = 1;
            while i + 1 < b.len() && depth > 0 {
                if b[i] == '/' && b[i + 1] == '*' {
                    depth += 1;
                    i += 2;
                } else if b[i] == '*' && b[i + 1] == '/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth > 0 {
                return Err(GuardError::UnterminatedLiteral);
            }
            out.push(' ');
            continue;
        }
        // A value between single quotation marks. Two single quotation
        // marks in the value are one quotation mark.
        //
        // A value that a letter `E` or `e` comes in front of, with no
        // identifier character before that letter, is an escape string. In
        // such a string a backslash escapes the character after it, so `E'\''`
        // is one value that holds one quotation mark, and the `\'` does not
        // end the value. DuckDB reads the backslash this way, and a guard that
        // did not would see a different number of values, and a `;` could then
        // hide from the guard but not from DuckDB.
        if c == '\'' {
            let e_string = i >= 1
                && (b[i - 1] == 'e' || b[i - 1] == 'E')
                && (i < 2 || !is_ident_char(b[i - 2]));
            i += 1;
            loop {
                if i >= b.len() {
                    return Err(GuardError::UnterminatedLiteral);
                }
                // A backslash in an escape string hides the next character,
                // including a quotation mark or a second backslash.
                if e_string && b[i] == '\\' {
                    i += 2;
                    continue;
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
        // A value between two dollar quotations, with a tag or with no tag:
        // `$$ ... $$` or `$tag$ ... $tag$`. The tag follows the rules of an
        // identifier.
        //
        // A `$` is the start of such a value only when no identifier character
        // comes in front of it. A `$` after an identifier character, such as
        // the `$` of `a$b` or of a parameter `$1`, is an ordinary character,
        // and the guard reads it as text. DuckDB reads it the same way, so a
        // `$1` cannot start a value that hides a `;` from the guard.
        if c == '$'
            && (i == 0 || !is_ident_char(b[i - 1]))
            && let Some(body_start) = dollar_tag(&b, i)
        {
            // The closing tag is the same `$tag$`.
            let close: Vec<char> = b[i..body_start].to_vec();
            let mut j = body_start;
            let end = loop {
                if j + close.len() > b.len() {
                    return Err(GuardError::UnterminatedLiteral);
                }
                if b[j..j + close.len()] == close[..] {
                    break j + close.len();
                }
                j += 1;
            };
            i = end;
            out.push_str(" '' ");
            current_has_content = true;
            continue;
        }
        // A `$` that is not the start of a dollar quotation falls through to
        // the end of the loop, which keeps it as an ordinary character.
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

/// Gives `true` for a character that a name of a function or a keyword holds.
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Reads the tag of a dollar quotation that starts at `start`, where `b[start]`
/// is `$`.
///
/// The function gives the index of the first character of the body, after the
/// opening `$tag$`, or `None` when the characters are not a dollar quotation. A
/// tag follows the rules of an identifier: it starts with a letter or a low
/// line, and it holds letters, digits and low lines.
fn dollar_tag(b: &[char], start: usize) -> Option<usize> {
    debug_assert_eq!(b[start], '$');
    let mut j = start + 1;
    // An empty tag: `$$`.
    if j < b.len() && b[j] == '$' {
        return Some(j + 1);
    }
    // A tag starts with a letter or a low line, not a digit.
    if j >= b.len() || !(b[j].is_alphabetic() || b[j] == '_') {
        return None;
    }
    while j < b.len() && (b[j].is_alphanumeric() || b[j] == '_') {
        j += 1;
    }
    (j < b.len() && b[j] == '$').then_some(j + 1)
}

/// Gives the name of a forbidden function that the clean text calls, if any.
///
/// The function looks for each name in [`FORBIDDEN_FUNCTIONS`] as a whole word
/// that a `(` follows, with spaces allowed between the two. A column or an alias
/// with the same name, which no `(` follows, does not match. The clean text
/// holds no value and no identifier in quotation marks, so a name inside a value
/// cannot match either.
fn contains_forbidden_call(scrubbed: &str) -> Option<String> {
    let b: Vec<char> = scrubbed.chars().collect();
    let mut i = 0;
    while i < b.len() {
        // The start of a word: an identifier character with no identifier
        // character in front of it.
        if is_ident_char(b[i]) && (i == 0 || !is_ident_char(b[i - 1])) {
            let start = i;
            while i < b.len() && is_ident_char(b[i]) {
                i += 1;
            }
            let word: String = b[start..i].iter().collect::<String>().to_ascii_lowercase();
            if FORBIDDEN_FUNCTIONS.contains(&word.as_str()) {
                let mut k = i;
                while k < b.len() && b[k].is_whitespace() {
                    k += 1;
                }
                if k < b.len() && b[k] == '(' {
                    return Some(word);
                }
            }
        } else {
            i += 1;
        }
    }
    None
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

    scan_forbidden(&scrubbed)
}

/// Rejects a clean text that holds a keyword that writes, or a call to a
/// function that writes, changes the state, or runs SQL from a value.
///
/// The function does not check the first word, so a caller can use it on the
/// body of a view or a macro, which does not start with a query keyword.
fn scan_forbidden(scrubbed: &str) -> Result<(), GuardError> {
    for word in scrubbed.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if word.is_empty() {
            continue;
        }
        let up = word.to_ascii_uppercase();
        if FORBIDDEN_ANYWHERE.contains(&up.as_str()) {
            return Err(GuardError::Forbidden(up));
        }
    }
    // A function that writes a file, changes the state, or runs SQL from a
    // value, called with no keyword that the step above catches.
    if let Some(name) = contains_forbidden_call(scrubbed) {
        return Err(GuardError::Forbidden(name));
    }
    Ok(())
}

/// Checks the stored SQL of a view inside a `.duckdb` file, before Peruse reads
/// the view.
///
/// A database file can hold a view whose query calls a function that writes a
/// file. DuckDB runs that query when Peruse reads the view, so the check comes
/// before the read. Refer to `docs/read-only-guard.md`, Layer B.
///
/// The stored text is a complete `CREATE VIEW <name> AS <query>` statement. The
/// function drops the part up to the first `AS`, which removes the `CREATE` and
/// the name, and it then checks the query for a keyword or a function that
/// writes. The clean-up blanks the name first, so an `AS` inside a name in
/// quotation marks does not end the search too early.
pub fn ensure_view_body_read_only(sql: &str) -> Result<(), GuardError> {
    let (scrubbed, _) = scrub(sql)?;
    let body = body_after_as(&scrubbed).unwrap_or(&scrubbed);
    scan_forbidden(body)
}

/// Checks the body of a macro inside a `.duckdb` file, before Peruse reads an
/// object that can call the macro.
///
/// The catalogue gives the body of a macro with no `CREATE` in front of it, so
/// the function checks the whole text. Refer to [`ensure_view_body_read_only`].
pub fn ensure_macro_body_read_only(sql: &str) -> Result<(), GuardError> {
    let (scrubbed, _) = scrub(sql)?;
    scan_forbidden(&scrubbed)
}

/// Gives the part of a clean `CREATE ... AS <body>` text after the first `AS`
/// word, or `None` when there is no such word.
fn body_after_as(scrubbed: &str) -> Option<&str> {
    let b: Vec<char> = scrubbed.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if is_ident_char(b[i]) && (i == 0 || !is_ident_char(b[i - 1])) {
            let start = i;
            while i < b.len() && is_ident_char(b[i]) {
                i += 1;
            }
            let word: String = b[start..i].iter().collect();
            if word.eq_ignore_ascii_case("AS") {
                // The byte index of `AS` in the original text equals the
                // character index here, because the scrubber keeps each
                // character outside a value or a comment as one character.
                let byte = scrubbed.char_indices().nth(i).map(|(p, _)| p);
                return byte.map(|p| &scrubbed[p..]);
            }
        } else {
            i += 1;
        }
    }
    None
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
    fn a_carriage_return_ends_a_line_comment() {
        // DuckDB ends a line comment at a carriage return as well as at a line
        // feed. A guard that ended it at the line feed alone would read the
        // part after `\r` as a comment, but DuckDB reads a second statement
        // there. An end-to-end test of this hole wrote a file with COPY.
        assert_eq!(
            ensure_read_only("SELECT 1 -- x\r; DROP TABLE t"),
            Err(GuardError::MultipleStatements)
        );
        assert_eq!(
            ensure_read_only("SELECT 1 -- x\r\n; COPY src TO 'out.csv'"),
            Err(GuardError::MultipleStatements)
        );
        // A line comment with no break after the carriage return is still one
        // query. The carriage return only ends the comment.
        assert!(ensure_read_only("SELECT 1 -- note\r").is_ok());
    }

    #[test]
    fn a_nested_block_comment_hides_nothing() {
        // DuckDB nests block comments, so `/* /* */ ... */` is one comment.
        // A guard that stopped at the first `*/` would read the keyword after
        // it as code for itself but not for DuckDB, or the other way round.
        assert!(ensure_read_only("SELECT 1 /* a /* b */ c */ FROM src").is_ok());
        assert_eq!(
            ensure_read_only("SELECT 1 /* /* */ ' */ ; DROP TABLE t --'"),
            Err(GuardError::MultipleStatements)
        );
        // A nested comment that never closes is refused, not read as code.
        assert_eq!(
            ensure_read_only("SELECT 1 /* /* */ FROM src"),
            Err(GuardError::UnterminatedLiteral)
        );
    }

    #[test]
    fn the_replace_function_is_not_a_write() {
        // `replace()` is a common text function, and `SELECT * REPLACE (...)`
        // is a DuckDB projection. Neither writes. DuckDB has no bare REPLACE
        // statement (REPLACE INTO is MySQL only), and a write through REPLACE
        // always carries CREATE or INSERT, which the guard still refuses.
        assert!(ensure_read_only("SELECT replace(name, 'a', 'b') FROM src").is_ok());
        assert!(ensure_read_only("SELECT * REPLACE (lower(name) AS name) FROM src").is_ok());
        // A write that uses REPLACE always starts with CREATE or INSERT, and
        // the guard still refuses each of those as the first word.
        assert_eq!(
            ensure_read_only("CREATE OR REPLACE TABLE t AS SELECT 1"),
            Err(GuardError::NotAQuery("CREATE".into()))
        );
        assert_eq!(
            ensure_read_only("INSERT OR REPLACE INTO t VALUES (1)"),
            Err(GuardError::NotAQuery("INSERT".into()))
        );
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
    fn a_function_that_writes_or_runs_sql_is_refused() {
        // Gap 1: the logging and profiling functions write a file. A view
        // inside a `.duckdb` file can call them when the file opens.
        for q in [
            "SELECT * FROM enable_logging('x', storage := 'file', storage_path := 'C:/evil.log')",
            "SELECT * FROM enable_profiling('json', save_location := 'C:/evil.json')",
            "SELECT write_log('x', level := 'warn')",
            "SELECT * FROM force_checkpoint()",
            // Gap 2: these run SQL that comes from a text value. The clean-up
            // blanks the value, so the guard never sees what is inside. The
            // only safe answer is to refuse the call itself.
            "SELECT * FROM query('DELETE FROM src')",
            "SELECT * FROM query_table('src')",
            "SELECT * FROM json_execute_serialized_sql('{}')",
        ] {
            assert!(
                matches!(ensure_read_only(q), Err(GuardError::Forbidden(_))),
                "should refuse: {q}"
            );
        }
    }

    #[test]
    fn a_column_or_alias_named_like_a_function_still_passes() {
        // The guard matches a function name only when a `(` follows it. A
        // column named `query` and an alias `enable_logging` are ordinary
        // reads.
        assert!(ensure_read_only("SELECT query FROM src").is_ok());
        assert!(ensure_read_only("SELECT a AS query FROM src").is_ok());
        assert!(ensure_read_only("SELECT \"enable_logging\" FROM src").is_ok());
        assert!(ensure_read_only("SELECT * FROM src WHERE query = 'x'").is_ok());
        // A space between the name and the `(` is still a call.
        assert!(ensure_read_only("SELECT * FROM query ('DROP TABLE t')").is_err());
    }

    #[test]
    fn an_escape_string_scrubs_like_duckdb() {
        // Gap 3: a backslash in an `E'...'` string escapes the next character.
        // `E'\''` is one value that holds a quotation mark. A guard that read
        // the `\'` as the end of the value would then read the rest of the
        // input wrong, and a `;` could hide from the guard.
        assert!(ensure_read_only(r"SELECT E'\''  FROM src").is_ok());
        // A statement break that an escape string tries to hide is still seen.
        assert_eq!(
            ensure_read_only(r"SELECT E'\'; DROP TABLE t --' , 1; DROP TABLE t"),
            Err(GuardError::MultipleStatements)
        );
        // A lower-case `e` prefix works the same way.
        assert!(ensure_read_only(r"SELECT e'a\\b' FROM src").is_ok());
        // An `E` that is the end of an identifier is not a string prefix.
        assert!(ensure_read_only("SELECT codeE FROM src WHERE codeE = 1").is_ok());
    }

    #[test]
    fn a_dollar_quote_with_a_tag_scrubs_like_duckdb() {
        // A tagged dollar quotation hides a `;` from the guard only if the
        // guard does not read the tag. DuckDB reads `$t$ ... $t$` as one value.
        assert!(ensure_read_only("SELECT $tag$ a ; b $tag$ FROM src").is_ok());
        assert_eq!(
            ensure_read_only("SELECT $t$x$t$; DROP TABLE t"),
            Err(GuardError::MultipleStatements)
        );
        // The empty tag `$$` still works.
        assert!(ensure_read_only("SELECT $$ a ; b $$ FROM src").is_ok());
        // A `$` after an identifier, and a parameter `$1`, are ordinary text,
        // not the start of a value. A `;` after them is a real separator.
        assert_eq!(
            ensure_read_only("SELECT a$1; DROP TABLE t"),
            Err(GuardError::MultipleStatements)
        );
    }

    #[test]
    fn every_catalogue_function_is_classified() {
        // The guard refuses a function that writes a file, changes the state,
        // or runs SQL from a value. This test reads the catalogue of DuckDB and
        // fails when a later version adds a table function, a table macro or a
        // pragma that this module does not classify. A new such function is
        // then a review item, and not a silent hole.
        //
        // A pragma reaches the engine only through the words `PRAGMA` or
        // `CALL`, which the guard refuses already, so a pragma that writes is
        // covered even when it is not in `FORBIDDEN_FUNCTIONS`. A table
        // function or a table macro can run with no such word, so each
        // dangerous one must be in `FORBIDDEN_FUNCTIONS`.
        let conn = duckdb::Connection::open_in_memory().unwrap();
        let classified: std::collections::HashSet<&str> = FORBIDDEN_FUNCTIONS
            .iter()
            .copied()
            .chain(REVIEWED_SAFE.iter().copied())
            .collect();

        let mut unknown: Vec<String> = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT function_name FROM duckdb_functions() \
                 WHERE function_type IN ('table', 'table_macro') \
                 UNION SELECT name FROM duckdb_settings() WHERE false \
                 ORDER BY 1",
            )
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            let name: String = r.get(0).unwrap();
            if !classified.contains(name.as_str()) {
                unknown.push(name);
            }
        }
        // The pragmas, from the catalogue of pragmas.
        let mut stmt = conn
            .prepare("SELECT DISTINCT function_name FROM duckdb_functions() WHERE function_type = 'pragma' ORDER BY 1")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            let name: String = r.get(0).unwrap();
            if !classified.contains(name.as_str()) {
                unknown.push(name);
            }
        }
        assert!(
            unknown.is_empty(),
            "DuckDB has table functions, table macros or pragmas that this guard does not \
             classify. Add each to FORBIDDEN_FUNCTIONS (when it writes a file, changes the \
             state, runs SQL from a value, or reaches the network) or to REVIEWED_SAFE:\n{unknown:?}"
        );
    }

    /// Each table function, table macro and pragma of the bundled DuckDB that
    /// the guard does not refuse. A function here was read and found to be a
    /// read, or to need the word `PRAGMA` or `CALL` that the guard refuses
    /// already. The test `every_catalogue_function_is_classified` fails when a
    /// later version of DuckDB adds a name that is in neither list.
    const REVIEWED_SAFE: &[&str] = &[
        // Table functions that read, describe or generate, and write nothing.
        "arrow_scan",
        "arrow_scan_dumb",
        "duckdb_approx_database_count",
        "duckdb_columns",
        "duckdb_connection_count",
        "duckdb_constraints",
        "duckdb_coordinate_systems",
        "duckdb_databases",
        "duckdb_dependencies",
        "duckdb_extensions",
        "duckdb_external_file_cache",
        "duckdb_functions",
        "duckdb_indexes",
        "duckdb_keywords",
        "duckdb_log_contexts",
        "duckdb_logs",
        "duckdb_memory",
        "duckdb_optimizers",
        "duckdb_prepared_statements",
        "duckdb_schemas",
        "duckdb_secret_types",
        "duckdb_secrets",
        "duckdb_sequences",
        "duckdb_settings",
        "duckdb_table_sample",
        "duckdb_tables",
        "duckdb_temporary_files",
        "duckdb_types",
        "duckdb_variables",
        "duckdb_views",
        "generate_series",
        "glob",
        "json_each",
        "json_tree",
        "parquet_bloom_probe",
        "parquet_file_metadata",
        "parquet_full_metadata",
        "parquet_kv_metadata",
        "parquet_metadata",
        "parquet_scan",
        "parquet_schema",
        "pragma_collations",
        "pragma_database_size",
        "pragma_metadata_info",
        "pragma_platform",
        "pragma_show",
        "pragma_storage_info",
        "pragma_table_info",
        "pragma_user_agent",
        "pragma_version",
        "range",
        // `read_*` read a file. The connection lock of `engine::lock_down`
        // keeps each read to the files that the user opened, so a read of
        // another file fails there. A read is not a write, so the promise of
        // "no change to the data" does not need the guard to refuse these.
        "read_blob",
        "read_csv",
        "read_csv_auto",
        "read_duckdb",
        "read_json",
        "read_json_auto",
        "read_json_objects",
        "read_json_objects_auto",
        "read_ndjson",
        "read_ndjson_auto",
        "read_ndjson_objects",
        "read_parquet",
        "read_text",
        "repeat",
        "repeat_row",
        "seq_scan",
        "sniff_csv",
        "summary",
        "test_all_types",
        "test_vector_types",
        "unnest",
        "which_secret",
        // Table macros.
        "duckdb_logs_parsed",
        "duckdb_profiling_settings",
        "histogram",
        "histogram_values",
        // Pragmas. Each reaches the engine only through `PRAGMA` or `CALL`,
        // and the guard refuses both of those words already. The ones that
        // write (`copy_database`, `import_database`, `force_checkpoint`, the
        // profiling pragmas) are in `FORBIDDEN_FUNCTIONS` as well, in case a
        // later DuckDB makes them callable as a table function.
        "add_parquet_key",
        "all_profiling_output",
        "collations",
        "database_list",
        "database_size",
        "disable_checkpoint_on_shutdown",
        "disable_object_cache",
        "disable_optimizer",
        "disable_print_progress_bar",
        "disable_profile",
        "disable_progress_bar",
        "disable_verification",
        "disable_verify_external",
        "disable_verify_fetch_row",
        "disable_verify_parallelism",
        "disable_verify_serializer",
        "enable_checkpoint_on_shutdown",
        "enable_object_cache",
        "enable_optimizer",
        "enable_print_progress_bar",
        "enable_profile",
        "enable_progress_bar",
        "enable_verification",
        "extension_versions",
        "functions",
        "metadata_info",
        "platform",
        "show",
        "show_databases",
        "show_tables",
        "show_tables_expanded",
        "storage_info",
        "table_info",
        "user_agent",
        "verify_external",
        "verify_fetch_row",
        "verify_parallelism",
        "verify_serializer",
        "version",
    ];

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
