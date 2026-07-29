//! The settings that Peruse keeps between sessions, and the resources of the
//! machine that it runs on.
//!
//! A user who opens a file each day does not want to write the same options on
//! the command line each time. This module reads a small TOML file, and the
//! settings page writes it.
//!
//! Three levels give the value of a setting. The level that is nearer to the
//! user wins:
//!
//! 1. The option on the command line.
//! 2. The setting in the file.
//! 3. The value that Peruse builds in.
//!
//! The file therefore changes the default, and the command line always wins
//! over it. A user can test an option one time without a change to the file.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The theme that Peruse paints with when the user names none.
///
/// The theme is dark. A terminal is dark for most users, and a light screen
/// in a dark room is hard on the eyes.
pub const DEFAULT_THEME: &str = "peruse-dark";

/// The number of files that Peruse remembers.
///
/// The chooser shows these at the top of its list. A longer list would push
/// the directory itself off the screen.
pub const MAX_RECENT: usize = 8;

/// The settings that Peruse keeps in a file.
///
/// Each field is optional. A field with no value takes the value that Peruse
/// builds in, and the file holds only the settings that the user changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// The name of a theme, or the path of a `.toml` theme file.
    pub theme: Option<String>,
    /// The number of threads that DuckDB can use.
    pub threads: Option<usize>,
    /// The memory that DuckDB can use before it writes to the disk, in whole
    /// gigabytes.
    ///
    /// The unit is always the gigabyte, and the number is always a whole
    /// number. A user of a data viewer thinks in gigabytes, and a choice of
    /// units gives the user one more thing to get wrong.
    ///
    /// An older file holds a text such as `"32GB"`. The function
    /// [`gigabytes`] reads that form too, so a file from an older version
    /// still works. Peruse writes the new form.
    #[serde(rename = "memory_limit", deserialize_with = "gigabytes")]
    pub memory_limit_gb: Option<u32>,
    /// The number of rows that the sniffer of a CSV file or a JSON file
    /// examines. The value -1 reads the whole file.
    pub sample_size: Option<i64>,
    /// `true` to never index a file of text when Peruse opens it.
    pub no_index: Option<bool>,
    /// The panels that stay on the screen: `none`, `meta`, `stats` or `both`.
    pub panels: Option<String>,
    /// The files that the user opened, the newest first.
    ///
    /// The chooser of files puts these at the top of its list. A user opens
    /// the same few files again and again, and a path is long to type.
    #[serde(default)]
    pub recent: Vec<String>,
}

