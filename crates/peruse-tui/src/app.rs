//! The state of the application, and the rules that change it.
//!
//! The user interface thread touches this structure only. Each operation that
//! can block goes to [`peruse_core::Worker`] as a request. The request carries
//! the current epoch.
//!
//! The answer comes back as a response. The function [`App::on_response`] then
//! adds the response to the state. If the view changed after the request, the
//! function discards the response instead.

use peruse_core::config::{Config, Resources};
use peruse_core::engine::{footer_briefs, ColumnBrief};
use peruse_core::filter::{FilterSet, Op, Term};
use peruse_core::query::{quote_str, Step};
use peruse_core::meta::FileMeta;
use peruse_core::model::RowCount;
use peruse_core::query::{Base, SortDir, SortKey, PROMPT_START};
use peruse_core::source::Format;
use peruse_core::stats::ColumnStats;
use peruse_core::theme::Theme;
use peruse_core::{sql_guard, Opened, Request, Response, RowPage, Schema, Source, View, Worker};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use std::time::{Duration, Instant};

use crate::clip;
use crate::commands::{self, Cmd};
use crate::input::{Action, LineInput, plain};
use crate::tree::{Family, Line, Tree};

/// The number of rows that Peruse reads before the first row of the viewport
/// and after the last row. A usual scroll therefore does not wait for the
/// engine.
const PREFETCH: u64 = 250;
/// The number of rows that the worker examines in the first search request.
///
/// The number is small, so a match near the cursor comes back immediately,
/// and the user can stop a search that finds nothing.
const SEARCH_CHUNK: u64 = 250_000;

/// The largest number of rows that one search request examines.
///
/// Each request reads the view from its start and then skips to its part, so
/// a request that starts late costs more than a request that starts early. A
/// search of ten million rows in parts of 250,000 therefore reads the file
/// forty times, and the cost grows with the square of the size.
///
/// The parts double in size instead. The first one stays small, so a match
/// near the cursor is still immediate, and the number of parts falls from
/// forty to about six.
const SEARCH_CHUNK_MAX: u64 = 4_000_000;
/// The largest number of match offsets that one search request gives.
const SEARCH_HITS: u32 = 500;
/// The largest size of a file of text that Peruse indexes when it opens it.
///
/// A scan of this size is quick, and the user does not notice it. A jump to
/// the last row then costs almost no time. For a larger file, Peruse waits for
/// the key `I`. The user therefore never waits for a scan that the user did
/// not ask for.
///
/// The index holds the file in memory, and it takes about one and a third
/// times the size of the file. This limit therefore also limits what Peruse
/// spends without asking: about 85 MB, which is one percent of a machine with
/// 8 GB. A limit of 256 MB would spend 340 MB of that machine, and on a disk
/// that turns it would read for some seconds before the user could do
/// anything.
const AUTO_INDEX_BYTES: u64 = 64 * 1024 * 1024;

/// The largest number of columns of a file that Peruse indexes when it opens
/// it.
///
/// The size in bytes is the wrong measure by itself. A file of 170 MB with
/// 10,000 columns is below the limit above, and the index of it costs 21
/// seconds and 2.7 GB. The number of columns is what makes that file slow,
/// so the number of columns needs its own limit.
const AUTO_INDEX_COLUMNS: usize = 256;

/// The number of rows or columns that one step moves, when the settings file
/// names no other number.
///
/// The keys `J`, `K`, `H` and `L` move by one step. A user who reads a file
/// looks at groups of rows, and one row for each press of a key is too slow.
/// Ten is a number that the user can count on the screen, and it is therefore
/// easy to predict.
pub const DEFAULT_STEP: usize = 10;

/// The largest step that the settings page accepts.
///
/// A step of some thousand rows is a jump, and the key `#` does a jump to a
/// row number better.
pub const MAX_STEP: usize = 1000;

/// The rows that one turn of the wheel moves.
///
/// A terminal and a text editor both move three lines for one turn. The grid
/// therefore moves by the same amount as the other windows of the user.
const WHEEL_ROWS: i64 = 3;

/// The columns that one turn of the wheel to the side moves.
///
/// A column is much wider than a row is high, so a turn moves fewer columns
/// than rows. Two columns keep the movement inside one screen.
const WHEEL_COLS: i64 = 2;

/// What one turn of the wheel does in the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wheel {
    /// Move this number of rows. A negative number moves up the view.
    Rows(i64),
    /// Move this number of columns. A negative number moves to the left.
    Cols(i64),
    /// The event is not a turn of the wheel.
    None,
}

/// Reads one mouse event and gives the movement for it.
///
/// The plain wheel always moves the view up and down. That is what the wheel
/// does in each other program, and it must not change.
///
/// The wheel with the control key, and the wheel with the shift key, move the
/// view to the side. A wheel that turns to the side does the same, but few mice
/// have one. Some terminals keep the control key with the wheel for the size of
/// the text, and Peruse then never sees the event. The shift key is therefore
/// the form that always works, and the two are both here.
fn wheel_of(ev: &MouseEvent) -> Wheel {
    let sideways = ev.modifiers.contains(KeyModifiers::SHIFT)
        || ev.modifiers.contains(KeyModifiers::CONTROL);
    match ev.kind {
        MouseEventKind::ScrollDown if sideways => Wheel::Cols(WHEEL_COLS),
        MouseEventKind::ScrollUp if sideways => Wheel::Cols(-WHEEL_COLS),
        MouseEventKind::ScrollDown => Wheel::Rows(WHEEL_ROWS),
        MouseEventKind::ScrollUp => Wheel::Rows(-WHEEL_ROWS),
        MouseEventKind::ScrollRight => Wheel::Cols(WHEEL_COLS),
        MouseEventKind::ScrollLeft => Wheel::Cols(-WHEEL_COLS),
        _ => Wheel::None,
    }
}

/// The longest time between two presses of the left button that Peruse reads
/// as one double click.
///
/// A terminal reports a press and a release, and never a double click. Peruse
/// therefore finds the double click itself: two presses of the left button, at
/// the same row and the same column of the terminal, inside this time. 400 ms
/// is the value that a desktop system uses, so a user who already clicks two
/// times to open something is inside it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// One press of the left button.
#[derive(Clone, Copy, Debug)]
struct Press {
    /// The column of the terminal.
    column: u16,
    /// The row of the terminal.
    row: u16,
    /// The time of the press.
    at: Instant,
    /// `true` when this press closed a double click.
    ///
    /// The next press then starts a new pair. Without this, three presses
    /// would give two double clicks, and a user who clicks quickly would open
    /// the same thing two times.
    paired: bool,
}

/// Finds a double click in the presses of the left button.
#[derive(Clone, Copy, Debug, Default)]
pub struct Clicks {
    /// The press that came before this one.
    last: Option<Press>,
}

impl Clicks {
    /// Records one press of the left button, and gives `true` when the press
    /// closes a double click.
    fn press(&mut self, column: u16, row: u16, now: Instant) -> bool {
        let double = self.last.is_some_and(|l| {
            !l.paired
                && l.column == column
                && l.row == row
                && now.duration_since(l.at) <= DOUBLE_CLICK
        });
        self.last = Some(Press {
            column,
            row,
            at: now,
            paired: double,
        });
        double
    }

    /// Records one press that happens now.
    pub fn press_now(&mut self, column: u16, row: u16) -> bool {
        self.press(column, row, Instant::now())
    }
}

/// Gives the reason that Peruse must not write the settings file now, or `None`
/// when it can write.
///
/// A file that Peruse cannot read gives the built-in settings and the reason.
/// The function [`peruse_core::config::Config::to_toml`] then writes the whole
/// file from the fields that Peruse holds, so a write would replace each setting
/// that the user wrote in the file, and every note in it. One character wrong in
/// the file would cost the user the rest of it.
///
/// A file that is not there is not a fault, and Peruse writes a new one.
fn write_blocked(path: &std::path::Path) -> Option<String> {
    Config::load_from(path)
        .1
        .map(|why| format!("the file has a fault, so nothing was written: {why}"))
}

/// Reads the step from the settings, and keeps it inside its limits.
///
/// The value is never below one. A step of zero would give a key that does
/// nothing, and the user would read that as a fault in the program. The value
/// has an upper limit, because a step of some million rows is a jump, and the
/// key `#` does a jump better.
fn step_of(setting: Option<usize>) -> i64 {
    setting.unwrap_or(DEFAULT_STEP).clamp(1, MAX_STEP) as i64
}

/// The smallest width of a column, in screen columns.
const MIN_COL_WIDTH: u16 = 3;
/// The largest width that Peruse gives to a column when it fits the widths.
const MAX_COL_WIDTH: u16 = 60;

/// The screen columns that a fitted column keeps free after its name.
///
/// Two parts of the header ask for this room, and the larger of the two gives
/// the number:
///
/// * `grid::draw_header` writes the type mark at the far side of the column,
///   and it keeps two blank columns between the name and that mark. The mark is
///   one character wide, so the header asks for 3. With less, the header drops
///   the mark.
/// * `grid::compact_line` writes the type at the left of the band and the share
///   of NULL values at the right, with one blank column between them. The share
///   is four characters at the most, as in `100%`, so the band asks for 5. With
///   less, the band drops the type of a column whose type is no wider than its
///   name.
///
/// The header of a sorted column also holds an arrow in front of the name, which
/// costs one screen column. The larger of the two demands covers it, because the
/// header then asks for 4 and this constant gives 5.
///
/// The test `grid::tests::the_room_after_a_name_covers_the_largest_share_of_null_values`
/// holds this value at 5. A share of 20% needs one column less, so a test over a
/// column with some values only does not hold it.
pub(crate) const NAME_HEADROOM: usize = 5;

/// Gives the width of one column, from the width of its name and the width of
/// the widest value on the screen.
///
/// The result covers the name and [`NAME_HEADROOM`], so the header and the band
/// always have their room, also for a column of narrow values.
///
/// The limit [`MAX_COL_WIDTH`] comes last and wins over that rule. A name of 56
/// characters or more therefore gets a width below `name + NAME_HEADROOM`, and
/// the header of such a column does squeeze the band. That is the correct trade:
/// one column with a very long name must not push each other column off the
/// right edge, and the user can widen it by hand with the key `>`.
fn fitted_width(name: usize, widest: usize) -> u16 {
    // The limit applies before the cast. A name has no limit of its own, and a
    // name of some thousand characters does not fit in 16 bits.
    let w = widest.max(name + NAME_HEADROOM).min(MAX_COL_WIDTH as usize);
    (w as u16).max(MIN_COL_WIDTH)
}

/// The kind of text that the prompt asks the user for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    /// A `WHERE` expression.
    Filter,
    /// A complete SQL statement.
    Sql,
    /// A value to search for.
    Search,
    /// A row number to move to.
    Goto,
}

impl PromptKind {
    /// Gives the word that the prompt shows in front of the text.
    pub fn label(self) -> &'static str {
        match self {
            PromptKind::Filter => "filter",
            PromptKind::Sql => "sql",
            PromptKind::Search => "search",
            PromptKind::Goto => "row",
        }
    }
}

/// What the keys do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The grid has the focus.
    Normal,
    /// The prompt has the focus.
    Prompt(PromptKind),
    /// The help overlay is open.
    Help,
    /// The command palette is open.
    Palette,
    /// The theme picker is open.
    ThemePicker,
    /// The cell inspector is open.
    Cell,
    /// The record view is open. It shows one row from the top to the bottom.
    Record,
    /// The filter builder is open.
    FilterBuild,
    /// The settings page is open.
    Settings,
}

/// One setting that the settings page can change.
///
/// The list is short on purpose. A setting belongs here when a user changes it
/// for each file, or when the machine decides the right value for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    /// The colors.
    Theme,
    /// The number of threads that DuckDB can use.
    Threads,
    /// The memory that DuckDB can use before it writes to the disk.
    MemoryLimit,
    /// The rows that the sniffer of a file of text examines.
    SampleSize,
    /// Whether Peruse indexes a file of text when it opens the file.
    NoIndex,
    /// The panels that stay at the side of the grid.
    Panels,
    /// The rows of facts under the column names.
    Band,
    /// The number of rows or columns that one step moves.
    Step,
}

impl Setting {
    /// Each setting, in the order that the page shows them.
    pub const ALL: &'static [Setting] = &[
        Setting::Theme,
        Setting::Threads,
        Setting::MemoryLimit,
        Setting::SampleSize,
        Setting::NoIndex,
        Setting::Panels,
        Setting::Band,
        Setting::Step,
    ];

    /// The name that the page shows.
    pub fn label(self) -> &'static str {
        match self {
            Setting::Theme => "theme",
            Setting::Threads => "threads",
            Setting::MemoryLimit => "memory limit",
            Setting::SampleSize => "sample size",
            Setting::NoIndex => "index at open",
            Setting::Panels => "panels",
            Setting::Band => "column details",
            Setting::Step => "step",
        }
    }

    /// What the setting does, in one line.
    pub fn help(self) -> &'static str {
        match self {
            Setting::Theme => "the colors. T also opens the picker",
            Setting::Threads => "threads for DuckDB. Empty means one for each core",
            Setting::MemoryLimit => "such as 4GB, before DuckDB writes to the disk",
            Setting::SampleSize => "rows the sniffer reads. -1 reads the whole file",
            Setting::NoIndex => "index a file of text at the start, for instant jumps",
            Setting::Panels => "keep at the side: none, meta, stats or both",
            Setting::Band => "rows under the headers: off, compact or detailed. d cycles",
            Setting::Step => "rows or columns that J, K, H and L move",
        }
    }

    /// Gives `true` when a change takes effect at the next file, and not now.
    ///
    /// DuckDB changes its threads and its memory while it runs. The sniffer
    /// and the index do their work when a file opens, so a change to them
    /// cannot reach the file that is open already.
    pub fn at_next_file(self) -> bool {
        matches!(self, Setting::SampleSize | Setting::NoIndex)
    }
}

/// The step that the filter builder is at.
///
/// The builder is a small machine with four steps. The user starts at the
/// list, adds a condition through the three steps, and comes back to the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Build {
    /// The list of the conditions.
    List,
    /// The user chooses a column.
    Column,
    /// The user chooses an operator.
    Op,
    /// The user types the value.
    Value,
    /// The user types the second value of a `between` condition.
    Value2,
    /// The user types one condition as a `WHERE` expression.
    Raw,
}

/// The condition that the filter builder makes now.
#[derive(Clone, Debug)]
pub struct Draft {
    /// The position of the column in the schema.
    pub col: usize,
    /// The operator.
    pub op: Op,
    /// The first value, from the step [`Build::Value`].
    pub value: String,
    /// The second value, from the step [`Build::Value2`]. Only the operator
    /// `between` uses it.
    pub value2: String,
    /// The position of the condition that the user edits. The value `None`
    /// shows that the builder makes a new condition.
    pub edit: Option<usize>,
}

impl Default for Draft {
    fn default() -> Draft {
        Draft {
            col: 0,
            op: Op::Eq,
            value: String::new(),
            value2: String::new(),
            edit: None,
        }
    }
}

/// The panel at the side of the grid, or below it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Panel {
    /// No panel is open.
    #[default]
    None,
    /// The metadata panel is open.
    Meta,
    /// The column statistics panel is open.
    Stats,
    /// Both panels are open, one above the other.
    Both,
}

impl Panel {
    /// Gives `true` when the metadata is on the screen.
    pub fn has_meta(self) -> bool {
        matches!(self, Panel::Meta | Panel::Both)
    }
    /// Gives `true` when the statistics of the column are on the screen.
    pub fn has_stats(self) -> bool {
        matches!(self, Panel::Stats | Panel::Both)
    }

    /// Reads a panel from the name that the settings file holds.
    pub fn parse(s: &str) -> Option<Panel> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "" => Some(Panel::None),
            "meta" | "metadata" => Some(Panel::Meta),
            "stats" | "statistics" => Some(Panel::Stats),
            "both" => Some(Panel::Both),
            _ => None,
        }
    }

    /// Gives the name that the settings file holds.
    pub fn name(self) -> &'static str {
        match self {
            Panel::None => "none",
            Panel::Meta => "meta",
            Panel::Stats => "stats",
            Panel::Both => "both",
        }
    }

    /// Gives the next panel, for a key that moves through the four.
    pub fn next(self) -> Panel {
        match self {
            Panel::None => Panel::Meta,
            Panel::Meta => Panel::Stats,
            Panel::Stats => Panel::Both,
            Panel::Both => Panel::None,
        }
    }
}

/// The rows of facts that the grid draws between the column names and the first
/// row of data.
///
/// The band answers the first question about a file: what is in each column?
/// The statistics panel answers it for one column at a time, and it stays at the
/// side. The band answers it for every column that is on the screen, in the
/// spirit of `df.info()` and `df.describe()` of pandas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Band {
    /// The band is not on the screen.
    #[default]
    Off,
    /// One row for each column: the type and the share of NULL values.
    Compact,
    /// Four rows for each column.
    Detailed,
}

impl Band {
    /// The number of rows of the detailed band.
    ///
    /// The four rows are the type, the share of NULL values, the count of the
    /// different values, and the range from the smallest value to the largest
    /// value. Those four answer the question "what is in this column?" and each
    /// of them has a meaning for every type.
    ///
    /// The mean and the deviation are not in the band. Only a column of numbers
    /// has them, so the row would be empty over each column of text, and a row
    /// of the grid is expensive. The statistics panel shows them.
    ///
    /// Every column gives the same rows in the same order, so the facts line up
    /// across the grid and the eye can compare one fact over many columns.
    pub const DETAIL_ROWS: u16 = 4;

    /// Gives the number of rows that this mode asks for.
    pub fn rows(self) -> u16 {
        match self {
            Band::Off => 0,
            Band::Compact => 1,
            Band::Detailed => Band::DETAIL_ROWS,
        }
    }

    /// Gives `true` when the band needs the count of the different values and
    /// the range of the values.
    ///
    /// The compact band needs the NULL share only, and the footer of a Parquet
    /// file gives that share with no query.
    pub fn needs_values(self) -> bool {
        matches!(self, Band::Detailed)
    }

    /// Reads a mode from the name that the settings file holds.
    pub fn parse(s: &str) -> Option<Band> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "" => Some(Band::Off),
            "compact" | "short" => Some(Band::Compact),
            "detailed" | "detail" | "full" => Some(Band::Detailed),
            _ => None,
        }
    }

    /// Gives the name that the settings file holds.
    pub fn name(self) -> &'static str {
        match self {
            Band::Off => "off",
            Band::Compact => "compact",
            Band::Detailed => "detailed",
        }
    }

    /// Gives the text that the status line shows after a change.
    pub fn label(self) -> &'static str {
        match self {
            Band::Off => "no column details",
            Band::Compact => "column details: compact",
            Band::Detailed => "column details: type, nulls, distinct, range",
        }
    }

    /// Gives the next mode, for the key that moves through the three.
    pub fn next(self) -> Band {
        match self {
            Band::Off => Band::Compact,
            Band::Compact => Band::Detailed,
            Band::Detailed => Band::Off,
        }
    }
}

/// Where the grid is on the screen.
///
/// A mouse event gives a row and a column of the terminal. Only the code that
/// draws the grid knows which cell is at that position, so it writes the
/// positions here after each frame. The mouse then reads them.
#[derive(Clone, Debug, Default)]
pub struct Hit {
    /// The row of the terminal that holds the column names.
    pub header_y: u16,
    /// The number of rows of the detail band, under the column names.
    ///
    /// The band takes rows from the data. Without this count, a click would
    /// land some rows above the row that the user pointed at.
    pub band: u16,
    /// The first row of the terminal that holds a row of data.
    pub top: u16,
    /// The number of rows of data on the screen. Zero means that the grid has
    /// no room, and that no click can land on a row.
    pub rows: u16,
    /// The first column of the terminal that the grid covers.
    pub left: u16,
    /// The number of columns of the terminal that the grid covers. Zero means
    /// that the grid has no room.
    ///
    /// A panel at the side of the grid sits on the same rows as the grid. Without
    /// this width, a click in the panel would count as a click on the row of the
    /// grid beside it, and the cursor would jump.
    pub width: u16,
    /// Each column that the grid draws: its position in the schema, its column
    /// of the terminal, and its width.
    pub cols: Vec<(usize, u16, u16)>,
}

