//! Peruse: a fast viewer for Parquet files and CSV files. It reads the data,
//! and it does not change the data.
//!
//! This file holds three things: the options of the command line, the start
//! and the end of the terminal, and the event loop.

mod app;
mod browser;
mod clip;
mod colors;
mod commands;
mod copying;
mod grid;
mod input;
mod overlays;
mod paint;
mod panels;
#[cfg(test)]
mod render_test;
mod sqlhl;
mod text;
mod tree;
mod ui;

use anyhow::{Context, Result};
use peruse_core::config::Config;
use clap::{CommandFactory, Parser};
use crossbeam_channel::{select, unbounded};
use peruse_core::query::Base;
use peruse_core::{sql_guard, OpenOptions, View, Worker};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event,
};
use ratatui::crossterm::execute;

use crate::app::App;
use crate::colors::Depth;

/// The options of the command line.
#[derive(Parser, Debug)]
#[command(
    name = "peruse",
    version,
    about = "A fast, read-only viewer for Parquet, CSV, TSV, JSON and Arrow files and DuckDB databases.",
    after_help = "Peruse never writes to your data. Press ? inside for keys.\n\n\
                  FORMATS:\n  \
                  parquet (.parquet .parq .pq)   csv (.csv .tsv .tab .psv)\n  \
                  json (.json .ndjson .jsonl)   duckdb (.duckdb .ddb)\n  \
                  Add .gz, .zst or .bz2 to any text format.\n  \
                  A database opens read-only, and Peruse shows one table of it.\n\n\
                  EXAMPLES:\n  \
                  peruse trips.parquet\n  \
                  peruse 'data/*.parquet'\n  \
                  peruse events.ndjson\n  \
                  peruse shop.duckdb --table orders\n  \
                  peruse big.csv --filter \"amount > 100\"\n  \
                  peruse sales.csv -q \"SELECT region, sum(amount) FROM src GROUP BY 1\""
)]
struct Cli {
    /// File or glob to open, e.g. data.parquet or 'part-*.csv'
    ///
    /// The option is not required. A call with no file prints this help,
    /// because that is what a user who types the name of the program alone
    /// wants to see.
    #[arg(value_name = "FILE")]
    file: Option<String>,

    /// Start with this SQL instead of the whole file. The file is the view `src`.
    #[arg(short = 'q', long)]
    query: Option<String>,

    /// Start with this WHERE expression applied
    #[arg(short = 'f', long)]
    filter: Option<String>,

    /// Colour theme name, or a path to a .toml theme
    #[arg(short = 't', long)]
    theme: Option<String>,

    /// List available themes and exit
    #[arg(long)]
    list_themes: bool,

    /// Print a CREATE TABLE statement for this database and exit.
    /// One of: oracle, mysql, postgres, snowflake, bigquery, sqlserver,
    /// duckdb, dynamodb
    #[arg(long, value_name = "DIALECT")]
    ddl: Option<String>,

    /// The table of a database to read, and the table name for --ddl.
    /// The default name for --ddl is the table, or the name of the file.
    #[arg(long, value_name = "NAME")]
    table: Option<String>,

    /// Override the CSV delimiter (e.g. ';' or 'tab')
    #[arg(long, value_name = "CHAR")]
    delimiter: Option<String>,

    /// Treat the first CSV row as data, not headers
    #[arg(long)]
    no_header: bool,

    /// Read every CSV column as text, skipping type inference
    #[arg(long)]
    all_varchar: bool,

    /// Skip malformed CSV rows instead of failing
    #[arg(long)]
    ignore_errors: bool,

    /// Rows the CSV sniffer inspects; -1 reads the whole file
    #[arg(long, value_name = "N")]
    sample_size: Option<i64>,

    /// Worker threads (default: all cores)
    #[arg(long, value_name = "N")]
    threads: Option<usize>,

