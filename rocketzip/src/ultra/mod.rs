//! RocketZip Ultra (.zfx): archive format tuned for the highest ratio.
//! Byte-compatible with the Python version (`ultra.py`): each implementation
//! reads the other's archives.
//!
//! Layout: "ZFX\x01" | block 0 | block 1 | ... | manifest (xz JSON) | trailer
//! trailer = manifest offset (u64) + manifest length (u64) + "ZFXEND01"
//! A block is one XZ stream, or (Max mode) framed context-mixing segments:
//! repeated [u32 raw len][u32 packed len][data].
pub mod precomp;

use crate::bail;
use crate::tarfmt::{build_filters, lz, pow2, Flt, PRESET_DICT};
use crate::util::*;
use liblzma::stream::{Action, Status, Stream};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

pub const MAGIC: &[u8; 4] = b"ZFX\x01";
const TRAILER_MAGIC: &[u8; 8] = b"ZFXEND01";
const TRAILER_LEN: u64 = 24;
const SAMPLE: usize = 2 << 20;
const CM_SAMPLE: usize = 1 << 20;
const CM_FULL_TRIAL: u64 = 4 << 20;
const SPLIT_MIN: u64 = 256 << 10;
/// Limits that stop crafted archives from demanding huge amounts of memory.
const MAX_MANIFEST: u64 = 512 << 20;
const MAX_DOC_BLOB: u64 = 2 << 30;
pub const DOC_MODES: [&str; 3] = ["off", "exact", "full"];
const GROUP_ORDER: [&str; 6] = ["text", "doc", "binary", "exe", "media", "packed"];
const ZLIB_FLAVOR: &str = "classic+ng";

