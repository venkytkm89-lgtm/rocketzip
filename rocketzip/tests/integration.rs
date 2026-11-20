//! End-to-end tests:  cargo test --release
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use rocketzip::engines::{deflate, deflate_blocked, Deflater, Family, Z_FINISH, Z_SYNC_FLUSH};
use rocketzip::util::Source;
use rocketzip::*;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Deterministic pseudo-random bytes.
fn rnd(n: usize, seed: u32) -> Vec<u8> {
    let mut s = seed;
    (0..n).map(|_| { s = s.wrapping_mul(1103515245).wrapping_add(12345); (s >> 16) as u8 }).collect()
}

fn make_tree(root: &Path) {
    fs::create_dir_all(root.join("docs/sub")).unwrap();
    fs::create_dir_all(root.join("empty_dir")).unwrap();
    fs::write(root.join("docs/report.txt"), "Quarterly report\n".repeat(4000)).unwrap();
    fs::write(root.join("docs/sub/notes.md"), format!("# Notes\n{}", "lorem ipsum ".repeat(3000))).unwrap();
    let csv: String = (0..20000).map(|i| format!("{i},{},{}\n", i * i, i % 7)).collect();
    fs::write(root.join("docs/data.csv"), csv).unwrap();
    fs::write(root.join("docs/random.bin"), rnd(50000, 1)).unwrap();
    fs::write(root.join("docs/ünïcødé файл.txt"), "unicode name ok").unwrap();
    fs::write(root.join("docs/empty.txt"), "").unwrap();
}

fn snapshot(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(base: &Path, p: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for e in fs::read_dir(p).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            if p.is_dir() {
                out.insert(rel, None);
                walk(base, &p, out);
            } else {
                out.insert(rel, Some(fs::read(&p).unwrap()));
            }
        }
    }
    let mut m = BTreeMap::new();
    walk(root, root, &mut m);
    m
}

fn opts(fmt: &str) -> CreateOptions {
    CreateOptions { fmt: Some(fmt.into()), ..Default::default() }
}