impl Config {
    /// Gives the path of the settings file.
    ///
    /// The file is beside the directory of the themes, so a user finds the two
    /// in one place.
    pub fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "peruse")
            .map(|d| d.config_dir().join("config.toml"))
    }

    /// Reads the settings file.
    ///
    /// A file that does not exist gives the built-in settings. A file that
    /// Peruse cannot read gives the built-in settings and the reason, because
    /// a mistake in the file must not stop the program from opening a file.
    pub fn load() -> (Config, Option<String>) {
        let Some(p) = Config::path() else {
            return (Config::default(), None);
        };
        Config::load_from(&p)
    }

    /// Reads a settings file at a path.
    ///
    /// A test gives its own path here. Without that, a test would write the
    /// settings of the user who runs it.
    pub fn load_from(p: &std::path::Path) -> (Config, Option<String>) {
        let text = match std::fs::read_to_string(p) {
            Ok(t) => t,
            // A file that is not there is the usual case, and not an error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (Config::default(), None);
            }
            Err(e) => {
                return (
                    Config::default(),
                    Some(format!("{}: {e}", p.display())),
                );
            }
        };
        match toml::from_str::<Config>(&text) {
            Ok(c) => (c, None),
            Err(e) => (
                Config::default(),
                Some(format!("{}: {}", p.display(), first_line(&e.to_string()))),
            ),
        }
    }

    /// Puts a file at the top of the list of the files that the user opened.
    ///
    /// A file that is in the list already moves to the top, so the list holds
    /// no name two times. The list stops at [`MAX_RECENT`] names.
    pub fn remember_file(&mut self, path: &str) {
        self.recent.retain(|r| r != path);
        self.recent.insert(0, path.to_string());
        self.recent.truncate(MAX_RECENT);
    }

    /// Gives the memory limit in the form that DuckDB takes.
    ///
    /// The text says `GiB`, and the screen says `GB`. The two are the same
    /// number here. DuckDB reads `GB` as 1000 million bytes and `GiB` as 1024
    /// million bytes, and the memory of a machine is always the second one.
    /// A limit of `8GB` would therefore give 7.4 of the gigabytes that the
    /// settings page shows, and the number on the screen would be a lie.
    pub fn memory_limit_text(&self) -> Option<String> {
        self.memory_limit_gb.map(|gb| format!("{gb}GiB"))
    }

    /// Gives the theme to paint with, and a message when the settings file
    /// names a theme that does not exist.
    ///
    /// A name in the file must never stop the program. A user keeps a theme
    /// today, a later version removes that theme or the user deletes the
    /// theme file, and Peruse would then refuse to open any file. The only
    /// way back would be to find the settings file and edit it by hand.
    ///
    /// A theme from the command line is a different case. The user asked for
    /// that name in this call, so a mistake in it is an error.
    pub fn theme_or_default(&self) -> (crate::theme::Theme, Option<String>) {
        let name = self.theme.as_deref().unwrap_or(DEFAULT_THEME);
        match crate::theme::resolve(name) {
            Ok(t) => (t, None),
            Err(e) => (
                crate::theme::builtin(DEFAULT_THEME).unwrap_or_default(),
                Some(e),
            ),
        }
    }

    /// Writes the settings file, and makes the directory if it is not there.
    pub fn save(&self) -> Result<PathBuf, String> {
        let Some(p) = Config::path() else {
            return Err("this system gives no directory for settings".into());
        };
        self.save_to(&p)
    }

    /// Writes the settings to a path, and makes the directory if it is not
    /// there.
    ///
    /// A test gives its own path here. Without that, a test would write the
    /// settings of the user who runs it.
    pub fn save_to(&self, p: &std::path::Path) -> Result<PathBuf, String> {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(p, self.to_toml()).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(p.to_path_buf())
    }

    /// Writes the settings as the text of the file.
    ///
    /// The function writes the file by hand, and not with a library, for one
    /// reason: a person reads this file, and the notes above each setting say
    /// what it does.
    pub fn to_toml(&self) -> String {
        let mut s = String::new();
        s.push_str("# The settings of peruse.\n");
        s.push_str("# An option on the command line wins over a setting here.\n");
        s.push_str("# Remove a line to go back to the value that peruse builds in.\n\n");

        s.push_str("# The name of a theme, or the path of a .toml theme file.\n");
        push_opt(&mut s, "theme", self.theme.as_deref().map(quote));

        s.push_str("\n# The number of threads that DuckDB can use.\n");
        s.push_str("# With no value, DuckDB uses one thread for each core.\n");
        push_opt(&mut s, "threads", self.threads.map(|v| v.to_string()));

        s.push_str("\n# The memory that DuckDB can use before it writes to the disk,\n");
        s.push_str("# as a whole number of gigabytes.\n");
        push_opt(
            &mut s,
            "memory_limit",
            self.memory_limit_gb.map(|v| v.to_string()),
        );

        s.push_str("\n# The rows that the CSV sniffer and the JSON sniffer examine.\n");
        s.push_str("# The value -1 reads the whole file, which is slower and always correct.\n");
        push_opt(&mut s, "sample_size", self.sample_size.map(|v| v.to_string()));

        s.push_str("\n# Set this to true to never index a file of text at the start.\n");
        push_opt(&mut s, "no_index", self.no_index.map(|v| v.to_string()));

        s.push_str("\n# The panels that stay at the side of the grid:\n");
        s.push_str("# none, meta, stats or both.\n");
        push_opt(&mut s, "panels", self.panels.as_deref().map(quote));

        s.push_str("\n# The files that you opened, the newest first.\n");
        s.push_str("# The chooser of files puts these at the top of its list.\n");
        if self.recent.is_empty() {
            s.push_str("# recent = []\n");
        } else {
            s.push_str("recent = [\n");
            for r in &self.recent {
                s.push_str(&format!("  {},\n", quote(r)));
            }
            s.push_str("]\n");
        }
        s
    }
}