impl Hit {
    /// Gives `true` when a column of the terminal is inside the grid.
    ///
    /// A panel at the side of the grid covers the same rows as the grid, so the
    /// row of an event does not say that the event belongs to the grid.
    pub fn holds(&self, x: u16) -> bool {
        self.width > 0 && x >= self.left && x < self.left.saturating_add(self.width)
    }

    /// Gives the offset of the row at a row of the terminal, counted from the
    /// first row on the screen. The result is `None` when the position is
    /// outside the grid.
    pub fn row_at(&self, y: u16) -> Option<u64> {
        (self.rows > 0 && y >= self.top && y < self.top.saturating_add(self.rows))
            .then(|| (y - self.top) as u64)
    }

    /// Gives `true` when a row of the terminal holds the column names or one row
    /// of the detail band.
    ///
    /// A click there moves to the column and leaves the row where it is. The
    /// band belongs to the header: it describes a column and not a row.
    pub fn on_labels(&self, y: u16) -> bool {
        y >= self.header_y && y < self.header_y.saturating_add(1 + self.band)
    }

    /// Gives the position in the schema of the column at a column of the
    /// terminal. The result is `None` for the gutter of row numbers and for
    /// the space after the last column.
    pub fn col_at(&self, x: u16) -> Option<usize> {
        self.cols
            .iter()
            .find(|(_, cx, w)| x >= *cx && x < cx.saturating_add(*w))
            .map(|(ci, _, _)| *ci)
    }
}

/// Where an overlay is on the screen.
///
/// This is the same idea as [`Hit`], for the box that covers the grid. Only the
/// code that draws an overlay knows the box that it chose, so each overlay
/// gives the box back and [`crate::ui::draw`] writes it here. The mouse then
/// reads it: a click outside the box closes the overlay, and a click on a line
/// of the list acts on that line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayHit {
    /// The mode that drew this box.
    ///
    /// A click uses the box in that mode only. A key can change the mode
    /// between two frames, and a click must never act on a box that is gone.
    pub mode: Mode,
    /// The box, with its border.
    pub area: Rect,
    /// One pair for each line of the list on the screen: the row of the
    /// terminal, and the position of that line in the list.
    ///
    /// A list scrolls, and some lists hold a heading that the selection goes
    /// past. The row on the screen is therefore not the position in the list,
    /// and a click needs this table to find the line under the pointer.
    pub lines: Vec<(u16, usize)>,
}

impl OverlayHit {
    /// Makes the record of a box that holds no list.
    pub fn new(mode: Mode, area: Rect) -> OverlayHit {
        OverlayHit {
            mode,
            area,
            lines: Vec::new(),
        }
    }

    /// Adds one line of the list, at a row of the terminal.
    pub fn line(&mut self, y: u16, at: usize) {
        self.lines.push((y, at));
    }

    /// Gives `true` when a position of the terminal is inside the box.
    pub fn holds(&self, x: u16, y: u16) -> bool {
        x >= self.area.x
            && x < self.area.right()
            && y >= self.area.y
            && y < self.area.bottom()
    }

    /// Gives the position in the list of the line at a row of the terminal.
    /// The result is `None` for the border, for a prompt and for a heading.
    pub fn line_at(&self, y: u16) -> Option<usize> {
        self.lines
            .iter()
            .find(|(ly, _)| *ly == y)
            .map(|(_, at)| *at)
    }
}

/// The kind of a message on the status line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    /// A message about the state of the program.
    Info,
    /// A message about an operation that succeeded.
    Ok,
    /// A message about a failure.
    Error,
}

/// One message on the status line.
#[derive(Clone, Debug)]
pub struct Status {
    /// The text of the message.
    pub text: String,
    /// The kind of the message. It selects the color and the sign.
    pub kind: StatusKind,
}

/// A search that runs now. It moves through the view one part at a time.
#[derive(Clone, Copy, Debug)]
struct Scan {
    /// `true` when the search moves down the view.
    forward: bool,
    /// `true` when a match on the row of the cursor counts. A search that the
    /// user types has the value `true`. The keys `n` and `N` have the value
    /// `false`, because they must always move the cursor.
    inclusive: bool,
    /// The first row of the next part, for a search down the view. For a
    /// search up the view, it is the row after the last row of the next part.
    next: u64,
    /// The number of rows that the search examined.
    examined: u64,
    /// The number of rows in the view.
    total: u64,
    /// The number of rows that the next part covers.
    chunk: u64,
}

/// One view that the user had, for the keys that go back and forward.
///
/// The step holds the filter as a list of conditions beside the expression.
/// Without the list, a step backward would give the rows of the old filter and
/// the conditions of the new one, and the builder would then disagree with the
/// grid.
#[derive(Clone, Debug, PartialEq)]
struct ViewStep {
    /// The relation, the filter and the sort.
    view: View,
    /// The conditions that compiled to the filter.
    fset: FilterSet,
}

/// The number of views that Peruse remembers.
///
/// The list holds one entry for each change, and a change is a press of a key.
/// This number is more than a user needs in one session, and it stops the list
/// from growing without an end.
const MAX_HISTORY: usize = 100;

/// The complete state of the application.
pub struct App {
    /// The handle to the engine thread.
    pub worker: Worker,
    /// The file or the files that Peruse shows.
    pub source: Source,
    /// The `read_parquet` call or `read_csv` call that reads the file.
    pub read_expr: String,
    /// The name and the type of each column of the view.
    pub schema: Schema,
    /// The description of what the grid shows.
    pub view: View,
    /// The epoch of the current view. Each change of the view increases it.
    epoch: u64,

    /// The view that the grid shows now. [`App::reload`] compares the new
    /// view against it, and a difference makes one entry of the history.
    applied: ViewStep,
    /// The views that the user had before this one. The key `u` takes one.
    history: Vec<ViewStep>,
    /// The views that the key `u` removed. The key `U` puts one back.
    undone: Vec<ViewStep>,
    /// `true` while `u` or `U` changes the view. The list of the views that
    /// `u` removed must not clear itself during that change.
    restoring: bool,

    /// The page of rows that the grid draws now.
    pub page: RowPage,
    /// The number of rows that the last page request asked for.
    ///
    /// A page with fewer rows than this number is the last page. Peruse can
    /// therefore tell the end of the data from a page that is still to come.
    page_limit: u32,
    /// The offset and the limit of the page request that has no answer yet.
    requested: Option<(u64, u32)>,
    /// The number of rows in the view.
    pub total: RowCount,

    /// The row of the cursor, as an offset in the view.
    pub cursor_row: u64,
    /// The column of the cursor, as a position in the schema.
    pub cursor_col: usize,
    /// The first row that the grid draws.
    pub top_row: u64,
    /// The first column that the grid draws, as a position in the list from
    /// [`App::visible_columns`].
    pub left_col: usize,
    /// The number of rows that the grid can draw.
    pub viewport_rows: usize,
    /// Where the grid is on the screen, for the mouse.
    pub hit: Hit,
    /// Where the overlay is on the screen, for the mouse. A mode with no
    /// overlay holds `None`.
    pub overlay: Option<OverlayHit>,
    /// The presses of the left button, for the double click.
    clicks: Clicks,

    /// The width of each column, in screen columns.
    pub widths: Vec<u16>,
    /// `true` for each column that the grid does not draw.
    pub hidden: Vec<bool>,
    /// `true` after Peruse fits the widths to the values on the screen.
    widths_fitted: bool,

    /// The theme that Peruse paints with.
    pub theme: Theme,
    /// Each theme that Peruse can use.
    pub themes: Vec<Theme>,
    /// The position of the current theme in `themes`.
    pub theme_idx: usize,

    /// What the keys do now.
    pub mode: Mode,
    /// The panel that is open.
    pub panel: Panel,
    /// The prompt that has the focus.
    pub input: LineInput,
    /// The message about a bad expression in the prompt.
    pub prompt_error: Option<String>,
    /// The text and the history of the filter prompt.
    pub filter_input: LineInput,
    /// The text and the history of the SQL prompt.
    pub sql_input: LineInput,
    /// The text and the history of the search prompt.
    pub search_input: LineInput,

    /// The message on the status line.
    pub status: Option<Status>,
    /// `true` while the worker has work to do.
    pub busy: bool,

    /// The statistics of each column that the engine measured for this view.
    ///
    /// The statistics of a column cost a scan, so Peruse keeps each answer
    /// until the view changes. Without this, a move across the columns with
    /// the statistics on the screen would ask for the same numbers again at
    /// each step back.
    stats_cache: std::collections::HashMap<String, ColumnStats>,
    /// The column that the engine measures now. It stops Peruse from asking
    /// for the same column two times.
    stats_pending: Option<String>,

    /// The facts of the detail band, for each column of this view.
    ///
    /// The key is the name of the column. [`App::reload`] empties the cache,
    /// because the facts describe one view. A move across the columns therefore
    /// costs no query while the answer is here.
    band_cache: std::collections::HashMap<String, ColumnBrief>,
    /// The columns of the one band request that is on its way to the engine.
    ///
    /// The set stops an endless re-ask: the engine can give no answer for a
    /// column, and without the set the band asks again at each frame.
    ///
    /// A new request replaces the set, and does not add to it. The worker keeps
    /// one band request only, so a new request drops the older one and the older
    /// answer never arrives. The columns of that older request must therefore go
    /// back into the list of the columns that the band still needs. Without
    /// this, a scroll to the side and back leaves a row of points on the first
    /// columns until the user changes the view.
    band_asked: std::collections::HashSet<String>,
    /// `true` when the band request that has no answer yet measures the count of
    /// the different values and the range.
    ///
    /// A compact request measures two counts, and that is not enough for the
    /// detailed band. Without this flag the record of what Peruse asked for would
    /// count as an answer for either mode, and a change from compact to detailed
    /// would leave the three other rows showing points for ever.
    band_asked_values: bool,

    /// The metadata of the file.
    pub meta: Option<FileMeta>,
    /// `true` after Peruse asked for the metadata.
    ///
    /// The metadata describes the file, so one request is enough for the whole
    /// session. Two parts ask for it: the metadata panel, and the compact band
    /// over a Parquet file.
    meta_asked: bool,
    /// `true` after a request for the metadata failed.
    ///
    /// The band then measures the columns with a query. Without this, a band over
    /// a Parquet file with a footer that Peruse cannot read would wait for an
    /// answer that never comes, and it would show a row of points for ever.
    meta_error: bool,
    /// `true` after a band request failed or the user cancelled it.
    ///
    /// The band asks one time for each set of columns, and a request that never
    /// answers would otherwise leave the band showing a row of points until the
    /// view changes. This flag stops the automatic ask, so a failure that repeats
    /// cannot make a storm of requests. The key `d` clears it, which gives the
    /// user a way to try again.
    band_error: bool,
    /// `true` after a statistics request failed or the user cancelled it. It
    /// works in the same way as [`App::band_error`], and the keys `i` and `M`
    /// clear it.
    stats_error: bool,
    /// The complete value in the cell inspector.
    pub cell_value: Option<String>,
    /// The first line that the cell inspector draws.
    pub cell_scroll: u16,
    /// `true` when the record view opened the cell inspector. The key `Esc`
    /// then goes back to the record view, and not to the grid.
    cell_from_record: bool,
    /// The title of the cell inspector, when the value comes from a path
    /// inside a structure and not from a column.
    pub cell_title: Option<String>,

    /// The selected line of the record view, as a position in the list from
    /// [`App::record_lines`].
    pub record_sel: usize,
    /// The text that selects the lines of the record view. An empty text
    /// shows each line.
    pub record_find: String,
    /// `true` while the user types in the find box of the record view.
    pub record_finding: bool,
    /// The settings that Peruse keeps between sessions.
    pub config: Config,
    /// The file that holds the settings.
    ///
    /// A test points this at its own file. Without it, a test would write the
    /// settings of the user who runs it.
    pub config_path: Option<std::path::PathBuf>,
    /// What the machine gives to Peruse.
    pub resources: Resources,
    /// The selected setting in the settings page.
    pub settings_sel: usize,
    /// `true` while the user types the value of a setting.
    pub settings_editing: bool,
    /// The values that DuckDB uses now, for the settings page.
    pub duck_threads: Option<String>,
    /// The memory limit that DuckDB uses now.
    pub duck_memory: Option<String>,

    /// The row of the record view, as a tree that opens and closes.
    pub record_tree: Tree,
    /// The row that the tree holds. The record view asks the engine again
    /// when the cursor moves to another row.
    record_row: Option<u64>,

    /// The filter, as a list of conditions. It is the source of the text in
    /// [`View::filter`], and the filter builder edits it.
    pub fset: FilterSet,
    /// The copy of the filter from the time before the user opened the
    /// builder. The key `Esc` puts it back.
    fset_saved: FilterSet,
    /// The step that the filter builder is at.
    pub build: Build,
    /// The selected condition in the list of the builder.
    pub build_sel: usize,
    /// The selected entry in the list of columns or the list of operators.
    pub pick_sel: usize,
    /// The condition that the builder makes now.
    pub draft: Draft,

    /// The value that the search looks for.
    pub needle: String,
    /// The offsets of the rows that match, from the smallest to the largest.
    pub hits: Vec<u64>,
    /// `true` after a scan covers the full view. The list `hits` then holds
    /// each match.
    hits_complete: bool,
    /// The search that runs now.
    scan: Option<Scan>,

    /// The position of the selected command in the palette.
    pub palette_sel: usize,
    /// The first line that the help overlay draws.
    pub help_scroll: u16,
    /// The position of the selected theme in the theme picker.
    pub theme_sel: usize,

    /// `true` when the engine can go directly to any row.
    pub seekable: bool,
    /// `true` while the worker indexes the CSV file.
    pub indexing: bool,
    /// `true` after the user asks Peruse to quit.
    pub quit: bool,
}

impl App {
    /// Makes the application state and asks for the first view.
    ///
    /// If `auto_index` is `true` and the CSV file is small, this function also
    /// asks the worker to index the file.
    pub fn new(worker: Worker, opened: Opened, theme: Theme, auto_index: bool) -> App {
        let ncols = opened.schema.len();
        let themes = peruse_core::theme::available();
        let theme_idx = themes.iter().position(|t| t.name == theme.name).unwrap_or(0);

        let mut app = App {
            worker,
            source: opened.source,
            read_expr: opened.read_expr,
            schema: opened.schema,
            view: View::default(),
            epoch: 0,

            applied: ViewStep {
                view: View::default(),
                fset: FilterSet::default(),
            },
            history: Vec::new(),
            undone: Vec::new(),
            restoring: false,

            page: RowPage::default(),
            page_limit: 0,
            requested: None,
            total: RowCount::Counting,

            cursor_row: 0,
            cursor_col: 0,
            top_row: 0,
            left_col: 0,
            viewport_rows: 20,
            hit: Hit::default(),
            overlay: None,
            clicks: Clicks::default(),

            widths: vec![12; ncols],
            hidden: vec![false; ncols],
            widths_fitted: false,

            theme,
            themes,
            theme_idx,

            mode: Mode::Normal,
            panel: Panel::None,
            input: LineInput::default(),
            prompt_error: None,
            filter_input: LineInput::default(),
            sql_input: LineInput::default(),
            search_input: LineInput::default(),

            status: None,
            busy: false,

            stats_cache: std::collections::HashMap::new(),
            stats_pending: None,
            band_cache: std::collections::HashMap::new(),
            band_asked: std::collections::HashSet::new(),
            band_asked_values: false,
            meta: None,
            meta_asked: false,
            meta_error: false,
            band_error: false,
            stats_error: false,
            cell_value: None,
            cell_scroll: 0,
            cell_from_record: false,
            cell_title: None,

            record_sel: 0,
            record_find: String::new(),
            record_finding: false,
            config: Config::default(),
            config_path: Config::path(),
            resources: Resources::read(),
            settings_sel: 0,
            settings_editing: false,
            duck_threads: None,
            duck_memory: None,

            record_tree: Tree::default(),
            record_row: None,

            fset: FilterSet::default(),
            fset_saved: FilterSet::default(),
            build: Build::List,
            build_sel: 0,
            pick_sel: 0,
            draft: Draft::default(),

            needle: String::new(),
            hits: Vec::new(),
            hits_complete: false,
            scan: None,

            palette_sel: 0,
            help_scroll: 0,
            theme_sel: 0,

            seekable: opened.seekable,
            indexing: false,
            quit: false,
        };
        app.reload(true);
        if auto_index
            && !app.seekable
            && app.source.bytes < AUTO_INDEX_BYTES
            && app.schema.len() <= AUTO_INDEX_COLUMNS
        {
            app.indexing = true;
            app.worker.send(Request::Index { epoch: app.epoch });
        }
        app
    }

    // ------------------------------------------------------- helper functions

    /// Gives the position of each column that the grid draws.
    pub fn visible_columns(&self) -> Vec<usize> {
        (0..self.schema.len()).filter(|i| !self.hidden[*i]).collect()
    }

    /// Gives the offset of the last row.
    ///
    /// The function gives `u64::MAX` while the number of rows is unknown.
    pub fn max_row(&self) -> u64 {
        if let Some(t) = self.total.value() {
            return t.saturating_sub(1);
        }
        if self.page_is_last() {
            return (self.page.offset + self.page.nrows as u64).saturating_sub(1);
        }
        u64::MAX
    }

    /// Gives `true` when the current page holds the last row of the view.
    fn page_is_last(&self) -> bool {
        self.page_limit > 0 && (self.page.nrows as u32) < self.page_limit
    }

    /// Gives `true` when the view holds no row.
    pub fn is_empty(&self) -> bool {
        matches!(self.total, RowCount::Exact(0))
    }

    /// Shows a message about the state of the program.
    pub fn info(&mut self, msg: impl Into<String>) {
        self.status = Some(Status { text: msg.into(), kind: StatusKind::Info });
    }
    /// Shows a message about an operation that succeeded.
    pub fn ok(&mut self, msg: impl Into<String>) {
        self.status = Some(Status { text: msg.into(), kind: StatusKind::Ok });
    }
    /// Shows a message about a failure.
    pub fn error(&mut self, msg: impl Into<String>) {
        self.status = Some(Status { text: msg.into(), kind: StatusKind::Error });
    }

    /// Discards each result of the old view and asks the worker for the new
    /// view. The function also increases the epoch.
    ///
    /// Each change of the view passes through here, so this is the one place
    /// that can remember what the user had before. A key that goes back needs
    /// no help from the code that made the change.
    fn reload(&mut self, reset_cursor: bool) {
        let now = ViewStep {
            view: self.view.clone(),
            fset: self.fset.clone(),
        };
        if now != self.applied {
            let before = std::mem::replace(&mut self.applied, now);
            self.history.push(before);
            if self.history.len() > MAX_HISTORY {
                self.history.remove(0);
            }
            // A new change makes the way forward meaningless. A step that
            // `u` removed can no longer follow the view that the user is on.
            if !self.restoring {
                self.undone.clear();
            }
        }
        self.epoch += 1;
        self.total = RowCount::Counting;
        self.page = RowPage::default();
        self.page_limit = 0;
        self.requested = None;
        // The statistics describe the rows of one view. A new view therefore
        // makes each answer wrong. The facts of the band follow the same rule.
        self.stats_cache.clear();
        self.stats_pending = None;
        // A new view is a new question, so a failure over the old view says
        // nothing about it. The two parts ask again.
        self.band_error = false;
        self.stats_error = false;
        self.band_cache.clear();
        self.band_asked.clear();
        // A match offset has no meaning in a different view.
        self.hits.clear();
        self.hits_complete = false;
        self.scan = None;
        if reset_cursor {
            self.cursor_row = 0;
            self.top_row = 0;
        }
        let limit = self.window_limit();
        self.worker.send(Request::SetView {
            epoch: self.epoch,
            view: self.view.clone(),
            limit,
        });
    }

    /// Applies a view that the command line gives, after the start.
    pub fn run_startup_view(&mut self) {
        self.reload(true);
    }

