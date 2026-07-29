//! An editor of one line. The filter prompt, the SQL prompt, the search prompt
//! and the row-number prompt all use it.
//!
//! Each position in this module counts characters, and not bytes. A user can
//! filter on a value with characters that are not ASCII characters, and a
//! viewer of data sees such values frequently. A position in bytes would then
//! be wrong.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::text;

/// What the editor did with a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The user pressed Enter. The caller must apply the text.
    Submit,
    /// The user pressed Esc. The caller must close the prompt.
    Cancel,
    /// The text or the cursor changed.
    Changed,
    /// The editor did nothing with the key.
    Ignored,
}

/// The text of one line, the cursor, and the history of the prompt.
#[derive(Clone, Debug, Default)]
pub struct LineInput {
    chars: Vec<char>,
    cursor: usize,
    history: Vec<String>,
    /// The position in the history. `None` shows that the user edits a new
    /// line.
    hist_pos: Option<usize>,
    /// The line that the user typed, while the user looks at the history.
    stash: Option<String>,
}

impl LineInput {
    /// Gives the text of the line.
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Gives `true` when the line holds no character.
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Puts a text in the line and moves the cursor to the end.
    pub fn set(&mut self, s: &str) {
        self.chars = s.chars().collect();
        self.cursor = self.chars.len();
        self.hist_pos = None;
        self.stash = None;
    }

    /// Removes each character from the line.
    pub fn clear(&mut self) {
        self.set("");
    }

    /// Gives the width of the text in front of the cursor, in screen columns.
    /// The terminal cursor goes to that position.
    pub fn cursor_col(&self) -> usize {
        let prefix: String = self.chars[..self.cursor].iter().collect();
        text::width(&prefix)
    }

    /// Puts the current line in the history.
    ///
    /// The function does not keep an empty line. If the line is in the history
    /// already, the function moves it to the end of the history.
    pub fn remember(&mut self) {
        let t = self.text();
        if t.trim().is_empty() {
            return;
        }
        self.history.retain(|h| h != &t);
        self.history.push(t);
        // This number is sufficient for one session, and it stops the
        // growth of the history.
        if self.history.len() > 200 {
            self.history.remove(0);
        }
    }


    /// Gives `true` when the cursor is after the last character.
    ///
    /// The ghost completion draws in the room after the cursor. In the middle
    /// of a line there is no such room: the text of the user is there.
    pub fn cursor_at_end(&self) -> bool {
        self.cursor >= self.chars.len()
    }

    /// Gives the name that the user started to type in front of the cursor.
    ///
    /// A name holds letters, numbers and the character `_`. The completion of
    /// a column name uses this function, so the word must stop at each
    /// character that a statement uses, such as `(`, `=` or a space.
    pub fn word_before_cursor(&self) -> String {
        let start = self.name_start();
        self.chars[start..self.cursor].iter().collect()
    }

    /// Replaces the name in front of the cursor and puts the cursor after it.
    pub fn replace_word_before_cursor(&mut self, text: &str) {
        let start = self.name_start();
        self.chars.splice(start..self.cursor, text.chars());
        self.cursor = start + text.chars().count();
        self.hist_pos = None;
    }

    /// Gives the position of the start of the name in front of the cursor.
    ///
    /// A full stop belongs to the name. A value inside a structure has a path
    /// such as `actor.login`, and the completion must see the whole path to
    /// know which fields to offer.
    fn name_start(&self) -> usize {
        let mut i = self.cursor;
        while i > 0
            && (self.chars[i - 1].is_alphanumeric()
                || self.chars[i - 1] == '_'
                || self.chars[i - 1] == '.')
        {
            i -= 1;
        }
        i
    }

