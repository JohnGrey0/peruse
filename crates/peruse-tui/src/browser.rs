//! The screen that Peruse shows when the user names no file.
//!
//! A user who types `peruse` alone wants to look at some data. The name of the
//! program is the one thing that such a user remembers, and a page of help is
//! not an answer to that wish. This screen lists the data files that are near,
//! and it opens one.
//!
//! The screen holds no engine and no worker. It reads a directory with the
//! functions of the standard library, and it gives one entry back. The program
//! then opens that entry exactly as it opens a path from the command line.
//!
//! The same screen chooses a table of a database. A database holds many tables,
//! and the grid shows one of them, so the user makes that choice in front of the
//! grid in the same way. The two lists share the keys, the find box and the
//! drawing: only the entries differ. Refer to [`Browser::of_database`].

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use peruse_core::config::Config;
use peruse_core::engine::DbTable;
use peruse_core::source::{by_extension, human_bytes, human_count, Format};
use peruse_core::Theme;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;
use ratatui::Frame;

use crate::app::Clicks;
use crate::colors::Depth;
use crate::input::{Action, LineInput, plain};
use crate::paint::Paint;
use crate::text;

/// The lines that one turn of the wheel moves in the chooser.
///
/// A terminal and a text editor both move three lines for one turn. The list
/// therefore moves by the same amount as the other windows of the user.
const WHEEL_LINES: usize = 3;

/// One line of the list: a file, a directory, or a table of a database.
#[derive(Clone, Debug)]
pub struct Entry {
    /// The complete path. For a table, the path of the database file.
    pub path: PathBuf,
    /// The name that the list shows.
    pub name: String,
    /// `true` for a directory.
    pub is_dir: bool,
    /// The size of the file, in bytes.
    pub bytes: u64,
    /// The time of the last change.
    pub modified: Option<SystemTime>,
    /// The format, when the extension gives one.
    pub format: Option<Format>,
    /// `true` when the entry comes from the list of files that the user
    /// opened before, and not from the directory.
    pub is_recent: bool,
    /// The table of a database, when the list shows the tables of one.
    ///
    /// One list holds files or tables, and never both. The value is therefore
    /// `None` for each entry of a directory.
    pub table: Option<DbTable>,
}

/// One line of the list. A heading is a line that the cursor goes past.
#[derive(Clone, Debug)]
enum Row {
    Heading(String),
    Item(Entry),
}

/// The state of the chooser.
pub struct Browser {
    /// The directory that the list shows.
    pub dir: PathBuf,
    /// The files that the user opened before.
    recent: Vec<Entry>,
    /// Each entry of the directory, before the find box removes any.
    entries: Vec<Entry>,
    /// The selected line.
    pub sel: usize,
    /// The text of the find box.
    pub find: LineInput,
    /// `true` while the user types in the find box.
    pub finding: bool,
    /// `true` while the list shows each file, and not the data files only.
    pub all_files: bool,
    /// The message about a directory that Peruse cannot read.
    pub error: Option<String>,
    /// The entry that the user chose.
    pub chosen: Option<Entry>,
    /// `true` after the user asks to leave.
    pub quit: bool,
    /// The name of the database whose tables the list shows.
    ///
    /// The value is `None` for a list of files. With a database, the list holds
    /// the tables of that one database, so the keys that walk the file system
    /// do nothing.
    database: Option<String>,
    /// One pair for each entry on the screen: the row of the terminal, and the
    /// position of that entry in the list.
    ///
    /// Only the code that draws the list knows which entry is at a row of the
    /// terminal. The list scrolls, and it holds headings that the cursor goes
    /// past, so the row on the screen is not the position in the list.
    lines: Vec<(u16, usize)>,
    /// The presses of the left button, for the double click.
    clicks: Clicks,
}

impl Browser {
    /// Makes the chooser, and reads the directory that it starts in.
    pub fn new(dir: PathBuf, config: &Config) -> Browser {
        let recent = config
            .recent
            .iter()
            .map(|r| peruse_core::source::untidy_path(r))
            // A file that the user moved or deleted must not stay on the
            // screen. The list would then offer a file that cannot open.
            .filter(|p| p.is_file())
            .map(|p| entry_of(&p, true))
            .collect();
        let mut b = Browser {
            dir,
            recent,
            entries: Vec::new(),
            sel: 0,
            find: LineInput::default(),
            finding: false,
            all_files: false,
            error: None,
            chosen: None,
            quit: false,
            database: None,
            lines: Vec::new(),
            clicks: Clicks::default(),
        };
        b.read_dir();
        b
    }

    /// Makes the chooser for the tables of one database.
    ///
    /// The caller reads the tables from the database first. That read costs no
    /// scan of the data: the count of the rows comes from the catalog of the
    /// database. A database can hold hundreds of tables, and a `count(*)` on
    /// each of them would read the whole file before the first frame.
    pub fn of_database(path: &Path, tables: &[DbTable]) -> Browser {
        let label = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let entries = tables
            .iter()
            .map(|t| Entry {
                path: path.to_path_buf(),
                name: t.label(),
                is_dir: false,
                bytes: 0,
                modified: None,
                format: Some(Format::DuckDb),
                is_recent: false,
                table: Some(t.clone()),
            })
            .collect();
        Browser {
            dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
            recent: Vec::new(),
            entries,
            sel: 0,
            find: LineInput::default(),
            finding: false,
            all_files: false,
            error: None,
            chosen: None,
            quit: false,
            database: Some(label),
            lines: Vec::new(),
            clicks: Clicks::default(),
        }
    }

    /// Gives the text that the title bar shows at the right of the name of the
    /// program.
    fn where_text(&self) -> String {
        match &self.database {
            Some(name) => format!("{name} · which table?"),
            None => short_path(&self.dir),
        }
    }