    /// Gives the number of rows to ask for in one page request.
    fn window_limit(&self) -> u32 {
        (self.viewport_rows as u64 + 2 * PREFETCH) as u32
    }

    /// Asks the worker for the rows that the grid needs.
    ///
    /// Peruse calls this function after each frame, because the true height of
    /// the viewport is known only then.
    pub fn ensure_rows(&mut self) {
        self.ensure_stats();
        self.ensure_band();
        if self.schema.is_empty() {
            return;
        }
        let visible = self.viewport_rows.max(1) as u64;
        let start = self.top_row.saturating_sub(PREFETCH);
        let limit = self.window_limit();

        let covered = self.page.ncols > 0
            && self.top_row >= self.page.offset
            && (self.top_row + visible <= self.page.offset + self.page.nrows as u64
                || self.page_is_last());
        if covered || self.requested == Some((start, limit)) {
            return;
        }
        self.requested = Some((start, limit));
        self.worker.send(Request::Page {
            epoch: self.epoch,
            view: self.view.clone(),
            schema: self.schema.clone(),
            offset: start,
            limit,
        });
    }

    /// Moves the viewport, if necessary, to keep the cursor on the screen.
    fn follow_cursor(&mut self) {
        let vis = self.viewport_rows.max(1) as u64;
        if self.cursor_row < self.top_row {
            self.top_row = self.cursor_row;
        } else if self.cursor_row >= self.top_row + vis {
            self.top_row = self.cursor_row - vis + 1;
        }
    }

    /// Gives the number of rows or columns that one step moves.
    pub fn step(&self) -> i64 {
        step_of(self.config.step)
    }

    /// Moves the cursor `delta` rows. A negative value moves it up the view.
    fn move_rows(&mut self, delta: i64) {
        let max = self.max_row();
        let next = if delta < 0 {
            self.cursor_row.saturating_sub(delta.unsigned_abs())
        } else {
            self.cursor_row.saturating_add(delta as u64).min(max)
        };
        self.cursor_row = next;
        self.follow_cursor();
    }

    /// Moves the cursor `delta` columns. A negative value moves it to the
    /// left. The function skips each column that the grid does not draw.
    fn move_cols(&mut self, delta: i64) {
        let vis = self.visible_columns();
        if vis.is_empty() {
            return;
        }
        let at = vis.iter().position(|c| *c == self.cursor_col).unwrap_or(0);
        let next = (at as i64 + delta).clamp(0, vis.len() as i64 - 1) as usize;
        self.cursor_col = vis[next];
    }

    /// Puts the panels of the settings file on the screen at the start.
    ///
    /// A name that Peruse does not know leaves the screen with no panel. A
    /// setting is not a reason to refuse to open a file.
    pub fn set_panel_from_setting(&mut self, name: &str) {
        match Panel::parse(name) {
            Some(p) => {
                self.panel = p;
                self.after_panel_change();
            }
            None => self.error(format!("settings: {name:?} is not a panel")),
        }
    }

    /// Clears the record of a statistics request that failed.
    ///
    /// A key that opens the statistics panel is the way back after a failure. The
    /// press is a request from the user for the numbers, and the answer may
    /// arrive this time.
    fn retry_stats(&mut self) {
        self.stats_error = false;
    }

    /// Asks for what a new panel needs.
    ///
    /// The statistics come at the next frame, through
    /// [`App::ensure_stats`]. The metadata needs one request, and it needs it
    /// one time for the file.
    fn after_panel_change(&mut self) {
        // A press of a panel key is a request from the user for the numbers, so
        // it is also the way back after a request that failed.
        self.retry_stats();
        if self.panel.has_meta() {
            // A user who opens the panel again asks for the metadata again. One
            // request is enough while it can succeed, but a request that failed
            // must not leave the panel empty for the whole session. The key is
            // the second try.
            if self.meta_error {
                self.meta_asked = false;
                self.meta_error = false;
            }
            self.request_meta();
        }
    }

    /// Asks the worker for the metadata of the file, one time for the session.
    ///
    /// The metadata describes the file, and not the view, so one answer serves
    /// each view. Without the guard, a caller that runs at each frame would
    /// read the footer again and again.
    fn request_meta(&mut self) {
        if self.meta.is_some() || self.meta_asked {
            return;
        }
        self.meta_asked = true;
        self.worker.send(Request::Meta { epoch: self.epoch });
    }

    /// Gives the mode of the detail band.
    ///
    /// The setting is the only copy of this state, so the mode of the last
    /// session is on the screen at the start with no more work. A name that
    /// Peruse does not know leaves the band off: a setting is not a reason to
    /// refuse to open a file. The settings page refuses a bad name while the
    /// user types it.
    pub fn band(&self) -> Band {
        match &self.config.band {
            Some(name) => Band::parse(name).unwrap_or(Band::Off),
            None => Band::Off,
        }
    }

    /// Gives the facts of one column for the detail band, when Peruse has them.
    pub fn brief(&self, ci: usize) -> Option<&ColumnBrief> {
        let name = &self.schema.columns.get(ci)?.name;
        self.band_cache.get(name)
    }

    /// Asks for the facts of each column that the band draws.
    ///
    /// One request covers every column that the grid draws now. The width of the
    /// terminal bounds that number, so the query stays small. One request for
    /// each column would read the view again for each column.
    ///
    /// The caller runs this one time for each frame, after the frame. The list
    /// of drawn columns comes from the frame, in [`Hit::cols`].
    fn ensure_band(&mut self) {
        if self.band() == Band::Off {
            return;
        }
        // A request that failed, or that the user cancelled, must not start
        // again at the next frame. The key `d` clears this flag.
        if self.band_error {
            return;
        }
        // A short grid gives the rows of the band back to the data, and the band
        // is then not on the screen at all. A query for facts that nothing draws
        // reads the whole file for nothing. The frame writes this count, so it
        // holds the mode of this frame. Refer to [`crate::grid::band_rows`].
        if self.hit.band == 0 {
            return;
        }
        let drawn: Vec<peruse_core::Column> = self
            .hit
            .cols
            .iter()
            .filter_map(|(ci, _, _)| self.schema.columns.get(*ci))
            .cloned()
            .collect();
        if drawn.is_empty() {
            return;
        }
        // The cache answers first, because it answers in constant time. A column
        // that Peruse asked about needs nothing more, whether the answer arrived
        // or not.
        let band = self.band();
        // A request that has no answer yet counts as an answer, but only when it
        // measures enough for the mode on the screen now. A compact request
        // measures two counts, so a change to the detailed band must ask again.
        let asked_enough = self.band_asked_values || !band.needs_values();
        let missing = drawn.iter().any(|c| {
            !brief_is_enough(self.band_cache.get(&c.name), band)
                && !(asked_enough && self.band_asked.contains(&c.name))
        });
        if !missing {
            return;
        }
        if !band.needs_values() && self.band_from_footer(&drawn) {
            return;
        }
        // The set holds the columns of this request, and not the columns of each
        // request of this view. The worker drops the older request, so its
        // columns need an answer again. Refer to [`App::band_asked`].
        self.band_asked = drawn.iter().map(|c| c.name.clone()).collect();
        self.band_asked_values = band.needs_values();
        self.worker.send(Request::Band {
            epoch: self.epoch,
            view: self.view.clone(),
            columns: drawn,
            // The compact band draws the share of NULL values alone. Each of the
            // three other facts reads the whole column, so the mode has to travel
            // with the request.
            values: band.needs_values(),
        });
    }

    /// Fills the band from the footer of a Parquet file, with no query.
    ///
    /// The footer holds the number of rows and the number of NULL values of each
    /// column. The compact band shows the type and the NULL share only, so the
    /// band over a plain Parquet file costs no query at all, also on a file of
    /// some gigabytes.
    ///
    /// The function gives `true` when the caller must not ask the engine. That
    /// covers the moment while the footer is on its way: the band shows a row of
    /// points until it arrives, and a wrong number never appears.
    fn band_from_footer(&mut self, drawn: &[peruse_core::Column]) -> bool {
        if !footer_can_answer(self.band(), &self.view, self.source.format) {
            return false;
        }
        if self.meta.is_none() {
            self.request_meta();
            // Wait for the footer. The band shows a row of points until it
            // arrives, and a wrong number never appears. A request that failed
            // gives no answer, so the caller then asks the engine.
            return !self.meta_error;
        }
        let briefs = self.meta.as_ref().and_then(|m| footer_briefs(m, drawn));
        match briefs {
            Some(briefs) => {
                for b in briefs {
                    self.band_cache.insert(b.column.clone(), b);
                }
                true
            }
            // The footer cannot answer for each column. One reason is a column
            // that holds a structure: the footer names each value inside it by
            // its path, and not by the name of the column.
            None => false,
        }
    }

    /// Gives the statistics of the column under the cursor, when the engine
    /// measured them already.
    pub fn stats(&self) -> Option<&ColumnStats> {
        let name = &self.schema.columns.get(self.cursor_col)?.name;
        self.stats_cache.get(name)
    }

    /// Asks the worker for the statistics of the column under the cursor,
    /// when the screen needs them and the cache does not hold them.
    ///
    /// The caller runs this one time for each frame, and not one time for
    /// each press of a key. A user who holds a key down moves across many
    /// columns between two frames, and each of those columns would otherwise
    /// start a scan that nobody reads.
    fn ensure_stats(&mut self) {
        if !self.panel.has_stats() {
            return;
        }
        // A request that failed, or that the user cancelled, must not start
        // again at the next frame. The keys `i` and `M` clear this flag.
        if self.stats_error {
            return;
        }
        let Some(col) = self.schema.columns.get(self.cursor_col).cloned() else {
            return;
        };
        if self.stats_cache.contains_key(&col.name)
            || self.stats_pending.as_deref() == Some(col.name.as_str())
        {
            return;
        }
        self.stats_pending = Some(col.name.clone());
        self.worker.send(Request::Stats {
            epoch: self.epoch,
            view: self.view.clone(),
            column: col,
            top_k: 8,
        });
    }

    /// Fits each column width to the values on the screen.
    ///
    /// The width covers the name of the column, the type character, and the
    /// widest value in the page. Refer to [`fitted_width`] for the room after
    /// the name and for the limit.
    pub fn fit_widths(&mut self) {
        for (i, col) in self.schema.columns.iter().enumerate() {
            let name = crate::text::width(&col.name);
            // Start at zero. The floor of the name belongs to `fitted_width`, and
            // a second floor here would only hide which rule set the width.
            let mut w = 0usize;
            for r in 0..self.page.nrows {
                let cell = self.page.cell(r, i).unwrap_or("NULL");
                // The loop needs to know whether the value reaches the limit,
                // and not how wide it is. A cell can hold 4096 characters.
                w = w.max(crate::text::width_capped(cell, MAX_COL_WIDTH as usize));
                if w >= MAX_COL_WIDTH as usize {
                    break;
                }
            }
            self.widths[i] = fitted_width(name, w);
        }
        self.widths_fitted = true;
    }

    // -------------------------------------------------------------- responses

    /// Adds one response from the worker to the state.
    ///
    /// The function discards each response with an old epoch, because the view
    /// changed after the request.
    pub fn on_response(&mut self, resp: Response) -> bool {
        // The response Busy is the one response with no view behind it.
        if let Response::Busy(b) = resp {
            // A response that repeats the state that the screen shows already
            // needs no frame. The worker reports Busy at the start and at the
            // end of each request, and a group of keys makes many of them.
            let changed = self.busy != b;
            self.busy = b;
            return changed;
        }
        // The index and the metadata describe the file, and not the view. A
        // change of the view must therefore not discard them. Without this
        // test, a start with `--filter` leaves the note "press I to index" on
        // the screen for the whole session, because the view changed before
        // the answer arrived.
        match resp {
            Response::Indexed { .. } => {
                self.indexing = false;
                self.seekable = true;
                self.ok("indexed — jumping is now instant");
                return true;
            }
            Response::Meta { ref meta, .. } => {
                self.meta = Some((**meta).clone());
                return true;
            }
            // A failure to read the metadata also describes the file, and not
            // the view. The detail band waits for the footer of a Parquet file,
            // so it must learn that no footer is coming, whatever the epoch of
            // the answer is.
            Response::Error { ref context, .. } if context == "metadata" => {
                self.meta_error = true;
            }
            _ => {}
        }
        let epoch = match &resp {
            Response::Schema { epoch, .. }
            | Response::Page { epoch, .. }
            | Response::Count { epoch, .. }
            | Response::Stats { epoch, .. }
            | Response::Band { epoch, .. }
            | Response::Cell { epoch, .. }
            | Response::RowJson { epoch, .. }
            | Response::Configured { epoch, .. }
            | Response::Search { epoch, .. }
            | Response::Meta { epoch, .. }
            | Response::Indexed { epoch }
            | Response::Error { epoch, .. } => *epoch,
            Response::Busy(_) => unreachable!(),
        };
        if epoch != self.epoch {
            // A response for a view that the user left changes nothing on the
            // screen, so it needs no frame.
            return false;
        }

        match resp {
            Response::Busy(_) => {}
            Response::Schema { schema, .. } => {
                let changed = schema.len() != self.schema.len()
                    || schema
                        .columns
                        .iter()
                        .zip(&self.schema.columns)
                        .any(|(a, b)| a.name != b.name);
                self.schema = schema;
                if changed {
                    self.widths = vec![12; self.schema.len()];
                    self.hidden = vec![false; self.schema.len()];
                    self.widths_fitted = false;
                    self.cursor_col = 0;
                    self.left_col = 0;
                }
            }
            Response::Page { page, .. } => {
                self.page_limit = self.requested.map(|(_, l)| l).unwrap_or(page.nrows as u32);
                self.page = page;
                self.requested = None;
                if !self.widths_fitted && self.page.nrows > 0 {
                    self.fit_widths();
                }
                // The number of rows can become known here. A cursor after
                // the last row must then move back to the last row.
                let max = self.max_row();
                if self.cursor_row > max {
                    self.cursor_row = max;
                    self.follow_cursor();
                }
            }
            Response::Count { total, .. } => {
                self.total = RowCount::Exact(total);
                let max = self.max_row();
                if self.cursor_row > max {
                    self.cursor_row = max;
                    self.follow_cursor();
                }
            }
            Response::Stats { stats, .. } => {
                if self.stats_pending.as_deref() == Some(stats.column.as_str()) {
                    self.stats_pending = None;
                }
                self.stats_cache.insert(stats.column.clone(), *stats);
            }
            Response::Band { briefs, .. } => {
                for b in briefs {
                    self.band_cache.insert(b.column.clone(), b);
                }
            }
            Response::RowJson { row, json, .. } => {
                // The user can move to another row while this answer is on
                // its way. The epoch cannot see that, because the view did
                // not change, so the row itself is the test.
                if Some(row) != self.record_row {
                    return false;
                }
                match json {
                    // The lines that are open, and the rule about the empty
                    // fields, both follow the user to the new row.
                    Some(j) => {
                        let old = std::mem::take(&mut self.record_tree);
                        self.record_tree = old.with_row(&j);
                    }
                    None => self.record_tree = Tree::default(),
                }
                let n = self.record_lines().len();
                self.record_sel = self.record_sel.min(n.saturating_sub(1));
            }
            Response::Cell { value, .. } => {
                self.cell_value = Some(value.unwrap_or_else(|| "NULL".into()));
                self.cell_scroll = 0;
            }
            Response::Configured { threads, memory_limit, .. } => {
                self.duck_threads = threads;
                self.duck_memory = memory_limit;
            }
            Response::Search { hits, .. } => self.apply_search(hits),
            // The code above handles these two, before the test of the epoch.
            Response::Indexed { .. } | Response::Meta { .. } => {}
            Response::Error { context, message, .. } => {
                self.indexing = false;
                // A request that failed must not leave its part of the screen
                // waiting for ever. Each of these two parts asks one time and
                // then waits for the answer, so a failure has to say that no
                // answer is coming. Without this, the band shows a row of points
                // and the statistics panel says "computing" until the view
                // changes.
                //
                // The flag stops the automatic ask, and it does not clear the
                // record of what Peruse asked for. A clear would ask again at the
                // next frame, and a request that fails every time would then make
                // a storm. The key of each part clears the flag instead.
                match context.as_str() {
                    "column band" => self.band_error = true,
                    "column stats" => {
                        self.stats_error = true;
                        self.stats_pending = None;
                    }
                    _ => {}
                }
                // The first line holds the useful part. DuckDB adds the full
                // statement after it, and that text is too long for a status
                // line of one row.
                let first = message.lines().next().unwrap_or(&message).trim().to_string();
                self.error(format!("{context}: {first}"));
            }
        }
        true
    }

    /// Finds the nearest known match in the given direction. The function does
    /// not ask the engine.
    ///
    /// With `inclusive`, a match on the row of the cursor counts. A search
    /// that the user types needs that behavior.
    fn known_hit(&self, forward: bool, inclusive: bool) -> Option<u64> {
        let c = self.cursor_row;
        if forward {
            self.hits
                .iter()
                .find(|h| if inclusive { **h >= c } else { **h > c })
                .copied()
        } else {
            self.hits
                .iter()
                .rev()
                .find(|h| if inclusive { **h <= c } else { **h < c })
                .copied()
        }
    }

    /// Moves the cursor to a match and reports the position of that match.
    fn goto_hit(&mut self, row: u64, note: &str) {
        self.cursor_row = row;
        self.follow_cursor();
        let at = self.hits.iter().position(|h| *h == row).unwrap_or(0) + 1;
        let n = self.hits.len();
        let total = if self.hits_complete {
            n.to_string()
        } else {
            format!("{n}+")
        };
        self.ok(format!("match {at}/{total}{note}"));
    }

    /// Moves the cursor to the next match or to the previous match.
    fn search(&mut self, forward: bool) {
        if self.needle.is_empty() {
            self.error("no search term — press / first");
            return;
        }
        if let Some(row) = self.known_hit(forward, false) {
            self.goto_hit(row, "");
            return;
        }
        if self.hits_complete {
            // Peruse knows each match already. The cursor therefore goes to
            // the other end of the view. A new scan of the file is not
            // necessary.
            let target = if forward {
                self.hits.first().copied()
            } else {
                self.hits.last().copied()
            };
            match target {
                Some(row) => self.goto_hit(row, " (wrapped)"),
                None => self.error(format!("no match for {:?}", self.needle)),
            }
            return;
        }
        let from = if forward {
            self.cursor_row.saturating_add(1)
        } else {
            self.cursor_row
        };
        self.start_scan(from, forward, false);
    }

    /// Starts a search at the row `from`.
    fn start_scan(&mut self, from: u64, forward: bool, inclusive: bool) {
        let Some(total) = self.total.value() else {
            // A row offset has a meaning only with a known number of rows.
            // The worker counts the rows already, so the user can try again.
            self.info("still counting — try again in a moment");
            return;
        };
        self.scan = Some(Scan {
            forward,
            inclusive,
            next: from,
            examined: 0,
            total,
            chunk: SEARCH_CHUNK,
        });
        self.request_scan_chunk();
    }

    /// Asks the worker to examine the next part of the view.
    ///
    /// The first call covers [`SEARCH_CHUNK`] rows, and each call after it
    /// covers two times the rows of the one before, up to
    /// [`SEARCH_CHUNK_MAX`]. The first answer therefore arrives at once, and
    /// a search of a large file does not read that file forty times.
    ///
    /// The user interface stays live through each part, and the key `Esc`
    /// stops the search inside one.
    fn request_scan_chunk(&mut self) {
        let Some(s) = self.scan.as_mut() else { return };
        let chunk = s.chunk;
        s.chunk = (s.chunk * 2).min(SEARCH_CHUNK_MAX);
        let (from, len) = if s.forward {
            let from = if s.next >= s.total { 0 } else { s.next };
            let len = chunk.min(s.total.saturating_sub(from)).max(1);
            s.next = from + len;
            (from, len)
        } else {
            let end = if s.next == 0 { s.total } else { s.next };
            let from = end.saturating_sub(chunk);
            s.next = from;
            (from, end - from)
        };
        s.examined = s.examined.saturating_add(len);
        let pct = (s.examined.min(s.total) * 100) / s.total.max(1);
        self.info(format!("searching… {pct}%  (Esc cancels)"));
        self.worker.send(Request::Search {
            epoch: self.epoch,
            view: self.view.clone(),
            schema: self.schema.clone(),
            needle: self.needle.clone(),
            from,
            scan: len,
            limit: SEARCH_HITS,
        });
    }

