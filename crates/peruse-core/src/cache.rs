//! The local copies of files that live on a network drive.
//!
//! DuckDB reads a Parquet file with many small reads at different positions,
//! and on a network drive each read waits for the network. A user who copies
//! a large file to the local disk before opening it gets a fast file, and this
//! module does that copy for the user:
//!
//! 1. [`plan`] finds out whether the source is on a network drive, whether it
//!    is large enough to be worth a copy, and whether a copy from an earlier
//!    session is still correct.
//! 2. [`CopyJob::run`] copies the files with one long sequential read each,
//!    and it reports the progress. A flag stops it.
//! 3. The engine then opens the copy, and the screen still names the source.
//!
//! The copies stay between sessions, in the cache directory of the system.
//! One entry holds the copy of one file, or of the files of one glob:
//!
//! ```text
//! <cache>/copies/<key>/manifest.toml   the source, and the size and time of each file
//! <cache>/copies/<key>/data/...        the copies, with the names of the source
//! <cache>/copies/<key>/tmp/...         a copy that is not complete yet
//! <cache>/copies/<key>/lease-<pid>     one for each Peruse that reads the entry now
//! ```
//!
//! The key comes from the complete path of the source. The manifest holds the
//! size and the time of the last change of each file of the source, so a
//! source that changed gets a new copy. The entries that were used the least
//! recently go first when the copies need more room than the limit allows.
//!
//! Peruse never writes to the source. It opens the source for reading only,
//! and each write goes to the cache directory.

use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::remote::Probe;
use crate::source::{self, Format, human_bytes};

/// The size below which Peruse reads a file from a network drive in place, in
/// megabytes.
///
/// A small file costs little to read in place, and a copy of it would put one
/// more entry in the cache for no gain that a user can feel.
pub const DEFAULT_MIN_MB: u64 = 32;

/// The space that the local copies can take together, in gigabytes.
pub const DEFAULT_MAX_GB: u64 = 20;

/// The space that a copy leaves free on the local disk, in bytes.
///
/// A disk that is full stops other programs too. A copy that would leave less
/// than this does not start, and Peruse reads the file in place.
const RESERVE: u64 = 1 << 30;

/// The size of one read from the source.
///
/// A network file system sends a large read as some requests at the same
/// time, so a large buffer keeps the network busy.
const CHUNK: usize = 8 << 20;

const MANIFEST: &str = "manifest.toml";
const DATA: &str = "data";
const TMP: &str = "tmp";
const LEASE: &str = "lease-";

/// When Peruse copies a file to the local disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Policy {
    /// Copy a file from a network drive when it is large enough.
    #[default]
    Auto,
    /// Copy each file, from any disk and of any size.
    Always,
    /// Read each file in place.
    Never,
}

impl Policy {
    /// Reads a policy from the text of the settings file.
    pub fn parse(s: &str) -> Option<Policy> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Policy::Auto),
            "always" | "yes" | "on" | "true" => Some(Policy::Always),
            "never" | "no" | "off" | "false" => Some(Policy::Never),
            _ => None,
        }
    }

    /// Gives the text that the settings file holds.
    pub fn name(self) -> &'static str {
        match self {
            Policy::Auto => "auto",
            Policy::Always => "always",
            Policy::Never => "never",
        }
    }
}

/// The settings of the local copies.
#[derive(Clone, Debug)]
pub struct Settings {
    /// When Peruse copies a file.
    pub policy: Policy,
    /// The size below which the policy `auto` reads in place, in bytes.
    pub min_bytes: u64,
    /// The space that the copies can take together, in bytes.
    pub max_bytes: u64,
    /// The directory of the copies. `None` when the system gives no cache
    /// directory, and Peruse then reads each file in place.
    pub dir: Option<PathBuf>,
}

impl Settings {
    /// Gives the settings from the settings file, and from the option of the
    /// command line when the user gives one.
    pub fn from_config(config: &crate::Config, cli: Option<Policy>) -> Settings {
        Settings {
            policy: cli.unwrap_or_else(|| config.local_copy_policy()),
            min_bytes: config
                .local_copy_min_mb
                .unwrap_or(DEFAULT_MIN_MB)
                .saturating_mul(1 << 20),
            max_bytes: config
                .local_copy_max_gb
                .unwrap_or(DEFAULT_MAX_GB)
                .saturating_mul(1 << 30),
            dir: copies_dir(),
        }
    }
}

/// Gives the directory that holds the local copies.
pub fn copies_dir() -> Option<PathBuf> {
    crate::dirs::cache_dir().map(|d| d.join("copies"))
}

/// The size and the time of the last change of one file of the source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    /// The path below the base directory of the source, with forward slashes.
    rel: String,
    size: u64,
    /// Nanoseconds after 1970. The time of a file on a share has a precision
    /// of 100 nanoseconds, and a change inside one second must still count.
    mtime_ns: i64,
}

/// One file of the source.
#[derive(Clone, Debug)]
struct Member {
    remote: PathBuf,
    rec: Record,
}

/// The file of an entry that says what the copies are copies of.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Manifest {
    /// The complete path of the source, or of a glob.
    source: String,
    /// The seconds after 1970 of the last use. The oldest entry goes first.
    last_used: u64,
    files: Vec<Record>,
}

/// What [`plan`] decided.
#[derive(Debug)]
pub enum Plan {
    /// Read the source in place. The text, when there is one, says why a copy
    /// was not possible, for the status line.
    InPlace(Option<String>),
    /// A copy from an earlier session is correct, and the engine can open it
    /// now.
    Ready(LocalCopy),
    /// Copy first. A copy from an earlier session can give some of the files.
    Copy(CopyJob),
}