    /// Memory ceiling before spilling to disk, e.g. 4GB
    #[arg(long, value_name = "SIZE")]
    memory_limit: Option<String>,

    /// Do not index CSVs on open, even small ones
    #[arg(long)]
    no_index: bool,

    /// Ignore the mouse, so the terminal selects text with it as usual
    #[arg(long)]
    no_mouse: bool,

    /// Copy the file to the local disk first, from any drive and of any size
    #[arg(long, conflicts_with = "no_local")]
    local: bool,

    /// Read the file in place, also from a network drive
    #[arg(long)]
    no_local: bool,
}

/// Reads the argument of the option `--delimiter`.
///
/// The function accepts the word `tab`, the two characters `\t`, the word
/// `space`, or one character.
fn parse_delimiter(s: &str) -> Result<char> {
    let c = match s {
        "tab" | "\\t" | "t" => '\t',
        "space" => ' ',
        other => {
            let mut it = other.chars();
            let c = it
                .next()
                .with_context(|| format!("--delimiter needs a character, got {s:?}"))?;
            anyhow::ensure!(it.next().is_none(), "--delimiter must be one character, got {s:?}");
            c
        }
    };
    Ok(c)
}

/// Runs the program, and writes an error in a form that is safe for the
/// terminal.
///
/// The message of an error can hold the data of the file: the CSV reader of
/// DuckDB quotes the line that it cannot read. A file can therefore put an
/// escape sequence into the message, and the terminal would obey it. It could
/// clear the screen, change the title of the window, or write to the clipboard.
/// The usual `main` that gives a `Result` writes the message as it is, so this
/// function writes it through [`text::for_terminal`].
fn main() -> std::process::ExitCode {
    match run_main() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {}", text::for_terminal(&format!("{e:?}")));
            std::process::ExitCode::FAILURE
        }
    }
}

