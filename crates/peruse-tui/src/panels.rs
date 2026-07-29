//! The two panels at the side of the grid: the metadata panel that the key `m`
//! opens, and the column statistics panel that the key `i` opens.

use peruse_core::model::Align;
use peruse_core::source::human_count;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Widget};

use crate::app::App;
use crate::paint::Paint;
use crate::text;

/// Draws the border and the title of a panel. Gives the area inside the border.
fn frame(buf: &mut Buffer, area: Rect, title: &str, app: &App, p: &Paint) -> Rect {
    let t = &app.theme;
    Block::bordered()
        .title(format!(" {title} "))
        .border_style(p.on(t.border, t.bg))
        .title_style(p.bold(p.on(t.accent, t.bg)))
        .style(p.on(t.fg, t.bg))
        .render(area, buf);
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

/// Draws one row of a label and a value. The label always has the same width,
/// so each value starts at the same position.
fn kv(buf: &mut Buffer, area: Rect, y: u16, label: &str, value: &str, p: &Paint, app: &App) {
    if y >= area.bottom() {
        return;
    }
    let t = &app.theme;
    let lw = 15usize.min(area.width as usize / 2);
    buf.set_stringn(
        area.x,
        y,
        text::fit(label, lw, Align::Left),
        lw,
        p.on(t.dim, t.bg),
    );
    let vw = (area.width as usize).saturating_sub(lw + 1);
    buf.set_stringn(
        area.x + lw as u16 + 1,
        y,
        text::truncate(value, vw),
        vw,
        p.on(t.fg, t.bg),
    );
}

/// Draws the title of a section of a panel.
fn section(buf: &mut Buffer, area: Rect, y: u16, title: &str, p: &Paint, app: &App) {
    if y >= area.bottom() {
        return;
    }
    let t = &app.theme;
    buf.set_stringn(
        area.x,
        y,
        text::fit(title, area.width as usize, Align::Left),
        area.width as usize,
        p.bold(p.on(t.accent, t.bg)),
    );
}

/// Writes a short text along the bottom edge of a panel.
pub fn note(buf: &mut Buffer, area: Rect, app: &App, p: &Paint, text: &str) {
    if area.height < 2 || area.width < 6 {
        return;
    }
    let t = &app.theme;
    let w = area.width.saturating_sub(4) as usize;
    buf.set_stringn(
        area.x + 2,
        area.bottom().saturating_sub(1),
        crate::text::truncate(text, w),
        w,
        p.on(t.warn, t.bg),
    );
}

/// One line of the list of columns in the metadata panel.
struct ColRow {
    /// The level: 0 for a column of the file, 1 for a field of a structure.
    depth: usize,
    /// The name that the line shows.
    name: String,
    /// The name that the footer of a Parquet file uses for this value. For a
    /// field of a structure, that name is the path, such as `actor.login`.
    path: String,
    /// The family of values, for the color.
    kind: peruse_core::CellKind,
    /// The short form of the type.
    short_type: String,
}

/// Builds the list of columns that the metadata panel draws, and gives the
/// position of the column under the cursor in that list.
///
/// The column under the cursor also shows its fields, when it holds a
/// structure. A file of JSON events holds a column `actor` with five fields
/// inside it, and the panel would say nothing about them.
///
/// Only the column under the cursor opens. A file can hold hundreds of
/// columns, and a list that opened each of them would be too long to read.
/// The user looks at one column at a time.
fn column_rows(app: &App) -> (Vec<ColRow>, usize) {
    let mut rows = Vec::with_capacity(app.schema.len() + 8);
    let mut at = 0;
    for (i, col) in app.schema.columns.iter().enumerate() {
        if i == app.cursor_col {
            at = rows.len();
        }
        rows.push(ColRow {
            depth: 0,
            name: col.name.clone(),
            path: col.name.clone(),
            kind: col.kind,
            short_type: col.short_type(),
        });
        if i == app.cursor_col {
            for f in peruse_core::model::struct_fields(&col.sql_type) {
                rows.push(ColRow {
                    depth: 1,
                    name: f.name.clone(),
                    // A Parquet footer names a leaf by its path.
                    path: format!("{}.{}", col.name, f.name),
                    kind: f.kind,
                    short_type: f.short_type(),
                });
            }
        }
    }
    (rows, at)
}

/// Draws the metadata panel.
///
/// With `compact`, the panel gives its room to the list of columns: it writes
/// fewer rows of the summary and no read expression. The stacked view uses
/// that form, because the statistics take the lower half of the side pane.
pub fn draw_meta(buf: &mut Buffer, area: Rect, app: &App, p: &Paint, compact: bool) {
    let inner = frame(buf, area, "metadata", app, p);
    let t = &app.theme;
    let mut y = inner.y;

    let Some(meta) = &app.meta else {
        buf.set_stringn(inner.x, y, "reading…", inner.width as usize, p.on(t.dim, t.bg));
        return;
    };

    // A short panel shows the first rows of the summary only. The list of
    // columns is the part that the user reads at each move of the cursor.
    let summary = meta.summary_rows();
    let take = if compact { 4.min(summary.len()) } else { summary.len() };
    for (k, v) in summary.into_iter().take(take) {
        kv(buf, inner, y, &k, &v, p, app);
        y += 1;
    }

    y += 1;
    section(buf, inner, y, "columns", p, app);
    y += 1;

    // The list follows the cursor of the grid. The panel therefore needs no
    // keys of its own: the keys h and l scroll the panel and the grid.
    //
    // The read expression takes the last rows, so the list stops above it.
    let reserve = if compact { 0 } else { 2 };
    let bottom = inner.bottom().saturating_sub(reserve);
    let (rows, at) = column_rows(app);
    let rows_left = bottom.saturating_sub(y) as usize;
    let n = rows.len();
    let window = rows_left.min(n);
    let start = at.saturating_sub(window / 2).min(n.saturating_sub(window));

    let num_rows = meta.parquet.as_ref().map(|pq| pq.num_rows).unwrap_or(0);
    for row in rows.iter().skip(start).take(window) {
        if y >= bottom {
            break;
        }
        let focused = row.depth == 0 && row.path == rows[at].path;
        let bg = if focused { t.cursor_row } else { t.bg };
        buf.set_stringn(
            inner.x,
            y,
            " ".repeat(inner.width as usize),
            inner.width as usize,
            p.bg(bg),
        );

        let name_w = (inner.width as usize).saturating_sub(20).max(6);
        let style = crate::grid::kind_color(t, row.kind);
        let name_style = if focused {
            p.bold(p.on(style, bg))
        } else {
            p.on(style, bg)
        };
        // A field of a structure moves to the right, so the eye sees that it
        // belongs to the column above it.
        let label = format!("{}{}", "  ".repeat(row.depth), row.name);
        buf.set_stringn(
            inner.x,
            y,
            text::fit(&label, name_w, Align::Left),
            name_w,
            name_style,
        );

        // For a Parquet file, the NULL count from the footer is exact and
        // costs almost no time. The footer names a field of a structure by
        // its path, so a field finds its own count.
        //
        // For a CSV file and a JSON file, the footer holds nothing. The panel
        // shows the type instead, and it does not show an estimate.
        let nulls = meta
            .columns
            .iter()
            .find(|c| c.name == row.path)
            .and_then(|c| c.null_count)
            .filter(|_| num_rows > 0)
            .map(|nulls| format!("{:.0}% null", nulls as f64 * 100.0 / num_rows as f64));

        let right = nulls.unwrap_or_else(|| row.short_type.clone());
        let rw = (inner.width as usize).saturating_sub(name_w + 1);
        buf.set_stringn(
            inner.x + name_w as u16 + 1,
            y,
            text::fit(&right, rw, Align::Right),
            rw,
            p.on(t.dim, bg),
        );
        y += 1;
    }

    // Say how many columns are outside the window. A file with 400 columns
    // shows about fifteen of them, and the user must know that.
    if window < n && y < bottom {
        let above = start;
        let below = n - (start + window);
        let more = match (above, below) {
            (0, b) => format!("… {b} more below"),
            (a, 0) => format!("… {a} more above"),
            (a, b) => format!("… {a} above, {b} below"),
        };
        buf.set_stringn(inner.x, y, &more, inner.width as usize, p.on(t.dim, t.bg));
        y += 1;
    }

    // The call that reads the same rows outside Peruse. The user can look at
    // the data and then write a script with these options. The user does not
    // need to find the options of the sniffer again.
    if !compact {
        y += 1;
        if y + 1 < inner.bottom() {
            section(buf, inner, y, "reads as", p, app);
            y += 1;
            for line in text::wrap(&app.read_expr, inner.width as usize) {
                if y >= inner.bottom() {
                    break;
                }
                buf.set_stringn(inner.x, y, &line, inner.width as usize, p.on(t.lit, t.bg));
                y += 1;
            }
        }
    }
}

/// Draws the column statistics panel.
pub fn draw_stats(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) {
    let title = app
        .schema
        .columns
        .get(app.cursor_col)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "column".into());
    let inner = frame(buf, area, &title, app, p);
    let t = &app.theme;
    let mut y = inner.y;

    let Some(s) = app.stats() else {
        buf.set_stringn(inner.x, y, "computing…", inner.width as usize, p.on(t.dim, t.bg));
        return;
    };

    for (k, v) in s.rows() {
        kv(buf, inner, y, &k, &v, p, app);
        y += 1;
    }

    if let Some(h) = &s.histogram {
        y += 1;
        section(buf, inner, y, "distribution", p, app);
        y += 1;
        if y < inner.bottom() {
            let spark = h.sparkline();
            buf.set_stringn(
                inner.x,
                y,
                text::truncate(&spark, inner.width as usize),
                inner.width as usize,
                p.on(t.accent, t.bg),
            );
            y += 1;
        }
        if y < inner.bottom() {
            let lo = format!("{:.6}", h.lo);
            let hi = format!("{:.6}", h.hi);
            let lo = lo.trim_end_matches('0').trim_end_matches('.').to_string();
            let hi = hi.trim_end_matches('0').trim_end_matches('.').to_string();
            let w = inner.width as usize;
            let gap = w.saturating_sub(text::width(&lo) + text::width(&hi));
            let line = format!("{lo}{}{hi}", " ".repeat(gap));
            buf.set_stringn(inner.x, y, text::truncate(&line, w), w, p.on(t.dim, t.bg));
            y += 1;
        }
    }

    // In a column of keys, each frequent value has the count 1. A row of
    // full bars would show a frequency that the data does not have.
    if s.top.iter().all(|(_, n)| *n <= 1) && !s.top.is_empty() {
        y += 1;
        if y < inner.bottom() {
            buf.set_stringn(
                inner.x,
                y,
                text::truncate("every sampled value occurs once", inner.width as usize),
                inner.width as usize,
                p.on(t.dim, t.bg),
            );
        }
    } else if !s.top.is_empty() {
        y += 1;
        section(buf, inner, y, "most common", p, app);
        y += 1;
        let max = s.top.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
        for (val, n) in &s.top {
            if y >= inner.bottom() {
                break;
            }
            let label = match val {
                None => "NULL".to_string(),
                Some(v) => text::sanitize(v),
            };
            let label_style: Style = match val {
                None => p.on(t.null, t.bg),
                Some(_) => p.on(t.fg, t.bg),
            };
            let count = human_count(*n);
            let bar_w = 10usize;
            let name_w = (inner.width as usize)
                .saturating_sub(bar_w + text::width(&count) + 2)
                .max(4);

            buf.set_stringn(
                inner.x,
                y,
                text::fit(&label, name_w, Align::Left),
                name_w,
                label_style,
            );
            let filled = ((*n as f64 / max as f64) * bar_w as f64).round() as usize;
            let bar = format!("{}{}", "▇".repeat(filled), " ".repeat(bar_w - filled));
            buf.set_stringn(inner.x + name_w as u16 + 1, y, &bar, bar_w, p.on(t.accent, t.bg));
            buf.set_stringn(
                inner.x + name_w as u16 + bar_w as u16 + 2,
                y,
                &count,
                text::width(&count),
                p.on(t.dim, t.bg),
            );
            y += 1;
        }
    }

    // With a filter, the statistics describe the rows that the filter keeps.
    // The user can forget that, so the panel gives a note.
    if app.view.filter.is_some() && y < inner.bottom() {
        y += 1;
        if y < inner.bottom() {
            buf.set_stringn(
                inner.x,
                y,
                text::truncate("↑ over filtered rows only", inner.width as usize),
                inner.width as usize,
                p.on(t.warn, t.bg),
            );
        }
    }
}
