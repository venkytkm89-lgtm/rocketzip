//! rocketzip-cli: 7-Zip-style command line.
//!
//!   rocketzip-cli a backup.zfx Documents/ -mx9 -v       create (Ultra)
//!   rocketzip-cli a backup.zfx Documents/ --max --docs full
//!   rocketzip-cli a backup.7z  Documents/ -p            7z with password (prompt)
//!   rocketzip-cli x backup.zfx -oRestored               extract
//!   rocketzip-cli l backup.zip                          list
//!   rocketzip-cli t backup.zfx                          test (SHA-256 per file)
//!   rocketzip-cli bench Documents/                      compare with 7-Zip / WinRAR
use clap::{Parser, Subcommand};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;
use rocketzip::ultra::UltraStats;
use rocketzip::*;

#[derive(Parser)]
#[command(name = "rocketzip", version, about = "7-Zip-style archiver with Ultra compression")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Add files to a new archive
    A {
        archive: PathBuf,
        #[arg(required = true)]
        sources: Vec<PathBuf>,
        /// Format (default: from extension): zfx, zip, 7z, tar.xz, ...
        #[arg(short = 't', long = "type")]
        fmt: Option<String>,
        /// Compression level 0-9, 9 = maximum (default). -mx9 works too
        #[arg(long, default_value_t = 9)]
        level: u32,
        /// ZIP method: deflate|bzip2|zstd|xz|store (or -mm=xz)
        #[arg(long)]
        method: Option<String>,
        /// Password (prompt if given without a value)
        #[arg(short = 'p', long, num_args = 0..=1, default_missing_value = "")]
        password: Option<String>,
        /// Try all strong formats and keep the smallest
        #[arg(long)]
        best: bool,
        /// ZIP: also compress already-compressed files (jpg, mp4, docx...)
        #[arg(long)]
        recompress: bool,
        /// LZMA dictionary cap for zfx/tar.xz/7z, e.g. 256m or 1g (default 64m)
        #[arg(long, value_parser = parse_mb)]
        dict: Option<u32>,
        /// Ultra document recompression: exact (default) | full | off
        #[arg(long, default_value = "exact")]
        docs: String,
        /// Ultra: don't merge identical files
        #[arg(long)]
        no_dedup: bool,
        /// Ultra Max: add the context-mixing engine (smallest output, slow)
        #[arg(long)]
        max: bool,
        /// Show Ultra statistics
        #[arg(short, long)]
        verbose: bool,
    },
    /// Extract with full paths
    X {
        archive: PathBuf,
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
        #[arg(short = 'p', long, num_args = 0..=1, default_missing_value = "")]
        password: Option<String>,
        #[arg(long)]
        skip_existing: bool,
    },
    /// List contents
    L {
        archive: PathBuf,
        #[arg(short = 'p', long, num_args = 0..=1, default_missing_value = "")]
        password: Option<String>,
    },
    /// Test integrity
    T {
        archive: PathBuf,
        #[arg(short = 'p', long, num_args = 0..=1, default_missing_value = "")]
        password: Option<String>,
    },
    /// Show supported formats
    Formats,
    /// Compare RocketZip with 7-Zip and WinRAR on your files
    Bench {
        source: PathBuf,
        /// Skip the slow Max mode
        #[arg(long)]
        quick: bool,
    },
}

fn parse_mb(s: &str) -> std::result::Result<u32, String> {
    let t = s.trim().to_lowercase();
    let t = t.trim_end_matches('b');
    let (num, mult) = if let Some(n) = t.strip_suffix('g') { (n, 1024.0) } else { (t.trim_end_matches('m'), 1.0) };
    num.parse::<f64>().map(|v| (v * mult) as u32).map_err(|_| format!("bad size: {s} (use e.g. 128m or 1g)"))
}

/// Accept 7-Zip switch spellings: -mx9 / -mx=9, -mm=xz, -oDIR, -pSECRET, -tzip
fn normalize(args: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for (i, a) in args.into_iter().enumerate() {
        let low = a.to_lowercase();
        if i > 0 && low.starts_with("-mx") && a[3..].trim_start_matches('=').chars().all(|c| c.is_ascii_digit())
            && a.len() > 3 {
            out.push("--level".into());
            out.push(a[3..].trim_start_matches('=').to_string());
        } else if low.starts_with("-mm=") {
            out.push("--method".into());
            out.push(a[4..].to_string());
        } else if i > 0 && a.len() > 2 && ["-o", "-p", "-t"].contains(&&a[..2]) && !a.starts_with("--") {
            out.push(a[..2].to_string());
            out.push(a[2..].to_string());
        } else {
            out.push(a);
        }
    }
    out
}

fn ask_password(p: Option<String>) -> Option<String> {
    match p {
        Some(s) if s.is_empty() => {
            eprint!("Password: ");
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).ok()?;
            Some(line.trim_end_matches(['\r', '\n']).to_string())
        }
        other => other,
    }
}