fn run_main() -> Result<()> {
    let cli = Cli::parse();

    if cli.list_themes {
        for t in peruse_core::theme::available() {
            println!("{}", t.name);
        }
        if let Some(dir) = peruse_core::theme::user_theme_dir() {
            println!("\nDrop .toml themes in {} to add your own.", dir.display());
        }
        return Ok(());
    }

    // The settings file gives the default for each option. The command line
    // wins over it, so a user can test an option one time without a change to
    // the file. A mistake in the file gives a message, and Peruse still opens
    // the file: a setting is not a reason to refuse the work.
    let (config, mut config_error) = Config::load();

    // A theme from the command line must be right, so a mistake in it stops
    // the program. A theme from the settings file must never do that: a name
    // that a later version removes would lock the user out of the program,
    // and the only way back would be to edit the file by hand.
    let theme = match &cli.theme {
        Some(name) => peruse_core::theme::resolve(name).map_err(anyhow::Error::msg)?,
        None => {
            let (theme, why) = config.theme_or_default();
            if let Some(w) = why {
                config_error.get_or_insert(w);
            }
            theme
        }
    };

    // A call with no file opens the chooser. A user who types the name of the
    // program alone wants to look at some data, and a page of help is not an
    // answer to that wish.
    //
    // Two calls still get the help. A terminal that is not there cannot hold
    // a chooser, so a call inside a pipeline must not wait for a key. The
    // option `--ddl` writes to the standard output, and it needs a file that
    // the caller names.
    // The grid and the chooser both draw on a terminal. Without one there is
    // nothing to draw on, and the escape sequences of a full screen would go
    // into the pipe or into the file.
    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdout());

    let file = match cli.file.clone() {
        Some(f) => f,
        None if cli.ddl.is_some() => {
            anyhow::bail!("--ddl needs a file. Give the path of the file to read.")
        }
        // A call with no file and no terminal is a call from a script that
        // wants to know what this program is.
        None if !interactive => {
            Cli::command().print_help()?;
            println!();
            return Ok(());
        }
        None => match browser::choose(&config, &theme)? {
            Some(p) => p.to_string_lossy().into_owned(),
            // The user left the chooser without a file.
            None => return Ok(()),
        },
    };

    // Check the query and the filter from the command line before Peruse
    // opens the file. A mistake in the text is then one error message on the
    // command line. The terminal does not start and then show an error.
    if let Some(q) = &cli.query {
        sql_guard::ensure_read_only(q).map_err(|e| anyhow::anyhow!("--query: {e}"))?;
    }
    if let Some(f) = &cli.filter {
        sql_guard::ensure_safe_predicate(f).map_err(|e| anyhow::anyhow!("--filter: {e}"))?;
    }

    // With no setting, Peruse gives DuckDB one half of the memory of the
    // machine. Without a limit, DuckDB takes 80 percent of it, and a viewer
    // of data is not the only program that the user runs.
    let resources = peruse_core::config::Resources::read();
    let mut opts = OpenOptions {
        threads: cli.threads.or(config.threads),
        memory_limit: cli
            .memory_limit
            .clone()
            .or_else(|| config.memory_limit_text())
            .or_else(|| resources.default_memory_text()),
        all_varchar: cli.all_varchar,
        ignore_errors: cli.ignore_errors,
        delimiter: cli.delimiter.as_deref().map(parse_delimiter).transpose()?,
        header: cli.no_header.then_some(false),
        sample_size: cli.sample_size.or(config.sample_size),
        // A database holds many tables. The option names the one that the grid
        // shows, and it means nothing for a file.
        table: cli.table.clone(),
    };

    // A large file on a network drive opens from a copy on the local disk.
    // The options of the command line win over the setting, as for each
    // other setting.
    let policy = if cli.local {
        Some(peruse_core::cache::Policy::Always)
    } else if cli.no_local {
        Some(peruse_core::cache::Policy::Never)
    } else {
        None
    };
    let copy_settings = peruse_core::cache::Settings::from_config(&config, policy);

    // The option --ddl writes to the standard output and stops. The terminal
    // never starts, so the result goes into a file or into another program.
    //
    // A copy from an earlier session makes the measurement fast. The option
    // makes no new copy: the measurement reads the whole file one time, and a
    // copy would read it one time more.
    if let Some(name) = &cli.ddl {
        let probe = peruse_core::remote::System;
        if let peruse_core::cache::Plan::Ready(copy) =
            peruse_core::cache::plan(&file, &copy_settings, &probe)
        {
            let rw = copy.rewrite();
            let (table, query, filter) = (cli.table.clone(), cli.query.clone(), cli.filter.clone());
            return print_ddl(&copy.input, &opts, name, table, query, filter)
                .map_err(|e| rw.apply_error(&e));
        }
        return print_ddl(&file, &opts, name, cli.table.clone(), cli.query.clone(), cli.filter.clone());
    }

    // The option --ddl writes text and needs no terminal, and it has left
    // this function already. Each other way through the program draws a
    // screen. A screen that goes into a pipe is a page of escape sequences,
    // and the program would then wait for a key that never arrives.
    if !interactive {
        anyhow::bail!(
            "peruse draws on a terminal, and this one has none.\n\
             To write to a file or to a pipe, use --ddl <dialect> instead."
        );
    }

    // A large file on a network drive gets a copy on the local disk first,
    // with a screen that shows the progress. The copy stays for the next
    // session, and it holds a lease until this session ends. Refer to
    // `peruse_core::cache`.
    let mut note = None;
    let local = match copying::prepare(&file, &copy_settings, &theme)? {
        copying::Outcome::Local(c) => Some(c),
        copying::Outcome::InPlace(why) => {
            note = why;
            None
        }
        copying::Outcome::Quit => return Ok(()),
    };
    let read = local.as_ref().map_or(file.as_str(), |c| c.input.as_str());

    // A database holds many tables, and the grid shows one of them. The user
    // chooses it here, in front of the grid, in the same way as the chooser
    // chooses a file.
    // The picker reads the tables of the copy. A message about it names the
    // source, as each other message does.
    let picked = pick_table(read, cli.query.is_some(), &opts, &theme)
        .map_err(|e| match &local {
            Some(c) => c.rewrite().apply_error(&e),
            None => e,
        });
    match picked? {
        Picked::Table(name) => opts.table = Some(name),
        Picked::NoChoice => {}
        // The user left the picker without a table.
        Picked::Left => return Ok(()),
    }

    let (worker, opened) = match &local {
        // A copy that does not open goes back to the source. The source then
        // gives its own message, or it opens.
        Some(c) => match Worker::spawn_local(c, opts.clone()) {
            Ok(w) => w,
            Err(e) => {
                note = Some(format!(
                    "the local copy did not open ({e:#}). Peruse reads the file in place"
                ));
                Worker::spawn(&file, opts)?
            }
        },
        None => Worker::spawn(&file, opts)?,
    };

    // Remember the file, so that the chooser can offer it at the next start.
    // The list holds the full path: a name by itself does not say which file
    // it is. A glob is not a file, and it does not go into the list.
    let mut config = config;
    if !peruse_core::source::looks_like_glob(&file)
        && let Ok(full) = std::fs::canonicalize(&file)
    {
        remember_recent(
            &mut config,
            &peruse_core::source::tidy_path(&full),
            Config::path().as_deref(),
        );
    }

    let auto_index = !(cli.no_index || config.no_index.unwrap_or(false));
    // A terminal that gives the mouse to the program does not select text with
    // the mouse in the usual way. A user who copies out of the grid with the
    // mouse each day can therefore turn the mouse off.
    let mouse = !cli.no_mouse && config.mouse.unwrap_or(true);
    // The view of the command line goes in before the first request. Refer to
    // `App::idle`.
    let mut application = App::idle(worker, opened, theme);
    application.config = config;
    // The user asked for these panels in an earlier session. Put them on the
    // screen, and ask for what they need.
    if let Some(name) = application.config.panels.clone() {
        application.set_panel_from_setting(&name);
    }
    if application.source.local_copy.is_some()
        && let Some(c) = &local
    {
        application.info(match c.copied {
            Some(t) => format!(
                "reading a local copy: {} copied in {}",
                peruse_core::source::human_bytes(c.bytes),
                copying::human_time(t.as_secs_f64())
            ),
            None => "reading the local copy from an earlier session".to_string(),
        });
    } else if let Some(n) = note {
        application.info(n);
    }
    if let Some(e) = config_error {
        application.error(format!("settings: {e}"));
    }

    if let Some(q) = cli.query {
        application.view.base = Base::Sql(q);
    }
    if let Some(f) = cli.filter {
        application.set_raw_filter(&f);
    }
    application.start(auto_index);

    run(&mut application, mouse)
}

