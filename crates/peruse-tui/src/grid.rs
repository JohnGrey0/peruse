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

use peruse_core::engine::ColumnBrief;
use peruse_core::model::{Align, CellKind, Column};
use peruse_core::source::human_count;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::app::{App, Band};
use crate::paint::Paint;
use crate::text;

/// The number of screen columns between two columns of the grid.
const GAP: u16 = 1;

/// The character that stands for a fact that has not arrived.
///
/// A blank cell would read as a zero, and a number from another view would be a
/// lie. The rows of data that are not in the page use the same character.
const WAITING: &str = "·";

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

/// Gives the number of rows of the detail band inside a grid of `height` rows.
///
/// The band gives its rows back to the data on a short terminal. Two rules
/// decide the number:
///
/// * One row goes to the column names, and one row must stay for the data. A
///   grid with room for one row of data therefore still shows that row.
/// * The data keeps one half of the rows at the minimum. The band is a summary
///   of the columns, and a summary must not become the main part of the screen.
///
/// The band writes its rows from the top, so a band that gives rows back keeps
/// the facts that come first. Refer to [`band_lines`].
pub fn band_rows(band: Band, height: u16) -> u16 {
    let free = height.saturating_sub(1);
    band.rows().min(free.saturating_sub(1)).min(free / 2)
}