    /// Reads the directory into the list.
    fn read_dir(&mut self) {
        self.entries.clear();
        self.error = None;
        let read = match std::fs::read_dir(&self.dir) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(format!("{}: {e}", self.dir.display()));
                return;
            }
        };
        for e in read.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            // A name that starts with a full stop is a file that the system
            // holds for itself. A viewer of data has no use for it.
            if name.starts_with('.') {
                continue;
            }
            self.entries.push(entry_of(&path, false));
            // Keep the name that the directory gave. A path with characters
            // that are not valid gives a different name from `file_name`.
            if let Some(last) = self.entries.last_mut() {
                last.name = name;
            }
        }
        // The directories come first, and each group follows the alphabet.
        // A user looks for a directory by its name, and for a file by its
        // name, so one order for the two would help neither.
        self.entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.sel = 0;
    }

    /// Gives the lines that the screen draws.
    ///
    /// The files that the user opened before come first, and only when the
    /// find box is empty. A user who types a name looks in this directory.
    fn rows(&self) -> Vec<Row> {
        let q = self.find.text().trim().to_lowercase();
        if self.database.is_some() {
            return self.table_rows(&q);
        }
        let mut out = Vec::new();

        if q.is_empty() && !self.recent.is_empty() {
            out.push(Row::Heading("recent".into()));
            for e in &self.recent {
                out.push(Row::Item(e.clone()));
            }
        }

        let keep: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| e.is_dir || self.all_files || e.format.is_some())
            .filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q))
            .collect();

        if !out.is_empty() {
            out.push(Row::Heading(short_path(&self.dir)));
        }
        // The parent directory is one line, so a user needs no key for it.
        if self.dir.parent().is_some() && q.is_empty() {
            out.push(Row::Item(Entry {
                path: self.dir.parent().unwrap().to_path_buf(),
                name: "..".into(),
                is_dir: true,
                bytes: 0,
                modified: None,
                format: None,
                is_recent: false,
                table: None,
            }));
        }
        for e in keep {
            out.push(Row::Item(e.clone()));
        }
        out
    }

    /// Gives the lines for the tables of a database.
    ///
    /// The tables come first and the views follow, and a heading names each
    /// group. A view holds no data of its own, so a user who looks for a table
    /// finds one at the top.
    fn table_rows(&self, q: &str) -> Vec<Row> {
        let mut out = Vec::new();
        for (heading, view) in [("tables", false), ("views", true)] {
            let keep: Vec<&Entry> = self
                .entries
                .iter()
                .filter(|e| e.table.as_ref().is_some_and(|t| t.is_view == view))
                .filter(|e| q.is_empty() || e.name.to_lowercase().contains(q))
                .collect();
            if keep.is_empty() {
                continue;
            }
            out.push(Row::Heading(heading.into()));
            for e in keep {
                out.push(Row::Item(e.clone()));
            }
        }
        out
    }

    /// Gives the entries that the screen draws, with no heading.
    pub fn items(&self) -> Vec<Entry> {
        self.rows()
            .into_iter()
            .filter_map(|r| match r {
                Row::Item(e) => Some(e),
                Row::Heading(_) => None,
            })
            .collect()
    }

    /// Gives the entry under the cursor.
    pub fn selected(&self) -> Option<Entry> {
        self.items().into_iter().nth(self.sel)
    }

    /// Gives the names in the list, for the ghost completion.
    fn names(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.name.clone()).collect()
    }

    /// Gives the part of a name that the user did not type yet.
    pub fn ghost(&self) -> Option<String> {
        if !self.finding || !self.find.cursor_at_end() {
            return None;
        }
        crate::app::ghost_from(&self.find.text(), &self.names())
    }

    /// Moves the cursor.
    fn move_sel(&mut self, delta: i64) {
        let n = self.items().len();
        if n == 0 {
            return;
        }
        self.sel = (self.sel as i64 + delta).clamp(0, n as i64 - 1) as usize;
    }

    /// Goes into a directory, or chooses a file or a table.
    fn open(&mut self) {
        let Some(e) = self.selected() else { return };
        if e.is_dir {
            self.go_to(e.path);
        } else {
            self.chosen = Some(e);
        }
    }

    /// Goes to a directory and reads it.
    fn go_to(&mut self, dir: PathBuf) {
        // Keep the path short and without `..` inside it, so the title bar
        // says where the user is.
        self.dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        self.find.clear();
        self.finding = false;
        self.read_dir();
    }

    /// Goes to the directory that holds this one.
    fn go_up(&mut self) {
        if let Some(p) = self.dir.parent().map(Path::to_path_buf) {
            self.go_to(p);
        }
    }

    /// Applies one key.
    pub fn on_key(&mut self, key: &KeyEvent) {
        if self.finding {
            // The key Tab takes the name that the ghost shows. Ctrl+Right and
            // Alt+Right move one word, so those two forms go to the editor.
            if key.code == KeyCode::Tab
                || (key.code == KeyCode::Right && plain(key) && self.ghost().is_some())
            {
                if let Some(rest) = self.ghost() {
                    let full = format!("{}{rest}", self.find.text());
                    self.find.set(&full);
                }
                return;
            }
            match self.find.handle(key) {
                Action::Cancel => {
                    self.finding = false;
                    self.find.clear();
                    self.sel = 0;
                }
                // Enter leaves the find box and keeps the text. The list is
                // short now, and the keys j and k move through it.
                Action::Submit => self.finding = false,
                Action::Changed => self.sel = 0,
                Action::Ignored => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                // The find box holds a name. The first Esc clears it.
                if self.find.is_empty() {
                    self.quit = true;
                } else {
                    self.find.clear();
                    self.sel = 0;
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::PageDown => self.move_sel(10),
            KeyCode::PageUp => self.move_sel(-10),
            KeyCode::Char('g') | KeyCode::Home => self.sel = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.sel = self.items().len().saturating_sub(1);
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.open(),
            // The three keys with a condition walk the file system. The list of
            // the tables of one database has nowhere to walk to, so each of
            // them does nothing there.
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace
                if self.database.is_none() =>
            {
                self.go_up()
            }
            KeyCode::Char('~') if self.database.is_none() => {
                if let Some(h) = peruse_core::dirs::home_dir() {
                    self.go_to(h);
                }
            }
            KeyCode::Char('/') => {
                self.finding = true;
                self.find.clear();
            }
            // Show each file, and not the data files only. A file with an
            // extension that Peruse does not know can still hold CSV.
            KeyCode::Char('a') if self.database.is_none() => {
                self.all_files = !self.all_files;
                self.sel = 0;
            }
            _ => {}
        }
    }

    /// Gives the position in the list of the entry at a row of the terminal.
    ///
    /// The result is `None` for the title bar, for the find box, for a heading
    /// and for the row of keys.
    fn line_at(&self, y: u16) -> Option<usize> {
        self.lines
            .iter()
            .find(|(ly, _)| *ly == y)
            .map(|(_, at)| *at)
    }

    /// Applies one mouse event.
    ///
    /// A click becomes the key that does the same thing, so the mouse and the
    /// keys can never disagree.
    ///
    /// The function gives `true` when the event changed something. The terminal
    /// reports each movement of the pointer while the mouse is on, and the
    /// chooser does nothing with those events. The caller draws no frame for
    /// them.
    pub fn on_mouse(&mut self, ev: &MouseEvent) -> bool {
        // The wheel moves the cursor of the list, and it never goes through the
        // key that does the same. After the key `/` the find box holds the
        // focus, and an arrow key belongs to the text there: it puts an older
        // line of the history in the box. A turn of the wheel must never write
        // in a box, so it moves the list itself.
        let step = match ev.kind {
            MouseEventKind::ScrollDown => Some(WHEEL_LINES as i64),
            MouseEventKind::ScrollUp => Some(-(WHEEL_LINES as i64)),
            _ => None,
        };
        if let Some(step) = step {
            // The wheel at the end of the list moves nothing, so it needs no
            // frame either.
            let before = self.sel;
            self.move_sel(step);
            return self.sel != before;
        }
        if ev.kind != MouseEventKind::Down(MouseButton::Left) {
            return false;
        }
        let double = self.clicks.press_now(ev.column, ev.row);
        let Some(at) = self.line_at(ev.row) else {
            return false;
        };
        // A double click opens the entry, as Enter does: it goes into a
        // directory, and it chooses a file or a table. One click selects only,
        // so a click that lands on the wrong line costs nothing.
        //
        // The second press acts on the entry that the first press chose, and
        // never on `at`. A new selection moves the window of the list, so the
        // frame between the two presses can put another entry under the
        // pointer. The two presses of a pair are at one position, so the entry
        // that the first press chose is the entry that the user means.
        if double {
            self.open();
            return true;
        }
        if at >= self.items().len() {
            return false;
        }
        // The pointer names an entry, so the find box gives up the focus. The
        // key Enter does the same in the box, and it keeps the text.
        let changed = self.sel != at || self.finding;
        self.finding = false;
        self.sel = at;
        changed
    }
}

/// Reads what the screen shows about one path.
fn entry_of(path: &Path, is_recent: bool) -> Entry {
    let meta = std::fs::metadata(path).ok();
    Entry {
        name: path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
        is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
        bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        modified: meta.as_ref().and_then(|m| m.modified().ok()),
        format: by_extension(path).map(|(f, _)| f),
        path: path.to_path_buf(),
        is_recent,
        table: None,
    }
}

/// Writes a path in a short form, with the home directory as `~`.
fn short_path(p: &Path) -> String {
    let full = p.to_string_lossy().replace('\\', "/");
    if let Some(home) = peruse_core::dirs::home_dir() {
        let h = home.to_string_lossy().replace('\\', "/");
        if let Some(rest) = full.strip_prefix(&h) {
            return format!("~{rest}");
        }
    }
    full
}

/// Writes how long ago a time was, in a few characters.
fn ago(t: SystemTime) -> String {
    let Ok(d) = SystemTime::now().duration_since(t) else {
        return String::new();
    };
    let s = d.as_secs();
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        86400..=2591999 => format!("{}d ago", s / 86400),
        _ => format!("{}mo ago", s / 2592000),
    }
}

