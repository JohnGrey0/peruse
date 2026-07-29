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

use peruse_core::model::{Align, CellKind};
use peruse_core::source::{human_bytes, human_count};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{Block, Clear, Widget};

use crate::app::{App, Build, Setting};
use crate::commands::{self, Cmd, BINDINGS, GROUPS};
use crate::paint::Paint;
use crate::text;
use crate::tree::Family;

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



/// Draws the ghost completion after the cursor, in a dim color.
///
/// The three prompts inside an overlay call this. The prompt at the bottom of
/// the screen draws its own, because it colors the SQL of the line first.
fn ghost(buf: &mut Buffer, app: &App, p: &Paint, at: Option<Position>, right: u16) {
    let (Some(pos), Some(rest)) = (at, app.ghost()) else {
        return;
    };
    if pos.x >= right {
        return;
    }
    let w = (right - pos.x) as usize;
    buf.set_stringn(
        pos.x,
        pos.y,
        text::truncate(&rest, w),
        w,
        p.on(app.theme.dim, app.theme.bg_alt),
    );
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
/// The grid shows a row from the left to the right, and it shows a value that
/// holds other values as one long text. A file with 300 columns therefore
/// needs 300 presses of a key to read one row, and a JSON file gives a wall of
/// text in one cell.
///
/// This view puts the fields under each other instead, and a field that holds
/// other values opens. The tree comes from the row as JSON, so a structure and
/// a list both open in the same way. See [`crate::tree`].
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
            ghost(buf, app, p, cursor, inner.right());
        }
    }
    let top = inner.y + if find_on { 2 } else { 0 };
    let list_h = (inner.height as usize).saturating_sub(if find_on { 2 } else { 0 });
    if list_h == 0 {
        return cursor;
    }

    // A row that the engine has not sent yet, and a row that this program
    // could not read, both need a word instead of an empty box.
    if app.record_tree.is_empty() {
        let (msg, style) = match &app.record_tree.error {
            Some(e) => (format!("cannot read this row: {e}"), p.on(t.error, t.bg_alt)),
            None => ("reading…".to_string(), p.on(t.dim, t.bg_alt)),
        };
        buf.set_stringn(
            inner.x,
            top,
            text::truncate(&msg, inner.width as usize),
            inner.width as usize,
            style,
        );
        hints(buf, outer, app, p, " Esc closes ");
        return cursor;
    }

    let lines = app.record_lines();
    if lines.is_empty() {
        let msg = if app.record_find.trim().is_empty() {
            "this row holds no field".to_string()
        } else {
            format!("no field holds {:?}", app.record_find)
        };
        buf.set_stringn(
            inner.x,
            top,
            text::truncate(&msg, inner.width as usize),
            inner.width as usize,
            p.on(t.dim, t.bg_alt),
        );
        hints(buf, outer, app, p, " Esc clears the find box ");
        return cursor;
    }

    let name_w = ((inner.width as usize) * 2 / 5).clamp(8, 44);
    let type_w = if (inner.width as usize) > name_w + 32 { 11 } else { 0 };
    let value_x = inner.x + (name_w + 1 + type_w) as u16 + if type_w > 0 { 1 } else { 0 };
    let value_w =
        (inner.width as usize).saturating_sub(name_w + 1 + type_w + if type_w > 0 { 1 } else { 0 });

    let sel = app.record_sel.min(lines.len() - 1);
    let start = sel.saturating_sub(list_h.saturating_sub(1));

    for (i, line) in lines.iter().skip(start).take(list_h).enumerate() {
        let y = top + i as u16;
        let selected = start + i == sel;
        let bg = if selected { t.cursor_row } else { t.bg_alt };
        buf.set_stringn(
            inner.x,
            y,
            " ".repeat(inner.width as usize),
            inner.width as usize,
            p.bg(bg),
        );

        // The mark shows the level and says whether the line can open. A
        // level costs two screen columns, so a deep tree still fits.
        let mark = if !line.kind.opens() {
            "  "
        } else if line.open {
            "▾ "
        } else {
            "▸ "
        };
        let name = format!("{}{mark}{}", "  ".repeat(line.depth), line.label);

        // A column of the file that the grid hides is still in the record.
        let hidden = line.depth == 0
            && app
                .schema
                .index_of(&line.label)
                .is_some_and(|c| app.hidden[c]);
        let name_style = if hidden {
            p.on(t.dim, bg)
        } else if selected {
            p.bold(p.on(t.accent, bg))
        } else if line.depth == 0 {
            p.on(t.fg, bg)
        } else {
            p.on(t.ident, bg)
        };
        buf.set_stringn(
            inner.x,
            y,
            text::fit(&name, name_w, Align::Left),
            name_w,
            name_style,
        );

        if type_w > 0 {
            // A column of the file shows the type that the file gives. Two
            // cases show the family of the value instead:
            //
            // * A field inside a structure has no type of its own.
            // * The type of a structure can be some thousand characters
            //   long. A cut of it, such as `STRUCT(id …`, says nothing that
            //   the word `struct` does not say.
            let type_text = match (line.depth, app.schema.index_of(&line.label)) {
                (0, Some(c)) if app.schema.columns[c].kind != CellKind::Nested => {
                    app.schema.columns[c].short_type()
                }
                _ => family_word(line).to_string(),
            };
            buf.set_stringn(
                inner.x + name_w as u16 + 1,
                y,
                text::fit(&type_text, type_w, Align::Left),
                type_w,
                p.on(t.dim, bg),
            );
        }

        let style = match line.family {
            Family::Null => p.on(t.null, bg),
            Family::Empty => p.on(t.dim, bg),
            Family::Number => p.on(t.number, bg),
            Family::Bool => p.on(t.boolean, bg),
            Family::Nested => p.on(t.nested, bg),
            Family::Text => p.on(t.string, bg),
        };
        buf.set_stringn(
            value_x,
            y,
            text::truncate(&text::sanitize(&line.value), value_w),
            value_w,
            style,
        );
    }

    // The keys change with the line and with the state. A narrow screen cuts
    // the end of this line, so the keys that the user needs most come first.
    // The two that open the record come before the rest, because a record of
    // structures is unreadable until it opens.
    let open_hint = if lines[sel].kind.opens() {
        "l/h open"
    } else {
        "Enter full value"
    };
    let all_hint = if app.record_tree.is_all_open() {
        "c close all"
    } else {
        "a open all"
    };
    let empty_hint = if app.record_tree.hide_empty {
        "z shows empty"
    } else {
        "z hides empty"
    };
    hints(
        buf,
        outer,
        app,
        p,
        &format!(
            " {}/{} · {open_hint} · {all_hint} · {empty_hint} · n/p row · / find · y copy · P path · = filter · Esc ",
            sel + 1,
            lines.len()
        ),
    );
    cursor
}

