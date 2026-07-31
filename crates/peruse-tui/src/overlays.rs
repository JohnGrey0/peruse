//! The six overlays that cover the grid:
//!
//! * the help
//! * the command palette
//! * the theme picker
//! * the cell inspector
//! * the record view, which shows one row from the top to the bottom
//! * the filter builder
//!
//! Each overlay gives its box back to the caller, as an
//! [`OverlayHit`]. The frame is the only place that knows where an overlay
//! sits, and the mouse needs that box: a click outside it closes the overlay,
//! and a click on a line of its list acts on that line.
//!
//! The overlays that take text give the position of the terminal cursor back
//! too. The caller therefore needs no second calculation of the layout.

use peruse_core::model::{Align, CellKind};
use peruse_core::source::{human_bytes, human_count};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Clear, Widget};

use crate::app::{App, Build, OverlayHit, Setting};
use crate::commands::{self, Cmd, BINDINGS, GROUPS};
use crate::paint::Paint;
use crate::text;
use crate::tree::Family;

/// What an overlay that takes text tells the caller after it draws.
///
/// The box goes to the mouse. The position of the terminal cursor goes to the
/// caller, which puts the cursor of the terminal there.
pub struct Drawn {
    /// The box, and each line of the list inside it.
    pub hit: OverlayHit,
    /// Where the terminal cursor goes, or `None` when it stays invisible.
    pub cursor: Option<Position>,
}

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