/// Reads the memory limit from the settings file.
///
/// The function accepts three forms:
///
/// * a whole number, which is a number of gigabytes
/// * a text such as `"32GB"`, from a file that an older version wrote
/// * a text such as `"512MB"`, which becomes a whole number of gigabytes
///
/// A value below one gigabyte becomes one gigabyte. DuckDB needs some memory
/// to run any query at all.
fn gigabytes<'de, D>(d: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Number(i64),
        Text(String),
    }
    let Some(raw) = Option::<Raw>::deserialize(d)? else {
        return Ok(None);
    };
    let gb = match raw {
        Raw::Number(n) => n.max(1) as u32,
        Raw::Text(s) => {
            let t = s.trim().to_ascii_uppercase();
            let (digits, factor) = if let Some(v) = t.strip_suffix("TB") {
                (v, 1024.0)
            } else if let Some(v) = t.strip_suffix("GB") {
                (v, 1.0)
            } else if let Some(v) = t.strip_suffix("MB") {
                (v, 1.0 / 1024.0)
            } else if let Some(v) = t.strip_suffix("KB") {
                (v, 1.0 / (1024.0 * 1024.0))
            } else {
                (t.as_str(), 1.0)
            };
            let n: f64 = digits
                .trim()
                .parse()
                .map_err(|_| D::Error::custom(format!("memory_limit: {s:?} is not a size")))?;
            ((n * factor).round() as i64).max(1) as u32
        }
    };
    Ok(Some(gb))
}

/// Writes one setting, or writes it as a note when it has no value.
fn push_opt(s: &mut String, key: &str, value: Option<String>) {
    match value {
        Some(v) => s.push_str(&format!("{key} = {v}\n")),
        None => s.push_str(&format!("# {key} =\n")),
    }
}

/// Puts a value in quotation marks for the TOML file.
fn quote(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Gives the first line of a message. A message from the TOML library holds
/// the full text of the file after the reason.
fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or(s).trim().to_string()
}

/// What the machine gives to Peruse.
///
/// The settings page shows these numbers. A user who sets a memory limit needs
/// to know how much memory the machine has, and a user who sets the threads
/// needs to know how many cores it has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resources {
    /// The number of threads that the machine can run at the same time.
    pub cores: usize,
    /// The name of the processor, when the system gives one.
    pub cpu: Option<String>,
    /// The memory of the machine, in bytes.
    pub total_memory: Option<u64>,
    /// The memory that is free now, in bytes.
    pub free_memory: Option<u64>,
    /// The directory that DuckDB writes to when a query needs more memory
    /// than its limit.
    pub temp_dir: PathBuf,
}

impl Resources {
    /// Reads the resources of the machine.
    pub fn read() -> Resources {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let total = sys.total_memory();
        let free = sys.available_memory();

        // A system that gives no number gives zero. Zero bytes of memory is
        // not a true answer, so it becomes "not known" instead.
        Resources {
            cores,
            cpu: cpu_name(),
            total_memory: (total > 0).then_some(total),
            free_memory: (free > 0).then_some(free),
            temp_dir: std::env::temp_dir().join("peruse"),
        }
    }

