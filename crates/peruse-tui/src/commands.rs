//! One table that holds each command, its keys and its description.
//!
//! Three parts of Peruse read the table [`BINDINGS`]:
//!
//! * the code that finds the command for a key
//! * the help overlay that the key `?` opens
//! * the command palette
//!
//! Peruse can therefore have many commands and stay easy to use. Each command
//! that a key starts is also in the help, and the user can also start it by
//! name from the palette. No command is behind a key that the user must know
//! before.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One command of Peruse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    // The commands that move the cursor.
    RowDown,
    RowUp,
    PageDown,
    PageUp,
    Top,
    Bottom,
    ColRight,
    ColLeft,
    ColFirst,
    ColLast,
    GotoRow,
    // The commands that change the view.
    SortCycle,
    SortClear,
    FilterBuild,
    Filter,
    FilterThisValue,
    FilterExcludeValue,
    FilterClear,
    Sql,
    Undo,
    Redo,
    ResetView,
    Search,
    SearchNext,
    SearchPrev,
    // The commands that open a panel or an overlay.
    ToggleMeta,
    ToggleStats,
    CyclePanels,
    InspectCell,
    Record,
    // The commands that change the columns.
    Widen,
    Narrow,
    FitWidths,
    HideColumn,
    ShowAllColumns,
    // The other commands.
    CopyCell,
    CopyRow,
    IndexCsv,
    ThemeNext,
    ThemePicker,
    Settings,
    Help,
    Palette,
    Cancel,
    Quit,
}

/// One command, with its keys and its text.
pub struct Binding {
    /// The command.
    pub cmd: Cmd,
    /// The keys that start the command.
    pub keys: &'static [(KeyCode, KeyModifiers)],
    /// The keys, as the help and the footer write them.
    pub label: &'static str,
    /// The description of the command.
    pub desc: &'static str,
    /// The group of the command in the help.
    pub group: &'static str,
}

/// A key with no modifier key.
const N: KeyModifiers = KeyModifiers::NONE;
/// A key with the control key.
const CTRL: KeyModifiers = KeyModifiers::CONTROL;