/// Minimal ZIP writer whose members are compressed with a chosen Deflate
/// (like documents made by real office software).
fn build_zip(path: &Path, files: &[(&str, Vec<u8>)], comp: &dyn Fn(&[u8]) -> Vec<u8>) {
    let mut out = Vec::new();
    let mut cd = Vec::new();
    for (name, data) in files {
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let c = comp(data);
        let off = out.len() as u32;
        let hdr = |sig: &[u8], central: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(sig);
            if central { h.extend_from_slice(&20u16.to_le_bytes()); }
            h.extend_from_slice(&20u16.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // flags
            h.extend_from_slice(&8u16.to_le_bytes()); // deflate
            h.extend_from_slice(&0u32.to_le_bytes()); // time/date
            h.extend_from_slice(&crc.sum().to_le_bytes());
            h.extend_from_slice(&(c.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            if central {
                h.extend_from_slice(&[0u8; 6]); // comment len, disk, internal attrs
                h.extend_from_slice(&0u32.to_le_bytes());
                h.extend_from_slice(&off.to_le_bytes());
            }
            h.extend_from_slice(name.as_bytes());
            h
        };
        out.extend(hdr(b"PK\x03\x04", false));
        out.extend_from_slice(&c);
        cd.extend(hdr(b"PK\x01\x02", true));
    }
    let cd_off = out.len() as u32;
    out.extend_from_slice(&cd);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    fs::write(path, out).unwrap();
}

fn docx_xml(n: usize, seed: u32) -> Vec<u8> {
    let words = ["revenue", "growth", "market", "customer", "quarter", "plan", "team", "risk"];
    let r = rnd(n * 2, seed);
    let mut s = String::from("<w:document>");
    for i in 0..n {
        s += &format!("<w:r><w:rPr><w:sz w:val=\"{}\"/></w:rPr><w:t>{} {}</w:t></w:r>",
                      20 + (r[2 * i] % 3) * 2, words[(r[2 * i + 1] % 8) as usize], i % 997);
    }
    s += "</w:document>";
    s.into_bytes()
}

// --------------------------------------------------------------- formats
#[test]
fn all_multi_file_formats_roundtrip() {
    let t = tmp();
    let src = t.path().join("src");
    make_tree(&src);
    let want = snapshot(&src.join("docs"));
    for fmt in ["zfx", "zip", "7z", "tar", "tar.gz", "tar.bz2", "tar.xz", "tar.zst"] {
        let arc = t.path().join(format!("out.{fmt}"));
        create_archive(&arc, &[src.join("docs"), src.join("empty_dir")], &opts(fmt), None).unwrap();
        assert_eq!(formats::detect_format(&arc).unwrap(), fmt);
        let (ok, msg) = test_archive(&arc, None, None).unwrap();
        assert!(ok, "{fmt}: {msg}");
        assert!(list_archive(&arc, None).unwrap().iter().any(|e| e.name.trim_end_matches('/') == "docs/sub/notes.md"));
        let out = t.path().join(format!("x_{fmt}"));
        extract_archive(&arc, Some(&out), None, true, None).unwrap();
        assert_eq!(snapshot(&out.join("docs")), want, "{fmt}");
        assert!(out.join("empty_dir").is_dir(), "{fmt}");
    }
}

#[test]
fn zip_methods_roundtrip() {
    let t = tmp();
    let src = t.path().join("src");
    make_tree(&src);
    for m in ["deflate", "bzip2", "zstd", "xz", "store"] {
        let arc = t.path().join(format!("{m}.zip"));
        create_archive(&arc, &[src.join("docs")], &CreateOptions { method: Some(m.into()), ..opts("zip") }, None).unwrap();
        let out = t.path().join(format!("x_{m}"));
        extract_archive(&arc, Some(&out), None, true, None).unwrap();
        assert_eq!(snapshot(&out.join("docs")), snapshot(&src.join("docs")), "{m}");
    }
}

#[test]
fn single_stream_formats() {
    let t = tmp();
    let f = t.path().join("report.txt");
    fs::write(&f, "hello stream ".repeat(5000)).unwrap();
    for fmt in ["gz", "bz2", "xz", "zst"] {
        let arc = t.path().join(format!("report.txt.{fmt}"));
        create_archive(&arc, std::slice::from_ref(&f), &opts(fmt), None).unwrap();
        assert_eq!(formats::detect_format(&arc).unwrap(), fmt);
        let out = t.path().join(format!("s_{fmt}"));
        extract_archive(&arc, Some(&out), None, true, None).unwrap();
        assert_eq!(fs::read(out.join("report.txt")).unwrap(), fs::read(&f).unwrap());
    }
    let src = t.path().join("dir");
    make_tree(&src);
    assert!(create_archive(&t.path().join("x.xz"), &[src], &opts("xz"), None).is_err());
}

#[test]
fn detect_by_content_and_office_docs() {
    let t = tmp();
    let src = t.path().join("src");
    make_tree(&src);
    let arc = t.path().join("a.tar.xz");
    create_archive(&arc, &[src.join("docs")], &opts("tar.xz"), None).unwrap();
    let renamed = t.path().join("mystery.dat");
    fs::rename(&arc, &renamed).unwrap();
    assert_eq!(formats::detect_format(&renamed).unwrap(), "tar.xz");

    let docx = t.path().join("letter.docx");
    build_zip(&docx, &[("word/document.xml", docx_xml(50, 3))], &|d| deflate(d, 6, 8, -15, Family::Classic).unwrap());
    let out = extract_archive(&docx, None, None, true, None).unwrap();
    assert!(out.join("word/document.xml").exists());
}

#[test]
fn archive_not_added_to_itself_and_skip_existing() {
    let t = tmp();
    let src = t.path().join("src");
    make_tree(&src);
    let arc = src.join("docs/self.zip");
    create_archive(&arc, &[src.join("docs")], &opts("zip"), None).unwrap();
    assert!(!list_archive(&arc, None).unwrap().iter().any(|e| e.name == "docs/self.zip"));
    let out = t.path().join("o");
    extract_archive(&arc, Some(&out), None, true, None).unwrap();
    fs::write(out.join("docs/report.txt"), "mine").unwrap();
    extract_archive(&arc, Some(&out), None, false, None).unwrap();
    assert_eq!(fs::read_to_string(out.join("docs/report.txt")).unwrap(), "mine");
}

#[test]
fn passwords() {
    let t = tmp();
    let src = t.path().join("src");
    make_tree(&src);
    for fmt in ["zip", "7z"] {
        let arc = t.path().join(format!("secret.{fmt}"));
        let o = CreateOptions { password: Some("p4ss".into()), ..opts(fmt) };
        create_archive(&arc, &[src.join("docs")], &o, None).unwrap();
        let bad = extract_archive(&arc, Some(&t.path().join("bad")), Some("wrong"), true, None);
        assert!(bad.is_err(), "{fmt} accepted a wrong password");
        let out = t.path().join(format!("ok_{fmt}"));
        extract_archive(&arc, Some(&out), Some("p4ss"), true, None).unwrap();
        assert_eq!(snapshot(&out.join("docs")), snapshot(&src.join("docs")), "{fmt}");
    }
    let r = create_archive(&t.path().join("p.tar.xz"), &[src], &CreateOptions { password: Some("x".into()), ..opts("tar.xz") }, None);
    assert!(r.is_err());
}

#[test]
fn corruption_detected() {
    let t = tmp();
    let f = t.path().join("report.txt");
    fs::write(&f, (0..40000).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
    for fmt in ["zip", "tar.xz", "7z", "zfx"] {
        let arc = t.path().join(format!("c.{fmt}"));
        create_archive(&arc, std::slice::from_ref(&f), &opts(fmt), None).unwrap();
        let mut d = fs::read(&arc).unwrap();
        let i = d.len() / 3;
        d[i] ^= 0xFF;
        fs::write(&arc, d).unwrap();
        let r = test_archive(&arc, None, None);
        assert!(!matches!(r, Ok((true, _))), "{fmt}: corruption not detected");
    }
}

// --------------------------------------------------------------- security
#[test]
fn zip_slip_blocked() {
    let t = tmp();
    let arc = t.path().join("evil.zip");
    let mut zw = zip::ZipWriter::new(fs::File::create(&arc).unwrap());
    for name in ["../escaped.txt", "/abs.txt", "ok.txt"] {
        zw.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(b"pwned").unwrap();
    }
    zw.finish().unwrap();
    let dest = t.path().join("dest");
    extract_archive(&arc, Some(&dest), None, true, None).unwrap();
    assert!(!t.path().join("escaped.txt").exists());
    assert!(dest.join("ok.txt").exists());
}

#[test]
fn tar_slip_and_absolute_symlink_blocked() {
    let t = tmp();
    let arc = t.path().join("evil.tar");
    let mut b = tar::Builder::new(fs::File::create(&arc).unwrap());
    for name in ["../escaped.txt", "good.txt"] {
        let mut h = tar::Header::new_old();
        h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
        h.set_size(5);
        h.set_mode(0o644);
        h.set_cksum();
        b.append(&h, &b"pwned"[..]).unwrap();
    }
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Symlink);
    h.set_size(0);
    b.append_link(&mut h, "link", "/etc/passwd").unwrap();
    b.finish().unwrap();
    drop(b);
    let dest = t.path().join("dest");
    extract_archive(&arc, Some(&dest), None, true, None).unwrap();
    assert!(!t.path().join("escaped.txt").exists());
    assert!(fs::symlink_metadata(dest.join("link")).is_err());
    assert!(dest.join("good.txt").exists());
}

#[test]
fn ultra_unsafe_names_blocked() {
    let t = tmp();
    let f = t.path().join("evil.txt");
    fs::write(&f, "pwned").unwrap();
    let arc = t.path().join("evil.zfx");
    let srcs = vec![
        Source { path: f.clone(), arcname: "../escaped.txt".into(), is_dir: false, size: 5 },
        Source { path: f.clone(), arcname: "ok.txt".into(), is_dir: false, size: 5 },
    ];
    let o = ultra::UltraOptions { level: 9, dict_mb: None, docs: "exact".into(), dedup: false,
                                  engine: "lzma".into(), threads: None };
    ultra::write_zfx(&arc, &srcs, &o, None).unwrap();
    extract_archive(&arc, Some(&t.path().join("dest")), None, true, None).unwrap();
    assert!(!t.path().join("escaped.txt").exists());
    assert!(t.path().join("dest/ok.txt").exists());
}

// --------------------------------------------------------------- Ultra
fn docs_dir(root: &Path) -> PathBuf {
    let d = root.join("docs");
    fs::create_dir_all(&d).unwrap();
    for n in 0..3u32 {
        build_zip(&d.join(format!("report{n}.docx")),
                  &[("[Content_Types].xml", b"<Types/>".repeat(50)), ("word/document.xml", docx_xml(6000, n))],
                  &|x| deflate(x, 6, 8, -15, Family::Classic).unwrap());
    }
    fs::copy(d.join("report0.docx"), d.join("copy_of_report0.docx")).unwrap();
    // LibreOffice-style block-parallel Deflate
    build_zip(&d.join("sheet.xlsx"), &[("xl/sheet1.xml", docx_xml(30000, 9))],
              &|x| deflate_blocked(x, 6, 8, Family::Classic, 128 << 10, Z_SYNC_FLUSH).unwrap());
    d
}

#[test]
fn ultra_exact_is_byte_identical_and_beats_xz() {
    let t = tmp();
    let d = docs_dir(t.path());
    let arc = t.path().join("a.zfx");
    let st = create_archive(&arc, std::slice::from_ref(&d), &opts("zfx"), None).unwrap().unwrap();
    assert_eq!(st.duplicates, 1);
    assert_eq!(st.docs_exact, 4, "all documents incl. block-parallel one recompressed exactly");
    let (ok, msg) = test_archive(&arc, None, None).unwrap();
    assert!(ok, "{msg}");
    let out = t.path().join("x");
    extract_archive(&arc, Some(&out), None, true, None).unwrap();
    assert_eq!(snapshot(&out.join("docs")), snapshot(&d));
    let txz = t.path().join("a.tar.xz");
    create_archive(&txz, &[d], &opts("tar.xz"), None).unwrap();
    let (z, x) = (fs::metadata(&arc).unwrap().len(), fs::metadata(&txz).unwrap().len());
    assert!(z * 10 < x * 6, "zfx {z} should be much smaller than tar.xz {x}");
}

#[test]
fn ultra_full_mode_rebuilds_foreign_documents_and_pdfs() {
    let t = tmp();
    let d = t.path().join("f");
    fs::create_dir_all(&d).unwrap();
    // Deflate settings zlib can't reproduce (filtered strategy), like MS Office files
    let filtered = |x: &[u8]| {
        let mut c = Deflater::new(Family::Classic, 6, -15, 8, 1, None).unwrap();
        let mut o = c.compress(x);
        o.extend(c.flush(Z_FINISH));
        o
    };
    build_zip(&d.join("book.xlsx"), &[("xl/sheet1.xml", docx_xml(20000, 5))], &filtered);
    let stream = deflate(&b"BT /F1 12 Tf (Hello PDF) Tj ET\n".repeat(2000), 6, 8, 15, Family::Classic).unwrap();
    let mut pdf = format!("%PDF-1.4\n1 0 obj\n<< /Length {} /Filter /FlateDecode >>\nstream\n", stream.len()).into_bytes();
    pdf.extend(stream);
    pdf.extend(b"\nendstream\nendobj\ntrailer\n<< >>\n%%EOF\n");
    fs::write(d.join("doc.pdf"), &pdf).unwrap();

    let exact = create_archive(&t.path().join("e.zfx"), std::slice::from_ref(&d), &opts("zfx"), None).unwrap().unwrap();
    assert_eq!(exact.docs, 0, "exact mode leaves both alone");
    let o = CreateOptions { docs: "full".into(), ..opts("zfx") };
    let st = create_archive(&t.path().join("f.zfx"), std::slice::from_ref(&d), &o, None).unwrap().unwrap();
    assert_eq!(st.docs, 2);
    assert!(fs::metadata(t.path().join("f.zfx")).unwrap().len() < fs::metadata(t.path().join("e.zfx")).unwrap().len());
    let out = extract_archive(&t.path().join("f.zfx"), Some(&t.path().join("x")), None, true, None).unwrap();
    assert_eq!(fs::read(out.join("f/doc.pdf")).unwrap(), pdf, "PDF restored byte-for-byte");
    let mut a = zip::ZipArchive::new(fs::File::open(out.join("f/book.xlsx")).unwrap()).unwrap();
    let mut s = String::new();
    std::io::Read::read_to_string(&mut a.by_index(0).unwrap(), &mut s).unwrap();
    assert_eq!(s.into_bytes(), docx_xml(20000, 5), "rebuilt document has identical content");
}

#[test]
fn ultra_off_mode_no_dedup_and_empty_files() {
    let t = tmp();
    let d = docs_dir(t.path());
    fs::write(d.join("empty.txt"), "").unwrap();
    let o = CreateOptions { docs: "off".into(), dedup: false, ..opts("zfx") };
    let st = create_archive(&t.path().join("o.zfx"), std::slice::from_ref(&d), &o, None).unwrap().unwrap();
    assert_eq!((st.docs, st.duplicates), (0, 0));
    let out = extract_archive(&t.path().join("o.zfx"), Some(&t.path().join("x")), None, true, None).unwrap();
    assert_eq!(snapshot(&out.join("docs")), snapshot(&d));
}

#[test]
fn ultra_truncated_archive_reports_error() {
    let t = tmp();
    let src = t.path().join("src");
    make_tree(&src);
    let arc = t.path().join("c.zfx");
    create_archive(&arc, &[src], &opts("zfx"), None).unwrap();
    let d = fs::read(&arc).unwrap();
    fs::write(&arc, &d[..d.len() - 100]).unwrap();
    assert!(list_archive(&arc, None).is_err());
}

#[test]
fn ultra_max_roundtrip_smaller_and_filters_by_content() {
    let t = tmp();
    let src = t.path().join("src");
    make_tree(&src);
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for e in fs::read_dir(&lib).unwrap().chain(fs::read_dir(lib.join("ultra")).unwrap()) {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            fs::copy(&p, src.join(p.file_name().unwrap())).unwrap(); // natural text
        }
    }
    let wav: Vec<u8> = (0..200000).flat_map(|i| (((i as f64 / 20.0).sin() * 8000.0) as i16).to_le_bytes()).collect();
    let mut w = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    w.extend(wav);
    fs::write(src.join("tone.wav"), w).unwrap();

    let plain = t.path().join("a.zfx");
    create_archive(&plain, std::slice::from_ref(&src), &opts("zfx"), None).unwrap();
    let mx = t.path().join("m.zfx");
    let st = create_archive(&mx, std::slice::from_ref(&src), &CreateOptions { engine: "max".into(), ..opts("zfx") }, None)
        .unwrap().unwrap();
    assert!(fs::metadata(&mx).unwrap().len() < fs::metadata(&plain).unwrap().len());
    assert!(st.groups.iter().any(|g| g.filter.starts_with("cm")));
    let (ok, msg) = test_archive(&mx, None, None).unwrap();
    assert!(ok, "{msg}");
    let out = extract_archive(&mx, Some(&t.path().join("x")), None, true, None).unwrap();
    assert_eq!(snapshot(&out.join("src")), snapshot(&src));

    let st = create_archive(&t.path().join("b.zfx"), &[src], &opts("zfx"), None).unwrap().unwrap();
    let media = st.groups.iter().find(|g| g.kind == "media").unwrap();
    assert!(media.filter.starts_with("delta"), "audio uses a delta filter, got {}", media.filter);
}

#[test]
fn best_picks_smallest_and_dictionary_option() {
    let t = tmp();
    let src = t.path().join("src");
    make_tree(&src);
    let (out, res) = create_best(&t.path().join("best"), std::slice::from_ref(&src), &CreateOptions::default(), None).unwrap();
    assert_eq!(fs::metadata(&out).unwrap().len(), res.iter().map(|r| r.1).min().unwrap());
    let arc = t.path().join("d.zfx");
    create_archive(&arc, std::slice::from_ref(&src), &CreateOptions { dict_mb: Some(128), ..opts("zfx") }, None).unwrap();
    let out = extract_archive(&arc, Some(&t.path().join("x")), None, true, None).unwrap();
    assert_eq!(snapshot(&out.join("src")), snapshot(&src));
}

#[test]
fn engines_reproduce_zlib_outputs() {
    // classic zlib output for this input never changes; zlib-ng must round-trip
    let data = docx_xml(3000, 1);
    for fam in Family::ALL {
        for lv in 1..=9 {
            let c = deflate(&data, lv, 8, -15, fam).unwrap();
            let (back, used) = ultra::precomp::inflate(&c, false).unwrap();
            assert_eq!((back, used), (data.clone(), c.len()));
            assert_eq!(ultra::precomp::find_params(&c, &data, -15, &[fam]).map(|p| p.2), Some(fam));
        }
    }
}

// --------------------------------------------------------------- CLI
#[test]
fn cli_roundtrip_with_7zip_style_switches() {
    let exe = env!("CARGO_BIN_EXE_rocketzip-cli");
    let t = tmp();
    let f = t.path().join("f.txt");
    fs::write(&f, "hello ".repeat(1000)).unwrap();
    let run = |args: &[&str]| std::process::Command::new(exe).args(args).output().unwrap();
    let p = |x: &Path| x.to_string_lossy().into_owned();
    assert!(run(&["a", &p(&t.path().join("o.zfx")), &p(&f), "-mx9", "-v"]).status.success());
    assert!(run(&["t", &p(&t.path().join("o.zfx"))]).status.success());
    assert!(run(&["l", &p(&t.path().join("o.zfx"))]).status.success());
    let o = format!("-o{}", p(&t.path().join("x")));
    assert!(run(&["x", &p(&t.path().join("o.zfx")), &o]).status.success());
    assert_eq!(fs::read_to_string(t.path().join("x/f.txt")).unwrap(), "hello ".repeat(1000));
    assert!(run(&["a", &p(&t.path().join("s.zip")), &p(&f), "-mm=zstd", "-pS3cret"]).status.success());
    assert!(!run(&["x", &p(&t.path().join("s.zip")), "-pbad", &format!("-o{}", p(&t.path().join("y")))]).status.success());
    assert_eq!(run(&["x", &p(&t.path().join("missing.zip"))]).status.code(), Some(1));
}