fn progress_bar() -> Option<Box<ProgressFn>> {
    if !std::io::stderr().is_terminal() {
        return None;
    }
    let last = Mutex::new(Instant::now());
    Some(Box::new(move |name: &str, done: u64, total: u64| {
        let mut l = last.lock().unwrap();
        if l.elapsed().as_millis() < 100 && done < total {
            return;
        }
        *l = Instant::now();
        let pct = if total > 0 { format!("{:3}%", done * 100 / total) } else { "    ".into() };
        let n: String = name.chars().rev().take(50).collect::<Vec<_>>().into_iter().rev().collect();
        eprint!("\r{pct} {:>10}  {n:<50}", human(Some(done)));
    }))
}

fn end_bar() {
    if std::io::stderr().is_terminal() {
        eprint!("\r{:70}\r", "");
    }
}

pub fn print_ultra_stats(st: &UltraStats) {
    println!("  files {}, folders {}, input {}", st.files, st.dirs, human(Some(st.input)));
    if st.duplicates > 0 {
        println!("  deduplicated {} identical file(s), saved {}", st.duplicates, human(Some(st.dup_bytes)));
    }
    if st.docs > 0 {
        println!("  recompressed {} document(s) ({} byte-exact, {} equivalent)", st.docs, st.docs_exact,
                 st.docs - st.docs_exact);
    }
    println!("  {:<12} {:<16} {:>6} {:>10} {:>10}  ratio", "group", "filter", "files", "in", "out");
    for g in &st.groups {
        let r = if g.input > 0 { format!("{:.1}%", g.output as f64 * 100.0 / g.input as f64) } else { "-".into() };
        println!("  {:<12} {:<16} {:>6} {:>10} {:>10}  {r}", g.kind, g.filter, g.files,
                 human(Some(g.input)), human(Some(g.output)));
    }
}

fn run(cli: Cli) -> Result<i32> {
    let bar = progress_bar();
    let cb: Option<&ProgressFn> = bar.as_deref();
    let t0 = Instant::now();
    match cli.cmd {
        Cmd::Formats => {
            println!("write: {}", formats::writable_formats().join(", "));
            println!("read : {} + docx/xlsx/pptx/odt/epub/jar/apk (ZIP containers)",
                     formats::readable_formats().join(", "));
            println!("Ultra Max engine: built in (Rust)");
        }
        Cmd::A { archive, sources, fmt, level, method, password, best, recompress, dict, docs, no_dedup,
                 max, verbose } => {
            let o = CreateOptions {
                fmt, level, method, password: ask_password(password), store_compressed: !recompress,
                dict_mb: dict, docs, dedup: !no_dedup, engine: if max { "max".into() } else { "lzma".into() },
            };
            let src_total: u64 = util::collect_sources(&sources, &[])?.iter().map(|e| e.size).sum();
            let out = if best {
                let (out, res) = create_best(&archive, &sources, &o, cb)?;
                end_bar();
                for (f, s) in res {
                    println!("  {f:<8} {:>10}", human(Some(s)));
                }
                out
            } else {
                let st = create_archive(&archive, &sources, &o, cb)?;
                end_bar();
                if let (Some(st), true) = (st, verbose) {
                    print_ultra_stats(&st);
                }
                archive.clone()
            };
            let size = std::fs::metadata(&out)?.len();
            let ratio = if src_total > 0 { format!("{:.1}%", size as f64 * 100.0 / src_total as f64) } else { "-".into() };
            println!("Created {}  {} -> {} ({ratio})  in {:.1}s", out.display(), human(Some(src_total)),
                     human(Some(size)), t0.elapsed().as_secs_f64());
        }
        Cmd::X { archive, output, password, skip_existing } => {
            let dest = extract_archive(&archive, output.as_deref(), ask_password(password).as_deref(),
                                       !skip_existing, cb)?;
            end_bar();
            println!("Extracted to {}  in {:.1}s", dest.display(), t0.elapsed().as_secs_f64());
        }
        Cmd::L { archive, password } => {
            let entries = list_archive(&archive, ask_password(password).as_deref())?;
            println!("{:<17} {:>10} {:>10}  Name", "Modified", "Size", "Packed");
            println!("{}", "-".repeat(60));
            let mut tot = 0;
            for e in &entries {
                let mt = e.mtime.and_then(|m| chrono::DateTime::from_timestamp(m as i64, 0))
                    .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                let (s, p) = if e.is_dir { (String::new(), String::new()) } else { (human(e.size), human(e.packed)) };
                println!("{mt:<17} {s:>10} {p:>10}  {}", e.name);
                tot += e.size.unwrap_or(0);
            }
            println!("{}", "-".repeat(60));
            let files = entries.iter().filter(|e| !e.is_dir).count();
            println!("{files} files, {} uncompressed, archive {}", human(Some(tot)),
                     human(Some(std::fs::metadata(&archive)?.len())));
        }
        Cmd::T { archive, password } => {
            let (ok, msg) = test_archive(&archive, ask_password(password).as_deref(), cb)?;
            end_bar();
            println!("{}{msg}", if ok { "Everything is Ok: " } else { "ERROR: " });
            return Ok(if ok { 0 } else { 2 });
        }
        Cmd::Bench { source, quick } => {
            bench::run(&source, quick, &mut |l: &str| println!("{l}"))?;
        }
    }
    Ok(0)
}

fn main() {
    let args = normalize(std::env::args().collect());
    let cli = Cli::parse_from(args);
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            end_bar();
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