// ---------------------------------------------------------------------------
// manifest (same JSON keys as the Python version)
// ---------------------------------------------------------------------------
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ManEntry {
    pub n: String,
    pub t: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub d: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub o: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Block {
    pub o: u64,
    pub c: u64,
    pub u: u64,
    pub k: String,
    pub f: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mb: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub e8: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Manifest {
    pub v: u32,
    #[serde(default)]
    pub zlib: Option<String>,
    #[serde(default)]
    pub created: Option<f64>,
    #[serde(default)]
    pub level: Option<u32>,
    pub blocks: Vec<Block>,
    pub entries: Vec<ManEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct GroupStat {
    pub kind: String,
    pub filter: String,
    pub files: usize,
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Clone, Default)]
pub struct UltraStats {
    pub files: usize,
    pub dirs: usize,
    pub input: u64,
    pub duplicates: usize,
    pub dup_bytes: u64,
    pub docs: usize,
    pub docs_exact: usize,
    pub groups: Vec<GroupStat>,
    pub output: u64,
}

#[derive(Debug, Clone)]
pub struct UltraOptions {
    pub level: u32,
    pub dict_mb: Option<u32>,
    pub docs: String,
    pub dedup: bool,
    pub engine: String,
    pub threads: Option<usize>,
}

// ---------------------------------------------------------------------------
// classification and filter selection
// ---------------------------------------------------------------------------
const EXE_EXT: &[&str] = &[".exe", ".dll", ".so", ".dylib", ".sys", ".o", ".a", ".lib", ".pyd", ".ocx"];
const MEDIA_EXT: &[&str] = &[".wav", ".bmp", ".tif", ".tiff", ".aif", ".aiff", ".pcm", ".raw", ".pgm",
                             ".ppm", ".pbm", ".tga", ".dds", ".psd", ".dng", ".cr2", ".nef", ".iso"];
const EXE_MAGIC: &[&[u8]] = &[b"MZ", b"\x7fELF", b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe", b"\xca\xfe\xba\xbe"];

fn looks_text(s: &[u8]) -> bool {
    if s.is_empty() {
        return true;
    }
    if s.contains(&0) {
        return false;
    }
    match std::str::from_utf8(s) {
        Ok(_) => return true,
        Err(e) if e.valid_up_to() >= s.len().saturating_sub(4) => return true,
        Err(_) => {}
    }
    let high = s.iter().filter(|&&b| b > 127).count();
    let ctrl = s.iter().filter(|&&b| b < 32 && b != 9 && b != 10 && b != 13).count();
    (ctrl as f64) < s.len() as f64 * 0.01 && (high as f64) < s.len() as f64 * 0.3
}

pub fn classify(name: &str, head: &[u8]) -> &'static str {
    let ext = ext_lower(name);
    if EXE_EXT.contains(&ext.as_str()) || EXE_MAGIC.iter().any(|m| head.starts_with(m)) {
        return "exe";
    }
    if MEDIA_EXT.contains(&ext.as_str()) || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WAVE")) {
        return "media";
    }
    // Same test as the Python version: zlib level 1 can't shrink it -> already compressed.
    if head.len() >= 4096
        && crate::engines::deflate(head, 1, 8, 15, crate::engines::Family::Classic)
            .is_some_and(|c| c.len() as f64 > 0.97 * head.len() as f64)
    {
        return "packed";
    }
    if looks_text(head) { "text" } else { "binary" }
}

#[derive(Clone, Debug)]
pub enum Spec {
    Lzma(Vec<Flt>),
    Cm { e8: bool },
}

fn lz_with(level: u32, d: u32, lc: Option<u32>, lp: Option<u32>, pb: Option<u32>) -> Flt {
    match lz(level, d) {
        Flt::Lzma2 { level, dict, nice, .. } => Flt::Lzma2 { level, dict, nice, lc, lp, pb },
        other => other,
    }
}

fn candidates(cls: &str, level: u32, d: u32) -> Vec<(String, Vec<Flt>)> {
    let base = ("lzma2".to_string(), vec![lz(level, d)]);
    if level == 0 || cls == "packed" {
        let p = if level == 0 { 0 } else { 1 };
        return vec![("lzma2-fast".into(), vec![Flt::Lzma2 { level: p, dict: d.min(1 << 20), nice: None,
                                                            lc: None, lp: None, pb: None }])];
    }
    match cls {
        "text" | "doc" => vec![("lzma2-text".into(), vec![lz_with(level, d, Some(4), Some(0), Some(0))]), base],
        "exe" => vec![
            ("x86+lzma2".into(), vec![Flt::X86, lz(level, d)]),
            ("arm+lzma2".into(), vec![Flt::Arm, lz(level, d)]),
            ("armthumb+lzma2".into(), vec![Flt::ArmThumb, lz(level, d)]),
            base,
        ],
        "media" => {
            let mut v: Vec<(String, Vec<Flt>)> = [2u32, 4, 1, 3].iter()
                .map(|&n| (format!("delta{n}+lzma2"), vec![Flt::Delta(n), lz(level, d)]))
                .collect();
            v.push(base);
            v
        }
        _ => vec![
            base,
            ("lzma2-aligned".into(), vec![lz_with(level, d, Some(2), Some(2), Some(2))]),
            ("x86+lzma2".into(), vec![Flt::X86, lz(level, d)]),
            ("delta4+lzma2".into(), vec![Flt::Delta(4), lz(level, d)]),
        ],
    }
}

fn with_dict(chain: &[Flt], d: u32) -> Vec<Flt> {
    chain.iter().map(|f| match f {
        Flt::Lzma2 { level, nice, lc, lp, pb, .. } => Flt::Lzma2 {
            level: *level,
            dict: if *level >= 9 { d } else { d.min(PRESET_DICT[*level as usize]) },
            nice: *nice, lc: *lc, lp: *lp, pb: *pb,
        },
        other => other.clone(),
    }).collect()
}

/// Size of `data` compressed raw with `chain` (for trials).
fn lzma_raw_size(data: &[u8], chain: &[Flt]) -> Option<usize> {
    let mut s = Stream::new_raw_encoder(&build_filters(chain).ok()?).ok()?;
    let mut out = Vec::with_capacity(data.len() / 2 + 4096);
    let mut pos = 0;
    loop {
        if out.capacity() - out.len() < 1 << 16 {
            out.reserve(1 << 20);
        }
        let before = s.total_in();
        let st = s.process_vec(&data[pos..], &mut out, if pos < data.len() { Action::Run } else { Action::Finish }).ok()?;
        pos += (s.total_in() - before) as usize;
        if st == Status::StreamEnd {
            return Some(out.len());
        }
    }
}

struct Item {
    idx: usize,
    name: String,
    payload: PathBuf,
    plen: u64,
    sha: String,
    transformed: bool,
    off: u64,
}

/// Representative sample: each file contributes in proportion to its size.
fn sample(items: &[Item], limit: usize) -> Vec<u8> {
    let total: u64 = items.iter().map(|i| i.plen).sum::<u64>().max(1);
    let mut out = Vec::new();
    for it in items {
        let take = if total <= limit as u64 { it.plen } else { (it.plen * limit as u64 / total).max(4096) };
        if let Ok(mut f) = File::open(&it.payload) {
            if take < it.plen {
                let _ = f.seek(SeekFrom::Start((it.plen - take) / 2)); // middle: headers aren't typical
            }
            let mut buf = Vec::new();
            let _ = f.take(take).read_to_end(&mut buf);
            out.extend(buf);
        }
    }
    out.truncate(limit.max(1));
    out
}

fn choose_filters(cls: &str, items: &[Item], level: u32, dict_cap: u32, engine: &str) -> (String, Spec) {
    let usize_: u64 = items.iter().map(|i| i.plen).sum();
    let d = pow2(usize_, 1 << 16, dict_cap);
    let cands = candidates(cls, level, d);
    let use_cm = engine == "max" && cls != "packed" && level >= 7;
    if (cands.len() == 1 || level < 7 || usize_ < 32 << 10) && !use_cm {
        let (n, c) = cands[0].clone();
        return (n, Spec::Lzma(c));
    }
    let smp = sample(items, SAMPLE);
    let sd = pow2(smp.len() as u64, 1 << 16, d);
    let mut best: Option<(usize, String)> = None;
    for (name, chain) in candidates(cls, level, sd) {
        if let Some(size) = lzma_raw_size(&smp, &chain)
            && best.as_ref().is_none_or(|b| size < b.0) {
                best = Some((size, name));
            }
    }
    let best_name = best.map(|b| b.1).unwrap_or_else(|| cands[0].0.clone());
    let (lz_name, lz_chain) = cands.into_iter().find(|c| c.0 == best_name).unwrap();
    if !use_cm {
        return (lz_name, Spec::Lzma(lz_chain));
    }
    // Compare the CM engine with the best LZMA2 chain on the same sample.
    let small = if usize_ <= CM_FULL_TRIAL { sample(items, CM_FULL_TRIAL as usize) } else { sample(items, CM_SAMPLE) };
    let small_chain = with_dict(&lz_chain, pow2(small.len() as u64, 1 << 16, d));
    let mut best = (lzma_raw_size(&small, &small_chain).unwrap_or(usize::MAX), lz_name, Spec::Lzma(lz_chain));
    let e8s: &[bool] = if cls == "exe" || cls == "binary" { &[false, true] } else { &[false] };
    for &e8 in e8s {
        if let Some(c) = zfcm::compress(&small, 20, e8)
            && c.len() < best.0 {
                best = (c.len(), if e8 { "cm+e8e9".into() } else { "cm".into() }, Spec::Cm { e8 });
            }
    }
    (best.1, best.2)
}

pub fn membits_for(dict_mb: Option<u32>) -> i32 {
    match dict_mb {
        Some(d) if d >= 512 => 24,
        Some(d) if d >= 128 => 23,
        _ => 22,
    }
}

fn sha_hex(h: Sha256) -> String {
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_file(p: &Path) -> Result<String> {
    let mut f = BufReader::new(File::open(p)?);
    let mut h = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(sha_hex(h));
        }
        h.update(&buf[..n]);
    }
}

