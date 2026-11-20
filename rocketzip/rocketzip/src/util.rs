//! Errors, progress reporting, safe extraction paths, and source collection.
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone)]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

macro_rules! from_err {
    ($($t:ty),*) => { $(impl From<$t> for Error { fn from(e: $t) -> Self { Error(e.to_string()) } })* };
}
from_err!(io::Error, zip::result::ZipError, serde_json::Error, liblzma::stream::Error,
          sevenz_rust2::Error, std::string::FromUtf8Error);

#[macro_export]
macro_rules! bail {
    ($($t:tt)*) => { return Err($crate::util::Error(format!($($t)*))) };
}

pub fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error(msg.into()))
}

/// Progress callback: (current item name, bytes done, bytes total).
pub type ProgressFn = dyn Fn(&str, u64, u64) + Send + Sync;

/// Thread-safe byte counter that reports to an optional callback.
pub struct Progress<'a> {
    pub total: AtomicU64,
    done: AtomicU64,
    cb: Option<&'a ProgressFn>,
}

impl<'a> Progress<'a> {
    pub fn new(total: u64, cb: Option<&'a ProgressFn>) -> Self {
        Progress { total: AtomicU64::new(total), done: AtomicU64::new(0), cb }
    }
    pub fn advance(&self, n: u64, name: &str) {
        let d = self.done.fetch_add(n, Ordering::Relaxed) + n;
        if let Some(cb) = self.cb {
            cb(name, d, self.total.load(Ordering::Relaxed));
        }
    }
    pub fn set_total(&self, t: u64) {
        self.total.store(t, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
    }
    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }
}

/// Reader wrapper that reports bytes read.
pub struct ProgressReader<'p, 'a, R> {
    pub inner: R,
    pub prog: &'p Progress<'a>,
    pub name: String,
}

impl<R: Read> Read for ProgressReader<'_, '_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.prog.advance(n as u64, &self.name);
        Ok(n)
    }
}

pub const CHUNK: usize = 1 << 20;

/// Copy with progress, returning bytes copied.
pub fn copy_progress(r: &mut dyn Read, w: &mut dyn Write, prog: &Progress, name: &str) -> io::Result<u64> {
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(total);
        }
        w.write_all(&buf[..n])?;
        total += n as u64;
        prog.advance(n as u64, name);
    }
}

pub fn human(n: Option<u64>) -> String {
    let Some(n) = n else { return "?".into() };
    let mut v = n as f64;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if v < 1024.0 || unit == "TB" {
            return if unit == "B" { format!("{n} B") } else { format!("{v:.1} {unit}") };
        }
        v /= 1024.0;
    }
    unreachable!()
}

/// Windows device names that can't be used as file names (any extension).
const RESERVED: &[&str] = &["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$",
    "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"];

/// Is one path component safe on every platform?
fn safe_component(p: &str) -> bool {
    if p.chars().any(|c| (c as u32) < 0x20 || c == ':') {
        return false; // control chars; ':' = drive letter or NTFS stream on Windows
    }
    // Windows silently strips trailing dots/spaces: ".. " and ". ." act as "..".
    if p.trim_end_matches([' ', '.']).is_empty() {
        return false;
    }
    let stem = p.split('.').next().unwrap_or("").trim_end_matches(' ').to_ascii_uppercase();
    !RESERVED.contains(&stem.as_str())
}

/// Map an archive member name to a path inside `dest`, or None if it would
/// escape (absolute path, drive letter, `..`, Windows tricks): "zip slip".
pub fn safe_target(dest: &Path, name: &str) -> Option<PathBuf> {
    let name = name.replace('\\', "/");
    let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.is_empty() || !parts.iter().all(|p| safe_component(p)) {
        return None;
    }
    let mut t = dest.to_path_buf();
    for p in parts {
        t.push(p);
    }
    Some(t)
}

