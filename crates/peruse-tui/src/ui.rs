//! The layout of the frame, and the parts around the grid.
//!
//! The frame has four rows: the title bar, the body, the status line and the
//! footer. The body holds the grid, and a panel when the user opens one.

use peruse_core::model::RowCount;
use peruse_core::query::Base;
use peruse_core::source::{human_bytes, human_count};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::Frame;

use crate::app::{App, Mode, Panel, PromptKind, StatusKind};
use crate::colors::Depth;
use crate::commands;
use crate::overlays::{self, FOOTER_HINTS};
use crate::paint::Paint;
use crate::sqlhl::{self, Tok};
use crate::{grid, panels, text};

/// The smallest width of the terminal for a panel at the side of the grid.
/// Below this width, the panel goes below the grid.
const SIDE_PANEL_MIN_WIDTH: u16 = 100;
/// The width of a panel at the side of the grid, in screen columns.
const SIDE_PANEL_WIDTH: u16 = 46;
/// The height of a panel below the grid, in rows.
const BOTTOM_PANEL_HEIGHT: u16 = 12;

/// Draws one complete frame.
pub fn draw(f: &mut Frame, app: &mut App, depth: Depth) {
    let p = Paint::new(depth);
    let area = f.area();
    let t = app.theme.clone();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // the title bar
            Constraint::Min(3),    // the body
            Constraint::Length(1), // the status line or the prompt
            Constraint::Length(1), // the footer
        ])
        .split(area);

    let body = if app.panel == Panel::None {
        rows[1]
    } else if area.width >= SIDE_PANEL_MIN_WIDTH {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Length(SIDE_PANEL_WIDTH)])
            .split(rows[1]);
        draw_panel(f, cols[1], app, &p);
        cols[0]
    } else {
        let h = BOTTOM_PANEL_HEIGHT.min(rows[1].height.saturating_sub(4));
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(h)])
            .split(rows[1]);
        draw_panel(f, split[1], app, &p);
        split[0]
    };

    draw_title(f, rows[0], app, &p);
    grid::draw(f.buffer_mut(), body, app, &p);

    let cursor = draw_status(f, rows[2], app, &p);
    draw_footer(f, rows[3], app, &p);

    // An overlay that takes text gives the position of the terminal cursor
    // back. The caller therefore needs no second calculation of the layout.
    //
    // Each overlay also gives its box back. This frame is the only place that
    // knows where an overlay sits, and the mouse needs the box. A mode with no
    // overlay writes `None`, so a click can never act on a box that is gone.
    let mut overlay_cursor = None;
    let overlay = match app.mode {
        Mode::Help => Some(overlays::draw_help(f.buffer_mut(), area, app, &p)),
        Mode::Palette => Some(overlays::draw_palette(f.buffer_mut(), area, app, &p)),
        Mode::ThemePicker => Some(overlays::draw_theme_picker(f.buffer_mut(), area, app, &p)),
        Mode::Cell => Some(overlays::draw_cell(f.buffer_mut(), area, app, &p)),
        Mode::Record => {
            let drawn = overlays::draw_record(f.buffer_mut(), area, app, &p);
            overlay_cursor = drawn.cursor;
            Some(drawn.hit)
        }
        Mode::FilterBuild => {
            let drawn = overlays::draw_filter_build(f.buffer_mut(), area, app, &p);
            overlay_cursor = drawn.cursor;
            Some(drawn.hit)
        }
        Mode::Settings => {
            let drawn = overlays::draw_settings(f.buffer_mut(), area, app, &p);
            overlay_cursor = drawn.cursor;
            Some(drawn.hit)
        }
        _ => None,
    };
    app.overlay = overlay;

    if let Some(pos) = overlay_cursor {
        f.set_cursor_position(pos);
        return;
    }

    // The palette holds its own prompt inside the overlay. Ask the overlay for
    // its box, so one calculation gives the box and the cursor. The prompt is
    // on the row `y + 1`, and its text starts at the column `x + 4`.
    if app.mode == Mode::Palette {
        let r = overlays::palette_rect(area);
        f.set_cursor_position(Position::new(
            r.x + 4 + app.input.cursor_col() as u16,
            r.y + 1,
        ));
    } else if let Some(pos) = cursor {
        f.set_cursor_position(pos);
    }

    let _ = t;
}