/// Puts a file at the top of the list of recent files, and keeps the list in
/// the settings file when that file has no fault.
///
/// A settings file that Peruse cannot read gives the built-in settings. A write
/// of those settings replaces the whole file, so one typing mistake in it would
/// cost the user each setting and each note, at the next file that the user
/// opens. The file therefore stays as it is, as for each other write. Refer to
/// `app::write_blocked`.
///
/// A list that Peruse cannot write is not a reason to stop. The user came here
/// to look at a file.
fn remember_recent(config: &mut Config, file: &str, path: Option<&std::path::Path>) {
    config.remember_file(file);
    // The function reads the file again and changes the list only, so a
    // setting that another window of Peruse wrote since the start stays.
    if let Some(p) = path {
        let _ = Config::remember_file_at(p, file);
    }
}

/// What the table picker gave back.
enum Picked {
    /// Read this table of the database.
    Table(String),
    /// The source needs no choice: it is not a database, the user named a table
    /// already, or the database holds one table only.
    NoChoice,
    /// The user left the picker. Peruse then stops with no message.
    Left,
}

/// Chooses the table of a database, when the database holds more than one.
///
/// The function reads the tables before the engine opens the file. That read
/// attaches the database read-only and asks its catalog, so it costs no scan of
/// the data. A database that a newer DuckDB wrote therefore gives its message
/// here, on the command line, and not behind a terminal that started already.
///
/// A statement from `--query` skips the picker: the user said what they want,
/// and a question about a table would be in the way. The view `src` then reads
/// the first table of the list, and the statement can name any other table
/// through the database alias that the metadata panel shows.
fn pick_table(
    file: &str,
    have_query: bool,
    opts: &OpenOptions,
    theme: &peruse_core::Theme,
) -> Result<Picked> {
    use peruse_core::source::{detect, looks_like_glob, Format};

    // A glob, a directory and a path that is not there all get their message
    // from the engine, which says what to do about each of them.
    let path = std::path::Path::new(file);
    if opts.table.is_some()
        || looks_like_glob(file)
        || !path.is_file()
        || detect(path).0 != Format::DuckDb
    {
        return Ok(Picked::NoChoice);
    }
    // This attaches the database, and `Engine::open` attaches it again after the
    // user chooses. The file is therefore opened two times, read-only both
    // times. That cost is deliberate: reading the catalog of a database with 60
    // tables takes 14 ms, and the user then spends seconds reading the list. To
    // pay it one time, `Engine::open` would have to take a connection that this
    // function opened, and the open path is where the read-only promise lives.
    // A measurement of 14 ms is not a reason to put a second way into it.
    let tables = peruse_core::engine::database_tables(path, opts)?;
    // One table, and no table at all, both need no question. The engine opens
    // the one table, and it reports the empty database.
    if tables.len() < 2 {
        return Ok(Picked::NoChoice);
    }
    if have_query {
        return Ok(Picked::Table(tables[0].label()));
    }
    match browser::choose_table(path, &tables, theme)? {
        Some(t) => Ok(Picked::Table(t.label())),
        None => Ok(Picked::Left),
    }
}

