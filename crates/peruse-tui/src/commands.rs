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
    HalfPageDown,
    HalfPageUp,
    StepDown,
    StepUp,
    StepLeft,
    StepRight,
    Top,
    Bottom,
    ColRight,
    ColLeft,
    ColFirst,
    ColLast,
    Origin,
    LastCell,
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
    CycleBand,
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
    // A laptop keyboard does not always have PgUp and PgDn. Each command that
    // moves by more than one row therefore also has a chord that a keyboard
    // with no number pad and no navigation block can reach.
    Binding {
        cmd: Cmd::HalfPageDown,
        keys: &[(KeyCode::Char('d'), CTRL)],
        label: "^D",
        desc: "down half a page",
        group: "Move",
    },
    Binding {
        cmd: Cmd::HalfPageUp,
        keys: &[(KeyCode::Char('u'), CTRL)],
        label: "^U",
        desc: "up half a page",
        group: "Move",
    },
    // The capital letter moves by the step, and the small letter moves by one.
    // The pairs j/J, k/K, h/H and l/L therefore keep the same direction.
    Binding {
        cmd: Cmd::StepDown,
        keys: &[(KeyCode::Char('J'), N)],
        label: "J",
        desc: "down one step of rows",
        group: "Move",
    },
    Binding {
        cmd: Cmd::StepUp,
        keys: &[(KeyCode::Char('K'), N)],
        label: "K",
        desc: "up one step of rows",
        group: "Move",
    },
    Binding {
        cmd: Cmd::StepRight,
        keys: &[(KeyCode::Char('L'), N)],
        label: "L",
        desc: "right one step of columns",
        group: "Move",
    },
    Binding {
        cmd: Cmd::StepLeft,
        keys: &[(KeyCode::Char('H'), N)],
        label: "H",
        desc: "left one step of columns",
        group: "Move",
    },
    Binding {
        cmd: Cmd::Top,
        keys: &[(KeyCode::Char('g'), N)],
        label: "g",
        desc: "first row",
        group: "Move",
    },
    Binding {
        cmd: Cmd::Bottom,
        keys: &[(KeyCode::Char('G'), N)],
        label: "G",
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
        label: "h / ← / S-Tab",
        desc: "previous column",
        group: "Move",
    },
    // The two ends of the record are on the letters `a` and `z`.
    //
    // Many laptop keyboards have no Home key and no End key, so a key for the
    // end of a record must not be one of those two. The characters `^` and `$`
    // are the keys that a text editor uses, but each of them needs the shift
    // key. The letters `a` and `z` are the first letter and the last letter of
    // the alphabet, they need no shift key, and each keyboard has them.
    Binding {
        cmd: Cmd::ColFirst,
        keys: &[
            (KeyCode::Char('a'), N),
            (KeyCode::Char('0'), N),
            (KeyCode::Char('^'), N),
            (KeyCode::Home, N),
        ],
        label: "a / 0 / ^ / Home",
        desc: "first column of this row",
        group: "Move",
    },
    Binding {
        cmd: Cmd::ColLast,
        keys: &[
            (KeyCode::Char('z'), N),
            (KeyCode::Char('$'), N),
            (KeyCode::End, N),
        ],
        label: "z / $ / End",
        desc: "last column of this row",
        group: "Move",
    },
    // One key goes to the very start of the file: the first row and the first
    // column together. A user who is deep inside a large file needs one key to
    // come back, and not two.
    //
    // The letter `o` is for the origin. The capital letter goes to the other
    // corner. A spreadsheet puts the same two commands on ^Home and ^End, so
    // those two chords do the same thing here.
    Binding {
        cmd: Cmd::Origin,
        keys: &[(KeyCode::Char('o'), N), (KeyCode::Home, CTRL)],
        label: "o / ^Home",
        desc: "back to the start: first row and first column",
        group: "Move",
    },
    Binding {
        cmd: Cmd::LastCell,
        keys: &[(KeyCode::Char('O'), N), (KeyCode::End, CTRL)],
        label: "O / ^End",
        desc: "the far corner: last row and last column",
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
    // The letter `d` is for the details. The statistics panel describes one
    // column at the side of the grid, and this band describes every column that
    // is on the screen, under the names.
    Binding {
        cmd: Cmd::CycleBand,
        keys: &[(KeyCode::Char('d'), N)],
        label: "d",
        desc: "column details under the headers: off, compact, detailed",
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
        label: "? / F1",
        desc: "this help",
        group: "Other",
    },
    Binding {
        // The chord ^P is not here. Visual Studio Code takes that chord for
        // its own file finder, and Peruse never sees it in that terminal.
        cmd: Cmd::Palette,
        keys: &[(KeyCode::Char(':'), N), (KeyCode::Char('p'), N)],
        label: ": / p",
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
        assert_eq!(resolve(&ev(KeyCode::Char('v'), N)), None);
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
    fn the_letter_d_cycles_the_detail_band() {
        // The band and the half page both live on the letter `d`. The chord ^D
        // moves the cursor, and the plain letter changes the band.
        assert_eq!(resolve(&ev(KeyCode::Char('d'), N)), Some(Cmd::CycleBand));
        assert_eq!(resolve(&ev(KeyCode::Char('d'), CTRL)), Some(Cmd::HalfPageDown));
    }

    #[test]
    fn control_chords_are_distinct_from_plain_letters() {
        assert_eq!(resolve(&ev(KeyCode::Char('f'), N)), Some(Cmd::FilterBuild));
        assert_eq!(resolve(&ev(KeyCode::Char('f'), CTRL)), Some(Cmd::PageDown));
        assert_eq!(resolve(&ev(KeyCode::Char('c'), CTRL)), Some(Cmd::Quit));
        assert_eq!(resolve(&ev(KeyCode::Char('c'), N)), None);
    }

    #[test]
    fn home_and_end_go_to_the_ends_of_the_record() {
        // A grid puts the ends of the line on these two keys. The two corners
        // of the file need the control key, as in a spreadsheet.
        assert_eq!(resolve(&ev(KeyCode::Home, N)), Some(Cmd::ColFirst));
        assert_eq!(resolve(&ev(KeyCode::End, N)), Some(Cmd::ColLast));
        assert_eq!(resolve(&ev(KeyCode::Home, CTRL)), Some(Cmd::Origin));
        assert_eq!(resolve(&ev(KeyCode::End, CTRL)), Some(Cmd::LastCell));
    }

    /// The commands that a user cannot reach on a keyboard with no navigation
    /// block, when the table gives them no other key.
    ///
    /// Many laptop keyboards have no Home key, no End key, no PgUp key and no
    /// PgDn key. Peruse must stay complete on such a keyboard.
    const NAVIGATION_BLOCK: &[KeyCode] = &[
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::PageDown,
    ];

    #[test]
    fn every_movement_works_on_a_keyboard_with_no_navigation_block() {
        for cmd in [
            Cmd::PageDown,
            Cmd::PageUp,
            Cmd::HalfPageDown,
            Cmd::HalfPageUp,
            Cmd::StepDown,
            Cmd::StepUp,
            Cmd::StepLeft,
            Cmd::StepRight,
            Cmd::Top,
            Cmd::Bottom,
            Cmd::ColFirst,
            Cmd::ColLast,
            Cmd::Origin,
            Cmd::LastCell,
        ] {
            let b = binding(cmd).expect("no entry in the table");
            assert!(
                b.keys
                    .iter()
                    .any(|(code, _)| !NAVIGATION_BLOCK.contains(code)),
                "{cmd:?} has no key outside the navigation block"
            );
        }
    }

    #[test]
    fn the_ends_of_a_row_need_no_shift_key() {
        // The characters ^ and $ both need the shift key. A user who moves to
        // the end of a row all day needs a key that does not.
        assert_eq!(resolve(&ev(KeyCode::Char('a'), N)), Some(Cmd::ColFirst));
        assert_eq!(resolve(&ev(KeyCode::Char('0'), N)), Some(Cmd::ColFirst));
        assert_eq!(resolve(&ev(KeyCode::Char('z'), N)), Some(Cmd::ColLast));
        // The keys of a text editor still work, for a user who knows them.
        assert_eq!(resolve(&ev(KeyCode::Char('^'), N)), Some(Cmd::ColFirst));
        assert_eq!(resolve(&ev(KeyCode::Char('$'), N)), Some(Cmd::ColLast));
    }

    #[test]
    fn one_key_goes_back_to_the_start_of_the_file() {
        // A user who is deep inside a large file needs one key to come back to
        // the first row and the first column together.
        assert_eq!(resolve(&ev(KeyCode::Char('o'), N)), Some(Cmd::Origin));
        assert_eq!(resolve(&ev(KeyCode::Char('O'), N)), Some(Cmd::LastCell));
        assert_eq!(
            resolve(&ev(KeyCode::Char('O'), KeyModifiers::SHIFT)),
            Some(Cmd::LastCell)
        );
    }

    #[test]
    fn the_palette_does_not_use_a_chord_that_an_editor_takes() {
        // Visual Studio Code takes ^P for its own file finder, and Peruse
        // never sees that chord inside its terminal.
        let b = binding(Cmd::Palette).unwrap();
        assert!(
            !b.keys.contains(&(KeyCode::Char('p'), CTRL)),
            "the palette must not use ^P"
        );
        assert_eq!(resolve(&ev(KeyCode::Char('p'), N)), Some(Cmd::Palette));
        assert_eq!(resolve(&ev(KeyCode::Char(':'), N)), Some(Cmd::Palette));
    }

    #[test]
    fn each_label_names_each_key_of_its_command() {
        // The help overlay and the footer both show the label, and the label is
        // the only place that tells the user which keys a command answers to. A
        // label that names fewer keys than the table binds hides a key
        // completely, and the user can only find it by reading the code.
        //
        // The test collects each fault and reports them together. One assertion
        // for each fault would stop at the first one.
        // The separator is a slash with a space at each side, and not a slash
        // alone. The key of the search is the slash character, and its label is
        // therefore that one character.
        let mut bad: Vec<String> = Vec::new();
        for b in BINDINGS {
            let named = b.label.split(" / ").count();
            if named != b.keys.len() {
                bad.push(format!(
                    "{:?} binds {} keys and its label {:?} names {named}",
                    b.cmd,
                    b.keys.len(),
                    b.label
                ));
            }
            if b.label.trim() != b.label {
                bad.push(format!(
                    "{:?} has a space at an end of its label {:?}",
                    b.cmd, b.label
                ));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
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