    /// Adds the matches from one part of the view to the state.
    ///
    /// The function then moves the cursor to a match, or asks for the next
    /// part of the view, or reports that the search found nothing.
    fn apply_search(&mut self, hits: Vec<u64>) {
        let Some(s) = self.scan.as_ref() else { return };
        let (forward, inclusive) = (s.forward, s.inclusive);
        let done = s.examined >= s.total;

        let found = !hits.is_empty();
        for h in hits {
            if let Err(i) = self.hits.binary_search(&h) {
                self.hits.insert(i, h);
            }
        }

        if found {
            self.scan = None;
            let start = self.cursor_row;
            // If no match is in front of the cursor, the cursor goes to the
            // other end of the view.
            let target = self.known_hit(forward, inclusive).or_else(|| {
                if forward {
                    self.hits.first().copied()
                } else {
                    self.hits.last().copied()
                }
            });
            match target {
                Some(row) => {
                    let wrapped = if forward { row < start } else { row > start };
                    self.goto_hit(row, if wrapped { " (wrapped)" } else { "" });
                }
                None => self.error(format!("no match for {:?}", self.needle)),
            }
            return;
        }

        if done {
            self.scan = None;
            self.hits_complete = true;
            if self.hits.is_empty() {
                self.error(format!("no match for {:?}", self.needle));
            } else {
                self.error("no further match");
            }
            return;
        }
        self.request_scan_chunk();
    }

    // -------------------------------------------------------------- the input

    /// Gives one key to the part of Peruse that has the focus.
    pub fn on_key(&mut self, key: &KeyEvent) {
        self.status = None;
        match self.mode {
            Mode::Prompt(kind) => self.prompt_key(kind, key),
            Mode::Help => self.overlay_key(key, |app, d| {
                app.help_scroll = app.help_scroll.saturating_add_signed(d as i16);
            }),
            Mode::Cell => self.overlay_key(key, |app, d| {
                app.cell_scroll = app.cell_scroll.saturating_add_signed(d as i16);
            }),
            Mode::Record => self.record_key(key),
            Mode::Settings => self.settings_key(key),
            Mode::FilterBuild => self.build_key(key),
            Mode::Palette => self.palette_key(key),
            Mode::ThemePicker => self.theme_picker_key(key),
            Mode::Normal => {
                if let Some(cmd) = commands::resolve(key) {
                    self.run(cmd);
                }
            }
        }
    }

    /// Gives one mouse event to the part of Peruse that has the focus.
    ///
    /// The function gives `true` when the event changed something. A mouse with
    /// capture on also reports each movement of the pointer, and Peruse does
    /// nothing with those. The caller draws no frame for them.
    pub fn on_mouse(&mut self, ev: &MouseEvent) -> bool {
        // Look for the double click first, and in each mode. A press that lands
        // on nothing still ends the pair, so a press in a panel and a press in
        // the grid can never join into one double click.
        let double = ev.kind == MouseEventKind::Down(MouseButton::Left)
            && self.clicks.press_now(ev.column, ev.row);
        match self.mode {
            Mode::Normal => self.grid_mouse(ev, double),
            // A prompt keeps the grid on the screen, and a user who writes a
            // filter looks at the data while doing it. The wheel therefore
            // still moves the grid. The arrow keys stay with the history of
            // the prompt, and a click does nothing.
            Mode::Prompt(_) => self.wheel_grid(ev),
            _ => self.overlay_mouse(ev, double),
        }
    }

