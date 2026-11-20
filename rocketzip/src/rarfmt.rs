//! RAR extraction (read-only; uses the official UnRAR source).
use crate::util::*;
use crate::{bail, Entry};
use std::fs;
use std::path::Path;

fn dos_time(t: u32) -> Option<f64> {
    use chrono::{Local, TimeZone};
    let (date, time) = (t >> 16, t & 0xFFFF);
    Local.with_ymd_and_hms(((date >> 9) + 1980) as i32, (date >> 5) & 15, date & 31,
                           time >> 11, (time >> 5) & 63, (time & 31) * 2)
        .single()
        .map(|d| d.timestamp() as f64)
}

fn rar_err(e: unrar::error::UnrarError) -> Error {
    let s = e.to_string();
    if s.to_lowercase().contains("password") {
        Error("Wrong or missing password".into())
    } else {
        Error(format!("RAR: {s}"))
    }
}

pub fn list_rar(archive: &Path, password: Option<&str>) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    let a = match password {
        Some(p) => unrar::Archive::with_password(archive, p),
        None => unrar::Archive::new(archive),
    };
    for h in a.open_for_listing().map_err(rar_err)? {
        let h = h.map_err(rar_err)?;
        out.push(Entry {
            name: h.filename.to_string_lossy().replace('\\', "/"),
            size: Some(h.unpacked_size),
            packed: None,
            mtime: dos_time(h.file_time),
            is_dir: h.is_directory(),
        });
    }
    Ok(out)
}

pub fn extract_rar(archive: &Path, dest: &Path, password: Option<&str>, overwrite: bool,
                   cb: Option<&ProgressFn>) -> Result<()> {
    let entries = list_rar(archive, password)?;
    let total: u64 = entries.iter().filter_map(|e| e.size).sum();
    check_space(dest, total)?;
    let prog = Progress::new(total, cb);
    let a = match password {
        Some(p) => unrar::Archive::with_password(archive, p),
        None => unrar::Archive::new(archive),
    };
    let mut cur = a.open_for_processing().map_err(rar_err)?;
    while let Some(h) = cur.read_header().map_err(rar_err)? {
        let e = h.entry();
        let name = e.filename.to_string_lossy().replace('\\', "/");
        let size = e.unpacked_size;
        let target = safe_target(dest, &name);
        cur = match target {
            None => {
                eprintln!("warning: blocked unsafe path: {name}");
                h.skip().map_err(rar_err)?
            }
            Some(t) if e.is_directory() => {
                create_dir_safe(dest, &t)?;
                h.skip().map_err(rar_err)?
            }
            Some(t) if !overwrite && t.exists() => h.skip().map_err(rar_err)?,
            Some(t) => {
                drop(create_file_safe(dest, &t)?); // checks the path and removes planted links
                let next = h.extract_to(&t).map_err(rar_err)?;
                // RAR can hold symbolic links; never leave one behind.
                if fs::symlink_metadata(&t).is_ok_and(|m| m.file_type().is_symlink()) {
                    fs::remove_file(&t)?;
                    eprintln!("warning: symbolic link not extracted: {name}");
                }
                prog.advance(size, &name);
                next
            }
        };
    }
    Ok(())
}

pub fn test_rar(archive: &Path, password: Option<&str>) -> Result<(bool, String)> {
    let a = match password {
        Some(p) => unrar::Archive::with_password(archive, p),
        None => unrar::Archive::new(archive),
    };
    let run = || -> Result<usize> {
        let mut n = 0;
        let mut cur = a.open_for_processing().map_err(rar_err)?;
        while let Some(h) = cur.read_header().map_err(rar_err)? {
            cur = if h.entry().is_file() { h.test().map_err(rar_err)? } else { h.skip().map_err(rar_err)? };
            n += 1;
        }
        Ok(n)
    };
    match run() {
        Ok(n) => Ok((true, format!("OK ({n} entries)"))),
        Err(e) if e.0.contains("password") => bail!("{}", e.0),
        Err(e) => Ok((false, e.0)),
    }
}
