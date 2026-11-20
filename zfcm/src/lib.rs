//! RocketZip Max context-mixing engine - Rust port of `zfcm.c`.
//!
//! Produces **byte-identical** output to the C engine, so `.zfx` archives made by
//! either one decode with the other. All arithmetic is integer-only and
//! deterministic on every platform.
//!
//! Built as a `cdylib`, this crate exports the same C ABI as `zfcm.c`
//! (`zfcm_version`, `zfcm_new`, `zfcm_free`, `zfcm_encode`, `zfcm_decode`,
//! `zfcm_e8e9`) under the same file name (`zfcm.dll`, `libzfcm.so`,
//! `libzfcm.dylib`), so RocketZip's Python code loads it unchanged.

use std::os::raw::{c_int, c_longlong, c_void};
use std::sync::OnceLock;

pub const VERSION: c_int = 1;
const LIMIT: i32 = 255;
const MIXLR: i32 = 3;
const MIXSH: u32 = 11;
const WINIT: i32 = 8;
const NCTX: usize = 9;
const NIN: usize = NCTX + 3; // + order0 + match + bias
const MINLEN: u32 = 7;
/// Bytes per coder segment used by RocketZip's framing (model state carries across).
pub const SEGMENT: usize = 1 << 20;

// ---------------------------------------------------------------------------
// logistic helpers (integer, deterministic)
// ---------------------------------------------------------------------------
const SQ: [i32; 33] = [
    1, 2, 3, 6, 10, 16, 27, 45, 73, 120, 194, 310, 488, 747, 1101, 1546, 2047, 2549, 2994, 3348,
    3607, 3785, 3901, 3975, 4022, 4050, 4068, 4079, 4085, 4089, 4092, 4093, 4094,
];

#[inline]
fn squash(d: i32) -> i32 {
    if d > 2047 {
        return 4095;
    }
    if d < -2047 {
        return 1;
    }
    let w = d & 127;
    let i = ((d >> 7) + 16) as usize;
    (SQ[i] * (128 - w) + SQ[i + 1] * w + 64) >> 7
}

struct Tables {
    stretch: [i16; 4096],
    rate: [i32; 256],
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut stretch = [0i16; 4096];
        let mut pi = 0i32;
        for x in -2047..=2047 {
            let v = squash(x);
            for j in pi..=v {
                stretch[j as usize] = x as i16;
            }
            pi = v + 1;
        }
        for j in pi..4096 {
            stretch[j as usize] = 2047;
        }
        let mut rate = [0i32; 256];
        for (n, r) in rate.iter_mut().enumerate() {
            *r = 4096 * 2 / (2 * n as i32 + 3); // 1/(n+1.5)
        }
        Tables { stretch, rate }
    })
}

// ---------------------------------------------------------------------------
// adaptive bit counters
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Slot {
    p: u16, // P(bit = 1) in 1/65536
    n: u8,  // times seen (adaptation speed)
    chk: u8,
}

#[inline]
fn slot_update(s: &mut Slot, y: i32, limit: i32, rate: &[i32; 256]) {
    let target = if y != 0 { 65535 } else { 0 };
    let mut p = s.p as i32;
    p += ((target - p) * rate[s.n as usize]) >> 12;
    s.p = p.clamp(32, 65503) as u16;
    if (s.n as i32) < limit {
        s.n += 1;
    }
}

// ---------------------------------------------------------------------------
// APM / SSE
// ---------------------------------------------------------------------------
struct Apm {
    t: Vec<u16>,
    idx: usize,
}

impl Apm {
    fn new(n: usize) -> Option<Apm> {
        let mut t = try_vec(n * 33, 0u16)?;
        for i in 0..n {
            for j in 0..33 {
                t[i * 33 + j] = (squash((j as i32 - 16) * 128) * 16) as u16;
            }
        }
        Some(Apm { t, idx: 0 })
    }

    #[inline]
    fn p(&mut self, pr: i32, cx: usize, st: &[i16; 4096]) -> i32 {
        let s = st[pr as usize] as i32 + 2048;
        let lo = (s >> 7) as usize;
        let w = s & 127;
        let base = cx * 33 + lo;
        self.idx = base + (w >> 6) as usize;
        (self.t[base] as i32 * (128 - w) + self.t[base + 1] as i32 * w) >> 11
    }