    /// Gives the position of the start of the word in front of the cursor.
    fn word_start(&self) -> usize {
        let mut i = self.cursor;
        while i > 0 && self.chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    /// Gives the position of the end of the word after the cursor.
    fn word_end(&self) -> usize {
        let mut i = self.cursor;
        let n = self.chars.len();
        while i < n && self.chars[i].is_whitespace() {
            i += 1;
        }
        while i < n && !self.chars[i].is_whitespace() {
            i += 1;
        }
        i
    }

    /// Moves through the history. The value -1 moves to an older line, and the
    /// value 1 moves to a newer line.
    fn recall(&mut self, delta: i32) -> Action {
        if self.history.is_empty() {
            return Action::Ignored;
        }
        let last = self.history.len() - 1;
        let next = match (self.hist_pos, delta) {
            (None, -1) => {
                self.stash = Some(self.text());
                Some(last)
            }
            (None, _) => return Action::Ignored,
            (Some(0), -1) => Some(0),
            (Some(i), -1) => Some(i - 1),
            (Some(i), _) if i >= last => None,
            (Some(i), _) => Some(i + 1),
        };
        match next {
            Some(i) => {
                let entry = self.history[i].clone();
                self.chars = entry.chars().collect();
                self.cursor = self.chars.len();
                self.hist_pos = Some(i);
            }
            None => {
                // The user moved past the newest line of the history. Put
                // the line of the user back in the prompt.
                let stash = self.stash.take().unwrap_or_default();
                self.chars = stash.chars().collect();
                self.cursor = self.chars.len();
                self.hist_pos = None;
            }
        }
        Action::Changed
    }

    /// Applies one key to the line and gives the result.
    pub fn handle(&mut self, key: &KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Enter => Action::Submit,
            KeyCode::Esc => Action::Cancel,

            KeyCode::Char('c') if ctrl => Action::Cancel,
            KeyCode::Char('a') if ctrl => {
                self.cursor = 0;
                Action::Changed
            }
            KeyCode::Char('e') if ctrl => {
                self.cursor = self.chars.len();
                Action::Changed
            }
            KeyCode::Char('u') if ctrl => {
                self.chars.drain(..self.cursor);
                self.cursor = 0;
                Action::Changed
            }
            KeyCode::Char('k') if ctrl => {
                self.chars.truncate(self.cursor);
                Action::Changed
            }
            KeyCode::Char('w') if ctrl => {
                let start = self.word_start();
                self.chars.drain(start..self.cursor);
                self.cursor = start;
                Action::Changed
            }
            KeyCode::Char('b') if alt => {
                self.cursor = self.word_start();
                Action::Changed
            }
            KeyCode::Char('f') if alt => {
                self.cursor = self.word_end();
                Action::Changed
            }

            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                Action::Changed
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.chars.len());
                Action::Changed
            }
            KeyCode::Home => {
                self.cursor = 0;
                Action::Changed
            }
            KeyCode::End => {
                self.cursor = self.chars.len();
                Action::Changed
            }
            KeyCode::Up => self.recall(-1),
            KeyCode::Down => self.recall(1),

            KeyCode::Backspace => {
                if self.cursor == 0 {
                    return Action::Ignored;
                }
                self.cursor -= 1;
                self.chars.remove(self.cursor);
                Action::Changed
            }
            KeyCode::Delete => {
                if self.cursor >= self.chars.len() {
                    return Action::Ignored;
                }
                self.chars.remove(self.cursor);
                Action::Changed
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
                self.hist_pos = None;
                Action::Changed
            }
            _ => Action::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typed(s: &str) -> LineInput {
        let mut i = LineInput::default();
        for c in s.chars() {
            i.handle(&key(KeyCode::Char(c)));
        }
        i
    }

    #[test]
    fn typing_and_backspace() {
        let mut i = typed("abc");
        assert_eq!(i.text(), "abc");
        assert_eq!(i.handle(&key(KeyCode::Backspace)), Action::Changed);
        assert_eq!(i.text(), "ab");
        i.handle(&key(KeyCode::Home));
        assert_eq!(i.handle(&key(KeyCode::Backspace)), Action::Ignored);
    }

    #[test]
    fn insertion_happens_at_the_cursor_not_the_end() {
        let mut i = typed("ac");
        i.handle(&key(KeyCode::Left));
        i.handle(&key(KeyCode::Char('b')));
        assert_eq!(i.text(), "abc");
    }

    #[test]
    fn cursor_column_uses_display_width() {
        let mut i = typed("日本x");
        assert_eq!(i.cursor_col(), 5);
        i.handle(&key(KeyCode::Home));
        assert_eq!(i.cursor_col(), 0);
    }

