//! TAR family (tar, tar.gz/bz2/xz/zst) and single-file streams (gz/bz2/xz/zst).
use crate::formats::{open_stream, strip_archive_ext};
use crate::util::*;
use crate::{bail, Entry};
use liblzma::stream::{Check, Filters, LzmaOptions, Stream, PRESET_EXTREME};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// XZ / LZMA2 filter chains (shared with Ultra)
// ---------------------------------------------------------------------------
#[derive(Clone, Debug)]
pub enum Flt {
    X86,
    Arm,
    ArmThumb,
    Delta(u32),
    Lzma2 { level: u32, dict: u32, nice: Option<u32>, lc: Option<u32>, lp: Option<u32>, pb: Option<u32> },
}

pub const PRESET_DICT: [u32; 10] = [1 << 18, 1 << 20, 2 << 20, 4 << 20, 4 << 20, 8 << 20, 8 << 20,
                                     16 << 20, 32 << 20, 64 << 20];

/// LZMA2 filter: level 9 adds the extreme preset, the given dictionary and nice_len 273.
pub fn lz(level: u32, d: u32) -> Flt {
    if level >= 9 {
        Flt::Lzma2 { level, dict: d, nice: Some(273), lc: None, lp: None, pb: None }
    } else {
        Flt::Lzma2 { level, dict: d.min(PRESET_DICT[level as usize]), nice: None, lc: None, lp: None, pb: None }
    }
}

pub fn build_filters(chain: &[Flt]) -> Result<Filters> {
    let mut f = Filters::new();
    for x in chain {
        match x {
            Flt::X86 => { f.x86(); }
            Flt::Arm => { f.arm(); }
            Flt::ArmThumb => { f.arm_thumb(); }
            Flt::Delta(n) => { f.delta_properties(&[(*n - 1) as u8])?; }
            Flt::Lzma2 { level, dict, nice, lc, lp, pb } => {
                let preset = level | if *level >= 9 { PRESET_EXTREME } else { 0 };
                let mut o = LzmaOptions::new_preset(preset)?;
                o.dict_size(*dict);
                if let Some(n) = nice { o.nice_len(*n); }
                if let Some(v) = lc { o.literal_context_bits(*v); }
                if let Some(v) = lp { o.literal_position_bits(*v); }
                if let Some(v) = pb { o.position_bits(*v); }
                f.lzma2(&o);
            }
        }
    }
    Ok(f)
}

pub fn pow2(n: u64, lo: u32, hi: u32) -> u32 {
    let mut d = lo as u64;
    while d < n && d < hi as u64 {
        d <<= 1;
    }
    d.min(hi as u64) as u32
}

/// Same settings as the Python version's .tar.xz writer.
pub fn tar_xz_chain(level: u32, total: u64, dict_mb: Option<u32>) -> Vec<Flt> {
    if level >= 9 {
        let cap = dict_mb.unwrap_or(64) << 20;
        vec![lz(9, pow2(total, 1 << 18, cap))]
    } else {
        vec![lz(level, PRESET_DICT[level as usize])]
    }
}

pub fn xz_writer<W: Write>(w: W, chain: &[Flt]) -> Result<liblzma::write::XzEncoder<W>> {
    let stream = Stream::new_stream_encoder(&build_filters(chain)?, Check::Crc64)?;
    Ok(liblzma::write::XzEncoder::new_stream(w, stream))
}

pub fn zstd_level(level: u32) -> i32 {
    match level {
        0 => 1,
        9 => 22,
        l => ((l as f64) * 19.0 / 8.0).round().max(1.0) as i32,
    }
}

