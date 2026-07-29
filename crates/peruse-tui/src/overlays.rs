//! The six overlays that cover the grid:
//!
//! * the help
//! * the command palette
//! * the theme picker
//! * the cell inspector
//! * the record view, which shows one row from the top to the bottom
//! * the filter builder
//!
//! The two overlays that take text give the position of the terminal cursor
//! back to the caller. The caller therefore needs no second calculation of the
//! layout.

use peruse_core::model::Align;
use peruse_core::source::human_count;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{Block, Clear, Widget};

use crate::app::{App, Build};
use crate::commands::{self, Cmd, BINDINGS, GROUPS};
use crate::paint::Paint;
use crate::text;

/// Gives an area in the middle of the screen, as a percentage of the screen.
fn centered(area: Rect, w_pct: u16, h_pct: u16, max_w: u16) -> Rect {
    // Multiply with 32 bits. A terminal of more than 819 columns makes
    // `width * 80` too large for 16 bits.
    let pct = |v: u16, p: u16| -> u16 { ((v as u32 * p as u32) / 100) as u16 };
    let w = pct(area.width, w_pct).min(max_w).min(area.width);
    let h = pct(area.height, h_pct).min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Gives an area in the middle of the screen with a height in rows.
///
/// A box that takes one line of text needs a height in rows, and not a
/// percentage: a percentage of a small terminal gives a box that cannot hold
/// the prompt. The height comes first, and the position follows it, so the box
/// always stays inside the screen.
fn centered_rows(area: Rect, w_pct: u16, rows: u16, max_w: u16) -> Rect {
    let pct = |v: u16, p: u16| -> u16 { ((v as u32 * p as u32) / 100) as u16 };
    let w = pct(area.width, w_pct).min(max_w).min(area.width);
    let h = rows.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}



/// Clears the area, draws the border and the title, and gives the area inside.
fn frame(buf: &mut Buffer, area: Rect, title: &str, app: &App, p: &Paint) -> Rect {
    let t = &app.theme;
    Clear.render(area, buf);
    Block::bordered()
        .title(format!(" {title} "))
        .border_style(p.on(t.accent, t.bg_alt))
        .title_style(p.bold(p.on(t.accent, t.bg_alt)))
        .style(p.on(t.fg, t.bg_alt))
        .render(area, buf);
    Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    }
}

/// Draws the help overlay.
pub fn draw_help(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) {
    let outer = centered(area, 80, 90, 78);
    let inner = frame(
        buf,
        outer,
        &format!("peruse {} — keys", peruse_core::VERSION),
        app,
        p,
    );
    let t = &app.theme;

    // The help comes from the same table that gives the command for a key.
    // The help and the keys can therefore never disagree.
    let mut lines: Vec<(String, String, bool)> = Vec::new();
    for group in GROUPS {
        lines.push((String::new(), String::new(), false));
        lines.push(((*group).to_string(), String::new(), true));
        for b in BINDINGS.iter().filter(|b| b.group == *group) {
            lines.push((b.label.to_string(), b.desc.to_string(), false));
        }
    }
    lines.push((String::new(), String::new(), false));
    lines.push(("In any prompt".into(), String::new(), true));
    for (k, d) in [
        ("Enter", "apply"),
        ("Esc", "cancel"),
        ("↑ / ↓", "previous / next entry from history"),
        ("^W / ^U / ^K", "delete word / to start / to end"),
        ("^A / ^E", "start / end of line"),
    ] {
        lines.push((k.to_string(), d.to_string(), false));
    }
    lines.push((String::new(), String::new(), false));
    lines.push(("Notes".into(), String::new(), true));
    lines.push((
        "read-only".into(),
        "queries that would write are rejected before they run".into(),
        false,
    ));
    lines.push((
        "clipboard".into(),
        "uses OSC 52, so copying works over SSH too".into(),
        false,
    ));

    let max_scroll = lines.len().saturating_sub(inner.height as usize) as u16;
    let scroll = app.help_scroll.min(max_scroll);
    let key_w = 16usize.min(inner.width as usize / 3);

    for (i, (k, d, is_head)) in lines
        .iter()
        .skip(scroll as usize)
        .take(inner.height as usize)
        .enumerate()
    {
        let y = inner.y + i as u16;
        if *is_head {
            buf.set_stringn(
                inner.x,
                y,
                text::fit(k, inner.width as usize, Align::Left),
                inner.width as usize,
                p.bold(p.on(t.accent, t.bg_alt)),
            );
            continue;
        }
        if k.is_empty() {
            continue;
        }
        buf.set_stringn(
            inner.x,
            y,
            text::fit(k, key_w, Align::Left),
            key_w,
            p.on(t.key_fg, t.bg_alt),
        );
        let dw = (inner.width as usize).saturating_sub(key_w + 1);
        buf.set_stringn(
            inner.x + key_w as u16 + 1,
            y,
            text::truncate(d, dw),
            dw,
            p.on(t.fg, t.bg_alt),
        );
    }

    if max_scroll > 0 {
        let hint = format!(" j/k to scroll · {}/{} ", scroll + 1, max_scroll + 1);
        buf.set_stringn(
            outer.x + 2,
            outer.bottom().saturating_sub(1),
            &hint,
            outer.width.saturating_sub(4) as usize,
            p.on(t.dim, t.bg_alt),
        );
    }
}

