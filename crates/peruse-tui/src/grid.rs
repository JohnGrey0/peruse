//! The grid: the table of rows and columns.
//!
//! This module writes each character into the buffer of the frame. It does not
//! use the widget `Table` of ratatui, because the grid needs control of each
//! cell. The widget does not give these four things:
//!
//! * a scroll to the left and to the right, one column at a time
//! * a column of row numbers that always stays on the screen
//! * a color for each family of values
//! * a different color for the part of a cell that a search matches
//!
//! The cost of one frame is proportional to the number of cells on the screen.
//! The size of the file does not change that cost.

use peruse_core::model::{Align, CellKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::app::App;
use crate::paint::Paint;
use crate::text;

/// The number of screen columns between two columns of the grid.
const GAP: u16 = 1;

/// The columns that one frame of the grid draws.
pub struct Layout {
    /// The columns that the grid draws. Each entry holds the position of the
    /// column in the schema, its position on the screen, and its width.
    pub drawn: Vec<(usize, u16, u16)>,
    /// The width of the column of row numbers, in screen columns.
    pub gutter: u16,
}

/// Gives the width for the column of row numbers.
///
/// The width covers the digits of the largest row number that the grid can
/// show now.
fn gutter_width(app: &App) -> u16 {
    // Each addition here saturates. While the count is unknown, the cursor
    // can sit on a very large row number, and the number after it does not
    // fit in 64 bits.
    let highest = app
        .total
        .value()
        .unwrap_or_else(|| app.top_row.saturating_add(app.viewport_rows as u64))
        .max(app.cursor_row.saturating_add(1))
        .max(1);
    (highest.to_string().len() as u16 + 1).max(4)
}

/// Scrolls the grid to the left or to the right until the cursor column is on
/// the screen. The function then gives the columns that fit.
///
/// The header and the body both need this result, so Peruse calculates it one
/// time before it draws them.
pub fn layout(app: &mut App, area: Rect) -> Layout {
    let gutter = gutter_width(app);
    let avail = area.width.saturating_sub(gutter);
    let vis = app.visible_columns();
    if vis.is_empty() || avail == 0 {
        return Layout { drawn: Vec::new(), gutter };
    }

    let cursor_at = vis.iter().position(|c| *c == app.cursor_col).unwrap_or(0);
    app.left_col = app.left_col.min(vis.len() - 1);
    if cursor_at < app.left_col {
        app.left_col = cursor_at;
    }

    // Move the first column to the right until the cursor column fits.
    loop {
        let mut used = 0u16;
        let mut last = app.left_col;
        for (n, &ci) in vis.iter().enumerate().skip(app.left_col) {
            let w = app.widths[ci].min(avail.saturating_sub(1).max(1));
            if used + w > avail && n > app.left_col {
                break;
            }
            used += w + GAP;
            last = n;
        }
        if cursor_at <= last || app.left_col >= cursor_at {
            break;
        }
        app.left_col += 1;
    }

    let mut drawn = Vec::new();
    let mut x = gutter;
    for &ci in vis.iter().skip(app.left_col) {
        let w = app.widths[ci].min(avail.saturating_sub(1).max(1));
        if x + w > area.width && !drawn.is_empty() {
            break;
        }
        drawn.push((ci, x, w));
        x += w + GAP;
        if x >= area.width {
            break;
        }
    }
    Layout { drawn, gutter }
}

/// Draws the header and the rows of the grid.
pub fn draw(buf: &mut Buffer, area: Rect, app: &mut App, p: &Paint) {
    let t = app.theme.clone();
    let base = p.on(t.fg, t.bg);
    for y in area.top()..area.bottom() {
        buf.set_stringn(area.x, y, " ".repeat(area.width as usize), area.width as usize, base);
    }
    if area.height < 2 {
        return;
    }

    let lay = layout(app, area);
    let body = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height - 1,
    };
    app.viewport_rows = body.height as usize;

    draw_header(buf, area, app, p, &lay);

    if app.schema.is_empty() {
        buf.set_stringn(area.x + 2, body.y, "no columns", 40, p.fg(t.dim));
        return;
    }
    if app.is_empty() {
        let msg = if app.view.filter.is_some() {
            "no rows match this filter — F clears it"
        } else {
            "no rows"
        };
        buf.set_stringn(area.x + 2, body.y + 1, msg, area.width as usize, p.fg(t.dim));
        return;
    }

    let scrollbar = area.width > 40 && app.total.value().is_some_and(|n| n > body.height as u64);
    let sb_x = area.right().saturating_sub(1);
    let match_style = p.on(t.match_fg, t.match_bg);

    for row in 0..body.height {
        let y = body.y + row;
        let abs = app.top_row + row as u64;
        let is_cursor_row = abs == app.cursor_row;

        // A row after the last row of the view stays empty. It must not
        // show an old value from a previous view with more rows.
        let in_page = app.page.contains(abs);
        let past_end = app.total.value().is_some_and(|n| abs >= n);
        if past_end {
            continue;
        }

        let row_bg = if is_cursor_row { Some(t.cursor_row) } else { None };
        if let Some(bg) = row_bg {
            buf.set_stringn(
                area.x,
                y,
                " ".repeat(area.width as usize),
                area.width as usize,
                p.on(t.fg, bg),
            );
        }

        // Draw the row number at the right side of the gutter. The first
        // row of the view has the number 1.
        let num = text::fit(&(abs + 1).to_string(), lay.gutter.saturating_sub(1) as usize, Align::Right);
        let gutter_style = if is_cursor_row {
            p.bold(p.on(t.accent, row_bg.unwrap_or(t.gutter_bg)))
        } else {
            p.on(t.gutter_fg, t.gutter_bg)
        };
        buf.set_stringn(area.x, y, &num, num.len(), gutter_style);

        for &(ci, x, w) in &lay.drawn {
            let col = &app.schema.columns[ci];
            let is_cursor_cell = is_cursor_row && ci == app.cursor_col;
            let bg = if is_cursor_cell {
                t.cursor_cell
            } else if let Some(b) = row_bg {
                b
            } else if ci == app.cursor_col {
                t.sel_col
            } else {
                t.bg
            };

            if !in_page {
                // The row is not in the current page. A row of points
                // keeps the grid steady. Without it, the grid becomes
                // empty and full again during a fast scroll.
                let dots = text::fit("·", w as usize, Align::Left);
                buf.set_stringn(area.x + x, y, &dots, w as usize, p.on(t.dim, bg));
                continue;
            }

            let r = (abs - app.page.offset) as usize;
            let (raw, style) = match app.page.cell(r, ci) {
                None => ("NULL".to_string(), p.on(t.null, bg)),
                Some(v) => (text::sanitize(v), p.on(kind_color(&t, col.kind), bg)),
            };
            let style = if is_cursor_cell { p.bold(style) } else { style };
            let shown = text::fit(&raw, w as usize, col.kind.align());
            buf.set_stringn(area.x + x, y, &shown, w as usize, style);

            if !app.needle.is_empty() {
                highlight(buf, area.x + x, y, w, &shown, &app.needle, match_style);
            }
        }

        if scrollbar {
            let total = app.total.value().unwrap_or(1).max(1);
            let pos = (app.cursor_row.min(total - 1) * (body.height as u64 - 1)) / (total - 1).max(1);
            let ch = if row as u64 == pos { "█" } else { "│" };
            let st = if row as u64 == pos { p.fg(t.accent) } else { p.fg(t.border) };
            buf.set_stringn(sb_x, y, ch, 1, st);
        }
    }
}