/// Draws one frame of the chooser.
///
/// The function writes where each entry of the list is on the screen. Only this
/// code knows that, and the mouse needs it.
pub fn draw(f: &mut Frame, b: &mut Browser, theme: &Theme, depth: Depth) -> Option<Position> {
    let p = Paint::new(depth);
    let area = f.area();
    let buf = f.buffer_mut();
    let t = theme;
    // A screen with no list holds no entry, so no click can land on one.
    // Without this, a click would use the positions of an older frame.
    b.lines.clear();
    if area.width < 20 || area.height < 4 {
        return None;
    }

    // The whole screen takes the background of the theme.
    for y in area.top()..area.bottom() {
        buf.set_stringn(
            area.x,
            y,
            " ".repeat(area.width as usize),
            area.width as usize,
            p.on(t.fg, t.bg),
        );
    }

    // The title bar: the name of the program, and where the user is.
    buf.set_stringn(
        area.x,
        area.y,
        " ".repeat(area.width as usize),
        area.width as usize,
        p.on(t.fg, t.bg_alt),
    );
    buf.set_stringn(area.x + 1, area.y, "peruse", 6, p.bold(p.on(t.accent, t.bg_alt)));
    let where_now = b.where_text();
    let w = (area.width as usize).saturating_sub(10);
    buf.set_stringn(
        area.x + 9,
        area.y,
        text::truncate(&where_now, w),
        w,
        p.on(t.fg, t.bg_alt),
    );

    let mut cursor = None;
    let mut y = area.y + 2;

    // The find box, when the user opened it.
    if b.finding || !b.find.is_empty() {
        let label = "find › ";
        let lw = text::width(label);
        buf.set_stringn(area.x + 1, y, label, lw, p.bold(p.on(t.accent, t.bg)));
        let vx = area.x + 1 + lw as u16;
        let vw = (area.right().saturating_sub(vx)) as usize;
        buf.set_stringn(
            vx,
            y,
            text::truncate(&b.find.text(), vw),
            vw,
            p.on(t.fg, t.bg),
        );
        if b.finding {
            let cx = (vx + b.find.cursor_col() as u16).min(area.right() - 1);
            cursor = Some(Position::new(cx, y));
            if let Some(rest) = b.ghost()
                && cx < area.right()
            {
                let gw = (area.right() - cx) as usize;
                buf.set_stringn(cx, y, text::truncate(&rest, gw), gw, p.on(t.dim, t.bg));
            }
        }
        y += 2;
    }

    if let Some(e) = &b.error {
        buf.set_stringn(
            area.x + 1,
            y,
            text::truncate(e, area.width.saturating_sub(2) as usize),
            area.width.saturating_sub(2) as usize,
            p.on(t.error, t.bg),
        );
        y += 2;
    }

    // The list. The heading rows are not lines that the cursor can reach, so
    // the position of the cursor counts the entries only.
    let rows = b.rows();
    let list_h = area.bottom().saturating_sub(y + 2) as usize;
    let items = rows.iter().filter(|r| matches!(r, Row::Item(_))).count();
    if items == 0 {
        let msg = if !b.find.is_empty() {
            "no name holds that text"
        } else if b.database.is_some() {
            "this database holds no table"
        } else {
            "no data file here. Press a to show every file, or h to go up."
        };
        buf.set_stringn(
            area.x + 1,
            y,
            text::truncate(msg, area.width.saturating_sub(2) as usize),
            area.width.saturating_sub(2) as usize,
            p.on(t.dim, t.bg),
        );
    }

    // Find the row of the selected entry, and scroll to keep it on the
    // screen. The heading rows move with the entries below them.
    let sel = b.sel.min(items.saturating_sub(1));
    let mut sel_row = 0;
    let mut at = 0;
    for (i, r) in rows.iter().enumerate() {
        if let Row::Item(_) = r {
            if at == sel {
                sel_row = i;
                break;
            }
            at += 1;
        }
    }
    let start = sel_row.saturating_sub(list_h.saturating_sub(1));

    let mut item_at = rows
        .iter()
        .take(start)
        .filter(|r| matches!(r, Row::Item(_)))
        .count();
    for r in rows.iter().skip(start).take(list_h) {
        if y >= area.bottom().saturating_sub(2) {
            break;
        }
        match r {
            Row::Heading(h) => {
                buf.set_stringn(
                    area.x + 1,
                    y,
                    text::truncate(h, area.width.saturating_sub(2) as usize),
                    area.width.saturating_sub(2) as usize,
                    p.bold(p.on(t.accent, t.bg)),
                );
            }
            Row::Item(e) => {
                let focused = item_at == sel;
                // Keep where this entry is, for the mouse. A heading is not a
                // line that the cursor can reach, so it gets no entry here.
                b.lines.push((y, item_at));
                item_at += 1;
                let bg = if focused { t.cursor_row } else { t.bg };
                buf.set_stringn(
                    area.x,
                    y,
                    " ".repeat(area.width as usize),
                    area.width as usize,
                    p.bg(bg),
                );

                // A directory ends with a slash, so the eye tells the two
                // apart with no color.
                let label = if e.is_dir {
                    format!("{}/", e.name)
                } else {
                    e.name.clone()
                };
                // A file that the user opened before shows its directory
                // too. The name by itself would not say which file it is.
                let label = if e.is_recent {
                    let parent = e
                        .path
                        .parent()
                        .map(short_path)
                        .unwrap_or_default();
                    format!("{label}   {parent}")
                } else {
                    label
                };

                let colour = if e.is_dir {
                    t.accent
                } else if e.format.is_some() {
                    t.fg
                } else {
                    t.dim
                };
                let style = if focused {
                    p.bold(p.on(colour, bg))
                } else {
                    p.on(colour, bg)
                };
                // The right block holds the format, the size and the time.
                // It needs 30 screen columns, and the two spaces at the left
                // edge and the one at the right edge need three more.
                let name_w = (area.width as usize).saturating_sub(34).max(8);
                buf.set_stringn(
                    area.x + 2,
                    y,
                    text::fit(&label, name_w, peruse_core::Align::Left),
                    name_w,
                    style,
                );

                if !e.is_dir {
                    let right = match &e.table {
                        // A table of a database. The word says which entries
                        // are views, and the count of the rows comes from the
                        // catalog, so the list costs no scan. The mark ~ says
                        // that the database estimated the count.
                        Some(tbl) => {
                            let kind = if tbl.is_view { "view" } else { "table" };
                            let count = tbl
                                .rows
                                .map(|n| format!("~{} rows", human_count(n)))
                                .unwrap_or_default();
                            format!("{kind:>8}  {count:>20}")
                        }
                        None => {
                            let fmt = e.format.map(|f| f.to_string()).unwrap_or_default();
                            let size = human_bytes(e.bytes);
                            let when = e.modified.map(ago).unwrap_or_default();
                            format!("{fmt:>8}  {size:>9}  {when:>9}")
                        }
                    };
                    let rx = area.x + 2 + name_w as u16;
                    let rw = (area.right().saturating_sub(rx + 1)) as usize;
                    buf.set_stringn(
                        rx,
                        y,
                        text::truncate(&right, rw),
                        rw,
                        p.on(t.dim, bg),
                    );
                }
            }
        }
        y += 1;
    }

    // The keys, along the last row.
    let hints = if b.finding {
        " type to find · Tab take · Enter list · Esc clear "
    } else if b.database.is_some() {
        " j/k move · Enter open · / find a table · q quit "
    } else if b.all_files {
        " j/k move · Enter open · h up · / find · a data files only · ~ home · q quit "
    } else {
        " j/k move · Enter open · h up · / find · a every file · ~ home · q quit "
    };
    let last = area.bottom().saturating_sub(1);
    buf.set_stringn(
        area.x,
        last,
        " ".repeat(area.width as usize),
        area.width as usize,
        p.on(t.status_fg, t.status_bg),
    );
    buf.set_stringn(
        area.x + 1,
        last,
        text::truncate(hints, area.width.saturating_sub(2) as usize),
        area.width.saturating_sub(2) as usize,
        p.on(t.status_fg, t.status_bg),
    );
    cursor
}