/// The smallest height that holds the two panels, one above the other.
///
/// Below this, each of the two would have one row of text inside its border.
/// One panel with room to say something is more use than two with none.
const BOTH_PANELS_MIN_HEIGHT: u16 = 14;
/// The smallest height of the statistics panel in the stacked view.
///
/// The statistics of a column arrive after the engine reads them, and the panel
/// holds one row until they do. This limit keeps the panel near the height of
/// the first numbers, so the border makes a small step and not a large one.
const STATS_MIN_HEIGHT: u16 = 8;
/// The rows that the metadata panel always keeps in the stacked view.
///
/// Two rows go to the border, one row to the heading `columns`, and the rest to
/// the list of columns and the first rows of the summary. Below this the panel
/// says nothing that the user can read.
const META_MIN_HEIGHT: u16 = 8;

/// Draws the panel that is open, or the two panels one above the other.
fn draw_panel(f: &mut Frame, area: Rect, app: &App, p: &Paint) {
    match app.panel {
        Panel::Meta => panels::draw_meta(f.buffer_mut(), area, app, p),
        Panel::Stats => panels::draw_stats(f.buffer_mut(), area, app, p),
        Panel::Both => {
            if area.height < BOTH_PANELS_MIN_HEIGHT {
                // Say why the second panel is not there. A panel that goes
                // away with no word looks like a fault.
                panels::draw_stats(f.buffer_mut(), area, app, p);
                panels::note(f.buffer_mut(), area, app, p, " screen too short for both ");
                return;
            }
            // The metadata goes on top, and the statistics below it. The order
            // never changes, so the eye finds each of them on the same side.
            //
            // The line between them moves, because each panel takes the height
            // that its own content needs. A column of numbers asks for four
            // more rows than a column of text, for the chart of the
            // distribution.
            let (meta, stats) = split_panels(area, panels::stats_content_height(app));
            panels::draw_meta(f.buffer_mut(), meta, app, p);
            panels::draw_stats(f.buffer_mut(), stats, app, p);
        }
        Panel::None => {}
    }
}

/// Divides the side pane between the metadata panel above and the statistics
/// panel below.
///
/// `stats_content` is the count of rows that the statistics need inside their
/// border. The statistics get that height and no more, because they have an
/// end. The metadata then keeps each row that is left: it holds the list of
/// columns, and that list has no end.
///
/// The limit on the statistics is the room that the metadata must keep, and not
/// one half of the pane. One half is the wrong limit in the two directions: on
/// a tall pane it cuts the statistics while the metadata above them holds empty
/// rows, and on a short pane it gives the metadata too little to read.
///
/// The result covers the area exactly, with no gap and no overlap.
fn split_panels(area: Rect, stats_content: u16) -> (Rect, Rect) {
    // Two rows for the border of the statistics panel.
    let want = stats_content.saturating_add(2).max(STATS_MIN_HEIGHT);
    // A column with many frequent values must not push the metadata out. On a
    // pane that cannot hold the two minimum heights, the metadata keeps one
    // half: the two panels are then both small, and neither one is empty.
    let room = area
        .height
        .saturating_sub(META_MIN_HEIGHT)
        .max(area.height / 2);
    let stats_h = want.min(room);
    let meta_h = area.height.saturating_sub(stats_h);
    (
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: meta_h,
        },
        Rect {
            x: area.x,
            y: area.y.saturating_add(meta_h),
            width: area.width,
            height: stats_h,
        },
    )
}

