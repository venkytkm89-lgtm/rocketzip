//! Document recompression: inflate the Deflate streams inside ZIP-based
//! documents (DOCX/XLSX/PPTX/ODT/EPUB/JAR...) and PDFs so LZMA2/CM can compress
//! the raw content, then reproduce the original bytes exactly on extraction.
//! The blob format is identical to the Python version's (`ultra.py`).
use crate::engines::{self, Family, Deflater, Z_FINISH, Z_FULL_FLUSH, Z_SYNC_FLUSH, PROBE};
use crate::util::{err, Error, Result};
use flate2::{Decompress, FlushDecompress, Status};
use std::io::Cursor;

pub const PRECOMP_MAX: usize = 256 << 20;
/// Most a single document may expand to (hostile "zip bomb" documents).
pub const MAX_INFLATED_TOTAL: usize = 1 << 30;
const RAW: u8 = 0;
const DEFLATE_EXACT: u8 = 1;
const ZLIB_EXACT: u8 = 2;
const DEFLATE_REBUILD: u8 = 3;
const DEFLATE_BLOCKED: u8 = 4;
const BLOCK_SIZES: [usize; 4] = [128 << 10, 64 << 10, 32 << 10, 256 << 10];
const PC_MAGIC: &[u8] = b"ZFPC";
const SEG_HDR: usize = 27; // <BBBQQQ

pub struct Seg {
    pub kind: u8,
    pub lv: u8,
    pub mem: u8,
    pub lh: u64,
    pub olen: u64,
    pub payload: Vec<u8>,
}

fn params() -> Vec<(i32, i32)> {
    let mut v = Vec::new();
    for mem in [8, 9] {
        for lv in [6, 9, 1, 5, 4, 3, 2, 7, 8] {
            v.push((lv, mem));
        }
    }
    v
}