    /// Gives one key to the program, as a press with no modifier.
    ///
    /// The mouse uses this for each action that a key already has. The click
    /// then runs the code of the key, so the two can never disagree.
    fn key(&mut self, code: KeyCode) {
        self.on_key(&KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// Gives one mouse event to the overlay that is open.
    ///
    /// Each overlay has its own list, its own limits and its own preview. A
    /// turn of the wheel therefore becomes presses of an arrow key, and a click
    /// becomes the key that does the same thing. The code that handles the keys
    /// stays the one place that acts.
    fn overlay_mouse(&mut self, ev: &MouseEvent, double: bool) -> bool {
        match ev.kind {
            MouseEventKind::ScrollDown => return self.wheel_overlay(true),
            MouseEventKind::ScrollUp => return self.wheel_overlay(false),
            MouseEventKind::Down(MouseButton::Left) => {}
            _ => return false,
        }

        // The frame writes where the box is. With no box, the mouse knows
        // nothing about the screen, and it must not act.
        let Some(hit) = self.overlay.as_ref() else {
            return false;
        };
        if hit.mode != self.mode {
            return false;
        }
        let inside = hit.holds(ev.column, ev.row);
        let at = hit.line_at(ev.row);

        if !inside {
            self.close_overlay();
            return true;
        }
        // A click on the border, on a prompt, on a heading or on the row of
        // keys changes nothing. The caller then draws no frame.
        let Some(at) = at else { return false };
        // The second press of a double click acts on the line that the first
        // press chose, and never on the line that `at` names now. A new
        // selection moves the window of the list: a list keeps the selected
        // line near the middle, so the frame between the two presses puts
        // another line under the pointer. Without this rule, a double click in
        // a long list runs a command that the user never pointed at. The two
        // presses of a pair are at one position, so the line that the first
        // press chose is the line that the user means.
        match self.mode {
            Mode::Record => self.record_click(at, double),
            Mode::Palette => self.palette_click(at, double),
            Mode::ThemePicker => self.theme_click(at, double),
            Mode::Settings => self.settings_click(at, double),
            Mode::FilterBuild => self.build_click(at, double),
            // The help and the cell inspector hold text and no list. No key
            // selects a line there, so no click does either.
            _ => false,
        }
    }

    /// Closes the open overlay and gives the grid back.
    ///
    /// A click outside a box means "leave this box", and it must leave in one
    /// press. The key `Esc` cannot do that work here, although it looks like the
    /// same thing:
    ///
    /// * In the record view, `Esc` first clears the text of the find line. A
    ///   click on the grid behind an open find line then needed two clicks.
    /// * In the filter builder, `Esc` goes back one step. From the step that
    ///   chooses an operator, a click outside needed three.
    /// * In the settings page, `Esc` first leaves the value that the user edits.
    ///
    /// The two states below need more than a change of the mode.
    fn close_overlay(&mut self) {
        match self.mode {
            // The picker paints each theme while the user moves through the
            // list. Leaving must put the theme of the user back, as `Esc` does,
            // or a click outside would keep a theme that the user only looked at.
            Mode::ThemePicker => {
                self.theme = self.themes[self.theme_idx.min(self.themes.len() - 1)].clone();
                self.mode = Mode::Normal;
            }
            // The cell inspector can come from the record view. Leaving it gives
            // the record back, and not the grid.
            Mode::Cell if self.cell_from_record => {
                self.cell_from_record = false;
                self.mode = Mode::Record;
            }
            Mode::Settings => {
                self.settings_editing = false;
                self.mode = Mode::Normal;
            }
            _ => self.mode = Mode::Normal,
        }
    }

    /// Moves the list of the open overlay for one turn of the wheel.
    ///
    /// The turn becomes presses of an arrow key, so the wheel and the keys move
    /// the same list, in the same way, with one piece of code.
    ///
    /// A box that takes text is the one place where that rule fails. The arrow
    /// key belongs to the text there: it walks the history of the box and puts
    /// an older line in it. A turn of the wheel must never write in a box, so
    /// those three states move the list themselves, or move nothing.
    fn wheel_overlay(&mut self, down: bool) -> bool {
        match self.mode {
            // The find box of the record view holds the focus, and the list of
            // the fields is still on the screen under it. Move the list and
            // leave the text of the box as the user typed it.
            Mode::Record if self.record_finding => {
                self.record_move(if down { WHEEL_ROWS } else { -WHEEL_ROWS });
                true
            }
            // The value of a setting is being typed. The selection must stay
            // where it is: Enter writes the text into the setting under the
            // selection, so a wheel that moved it would write in another
            // setting. This is the rule that a click there follows too.
            Mode::Settings if self.settings_editing => false,
            // The two value steps and the SQL step hold a prompt and no list,
            // so there is nothing for the wheel to move.
            Mode::FilterBuild
                if matches!(self.build, Build::Value | Build::Value2 | Build::Raw) =>
            {
                false
            }
            _ => {
                // Read the position of each list before and after. A wheel at
                // the end of a list moves nothing, and the caller must then draw
                // no frame: a user who holds the wheel at the bottom of the help
                // would otherwise draw the same screen again and again.
                let before = self.overlay_scroll();
                let code = if down { KeyCode::Down } else { KeyCode::Up };
                for _ in 0..WHEEL_ROWS {
                    self.key(code);
                }
                self.overlay_scroll() != before
            }
        }
    }

    /// Gives the position of the list of each overlay, as one value.
    ///
    /// The wheel compares this before and after it moves, so it can say whether
    /// anything changed. One value for every overlay keeps the comparison in one
    /// place: a new overlay with a list adds its position here, and the wheel
    /// needs no change.
    fn overlay_scroll(&self) -> (u16, u16, usize, usize, usize, usize) {
        (
            self.help_scroll,
            self.cell_scroll,
            self.palette_sel,
            self.theme_sel,
            self.settings_sel,
            self.record_sel,
        )
    }

    /// Handles a click on a line of the record view.
    ///
    /// The rule is the rule of the grid: the first click on a line chooses it,
    /// and a click on the line that is chosen already acts on it. A user who
    /// only wants to read another line therefore never opens or closes a value
    /// by accident, and a user who wants to open one still needs one click.
    fn record_click(&mut self, at: usize, double: bool) -> bool {
        if double {
            // A double click does what Enter does: it opens a value that holds
            // other values, and it shows a single value in full.
            self.key(KeyCode::Enter);
            return true;
        }
        if at >= self.record_lines().len() {
            return false;
        }
        let same = self.record_sel == at && !self.record_finding;
        // The pointer names a line, so the find box gives up the focus. The key
        // Enter does the same in the find box, and it keeps the text.
        self.record_finding = false;
        self.record_sel = at;
        if !same {
            // The click only chose the line. The caller draws a frame, because
            // the mark of the selection moved.
            return true;
        }
        // The line was chosen already, so the click opens or closes the value,
        // as the key Space does. A line that holds no other values does nothing,
        // and the frame is then the same as the frame before it.
        let before = self.record_lines().len();
        self.key(KeyCode::Char(' '));
        self.record_lines().len() != before
    }

    /// Handles a click on a line of the command palette.
    fn palette_click(&mut self, at: usize, double: bool) -> bool {
        // A double click runs the command, as Enter does.
        if double {
            self.key(KeyCode::Enter);
            return true;
        }
        if at >= self.palette_items().len() {
            return false;
        }
        self.palette_sel = at;
        true
    }

    /// Handles a click on a line of the theme picker.
    fn theme_click(&mut self, at: usize, double: bool) -> bool {
        // A double click keeps the theme, as Enter does.
        if double {
            self.key(KeyCode::Enter);
            return true;
        }
        if at >= self.themes.len() {
            return false;
        }
        self.theme_preview(at);
        true
    }

    /// Handles a click on a line of the settings page.
    fn settings_click(&mut self, at: usize, double: bool) -> bool {
        // The value has the focus while the user types it. A click on another
        // setting would lose what the user typed, so the click does nothing.
        // Esc and Enter leave the value, as they do for the keys.
        if self.settings_editing {
            return false;
        }
        // A double click starts to edit the setting, as Enter does.
        if double {
            self.key(KeyCode::Enter);
            return true;
        }
        if at >= Setting::ALL.len() {
            return false;
        }
        self.settings_sel = at;
        true
    }

    /// Handles a click on a line of the filter builder.
    fn build_click(&mut self, at: usize, double: bool) -> bool {
        match self.build {
            Build::List => {
                // A double click edits the condition, as the key `e` does. The
                // key Enter applies the filter, and a click that lands on the
                // wrong row must not start a query of the whole file.
                if double {
                    self.key(KeyCode::Char('e'));
                    return true;
                }
                if at >= self.fset.conditions.len() {
                    return false;
                }
                self.build_sel = at;
                true
            }
            Build::Column => {
                if double {
                    self.key(KeyCode::Enter);
                    return true;
                }
                if at >= self.build_columns().len() {
                    return false;
                }
                self.pick_sel = at;
                true
            }
            Build::Op => {
                if double {
                    self.key(KeyCode::Enter);
                    return true;
                }
                if at >= self.build_ops().len() {
                    return false;
                }
                self.pick_sel = at;
                true
            }
            // The two value steps and the SQL step hold a prompt and no list.
            Build::Value | Build::Value2 | Build::Raw => false,
        }
    }

    /// Moves the grid for a turn of the wheel. Gives `true` when the event was
    /// a turn of the wheel.
    fn wheel_grid(&mut self, ev: &MouseEvent) -> bool {
        match wheel_of(ev) {
            Wheel::Rows(n) => self.move_rows(n),
            Wheel::Cols(n) => self.move_cols(n),
            Wheel::None => return false,
        }
        true
    }

    /// Gives one mouse event to the grid.
    fn grid_mouse(&mut self, ev: &MouseEvent, double: bool) -> bool {
        if self.wheel_grid(ev) {
            return true;
        }
        if ev.kind != MouseEventKind::Down(MouseButton::Left) {
            return false;
        }
        // A panel at the side of the grid covers the same rows as the grid. A
        // click in the panel must therefore not move the cursor of the grid.
        if !self.hit.holds(ev.column) {
            return false;
        }
        let on_labels = self.hit.on_labels(ev.row);
        let row = self.hit.row_at(ev.row);
        if !on_labels && row.is_none() {
            return false;
        }
        // A grid with room for more rows than the file holds keeps the rows
        // under the last one empty. There is no cell there, so a click must
        // change nothing: without this, a click on the empty part of the
        // screen would send the cursor to the last row of the file.
        if let Some(off) = row
            && self.top_row.saturating_add(off) > self.max_row()
        {
            return false;
        }
        // A click on the row of the names, or on the detail band, moves to that
        // column and does not sort. A sort of a large file costs seconds, and a
        // click that lands on the wrong column must not start that work. The key
        // `s` sorts.
        // The gutter of row numbers and the space after the last column hold
        // no column. A click there moves nothing, and the caller draws no
        // frame for it.
        // A click on a cell that the cursor is on already opens the record view,
        // and so does a double click. A click on any other cell moves the cursor
        // and no more.
        //
        // The rule gives the two things that a user wants from one button. A
        // click on a cell opens that record, which is what the user asked for. A
        // user who only wants to choose a cell still can, because the first click
        // on a cell never opens a box on top of the data. A file chooser of a
        // desktop works in this way.
        let was_here = row.is_some_and(|off| {
            let target = self.top_row.saturating_add(off);
            target == self.cursor_row
                && self.hit.col_at(ev.column).is_none_or(|ci| ci == self.cursor_col)
        });

        let mut acted = false;
        if let Some(ci) = self.hit.col_at(ev.column) {
            self.cursor_col = ci;
            acted = true;
        }
        if let Some(off) = row {
            self.cursor_row = self.top_row.saturating_add(off).min(self.max_row());
            self.follow_cursor();
            if double || was_here {
                self.open_record();
            }
            acted = true;
        }
        acted
    }

    /// Handles a key in the help overlay or in the cell inspector.
    fn overlay_key(&mut self, key: &KeyEvent, mut scroll: impl FnMut(&mut App, i32)) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                // The cell inspector can come from the record view. The user
                // must then go back to the record, and not to the grid.
                self.mode = if self.mode == Mode::Cell && self.cell_from_record {
                    self.cell_from_record = false;
                    Mode::Record
                } else {
                    Mode::Normal
                };
            }
            KeyCode::Char('j') | KeyCode::Down => scroll(self, 1),
            KeyCode::Char('k') | KeyCode::Up => scroll(self, -1),
            KeyCode::PageDown => scroll(self, 10),
            KeyCode::PageUp => scroll(self, -10),
            KeyCode::Char('y') if self.mode == Mode::Cell => {
                let v = self.cell_value.clone().unwrap_or_default();
                self.copy(&v, "cell");
            }
            _ => {}
        }
    }

    /// Handles a key in the command palette.
    fn palette_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Up => {
                self.palette_sel = self.palette_sel.saturating_sub(1);
                return;
            }
            KeyCode::Down => {
                self.palette_sel = (self.palette_sel + 1).min(self.palette_items().len().saturating_sub(1));
                return;
            }
            KeyCode::Enter => {
                let items = self.palette_items();
                if let Some(cmd) = items.get(self.palette_sel).copied() {
                    self.mode = Mode::Normal;
                    self.run(cmd);
                } else {
                    self.mode = Mode::Normal;
                }
                return;
            }
            _ => {}
        }
        if self.input.handle(key) == Action::Changed {
            self.palette_sel = 0;
        }
    }

    /// Gives each command that matches the text in the palette.
    pub fn palette_items(&self) -> Vec<Cmd> {
        let q = self.input.text();
        commands::BINDINGS
            .iter()
            .filter(|b| {
                commands::fuzzy_match(&q, b.desc)
                    || commands::fuzzy_match(&q, b.group)
                    || b.label.contains(q.trim())
            })
            .map(|b| b.cmd)
            .collect()
    }

    /// Handles a key in the theme picker.
    fn theme_picker_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // Go back to the theme from the time before the user opened
                // the picker.
                self.theme = self.themes[self.theme_idx].clone();
                self.mode = Mode::Normal;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.theme_preview(self.theme_sel.saturating_sub(1));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.theme_preview(self.theme_sel + 1);
            }
            KeyCode::Enter => {
                self.theme_idx = self.theme_sel;
                self.theme = self.themes[self.theme_idx].clone();
                self.mode = Mode::Normal;
                self.remember_theme();
            }
            _ => {}
        }
    }

    /// Selects a theme in the picker and paints with it at once.
    ///
    /// The preview is the reason that the picker exists: the name of a theme
    /// says nothing about its colors. The arrow keys and a click both come
    /// here, so the two always give the same result.
    fn theme_preview(&mut self, at: usize) {
        let at = at.min(self.themes.len().saturating_sub(1));
        let Some(theme) = self.themes.get(at).cloned() else {
            return;
        };
        self.theme_sel = at;
        self.theme = theme;
    }

    /// Completes the column name that the user started to type.
    ///
    /// The key `Tab` calls this function in the filter prompt and in the SQL
    /// prompt. A file can hold hundreds of columns with long names, and the
    /// user must not have to type such a name without a mistake.
    ///
    /// One match goes straight into the line. Some matches give the part that
    /// each of them starts with, and the prompt then shows the names.
    fn complete_column(&mut self) {
        let word = self.input.word_before_cursor();
        if word.is_empty() {
            self.prompt_error = Some("type the start of a column name first".into());
            return;
        }

        // A path such as `actor.log` names a field inside a structure. The
        // part in front of the last full stop says which structure, and the
        // part after it is what the user started to type.
        let (prefix, partial) = match word.rfind('.') {
            Some(i) => (&word[..i], &word[i + 1..]),
            None => ("", word.as_str()),
        };
        let names: Vec<String> = match self.fields_at(prefix) {
            Some(fields) => fields,
            None => {
                self.prompt_error = Some(format!("{prefix} holds no fields"));
                return;
            }
        };
        let lower = partial.to_lowercase();
        let matches: Vec<&str> = names
            .iter()
            .map(|s| s.as_str())
            .filter(|n| n.to_lowercase().starts_with(&lower))
            .collect();

        // The whole path goes back into the line, so the part in front of the
        // last full stop keeps its own quotation marks.
        let put = |app: &mut App, last: &str| {
            let mut out = String::new();
            for seg in prefix.split('.').filter(|s| !s.is_empty()) {
                out.push_str(&quote_if_needed(seg));
                out.push('.');
            }
            out.push_str(&quote_if_needed(last));
            app.input.replace_word_before_cursor(&out);
        };

        match matches.len() {
            0 if prefix.is_empty() => {
                self.prompt_error = Some(format!("no column starts with {partial:?}"))
            }
            0 => self.prompt_error = Some(format!("{prefix} has no field {partial:?}")),
            1 => {
                let only = matches[0].to_string();
                put(self, &only);
                self.prompt_error = None;
            }
            _ => {
                // Put in the part that each name starts with. The user can
                // then type one more character and press Tab again.
                let common = common_prefix(&matches);
                if common.chars().count() > partial.chars().count() {
                    put(self, &common);
                }
                let shown: Vec<&str> = matches.iter().take(6).copied().collect();
                let more = matches.len().saturating_sub(shown.len());
                self.prompt_error = Some(if more > 0 {
                    format!("{} … +{more}", shown.join(" "))
                } else {
                    shown.join(" ")
                });
            }
        }
    }

    /// Gives the part of a name that the user did not type yet.
    ///
    /// The prompt draws this text after the cursor, in a dim color. A user
    /// who types `am` sees `amount` at once, and does not have to remember
    /// the names or press a key to see them. The key `Tab` takes it.
    ///
    /// Each prompt with a known list of answers gets this help:
    ///
    /// | Prompt | The list |
    /// |---|---|
    /// | The filter and the SQL statement | the columns, and the fields inside them |
    /// | The text step of the filter builder | the same |
    /// | The find box of the record view | the fields of the row |
    /// | The value of a setting | the answers that the setting takes |
    ///
    /// The search prompt and the row-number prompt get none. Peruse cannot
    /// know what a user looks for.
    ///
    /// The text appears at the end of the line only. In the middle of a line
    /// there is no room for it: the text of the user is there.
    pub fn ghost(&self) -> Option<String> {
        if !self.input.cursor_at_end() {
            return None;
        }
        match self.mode {
            Mode::Prompt(PromptKind::Filter) | Mode::Prompt(PromptKind::Sql) => self.ghost_column(),
            Mode::FilterBuild if self.build == Build::Raw => self.ghost_column(),
            Mode::Record if self.record_finding => {
                let names: Vec<String> = self
                    .record_tree
                    .lines("")
                    .into_iter()
                    .map(|l| l.label)
                    .collect();
                ghost_from(&self.input.text(), &names)
            }
            Mode::Settings if self.settings_editing => {
                let s = Setting::ALL[self.settings_sel.min(Setting::ALL.len() - 1)];
                ghost_from(&self.input.text(), &self.setting_choices(s))
            }
            _ => None,
        }
    }

    /// Puts the ghost completion into the line, when the key asks for it.
    ///
    /// The function gives the complete text of the line after the change, so
    /// that a caller which keeps its own copy of that text can follow it.
    fn take_ghost(&mut self, key: &KeyEvent) -> Option<String> {
        let take = key.code == KeyCode::Tab
            || (key.code == KeyCode::Right && plain(key) && self.input.cursor_at_end());
        if !take {
            return None;
        }
        let rest = self.ghost()?;
        let full = format!("{}{rest}", self.input.text());
        self.input.set(&full);
        Some(full)
    }

    /// Gives the answers that one setting takes, for the ghost completion.
    ///
    /// A setting that takes a number has no list. A number is not a name, and
    /// no part of it says what the rest is.
    fn setting_choices(&self, s: Setting) -> Vec<String> {
        match s {
            Setting::Theme => self.themes.iter().map(|t| t.name.clone()).collect(),
            Setting::Panels => ["none", "meta", "stats", "both"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            Setting::Band => ["off", "compact", "detailed"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            Setting::NoIndex => vec!["yes".into(), "no".into()],
            Setting::Threads | Setting::MemoryLimit | Setting::SampleSize | Setting::Step => {
                Vec::new()
            }
        }
    }

    /// Gives the part of a column name, or of a field name, that the user did
    /// not type yet.
    fn ghost_column(&self) -> Option<String> {
        let word = self.input.word_before_cursor();
        let (prefix, partial) = match word.rfind('.') {
            Some(i) => (&word[..i], &word[i + 1..]),
            None => ("", word.as_str()),
        };
        if partial.is_empty() {
            return None;
        }
        ghost_from(partial, &self.fields_at(prefix)?)
    }

    /// Gives the names that a path can hold at its next level.
    ///
    /// An empty path gives the columns of the file. A path such as `actor`
    /// gives the fields of that structure, and `payload.commits` gives the
    /// fields of the structures inside that list.
    ///
    /// The function gives `None` when a step of the path names nothing, or
    /// names a value that holds no field.
    fn fields_at(&self, path: &str) -> Option<Vec<String>> {
        if path.is_empty() {
            return Some(self.schema.columns.iter().map(|c| c.name.clone()).collect());
        }
        let mut ty: Option<String> = None;
        for seg in path.split('.') {
            let seg = seg.trim_matches('"');
            let next = match &ty {
                // The first step names a column of the file.
                None => self
                    .schema
                    .columns
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(seg))
                    .map(|c| c.sql_type.clone())?,
                // Each step after it names a field of a structure. A list of
                // structures gives the fields of the structure inside it, so
                // that `payload.commits.sha` completes.
                Some(t) => {
                    let base = t.trim_end_matches("[]");
                    peruse_core::model::struct_fields(base)
                        .into_iter()
                        .find(|f| f.name.eq_ignore_ascii_case(seg))
                        .map(|f| f.sql_type)?
                }
            };
            ty = Some(next);
        }
        let t = ty?;
        let fields = peruse_core::model::struct_fields(t.trim_end_matches("[]"));
        if fields.is_empty() {
            return None;
        }
        Some(fields.into_iter().map(|f| f.name).collect())
    }

    /// Handles a key in the prompt.
    fn prompt_key(&mut self, kind: PromptKind, key: &KeyEvent) {
        // The line editor has no list of columns, so the completion happens
        // here, in front of it.
        if matches!(kind, PromptKind::Filter | PromptKind::Sql) {
            // The key Tab takes the completion. The key -> also takes it, at
            // the end of a line, where that key has nothing else to do. A
            // user of a shell knows that second form. Ctrl+-> and Alt+-> move
            // one word, so those two forms keep that operation.
            let take = key.code == KeyCode::Tab
                || (key.code == KeyCode::Right && plain(key) && self.ghost().is_some());
            if take {
                self.complete_column();
                return;
            }
        }
        match self.input.handle(key) {
            Action::Cancel => {
                // Keep the line in the history before the prompt closes. The key
                // `Esc` would otherwise throw away a statement of 200 characters
                // with no way back, and the arrow key of the history is the way
                // back that a user expects.
                //
                // A Mac makes this more than a convenience. A terminal with the
                // setting "Option as Esc+" sends `Esc` in front of the arrow key,
                // so `Option` with an arrow, which is the key for a jump of one
                // word, arrives here as a cancel. Without this line, that jump
                // deletes the work of the user.
                self.input.remember();
                self.mode = Mode::Normal;
                self.prompt_error = None;
            }
            Action::Submit => self.submit_prompt(kind),
            Action::Changed => {
                // Check the text while the user types it. A bad expression
                // is then visible before the user presses Enter, and not
                // after a request to the worker.
                self.prompt_error = match kind {
                    PromptKind::Filter if !self.input.is_empty() => {
                        sql_guard::ensure_safe_predicate(&self.input.text())
                            .err()
                            .map(|e| e.to_string())
                    }
                    PromptKind::Sql if !self.input.is_empty() => {
                        sql_guard::ensure_read_only(&self.input.text())
                            .err()
                            .map(|e| e.to_string())
                    }
                    _ => None,
                };
            }
            Action::Ignored => {}
        }
    }

    /// Applies the text of the prompt after the user presses Enter.
    fn submit_prompt(&mut self, kind: PromptKind) {
        let text = self.input.text();
        let trimmed = text.trim().to_string();
        match kind {
            PromptKind::Filter => {
                if !trimmed.is_empty()
                    && let Err(e) = sql_guard::ensure_safe_predicate(&trimmed) {
                        self.prompt_error = Some(e.to_string());
                        return;
                    }
                self.input.remember();
                self.filter_input = self.input.clone();
                // The expression becomes the complete filter, as one
                // condition of the list. The builder can then show it, and a
                // quick filter can add a condition beside it.
                self.set_raw_filter(&trimmed);
                self.mode = Mode::Normal;
                self.prompt_error = None;
                self.reload(true);
            }
            PromptKind::Sql => {
                // The prompt opens with a statement that ends at the word WHERE,
                // for the user to finish. A press of Enter on that text alone
                // asks the database to read a condition that is not there, and
                // the answer is a message from the parser of DuckDB.
                //
                // The cost is worse than the message: the code below clears the
                // filter and the sort before it reads the answer, so `e` and then
                // Enter would throw away the filter that the user is looking at.
                // Keep the prompt open instead, and say what is missing.
                if trimmed.ends_with("WHERE") || trimmed.ends_with("where") {
                    self.prompt_error =
                        Some("write a condition after WHERE, or press Esc".into());
                    return;
                }
                if trimmed.is_empty() {
                    self.view.base = Base::Source;
                } else {
                    if let Err(e) = sql_guard::ensure_read_only(&trimmed) {
                        self.prompt_error = Some(e.to_string());
                        return;
                    }
                    self.view.base = Base::Sql(trimmed);
                }
                self.input.remember();
                self.sql_input = self.input.clone();
                // The new statement can give other columns. A filter or a
                // sort on the old columns is then wrong, so remove them.
                self.fset.clear();
                self.view.filter = None;
                self.view.sort.clear();
                self.mode = Mode::Normal;
                self.prompt_error = None;
                self.reload(true);
            }
            PromptKind::Search => {
                self.input.remember();
                self.search_input = self.input.clone();
                self.needle = trimmed;
                self.mode = Mode::Normal;
                self.hits.clear();
                self.hits_complete = false;
                self.scan = None;
                if self.needle.is_empty() {
                    return;
                }
                // A new search includes the row of the cursor. The user
                // types the value on the screen, and the search must find
                // that value. It must not move past it.
                self.start_scan(self.cursor_row, true, true);
            }
            PromptKind::Goto => {
                self.mode = Mode::Normal;
                match trimmed.replace([',', '_'], "").parse::<u64>() {
                    // The first row on the screen has the number 1.
                    Ok(n) if n >= 1 => {
                        // While the count is unknown, `max_row` gives the
                        // largest number that 64 bits hold. A jump to that
                        // row would put the cursor on a row that does not
                        // exist, and the number of the row after it does not
                        // fit in 64 bits.
                        let max = self.max_row();
                        if max == u64::MAX {
                            self.info("still counting — try again in a moment");
                            return;
                        }
                        self.cursor_row = (n - 1).min(max);
                        self.follow_cursor();
                    }
                    _ => self.error(format!("not a row number: {trimmed:?}")),
                }
            }
        }
    }

    /// Opens the prompt and fills it with the text that the user needs.
    fn open_prompt(&mut self, kind: PromptKind) {
        self.prompt_error = None;
        self.input = match kind {
            PromptKind::Filter => {
                let mut i = self.filter_input.clone();
                i.set(self.view.filter.as_deref().unwrap_or(""));
                i
            }
            PromptKind::Sql => {
                let mut i = self.sql_input.clone();
                let current = match &self.view.base {
                    // Fill the prompt with the statement behind the grid. The
                    // user then corrects that statement, and does not write it
                    // again.
                    Base::Sql(s) => s.clone(),
                    // The grid reads the file, so the prompt has no statement
                    // of its own to show. It starts from the statement that
                    // most users want. Refer to `query::PROMPT_START`.
                    Base::Source => PROMPT_START.to_string(),
                };
                i.set(&current);
                i
            }
            PromptKind::Search => {
                let mut i = self.search_input.clone();
                i.set(&self.needle);
                i
            }
            PromptKind::Goto => {
                let mut i = LineInput::default();
                i.clear();
                i
            }
        };
        self.mode = Mode::Prompt(kind);
    }

    /// Copies a text to the clipboard and reports the result.
    fn copy(&mut self, text: &str, what: &str) {
        match clip::copy(text) {
            Ok(()) => self.ok(format!("copied {what} ({} chars)", text.chars().count())),
            Err(e) => self.error(format!("copy failed: {e}")),
        }
    }

    /// Gives the text in the cell under the cursor, from the current page.
    fn current_cell_text(&self) -> Option<String> {
        let r = self.cursor_row.checked_sub(self.page.offset)? as usize;
        Some(self.page.cell(r, self.cursor_col).unwrap_or("NULL").to_string())
    }

    // ---------------------------------------------------- the record view

    /// Gives the row of the cursor inside the current page.
    ///
    /// The function gives `None` when the page does not hold that row. The
    /// engine is then still reading it.
    pub fn page_row(&self) -> Option<usize> {
        let r = self.cursor_row.checked_sub(self.page.offset)? as usize;
        (r < self.page.nrows).then_some(r)
    }

    /// Gives the lines that the record view draws.
    ///
    /// The find box removes each line whose name and value do not hold the
    /// text. A file with 400 columns is one reason for that box, and a value
    /// that holds other values is the second: a match can be three levels
    /// down, and the user must see the way to it.
    ///
    /// The record view shows a hidden column too. The user opens this view to
    /// see the complete row, and a column that the grid hides is exactly the
    /// column that the user cannot see in the grid.
    pub fn record_lines(&self) -> Vec<Line> {
        self.record_tree.lines(&self.record_find)
    }

    /// Gives the selected line of the record view.
    pub fn record_line(&self) -> Option<Line> {
        self.record_lines().into_iter().nth(self.record_sel)
    }

    /// Gives the position in the schema of the column that holds the selected
    /// line. A line that is three levels down still belongs to one column.
    pub fn record_column(&self) -> Option<usize> {
        let line = self.record_line()?;
        match line.path.first()? {
            Step::Field(name) => self.schema.index_of(name),
            Step::Index(_) => None,
        }
    }

    /// Asks the engine for the row that the record view shows.
    ///
    /// The engine gives the row as JSON. A value that holds other values then
    /// has one form that this program can read, and the record view can open
    /// it. See [`crate::tree`].
    fn request_record(&mut self) {
        if self.schema.is_empty() {
            return;
        }
        self.record_row = Some(self.cursor_row);
        self.worker.send(Request::RowJson {
            epoch: self.epoch,
            view: self.view.clone(),
            schema: self.schema.clone(),
            row: self.cursor_row,
        });
    }

    /// Opens the record view on the row under the cursor.
    fn open_record(&mut self) {
        if self.schema.is_empty() {
            self.error("no columns to show");
            return;
        }
        if self.page_row().is_none() {
            self.error("no row here yet");
            return;
        }
        self.record_find.clear();
        self.record_finding = false;
        self.record_tree = Tree::default();
        // Start on the column under the cursor. The user then sees the value
        // that the cursor was on, with each other value of the same row.
        self.record_sel = self.cursor_col.min(self.schema.len() - 1);
        self.mode = Mode::Record;
        self.request_record();
    }

    /// Opens the record view with the column under the cursor already open.
    ///
    /// The key `Enter` on a cell that holds other values comes here. The user
    /// asked to see one value, so the way into that value must need no second
    /// key.
    fn open_record_at_column(&mut self) {
        self.open_record();
        // The row is not there yet, and the tree holds no line. The list of
        // the lines that are open is a list of paths, and it survives the
        // row that arrives after it, so this works before the answer.
        if self.mode == Mode::Record
            && let Some(col) = self.schema.columns.get(self.cursor_col)
        {
            self.record_tree.open_path(&[Step::Field(col.name.clone())]);
        }
    }

    /// Closes the record view and moves the cursor of the grid to the field
    /// that the record view was on.
    fn close_record(&mut self) {
        if let Some(c) = self.record_column() {
            // Do not move the cursor to a column that the grid hides. The
            // cursor must always be on a column that the user can see.
            if !self.hidden[c] {
                self.cursor_col = c;
            }
        }
        self.mode = Mode::Normal;
    }

    /// Moves the selection in the record view.
    fn record_move(&mut self, delta: i64) {
        let n = self.record_lines().len();
        if n == 0 {
            return;
        }
        self.record_sel = (self.record_sel as i64 + delta).clamp(0, n as i64 - 1) as usize;
    }

    /// Handles a key in the record view.
    fn record_key(&mut self, key: &KeyEvent) {
        if self.record_finding {
            if let Some(rest) = self.take_ghost(key) {
                self.record_find = rest;
                self.record_sel = 0;
                return;
            }
            match self.input.handle(key) {
                // Esc removes the text of the find box, and shows each field
                // again. A second Esc closes the record view.
                Action::Cancel => {
                    self.record_finding = false;
                    self.record_find.clear();
                    self.record_sel = 0;
                }
                Action::Submit => self.record_finding = false,
                Action::Changed => {
                    self.record_find = self.input.text();
                    self.record_sel = 0;
                }
                Action::Ignored => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('r') => self.close_record(),
            KeyCode::Char('j') | KeyCode::Down => self.record_move(1),
            KeyCode::Char('k') | KeyCode::Up => self.record_move(-1),
            KeyCode::PageDown => self.record_move(10),
            KeyCode::PageUp => self.record_move(-10),
            KeyCode::Char('g') | KeyCode::Home => self.record_sel = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.record_sel = self.record_lines().len().saturating_sub(1);
            }
            // The next row and the previous row. The position in the list
            // stays, so the user can follow one field over some rows.
            KeyCode::Char('n') => self.step_record(1),
            KeyCode::Char('p') => self.step_record(-1),
            KeyCode::Char('/') => {
                self.record_finding = true;
                self.input.set(&self.record_find.clone());
            }

            // Open a value that holds other values, and close it again.
            KeyCode::Right | KeyCode::Char('l') => self.record_open(),
            KeyCode::Left | KeyCode::Char('h') => self.record_close(),
            KeyCode::Char(' ') => {
                if let Some(l) = self.record_line()
                    && l.kind.opens()
                {
                    self.record_tree.toggle(&l.path);
                }
            }
            KeyCode::Enter => {
                // A value that holds other values opens. A value that holds
                // one value goes to the cell inspector, which shows it in
                // full: the record view cuts a long value at the right edge.
                match self.record_line() {
                    Some(l) if l.kind.opens() => self.record_open(),
                    Some(l) => self.inspect_record_value(&l),
                    None => {}
                }
            }
            KeyCode::Char('a') => {
                self.record_tree.expand_all();
                self.ok("each level is open");
            }
            KeyCode::Char('c') => {
                self.record_tree.collapse_all();
                self.record_sel = 0;
                self.ok("each level is closed");
            }
            // Show the fields that hold no value, and hide them again.
            KeyCode::Char('z') => {
                self.record_tree.hide_empty = !self.record_tree.hide_empty;
                self.record_sel = 0;
                if self.record_tree.hide_empty {
                    self.ok("the fields with no value are hidden");
                } else {
                    self.ok("each field is shown");
                }
            }

            KeyCode::Char('y') => {
                match self
                    .record_line()
                    .and_then(|l| self.record_tree.value_at(&l.path))
                {
                    Some(v) => self.copy(&v, "value"),
                    None => self.error("no value here yet"),
                }
            }
            KeyCode::Char('Y') => match self.record_tree.as_json() {
                Some(j) => self.copy(&j, "record as JSON"),
                None => self.error("no row here yet"),
            },
            // The path is what a user needs to write a statement about a
            // value that is three levels down.
            KeyCode::Char('P') => match self.record_line() {
                Some(l) => {
                    let p = peruse_core::query::quote_path(&l.path);
                    self.copy(&p, "path")
                }
                None => self.error("no field here"),
            },

            KeyCode::Char('=') => self.filter_record_value(true),
            KeyCode::Char('!') => self.filter_record_value(false),
            _ => {}
        }
    }

    /// Opens the selected line of the record view.
    fn record_open(&mut self) {
        if let Some(l) = self.record_line()
            && l.kind.opens()
        {
            self.record_tree.open_path(&l.path);
        }
    }

    /// Closes the selected line, or moves to the line that holds it.
    ///
    /// A line that is closed already has nothing to close. The user then
    /// wants to leave that level, so the selection moves to the line above it
    /// at the smaller depth.
    fn record_close(&mut self) {
        let Some(l) = self.record_line() else { return };
        if l.kind.opens() && l.open {
            self.record_tree.close_path(&l.path);
            return;
        }
        if l.depth == 0 {
            return;
        }
        let lines = self.record_lines();
        if let Some(parent) = (0..self.record_sel)
            .rev()
            .find(|i| lines[*i].depth < l.depth)
        {
            self.record_sel = parent;
        }
    }

    /// Opens the cell inspector on a value of the record view.
    fn inspect_record_value(&mut self, line: &Line) {
        let Some(v) = self.record_tree.value_at(&line.path) else {
            return;
        };
        // A value that is inside a structure has no column of its own, so the
        // engine cannot read it again. The tree holds the complete value
        // already, and the inspector takes it as it is.
        self.cell_value = Some(v);
        self.cell_scroll = 0;
        self.cell_from_record = true;
        self.cell_title = Some(line.path_text());
        self.mode = Mode::Cell;
    }

    /// Adds a condition on the selected value of the record view.
    fn filter_record_value(&mut self, keep: bool) {
        let Some(line) = self.record_line() else {
            return;
        };
        // A column of the row is a column of the view. The builder can hold
        // it as a condition that the user can edit later.
        if line.path.len() == 1
            && let Some(c) = self.record_column()
        {
            self.cursor_col = c;
            self.close_record();
            self.filter_this_value(keep);
            return;
        }
        // A value inside a structure has no column. DuckDB reads it by its
        // path, so the condition holds that path as an expression.
        if line.kind.opens() {
            self.error("cannot filter on a value that holds other values");
            return;
        }
        let path = peruse_core::query::quote_path(&line.path);
        let sql = match (line.family, keep) {
            (Family::Null, true) => format!("{path} IS NULL"),
            (Family::Null, false) => format!("{path} IS NOT NULL"),
            (Family::Number | Family::Bool, true) => format!("{path} = {}", line.value),
            (Family::Number | Family::Bool, false) => format!("{path} <> {}", line.value),
            (_, true) => format!("{path} = {}", quote_str(&self.record_text(&line))),
            (_, false) => format!("{path} <> {}", quote_str(&self.record_text(&line))),
        };
        if let Err(e) = sql_guard::ensure_safe_predicate(&sql) {
            self.error(format!("cannot filter on this value: {e}"));
            return;
        }
        self.close_record();
        self.fset.push(Term::Raw(sql));
        self.apply_fset();
        self.ok(format!("filter: {}", line.path_text()));
    }

    /// Gives the true text of a value, and not the short form that the record
    /// view draws. The short form `(empty)` is not the value.
    fn record_text(&self, line: &Line) -> String {
        self.record_tree
            .value_at(&line.path)
            .unwrap_or_else(|| line.value.clone())
    }

    /// Moves the record view to the next row or to the previous row.
    ///
    /// The lines that are open stay open. The user moves through the rows to
    /// compare one field, and a tree that closes at each row would make that
    /// impossible.
    fn step_record(&mut self, delta: i64) {
        let before = self.cursor_row;
        self.move_rows(delta);
        if self.cursor_row == before {
            self.info(if delta > 0 { "last row" } else { "first row" });
            return;
        }
        self.request_record();
    }

    // ---------------------------------------------------- the settings page

    /// Gives the value of one setting, as the page shows it.
    ///
    /// An empty text means that the setting has no value, and that Peruse
    /// uses the value that it builds in.
    pub fn setting_value(&self, s: Setting) -> String {
        match s {
            Setting::Theme => self.config.theme.clone().unwrap_or_default(),
            Setting::Threads => self.config.threads.map(|v| v.to_string()).unwrap_or_default(),
            Setting::MemoryLimit => self
                .config
                .memory_limit_gb
                .map(|v| format!("{v} GB"))
                .unwrap_or_default(),
            Setting::SampleSize => self
                .config
                .sample_size
                .map(|v| v.to_string())
                .unwrap_or_default(),
            Setting::NoIndex => match self.config.no_index {
                Some(true) => "no".into(),
                Some(false) => "yes".into(),
                None => String::new(),
            },
            Setting::Panels => self.config.panels.clone().unwrap_or_default(),
            Setting::Band => self.config.band.clone().unwrap_or_default(),
            Setting::Step => self.config.step.map(|v| v.to_string()).unwrap_or_default(),
        }
    }

    /// Gives the value that Peruse uses when the setting has none.
    pub fn setting_default(&self, s: Setting) -> String {
        match s {
            Setting::Theme => "peruse-dark".into(),
            Setting::Threads => format!("{} (one for each core)", self.resources.cores),
            Setting::MemoryLimit => self
                .resources
                .default_memory_gb()
                .map(|gb| format!("{gb} GB (half of this machine)"))
                .unwrap_or_else(|| "the rule of DuckDB".into()),
            Setting::SampleSize => "20,480 rows".into(),
            Setting::NoIndex => "yes, below 256 MB".into(),
            Setting::Panels => "none".into(),
            Setting::Band => "off".into(),
            Setting::Step => format!("{DEFAULT_STEP} rows or columns"),
        }
    }

    /// Keeps the theme in the settings file.
    ///
    /// A theme is a choice that a user makes one time, so the key `t` and the
    /// picker write it at once. The user does not have to open the settings
    /// page to keep a choice that they made there.
    ///
    /// The function writes the theme by itself. It reads the file again and
    /// changes one line of it, so a setting that the user is testing in this
    /// session does not go into the file without a request.
    fn remember_theme(&mut self) {
        let name = self.theme.name.clone();
        self.config.theme = Some(name.clone());
        let Some(path) = self.config_path.clone() else {
            self.ok(format!("theme: {name}"));
            return;
        };
        // A file that Peruse cannot read gives the built-in settings and the
        // reason. A write of those settings would replace the file, and each
        // setting that the user wrote in it, plus every note, would be gone. The
        // file therefore stays as it is, and the message says why.
        if let Some(why) = write_blocked(&path) {
            self.ok(format!("theme: {name} ({why})"));
            return;
        }
        let (mut on_disk, _) = Config::load_from(&path);
        if on_disk.theme.as_deref() == Some(name.as_str()) {
            self.ok(format!("theme: {name}"));
            return;
        }
        on_disk.theme = Some(name.clone());
        match on_disk.save_to(&path) {
            Ok(_) => self.ok(format!("theme: {name} (kept)")),
            // A theme that Peruse cannot keep still works for this session.
            Err(e) => self.ok(format!("theme: {name} (not kept: {e})")),
        }
    }

    /// Keeps the mode of the detail band in the settings file.
    ///
    /// The key `d` writes the mode at once, as the key `t` writes the theme. A
    /// user who turns the band on wants it on at the next start as well, and a
    /// second key to keep it is a key that the user forgets to press.
    ///
    /// The function reads the file again and changes one line of it, so a setting
    /// that the user is testing in this session does not go into the file without
    /// a request. It gives the text for the status line: nothing when the mode is
    /// in the file, and the reason when it is not.
    fn remember_band(&mut self) -> String {
        let name = self.band().name();
        let Some(path) = self.config_path.clone() else {
            return String::new();
        };
        // A file that Peruse cannot read gives the built-in settings and the
        // reason. A write of those settings would replace the file, and each
        // setting that the user wrote in it, plus every note, would be gone. The
        // key `d` is a key that a user presses often, so this is the more likely
        // way to lose a file. The file therefore stays as it is.
        if let Some(why) = write_blocked(&path) {
            return format!(" ({why})");
        }
        let (mut on_disk, _) = Config::load_from(&path);
        if on_disk.band.as_deref() == Some(name) {
            return String::new();
        }
        on_disk.band = Some(name.to_string());
        match on_disk.save_to(&path) {
            Ok(_) => String::new(),
            // A mode that Peruse cannot keep still works for this session.
            Err(e) => format!(" (not kept: {e})"),
        }
    }

    /// Writes the settings to the file.
    fn save_config(&self) -> Result<(), String> {
        let Some(p) = &self.config_path else {
            return Err("this system gives no directory for settings".into());
        };
        if let Some(why) = write_blocked(p) {
            return Err(why);
        }
        self.config.save_to(p).map(|_| ())
    }

    /// Opens the settings page.
    fn open_settings(&mut self) {
        self.settings_sel = 0;
        self.settings_editing = false;
        self.mode = Mode::Settings;
        // Ask DuckDB what it uses now. The page then shows the truth, and
        // not the wish of the user.
        self.worker.send(Request::Configure {
            epoch: self.epoch,
            threads: None,
            memory_limit: None,
        });
    }

    /// Takes the text that the user typed as the value of a setting.
    fn commit_setting(&mut self, s: Setting, text: &str) {
        let t = text.trim();
        let empty = t.is_empty();
        match s {
            Setting::Theme => {
                if empty {
                    self.config.theme = None;
                } else {
                    // Apply the theme at once, so the user sees the result.
                    match peruse_core::theme::resolve(t) {
                        Ok(theme) => {
                            self.theme_idx = self
                                .themes
                                .iter()
                                .position(|x| x.name == theme.name)
                                .unwrap_or(self.theme_idx);
                            self.theme = theme;
                            self.config.theme = Some(t.to_string());
                        }
                        Err(e) => {
                            self.error(format!("theme: {e}"));
                            return;
                        }
                    }
                }
            }
            Setting::Threads => {
                if empty {
                    self.config.threads = None;
                } else {
                    match t.parse::<usize>() {
                        Ok(n) if n >= 1 => self.config.threads = Some(n),
                        _ => {
                            self.error(format!("threads: {t:?} is not a number of 1 or more"));
                            return;
                        }
                    }
                }
            }
            Setting::MemoryLimit => {
                if empty {
                    self.config.memory_limit_gb = None;
                } else {
                    // The unit is always the gigabyte. A user who writes
                    // "8GB" means the same thing, so take that form too.
                    //
                    // Each other unit is refused, and not read as gigabytes.
                    // A user who writes "512MB" wants half a gigabyte, and
                    // 512 gigabytes would be a bad surprise.
                    let upper = t.to_ascii_uppercase();
                    let digits = upper
                        .strip_suffix("GIB")
                        .or_else(|| upper.strip_suffix("GB"))
                        .or_else(|| upper.strip_suffix("G"))
                        .unwrap_or(&upper)
                        .trim();
                    match digits.parse::<u32>() {
                        Ok(n) if n >= 1 => self.config.memory_limit_gb = Some(n),
                        _ => {
                            self.error(format!(
                                "memory limit: write a whole number of gigabytes, such as 8 (got {t:?})"
                            ));
                            return;
                        }
                    }
                }
            }
            Setting::SampleSize => {
                if empty {
                    self.config.sample_size = None;
                } else {
                    match t.replace([',', '_'], "").parse::<i64>() {
                        Ok(n) if n == -1 || n > 0 => self.config.sample_size = Some(n),
                        _ => {
                            self.error(format!("sample size: {t:?} is not a number, and not -1"));
                            return;
                        }
                    }
                }
            }
            Setting::Panels => {
                if empty {
                    self.config.panels = None;
                    self.panel = Panel::None;
                } else {
                    match Panel::parse(t) {
                        Some(pan) => {
                            self.panel = pan;
                            self.config.panels = Some(pan.name().to_string());
                            self.after_panel_change();
                        }
                        None => {
                            self.error(format!(
                                "panels: write none, meta, stats or both (got {t:?})"
                            ));
                            return;
                        }
                    }
                }
            }
            Setting::Band => {
                if empty {
                    self.config.band = None;
                } else {
                    match Band::parse(t) {
                        Some(b) => self.config.band = Some(b.name().to_string()),
                        None => {
                            self.error(format!(
                                "column details: write off, compact or detailed (got {t:?})"
                            ));
                            return;
                        }
                    }
                }
            }
            Setting::Step => {
                if empty {
                    self.config.step = None;
                } else {
                    match t.replace([',', '_'], "").parse::<usize>() {
                        Ok(n) if (1..=MAX_STEP).contains(&n) => self.config.step = Some(n),
                        _ => {
                            self.error(format!(
                                "step: write a number from 1 to {MAX_STEP} (got {t:?})"
                            ));
                            return;
                        }
                    }
                }
            }
            Setting::NoIndex => {
                self.config.no_index = match t.to_ascii_lowercase().as_str() {
                    "" => None,
                    "yes" | "true" | "y" | "on" => Some(false),
                    "no" | "false" | "n" | "off" => Some(true),
                    _ => {
                        self.error("index at open: write yes or no");
                        return;
                    }
                };
            }
        }
        self.settings_editing = false;
        self.apply_engine_settings();
        // Keep the change at once. A user who changes a setting means it, and
        // a second key to keep it is one that the user forgets to press.
        let kept = match self.save_config() {
            Ok(()) => String::new(),
            Err(e) => format!(" (not kept: {e})"),
        };
        if s.at_next_file() {
            self.info(format!(
                "{}: takes effect at the next file{kept}",
                s.label()
            ));
        } else if !kept.is_empty() {
            self.error(format!("{}{kept}", s.label()));
        }
    }

    /// Sends the settings that DuckDB can change while it runs.
    fn apply_engine_settings(&mut self) {
        self.worker.send(Request::Configure {
            epoch: self.epoch,
            threads: self.config.threads,
            memory_limit: self.config.memory_limit_text(),
        });
    }

    /// Handles a key in the settings page.
    fn settings_key(&mut self, key: &KeyEvent) {
        if self.settings_editing {
            if self.take_ghost(key).is_some() {
                return;
            }
            match self.input.handle(key) {
                Action::Cancel => self.settings_editing = false,
                Action::Submit => {
                    let text = self.input.text();
                    let s = Setting::ALL[self.settings_sel.min(Setting::ALL.len() - 1)];
                    self.commit_setting(s, &text);
                }
                _ => {}
            }
            return;
        }
        let n = Setting::ALL.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => {
                self.settings_sel = (self.settings_sel + 1).min(n - 1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.settings_sel = self.settings_sel.saturating_sub(1);
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                let s = Setting::ALL[self.settings_sel.min(n - 1)];
                self.input.set(&self.setting_value(s));
                self.settings_editing = true;
            }
            // Remove the value, so Peruse uses the one that it builds in.
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => {
                let s = Setting::ALL[self.settings_sel.min(n - 1)];
                self.commit_setting(s, "");
                self.ok(format!("{}: back to the built-in value", s.label()));
            }
            // Take the value that fits this machine.
            KeyCode::Char('m') => {
                let s = Setting::ALL[self.settings_sel.min(n - 1)];
                match s {
                    Setting::Threads => {
                        let v = self.resources.cores.to_string();
                        self.commit_setting(s, &v);
                        self.ok(format!("threads: {v}, one for each core"));
                    }
                    Setting::MemoryLimit => match self.resources.default_memory_gb() {
                        Some(gb) => {
                            self.commit_setting(s, &gb.to_string());
                            self.ok(format!("memory limit: {gb} GB, half of this machine"));
                        }
                        None => self.error("this system does not report its memory"),
                    },
                    _ => self.info("this setting has no value from the machine"),
                }
            }
            KeyCode::Char('T') => {
                self.theme_sel = self.theme_idx;
                self.mode = Mode::ThemePicker;
            }
            _ => {}
        }
    }

    // ------------------------------------------------- the filter builder

    /// Puts the compiled filter in the view and asks for the new rows.
    ///
    /// The builder writes the expression itself, so the expression is safe by
    /// the way that it is built. The guard runs on it in any case. It costs
    /// almost no time, and it keeps the promise of the program true if a
    /// later change to the compiler makes a mistake.
    fn apply_fset(&mut self) {
        let sql = self.fset.to_sql();
        if let Some(s) = &sql
            && let Err(e) = sql_guard::ensure_safe_predicate(s)
        {
            self.error(format!("the filter is not safe to run: {e}"));
            return;
        }
        if sql == self.view.filter {
            return;
        }
        self.view.filter = sql;
        self.reload(true);
    }

    /// Writes what the view shows now, in a few words.
    ///
    /// A user who goes back one step must see where they arrived. The name of
    /// the key alone does not say that.
    fn view_summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Base::Sql(_) = &self.view.base {
            parts.push("a statement".into());
        }
        if let Some(f) = &self.view.filter {
            parts.push(format!("filter {}", crate::text::truncate(f, 40)));
        }
        if let Some(k) = self.view.sort.first() {
            parts.push(format!("sort {}{}", k.dir.arrow(), k.column));
        }
        if parts.is_empty() {
            "the whole file".into()
        } else {
            parts.join(", ")
        }
    }

    /// Goes back to the view that the user had before this one, or forward to
    /// a view that the key `u` removed.
    fn step_history(&mut self, back: bool) {
        let taken = if back {
            self.history.pop()
        } else {
            self.undone.pop()
        };
        let Some(step) = taken else {
            self.info(if back {
                "nothing to go back to"
            } else {
                "nothing to go forward to"
            });
            return;
        };
        let current = ViewStep {
            view: self.view.clone(),
            fset: self.fset.clone(),
        };
        self.view = step.view;
        self.fset = step.fset;

        // `reload` writes the history itself. Tell it that this change comes
        // from the history, so it keeps the way forward.
        self.restoring = true;
        self.reload(true);
        self.restoring = false;

        if back {
            // `reload` put the view that the user left into the history.
            // Move it to the list that the key `U` reads instead.
            self.history.pop();
            self.undone.push(current);
        } else {
            self.history.push(current);
        }
        let where_now = self.view_summary();
        self.ok(format!(
            "{} {where_now}",
            if back { "back to" } else { "forward to" }
        ));
    }

    /// Takes a `WHERE` expression as the complete filter.
    ///
    /// The prompt `E` and the option `--filter` both use this function. The
    /// expression becomes one condition of the list, so the builder can show
    /// it and a quick filter can add a condition beside it.
    pub fn set_raw_filter(&mut self, sql: &str) {
        self.fset = if sql.trim().is_empty() {
            FilterSet::default()
        } else {
            FilterSet::from_raw(sql)
        };
        self.view.filter = self.fset.to_sql();
    }

    /// Opens the filter builder.
    fn open_builder(&mut self) {
        if self.schema.is_empty() {
            self.error("no columns to filter on");
            return;
        }
        self.fset_saved = self.fset.clone();
        self.build_sel = 0;
        self.prompt_error = None;
        self.mode = Mode::FilterBuild;
        if self.fset.conditions.is_empty() {
            // With no condition, the list would be an empty box. Go straight
            // to the list of columns, which is the first step in each case.
            self.begin_condition(None);
        } else {
            self.build = Build::List;
        }
    }

    /// Leaves the builder and puts back the filter from the time before the
    /// user opened it.
    fn cancel_builder(&mut self) {
        self.fset = self.fset_saved.clone();
        self.prompt_error = None;
        self.mode = Mode::Normal;
    }

    /// Starts a new condition, or starts to edit one.
    fn begin_condition(&mut self, edit: Option<usize>) {
        // A condition that the user typed has no column and no operator. The
        // text step is the one editor that fits it.
        if let Some(i) = edit
            && let Some(Term::Raw(s)) = self.fset.conditions.get(i).map(|c| &c.term)
        {
            let text = s.clone();
            self.draft = Draft { edit, ..Draft::default() };
            self.input.set(&text);
            self.prompt_error = None;
            self.build = Build::Raw;
            return;
        }

        self.draft = Draft { edit, ..Draft::default() };
        match edit.and_then(|i| self.fset.conditions.get(i)).map(|c| &c.term) {
            Some(Term::Cmp { column, op, value, value2, .. }) => {
                self.draft.col = self.schema.index_of(column).unwrap_or(0);
                self.draft.op = *op;
                self.draft.value = value.clone();
                self.draft.value2 = value2.clone();
            }
            // A new condition starts on the column under the cursor. That is
            // the column that the user looks at.
            _ => self.draft.col = self.cursor_col.min(self.schema.len().saturating_sub(1)),
        }
        self.input.clear();
        self.build = Build::Column;
        let cols = self.build_columns();
        self.pick_sel = cols.iter().position(|c| *c == self.draft.col).unwrap_or(0);
    }

    /// Gives the position of each column that the list of the builder shows.
    pub fn build_columns(&self) -> Vec<usize> {
        let q = self.input.text();
        (0..self.schema.len())
            .filter(|i| commands::fuzzy_match(&q, &self.schema.columns[*i].name))
            .collect()
    }

    /// Gives the operators that the builder offers for the draft column.
    pub fn build_ops(&self) -> &'static [Op] {
        let kind = self
            .schema
            .columns
            .get(self.draft.col)
            .map(|c| c.kind)
            .unwrap_or(peruse_core::CellKind::Text);
        Op::for_kind(kind)
    }

    /// Puts the draft condition in the list and goes back to the list.
    fn commit_condition(&mut self) {
        let Some(col) = self.schema.columns.get(self.draft.col) else {
            self.build = Build::List;
            return;
        };
        let term = Term::Cmp {
            column: col.name.clone(),
            kind: col.kind,
            op: self.draft.op,
            value: self.draft.value.clone(),
            value2: self.draft.value2.clone(),
        };
        match self.draft.edit {
            Some(i) if i < self.fset.conditions.len() => {
                self.fset.conditions[i].term = term;
                self.build_sel = i;
            }
            _ => {
                self.fset.push(term);
                self.build_sel = self.fset.conditions.len() - 1;
            }
        }
        self.input.clear();
        self.build = Build::List;
    }

    /// Gives one key to the step that the builder is at.
    fn build_key(&mut self, key: &KeyEvent) {
        match self.build {
            Build::List => self.build_list_key(key),
            Build::Column => self.build_column_key(key),
            Build::Op => self.build_op_key(key),
            Build::Value | Build::Value2 => self.build_value_key(key),
            Build::Raw => self.build_raw_key(key),
        }
    }

    /// Handles a key in the list of conditions.
    fn build_list_key(&mut self, key: &KeyEvent) {
        let n = self.fset.conditions.len();
        match key.code {
            KeyCode::Esc => self.cancel_builder(),
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                self.apply_fset();
                match self.fset.to_sql() {
                    Some(_) => {
                        let n = self.fset.conditions.len();
                        self.ok(format!("filter applied ({n} condition(s))"));
                    }
                    None => self.info("no filter"),
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.build_sel = (self.build_sel + 1).min(n.saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => self.build_sel = self.build_sel.saturating_sub(1),
            KeyCode::Char('a') | KeyCode::Char('+') => self.begin_condition(None),
            KeyCode::Char('e') => {
                if self.build_sel < n {
                    self.begin_condition(Some(self.build_sel));
                }
            }
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => {
                if self.build_sel < n {
                    self.fset.conditions.remove(self.build_sel);
                    self.build_sel = self
                        .build_sel
                        .min(self.fset.conditions.len().saturating_sub(1));
                }
            }
            // The first condition has no word in front of it, so it has no
            // word to change.
            KeyCode::Char('o') => {
                if self.build_sel > 0
                    && let Some(c) = self.fset.conditions.get_mut(self.build_sel)
                {
                    c.join = c.join.other();
                }
            }
            KeyCode::Char('c') => {
                self.fset.clear();
                self.build_sel = 0;
            }
            KeyCode::Char('r') => {
                self.draft = Draft::default();
                self.input.clear();
                self.prompt_error = None;
                self.build = Build::Raw;
            }
            _ => {}
        }
    }

    /// Handles a key in the list of columns.
    fn build_column_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // Go back to the list. With no condition in the list, there
                // is nothing to go back to, so leave the builder.
                if self.fset.conditions.is_empty() {
                    self.cancel_builder();
                } else {
                    self.input.clear();
                    self.build = Build::List;
                }
            }
            KeyCode::Up => self.pick_sel = self.pick_sel.saturating_sub(1),
            KeyCode::Down => {
                let n = self.build_columns().len();
                self.pick_sel = (self.pick_sel + 1).min(n.saturating_sub(1));
            }
            KeyCode::Enter => {
                let cols = self.build_columns();
                let Some(&c) = cols.get(self.pick_sel) else {
                    return;
                };
                self.draft.col = c;
                // Keep the operator when it still has a meaning for the new
                // column. A BLOB column, for example, has no `>` operator.
                let ops = self.build_ops();
                self.pick_sel = ops.iter().position(|o| *o == self.draft.op).unwrap_or(0);
                self.draft.op = ops[self.pick_sel];
                self.build = Build::Op;
            }
            _ => {
                if self.input.handle(key) == Action::Changed {
                    self.pick_sel = 0;
                }
            }
        }
    }

    /// Handles a key in the list of operators.
    fn build_op_key(&mut self, key: &KeyEvent) {
        let ops = self.build_ops();
        match key.code {
            KeyCode::Esc => self.build = Build::Column,
            KeyCode::Up | KeyCode::Char('k') => self.pick_sel = self.pick_sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.pick_sel = (self.pick_sel + 1).min(ops.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                self.draft.op = ops[self.pick_sel.min(ops.len().saturating_sub(1))];
                if self.draft.op.needs_value() {
                    self.input.set(&self.draft.value.clone());
                    self.build = Build::Value;
                } else {
                    self.commit_condition();
                }
            }
            _ => {}
        }
    }

    /// Handles a key in the value step.
    fn build_value_key(&mut self, key: &KeyEvent) {
        match self.input.handle(key) {
            Action::Cancel => {
                if self.build == Build::Value2 {
                    self.input.set(&self.draft.value.clone());
                    self.build = Build::Value;
                } else {
                    self.build = Build::Op;
                }
            }
            Action::Submit => {
                if self.build == Build::Value {
                    self.draft.value = self.input.text();
                    if self.draft.op.needs_second_value() {
                        self.input.set(&self.draft.value2.clone());
                        self.build = Build::Value2;
                        return;
                    }
                } else {
                    self.draft.value2 = self.input.text();
                }
                self.commit_condition();
            }
            _ => {}
        }
    }

    /// Handles a key in the step that takes a `WHERE` expression.
    fn build_raw_key(&mut self, key: &KeyEvent) {
        if key.code == KeyCode::Tab
            || (key.code == KeyCode::Right && plain(key) && self.ghost().is_some())
        {
            self.complete_column();
            return;
        }
        match self.input.handle(key) {
            Action::Cancel => {
                self.prompt_error = None;
                self.input.clear();
                self.build = Build::List;
                if self.fset.conditions.is_empty() {
                    self.cancel_builder();
                }
            }
            Action::Submit => {
                let text = self.input.text().trim().to_string();
                if text.is_empty() {
                    // An empty expression removes the condition.
                    if let Some(i) = self.draft.edit
                        && i < self.fset.conditions.len()
                    {
                        self.fset.conditions.remove(i);
                    }
                } else {
                    if let Err(e) = sql_guard::ensure_safe_predicate(&text) {
                        self.prompt_error = Some(e.to_string());
                        return;
                    }
                    self.input.remember();
                    self.filter_input = self.input.clone();
                    match self.draft.edit {
                        Some(i) if i < self.fset.conditions.len() => {
                            self.fset.conditions[i].term = Term::Raw(text);
                        }
                        _ => self.fset.push(Term::Raw(text)),
                    }
                }
                self.build_sel = self
                    .build_sel
                    .min(self.fset.conditions.len().saturating_sub(1));
                self.prompt_error = None;
                self.input.clear();
                self.build = Build::List;
            }
            Action::Changed => {
                self.prompt_error = (!self.input.is_empty())
                    .then(|| sql_guard::ensure_safe_predicate(&self.input.text()).err())
                    .flatten()
                    .map(|e| e.to_string());
            }
            Action::Ignored => {}
        }
    }

    /// Adds a condition on the cell under the cursor, and applies the filter.
    ///
    /// With `keep`, the filter holds the rows that have this value. Without
    /// it, the filter removes them.
    fn filter_this_value(&mut self, keep: bool) {
        let Some(col) = self.schema.columns.get(self.cursor_col).cloned() else {
            self.error("no column here");
            return;
        };
        let Some(r) = self.page_row() else {
            self.error("no row here yet");
            return;
        };
        // The grid shows the size of a BLOB value, and not its bytes. A
        // filter on that text would find no row.
        if col.kind == peruse_core::CellKind::Binary {
            self.error("a BLOB column has no value to filter on");
            return;
        }
        // A page cuts a long value. A test for equality against the cut text
        // would find no row, so refuse instead of giving a wrong answer.
        if self.page.is_truncated(r, self.cursor_col) {
            self.error("value is cut short here — use f to filter on part of it");
            return;
        }
        let value = self.page.cell(r, self.cursor_col);
        let op = match (value.is_some(), keep) {
            (false, true) => Op::IsNull,
            (false, false) => Op::IsNotNull,
            (true, true) => Op::Eq,
            (true, false) => Op::Ne,
        };
        self.fset.push(Term::Cmp {
            column: col.name.clone(),
            kind: col.kind,
            op,
            value: value.unwrap_or("").to_string(),
            value2: String::new(),
        });
        let shown = crate::text::truncate(value.unwrap_or(""), 30);
        self.apply_fset();
        self.ok(format!("filter: {} {} {shown}", col.name, op.label()));
    }

    // ----------------------------------------------------------- the commands

    /// Runs one command.
    pub fn run(&mut self, cmd: Cmd) {
        let vis_rows = self.viewport_rows.max(1) as i64;
        match cmd {
            Cmd::Quit => self.quit = true,
            Cmd::Help => {
                self.help_scroll = 0;
                self.mode = Mode::Help;
            }
            Cmd::Palette => {
                self.input = LineInput::default();
                self.palette_sel = 0;
                self.mode = Mode::Palette;
            }
            Cmd::Cancel => {
                if self.busy {
                    self.worker.cancel();
                    self.info("cancelled");
                } else if self.panel != Panel::None {
                    self.panel = Panel::None;
                }
            }

            Cmd::RowDown => self.move_rows(1),
            Cmd::RowUp => self.move_rows(-1),
            Cmd::PageDown => self.move_rows(vis_rows),
            Cmd::PageUp => self.move_rows(-vis_rows),
            Cmd::HalfPageDown => self.move_rows((vis_rows / 2).max(1)),
            Cmd::HalfPageUp => self.move_rows(-(vis_rows / 2).max(1)),
            Cmd::StepDown => self.move_rows(self.step()),
            Cmd::StepUp => self.move_rows(-self.step()),
            Cmd::StepRight => self.move_cols(self.step()),
            Cmd::StepLeft => self.move_cols(-self.step()),
            Cmd::Top => {
                self.cursor_row = 0;
                self.follow_cursor();
            }
            Cmd::Bottom => {
                let max = self.max_row();
                if max == u64::MAX {
                    self.info("still counting — try again in a moment");
                } else {
                    self.cursor_row = max;
                    self.follow_cursor();
                }
            }
            Cmd::ColRight => self.move_cols(1),
            Cmd::ColLeft => self.move_cols(-1),
            Cmd::ColFirst => self.move_cols(-(self.schema.len() as i64)),
            Cmd::ColLast => self.move_cols(self.schema.len() as i64),
            Cmd::Origin => {
                self.cursor_row = 0;
                self.top_row = 0;
                self.left_col = 0;
                self.move_cols(-(self.schema.len() as i64));
            }
            Cmd::LastCell => {
                // Test the count of rows before anything moves. A cursor that
                // moved to the last column, under a message that says that
                // nothing happened, reads as a fault in the program.
                let max = self.max_row();
                if max == u64::MAX {
                    self.info("still counting — try again in a moment");
                    return;
                }
                self.move_cols(self.schema.len() as i64);
                self.cursor_row = max;
                self.follow_cursor();
            }
            Cmd::GotoRow => self.open_prompt(PromptKind::Goto),

            Cmd::SortCycle => {
                let Some(col) = self.schema.columns.get(self.cursor_col) else {
                    return;
                };
                let name = col.name.clone();
                // Peruse sorts on one column at a time. The result is easy
                // to predict, and three presses of the key give each of the
                // three states.
                let next = match self.view.sort.first() {
                    Some(k) if k.column == name && k.dir == SortDir::Asc => {
                        Some(SortKey { column: name, dir: SortDir::Desc })
                    }
                    Some(k) if k.column == name => None,
                    _ => Some(SortKey { column: name, dir: SortDir::Asc }),
                };
                self.view.sort = next.into_iter().collect();
                self.reload(true);
            }
            Cmd::SortClear => {
                if self.view.sort.is_empty() {
                    self.info("not sorted");
                } else {
                    self.view.sort.clear();
                    self.reload(true);
                }
            }
            Cmd::FilterBuild => self.open_builder(),
            Cmd::Filter => self.open_prompt(PromptKind::Filter),
            Cmd::FilterThisValue => self.filter_this_value(true),
            Cmd::FilterExcludeValue => self.filter_this_value(false),
            Cmd::FilterClear => {
                if self.view.filter.is_none() {
                    self.info("no filter set");
                } else {
                    self.fset.clear();
                    self.view.filter = None;
                    self.reload(true);
                }
            }
            Cmd::Sql => self.open_prompt(PromptKind::Sql),
            Cmd::Undo => self.step_history(true),
            Cmd::Redo => self.step_history(false),
            Cmd::ResetView => {
                self.view = View::default();
                self.fset.clear();
                self.needle.clear();
                self.hits.clear();
                self.hidden = vec![false; self.schema.len()];
                self.reload(true);
                self.ok("view reset");
            }
            Cmd::Search => self.open_prompt(PromptKind::Search),
            Cmd::SearchNext => self.search(true),
            Cmd::SearchPrev => self.search(false),

            // The key adds the panel to the screen, or removes it. It keeps
            // the other panel, so the two keys together give the view with
            // both of them.
            Cmd::ToggleMeta => {
                self.panel = match self.panel {
                    Panel::Meta => Panel::None,
                    Panel::Both => Panel::Stats,
                    Panel::Stats => Panel::Both,
                    Panel::None => Panel::Meta,
                };
                self.after_panel_change();
            }
            Cmd::ToggleStats => {
                self.panel = match self.panel {
                    Panel::Stats => Panel::None,
                    Panel::Both => Panel::Meta,
                    Panel::Meta => Panel::Both,
                    Panel::None => Panel::Stats,
                };
                self.after_panel_change();
            }
            // One key moves through the four states, for a user who does not
            // want to remember which key adds which panel.
            Cmd::CyclePanels => {
                self.panel = self.panel.next();
                self.after_panel_change();
                let name = match self.panel {
                    Panel::None => "no panel",
                    Panel::Meta => "metadata",
                    Panel::Stats => "column statistics",
                    Panel::Both => "metadata and statistics",
                };
                self.ok(name);
            }
            // One key moves through the three modes of the band. The band is a
            // second view of the facts that the statistics panel gives, and it
            // covers every column on the screen at the same time.
            Cmd::CycleBand => {
                let next = self.band().next();
                self.config.band = Some(next.name().to_string());
                // The key is the way back after a request that failed. The band
                // asks again for the columns on the screen.
                self.band_error = false;
                self.band_asked.clear();
                let note = self.remember_band();
                self.ok(format!("{}{note}", next.label()));
            }
            Cmd::Record => self.open_record(),
            Cmd::InspectCell => {
                let Some(col) = self.schema.columns.get(self.cursor_col) else {
                    return;
                };
                // A value that holds other values has no useful form as one
                // piece of text. The text that DuckDB writes for a structure,
                //
                //   {'id': 665991, 'login': petroav, 'gravatar_id': '', ...}
                //
                // says what the value holds and nothing more: the user cannot
                // read one field of it, cannot copy one field, and cannot
                // filter on one field. The record view opens such a value
                // instead, with the column already open.
                if col.kind == peruse_core::CellKind::Nested {
                    self.open_record_at_column();
                    return;
                }
                self.cell_value = self.current_cell_text();
                self.cell_scroll = 0;
                self.cell_title = None;
                self.mode = Mode::Cell;
                // Ask the engine for the complete value. The copy in the
                // grid stops at the limit, in the middle of the value.
                self.worker.send(Request::Cell {
                    epoch: self.epoch,
                    view: self.view.clone(),
                    column: col.name.clone(),
                    row: self.cursor_row,
                });
            }

            Cmd::Widen => {
                if let Some(w) = self.widths.get_mut(self.cursor_col) {
                    *w = (*w + 4).min(MAX_COL_WIDTH * 4);
                }
            }
            Cmd::Narrow => {
                if let Some(w) = self.widths.get_mut(self.cursor_col) {
                    *w = w.saturating_sub(4).max(MIN_COL_WIDTH);
                }
            }
            Cmd::FitWidths => {
                self.fit_widths();
                self.ok("column widths re-fitted");
            }
            Cmd::HideColumn => {
                if self.visible_columns().len() <= 1 {
                    self.error("cannot hide the last visible column");
                    return;
                }
                self.hidden[self.cursor_col] = true;
                let vis = self.visible_columns();
                if !vis.contains(&self.cursor_col) {
                    self.cursor_col = *vis.last().unwrap();
                }
                let name = self.schema.columns[self.cursor_col].name.clone();
                self.ok(format!("hidden — X shows all again ({name} selected)"));
            }
            Cmd::ShowAllColumns => {
                let n = self.hidden.iter().filter(|h| **h).count();
                self.hidden = vec![false; self.schema.len()];
                self.ok(format!("{n} column(s) shown"));
            }

            Cmd::CopyCell => {
                let Some(v) = self.current_cell_text() else {
                    self.error("no cell here yet");
                    return;
                };
                self.copy(&v, "cell");
            }
            Cmd::CopyRow => {
                let Some(r) = self.cursor_row.checked_sub(self.page.offset) else {
                    self.error("no row here yet");
                    return;
                };
                let r = r as usize;
                if r >= self.page.nrows {
                    self.error("no row here yet");
                    return;
                }
                // A missing value copies as the word NULL, and not as an
                // empty text. Peruse shows the two as different values
                // everywhere else, and the clipboard must not lose that
                // difference.
                let line: Vec<String> = self
                    .visible_columns()
                    .iter()
                    .map(|c| self.page.cell(r, *c).unwrap_or("NULL").to_string())
                    .collect();
                self.copy(&line.join("\t"), "row");
            }
            Cmd::IndexCsv => {
                if self.seekable {
                    self.info("already random-access");
                } else if self.indexing {
                    self.info("already indexing");
                } else {
                    self.indexing = true;
                    self.info("indexing…");
                    self.worker.send(Request::Index { epoch: self.epoch });
                }
            }
            Cmd::ThemeNext => {
                self.theme_idx = (self.theme_idx + 1) % self.themes.len();
                self.theme = self.themes[self.theme_idx].clone();
                self.remember_theme();
            }
            Cmd::ThemePicker => {
                self.theme_sel = self.theme_idx;
                self.mode = Mode::ThemePicker;
            }
            Cmd::Settings => self.open_settings(),
        }
    }
}

