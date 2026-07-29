//! Peruse: a fast viewer for Parquet files and CSV files. It reads the data,
//! and it does not change the data.
//!
//! This file holds three things: the options of the command line, the start
//! and the end of the terminal, and the event loop.

mod app;
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
mod ui;

use anyhow::{Context, Result};
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
    #[arg(short = 't', long, default_value = "peruse-dark")]
    theme: String,

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

    // A call with no file is a request for the help, and not a mistake. The
    // user typed the name of the program to find out what it does, and an
    // error message is the wrong answer to that question.
    let Some(file) = cli.file.clone() else {
        Cli::command().print_help()?;
        println!();
        return Ok(());
    };

    let theme = peruse_core::theme::resolve(&cli.theme).map_err(anyhow::Error::msg)?;

    // Check the query and the filter from the command line before Peruse
    // opens the file. A mistake in the text is then one error message on the
    // command line. The terminal does not start and then show an error.
    if let Some(q) = &cli.query {
        sql_guard::ensure_read_only(q).map_err(|e| anyhow::anyhow!("--query: {e}"))?;
    }
    if let Some(f) = &cli.filter {
        sql_guard::ensure_safe_predicate(f).map_err(|e| anyhow::anyhow!("--filter: {e}"))?;
    }

    let opts = OpenOptions {
        threads: cli.threads,
        memory_limit: cli.memory_limit.clone(),
        all_varchar: cli.all_varchar,
        ignore_errors: cli.ignore_errors,
        delimiter: cli.delimiter.as_deref().map(parse_delimiter).transpose()?,
        header: cli.no_header.then_some(false),
        sample_size: cli.sample_size,
    };

    // The option --ddl writes to the standard output and stops. The terminal
    // never starts, so the result goes into a file or into another program.
    if let Some(name) = &cli.ddl {
        return print_ddl(&file, &opts, name, cli.table.clone(), cli.query.clone(), cli.filter.clone());
    }

    let (worker, opened) = Worker::spawn(&file, opts)?;
    let auto_index = !cli.no_index;
    let mut application = App::new(worker, opened, theme, auto_index);

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
