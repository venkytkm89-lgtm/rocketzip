//! Format-independent operations: create, extract, list, test, best.
use crate::formats::*;
use crate::ultra::{self, UltraStats};
use crate::util::*;
use crate::{bail, sevenz, tarfmt, zipfmt, rarfmt, Entry};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CreateOptions {
    /// Format (default: from the archive's extension).
    pub fmt: Option<String>,
    /// 0 (store) ..= 9 (ultra).
    pub level: u32,
    /// ZIP method: deflate | bzip2 | zstd | xz | store.
    pub method: Option<String>,
    pub password: Option<String>,
    /// ZIP: store already-compressed files (jpg, mp4, docx...) instead of recompressing.
    pub store_compressed: bool,
    /// LZMA dictionary cap in MB for zfx / tar.xz / 7z (default 64, max 1536).
    pub dict_mb: Option<u32>,
    /// Ultra document recompression: exact | full | off.
    pub docs: String,
    /// Ultra: store identical files once.
    pub dedup: bool,
    /// Ultra engine: "lzma" (default) or "max" (adds context mixing).
    pub engine: String,
}

impl Default for CreateOptions {
    fn default() -> Self {
        CreateOptions { fmt: None, level: 9, method: None, password: None, store_compressed: true,
                        dict_mb: None, docs: "exact".into(), dedup: true, engine: "lzma".into() }
    }
}

fn remove_quiet(p: &Path) {
    let _ = fs::remove_file(p);
}

/// Create `archive` from files/folders. Returns Ultra statistics for .zfx.
pub fn create_archive(archive: &Path, sources: &[PathBuf], o: &CreateOptions,
                      cb: Option<&ProgressFn>) -> Result<Option<UltraStats>> {
    let fmt = alias(o.fmt.as_deref().or(format_from_name(archive)).unwrap_or(""));
    if !writable_formats().contains(&fmt.as_str()) {
        bail!("Cannot write format '{fmt}'. Available: {}", writable_formats().join(", "));
    }
    if o.level > 9 {
        bail!("Compression level must be 0-9");
    }
    if o.password.is_some() && fmt != "7z" && fmt != "zip" {
        bail!("Only 7z and zip support passwords (Ultra .zfx does not encrypt)");
    }
    if let Some(d) = o.dict_mb
        && !(1..=1536).contains(&d) {
            bail!("Dictionary size must be 1-1536 MB");
        }
    let mut entries = collect_sources(sources, &[archive.to_path_buf()])?;
    if entries.is_empty() {
        bail!("Nothing to add");
    }
    if is_stream(&fmt) && !entries.iter().any(|e| !e.is_dir) {
        bail!(".{fmt} compresses a single file");
    }
    if SOLID_FORMATS.contains(&fmt.as_str()) {
        entries = solid_order(entries);
    }
    let prog = Progress::new(entries.iter().map(|e| e.size).sum(), cb);
    let dir = archive.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&dir)?;
    // Random, exclusively created temp name: nobody can pre-plant a link at it.
    let tmp_path = tempfile::Builder::new().prefix(".rocketzip-").suffix(".part")
        .tempfile_in(&dir)?.into_temp_path();
    let tmp = tmp_path.to_path_buf();
    let pw = o.password.as_deref();
    let res: Result<Option<UltraStats>> = (|| {
        match fmt.as_str() {
            "zip" => zipfmt::write_zip(&tmp, &entries, o.level, o.method.as_deref(), pw,
                                       o.store_compressed, &prog)?,
            "7z" => sevenz::write_7z(&tmp, &entries, o.level, o.dict_mb, pw, &prog)?,
            f if is_tar_family(f) => tarfmt::write_tar(&tmp, &entries, f, o.level, o.dict_mb, &prog)?,
            f if is_stream(f) => tarfmt::write_stream(&tmp, &entries, f, o.level, o.dict_mb, &prog)?,
            "zfx" => {
                let st = ultra::write_zfx(&tmp, &entries, &ultra::UltraOptions {
                    level: o.level, dict_mb: o.dict_mb, docs: o.docs.clone(), dedup: o.dedup,
                    engine: o.engine.clone(), threads: None,
                }, cb)?;
                return Ok(Some(st));
            }
            _ => unreachable!(),
        }
        Ok(None)
    })();
    match res {
        Ok(st) => {
            tmp_path.persist(archive).map_err(|e| Error(e.to_string()))?;
            Ok(st)
        }
        Err(e) => {
            remove_quiet(&tmp);
            Err(e)
        }
    }
}

/// Default destination: folder named after the archive (streams: next to it).
pub fn default_dest(archive: &Path, fmt: &str) -> PathBuf {
    if is_stream(fmt) {
        return archive.parent().map(Path::to_path_buf).unwrap_or_default();
    }
    let name = archive.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let stem = strip_archive_ext(&name);
    archive.with_file_name(if stem != name { stem } else { format!("{name}_extracted") })
}