/// The progress of a copy.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    /// The bytes that are copied.
    pub done: u64,
    /// The bytes to copy.
    pub total: u64,
    /// The path of the file that the copy reads now, below the base directory.
    pub file: String,
}

/// Why a copy stopped.
#[derive(Debug)]
pub enum CopyError {
    /// The user stopped it.
    Cancelled,
    /// A read, a write or a test failed. The text names the source.
    Failed(String),
}

impl std::fmt::Display for CopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopyError::Cancelled => f.write_str("the copy was stopped"),
            CopyError::Failed(m) => f.write_str(m),
        }
    }
}

/// A copy on the local disk that the engine can open.
///
/// The value holds a lease on its entry. While it lives, no other Peruse
/// removes the entry to make room. Keep it until the end of the session.
#[derive(Debug)]
pub struct LocalCopy {
    /// The path or the glob that the engine opens.
    pub input: String,
    /// The path or the glob that the user gave.
    pub origin: String,
    /// The directory that holds the copies.
    pub dir: PathBuf,
    /// The part of the source path in front of the names of the files, as the
    /// user wrote it. Empty for a name with no directory.
    remote_base: String,
    /// The size of the copies, in bytes.
    pub bytes: u64,
    /// The time that the copy took, or `None` when an earlier session made it.
    pub copied: Option<Duration>,
    _lease: Option<Lease>,
}

/// The changes that make a text about the copy name the source instead.
///
/// The worker applies it to each message and to the metadata, so that the
/// screen never names the cache directory as the place of the data.
#[derive(Clone, Debug)]
pub struct Rewrite {
    /// The path or the glob that the user gave.
    pub origin: String,
    /// The path or the glob of the copy, for the metadata panel.
    pub local: String,
    pairs: Vec<(String, String)>,
}

impl Rewrite {
    /// Gives the text with each path of the copy changed to the path of the
    /// source.
    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (from, to) in &self.pairs {
            if !from.is_empty() && out.contains(from.as_str()) {
                out = out.replace(from.as_str(), to);
            }
        }
        out
    }

    /// Gives the error with each path of the copy changed to the path of the
    /// source. Each level of the error keeps its place.
    pub fn apply_error(&self, e: &anyhow::Error) -> anyhow::Error {
        let mut parts: Vec<String> = e.chain().map(|c| self.apply(&c.to_string())).collect();
        let mut out = anyhow::anyhow!(parts.pop().unwrap_or_default());
        while let Some(p) = parts.pop() {
            out = out.context(p);
        }
        out
    }
}

impl LocalCopy {
    /// Gives the changes that name the source in place of the copy.
    pub fn rewrite(&self) -> Rewrite {
        let sep = std::path::MAIN_SEPARATOR;
        let native = self.dir.to_string_lossy().into_owned();
        let forward = native.replace('\\', "/");
        let (base_native, base_forward) = if self.remote_base.is_empty() {
            (String::new(), String::new())
        } else {
            (
                format!("{}{sep}", self.remote_base),
                format!("{}/", self.remote_base.replace('\\', "/")),
            )
        };
        Rewrite {
            origin: self.origin.clone(),
            local: self.input.clone(),
            pairs: vec![
                (format!("{forward}/"), base_forward),
                (format!("{native}{sep}"), base_native),
            ],
        }
    }
}

/// The work of one copy. [`CopyJob::run`] does it.
#[derive(Debug)]
pub struct CopyJob {
    root: PathBuf,
    entry: PathBuf,
    key: String,
    identity: String,
    members: Vec<Member>,
    /// The positions in `members` of the files to copy.
    todo: Vec<usize>,
    max_bytes: u64,
    copy: LocalCopy,
}

impl CopyJob {
    /// Gives the bytes that the copy reads from the network.
    pub fn total_bytes(&self) -> u64 {
        self.todo.iter().map(|i| self.members[*i].rec.size).sum()
    }

    /// Gives the number of files that the copy reads.
    pub fn files(&self) -> usize {
        self.todo.len()
    }

    /// Gives the path or the glob that the user gave.
    pub fn origin(&self) -> &str {
        &self.copy.origin
    }

    /// Gives the directory that receives the copies.
    pub fn dir(&self) -> &Path {
        &self.copy.dir
    }

    /// Copies the files, and gives the copy that the engine can open.
    ///
    /// The function first removes old entries when the copies need the room,
    /// and it refuses to fill the disk. It calls `progress` after each read.
    /// It examines `cancel` before each read, and it stops with
    /// [`CopyError::Cancelled`] when the flag is set. A file that is not
    /// complete never appears in the directory of the copies.
    pub fn run(
        mut self,
        progress: &mut dyn FnMut(&Progress),
        cancel: &AtomicBool,
        probe: &dyn Probe,
    ) -> Result<LocalCopy, CopyError> {
        let started = Instant::now();
        let total = self.total_bytes();
        evict(&self.root, &self.key, total, self.max_bytes, probe);
        if let Some(free) = probe.free_bytes(&self.entry)
            && free < total.saturating_add(RESERVE)
        {
            return Err(CopyError::Failed(format!(
                "the local disk has {} free, and a copy of {} needs {} more than that. \
                 Peruse reads it in place.",
                human_bytes(free),
                self.copy.origin,
                human_bytes(total.saturating_add(RESERVE) - free)
            )));
        }

        let tmp = self.entry.join(TMP);
        let data = self.entry.join(DATA);
        fs::create_dir_all(&tmp).map_err(|e| local_error(&tmp, e))?;
        remove_old_parts(&tmp);
        let mut state = Progress {
            done: 0,
            total,
            file: String::new(),
        };
        for (n, i) in self.todo.iter().enumerate() {
            let m = &self.members[*i];
            state.file = m.rec.rel.clone();
            let part = tmp.join(format!("{n}.{}.part", std::process::id()));
            let dest = data.join(&m.rec.rel);
            copy_one(m, &part, &dest, &mut state, progress, cancel)?;
        }

        // A file that the source no longer holds must not stay. A glob over
        // the copies would find it.
        let keep: Vec<&str> = self.members.iter().map(|m| m.rec.rel.as_str()).collect();
        remove_others(&data, &data, &keep);
        let _ = fs::remove_dir(&tmp);

        let manifest = Manifest {
            source: self.identity.clone(),
            last_used: now_secs(),
            files: self.members.iter().map(|m| m.rec.clone()).collect(),
        };
        write_manifest(&self.entry, &manifest).map_err(|e| local_error(&self.entry, e))?;
        self.copy.copied = Some(started.elapsed());
        Ok(self.copy)
    }
}