    #[inline]
    fn update(&mut self, y: i32) {
        let g = if y != 0 { 65535 } else { 0 };
        let t = self.t[self.idx] as i32;
        self.t[self.idx] = (t + ((g - t) >> 6)) as u16;
    }
}

fn try_vec<T: Clone>(n: usize, v: T) -> Option<Vec<T>> {
    let mut x = Vec::new();
    x.try_reserve_exact(n).ok()?;
    x.resize(n, v);
    Some(x)
}

#[inline]
fn mix32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h
}

#[inline]
fn hash2(a: u32, b: u32, i: u32) -> u32 {
    mix32(a.wrapping_mul(0x9E37_79B1) ^ mix32(b.wrapping_add(i.wrapping_mul(0x632B_E5AB))) ^ i)
}

// ---------------------------------------------------------------------------
// model
// ---------------------------------------------------------------------------
/// One compression/decompression stream. Model state persists across segments.
pub struct Model {
    tb: &'static Tables,
    bits: u32,
    mask: u32,
    t: Vec<Slot>, // NCTX tables of 2^bits slots, stored back to back
    bucket: [usize; NCTX],
    cur: [usize; NCTX],
    nib: u32,
    o0: Vec<Slot>,
    o0cur: usize,
    h: [u32; NCTX],
    c0: u32,
    c4: u32,
    c8: u32,
    word0: u32,
    word1: u32,
    bpos: i32,
    // match model
    buf: Vec<u8>,
    bufmask: u32,
    pos: u32,
    mtab: Vec<u32>,
    mtmask: u32,
    mptr: u32,
    mlen: u32,
    mexp: i32,
    mm: [Slot; 128],
    mmcur: Option<usize>,
    // mixers
    x: [i32; NIN],
    wa: Vec<i32>,
    wb: Vec<i32>,
    sela: usize,
    selb: usize,
    pra: i32,
    prb: i32,
    a1: Apm,
    a2: Apm,
    pr: i32,
}

impl Model {
    /// `membits` 16..=26 (clamped). Memory ~ 9 * 4 * 2^membits bytes + history.
    /// Returns None if memory can't be allocated.
    pub fn new(membits: i32) -> Option<Box<Model>> {
        let tb = tables();
        let bits = membits.clamp(16, 26) as u32;
        let bufmask = (1u32 << (bits + 2)) - 1;
        let mtmask = (1u32 << bits) - 1;
        let mut mm = [Slot::default(); 128];
        for (i, s) in mm.iter_mut().enumerate() {
            s.p = if i & 1 == 1 { 49152 } else { 16384 };
        }
        let mut m = Box::new(Model {
            tb,
            bits,
            mask: (1u32 << (bits - 4)) - 1, // buckets of 16 slots
            t: try_vec(NCTX << bits, Slot::default())?,
            bucket: [0; NCTX],
            cur: [0; NCTX],
            nib: 0,
            o0: try_vec(256 * 256, Slot { p: 32768, n: 0, chk: 0 })?,
            o0cur: 0,
            h: [0; NCTX],
            c0: 1,
            c4: 0,
            c8: 0,
            word0: 0,
            word1: 0,
            bpos: 0,
            buf: try_vec(bufmask as usize + 1, 0u8)?,
            bufmask,
            pos: 0,
            mtab: try_vec(mtmask as usize + 1, 0u32)?,
            mtmask,
            mptr: 0,
            mlen: 0,
            mexp: -1,
            mm,
            mmcur: None,
            x: [0; NIN],
            wa: try_vec(NIN * 256 * 3, (1 << 16) / WINIT)?,
            wb: try_vec(NIN * 256 * 8, (1 << 16) / WINIT)?,
            sela: 0,
            selb: 0,
            pra: 0,
            prb: 0,
            a1: Apm::new(256)?,
            a2: Apm::new(65536)?,
            pr: 2048,
        });
        for i in 0..NCTX {
            m.h[i] = hash2(0, 0, i as u32 + 1);
        }
        Some(m)
    }