// ---------------------------------------------------------------------------
// writer
// ---------------------------------------------------------------------------
enum BlockSink {
    Xz(liblzma::write::XzEncoder<BufWriter<File>>),
    Cm { model: Box<zfcm::Model>, e8: bool, pend: Vec<u8>, out: BufWriter<File>, tmp: Vec<u8> },
}

impl BlockSink {
    fn write(&mut self, data: &[u8], prog: &Progress, name: &str) -> Result<()> {
        match self {
            BlockSink::Xz(w) => {
                w.write_all(data)?;
                prog.advance(data.len() as u64, name);
            }
            BlockSink::Cm { pend, .. } => {
                pend.extend_from_slice(data);
                self.emit(false, prog, name)?;
            }
        }
        Ok(())
    }

    fn emit(&mut self, last: bool, prog: &Progress, name: &str) -> Result<()> {
        if let BlockSink::Cm { model, e8, pend, out, tmp } = self {
            while pend.len() >= zfcm::SEGMENT || (last && !pend.is_empty()) {
                let n = pend.len().min(zfcm::SEGMENT);
                let mut seg: Vec<u8> = pend.drain(..n).collect();
                if *e8 {
                    zfcm::e8e9(&mut seg, true);
                }
                if tmp.len() < zfcm::bound(n) {
                    tmp.resize(zfcm::bound(n), 0);
                }
                let c = model.encode_segment(&seg, tmp).ok_or_else(|| Error("CM buffer too small".into()))?;
                out.write_all(&(n as u32).to_le_bytes())?;
                out.write_all(&(c as u32).to_le_bytes())?;
                out.write_all(&tmp[..c])?;
                prog.advance(n as u64, name);
            }
        }
        Ok(())
    }