/// Gives a message for a failure on the local disk.
fn local_error(path: &Path, e: std::io::Error) -> CopyError {
    CopyError::Failed(format!(
        "cannot write the local copy to {}: {e}. Peruse reads the file in place.",
        path.display()
    ))
}

/// Opens a file of the source for reading only, and tells the system that the
/// reads go from the start to the end.
fn open_source(path: &Path) -> std::io::Result<File> {
    let mut o = fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_SEQUENTIAL_SCAN. The cache of Windows then reads ahead
        // of the program, and it does not keep the pages that it read.
        o.custom_flags(0x0800_0000);
    }
    o.open(path)
}

/// Copies one file to a temporary name, tests it, and gives it its name.
fn copy_one(
    m: &Member,
    part: &Path,
    dest: &Path,
    state: &mut Progress,
    progress: &mut dyn FnMut(&Progress),
    cancel: &AtomicBool,
) -> Result<(), CopyError> {
    let remote = &m.remote;
    let mut src =
        open_source(remote).map_err(|e| CopyError::Failed(format!("{}: {e}", remote.display())))?;
    let mut dst = File::create(part).map_err(|e| local_error(part, e))?;
    let fail = |e: CopyError| {
        let _ = fs::remove_file(part);
        e
    };
    let mut buf = vec![0u8; CHUNK];
    let mut copied = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(dst);
            return Err(fail(CopyError::Cancelled));
        }
        let n = match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                drop(dst);
                return Err(fail(CopyError::Failed(format!(
                    "{}: {e}",
                    remote.display()
                ))));
            }
        };
        if let Err(e) = dst.write_all(&buf[..n]) {
            drop(dst);
            return Err(fail(local_error(part, e)));
        }
        copied += n as u64;
        state.done += n as u64;
        progress(state);
    }
    drop(dst);
    drop(src);

    // A program that writes the source during the copy gives a copy that is
    // half old and half new. The size and the time show that.
    let after = stat(remote);
    if copied != m.rec.size || after != Some((m.rec.size, m.rec.mtime_ns)) {
        return Err(fail(CopyError::Failed(format!(
            "{} changed while Peruse copied it. Peruse reads it in place.",
            remote.display()
        ))));
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(|e| fail(local_error(dir, e)))?;
    }
    if let Err(e) = fs::rename(part, dest) {
        let _ = fs::remove_file(part);
        // A second Peruse that copied the same file at the same time can hold
        // its copy open, and Windows then refuses the new name. Its copy is
        // as good as this one when the size agrees.
        let same = fs::metadata(dest).map(|d| d.len()).ok() == Some(m.rec.size);
        if !same {
            return Err(local_error(dest, e));
        }
    }
    Ok(())
}

/// The age after which a part of a copy belongs to a Peruse that stopped.
///
/// A copy that runs writes its part at each read, so its part is never this
/// old. A part of this age comes from a Peruse that the user closed, or that
/// stopped, during a copy.
const OLD_PART: Duration = Duration::from_secs(10 * 60);