/// Gives the first item of a window that shows the item `sel`.
///
/// The selected item stays near the middle of the window, so the user sees the
/// items after it and the items before it.
///
/// A window that keeps the selected item on its last row shows nothing after
/// that item. The user then cannot see where the list goes: a list of fifty
/// commands reads as a list of one, and the key to move down looks broken.
/// Each list in each overlay therefore uses this one function.
fn window_start(sel: usize, len: usize, window: usize) -> usize {
    if window == 0 || len <= window {
        return 0;
    }
    sel.saturating_sub(window / 2).min(len - window)
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

/// The dim mark after a column name, and what each mark says.
///
/// The list holds one line for each family of values. A mark with no line in
/// the help is a mark that nobody can read, so a new family needs a line here.
/// The marks come from [`CellKind::badge`], and a test keeps the two together.
const TYPE_MARKS: &[(&str, &str)] = &[
    ("#", "number: an integer, a decimal or a float"),
    ("\"", "text"),
    ("?", "boolean: true or false"),
    ("@", "date, time, timestamp or interval"),
    ("~", "binary, such as BLOB"),
    ("{", "structure, list or map"),
];

/// Draws the help overlay, and gives its box back for the mouse.
pub fn draw_help(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> OverlayHit {
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
        // The Option key of a Mac sends Alt with the arrow, so the help names
        // that form as well. Without it, a user of a Mac never learns the keys
        // that the machine of that user can send.
        ("^← / ^→", "one word left / right; Alt+←/→, Alt+B, Alt+F do the same"),
        ("^Backspace", "delete the word in front; Alt+Backspace is the same"),
        ("^Delete", "delete the word after; Alt+D is the same"),
    ] {
        lines.push((k.to_string(), d.to_string(), false));
    }
    lines.push((String::new(), String::new(), false));
    lines.push(("Mouse".into(), String::new(), true));
    for (k, d) in [
        ("wheel", "up and down the rows"),
        ("Shift + wheel", "across the columns"),
        ("wheel sideways", "the same as Shift and the wheel"),
        ("click", "put the cursor on that cell"),
        ("double click", "open that row as a record, as r does"),
        ("click a name", "go to that column; a click never sorts"),
        ("wheel, overlay", "move the selection of the overlay"),
        ("click, overlay", "select that line; on a value, open or close it"),
        ("double, overlay", "open, run or apply, as Enter does"),
        ("click outside", "close the overlay, as Esc does"),
        ("--no-mouse", "start with no mouse, so the terminal selects text"),
        ("mouse = false", "the same, in the settings file"),
    ] {
        lines.push((k.to_string(), d.to_string(), false));
    }
    lines.push((String::new(), String::new(), false));
    // A user asked what the letter after a column name means. The legend is
    // the answer, and it is beside the keys that the user already reads.
    lines.push((
        "Column types — the dim mark after a column name".into(),
        String::new(),
        true,
    ));
    for (k, d) in TYPE_MARKS {
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
    // The help holds text and no list. A click inside it therefore selects
    // nothing, and a click outside it closes the overlay.
    OverlayHit::new(app.mode, outer)
}

/// The width of the marker column of the palette, in screen columns.
///
/// The marker column holds the mark of the selected row. Each row keeps the
/// column, so the key labels of the rows stay in one straight column.
const MARK_W: usize = 2;

/// One row of the palette on the screen.
///
/// The palette groups the full list under headings, so a row is not always a
/// command. A command therefore carries its own position in the list of
/// matches: the selection counts commands, and not rows.
enum Row {
    /// An empty row in front of a heading.
    Blank,
    /// The name of a group.
    Head(&'static str),
    /// One command, with its position in the list of matches.
    Item(usize, &'static commands::Binding),
}

/// Gives the box of the command palette.
///
/// The palette draws its own prompt inside the box, and the caller puts the
/// terminal cursor on that prompt. The caller therefore needs the box: the
/// prompt is on the row `y + 1`, and the text of the prompt starts at the
/// column `x + 4`.
pub fn palette_rect(area: Rect) -> Rect {
    // The widest row holds a key label, the longest description and a group
    // tag. A box of 96 columns holds the three with no cut.
    centered(area, 80, 80, 96)
}

/// Gives the byte position in `hay` of each character that `needle` matched.
///
/// The palette marks these characters. The user then sees why a row is in the
/// list, and not only that the list is short.
///
/// The walk goes through the original text, and not through a copy in small
/// letters. A change to small letters can change the number of characters, so
/// a position in the copy is not always a position in the original. See
/// [`text::find_ci`] for the same rule.
///
/// The result is `None` when the text does not match, so the function agrees
/// with [`commands::fuzzy_match`].
fn match_positions(needle: &str, hay: &str) -> Option<Vec<usize>> {
    // Take the whitespace out, as the match of the palette does.
    let want: Vec<char> = needle
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if want.is_empty() {
        return Some(Vec::new());
    }
    let mut at = 0usize;
    let mut out = Vec::new();
    for (i, hc) in hay.char_indices() {
        let mut hit = false;
        // One character can give more than one character in small letters.
        // Each of them can match, because the palette matches the same
        // sequence of characters.
        for lc in hc.to_lowercase() {
            if at < want.len() && want[at] == lc {
                at += 1;
                hit = true;
            }
        }
        if hit {
            out.push(i);
        }
        if at == want.len() {
            return Some(out);
        }
    }
    None
}

/// Draws a description and marks the characters that the query matched.
///
/// `styles` holds the style of the text and the style of a matched character.
fn draw_desc(
    buf: &mut Buffer,
    at: Position,
    w: usize,
    desc: &str,
    hits: &[usize],
    styles: (Style, Style),
) {
    if w == 0 {
        return;
    }
    buf.set_stringn(at.x, at.y, text::truncate(desc, w), w, styles.0);
    if hits.is_empty() {
        return;
    }
    // `truncate` keeps one column for the character that shows the cut. A
    // description that is too long therefore has one column less of its own.
    let limit = if text::width(desc) > w {
        w.saturating_sub(1)
    } else {
        w
    };
    let mut col = 0usize;
    for (i, c) in desc.char_indices() {
        let one = &desc[i..i + c.len_utf8()];
        let cw = text::width(one);
        if col + cw > limit {
            break;
        }
        if hits.binary_search(&i).is_ok() {
            buf.set_stringn(at.x + col as u16, at.y, one, cw, styles.1);
        }
        col += cw;
    }
}

/// Draws the command palette.
///
/// One row reads as one command: a mark, the keys, the description and the
/// group. The full list comes under group headings, and a query gives a flat
/// list with the matched characters marked.
pub fn draw_palette(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> OverlayHit {
    let outer = palette_rect(area);
    let inner = frame(buf, outer, "run a command", app, p);
    let t = &app.theme;
    let mut hit = OverlayHit::new(app.mode, outer);
    if inner.width < 8 || inner.height < 1 {
        return hit;
    }

    let query = app.input.text();
    let items = app.palette_items();
    let grouped = query.trim().is_empty();

    // The count says that the list is short because of the query, and not
    // because Peruse has few commands.
    let count = if grouped {
        format!("{} commands", BINDINGS.len())
    } else {
        format!("{} of {} commands", items.len(), BINDINGS.len())
    };
    let count_w = text::width(&count);
    // The query keeps the room on a narrow box, because the user types there.
    let with_count = (inner.width as usize) >= count_w + 14;
    let prompt_w = if with_count {
        (inner.width as usize) - count_w - 2
    } else {
        inner.width as usize
    };
    // The mark of the prompt takes two columns, and the caller counts on that
    // width to put the terminal cursor. See [`palette_rect`].
    buf.set_stringn(inner.x, inner.y, "› ", MARK_W, p.bold(p.on(t.accent, t.bg_alt)));
    buf.set_stringn(
        inner.x + MARK_W as u16,
        inner.y,
        text::truncate(&query, prompt_w.saturating_sub(MARK_W)),
        prompt_w.saturating_sub(MARK_W),
        p.on(t.fg, t.bg_alt),
    );
    if with_count {
        buf.set_stringn(
            inner.right() - count_w as u16,
            inner.y,
            &count,
            count_w,
            p.on(t.dim, t.bg_alt),
        );
    }

    if items.is_empty() {
        // A box with no row and no way out is the worst state of an overlay.
        // Say what the query did, and say which key closes the palette.
        for (n, line) in [
            format!("no command matches {query:?}"),
            "delete a character to see more, or press Esc to close".to_string(),
        ]
        .iter()
        .enumerate()
        {
            let y = inner.y + 2 + n as u16;
            if y >= inner.bottom() {
                break;
            }
            buf.set_stringn(
                inner.x,
                y,
                text::truncate(line, inner.width as usize),
                inner.width as usize,
                p.on(t.dim, t.bg_alt),
            );
        }
        hints(buf, outer, app, p, " Esc closes the palette ");
        return hit;
    }

    let list_h = (inner.height as usize).saturating_sub(2);
    if list_h == 0 {
        // The box is too short for one command. Write the key that closes the
        // palette. A box with no row and no way out is the worst state of an
        // overlay: the user cannot see what to press.
        hints(buf, outer, app, p, " Esc closes the palette ");
        return hit;
    }
    let sel = app.palette_sel.min(items.len() - 1);

    // The rows come from the list of matches in its own order, because the up
    // key and the down key walk that order. A heading comes in front of each
    // change of group, so the order stays and the eye still sees the families.
    let mut rows: Vec<Row> = Vec::new();
    let mut group = "";
    for (i, cmd) in items.iter().enumerate() {
        let Some(b) = commands::binding(*cmd) else {
            continue;
        };
        if grouped && b.group != group {
            if !rows.is_empty() {
                rows.push(Row::Blank);
            }
            rows.push(Row::Head(b.group));
            group = b.group;
        }
        rows.push(Row::Item(i, b));
    }

    let sel_at = rows
        .iter()
        .position(|r| matches!(r, Row::Item(i, _) if *i == sel))
        .unwrap_or(0);
    let start = window_start(sel_at, rows.len(), list_h);

    // The key column is as wide as the widest label of the table. The
    // descriptions of the full list and of a short list then start at the same
    // column, and the eye reads one straight column of text.
    let key_w = BINDINGS
        .iter()
        .map(|b| text::width(b.label))
        .max()
        .unwrap_or(0)
        .min(inner.width as usize / 3);
    let tag_w = GROUPS.iter().map(|g| text::width(g)).max().unwrap_or(0);
    // The tag goes away first on a narrow box. The description says what the
    // command does, and the tag only says the family. A row therefore needs
    // the mark, the keys, 24 columns of description and one space of its own
    // before the tag can come back.
    let room = MARK_W + key_w + 2 + 24 + 1 + tag_w;
    let tag_w = if (inner.width as usize) >= room { tag_w } else { 0 };
    let desc_x = inner.x + (MARK_W + key_w + 2) as u16;
    let desc_w = (inner.width as usize)
        .saturating_sub(MARK_W + key_w + 2 + if tag_w > 0 { tag_w + 1 } else { 0 });

    for (n, row) in rows.iter().skip(start).take(list_h).enumerate() {
        let y = inner.y + 2 + n as u16;
        let (i, b) = match row {
            Row::Blank => continue,
            Row::Head(g) => {
                // The heading uses the language of the help overlay, so the
                // two lists of commands look like one list.
                buf.set_stringn(
                    inner.x,
                    y,
                    text::fit(g, inner.width as usize, Align::Left),
                    inner.width as usize,
                    p.bold(p.on(t.accent, t.bg_alt)),
                );
                continue;
            }
            Row::Item(i, b) => (*i, *b),
        };
        // A heading and an empty row are not commands, so only these rows go
        // to the mouse. The list also scrolls, and the position on the screen
        // is therefore not the position in the list.
        hit.line(y, i);
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
            if selected { "▸ " } else { "  " },
            MARK_W,
            p.bold(p.on(t.accent, bg)),
        );
        // The label goes to the right side of its column. The keys are short,
        // so the two columns of text stay close to each other.
        buf.set_stringn(
            inner.x + MARK_W as u16,
            y,
            text::fit(b.label, key_w, Align::Right),
            key_w,
            p.on(t.key_fg, bg),
        );

        let hits = match_positions(&query, b.desc);
        let base = if selected {
            p.bold(p.on(t.fg, bg))
        } else {
            p.on(t.fg, bg)
        };
        draw_desc(
            buf,
            Position::new(desc_x, y),
            desc_w,
            b.desc,
            hits.as_deref().unwrap_or(&[]),
            (base, p.bold(p.on(t.accent, bg))),
        );

        if tag_w > 0 {
            // A row can be in the list because the query matches the name of
            // the group. The tag then takes the color of a match, because no
            // character of the description carries the mark.
            let tag_style = if hits.is_none() && commands::fuzzy_match(&query, b.group) {
                p.on(t.accent, bg)
            } else {
                p.on(t.dim, bg)
            };
            buf.set_stringn(
                inner.right() - tag_w as u16,
                y,
                text::fit(b.group, tag_w, Align::Right),
                tag_w,
                tag_style,
            );
        }
    }

    hints(buf, outer, app, p, " ↑↓ select · Enter run · Esc closes ");
    hit
}

/// Draws the theme picker.
pub fn draw_theme_picker(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> OverlayHit {
    let outer = centered(area, 60, 70, 60);
    let inner = frame(buf, outer, "theme", app, p);
    let t = &app.theme;
    let mut hit = OverlayHit::new(app.mode, outer);

    let list_h = inner.height.saturating_sub(1) as usize;
    let sel = app.theme_sel.min(app.themes.len().saturating_sub(1));
    let start = window_start(sel, app.themes.len(), list_h);

    for (i, theme) in app.themes.iter().skip(start).take(list_h).enumerate() {
        let y = inner.y + i as u16;
        hit.line(y, start + i);
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
    hit
}

/// Draws the cell inspector with the complete value of one cell.
pub fn draw_cell(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> OverlayHit {
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
    // The inspector holds one value and no list. A click inside it therefore
    // selects nothing.
    OverlayHit::new(app.mode, outer)
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
pub fn draw_record(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> Drawn {
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
    let mut hit = OverlayHit::new(app.mode, outer);
    if inner.width < 10 || inner.height < 2 {
        return Drawn { hit, cursor: None };
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
        return Drawn { hit, cursor };
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
        return Drawn { hit, cursor };
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
        return Drawn { hit, cursor };
    }

    let name_w = ((inner.width as usize) * 2 / 5).clamp(8, 44);
    let type_w = if (inner.width as usize) > name_w + 32 { 11 } else { 0 };
    let value_x = inner.x + (name_w + 1 + type_w) as u16 + if type_w > 0 { 1 } else { 0 };
    let value_w =
        (inner.width as usize).saturating_sub(name_w + 1 + type_w + if type_w > 0 { 1 } else { 0 });

    let sel = app.record_sel.min(lines.len() - 1);
    let start = window_start(sel, lines.len(), list_h);

    for (i, line) in lines.iter().skip(start).take(list_h).enumerate() {
        let y = top + i as u16;
        // The record of one row can hold hundreds of lines, so the list
        // scrolls. A click must find the line under the pointer, and the
        // position on the screen is not the position in the list.
        hit.line(y, start + i);
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
            //   long. A cut of it, such as `STRUCT(id ...`, says nothing that
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
    Drawn { hit, cursor }
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
pub fn draw_filter_build(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> Drawn {
    match app.build {
        Build::List => Drawn {
            hit: draw_build_list(buf, area, app, p),
            cursor: None,
        },
        Build::Column => draw_build_column(buf, area, app, p),
        Build::Op => Drawn {
            hit: draw_build_op(buf, area, app, p),
            cursor: None,
        },
        Build::Value | Build::Value2 | Build::Raw => draw_build_value(buf, area, app, p),
    }
}

/// Draws the list of conditions.
fn draw_build_list(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> OverlayHit {
    let outer = centered(area, 80, 70, 96);
    let inner = frame(buf, outer, "filter", app, p);
    let t = &app.theme;
    let mut hit = OverlayHit::new(app.mode, outer);
    if inner.width < 10 || inner.height < 2 {
        return hit;
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
        return hit;
    }

    // Keep the last two rows for the compiled expression.
    let list_h = (inner.height as usize).saturating_sub(2).max(1);
    let sel = app.build_sel.min(app.fset.conditions.len() - 1);
    let start = window_start(sel, app.fset.conditions.len(), list_h);

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
        hit.line(y, at);
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
    hit
}

/// Draws the list of columns.
fn draw_build_column(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> Drawn {
    let outer = centered(area, 70, 76, 76);
    let inner = frame(buf, outer, "filter · which column?", app, p);
    let t = &app.theme;
    let mut hit = OverlayHit::new(app.mode, outer);
    if inner.width < 10 || inner.height < 3 {
        return Drawn { hit, cursor: None };
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
        return Drawn {
            hit,
            cursor: Some(cursor),
        };
    }
    let sel = app.pick_sel.min(cols.len() - 1);
    let start = window_start(sel, cols.len(), list_h);
    let name_w = ((inner.width as usize) * 3 / 5).max(8);

    for (i, c) in cols.iter().skip(start).take(list_h).enumerate() {
        let y = inner.y + 2 + i as u16;
        hit.line(y, start + i);
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
    Drawn {
        hit,
        cursor: Some(cursor),
    }
}

/// Draws the list of operators.
fn draw_build_op(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> OverlayHit {
    let name = app
        .schema
        .columns
        .get(app.draft.col)
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let outer = centered(area, 66, 70, 72);
    let inner = frame(buf, outer, &format!("filter · {name} …"), app, p);
    let t = &app.theme;
    let mut hit = OverlayHit::new(app.mode, outer);
    if inner.width < 10 || inner.height < 1 {
        return hit;
    }

    let ops = app.build_ops();
    let list_h = inner.height as usize;
    let sel = app.pick_sel.min(ops.len().saturating_sub(1));
    let start = window_start(sel, ops.len(), list_h);
    let op_w = 18usize.min((inner.width as usize) / 2);

    for (i, op) in ops.iter().skip(start).take(list_h).enumerate() {
        let y = inner.y + i as u16;
        hit.line(y, start + i);
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
    hit
}

/// Draws the step that takes a value, and the step that takes a complete
/// `WHERE` expression.
fn draw_build_value(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> Drawn {
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
    // A value step holds a prompt and no list. A click inside it therefore
    // selects nothing, and a click outside it goes back one step.
    let hit = OverlayHit::new(app.mode, outer);
    if inner.width < 8 || inner.height < 1 {
        return Drawn { hit, cursor: None };
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
    Drawn { hit, cursor: at }
}

/// Draws the settings page.
///
/// The page has two halves. The upper half holds the settings that the user
/// can change. The lower half holds what the machine gives and what DuckDB
/// uses now. A user who sets a memory limit needs to know how much memory the
/// machine has, and a user who sets the threads needs to know how many cores
/// it has. Without those numbers the user is guessing.
pub fn draw_settings(buf: &mut Buffer, area: Rect, app: &App, p: &Paint) -> Drawn {
    let outer = centered(area, 80, 88, 88);
    // Every change writes the file at once, so the title needs no mark for
    // a change that the user did not keep.
    let inner = frame(buf, outer, "settings · kept as you change them", app, p);
    let t = &app.theme;
    let mut hit = OverlayHit::new(app.mode, outer);
    if inner.width < 20 || inner.height < 4 {
        return Drawn { hit, cursor: None };
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
        // The lower half of the page holds facts about the machine, and no
        // setting. Only these rows go to the mouse.
        hit.line(y, i);
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
    Drawn { hit, cursor }
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
    use crate::colors::Depth;

    /// Opens a small file and puts the program in the palette.
    ///
    /// The palette needs the true state of the program, because it reads the
    /// list of matches from it.
    fn palette_app(tag: &str) -> App {
        let dir = std::env::temp_dir().join(format!("peruse-overlays-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.csv");
        std::fs::write(&path, "id,name\n1,alice\n2,bob\n").unwrap();
        let (worker, opened) = peruse_core::Worker::spawn(
            path.to_str().unwrap(),
            peruse_core::OpenOptions::default(),
        )
        .unwrap();
        let mut app = App::new(
            worker,
            opened,
            peruse_core::theme::builtin("peruse-dark").unwrap(),
            false,
        );
        // A test must never write the settings of the user who runs it.
        app.config_path = Some(dir.join("config.toml"));
        app.run(Cmd::Palette);
        app
    }

    /// Gives the characters of a buffer as one text, one line for each row.
    fn dump(buf: &Buffer) -> String {
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

    #[test]
    fn the_marked_characters_agree_with_the_match_of_the_palette() {
        // The palette marks the characters of a description that the query
        // matched. A mark on a row that does not match, or no mark on a row
        // that matches, would tell the user the wrong reason for the list.
        for b in BINDINGS {
            for q in ["", "t", "theme", "sort", "zzz", "next row", "TP", "→", "col"] {
                let got = match_positions(q, b.desc);
                assert_eq!(
                    got.is_some(),
                    commands::fuzzy_match(q, b.desc),
                    "{q:?} against {:?}",
                    b.desc
                );
                let mut last: Option<usize> = None;
                for i in got.unwrap_or_default() {
                    assert!(i < b.desc.len(), "{i} is past {:?}", b.desc);
                    assert!(b.desc.is_char_boundary(i), "{i} splits {:?}", b.desc);
                    if let Some(l) = last {
                        assert!(i > l, "{i} comes after {l} in {:?}", b.desc);
                    }
                    last = Some(i);
                }
            }
        }
    }

    #[test]
    fn a_query_with_no_ascii_character_gives_positions_on_character_limits() {
        // The palette cuts the description at these positions to color one
        // character. A position inside a character stops the program.
        let hays = [
            "sort by this column (asc → desc → off)",
            "next theme",
            "日本 theme",
            "\u{130}stanbul",
        ];
        for q in ["é", "→", "日本", "\u{130}", "ß", "…", "  ", "t→"] {
            for hay in hays {
                let got = match_positions(q, hay);
                assert_eq!(
                    got.is_some(),
                    commands::fuzzy_match(q, hay),
                    "{q:?} against {hay:?}"
                );
                for i in got.unwrap_or_default() {
                    assert!(hay.is_char_boundary(i), "{i} splits {hay:?}");
                }
            }
        }
    }

    #[test]
    fn the_palette_box_stays_inside_a_very_wide_terminal() {
        // The value width * 80 is more than a 16-bit number can hold when the
        // terminal has more than 819 columns.
        for (w, h) in [(1000u16, 60u16), (2000, 200), (65535, 65535), (20, 6)] {
            let area = Rect { x: 0, y: 0, width: w, height: h };
            let r = palette_rect(area);
            assert!(r.right() <= area.right(), "right edge outside {w}");
            assert!(r.bottom() <= area.bottom(), "bottom edge outside {h}");
        }
    }

    #[test]
    fn the_palette_draws_inside_a_small_and_a_large_terminal() {
        // A box on a small terminal holds no row of the list, and a box on a
        // large terminal holds each row. Both must draw inside the buffer.
        let mut app = palette_app("size");
        let p = Paint::new(Depth::True);
        for (w, h) in [(20u16, 6u16), (40, 10), (300, 80)] {
            let area = Rect::new(0, 0, w, h);
            for q in ["", "theme", "zzz"] {
                app.input.set(q);
                let mut buf = Buffer::empty(area);
                draw_palette(&mut buf, area, &app, &p);
                // A write outside the buffer stops the program. The area and
                // the number of cells therefore say that each write landed
                // inside the screen.
                assert_eq!(buf.area, area, "{q:?} on {w}x{h} changed the buffer");
                assert_eq!(
                    buf.content.len(),
                    w as usize * h as usize,
                    "{q:?} on {w}x{h} changed the number of cells"
                );
            }
        }
    }

    #[test]
    fn each_family_of_values_has_a_line_in_the_help() {
        for kind in [
            CellKind::Number,
            CellKind::Text,
            CellKind::Bool,
            CellKind::Temporal,
            CellKind::Binary,
            CellKind::Nested,
        ] {
            // A new family of values must come into this match as well. The
            // match has no wildcard, so the compiler asks for it, and the test
            // then asks for the line in the help.
            match kind {
                CellKind::Number
                | CellKind::Text
                | CellKind::Bool
                | CellKind::Temporal
                | CellKind::Binary
                | CellKind::Nested => {}
            }
            let mark = kind.badge().to_string();
            assert!(
                TYPE_MARKS.iter().any(|(k, _)| *k == mark),
                "the help says nothing about the mark {mark:?} of {kind:?}"
            );
        }
    }

    #[test]
    fn the_help_names_the_keys_of_a_mac_that_move_one_word() {
        // Alt with an arrow moves the cursor one word, because the Option key
        // of a Mac sends Alt. The editor in `input.rs` takes Ctrl and Alt with
        // an arrow, and Alt+B and Alt+F. A key that no page names is a key that
        // nobody presses, so the row names each of the four forms.
        let app = palette_app("help-word");
        let p = Paint::new(Depth::True);
        // This is the widest box of the help. A description that does not fit
        // here is cut, and the cut takes the last keys of the row away.
        let area = Rect::new(0, 0, 200, 300);
        let mut buf = Buffer::empty(area);
        draw_help(&mut buf, area, &app, &p);
        let s = dump(&buf);
        let row = s
            .lines()
            .find(|l| l.contains("^←"))
            .unwrap_or_else(|| panic!("the help has no row for the word keys\n{s}"));
        for key in ["Alt+←/→", "Alt+B", "Alt+F"] {
            assert!(row.contains(key), "the help does not name {key}:\n{row}");
        }
        assert!(!row.contains('…'), "the widest box cuts the row:\n{row}");
    }

    #[test]
    fn the_commands_of_one_group_follow_each_other_in_the_table() {
        // The palette walks the table one time and writes a heading at each
        // change of group. A group in two parts would therefore give the same
        // heading two times. The help overlay reads the table one group at a
        // time, so only the palette sees this rule.
        let mut seen: Vec<&str> = Vec::new();
        let mut group = "";
        for b in BINDINGS {
            if b.group == group {
                continue;
            }
            assert!(
                !seen.contains(&b.group),
                "the group {:?} comes back after {group:?}",
                b.group
            );
            seen.push(b.group);
            group = b.group;
        }
    }

    #[test]
    fn the_prompt_of_the_palette_is_where_the_caller_puts_the_cursor() {
        // [`palette_rect`] says that the text of the prompt starts at the
        // column `x + 4` on the row `y + 1`, and `ui::draw` puts the terminal
        // cursor there. A change to the box or to the mark of the prompt must
        // therefore change the two places together.
        let mut app = palette_app("cursor");
        let p = Paint::new(Depth::True);
        app.input.set("zq");
        for (w, h) in [(110u16, 24u16), (60, 12), (300, 80)] {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            draw_palette(&mut buf, area, &app, &p);
            let r = palette_rect(area);
            assert_eq!(
                (buf[(r.x + 4, r.y + 1)].symbol(), buf[(r.x + 5, r.y + 1)].symbol()),
                ("z", "q"),
                "the prompt is not at x + 4 on y + 1 of a {w}x{h} screen\n{}",
                dump(&buf)
            );
        }
    }

    #[test]
    fn the_palette_groups_the_full_list_and_counts_a_short_one() {
        let mut app = palette_app("text");
        let p = Paint::new(Depth::True);
        let area = Rect::new(0, 0, 110, 24);

        let mut buf = Buffer::empty(area);
        draw_palette(&mut buf, area, &app, &p);
        let s = dump(&buf);
        // A heading is alone on its row. The name of a group is also the tag at
        // the end of each row, so a test of the text alone proves nothing.
        assert!(
            s.lines()
                .any(|l| l.trim_matches(|c: char| !c.is_alphanumeric()) == "Move"),
            "no group heading in the full list\n{s}"
        );
        assert!(s.contains("▸"), "no mark on the selected row\n{s}");
        assert!(
            s.contains(&format!("{} commands", BINDINGS.len())),
            "no count of the commands\n{s}"
        );

        app.input.set("theme");
        let mut buf = Buffer::empty(area);
        draw_palette(&mut buf, area, &app, &p);
        let s = dump(&buf);
        let n = app.palette_items().len();
        assert!(n > 0 && n < BINDINGS.len(), "the query filtered nothing");
        assert!(
            s.contains(&format!("{n} of {} commands", BINDINGS.len())),
            "no count of the matches\n{s}"
        );
        // The description stays on the screen in full, because the user reads
        // the description and not the key.
        assert!(s.contains("next theme"), "the description is cut\n{s}");
    }

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

    #[test]
    fn a_window_always_holds_the_selected_item() {
        for len in [1usize, 2, 5, 20, 53, 100] {
            for window in [1usize, 2, 4, 15, 60] {
                // A caller always clamps the selection to the list first, so a
                // position past the last item is not an input that can happen.
                for sel in [0, 1, len / 2, len - 1].into_iter().filter(|s| *s < len) {
                    let start = window_start(sel, len, window);
                    let at = format!("len {len} window {window} sel {sel}");
                    assert!(start <= sel, "the item is above the window: {at}");
                    assert!(sel < start + window, "the item is below the window: {at}");
                    if len > window {
                        assert!(
                            start + window <= len,
                            "the window goes past the last item: {at}"
                        );
                    } else {
                        assert_eq!(start, 0, "a list that fits needs no scroll: {at}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_window_shows_the_items_after_the_selected_one() {
        // This is the reason that the function exists. A window that keeps the
        // selected item on its last row shows nothing after it, and a list of
        // 53 commands then reads as a list of one.
        let (sel, len, window) = (20usize, 53usize, 15usize);
        let start = window_start(sel, len, window);
        assert!(
            start + window > sel + 1,
            "no item follows the selected one: start {start}"
        );
        assert!(start < sel, "no item comes before the selected one");
    }

    #[test]
    fn a_window_of_no_rows_starts_at_the_first_item() {
        // A box that is too short for one row must not divide by zero, and must
        // not give a start that is past the end of the list.
        assert_eq!(window_start(9, 20, 0), 0);
        assert_eq!(window_start(0, 0, 5), 0);
        assert_eq!(window_start(9, 0, 0), 0);
    }
}