    fn finish(mut self, prog: &Progress) -> Result<()> {
        self.emit(true, prog, "")?;
        match self {
            BlockSink::Xz(w) => { w.finish()?.flush()?; }
            BlockSink::Cm { mut out, .. } => out.flush()?,
        }
        Ok(())
    }
}

pub fn write_zfx(out_path: &Path, entries: &[Source], o: &UltraOptions, cb: Option<&ProgressFn>) -> Result<UltraStats> {
    if !DOC_MODES.contains(&o.docs.as_str()) {
        bail!("--docs must be one of off, exact, full");
    }
    if o.engine != "lzma" && o.engine != "max" {
        bail!("engine must be 'lzma' or 'max'");
    }
    let dict_cap = o.dict_mb.unwrap_or(64) << 20;
    let membits = membits_for(o.dict_mb);
    let mut st = UltraStats::default();
    let mut ents: Vec<ManEntry> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut groups: HashMap<String, Vec<Item>> = HashMap::new();
    let total_in: u64 = entries.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
    let analyse = Progress::new(total_in, cb);
    let dir = out_path.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let td = tempfile::Builder::new().prefix("zfx-").tempdir_in(&dir)?;

    // ---- 1. analyse: hash, dedup, recompress documents, classify ----
    for e in entries {
        let meta = fs::metadata(&e.path)?;
        let mut rec = ManEntry { n: e.arcname.clone(), m: Some(mtime_secs(&meta)),
                                 p: Some(file_mode(&meta) & 0o7777), ..Default::default() };
        if e.is_dir {
            rec.t = "d".into();
            ents.push(rec);
            st.dirs += 1;
            continue;
        }
        rec.t = "f".into();
        let mut head = vec![0u8; 65536];
        let n = File::open(&e.path)?.read(&mut head)?;
        head.truncate(n);
        let size = meta.len();
        let is_doc = o.docs != "off" && size <= precomp::PRECOMP_MAX as u64
            && (head.starts_with(b"PK\x03\x04") || (o.docs == "full" && head.starts_with(b"%PDF-")));
        let mut data = None;
        if is_doc {
            let d = fs::read(&e.path)?;
            rec.s = Some(d.len() as u64);
            rec.h = Some(sha_hex(Sha256::new_with_prefix(&d)));
            data = Some(d);
        } else {
            rec.s = Some(size);
            rec.h = Some(hash_file(&e.path)?);
        }
        let (sz, h) = (rec.s.unwrap(), rec.h.clone().unwrap());
        st.files += 1;
        st.input += sz;
        analyse.advance(sz, &format!("[analyse] {}", e.arcname));

        if o.dedup
            && let Some(&prev) = seen.get(&h)
                && ents[prev].s == Some(sz) {
                    rec.d = Some(prev);
                    st.duplicates += 1;
                    st.dup_bytes += sz;
                    ents.push(rec);
                    continue;
                }
        seen.insert(h.clone(), ents.len());

        let (mut payload, mut plen, mut transformed, mut cls) = (e.path.clone(), sz, false, None);
        if let Some(d) = data.take()
            && let Some((blob, exact)) = precomp::precompress(&d, &o.docs) {
                payload = td.path().join(format!("pc{}", ents.len()));
                fs::write(&payload, &blob)?;
                plen = blob.len() as u64;
                transformed = true;
                cls = Some("doc");
                rec.x = Some(if exact { "pc" } else { "pc~" }.into());
                st.docs += 1;
                st.docs_exact += exact as usize;
            }
        let cls = cls.unwrap_or_else(|| classify(&e.arcname, &head));
        if plen == 0 {
            rec.b = Some(-1);
            rec.o = Some(0);
            rec.l = Some(0);
        } else {
            groups.entry(cls.to_string()).or_default().push(Item {
                idx: ents.len(), name: e.arcname.clone(), payload, plen, sha: h, transformed, off: 0,
            });
        }
        ents.push(rec);
    }

    // ---- 2. groups (Max: split by file type), filter choice, parallel compression ----
    if o.engine == "max" {
        for g in ["text", "doc", "binary"] {
            let Some(items) = groups.remove(g) else { continue };
            let mut byext: Vec<(String, Vec<Item>)> = Vec::new();
            for it in items {
                let ext = { let x = ext_lower(&it.name); if x.is_empty() { "(none)".to_string() } else { x } };
                match byext.iter_mut().find(|(e, _)| *e == ext) {
                    Some((_, v)) => v.push(it),
                    None => byext.push((ext, vec![it])),
                }
            }
            let many = byext.len() > 1;
            let mut rest = Vec::new();
            for (ext, its) in byext {
                if many && its.iter().map(|i| i.plen).sum::<u64>() >= SPLIT_MIN {
                    groups.insert(format!("{g}:{ext}"), its);
                } else {
                    rest.extend(its);
                }
            }
            groups.insert(g.to_string(), rest);
        }
    }
    let mut order: Vec<String> = groups.iter().filter(|(_, v)| !v.is_empty()).map(|(k, _)| k.clone()).collect();
    order.sort_by_key(|g| {
        let base = g.split(':').next().unwrap();
        (GROUP_ORDER.iter().position(|x| *x == base).unwrap_or(99), g.clone())
    });
    for g in &order {
        groups.get_mut(g).unwrap().sort_by_key(|i| {
            let p = Path::new(&i.name);
            (ext_lower(&i.name), p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default(),
             i.name.clone())
        });
    }
    if let Some(cb) = cb {
        cb("[tuning filters]", 0, 1);
    }
    let plan: Vec<(String, String, Spec)> = order.iter().map(|g| {
        let (name, spec) = choose_filters(g.split(':').next().unwrap(), &groups[g], o.level, dict_cap, &o.engine);
        (g.clone(), name, spec)
    }).collect();

    let total: u64 = order.iter().map(|g| groups[g].iter().map(|i| i.plen).sum::<u64>()).sum();
    let prog = Progress::new(total, cb);
    let biggest = order.iter().map(|g| pow2(groups[g].iter().map(|i| i.plen).sum(), 1 << 16, dict_cap) as u64)
        .max().unwrap_or(1);
    let mut mem_per = biggest * 11 + (64 << 20);
    if plan.iter().any(|p| matches!(p.2, Spec::Cm { .. })) {
        mem_per = mem_per.max((40u64 << membits) + (64 << 20));
    }
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let workers = o.threads.unwrap_or_else(|| order.len().min(cpus).min(((2u64 << 30) / mem_per) as usize).max(1));

    let group_items: Vec<Mutex<Vec<Item>>> = order.iter().map(|g| Mutex::new(groups.remove(g).unwrap())).collect();
    let results: Vec<Mutex<Option<Result<(PathBuf, u64)>>>> = order.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..workers.max(1) {
            s.spawn(|| loop {
                let gi = next.fetch_add(1, Ordering::SeqCst);
                if gi >= plan.len() {
                    break;
                }
                let mut items = group_items[gi].lock().unwrap();
                let r = compress_group(gi, &plan[gi].2, &mut items, td.path(), membits, &prog);
                *results[gi].lock().unwrap() = Some(r);
            });
        }
    });

    // ---- 3. assemble ----
    let mut blocks = Vec::new();
    let mut fo = BufWriter::new(File::create(out_path)?);
    fo.write_all(MAGIC)?;
    let mut offset = MAGIC.len() as u64;
    for (gi, (g, fname, spec)) in plan.iter().enumerate() {
        let (path, usize_) = results[gi].lock().unwrap().take().unwrap()?;
        let c = io::copy(&mut File::open(&path)?, &mut fo)?;
        let mut blk = Block { o: offset, c, u: usize_, k: g.clone(), f: fname.clone(), z: None, mb: None, e8: None };
        if let Spec::Cm { e8 } = spec {
            blk.z = Some("cm".into());
            blk.mb = Some(membits);
            blk.e8 = Some(*e8);
        }
        let items = group_items[gi].lock().unwrap();
        for it in items.iter() {
            ents[it.idx].b = Some(gi as i64);
            ents[it.idx].o = Some(it.off);
            ents[it.idx].l = Some(it.plen);
        }
        st.groups.push(GroupStat { kind: g.clone(), filter: fname.clone(), files: items.len(), input: usize_, output: c });
        offset += c;
        blocks.push(blk);
    }
    let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).ok();
    let man = Manifest { v: 1, zlib: Some(ZLIB_FLAVOR.into()), created, level: Some(o.level), blocks, entries: ents };
    let json = serde_json::to_vec(&man)?;
    let mut enc = liblzma::write::XzEncoder::new(Vec::new(), 9);
    enc.write_all(&json)?;
    let mbytes = enc.finish()?;
    fo.write_all(&mbytes)?;
    fo.write_all(&offset.to_le_bytes())?;
    fo.write_all(&(mbytes.len() as u64).to_le_bytes())?;
    fo.write_all(TRAILER_MAGIC)?;
    fo.flush()?;
    st.output = offset + mbytes.len() as u64 + TRAILER_LEN;
    Ok(st)
}