/// Measures the file and writes a `CREATE TABLE` statement for one database.
///
/// The statement follows the view, so `--query` and `--filter` also change it.
/// A user can therefore write a table for the result of a statement, and not
/// for the file alone.
fn print_ddl(
    file: &str,
    opts: &OpenOptions,
    dialect: &str,
    table: Option<String>,
    query: Option<String>,
    filter: Option<String>,
) -> Result<()> {
    let Some(d) = peruse_core::ddl::Dialect::parse(dialect) else {
        anyhow::bail!(
            "--ddl: {dialect:?} is not a database that Peruse knows.\nTry one of: {}",
            peruse_core::ddl::Dialect::names()
        );
    };

    let engine = peruse_core::Engine::open(file, opts)?;
    let view = View {
        base: match query {
            Some(q) => Base::Sql(q),
            None => Base::Source,
        },
        filter,
        sort: Vec::new(),
    };

    let name = ddl_name(
        table.as_deref(),
        engine.table().map(|t| t.name.as_str()),
        &engine.source.label,
    );

    let profile = engine
        .profile(&view, &name)
        .with_context(|| format!("measuring {file}"))?;
    let text = peruse_core::ddl::render(&profile, d);
    // A name of a column can hold an escape sequence. The statement keeps it
    // when it goes into a file or a pipe, because the name is part of the
    // data. A terminal would obey the sequence, so the terminal gets a visible
    // form of it.
    if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        print!("{}", text::for_terminal(&text));
    } else {
        print!("{text}");
    }
    Ok(())
}

