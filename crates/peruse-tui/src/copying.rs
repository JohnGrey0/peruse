//! The screen that shows the copy of a file from a network drive.
//!
//! Peruse copies a large file from a network drive to the local disk before
//! it opens the file. The copy is one long read, and each step after it reads
//! the local disk. Refer to `peruse_core::cache`.
//!
//! The screen shows the bytes, the rate and the time that is left. The key
//! `Esc` stops the copy, and Peruse then reads the file on the network. A copy
//! that ends in less than a moment shows no screen at all.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, unbounded};
use peruse_core::Theme;
use peruse_core::cache::{self, CopyError, CopyJob, LocalCopy, Plan, Progress, Settings};
use peruse_core::source::human_bytes;
use ratatui::Frame;

use crate::colors::Depth;
use crate::paint::Paint;
use crate::text;

/// What the program does after the screen.
#[derive(Debug)]
pub enum Outcome {
    /// Open the local copy.
    Local(LocalCopy),
    /// Open the source in place. The text says why, when the user needs to
    /// know.
    InPlace(Option<String>),
    /// The user pressed Ctrl+C. Peruse stops.
    Quit,
}

/// The time that a copy can take before the screen appears.
///
/// A copy of some megabytes ends in less time than a frame needs, and a screen
/// that appears and goes away again at once is only a flash.
const QUIET: Duration = Duration::from_millis(400);

/// Decides how Peruse reads the input, and copies it when the plan says so.
pub fn prepare(input: &str, settings: &Settings, theme: &Theme) -> anyhow::Result<Outcome> {
    let probe = peruse_core::remote::System;
    match cache::plan(input, settings, &probe) {
        Plan::InPlace(why) => Ok(Outcome::InPlace(why)),
        Plan::Ready(c) => Ok(Outcome::Local(c)),
        Plan::Copy(job) => copy_with_screen(job, theme),
    }
}

/// A message from the thread that copies.
enum Msg {
    Progress(Progress),
    Done(Result<LocalCopy, CopyError>),
}

/// What the screen shows.
pub struct CopyView {
    /// The path that the user gave.
    pub origin: String,
    /// The directory that receives the copy.
    pub dir: String,
    /// The number of files to copy.
    pub files: usize,
    /// The progress, from the thread that copies.
    pub progress: Progress,
    /// The time that the copy has taken.
    pub elapsed: Duration,
    /// `true` after the user pressed Esc.
    pub stopping: bool,
}

impl CopyView {
    /// Gives the rate in bytes for each second, when the copy has run long
    /// enough to give a number that means something.
    fn rate(&self) -> Option<f64> {
        let secs = self.elapsed.as_secs_f64();
        (secs >= 0.5 && self.progress.done > 0).then(|| self.progress.done as f64 / secs)
    }

    /// Gives the line under the bar: the bytes, the rate and the time left.
    pub fn status(&self) -> String {
        let p = &self.progress;
        let mut s = format!("{} of {}", human_bytes(p.done), human_bytes(p.total));
        if let Some(r) = self.rate() {
            s.push_str(&format!("  ·  {}/s", human_bytes(r as u64)));
            let left = p.total.saturating_sub(p.done) as f64 / r;
            s.push_str(&format!("  ·  {} left", human_time(left)));
        }
        s
    }

    /// Gives the share of the copy that is done, from 0 to 1.
    fn ratio(&self) -> f64 {
        if self.progress.total == 0 {
            return 1.0;
        }
        (self.progress.done as f64 / self.progress.total as f64).clamp(0.0, 1.0)
    }
}