/// Gives `true` when the footer of the file can answer for the detail band, so
/// that the band costs no query.
///
/// Three things must hold together:
///
/// * The mode needs the NULL share only. The footer holds no count of the
///   different values and no true range.
/// * The view reads the file itself, with no filter. The footer describes the
///   whole file, so a filter or a statement of the user makes its counts wrong.
/// * The format is Parquet. No other format that Peruse reads has a footer.
fn footer_can_answer(band: Band, view: &View, format: Format) -> bool {
    !band.needs_values() && view.is_unfiltered_source() && format == Format::Parquet
}

/// Gives `true` when the facts of a column answer everything that a mode of the
/// detail band draws.
///
/// The compact band needs the share of NULL values only, and the footer of a
/// Parquet file gives that share with no query. The detailed band also needs the
/// count of the different values and the range, and the footer holds neither.
/// The band must therefore ask the engine when the user moves from the compact
/// mode to the detailed mode, and the answer of the footer is no longer enough.
fn brief_is_enough(brief: Option<&ColumnBrief>, band: Band) -> bool {
    match brief {
        None => false,
        Some(b) => !band.needs_values() || b.n_distinct.is_some(),
    }
}

/// Gives the part of a name that the user did not type yet.
///
/// The shortest name that starts with the text is the one that the user most
/// probably wants. A file with `amount` and `amount_tax` therefore gives
/// `amount` for the text `am`.
pub fn ghost_from(typed: &str, names: &[String]) -> Option<String> {
    if typed.is_empty() {
        return None;
    }
    let lower = typed.to_lowercase();
    let mut hits: Vec<&String> = names
        .iter()
        .filter(|n| n.to_lowercase().starts_with(&lower))
        .collect();
    if hits.is_empty() {
        return None;
    }
    hits.sort_by_key(|n| (n.chars().count(), n.as_str()));
    let rest: String = hits[0].chars().skip(typed.chars().count()).collect();
    (!rest.is_empty()).then_some(rest)
}