/// Draws the command palette.
pub fn draw_palette(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) {
    let outer = centered(area, 70, 70, 76);
    let inner = frame(buf, outer, "run a command", app, p);
    let t = &app.theme;

    let query = app.input.text();
    buf.set_stringn(
        inner.x,
        inner.y,
        format!("› {query}"),
        inner.width as usize,
        p.on(t.fg, t.bg_alt),
    );

    let items = app.palette_items();
    let list_h = inner.height.saturating_sub(2) as usize;
    let sel = app.palette_sel.min(items.len().saturating_sub(1));
    let start = sel.saturating_sub(list_h.saturating_sub(1));

    if items.is_empty() {
        buf.set_stringn(
            inner.x,
            inner.y + 2,
            "no matching command",
            inner.width as usize,
            p.on(t.dim, t.bg_alt),
        );
        return;
    }

    for (i, cmd) in items.iter().skip(start).take(list_h).enumerate() {
        let Some(b) = commands::binding(*cmd) else {
            continue;
        };
        let y = inner.y + 2 + i as u16;
        let selected = start + i == sel;
        let bg = if selected { t.cursor_row } else { t.bg_alt };
        buf.set_stringn(
            inner.x,
            y,
            " ".repeat(inner.width as usize),
            inner.width as usize,
            p.bg(bg),
        );

        let key_w = 14usize.min(inner.width as usize / 3);
        buf.set_stringn(
            inner.x,
            y,
            text::fit(b.label, key_w, Align::Left),
            key_w,
            p.on(t.key_fg, bg),
        );
        let dw = (inner.width as usize).saturating_sub(key_w + 1);
        let style = if selected {
            p.bold(p.on(t.fg, bg))
        } else {
            p.on(t.fg, bg)
        };
        buf.set_stringn(inner.x + key_w as u16 + 1, y, text::truncate(b.desc, dw), dw, style);
    }
}

/// Draws the theme picker.
pub fn draw_theme_picker(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) {
    let outer = centered(area, 60, 70, 60);
    let inner = frame(buf, outer, "theme", app, p);
    let t = &app.theme;

    let list_h = inner.height.saturating_sub(1) as usize;
    let sel = app.theme_sel.min(app.themes.len().saturating_sub(1));
    let start = sel.saturating_sub(list_h.saturating_sub(1));

    for (i, theme) in app.themes.iter().skip(start).take(list_h).enumerate() {
        let y = inner.y + i as u16;
        let selected = start + i == sel;
        let bg = if selected { t.cursor_row } else { t.bg_alt };
        buf.set_stringn(
            inner.x,
            y,
            " ".repeat(inner.width as usize),
            inner.width as usize,
            p.bg(bg),
        );
        let name_w = (inner.width as usize).saturating_sub(14).max(8);
        let style = if selected {
            p.bold(p.on(t.accent, bg))
        } else {
            p.on(t.fg, bg)
        };
        buf.set_stringn(
            inner.x,
            y,
            text::fit(&theme.name, name_w, Align::Left),
            name_w,
            style,
        );

        // Show the colors that the theme gives to the families of values.
        // The user can then select a theme without a test of each theme.
        let swatch = [theme.number, theme.string, theme.temporal, theme.boolean, theme.null];
        for (n, c) in swatch.iter().enumerate() {
            buf.set_stringn(
                inner.x + name_w as u16 + 1 + n as u16 * 2,
                y,
                "██",
                2,
                p.on(*c, bg),
            );
        }
    }

    buf.set_stringn(
        inner.x,
        outer.bottom().saturating_sub(1),
        " ↑↓ preview · Enter apply · Esc cancel ",
        inner.width as usize,
        p.on(t.dim, t.bg_alt),
    );
}