    #[inline]
    fn predict(&mut self) {
        let tb: &'static Tables = self.tb;
        let st = &tb.stretch;
        if self.bpos == 0 || self.bpos == 4 {
            // one 64-byte bucket holds all 15 counters for the next 4 bits
            for i in 0..NCTX {
                let f = mix32(self.h[i].wrapping_add(self.c0.wrapping_mul(0x9E37_79B1)));
                let chk = ((f >> 16) | 1) as u16; // 16-bit tag, never 0
                let bi = (f & self.mask) as usize;
                let base = i << self.bits;
                let b0 = base + (bi << 4);
                let b1 = base + ((bi ^ 1) << 4);
                let b = if self.t[b0].p == chk {
                    b0
                } else if self.t[b1].p == chk {
                    b1
                } else {
                    // evict the bucket whose context was seen less often
                    let b = if self.t[b0 + 1].n <= self.t[b1 + 1].n { b0 } else { b1 };
                    self.t[b] = Slot { p: chk, n: 0, chk: 0 };
                    for s in &mut self.t[b + 1..b + 16] {
                        *s = Slot { p: 32768, n: 0, chk: 0 };
                    }
                    b
                };
                self.bucket[i] = b;
            }
            self.nib = 1;
        }
        let mut k = 0;
        for i in 0..NCTX {
            let si = self.bucket[i] + self.nib as usize;
            self.cur[i] = si;
            let s = self.t[si];
            self.x[k] = if s.n != 0 { st[(s.p >> 4) as usize] as i32 } else { 0 };
            k += 1;
        }
        self.o0cur = (((self.c4 & 0xFF) << 8) | self.c0) as usize;
        self.x[k] = st[(self.o0[self.o0cur].p >> 4) as usize] as i32;
        k += 1;

        let mut lb = 0i32;
        if self.mlen > 0 && self.mexp >= 0 {
            let expbit = (self.mexp >> (7 - self.bpos)) & 1;
            lb = if self.mlen < 16 {
                self.mlen as i32
            } else {
                16 + ((self.mlen - 16) >> 3).min(15) as i32
            };
            let mi = (lb * 2 + expbit) as usize;
            self.mmcur = Some(mi);
            self.x[k] = st[(self.mm[mi].p >> 4) as usize] as i32;
        } else {
            self.mmcur = None;
            self.x[k] = 0;
        }
        k += 1;
        self.x[k] = 256; // bias

        // two mixers with different weight-set selectors, averaged in logit space
        self.sela = self.c0 as usize + 256 * (if lb == 0 { 0 } else if lb < 16 { 1 } else { 2 });
        self.selb = (((self.c4 & 0xFF) << 3) as usize) | self.bpos as usize;
        let wa = &self.wa[self.sela * NIN..self.sela * NIN + NIN];
        let wb = &self.wb[self.selb * NIN..self.selb * NIN + NIN];
        let (mut da, mut db) = (0i64, 0i64);
        for i in 0..NIN {
            da += self.x[i] as i64 * wa[i] as i64;
            db += self.x[i] as i64 * wb[i] as i64;
        }
        let sa = (da >> 16) as i32;
        let sb = (db >> 16) as i32;
        self.pra = squash(sa);
        self.prb = squash(sb);
        let pm = squash((sa + sb) >> 1);

        let p1 = self.a1.p(pm, self.c0 as usize, st);
        let p2 = self.a2.p(pm, (self.c0 | ((self.c4 & 0xFF) << 8)) as usize, st);
        self.pr = ((pm + p1 + 2 * p2 + 2) >> 2).clamp(1, 4095);
    }

    #[inline]
    fn train(w: &mut [i32], x: &[i32; NIN], err: i32) {
        for i in 0..NIN {
            let nw = w[i] + ((x[i] * err) >> MIXSH);
            w[i] = nw.clamp(-(1 << 22), 1 << 22);
        }
    }