/// Refuse to go through a symbolic link (or junction) inside the destination:
/// a malicious archive could first plant a link and then write "through" it.
/// Checks every existing component below `dest`, including `target` itself
/// when `include_last` is set.
pub fn check_no_links(dest: &Path, target: &Path, include_last: bool) -> Result<()> {
    let rel = target.strip_prefix(dest).map_err(|_| Error(format!("blocked unsafe path: {}", target.display())))?;
    let comps: Vec<_> = rel.components().collect();
    let n = if include_last { comps.len() } else { comps.len().saturating_sub(1) };
    let mut cur = dest.to_path_buf();
    for c in &comps[..n] {
        cur.push(c);
        if let Ok(m) = fs::symlink_metadata(&cur) {
            if m.file_type().is_symlink() || is_junction(&m) {
                bail!("blocked: {} would be written through a link ({})", target.display(), cur.display());
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_junction(m: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    m.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
}
#[cfg(not(windows))]
fn is_junction(_m: &fs::Metadata) -> bool {
    false
}

/// Create a directory inside `dest` without following links.
pub fn create_dir_safe(dest: &Path, target: &Path) -> Result<()> {
    check_no_links(dest, target, true)?;
    fs::create_dir_all(target)?;
    check_no_links(dest, target, true)
}

/// Create (or replace) a file inside `dest` without following links.
pub fn create_file_safe(dest: &Path, target: &Path) -> Result<File> {
    if let Some(p) = target.parent() {
        create_dir_safe(dest, p)?;
    }
    check_no_links(dest, target, false)?;
    match fs::symlink_metadata(target) {
        Ok(m) if m.is_dir() => bail!("cannot replace folder {} with a file", target.display()),
        Ok(_) => fs::remove_file(target)?, // also removes a planted link instead of following it
        Err(_) => {}
    }
    Ok(fs::OpenOptions::new().write(true).create_new(true).open(target)?)
}

/// Stops extraction before a "decompression bomb" fills the disk.
pub struct DiskGuard {
    dest: PathBuf,
    since: u64,
}

const GUARD_EVERY: u64 = 64 << 20;
const GUARD_RESERVE: u64 = 256 << 20;

impl DiskGuard {
    pub fn new(dest: &Path) -> DiskGuard {
        DiskGuard { dest: dest.to_path_buf(), since: 0 }
    }
    pub fn tick(&mut self, n: u64) -> io::Result<()> {
        self.since += n;
        if self.since >= GUARD_EVERY {
            self.since = 0;
            if let Ok(free) = fs4::available_space(&self.dest) {
                if free < GUARD_RESERVE {
                    return Err(io::Error::other(
                        "stopped: the disk is nearly full (the archive may be a decompression bomb)"));
                }
            }
        }
        Ok(())
    }
}

/// Copy during extraction: progress plus the disk-space guard.
pub fn copy_extract(r: &mut dyn Read, w: &mut dyn Write, prog: &Progress, name: &str,
                    guard: &mut DiskGuard) -> io::Result<u64> {
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(total);
        }
        guard.tick(n as u64)?;
        w.write_all(&buf[..n])?;
        total += n as u64;
        prog.advance(n as u64, name);
    }
}

pub fn check_space(dest: &Path, need: u64) -> Result<()> {
    if let Ok(free) = fs4::available_space(dest)
        && need > free {
            bail!("Not enough disk space: need {}, free {}", human(Some(need)), human(Some(free)));
        }
    Ok(())
}

/// Temporary sibling path used for atomic writes.
pub fn part_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

pub fn set_mtime(p: &Path, secs: f64) {
    if secs.is_finite() && secs > 0.0 {
        let ft = filetime::FileTime::from_unix_time(secs.trunc() as i64, (secs.fract() * 1e9) as u32);
        let _ = filetime::set_file_mtime(p, ft);
    }
}

#[cfg(unix)]
pub fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    if mode & 0o777 != 0 {
        let _ = fs::set_permissions(p, fs::Permissions::from_mode((mode & 0o777) | 0o600));
    }
}
#[cfg(not(unix))]
pub fn set_mode(_p: &Path, _mode: u32) {}

pub fn file_mode(meta: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        if meta.permissions().readonly() { 0o444 } else { 0o644 }
    }
}

