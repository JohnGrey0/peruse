//! The text functions that know the width of a character on the screen.
//!
//! Each text that the grid draws comes through this module, for two reasons:
//!
//! * A cell can hold a control character. Such a character can break the
//!   layout of the screen.
//! * A Chinese, Japanese or Korean character uses two screen columns, and an
//!   emoji does the same. The function `str::len` counts bytes, so it is never
//!   the correct measure of a width.

use peruse_core::Align;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Gives the width of a text on the screen, in screen columns.
pub fn width(s: &str) -> usize {
    s.width()
}

/// Makes a value safe to draw on one row.
///
/// The function replaces a line end and a tab with the Unicode character for
/// that control character. It does not remove them. A value with more than one
/// line is therefore visible as such a value. Without this rule, the value
/// looks like one short text.
pub fn sanitize(s: &str) -> String {
    if !s.chars().any(|c| c.is_control()) {
        return s.to_string();
    }
    s.chars()
        .map(|c| match c {
            '\n' => '␊',
            '\r' => '␍',
            '\t' => '␉',
            c if c.is_control() => '·',
            c => c,
        })
        .collect()
}

/// Cuts a text to `max` screen columns. The character `...` shows the cut.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if width(s) <= max {
        return s.to_string();
    }
    // Keep one screen column for the character that shows the cut.
    let budget = max.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// Gives the width of a text on the screen, and stops at `cap` screen columns.
///
/// A caller that compares a width with a limit does not need the full width. A
/// cell can hold 4096 characters, and a comparison with a limit of 60 columns
/// must not walk each of them.
pub fn width_capped(s: &str, cap: usize) -> usize {
    let mut used = 0usize;
    for c in s.chars() {
        used += c.width().unwrap_or(0);
        if used >= cap {
            return used;
        }
    }
    used
}

/// Cuts a text and then adds spaces, until the text has the width `w`.
pub fn fit(s: &str, w: usize, align: Align) -> String {
    let t = truncate(s, w);
    let pad = w.saturating_sub(width(&t));
    match align {
        Align::Left => format!("{t}{}", " ".repeat(pad)),
        Align::Right => format!("{}{t}", " ".repeat(pad)),
    }
}

/// Finds the first occurrence of `needle` in `hay`. The search ignores the
/// case of the letters. The function gives the first byte and the last byte.
pub fn find_ci(hay: &str, needle: &str) -> Option<(usize, usize)> {
    // Compare the characters of `hay` one at a time. Do not make a second text
    // with small letters and then search in it.
    //
    // A change to small letters can change the number of characters. The
    // Turkish capital letter I with a dot above becomes two characters. The
    // positions in the second text are then different from the positions in
    // `hay`. The result then marks the wrong characters.
    let lower: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if lower.is_empty() {
        return None;
    }
    for (start, _) in hay.char_indices() {
        if let Some(end) = match_at(hay, start, &lower) {
            return Some((start, end));
        }
    }
    None
}

/// Compares `needle` with the text at position `start`. The comparison ignores
/// the case of the letters. The function gives the byte after the last byte of
/// the match.
///
/// The start and the end are always at the limit of a character. The caller can
/// therefore cut the text at these positions.
fn match_at(hay: &str, start: usize, needle: &[char]) -> Option<usize> {
    let mut idx = 0usize;
    let mut end = start;
    for hc in hay[start..].chars() {
        for lc in hc.to_lowercase() {
            if idx >= needle.len() {
                break;
            }
            if needle[idx] != lc {
                return None;
            }
            idx += 1;
        }
        end += hc.len_utf8();
        if idx >= needle.len() {
            return Some(end);
        }
    }
    None
}