/// Runs the chooser, and gives the file that the user chose.
///
/// The function gives `None` when the user leaves with no file. It starts the
/// terminal and stops it again, so the caller can then open the file exactly
/// as it opens a file from the command line.
pub fn choose(config: &Config, theme: &Theme) -> anyhow::Result<Option<PathBuf>> {
    let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut b = Browser::new(dir, config);
    Ok(run(&mut b, theme)?.map(|e| e.path))
}

/// Runs the chooser over the tables of a database, and gives the table that the
/// user chose.
///
/// The caller reads the tables from the database first, and it then gives the
/// name of the chosen table to the engine. The engine therefore needs no
/// question of its own, in the same way as it needs no chooser of files.
pub fn choose_table(
    path: &Path,
    tables: &[DbTable],
    theme: &Theme,
) -> anyhow::Result<Option<DbTable>> {
    let mut b = Browser::of_database(path, tables);
    Ok(run(&mut b, theme)?.and_then(|e| e.table))
}

/// Gives `true` when the chooser takes the mouse from the terminal.
///
/// The chooser runs in front of the application, and the options of the command
/// line do not reach it. It therefore reads the option and the setting itself. A
/// user who turned the mouse off must keep the usual selection of text on this
/// screen too.
fn mouse_wanted() -> bool {
    if std::env::args().any(|a| a == "--no-mouse") {
        return false;
    }
    Config::load().0.mouse.unwrap_or(true)
}