/// Writes a number of seconds as a short time, such as `9 s` or `2 min 5 s`.
pub fn human_time(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min {} s", s / 60, s % 60),
        _ => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

/// Copies the files on a thread, and shows the screen when the copy is slow.
fn copy_with_screen(job: CopyJob, theme: &Theme) -> anyhow::Result<Outcome> {
    let mut view = CopyView {
        origin: job.origin().to_string(),
        dir: job.dir().display().to_string(),
        files: job.files(),
        progress: Progress {
            done: 0,
            total: job.total_bytes(),
            file: String::new(),
        },
        elapsed: Duration::ZERO,
        stopping: false,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = unbounded::<Msg>();
    let flag = cancel.clone();
    let worker = std::thread::Builder::new()
        .name("peruse-copy".into())
        .spawn(move || {
            let probe = peruse_core::remote::System;
            let progress_tx = tx.clone();
            let r = job.run(
                &mut |p| {
                    let _ = progress_tx.send(Msg::Progress(p.clone()));
                },
                &flag,
                &probe,
            );
            let _ = tx.send(Msg::Done(r));
        })?;
    let started = Instant::now();

    // A copy that ends quickly needs no screen.
    let mut done = drain_until(&rx, &mut view, started + QUIET);
    let mut quit = false;
    if done.is_none() {
        let (r, q) = show(&rx, &mut view, started, &cancel, theme)?;
        done = r;
        quit = q;
    }
    // A read that waits for a server that went away does not end, and the
    // thread then never sees the flag. Such a thread stays behind, and it
    // removes its part of the copy when the read returns.
    if done.is_some() {
        let _ = worker.join();
    }
    let stopped = || {
        Outcome::InPlace(Some(format!(
            "the copy stopped. Peruse reads {} on the network",
            view.origin
        )))
    };
    Ok(match done {
        Some(Ok(c)) => Outcome::Local(c),
        _ if quit => Outcome::Quit,
        None | Some(Err(CopyError::Cancelled)) => stopped(),
        Some(Err(CopyError::Failed(m))) => Outcome::InPlace(Some(m)),
    })
}

/// Takes the messages of the copy until the copy ends or the time comes.
fn drain_until(
    rx: &Receiver<Msg>,
    view: &mut CopyView,
    until: Instant,
) -> Option<Result<LocalCopy, CopyError>> {
    loop {
        let now = Instant::now();
        if now >= until {
            return None;
        }
        match rx.recv_timeout(until - now) {
            Ok(Msg::Progress(p)) => view.progress = p,
            Ok(Msg::Done(r)) => return Some(r),
            Err(_) => return None,
        }
    }
}

/// The time that the copy has to stop after the user asks.
///
/// The copy looks at the flag before each read, and one read takes some
/// milliseconds. A read that takes longer waits for a network that does not
/// answer, and the user must not wait for it.
const STOP_WAIT: Duration = Duration::from_secs(2);

/// Draws the screen until the copy ends, and reads the keys.
///
/// The function gives the result of the copy, or `None` when the copy did not
/// stop in time after the user asked. It also gives `true` when the user asked
/// to quit.
fn show(
    rx: &Receiver<Msg>,
    view: &mut CopyView,
    started: Instant,
    cancel: &AtomicBool,
    theme: &Theme,
) -> anyhow::Result<(Option<Result<LocalCopy, CopyError>>, bool)> {
    use ratatui::crossterm::event::{self, Event, KeyCode, KeyModifiers};

    let depth = Depth::detect();
    let mut terminal = ratatui::init();
    let mut quit = false;
    let mut asked: Option<Instant> = None;
    let result = loop {
        view.elapsed = started.elapsed();
        if let Err(e) = terminal.draw(|f| draw(f, view, theme, depth)) {
            // The copy goes on with no screen. Stop it, and say why.
            cancel.store(true, Ordering::Relaxed);
            ratatui::restore();
            return Err(e.into());
        }
        // The rate and the time left change while no key arrives, so the
        // screen draws again ten times each second.
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(k) = event::read()?
            && k.is_press()
        {
            let ctrl_c =
                k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL);
            if ctrl_c || k.code == KeyCode::Esc || k.code == KeyCode::Char('q') {
                quit |= ctrl_c;
                view.stopping = true;
                cancel.store(true, Ordering::Relaxed);
                asked.get_or_insert_with(Instant::now);
            }
        }
        let mut end = None;
        while let Ok(m) = rx.try_recv() {
            match m {
                Msg::Progress(p) => view.progress = p,
                Msg::Done(r) => end = Some(r),
            }
        }
        if end.is_some() {
            break end;
        }
        if asked.is_some_and(|t| t.elapsed() > STOP_WAIT) {
            break None;
        }
    };
    ratatui::restore();
    Ok((result, quit))
}

/// Draws the screen of the copy.
pub fn draw(f: &mut Frame, v: &CopyView, theme: &Theme, depth: Depth) {
    let p = Paint::new(depth);
    let t = theme;
    let area = f.area();
    let buf = f.buffer_mut();
    if area.width < 10 || area.height < 3 {
        return;
    }
    for y in area.top()..area.bottom() {
        buf.set_stringn(
            area.x,
            y,
            " ".repeat(area.width as usize),
            area.width as usize,
            p.on(t.fg, t.bg),
        );
    }
    buf.set_stringn(
        area.x,
        area.y,
        " ".repeat(area.width as usize),
        area.width as usize,
        p.on(t.fg, t.bg_alt),
    );
    buf.set_stringn(
        area.x + 1,
        area.y,
        "peruse",
        6,
        p.bold(p.on(t.accent, t.bg_alt)),
    );
    let title = "  a local copy of a file from a network drive";
    buf.set_stringn(
        area.x + 7,
        area.y,
        title,
        (area.width as usize).saturating_sub(8),
        p.on(t.dim, t.bg_alt),
    );

    let x = area.x + 3;
    let w = (area.width as usize).saturating_sub(6).min(100);
    let mut y = area.y + 2;
    let line = |buf: &mut ratatui::buffer::Buffer, y: &mut u16, s: &str, style| {
        if *y < area.bottom() {
            buf.set_stringn(x, *y, text::truncate(s, w), w, style);
        }
        *y += 1;
    };

    let name = std::path::Path::new(&v.origin)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| v.origin.clone());
    line(buf, &mut y, &name, p.bold(p.on(t.fg, t.bg)));
    line(
        buf,
        &mut y,
        "Peruse copies it to this computer one time. Each step after that is fast.",
        p.on(t.dim, t.bg),
    );
    y += 1;
    line(
        buf,
        &mut y,
        &format!("from  {}", v.origin),
        p.on(t.fg, t.bg),
    );
    line(buf, &mut y, &format!("to    {}", v.dir), p.on(t.dim, t.bg));
    if v.files > 1 && !v.progress.file.is_empty() {
        line(
            buf,
            &mut y,
            &format!("now   {}", v.progress.file),
            p.on(t.dim, t.bg),
        );
    }
    y += 1;

    // The bar, with the percentage after it.
    let pct = format!(" {:>3.0}%", v.ratio() * 100.0);
    let bar_w = w.saturating_sub(pct.len()).max(1);
    let full = ((bar_w as f64) * v.ratio()).round() as usize;
    if y < area.bottom() {
        buf.set_stringn(x, y, "█".repeat(full), full, p.on(t.accent, t.bg));
        buf.set_stringn(
            x + full as u16,
            y,
            "░".repeat(bar_w - full),
            bar_w - full,
            p.on(t.border, t.bg),
        );
        buf.set_stringn(
            x + bar_w as u16,
            y,
            &pct,
            pct.len(),
            p.bold(p.on(t.fg, t.bg)),
        );
    }
    y += 1;
    line(buf, &mut y, &v.status(), p.on(t.fg, t.bg));
    y += 1;

    if v.stopping {
        line(buf, &mut y, "stopping the copy…", p.on(t.warn, t.bg));
    } else {
        // The keys stand at the start of the line, where a user who reads no
        // text still sees them.
        let keys = "Esc  read it on the network instead      Ctrl+C  quit";
        line(buf, &mut y, keys, p.on(t.key_fg, t.bg));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn screen(v: &CopyView) -> String {
        let theme = peruse_core::theme::builtin("peruse-dark").unwrap();
        let mut term = Terminal::new(TestBackend::new(90, 16)).unwrap();
        term.draw(|f| draw(f, v, &theme, Depth::True)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn view(done: u64, secs: u64) -> CopyView {
        CopyView {
            origin: r"\\porsche\apps\data\sales.parquet".into(),
            dir: r"C:\Users\me\AppData\Local\peruse\cache\copies\0123\data".into(),
            files: 1,
            progress: Progress {
                done,
                total: 1000 << 20,
                file: "sales.parquet".into(),
            },
            elapsed: Duration::from_secs(secs),
            stopping: false,
        }
    }

    #[test]
    fn the_screen_shows_the_source_the_progress_and_the_way_out() {
        let s = screen(&view(250 << 20, 5));
        assert!(s.contains("sales.parquet"), "{s}");
        assert!(
            s.contains(r"from  \\porsche\apps\data\sales.parquet"),
            "{s}"
        );
        assert!(s.contains(" 25%"), "{s}");
        assert!(s.contains("250 MB of 1000 MB"), "{s}");
        assert!(s.contains("50.0 MB/s"), "{s}");
        assert!(s.contains("15 s left"), "{s}");
        assert!(s.contains("Esc  read it on the network instead"), "{s}");
    }

    #[test]
    fn the_rate_waits_for_a_number_that_means_something() {
        let v = view(10 << 20, 0);
        assert!(!v.status().contains("/s"), "{}", v.status());
        let mut v = view(500 << 20, 10);
        v.stopping = true;
        let s = screen(&v);
        assert!(s.contains("stopping the copy"), "{s}");
        assert!(!s.contains("Esc  read"), "{s}");
    }

    #[test]
    fn a_time_is_short_and_readable() {
        assert_eq!(human_time(9.4), "9 s");
        assert_eq!(human_time(125.0), "2 min 5 s");
        assert_eq!(human_time(3700.0), "1 h 1 min");
        assert_eq!(human_time(-3.0), "0 s");
    }
}