/// Removes the parts of copies that a Peruse left behind.
fn remove_old_parts(tmp: &Path) {
    let Ok(list) = fs::read_dir(tmp) else {
        return;
    };
    for e in list.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > OLD_PART);
        if old {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// Removes each file below `dir` that is not in `keep`, and each directory
/// that is then empty.
fn remove_others(base: &Path, dir: &Path, keep: &[&str]) {
    let Ok(list) = fs::read_dir(dir) else {
        return;
    };
    for e in list.flatten() {
        let p = e.path();
        if p.is_dir() {
            remove_others(base, &p, keep);
            let _ = fs::remove_dir(&p);
        } else {
            let rel = p
                .strip_prefix(base)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if !keep.contains(&rel.as_str()) {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

/// Gives the size and the time of the last change of a file.
fn stat(path: &Path) -> Option<(u64, i64)> {
    let m = fs::metadata(path).ok()?;
    if !m.is_file() {
        return None;
    }
    let t = m.modified().ok()?;
    let ns = match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_nanos()).unwrap_or(i64::MAX),
    };
    Some((m.len(), ns))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Gives a key of 16 hexadecimal digits for a text.
///
/// The function is FNV-1a with 64 bits. The hash of the standard library can
/// change between two versions of Rust, and the key must stay the same, or a
/// new version of Peruse would copy each file again.
fn key_of(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Gives the complete path of a file in the form that keys the cache.
///
/// Windows does not see a difference between capital and small letters in a
/// path, so the key does not either. A mapped drive letter and the path of the
/// share give the same complete path, so they share one copy.
fn identity_of(path: &Path) -> String {
    let full = fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf());
    let text = full.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// The source, after Peruse found its files.
struct Target {
    identity: String,
    members: Vec<Member>,
    /// The part of the input in front of the names of the files.
    remote_base: String,
    /// The part of the input after the base: the name of the file, or the
    /// part of the glob with the wildcards.
    suffix: String,
    /// The directory to test for a network drive.
    probe: PathBuf,
}

/// Finds the files of the source.
///
/// The function gives `None` when the source is not a file or a glob that
/// Peruse can copy. The engine then opens the input itself, and its message
/// says what is wrong.
fn resolve(input: &str) -> Option<Target> {
    if source::looks_like_glob(input) {
        return resolve_glob(input);
    }
    let path = Path::new(input);
    let name = path.file_name()?.to_str()?.to_string();
    let base = input.strip_suffix(name.as_str())?;
    let remote_base = base.trim_end_matches(['/', '\\']).to_string();
    // The root directory keeps its separator: `/data.csv` has the base `/`.
    let remote_base = if remote_base.is_empty() && !base.is_empty() {
        base.to_string()
    } else {
        remote_base
    };
    let mut members = vec![member(path, &name)?];
    // A database keeps the changes that it did not write into the file yet in
    // a second file beside it. The copy must hold both, or it would show
    // older rows than the source.
    if source::detect(path).0 == Format::DuckDb {
        let wal_name = format!("{name}.wal");
        let wal = PathBuf::from(format!("{input}.wal"));
        if wal.is_file() {
            members.push(member(&wal, &wal_name)?);
        }
    }
    let probe = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    Some(Target {
        identity: identity_of(path),
        members,
        remote_base,
        suffix: name,
        probe,
    })
}

fn member(path: &Path, rel: &str) -> Option<Member> {
    let (size, mtime_ns) = stat(path)?;
    Some(Member {
        remote: path.to_path_buf(),
        rec: Record {
            rel: rel.to_string(),
            size,
            mtime_ns,
        },
    })
}

/// Finds the files of a glob, with the DuckDB function `glob()`. The set is
/// therefore the set that the engine reads.
fn resolve_glob(input: &str) -> Option<Target> {
    let pattern = input.replace('\\', "/");
    let wild = pattern.find(['*', '?', '['])?;
    let slash = pattern[..wild].rfind('/');
    let (base, suffix) = match slash {
        Some(i) => (&pattern[..i], &pattern[i + 1..]),
        None => ("", pattern.as_str()),
    };
    // The root directory keeps its slash.
    let base_dir = if base.is_empty() && slash.is_some() {
        "/"
    } else {
        base
    };

    let conn = duckdb::Connection::open_in_memory().ok()?;
    let mut stmt = conn.prepare("SELECT file FROM glob(?)").ok()?;
    let mut rows = stmt.query([&pattern]).ok()?;
    let mut files = Vec::new();
    while let Ok(Some(r)) = rows.next() {
        files.push(r.get::<_, String>(0).ok()?);
    }
    files.sort();

    let mut members = Vec::new();
    for f in &files {
        let norm = f.replace('\\', "/");
        let rel = if base.is_empty() && slash.is_none() {
            norm.as_str()
        } else {
            norm.strip_prefix(base)?.strip_prefix('/')?
        };
        // A part `..` would put a copy outside its entry.
        if rel.is_empty() || rel.split('/').any(|p| p == ".." || p.is_empty()) {
            return None;
        }
        members.push(member(Path::new(f), rel)?);
    }
    let probe = if base_dir.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(base_dir)
    };
    Some(Target {
        identity: format!("{}/{suffix}", identity_of(&probe)),
        members,
        remote_base: input[..slash.unwrap_or(0)].to_string(),
        suffix: suffix.to_string(),
        probe,
    })
}

/// Gives `true` when the path is inside the directory.
fn inside(path: &Path, dir: &Path) -> bool {
    let full = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    full(path).starts_with(full(dir))
}

/// Decides how Peruse reads the input.
///
/// The function reads the sizes and the times of the files of the source, and
/// the manifest of an earlier copy. It copies nothing and removes nothing, so
/// a caller that does not want to wait can drop the plan.
pub fn plan(input: &str, settings: &Settings, probe: &dyn Probe) -> Plan {
    if settings.policy == Policy::Never {
        return Plan::InPlace(None);
    }
    let Some(root) = settings.dir.clone() else {
        return Plan::InPlace(None);
    };
    let Some(t) = resolve(input) else {
        return Plan::InPlace(None);
    };
    if t.members.is_empty() || inside(&t.probe, &root) {
        return Plan::InPlace(None);
    }
    if settings.policy == Policy::Auto && !probe.is_remote(&t.probe) {
        return Plan::InPlace(None);
    }
    let total: u64 = t.members.iter().map(|m| m.rec.size).sum();
    if settings.policy == Policy::Auto && total < settings.min_bytes {
        return Plan::InPlace(None);
    }
    if total > settings.max_bytes {
        return Plan::InPlace(Some(format!(
            "{} is {}, more than the {} that the local copies can take. \
             Peruse reads it in place.",
            input,
            human_bytes(total),
            human_bytes(settings.max_bytes)
        )));
    }
    if probe.is_remote(&root) {
        return Plan::InPlace(Some(format!(
            "the directory of the local copies, {}, is on a network drive. \
             Peruse reads {input} in place.",
            root.display()
        )));
    }

    let key = key_of(&t.identity);
    let entry = root.join(&key);
    let data = entry.join(DATA);
    if let Err(e) = fs::create_dir_all(&data) {
        return Plan::InPlace(Some(format!(
            "cannot write the local copy to {}: {e}. Peruse reads {input} in place.",
            data.display()
        )));
    }
    let local_input = if source::looks_like_glob(&t.suffix) {
        let d = data.to_string_lossy().replace('\\', "/");
        // A directory name with a wildcard character would change the glob.
        if source::looks_like_glob(&d) {
            return Plan::InPlace(None);
        }
        format!("{d}/{}", t.suffix)
    } else {
        data.join(&t.suffix).to_string_lossy().into_owned()
    };
    let copy = LocalCopy {
        input: local_input,
        origin: input.to_string(),
        dir: data.clone(),
        remote_base: t.remote_base,
        bytes: total,
        copied: None,
        _lease: Lease::take(&entry),
    };

    let mut manifest = read_manifest(&entry)
        .filter(|m| m.source == t.identity)
        .unwrap_or_default();
    let todo: Vec<usize> = t
        .members
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            let recorded = manifest.files.contains(&m.rec);
            let on_disk = fs::metadata(data.join(&m.rec.rel)).map(|d| d.len()).ok();
            !(recorded && on_disk == Some(m.rec.size))
        })
        .map(|(i, _)| i)
        .collect();
    // Each file of the source is in the manifest when nothing is left to copy,
    // so a longer manifest names a file that the source no longer holds. The
    // copy of that file must go, or a glob over the copies would find it.
    let gone = manifest.files.len() != t.members.len();
    if todo.is_empty() && !gone {
        // The entry was used now, so it goes last when the cache needs room.
        manifest.last_used = now_secs();
        let _ = write_manifest(&entry, &manifest);
        return Plan::Ready(copy);
    }
    Plan::Copy(CopyJob {
        root,
        entry,
        key,
        identity: t.identity,
        members: t.members,
        todo,
        max_bytes: settings.max_bytes,
        copy,
    })
}

fn read_manifest(entry: &Path) -> Option<Manifest> {
    let text = fs::read_to_string(entry.join(MANIFEST)).ok()?;
    toml::from_str(&text).ok()
}

/// Writes the manifest to a temporary name first, so that a reader never
/// sees half of it.
fn write_manifest(entry: &Path, m: &Manifest) -> std::io::Result<()> {
    let text = toml::to_string(m).map_err(std::io::Error::other)?;
    let part = entry.join(format!("{MANIFEST}.{}.part", std::process::id()));
    fs::write(&part, text)?;
    fs::rename(&part, entry.join(MANIFEST)).inspect_err(|_| {
        let _ = fs::remove_file(&part);
    })
}

/// One entry of the cache, for the removal of old entries.
struct EntryInfo {
    dir: PathBuf,
    key: String,
    bytes: u64,
    last_used: u64,
}

fn entries(root: &Path) -> Vec<EntryInfo> {
    let Ok(list) = fs::read_dir(root) else {
        return Vec::new();
    };
    list.flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let dir = e.path();
            EntryInfo {
                key: e.file_name().to_string_lossy().into_owned(),
                // The copies take the space. A manifest and a lease take some
                // bytes, and the limit is not about them.
                bytes: dir_bytes(&dir.join(DATA)) + dir_bytes(&dir.join(TMP)),
                last_used: read_manifest(&dir).map(|m| m.last_used).unwrap_or(0),
                dir,
            }
        })
        .collect()
}