// ---------------------------------------------------------------------------
// compression sinks
// ---------------------------------------------------------------------------
pub enum Sink {
    Plain(BufWriter<File>),
    Gz(flate2::write::GzEncoder<BufWriter<File>>),
    Bz(bzip2::write::BzEncoder<BufWriter<File>>),
    Xz(liblzma::write::XzEncoder<BufWriter<File>>),
    Zst(zstd::stream::write::Encoder<'static, BufWriter<File>>),
}

impl Sink {
    pub fn new(path: &Path, comp: &str, level: u32, total: u64, dict_mb: Option<u32>,
               gz_name: Option<&str>) -> Result<Sink> {
        let w = BufWriter::new(File::create(path)?);
        Ok(match comp {
            "" => Sink::Plain(w),
            "gz" => {
                let mut b = flate2::GzBuilder::new();
                if let Some(n) = gz_name {
                    b = b.filename(n.as_bytes());
                }
                Sink::Gz(b.write(w, flate2::Compression::new(level)))
            }
            "bz2" => Sink::Bz(bzip2::write::BzEncoder::new(w, bzip2::Compression::new(level.max(1)))),
            "xz" => Sink::Xz(xz_writer(w, &tar_xz_chain(level, total, dict_mb))?),
            "zst" => {
                let mut e = zstd::stream::write::Encoder::new(w, zstd_level(level))?;
                let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
                let _ = e.multithread(threads as u32);
                if level >= 9 {
                    let _ = e.long_distance_matching(true);
                }
                Sink::Zst(e)
            }
            other => bail!("unknown compression {other}"),
        })
    }

    pub fn finish(self) -> Result<()> {
        match self {
            Sink::Plain(mut w) => w.flush()?,
            Sink::Gz(e) => { e.finish()?.flush()?; }
            Sink::Bz(e) => { e.finish()?.flush()?; }
            Sink::Xz(e) => { e.finish()?.flush()?; }
            Sink::Zst(e) => { e.finish()?.flush()?; }
        }
        Ok(())
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Sink::Plain(w) => w.write(buf),
            Sink::Gz(w) => w.write(buf),
            Sink::Bz(w) => w.write(buf),
            Sink::Xz(w) => w.write(buf),
            Sink::Zst(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Sink::Plain(w) => w.flush(),
            Sink::Gz(w) => w.flush(),
            Sink::Bz(w) => w.flush(),
            Sink::Xz(w) => w.flush(),
            Sink::Zst(w) => w.flush(),
        }
    }
}

fn comp_of(fmt: &str) -> &str {
    fmt.strip_prefix("tar").map(|s| s.trim_start_matches('.')).unwrap_or(fmt)
}

// ---------------------------------------------------------------------------
// TAR write
// ---------------------------------------------------------------------------
pub fn write_tar(tmp: &Path, entries: &[Source], fmt: &str, level: u32, dict_mb: Option<u32>,
                 prog: &Progress) -> Result<()> {
    let total: u64 = entries.iter().map(|e| e.size).sum();
    let sink = Sink::new(tmp, comp_of(fmt), level, total, dict_mb, None)?;
    let mut b = tar::Builder::new(sink);
    b.follow_symlinks(false);
    for e in entries {
        let lmeta = fs::symlink_metadata(&e.path)?;
        let mut h = tar::Header::new_gnu();
        if lmeta.file_type().is_symlink() {
            h.set_metadata_in_mode(&lmeta, tar::HeaderMode::Complete);
            h.set_size(0);
            let target = fs::read_link(&e.path)?;
            b.append_link(&mut h, &e.arcname, &target)?;
        } else if e.is_dir {
            h.set_metadata_in_mode(&lmeta, tar::HeaderMode::Complete);
            h.set_entry_type(tar::EntryType::Directory);
            h.set_size(0);
            b.append_data(&mut h, format!("{}/", e.arcname), io::empty())?;
        } else {
            let f = File::open(&e.path)?;
            let meta = f.metadata()?;
            h.set_metadata_in_mode(&meta, tar::HeaderMode::Complete);
            h.set_size(meta.len());
            let r = ProgressReader { inner: BufReader::new(f), prog, name: e.arcname.clone() };
            b.append_data(&mut h, &e.arcname, r)?;
        }
    }
    b.into_inner()?.finish()
}

// ---------------------------------------------------------------------------
// TAR read
// ---------------------------------------------------------------------------
/// Counts compressed bytes read, so progress works for streaming formats.
struct Counting<'p, 'a> {
    f: BufReader<File>,
    prog: &'p Progress<'a>,
    name: std::cell::RefCell<String>,
}
impl Read for Counting<'_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.f.read(buf)?;
        self.prog.advance(n as u64, &self.name.borrow());
        Ok(n)
    }
}