/// Draws the title bar.
fn draw_title(f: &mut Frame, area: Rect, app: &App, p: &Paint) {
    // The layout gives an area of zero height when the terminal is too small
    // for each part of the frame. Peruse cannot draw in such an area.
    if area.height == 0 || area.width == 0 {
        return;
    }
    let t = &app.theme;
    let buf = f.buffer_mut();
    let base = p.on(t.fg, t.bg_alt);
    buf.set_stringn(area.x, area.y, " ".repeat(area.width as usize), area.width as usize, base);

    let mut x = area.x + 1;
    let put = |s: &str, style, buf: &mut ratatui::buffer::Buffer, x: &mut u16| {
        if *x >= area.right() {
            return;
        }
        let w = (area.right() - *x) as usize;
        buf.set_stringn(*x, area.y, text::truncate(s, w), w, style);
        *x += text::width(s).min(w) as u16;
    };

    put("peruse", p.bold(p.on(t.accent, t.bg_alt)), buf, &mut x);
    put("  ", base, buf, &mut x);
    put(&app.source.title(), p.bold(base), buf, &mut x);
    put("  ", base, buf, &mut x);

    let shape = match app.total {
        RowCount::Exact(n) => format!("{} × {}", human_count(n), app.schema.len()),
        _ => format!("… × {}", app.schema.len()),
    };
    put(&shape, base, buf, &mut x);
    put("  ", base, buf, &mut x);
    put(&human_bytes(app.source.bytes), p.on(t.dim, t.bg_alt), buf, &mut x);
    put("  ", base, buf, &mut x);
    put(&app.source.format.to_string(), p.on(t.dim, t.bg_alt), buf, &mut x);

    // Show each part of the view that removes rows or columns. The user
    // must always know why the grid shows fewer rows than the file holds.
    //
    // The filter shows its own text, and not the word "filtered" alone. The
    // user can then read the condition without a press of a key. A long
    // expression gets a short form, and the key `f` shows it in full.
    if let Some(f) = &app.view.filter {
        put("  ", base, buf, &mut x);
        let room = (area.right().saturating_sub(x) as usize).saturating_sub(24);
        put(
            &format!("filter: {}", text::truncate(f, room.clamp(8, 48))),
            p.on(t.warn, t.bg_alt),
            buf,
            &mut x,
        );
    }
    if matches!(app.view.base, Base::Sql(_)) {
        put("  ", base, buf, &mut x);
        put("query", p.on(t.warn, t.bg_alt), buf, &mut x);
    }
    if let Some(k) = app.view.sort.first() {
        put("  ", base, buf, &mut x);
        put(&format!("{}{}", k.dir.arrow(), k.column), p.on(t.warn, t.bg_alt), buf, &mut x);
    }
    let hidden = app.hidden.iter().filter(|h| **h).count();
    if hidden > 0 {
        put("  ", base, buf, &mut x);
        put(&format!("{hidden} hidden"), p.on(t.warn, t.bg_alt), buf, &mut x);
    }

    // At the right side: the work of the worker, or the name of the theme.
    let right = if app.busy {
        "⟳ running · Esc cancels".to_string()
    } else if app.indexing {
        "⟳ indexing".to_string()
    } else {
        app.theme.name.clone()
    };
    let rw = text::width(&right);
    if area.width as usize > rw + 2 {
        let rx = area.right() - rw as u16 - 1;
        if rx > x {
            let style = if app.busy || app.indexing {
                p.on(t.warn, t.bg_alt)
            } else {
                p.on(t.dim, t.bg_alt)
            };
            buf.set_stringn(rx, area.y, &right, rw, style);
        }
    }
}

