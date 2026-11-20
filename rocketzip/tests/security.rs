//! Regression tests for security fixes: each test is a real attack file.
use std::fs;
use std::io::Write;
use std::path::Path;
use rocketzip::util::safe_target;
use rocketzip::*;

/// Builds a raw .zfx with one block and the given manifest entries.
fn craft_zfx(path: &Path, block: &[u8], mut blk: serde_json::Value, entries: serde_json::Value) {
    let mut out = b"ZFX\x01".to_vec();
    blk["o"] = 4.into();
    blk["c"] = (block.len() as u64).into();
    out.extend_from_slice(block);
    let man = serde_json::json!({"v": 1, "zlib": "classic+ng", "blocks": [blk], "entries": entries});
    let mut e = liblzma::write::XzEncoder::new(Vec::new(), 6);
    e.write_all(man.to_string().as_bytes()).unwrap();
    let m = e.finish().unwrap();
    let mo = out.len() as u64;
    out.extend_from_slice(&m);
    out.extend_from_slice(&mo.to_le_bytes());
    out.extend_from_slice(&(m.len() as u64).to_le_bytes());
    out.extend_from_slice(b"ZFXEND01");
    fs::write(path, out).unwrap();
}

fn xz(data: &[u8]) -> Vec<u8> {
    let mut e = liblzma::write::XzEncoder::new(Vec::new(), 6);
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

#[test]
fn names_that_escape_on_windows_are_rejected_everywhere() {
    let d = Path::new("/dest");
    for bad in ["a/C:/x", "C:x", "a/.. /b", "a/. ./b", "a/... /b", "..", "a/../b", "/../x",
                "dir/CON", "nul.txt", "a/Com1.log", "x\u{1}y", "f.txt:stream", "a\\..\\..\\b"] {
        assert!(safe_target(d, bad).is_none(), "should block {bad:?}");
    }
    for ok in ["a/b.txt", "/abs/ok.txt", "folder/./file", "con-artist.txt", "a/..b/c", "nulled"] {
        assert!(safe_target(d, ok).is_some(), "should allow {ok:?}");
    }
}

#[cfg(unix)]
#[test]
fn tar_symlink_chain_cannot_write_outside() {
    let t = tempfile::tempdir().unwrap();
    let arc = t.path().join("chain.tar");
    let mut b = tar::Builder::new(fs::File::create(&arc).unwrap());
    for (name, target) in [("s", "."), ("s/l", "../escaped")] {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        b.append_link(&mut h, name, target).unwrap();
    }
    let mut h = tar::Header::new_gnu();
    h.set_size(5);
    h.set_mode(0o644);
    h.set_cksum();
    b.append_data(&mut h, "s/l/pwned.txt", &b"pwned"[..]).unwrap();
    b.finish().unwrap();
    drop(b);
    fs::create_dir(t.path().join("escaped")).unwrap();
    let _ = extract_archive(&arc, Some(&t.path().join("dest")), None, true, None);
    assert!(!t.path().join("escaped/pwned.txt").exists(), "wrote outside the destination");
}

#[cfg(unix)]
#[test]
fn never_writes_through_a_link_already_in_the_destination() {
    let t = tempfile::tempdir().unwrap();
    let dest = t.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    fs::create_dir(t.path().join("secret")).unwrap();
    std::os::unix::fs::symlink(t.path().join("secret"), dest.join("docs")).unwrap();
    std::os::unix::fs::symlink(t.path().join("victim.txt"), dest.join("note.txt")).unwrap();
    let arc = t.path().join("a.zip");
    let mut zw = zip::ZipWriter::new(fs::File::create(&arc).unwrap());
    for name in ["docs/evil.txt", "note.txt"] {
        zw.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(b"pwned").unwrap();
    }
    zw.finish().unwrap();
    let _ = extract_archive(&arc, Some(&dest), None, true, None);
    assert!(!t.path().join("secret/evil.txt").exists(), "followed a directory link");
    assert!(!t.path().join("victim.txt").exists(), "followed a file link");
}

#[test]
fn crafted_document_data_does_not_crash() {
    let t = tempfile::tempdir().unwrap();
    // invalid deflate level 15 inside a recompressed-document blob
    let mut blob = b"ZFPCZ".to_vec();
    blob.extend_from_slice(&[1, 15, 8]);
    blob.extend_from_slice(&0u64.to_le_bytes());
    blob.extend_from_slice(&10u64.to_le_bytes());
    blob.extend_from_slice(&5u64.to_le_bytes());
    blob.extend_from_slice(b"hello");
    let arc = t.path().join("crash.zfx");
    craft_zfx(&arc, &xz(&blob), serde_json::json!({"u": blob.len(), "k": "doc", "f": "lzma2"}),
              serde_json::json!([{"n": "a.docx", "t": "f", "s": 5, "h": "00".repeat(32), "x": "pc",
                                  "b": 0, "o": 0, "l": blob.len()}]));
    assert!(extract_archive(&arc, Some(&t.path().join("x")), None, true, None).is_err());
    assert!(!matches!(test_archive(&arc, None, None), Ok((true, _))));
}

#[test]
fn crafted_sizes_do_not_exhaust_memory() {
    let t = tempfile::tempdir().unwrap();
    // Max-engine segment claiming 4 GB
    let mut cm = 0xFFFF_FFF0u32.to_le_bytes().to_vec();
    cm.extend_from_slice(&4u32.to_le_bytes());
    cm.extend_from_slice(&[0; 4]);
    let a = t.path().join("mem.zfx");
    craft_zfx(&a, &cm, serde_json::json!({"u": 100, "k": "text", "f": "cm", "z": "cm", "mb": 22, "e8": false}),
              serde_json::json!([{"n": "a.txt", "t": "f", "s": 100, "h": "00".repeat(32), "b": 0, "o": 0, "l": 100}]));
    assert!(!matches!(test_archive(&a, None, None), Ok((true, _))));
    // absurd memory setting for the Max engine
    let b = t.path().join("mb.zfx");
    craft_zfx(&b, &[], serde_json::json!({"u": 1, "k": "text", "f": "cm", "z": "cm", "mb": 30, "e8": false}),
              serde_json::json!([{"n": "a.txt", "t": "f", "s": 1, "h": "00".repeat(32), "b": 0, "o": 0, "l": 1}]));
    assert!(!matches!(test_archive(&b, None, None), Ok((true, _))));
    // document blob claiming 100 GB
    let c = t.path().join("blob.zfx");
    craft_zfx(&c, &xz(b"x"), serde_json::json!({"u": 1, "k": "doc", "f": "lzma2"}),
              serde_json::json!([{"n": "a.docx", "t": "f", "s": 1, "h": "00".repeat(32), "x": "pc",
                                  "b": 0, "o": 0, "l": 100u64 << 30}]));
    assert!(extract_archive(&c, Some(&t.path().join("y")), None, true, None).is_err());
}

#[test]
fn unsafe_names_in_every_format_are_blocked() {
    let t = tempfile::tempdir().unwrap();
    let arc = t.path().join("evil.zip");
    let mut zw = zip::ZipWriter::new(fs::File::create(&arc).unwrap());
    for name in ["a/.. /x.txt", "C:/x.txt", "ok.txt"] {
        zw.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(b"data").unwrap();
    }
    zw.finish().unwrap();
    let dest = t.path().join("d");
    extract_archive(&arc, Some(&dest), None, true, None).unwrap();
    // Only ok.txt may be written. (List the folder instead of testing dest.join("C:"):
    // on Windows that path means "current folder on drive C:", which always exists.)
    let names: Vec<String> = fs::read_dir(&dest).unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["ok.txt".to_string()], "unexpected files: {names:?}");
}

#[test]
fn archive_temp_file_is_not_predictable() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("f.txt");
    fs::write(&f, "hello").unwrap();
    // A pre-planted file at the old predictable name must be left alone.
    fs::write(t.path().join("out.zip.part"), "not mine").unwrap();
    create_archive(&t.path().join("out.zip"), &[f], &CreateOptions::default(), None).unwrap();
    assert_eq!(fs::read_to_string(t.path().join("out.zip.part")).unwrap(), "not mine");
    assert!(t.path().join("out.zip").exists());
}