/// Draws the cell inspector with the complete value of one cell.
pub fn draw_cell(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) {
    let outer = centered(area, 80, 70, 100);
    let col = app
        .schema
        .columns
        .get(app.cursor_col)
        .map(|c| format!("{} · {}", c.name, c.short_type()))
        .unwrap_or_default();
    let inner = frame(
        buf,
        outer,
        &format!("row {} · {col}", app.cursor_row.saturating_add(1)),
        app,
        p,
    );
    let t = &app.theme;

    let value = app.cell_value.as_deref().unwrap_or("…");
    let lines = text::wrap(value, inner.width as usize);
    let max_scroll = lines.len().saturating_sub(inner.height as usize) as u16;
    let scroll = app.cell_scroll.min(max_scroll);

    let kind = app
        .schema
        .columns
        .get(app.cursor_col)
        .map(|c| c.kind)
        .unwrap_or(peruse_core::CellKind::Text);
    let style = if value == "NULL" {
        p.on(t.null, t.bg_alt)
    } else {
        p.on(crate::grid::kind_color(t, kind), t.bg_alt)
    };

    for (i, line) in lines
        .iter()
        .skip(scroll as usize)
        .take(inner.height as usize)
        .enumerate()
    {
        buf.set_stringn(
            inner.x,
            inner.y + i as u16,
            line,
            inner.width as usize,
            style,
        );
    }

    let footer = format!(
        " {} chars · {} lines{} · y copies · Esc closes ",
        value.chars().count(),
        lines.len(),
        if max_scroll > 0 { " · j/k scrolls" } else { "" }
    );
    buf.set_stringn(
        outer.x + 2,
        outer.bottom().saturating_sub(1),
        text::truncate(&footer, outer.width.saturating_sub(4) as usize),
        outer.width.saturating_sub(4) as usize,
        p.on(t.dim, t.bg_alt),
    );
}

/// Writes the keys along the bottom edge of an overlay.
fn hints(buf: &mut Buffer, outer: Rect, app: &App, p: &Paint, text: &str) {
    let t = &app.theme;
    let w = outer.width.saturating_sub(4) as usize;
    buf.set_stringn(
        outer.x + 2,
        outer.bottom().saturating_sub(1),
        crate::text::truncate(text, w),
        w,
        p.on(t.dim, t.bg_alt),
    );
}