fn open_tar_reader<'p, 'a>(archive: &Path, fmt: &str, prog: &'p Progress<'a>)
                           -> Result<tar::Archive<Box<dyn Read + 'p>>> {
    let c = Counting { f: BufReader::new(File::open(archive)?), prog, name: Default::default() };
    let r: Box<dyn Read + 'p> = match comp_of(fmt) {
        "" => Box::new(c),
        "gz" => Box::new(flate2::read::MultiGzDecoder::new(c)),
        "bz2" => Box::new(bzip2::read::MultiBzDecoder::new(c)),
        "xz" => Box::new(liblzma::read::XzDecoder::new_multi_decoder(c)),
        "zst" => Box::new(zstd::stream::read::Decoder::new(c)?),
        other => bail!("unknown tar compression {other}"),
    };
    Ok(tar::Archive::new(r))
}

fn entry_name(e: &tar::Entry<impl Read>) -> String {
    String::from_utf8_lossy(&e.path_bytes()).into_owned()
}

pub fn extract_tar(archive: &Path, fmt: &str, dest: &Path, overwrite: bool,
                   cb: Option<&ProgressFn>) -> Result<()> {
    let prog = Progress::new(fs::metadata(archive)?.len(), cb);
    let mut ar = open_tar_reader(archive, fmt, &prog)?;
    let mut guard = DiskGuard::new(dest);
    let quiet = Progress::new(0, None);
    // Links are created only after every file, so nothing is ever written through them.
    let mut hardlinks: Vec<(PathBuf, String)> = Vec::new();
    let mut symlinks: Vec<(PathBuf, String, String)> = Vec::new();
    for ent in ar.entries()? {
        let mut ent = ent?;
        let name = entry_name(&ent);
        let Some(target) = safe_target(dest, &name) else {
            eprintln!("warning: blocked unsafe path: {name}");
            continue;
        };
        let kind = ent.header().entry_type();
        if kind.is_dir() {
            create_dir_safe(dest, &target)?;
            continue;
        }
        if !overwrite && target.exists() {
            continue;
        }
        let mtime = ent.header().mtime().unwrap_or(0) as f64;
        let mode = ent.header().mode().unwrap_or(0);
        if kind.is_file() || kind == tar::EntryType::Continuous || kind == tar::EntryType::GNUSparse {
            let mut out = BufWriter::new(create_file_safe(dest, &target)?);
            copy_extract(&mut ent, &mut out, &quiet, &name, &mut guard)?;
            out.flush()?;
            drop(out);
            set_mtime(&target, mtime);
            set_mode(&target, mode);
        } else if kind.is_symlink() {
            let link = ent.link_name()?.map(|l| l.to_string_lossy().into_owned()).unwrap_or_default();
            // Only relative links that stay inside the destination are created.
            let parent_rel = Path::new(&name).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let resolved = format!("{parent_rel}/{link}");
            if link.starts_with('/') || link.contains(':') || !stays_inside(&resolved) {
                eprintln!("warning: blocked link outside destination: {name} -> {link}");
                continue;
            }
            symlinks.push((target, link, name));
        } else if kind.is_hard_link() {
            let link = ent.link_name()?.map(|l| l.to_string_lossy().into_owned()).unwrap_or_default();
            hardlinks.push((target, link));
        } else {
            eprintln!("warning: skipped special file: {name}");
        }
    }
    for (target, link) in hardlinks {
        let src = safe_target(dest, &link).filter(|s| {
            check_no_links(dest, s, true).is_ok()
                && fs::symlink_metadata(s).is_ok_and(|m| m.file_type().is_file())
        });
        match src {
            Some(src) => {
                let mut out = create_file_safe(dest, &target)?;
                io::copy(&mut File::open(&src)?, &mut out)?;
            }
            None => eprintln!("warning: blocked hard link: {} -> {link}", target.display()),
        }
    }
    for (target, link, name) in symlinks {
        if check_no_links(dest, &target, true).is_err() || fs::symlink_metadata(&target).is_ok() {
            eprintln!("warning: skipped link {name}: path already used");
            continue;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&link, &target)?;
        #[cfg(not(unix))]
        eprintln!("warning: symbolic link not created on this system: {name} -> {link}");
    }
    Ok(())
}

/// Does a relative path stay inside its root after resolving `..`?
fn stays_inside(p: &str) -> bool {
    let mut depth: i32 = 0;
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => depth += 1,
        }
    }
    true
}