/// Chooses the name of the table in the `CREATE TABLE` statement of `--ddl`.
///
/// For a database, the option `--table` chose which table to read, and the name
/// of that table is then the name of the statement. The catalog of the database
/// holds the true spelling, so `--table orders` and `--table ORDERS` both give
/// the name that the database uses.
///
/// For a file, the option gives the name itself, and the name of the file is the
/// default. A name that a database rejects is no use, so each character that is
/// not a letter, a digit or `_` becomes `_`.
fn ddl_name(asked: Option<&str>, db_table: Option<&str>, label: &str) -> String {
    let raw = match (db_table, asked) {
        (Some(table), _) => table.to_string(),
        (None, Some(name)) => return name.to_string(),
        (None, None) => std::path::Path::new(label)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "data".into()),
    };
    let clean: String = raw
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    // A name that starts with a digit is not legal in most databases.
    if clean.starts_with(|c: char| c.is_ascii_digit()) {
        format!("t_{clean}")
    } else {
        clean
    }
}

/// Turns the mouse on, and makes sure that a panic turns it off again.
///
/// The function gives `false` when the terminal refused the request. Peruse
/// then works with the keys alone, which is not a reason to stop.
fn enable_mouse() -> bool {
    if execute!(std::io::stdout(), EnableMouseCapture).is_err() {
        return false;
    }
    // The function `ratatui::init` puts a hook in place that brings the
    // terminal back on a panic. That hook knows nothing about the mouse. This
    // hook therefore turns the mouse off first, and then calls the hook that
    // was there before it.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        previous(info);
    }));
    true
}

/// Asks the terminal to send a paste as one event, and makes sure that a panic
/// turns that mode off again.
///
/// The hook of `ratatui::init` knows nothing about this mode. A shell after a
/// panic would otherwise get marks around each paste, as text.
fn enable_paste() -> bool {
    if execute!(std::io::stdout(), EnableBracketedPaste).is_err() {
        return false;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableBracketedPaste);
        previous(info);
    }));
    true
}

/// Draws the frames and reads the events until the user quits.
fn run(application: &mut App, mouse: bool) -> Result<()> {
    let depth = Depth::detect();
    let mut terminal = ratatui::init();
    let mouse = mouse && enable_mouse();
    // The terminal then sends a paste as one event. Without it, a statement in
    // more than one line arrives as keys, and its first line break runs the
    // prompt. Refer to `App::on_paste`. A terminal that does not know the
    // request ignores it, and the paste then arrives as keys, as before.
    let paste = enable_paste();

    // A read from the terminal blocks. The reads therefore run on their own
    // thread, and each event arrives as a message. The engine responses
    // arrive as messages on a second channel. The main loop then waits until
    // a message arrives on one of the two channels. It does not examine the
    // channels again and again, and it uses no processor time when it waits.
    let (key_tx, key_rx) = unbounded::<Event>();
    std::thread::Builder::new()
        .name("peruse-input".into())
        .spawn(move || {
            while let Ok(ev) = event::read() {
                if key_tx.send(ev).is_err() {
                    break;
                }
            }
        })?;

    let resp_rx = application.worker.responses().clone();

    // Draw only after something changed. With the mouse on, the terminal
    // reports each movement of the pointer, and Peruse does nothing with those
    // events. A frame for each of them would spend the processor of the user to
    // draw the same screen again.
    let mut dirty = true;
    let result = loop {
        if dirty {
            if let Err(e) = terminal.draw(|f| ui::draw(f, application, depth)) {
                break Err(e.into());
            }
            application.ensure_rows();
        }
        if application.quit {
            break Ok(());
        }
        dirty = false;

        select! {
            recv(key_rx) -> ev => {
                let Ok(ev) = ev else { break Ok(()) };
                dirty |= handle_event(application, ev);
                // Read the other events of a group before the next frame. A
                // key that the user holds down then moves the cursor at full
                // speed. Without this loop, each event needs one frame.
                while let Ok(ev) = key_rx.try_recv() {
                    dirty |= handle_event(application, ev);
                    if application.quit { break; }
                }
            }
            recv(resp_rx) -> resp => {
                // The engine thread stopped while the program runs. That is
                // a fault, and the exit code must say so.
                let Ok(resp) = resp else {
                    break Err(anyhow::anyhow!("the engine stopped unexpectedly"));
                };
                dirty |= application.on_response(resp);
                while let Ok(resp) = resp_rx.try_recv() {
                    dirty |= application.on_response(resp);
                }
            }
        }
    };

    if paste {
        let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    }
    if mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    ratatui::restore();
    result
}