/// Gives the word for the family of a value, for the type column.
fn family_word(line: &crate::tree::Line) -> &'static str {
    use crate::tree::NodeKind;
    match line.kind {
        NodeKind::Object(_) => "struct",
        NodeKind::Array(_) => "list",
        NodeKind::Leaf => match line.family {
            Family::Null => "null",
            Family::Empty | Family::Text => "text",
            Family::Number => "number",
            Family::Bool => "boolean",
            Family::Nested => "struct",
        },
    }
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
            " Enter add · Tab take · Esc back "
        } else {
            " Enter next · Esc back "
        },
    );
    let at = Some(Position::new(
        (inner.x + lw as u16 + app.input.cursor_col() as u16).min(inner.right() - 1),
        inner.y,
    ));
    ghost(buf, app, p, at, inner.right());
    at
}

/// Draws the settings page.
///
/// The page has two halves. The upper half holds the settings that the user
/// can change. The lower half holds what the machine gives and what DuckDB
/// uses now. A user who sets a memory limit needs to know how much memory the
/// machine has, and a user who sets the threads needs to know how many cores
/// it has. Without those numbers the user is guessing.
pub fn draw_settings(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> Option<Position> {
    let outer = centered(area, 80, 88, 88);
    // Every change writes the file at once, so the title needs no mark for
    // a change that the user did not keep.
    let inner = frame(buf, outer, "settings · kept as you change them", app, p);
    let t = &app.theme;
    if inner.width < 20 || inner.height < 4 {
        return None;
    }

    let name_w = 15usize.min(inner.width as usize / 3);
    let value_w = 22usize.min(inner.width as usize / 3);
    let mut y = inner.y;
    let mut cursor = None;

    let sel = app.settings_sel.min(Setting::ALL.len() - 1);
    for (i, s) in Setting::ALL.iter().enumerate() {
        if y >= inner.bottom() {
            break;
        }
        let selected = i == sel;
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
            text::fit(s.label(), name_w, Align::Left),
            name_w,
            if selected {
                p.bold(p.on(t.accent, bg))
            } else {
                p.on(t.fg, bg)
            },
        );

        // A setting with no value shows the value that Peruse builds in, in
        // a dim color. The user then sees what happens without a setting.
        let x = inner.x + name_w as u16 + 1;
        let value = app.setting_value(*s);
        if selected && app.settings_editing {
            let shown = app.input.text();
            buf.set_stringn(
                x,
                y,
                text::fit(&shown, value_w, Align::Left),
                value_w,
                p.on(t.lit, bg),
            );
            cursor = Some(Position::new(
                (x + app.input.cursor_col() as u16).min(inner.right() - 1),
                y,
            ));
            ghost(buf, app, p, cursor, x + value_w as u16);
        } else if value.is_empty() {
            buf.set_stringn(
                x,
                y,
                text::fit(&app.setting_default(*s), value_w, Align::Left),
                value_w,
                p.on(t.dim, bg),
            );
        } else {
            buf.set_stringn(
                x,
                y,
                text::fit(&value, value_w, Align::Left),
                value_w,
                p.on(t.lit, bg),
            );
        }

        let hx = x + value_w as u16 + 1;
        let hw = (inner.right().saturating_sub(hx)) as usize;
        buf.set_stringn(hx, y, text::truncate(s.help(), hw), hw, p.on(t.dim, bg));
        y += 1;
    }

    // The lower half: the machine, and what DuckDB uses now.
    let r = &app.resources;
    let mem = |v: Option<u64>| match v {
        Some(n) => human_bytes(n),
        None => "not known".into(),
    };
    let mut facts: Vec<(String, String)> = Vec::new();
    facts.push((
        "cores".into(),
        match &r.cpu {
            Some(name) => format!("{}  ·  {name}", r.cores),
            None => r.cores.to_string(),
        },
    ));
    facts.push((
        "memory".into(),
        format!("{} free of {}", mem(r.free_memory), mem(r.total_memory)),
    ));
    facts.push((
        "duckdb now".into(),
        format!(
            "{} threads  ·  {} memory limit",
            app.duck_threads.as_deref().unwrap_or("?"),
            app.duck_memory.as_deref().unwrap_or("?")
        ),
    ));
    facts.push(("spill to".into(), r.temp_dir.display().to_string()));
    facts.push((
        "file".into(),
        match peruse_core::config::Config::path() {
            Some(p) => p.display().to_string(),
            None => "this system gives no directory".into(),
        },
    ));

    y += 1;
    if y < inner.bottom() {
        buf.set_stringn(
            inner.x,
            y,
            text::truncate("this machine", inner.width as usize),
            inner.width as usize,
            p.bold(p.on(t.accent, t.bg_alt)),
        );
        y += 1;
    }
    for (k, v) in facts {
        if y >= inner.bottom() {
            break;
        }
        buf.set_stringn(
            inner.x,
            y,
            text::fit(&k, name_w, Align::Left),
            name_w,
            p.on(t.dim, t.bg_alt),
        );
        let vx = inner.x + name_w as u16 + 1;
        let vw = (inner.right().saturating_sub(vx)) as usize;
        buf.set_stringn(vx, y, text::truncate(&v, vw), vw, p.on(t.fg, t.bg_alt));
        y += 1;
    }

    let keys = if app.settings_editing {
        " Enter apply · Esc cancel ".to_string()
    } else {
        let machine = if matches!(
            Setting::ALL[sel],
            Setting::Threads | Setting::MemoryLimit
        ) {
            "m use this machine · "
        } else {
            ""
        };
        format!(" Enter change · d built-in · {machine}T themes · Esc close ")
    };
    hints(buf, outer, app, p, &keys);
    cursor
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
    Cmd::Undo,
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