pub fn list_tar(archive: &Path, fmt: &str) -> Result<Vec<Entry>> {
    let prog = Progress::new(0, None);
    let mut ar = open_tar_reader(archive, fmt, &prog)?;
    let mut out = Vec::new();
    for ent in ar.entries()? {
        let ent = ent?;
        let is_dir = ent.header().entry_type().is_dir();
        let mut name = entry_name(&ent);
        if is_dir && !name.ends_with('/') {
            name.push('/');
        }
        out.push(Entry { name, size: Some(ent.size()), packed: None,
                         mtime: ent.header().mtime().ok().map(|m| m as f64), is_dir });
    }
    Ok(out)
}

pub fn test_tar(archive: &Path, fmt: &str, cb: Option<&ProgressFn>) -> Result<(bool, String)> {
    let prog = Progress::new(fs::metadata(archive)?.len(), cb);
    let run = || -> Result<usize> {
        let mut ar = open_tar_reader(archive, fmt, &prog)?;
        let mut n = 0;
        for ent in ar.entries()? {
            let mut ent = ent?;
            io::copy(&mut ent, &mut io::sink())?;
            n += 1;
        }
        Ok(n)
    };
    Ok(match run() {
        Ok(n) => (true, format!("OK ({n} entries)")),
        Err(e) => (false, e.0),
    })
}

// ---------------------------------------------------------------------------
// single-file streams
// ---------------------------------------------------------------------------
pub fn stream_out_name(archive: &Path) -> String {
    let name = archive.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let stem = strip_archive_ext(&name);
    if stem != name { stem } else { format!("{name}.out") }
}

pub fn write_stream(tmp: &Path, entries: &[Source], fmt: &str, level: u32, dict_mb: Option<u32>,
                    prog: &Progress) -> Result<()> {
    let files: Vec<&Source> = entries.iter().filter(|e| !e.is_dir).collect();
    if files.len() != 1 {
        bail!(".{fmt} holds exactly one file - use tar.{fmt} for several");
    }
    let e = files[0];
    let base = Path::new(&e.arcname).file_name().map(|n| n.to_string_lossy().into_owned());
    let mut sink = Sink::new(tmp, fmt, level, e.size, dict_mb, base.as_deref())?;
    let mut src = BufReader::new(File::open(&e.path)?);
    copy_progress(&mut src, &mut sink, prog, &e.arcname)?;
    sink.finish()
}

pub fn extract_stream(archive: &Path, fmt: &str, dest: &Path, overwrite: bool,
                      cb: Option<&ProgressFn>) -> Result<()> {
    let out = dest.join(stream_out_name(archive));
    if out.exists() && !overwrite {
        return Ok(());
    }
    let prog = Progress::new(0, cb);
    let tmp = part_path(&out);
    let mut guard = DiskGuard::new(dest);
    let res = (|| -> Result<()> {
        let mut src = open_stream(archive, fmt)?;
        let mut w = BufWriter::new(create_file_safe(dest, &tmp)?);
        let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        copy_extract(&mut src, &mut w, &prog, &name, &mut guard)?;
        w.flush()?;
        Ok(())
    })();
    match res {
        Ok(()) => {
            fs::rename(&tmp, &out)?;
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

pub fn list_stream(archive: &Path, fmt: &str) -> Result<Vec<Entry>> {
    let meta = fs::metadata(archive)?;
    let mut size = None;
    if fmt == "gz" && meta.len() >= 4 {
        use std::io::{Seek, SeekFrom};
        let mut f = File::open(archive)?;
        f.seek(SeekFrom::End(-4))?;
        let mut b = [0u8; 4];
        f.read_exact(&mut b)?;
        size = Some(u32::from_le_bytes(b) as u64); // ISIZE: size mod 4 GiB
    }
    Ok(vec![Entry { name: stream_out_name(archive), size, packed: Some(meta.len()),
                    mtime: Some(mtime_secs(&meta)), is_dir: false }])
}

pub fn test_stream(archive: &Path, fmt: &str) -> Result<(bool, String)> {
    let run = || -> Result<()> {
        io::copy(&mut open_stream(archive, fmt)?, &mut io::sink())?;
        Ok(())
    };
    Ok(match run() {
        Ok(()) => (true, "OK".into()),
        Err(e) => (false, e.0),
    })
}