fn u16le(d: &[u8], p: usize) -> Option<u64> {
    d.get(p..p + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as u64)
}
fn u32le(d: &[u8], p: usize) -> Option<u64> {
    d.get(p..p + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as u64)
}
fn u64le(d: &[u8], p: usize) -> Option<u64> {
    d.get(p..p + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
}

/// Inflate one stream. Returns (data, bytes of `raw` consumed) if it ended cleanly.
pub fn inflate(raw: &[u8], zlib_header: bool) -> Option<(Vec<u8>, usize)> {
    let mut d = Decompress::new(zlib_header);
    let mut out: Vec<u8> = Vec::with_capacity((raw.len() * 4).min(PRECOMP_MAX).max(1024));
    loop {
        if out.len() >= PRECOMP_MAX {
            return None;
        }
        if out.capacity() - out.len() < 1 << 16 {
            out.reserve(out.len().max(1 << 16));
        }
        let (bi, bo) = (d.total_in(), d.total_out());
        let st = d.decompress_vec(&raw[d.total_in() as usize..], &mut out, FlushDecompress::None).ok()?;
        if st == Status::StreamEnd {
            return Some((out, d.total_in() as usize));
        }
        if d.total_in() == bi && d.total_out() == bo {
            return None; // truncated or not a stream
        }
    }
}

/// Find (level, memlevel, family) that reproduce `raw` exactly from `data`.
pub fn find_params(raw: &[u8], data: &[u8], wbits: i32, families: &[Family]) -> Option<(i32, i32, Family)> {
    for &fam in families {
        for (lv, mem) in params() {
            let mut c = Deflater::new(fam, lv, wbits, mem, 0, None)?;
            let cut = data.len().min(PROBE);
            let mut out = c.compress(&data[..cut]);
            if !out.is_empty() && !raw.starts_with(&out) {
                continue; // cheap early reject
            }
            out.extend(c.compress(&data[cut..]));
            out.extend(c.flush(Z_FINISH));
            if out == raw {
                return Some((lv, mem, fam));
            }
        }
    }
    None
}

/// Block-parallel Deflate (LibreOffice / pigz): (level, mem, family, block index, flush).
fn find_blocked(raw: &[u8], data: &[u8]) -> Option<(i32, i32, Family, usize, i32)> {
    if data.len() <= BLOCK_SIZES[2] || count_sync(raw) == 0 {
        return None;
    }
    for fam in Family::ALL {
        for (bi, &bs) in BLOCK_SIZES.iter().enumerate() {
            if data.len() <= bs {
                continue;
            }
            for flush in [Z_SYNC_FLUSH, Z_FULL_FLUSH] {
                for (lv, mem) in params() {
                    let mut c = Deflater::new(fam, lv, -15, mem, 0, None)?;
                    let mut head = c.compress(&data[..bs]);
                    head.extend(c.flush(flush));
                    if !raw.starts_with(&head) {
                        continue;
                    }
                    if engines::deflate_blocked(data, lv, mem, fam, bs, flush).as_deref() == Some(raw) {
                        return Some((lv, mem, fam, bi, flush));
                    }
                }
            }
        }
    }
    None
}

fn count_sync(raw: &[u8]) -> usize {
    raw.windows(4).filter(|w| *w == [0, 0, 0xff, 0xff]).count()
}

struct CdEnt {
    method: u64,
    flags: u64,
    csize: u64,
    lho: u64,
}

/// Minimal ZIP central-directory reader (handles ZIP64 and prefixed data).
fn parse_cd(d: &[u8]) -> Option<Vec<CdEnt>> {
    let lo = d.len().saturating_sub(65557);
    let eocd = (lo..d.len().saturating_sub(21)).rev().find(|&i| &d[i..i + 4] == b"PK\x05\x06")?;
    let mut count = u16le(d, eocd + 10)?;
    let cd_size = u32le(d, eocd + 12)?;
    let mut cd_off = u32le(d, eocd + 16)?;
    let mut concat = (eocd as i64) - cd_size as i64 - cd_off as i64;
    if count == 0xFFFF || cd_off == 0xFFFF_FFFF {
        let loc = eocd.checked_sub(20)?;
        if &d[loc..loc + 4] != b"PK\x06\x07" {
            return None;
        }
        let z = u64le(d, loc + 8)? as usize;
        if d.get(z..z + 4)? != b"PK\x06\x06" {
            return None;
        }
        count = u64le(d, z + 32)?;
        let size64 = u64le(d, z + 40)?;
        cd_off = u64le(d, z + 48)?;
        concat = (z as i64) - size64 as i64 - cd_off as i64;
    }
    let concat = concat.max(0) as u64;
    let mut p = (cd_off + concat) as usize;
    let mut out = Vec::new();
    for _ in 0..count {
        if d.get(p..p + 4)? != b"PK\x01\x02" {
            return None;
        }
        let flags = u16le(d, p + 8)?;
        let method = u16le(d, p + 10)?;
        let mut csize = u32le(d, p + 20)?;
        let usize_ = u32le(d, p + 24)?;
        let (n, m, k) = (u16le(d, p + 28)? as usize, u16le(d, p + 30)? as usize, u16le(d, p + 32)? as usize);
        let mut lho = u32le(d, p + 42)?;
        if let Some(mut q) = find_extra(d, p + 46 + n, m, 1) {
            if usize_ == 0xFFFF_FFFF { q += 8; }
            if csize == 0xFFFF_FFFF { csize = u64le(d, q)?; q += 8; }
            if lho == 0xFFFF_FFFF { lho = u64le(d, q)?; }
        }
        out.push(CdEnt { method, flags, csize, lho: lho + concat });
        p += 46 + n + m + k;
    }
    Some(out)
}

fn find_extra(buf: &[u8], start: usize, length: usize, id: u64) -> Option<usize> {
    let (mut p, end) = (start, start + length);
    while p + 4 <= end {
        let hid = u16le(buf, p)?;
        let size = u16le(buf, p + 2)? as usize;
        if hid == id {
            return Some(p + 4);
        }
        p += 4 + size;
    }
    None
}

fn raw_seg(slice: &[u8]) -> Seg {
    Seg { kind: RAW, lv: 0, mem: 0, lh: 0, olen: slice.len() as u64, payload: slice.to_vec() }
}

fn precomp_zip(data: &[u8], rebuild_ok: bool) -> Option<Vec<Seg>> {
    let mut ents = parse_cd(data)?;
    ents.sort_by_key(|e| e.lho);
    let (mut segs, mut pos, mut hits) = (Vec::new(), 0usize, 0);
    let mut inflated = 0usize;
    for zi in ents {
        if zi.method != 8 || zi.flags & 1 != 0 || zi.csize < 128 {
            continue;
        }
        let lh = zi.lho as usize;
        if lh < pos || data.get(lh..lh + 4) != Some(b"PK\x03\x04") {
            continue;
        }
        let (Some(n), Some(m)) = (u16le(data, lh + 26), u16le(data, lh + 28)) else { continue };
        let start = lh + 30 + n as usize + m as usize;
        let end = start + zi.csize as usize;
        if end > data.len() {
            continue;
        }
        let raw = &data[start..end];
        let Some((out, used)) = inflate(raw, false) else { continue };
        if used != raw.len() {
            continue;
        }
        inflated += out.len();
        if inflated > MAX_INFLATED_TOTAL {
            return None; // expands absurdly: store the document as it is
        }
        let blocky = count_sync(raw) >= 1.max(out.len() / (256 << 10));
        let pb = if blocky { find_blocked(raw, &out) } else { None };
        let p = if pb.is_some() { None } else { find_params(raw, &out, -15, &Family::ALL) };
        let pb = pb.or_else(|| if !blocky && p.is_none() { find_blocked(raw, &out) } else { None });
        let seg = if let Some((lv, mem, fam)) = p {
            Seg { kind: DEFLATE_EXACT, lv: lv as u8 | fam.id() << 4, mem: mem as u8, lh: lh as u64,
                  olen: raw.len() as u64, payload: out }
        } else if let Some((lv, mem, fam, bi, flush)) = pb {
            Seg { kind: DEFLATE_BLOCKED, lv: lv as u8 | fam.id() << 4,
                  mem: mem as u8 | (bi as u8) << 4 | if flush == Z_FULL_FLUSH { 64 } else { 0 },
                  lh: lh as u64, olen: raw.len() as u64, payload: out }
        } else if rebuild_ok {
            Seg { kind: DEFLATE_REBUILD, lv: 9 | Family::Classic.id() << 4, mem: 9, lh: lh as u64,
                  olen: raw.len() as u64, payload: out }
        } else {
            continue;
        };
        segs.push(raw_seg(&data[pos..start]));
        segs.push(seg);
        pos = end;
        hits += 1;
    }
    if hits == 0 {
        return None;
    }
    segs.push(raw_seg(&data[pos..]));
    Some(segs)
}

fn precomp_pdf(data: &[u8]) -> Option<Vec<Seg>> {
    let (mut segs, mut pos, mut hits) = (Vec::new(), 0usize, 0);
    let mut i = 0usize;
    while let Some(off) = memfind(&data[i..], b"stream") {
        let at = i + off;
        i = at + 6;
        if at >= 3 && &data[at - 3..at] == b"end" {
            continue;
        }
        let start = if data.get(at + 6..at + 8) == Some(b"\r\n") {
            at + 8
        } else if data.get(at + 6) == Some(&b'\n') {
            at + 7
        } else {
            continue;
        };
        if start < pos || start + 2 > data.len() {
            continue;
        }
        let (cmf, flg) = (data[start] as u32, data[start + 1] as u32);
        if cmf & 0x0F != 8 || (cmf * 256 + flg) % 31 != 0 {
            continue;
        }
        let Some(stop) = memfind(&data[start..], b"endstream").map(|s| start + s) else { break };
        let Some((out, used)) = inflate(&data[start..stop], true) else { continue };
        if used < 64 {
            continue;
        }
        let raw = &data[start..start + used];
        let Some((lv, mem, fam)) = find_params(raw, &out, 15, &Family::ALL) else { continue };
        segs.push(raw_seg(&data[pos..start]));
        segs.push(Seg { kind: ZLIB_EXACT, lv: lv as u8 | fam.id() << 4, mem: mem as u8, lh: 0,
                        olen: used as u64, payload: out });
        pos = start + used;
        i = pos;
        hits += 1;
    }
    if hits == 0 {
        return None;
    }
    segs.push(raw_seg(&data[pos..]));
    Some(segs)
}

fn memfind(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

pub fn encode(ctype: u8, segs: &[Seg]) -> Vec<u8> {
    let mut out = Vec::with_capacity(segs.iter().map(|s| s.payload.len() + SEG_HDR).sum::<usize>() + 5);
    out.extend_from_slice(PC_MAGIC);
    out.push(ctype);
    for s in segs {
        out.extend_from_slice(&[s.kind, s.lv, s.mem]);
        out.extend_from_slice(&s.lh.to_le_bytes());
        out.extend_from_slice(&s.olen.to_le_bytes());
        out.extend_from_slice(&(s.payload.len() as u64).to_le_bytes());
        out.extend_from_slice(&s.payload);
    }
    out
}

pub fn decode(blob: &[u8]) -> Result<(u8, Vec<Seg>)> {
    if blob.len() < 5 || &blob[..4] != PC_MAGIC {
        return err("bad recompressed document");
    }
    let ctype = blob[4];
    let (mut p, mut segs) = (5usize, Vec::new());
    while p < blob.len() {
        let h = blob.get(p..p + SEG_HDR).ok_or_else(|| Error("truncated document blob".into()))?;
        let lh = u64::from_le_bytes(h[3..11].try_into().unwrap());
        let olen = u64::from_le_bytes(h[11..19].try_into().unwrap());
        let plen = u64::from_le_bytes(h[19..27].try_into().unwrap()) as usize;
        p += SEG_HDR;
        let payload = blob.get(p..p + plen).ok_or_else(|| Error("truncated document blob".into()))?.to_vec();
        p += plen;
        segs.push(Seg { kind: h[0], lv: h[1], mem: h[2], lh, olen, payload });
    }
    Ok((ctype, segs))
}

/// (orig start, orig len, new len, new start, local header offset)
type Change = (u64, u64, u64, u64, u64);

pub fn rebuild(ctype: u8, segs: &[Seg]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut changes: Vec<Change> = Vec::new();
    let mut opos = 0u64;
    for s in segs {
        let fam = Family::from_id(s.lv >> 4);
        let new = match s.kind {
            RAW => {
                out.extend_from_slice(&s.payload);
                opos += s.olen;
                continue;
            }
            DEFLATE_BLOCKED => engines::deflate_blocked(
                &s.payload, (s.lv & 15) as i32, (s.mem & 15) as i32, fam,
                BLOCK_SIZES[((s.mem >> 4) & 3) as usize],
                if s.mem & 64 != 0 { Z_FULL_FLUSH } else { Z_SYNC_FLUSH }),
            ZLIB_EXACT => engines::deflate(&s.payload, (s.lv & 15) as i32, s.mem as i32, 15, fam),
            DEFLATE_EXACT | DEFLATE_REBUILD => engines::deflate(&s.payload, (s.lv & 15) as i32, s.mem as i32, -15, fam),
            k => return err(format!("unknown document segment type {k}")),
        }
        .ok_or_else(|| Error("invalid compression settings in document data".into()))?;
        changes.push((opos, s.olen, new.len() as u64, out.len() as u64, s.lh));
        out.extend_from_slice(&new);
        opos += s.olen;
    }
    if ctype == b'Z' && changes.iter().any(|c| c.1 != c.2) {
        patch_zip(&mut out, &changes).ok_or_else(|| Error("could not rebuild ZIP structure".into()))?;
    }
    Ok(out)
}

fn put32(b: &mut [u8], p: usize, v: u64) -> Option<()> {
    b.get_mut(p..p + 4)?.copy_from_slice(&(v as u32).to_le_bytes());
    Some(())
}
fn put64(b: &mut [u8], p: usize, v: u64) -> Option<()> {
    b.get_mut(p..p + 8)?.copy_from_slice(&v.to_le_bytes());
    Some(())
}

/// Fix sizes and offsets after re-deflated members changed length.
fn patch_zip(buf: &mut [u8], changes: &[Change]) -> Option<()> {
    let deltas: Vec<(u64, i64)> = changes.iter().map(|c| (c.0 + c.1, c.2 as i64 - c.1 as i64)).collect();
    let mapo = |x: u64| -> u64 {
        (x as i64 + deltas.iter().filter(|(end, _)| *end <= x).map(|(_, d)| d).sum::<i64>()) as u64
    };
    let mut new_size = std::collections::HashMap::new();
    for &(_, ol, nl, ns, lh) in changes {
        if nl == ol {
            continue;
        }
        new_size.insert(lh, nl);
        let nlh = mapo(lh) as usize;
        let flags = u16le(buf, nlh + 6)?;
        let csz = u32le(buf, nlh + 18)?;
        let (n, m) = (u16le(buf, nlh + 26)? as usize, u16le(buf, nlh + 28)? as usize);
        let z64 = find_extra(buf, nlh + 30 + n, m, 1);
        if csz == 0xFFFF_FFFF && z64.is_some() {
            put64(buf, z64? + 8, nl)?;
        } else if csz != 0 || flags & 8 == 0 {
            put32(buf, nlh + 18, nl)?;
        }
        if flags & 8 != 0 {
            let mut d = (ns + nl) as usize;
            if buf.get(d..d + 4) == Some(b"PK\x07\x08") {
                d += 4;
            }
            if z64.is_some() { put64(buf, d + 4, nl)?; } else { put32(buf, d + 4, nl)?; }
        }
    }
    let lo = buf.len().saturating_sub(65557);
    let eocd = (lo..buf.len().saturating_sub(21)).rev().find(|&i| &buf[i..i + 4] == b"PK\x05\x06")?;
    let mut count = u16le(buf, eocd + 10)?;
    let mut cd_off = u32le(buf, eocd + 16)?;
    if cd_off == 0xFFFF_FFFF || count == 0xFFFF {
        let loc = eocd.checked_sub(20)?;
        if &buf[loc..loc + 4] != b"PK\x06\x07" {
            return None;
        }
        let z64_rec = mapo(u64le(buf, loc + 8)?);
        put64(buf, loc + 8, z64_rec)?;
        count = u64le(buf, z64_rec as usize + 32)?;
        cd_off = u64le(buf, z64_rec as usize + 48)?;
        put64(buf, z64_rec as usize + 48, mapo(cd_off))?;
    }
    if u32le(buf, eocd + 16)? != 0xFFFF_FFFF {
        put32(buf, eocd + 16, mapo(cd_off))?;
    }
    let mut p = mapo(cd_off) as usize;
    for _ in 0..count {
        if buf.get(p..p + 4)? != b"PK\x01\x02" {
            return None;
        }
        let csz = u32le(buf, p + 20)?;
        let usz = u32le(buf, p + 24)?;
        let (n, m, k) = (u16le(buf, p + 28)? as usize, u16le(buf, p + 30)? as usize, u16le(buf, p + 32)? as usize);
        let mut lho = u32le(buf, p + 42)?;
        let ex = find_extra(buf, p + 46 + n, m, 1);
        let (mut q_cs, mut q_lh) = (None, None);
        if let Some(mut q) = ex {
            if usz == 0xFFFF_FFFF { q += 8; }
            if csz == 0xFFFF_FFFF { q_cs = Some(q); q += 8; }
            if lho == 0xFFFF_FFFF { q_lh = Some(q); lho = u64le(buf, q)?; }
        }
        if let Some(&ns) = new_size.get(&lho) {
            match q_cs { Some(q) => put64(buf, q, ns)?, None => put32(buf, p + 20, ns)? }
        }
        match q_lh { Some(q) => put64(buf, q, mapo(lho))?, None => put32(buf, p + 42, mapo(lho))? }
        p += 46 + n + m + k;
    }
    Some(())
}

/// Valid ZIP whose members' CRCs check out (and match the original's).
pub fn zip_equivalent(original: Option<&[u8]>, rebuilt: &[u8]) -> bool {
    fn members(d: &[u8], verify: bool) -> Option<Vec<(String, u32, u64)>> {
        let mut za = zip::ZipArchive::new(Cursor::new(d)).ok()?;
        let mut v = Vec::new();
        for i in 0..za.len() {
            let mut f = if verify { za.by_index(i).ok()? } else { za.by_index_raw(i).ok()? };
            let meta = (f.name().ok()?.into_owned(), f.crc32(), f.size());
            if verify && !f.is_dir() {
                std::io::copy(&mut f, &mut std::io::sink()).ok()?; // CRC checked at EOF
            }
            v.push(meta);
        }
        Some(v)
    }
    let Some(a) = members(rebuilt, true) else { return false };
    match original {
        Some(o) => members(o, false).is_some_and(|b| a == b),
        None => true,
    }
}

/// Returns (blob, exact) or None if the file doesn't benefit.
pub fn precompress(data: &[u8], mode: &str) -> Option<(Vec<u8>, bool)> {
    if mode == "off" {
        return None;
    }
    if data.starts_with(b"PK\x03\x04") {
        let tries: &[bool] = if mode == "full" { &[true, false] } else { &[false] };
        for &rebuild_ok in tries {
            let Some(segs) = precomp_zip(data, rebuild_ok) else { continue };
            let Ok(rebuilt) = rebuild(b'Z', &segs) else { continue };
            if rebuilt == data {
                return Some((encode(b'Z', &segs), true));
            }
            if rebuild_ok && zip_equivalent(Some(data), &rebuilt) {
                return Some((encode(b'Z', &segs), false));
            }
        }
        return None;
    }
    if data.starts_with(b"%PDF-") && mode == "full" {
        // PDFs have no "equivalent rebuild" fallback, so only in opt-in full mode.
        let segs = precomp_pdf(data)?;
        if rebuild(b'P', &segs).ok()? == data {
            return Some((encode(b'P', &segs), true));
        }
    }
    None
}

pub fn restore(blob: &[u8]) -> Result<(Vec<u8>, u8)> {
    let (ctype, segs) = decode(blob)?;
    Ok((rebuild(ctype, &segs)?, ctype))
}
