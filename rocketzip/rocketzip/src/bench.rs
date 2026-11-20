//! Head-to-head benchmark: RocketZip vs 7-Zip and WinRAR on your own files.
use crate::util::*;
use crate::{create_archive, human, test_archive, CreateOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

const SZ_PLACES: &[&str] = &[r"C:\Program Files\7-Zip\7z.exe", r"C:\Program Files (x86)\7-Zip\7z.exe",
                             "/opt/homebrew/bin/7zz", "/usr/local/bin/7zz", "/usr/bin/7zz"];
const RAR_PLACES: &[&str] = &[r"C:\Program Files\WinRAR\Rar.exe", r"C:\Program Files (x86)\WinRAR\Rar.exe",
                              "/usr/local/bin/rar", "/opt/homebrew/bin/rar"];

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for cand in [name.to_string(), format!("{name}.exe")] {
            let p = dir.join(&cand);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn find_tool(names: &[&str], places: &[&str]) -> Option<PathBuf> {
    names.iter().find_map(|n| which(n))
        .or_else(|| places.iter().map(PathBuf::from).find(|p| p.is_file()))
}

pub struct Row {
    pub name: String,
    pub size: Option<u64>,
    pub secs: f64,
    pub ours: bool,
    pub error: String,
}

type Rival = (String, Box<dyn Fn(&Path, &Path) -> Command>, &'static str);

fn rivals() -> Vec<Rival> {
    let mut v: Vec<Rival> = Vec::new();
    if let Some(sz) = find_tool(&["7z", "7zz", "7za"], SZ_PLACES) {
        let s1 = sz.clone();
        v.push(("7-Zip Ultra (LZMA2)".into(), Box::new(move |a, s| {
            let mut c = Command::new(&s1);
            c.args(["a", "-t7z", "-mx=9", "-m0=lzma2", "-mfb=273", "-ms=on", "-mqs=on", "-bd", "-y"]).arg(a).arg(s);
            c
        }), ".7z"));
        v.push(("7-Zip Ultra (PPMd)".into(), Box::new(move |a, s| {
            let mut c = Command::new(&sz);
            c.args(["a", "-t7z", "-mx=9", "-m0=PPMd", "-ms=on", "-mqs=on", "-bd", "-y"]).arg(a).arg(s);
            c
        }), ".7z"));
    }
    if let Some(rar) = find_tool(&["rar", "Rar"], RAR_PLACES) {
        v.push(("WinRAR Best (-m5 solid)".into(), Box::new(move |a, s| {
            let mut c = Command::new(&rar);
            c.args(["a", "-m5", "-ma5", "-md128m", "-s", "-r", "-ep1", "-idq", "-y"]).arg(a).arg(s);
            c
        }), ".rar"));
    }
    v
}

pub fn run(src: &Path, quick: bool, out: &mut dyn FnMut(&str)) -> Result<Vec<Row>> {
    if !src.exists() {
        return err(format!("Not found: {}", src.display()));
    }
    let total: u64 = collect_sources(&[src.to_path_buf()], &[])?.iter().map(|e| e.size).sum();
    let mut rows = Vec::new();
    let rv = rivals();
    if rv.is_empty() {
        out("note: 7-Zip and WinRAR not found - install them (or add them to PATH) for a head-to-head comparison.");
    }
    let td = tempfile::Builder::new().prefix("zfbench-").tempdir()?;
    for (label, cmd, ext) in rv {
        let arc = td.path().join(format!("rival{}{ext}", rows.len()));
        out(&format!("  running {label} ..."));
        let t = Instant::now();
        let r = cmd(&arc, src).output();
        let (size, error) = match r {
            Ok(o) if o.status.success() && arc.exists() => (std::fs::metadata(&arc).ok().map(|m| m.len()), String::new()),
            Ok(o) => (None, String::from_utf8_lossy(&o.stderr).chars().take(200).collect()),
            Err(e) => (None, e.to_string()),
        };
        rows.push(Row { name: label, size, secs: t.elapsed().as_secs_f64(), ours: false, error });
    }
    let mut modes: Vec<(&str, CreateOptions)> = vec![
        ("RocketZip Ultra", CreateOptions::default()),
        ("RocketZip Ultra + docs full", CreateOptions { docs: "full".into(), ..Default::default() }),
    ];
    if !quick {
        modes.push(("RocketZip Ultra Max", CreateOptions { docs: "full".into(), engine: "max".into(), ..Default::default() }));
    }
    for (label, mut o) in modes {
        let arc = td.path().join(format!("zf{}.zfx", rows.len()));
        o.fmt = Some("zfx".into());
        out(&format!("  running {label} ..."));
        let t = Instant::now();
        let r = create_archive(&arc, &[src.to_path_buf()], &o, None)
            .and_then(|_| test_archive(&arc, None, None));
        let secs = t.elapsed().as_secs_f64();
        rows.push(match r {
            Ok((ok, msg)) => Row { name: label.into(), size: std::fs::metadata(&arc).ok().map(|m| m.len()),
                                   secs, ours: true, error: if ok { String::new() } else { format!("verify failed: {msg}") } },
            Err(e) => Row { name: label.into(), size: None, secs, ours: true, error: e.0 },
        });
    }
    let best_rival = rows.iter().filter(|r| !r.ours).filter_map(|r| r.size).min();
    out("");
    out(&format!("Input: {}  ({})", src.display(), human(Some(total))));
    out(&format!("{:<30} {:>10} {:>7} {:>14} {:>8}", "Tool", "Size", "Ratio", "vs best rival", "Time"));
    out(&"-".repeat(73));
    let mut sorted: Vec<&Row> = rows.iter().collect();
    sorted.sort_by_key(|r| r.size.unwrap_or(u64::MAX));
    for r in sorted {
        let Some(size) = r.size else {
            out(&format!("{:<30} {:>10}   {}", r.name, "failed", r.error.chars().take(40).collect::<String>()));
            continue;
        };
        let ratio = if total > 0 { format!("{:.1}%", size as f64 * 100.0 / total as f64) } else { "-".into() };
        let vs = match best_rival {
            Some(b) if r.ours => format!("{:+.1}%", (size as f64 - b as f64) * 100.0 / b as f64),
            _ => String::new(),
        };
        out(&format!("{:<30} {:>10} {ratio:>7} {vs:>14} {:>7.1}s{}", r.name, human(Some(size)), r.secs,
                     if r.error.is_empty() { "" } else { "   (verify FAILED)" }));
    }
    let ours = rows.iter().filter(|r| r.ours && r.error.is_empty()).filter_map(|r| r.size).min();
    if let (Some(b), Some(o)) = (best_rival, ours) {
        let diff = (b as f64 - o as f64) * 100.0 / b as f64;
        out("");
        out(&format!("RocketZip's best is {:.1}% {} than the best of 7-Zip/WinRAR.", diff.abs(),
                     if diff >= 0.0 { "smaller" } else { "LARGER" }));
    }
    Ok(rows)
}
