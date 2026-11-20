//! zfcm-tool: compress / decompress files with the RocketZip Max engine.
//!
//!   zfcm-tool c [--e8e9] [--mem N] INPUT OUTPUT    compress
//!   zfcm-tool d [--e8e9] [--mem N] INPUT OUTPUT    decompress
//!   zfcm-tool bench [--mem N] FILE...              speed and ratio
use std::time::Instant;
use std::{env, fs, process};

fn usage() -> ! {
    eprintln!(
        "usage:\n  zfcm-tool c [--e8e9] [--mem N] INPUT OUTPUT\n  \
         zfcm-tool d [--e8e9] [--mem N] INPUT OUTPUT\n  zfcm-tool bench [--mem N] FILE..."
    );
    process::exit(2);
}

fn main() {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let mut e8 = false;
    let mut mem = 22;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--e8e9" => {
                e8 = true;
                args.remove(i);
            }
            "--mem" => {
                args.remove(i);
                mem = args.get(i).and_then(|v| v.parse().ok()).unwrap_or_else(|| usage());
                args.remove(i);
            }
            _ => i += 1,
        }
    }
    let fail = |msg: String| -> ! {
        eprintln!("error: {msg}");
        process::exit(1);
    };
    match args.first().map(String::as_str) {
        Some("c") | Some("d") if args.len() == 3 => {
            let data = fs::read(&args[1]).unwrap_or_else(|e| fail(format!("{}: {e}", args[1])));
            let out = if args[0] == "c" {
                zfcm::compress(&data, mem, e8)
            } else {
                zfcm::decompress(&data, mem, e8)
            }
            .unwrap_or_else(|| fail("out of memory or corrupt input".into()));
            fs::write(&args[2], &out).unwrap_or_else(|e| fail(format!("{}: {e}", args[2])));
            println!("{} -> {} bytes", data.len(), out.len());
        }
        Some("bench") if args.len() > 1 => {
            for f in &args[1..] {
                let data = fs::read(f).unwrap_or_else(|e| fail(format!("{f}: {e}")));
                let t = Instant::now();
                let c = zfcm::compress(&data, mem, e8).unwrap_or_else(|| fail("out of memory".into()));
                let tc = t.elapsed().as_secs_f64();
                let t = Instant::now();
                let d = zfcm::decompress(&c, mem, e8).unwrap();
                let td = t.elapsed().as_secs_f64();
                assert!(d == data, "round-trip mismatch on {f}");
                let mb = data.len() as f64 / 1e6;
                println!(
                    "{f}: {} -> {} ({:.1}%)  compress {:.2} MB/s  decompress {:.2} MB/s",
                    data.len(),
                    c.len(),
                    c.len() as f64 * 100.0 / data.len().max(1) as f64,
                    mb / tc.max(1e-9),
                    mb / td.max(1e-9)
                );
            }
        }
        _ => usage(),
    }
}