fn dir_bytes(dir: &Path) -> u64 {
    let Ok(list) = fs::read_dir(dir) else {
        return 0;
    };
    list.flatten()
        .map(|e| {
            let p = e.path();
            if p.is_dir() {
                dir_bytes(&p)
            } else {
                e.metadata().map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

/// Removes the entries that were used the least recently, until a copy of
/// `need` bytes fits under the limit and on the disk.
///
/// The function never removes the entry `keep`, and never an entry that
/// another Peruse reads now.
fn evict(root: &Path, keep: &str, need: u64, cap: u64, probe: &dyn Probe) {
    let mut list = entries(root);
    let mut total: u64 = list.iter().map(|e| e.bytes).sum();
    let free = probe.free_bytes(root);
    let mut freed = 0u64;
    list.sort_by_key(|e| e.last_used);
    for e in list.iter().filter(|e| e.key != keep) {
        let over = total.saturating_add(need) > cap;
        let short = free.is_some_and(|f| f.saturating_add(freed) < need.saturating_add(RESERVE));
        if !over && !short {
            break;
        }
        if remove_entry(&e.dir) {
            total = total.saturating_sub(e.bytes);
            freed += e.bytes;
        }
    }
}

/// Removes one entry, unless a Peruse reads it now.
///
/// The manifest goes first. An entry that Windows cannot remove completely,
/// because a file is open, then counts as an entry with no copy, and the next
/// session copies the file again and does not read a copy that is not
/// complete.
fn remove_entry(dir: &Path) -> bool {
    if in_use(dir) {
        return false;
    }
    let _ = fs::remove_file(dir.join(MANIFEST));
    fs::remove_dir_all(dir).is_ok()
}

/// Gives `true` when a Peruse holds a lease on the entry.
fn in_use(dir: &Path) -> bool {
    let Ok(list) = fs::read_dir(dir) else {
        return false;
    };
    list.flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with(LEASE) && Lease::is_held(&e.path()))
}

/// What [`clear`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cleared {
    /// The entries that it removed.
    pub removed: usize,
    /// The bytes that it gave back to the disk.
    pub bytes: u64,
    /// The entries that a Peruse reads now, which stay.
    pub kept: usize,
}

/// Removes each local copy that no Peruse reads now.
pub fn clear(root: &Path) -> Cleared {
    let mut out = Cleared::default();
    for e in entries(root) {
        if remove_entry(&e.dir) {
            out.removed += 1;
            out.bytes += e.bytes;
        } else {
            out.kept += 1;
        }
    }
    out
}

/// Gives the number of entries and the bytes that the local copies take.
pub fn usage(root: &Path) -> (usize, u64) {
    let list = entries(root);
    (list.len(), list.iter().map(|e| e.bytes).sum())
}

/// A mark that a Peruse reads an entry now.
///
/// The mark is a file `lease-<pid>` that the process holds open. On Windows
/// the file is open with no permission for a delete, and on the other systems
/// it holds a shared `flock`. A process that stops for any reason closes the
/// file, so a mark of a process that is gone never protects an entry.
#[derive(Debug)]
struct Lease {
    file: Option<File>,
    path: PathBuf,
}

impl Lease {
    fn take(entry: &Path) -> Option<Lease> {
        let path = entry.join(format!("{LEASE}{}", std::process::id()));
        let mut o = fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_SHARE_READ, and no FILE_SHARE_DELETE.
            o.share_mode(1);
        }
        let file = o.open(&path).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            // SAFETY: the descriptor belongs to `file`, which is open here.
            unsafe {
                libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB);
            }
        }
        Some(Lease {
            file: Some(file),
            path,
        })
    }

    /// Gives `true` when a living process holds the lease. A lease of a
    /// process that is gone is removed.
    fn is_held(path: &Path) -> bool {
        #[cfg(windows)]
        {
            match fs::remove_file(path) {
                Ok(()) => false,
                Err(e) => e.kind() != std::io::ErrorKind::NotFound,
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let Ok(f) = File::open(path) else {
                return false;
            };
            // SAFETY: the descriptor belongs to `f`, which is open here.
            let free = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
            if free {
                let _ = fs::remove_file(path);
            }
            !free
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = path;
            false
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // Close the file first. Windows refuses to remove a file that is open
        // with no permission for a delete, and that includes this process.
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A probe that treats each path below `remote` as a network drive.
    struct Fake {
        remote: PathBuf,
        free: Option<u64>,
        asked: Cell<usize>,
    }

    impl Fake {
        fn new(remote: &Path) -> Fake {
            Fake {
                remote: remote.to_path_buf(),
                free: Some(u64::MAX / 4),
                asked: Cell::new(0),
            }
        }
    }

    impl Probe for Fake {
        fn is_remote(&self, path: &Path) -> bool {
            self.asked.set(self.asked.get() + 1);
            inside(path, &self.remote)
        }
        fn free_bytes(&self, _dir: &Path) -> Option<u64> {
            self.free
        }
    }

    /// A directory for one test, with a "share" and a cache inside it.
    fn setup(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("peruse-cache-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let share = base.join("share");
        let cache = base.join("cache");
        fs::create_dir_all(&share).unwrap();
        (base, share, cache)
    }

    fn settings(cache: &Path) -> Settings {
        Settings {
            policy: Policy::Auto,
            min_bytes: 0,
            max_bytes: 1 << 40,
            dir: Some(cache.to_path_buf()),
        }
    }

    fn run(job: CopyJob, probe: &dyn Probe) -> LocalCopy {
        let cancel = AtomicBool::new(false);
        job.run(&mut |_| {}, &cancel, probe).unwrap()
    }

    fn expect_copy(p: Plan) -> CopyJob {
        match p {
            Plan::Copy(j) => j,
            other => panic!("expected a copy, got {other:?}"),
        }
    }

    #[test]
    fn a_file_on_a_local_disk_is_read_in_place() {
        let (base, share, cache) = setup("local");
        let f = share.join("a.csv");
        fs::write(&f, "x\n1\n").unwrap();
        // The fake treats a different directory as the network.
        let probe = Fake::new(&base.join("elsewhere"));
        let p = plan(f.to_str().unwrap(), &settings(&cache), &probe);
        assert!(matches!(p, Plan::InPlace(None)), "{p:?}");
        // The policy `always` copies it all the same.
        let s = Settings {
            policy: Policy::Always,
            ..settings(&cache)
        };
        assert!(matches!(
            plan(f.to_str().unwrap(), &s, &probe),
            Plan::Copy(_)
        ));
        // The policy `never` asks nothing at all.
        let s = Settings {
            policy: Policy::Never,
            ..settings(&cache)
        };
        let before = probe.asked.get();
        assert!(matches!(
            plan(f.to_str().unwrap(), &s, &probe),
            Plan::InPlace(None)
        ));
        assert_eq!(probe.asked.get(), before);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_copy_is_made_one_time_and_used_again() {
        let (base, share, cache) = setup("reuse");
        let f = share.join("data.csv");
        fs::write(&f, "id,name\n1,a\n2,b\n").unwrap();
        let probe = Fake::new(&share);
        let input = f.to_str().unwrap();

        let job = expect_copy(plan(input, &settings(&cache), &probe));
        assert_eq!(job.total_bytes(), 16);
        let mut seen = Vec::new();
        let cancel = AtomicBool::new(false);
        let local = job
            .run(&mut |p| seen.push(p.done), &cancel, &probe)
            .unwrap();
        assert_eq!(seen.last(), Some(&16), "the progress reaches the end");
        assert!(local.copied.is_some());
        assert_eq!(fs::read(&local.input).unwrap(), fs::read(&f).unwrap());
        assert!(
            Path::new(&local.input).ends_with("data.csv"),
            "the copy keeps the name"
        );
        assert_eq!(local.origin, input);
        drop(local);

        // The second open finds the copy, and copies nothing.
        match plan(input, &settings(&cache), &probe) {
            Plan::Ready(l) => assert!(l.copied.is_none()),
            other => panic!("expected the old copy, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_source_that_changed_gets_a_new_copy() {
        let (base, share, cache) = setup("stale");
        let f = share.join("data.csv");
        fs::write(&f, "id\n1\n").unwrap();
        let probe = Fake::new(&share);
        let input = f.to_str().unwrap();
        let old = run(expect_copy(plan(input, &settings(&cache), &probe)), &probe);
        let old_input = old.input.clone();
        drop(old);

        // The same size and a new time is a change too.
        fs::write(&f, "id\n2\n").unwrap();
        let t = SystemTime::now() + Duration::from_secs(5);
        File::options()
            .write(true)
            .open(&f)
            .unwrap()
            .set_modified(t)
            .unwrap();
        let job = expect_copy(plan(input, &settings(&cache), &probe));
        let new = run(job, &probe);
        assert_eq!(new.input, old_input, "the same source keeps its entry");
        assert_eq!(fs::read_to_string(&new.input).unwrap(), "id\n2\n");

        // A copy that somebody cut short is not used.
        drop(new);
        fs::write(&old_input, "id\n").unwrap();
        assert!(matches!(
            plan(input, &settings(&cache), &probe),
            Plan::Copy(_)
        ));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_small_file_is_read_in_place_and_a_huge_one_says_why() {
        let (base, share, cache) = setup("sizes");
        let f = share.join("data.csv");
        fs::write(&f, "id\n1\n").unwrap();
        let probe = Fake::new(&share);
        let input = f.to_str().unwrap();
        let s = Settings {
            min_bytes: 1 << 20,
            ..settings(&cache)
        };
        assert!(matches!(plan(input, &s, &probe), Plan::InPlace(None)));
        // The policy `always` ignores the size limit below.
        let s = Settings {
            policy: Policy::Always,
            ..s
        };
        assert!(matches!(plan(input, &s, &probe), Plan::Copy(_)));
        let s = Settings {
            max_bytes: 3,
            ..settings(&cache)
        };
        match plan(input, &s, &probe) {
            Plan::InPlace(Some(why)) => assert!(why.contains(input), "{why}"),
            other => panic!("{other:?}"),
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_stopped_copy_leaves_no_file() {
        let (base, share, cache) = setup("cancel");
        let f = share.join("big.csv");
        fs::write(&f, vec![b'x'; CHUNK * 2 + 10]).unwrap();
        let probe = Fake::new(&share);
        let job = expect_copy(plan(f.to_str().unwrap(), &settings(&cache), &probe));
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        // The user presses Esc after the first read.
        let r = job.run(
            &mut |_| {
                calls += 1;
                cancel.store(true, Ordering::Relaxed);
            },
            &cancel,
            &probe,
        );
        assert!(matches!(r, Err(CopyError::Cancelled)), "{r:?}");
        assert_eq!(calls, 1, "the copy stops at the next read");
        let entry = fs::read_dir(&cache)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(dir_bytes(&entry), 0, "no part of the copy stays");
        // The next session copies again, from the start.
        assert!(matches!(
            plan(f.to_str().unwrap(), &settings(&cache), &probe),
            Plan::Copy(_)
        ));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_full_disk_stops_the_copy_before_it_starts() {
        let (base, share, cache) = setup("full");
        let f = share.join("data.csv");
        fs::write(&f, "id\n1\n").unwrap();
        let mut probe = Fake::new(&share);
        probe.free = Some(RESERVE / 2);
        let job = expect_copy(plan(f.to_str().unwrap(), &settings(&cache), &probe));
        let cancel = AtomicBool::new(false);
        match job.run(&mut |_| {}, &cancel, &probe) {
            Err(CopyError::Failed(m)) => assert!(m.contains("free"), "{m}"),
            other => panic!("{other:?}"),
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn the_oldest_copy_goes_first_when_the_cache_is_full() {
        let (base, share, cache) = setup("evict");
        let probe = Fake::new(&share);
        let s = Settings {
            max_bytes: 250,
            ..settings(&cache)
        };
        let mut inputs = Vec::new();
        for (i, name) in ["a.csv", "b.csv", "c.csv"].iter().enumerate() {
            let f = share.join(name);
            fs::write(&f, vec![b'0' + i as u8; 100]).unwrap();
            inputs.push(f.to_str().unwrap().to_string());
        }
        let a = run(expect_copy(plan(&inputs[0], &s, &probe)), &probe);
        drop(a);
        let b = run(expect_copy(plan(&inputs[1], &s, &probe)), &probe);
        drop(b);
        // Use `a` again, so that `b` is now the oldest.
        let mut m =
            read_manifest(&cache.join(key_of(&identity_of(Path::new(&inputs[1]))))).unwrap();
        m.last_used = 1;
        write_manifest(&cache.join(key_of(&identity_of(Path::new(&inputs[1])))), &m).unwrap();
        assert!(matches!(plan(&inputs[0], &s, &probe), Plan::Ready(_)));

        let c = run(expect_copy(plan(&inputs[2], &s, &probe)), &probe);
        drop(c);
        assert!(
            matches!(plan(&inputs[0], &s, &probe), Plan::Ready(_)),
            "a stays"
        );
        assert!(
            matches!(plan(&inputs[1], &s, &probe), Plan::Copy(_)),
            "b went"
        );
        assert_eq!(usage(&cache).1, 200);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_copy_that_a_peruse_reads_now_is_never_removed() {
        let (base, share, cache) = setup("lease");
        let probe = Fake::new(&share);
        let s = Settings {
            max_bytes: 150,
            ..settings(&cache)
        };
        let fa = share.join("a.csv");
        let fb = share.join("b.csv");
        fs::write(&fa, vec![b'a'; 100]).unwrap();
        fs::write(&fb, vec![b'b'; 100]).unwrap();
        // The session that reads `a` is still open.
        let a = run(expect_copy(plan(fa.to_str().unwrap(), &s, &probe)), &probe);
        let b = run(expect_copy(plan(fb.to_str().unwrap(), &s, &probe)), &probe);
        assert!(Path::new(&a.input).is_file(), "the copy in use stays");
        assert!(Path::new(&b.input).is_file());
        // The command that clears the cache keeps both while they are open.
        let c = clear(&cache);
        assert_eq!((c.removed, c.kept), (0, 2), "{c:?}");
        drop(a);
        let c = clear(&cache);
        assert_eq!((c.removed, c.kept), (1, 1), "{c:?}");
        assert_eq!(c.bytes, 100);
        drop(b);
        assert_eq!(clear(&cache).removed, 1);
        assert_eq!(usage(&cache), (0, 0));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn the_messages_about_the_copy_name_the_source() {
        let (base, share, cache) = setup("rewrite");
        let f = share.join("data.csv");
        fs::write(&f, "id\n1\n").unwrap();
        let probe = Fake::new(&share);
        let input = f.to_str().unwrap();
        let local = run(expect_copy(plan(input, &settings(&cache), &probe)), &probe);
        let rw = local.rewrite();
        let fwd = local.input.replace('\\', "/");
        let text = format!("read_csv(['{fwd}']) failed; also {}", local.input);
        let back = rw.apply(&text);
        assert!(
            !back.contains(&cache.to_string_lossy().replace('\\', "/")),
            "{back}"
        );
        assert!(!back.contains(&*cache.to_string_lossy()), "{back}");
        assert!(back.contains(&input.replace('\\', "/")), "{back}");
        assert!(back.contains(input), "{back}");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_glob_copies_each_file_and_keeps_the_directories() {
        let (base, share, cache) = setup("glob");
        let probe = Fake::new(&share);
        fs::create_dir_all(share.join("year=2024")).unwrap();
        fs::create_dir_all(share.join("year=2025")).unwrap();
        fs::write(share.join("year=2024/a.csv"), "id\n1\n").unwrap();
        fs::write(share.join("year=2025/b.csv"), "id\n2\n").unwrap();
        fs::write(share.join("notes.txt"), "not data").unwrap();
        let pattern = format!("{}/*/*.csv", share.to_string_lossy().replace('\\', "/"));
        let job = expect_copy(plan(&pattern, &settings(&cache), &probe));
        assert_eq!(job.files(), 2);
        let local = run(job, &probe);
        assert!(local.input.ends_with("/data/*/*.csv"), "{}", local.input);
        assert!(local.dir.join("year=2024").join("a.csv").is_file());
        assert!(!local.dir.join("notes.txt").exists());
        // The read expression names the share again.
        let fwd = local.dir.to_string_lossy().replace('\\', "/");
        let back = local.rewrite().apply(&format!("['{fwd}/year=2025/b.csv']"));
        assert_eq!(
            back,
            format!(
                "['{}/year=2025/b.csv']",
                share.to_string_lossy().replace('\\', "/")
            )
        );
        drop(local);

        // A file that left the share leaves the copy too, and the other file
        // is not copied again.
        fs::remove_file(share.join("year=2024/a.csv")).unwrap();
        let job = expect_copy(plan(&pattern, &settings(&cache), &probe));
        assert_eq!(job.files(), 0, "b is still correct");
        let local = run(job, &probe);
        assert!(!local.dir.join("year=2024").join("a.csv").exists());
        assert!(local.dir.join("year=2025").join("b.csv").is_file());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_file_in_the_cache_is_never_copied_again() {
        let (base, share, cache) = setup("inside");
        let f = share.join("data.csv");
        fs::write(&f, "id\n1\n").unwrap();
        let probe = Fake::new(&share);
        let local = run(
            expect_copy(plan(f.to_str().unwrap(), &settings(&cache), &probe)),
            &probe,
        );
        let s = Settings {
            policy: Policy::Always,
            ..settings(&cache)
        };
        assert!(matches!(
            plan(&local.input, &s, &probe),
            Plan::InPlace(None)
        ));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_part_that_a_stopped_peruse_left_goes_and_a_fresh_part_stays() {
        let (base, _share, _cache) = setup("parts");
        let tmp = base.join("tmp");
        fs::create_dir_all(&tmp).unwrap();
        let old = tmp.join("0.1234.part");
        let fresh = tmp.join("0.5678.part");
        fs::write(&old, "x").unwrap();
        fs::write(&fresh, "x").unwrap();
        let then = SystemTime::now() - OLD_PART - Duration::from_secs(60);
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(then)
            .unwrap();
        remove_old_parts(&tmp);
        assert!(!old.exists());
        assert!(fresh.exists(), "a copy that runs now keeps its part");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn the_key_does_not_change_between_versions() {
        // A new key would copy each file again after an update.
        assert_eq!(key_of(""), "cbf29ce484222325");
        assert_eq!(key_of("a"), "af63dc4c8601ec8c");
    }

    #[test]
    fn the_policy_reads_from_the_settings_file() {
        assert_eq!(Policy::parse("Auto"), Some(Policy::Auto));
        assert_eq!(Policy::parse(" always "), Some(Policy::Always));
        assert_eq!(Policy::parse("never"), Some(Policy::Never));
        assert_eq!(Policy::parse("sometimes"), None);
        for p in [Policy::Auto, Policy::Always, Policy::Never] {
            assert_eq!(Policy::parse(p.name()), Some(p));
        }
    }
}
