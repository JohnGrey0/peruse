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
    let mut overlay_cursor = None;
    match app.mode {
        Mode::Help => overlays::draw_help(f.buffer_mut(), area, app, &p),
        Mode::Palette => overlays::draw_palette(f.buffer_mut(), area, app, &p),
        Mode::ThemePicker => overlays::draw_theme_picker(f.buffer_mut(), area, app, &p),
        Mode::Cell => overlays::draw_cell(f.buffer_mut(), area, app, &p),
        Mode::Record => overlay_cursor = overlays::draw_record(f.buffer_mut(), area, app, &p),
        Mode::FilterBuild => {
            overlay_cursor = overlays::draw_filter_build(f.buffer_mut(), area, app, &p)
        }
        Mode::Settings => overlay_cursor = overlays::draw_settings(f.buffer_mut(), area, app, &p),
        _ => {}
    }

    if let Some(pos) = overlay_cursor {
        f.set_cursor_position(pos);
        return;
    }

    // The palette holds its own prompt inside the overlay.
    if app.mode == Mode::Palette {
        // Multiply with 32 bits. A terminal of more than 936 columns makes
        // `width * 70` too large for 16 bits. These two calculations must give
        // the same result as `overlays::centered` with the same percentages.
        let pct = |v: u16, p: u16| -> u16 { ((v as u32 * p as u32) / 100) as u16 };
        let outer_x = area.x + (area.width - pct(area.width, 70).min(76)) / 2;
        f.set_cursor_position(Position::new(
            outer_x + 4 + app.input.cursor_col() as u16,
            area.y + (area.height - pct(area.height, 70)) / 2 + 1,
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
/// The rows that the statistics take in the stacked view.
///
/// The statistics of a column have an end: some rows of numbers, a chart of
/// one row, and the most frequent values. The metadata holds the list of
/// columns, which has no end, so the metadata takes the room that is left.
const STATS_HEIGHT: u16 = 18;

/// Draws the panel that is open, or the two panels one above the other.
fn draw_panel(f: &mut Frame, area: Rect, app: &App, p: &Paint) {
    match app.panel {
        Panel::Meta => panels::draw_meta(f.buffer_mut(), area, app, p, false),
        Panel::Stats => panels::draw_stats(f.buffer_mut(), area, app, p),
        Panel::Both => {
            if area.height < BOTH_PANELS_MIN_HEIGHT {
                // Say why the second panel is not there. A panel that goes
                // away with no word looks like a fault.
                panels::draw_stats(f.buffer_mut(), area, app, p);
                panels::note(f.buffer_mut(), area, app, p, " screen too short for both ");
                return;
            }
            // The metadata goes on top, and the statistics below it. The
            // order never changes, so the eye finds each of them in the same
            // place.
            let stats_h = STATS_HEIGHT.min(area.height / 2).max(8);
            let parts = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(6), Constraint::Length(stats_h)])
                .split(area);
            panels::draw_meta(f.buffer_mut(), parts[0], app, p, true);
            panels::draw_stats(f.buffer_mut(), parts[1], app, p);
        }
        Panel::None => {}
    }
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

    #[test]
    fn every_footer_hint_has_a_short_form() {
        // The footer has little space. A description with no short form
        // falls back to its first word, and that word is often the wrong
        // one: "build a filter …" would show as "build".
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
