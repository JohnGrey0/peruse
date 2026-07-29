//! A small program that finds the parts of a SQL statement, for the colors of
//! the prompt.
//!
//! This module is not a parser. It must be correct only about the start and
//! the end of a value, a comment, a number and a keyword. The user then sees a
//! quotation mark with no partner as soon as the user types it.

/// One kind of part of a SQL statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    /// A word of the SQL language, such as `SELECT`.
    Keyword,
    /// A value between single quotation marks.
    Str,
    /// A number.
    Num,
    /// A comment.
    Comment,
    /// A name, or an identifier between double quotation marks.
    Ident,
    /// A character such as a comma or a bracket.
    Punct,
    /// One space or more.
    Space,
}

/// The words of the SQL language that the prompt shows in the keyword color.
const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "GROUP", "BY", "ORDER", "HAVING", "LIMIT", "OFFSET", "AS", "ON",
    "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "FULL", "CROSS", "USING", "AND", "OR", "NOT", "IN",
    "IS", "NULL", "LIKE", "ILIKE", "BETWEEN", "CASE", "WHEN", "THEN", "ELSE", "END", "DISTINCT",
    "UNION", "ALL", "EXCEPT", "INTERSECT", "WITH", "ASC", "DESC", "CAST", "TRY_CAST", "OVER",
    "PARTITION", "QUALIFY", "WINDOW", "EXISTS", "ANY", "SOME", "VALUES", "DESCRIBE", "SUMMARIZE",
    "EXPLAIN", "PIVOT", "UNPIVOT", "TRUE", "FALSE", "NULLS", "FIRST", "LAST", "ESCAPE", "SIMILAR",
];

/// Gives `true` when the word is a keyword. The test ignores the case of the
/// letters.
fn is_keyword(word: &str) -> bool {
    KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(word))
}

/// Splits a statement into parts, and gives the kind of each part.
///
/// The parts follow each other, and each part gets one color. A join of the
/// parts always gives the input text again.
pub fn tokens(sql: &str) -> Vec<(String, Tok)> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out: Vec<(String, Tok)> = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        let start = i;

        let tok = if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            Tok::Comment
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            Tok::Comment
        } else if c == '\'' || c == '"' {
            let quote = c;
            i += 1;
            while i < chars.len() {
                if chars[i] == quote {
                    // Two quotation marks are one quotation mark in the
                    // value. They do not end the value.
                    if chars.get(i + 1) == Some(&quote) {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            if quote == '"' { Tok::Ident } else { Tok::Str }
        } else if c.is_ascii_digit() {
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                i += 1;
            }
            Tok::Num
        } else if c.is_alphabetic() || c == '_' {
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if is_keyword(&word) { Tok::Keyword } else { Tok::Ident }
        } else if c.is_whitespace() {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            Tok::Space
        } else {
            i += 1;
            Tok::Punct
        };

        out.push((chars[start..i].iter().collect(), tok));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(sql: &str) -> Vec<Tok> {
        tokens(sql).into_iter().map(|(_, t)| t).collect()
    }

    #[test]
    fn segments_always_reconstruct_the_input() {
        for sql in [
            "select * from t where a = 'x' -- note",
            "",
            "  ",
            "/* unterminated",
            "'unterminated",
            "SELECT \"weird col\", 1.5e3 FROM t",
            "naïve",
        ] {
            let joined: String = tokens(sql).into_iter().map(|(s, _)| s).collect();
            assert_eq!(joined, sql, "round trip failed for {sql:?}");
        }
    }

    #[test]
    fn keywords_are_case_insensitive_but_identifiers_are_not_keywords() {
        assert_eq!(kinds("select")[0], Tok::Keyword);
        assert_eq!(kinds("SELECT")[0], Tok::Keyword);
        assert_eq!(kinds("selected")[0], Tok::Ident, "prefix is not a keyword");
        assert_eq!(kinds("amount")[0], Tok::Ident);
    }

    #[test]
    fn strings_and_quoted_identifiers_are_distinguished() {
        assert_eq!(kinds("'text'")[0], Tok::Str);
        assert_eq!(kinds("\"column\"")[0], Tok::Ident);
    }

    #[test]
    fn doubled_quotes_do_not_end_a_literal() {
        let t = tokens("'o''brien' x");
        assert_eq!(t[0].0, "'o''brien'");
        assert_eq!(t[0].1, Tok::Str);
    }

    #[test]
    fn comments_run_to_end_of_line_only() {
        let t = tokens("a -- c\nb");
        assert_eq!(t[2].1, Tok::Comment);
        assert_eq!(t[2].0, "-- c");
        assert_eq!(t.last().unwrap().0, "b");
    }

    #[test]
    fn numbers_absorb_decimals_and_exponents() {
        let t = tokens("1.5e3");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].1, Tok::Num);
    }
}