/// Breaks a text into lines of `w` screen columns. The function breaks the
/// text at a space when it can.
///
/// The cell inspector uses this function. It is the one part of Peruse that
/// shows a long value in full.
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    if w == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for raw_line in s.split('\n') {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut cur = String::new();
        let mut cur_w = 0;
        for word in line.split_inclusive(' ') {
            let ww = width(word);
            if cur_w + ww > w && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            // A word that is longer than one line must break in the
            // middle of the word.
            if ww > w {
                for c in word.chars() {
                    let cw = c.width().unwrap_or(0);
                    if cur_w + cw > w {
                        out.push(std::mem::take(&mut cur));
                        cur_w = 0;
                    }
                    cur.push(c);
                    cur_w += cw;
                }
            } else {
                cur.push_str(word);
                cur_w += ww;
            }
        }
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_glyphs_count_as_two_columns() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("日本"), 4);
    }

    #[test]
    fn control_characters_become_visible() {
        assert_eq!(sanitize("a\nb"), "a␊b");
        assert_eq!(sanitize("a\tb"), "a␉b");
        assert_eq!(sanitize("plain"), "plain");
        assert_eq!(width(&sanitize("a\nb")), 3, "stays one column per char");
    }

    #[test]
    fn truncation_respects_display_width() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello", 4), "hel…");
        assert_eq!(truncate("hello", 0), "");
        // A character of two screen columns must not lose one half.
        let t = truncate("日本語", 4);
        assert!(width(&t) <= 4, "got {t:?} width {}", width(&t));
    }

    #[test]
    fn a_capped_width_agrees_with_the_full_width_below_the_limit() {
        // The caller compares the result with the limit, so the two functions
        // must give the same answer for each text that is inside the limit.
        for s in ["", "a", "hello", "日本語", "naïve café", "a\u{130}b"] {
            let full = width(s);
            assert_eq!(width_capped(s, 100), full, "text {s:?}");
            // At the limit the result is only known to have reached it.
            assert!(width_capped(s, 1) >= full.min(1), "text {s:?}");
        }
    }

    #[test]
    fn a_capped_width_stops_at_the_limit() {
        let long = "x".repeat(4096);
        assert_eq!(width_capped(&long, 60), 60);
        // A character of two screen columns can pass the limit by one column.
        let wide = "日".repeat(100);
        let w = width_capped(&wide, 61);
        assert!((61..=62).contains(&w), "got {w}");
    }

    #[test]
    fn fit_pads_to_exact_width_on_the_right_side() {
        assert_eq!(fit("ab", 5, Align::Left), "ab   ");
        assert_eq!(fit("ab", 5, Align::Right), "   ab");
        assert_eq!(width(&fit("日本語", 5, Align::Left)), 5);
        assert_eq!(width(&fit("toolongvalue", 6, Align::Left)), 6);
    }

    #[test]
    fn case_insensitive_find_returns_original_byte_range() {
        let hay = "Hello World";
        let (s, e) = find_ci(hay, "world").unwrap();
        assert_eq!(&hay[s..e], "World");
        assert_eq!(find_ci(hay, "nope"), None);
        assert_eq!(find_ci(hay, ""), None);
    }

    #[test]
    fn find_ci_is_correct_when_a_letter_changes_length() {
        // The Turkish capital letter I with a dot above becomes two characters
        // in small letters. A search in a copy with small letters therefore
        // gives a position that is one character too large.
        let hay = "Hello \u{130} World";
        let (s, e) = find_ci(hay, "world").unwrap();
        assert_eq!(&hay[s..e], "World");
    }

    #[test]
    fn find_ci_always_gives_character_limits() {
        // The caller cuts the text at these positions. A position inside a
        // character stops the program.
        for hay in ["\u{130}\u{130}World", "a\u{8a9e}b World", "\u{1e9e}World"] {
            if let Some((s, e)) = find_ci(hay, "world") {
                assert!(hay.is_char_boundary(s), "start {s} of {hay:?}");
                assert!(hay.is_char_boundary(e), "end {e} of {hay:?}");
                assert_eq!(hay[s..e].to_lowercase(), "world");
            }
        }
    }

    #[test]
    fn find_ci_handles_multibyte_prefixes() {
        let hay = "日本 target";
        let (s, e) = find_ci(hay, "TARGET").unwrap();
        assert_eq!(&hay[s..e], "target");
    }

    #[test]
    fn wrapping_breaks_on_words_and_keeps_blank_lines() {
        let lines = wrap("the quick brown fox", 10);
        assert!(lines.iter().all(|l| width(l) <= 10), "{lines:?}");
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn wrapping_splits_a_word_longer_than_the_line() {
        let lines = wrap(&"x".repeat(25), 10);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| width(l) <= 10));
        assert_eq!(lines.concat(), "x".repeat(25));
    }
}