    #[inline]
    fn update(&mut self, y: i32) {
        let tb: &'static Tables = self.tb;
        let rate = &tb.rate;
        for i in 0..NCTX {
            slot_update(&mut self.t[self.cur[i]], y, LIMIT, rate);
        }
        slot_update(&mut self.o0[self.o0cur], y, 60, rate);
        if let Some(mi) = self.mmcur {
            slot_update(&mut self.mm[mi], y, LIMIT, rate);
            if ((self.mexp >> (7 - self.bpos)) & 1) != y {
                self.mlen = 0;
            }
        }
        let (sa, sb) = (self.sela * NIN, self.selb * NIN);
        Self::train(&mut self.wa[sa..sa + NIN], &self.x, ((y << 12) - self.pra) * MIXLR);
        Self::train(&mut self.wb[sb..sb + NIN], &self.x, ((y << 12) - self.prb) * MIXLR);
        self.a1.update(y);
        self.a2.update(y);

        self.c0 = (self.c0 << 1) | y as u32;
        self.nib = (self.nib << 1) | y as u32;
        self.bpos += 1;
        if self.bpos < 8 {
            return;
        }

        // ---- byte boundary ----
        let c = self.c0 & 0xFF;
        self.c0 = 1;
        self.bpos = 0;
        self.c8 = (self.c8 << 8) | (self.c4 >> 24);
        self.c4 = (self.c4 << 8) | c;

        if (c as u8).is_ascii_alphabetic() || c >= 128 {
            let lc = if (b'A' as u32..=b'Z' as u32).contains(&c) { c + 32 } else { c };
            self.word0 = self.word0.wrapping_add(lc + 1).wrapping_mul(0x2F0B_3A49);
        } else if self.word0 != 0 {
            self.word1 = self.word0;
            self.word0 = 0;
        }

        let c4 = self.c4;
        self.h[0] = hash2(c4 & 0xFF, 0, 1);
        self.h[1] = hash2(c4 & 0xFFFF, 0, 2);
        self.h[2] = hash2(c4 & 0xFF_FFFF, 0, 3);
        self.h[3] = hash2(c4, 0, 4);
        self.h[4] = hash2(c4, self.c8 & 0xFFFF, 6);
        self.h[5] = hash2(self.word0, c4 & 0xFF, 7);
        self.h[6] = hash2(self.word0, self.word1, 8);
        self.h[7] = hash2(c4 & 0xFF_FF00, 0, 9); // skip last byte
        self.h[8] = hash2(c4 & 0xFF00_FF00, 0, 10); // bytes 2 and 4 back: tables

        // match model
        self.buf[(self.pos & self.bufmask) as usize] = c as u8;
        self.pos = self.pos.wrapping_add(1);
        if self.mlen > 0 {
            self.mptr = self.mptr.wrapping_add(1);
            if self.mlen < 65535 {
                self.mlen += 1;
            }
        }
        if self.pos >= MINLEN {
            let hh = (hash2(c4, self.c8 & 0xFF_FFFF, 11) & self.mtmask) as usize;
            if self.mlen == 0 {
                let cand = self.mtab[hh];
                if cand > 0 && self.pos.wrapping_sub(cand) <= self.bufmask {
                    let mut len = 0u32;
                    while len < 32
                        && len < cand
                        && self.buf[(cand.wrapping_sub(1 + len) & self.bufmask) as usize]
                            == self.buf[(self.pos.wrapping_sub(1 + len) & self.bufmask) as usize]
                    {
                        len += 1;
                    }
                    if len >= MINLEN {
                        self.mlen = len;
                        self.mptr = cand;
                    }
                }
            }
            self.mtab[hh] = self.pos;
        }
        self.mexp = if self.mlen > 0 {
            self.buf[(self.mptr & self.bufmask) as usize] as i32
        } else {
            -1
        };
    }

    /// Encode `input` as one segment into `out`. Returns bytes written, or
    /// None if `out` is too small (`bound(input.len())` is always enough).
    pub fn encode_segment(&mut self, input: &[u8], out: &mut [u8]) -> Option<usize> {
        let (mut x1, mut x2) = (0u32, 0xFFFF_FFFFu32);
        let mut o = 0usize;
        for &byte in input {
            let c = byte as i32;
            for b in (0..8).rev() {
                let y = (c >> b) & 1;
                self.predict();
                let xmid = x1 + (((x2 - x1) as u64 * self.pr as u64) >> 12) as u32;
                if y != 0 {
                    x2 = xmid;
                } else {
                    x1 = xmid + 1;
                }
                self.update(y);
                while (x1 ^ x2) & 0xFF00_0000 == 0 {
                    *out.get_mut(o)? = (x2 >> 24) as u8;
                    o += 1;
                    x1 <<= 8;
                    x2 = (x2 << 8) | 255;
                }
            }
        }
        for _ in 0..4 {
            *out.get_mut(o)? = (x1 >> 24) as u8;
            o += 1;
            x1 <<= 8;
        }
        Some(o)
    }

