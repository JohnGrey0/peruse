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
use peruse_core::filter::{FilterSet, Op, Term};
use peruse_core::query::{quote_str, Step};
use peruse_core::meta::FileMeta;
use peruse_core::model::RowCount;
use peruse_core::query::{Base, SortDir, SortKey};
use peruse_core::stats::ColumnStats;
use peruse_core::theme::Theme;
use peruse_core::{sql_guard, Opened, Request, Response, RowPage, Schema, Source, View, Worker};
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::clip;
use crate::commands::{self, Cmd};
use crate::input::{Action, LineInput};
use crate::tree::{Family, Line, Tree};

/// The number of rows that Peruse reads before the first row of the viewport
/// and after the last row. A usual scroll therefore does not wait for the
/// engine.
const PREFETCH: u64 = 250;
/// The number of rows that the worker examines in one search request.
///
/// The number is small, so a match near the cursor comes back immediately, and
/// the user can stop a search that finds nothing. The number is also large, so
/// a search of a full file does not need thousands of requests.
const SEARCH_CHUNK: u64 = 250_000;
/// The largest number of match offsets that one search request gives.
const SEARCH_HITS: u32 = 500;
/// The largest size of a CSV file that Peruse indexes when it opens the file.
///
/// A scan of this size is quick, and the user does not notice it. A jump to
/// the last row then costs almost no time. For a larger file, Peruse waits for
/// the key `I`. The user therefore never waits for a scan that the user did
/// not ask for.
const AUTO_INDEX_BYTES: u64 = 256 * 1024 * 1024;

/// The smallest width of a column, in screen columns.
const MIN_COL_WIDTH: u16 = 3;
/// The largest width that Peruse gives to a column when it fits the widths.
const MAX_COL_WIDTH: u16 = 60;

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
}