/// Draws the header and the rows of the grid.
pub fn draw(buf: &mut Buffer, area: Rect, app: &mut App, p: &Paint) {
    let t = app.theme.clone();
    let base = p.on(t.fg, t.bg);
    // Build the row of spaces one time. One allocation for each row of the
    // screen, at each frame, is work that gives nothing.
    let blank = " ".repeat(area.width as usize);
    for y in area.top()..area.bottom() {
        buf.set_stringn(area.x, y, &blank, area.width as usize, base);
    }
    if area.height < 2 {
        // There is no row of the grid on the screen, so no click can land on
        // one. Without this, a click would use the positions of an older frame.
        // Clear each position, and not the counts alone. A click would otherwise
        // still find a column of the frame before this one.
        app.hit = crate::app::Hit::default();
        return;
    }

    let lay = layout(app, area);
    // The band takes its rows from the data. The row of the data at the bottom
    // of the page would otherwise be a row that Peruse asks for and never draws.
    let band = band_rows(app.band(), area.height);
    let body = Rect {
        x: area.x,
        y: area.y + 1 + band,
        width: area.width,
        height: area.height - 1 - band,
    };
    app.viewport_rows = body.height as usize;
    // Keep where the grid is on the screen. A click of the mouse arrives as a
    // row and a column of the terminal, and only the code that draws the grid
    // knows which cell is at that position.
    app.hit.header_y = area.y;
    app.hit.band = band;
    app.hit.top = body.y;
    app.hit.rows = body.height;
    // A panel at the side of the grid covers the same rows as the grid, so the
    // mouse needs the columns of the grid as well as its rows.
    app.hit.left = area.x;
    app.hit.width = area.width;
    app.hit.cols.clear();
    app.hit
        .cols
        .extend(lay.drawn.iter().map(|(ci, x, w)| (*ci, area.x + x, *w)));

    draw_header(buf, area, app, p, &lay);
    if band > 0 {
        draw_band(buf, area, app, p, &lay, band);
    }

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
        // The message keeps a blank row above it. On a short grid there is no
        // room for that row, and the message must stay inside the grid.
        let y = (body.y + 1).min(area.bottom().saturating_sub(1));
        buf.set_stringn(area.x + 2, y, msg, area.width as usize, p.fg(t.dim));
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
            buf.set_stringn(area.x, y, &blank, area.width as usize, p.on(t.fg, bg));
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
                let dots = waiting(w as usize);
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
        // right. The type character goes to the other side.
        let align = col.kind.align();
        let text_w = w as usize;

        let focused = ci == app.cursor_col;
        let style = if focused {
            p.bold(p.on(t.accent, t.header_bg))
        } else if sorted.is_some() {
            p.bold(hdr)
        } else {
            hdr
        };

        // The type character needs two spaces between it and the name.
        //
        // With one space, that character reads as the last letter of the
        // name. The character for a column of text is the letter `a`, and a
        // name that nearly fills the column then shows as `customer_name a`.
        // A user reads that as a spelling mistake, and not as the type.
        //
        // The character also keeps a dim color, and the name keeps the color
        // of the header. The two are therefore clearly two things.
        let mut utf8 = [0u8; 4];
        let badge = col.kind.badge().encode_utf8(&mut utf8);
        let room = text::width(&label) + 3 <= text_w;
        let name_w = text_w - if room { 2 } else { 0 };
        let badge_style = p.on(t.dim, t.header_bg);

        match align {
            Align::Left => {
                buf.set_stringn(area.x + x, y, text::fit(&label, name_w, align), name_w, style);
                if room {
                    buf.set_stringn(area.x + x + text_w as u16 - 1, y, badge, 1, badge_style);
                }
            }
            Align::Right => {
                if room {
                    buf.set_stringn(area.x + x, y, badge, 1, badge_style);
                }
                let off = (text_w - name_w) as u16;
                buf.set_stringn(area.x + x + off, y, text::fit(&label, name_w, align), name_w, style);
            }
        }
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

/// Draws the detail band: the rows of facts between the column names and the
/// first row of data.
///
/// The band takes the colors of the header, so the eye reads the names and the
/// facts as one block that describes the columns. The rows of data below it keep
/// the background of the grid.
///
/// The band of the column under the cursor uses [`band_focus`], and the name
/// above it keeps the accent color. Refer to that function for the three levels.
fn draw_band(buf: &mut Buffer, area: Rect, app: &App, p: &Paint, lay: &Layout, rows: u16) {
    let t = &app.theme;
    let base = p.on(t.dim, t.header_bg);
    let blank = " ".repeat(area.width as usize);
    for r in 0..rows {
        buf.set_stringn(area.x, area.y + 1 + r, &blank, area.width as usize, base);
    }

    let band = app.band();
    // One list of rows, for every column of the frame. The band draws up to four
    // rows for each column that is on the screen, and a new list for each of
    // them would allocate at each frame for work that the frame before did.
    let mut lines: Vec<String> = Vec::with_capacity(Band::DETAIL_ROWS as usize);
    for &(ci, x, w) in &lay.drawn {
        let col = &app.schema.columns[ci];
        let style = if ci == app.cursor_col {
            // The column under the cursor stands out, but not as much as its
            // name in the header above it.
            p.on(band_focus(t), t.header_bg)
        } else {
            base
        };
        band_lines(col, app.brief(ci), band, w as usize, &mut lines);
        for (r, line) in lines.iter().take(rows as usize).enumerate() {
            buf.set_stringn(area.x + x, area.y + 1 + r as u16, line, w as usize, style);
        }
    }
}

/// The part of the dim color in the band of the column under the cursor, as a
/// percentage.
///
/// One half puts the mix as far from the accent color as it is from the dim
/// color. The three levels of the header are therefore as clear as the palette
/// permits. The theme with the least room is nord, where the accent color is
/// near the dim color already. Each of the two steps of the mix is then 16 of
/// the 255 levels of a channel, and no other percentage gives that theme more.
///
/// The mix is the first of two steps. Refer to [`BAND_FOCUS_CHROMA`].
const BAND_FOCUS_MIX: u32 = 50;

/// How much color the middle level keeps, as a percentage of the color of the
/// mix.
///
/// A terminal with 256 colors puts each color in a cube of 6 by 6 by 6 colors.
/// The steps of that cube are as large as 95 of the 255 levels of a channel, so
/// the mix can land in the same cell of the cube as the accent color or as the
/// dim color. One of the three levels then disappears, and only on that
/// terminal. The themes nord, gruvbox-dark and everforest-dark did that.
///
/// A mix of other parts cannot help those themes: the two colors are so near
/// each other that the full line between them lies in two cells. Peruse
/// therefore takes color out of the middle level, one step at a time, until the
/// three levels are three cells.
///
/// The steps go down and never up. A color that goes up hits the end of a
/// channel and loses brightness there, and the brightness must not change: the
/// order of the three levels comes from it.
const BAND_FOCUS_CHROMA: [u32; 6] = [100, 80, 60, 40, 20, 0];

/// Gives a color with less color and the same brightness.
///
/// `pct` is the part of the color of `c` that stays. The value 100 gives `c`,
/// and the value 0 gives the gray that has the brightness of `c`.
fn less_color(c: peruse_core::theme::Color, pct: u32) -> peruse_core::theme::Color {
    // The brightness of a gray is the level of its three channels, and the
    // brightness of a mix is the mix of the two brightnesses. A step toward
    // this gray therefore keeps the brightness of `c`.
    let level = c.luma().min(255) as u8;
    let gray = peruse_core::theme::Color { r: level, g: level, b: level };
    gray.mix(c, pct)
}

/// Gives the color of the band under the column that the cursor is on.
///
/// The header shows three levels, and the user must see which of them is the
/// name of the column:
///
/// 1. the name of that column, in the accent color and in thick letters
/// 2. the facts of that column, in this color and in thin letters
/// 3. the facts of each other column, in the dim color
///
/// This color is a mix of the accent color and the dim color, so it stands
/// between the two. It is never one of them, and the eye reads the name as the
/// stronger of the two rows.
///
/// The mix then gives color away until the three levels are three colors of the
/// xterm-256 set. Refer to [`BAND_FOCUS_CHROMA`]. One color goes to a terminal
/// with 24-bit color and to a terminal with 256 colors, so the two show the same
/// three levels.
pub fn band_focus(t: &peruse_core::Theme) -> peruse_core::theme::Color {
    let mid = t.accent.mix(t.dim, BAND_FOCUS_MIX);
    let (accent, dim) = (crate::colors::to_256(t.accent), crate::colors::to_256(t.dim));
    for pct in BAND_FOCUS_CHROMA {
        let c = less_color(mid, pct);
        let i = crate::colors::to_256(c);
        if i != accent && i != dim {
            return c;
        }
    }
    // A theme from a file can put the accent color and the dim color in one
    // cell of the cube. No middle color can then stand between the two on a
    // terminal with 256 colors, and the mix is still the best of the three.
    mid
}

/// Builds the rows of the detail band for one column, each one fitted to the
/// width of the column.
///
/// Every column gives the same number of rows, and the same fact on the same
/// row, so the facts line up across the grid. Refer to [`Band::DETAIL_ROWS`] for
/// the four facts and for the reason that they are those four.
///
/// A column with no facts gives a row of points. A blank row would read as a
/// zero, and a number from another view would be a lie.
///
/// The rows go into `out`, which the caller reuses for each column of the frame.
/// The function clears it first.
fn band_lines(
    col: &Column,
    brief: Option<&ColumnBrief>,
    band: Band,
    w: usize,
    out: &mut Vec<String>,
) {
    out.clear();
    let Some(b) = brief else {
        for _ in 0..band.rows() {
            out.push(waiting(w));
        }
        return;
    };
    match band {
        Band::Off => {}
        Band::Compact => out.push(compact_line(col, b, w)),
        Band::Detailed => {
            out.push(text::fit(&col.short_type(), w, Align::Left));
            out.push(text::fit(&with_word(&null_share(b), "null", w), w, Align::Left));
            out.push(text::fit(&distinct_line(b, w), w, Align::Left));
            out.push(text::fit(&range_line(col, b, w), w, Align::Left));
        }
    }
}

/// Builds a row for a fact that has not arrived.
fn waiting(w: usize) -> String {
    text::fit(WAITING, w, Align::Left)
}

/// Builds the one row of the compact band: the type and the share of NULL
/// values.
fn compact_line(col: &Column, b: &ColumnBrief, w: usize) -> String {
    let share = null_share(b);
    let ty = col.short_type();
    // The type goes at the left and the share at the right, as the header puts
    // the name at the left and the character of the family at the right. The
    // shares then stand in one line down the grid, and the eye finds the column
    // with the missing values.
    let gap = w
        .saturating_sub(text::width(&ty))
        .saturating_sub(text::width(&share));
    if gap >= 1 {
        return format!("{ty}{}{share}", " ".repeat(gap));
    }
    // A narrow column keeps the share. The header shows the family of the type
    // already, with its own character.
    text::fit(&share, w, Align::Right)
}

/// Gives the share of NULL values of a column, as a percentage.
///
/// A share that is not zero never shows as `0%`. A column with one missing value
/// in a million rows must not look complete. The same rule holds at the other
/// end: a share below 100 never shows as `100%`, so a column with one value in a
/// million rows does not look empty.
fn null_share(b: &ColumnBrief) -> String {
    let pct = b.null_pct();
    if pct > 0.0 && pct < 1.0 {
        "<1%".to_string()
    } else if pct > 99.0 && pct < 100.0 {
        ">99%".to_string()
    } else {
        format!("{pct:.0}%")
    }
}

/// Adds a word to a value when the column is wide enough for it.
///
/// A narrow column keeps the value and drops the word. A value with a cut word,
/// such as a share and the first letter of `null`, says less than the value by
/// itself.
fn with_word(value: &str, word: &str, w: usize) -> String {
    let long = format!("{value} {word}");
    if text::width(&long) <= w {
        long
    } else {
        value.to_string()
    }
}

/// Builds the row with the count of the different values.
///
/// The character `~` says that the count is an estimate. Refer to
/// [`peruse_core::query::View::band_sql`].
fn distinct_line(b: &ColumnBrief, w: usize) -> String {
    match b.n_distinct {
        Some(n) => with_word(&format!("~{}", human_count(n)), "distinct", w),
        None => waiting(w),
    }
}

/// Builds the row with the smallest value and the largest value.
///
/// The row divides the width of the column between the two ends. A long value at
/// one end can therefore not push the other end off the column.
fn range_line(col: &Column, b: &ColumnBrief, w: usize) -> String {
    // A structure, a list and a BLOB have no order, so they have no range.
    if matches!(col.kind, CellKind::Nested | CellKind::Binary) {
        return "no order".to_string();
    }
    let (Some(lo), Some(hi)) = (&b.min, &b.max) else {
        // The engine gives no value for a column where each value is NULL.
        return if b.n_present == 0 {
            "all null".to_string()
        } else {
            waiting(w)
        };
    };
    let lo = text::sanitize(lo);
    let hi = text::sanitize(hi);
    if lo == hi {
        // One value only. An arrow between two copies of it says nothing.
        return lo;
    }
    // Three screen columns go to the arrow and to the space on each side of it.
    let room = w.saturating_sub(3);
    if room < 2 {
        return format!("{lo}→{hi}");
    }
    // Each end asks for one half of the room and no more than it needs. An end
    // that needs less gives the remainder to the other end. A range from `1` to
    // `4000.75` therefore keeps the small number complete, and it does not cut
    // both ends where only one end is long.
    //
    // Each end keeps one screen column at the minimum, so the row always shows
    // that the column has two ends. The smallest value can be the empty text,
    // and its width is then zero: a column of text whose smallest value is `""`
    // would draw `-> US` with nothing at the left of the arrow.
    let half = (room / 2).max(1);
    let left = text::width(&lo).max(1).min(half);
    let right = (room - left).min(text::width(&hi).max(1));
    format!(
        "{} → {}",
        text::truncate(&lo, room - right),
        text::truncate(&hi, right)
    )
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

#[cfg(test)]
mod tests {

    /// Gives the rows of the band for one column, as a list of its own.
    ///
    /// The function `band_lines` writes into a list that the caller reuses for
    /// every column of the frame. A test wants one list for one column.
    fn lines_of(col: &Column, brief: Option<&ColumnBrief>, band: Band, w: usize) -> Vec<String> {
        let mut out = Vec::new();
        band_lines(col, brief, band, w, &mut out);
        out
    }
    use super::*;

    fn brief(total: u64, present: u64, distinct: u64, lo: &str, hi: &str) -> ColumnBrief {
        ColumnBrief {
            column: "c".into(),
            n_total: total,
            n_present: present,
            n_distinct: Some(distinct),
            min: Some(lo.into()),
            max: Some(hi.into()),
        }
    }

    #[test]
    fn the_band_takes_the_rows_of_its_mode_when_the_grid_is_tall() {
        // A grid of 20 rows: one row of names, the band, and the rest for data.
        assert_eq!(band_rows(Band::Off, 20), 0);
        assert_eq!(band_rows(Band::Compact, 20), 1);
        assert_eq!(band_rows(Band::Detailed, 20), Band::DETAIL_ROWS);
    }

    #[test]
    fn a_short_grid_gives_the_rows_of_the_band_back_to_the_data() {
        // One row must always stay for the data, and the data keeps one half of
        // the rows. A grid with room for one row of data still draws that row.
        for (height, want) in [(0u16, 0u16), (1, 0), (2, 0), (3, 1), (5, 2), (8, 3), (10, 4)] {
            let rows = band_rows(Band::Detailed, height);
            assert_eq!(rows, want, "a grid of {height} rows");
            if height >= 2 {
                assert!(
                    height - 1 - rows >= 1,
                    "a grid of {height} rows kept no row of data"
                );
            }
        }
        // The compact band needs one row, and it gives that row back as well.
        assert_eq!(band_rows(Band::Compact, 2), 0);
        assert_eq!(band_rows(Band::Compact, 3), 1);
    }

    #[test]
    fn the_compact_band_puts_the_type_at_the_left_and_the_share_at_the_right() {
        let col = Column::new("id", "BIGINT", false);
        let lines = lines_of(&col, Some(&brief(100, 100, 100, "1", "100")), Band::Compact, 12);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "BIGINT    0%");
        // A narrow column keeps the share. The header shows the family already.
        let narrow = lines_of(&col, Some(&brief(100, 80, 80, "1", "100")), Band::Compact, 5);
        assert_eq!(narrow[0], "  20%");
    }

    #[test]
    fn a_share_that_is_not_zero_never_shows_as_zero() {
        // One missing value in a million rows must not look complete.
        let b = brief(1_000_000, 999_999, 999_999, "1", "2");
        assert_eq!(null_share(&b), "<1%");
        // One value in a million rows must not look empty. The row of the range
        // shows that value, so a share of 100% is a contradiction on the screen.
        assert_eq!(null_share(&brief(1_000_000, 1, 1, "1", "1")), ">99%");
        // A column of NULL values keeps the full share.
        assert_eq!(null_share(&brief(1_000_000, 0, 0, "1", "2")), "100%");
        assert_eq!(null_share(&brief(10, 10, 10, "1", "2")), "0%");
        assert_eq!(null_share(&brief(10, 5, 5, "1", "2")), "50%");
        // A view with no row divides by nothing.
        assert_eq!(null_share(&ColumnBrief::default()), "0%");
    }

    #[test]
    fn the_detailed_band_gives_four_rows_that_line_up() {
        let col = Column::new("name", "VARCHAR", true);
        let lines = lines_of(&col, Some(&brief(1000, 900, 42, "alice", "zoe")), Band::Detailed, 16);
        assert_eq!(lines.len() as u16, Band::DETAIL_ROWS);
        assert_eq!(lines[0], "VARCHAR         ");
        assert_eq!(lines[1], "10% null        ");
        assert_eq!(lines[2], "~42 distinct    ");
        assert_eq!(lines[3], "alice → zoe     ");
        // Every row has the width of the column, so the columns line up.
        assert!(lines.iter().all(|l| text::width(l) == 16), "{lines:?}");
    }

    #[test]
    fn a_narrow_column_keeps_the_number_and_drops_the_word() {
        // A number with a cut word, such as the count and the first letter of
        // `distinct`, says less than the number by itself.
        let col = Column::new("n", "INTEGER", true);
        let lines = lines_of(&col, Some(&brief(1000, 900, 42, "1", "999")), Band::Detailed, 6);
        assert_eq!(lines[1], "10%   ");
        assert_eq!(lines[2], "~42   ");
        // The short end of the range gives its room to the long end.
        assert_eq!(lines[3], "1 → 9…");
    }

    #[test]
    fn a_column_with_no_facts_shows_points_and_never_a_number() {
        // A blank row would read as a zero, and a number from another view would
        // be a lie.
        let col = Column::new("id", "BIGINT", false);
        for band in [Band::Compact, Band::Detailed] {
            let lines = lines_of(&col, None, band, 8);
            assert_eq!(lines.len() as u16, band.rows());
            assert!(lines.iter().all(|l| l == "·       "), "{lines:?}");
        }
    }

    #[test]
    fn a_column_from_the_footer_has_no_count_of_the_different_values() {
        // The footer of a Parquet file gives the NULL share and no more. The row
        // of the count therefore shows points, and not a zero.
        let col = Column::new("id", "BIGINT", false);
        let footer = ColumnBrief {
            column: "id".into(),
            n_total: 1000,
            n_present: 750,
            n_distinct: None,
            min: None,
            max: None,
        };
        let lines = lines_of(&col, Some(&footer), Band::Detailed, 8);
        assert_eq!(lines[1], "25% null");
        assert_eq!(lines[2], "·       ", "no count arrived");
        assert_eq!(lines[3], "·       ", "no range arrived");
        // The compact band needs nothing more than the share.
        assert_eq!(lines_of(&col, Some(&footer), Band::Compact, 12)[0], "BIGINT   25%");
    }

    #[test]
    fn the_range_says_what_it_can_for_each_family_of_values() {
        let all_null = ColumnBrief {
            column: "c".into(),
            n_total: 10,
            n_present: 0,
            n_distinct: Some(0),
            min: None,
            max: None,
        };
        let text = Column::new("c", "VARCHAR", true);
        assert_eq!(range_line(&text, &all_null, 12), "all null");
        // A structure, a list and a BLOB have no order.
        for ty in ["STRUCT(a INTEGER)", "BLOB", "INTEGER[]"] {
            let col = Column::new("c", ty, true);
            assert_eq!(range_line(&col, &brief(10, 10, 10, "x", "y"), 12), "no order", "{ty}");
        }
        // One value only. An arrow between two copies of it says nothing.
        assert_eq!(range_line(&text, &brief(10, 10, 1, "EU", "EU"), 12), "EU");
    }

    #[test]
    fn the_range_gives_the_room_to_the_end_that_needs_it() {
        // The short end keeps its whole value, and the long end takes the room
        // that is left.
        let num = Column::new("c", "DOUBLE", true);
        assert_eq!(
            range_line(&num, &brief(5, 4, 4, "7.0", "4000.75"), 11),
            "7.0 → 4000…"
        );
        // The same rule with the long end at the left. The row still shows that
        // the column has two ends.
        let text = Column::new("c", "VARCHAR", true);
        assert_eq!(range_line(&text, &brief(5, 5, 3, "APAC", "US"), 6), "… → US");
    }

    #[test]
    fn every_theme_gives_three_levels_of_color_in_the_header() {
        // The name of the column under the cursor, the facts of that column, and
        // the facts of each other column must read as three levels. Each theme
        // derives its colors from a short Base, so the three must hold apart in
        // every one of them, and not in the default theme alone.
        let step = |a: peruse_core::theme::Color, b: peruse_core::theme::Color| {
            let d = |x: u8, y: u8| x.abs_diff(y) as u32;
            d(a.r, b.r).max(d(a.g, b.g)).max(d(a.b, b.b))
        };
        for name in peruse_core::theme::builtin_names() {
            let t = peruse_core::theme::builtin(name).unwrap();
            let mid = band_focus(&t);
            // The smallest step of the built-in themes is 17, from nord and
            // from rose-pine-dawn, whose accent colors are near their dim
            // colors already. The limit below is 12 and not 17, so that a new
            // theme with a slightly tighter palette can come in. A step below
            // 12 is not visible on a terminal.
            assert!(step(t.accent, mid) >= 12, "{name}: the band is too near the name");
            assert!(step(mid, t.dim) >= 12, "{name}: the band is too near the other bands");
            // The middle level stands between the two others. A dark theme makes
            // it brighter than the other bands, and a light theme makes it
            // darker, because a light background needs the other direction.
            let (a, m, d) = (t.accent.luma(), mid.luma(), t.dim.luma());
            assert!(
                (a > m && m > d) || (a < m && m < d),
                "{name}: the three levels are not in order: {a} {m} {d}"
            );

            // The three colors above are the colors of the theme, and a
            // terminal does not always get them. A terminal with 256 colors
            // gets the nearest color of a cube of 6 by 6 by 6 colors, and the
            // steps of that cube are as large as 95 of the 255 levels of a
            // channel. Two colors that stand apart here can therefore become
            // one color there, and the user of that terminal then sees two
            // levels and not three. The themes nord, gruvbox-dark and
            // everforest-dark did that while this test looked at the colors of
            // the theme only.
            for depth in [crate::colors::Depth::True, crate::colors::Depth::Indexed256] {
                let (a, m, d) = (
                    crate::colors::conv(t.accent, depth),
                    crate::colors::conv(mid, depth),
                    crate::colors::conv(t.dim, depth),
                );
                assert_ne!(a, m, "{name} on {depth:?}: the band takes the color of the name");
                assert_ne!(m, d, "{name} on {depth:?}: the band takes the color of the other bands");
                assert_ne!(a, d, "{name} on {depth:?}: the name takes the color of the other bands");
            }
        }
    }

    #[test]
    fn the_middle_level_keeps_the_brightness_when_it_gives_color_away() {
        // The order of the three levels comes from the brightness, so a step
        // that takes color out of the middle level must not change it. The
        // steps of `BAND_FOCUS_CHROMA` therefore go toward the gray that has
        // the brightness of the color, and never past it.
        for hex in [0x81a1c1u32, 0x928374, 0x7fbbb3, 0xffffff, 0x000000, 0x101020] {
            let c = peruse_core::theme::rgb(hex);
            for pct in BAND_FOCUS_CHROMA {
                let out = less_color(c, pct);
                assert!(
                    out.luma().abs_diff(c.luma()) <= 1,
                    "{hex:#08x} at {pct}%: {} is not the brightness of {}",
                    out.luma(),
                    c.luma()
                );
            }
            assert_eq!(less_color(c, 100), c, "{hex:#08x}: 100% is the color itself");
            let g = less_color(c, 0);
            assert_eq!((g.r, g.g), (g.b, g.b), "{hex:#08x}: 0% is not a gray");
        }
    }

    #[test]
    fn the_room_after_a_name_covers_the_largest_share_of_null_values() {
        // This test holds the value of `app::NAME_HEADROOM` at 5.
        //
        // The compact band writes the type at the left and the share of NULL
        // values at the right, with one blank column between the two. The widest
        // share is `100%`, which is four characters. A column whose type is as
        // wide as its name therefore needs 1 + 4 screen columns after the name,
        // and less than that drops the type.
        //
        // A share of 20% needs one column less, so a band over a column with
        // some values only does not hold this number. The column below has no
        // value at all.
        let col = Column::new("VARCHAR", "VARCHAR", true);
        let w = text::width(&col.name) + crate::app::NAME_HEADROOM;
        let line = compact_line(&col, &brief(1000, 0, 0, "", ""), w);
        assert!(line.starts_with("VARCHAR"), "the band lost the type: {line:?}");
        assert!(line.ends_with("100%"), "the band lost the share: {line:?}");
        assert_eq!(text::width(&line), w, "the row is not the width of the column");
    }

    #[test]
    fn a_value_of_more_than_one_line_stays_on_one_row() {
        // A row of the band is one row. A line end inside a value must not push
        // the grid down.
        let col = Column::new("c", "VARCHAR", true);
        let line = range_line(&col, &brief(10, 10, 2, "a\nb", "z"), 20);
        assert!(!line.contains('\n'), "got {line:?}");
    }
}