    /// Decode one segment from `input` into exactly `out.len()` bytes.
    /// Returns the number of input bytes consumed.
    pub fn decode_segment(&mut self, input: &[u8], out: &mut [u8]) -> usize {
        let (mut x1, mut x2, mut x) = (0u32, 0xFFFF_FFFFu32, 0u32);
        let mut ip = 0usize;
        let next = |ip: &mut usize| -> u32 {
            let v = input.get(*ip).copied().unwrap_or(0) as u32;
            if *ip < input.len() {
                *ip += 1;
            }
            v
        };
        for _ in 0..4 {
            x = (x << 8) | next(&mut ip);
        }
        for o in out.iter_mut() {
            let mut c = 0i32;
            for _ in 0..8 {
                self.predict();
                let xmid = x1 + (((x2 - x1) as u64 * self.pr as u64) >> 12) as u32;
                let y = (x <= xmid) as i32;
                if y != 0 {
                    x2 = xmid;
                } else {
                    x1 = xmid + 1;
                }
                self.update(y);
                c = (c << 1) | y;
                while (x1 ^ x2) & 0xFF00_0000 == 0 {
                    x1 <<= 8;
                    x2 = (x2 << 8) | 255;
                    x = (x << 8) | next(&mut ip);
                }
            }
            *o = c as u8;
        }
        ip
    }
}

/// Output buffer size that always fits one encoded segment of `n` bytes.
pub fn bound(n: usize) -> usize {
    n + (n >> 3) + 4096
}

/// x86 CALL/JMP transform: relative -> absolute targets (encode) or back.
pub fn e8e9(buf: &mut [u8], encode: bool) {
    let n = buf.len();
    if n < 5 {
        return;
    }
    let mut i = 0usize;
    while i + 5 <= n {
        if buf[i] == 0xE8 || buf[i] == 0xE9 {
            let v = u32::from_le_bytes([buf[i + 1], buf[i + 2], buf[i + 3], buf[i + 4]]);
            let at = (i + 5) as u32;
            let v = if encode { v.wrapping_add(at) } else { v.wrapping_sub(at) };
            buf[i + 1..i + 5].copy_from_slice(&v.to_le_bytes());
            i += 5;
        } else {
            i += 1;
        }
    }
}

/// Compress with RocketZip's segment framing: repeated [u32 len][u32 clen][data].
pub fn compress(data: &[u8], membits: i32, use_e8e9: bool) -> Option<Vec<u8>> {
    let mut m = Model::new(membits)?;
    let mut out = Vec::new();
    let mut tmp = vec![0u8; bound(SEGMENT)];
    for seg in data.chunks(SEGMENT) {
        let mut s = seg.to_vec();
        if use_e8e9 {
            e8e9(&mut s, true);
        }
        let n = m.encode_segment(&s, &mut tmp)?;
        out.extend_from_slice(&(seg.len() as u32).to_le_bytes());
        out.extend_from_slice(&(n as u32).to_le_bytes());
        out.extend_from_slice(&tmp[..n]);
    }
    Some(out)
}