impl Setting {
    /// Each setting, in the order that the page shows them.
    pub const ALL: &'static [Setting] = &[
        Setting::Theme,
        Setting::Threads,
        Setting::MemoryLimit,
        Setting::SampleSize,
        Setting::NoIndex,
    ];

    /// The name that the page shows.
    pub fn label(self) -> &'static str {
        match self {
            Setting::Theme => "theme",
            Setting::Threads => "threads",
            Setting::MemoryLimit => "memory limit",
            Setting::SampleSize => "sample size",
            Setting::NoIndex => "index at open",
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    /// No panel is open.
    None,
    /// The metadata panel is open.
    Meta,
    /// The column statistics panel is open.
    Stats,
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

    /// The statistics of the column under the cursor.
    pub stats: Option<ColumnStats>,
    /// The metadata of the file.
    pub meta: Option<FileMeta>,
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

            stats: None,
            meta: None,
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
        if auto_index && !app.seekable && app.source.bytes < AUTO_INDEX_BYTES {
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
        self.stats = None;
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
        self.stats = None;
        if self.panel == Panel::Stats {
            self.request_stats();
        }
    }

    /// Asks the worker for the statistics of the column under the cursor.
    fn request_stats(&mut self) {
        let Some(col) = self.schema.columns.get(self.cursor_col).cloned() else {
            return;
        };
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
    /// widest value in the page. The width has a limit, so one long column of
    /// text cannot push the other columns off the right edge.
    pub fn fit_widths(&mut self) {
        for (i, col) in self.schema.columns.iter().enumerate() {
            let mut w = crate::text::width(&col.name).max(4);
            for r in 0..self.page.nrows {
                let cell = self.page.cell(r, i).unwrap_or("NULL");
                w = w.max(crate::text::width(cell));
                if w >= MAX_COL_WIDTH as usize {
                    break;
                }
            }
            self.widths[i] = (w as u16).clamp(MIN_COL_WIDTH, MAX_COL_WIDTH);
        }
        self.widths_fitted = true;
    }

    // -------------------------------------------------------------- responses

    /// Adds one response from the worker to the state.
    ///
    /// The function discards each response with an old epoch, because the view
    /// changed after the request.
    pub fn on_response(&mut self, resp: Response) {
        // The response Busy is the one response with no view behind it.
        if let Response::Busy(b) = resp {
            self.busy = b;
            return;
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
                return;
            }
            Response::Meta { ref meta, .. } => {
                self.meta = Some((**meta).clone());
                return;
            }
            _ => {}
        }
        let epoch = match &resp {
            Response::Schema { epoch, .. }
            | Response::Page { epoch, .. }
            | Response::Count { epoch, .. }
            | Response::Stats { epoch, .. }
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
            return; // a response for a view that the user left
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
            Response::Stats { stats, .. } => self.stats = Some(*stats),
            Response::RowJson { row, json, .. } => {
                // The user can move to another row while this answer is on
                // its way. The epoch cannot see that, because the view did
                // not change, so the row itself is the test.
                if Some(row) != self.record_row {
                    return;
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
                // The first line holds the useful part. DuckDB adds the full
                // statement after it, and that text is too long for a status
                // line of one row.
                let first = message.lines().next().unwrap_or(&message).trim().to_string();
                self.error(format!("{context}: {first}"));
            }
        }
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
        });
        self.request_scan_chunk();
    }

    /// Asks the worker to examine the next part of the view.
    ///
    /// Each call covers [`SEARCH_CHUNK`] rows at the most. The worker
    /// therefore answers quickly, the user interface stays live, and the key
    /// `Esc` can stop the search.
    fn request_scan_chunk(&mut self) {
        let Some(s) = self.scan.as_mut() else { return };
        let (from, len) = if s.forward {
            let from = if s.next >= s.total { 0 } else { s.next };
            let len = SEARCH_CHUNK.min(s.total.saturating_sub(from)).max(1);
            s.next = from + len;
            (from, len)
        } else {
            let end = if s.next == 0 { s.total } else { s.next };
            let from = end.saturating_sub(SEARCH_CHUNK);
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
                self.theme_sel = self.theme_sel.saturating_sub(1);
                self.theme = self.themes[self.theme_sel].clone();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.theme_sel = (self.theme_sel + 1).min(self.themes.len() - 1);
                self.theme = self.themes[self.theme_sel].clone();
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
        let lower = word.to_lowercase();
        let names: Vec<&str> = self
            .schema
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .filter(|n| n.to_lowercase().starts_with(&lower))
            .collect();
        match names.len() {
            0 => self.prompt_error = Some(format!("no column starts with {word:?}")),
            1 => {
                let name = quote_if_needed(names[0]);
                self.input.replace_word_before_cursor(&name);
                self.prompt_error = None;
            }
            _ => {
                // Put in the part that each name starts with. The user can
                // then type one more character and press Tab again.
                let common = common_prefix(&names);
                if common.chars().count() > word.chars().count() {
                    self.input.replace_word_before_cursor(&common);
                }
                let shown: Vec<&str> = names.iter().take(6).copied().collect();
                let more = names.len().saturating_sub(shown.len());
                self.prompt_error = Some(if more > 0 {
                    format!("{} … +{more}", shown.join(" "))
                } else {
                    shown.join(" ")
                });
            }
        }
    }

    /// Handles a key in the prompt.
    fn prompt_key(&mut self, kind: PromptKind, key: &KeyEvent) {
        // The line editor has no list of columns, so the completion happens
        // here, in front of it.
        if key.code == KeyCode::Tab && matches!(kind, PromptKind::Filter | PromptKind::Sql) {
            self.complete_column();
            return;
        }
        match self.input.handle(key) {
            Action::Cancel => {
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
                    Base::Sql(s) => s.clone(),
                    // Fill the prompt with the statement behind the grid.
                    // The user then starts from a statement that works, and
                    // not from an empty line.
                    Base::Source => "SELECT * FROM src".to_string(),
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
                .map(|gb| format!("{gb} GB (a quarter of this machine)"))
                .unwrap_or_else(|| "the rule of DuckDB".into()),
            Setting::SampleSize => "20,480 rows".into(),
            Setting::NoIndex => "yes, below 256 MB".into(),
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

    /// Writes the settings to the file.
    fn save_config(&self) -> Result<(), String> {
        let Some(p) = &self.config_path else {
            return Err("this system gives no directory for settings".into());
        };
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
                            self.ok(format!("memory limit: {gb} GB, a quarter of this machine"));
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

            Cmd::ToggleMeta => {
                self.panel = if self.panel == Panel::Meta { Panel::None } else { Panel::Meta };
                if self.panel == Panel::Meta && self.meta.is_none() {
                    self.worker.send(Request::Meta { epoch: self.epoch });
                }
            }
            Cmd::ToggleStats => {
                self.panel = if self.panel == Panel::Stats { Panel::None } else { Panel::Stats };
                if self.panel == Panel::Stats {
                    self.request_stats();
                }
            }
            Cmd::Record => self.open_record(),
            Cmd::InspectCell => {
                let Some(col) = self.schema.columns.get(self.cursor_col) else {
                    return;
                };
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