/// Draws the status line or the prompt.
///
/// The function gives the position of the terminal cursor, or `None` when the
/// terminal cursor must stay invisible.
fn draw_status(f: &mut Frame, area: Rect, app: &App, p: &Paint) -> Option<Position> {
    // The layout gives an area of zero height when the terminal is too small
    // for each part of the frame. Peruse cannot draw in such an area.
    if area.height == 0 || area.width == 0 {
        return None;
    }
    let t = &app.theme;
    let buf = f.buffer_mut();
    let base = p.on(t.fg, t.bg);
    buf.set_stringn(area.x, area.y, " ".repeat(area.width as usize), area.width as usize, base);

    if let Mode::Prompt(kind) = app.mode {
        let label = format!("{} › ", kind.label());
        let lw = text::width(&label);
        buf.set_stringn(area.x, area.y, &label, lw, p.bold(p.on(t.accent, t.bg)));

        let value = app.input.text();
        let vx = area.x + lw as u16;
        let avail = (area.width as usize).saturating_sub(lw);

        match kind {
            PromptKind::Filter | PromptKind::Sql => {
                let mut x = vx;
                for (seg, tok) in sqlhl::tokens(&value) {
                    if x >= area.right() {
                        break;
                    }
                    let style = match tok {
                        Tok::Keyword => p.bold(p.on(t.kw, t.bg)),
                        Tok::Str => p.on(t.lit, t.bg),
                        Tok::Num => p.on(t.number, t.bg),
                        Tok::Comment => p.on(t.comment, t.bg),
                        Tok::Ident => p.on(t.ident, t.bg),
                        Tok::Punct | Tok::Space => base,
                    };
                    let w = (area.right() - x) as usize;
                    buf.set_stringn(x, area.y, &seg, w, style);
                    x += text::width(&seg).min(w) as u16;
                }
            }
            _ => {
                buf.set_stringn(vx, area.y, text::truncate(&value, avail), avail, base);
            }
        }

        // The name of a column that the user did not type yet, in a dim
        // color, after the cursor. The key Tab and the key -> take it.
        if let Some(rest) = app.ghost() {
            let gx = vx + app.input.cursor_col() as u16;
            if gx < area.right() {
                let w = (area.right() - gx) as usize;
                buf.set_stringn(
                    gx,
                    area.y,
                    text::truncate(&rest, w),
                    w,
                    p.on(t.dim, t.bg),
                );
            }
        }

        // Report a bad expression while the user types it. The message
        // goes at the right side, so it does not move the text of the user.
        if let Some(err) = &app.prompt_error {
            let msg = format!(" ✕ {err}");
            let mw = text::width(&msg).min(area.width as usize / 2);
            if mw > 4 {
                let mx = area.right() - mw as u16;
                buf.set_stringn(mx, area.y, text::truncate(&msg, mw), mw, p.on(t.error, t.bg));
            }
        }

        return Some(Position::new(
            (vx + app.input.cursor_col() as u16).min(area.right() - 1),
            area.y,
        ));
    }

    if let Some(s) = &app.status {
        let style = match s.kind {
            StatusKind::Error => p.on(t.error, t.bg),
            StatusKind::Ok => p.on(t.ok, t.bg),
            StatusKind::Info => p.on(t.fg, t.bg),
        };
        let icon = match s.kind {
            StatusKind::Error => "✕ ",
            StatusKind::Ok => "✓ ",
            StatusKind::Info => "· ",
        };
        let msg = format!("{icon}{}", s.text);
        buf.set_stringn(
            area.x + 1,
            area.y,
            text::truncate(&msg, area.width.saturating_sub(2) as usize),
            area.width.saturating_sub(2) as usize,
            style,
        );
        return None;
    }

    // With no message: the position of the cursor, and the column under it.
    let mut left = String::new();
    if let Some(col) = app.schema.columns.get(app.cursor_col) {
        let vis = app.visible_columns();
        let at = vis.iter().position(|c| *c == app.cursor_col).unwrap_or(0) + 1;
        left = format!("{} {}  ·  col {at}/{}", col.name, col.short_type(), vis.len());
    }
    buf.set_stringn(
        area.x + 1,
        area.y,
        text::truncate(&left, area.width.saturating_sub(2) as usize),
        area.width.saturating_sub(2) as usize,
        p.on(t.dim, t.bg),
    );

    // The number of the row after the cursor saturates. While the count is
    // unknown, the cursor can sit on a very large row number.
    let at = app.cursor_row.saturating_add(1);
    let right = match app.total {
        RowCount::Exact(0) => "no rows".to_string(),
        RowCount::Exact(n) => format!(
            "row {}/{}  {:>3}%",
            human_count(at),
            human_count(n),
            (at.min(n)) * 100 / n
        ),
        _ => format!("row {}  ·  {}", human_count(at), app.total.label()),
    };
    let rw = text::width(&right);
    if area.width as usize > rw + text::width(&left) + 3 {
        buf.set_stringn(
            area.right() - rw as u16 - 1,
            area.y,
            &right,
            rw,
            p.on(t.dim, t.bg),
        );
    }
    None
}

