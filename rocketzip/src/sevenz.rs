//! 7z read/write (LZMA2 solid, AES-256 with encrypted headers).
use crate::util::*;
use crate::{bail, Entry};
use sevenz_rust2::encoder_options::{AesEncoderOptions, Lzma2Options};
use sevenz_rust2::{ArchiveEntry, ArchiveReader, ArchiveWriter, EncoderConfiguration, EncoderMethod,
                   Password, SourceReader};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn pw(password: Option<&str>) -> Password {
    password.map(Password::from).unwrap_or_else(Password::empty)
}

fn nt_secs(e: &ArchiveEntry) -> Option<f64> {
    if !e.has_last_modified_date {
        return None;
    }
    let t: SystemTime = e.last_modified_date.into();
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs_f64())
}

fn map_err(e: sevenz_rust2::Error) -> Error {
    let s = e.to_string();
    if s.to_lowercase().contains("password") {
        Error("Wrong or missing password".into())
    } else {
        Error(s)
    }
}

pub fn write_7z(tmp: &Path, entries: &[Source], level: u32, dict_mb: Option<u32>,
                password: Option<&str>, prog: &Progress) -> Result<()> {
    let mut w = ArchiveWriter::create(tmp).map_err(map_err)?;
    let mut methods: Vec<EncoderConfiguration> = Vec::new();
    if level == 0 {
        methods.push(EncoderMethod::COPY.into());
    } else {
        let mut o = Lzma2Options::from_level(level);
        if level >= 9 {
            let total: u64 = entries.iter().map(|e| e.size).sum();
            let cap = dict_mb.unwrap_or(64).min(1024) << 20;
            o.set_dictionary_size(crate::tarfmt::pow2(total, 1 << 18, cap));
        }
        methods.push(o.into());
    }
    if let Some(p) = password {
        methods.push(AesEncoderOptions::new(Password::from(p)).into());
    }
    w.set_content_methods(methods);
    if password.is_some() {
        w.set_encrypt_header(true); // hide file names too
    }
    for e in entries.iter().filter(|e| e.is_dir) {
        w.push_archive_entry::<File>(ArchiveEntry::from_path(&e.path, e.arcname.clone()), None)
            .map_err(map_err)?;
    }
    let files: Vec<&Source> = entries.iter().filter(|e| !e.is_dir).collect();
    if !files.is_empty() {
        let ents: Vec<ArchiveEntry> = files.iter()
            .map(|e| ArchiveEntry::from_path(&e.path, e.arcname.clone()))
            .collect();
        let mut readers = Vec::new();
        for e in &files {
            readers.push(SourceReader::new(ProgressReader {
                inner: BufReader::new(File::open(&e.path)?), prog, name: e.arcname.clone(),
            }));
        }
        w.push_archive_entries(ents, readers).map_err(map_err)?; // one solid block
    }
    w.finish()?;
    Ok(())
}

pub fn extract_7z(archive: &Path, dest: &Path, password: Option<&str>, overwrite: bool,
                  cb: Option<&ProgressFn>) -> Result<()> {
    let mut r = ArchiveReader::open(archive, pw(password)).map_err(map_err)?;
    let files = &r.archive().files;
    if let Some(bad) = files.iter().find(|f| safe_target(dest, f.name()).is_none()) {
        bail!("Archive contains unsafe paths, refusing: {}", bad.name());
    }
    let total: u64 = files.iter().map(|f| f.size()).sum();
    check_space(dest, total)?;
    let prog = Progress::new(total, cb);
    let mut guard = DiskGuard::new(dest);
    let mut failure: Option<Error> = None;
    r.for_each_entries(|ent, rd| {
        let target = safe_target(dest, ent.name()).expect("checked above");
        let res = (|| -> Result<()> {
            if ent.is_directory() {
                create_dir_safe(dest, &target)?;
                return Ok(());
            }
            if !overwrite && target.exists() {
                io::copy(rd, &mut io::sink())?;
                return Ok(());
            }
            let mut out = BufWriter::new(create_file_safe(dest, &target)?);
            copy_extract(rd, &mut out, &prog, ent.name(), &mut guard)?;
            out.flush()?;
            drop(out);
            if let Some(t) = nt_secs(ent) {
                set_mtime(&target, t);
            }
            Ok(())
        })();
        match res {
            Ok(()) => Ok(true),
            Err(e) => {
                failure = Some(e);
                Ok(false)
            }
        }
    }).map_err(map_err)?;
    failure.map_or(Ok(()), Err)
}

pub fn list_7z(archive: &Path, password: Option<&str>) -> Result<Vec<Entry>> {
    let r = ArchiveReader::open(archive, pw(password)).map_err(map_err)?;
    Ok(r.archive().files.iter().map(|f| Entry {
        name: f.name().to_string() + if f.is_directory() { "/" } else { "" },
        size: Some(f.size()),
        packed: if f.compressed_size > 0 { Some(f.compressed_size) } else { None },
        mtime: nt_secs(f),
        is_dir: f.is_directory(),
    }).collect())
}

pub fn test_7z(archive: &Path, password: Option<&str>, cb: Option<&ProgressFn>) -> Result<(bool, String)> {
    let prog = Progress::new(0, cb);
    let res = (|| -> Result<usize> {
        let mut r = ArchiveReader::open(archive, pw(password)).map_err(map_err)?;
        let mut n = 0;
        r.for_each_entries(|ent, rd| {
            let mut buf = vec![0u8; CHUNK];
            loop {
                let k = rd.read(&mut buf)?;
                if k == 0 {
                    break;
                }
                prog.advance(k as u64, ent.name());
            }
            n += 1;
            Ok(true)
        }).map_err(map_err)?;
        Ok(n)
    })();
    Ok(match res {
        Ok(n) => (true, format!("OK ({n} entries)")),
        Err(e) => (false, e.0),
    })
}