    #[test]
    fn multibyte_editing_does_not_panic_or_corrupt() {
        let mut i = typed("naïve café");
        i.handle(&ctrl('w'));
        assert_eq!(i.text(), "naïve ");
        i.handle(&ctrl('u'));
        assert_eq!(i.text(), "");
    }

    #[test]
    fn kill_to_end_and_start() {
        let mut i = typed("hello world");
        i.handle(&key(KeyCode::Left));
        i.handle(&ctrl('k'));
        assert_eq!(i.text(), "hello worl");
        i.handle(&ctrl('a'));
        i.handle(&ctrl('k'));
        assert_eq!(i.text(), "");
    }

    #[test]
    fn word_delete_skips_trailing_spaces() {
        let mut i = typed("select a from   ");
        i.handle(&ctrl('w'));
        assert_eq!(i.text(), "select a ");
    }

    #[test]
    fn submit_and_cancel_are_reported() {
        let mut i = typed("x");
        assert_eq!(i.handle(&key(KeyCode::Enter)), Action::Submit);
        assert_eq!(i.handle(&key(KeyCode::Esc)), Action::Cancel);
        assert_eq!(i.handle(&ctrl('c')), Action::Cancel);
    }

    #[test]
    fn history_walks_back_and_forward_and_restores_the_draft() {
        let mut i = typed("first");
        i.remember();
        i.clear();
        for c in "second".chars() {
            i.handle(&key(KeyCode::Char(c)));
        }
        i.remember();
        i.clear();
        for c in "draft".chars() {
            i.handle(&key(KeyCode::Char(c)));
        }

        i.handle(&key(KeyCode::Up));
        assert_eq!(i.text(), "second");
        i.handle(&key(KeyCode::Up));
        assert_eq!(i.text(), "first");
        i.handle(&key(KeyCode::Up));
        assert_eq!(i.text(), "first", "stops at the oldest entry");
        i.handle(&key(KeyCode::Down));
        assert_eq!(i.text(), "second");
        i.handle(&key(KeyCode::Down));
        assert_eq!(i.text(), "draft", "the unsent line comes back");
    }

    #[test]
    fn re_running_an_entry_moves_it_to_the_front_rather_than_duplicating() {
        let mut i = LineInput::default();
        for entry in ["alpha", "beta", "alpha"] {
            i.set(entry);
            i.remember();
        }
        i.clear();
        i.remember(); // the history does not keep an empty line

        i.handle(&key(KeyCode::Up));
        assert_eq!(i.text(), "alpha", "most recent first");
        i.handle(&key(KeyCode::Up));
        assert_eq!(i.text(), "beta");
        i.handle(&key(KeyCode::Up));
        assert_eq!(i.text(), "beta", "only two entries, not three");
    }

    #[test]
    fn down_with_no_history_browsing_is_ignored() {
        let mut i = typed("x");
        assert_eq!(i.handle(&key(KeyCode::Down)), Action::Ignored);
    }

    #[test]
    fn the_name_in_front_of_the_cursor_stops_at_a_punctuation_character() {
        assert_eq!(typed("amount").word_before_cursor(), "amount");
        assert_eq!(typed("a > am").word_before_cursor(), "am");
        assert_eq!(typed("lower(reg").word_before_cursor(), "reg");
        assert_eq!(typed("a = ").word_before_cursor(), "");
        assert_eq!(typed("").word_before_cursor(), "");
    }

    #[test]
    fn completing_a_name_replaces_only_that_name() {
        let mut i = typed("amount > 1 AND reg");
        i.replace_word_before_cursor("region");
        assert_eq!(i.text(), "amount > 1 AND region");
        assert_eq!(i.cursor_col(), i.text().chars().count());
    }

    #[test]
    fn completing_in_the_middle_of_a_line_keeps_the_end() {
        let mut i = typed("reg = 'EU'");
        for _ in 0..7 {
            i.handle(&key(KeyCode::Left));
        }
        i.replace_word_before_cursor("region");
        assert_eq!(i.text(), "region = 'EU'");
    }

    #[test]
    fn completing_a_name_with_characters_of_more_than_one_byte_is_correct() {
        // A column name can hold a character that is not an ASCII character.
        // Each position in this module counts characters, so the result must
        // stay correct.
        let mut i = typed("café");
        i.replace_word_before_cursor("café_total");
        assert_eq!(i.text(), "café_total");
    }
}