pub fn extract_archive(archive: &Path, dest: Option<&Path>, password: Option<&str>,
                       overwrite: bool, cb: Option<&ProgressFn>) -> Result<PathBuf> {
    let fmt = detect_format(archive)?;
    let dest = dest.map(Path::to_path_buf).unwrap_or_else(|| default_dest(archive, &fmt));
    fs::create_dir_all(&dest)?;
    let dest = dunce::canonicalize(&dest)?;
    match fmt.as_str() {
        "zip" => zipfmt::extract_zip(archive, &dest, password, overwrite, cb)?,
        "7z" => sevenz::extract_7z(archive, &dest, password, overwrite, cb)?,
        "rar" => rarfmt::extract_rar(archive, &dest, password, overwrite, cb)?,
        "zfx" => {
            let st = ultra::extract_zfx(archive, Some(&dest), overwrite, cb)?;
            if !st.errors.is_empty() {
                bail!("{} file(s) could not be restored:\n  {}", st.errors.len(),
                      st.errors.iter().take(10).cloned().collect::<Vec<_>>().join("\n  "));
            }
        }
        f if is_tar_family(f) => tarfmt::extract_tar(archive, f, &dest, overwrite, cb)?,
        f if is_stream(f) => tarfmt::extract_stream(archive, f, &dest, overwrite, cb)?,
        f => bail!("Unsupported format {f}"),
    }
    Ok(dest)
}

pub fn list_archive(archive: &Path, password: Option<&str>) -> Result<Vec<Entry>> {
    let fmt = detect_format(archive)?;
    match fmt.as_str() {
        "zip" => zipfmt::list_zip(archive),
        "7z" => sevenz::list_7z(archive, password),
        "rar" => rarfmt::list_rar(archive, password),
        "zfx" => ultra::list_zfx(archive),
        f if is_tar_family(f) => tarfmt::list_tar(archive, f),
        f if is_stream(f) => tarfmt::list_stream(archive, f),
        f => bail!("Unsupported format {f}"),
    }
}

/// Decompress everything and verify checksums. Ok((passed, message)).
pub fn test_archive(archive: &Path, password: Option<&str>, cb: Option<&ProgressFn>) -> Result<(bool, String)> {
    let fmt = detect_format(archive)?;
    match fmt.as_str() {
        "zip" => zipfmt::test_zip(archive, password, cb),
        "7z" => sevenz::test_7z(archive, password, cb),
        "rar" => rarfmt::test_rar(archive, password),
        "zfx" => match ultra::extract_zfx(archive, None, true, cb) {
            Ok(st) if !st.errors.is_empty() => Ok((false, st.errors.iter().take(5).cloned().collect::<Vec<_>>().join("; "))),
            Ok(st) => {
                let n = st.equivalent + st.rebuilt;
                let note = if n > 0 { format!(", {n} document(s) rebuilt with identical content") } else { String::new() };
                Ok((true, format!("OK ({} files verified with SHA-256{note})", st.files)))
            }
            Err(e) => Ok((false, e.0)),
        },
        f if is_tar_family(f) => tarfmt::test_tar(archive, f, cb),
        f if is_stream(f) => tarfmt::test_stream(archive, f),
        f => bail!("Unsupported format {f}"),
    }
}

/// Try every strong format, keep the smallest. Returns (path, [(format, size)]).
pub fn create_best(base: &Path, sources: &[PathBuf], o: &CreateOptions,
                   cb: Option<&ProgressFn>) -> Result<(PathBuf, Vec<(String, u64)>)> {
    let name = base.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let base = base.with_file_name(strip_archive_ext(&name));
    let cands: Vec<&str> = if o.password.is_some() { vec!["7z", "zip"] } else { vec!["zfx", "7z", "tar.xz", "tar.bz2"] };
    let dir = base.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let td = tempfile::Builder::new().prefix("zfbest").tempdir_in(&dir)?;
    let mut results = Vec::new();
    for f in &cands {
        let p = td.path().join(format!("cand.{f}"));
        let mut oo = o.clone();
        oo.fmt = Some(f.to_string());
        if *f != "zfx" {
            oo.engine = "lzma".into();
        }
        create_archive(&p, sources, &oo, cb)?;
        results.push((f.to_string(), fs::metadata(&p)?.len()));
    }
    results.sort_by_key(|r| r.1);
    let best = &results[0].0;
    let fin = base.with_file_name(format!("{}.{best}", base.file_name().unwrap().to_string_lossy()));
    fs::rename(td.path().join(format!("cand.{best}")), &fin)?;
    Ok((fin, results))
}
