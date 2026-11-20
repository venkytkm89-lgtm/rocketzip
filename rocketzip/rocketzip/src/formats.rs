//! Archive format names, detection by content (magic bytes) and by name.
use crate::util::{err, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Longest extensions first so ".tar.gz" wins over ".gz".
pub const EXT_MAP: &[(&str, &str)] = &[
    (".tar.gz", "tar.gz"), (".tgz", "tar.gz"),
    (".tar.bz2", "tar.bz2"), (".tbz2", "tar.bz2"), (".tbz", "tar.bz2"),
    (".tar.xz", "tar.xz"), (".txz", "tar.xz"),
    (".tar.zst", "tar.zst"), (".tzst", "tar.zst"),
    (".tar", "tar"), (".zip", "zip"), (".zfx", "zfx"), (".7z", "7z"), (".rar", "rar"),
    (".gz", "gz"), (".bz2", "bz2"), (".xz", "xz"), (".zst", "zst"),
    // Office / ebook / Java containers are ZIP files underneath.
    (".docx", "zip"), (".xlsx", "zip"), (".pptx", "zip"), (".odt", "zip"),
    (".ods", "zip"), (".odp", "zip"), (".epub", "zip"), (".jar", "zip"),
    (".apk", "zip"), (".cbz", "zip"),
];

pub const TAR_FORMATS: &[&str] = &["tar", "tar.gz", "tar.bz2", "tar.xz", "tar.zst"];
pub const STREAM_FORMATS: &[&str] = &["gz", "bz2", "xz", "zst"];
pub const SOLID_FORMATS: &[&str] = &["tar", "tar.gz", "tar.bz2", "tar.xz", "tar.zst", "7z", "zfx"];

pub const ALREADY_COMPRESSED: &[&str] = &[
    ".jpg", ".jpeg", ".png", ".gif", ".webp", ".heic", ".avif",
    ".mp3", ".aac", ".ogg", ".opus", ".flac", ".m4a",
    ".mp4", ".mkv", ".avi", ".mov", ".webm",
    ".zip", ".7z", ".rar", ".gz", ".bz2", ".xz", ".zst", ".tgz",
    ".docx", ".xlsx", ".pptx", ".odt", ".ods", ".odp", ".epub", ".jar", ".apk",
];

pub fn writable_formats() -> Vec<&'static str> {
    vec!["zfx", "zip", "7z", "tar", "tar.gz", "tar.bz2", "tar.xz", "tar.zst", "gz", "bz2", "xz", "zst"]
}

pub fn readable_formats() -> Vec<&'static str> {
    let mut v = writable_formats();
    v.push("rar");
    v
}

pub fn alias(fmt: &str) -> String {
    let f = fmt.trim().trim_start_matches('.').to_lowercase();
    match f.as_str() {
        "ultra" => "zfx".into(),
        "tgz" => "tar.gz".into(),
        "txz" => "tar.xz".into(),
        "tbz2" | "tbz" => "tar.bz2".into(),
        "tzst" => "tar.zst".into(),
        "7zip" => "7z".into(),
        _ => f,
    }
}

pub fn is_tar_family(f: &str) -> bool {
    TAR_FORMATS.contains(&f)
}
pub fn is_stream(f: &str) -> bool {
    STREAM_FORMATS.contains(&f)
}

pub fn format_from_name(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    EXT_MAP.iter().find(|(ext, _)| name.ends_with(ext)).map(|(_, f)| *f)
}

pub fn strip_archive_ext(name: &str) -> String {
    let low = name.to_lowercase();
    for (ext, _) in EXT_MAP {
        if low.ends_with(ext) && name.len() > ext.len() {
            return name[..name.len() - ext.len()].to_string();
        }
    }
    name.to_string()
}

/// Is this 512-byte block a valid tar header (checksum matches)?
pub fn is_tar_header(h: &[u8]) -> bool {
    if h.len() < 512 || h[..512].iter().all(|&b| b == 0) {
        return false;
    }
    let field = &h[148..156];
    let s: String = field.iter().take_while(|&&b| b != 0 && b != b' ').map(|&b| b as char).collect();
    let Ok(want) = u32::from_str_radix(s.trim(), 8) else { return false };
    let sum: u32 = h[..512].iter().enumerate()
        .map(|(i, &b)| if (148..156).contains(&i) { b' ' as u32 } else { b as u32 })
        .sum();
    sum == want
}

pub fn open_stream(path: &Path, comp: &str) -> Result<Box<dyn Read + Send>> {
    let f = std::io::BufReader::new(File::open(path)?);
    Ok(match comp {
        "gz" => Box::new(flate2::read::MultiGzDecoder::new(f)),
        "bz2" => Box::new(bzip2::read::MultiBzDecoder::new(f)),
        "xz" => Box::new(liblzma::read::XzDecoder::new_multi_decoder(f)),
        "zst" => Box::new(zstd::stream::read::Decoder::with_buffer(f)?),
        _ => return err(format!("unknown stream type {comp}")),
    })
}

fn is_tar_stream(path: &Path, comp: &str) -> bool {
    let Ok(mut r) = open_stream(path, comp) else { return false };
    let mut h = [0u8; 512];
    let mut got = 0;
    while got < 512 {
        match r.read(&mut h[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    got == 512 && is_tar_header(&h)
}

/// Detect by content first, then by file name.
pub fn detect_format(path: &Path) -> Result<String> {
    if !path.is_file() {
        return err(format!("Not a file: {}", path.display()));
    }
    let mut head = vec![0u8; 512];
    let n = File::open(path)?.read(&mut head)?;
    head.truncate(n);
    let starts = |m: &[u8]| head.starts_with(m);
    if starts(b"ZFX\x01") {
        return Ok("zfx".into());
    }
    if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") || starts(b"PK\x07\x08") {
        return Ok("zip".into());
    }
    if starts(b"7z\xbc\xaf\x27\x1c") {
        return Ok("7z".into());
    }
    if starts(b"Rar!\x1a\x07") {
        return Ok("rar".into());
    }
    let comp = if starts(b"\x1f\x8b") {
        Some("gz")
    } else if starts(b"BZh") {
        Some("bz2")
    } else if starts(b"\xfd7zXZ\x00") {
        Some("xz")
    } else if starts(b"\x28\xb5\x2f\xfd") {
        Some("zst")
    } else {
        None
    };
    if let Some(c) = comp {
        return Ok(if is_tar_stream(path, c) { format!("tar.{c}") } else { c.to_string() });
    }
    if head.len() >= 262 && &head[257..262] == b"ustar" || is_tar_header(&head) {
        return Ok("tar".into());
    }
    if let Ok(f) = File::open(path)
        && zip::ZipArchive::new(f).is_ok() {
            return Ok("zip".into()); // e.g. self-extracting exe with a ZIP payload
        }
    if let Some(f) = format_from_name(path) {
        return Ok(f.to_string());
    }
    err(format!("Unrecognised archive format: {}", path.file_name().unwrap_or_default().to_string_lossy()))
}