fn compress_group(gi: usize, spec: &Spec, items: &mut [Item], td: &Path, membits: i32,
                  prog: &Progress) -> Result<(PathBuf, u64)> {
    let path = td.join(format!("blk{gi}"));
    let out = BufWriter::new(File::create(&path)?);
    let mut sink = match spec {
        Spec::Lzma(chain) => BlockSink::Xz(crate::tarfmt::xz_writer(out, chain)?),
        Spec::Cm { e8 } => BlockSink::Cm {
            model: zfcm::Model::new(membits).ok_or_else(|| Error("not enough memory for the Max engine".into()))?,
            e8: *e8, pend: Vec::new(), out, tmp: Vec::new(),
        },
    };
    let mut off = 0u64;
    let mut buf = vec![0u8; CHUNK];
    for it in items.iter_mut() {
        it.off = off;
        let mut h = if it.transformed { None } else { Some(Sha256::new()) };
        let mut f = File::open(&it.payload)?;
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if let Some(h) = h.as_mut() {
                h.update(&buf[..n]);
            }
            sink.write(&buf[..n], prog, &it.name)?;
            off += n as u64;
        }
        if off - it.off != it.plen || h.is_some_and(|h| sha_hex(h) != it.sha) {
            bail!("{} changed while being compressed", it.name);
        }
    }
    sink.finish(prog)?;
    Ok((path, off))
}