/// Draws the footer with the key hints.
fn draw_footer(f: &mut Frame, area: Rect, app: &App, p: &Paint) {
    // The layout gives an area of zero height when the terminal is too small
    // for each part of the frame. Peruse cannot draw in such an area.
    if area.height == 0 || area.width == 0 {
        return;
    }
    let t = &app.theme;
    let buf = f.buffer_mut();
    let base = p.on(t.status_fg, t.status_bg);
    buf.set_stringn(area.x, area.y, " ".repeat(area.width as usize), area.width as usize, base);

    // The hints follow the mode. An overlay and a prompt show their own
    // keys. The keys of the grid do not work in an overlay or a prompt.
    let pairs: Vec<(String, String)> = match app.mode {
        Mode::Normal => FOOTER_HINTS
            .iter()
            .filter_map(|c| commands::binding(*c))
            .map(|b| (b.label.to_string(), short_desc(b.desc)))
            .collect(),
        Mode::Prompt(_) => vec![
            ("Enter".into(), "apply".into()),
            ("Esc".into(), "cancel".into()),
            ("↑↓".into(), "history".into()),
        ],
        Mode::Help | Mode::Cell => vec![
            ("j/k".into(), "scroll".into()),
            ("Esc".into(), "close".into()),
        ],
        // These overlays write their own keys along their bottom edge, where
        // the user is already looking.
        Mode::Record | Mode::FilterBuild | Mode::Settings => {
            vec![("Esc".into(), "close".into())]
        }
        Mode::Palette => vec![
            ("↑↓".into(), "select".into()),
            ("Enter".into(), "run".into()),
            ("Esc".into(), "close".into()),
        ],
        Mode::ThemePicker => vec![
            ("↑↓".into(), "preview".into()),
            ("Enter".into(), "apply".into()),
            ("Esc".into(), "cancel".into()),
        ],
    };

    // A CSV file with no index has a real limit, and the user must know it.
    // The note therefore takes its space first, and the key hints fill the
    // space that stays. The loss of the last key hint costs nothing. The
    // loss of the note costs the user the reason for a slow jump.
    let note = (!app.seekable && !app.indexing && app.mode == Mode::Normal)
        .then(|| {
            format!(
                "{}: press I to index for instant jumps",
                app.source.format.to_string().to_uppercase()
            )
        })
        .filter(|n| text::width(n) + 12 < area.width as usize);
    let hint_budget = area.right() as usize
        - note.as_deref().map(|n| text::width(n) + 2).unwrap_or(0)
        - 1;

    let mut x = area.x + 1;
    for (k, d) in pairs {
        let need = text::width(&k) + text::width(&d) + 3;
        if x as usize + need >= hint_budget {
            break;
        }
        buf.set_stringn(x, area.y, &k, k.len(), p.bold(p.on(t.key_fg, t.status_bg)));
        x += text::width(&k) as u16;
        buf.set_stringn(x + 1, area.y, &d, d.len(), base);
        x += text::width(&d) as u16 + 3;
    }

    if let Some(note) = &note {
        let w = text::width(note);
        buf.set_stringn(
            area.right() - w as u16 - 1,
            area.y,
            note,
            w,
            p.on(t.warn, t.status_bg),
        );
    }
}

