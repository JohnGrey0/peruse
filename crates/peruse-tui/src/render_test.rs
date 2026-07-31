//! The tests of the complete program: a true file, a true engine thread and a
//! true frame.
//!
//! These tests use the same path through the code as the program:
//! `Worker::spawn`, then `App`, then `ui::draw`. The frame goes to the
//! `TestBackend` of ratatui. Each test therefore examines the characters on
//! the screen, and not the state between the steps.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use peruse_core::query::{Base, SortDir, SortKey};
use peruse_core::{OpenOptions, Worker};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::app::{App, Band, Build, Mode, Panel, PromptKind};
use crate::colors::Depth;
use crate::commands::Cmd;
use crate::ui;

const SAMPLE: &str = "id,name,amount,region\n\
                      1,alice,10.5,EU\n\
                      2,bob,,US\n\
                      3,carol,30.25,EU\n\
                      4,dave,4000.75,APAC\n\
                      5,erin,7.0,EU\n";

/// Writes a test file in a new directory and gives its path.
fn write_sample(tag: &str, body: &str, ext: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("peruse-render-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("data.{ext}"));
    std::fs::File::create(&p)
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    p
}

/// Opens a file and makes a terminal for the test.
///
/// The settings go to a file beside the data of the test. The program keeps a
/// setting as soon as the user changes it, and a test must never write the
/// settings of the user who runs it.
fn open(path: &Path) -> (App, Terminal<TestBackend>) {
    let (worker, opened) = Worker::spawn(path.to_str().unwrap(), OpenOptions::default()).unwrap();
    let mut app = App::new(
        worker,
        opened,
        peruse_core::theme::builtin("peruse-dark").unwrap(),
        false,
    );
    app.config_path = path.parent().map(|d| d.join("config.toml"));
    let terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
    (app, terminal)
}

/// Draws frames and adds the engine responses until no response arrives.
///
/// The function waits for some empty periods, and not for one. A query for the
/// statistics of a column can need some hundred milliseconds. After one empty
/// period, a test can therefore examine a screen with one part of the data.
fn settle(app: &mut App, term: &mut Terminal<TestBackend>) {
    let rx = app.worker.responses().clone();
    let mut quiet = 0;
    for _ in 0..80 {
        term.draw(|f| ui::draw(f, app, Depth::True)).unwrap();
        app.ensure_rows();
        let mut got = false;
        while let Ok(r) = rx.recv_timeout(Duration::from_millis(75)) {
            app.on_response(r);
            got = true;
        }
        quiet = if got { 0 } else { quiet + 1 };
        if quiet >= 4 {
            break;
        }
    }
    term.draw(|f| ui::draw(f, app, Depth::True)).unwrap();
}

/// Gives the characters of the screen, one text for each row.
fn lines(term: &Terminal<TestBackend>) -> Vec<String> {
    let buf = term.backend().buffer();
    let a = buf.area;
    (a.top()..a.bottom())
        .map(|y| {
            (a.left()..a.right())
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

/// Gives the characters of the screen as one text.
fn screen(term: &Terminal<TestBackend>) -> String {
    lines(term).join("\n")
}

#[test]
fn grid_shows_headers_row_numbers_and_values() {
    let p = write_sample("basic", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    let s = screen(&term);

    for header in ["id", "name", "amount", "region"] {
        assert!(s.contains(header), "missing header {header}\n{s}");
    }
    for value in ["alice", "bob", "carol", "4000.75", "APAC"] {
        assert!(s.contains(value), "missing value {value}\n{s}");
    }
    // The first row has the number 1.
    let rows = lines(&term);
    assert!(
        rows.iter().any(|l| l.trim_start().starts_with('1')),
        "no row numbers\n{s}"
    );
    // The title bar shows the size of the view after the count arrives.
    assert!(s.contains("5 × 4"), "shape missing from title\n{s}");
    assert!(s.contains("data.csv"), "filename missing\n{s}");
}

#[test]
fn null_is_shown_as_null_not_as_blank() {
    let p = write_sample("nulls", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    assert!(
        screen(&term).contains("NULL"),
        "empty CSV field should render as NULL\n{}",
        screen(&term)
    );
}

#[test]
fn status_line_describes_the_focused_column_and_row() {
    let p = write_sample("status", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::RowDown);
    app.run(Cmd::ColRight);
    settle(&mut app, &mut term);
    let s = screen(&term);

    assert!(s.contains("name"), "column name not in status\n{s}");
    assert!(s.contains("VARCHAR"), "column type not in status\n{s}");
    assert!(s.contains("row 2/5"), "row position not in status\n{s}");
}

#[test]
fn footer_always_advertises_help() {
    let p = write_sample("footer", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    let last = lines(&term).pop().unwrap();
    assert!(last.contains('?'), "footer must offer help: {last:?}");
    assert!(last.contains("quit"), "footer must offer quit: {last:?}");
}

#[test]
fn filtering_narrows_the_grid_and_flags_the_title() {
    let p = write_sample("filter", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.view.filter = Some("region = 'EU'".into());
    app.run_startup_view();
    settle(&mut app, &mut term);
    let s = screen(&term);

    assert!(s.contains("alice"), "kept row missing\n{s}");
    assert!(!s.contains("dave"), "filtered-out row still shown\n{s}");
    // The title shows the expression, and not the word "filtered" alone. The
    // user can then read the condition without a press of a key.
    assert!(
        s.contains("filter: region = 'EU'"),
        "title does not show the filter\n{s}"
    );
    assert!(s.contains("3 × 4"), "shape not recounted\n{s}");
}

#[test]
fn a_filter_matching_nothing_says_so_and_offers_the_way_out() {
    let p = write_sample("empty", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.view.filter = Some("region = 'NOWHERE'".into());
    app.run_startup_view();
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert!(s.contains("no rows match"), "no empty-state message\n{s}");
    assert!(s.contains("F clears it"), "no way out offered\n{s}");
}

#[test]
fn sorting_reorders_rows_and_marks_the_header() {
    let p = write_sample("sort", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.view.sort = vec![SortKey {
        column: "amount".into(),
        dir: SortDir::Desc,
    }];
    app.run_startup_view();
    settle(&mut app, &mut term);
    let s = screen(&term);

    let dave = s.find("dave").expect("dave present");
    let alice = s.find("alice").expect("alice present");
    assert!(dave < alice, "4000.75 should sort above 10.5\n{s}");
    assert!(s.contains('▼'), "no sort arrow in header\n{s}");
}

#[test]
fn a_sql_query_replaces_the_grid_contents() {
    let p = write_sample("sql", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.view.base = Base::Sql(
        "SELECT region, count(*) AS n FROM src GROUP BY 1 ORDER BY region".into(),
    );
    app.run_startup_view();
    settle(&mut app, &mut term);
    let s = screen(&term);

    assert!(s.contains("region"), "projected column missing\n{s}");
    assert!(s.contains('n'), "aggregate column missing\n{s}");
    assert!(!s.contains("alice"), "old columns still shown\n{s}");
    assert!(s.contains("query"), "title does not flag the query\n{s}");
    assert!(s.contains("3 × 2"), "shape not updated\n{s}");
}

#[test]
fn the_query_prompt_opens_with_a_statement_that_the_user_finishes() {
    let p = write_sample("sql-prompt", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    // The key `e` opens the prompt with the start of a statement, and the
    // cursor comes after the last character. A user who wants a part of the
    // file types the condition and nothing else.
    press_char(&mut app, 'e');
    assert_eq!(app.mode, Mode::Prompt(PromptKind::Sql));
    assert_eq!(app.input.text(), peruse_core::query::PROMPT_START);
    assert_eq!(app.input.cursor_col(), app.input.text().chars().count());
    // The text reads and does not write, so the prompt reports no error for it.
    assert_eq!(app.prompt_error, None, "the prompt must not open with an error");
    settle(&mut app, &mut term);
    assert!(
        screen(&term).contains("SELECT * FROM src WHERE"),
        "the statement is not on the screen\n{}",
        screen(&term)
    );

    type_text(&mut app, "region = 'EU'");
    assert_eq!(app.prompt_error, None, "a statement that reads is not an error");
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert!(s.contains("3 × 4"), "the statement did not run\n{s}");
    assert!(!s.contains("dave"), "a row outside the EU is still there\n{s}");
}

#[test]
fn the_query_prompt_opens_with_the_statement_that_the_grid_shows() {
    let p = write_sample("sql-prompt-again", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    app.view.base = Base::Sql("SELECT id, name FROM src".into());
    app.run_startup_view();
    settle(&mut app, &mut term);

    // The grid holds a statement already, so the prompt opens with it. The user
    // corrects that statement, and does not write it again.
    press_char(&mut app, 'e');
    assert_eq!(app.input.text(), "SELECT id, name FROM src");
}

#[test]
fn the_key_ctrl_u_empties_the_query_prompt_and_esc_leaves_the_grid_alone() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let p = write_sample("sql-prompt-clear", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    // The title bar, the column names and the five rows of data.
    let before = lines(&term)[..7].to_vec();

    // The key ^U is the way out for a user who wants another statement.
    press_char(&mut app, 'e');
    app.on_key(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert!(app.input.is_empty(), "^U must give an empty line");

    // Esc changes nothing: the grid still reads the file.
    type_text(&mut app, "SELECT 1 AS x");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.view.base, Base::Source, "Esc must not run the statement");
    settle(&mut app, &mut term);
    assert_eq!(lines(&term)[..7], before[..], "Esc changed the grid");
}

#[test]
fn hiding_a_column_removes_it_and_the_title_says_how_many() {
    let p = write_sample("hide", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    assert!(screen(&term).contains("alice"));

    app.run(Cmd::ColRight); // move the cursor to the column `name`
    app.run(Cmd::HideColumn);
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert!(!s.contains("alice"), "hidden column still drawn\n{s}");
    assert!(s.contains("1 hidden"), "title does not report it\n{s}");

    app.run(Cmd::ShowAllColumns);
    settle(&mut app, &mut term);
    assert!(screen(&term).contains("alice"), "X did not restore the column");
}

#[test]
fn help_overlay_is_generated_from_the_binding_table() {
    let p = write_sample("help", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::Help);
    settle(&mut app, &mut term);
    let s = screen(&term);

    assert_eq!(app.mode, Mode::Help);
    assert!(s.contains("Move"), "group heading missing\n{s}");
    assert!(s.contains("next row"), "binding description missing\n{s}");
    assert!(s.contains("j/k to scroll"), "long help must offer scrolling\n{s}");

    // The overlay is higher than the terminal. A scroll gives the end.
    app.help_scroll = u16::MAX;
    settle(&mut app, &mut term);
    let end = screen(&term);
    assert!(end.contains("read-only"), "read-only note missing at the end\n{end}");
    assert!(end.contains("OSC 52"), "clipboard note missing at the end\n{end}");
}

#[test]
fn command_palette_filters_as_you_type() {
    let p = write_sample("palette", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::Palette);
    assert_eq!(app.mode, Mode::Palette);
    let all = app.palette_items().len();
    assert!(all > 20, "palette should list everything by default");

    for c in "theme".chars() {
        app.on_key(&ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char(c),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
    }
    let filtered = app.palette_items();
    assert!(filtered.len() < all && !filtered.is_empty(), "typing did not filter");
    assert!(filtered.contains(&Cmd::ThemeNext));

    settle(&mut app, &mut term);
    assert!(screen(&term).contains("next theme"), "palette not drawn");
}

#[test]
fn metadata_panel_reports_the_sniffed_csv_dialect() {
    let p = write_sample("meta", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::ToggleMeta);
    settle(&mut app, &mut term);
    let s = screen(&term);

    assert_eq!(app.panel, Panel::Meta);
    assert!(s.contains("metadata"), "panel title missing\n{s}");
    assert!(s.contains("delimiter"), "dialect not reported\n{s}");
    assert!(s.contains("read_csv"), "reproducible read call missing\n{s}");
}

#[test]
fn the_height_of_the_statistics_agrees_with_the_rows_that_it_draws() {
    // `stats_content_height` and `draw_stats` walk the same list of sections,
    // and until now they agreed only by a comment. A disagreement gives the
    // stacked view a panel with an empty row at the bottom, or a panel that cuts
    // its last row, and the whole test suite could not fail on it. One such
    // disagreement was real: a branch of `draw_stats` did not step past the row
    // that it wrote.
    //
    // The test draws the panel into an area that is taller than any content, so
    // nothing is cut, and then compares the last row that holds a character with
    // the height that the function reports.
    let p = write_sample("stats-height", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    app.panel = Panel::Stats;
    settle(&mut app, &mut term);

    let paint = crate::paint::Paint::new(Depth::True);
    for filter in [None, Some("region = 'EU'".to_string())] {
        app.view.filter = filter.clone();
        settle(&mut app, &mut term);
        // Walk every column, because each family of values gives the panel a
        // different set of sections: only a column of numbers has a chart.
        for col in 0..app.schema.len() {
            app.cursor_col = col;
            settle(&mut app, &mut term);
            let name = app.schema.columns[col].name.clone();

            let area = ratatui::layout::Rect { x: 0, y: 0, width: 46, height: 60 };
            let mut buf = ratatui::buffer::Buffer::empty(area);
            crate::panels::draw_stats(&mut buf, area, &app, &paint);

            // The border takes the first row and the last row, and it writes a
            // line across the whole width of each of them. The content is
            // therefore inside the rows 1 to height - 2.
            let last_written = (1..area.height - 1)
                .rev()
                .find(|y| {
                    (1..area.width - 1).any(|x| buf[(x, *y)].symbol().trim() != "")
                })
                .expect("the panel wrote nothing");
            let want = crate::panels::stats_content_height(&app);
            assert_eq!(
                last_written, want,
                "column {name}, filter {filter:?}: the panel wrote up to the row \
                 {last_written} and the height says {want}"
            );
        }
    }
}

#[test]
fn stats_panel_summarises_the_focused_column() {
    let p = write_sample("stats", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::ColRight);
    app.run(Cmd::ColRight); // move the cursor to the column `amount`
    app.run(Cmd::ToggleStats);
    settle(&mut app, &mut term);
    let s = screen(&term);

    assert!(s.contains("nulls"), "null count missing\n{s}");
    assert!(s.contains("distinct"), "distinct count missing\n{s}");
    assert!(s.contains("4000.75"), "max value missing\n{s}");
    assert!(s.contains("distribution"), "histogram missing for a number\n{s}");
}

#[test]
fn search_moves_the_cursor_to_the_match() {
    let p = write_sample("search", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.needle = "carol".into();
    app.run(Cmd::SearchNext);
    settle(&mut app, &mut term);

    assert_eq!(app.cursor_row, 2, "cursor did not land on carol's row");
    assert!(
        screen(&term).contains("match 1/1"),
        "hit count not reported\n{}",
        screen(&term)
    );

    // After the message goes away, the status line shows the position again.
    app.on_key(&ratatui::crossterm::event::KeyEvent::new(
        ratatui::crossterm::event::KeyCode::Char('z'), // no command: it only removes the message
        ratatui::crossterm::event::KeyModifiers::NONE,
    ));
    settle(&mut app, &mut term);
    assert!(screen(&term).contains("row 3/5"), "position not shown\n{}", screen(&term));
}

/// Types a search value with the keys, in the same way as a user. The function
/// does not write to `app.needle`.
fn type_search(app: &mut App, term: &mut Terminal<TestBackend>, needle: &str) {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.run(Cmd::Search);
    for c in needle.chars() {
        app.on_key(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    settle(app, term);
}

#[test]
fn a_freshly_typed_search_matches_the_row_under_the_cursor() {
    let p = write_sample("search-inclusive", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::RowDown); // row 2 holds bob
    assert_eq!(app.cursor_row, 1);
    type_search(&mut app, &mut term, "bob");
    assert_eq!(app.cursor_row, 1, "should stay put, not skip to the next match");
}

#[test]
fn n_and_shift_n_walk_matches_and_wrap_at_the_ends() {
    // The value `EU` is in the rows 1, 3 and 5.
    let p = write_sample("search-walk", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    type_search(&mut app, &mut term, "EU");
    assert_eq!(app.cursor_row, 0);

    app.run(Cmd::SearchNext);
    settle(&mut app, &mut term);
    assert_eq!(app.cursor_row, 2);

    app.run(Cmd::SearchNext);
    settle(&mut app, &mut term);
    assert_eq!(app.cursor_row, 4);

    // After the last match, the search goes to the first match.
    app.run(Cmd::SearchNext);
    settle(&mut app, &mut term);
    assert_eq!(app.cursor_row, 0, "did not wrap forward");
    assert!(screen(&term).contains("wrapped"), "wrap not reported");

    // A search up the view from the first match goes to the last match.
    app.run(Cmd::SearchPrev);
    settle(&mut app, &mut term);
    assert_eq!(app.cursor_row, 4, "did not wrap backward");

    app.run(Cmd::SearchPrev);
    settle(&mut app, &mut term);
    assert_eq!(app.cursor_row, 2);
}

#[test]
fn a_search_with_no_match_says_so_and_leaves_the_cursor_alone() {
    let p = write_sample("search-miss", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::RowDown);

    type_search(&mut app, &mut term, "zebra");
    assert_eq!(app.cursor_row, 1, "cursor moved on a failed search");
    assert!(
        screen(&term).contains("no match"),
        "failure not reported\n{}",
        screen(&term)
    );
}

#[test]
fn changing_the_view_discards_stale_match_offsets() {
    let p = write_sample("search-stale", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    type_search(&mut app, &mut term, "EU");
    assert!(!app.hits.is_empty());

    // A match offset is a position in the old view. A new sort therefore
    // makes each offset wrong.
    app.view.sort = vec![SortKey {
        column: "id".into(),
        dir: SortDir::Desc,
    }];
    app.run_startup_view();
    settle(&mut app, &mut term);
    assert!(app.hits.is_empty(), "stale offsets kept across a view change");
}

#[test]
fn cell_inspector_shows_a_value_too_wide_for_the_grid() {
    let long = "y".repeat(600);
    let p = write_sample("cell", &format!("a,b\n1,{long}\n"), "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::ColRight);
    app.run(Cmd::InspectCell);
    settle(&mut app, &mut term);
    let s = screen(&term);

    assert_eq!(app.mode, Mode::Cell);
    assert!(s.contains("600 chars"), "length not reported\n{s}");
    assert!(s.contains("row 1"), "row not identified\n{s}");
    // The value goes across some lines. The inspector does not cut it.
    assert!(s.matches("yyyyyyyyyy").count() > 3, "value not wrapped\n{s}");
}

#[test]
fn wide_files_scroll_horizontally_and_signal_more_columns() {
    let header: Vec<String> = (0..40).map(|i| format!("column_{i:02}")).collect();
    let row: Vec<String> = (0..40).map(|i| format!("v{i}")).collect();
    let body = format!("{}\n{}\n", header.join(","), row.join(","));
    let p = write_sample("wide", &body, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    assert!(screen(&term).contains('›'), "no more-columns indicator");
    assert!(screen(&term).contains("column_00"));

    for _ in 0..30 {
        app.run(Cmd::ColRight);
    }
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert!(s.contains("column_30"), "cursor column not scrolled into view\n{s}");
    assert!(s.contains('‹'), "no columns-to-the-left indicator\n{s}");
}

#[test]
fn a_bad_filter_reports_an_error_and_keeps_the_previous_rows() {
    let p = write_sample("badfilter", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.view.filter = Some("no_such_column > 1".into());
    app.run_startup_view();
    settle(&mut app, &mut term);
    let s = screen(&term);

    assert!(s.contains('✕'), "no error indicator\n{s}");
    assert!(
        s.to_lowercase().contains("no_such_column"),
        "error does not name the problem\n{s}"
    );
}

#[test]
fn csv_without_an_index_advertises_how_to_get_one() {
    let p = write_sample("hint", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    assert!(!app.seekable);
    assert!(
        screen(&term).contains("press I to index"),
        "no indexing hint for a streamed CSV"
    );

    app.run(Cmd::IndexCsv);
    settle(&mut app, &mut term);
    assert!(app.seekable, "indexing did not complete");
    assert!(!screen(&term).contains("press I to index"), "hint should be gone");
}

#[test]
fn theme_switching_repaints_without_disturbing_the_data() {
    let p = write_sample("theme", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    let before = app.theme.name.clone();

    app.run(Cmd::ThemeNext);
    settle(&mut app, &mut term);
    assert_ne!(app.theme.name, before);
    assert!(screen(&term).contains("alice"), "data lost on theme change");
    assert!(screen(&term).contains(&app.theme.name), "theme name not shown");
}

/// A tool for the developer. It prints true frames, so that a person can
/// examine the layout.
///
///     cargo test -p peruse-tui --bin peruse dump_screen -- --ignored --nocapture
///     PERUSE_DUMP=/path/to/file.parquet cargo test ... dump_screen -- --ignored --nocapture
#[test]
#[ignore = "prints a frame instead of asserting"]
fn dump_screen() {
    let path = match std::env::var("PERUSE_DUMP") {
        Ok(p) => PathBuf::from(p),
        Err(_) => write_sample("dump", SAMPLE, "csv"),
    };
    let (worker, opened) = Worker::spawn(path.to_str().unwrap(), OpenOptions::default()).unwrap();
    let mut app = App::new(
        worker,
        opened,
        peruse_core::theme::builtin("peruse-dark").unwrap(),
        true,
    );
    let mut term = Terminal::new(TestBackend::new(120, 26)).unwrap();
    settle(&mut app, &mut term);
    println!("\n--- grid ---\n{}", screen(&term));

    app.run(Cmd::ToggleMeta);
    settle(&mut app, &mut term);
    println!("\n--- metadata panel ---\n{}", screen(&term));

    app.run(Cmd::ToggleMeta);
    app.run(Cmd::ToggleStats);
    settle(&mut app, &mut term);
    println!("\n--- column stats ---\n{}", screen(&term));

    app.run(Cmd::ToggleStats);
    // Write the mode of the band into the settings of this application only.
    // The key `d` would write the settings file of the user who runs the test.
    for mode in ["compact", "detailed"] {
        app.config.band = Some(mode.to_string());
        settle(&mut app, &mut term);
        println!("\n--- band, {mode} ---\n{}", screen(&term));
    }
    app.config.band = None;

    app.run(Cmd::Help);
    settle(&mut app, &mut term);
    println!("\n--- help ---\n{}", screen(&term));
}

/// Gives one key to the application.
fn press(app: &mut App, code: ratatui::crossterm::event::KeyCode) {
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
    app.on_key(&KeyEvent::new(code, KeyModifiers::NONE));
}

/// Gives one character key to the application.
fn press_char(app: &mut App, c: char) {
    press(app, ratatui::crossterm::event::KeyCode::Char(c));
}

/// Types a text, one character at a time.
fn type_text(app: &mut App, s: &str) {
    for c in s.chars() {
        press_char(app, c);
    }
}

/// A file with many columns. The record view exists for this shape of file.
fn wide_sample() -> String {
    let names: Vec<String> = (0..60).map(|i| format!("col_{i:02}")).collect();
    let values: Vec<String> = (0..60).map(|i| format!("v{i}")).collect();
    format!("{}\n{}\n{}\n", names.join(","), values.join(","), values.join(","))
}

#[test]
fn the_record_view_shows_one_row_from_the_top_to_the_bottom() {
    let p = write_sample("record", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::RowDown); // go to alice's row, the second row of the file
    settle(&mut app, &mut term);
    app.run(Cmd::Record);
    settle(&mut app, &mut term);
    let s = screen(&term);

    // Each column of the row is on its own line, with its name and its type.
    for name in ["id", "name", "amount", "region"] {
        assert!(s.contains(name), "field {name} missing\n{s}");
    }
    assert!(s.contains("bob"), "value missing\n{s}");
    assert!(s.contains("record 2 of 5"), "position missing\n{s}");
    assert!(s.contains("VARCHAR"), "type missing\n{s}");
    // The empty amount of the second row is a NULL, and not an empty text.
    assert!(s.contains("NULL"), "NULL not marked\n{s}");
}

#[test]
fn the_record_view_finds_a_column_by_name_among_many() {
    let p = write_sample("record-find", &wide_sample(), "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Record);
    settle(&mut app, &mut term);

    // A screen of 24 rows cannot show 60 columns. The find box is the way to
    // the column that the user wants.
    press_char(&mut app, '/');
    type_text(&mut app, "col_57");
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert!(s.contains("col_57"), "the column found is not shown\n{s}");
    assert!(s.contains("v57"), "its value is not shown\n{s}");
    assert!(!s.contains("col_01"), "other columns are still shown\n{s}");
}

#[test]
fn the_record_view_steps_to_the_next_row_and_keeps_the_field() {
    let p = write_sample("record-step", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Record);
    settle(&mut app, &mut term);

    press_char(&mut app, 'j'); // the field `name`
    press_char(&mut app, 'n'); // the next row
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert!(s.contains("record 2 of 5"), "did not step\n{s}");
    assert_eq!(app.record_sel, 1, "the selected field must not move");
    assert!(s.contains("bob"), "the new row is not shown\n{s}");
}

#[test]
fn closing_the_record_view_moves_the_grid_to_the_field_that_was_selected() {
    let p = write_sample("record-close", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Record);
    // The engine sends the row, and the tree needs it before it has lines.
    settle(&mut app, &mut term);
    press_char(&mut app, 'j');
    press_char(&mut app, 'j'); // the field `amount`
    press(&mut app, ratatui::crossterm::event::KeyCode::Esc);
    settle(&mut app, &mut term);

    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.cursor_col, 2, "the grid did not follow the record view");
}

#[test]
fn the_filter_builder_makes_a_condition_from_menus() {
    let p = write_sample("build", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    // With no condition, the builder opens on the list of columns.
    app.run(Cmd::FilterBuild);
    settle(&mut app, &mut term);
    assert!(
        screen(&term).contains("which column?"),
        "the builder did not open on the columns\n{}",
        screen(&term)
    );

    type_text(&mut app, "region");
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert!(s.contains("contains"), "no operator list\n{s}");

    // The list opens on `=`, and not on its first entry. That is the
    // operator that a user wants most of the time.
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    type_text(&mut app, "EU");
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert!(
        s.contains("WHERE (\"region\" = 'EU')"),
        "the list does not show the compiled expression\n{s}"
    );

    press(&mut app, ratatui::crossterm::event::KeyCode::Enter); // apply
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.view.filter.as_deref(), Some("(\"region\" = 'EU')"));
    assert!(s.contains("alice"), "kept row missing\n{s}");
    assert!(!s.contains("dave"), "removed row still shown\n{s}");
    assert!(s.contains("3 × 4"), "the count did not follow\n{s}");
}

#[test]
fn the_quick_filter_uses_the_value_under_the_cursor() {
    let p = write_sample("quick", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::ColLast); // the column `region`, value EU on the first row
    app.run(Cmd::FilterThisValue);
    settle(&mut app, &mut term);
    let s = screen(&term);

    assert_eq!(app.view.filter.as_deref(), Some("(\"region\" = 'EU')"));
    assert!(!s.contains("dave"), "removed row still shown\n{s}");

    // A second quick filter adds a condition beside the first one.
    app.run(Cmd::ColFirst);
    app.run(Cmd::FilterExcludeValue);
    settle(&mut app, &mut term);
    assert_eq!(
        app.view.filter.as_deref(),
        Some("((\"region\" = 'EU') AND (\"id\" <> 1))")
    );
}

#[test]
fn a_quick_filter_on_a_null_uses_a_test_for_null() {
    let p = write_sample("quick-null", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    // The second row has no amount. A test for equality against the text
    // "NULL" would find nothing.
    app.run(Cmd::RowDown);
    app.run(Cmd::ColRight);
    app.run(Cmd::ColRight);
    app.run(Cmd::FilterThisValue);
    settle(&mut app, &mut term);
    assert_eq!(app.view.filter.as_deref(), Some("(\"amount\" IS NULL)"));
    assert!(screen(&term).contains("1 × 4"), "{}", screen(&term));
}

#[test]
fn a_typed_expression_and_a_built_condition_stand_together() {
    let p = write_sample("mixed", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::Filter);
    type_text(&mut app, "amount > 5");
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    settle(&mut app, &mut term);
    assert_eq!(app.view.filter.as_deref(), Some("(amount > 5)"));

    // The expression that the user typed becomes one condition of the list.
    // A quick filter therefore adds to it, and does not replace it.
    app.run(Cmd::ColLast);
    app.run(Cmd::FilterThisValue);
    settle(&mut app, &mut term);
    assert_eq!(
        app.view.filter.as_deref(),
        Some("((amount > 5) AND (\"region\" = 'EU'))")
    );
    // alice, carol and erin are in the EU and have an amount above 5.
    assert!(screen(&term).contains("3 × 4"), "{}", screen(&term));
}

#[test]
fn the_filter_prompt_completes_a_column_name() {
    let p = write_sample("complete", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::Filter);
    type_text(&mut app, "reg");
    press(&mut app, ratatui::crossterm::event::KeyCode::Tab);
    assert_eq!(app.input.text(), "region");

    // Two columns start with the same letter, so Tab gives the part that
    // both of them start with, and names them.
    app.input.clear();
    type_text(&mut app, "a");
    press(&mut app, ratatui::crossterm::event::KeyCode::Tab);
    assert_eq!(app.input.text(), "amount");
    settle(&mut app, &mut term);
}

#[test]
fn tiny_terminals_still_draw_without_panicking() {
    let p = write_sample("tiny", SAMPLE, "csv");
    let (worker, opened) = Worker::spawn(p.to_str().unwrap(), OpenOptions::default()).unwrap();
    let mut app = App::new(worker, opened, peruse_core::theme::Theme::default(), false);

    // These sizes are small, so the layout cannot give each part its full
    // space.
    for (w, h) in [(20u16, 6u16), (10, 4), (40, 5), (200, 60)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        settle(&mut app, &mut term);
        app.run(Cmd::ToggleMeta);
        settle(&mut app, &mut term);
        app.run(Cmd::ToggleStats);
        settle(&mut app, &mut term);
        app.run(Cmd::Help);
        settle(&mut app, &mut term);

        // The record view and the filter builder draw a list, a prompt and a
        // line of keys. Each of them must fit in a box of four rows.
        app.mode = Mode::Normal;
        app.run(Cmd::Record);
        settle(&mut app, &mut term);
        press_char(&mut app, '/');
        type_text(&mut app, "am");
        settle(&mut app, &mut term);

        app.mode = Mode::Normal;
        app.run(Cmd::FilterBuild);
        settle(&mut app, &mut term);
        for step in [Build::List, Build::Op, Build::Value, Build::Value2, Build::Raw] {
            app.build = step;
            settle(&mut app, &mut term);
        }

        // The band of facts takes its rows from the data, and it colors the
        // column under the cursor. A grid of four rows has room for no band at
        // all, so each mode must still draw.
        app.mode = Mode::Normal;
        for mode in ["compact", "detailed"] {
            app.config.band = Some(mode.to_string());
            settle(&mut app, &mut term);
            app.run(Cmd::ColRight);
            settle(&mut app, &mut term);
        }
        app.config.band = None;

        app.run(Cmd::Cancel);
        app.mode = Mode::Normal;
        app.panel = Panel::None;
    }
}

#[test]
fn drilling_into_a_json_file_of_nested_objects() {
    // A JSON file holds a list of objects, and an object holds other
    // objects. This is the shape that the grid cannot show.
    let body = "[{\"id\":\"249\",\"actor\":{\"id\":665991,\"login\":\"petroav\"},\
                 \"payload\":{\"ref\":\"master\",\"push_id\":null,\
                 \"commits\":[{\"sha\":\"aa\",\"message\":\"one\"},\
                 {\"sha\":\"bb\",\"message\":\"two\"}]}},\
                {\"id\":\"250\",\"actor\":{\"id\":3854017,\"login\":\"rspt\"},\
                 \"payload\":{\"ref\":null,\"push_id\":536,\"commits\":null}}]";
    let p = write_sample("json-drill", body, "json");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    // The grid shows a nested value as one long text, and that is the
    // problem this view exists for.
    assert!(screen(&term).contains("{'id':"), "{}", screen(&term));

    app.run(Cmd::Record);
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert!(s.contains("actor"), "no fields\n{s}");
    assert!(s.contains("2 fields"), "no short form for a structure\n{s}");

    // Open `payload`, then `commits`, then read one commit.
    press_char(&mut app, 'j'); // actor
    press_char(&mut app, 'j'); // payload
    press_char(&mut app, 'l');
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert!(s.contains("master"), "payload did not open\n{s}");
    // The field `push_id` holds no value in this row, so it is hidden.
    assert!(!s.contains("push_id"), "an empty field is shown\n{s}");

    press_char(&mut app, 'j'); // ref
    press_char(&mut app, 'j'); // commits
    press_char(&mut app, 'l');
    press_char(&mut app, 'j'); // [0]
    press_char(&mut app, 'l');
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert!(s.contains("sha"), "the list did not open\n{s}");
    assert!(s.contains("aa"), "the value of a commit is missing\n{s}");
}

#[test]
fn the_record_view_finds_a_field_that_is_three_levels_down() {
    let body = "[{\"a\":{\"b\":{\"c\":{\"target\":\"found me\"}}}}]";
    let p = write_sample("json-find", body, "json");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Record);
    settle(&mut app, &mut term);

    press_char(&mut app, '/');
    type_text(&mut app, "target");
    settle(&mut app, &mut term);
    let s = screen(&term);
    // The find box opens the way to the match, so the user sees it at once.
    assert!(s.contains("target"), "the match is missing\n{s}");
    assert!(s.contains("found me"), "the value is missing\n{s}");
}

#[test]
fn a_filter_on_a_value_inside_a_structure_uses_its_path() {
    let body = "[{\"id\":1,\"actor\":{\"login\":\"alice\"}},\
                {\"id\":2,\"actor\":{\"login\":\"bob\"}},\
                {\"id\":3,\"actor\":{\"login\":\"alice\"}}]";
    let p = write_sample("json-filter", body, "json");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Record);
    settle(&mut app, &mut term);

    press_char(&mut app, 'j'); // actor
    press_char(&mut app, 'l'); // open it
    press_char(&mut app, 'j'); // login
    press_char(&mut app, '='); // keep the rows with this value
    settle(&mut app, &mut term);

    assert_eq!(app.view.filter.as_deref(), Some("(\"actor\".\"login\" = 'alice')"));
    assert!(screen(&term).contains("2 × 2"), "{}", screen(&term));
}

#[test]
fn the_settings_page_shows_the_settings_and_the_machine() {
    let p = write_sample("settings", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::Settings);
    settle(&mut app, &mut term);
    let s = screen(&term);

    // The last setting of the list must be on the screen as well. A new setting
    // must not push another one off the page.
    for label in [
        "theme",
        "threads",
        "memory limit",
        "sample size",
        "column details",
        "step",
    ] {
        assert!(s.contains(label), "setting {label} missing\n{s}");
    }
    // The page says what the machine gives. A user who sets a memory limit
    // needs to know how much memory the machine has.
    assert!(s.contains("this machine"), "no resources\n{s}");
    assert!(s.contains("cores"), "no core count\n{s}");
    assert!(s.contains("memory"), "no memory\n{s}");
    // It also says what DuckDB uses now, and not only what the user asked
    // for.
    assert!(s.contains("duckdb now"), "no live settings\n{s}");
    assert!(
        app.duck_threads.is_some(),
        "the engine did not report its threads"
    );
}

#[test]
fn changing_a_setting_applies_it_and_keeps_it() {
    let p = write_sample("settings-edit", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Settings);
    settle(&mut app, &mut term);

    // Move to `threads` and give it a value.
    press_char(&mut app, 'j');
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    type_text(&mut app, "3");
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    settle(&mut app, &mut term);

    assert_eq!(app.config.threads, Some(3));
    // DuckDB changes its threads while it runs, so the page shows the new
    // value without a restart.
    assert_eq!(app.duck_threads.as_deref(), Some("3"));
    // The change goes into the file at once. A second key to keep it is one
    // that the user forgets to press.
    assert!(
        screen(&term).contains("kept as you change them"),
        "{}",
        screen(&term)
    );
}

#[test]
fn a_setting_with_no_value_shows_what_peruse_uses_instead() {
    let p = write_sample("settings-default", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Settings);
    settle(&mut app, &mut term);

    // Nothing is set, so the page shows the built-in value in its place.
    assert_eq!(app.setting_value(crate::app::Setting::Threads), "");
    assert!(
        app.setting_default(crate::app::Setting::Threads)
            .contains("each core"),
        "{}",
        app.setting_default(crate::app::Setting::Threads)
    );
    assert!(screen(&term).contains("each core"), "{}", screen(&term));
}

#[test]
fn a_bad_value_for_a_setting_is_refused_with_a_reason() {
    let p = write_sample("settings-bad", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Settings);
    settle(&mut app, &mut term);

    press_char(&mut app, 'j'); // threads
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    type_text(&mut app, "lots");
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    settle(&mut app, &mut term);

    assert_eq!(app.config.threads, None, "a bad value must not be taken");
    assert!(screen(&term).contains("not a number"), "{}", screen(&term));
}

#[test]
fn the_machine_can_give_the_value_of_a_setting() {
    let p = write_sample("settings-machine", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Settings);
    settle(&mut app, &mut term);

    press_char(&mut app, 'j'); // threads
    press_char(&mut app, 'm'); // take the value of this machine
    settle(&mut app, &mut term);
    assert_eq!(app.config.threads, Some(app.resources.cores));
}

#[test]
fn undo_goes_back_one_filter_at_a_time() {
    let p = write_sample("undo", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    // Two filters, one after the other.
    app.run(Cmd::ColLast); // region, EU on the first row
    app.run(Cmd::FilterThisValue);
    settle(&mut app, &mut term);
    assert_eq!(app.view.filter.as_deref(), Some("(\"region\" = 'EU')"));

    app.run(Cmd::ColFirst); // id
    app.run(Cmd::FilterExcludeValue);
    settle(&mut app, &mut term);
    assert_eq!(
        app.view.filter.as_deref(),
        Some("((\"region\" = 'EU') AND (\"id\" <> 1))")
    );

    // One key goes back one step, and the grid follows.
    app.run(Cmd::Undo);
    settle(&mut app, &mut term);
    assert_eq!(app.view.filter.as_deref(), Some("(\"region\" = 'EU')"));
    assert!(screen(&term).contains("3 × 4"), "{}", screen(&term));
    // The builder must agree with the grid after a step backward.
    assert_eq!(app.fset.len(), 1, "the conditions did not follow");

    app.run(Cmd::Undo);
    settle(&mut app, &mut term);
    assert_eq!(app.view.filter, None);
    assert!(screen(&term).contains("5 × 4"), "{}", screen(&term));
    assert!(app.fset.is_empty());
}

#[test]
fn undo_says_where_it_arrived_and_stops_at_the_start() {
    let p = write_sample("undo-end", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::SortCycle);
    settle(&mut app, &mut term);
    app.run(Cmd::Undo);
    settle(&mut app, &mut term);
    // The message names the view that the user arrived at.
    assert!(
        screen(&term).contains("back to the whole file"),
        "{}",
        screen(&term)
    );

    // At the start there is nothing behind, and the key says so.
    app.run(Cmd::Undo);
    settle(&mut app, &mut term);
    assert!(
        screen(&term).contains("nothing to go back to"),
        "{}",
        screen(&term)
    );
}

#[test]
fn redo_puts_back_the_change_that_undo_removed() {
    let p = write_sample("redo", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::ColLast);
    app.run(Cmd::FilterThisValue);
    settle(&mut app, &mut term);
    app.run(Cmd::Undo);
    settle(&mut app, &mut term);
    assert_eq!(app.view.filter, None);

    app.run(Cmd::Redo);
    settle(&mut app, &mut term);
    assert_eq!(app.view.filter.as_deref(), Some("(\"region\" = 'EU')"));
    assert!(screen(&term).contains("3 × 4"), "{}", screen(&term));
}

#[test]
fn a_new_change_removes_the_way_forward() {
    let p = write_sample("undo-branch", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::ColLast);
    app.run(Cmd::FilterThisValue);
    settle(&mut app, &mut term);
    app.run(Cmd::Undo);
    settle(&mut app, &mut term);

    // A change after a step backward cannot keep the old way forward.
    app.run(Cmd::SortCycle);
    settle(&mut app, &mut term);
    app.run(Cmd::Redo);
    settle(&mut app, &mut term);
    assert!(
        screen(&term).contains("nothing to go forward to"),
        "{}",
        screen(&term)
    );
}

#[test]
fn undo_also_covers_a_sql_statement() {
    let p = write_sample("undo-sql", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.view.base = Base::Sql("SELECT region FROM src".into());
    app.run_startup_view();
    settle(&mut app, &mut term);
    assert!(screen(&term).contains("5 × 1"), "{}", screen(&term));

    app.run(Cmd::Undo);
    settle(&mut app, &mut term);
    assert_eq!(app.view.base, Base::Source);
    assert!(screen(&term).contains("5 × 4"), "{}", screen(&term));
}

#[test]
fn a_setting_goes_into_the_file_as_soon_as_it_changes() {
    let p = write_sample("settings-keep", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    let cfg = app.config_path.clone().unwrap();

    app.run(Cmd::Settings);
    settle(&mut app, &mut term);
    press_char(&mut app, 'j'); // threads
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    type_text(&mut app, "2");
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    settle(&mut app, &mut term);

    // The file holds the change with no second key.
    let (on_disk, err) = peruse_core::config::Config::load_from(&cfg);
    assert_eq!(err, None);
    assert_eq!(on_disk.threads, Some(2));
}

#[test]
fn the_memory_limit_takes_whole_gigabytes_only() {
    let p = write_sample("settings-gb", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Settings);
    settle(&mut app, &mut term);

    press_char(&mut app, 'j');
    press_char(&mut app, 'j'); // memory limit
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    type_text(&mut app, "8");
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    settle(&mut app, &mut term);
    assert_eq!(app.config.memory_limit_gb, Some(8));
    assert_eq!(app.duck_memory.as_deref(), Some("8.0 GiB"));

    // A size with a unit that is not the gigabyte says what to write.
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    app.input.clear();
    type_text(&mut app, "512MB");
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    settle(&mut app, &mut term);
    assert_eq!(app.config.memory_limit_gb, Some(8), "the old value stays");
    assert!(
        screen(&term).contains("whole number of gigabytes"),
        "{}",
        screen(&term)
    );
}

#[test]
fn with_no_setting_duckdb_gets_half_of_the_machine() {
    // Without a limit DuckDB takes 80 percent of the memory for itself, and
    // a viewer of data is not the only program that the user runs.
    let r = peruse_core::config::Resources::read();
    if let Some(gb) = r.default_memory_gb() {
        assert!(gb >= 1);
        assert_eq!(r.default_memory_text(), Some(format!("{gb}GiB")));
    }
}

#[test]
fn both_panels_stack_with_metadata_above_the_statistics() {
    let p = write_sample("panels-both", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::CyclePanels); // meta
    app.run(Cmd::CyclePanels); // stats
    app.run(Cmd::CyclePanels); // both
    assert_eq!(app.panel, Panel::Both);
    settle(&mut app, &mut term);

    let rows = lines(&term);
    let meta_at = rows.iter().position(|l| l.contains("metadata"));
    let stats_at = rows.iter().position(|l| l.contains("stddev"));
    assert!(meta_at.is_some(), "no metadata panel\n{}", screen(&term));
    assert!(stats_at.is_some(), "no statistics panel\n{}", screen(&term));
    // The order never changes, so the eye finds each of them in one place.
    assert!(
        meta_at < stats_at,
        "the metadata must be above the statistics\n{}",
        screen(&term)
    );
}

#[test]
fn a_tall_side_pane_shows_the_whole_metadata_beside_the_statistics() {
    // The statistics of a column end after some rows. The metadata keeps each
    // row that is left, so a tall screen shows every part of it. The panel
    // wrote four rows of the summary before, whatever the height was.
    let p = write_sample("panels-tall", SAMPLE, "csv");
    let (mut app, _) = open(&p);
    let mut term = Terminal::new(TestBackend::new(200, 60)).unwrap();

    app.run(Cmd::ToggleMeta);
    app.run(Cmd::ToggleStats);
    assert_eq!(app.panel, Panel::Both);
    settle(&mut app, &mut term);
    let s = screen(&term);
    let rows = lines(&term);

    // The summary of this file has eight rows, and each of them is there.
    let summary = [
        "format",
        "files",
        "size on disk",
        "delimiter",
        "quote",
        "escape",
        "line ending",
        "header row",
    ];
    for label in summary {
        assert!(s.contains(label), "summary row {label} missing\n{s}");
    }
    let written = summary
        .iter()
        .filter(|label| rows.iter().any(|l| l.contains(**label)))
        .count();
    assert!(written > 4, "the summary still stops at four rows\n{s}");

    // Rows stay free after the list of columns, so the read call is there too.
    assert!(s.contains("reads as"), "no read call\n{s}");
    assert!(s.contains("read_csv"), "no read call\n{s}");

    // The statistics keep their own place below the metadata, and they are
    // complete: the metadata does not take the rows that they need.
    let meta_at = rows.iter().position(|l| l.contains("metadata"));
    let stats_at = rows.iter().position(|l| l.contains("stddev"));
    // A panel that is not there gives no position, and one position that is
    // absent is smaller than any other. Test for the two panels first.
    assert!(meta_at.is_some(), "no metadata panel\n{s}");
    assert!(stats_at.is_some(), "no statistics panel\n{s}");
    assert!(
        meta_at < stats_at,
        "the metadata must be above the statistics\n{s}"
    );
    assert!(s.contains("distribution"), "no chart in the statistics\n{s}");
}

#[test]
fn the_note_about_the_filter_keeps_a_blank_row_above_it() {
    // The statistics panel asks the layout for a height, and it must then use
    // the rows that it asked for. A column of unique values ends with two
    // notes: one note about the values, and one note about the filter.
    let p = write_sample("panels-filter-note", SAMPLE, "csv");
    let (mut app, _) = open(&p);
    let mut term = Terminal::new(TestBackend::new(200, 60)).unwrap();
    settle(&mut app, &mut term);

    app.run(Cmd::ColLast); // the column `region`, value EU on the first row
    app.run(Cmd::FilterThisValue);
    settle(&mut app, &mut term);
    app.run(Cmd::ColFirst); // the column `id`, and each of its values is unique
    app.run(Cmd::ToggleMeta);
    app.run(Cmd::ToggleStats);
    settle(&mut app, &mut term);

    let s = screen(&term);
    let rows = lines(&term);
    let once = rows.iter().position(|l| l.contains("value occurs once"));
    let filter = rows.iter().position(|l| l.contains("over filtered rows only"));
    assert!(once.is_some(), "no note about the values\n{s}");
    assert!(filter.is_some(), "no note about the filter\n{s}");
    assert_eq!(
        filter,
        once.map(|y| y + 2),
        "the two notes need a blank row between them\n{s}"
    );
}

#[test]
fn a_list_of_columns_that_is_too_long_says_how_many_are_outside_it() {
    // A short panel cannot show sixty columns. The user must know that the
    // list goes on, so the list keeps its last row for the count.
    let p = write_sample("panels-more", &wide_sample(), "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::ToggleMeta);
    app.run(Cmd::ToggleStats);
    settle(&mut app, &mut term);
    assert!(
        screen(&term).contains("more below"),
        "the panel does not say how many columns are outside the list\n{}",
        screen(&term)
    );
}

#[test]
fn the_two_panel_keys_add_and_remove_one_panel_each() {
    let p = write_sample("panels-keys", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::ToggleMeta);
    assert_eq!(app.panel, Panel::Meta);
    // The second key adds its panel and keeps the first one.
    app.run(Cmd::ToggleStats);
    assert_eq!(app.panel, Panel::Both);
    // Each key then removes its own panel only.
    app.run(Cmd::ToggleMeta);
    assert_eq!(app.panel, Panel::Stats);
    app.run(Cmd::ToggleStats);
    assert_eq!(app.panel, Panel::None);
}

#[test]
fn the_panels_of_the_settings_file_are_on_the_screen_at_the_start() {
    let p = write_sample("panels-setting", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    app.set_panel_from_setting("both");
    settle(&mut app, &mut term);
    assert_eq!(app.panel, Panel::Both);
    assert!(screen(&term).contains("metadata"), "{}", screen(&term));

    // A name that Peruse does not know leaves the screen with no panel, and
    // it says so. A setting must not stop the program.
    app.set_panel_from_setting("nonsense");
    settle(&mut app, &mut term);
    assert!(screen(&term).contains("is not a panel"), "{}", screen(&term));
}

#[test]
fn the_metadata_panel_opens_the_fields_of_a_structure() {
    // A JSON file holds a column that holds other values. The panel said
    // nothing about those fields before.
    let body = "[{\"id\":1,\"actor\":{\"login\":\"alice\",\"site_admin\":false}}]";
    let p = write_sample("panels-struct", body, "json");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    // A line of the panel holds the name of the field and its type. The grid
    // also writes the text of the structure, so the name by itself is not a
    // test of the panel.
    fn field_line(term: &Terminal<TestBackend>, name: &str, ty: &str) -> bool {
        lines(term).iter().any(|l| l.contains(name) && l.contains(ty))
    }

    app.run(Cmd::ToggleMeta);
    settle(&mut app, &mut term);
    // The cursor is on `id`, so the panel opens no structure.
    assert!(
        !field_line(&term, "site_admin", "BOOLEAN"),
        "a structure opened before the cursor reached it\n{}",
        screen(&term)
    );

    app.run(Cmd::ColRight); // actor
    settle(&mut app, &mut term);
    assert!(
        field_line(&term, "login", "VARCHAR"),
        "no field of the structure\n{}",
        screen(&term)
    );
    assert!(
        field_line(&term, "site_admin", "BOOLEAN"),
        "no field of the structure\n{}",
        screen(&term)
    );
}

#[test]
fn moving_across_the_columns_asks_for_each_column_one_time() {
    let p = write_sample("panels-cache", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::ToggleStats);
    settle(&mut app, &mut term);

    // Move to the end and back again. The statistics of a column cost a
    // scan, so the second visit must read the answer that Peruse kept.
    for _ in 0..3 {
        app.run(Cmd::ColRight);
        settle(&mut app, &mut term);
    }
    for _ in 0..3 {
        app.run(Cmd::ColLeft);
    }
    // No settle: the answer is there already, so the panel needs no wait.
    app.ensure_rows();
    assert!(
        app.stats().is_some(),
        "the statistics of a column were not kept"
    );
    assert_eq!(app.stats().map(|s| s.column.as_str()), Some("id"));
}

// ------------------------------------------------- the detail band

/// A file with column names that are wide enough for the band to write a word
/// beside each of its numbers.
///
/// The names of [`SAMPLE`] are two to six characters wide, and the band drops a
/// word or a type on the narrow ones.
const BAND_SAMPLE: &str = "customer_id,customer_name,amount_paid,region\n\
                           1,alice,10.5,EU\n\
                           2,bob,,US\n\
                           3,carol,30.25,EU\n\
                           4,dave,4000.75,APAC\n\
                           5,erin,7.0,EU\n";

/// The row of the terminal that holds the column names.
///
/// The title bar is above it, on the row 0. The band starts on the row after it,
/// and the first row of data comes after the band.
const HEADER_ROW: usize = 1;

/// Gives the rows of the detail band from the screen.
fn band(term: &Terminal<TestBackend>, rows: usize) -> Vec<String> {
    lines(term)[HEADER_ROW + 1..HEADER_ROW + 1 + rows].to_vec()
}

/// Makes a click of the left button at a position of the terminal.
fn click(app: &mut App, column: u16, row: u16) -> bool {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    app.on_mouse(&MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

#[test]
fn the_key_d_moves_through_the_three_modes_of_the_band() {
    let p = write_sample("band-key", BAND_SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    // With no band, the first row of data comes straight after the names.
    assert_eq!(app.band(), Band::Off);
    let rows = lines(&term);
    assert!(rows[HEADER_ROW].contains("customer_id"), "no names\n{}", screen(&term));
    assert!(rows[HEADER_ROW + 1].contains("alice"), "no data\n{}", screen(&term));
    let all_rows = app.viewport_rows;

    // The compact band: one row for each column, with the type and the share of
    // NULL values.
    press_char(&mut app, 'd');
    settle(&mut app, &mut term);
    assert_eq!(app.band(), Band::Compact);
    let b = band(&term, 1);
    assert!(b[0].contains("BIGINT"), "no type in the band\n{}", screen(&term));
    assert!(b[0].contains("VARCHAR"), "no type in the band\n{}", screen(&term));
    assert!(b[0].contains("20%"), "no NULL share of amount_paid\n{}", screen(&term));
    assert!(
        lines(&term)[HEADER_ROW + 2].contains("alice"),
        "the data did not move down one row\n{}",
        screen(&term)
    );
    assert_eq!(app.viewport_rows, all_rows - 1, "the band takes one row");

    // The detailed band: four rows for each column.
    press_char(&mut app, 'd');
    settle(&mut app, &mut term);
    assert_eq!(app.band(), Band::Detailed);
    let b = band(&term, Band::DETAIL_ROWS as usize);
    assert!(b[0].contains("BIGINT"), "row 1 is the type\n{}", screen(&term));
    assert!(b[1].contains("0% null"), "row 2 is the NULL share\n{}", screen(&term));
    assert!(b[1].contains("20% null"), "amount_paid holds one NULL\n{}", screen(&term));
    assert!(b[2].contains("~5 distinct"), "row 3 is the count\n{}", screen(&term));
    assert!(b[3].contains("1 → 5"), "row 4 is the range\n{}", screen(&term));
    assert!(
        lines(&term)[HEADER_ROW + 1 + Band::DETAIL_ROWS as usize].contains("alice"),
        "the data must start under the band\n{}",
        screen(&term)
    );
    assert_eq!(
        app.viewport_rows,
        all_rows - Band::DETAIL_ROWS as usize,
        "the band takes four rows from the data"
    );

    // The third press turns the band off, and the grid is as it was.
    press_char(&mut app, 'd');
    settle(&mut app, &mut term);
    assert_eq!(app.band(), Band::Off);
    assert!(lines(&term)[HEADER_ROW + 1].contains("alice"), "{}", screen(&term));
    assert_eq!(app.viewport_rows, all_rows);
}

#[test]
fn the_band_of_the_settings_file_is_on_the_screen_at_the_start() {
    // The setting is the only copy of the mode, so the band of the last session
    // is on the screen with no key.
    let p = write_sample("band-setting", BAND_SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    app.config.band = Some("compact".into());
    settle(&mut app, &mut term);
    assert_eq!(app.band(), Band::Compact);
    assert!(band(&term, 1)[0].contains("BIGINT"), "{}", screen(&term));

    // A name that Peruse does not know leaves the band off. A setting is not a
    // reason to refuse to open a file.
    app.config.band = Some("nonsense".into());
    settle(&mut app, &mut term);
    assert_eq!(app.band(), Band::Off);
    assert!(lines(&term)[HEADER_ROW + 1].contains("alice"), "{}", screen(&term));
}

#[test]
fn a_fitted_column_keeps_room_for_its_type_mark_and_for_the_band() {
    // A column whose name is as wide as its widest value ended exactly as wide
    // as the name. The header then dropped the type mark, and the band fell back
    // to the share of NULL values alone.
    let p = write_sample("fit-headroom", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    app.config.band = Some("compact".into());
    settle(&mut app, &mut term);

    // Each column shows the mark of its family. The mark of a column of numbers
    // goes in front of the name, on the side of the digits.
    marks_are_all_on_the_screen(&app, &term);

    // The band writes the type beside the share, and no longer the share alone.
    let b = band(&term, 1);
    assert!(b[0].contains("DOUBLE"), "no type for amount\n{}", b[0]);
    assert!(b[0].contains("VARCHAR"), "no type for region\n{}", b[0]);
    assert!(b[0].contains("20%"), "no share for amount\n{}", b[0]);

    // A sort adds an arrow in front of the name, which costs one screen column.
    // The room after the name covers that arrow as well, so a sorted column
    // keeps its mark.
    app.run(Cmd::SortCycle);
    settle(&mut app, &mut term);
    assert!(!app.view.sort.is_empty(), "the column is not sorted");
    marks_are_all_on_the_screen(&app, &term);
}

/// Reads the mark of the family at the exact screen position of each column of
/// the frame, and compares it with the mark of that column.
fn marks_are_all_on_the_screen(app: &App, term: &Terminal<TestBackend>) {
    let head: Vec<char> = lines(term)[HEADER_ROW].chars().collect();
    for &(ci, x, w) in &app.hit.cols {
        let col = &app.schema.columns[ci];
        let at = match col.kind.align() {
            peruse_core::model::Align::Left => x + w - 1,
            peruse_core::model::Align::Right => x,
        };
        assert_eq!(
            head[at as usize],
            col.kind.badge(),
            "the column {} lost its type mark\n{}",
            col.name,
            screen(term)
        );
    }
}

#[test]
fn the_name_of_the_column_under_the_cursor_is_stronger_than_its_band() {
    // The header shows three levels: the name of the column under the cursor,
    // the facts of that column, and the facts of each other column. Without the
    // middle level, the name and its facts read as one block.
    use ratatui::style::Modifier;
    let p = write_sample("band-levels", BAND_SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    app.config.band = Some("detailed".into());
    // The column `customer_name` holds text, so its name starts at the left
    // edge of the column.
    app.run(Cmd::ColRight);
    settle(&mut app, &mut term);

    let t = app.theme.clone();
    let (cursor_ci, cursor_x, _) = app.hit.cols[1];
    let (other_ci, other_x, _) = app.hit.cols[2];
    // Read the two columns from the frame and not from a count. A frame that
    // scrolled to the right would otherwise compare two columns that are not the
    // two that this test names.
    assert_eq!(cursor_ci, app.cursor_col, "the cursor is on another column");
    assert_ne!(other_ci, app.cursor_col, "the second column is the cursor column");
    let y = HEADER_ROW as u16;
    let paint = |c| crate::colors::conv(c, Depth::True);
    let buf = term.backend().buffer();
    let name = &buf[(cursor_x, y)];
    let facts = &buf[(cursor_x, y + 1)];
    let other = &buf[(other_x, y + 1)];

    assert_eq!(name.fg, paint(t.accent), "the name keeps the accent color");
    assert!(
        name.modifier.contains(Modifier::BOLD),
        "the name keeps its thick letters"
    );
    assert_eq!(
        facts.fg,
        paint(crate::grid::band_focus(&t)),
        "the band under the name must be quieter than the name"
    );
    assert!(
        !facts.modifier.contains(Modifier::BOLD),
        "thin letters keep the name the stronger of the two"
    );
    assert_eq!(other.fg, paint(t.dim), "the band of another column stays dim");
    assert_ne!(name.fg, facts.fg, "the name and its facts are one block");
    assert_ne!(facts.fg, other.fg, "the column under the cursor does not stand out");
}

#[test]
fn a_click_under_the_band_lands_on_the_row_that_the_user_pointed_at() {
    let p = write_sample("band-click", BAND_SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    app.config.band = Some("detailed".into());
    settle(&mut app, &mut term);

    // The third row of data on the screen holds carol. A click on it must move
    // the cursor to that row, and not to a row four rows above it.
    let y = HEADER_ROW + 1 + Band::DETAIL_ROWS as usize + 2;
    assert!(
        lines(&term)[y].contains("carol"),
        "the test points at the wrong row\n{}",
        screen(&term)
    );
    assert!(click(&mut app, 20, y as u16));
    assert_eq!(app.cursor_row, 2, "the click landed on another row");

    // A click on a row of the band moves to that column, as a click on the row
    // of the names does. The band describes a column and not a row.
    let x = lines(&term)[HEADER_ROW].find("region").expect("no column region") as u16;
    assert!(click(&mut app, x, (HEADER_ROW + 2) as u16));
    assert_eq!(app.cursor_col, 3, "the click did not reach the column");
    assert_eq!(app.cursor_row, 2, "a click on the band must not move the row");
}

#[test]
fn a_terminal_too_short_for_the_band_still_draws_the_data() {
    let p = write_sample("band-tiny", BAND_SAMPLE, "csv");
    let (worker, opened) = Worker::spawn(p.to_str().unwrap(), OpenOptions::default()).unwrap();
    let mut app = App::new(worker, opened, peruse_core::theme::Theme::default(), false);
    // Write no settings file: this application has the path of the user.
    app.config.band = Some("detailed".into());

    for (w, h) in [(10u16, 4u16), (20, 6), (40, 8), (110, 24)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        settle(&mut app, &mut term);
        // The band gives its rows back to the data. A band with rows on the
        // screen therefore always has a row of data under it.
        assert!(
            app.hit.band == 0 || app.hit.rows >= 1,
            "{w}x{h}: the band pushed the data off the screen\n{}",
            screen(&term)
        );
        assert!(
            app.viewport_rows >= 1,
            "{w}x{h}: no room for a row of data\n{}",
            screen(&term)
        );
        if h >= 6 {
            // A terminal of this height holds the names, one row of the band and
            // one row of data.
            let rows = lines(&term);
            assert!(
                rows[HEADER_ROW].contains("customer_id"),
                "{w}x{h}: no column names\n{}",
                screen(&term)
            );
            assert!(app.hit.band >= 1, "{w}x{h}: no band at all");
            // A narrow terminal holds the first column only, so the number of
            // the row is the mark of a row of data.
            assert!(
                rows.iter().any(|l| l.trim_start().starts_with('1')),
                "{w}x{h}: no row of data\n{}",
                screen(&term)
            );
        }
    }
}

#[test]
fn the_band_over_a_filtered_view_reports_the_filtered_rows() {
    // The band describes what the grid shows. A share over the whole file would
    // be a lie under a filter.
    let p = write_sample("band-filter", BAND_SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    app.config.band = Some("detailed".into());
    settle(&mut app, &mut term);
    assert!(
        band(&term, 4)[1].contains("20% null"),
        "one row in five holds no amount\n{}",
        screen(&term)
    );

    // The one row of bob holds no amount, so the column is then all NULL.
    app.view.filter = Some("customer_name = 'bob'".into());
    app.run_startup_view();
    settle(&mut app, &mut term);
    let b = band(&term, 4);
    assert!(
        b[1].contains("100% null"),
        "the band did not follow the filter\n{}",
        screen(&term)
    );
    assert!(!b[1].contains("20% null"), "an old number stayed\n{}", screen(&term));
    assert!(
        b[3].contains("all null"),
        "a column of NULL values has no range\n{}",
        screen(&term)
    );
}

#[test]
fn a_band_answer_from_an_old_view_changes_nothing() {
    use peruse_core::engine::ColumnBrief;
    let p = write_sample("band-stale", BAND_SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    app.config.band = Some("compact".into());
    settle(&mut app, &mut term);
    assert!(band(&term, 1)[0].contains("BIGINT"), "{}", screen(&term));

    // An answer for a view that the user left must not reach the screen. The
    // epoch 0 is older than any view that Peruse asked for.
    let changed = app.on_response(peruse_core::Response::Band {
        epoch: 0,
        briefs: vec![ColumnBrief {
            column: "customer_id".into(),
            n_total: 100,
            n_present: 1,
            n_distinct: Some(1),
            min: None,
            max: None,
        }],
    });
    assert!(!changed, "a stale answer asked for a new frame");
    settle(&mut app, &mut term);
    let b = band(&term, 1);
    assert!(b[0].contains("BIGINT"), "the type went away\n{}", screen(&term));
    assert!(!b[0].contains("99%"), "a stale number reached the band\n{}", screen(&term));
    // The facts of the view that the user is on are still there. The compact
    // band measures the two counts only, so the count of the rows is the fact to
    // test here.
    assert_eq!(app.brief(0).map(|b| b.n_total), Some(5));
    assert_eq!(app.brief(0).map(|b| b.n_present), Some(5));
}

#[test]
fn the_band_over_a_file_of_text_measures_the_columns_with_one_query() {
    // A CSV file has no footer, so the engine must measure the columns.
    //
    // The mode of the band decides what the query measures. The compact band
    // draws the share of NULL values alone, and each of the three other facts
    // reads the whole column, so compact must leave them unknown. The detailed
    // band then asks again and gets them.
    let p = write_sample("band-query", BAND_SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    app.config.band = Some("compact".into());
    settle(&mut app, &mut term);

    assert_eq!(app.brief(0).map(|b| b.n_total), Some(5), "no query ran");
    assert_eq!(
        app.brief(0).and_then(|b| b.n_distinct),
        None,
        "compact must not pay for the count of the different values"
    );

    app.config.band = Some("detailed".into());
    settle(&mut app, &mut term);
    assert_eq!(
        app.brief(0).and_then(|b| b.n_distinct),
        Some(5),
        "the detailed band must ask again for the three other facts"
    );
    app.config.band = Some("compact".into());
    settle(&mut app, &mut term);
    // A move across the columns must not ask again. The answer covers each
    // column that the grid draws.
    for _ in 0..3 {
        app.run(Cmd::ColRight);
    }
    app.ensure_rows();
    assert!(app.brief(3).is_some(), "the answer covered one column only");
    assert!(!app.busy, "a move across the columns started a new query");
}

#[test]
fn the_band_asks_again_for_the_columns_of_a_request_that_the_worker_dropped() {
    // The worker keeps one band request only. A move to the side gives a new
    // request, the worker drops the older one, and the answer of the older one
    // never arrives. The band must ask again for those columns when the user
    // comes back to them. Without this, they show a row of points until the user
    // changes the view.
    let p = write_sample("band-dropped", &wide_sample(), "csv");
    let (mut app, mut term) = open(&p);
    app.config.band = Some("compact".into());

    // One frame asks the engine about the first columns. The test then drops
    // every answer of the band, exactly as the worker drops the request itself.
    let rx = app.worker.responses().clone();
    for _ in 0..8 {
        term.draw(|f| ui::draw(f, &mut app, Depth::True)).unwrap();
        app.ensure_rows();
        while let Ok(r) = rx.recv_timeout(Duration::from_millis(75)) {
            if !matches!(r, peruse_core::Response::Band { .. }) {
                app.on_response(r);
            }
        }
    }
    assert!(app.brief(0).is_none(), "the test kept an answer of the band");

    // The last columns get their own request, and this one arrives.
    app.run(Cmd::ColLast);
    settle(&mut app, &mut term);
    let last = app.schema.len() - 1;
    assert!(app.brief(last).is_some(), "no answer for the last column");

    // Back to the first columns. No answer for them ever arrived, so the band
    // must ask again.
    app.run(Cmd::ColFirst);
    settle(&mut app, &mut term);
    assert!(app.brief(0).is_some(), "the band never asked again");
}

#[test]
fn a_grid_with_no_room_for_the_band_asks_the_engine_nothing() {
    // The band gives its rows back to the data on a short grid, so a short grid
    // shows no band at all. A query for facts that nothing draws reads the whole
    // file for nothing.
    let p = write_sample("band-noroom", BAND_SAMPLE, "csv");
    let (worker, opened) = Worker::spawn(p.to_str().unwrap(), OpenOptions::default()).unwrap();
    let mut app = App::new(worker, opened, peruse_core::theme::Theme::default(), false);
    // Write no settings file: this application has the path of the user.
    app.config.band = Some("detailed".into());
    let mut term = Terminal::new(TestBackend::new(40, 2)).unwrap();
    settle(&mut app, &mut term);

    assert_eq!(app.hit.band, 0, "the grid has room for a band\n{}", screen(&term));
    assert!(
        app.brief(0).is_none(),
        "the band measured a column that it cannot draw"
    );
}

#[test]
fn the_filter_prompt_shows_the_rest_of_a_column_name_as_you_type() {
    let p = write_sample("ghost", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::Filter);
    type_text(&mut app, "am");
    settle(&mut app, &mut term);
    // The line holds what the user typed, and the screen shows the rest.
    assert_eq!(app.input.text(), "am");
    assert_eq!(app.ghost().as_deref(), Some("ount"));
    assert!(screen(&term).contains("amount"), "{}", screen(&term));

    // The key Tab takes it.
    press(&mut app, ratatui::crossterm::event::KeyCode::Tab);
    assert_eq!(app.input.text(), "amount");
    assert_eq!(app.ghost(), None, "a complete name needs no ghost");
}

#[test]
fn the_key_right_also_takes_the_ghost_at_the_end_of_a_line() {
    let p = write_sample("ghost-right", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::Filter);
    type_text(&mut app, "reg");
    press(&mut app, ratatui::crossterm::event::KeyCode::Right);
    assert_eq!(app.input.text(), "region");
}

#[test]
fn ctrl_and_alt_with_the_key_right_move_a_word_and_do_not_take_the_ghost() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let p = write_sample("ghost-word-right", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::Filter);
    type_text(&mut app, "reg");
    // These two forms move the cursor one word. The cursor is at the end of
    // the line, so the line does not change.
    app.on_key(&KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
    assert_eq!(app.input.text(), "reg", "Ctrl and the key right move a word");
    app.on_key(&KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(app.input.text(), "reg", "Alt and the key right move a word");
    // The key right alone still takes the ghost completion.
    press(&mut app, KeyCode::Right);
    assert_eq!(app.input.text(), "region");
}

#[test]
fn the_shortest_name_is_the_one_that_the_ghost_shows() {
    let body = "a,amount,amount_tax\n1,2,3\n";
    let p = write_sample("ghost-short", body, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::Filter);
    type_text(&mut app, "amo");
    assert_eq!(app.ghost().as_deref(), Some("unt"));
}

#[test]
fn there_is_no_ghost_in_the_middle_of_a_line() {
    let p = write_sample("ghost-mid", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::Filter);
    type_text(&mut app, "am");
    press(&mut app, ratatui::crossterm::event::KeyCode::Left);
    // The text of the user is after the cursor, so there is no room.
    assert_eq!(app.ghost(), None);
}

#[test]
fn the_ghost_follows_a_path_into_a_structure() {
    let body = "[{\"id\":1,\"actor\":{\"login\":\"alice\",\"site_admin\":false}}]";
    let p = write_sample("ghost-struct", body, "json");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::Filter);
    type_text(&mut app, "act");
    assert_eq!(app.ghost().as_deref(), Some("or"));
    press(&mut app, ratatui::crossterm::event::KeyCode::Tab);
    assert_eq!(app.input.text(), "actor");

    // A full stop moves into the structure, and the fields follow.
    type_text(&mut app, ".log");
    assert_eq!(app.ghost().as_deref(), Some("in"));
    press(&mut app, ratatui::crossterm::event::KeyCode::Tab);
    assert_eq!(app.input.text(), "actor.login");
    settle(&mut app, &mut term);
}

#[test]
fn a_field_of_a_structure_completes_and_the_filter_runs() {
    let body = "[{\"id\":1,\"actor\":{\"login\":\"alice\"}},\
                {\"id\":2,\"actor\":{\"login\":\"bob\"}}]";
    let p = write_sample("ghost-run", body, "json");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::Filter);
    type_text(&mut app, "actor.log");
    press(&mut app, ratatui::crossterm::event::KeyCode::Tab);
    type_text(&mut app, " = 'alice'");
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    settle(&mut app, &mut term);

    assert_eq!(app.view.filter.as_deref(), Some("(actor.login = 'alice')"));
    assert!(screen(&term).contains("1 × 2"), "{}", screen(&term));
}

#[test]
fn a_setting_shows_the_rest_of_its_answer() {
    let p = write_sample("ghost-setting", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Settings);
    settle(&mut app, &mut term);

    // The theme is the first setting, and the names of the themes are known.
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    app.input.clear();
    type_text(&mut app, "dra");
    settle(&mut app, &mut term);
    assert_eq!(app.ghost().as_deref(), Some("cula"));
    assert!(screen(&term).contains("dracula"), "{}", screen(&term));

    press(&mut app, ratatui::crossterm::event::KeyCode::Tab);
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    settle(&mut app, &mut term);
    assert_eq!(app.config.theme.as_deref(), Some("dracula"));
    assert_eq!(app.theme.name, "dracula");
}

#[test]
fn a_setting_that_takes_a_number_has_no_ghost() {
    let p = write_sample("ghost-number", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Settings);
    settle(&mut app, &mut term);

    press_char(&mut app, 'j'); // threads
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    type_text(&mut app, "1");
    // No part of a number says what the rest of it is.
    assert_eq!(app.ghost(), None);
}

#[test]
fn enter_on_a_struct_cell_drills_into_it_instead_of_showing_its_text() {
    // The text that DuckDB writes for a structure says what the value holds
    // and nothing more. The user cannot read one field of it.
    let body = "[{\"id\":1,\"actor\":{\"id\":665991,\"login\":\"petroav\",\"gravatar_id\":\"\"}}]";
    let p = write_sample("enter-struct", body, "json");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::ColRight); // the column `actor`
    app.run(Cmd::InspectCell);
    for _ in 0..10 {
        if !app.record_tree.is_empty() {
            break;
        }
        settle(&mut app, &mut term);
    }
    let s = screen(&term);

    // The record view opens, and not the cell inspector.
    assert_eq!(app.mode, Mode::Record);
    // The column is open already, so the fields need no second key.
    for field in ["login", "gravatar_id"] {
        assert!(s.contains(field), "field {field} missing\n{s}");
    }
    assert!(s.contains("petroav"), "value missing\n{s}");
    assert!(s.contains("(empty)"), "an empty text is not a raw quotation mark\n{s}");
    // The line of the structure holds the count, and not the raw text.
    assert!(s.contains("{3 fields}"), "no short form\n{s}");

    // The cursor of the record view is on the column that the user chose.
    assert_eq!(app.record_line().map(|l| l.label), Some("actor".into()));

    // Esc goes back to the grid, on the same column.
    press(&mut app, ratatui::crossterm::event::KeyCode::Esc);
    settle(&mut app, &mut term);
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.schema.columns[app.cursor_col].name, "actor");
}

#[test]
fn enter_on_a_list_cell_drills_into_it_too() {
    let body = "[{\"id\":1,\"tags\":[\"red\",\"green\",\"blue\"]}]";
    let p = write_sample("enter-list", body, "json");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::ColRight); // the column `tags`
    app.run(Cmd::InspectCell);
    for _ in 0..10 {
        if !app.record_tree.is_empty() {
            break;
        }
        settle(&mut app, &mut term);
    }
    let s = screen(&term);
    assert_eq!(app.mode, Mode::Record);
    assert!(s.contains("[3 items]"), "no short form for a list\n{s}");
    assert!(s.contains("green"), "an item of the list is missing\n{s}");
}

#[test]
fn enter_on_a_plain_cell_still_opens_the_cell_inspector() {
    let p = write_sample("enter-plain", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    app.run(Cmd::ColRight); // the column `name`
    app.run(Cmd::InspectCell);
    settle(&mut app, &mut term);
    assert_eq!(app.mode, Mode::Cell);
    assert!(screen(&term).contains("alice"), "{}", screen(&term));
}

/// Writes a DuckDB database file in a new directory and gives its path.
///
/// A database is the one source that DuckDB itself opens, so a test of the whole
/// program needs a true file. The test writes it with a connection of its own,
/// and it closes that connection before Peruse attaches the file.
fn write_database(tag: &str, sql: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("peruse-render-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("shop.duckdb");
    let conn = duckdb::Connection::open(&p).unwrap();
    conn.execute_batch(sql).unwrap();
    drop(conn);
    p
}

/// Opens one table of a database, with the index at open turned on.
fn open_table(path: &Path, table: &str) -> (App, Terminal<TestBackend>) {
    let opts = OpenOptions {
        table: Some(table.to_string()),
        ..Default::default()
    };
    let (worker, opened) = Worker::spawn(path.to_str().unwrap(), opts).unwrap();
    // The last argument turns the index at open on. A database needs no index,
    // and this test proves that Peruse builds none.
    let mut app = App::new(
        worker,
        opened,
        peruse_core::theme::builtin("peruse-dark").unwrap(),
        true,
    );
    app.config_path = path.parent().map(|d| d.join("config.toml"));
    let terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
    (app, terminal)
}

#[test]
fn a_table_of_a_database_draws_as_a_file_does_and_needs_no_index() {
    let p = write_database(
        "duckdb",
        "CREATE TABLE customers AS SELECT 1 AS id, 'ann' AS name;\n\
         CREATE TABLE orders AS SELECT i AS id, ('c' || i) AS code, \
                (i * 1.25) AS total FROM range(6) t(i);",
    );
    let (mut app, mut term) = open_table(&p, "orders");

    // A table of a database gives direct access already, so `App::new` must
    // start no index. The test reads this before the first frame: after the
    // index of a small file arrives, both values say the same thing again.
    assert!(app.seekable, "a database table gives direct access");
    assert!(!app.indexing, "the program started an index for nothing");

    settle(&mut app, &mut term);
    let s = screen(&term);

    // The title bar names the file and the table. The name of the file alone
    // would not say which rows the grid shows.
    assert!(s.contains("shop.duckdb"), "no file name\n{s}");
    assert!(s.contains("main.orders"), "no table name\n{s}");
    assert!(s.contains("6 × 3"), "no shape\n{s}");
    assert!(s.contains("duckdb"), "no format\n{s}");
    for value in ["id", "code", "total", "c3"] {
        assert!(s.contains(value), "missing {value}\n{s}");
    }

    // The note that asks for the key I must stay away, and no message may say
    // that Peruse copied the table.
    assert!(!s.contains("press I"), "the note asks for an index\n{s}");
    assert!(!s.contains("indexed"), "the program copied the table\n{s}");

    // The metadata panel shows the two statements that open the table. A user
    // can paste them into another program and read the same rows.
    app.run(Cmd::ToggleMeta);
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert!(s.contains("ATTACH"), "no attach statement\n{s}");
    assert!(s.contains("READ_ONLY"), "the panel hides the read-only flag\n{s}");
}

#[test]
fn a_statement_over_a_database_reaches_a_second_table() {
    let p = write_database(
        "duckdb-join",
        "CREATE TABLE customers AS SELECT 1 AS id, 'ann' AS name;\n\
         CREATE TABLE orders AS SELECT 1 AS customer, 5 AS qty;",
    );
    let (mut app, mut term) = open_table(&p, "orders");
    settle(&mut app, &mut term);

    // The alias of the attached database is in the metadata panel, so a user
    // can join the table on the screen with another table of the same file.
    app.view.base = Base::Sql(
        "SELECT c.name, o.qty FROM src o \
         JOIN __peruse_db.main.customers c ON c.id = o.customer"
            .into(),
    );
    app.run_startup_view();
    settle(&mut app, &mut term);
    let s = screen(&term);
    assert!(s.contains("ann"), "the second table did not arrive\n{s}");
}

// ------------------------------------------------------------- the mouse

/// Gives one mouse event that is not a press of the left button.
fn mouse(app: &mut App, kind: ratatui::crossterm::event::MouseEventKind, x: u16, y: u16) -> bool {
    use ratatui::crossterm::event::{KeyModifiers, MouseEvent};
    app.on_mouse(&MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    })
}

/// Gives one turn of the wheel at a position of the terminal.
fn wheel(
    app: &mut App,
    kind: ratatui::crossterm::event::MouseEventKind,
    mods: ratatui::crossterm::event::KeyModifiers,
    x: u16,
    y: u16,
) -> bool {
    use ratatui::crossterm::event::MouseEvent;
    app.on_mouse(&MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: mods,
    })
}

/// Draws one frame, as the loop of the program does after an event.
fn frame(app: &mut App, term: &mut Terminal<TestBackend>) {
    term.draw(|f| ui::draw(f, app, Depth::True)).unwrap();
}

/// Gives the box of the overlay that the last frame drew.
fn overlay_box(app: &App) -> ratatui::layout::Rect {
    app.overlay
        .as_ref()
        .expect("the frame wrote no box for the overlay")
        .area
}

/// Gives the row of the terminal of one line of the list of the overlay, with
/// the position of that line in the list.
fn overlay_line(app: &App, n: usize) -> (u16, usize) {
    let hit = app.overlay.as_ref().expect("no box for the overlay");
    *hit.lines.get(n).expect("the overlay drew no such line")
}

/// Gives the row of the terminal that holds a text.
fn row_of(term: &Terminal<TestBackend>, text: &str) -> u16 {
    lines(term)
        .iter()
        .position(|l| l.contains(text))
        .unwrap_or_else(|| panic!("no row holds {text:?}"))
        as u16
}

#[test]
fn a_click_on_the_cell_of_the_cursor_opens_the_record_view() {
    use ratatui::crossterm::event::KeyCode;
    let p = write_sample("mouse-double", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    let y = row_of(&term, "carol");
    let (_, x, _) = app.hit.cols[1];

    // The first click on a cell chooses it and no more. A user who only wants to
    // read another cell must not get a large box on top of the data.
    assert!(click(&mut app, x, y));
    assert_eq!(app.mode, Mode::Normal, "the first click opened a box");
    assert_eq!(app.cursor_row, 2, "the click landed on another row");
    assert_eq!(app.cursor_col, 1, "the click landed on another column");

    // A click on the cell that the cursor is on opens the record view, as the
    // key `r` does. This is the rule that a user asked for: a click on a cell
    // opens that record.
    assert!(click(&mut app, x, y));
    assert_eq!(app.mode, Mode::Record);
    settle(&mut app, &mut term);
    assert!(
        screen(&term).contains("carol"),
        "the record view shows another row\n{}",
        screen(&term)
    );

    // A click on a different cell chooses that cell and opens nothing, whatever
    // came before it.
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.mode, Mode::Normal);
    // Draw the grid again. The record view covered it, and the next position
    // must come from the grid and not from the box that was on top of it.
    settle(&mut app, &mut term);
    let other = row_of(&term, "alice");
    assert!(click(&mut app, x, other));
    assert_eq!(
        app.mode,
        Mode::Normal,
        "a click on another cell opened the record"
    );
    assert_eq!(app.cursor_row, 0, "the click landed on another row");

    // The same cell again, and it opens.
    assert!(click(&mut app, x, other));
    assert_eq!(app.mode, Mode::Record);
}

#[test]
fn a_click_outside_an_overlay_closes_it_and_a_click_inside_does_not() {
    let p = write_sample("mouse-outside", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    // Each box that covers the grid. A click outside gives the grid back, as
    // Esc does, so the user never has to look for a key to leave.
    for cmd in [
        Cmd::Record,
        Cmd::Help,
        Cmd::InspectCell,
        Cmd::Palette,
        Cmd::ThemePicker,
        Cmd::Settings,
        Cmd::FilterBuild,
    ] {
        app.run(cmd);
        settle(&mut app, &mut term);
        assert_ne!(app.mode, Mode::Normal, "{cmd:?} opened no box");
        let area = overlay_box(&app);

        // The border is inside the box.
        click(&mut app, area.x, area.y);
        assert_ne!(app.mode, Mode::Normal, "{cmd:?} closed on a click inside it");
        click(
            &mut app,
            area.x + area.width / 2,
            area.y + area.height.saturating_sub(1),
        );
        assert_ne!(app.mode, Mode::Normal, "{cmd:?} closed on a click inside it");

        // The title bar of the screen is outside each box.
        assert!(click(&mut app, 0, 0), "{cmd:?} did nothing for a click outside");
        assert_eq!(app.mode, Mode::Normal, "{cmd:?} stayed open");
        settle(&mut app, &mut term);
        assert!(
            app.overlay.is_none(),
            "{cmd:?} left its box behind after it closed"
        );
    }
}

#[test]
fn a_click_in_the_record_view_selects_the_line_under_the_pointer() {
    // The record of a row with 60 fields is longer than the box, so the list
    // scrolls. A click must find the line under the pointer, and not the line
    // at that offset in the list.
    let p = write_sample("mouse-record-scroll", &wide_sample(), "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Record);
    settle(&mut app, &mut term);

    press_char(&mut app, 'G');
    settle(&mut app, &mut term);
    assert!(app.record_sel > 0, "the list did not move to the last field");

    let (y, _) = overlay_line(&app, 0);
    let row = lines(&term)[y as usize].clone();
    let area = overlay_box(&app);
    assert!(click(&mut app, area.x + 3, y));

    let line = app.record_line().expect("no line under the pointer");
    assert!(
        row.contains(&line.label),
        "the click selected another line: the row {row:?} against the field {:?}",
        line.label
    );
    assert!(
        app.record_sel > 0,
        "the click used the offset on the screen as the position in the list"
    );
}

#[test]
fn a_click_opens_and_closes_a_value_that_holds_other_values() {
    let body = "[{\"id\":1,\"actor\":{\"login\":\"petroav\",\"site\":\"github\"}}]";
    let p = write_sample("mouse-drill", body, "json");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Record);
    settle(&mut app, &mut term);

    let y = row_of(&term, "actor");
    let area = overlay_box(&app);

    // The first click chooses the line. A user who only wants to read another
    // line must not open or close a value by accident, so the value stays shut.
    assert!(click(&mut app, area.x + 3, y));
    settle(&mut app, &mut term);
    assert!(
        !screen(&term).contains("login"),
        "the first click opened the value\n{}",
        screen(&term)
    );

    // A click on the line that is chosen already opens the value, as the key
    // Space does. The click lands on another column of that line: two presses at
    // one position inside a short time are a double click, and a double click
    // does what Enter does.
    assert!(click(&mut app, area.x + 6, y));
    settle(&mut app, &mut term);
    assert!(
        screen(&term).contains("login"),
        "the click did not open the value\n{}",
        screen(&term)
    );

    // The next click on that line closes it again.
    assert!(click(&mut app, area.x + 9, y));
    settle(&mut app, &mut term);
    assert!(
        !screen(&term).contains("login"),
        "the click did not close the value\n{}",
        screen(&term)
    );

    // A double click on the line opens it and leaves it open.
    click(&mut app, area.x + 3, y);
    click(&mut app, area.x + 3, y);
    settle(&mut app, &mut term);
    assert!(
        screen(&term).contains("login"),
        "the double click left the value closed\n{}",
        screen(&term)
    );
}

#[test]
fn a_double_click_on_a_field_of_the_record_shows_the_value_in_full() {
    // The record view cuts a long value at the right edge. Enter opens the
    // inspector on a single value, and the double click does the same.
    let p = write_sample("mouse-inspect", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Record);
    settle(&mut app, &mut term);

    let y = row_of(&term, "alice");
    let area = overlay_box(&app);
    click(&mut app, area.x + 3, y);
    assert_eq!(app.mode, Mode::Record, "one click opened the inspector");
    click(&mut app, area.x + 3, y);
    assert_eq!(app.mode, Mode::Cell);
    settle(&mut app, &mut term);
    assert!(
        screen(&term).contains("alice"),
        "the inspector shows another value\n{}",
        screen(&term)
    );

    // The inspector came from the record view, so a click outside it goes back
    // to the record view, exactly as Esc does.
    click(&mut app, 0, 0);
    assert_eq!(app.mode, Mode::Record);
}

#[test]
fn a_click_previews_a_theme_and_a_double_click_keeps_it() {
    let p = write_sample("mouse-theme", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::ThemePicker);
    settle(&mut app, &mut term);

    let (y, at) = overlay_line(&app, 2);
    let want = app.themes[at].name.clone();
    assert_ne!(want, app.theme.name, "the test points at the current theme");
    let area = overlay_box(&app);

    assert!(click(&mut app, area.x + 3, y));
    assert_eq!(app.theme_sel, at);
    assert_eq!(app.theme.name, want, "the click did not preview the theme");
    assert_eq!(app.mode, Mode::ThemePicker, "one click closed the picker");

    // The second press keeps the theme, as Enter does.
    assert!(click(&mut app, area.x + 3, y));
    assert_eq!(app.mode, Mode::Normal);
    assert_eq!(app.theme.name, want);
    assert_eq!(app.themes[app.theme_idx].name, want, "the theme came back");
}

#[test]
fn a_click_selects_a_setting_and_a_double_click_starts_to_edit_it() {
    let p = write_sample("mouse-settings", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Settings);
    settle(&mut app, &mut term);

    let (first_y, _) = overlay_line(&app, 0);
    let (y, at) = overlay_line(&app, 3);
    let area = overlay_box(&app);
    assert!(click(&mut app, area.x + 3, y));
    assert_eq!(app.settings_sel, at);
    assert!(!app.settings_editing, "one click started an edit");

    assert!(click(&mut app, area.x + 3, y));
    assert!(app.settings_editing, "the double click started no edit");

    // The value has the focus while the user types it. A click on another
    // setting must not lose what the user typed.
    assert!(!click(&mut app, area.x + 3, first_y));
    assert!(app.settings_editing);
    assert_eq!(app.settings_sel, at);
}

#[test]
fn a_click_selects_a_command_of_the_palette_and_a_double_click_runs_it() {
    let p = write_sample("mouse-palette", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Palette);
    type_text(&mut app, "settings");
    settle(&mut app, &mut term);

    let want = app
        .palette_items()
        .iter()
        .position(|c| *c == Cmd::Settings)
        .expect("the query found no settings command");
    let hit = app.overlay.as_ref().expect("no box for the palette");
    let (y, at) = *hit
        .lines
        .iter()
        .find(|(_, i)| *i == want)
        .expect("the command is not on the screen");
    let area = overlay_box(&app);

    assert!(click(&mut app, area.x + 3, y));
    assert_eq!(app.palette_sel, at);
    assert_eq!(app.mode, Mode::Palette, "one click ran a command");

    assert!(click(&mut app, area.x + 3, y));
    assert_eq!(app.mode, Mode::Settings, "the double click ran no command");
}

#[test]
fn a_movement_of_the_pointer_draws_no_frame() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    // The terminal reports each movement of the pointer while the mouse is on.
    // A frame for each of them would spend the processor of the user to draw
    // the same screen again.
    let p = write_sample("mouse-moved", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);

    let quiet = [
        MouseEventKind::Moved,
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
        MouseEventKind::Down(MouseButton::Right),
    ];
    for kind in quiet {
        assert!(!mouse(&mut app, kind, 20, 4), "the grid moved for {kind:?}");
    }

    app.run(Cmd::Record);
    settle(&mut app, &mut term);
    let area = overlay_box(&app);
    for kind in quiet {
        assert!(
            !mouse(&mut app, kind, area.x + 3, area.y + 2),
            "the record view moved for {kind:?}"
        );
        assert_eq!(app.mode, Mode::Record, "{kind:?} closed the record view");
    }
}

#[test]
fn the_wheel_moves_the_rows_and_the_shift_key_moves_the_columns() {
    use ratatui::crossterm::event::{KeyModifiers, MouseEventKind};
    let p = write_sample("mouse-wheel-grid", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    let none = KeyModifiers::NONE;

    assert!(wheel(&mut app, MouseEventKind::ScrollDown, none, 20, 4));
    assert_eq!(app.cursor_row, 3, "the wheel moves three rows");
    assert!(wheel(&mut app, MouseEventKind::ScrollUp, none, 20, 4));
    assert_eq!(app.cursor_row, 0);

    // The shift key with the wheel moves to the side, and the row stays.
    assert!(wheel(
        &mut app,
        MouseEventKind::ScrollDown,
        KeyModifiers::SHIFT,
        20,
        4
    ));
    assert_eq!(app.cursor_col, 2, "the wheel with Shift moves two columns");
    assert_eq!(app.cursor_row, 0, "the wheel with Shift moved a row too");
    // A wheel that turns to the side does the same, with no modifier.
    assert!(wheel(&mut app, MouseEventKind::ScrollLeft, none, 20, 4));
    assert_eq!(app.cursor_col, 0);
}

#[test]
fn the_wheel_moves_the_selection_of_an_overlay() {
    use ratatui::crossterm::event::{KeyModifiers, MouseEventKind};
    let p = write_sample("mouse-wheel-overlay", &wide_sample(), "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::Record);
    settle(&mut app, &mut term);
    assert_eq!(app.record_sel, 0);
    let none = KeyModifiers::NONE;

    let area = overlay_box(&app);
    let (x, y) = (area.x + 3, area.y + 2);
    assert!(wheel(&mut app, MouseEventKind::ScrollDown, none, x, y));
    assert_eq!(app.record_sel, 3, "the wheel moved another number of lines");
    assert!(wheel(&mut app, MouseEventKind::ScrollUp, none, x, y));
    assert_eq!(app.record_sel, 0);
}

#[test]
fn the_wheel_never_writes_in_a_box_that_takes_text() {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseEventKind};
    // An arrow key inside a box that takes text belongs to the text: it walks
    // the history of the box and puts an older line in it. A turn of the wheel
    // must never do that.
    let p = write_sample("mouse-wheel-text", &wide_sample(), "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    let none = KeyModifiers::NONE;
    // Put one line in the history that each prompt of the program shares.
    app.input.set("amount > 100");
    app.input.remember();

    // The find box of the record view. The list of the fields is still on the
    // screen under it, so the wheel moves that list and leaves the text.
    app.run(Cmd::Record);
    settle(&mut app, &mut term);
    press_char(&mut app, '/');
    type_text(&mut app, "col_1");
    settle(&mut app, &mut term);
    assert!(app.record_finding);
    assert!(app.record_lines().len() > 4, "too few fields for the test");

    assert!(wheel(&mut app, MouseEventKind::ScrollDown, none, 20, 8));
    assert_eq!(app.record_sel, 3, "the wheel did not move the list");
    assert!(wheel(&mut app, MouseEventKind::ScrollUp, none, 20, 8));
    assert_eq!(app.record_sel, 0);
    assert_eq!(app.record_find, "col_1", "the wheel wrote in the find box");
    assert!(app.record_finding, "the wheel left the find box");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.mode, Mode::Normal);

    // The value of a setting. The selection must stay where it is: Enter
    // writes the text into the setting under the selection.
    app.run(Cmd::Settings);
    settle(&mut app, &mut term);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert!(app.settings_editing);
    let sel = app.settings_sel;
    app.input.set("7");
    for kind in [MouseEventKind::ScrollUp, MouseEventKind::ScrollDown] {
        assert!(
            !wheel(&mut app, kind, none, 20, 8),
            "the wheel acted while a value was being typed"
        );
        assert_eq!(app.settings_sel, sel, "the wheel went to another setting");
        assert_eq!(app.input.text(), "7", "the wheel wrote in the value");
    }
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.mode, Mode::Normal);

    // The value step of the filter builder holds a prompt and no list, so the
    // wheel has nothing to move.
    app.run(Cmd::FilterBuild);
    settle(&mut app, &mut term);
    assert_eq!(app.build, Build::Column, "the builder opened at another step");
    press(&mut app, KeyCode::Enter); // take the column
    press(&mut app, KeyCode::Enter); // take the operator
    assert_eq!(app.build, Build::Value);
    app.input.set("v42");
    for kind in [MouseEventKind::ScrollUp, MouseEventKind::ScrollDown] {
        assert!(
            !wheel(&mut app, kind, none, 20, 8),
            "the wheel acted in a step that takes text"
        );
        assert_eq!(app.build, Build::Value, "the wheel left the step");
        assert_eq!(app.input.text(), "v42", "the wheel wrote in the box");
    }
}

#[test]
fn a_double_click_in_a_long_list_keeps_the_line_of_the_first_press() {
    // A list keeps the selected line near the middle, so a new selection moves
    // the window. The frame between the two presses of a double click then
    // puts another line under the pointer, and the second press must still act
    // on the line that the first press chose.
    let p = write_sample("mouse-double-scroll", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    app.run(Cmd::ThemePicker);
    frame(&mut app, &mut term);

    let hit = app.overlay.as_ref().expect("no box for the theme picker");
    assert!(
        hit.lines.len() < app.themes.len(),
        "the list holds each theme, so it never moves"
    );
    // The last line of the window is the furthest from the middle.
    let (y, at) = *hit.lines.last().expect("the picker drew no line");
    let want = app.themes[at].name.clone();
    let area = overlay_box(&app);

    assert!(click(&mut app, area.x + 3, y));
    assert_eq!(app.theme_sel, at);
    frame(&mut app, &mut term);
    assert_ne!(
        app.overlay.as_ref().unwrap().line_at(y),
        Some(at),
        "the list stayed still, so this test proves nothing"
    );

    assert!(click(&mut app, area.x + 3, y));
    assert_eq!(app.mode, Mode::Normal, "the double click kept no theme");
    assert_eq!(
        app.themes[app.theme_idx].name, want,
        "the double click kept the theme that the list moved under the pointer"
    );
}

#[test]
fn a_click_under_the_last_row_of_the_file_moves_nothing() {
    // The grid keeps the rows under the last one empty. There is no cell
    // there, so a click must change nothing: without this, a click on the
    // empty part of the screen would send the cursor to the last row.
    let p = write_sample("mouse-past-end", SAMPLE, "csv");
    let (mut app, mut term) = open(&p);
    settle(&mut app, &mut term);
    assert_eq!(app.cursor_row, 0);

    let y = row_of(&term, "erin") + 1;
    assert!(
        app.hit.row_at(y).is_some(),
        "the row under the last one is outside the grid"
    );
    let (_, x, _) = app.hit.cols[1];
    assert!(!click(&mut app, x, y), "the click acted under the last row");
    assert_eq!(app.cursor_row, 0, "the click went to the last row");
    assert_eq!(app.cursor_col, 0, "the click moved to another column");

    // A second press at the same place must not open the record view either.
    assert!(!click(&mut app, x, y));
    assert_eq!(app.mode, Mode::Normal);
}

#[test]
fn the_mouse_does_not_stop_the_program_on_a_small_terminal() {
    let p = write_sample("mouse-small", SAMPLE, "csv");
    let (mut app, _) = open(&p);
    for (w, h) in [(10u16, 4u16), (20, 6), (40, 10)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        settle(&mut app, &mut term);
        for cmd in [
            Cmd::Record,
            Cmd::Help,
            Cmd::Palette,
            Cmd::Settings,
            Cmd::ThemePicker,
            Cmd::FilterBuild,
        ] {
            app.run(cmd);
            settle(&mut app, &mut term);
            for x in [0, w / 2, w - 1] {
                for y in [0, h / 2, h - 1] {
                    click(&mut app, x, y);
                    settle(&mut app, &mut term);
                }
            }
            app.mode = Mode::Normal;
            app.settings_editing = false;
        }
    }
}