pub fn mtime_secs(meta: &fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub arcname: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Files and folders to archive, in a stable order, without duplicates and
/// without the archive being written (`exclude`).
pub fn collect_sources(sources: &[PathBuf], exclude: &[PathBuf]) -> Result<Vec<Source>> {
    let excl: Vec<PathBuf> = exclude.iter().filter_map(|e| dunce::canonicalize(e).ok()).collect();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    fn add(out: &mut Vec<Source>, seen: &mut std::collections::HashSet<String>, excl: &[PathBuf],
           p: &Path, arc: String, is_dir: bool) {
        if !seen.insert(arc.clone()) {
            return;
        }
        let mut size = 0;
        if !is_dir {
            if let Ok(c) = dunce::canonicalize(p)
                && excl.contains(&c) {
                    seen.remove(&arc);
                    return;
                }
            match fs::metadata(p) {
                Ok(m) => size = m.len(),
                Err(e) => {
                    eprintln!("warning: skipping {}: {e}", p.display());
                    seen.remove(&arc);
                    return;
                }
            }
        }
        out.push(Source { path: p.to_path_buf(), arcname: arc, is_dir, size });
    }

    fn walk(out: &mut Vec<Source>, seen: &mut std::collections::HashSet<String>, excl: &[PathBuf],
            dir: &Path, arc: &str) {
        let mut items: Vec<_> = match fs::read_dir(dir) {
            Ok(r) => r.filter_map(|e| e.ok()).collect(),
            Err(e) => {
                eprintln!("warning: cannot read {}: {e}", dir.display());
                return;
            }
        };
        items.sort_by_key(|e| e.file_name());
        let (mut dirs, mut files) = (Vec::new(), Vec::new());
        for e in items {
            let p = e.path();
            let is_link = fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(false);
            if p.is_dir() {
                dirs.push((p, is_link));
            } else {
                files.push(p);
            }
        }
        for (d, _) in &dirs {
            let a = format!("{arc}/{}", d.file_name().unwrap().to_string_lossy());
            add(out, seen, excl, d, a, true);
        }
        for f in &files {
            let a = format!("{arc}/{}", f.file_name().unwrap().to_string_lossy());
            add(out, seen, excl, f, a, false);
        }
        for (d, is_link) in &dirs {
            if !is_link {
                let a = format!("{arc}/{}", d.file_name().unwrap().to_string_lossy());
                walk(out, seen, excl, d, &a);
            }
        }
    }

    for s in sources {
        if !s.exists() {
            bail!("Not found: {}", s.display());
        }
        let name = match s.file_name() {
            Some(n) if n != "." && n != ".." => n.to_string_lossy().into_owned(),
            _ => dunce::canonicalize(s)?
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "root".into()),
        };
        if s.is_dir() {
            add(&mut out, &mut seen, &excl, s, name.clone(), true);
            walk(&mut out, &mut seen, &excl, s, &name);
        } else {
            add(&mut out, &mut seen, &excl, s, name, false);
        }
    }
    Ok(out)
}

/// Group files by extension (like 7-Zip) so similar data sits together.
pub fn solid_order(entries: Vec<Source>) -> Vec<Source> {
    let (mut dirs, mut files): (Vec<_>, Vec<_>) = entries.into_iter().partition(|e| e.is_dir);
    files.sort_by_key(|e| {
        let p = Path::new(&e.arcname);
        (
            p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default(),
            p.file_name().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default(),
            e.arcname.clone(),
        )
    });
    dirs.append(&mut files);
    dirs
}

pub fn ext_lower(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|x| format!(".{}", x.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}