// ---------------------------------------------------------------------------
// reader
// ---------------------------------------------------------------------------
pub fn is_zfx(p: &Path) -> bool {
    let mut m = [0u8; 4];
    File::open(p).and_then(|mut f| f.read_exact(&mut m)).is_ok() && &m == MAGIC
}

pub fn read_manifest(path: &Path) -> Result<Manifest> {
    let mut f = File::open(path)?;
    let mut m = [0u8; 4];
    if f.read_exact(&mut m).is_err() || &m != MAGIC {
        bail!("not a .zfx archive");
    }
    let end = f.seek(SeekFrom::End(0))?;
    if end < 4 + TRAILER_LEN {
        bail!("damaged .zfx: too short");
    }
    f.seek(SeekFrom::Start(end - TRAILER_LEN))?;
    let mut t = [0u8; 24];
    f.read_exact(&mut t)?;
    let mo = u64::from_le_bytes(t[0..8].try_into().unwrap());
    let ml = u64::from_le_bytes(t[8..16].try_into().unwrap());
    if &t[16..24] != TRAILER_MAGIC || mo.checked_add(ml).is_none_or(|e| e > end - TRAILER_LEN) {
        bail!("damaged .zfx: index missing (incomplete download?)");
    }
    f.seek(SeekFrom::Start(mo))?;
    let mut comp = vec![0u8; ml as usize];
    f.read_exact(&mut comp)?;
    let mut json = Vec::new();
    liblzma::read::XzDecoder::new(&comp[..]).take(MAX_MANIFEST + 1).read_to_end(&mut json)
        .map_err(|e| Error(format!("damaged .zfx index: {e}")))?;
    if json.len() as u64 > MAX_MANIFEST {
        bail!("damaged .zfx: index too large");
    }
    let man: Manifest = serde_json::from_slice(&json).map_err(|e| Error(format!("damaged .zfx index: {e}")))?;
    if man.v != 1 {
        bail!("unsupported .zfx version {}", man.v);
    }
    Ok(man)
}

pub fn list_zfx(path: &Path) -> Result<Vec<crate::Entry>> {
    Ok(read_manifest(path)?.entries.iter().map(|e| crate::Entry {
        name: if e.t == "d" { format!("{}/", e.n) } else { e.n.clone() },
        size: e.s,
        packed: None,
        mtime: e.m,
        is_dir: e.t == "d",
    }).collect())
}

