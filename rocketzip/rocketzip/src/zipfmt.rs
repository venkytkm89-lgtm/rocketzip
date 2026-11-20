//! ZIP read/write (Deflate, BZip2, Zstd, XZ, Store; AES-256 passwords).
use crate::formats::ALREADY_COMPRESSED;
use crate::util::*;
use crate::{bail, Entry};
use chrono::{Datelike, Local, TimeZone, Timelike};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

fn to_zip_time(secs: f64) -> Option<zip::DateTime> {
    let t = Local.timestamp_opt(secs as i64, 0).single()?;
    zip::DateTime::from_date_and_time(
        t.year().clamp(1980, 2107) as u16, t.month() as u8, t.day() as u8,
        t.hour() as u8, t.minute() as u8, t.second() as u8,
    ).ok()
}

pub fn from_zip_time(d: Option<zip::DateTime>) -> Option<f64> {
    let d = d?;
    Local.with_ymd_and_hms(d.year() as i32, d.month() as u32, d.day() as u32,
                           d.hour() as u32, d.minute() as u32, d.second() as u32)
        .single()
        .map(|t| t.timestamp() as f64)
}

pub fn method_from_name(name: &str) -> Result<CompressionMethod> {
    Ok(match name.to_lowercase().as_str() {
        "store" => CompressionMethod::Stored,
        "deflate" => CompressionMethod::Deflated,
        "bzip2" => CompressionMethod::Bzip2,
        "zstd" => CompressionMethod::Zstd,
        "xz" => CompressionMethod::Xz,
        "lzma" => bail!("ZIP method 'lzma' is read-only in this version; use 'xz' (same \
                         LZMA2 strength, opens in 7-Zip) or the .7z/.zfx formats"),
        other => bail!("ZIP method must be store, deflate, bzip2, zstd or xz (got '{other}')"),
    })
}

pub fn write_zip(tmp: &Path, entries: &[Source], level: u32, method: Option<&str>,
                 password: Option<&str>, store_compressed: bool, prog: &Progress) -> Result<()> {
    let mut m = method_from_name(method.unwrap_or("deflate"))?;
    if level == 0 {
        m = CompressionMethod::Stored;
    }
    let clevel: Option<i64> = match m {
        CompressionMethod::Stored => None,
        CompressionMethod::Zstd => Some(if level >= 9 { 19 } else { (level as i64 * 2).max(1) }),
        _ => Some(level.max(1) as i64),
    };
    let mut zw = ZipWriter::new(BufWriter::new(File::create(tmp)?));
    for e in entries {
        let meta = fs::metadata(&e.path)?;
        let mut opts = SimpleFileOptions::default()
            .unix_permissions(file_mode(&meta) & 0o777)
            .large_file(e.size >= 0xFFFF_0000);
        if let Some(t) = to_zip_time(mtime_secs(&meta)) {
            opts = opts.last_modified_time(t);
        }
        if let Some(pw) = password {
            opts = opts.with_aes_encryption(zip::AesMode::Aes256, pw);
        }
        if e.is_dir {
            zw.add_directory(format!("{}/", e.arcname), opts)?;
            continue;
        }
        let stored = store_compressed && ALREADY_COMPRESSED.contains(&ext_lower(&e.arcname).as_str());
        let opts = if stored {
            opts.compression_method(CompressionMethod::Stored)
        } else {
            opts.compression_method(m).compression_level(clevel)
        };
        zw.start_file(e.arcname.as_str(), opts)?;
        let mut src = BufReader::new(File::open(&e.path)?);
        copy_progress(&mut src, &mut zw, prog, &e.arcname)?;
    }
    zw.finish()?.flush()?;
    Ok(())
}

fn open_entry<'a>(za: &'a mut ZipArchive<BufReader<File>>, i: usize, password: Option<&str>)
                  -> Result<zip::read::ZipFile<'a, BufReader<File>>> {
    let r = match password {
        Some(pw) => za.by_index_decrypt(i, pw.as_bytes()),
        None => za.by_index(i),
    };
    r.map_err(|e| match e {
        zip::result::ZipError::UnsupportedArchive(m) if m.to_lowercase().contains("password") =>
            Error("Wrong or missing password".into()),
        zip::result::ZipError::InvalidPassword => Error("Wrong or missing password".into()),
        other => Error(other.to_string()),
    })
}

pub fn extract_zip(archive: &Path, dest: &Path, password: Option<&str>, overwrite: bool,
                   cb: Option<&ProgressFn>) -> Result<()> {
    let mut za = ZipArchive::new(BufReader::new(File::open(archive)?))?;
    let mut total = 0u64;
    for i in 0..za.len() {
        total += za.by_index_raw(i)?.size();
    }
    check_space(dest, total)?;
    let prog = Progress::new(total, cb);
    let mut guard = DiskGuard::new(dest);
    for i in 0..za.len() {
        let mut f = open_entry(&mut za, i, password)?;
        let name = f.name()?.into_owned();
        let Some(target) = safe_target(dest, &name) else {
            eprintln!("warning: blocked unsafe path: {name}");
            continue;
        };
        if f.is_dir() {
            create_dir_safe(dest, &target)?;
            continue;
        }
        if target.exists() && !overwrite {
            prog.advance(f.size(), &name);
            continue;
        }
        // Symlink entries become plain files holding the link text (never followed).
        let mut out = BufWriter::new(create_file_safe(dest, &target)?);
        copy_extract(&mut f, &mut out, &prog, &name, &mut guard)
            .map_err(|e| Error(format!("{name}: {e}")))?;
        out.flush()?;
        drop(out);
        if let Some(t) = from_zip_time(f.last_modified()) {
            set_mtime(&target, t);
        }
        if let Some(mode) = f.unix_mode()
            && !f.is_symlink() {
                set_mode(&target, mode);
            }
    }
    Ok(())
}

pub fn list_zip(archive: &Path) -> Result<Vec<Entry>> {
    let mut za = ZipArchive::new(BufReader::new(File::open(archive)?))?;
    let mut out = Vec::new();
    for i in 0..za.len() {
        let f = za.by_index_raw(i)?;
        out.push(Entry {
            name: f.name()?.into_owned(),
            size: Some(f.size()),
            packed: Some(f.compressed_size()),
            mtime: from_zip_time(f.last_modified()),
            is_dir: f.is_dir(),
        });
    }
    Ok(out)
}

/// Decompress every member; the reader verifies each CRC-32.
pub fn test_zip(archive: &Path, password: Option<&str>, cb: Option<&ProgressFn>) -> Result<(bool, String)> {
    let mut za = ZipArchive::new(BufReader::new(File::open(archive)?))?;
    let prog = Progress::new(0, cb);
    let mut sink = std::io::sink();
    for i in 0..za.len() {
        let mut f = open_entry(&mut za, i, password)?;
        let name = f.name()?.into_owned();
        if f.is_dir() {
            continue;
        }
        let mut buf = vec![0u8; CHUNK];
        loop {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    sink.write_all(&buf[..n])?;
                    prog.advance(n as u64, &name);
                }
                Err(e) => return Ok((false, format!("CRC/data error in {name}: {e}"))),
            }
        }
    }
    Ok((true, "OK".into()))
}
