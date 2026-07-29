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
use ratatui::crossterm::event::{self, Event};

use crate::app::App;
use crate::colors::Depth;

/// The options of the command line.
#[derive(Parser, Debug)]
#[command(
    name = "peruse",
    version,
    about = "A fast, read-only viewer for Parquet, CSV, TSV and JSON data files.",
    after_help = "Peruse never writes to your data. Press ? inside for keys.\n\n\
                  FORMATS:\n  \
                  parquet (.parquet .parq .pq)   csv (.csv .tsv .tab .psv)\n  \
                  json (.json .ndjson .jsonl)\n  \
                  Add .gz, .zst or .bz2 to any text format.\n\n\
                  EXAMPLES:\n  \
                  peruse trips.parquet\n  \
                  peruse 'data/*.parquet'\n  \
                  peruse events.ndjson\n  \
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

    /// The table name for --ddl. The default is the name of the file.
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

fn main() -> Result<()> {
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
    let opts = OpenOptions {
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
    };

    // The option --ddl writes to the standard output and stops. The terminal
    // never starts, so the result goes into a file or into another program.
    if let Some(name) = &cli.ddl {
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

    let (worker, opened) = Worker::spawn(&file, opts)?;

    // Remember the file, so that the chooser can offer it at the next start.
    // The list holds the full path: a name by itself does not say which file
    // it is. A glob is not a file, and it does not go into the list.
    let mut config = config;
    if !peruse_core::source::looks_like_glob(&file)
        && let Ok(full) = std::fs::canonicalize(&file)
    {
        config.remember_file(&peruse_core::source::tidy_path(&full));
        if let Some(p) = Config::path() {
            // A list that Peruse cannot write is not a reason to stop. The
            // user came here to look at a file.
            let _ = config.save_to(&p);
        }
    }

    let auto_index = !(cli.no_index || config.no_index.unwrap_or(false));
    let mut application = App::new(worker, opened, theme, auto_index);
    application.config = config;
    // The user asked for these panels in an earlier session. Put them on the
    // screen, and ask for what they need.
    if let Some(name) = application.config.panels.clone() {
        application.set_panel_from_setting(&name);
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
    if application.view != View::default() {
        application.run_startup_view();
    }

    run(&mut application)
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

    // Take the name of the file, with no directory and no extension, as the
    // name of the table. A name that a database rejects is no use, so change
    // each character that is not a letter, a digit or `_`.
    let name = table.unwrap_or_else(|| {
        let stem = std::path::Path::new(&engine.source.label)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "data".into());
        let clean: String = stem
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
            .collect();
        // A name that starts with a digit is not legal in most databases.
        if clean.starts_with(|c: char| c.is_ascii_digit()) {
            format!("t_{clean}")
        } else {
            clean
        }
    });

    let profile = engine
        .profile(&view, &name)
        .with_context(|| format!("measuring {file}"))?;
    print!("{}", peruse_core::ddl::render(&profile, d));
    Ok(())
}

/// Draws the frames and reads the events until the user quits.
fn run(application: &mut App) -> Result<()> {
    let depth = Depth::detect();
    let mut terminal = ratatui::init();

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
    let result = loop {
        if let Err(e) = terminal.draw(|f| ui::draw(f, application, depth)) {
            break Err(e.into());
        }
        application.ensure_rows();
        if application.quit {
            break Ok(());
        }

        select! {
            recv(key_rx) -> ev => {
                let Ok(ev) = ev else { break Ok(()) };
                handle_event(application, ev);
                // Read the other events of a group before the next frame. A
                // key that the user holds down then moves the cursor at full
                // speed. Without this loop, each event needs one frame.
                while let Ok(ev) = key_rx.try_recv() {
                    handle_event(application, ev);
                    if application.quit { break; }
                }
            }
            recv(resp_rx) -> resp => {
                let Ok(resp) = resp else { break Ok(()) };
                application.on_response(resp);
                while let Ok(resp) = resp_rx.try_recv() {
                    application.on_response(resp);
                }
            }
        }
    };

    ratatui::restore();
    result
}

/// Gives one terminal event to the application.
fn handle_event(application: &mut App, ev: Event) {
    match ev {
        // Windows sends one event for the press of a key and one event for
        // the release. Use the press only, or each key acts two times.
        Event::Key(k) if k.is_press() => application.on_key(&k),
        _ => {}
    }
}