/// Draws the record view: one row of the grid, from the top to the bottom.
///
/// The grid shows a row from the left to the right. A file with 300 columns
/// therefore needs 300 presses of a key to read one row. This view puts the
/// columns under each other instead, so the whole row fits on some screens,
/// and the find box goes to one column by its name.
///
/// The values come from the page that the grid holds already. The view
/// therefore costs no query, and it opens immediately.
pub fn draw_record(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> Option<Position> {
    let outer = centered(area, 90, 92, 120);
    let position = match app.total.value() {
        Some(n) => format!(
            "record {} of {}",
            human_count(app.cursor_row.saturating_add(1)),
            human_count(n)
        ),
        None => format!("record {}", human_count(app.cursor_row.saturating_add(1))),
    };
    let inner = frame(buf, outer, &position, app, p);
    let t = &app.theme;
    if inner.width < 10 || inner.height < 2 {
        return None;
    }

    // The find box takes the first two rows, but only when it is in use.
    let find_on = app.record_finding || !app.record_find.is_empty();
    let mut cursor = None;
    if find_on {
        let label = "find › ";
        let lw = text::width(label);
        buf.set_stringn(
            inner.x,
            inner.y,
            label,
            lw,
            p.bold(p.on(t.accent, t.bg_alt)),
        );
        let shown = if app.record_finding {
            app.input.text()
        } else {
            app.record_find.clone()
        };
        let w = (inner.width as usize).saturating_sub(lw);
        buf.set_stringn(
            inner.x + lw as u16,
            inner.y,
            text::truncate(&shown, w),
            w,
            p.on(t.fg, t.bg_alt),
        );
        if app.record_finding {
            cursor = Some(Position::new(
                (inner.x + lw as u16 + app.input.cursor_col() as u16).min(inner.right() - 1),
                inner.y,
            ));
        }
    }
    let top = inner.y + if find_on { 2 } else { 0 };
    let list_h = (inner.height as usize).saturating_sub(if find_on { 2 } else { 0 });
    if list_h == 0 {
        return cursor;
    }

    let fields = app.record_fields();
    if fields.is_empty() {
        buf.set_stringn(
            inner.x,
            top,
            format!("no column name holds {:?}", app.record_find),
            inner.width as usize,
            p.on(t.dim, t.bg_alt),
        );
        hints(buf, outer, app, p, " Esc clears the find box ");
        return cursor;
    }

    // The width of the name column comes from each column of the file, and
    // not from the columns that the find box keeps. The list therefore does
    // not move sideways while the user types in the find box.
    let widest = app
        .schema
        .columns
        .iter()
        .map(|c| text::width(&c.name))
        .max()
        .unwrap_or(8);
    let name_w = widest.clamp(6, ((inner.width as usize) * 2 / 5).max(6));
    let type_w = if (inner.width as usize) > name_w + 32 { 12 } else { 0 };
    let gap = if type_w > 0 { 2 } else { 1 };
    let value_x = inner.x + (name_w + 1 + type_w) as u16 + (gap - 1) as u16;
    let value_w = (inner.width as usize).saturating_sub(name_w + 1 + type_w + gap - 1);

    let sel = app.record_sel.min(fields.len() - 1);
    let start = sel.saturating_sub(list_h.saturating_sub(1));

    for (i, col) in fields.iter().skip(start).take(list_h).enumerate() {
        let y = top + i as u16;
        let column = &app.schema.columns[*col];
        let selected = start + i == sel;
        let bg = if selected { t.cursor_row } else { t.bg_alt };
        buf.set_stringn(
            inner.x,
            y,
            " ".repeat(inner.width as usize),
            inner.width as usize,
            p.bg(bg),
        );

        // A column that the grid hides is still in the record. The user opens
        // this view to see the complete row.
        let name_style = if app.hidden[*col] {
            p.on(t.dim, bg)
        } else if selected {
            p.bold(p.on(t.accent, bg))
        } else {
            p.on(t.fg, bg)
        };
        buf.set_stringn(
            inner.x,
            y,
            text::fit(&column.name, name_w, Align::Left),
            name_w,
            name_style,
        );
        if type_w > 0 {
            buf.set_stringn(
                inner.x + name_w as u16 + 1,
                y,
                text::fit(&column.short_type(), type_w, Align::Left),
                type_w,
                p.on(t.dim, bg),
            );
        }

        // Three cases look the same in a plain grid, and they are not the
        // same: a value that is missing, a text with no character, and a
        // value that the engine has not read yet.
        let (shown, style) = match app.record_value(*col) {
            None => ("…".to_string(), p.on(t.dim, bg)),
            Some(None) => ("NULL".to_string(), p.on(t.null, bg)),
            Some(Some("")) => ("(empty)".to_string(), p.on(t.dim, bg)),
            Some(Some(v)) => (
                text::sanitize(v),
                p.on(crate::grid::kind_color(t, column.kind), bg),
            ),
        };
        buf.set_stringn(
            value_x,
            y,
            text::truncate(&shown, value_w),
            value_w,
            style,
        );
    }

    let hidden = if app.hidden[fields[sel]] { " · hidden in the grid" } else { "" };
    hints(
        buf,
        outer,
        app,
        p,
        &format!(
            " field {}/{} · j/k move · n/p record · / find · Enter full value · y copy · = filter · Esc close{hidden} ",
            sel + 1,
            fields.len()
        ),
    );
    cursor
}

/// Draws the filter builder.
///
/// The builder is a small machine with five steps. Each step draws its own
/// list or its own prompt in the same box, so the user always looks at the
/// same place.
pub fn draw_filter_build(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> Option<Position> {
    match app.build {
        Build::List => {
            draw_build_list(buf, area, app, p);
            None
        }
        Build::Column => draw_build_column(buf, area, app, p),
        Build::Op => {
            draw_build_op(buf, area, app, p);
            None
        }
        Build::Value | Build::Value2 | Build::Raw => draw_build_value(buf, area, app, p),
    }
}

/// Draws the list of conditions.
fn draw_build_list(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) {
    let outer = centered(area, 80, 70, 96);
    let inner = frame(buf, outer, "filter", app, p);
    let t = &app.theme;
    if inner.width < 10 || inner.height < 2 {
        return;
    }

    if app.fset.conditions.is_empty() {
        buf.set_stringn(
            inner.x,
            inner.y,
            "no condition yet — press a to add one",
            inner.width as usize,
            p.on(t.dim, t.bg_alt),
        );
        hints(buf, outer, app, p, " a add · r SQL · Esc close ");
        return;
    }

    // Keep the last two rows for the compiled expression.
    let list_h = (inner.height as usize).saturating_sub(2).max(1);
    let sel = app.build_sel.min(app.fset.conditions.len() - 1);
    let start = sel.saturating_sub(list_h.saturating_sub(1));

    let join_w = 4usize;
    let col_w = 22usize.min((inner.width as usize) / 3);
    let op_w = 17usize.min((inner.width as usize) / 4);

    for (i, cond) in app
        .fset
        .conditions
        .iter()
        .skip(start)
        .take(list_h)
        .enumerate()
    {
        let at = start + i;
        let y = inner.y + i as u16;
        let selected = at == sel;
        let bg = if selected { t.cursor_row } else { t.bg_alt };
        buf.set_stringn(
            inner.x,
            y,
            " ".repeat(inner.width as usize),
            inner.width as usize,
            p.bg(bg),
        );

        // The first condition has no word in front of it.
        let join = if at == 0 { "" } else { cond.join.word() };
        buf.set_stringn(
            inner.x,
            y,
            text::fit(join, join_w, Align::Left),
            join_w,
            p.on(t.kw, bg),
        );

        let (column, op, value) = cond.term.parts();
        let mut x = inner.x + join_w as u16;
        buf.set_stringn(
            x,
            y,
            text::fit(&column, col_w, Align::Left),
            col_w,
            if selected {
                p.bold(p.on(t.accent, bg))
            } else {
                p.on(t.fg, bg)
            },
        );
        x += col_w as u16 + 1;
        buf.set_stringn(
            x,
            y,
            text::fit(&op, op_w, Align::Left),
            op_w,
            p.on(t.kw, bg),
        );
        x += op_w as u16 + 1;
        let vw = (inner.right().saturating_sub(x)) as usize;
        buf.set_stringn(x, y, text::truncate(&value, vw), vw, p.on(t.lit, bg));
    }

    // Show the expression that the list compiles to. The user can then see
    // what the builder sends to the database.
    let sql = app.fset.to_sql().unwrap_or_else(|| "(no filter)".into());
    buf.set_stringn(
        inner.x,
        inner.bottom().saturating_sub(1),
        text::truncate(&format!("WHERE {sql}"), inner.width as usize),
        inner.width as usize,
        p.on(t.dim, t.bg_alt),
    );
    hints(
        buf,
        outer,
        app,
        p,
        " a add · e edit · d delete · o AND/OR · c clear · r SQL · Enter apply · Esc cancel ",
    );
}

/// Draws the list of columns.
fn draw_build_column(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> Option<Position> {
    let outer = centered(area, 70, 76, 76);
    let inner = frame(buf, outer, "filter · which column?", app, p);
    let t = &app.theme;
    if inner.width < 10 || inner.height < 3 {
        return None;
    }

    let query = app.input.text();
    let label = "› ";
    buf.set_stringn(
        inner.x,
        inner.y,
        format!("{label}{query}"),
        inner.width as usize,
        p.on(t.fg, t.bg_alt),
    );
    let cursor = Position::new(
        (inner.x + text::width(label) as u16 + app.input.cursor_col() as u16)
            .min(inner.right() - 1),
        inner.y,
    );

    let cols = app.build_columns();
    let list_h = (inner.height as usize).saturating_sub(2);
    if cols.is_empty() || list_h == 0 {
        buf.set_stringn(
            inner.x,
            inner.y + 2,
            "no column matches",
            inner.width as usize,
            p.on(t.dim, t.bg_alt),
        );
        hints(buf, outer, app, p, " type to search · Esc back ");
        return Some(cursor);
    }
    let sel = app.pick_sel.min(cols.len() - 1);
    let start = sel.saturating_sub(list_h.saturating_sub(1));
    let name_w = ((inner.width as usize) * 3 / 5).max(8);

    for (i, c) in cols.iter().skip(start).take(list_h).enumerate() {
        let y = inner.y + 2 + i as u16;
        let column = &app.schema.columns[*c];
        let selected = start + i == sel;
        let bg = if selected { t.cursor_row } else { t.bg_alt };
        buf.set_stringn(
            inner.x,
            y,
            " ".repeat(inner.width as usize),
            inner.width as usize,
            p.bg(bg),
        );
        buf.set_stringn(
            inner.x,
            y,
            text::fit(&column.name, name_w, Align::Left),
            name_w,
            if selected {
                p.bold(p.on(t.accent, bg))
            } else {
                p.on(t.fg, bg)
            },
        );
        let tw = (inner.width as usize).saturating_sub(name_w + 1);
        buf.set_stringn(
            inner.x + name_w as u16 + 1,
            y,
            text::truncate(&column.short_type(), tw),
            tw,
            p.on(t.dim, bg),
        );
    }
    hints(
        buf,
        outer,
        app,
        p,
        &format!(
            " {}/{} · type to search · ↑↓ select · Enter next · Esc back ",
            sel + 1,
            cols.len()
        ),
    );
    Some(cursor)
}

/// Draws the list of operators.
fn draw_build_op(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) {
    let name = app
        .schema
        .columns
        .get(app.draft.col)
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let outer = centered(area, 66, 70, 72);
    let inner = frame(buf, outer, &format!("filter · {name} …"), app, p);
    let t = &app.theme;
    if inner.width < 10 || inner.height < 1 {
        return;
    }

    let ops = app.build_ops();
    let list_h = inner.height as usize;
    let sel = app.pick_sel.min(ops.len().saturating_sub(1));
    let start = sel.saturating_sub(list_h.saturating_sub(1));
    let op_w = 18usize.min((inner.width as usize) / 2);

    for (i, op) in ops.iter().skip(start).take(list_h).enumerate() {
        let y = inner.y + i as u16;
        let selected = start + i == sel;
        let bg = if selected { t.cursor_row } else { t.bg_alt };
        buf.set_stringn(
            inner.x,
            y,
            " ".repeat(inner.width as usize),
            inner.width as usize,
            p.bg(bg),
        );
        buf.set_stringn(
            inner.x,
            y,
            text::fit(op.label(), op_w, Align::Left),
            op_w,
            if selected {
                p.bold(p.on(t.accent, bg))
            } else {
                p.on(t.kw, bg)
            },
        );
        let dw = (inner.width as usize).saturating_sub(op_w + 1);
        buf.set_stringn(
            inner.x + op_w as u16 + 1,
            y,
            text::truncate(op.help(), dw),
            dw,
            p.on(t.dim, bg),
        );
    }
    hints(buf, outer, app, p, " ↑↓ select · Enter next · Esc back ");
}

/// Draws the step that takes a value, and the step that takes a complete
/// `WHERE` expression.
fn draw_build_value(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> Option<Position> {
    let column = app
        .schema
        .columns
        .get(app.draft.col)
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let (title, label) = match app.build {
        Build::Raw => ("filter · WHERE expression".to_string(), "where".to_string()),
        Build::Value2 => (
            format!("filter · {column} {} {} …", app.draft.op.label(), app.draft.value),
            "and".to_string(),
        ),
        _ => (
            format!("filter · {column} {}", app.draft.op.label()),
            app.draft.op.value_hint().to_string(),
        ),
    };
    // The box holds a border, one line of text and one line for a message.
    let outer = centered_rows(area, 70, 7, 84);
    let inner = frame(buf, outer, &title, app, p);
    let t = &app.theme;
    if inner.width < 8 || inner.height < 1 {
        return None;
    }

    let prefix = format!("{label} › ");
    let lw = text::width(&prefix);
    buf.set_stringn(
        inner.x,
        inner.y,
        &prefix,
        lw,
        p.bold(p.on(t.accent, t.bg_alt)),
    );
    let value = app.input.text();
    let vw = (inner.width as usize).saturating_sub(lw);
    buf.set_stringn(
        inner.x + lw as u16,
        inner.y,
        text::truncate(&value, vw),
        vw,
        p.on(t.lit, t.bg_alt),
    );

    // The guard checks a typed expression while the user types it. A very
    // small screen has no room for the message, and the prompt comes first.
    let note_y = inner.y + 2;
    if note_y < inner.bottom() {
        let note = match (&app.prompt_error, app.build) {
            (Some(err), _) => Some((format!("✕ {err}"), t.error)),
            (None, Build::Raw) => Some((
                "any WHERE expression that DuckDB understands".to_string(),
                t.dim,
            )),
            _ => None,
        };
        if let Some((text, colour)) = note {
            buf.set_stringn(
                inner.x,
                note_y,
                text::truncate(&text, inner.width as usize),
                inner.width as usize,
                p.on(colour, t.bg_alt),
            );
        }
    }

    hints(
        buf,
        outer,
        app,
        p,
        if app.build == Build::Raw {
            " Enter add · Esc back "
        } else {
            " Enter next · Esc back "
        },
    );
    Some(Position::new(
        (inner.x + lw as u16 + app.input.cursor_col() as u16).min(inner.right() - 1),
        inner.y,
    ))
}

/// The commands that the footer shows, the most important command first.
///
/// A narrow terminal cuts the list at the end. The keys for "quit" and for
/// "help" must therefore never be the keys that Peruse removes. The other
/// commands are one press of `?` away.
pub const FOOTER_HINTS: &[Cmd] = &[
    Cmd::Help,
    Cmd::Quit,
    Cmd::Search,
    Cmd::FilterBuild,
    Cmd::Record,
    Cmd::Sql,
    Cmd::SortCycle,
    Cmd::ToggleMeta,
    Cmd::ToggleStats,
    Cmd::Palette,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_with_a_height_in_rows_stays_inside_a_small_screen() {
        // A height that the code sets after it centres the box would push
        // the bottom edge past the last row of the screen. The program then
        // writes outside the buffer and stops.
        for (w, h) in [(20u16, 6u16), (10, 4), (40, 5), (200, 60), (8, 1)] {
            let area = Rect { x: 0, y: 0, width: w, height: h };
            let c = centered_rows(area, 70, 7, 84);
            assert!(c.bottom() <= area.bottom(), "bottom {} of {h}", c.bottom());
            assert!(c.right() <= area.right(), "right {} of {w}", c.right());
        }
    }

    #[test]
    fn a_very_wide_terminal_does_not_overflow_the_layout_calculation() {
        // The value width * 80 is more than a 16-bit number can hold when the
        // terminal has more than 819 columns.
        for (w, h) in [(1000u16, 60u16), (2000, 200), (65535, 65535), (10, 4)] {
            let area = Rect { x: 0, y: 0, width: w, height: h };
            let c = centered(area, 80, 90, 78);
            assert!(c.width <= area.width, "width {} of {w}", c.width);
            assert!(c.height <= area.height, "height {} of {h}", c.height);
            assert!(c.right() <= area.right(), "right edge outside {w}");
            assert!(c.bottom() <= area.bottom(), "bottom edge outside {h}");
        }
    }
}