/// Inverse of [`compress`]. Returns None on malformed framing.
pub fn decompress(blob: &[u8], membits: i32, use_e8e9: bool) -> Option<Vec<u8>> {
    let mut m = Model::new(membits)?;
    let mut out = Vec::new();
    let mut p = 0usize;
    while p < blob.len() {
        let hdr = blob.get(p..p + 8)?;
        let ulen = u32::from_le_bytes(hdr[0..4].try_into().ok()?) as usize;
        let clen = u32::from_le_bytes(hdr[4..8].try_into().ok()?) as usize;
        p += 8;
        let comp = blob.get(p..p + clen)?;
        p += clen;
        let start = out.len();
        out.resize(start + ulen, 0);
        m.decode_segment(comp, &mut out[start..]);
        if use_e8e9 {
            e8e9(&mut out[start..], false);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// C ABI - identical to zfcm.c, so cmengine.py loads this library unchanged
// ---------------------------------------------------------------------------
unsafe fn slice<'a>(p: *const u8, n: usize) -> &'a [u8] {
    if n == 0 || p.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts(p, n)
    }
}

unsafe fn slice_mut<'a>(p: *mut u8, n: usize) -> &'a mut [u8] {
    if n == 0 || p.is_null() {
        &mut []
    } else {
        std::slice::from_raw_parts_mut(p, n)
    }
}

#[no_mangle]
pub extern "C" fn zfcm_version() -> c_int {
    VERSION
}

#[no_mangle]
pub extern "C" fn zfcm_new(membits: c_int) -> *mut c_void {
    match Model::new(membits) {
        Some(m) => Box::into_raw(m) as *mut c_void,
        None => std::ptr::null_mut(),
    }
}

/// # Safety
/// `h` must come from `zfcm_new` and not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn zfcm_free(h: *mut c_void) {
    if !h.is_null() {
        drop(Box::from_raw(h as *mut Model));
    }
}

/// # Safety
/// `h` from `zfcm_new`; `input`/`out` valid for `n`/`cap` bytes.
#[no_mangle]
pub unsafe extern "C" fn zfcm_encode(
    h: *mut c_void,
    input: *const u8,
    n: usize,
    out: *mut u8,
    cap: usize,
) -> c_longlong {
    if h.is_null() {
        return -1;
    }
    let m = &mut *(h as *mut Model);
    match m.encode_segment(slice(input, n), slice_mut(out, cap)) {
        Some(o) => o as c_longlong,
        None => -1,
    }
}

/// # Safety
/// `h` from `zfcm_new`; `input`/`out` valid for `n`/`outlen` bytes.
#[no_mangle]
pub unsafe extern "C" fn zfcm_decode(
    h: *mut c_void,
    input: *const u8,
    n: usize,
    out: *mut u8,
    outlen: usize,
) -> c_longlong {
    if h.is_null() {
        return -1;
    }
    let m = &mut *(h as *mut Model);
    m.decode_segment(slice(input, n), slice_mut(out, outlen)) as c_longlong
}

/// # Safety
/// `buf` valid for `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn zfcm_e8e9(buf: *mut u8, n: usize, encode: c_int) {
    e8e9(slice_mut(buf, n), encode != 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(n: usize, seed: u32) -> Vec<u8> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (s >> 16) as u8
            })
            .collect()
    }

    #[test]
    fn roundtrip_various() {
        let text: Vec<u8> = (0..60_000)
            .flat_map(|i| format!("line {i}: the quick brown fox {}\n", i % 17).into_bytes())
            .collect();
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            b"x".to_vec(),
            vec![0u8; 300_000],
            lcg(100_000, 7),
            text, // > 1 MB: several segments
        ];
        for data in cases {
            for e8 in [false, true] {
                let c = compress(&data, 18, e8).unwrap();
                assert_eq!(decompress(&c, 18, e8).unwrap(), data);
            }
        }
    }

    #[test]
    fn compresses_text() {
        let text: Vec<u8> = (0..20_000)
            .flat_map(|i| format!("record {i} value {}\n", i * 7 % 1000).into_bytes())
            .collect();
        let c = compress(&text, 20, false).unwrap();
        assert!(c.len() < text.len() / 5);
    }

    #[test]
    fn e8e9_is_reversible() {
        let mut d = lcg(10_000, 3);
        for i in (0..d.len()).step_by(37) {
            d[i] = 0xE8;
        }
        let orig = d.clone();
        e8e9(&mut d, true);
        assert_ne!(d, orig);
        e8e9(&mut d, false);
        assert_eq!(d, orig);
    }

    #[test]
    fn truncated_input_is_safe() {
        let c = compress(b"hello hello hello", 16, false).unwrap();
        assert!(decompress(&c[..c.len() - 3], 16, false).is_none());
    }
}