/// The one table of commands, keys and descriptions.
pub static BINDINGS: &[Binding] = &[
    Binding {
        cmd: Cmd::RowDown,
        keys: &[(KeyCode::Char('j'), N), (KeyCode::Down, N)],
        label: "j / ↓",
        desc: "next row",
        group: "Move",
    },
    Binding {
        cmd: Cmd::RowUp,
        keys: &[(KeyCode::Char('k'), N), (KeyCode::Up, N)],
        label: "k / ↑",
        desc: "previous row",
        group: "Move",
    },
    Binding {
        cmd: Cmd::PageDown,
        keys: &[(KeyCode::PageDown, N), (KeyCode::Char('f'), CTRL)],
        label: "PgDn / ^F",
        desc: "page down",
        group: "Move",
    },
    Binding {
        cmd: Cmd::PageUp,
        keys: &[(KeyCode::PageUp, N), (KeyCode::Char('b'), CTRL)],
        label: "PgUp / ^B",
        desc: "page up",
        group: "Move",
    },
    Binding {
        cmd: Cmd::Top,
        keys: &[(KeyCode::Char('g'), N), (KeyCode::Home, N)],
        label: "g / Home",
        desc: "first row",
        group: "Move",
    },
    Binding {
        cmd: Cmd::Bottom,
        keys: &[(KeyCode::Char('G'), N), (KeyCode::End, N)],
        label: "G / End",
        desc: "last row",
        group: "Move",
    },
    Binding {
        cmd: Cmd::ColRight,
        keys: &[(KeyCode::Char('l'), N), (KeyCode::Right, N), (KeyCode::Tab, N)],
        label: "l / → / Tab",
        desc: "next column",
        group: "Move",
    },
    Binding {
        cmd: Cmd::ColLeft,
        keys: &[(KeyCode::Char('h'), N), (KeyCode::Left, N), (KeyCode::BackTab, N)],
        label: "h / ← ",
        desc: "previous column",
        group: "Move",
    },
    Binding {
        cmd: Cmd::ColFirst,
        keys: &[(KeyCode::Char('^'), N)],
        label: "^",
        desc: "first column",
        group: "Move",
    },
    Binding {
        cmd: Cmd::ColLast,
        keys: &[(KeyCode::Char('$'), N)],
        label: "$",
        desc: "last column",
        group: "Move",
    },
    Binding {
        cmd: Cmd::GotoRow,
        keys: &[(KeyCode::Char('#'), N)],
        label: "#",
        desc: "jump to row number",
        group: "Move",
    },
    Binding {
        cmd: Cmd::SortCycle,
        keys: &[(KeyCode::Char('s'), N)],
        label: "s",
        desc: "sort by this column (asc → desc → off)",
        group: "Query",
    },
    Binding {
        cmd: Cmd::SortClear,
        keys: &[(KeyCode::Char('S'), N)],
        label: "S",
        desc: "clear all sorting",
        group: "Query",
    },
    Binding {
        cmd: Cmd::FilterBuild,
        keys: &[(KeyCode::Char('f'), N)],
        label: "f",
        desc: "build a filter from menus (no SQL needed)",
        group: "Query",
    },
    Binding {
        cmd: Cmd::Filter,
        keys: &[(KeyCode::Char('E'), N)],
        label: "E",
        desc: "filter rows with a WHERE expression",
        group: "Query",
    },
    Binding {
        cmd: Cmd::FilterThisValue,
        keys: &[(KeyCode::Char('='), N)],
        label: "=",
        desc: "keep only the rows with the value in this cell",
        group: "Query",
    },
    Binding {
        cmd: Cmd::FilterExcludeValue,
        keys: &[(KeyCode::Char('!'), N)],
        label: "!",
        desc: "remove the rows with the value in this cell",
        group: "Query",
    },
    Binding {
        cmd: Cmd::FilterClear,
        keys: &[(KeyCode::Char('F'), N)],
        label: "F",
        desc: "clear the filter",
        group: "Query",
    },
    Binding {
        cmd: Cmd::Sql,
        keys: &[(KeyCode::Char('e'), N)],
        label: "e",
        desc: "edit the SQL query behind the grid",
        group: "Query",
    },
    Binding {
        cmd: Cmd::Undo,
        keys: &[(KeyCode::Char('u'), N)],
        label: "u",
        desc: "undo the last filter, sort or query",
        group: "Query",
    },
    Binding {
        cmd: Cmd::Redo,
        keys: &[(KeyCode::Char('U'), N)],
        label: "U",
        desc: "redo the change that u undid",
        group: "Query",
    },
    Binding {
        cmd: Cmd::ResetView,
        keys: &[(KeyCode::Char('R'), N)],
        label: "R",
        desc: "reset to the whole file",
        group: "Query",
    },
    Binding {
        cmd: Cmd::Search,
        keys: &[(KeyCode::Char('/'), N)],
        label: "/",
        desc: "search all columns",
        group: "Query",
    },
    Binding {
        cmd: Cmd::SearchNext,
        keys: &[(KeyCode::Char('n'), N)],
        label: "n",
        desc: "next match",
        group: "Query",
    },
    Binding {
        cmd: Cmd::SearchPrev,
        keys: &[(KeyCode::Char('N'), N)],
        label: "N",
        desc: "previous match",
        group: "Query",
    },
    Binding {
        cmd: Cmd::ToggleMeta,
        keys: &[(KeyCode::Char('m'), N)],
        label: "m",
        desc: "file metadata panel",
        group: "Inspect",
    },
    Binding {
        cmd: Cmd::ToggleStats,
        keys: &[(KeyCode::Char('i'), N)],
        label: "i",
        desc: "statistics for this column",
        group: "Inspect",
    },
    Binding {
        cmd: Cmd::CyclePanels,
        keys: &[(KeyCode::Char('M'), N)],
        label: "M",
        desc: "cycle the side panels: none, metadata, statistics, both",
        group: "Inspect",
    },
    Binding {
        cmd: Cmd::InspectCell,
        keys: &[(KeyCode::Enter, N)],
        label: "Enter",
        desc: "show this cell in full",
        group: "Inspect",
    },
    Binding {
        cmd: Cmd::Record,
        keys: &[(KeyCode::Char('r'), N)],
        label: "r",
        desc: "show this row as a vertical record, one column per line",
        group: "Inspect",
    },
    Binding {
        cmd: Cmd::Widen,
        keys: &[(KeyCode::Char('>'), N)],
        label: ">",
        desc: "widen this column",
        group: "Columns",
    },
    Binding {
        cmd: Cmd::Narrow,
        keys: &[(KeyCode::Char('<'), N)],
        label: "<",
        desc: "narrow this column",
        group: "Columns",
    },
    Binding {
        cmd: Cmd::FitWidths,
        keys: &[(KeyCode::Char('w'), N)],
        label: "w",
        desc: "re-fit all column widths to what is on screen",
        group: "Columns",
    },
    Binding {
        cmd: Cmd::HideColumn,
        keys: &[(KeyCode::Char('x'), N)],
        label: "x",
        desc: "hide this column",
        group: "Columns",
    },
    Binding {
        cmd: Cmd::ShowAllColumns,
        keys: &[(KeyCode::Char('X'), N)],
        label: "X",
        desc: "show all hidden columns",
        group: "Columns",
    },
    Binding {
        cmd: Cmd::CopyCell,
        keys: &[(KeyCode::Char('y'), N)],
        label: "y",
        desc: "copy this cell to the clipboard",
        group: "Other",
    },
    Binding {
        cmd: Cmd::CopyRow,
        keys: &[(KeyCode::Char('Y'), N)],
        label: "Y",
        desc: "copy this row as TSV",
        group: "Other",
    },
    Binding {
        cmd: Cmd::IndexCsv,
        keys: &[(KeyCode::Char('I'), N)],
        label: "I",
        desc: "index this CSV now (makes jumping instant)",
        group: "Other",
    },
    Binding {
        cmd: Cmd::ThemeNext,
        keys: &[(KeyCode::Char('t'), N)],
        label: "t",
        desc: "next theme",
        group: "Other",
    },
    Binding {
        cmd: Cmd::ThemePicker,
        keys: &[(KeyCode::Char('T'), N)],
        label: "T",
        desc: "choose a theme",
        group: "Other",
    },
    Binding {
        cmd: Cmd::Settings,
        keys: &[(KeyCode::Char(','), N)],
        label: ",",
        desc: "settings, and what this machine gives",
        group: "Other",
    },
    Binding {
        cmd: Cmd::Help,
        keys: &[(KeyCode::Char('?'), N), (KeyCode::F(1), N)],
        label: "?",
        desc: "this help",
        group: "Other",
    },
    Binding {
        cmd: Cmd::Palette,
        keys: &[(KeyCode::Char(':'), N), (KeyCode::Char('p'), CTRL)],
        label: ": / ^P",
        desc: "run a command by name",
        group: "Other",
    },
    Binding {
        cmd: Cmd::Cancel,
        keys: &[(KeyCode::Esc, N)],
        label: "Esc",
        desc: "cancel the running query",
        group: "Other",
    },
    Binding {
        cmd: Cmd::Quit,
        keys: &[(KeyCode::Char('q'), N), (KeyCode::Char('c'), CTRL)],
        label: "q / ^C",
        desc: "quit",
        group: "Other",
    },
];