/// Decodes framed context-mixing segments.
struct CmReader {
    src: io::Take<BufReader<File>>,
    model: Box<zfcm::Model>,
    e8: bool,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for CmReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos >= self.buf.len() {
            let mut hdr = [0u8; 8];
            match self.src.read_exact(&mut hdr) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(0),
                Err(e) => return Err(e),
            }
            let ulen = u32::from_le_bytes(hdr[0..4].try_into().unwrap()) as usize;
            let clen = u32::from_le_bytes(hdr[4..8].try_into().unwrap()) as usize;
            if ulen > zfcm::SEGMENT || clen > zfcm::bound(zfcm::SEGMENT) {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "damaged Max-engine segment"));
            }
            let mut comp = vec![0u8; clen];
            self.src.read_exact(&mut comp)?;
            self.buf = vec![0u8; ulen];
            self.model.decode_segment(&comp, &mut self.buf);
            if self.e8 {
                zfcm::e8e9(&mut self.buf, false);
            }
            self.pos = 0;
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn block_reader(path: &Path, blk: &Block) -> Result<Box<dyn Read>> {
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(blk.o))?;
    let src = BufReader::new(f).take(blk.c);
    if blk.z.as_deref() == Some("cm") {
        let mb = blk.mb.unwrap_or(22);
        if !(16..=24).contains(&mb) {
            bail!("damaged .zfx: invalid Max-engine memory setting {mb}");
        }
        let model = zfcm::Model::new(mb)
            .ok_or_else(|| Error("not enough memory for the Max engine".into()))?;
        Ok(Box::new(CmReader { src, model, e8: blk.e8.unwrap_or(false), buf: Vec::new(), pos: 0 }))
    } else {
        Ok(Box::new(liblzma::read::XzDecoder::new(src)))
    }
}

#[derive(Debug, Default)]
pub struct ExtractStats {
    pub files: usize,
    pub equivalent: usize,
    pub rebuilt: usize,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

fn part(t: &Path) -> PathBuf {
    let mut s = t.as_os_str().to_owned();
    s.push(".zfpart");
    PathBuf::from(s)
}

/// Receives one stored payload and writes it to every target (dedup).
struct Out<'a> {
    ent: &'a ManEntry,
    targets: Vec<(PathBuf, &'a ManEntry)>,
    dest: Option<&'a Path>,
    active: bool,
    buf: Option<Vec<u8>>,
    hasher: Sha256,
    files: Vec<BufWriter<File>>,
    guard: Option<DiskGuard>,
}

impl<'a> Out<'a> {
    fn new(ent: &'a ManEntry, targets: Vec<(PathBuf, &'a ManEntry)>, dest: Option<&'a Path>,
           verify: bool) -> Result<Out<'a>> {
        let active = verify || !targets.is_empty();
        if ent.x.is_some() && ent.l.unwrap_or(0) > MAX_DOC_BLOB {
            bail!("damaged .zfx: document data for {} is impossibly large", ent.n);
        }
        let buf = if ent.x.is_some() && active { Some(Vec::new()) } else { None };
        let mut files = Vec::new();
        if buf.is_none() && active {
            for (t, _) in &targets {
                files.push(BufWriter::new(create_file_safe(dest.unwrap(), &part(t))?));
            }
        }
        Ok(Out { ent, targets, dest, active, buf, hasher: Sha256::new(), files,
                 guard: dest.map(DiskGuard::new) })
    }

    fn close(mut self, man: &Manifest) -> Result<(&'static str, String)> {
        if !self.active {
            return Ok(("skip", String::new()));
        }
        let want = self.ent.h.clone().unwrap_or_default();
        let (mut status, mut why) = ("ok", String::new());
        if let Some(blob) = self.buf.take() {
            match precomp::restore(&blob) {
                Err(e) => { status = "bad"; why = format!("cannot rebuild: {e}"); }
                Ok((data, ctype)) => {
                    if sha_hex(Sha256::new_with_prefix(&data)) != want {
                        if ctype == b'Z' && precomp::zip_equivalent(None, &data) {
                            // "pc~" = rebuilt by design (--docs full); "pc" = zlib mismatch
                            status = if self.ent.x.as_deref() == Some("pc~") { "rebuilt" } else { "equivalent" };
                        } else {
                            status = "bad";
                            why = format!("cannot rebuild byte-exactly (archive zlib {})",
                                          man.zlib.clone().unwrap_or_default());
                        }
                    }
                    if status != "bad" {
                        for (t, _) in &self.targets {
                            create_file_safe(self.dest.unwrap(), &part(t))?.write_all(&data)?;
                        }
                    }
                }
            }
        } else {
            for mut f in self.files.drain(..) {
                f.flush()?;
            }
            if sha_hex(self.hasher.clone()) != want {
                status = "bad";
                why = "checksum mismatch (archive is damaged)".into();
            }
        }
        for (t, e) in &self.targets {
            if status == "bad" {
                let _ = fs::remove_file(part(t));
            } else {
                fs::rename(part(t), t)?;
                if let Some(m) = e.m {
                    set_mtime(t, m);
                }
                if let Some(p) = e.p {
                    set_mode(t, p);
                }
            }
        }
        Ok((status, why))
    }
}