/// Gives a short form of a command description, for the footer.
///
/// The footer has little space. The help overlay shows the long form.
fn short_desc(desc: &str) -> String {
    let first = desc.split([' ', '(']).next().unwrap_or(desc);
    match desc {
        d if d.starts_with("build a filter") => "filter".into(),
        d if d.starts_with("undo the last") => "undo".into(),
        d if d.starts_with("redo the change") => "redo".into(),
        d if d.starts_with("show this row") => "record".into(),
        d if d.starts_with("filter rows") => "where".into(),
        d if d.starts_with("edit the SQL") => "query".into(),
        d if d.starts_with("sort by") => "sort".into(),
        d if d.starts_with("file metadata") => "meta".into(),
        d if d.starts_with("statistics") => "stats".into(),
        d if d.starts_with("search all") => "search".into(),
        d if d.starts_with("run a command") => "commands".into(),
        d if d.starts_with("this help") => "help".into(),
        _ => first.to_string(),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_shorten_for_the_footer() {
        assert_eq!(short_desc("filter rows with a WHERE expression"), "where");
        assert_eq!(
            short_desc("build a filter from menus (no SQL needed)"),
            "filter"
        );
        assert_eq!(
            short_desc("show this row as a vertical record, one column per line"),
            "record"
        );
        assert_eq!(short_desc("sort by this column (asc → desc → off)"), "sort");
        assert_eq!(short_desc("quit"), "quit");
    }

    /// A side pane of the given height, as the layout of the body gives it.
    fn pane(height: u16) -> Rect {
        Rect {
            x: 54,
            y: 1,
            width: SIDE_PANEL_WIDTH,
            height,
        }
    }

    /// Tests that the two panels cover the pane and nothing outside it.
    fn covers_the_pane(area: Rect, meta: Rect, stats: Rect) {
        assert_eq!(meta.y, area.y, "the metadata starts at the top");
        assert_eq!(stats.y, meta.bottom(), "no gap and no overlap");
        assert_eq!(
            meta.height + stats.height,
            area.height,
            "the two panels must fill the pane"
        );
        assert!(stats.bottom() <= area.bottom(), "the statistics leave the pane");
    }

    #[test]
    fn the_statistics_take_the_height_of_their_content_and_no_more() {
        // A tall pane: the statistics end after their content, and each row
        // that is left goes to the metadata. The metadata holds the list of
        // columns, and that list has no end.
        let area = pane(57);
        let (meta, stats) = split_panels(area, 12);
        assert_eq!(stats.height, 14, "12 rows of content and 2 of border");
        assert_eq!(meta.height, 43);
        covers_the_pane(area, meta, stats);
    }

    #[test]
    fn a_short_pane_still_holds_the_two_panels() {
        let area = pane(BOTH_PANELS_MIN_HEIGHT);
        let (meta, stats) = split_panels(area, 12);
        covers_the_pane(area, meta, stats);
        assert!(meta.height >= 6, "the metadata needs room to say something");
        assert!(stats.height >= 6, "the statistics need room to say something");
    }

    #[test]
    fn a_pane_of_no_rows_makes_no_panel_outside_it() {
        for height in [0u16, 1] {
            let area = pane(height);
            let (meta, stats) = split_panels(area, 12);
            covers_the_pane(area, meta, stats);
            // One row cannot hold a border, so the statistics get nothing and
            // draw nothing.
            assert_eq!(stats.height, 0);
        }
    }

    #[test]
    fn content_taller_than_the_pane_cannot_squeeze_the_metadata_out() {
        // A column with many frequent values asks for more rows than the pane
        // has. The metadata keeps its smallest usable height, and the
        // statistics take each row that is left.
        let area = pane(30);
        let (meta, stats) = split_panels(area, 200);
        assert_eq!(meta.height, META_MIN_HEIGHT);
        assert_eq!(stats.height, 30 - META_MIN_HEIGHT);
        covers_the_pane(area, meta, stats);
    }

    #[test]
    fn a_tall_pane_gives_the_statistics_each_row_of_their_content() {
        // One half of the pane is the wrong limit in this direction. One half
        // of 57 rows is 28, so content of 30 rows lost 2 rows of frequent
        // values while the metadata above it held 27 empty rows.
        let area = pane(57);
        let (meta, stats) = split_panels(area, 30);
        assert_eq!(stats.height, 32, "30 rows of content and 2 of border");
        assert_eq!(meta.height, 25);
        covers_the_pane(area, meta, stats);
    }

    #[test]
    fn a_pane_that_holds_neither_minimum_gives_the_metadata_one_half() {
        // The two smallest heights together need more rows than this pane has.
        // Neither panel can have what it asks for, so the two share the pane
        // and neither one is empty.
        let area = pane(BOTH_PANELS_MIN_HEIGHT);
        let (meta, stats) = split_panels(area, 40);
        assert_eq!(meta.height, BOTH_PANELS_MIN_HEIGHT / 2);
        assert_eq!(stats.height, BOTH_PANELS_MIN_HEIGHT / 2);
        covers_the_pane(area, meta, stats);
    }

    #[test]
    fn the_statistics_keep_a_floor_while_the_engine_reads_them() {
        // With no answer yet the panel shows one row. The limit keeps the
        // panel near the height of the first numbers, so the border makes a
        // small step when the answer arrives.
        let area = pane(57);
        let (_, stats) = split_panels(area, 1);
        assert_eq!(stats.height, STATS_MIN_HEIGHT);
    }

    #[test]
    fn every_footer_hint_has_a_short_form() {
        // The footer has little space. A description with no short form
        // falls back to its first word, and that word is often the wrong
        // one: "build a filter ..." would show as "build".
        for cmd in overlays::FOOTER_HINTS {
            let b = commands::binding(*cmd).expect("hint with no entry in the table");
            let short = short_desc(b.desc);
            assert!(
                short.chars().count() <= 9,
                "{:?} gives the long form {short:?}",
                b.cmd
            );
        }
    }

}