/// The groups, in the order that the help overlay shows them.
pub const GROUPS: &[&str] = &["Move", "Query", "Inspect", "Columns", "Other"];

/// Removes the shift key from the modifier keys of a character.
///
/// A character that the terminal can print holds its own shift state. The
/// character `G` is the shift key and the key `g`. A test of the shift key
/// would therefore make each binding of a capital letter impossible to match.
fn normalise(key: &KeyEvent) -> KeyModifiers {
    match key.code {
        KeyCode::Char(_) => key.modifiers.difference(KeyModifiers::SHIFT),
        _ => key.modifiers,
    }
}

/// Finds the command for a key. Gives `None` when no command has that key.
pub fn resolve(key: &KeyEvent) -> Option<Cmd> {
    let mods = normalise(key);
    BINDINGS
        .iter()
        .find(|b| {
            b.keys
                .iter()
                .any(|(code, m)| *code == key.code && *m == mods)
        })
        .map(|b| b.cmd)
}

/// Finds the table entry of a command.
pub fn binding(cmd: Cmd) -> Option<&'static Binding> {
    BINDINGS.iter().find(|b| b.cmd == cmd)
}

/// Gives `true` when each character of `needle` occurs in `hay`, in the same
/// order. The characters do not need to follow each other, so `tp` finds
/// "theme picker".
pub fn fuzzy_match(needle: &str, hay: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let hay = hay.to_lowercase();
    let mut chars = hay.chars();
    needle
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|c| chars.any(|h| h == c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn plain_and_arrow_keys_resolve() {
        assert_eq!(resolve(&ev(KeyCode::Char('j'), N)), Some(Cmd::RowDown));
        assert_eq!(resolve(&ev(KeyCode::Down, N)), Some(Cmd::RowDown));
        assert_eq!(resolve(&ev(KeyCode::Char('q'), N)), Some(Cmd::Quit));
        assert_eq!(resolve(&ev(KeyCode::Char('z'), N)), None);
    }

    #[test]
    fn uppercase_bindings_match_with_or_without_the_shift_flag() {
        // Some terminals send the shift key with the capital letter, and
        // some terminals do not. Peruse must accept the two forms.
        assert_eq!(resolve(&ev(KeyCode::Char('G'), N)), Some(Cmd::Bottom));
        assert_eq!(
            resolve(&ev(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            Some(Cmd::Bottom)
        );
        assert_eq!(resolve(&ev(KeyCode::Char('g'), N)), Some(Cmd::Top));
    }

    #[test]
    fn control_chords_are_distinct_from_plain_letters() {
        assert_eq!(resolve(&ev(KeyCode::Char('f'), N)), Some(Cmd::FilterBuild));
        assert_eq!(resolve(&ev(KeyCode::Char('f'), CTRL)), Some(Cmd::PageDown));
        assert_eq!(resolve(&ev(KeyCode::Char('c'), CTRL)), Some(Cmd::Quit));
        assert_eq!(resolve(&ev(KeyCode::Char('c'), N)), None);
    }

    #[test]
    fn every_command_is_discoverable() {
        // The palette and the help come from this table. A command with no
        // entry is therefore available only to a user who knows its key.
        for b in BINDINGS {
            assert!(!b.desc.is_empty(), "{:?} has no description", b.cmd);
            assert!(!b.label.is_empty(), "{:?} has no key label", b.cmd);
            assert!(
                GROUPS.contains(&b.group),
                "{:?} is in unknown group {:?}",
                b.cmd,
                b.group
            );
            assert!(!b.keys.is_empty(), "{:?} has no keys", b.cmd);
        }
    }

    #[test]
    fn no_two_commands_claim_the_same_chord() {
        let mut seen: Vec<(KeyCode, KeyModifiers, Cmd)> = Vec::new();
        for b in BINDINGS {
            for k in b.keys {
                if let Some((_, _, other)) =
                    seen.iter().find(|(c, m, _)| *c == k.0 && *m == k.1)
                {
                    panic!("{:?} and {:?} both bind {:?}", b.cmd, other, k);
                }
                seen.push((k.0, k.1, b.cmd));
            }
        }
    }

    #[test]
    fn fuzzy_matching_is_subsequence_based() {
        assert!(fuzzy_match("tp", "theme picker"));
        assert!(fuzzy_match("", "anything"));
        assert!(fuzzy_match("SORT", "sort by this column"));
        assert!(!fuzzy_match("zzz", "sort by this column"));
        assert!(!fuzzy_match("pt", "theme picker"), "order matters");
    }
}