/// Draws the chooser and reads the events until the user chooses or leaves.
fn run(b: &mut Browser, theme: &Theme) -> anyhow::Result<Option<Entry>> {
    use ratatui::crossterm::event::{self, DisableMouseCapture, Event};
    use ratatui::crossterm::execute;

    let depth = Depth::detect();
    let mut terminal = ratatui::init();
    // The application turns the mouse on in the same way, and it puts back the
    // hook that a panic needs. One function therefore holds that rule.
    let mouse = mouse_wanted() && crate::enable_mouse();

    // Draw only after something changed. With the mouse on, the terminal
    // reports each movement of the pointer, and the chooser does nothing with
    // those events. A frame for each of them would spend the processor of the
    // user to draw the same screen again. The main program keeps the same rule.
    let mut dirty = true;
    let result = loop {
        if dirty
            && let Err(e) = terminal.draw(|f| {
                if let Some(pos) = draw(f, b, theme, depth) {
                    f.set_cursor_position(pos);
                }
            })
        {
            break Err(e.into());
        }
        if b.quit {
            break Ok(None);
        }
        if let Some(e) = b.chosen.clone() {
            break Ok(Some(e));
        }
        dirty = false;
        match event::read() {
            // Windows sends one event for the press of a key and one event
            // for the release. Use the press only, or each key acts two
            // times.
            Ok(Event::Key(k)) if k.is_press() => {
                b.on_key(&k);
                dirty = true;
            }
            Ok(Event::Mouse(m)) => dirty = b.on_mouse(&m),
            // A terminal of a new size needs the whole screen again.
            Ok(Event::Resize(_, _)) => dirty = true,
            Ok(_) => {}
            Err(e) => break Err(e.into()),
        }
    };

    if mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    /// Makes a directory with some files in it.
    fn sample_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("peruse-browser-{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        for name in ["a.csv", "b.parquet", "notes.txt", "c.json"] {
            std::fs::write(d.join(name), b"x").unwrap();
        }
        std::fs::write(d.join(".hidden.csv"), b"x").unwrap();
        d
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn names(b: &Browser) -> Vec<String> {
        b.items().into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn the_list_holds_the_directories_and_the_data_files() {
        let d = sample_dir("list");
        let b = Browser::new(d, &Config::default());
        let n = names(&b);
        // The parent comes first, then the directories, then the files.
        assert_eq!(n[0], "..");
        assert_eq!(n[1], "sub");
        assert!(n.contains(&"a.csv".to_string()), "{n:?}");
        assert!(n.contains(&"b.parquet".to_string()), "{n:?}");
        assert!(n.contains(&"c.json".to_string()), "{n:?}");
        // A file that Peruse cannot read is not in the list.
        assert!(!n.contains(&"notes.txt".to_string()), "{n:?}");
        // A name that starts with a full stop is never in the list.
        assert!(!n.iter().any(|x| x.starts_with('.') && x != ".."), "{n:?}");
    }

    #[test]
    fn the_key_a_shows_each_file() {
        let d = sample_dir("all");
        let mut b = Browser::new(d, &Config::default());
        b.on_key(&key(KeyCode::Char('a')));
        assert!(names(&b).contains(&"notes.txt".to_string()));
        b.on_key(&key(KeyCode::Char('a')));
        assert!(!names(&b).contains(&"notes.txt".to_string()));
    }

    #[test]
    fn the_find_box_keeps_the_names_that_hold_the_text() {
        let d = sample_dir("find");
        let mut b = Browser::new(d, &Config::default());
        b.on_key(&key(KeyCode::Char('/')));
        for c in "par".chars() {
            b.on_key(&key(KeyCode::Char(c)));
        }
        assert_eq!(names(&b), ["b.parquet"]);
        // The parent goes away with a find box, because the user looks in
        // this directory.
        assert!(!names(&b).contains(&"..".to_string()));
    }

    #[test]
    fn the_find_box_writes_the_rest_of_a_name() {
        let d = sample_dir("ghost");
        let mut b = Browser::new(d, &Config::default());
        b.on_key(&key(KeyCode::Char('/')));
        for c in "b.".chars() {
            b.on_key(&key(KeyCode::Char(c)));
        }
        assert_eq!(b.ghost().as_deref(), Some("parquet"));
        b.on_key(&key(KeyCode::Tab));
        assert_eq!(b.find.text(), "b.parquet");
    }

    #[test]
    fn a_file_gives_its_path_back_and_a_directory_opens() {
        let d = sample_dir("open");
        let mut b = Browser::new(d.clone(), &Config::default());

        // The entry `sub` is a directory, so Enter goes into it.
        b.on_key(&key(KeyCode::Down));
        assert_eq!(b.selected().map(|e| e.name), Some("sub".into()));
        b.on_key(&key(KeyCode::Enter));
        assert!(b.chosen.is_none(), "a directory is not a choice");
        assert!(b.dir.ends_with("sub"), "{:?}", b.dir);

        // The key h goes back, and a file is a choice.
        b.on_key(&key(KeyCode::Char('h')));
        b.on_key(&key(KeyCode::Char('/')));
        for c in "a.csv".chars() {
            b.on_key(&key(KeyCode::Char(c)));
        }
        b.on_key(&key(KeyCode::Enter)); // leave the find box
        b.on_key(&key(KeyCode::Enter)); // choose the file
        assert_eq!(
            b.chosen.as_ref().and_then(|e| e.path.file_name()),
            Some(std::ffi::OsStr::new("a.csv"))
        );
    }

    /// Makes a list of tables for the picker.
    fn tables() -> Vec<DbTable> {
        vec![
            DbTable {
                schema: "main".into(),
                name: "customers".into(),
                is_view: false,
                rows: Some(12_480),
            },
            DbTable {
                schema: "main".into(),
                name: "orders".into(),
                is_view: false,
                rows: Some(3),
            },
            DbTable {
                schema: "staging".into(),
                name: "orders_raw".into(),
                is_view: false,
                rows: None,
            },
            DbTable {
                schema: "main".into(),
                name: "recent_orders".into(),
                is_view: true,
                rows: None,
            },
        ]
    }

    #[test]
    fn the_picker_lists_the_tables_and_then_the_views() {
        let db = PathBuf::from("/data/shop.duckdb");
        let b = Browser::of_database(&db, &tables());
        // A view holds no data of its own, so the tables come first.
        assert_eq!(
            names(&b),
            ["main.customers", "main.orders", "staging.orders_raw", "main.recent_orders"]
        );
        // The keys that walk the file system do nothing here: the list holds
        // the tables of one database.
        let mut b = b;
        b.on_key(&key(KeyCode::Char('h')));
        b.on_key(&key(KeyCode::Char('a')));
        assert_eq!(names(&b).len(), 4);
        assert!(b.chosen.is_none());
    }

    #[test]
    fn the_picker_finds_a_table_by_its_name() {
        // A database can hold hundreds of tables, so the picker needs the same
        // find box as the list of files.
        let db = PathBuf::from("/data/shop.duckdb");
        let mut b = Browser::of_database(&db, &tables());
        b.on_key(&key(KeyCode::Char('/')));
        for c in "order".chars() {
            b.on_key(&key(KeyCode::Char(c)));
        }
        assert_eq!(
            names(&b),
            ["main.orders", "staging.orders_raw", "main.recent_orders"]
        );

        // Enter leaves the find box, and Enter chooses the table.
        b.on_key(&key(KeyCode::Enter));
        b.on_key(&key(KeyCode::Enter));
        let got = b.chosen.as_ref().and_then(|e| e.table.clone()).unwrap();
        assert_eq!(got.name, "orders");
        assert!(!got.is_view);
    }

    #[test]
    fn the_picker_can_choose_a_view() {
        let db = PathBuf::from("/data/shop.duckdb");
        let mut b = Browser::of_database(&db, &tables());
        b.on_key(&key(KeyCode::End));
        b.on_key(&key(KeyCode::Enter));
        let got = b.chosen.as_ref().and_then(|e| e.table.clone()).unwrap();
        assert_eq!(got.name, "recent_orders");
        assert!(got.is_view, "the last entry is a view");
    }

    #[test]
    fn the_files_that_the_user_opened_come_first() {
        let d = sample_dir("recent");
        let recent = d.join("b.parquet");
        let config = Config {
            recent: vec![recent.to_string_lossy().into_owned()],
            ..Config::default()
        };
        let b = Browser::new(d, &config);
        assert_eq!(names(&b)[0], "b.parquet", "{:?}", names(&b));

        // A file that the user moved or deleted is not on the screen. The
        // list would offer a file that cannot open.
        let gone = Config {
            recent: vec!["/no/such/file.csv".into()],
            ..Config::default()
        };
        let b = Browser::new(sample_dir("recent-gone"), &gone);
        assert_eq!(names(&b)[0], "..");
    }

    #[test]
    fn a_directory_that_peruse_cannot_read_gives_a_message() {
        let mut b = Browser::new(PathBuf::from("/no/such/directory"), &Config::default());
        assert!(b.error.is_some());
        // The screen still works, and the user can leave.
        b.on_key(&key(KeyCode::Char('q')));
        assert!(b.quit);
    }

    #[test]
    fn the_key_q_clears_the_find_box_before_it_leaves() {
        let d = sample_dir("quit");
        let mut b = Browser::new(d, &Config::default());
        b.on_key(&key(KeyCode::Char('/')));
        b.on_key(&key(KeyCode::Char('a')));
        b.on_key(&key(KeyCode::Enter));
        assert!(!b.find.is_empty());

        b.on_key(&key(KeyCode::Char('q')));
        assert!(!b.quit, "the first q clears the find box");
        assert!(b.find.is_empty());
        b.on_key(&key(KeyCode::Char('q')));
        assert!(b.quit);
    }

    #[test]
    fn a_time_becomes_a_few_characters() {
        let now = SystemTime::now();
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - std::time::Duration::from_secs(120)), "2m ago");
        assert_eq!(ago(now - std::time::Duration::from_secs(7200)), "2h ago");
        assert_eq!(ago(now - std::time::Duration::from_secs(172800)), "2d ago");
        // A time in the future is not a fault that stops the program.
        assert_eq!(ago(now + std::time::Duration::from_secs(60)), "");
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;
    use ratatui::Terminal;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn screen(t: &Terminal<TestBackend>) -> String {
        let buf = t.backend().buffer();
        let a = buf.area;
        (a.top()..a.bottom())
            .map(|y| {
                (a.left()..a.right())
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn sample(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("peruse-browser-draw-{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("archive")).unwrap();
        for n in ["trips.parquet", "sales.csv", "events.ndjson", "readme.txt"] {
            std::fs::write(d.join(n), b"x".repeat(4096)).unwrap();
        }
        d
    }

    /// Makes one press of the left button at a position of the terminal.
    ///
    /// The result says if the press changed something, so a test can prove
    /// that the chooser draws a frame for it.
    fn click(b: &mut Browser, column: u16, row: u16) -> bool {
        b.on_mouse(&MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    /// Gives the row of the terminal that holds a text.
    fn row_of(t: &Terminal<TestBackend>, text: &str) -> u16 {
        screen(t)
            .lines()
            .position(|l| l.contains(text))
            .unwrap_or_else(|| panic!("no row holds {text:?}")) as u16
    }

    #[test]
    fn a_click_selects_an_entry_and_a_double_click_opens_it() {
        let d = sample("click");
        let mut b = Browser::new(d, &Config::default());
        let theme = peruse_core::theme::builtin("peruse-dark").unwrap();
        let mut term = Terminal::new(TestBackend::new(90, 18)).unwrap();
        term.draw(|f| {
            draw(f, &mut b, &theme, Depth::True);
        })
        .unwrap();

        // One click selects the entry under the pointer and no more, so a
        // click that lands on the wrong line costs nothing.
        let y = row_of(&term, "sales.csv");
        click(&mut b, 4, y);
        assert_eq!(b.selected().map(|e| e.name), Some("sales.csv".into()));
        assert!(b.chosen.is_none(), "one click chose a file");

        // The second press at the same place chooses the file, as Enter does.
        click(&mut b, 4, y);
        assert_eq!(b.chosen.map(|e| e.name), Some("sales.csv".into()));
    }

    #[test]
    fn a_double_click_on_a_directory_goes_into_it() {
        let d = sample("click-dir");
        let mut b = Browser::new(d, &Config::default());
        let theme = peruse_core::theme::builtin("peruse-dark").unwrap();
        let mut term = Terminal::new(TestBackend::new(90, 18)).unwrap();
        term.draw(|f| {
            draw(f, &mut b, &theme, Depth::True);
        })
        .unwrap();

        let y = row_of(&term, "archive/");
        click(&mut b, 4, y);
        click(&mut b, 4, y);
        assert_eq!(
            b.dir.file_name().map(|s| s.to_string_lossy().into_owned()),
            Some("archive".into()),
            "the double click did not go into the directory"
        );
        assert!(b.chosen.is_none(), "a directory is not a choice");
    }

    #[test]
    fn a_click_beside_the_list_selects_nothing() {
        let d = sample("click-nowhere");
        // A file that the user opened before puts a heading on the screen. A
        // heading is a line that the cursor goes past, so a click on it must
        // move nothing.
        let config = Config {
            recent: vec![d.join("sales.csv").to_string_lossy().into_owned()],
            ..Config::default()
        };
        let mut b = Browser::new(d, &config);
        let theme = peruse_core::theme::builtin("peruse-dark").unwrap();
        let mut term = Terminal::new(TestBackend::new(90, 18)).unwrap();
        term.draw(|f| {
            draw(f, &mut b, &theme, Depth::True);
        })
        .unwrap();
        let before = b.sel;

        // The title bar, a heading and the row of keys hold no entry.
        for y in [0u16, row_of(&term, "recent"), 17] {
            click(&mut b, 4, y);
            assert_eq!(b.sel, before, "a click on the row {y} moved the cursor");
            assert!(b.chosen.is_none());
        }
    }

    #[test]
    fn the_wheel_moves_the_cursor_of_the_chooser() {
        let d = sample("click-wheel");
        let mut b = Browser::new(d, &Config::default());
        let theme = peruse_core::theme::builtin("peruse-dark").unwrap();
        let mut term = Terminal::new(TestBackend::new(90, 18)).unwrap();
        term.draw(|f| {
            draw(f, &mut b, &theme, Depth::True);
        })
        .unwrap();

        let down = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 4,
            row: 6,
            modifiers: KeyModifiers::NONE,
        };
        b.on_mouse(&down);
        assert_eq!(b.sel, WHEEL_LINES, "the wheel moved another number of lines");
        b.on_mouse(&MouseEvent {
            kind: MouseEventKind::ScrollUp,
            ..down
        });
        assert_eq!(b.sel, 0);
        // A movement of the pointer changes nothing.
        b.on_mouse(&MouseEvent {
            kind: MouseEventKind::Moved,
            ..down
        });
        assert_eq!(b.sel, 0);
        assert!(b.chosen.is_none());
    }

    #[test]
    fn a_movement_of_the_pointer_draws_no_frame() {
        // The terminal reports each movement of the pointer while the mouse is
        // on. A frame for each of them would spend the processor of the user to
        // draw the same screen again.
        let d = sample("moved");
        let mut b = Browser::new(d, &Config::default());
        let theme = peruse_core::theme::builtin("peruse-dark").unwrap();
        let mut term = Terminal::new(TestBackend::new(90, 18)).unwrap();
        term.draw(|f| {
            draw(f, &mut b, &theme, Depth::True);
        })
        .unwrap();

        let y = row_of(&term, "sales.csv");
        let quiet = [
            MouseEventKind::Moved,
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Down(MouseButton::Right),
        ];
        let event = |kind| MouseEvent {
            kind,
            column: 4,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        for kind in quiet {
            assert!(!b.on_mouse(&event(kind)), "the chooser drew a frame for {kind:?}");
            assert_eq!(b.sel, 0, "{kind:?} moved the cursor");
            assert!(b.chosen.is_none(), "{kind:?} chose an entry");
        }

        // An event that changes the screen still asks for a frame. Without
        // this, the chooser would show an old screen.
        assert!(click(&mut b, 4, y), "a click on an entry drew no frame");
        assert!(
            b.on_mouse(&event(MouseEventKind::ScrollDown)),
            "the wheel drew no frame"
        );
        // The title bar holds no entry, so a press there changes nothing.
        assert!(!click(&mut b, 4, 0), "a press on the title bar drew a frame");

        // The find box keeps the focus through a movement of the pointer, and
        // it needs no frame for one either.
        b.on_key(&key(KeyCode::Char('/')));
        for kind in quiet {
            assert!(!b.on_mouse(&event(kind)), "the find box drew a frame for {kind:?}");
            assert!(b.finding, "{kind:?} took the focus out of the find box");
        }
    }

    #[test]
    fn a_double_click_in_a_long_list_opens_the_entry_of_the_first_press() {
        // The window keeps the selected entry on its last row, so a new
        // selection moves the window. The frame between the two presses of a
        // double click then puts another entry under the pointer, and the
        // second press must still open the entry of the first press.
        let d = std::env::temp_dir().join("peruse-browser-draw-long");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for n in 0..40 {
            std::fs::write(d.join(format!("file_{n:02}.csv")), b"a\n1\n").unwrap();
        }
        let mut b = Browser::new(d, &Config::default());
        let theme = peruse_core::theme::builtin("peruse-dark").unwrap();
        let mut term = Terminal::new(TestBackend::new(90, 18)).unwrap();
        let draw_one = |b: &mut Browser, term: &mut Terminal<TestBackend>| {
            term.draw(|f| {
                draw(f, b, &theme, Depth::True);
            })
            .unwrap();
        };

        // Go to the end, so the window shows the last entries.
        b.on_key(&key(KeyCode::Char('G')));
        draw_one(&mut b, &mut term);
        assert!(b.lines.len() < b.items().len(), "the list never moves");

        // The first line of the window is the furthest from the last row.
        let (y, at) = b.lines[0];
        let want = b.items()[at].name.clone();
        click(&mut b, 4, y);
        assert_eq!(b.sel, at);
        draw_one(&mut b, &mut term);
        assert_ne!(
            b.line_at(y),
            Some(at),
            "the list stayed still, so this test proves nothing"
        );

        click(&mut b, 4, y);
        assert_eq!(
            b.chosen.map(|e| e.name),
            Some(want),
            "the double click chose the entry that the list moved under it"
        );
    }

    #[test]
    fn the_wheel_moves_the_cursor_while_the_find_box_has_the_focus() {
        // The key `/` gives the focus to the find box, and an arrow key then
        // belongs to the text: it walks the history of the box. A turn of the
        // wheel must move the list instead, and write nothing.
        let d = sample("click-wheel-find");
        let mut b = Browser::new(d, &Config::default());
        let theme = peruse_core::theme::builtin("peruse-dark").unwrap();
        let mut term = Terminal::new(TestBackend::new(90, 18)).unwrap();
        term.draw(|f| {
            draw(f, &mut b, &theme, Depth::True);
        })
        .unwrap();

        b.on_key(&key(KeyCode::Char('/')));
        assert!(b.finding);
        assert!(b.items().len() > WHEEL_LINES, "too few entries for the test");
        b.on_mouse(&MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 4,
            row: 6,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(b.sel, WHEEL_LINES, "the wheel did not move the list");
        assert!(b.finding, "the wheel took the focus out of the find box");
        assert!(b.find.is_empty(), "the wheel wrote in the find box");
    }

    #[test]
    fn the_screen_names_each_file_with_its_format_and_its_size() {
        let d = sample("basic");
        let mut b = Browser::new(d, &Config::default());
        let theme = peruse_core::theme::builtin("peruse-dark").unwrap();
        let mut term = Terminal::new(TestBackend::new(90, 18)).unwrap();
        term.draw(|f| {
            draw(f, &mut b, &theme, Depth::True);
        })
        .unwrap();
        let s = screen(&term);

        assert!(s.contains("peruse"), "no title\n{s}");
        assert!(s.contains("archive/"), "a directory ends with a slash\n{s}");
        assert!(s.contains("trips.parquet"), "{s}");
        assert!(s.contains("parquet"), "no format\n{s}");
        assert!(s.contains("4.00 KB"), "no size\n{s}");
        assert!(s.contains("just now"), "no time\n{s}");
        // A file that Peruse cannot read is not on the screen.
        assert!(!s.contains("readme.txt"), "{s}");
        assert!(s.contains("q quit"), "no keys\n{s}");
    }

    #[test]
    fn the_screen_shows_the_files_that_the_user_opened_before() {
        let d = sample("recent");
        let config = Config {
            recent: vec![d.join("sales.csv").to_string_lossy().into_owned()],
            ..Config::default()
        };
        let mut b = Browser::new(d, &config);
        let theme = peruse_core::theme::builtin("peruse-dark").unwrap();
        let mut term = Terminal::new(TestBackend::new(90, 18)).unwrap();
        term.draw(|f| {
            draw(f, &mut b, &theme, Depth::True);
        })
        .unwrap();
        let s = screen(&term);
        assert!(s.contains("recent"), "no heading\n{s}");
        // The heading of the directory follows the files that came before.
        let recent_at = s.find("recent").unwrap();
        let sales_at = s.find("sales.csv").unwrap();
        assert!(recent_at < sales_at, "{s}");
    }

    #[test]
    fn the_screen_of_a_database_names_the_tables_the_views_and_the_row_counts() {
        let tables = vec![
            DbTable {
                schema: "main".into(),
                name: "customers".into(),
                is_view: false,
                rows: Some(12_480),
            },
            DbTable {
                schema: "main".into(),
                name: "recent_orders".into(),
                is_view: true,
                rows: None,
            },
        ];
        let mut b = Browser::of_database(Path::new("/data/shop.duckdb"), &tables);
        let theme = peruse_core::theme::builtin("peruse-dark").unwrap();
        let mut term = Terminal::new(TestBackend::new(90, 18)).unwrap();
        term.draw(|f| {
            draw(f, &mut b, &theme, Depth::True);
        })
        .unwrap();
        let s = screen(&term);

        assert!(s.contains("shop.duckdb"), "no database named\n{s}");
        assert!(s.contains("which table?"), "{s}");
        assert!(s.contains("main.customers"), "{s}");
        // The heading and the word both say which entries are views.
        assert!(s.contains("tables"), "no heading\n{s}");
        assert!(s.contains("views"), "no heading for the views\n{s}");
        assert!(s.contains("view"), "{s}");
        // The count comes from the catalog of the database, and the mark ~ says
        // that the database estimated it.
        assert!(s.contains("~12,480 rows"), "no row count\n{s}");
        assert!(s.contains("find a table"), "no keys\n{s}");

        // The picker draws with the same code as the chooser of files, so a
        // terminal that is too small for it must not stop the program either.
        for (w, h) in [(20u16, 6u16), (10, 4), (8, 2), (200, 60)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| {
                draw(f, &mut b, &theme, Depth::True);
            })
            .unwrap();
            b.on_key(&key(KeyCode::Char('/')));
            b.on_key(&key(KeyCode::Char('c')));
            term.draw(|f| {
                draw(f, &mut b, &theme, Depth::True);
            })
            .unwrap();
            b.on_key(&key(KeyCode::Esc));
        }
    }

    #[test]
    fn a_small_screen_does_not_stop_the_program() {
        let d = sample("small");
        let mut b = Browser::new(d, &Config::default());
        let theme = peruse_core::theme::builtin("peruse-dark").unwrap();
        for (w, h) in [(90u16, 18u16), (20, 6), (10, 4), (200, 60), (8, 2)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| {
                draw(f, &mut b, &theme, Depth::True);
            })
            .unwrap();
            b.on_key(&key(KeyCode::Char('/')));
            b.on_key(&key(KeyCode::Char('s')));
            term.draw(|f| {
                draw(f, &mut b, &theme, Depth::True);
            })
            .unwrap();
            b.on_key(&key(KeyCode::Esc));
        }
    }
}