    /// Gives the memory limit that Peruse uses when the user sets none, in
    /// whole gigabytes.
    ///
    /// The value is one half of the memory of the machine. DuckDB takes 80
    /// percent of the memory for itself if nobody tells it otherwise, and a
    /// viewer of data is not the only program that a user runs. One half
    /// leaves room for the editor, the browser and the rest.
    ///
    /// The value is never below one gigabyte. DuckDB needs some memory to
    /// run any query at all.
    ///
    /// The number comes from the memory of the machine, and not from the
    /// memory that is free now. A free number changes at each start, and the
    /// user would then see a different limit each time.
    pub fn default_memory_gb(&self) -> Option<u32> {
        let total = self.total_memory?;
        let gib = total / (1024 * 1024 * 1024);
        // A small machine gets a smaller part. The limit is the point where
        // DuckDB starts to write to the disk, and that is the graceful way
        // to run out of memory. A limit near the size of the machine takes
        // that way away: the operating system starts to write to the disk
        // instead, and that is much slower.
        //
        // A measurement of an index of a file of 1.36 GB shows the trade: a
        // limit of 1 GiB made the work 17 percent slower, and it halved the
        // memory that the program held. On a machine of 8 GB, where a
        // browser and an editor hold half of the memory already, that trade
        // is a good one.
        let part = if gib <= 8 { total * 3 / 10 } else { total / 2 };
        let gb = part / (1024 * 1024 * 1024);
        Some((gb as u32).max(1))
    }

    /// Gives the same value in the form that DuckDB takes. See
    /// [`Config::memory_limit_text`] for the reason that the unit is `GiB`.
    pub fn default_memory_text(&self) -> Option<String> {
        self.default_memory_gb().map(|gb| format!("{gb}GiB"))
    }
}