/// Paints the part of a cell that the search matches.
///
/// The grid draws the cell first, and this function then paints the match. The
/// match keeps its own colors on each background color. A match on the row of
/// the cursor is therefore as clear as a match on any other row.
fn highlight(buf: &mut Buffer, x: u16, y: u16, w: u16, shown: &str, needle: &str, style: Style) {
    let Some((s, e)) = text::find_ci(shown, needle) else {
        return;
    };
    let before = text::width(&shown[..s]);
    let matched = &shown[s..e];
    if before >= w as usize {
        return;
    }
    buf.set_stringn(
        x + before as u16,
        y,
        matched,
        (w as usize).saturating_sub(before),
        style,
    );
}

/// Draws the row of column headers.
fn draw_header(buf: &mut Buffer, area: Rect, app: &App, p: &Paint, lay: &Layout) {
    let t = &app.theme;
    let y = area.y;
    let hdr = p.on(t.header_fg, t.header_bg);
    buf.set_stringn(area.x, y, " ".repeat(area.width as usize), area.width as usize, hdr);

    for &(ci, x, w) in &lay.drawn {
        let col = &app.schema.columns[ci];
        let sorted = app.view.sort.iter().find(|k| k.column == col.name);

        let mut label = String::new();
        if let Some(k) = sorted {
            label.push(k.dir.arrow());
        }
        label.push_str(&col.name);

        // Put the name of the column on the same side as the values. A
        // column of numbers is then one group at the right side. Without
        // this rule, the name is at the left and the digits are at the
        // right. The type character goes to the other side, but only when
        // the name does not fill the column.
        let badge = col.kind.badge();
        let align = col.kind.align();
        let text_w = w as usize;
        let shown = if text::width(&label) + 2 <= text_w {
            let pad = text_w - text::width(&label) - 1;
            match align {
                Align::Left => format!("{label}{}{badge}", " ".repeat(pad)),
                Align::Right => format!("{badge}{}{label}", " ".repeat(pad)),
            }
        } else {
            text::fit(&label, text_w, align)
        };

        let focused = ci == app.cursor_col;
        let style = if focused {
            p.bold(p.on(t.accent, t.header_bg))
        } else if sorted.is_some() {
            p.bold(hdr)
        } else {
            hdr
        };
        buf.set_stringn(area.x + x, y, &shown, text_w, style);
    }

    // Show a sign when more columns are at the left or at the right.
    let vis = app.visible_columns();
    if app.left_col > 0 {
        buf.set_stringn(area.x + lay.gutter.saturating_sub(1), y, "‹", 1, p.bold(p.on(t.warn, t.header_bg)));
    }
    let last_drawn = lay.drawn.last().map(|(ci, _, _)| *ci);
    if let Some(last) = last_drawn
        && vis.last() != Some(&last) {
            buf.set_stringn(
                area.right().saturating_sub(1),
                y,
                "›",
                1,
                p.bold(p.on(t.warn, t.header_bg)),
            );
        }
}

/// Gives the color of the theme for one family of values.
pub fn kind_color(t: &peruse_core::Theme, kind: CellKind) -> peruse_core::theme::Color {
    match kind {
        CellKind::Number => t.number,
        CellKind::Text => t.string,
        CellKind::Bool => t.boolean,
        CellKind::Temporal => t.temporal,
        CellKind::Binary => t.binary,
        CellKind::Nested => t.nested,
    }
}