impl Write for Out<'_> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if !self.active {
            return Ok(b.len());
        }
        if let Some(g) = self.guard.as_mut() {
            g.tick(b.len() as u64)?;
        }
        if let Some(buf) = self.buf.as_mut() {
            if (buf.len() + b.len()) as u64 > MAX_DOC_BLOB {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "document data too large"));
            }
            buf.extend_from_slice(b);
        } else {
            self.hasher.update(b);
            for f in &mut self.files {
                f.write_all(b)?;
            }
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Extract into `dest`; `dest = None` only verifies (used by `test`).
pub fn extract_zfx(path: &Path, dest: Option<&Path>, overwrite: bool, cb: Option<&ProgressFn>) -> Result<ExtractStats> {
    let man = read_manifest(path)?;
    let ents = &man.entries;
    let mut st = ExtractStats::default();
    let mut targets: HashMap<usize, PathBuf> = HashMap::new();
    if let Some(dest) = dest {
        for (i, e) in ents.iter().enumerate() {
            let Some(t) = safe_target(dest, &e.n) else {
                let w = format!("blocked unsafe path: {}", e.n);
                eprintln!("warning: {w}");
                st.warnings.push(w);
                continue;
            };
            if e.t == "d" {
                create_dir_safe(dest, &t)?;
                continue;
            }
            if t.exists() && !overwrite {
                continue;
            }
            targets.insert(i, t);
        }
    }
    let mut sinks: HashMap<usize, Vec<(PathBuf, &ManEntry)>> = HashMap::new();
    let mut tlist: Vec<_> = targets.into_iter().collect();
    tlist.sort_by_key(|(i, _)| *i);
    for (i, t) in tlist {
        sinks.entry(ents[i].d.unwrap_or(i)).or_default().push((t, &ents[i]));
    }
    let verify = dest.is_none();
    let record = |e: &ManEntry, (status, why): (&str, String), st: &mut ExtractStats| match status {
        "skip" => {}
        "bad" => st.errors.push(format!("{}: {why}", e.n)),
        s => {
            st.files += 1;
            if s == "equivalent" { st.equivalent += 1; }
            if s == "rebuilt" { st.rebuilt += 1; }
        }
    };
    // empty files
    for (i, e) in ents.iter().enumerate() {
        if e.t == "f" && e.d.is_none() && e.l.unwrap_or(0) == 0 {
            let out = Out::new(e, sinks.remove(&i).unwrap_or_default(), dest, verify)?;
            let r = out.close(&man)?;
            record(e, r, &mut st);
        }
    }
    let total: u64 = man.blocks.iter().map(|b| b.u).sum();
    let prog = Progress::new(total, cb);
    for (bi, blk) in man.blocks.iter().enumerate() {
        let mut members: Vec<(u64, usize)> = ents.iter().enumerate()
            .filter(|(_, e)| e.t == "f" && e.d.is_none() && e.b == Some(bi as i64) && e.l.unwrap_or(0) > 0)
            .map(|(i, e)| (e.o.unwrap_or(0), i))
            .collect();
        members.sort();
        if !verify && !members.iter().any(|(_, i)| sinks.contains_key(i)) {
            prog.advance(blk.u, "");
            continue; // nothing wanted from this block
        }
        let mut rd = block_reader(path, blk)?;
        let mut pos = 0u64;
        for (off, i) in members {
            let e = &ents[i];
            if off > pos {
                io::copy(&mut (&mut rd).take(off - pos), &mut io::sink())?;
                pos = off;
            }
            let l = e.l.unwrap_or(0);
            let mut out = Out::new(e, sinks.remove(&i).unwrap_or_default(), dest, verify)?;
            let got = copy_progress(&mut (&mut rd).take(l), &mut out, &prog, &e.n)
                .map_err(|err| Error(format!("damaged .zfx block {bi}: {err}")))?;
            pos += got;
            if got < l {
                st.errors.push(format!("block {bi} ended early ({} lost)", e.n));
                drop(out);
                break;
            }
            let r = out.close(&man)?;
            record(e, r, &mut st);
        }
    }
    if st.equivalent > 0 {
        let w = format!("{} document(s) rebuilt with identical content but different bytes \
                         (archive zlib {}, this program {ZLIB_FLAVOR})", st.equivalent,
                        man.zlib.clone().unwrap_or_default());
        eprintln!("warning: {w}");
        st.warnings.push(w);
    }
    Ok(st)
}