/// Gives the name of the processor.
fn cpu_name() -> Option<String> {
    let mut sys = sysinfo::System::new();
    sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
    let name = sys.cpus().first()?.brand().trim().to_string();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_with_no_setting_gives_the_built_in_values() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c, Config::default());
        assert!(c.theme.is_none());
    }

    #[test]
    fn a_file_reads_back_the_settings_that_it_holds() {
        let text = "theme = \"nord\"\nthreads = 4\nmemory_limit = \"2GB\"\n";
        let c: Config = toml::from_str(text).unwrap();
        assert_eq!(c.theme.as_deref(), Some("nord"));
        assert_eq!(c.threads, Some(4));
        assert_eq!(c.memory_limit_gb, Some(2));
        // A setting that the file does not name keeps no value.
        assert_eq!(c.sample_size, None);
    }

    #[test]
    fn the_text_of_the_file_reads_back_as_the_same_settings() {
        let c = Config {
            theme: Some("peruse-dark".into()),
            threads: Some(8),
            memory_limit_gb: Some(4),
            sample_size: Some(-1),
            no_index: Some(true),
            panels: Some("both".into()),
            recent: vec!["/data/a.parquet".into(), "C:/data/b.csv".into()],
        };
        let back: Config = toml::from_str(&c.to_toml()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn the_list_of_files_holds_no_name_two_times() {
        let mut c = Config::default();
        for f in ["a.csv", "b.csv", "a.csv"] {
            c.remember_file(f);
        }
        // The newest comes first, and `a.csv` moved to the top.
        assert_eq!(c.recent, ["a.csv", "b.csv"]);
    }

    #[test]
    fn the_list_of_files_stops_at_a_length() {
        let mut c = Config::default();
        for i in 0..(MAX_RECENT + 5) {
            c.remember_file(&format!("file{i}.csv"));
        }
        assert_eq!(c.recent.len(), MAX_RECENT);
        assert_eq!(c.recent[0], format!("file{}.csv", MAX_RECENT + 4));
    }

    #[test]
    fn a_setting_with_no_value_becomes_a_note_and_not_a_line() {
        let text = Config::default().to_toml();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back, Config::default());
        assert!(text.contains("# theme ="), "{text}");
        assert!(!text.contains("\ntheme ="), "{text}");
    }

    #[test]
    fn a_theme_path_with_a_backslash_reads_back() {
        // A path on Windows holds backslashes, and TOML reads a backslash as
        // an escape character.
        let c = Config {
            theme: Some("C:\\Users\\me\\themes\\mine.toml".into()),
            ..Config::default()
        };
        let back: Config = toml::from_str(&c.to_toml()).unwrap();
        assert_eq!(back.theme, c.theme);
    }

    #[test]
    fn a_file_with_a_name_that_peruse_does_not_know_is_refused() {
        // A name with a spelling mistake must not pass in silence. The user
        // would then look for the reason that the setting does nothing.
        let e = toml::from_str::<Config>("thread = 4").unwrap_err();
        assert!(e.to_string().contains("unknown field"), "{e}");
    }

    #[test]
    fn a_theme_that_no_longer_exists_does_not_stop_the_program() {
        // A user keeps a theme today, and a later version removes it. Peruse
        // must still open a file, and it must say why the colors changed.
        let c = Config {
            theme: Some("a-theme-that-was-removed".into()),
            ..Config::default()
        };
        let (theme, msg) = c.theme_or_default();
        assert_eq!(theme.name, DEFAULT_THEME);
        assert!(msg.is_some_and(|m| m.contains("unknown theme")));
    }

    #[test]
    fn the_theme_of_the_file_is_the_theme_that_peruse_paints_with() {
        let c = Config {
            theme: Some("nord".into()),
            ..Config::default()
        };
        let (theme, msg) = c.theme_or_default();
        assert_eq!(theme.name, "nord");
        assert_eq!(msg, None);
    }

    #[test]
    fn the_theme_that_peruse_builds_in_is_dark() {
        // A light screen in a dark room is hard on the eyes, and a terminal
        // is dark for most users.
        let (theme, msg) = Config::default().theme_or_default();
        assert_eq!(theme.name, DEFAULT_THEME);
        assert!(theme.dark, "the built-in theme must be dark");
        assert_eq!(msg, None);
    }

    #[test]
    fn the_machine_gives_its_number_of_cores() {
        let r = Resources::read();
        assert!(r.cores >= 1);
        assert!(r.temp_dir.ends_with("peruse"));
    }

    #[test]
    fn the_built_in_limit_leaves_room_for_the_other_programs() {
        let r = Resources {
            cores: 8,
            cpu: None,
            total_memory: Some(64 * 1024 * 1024 * 1024),
            free_memory: None,
            temp_dir: PathBuf::from("/tmp"),
        };
        assert_eq!(r.default_memory_gb(), Some(32));
        assert_eq!(r.default_memory_text().as_deref(), Some("32GiB"));

        // A machine of 8 GB gets a smaller part, so that DuckDB writes to
        // its own temporary directory before the operating system starts to
        // write to the disk.
        let laptop = Resources {
            total_memory: Some(8 * 1024 * 1024 * 1024),
            ..r.clone()
        };
        assert_eq!(laptop.default_memory_gb(), Some(2));

        // A small machine still gives DuckDB something to work with.
        let small = Resources {
            total_memory: Some(2 * 1024 * 1024 * 1024),
            ..r.clone()
        };
        assert_eq!(small.default_memory_gb(), Some(1));

        // A machine with no number gives no value, and DuckDB then uses its
        // own rule.
        let none = Resources {
            total_memory: None,
            ..r
        };
        assert_eq!(none.default_memory_gb(), None);
    }

    #[test]
    fn the_memory_limit_is_a_whole_number_of_gigabytes() {
        let c: Config = toml::from_str("memory_limit = 8").unwrap();
        assert_eq!(c.memory_limit_gb, Some(8));
        assert_eq!(c.memory_limit_text().as_deref(), Some("8GiB"));
        // Peruse writes the number, and not a text.
        assert!(c.to_toml().contains("memory_limit = 8"), "{}", c.to_toml());
    }

    #[test]
    fn a_size_from_an_older_file_still_reads() {
        // An older version wrote a text such as "32GB". A user must not lose
        // a setting because a later version changed the form of the file.
        for (text, want) in [
            ("memory_limit = \"32GB\"", 32u32),
            ("memory_limit = \"4gb\"", 4),
            ("memory_limit = \"2TB\"", 2048),
            ("memory_limit = \"512MB\"", 1),
            ("memory_limit = \"16\"", 16),
        ] {
            let c: Config = toml::from_str(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(c.memory_limit_gb, Some(want), "{text}");
        }
        // A value that is not a size says so.
        assert!(toml::from_str::<Config>("memory_limit = \"lots\"").is_err());
    }

    #[test]
    fn a_bad_message_from_the_library_becomes_one_line() {
        assert_eq!(first_line("bad thing\nfull text\nmore"), "bad thing");
    }
}
