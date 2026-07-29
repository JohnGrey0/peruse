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

use crate::app::{App, Build, Mode, Panel};
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

    for label in ["theme", "threads", "memory limit", "sample size"] {
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