/// Gives the part that each of the names starts with.
///
/// The comparison ignores the case of the letters, because the completion
/// also ignores it. The result comes from the first name, so it keeps the
/// case that the file uses.
fn common_prefix(names: &[&str]) -> String {
    let Some(first) = names.first() else {
        return String::new();
    };
    let mut n = first.chars().count();
    for name in &names[1..] {
        let same = first
            .chars()
            .flat_map(char::to_lowercase)
            .zip(name.chars().flat_map(char::to_lowercase))
            .take_while(|(a, b)| a == b)
            .count();
        n = n.min(same);
    }
    first.chars().take(n).collect()
}

/// Puts a column name in double quotation marks when a statement needs them.
///
/// A name of letters, numbers and the character `_` needs no marks, and the
/// line stays easy to read. Each other name gets the marks, so a name with a
/// space or with a full stop still works.
fn quote_if_needed(name: &str) -> String {
    let plain = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        name.to_string()
    } else {
        peruse_core::query::quote_ident(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Makes one turn of the wheel.
    fn wheel(kind: MouseEventKind, mods: KeyModifiers) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: mods,
        }
    }

    const NONE: KeyModifiers = KeyModifiers::NONE;

    #[test]
    fn the_plain_wheel_always_moves_up_and_down() {
        // The wheel moves the view up and down in each other program. A user
        // reads a file of rows with this one movement, so it must never change
        // to a movement to the side.
        assert_eq!(
            wheel_of(&wheel(MouseEventKind::ScrollUp, NONE)),
            Wheel::Rows(-WHEEL_ROWS)
        );
        // Read the number out of the result, and do not test the constant. The
        // wheel down must move down the view, and by more than nothing.
        let Wheel::Rows(down) = wheel_of(&wheel(MouseEventKind::ScrollDown, NONE)) else {
            panic!("the wheel down must move rows");
        };
        assert!(down > 0, "the wheel down must move down the view, got {down}");
    }

    #[test]
    fn the_wheel_with_the_control_key_moves_to_the_side() {
        for mods in [KeyModifiers::CONTROL, KeyModifiers::SHIFT] {
            assert_eq!(
                wheel_of(&wheel(MouseEventKind::ScrollDown, mods)),
                Wheel::Cols(WHEEL_COLS),
                "{mods:?} with the wheel down"
            );
            assert_eq!(
                wheel_of(&wheel(MouseEventKind::ScrollUp, mods)),
                Wheel::Cols(-WHEEL_COLS),
                "{mods:?} with the wheel up"
            );
        }
        // The two keys together give the same result, and not two movements.
        let both = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(
            wheel_of(&wheel(MouseEventKind::ScrollDown, both)),
            Wheel::Cols(WHEEL_COLS)
        );
    }

    #[test]
    fn a_wheel_that_turns_to_the_side_moves_to_the_side() {
        assert_eq!(
            wheel_of(&wheel(MouseEventKind::ScrollRight, NONE)),
            Wheel::Cols(WHEEL_COLS)
        );
        assert_eq!(
            wheel_of(&wheel(MouseEventKind::ScrollLeft, NONE)),
            Wheel::Cols(-WHEEL_COLS)
        );
    }

    #[test]
    fn a_movement_of_the_pointer_moves_nothing() {
        // The terminal reports each movement of the pointer while the mouse is
        // on. Peruse does nothing with those events, and it draws no frame for
        // them.
        for kind in [
            MouseEventKind::Moved,
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Down(MouseButton::Right),
        ] {
            assert_eq!(wheel_of(&wheel(kind, NONE)), Wheel::None, "{kind:?}");
        }
    }

    /// Makes the positions of a grid of three columns.
    ///
    /// The gutter of row numbers takes the columns 0 to 3. The three columns of
    /// data then start at the column 4.
    fn hit() -> Hit {
        Hit {
            header_y: 1,
            band: 0,
            top: 2,
            rows: 10,
            cols: vec![(0, 4, 6), (1, 10, 6), (2, 16, 6)],
            // The grid covers the columns 0 to 21. A panel would start at 22.
            left: 0,
            width: 22,
        }
    }

    /// Makes the positions of the same grid with the detailed band on.
    ///
    /// The band takes four rows under the column names, so the first row of data
    /// moves from the row 2 to the row 6.
    fn hit_with_band() -> Hit {
        Hit {
            header_y: 1,
            band: Band::DETAIL_ROWS,
            top: 2 + Band::DETAIL_ROWS,
            rows: 6,
            ..hit()
        }
    }

    #[test]
    fn a_click_finds_the_row_and_the_column_under_the_pointer() {
        let h = hit();
        assert_eq!(h.row_at(2), Some(0), "the first row of data");
        assert_eq!(h.row_at(11), Some(9), "the last row of data");
        assert_eq!(h.col_at(4), Some(0), "the left edge of the first column");
        assert_eq!(h.col_at(9), Some(0), "the right edge of the first column");
        assert_eq!(h.col_at(10), Some(1), "the next column starts here");
        assert_eq!(h.col_at(21), Some(2));
    }

    #[test]
    fn a_click_outside_the_grid_finds_nothing() {
        let h = hit();
        assert_eq!(h.row_at(0), None, "the title bar");
        assert_eq!(h.row_at(1), None, "the row of the column names");
        assert_eq!(h.row_at(12), None, "the status line");
        assert_eq!(h.col_at(0), None, "the gutter of row numbers");
        assert_eq!(h.col_at(3), None, "the last column of the gutter");
        assert_eq!(h.col_at(22), None, "the space after the last column");
    }

    #[test]
    fn a_click_in_a_panel_at_the_side_finds_no_row_of_the_grid() {
        // A panel covers the same rows as the grid, so the row of a click does
        // not say that the click belongs to the grid. Without the test of the
        // column, a click on the list of columns in the metadata panel moved the
        // cursor of the grid to the row beside it.
        let h = hit();
        assert!(h.holds(0), "the gutter of row numbers is in the grid");
        assert!(h.holds(21), "the last column of the grid");
        assert!(!h.holds(22), "the first column of a panel");
        assert!(!h.holds(60), "deep inside a panel");
    }

    #[test]
    fn a_grid_with_no_room_holds_no_column() {
        // The grid writes a width of zero when the terminal is too small. No
        // click can then land on it.
        let h = Hit { width: 0, ..hit() };
        for x in 0..40 {
            assert!(!h.holds(x), "column {x}");
        }
    }

    #[test]
    fn a_grid_with_no_room_finds_nothing() {
        // The grid writes rows = 0 when the terminal is too small to hold one
        // row of data. Without this, a click would use the positions of an
        // older frame.
        let h = Hit { rows: 0, ..hit() };
        for y in 0..20 {
            assert_eq!(h.row_at(y), None, "row {y}");
        }
    }

    #[test]
    fn the_last_column_of_the_screen_does_not_overflow_the_calculation() {
        // A column that starts near the right edge of a very wide terminal can
        // make the sum of its position and its width too large for 16 bits.
        let h = Hit {
            header_y: 0,
            band: 0,
            top: 1,
            rows: 4,
            left: u16::MAX - 4,
            width: 4,
            cols: vec![(0, u16::MAX - 2, 40)],
        };
        assert_eq!(h.col_at(u16::MAX - 1), Some(0));
        assert_eq!(h.col_at(0), None);
        // The width of the grid must not overflow the sum either.
        assert!(h.holds(u16::MAX - 1));
        assert!(!h.holds(0));
    }

    #[test]
    fn a_click_under_the_band_finds_the_row_that_the_user_pointed_at() {
        // The band takes rows from the data, so the first row of data is lower.
        // Without the band in the calculation, each click would land four rows
        // above the row that the user pointed at.
        let h = hit_with_band();
        assert_eq!(h.row_at(6), Some(0), "the first row of data");
        assert_eq!(h.row_at(11), Some(5), "the last row of data");
        assert_eq!(h.row_at(12), None, "below the last row of data");
        // The band belongs to the header. A click there moves to the column and
        // leaves the row where it is.
        for y in 1..=5 {
            assert_eq!(h.row_at(y), None, "row {y} is the header or the band");
            assert!(h.on_labels(y), "row {y} must count as a label row");
        }
        assert!(!h.on_labels(0), "the title bar");
        assert!(!h.on_labels(6), "the first row of data");
    }

    #[test]
    fn with_no_band_only_the_row_of_the_names_is_a_label_row() {
        let h = hit();
        assert!(h.on_labels(1));
        assert!(!h.on_labels(2), "the first row of data");
        assert!(!h.on_labels(0));
    }

    #[test]
    fn two_quick_presses_at_one_place_are_a_double_click() {
        // A terminal reports no double click. Peruse finds it from two presses
        // at the same position, inside a short time.
        let t0 = Instant::now();
        let mut c = Clicks::default();
        assert!(!c.press(10, 5, t0), "one press alone is not a double click");
        assert!(c.press(10, 5, t0 + Duration::from_millis(120)));
    }

    #[test]
    fn a_third_press_is_not_a_second_double_click() {
        // Without this rule, a user who clicks quickly three times would open
        // the same thing two times.
        let t0 = Instant::now();
        let mut c = Clicks::default();
        assert!(!c.press(4, 4, t0));
        assert!(c.press(4, 4, t0 + Duration::from_millis(100)));
        assert!(
            !c.press(4, 4, t0 + Duration::from_millis(200)),
            "the third press must start a new pair"
        );
        // The fourth press closes that new pair.
        assert!(c.press(4, 4, t0 + Duration::from_millis(300)));
    }

    #[test]
    fn two_presses_far_apart_in_time_are_two_clicks() {
        let t0 = Instant::now();
        let mut c = Clicks::default();
        assert!(!c.press(2, 2, t0));
        assert!(!c.press(2, 2, t0 + DOUBLE_CLICK + Duration::from_millis(1)));
        // The limit itself still counts, so a press exactly at the limit opens
        // the record and does not move the cursor only.
        let mut c = Clicks::default();
        assert!(!c.press(2, 2, t0));
        assert!(c.press(2, 2, t0 + DOUBLE_CLICK));
    }

    #[test]
    fn two_presses_at_different_positions_are_two_clicks() {
        // A user who chooses one cell and then another one must not open the
        // record view. The hand moves, so the position is the test.
        let t0 = Instant::now();
        for (x, y) in [(11u16, 5u16), (10, 6), (0, 0)] {
            let mut c = Clicks::default();
            assert!(!c.press(10, 5, t0));
            assert!(
                !c.press(x, y, t0 + Duration::from_millis(50)),
                "a press at {x},{y} after one at 10,5"
            );
        }
    }

    /// Makes the positions of an overlay with a list of six lines.
    ///
    /// The box sits at the column 10 and the row 2, and it is 40 columns wide
    /// and 12 rows high. The list starts under the border, and it shows the
    /// lines 20 to 25 of a longer list.
    fn overlay() -> OverlayHit {
        let mut h = OverlayHit::new(
            Mode::Record,
            Rect {
                x: 10,
                y: 2,
                width: 40,
                height: 12,
            },
        );
        for i in 0..6u16 {
            h.line(3 + i, 20 + i as usize);
        }
        h
    }

    #[test]
    fn a_click_inside_an_overlay_is_not_a_click_outside_it() {
        // A click outside the box closes the overlay. A click inside it must
        // therefore never count as outside, or the box would close under the
        // hand of the user.
        let h = overlay();
        assert!(h.holds(10, 2), "the top left corner of the border");
        assert!(h.holds(49, 13), "the bottom right corner of the border");
        assert!(h.holds(30, 8), "the middle of the box");
        assert!(!h.holds(9, 8), "one column to the left of the box");
        assert!(!h.holds(50, 8), "one column to the right of the box");
        assert!(!h.holds(30, 1), "one row above the box");
        assert!(!h.holds(30, 14), "one row below the box");
        assert!(!h.holds(0, 0), "the title bar of the screen");
    }

    #[test]
    fn a_click_in_a_list_finds_the_line_under_the_pointer() {
        // The record of one row can hold hundreds of lines, so the list
        // scrolls. A click must find the line under the pointer, and not the
        // line at that offset in the list.
        let h = overlay();
        assert_eq!(h.line_at(3), Some(20), "the first line on the screen");
        assert_eq!(h.line_at(5), Some(22));
        assert_eq!(h.line_at(8), Some(25), "the last line on the screen");
        // The border, the row of keys and each row outside the list hold no
        // line, so a click there changes nothing.
        assert_eq!(h.line_at(2), None, "the top border");
        assert_eq!(h.line_at(9), None, "under the last line");
        assert_eq!(h.line_at(13), None, "the row of keys");
    }

    #[test]
    fn an_overlay_with_no_list_answers_no_line() {
        // The help and the cell inspector hold text and no list. A click inside
        // them selects nothing.
        let h = OverlayHit::new(Mode::Help, Rect::new(0, 0, 20, 10));
        for y in 0..12 {
            assert_eq!(h.line_at(y), None, "row {y}");
        }
        assert!(h.holds(0, 0));
    }

    #[test]
    fn the_band_moves_through_off_compact_and_detailed() {
        assert_eq!(Band::Off.next(), Band::Compact);
        assert_eq!(Band::Compact.next(), Band::Detailed);
        assert_eq!(Band::Detailed.next(), Band::Off);
        // Three presses of the key come back to the start.
        assert_eq!(Band::Off.next().next().next(), Band::Off);
        assert_eq!(Band::Off.rows(), 0);
        assert_eq!(Band::Compact.rows(), 1);
        assert_eq!(Band::Detailed.rows(), Band::DETAIL_ROWS);
        // Only the detailed band needs the values, so the compact band over a
        // Parquet file can come from the footer.
        assert!(!Band::Compact.needs_values());
        assert!(Band::Detailed.needs_values());
    }

    #[test]
    fn a_plain_parquet_file_needs_no_query_for_the_compact_band() {
        // The footer of a Parquet file holds the number of rows and the number
        // of NULL values of each column. The compact band needs nothing more, so
        // it costs no query, also on a file of some gigabytes.
        let plain = View::default();
        assert!(footer_can_answer(Band::Compact, &plain, Format::Parquet));

        // The detailed band needs the values, and the footer holds none.
        assert!(!footer_can_answer(Band::Detailed, &plain, Format::Parquet));
        // No other format has a footer.
        for f in [Format::Csv, Format::Json, Format::Arrow] {
            assert!(!footer_can_answer(Band::Compact, &plain, f), "{f:?}");
        }
        // The footer describes the whole file. A filter and a statement of the
        // user both change the counts, so the engine must measure them.
        let filtered = View {
            filter: Some("id > 1".into()),
            ..Default::default()
        };
        assert!(!footer_can_answer(Band::Compact, &filtered, Format::Parquet));
        let sql = View {
            base: Base::Sql("SELECT * FROM src".into()),
            ..Default::default()
        };
        assert!(!footer_can_answer(Band::Compact, &sql, Format::Parquet));
    }

    #[test]
    fn the_answer_of_the_footer_is_not_enough_for_the_detailed_band() {
        // The footer of a Parquet file gives the NULL share and no more. A user
        // who moves from the compact band to the detailed band must therefore
        // start a query, and the two rows of the values must not stay as points.
        let from_footer = ColumnBrief {
            column: "id".into(),
            n_total: 100,
            n_present: 90,
            n_distinct: None,
            min: None,
            max: None,
        };
        let measured = ColumnBrief {
            n_distinct: Some(90),
            ..from_footer.clone()
        };
        assert!(brief_is_enough(Some(&from_footer), Band::Compact));
        assert!(!brief_is_enough(Some(&from_footer), Band::Detailed));
        // A measured column answers each mode, so a move back to the compact
        // band asks for nothing.
        assert!(brief_is_enough(Some(&measured), Band::Compact));
        assert!(brief_is_enough(Some(&measured), Band::Detailed));
        // A column with no facts at all always needs an answer.
        assert!(!brief_is_enough(None, Band::Compact));
        assert!(!brief_is_enough(None, Band::Detailed));
    }

    #[test]
    fn the_name_of_the_band_reads_back_from_the_settings_file() {
        for b in [Band::Off, Band::Compact, Band::Detailed] {
            assert_eq!(Band::parse(b.name()), Some(b), "{}", b.name());
        }
        assert_eq!(Band::parse(" Detailed "), Some(Band::Detailed));
        assert_eq!(Band::parse(""), Some(Band::Off));
        // A name that Peruse does not know is not a band.
        assert_eq!(Band::parse("wide"), None);
    }

    #[test]
    fn a_settings_file_with_a_fault_is_never_written_over() {
        // `Config::to_toml` writes the whole file from the fields that Peruse
        // holds, and a file that does not parse gives the built-in fields. A
        // write would therefore replace each setting that the user wrote, and
        // every note in the file. The key `d` writes the file at each press, so
        // one character wrong in the file would cost the user the rest of it.
        let dir = std::env::temp_dir().join("peruse-write-blocked");
        std::fs::create_dir_all(&dir).unwrap();

        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "theme =\n").unwrap();
        let why = write_blocked(&bad).expect("a file that does not parse must block a write");
        assert!(why.contains("nothing was written"), "{why}");

        let good = dir.join("good.toml");
        std::fs::write(&good, "theme = \"nord\"\nstep = 25\n").unwrap();
        assert_eq!(write_blocked(&good), None, "a file that parses must not block");

        // A file that is not there is not a fault. Peruse writes a new one.
        let missing = dir.join("not-there.toml");
        let _ = std::fs::remove_file(&missing);
        assert_eq!(write_blocked(&missing), None);

        // A name that Peruse does not know is a fault too, and it is the easy
        // mistake to make by hand.
        let unknown = dir.join("unknown.toml");
        std::fs::write(&unknown, "thheme = \"nord\"\n").unwrap();
        assert!(write_blocked(&unknown).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_step_stays_inside_its_limits() {
        assert_eq!(step_of(None), DEFAULT_STEP as i64);
        assert_eq!(step_of(Some(1)), 1);
        assert_eq!(step_of(Some(25)), 25);
        // A step of zero would give a key that does nothing.
        assert_eq!(step_of(Some(0)), 1);
        // A step of some million rows is a jump, and the key # does that.
        assert_eq!(step_of(Some(usize::MAX)), MAX_STEP as i64);
    }

    #[test]
    fn a_fitted_column_always_keeps_the_room_after_its_name() {
        // A column whose values are as narrow as its name ends as wide as its
        // name. The header then drops the type mark, and the band drops the
        // type. Every fitted column therefore keeps NAME_HEADROOM after the
        // name, and it still covers the widest value.
        for name in [0usize, 1, 2, 6, 13, 40] {
            for widest in [0usize, 1, 4, 9, 30, 55] {
                let w = fitted_width(name, widest) as usize;
                assert!(
                    w >= name + NAME_HEADROOM,
                    "a name of {name} with a value of {widest} got the width {w}"
                );
                assert!(
                    w >= widest.min(MAX_COL_WIDTH as usize),
                    "a value of {widest} does not fit in the width {w}"
                );
                assert!(w >= MIN_COL_WIDTH as usize && w <= MAX_COL_WIDTH as usize);
            }
        }
    }

    #[test]
    fn a_very_long_name_stops_at_the_largest_width() {
        // The room after the name never breaks the limit. One column with a
        // long name must not push the other columns off the right edge.
        assert_eq!(fitted_width(MAX_COL_WIDTH as usize, 0), MAX_COL_WIDTH);
        assert_eq!(fitted_width(200, 4), MAX_COL_WIDTH);
        // A name of some thousand characters does not fit in 16 bits, so the
        // limit must apply before the cast.
        assert_eq!(fitted_width(100_000, 0), MAX_COL_WIDTH);
    }
}