/// Gives one terminal event to the application.
///
/// The function gives `true` when the event can change the screen. An event
/// that changes nothing needs no frame.
fn handle_event(application: &mut App, ev: Event) -> bool {
    match ev {
        // Windows sends one event for the press of a key and one event for
        // the release. Use the press only, or each key acts two times.
        Event::Key(k) if k.is_press() => {
            application.on_key(&k);
            true
        }
        Event::Mouse(m) => application.on_mouse(&m),
        Event::Paste(text) => application.on_paste(&text),
        Event::Resize(_, _) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_settings_file_with_a_fault_is_not_written_over_by_the_list_of_recent_files() {
        let dir = std::env::temp_dir().join("peruse-recent-blocked");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        // One typing mistake. The load gives the built-in settings.
        let bad = "theme = \"nord\"\nstep = 25\nthheme = \"x\"\n";
        std::fs::write(&path, bad).unwrap();
        let (mut config, why) = Config::load_from(&path);
        assert!(why.is_some(), "the file must have a fault for this test");
        remember_recent(&mut config, "/data/a.csv", Some(&path));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bad, "the file was written over");
        assert_eq!(config.recent.first().map(String::as_str), Some("/data/a.csv"));

        // A file with no fault gets the list, and keeps its settings.
        std::fs::write(&path, "theme = \"nord\"\nstep = 25\n").unwrap();
        let (mut config, why) = Config::load_from(&path);
        assert!(why.is_none());
        remember_recent(&mut config, "/data/a.csv", Some(&path));
        let (back, _) = Config::load_from(&path);
        assert_eq!(back.recent, vec!["/data/a.csv".to_string()]);
        assert_eq!(back.step, Some(25));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ddl_name_comes_from_the_table_of_a_database() {
        // The option --table chose the table of the database, so that table
        // names the CREATE TABLE statement. The catalog holds the true
        // spelling, and a name with a space becomes a name that a database
        // accepts.
        assert_eq!(ddl_name(Some("orders"), Some("orders"), "shop.duckdb"), "orders");
        assert_eq!(ddl_name(Some("ORDERS"), Some("Orders"), "shop.duckdb"), "Orders");
        assert_eq!(
            ddl_name(Some("order items"), Some("order items"), "shop.duckdb"),
            "order_items"
        );
        // A database with one table needs no --table, and the name still comes
        // from that table.
        assert_eq!(ddl_name(None, Some("sales"), "shop.duckdb"), "sales");
    }

    #[test]
    fn the_ddl_name_of_a_file_is_the_option_or_the_file() {
        assert_eq!(ddl_name(Some("trips"), None, "yellow.parquet"), "trips");
        assert_eq!(ddl_name(None, None, "yellow.parquet"), "yellow");
        // A name that a database rejects is no use.
        assert_eq!(ddl_name(None, None, "2024-sales.csv"), "t_2024_sales");
    }

    #[test]
    fn the_delimiter_option_reads_a_word_or_a_character() {
        assert_eq!(parse_delimiter("tab").unwrap(), '\t');
        assert_eq!(parse_delimiter("\\t").unwrap(), '\t');
        assert_eq!(parse_delimiter("space").unwrap(), ' ');
        assert_eq!(parse_delimiter(";").unwrap(), ';');
        assert!(parse_delimiter("ab").is_err(), "two characters are not one");
    }
}
