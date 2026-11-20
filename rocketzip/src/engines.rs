//! Two Deflate implementations for exact recompression of documents:
//! classic zlib 1.3 (libz-sys) and zlib-ng 2.3 (libz-ng-sys), both bundled.
//! They emit different bytes for the same settings, and real files come from both.
use std::os::raw::c_int;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Classic,
    Ng,
}

impl Family {
    pub const ALL: [Family; 2] = [Family::Classic, Family::Ng]; // classic first: more common
    pub fn id(self) -> u8 {
        match self {
            Family::Classic => 0,
            Family::Ng => 1,
        }
    }
    pub fn from_id(id: u8) -> Family {
        if id == 1 { Family::Ng } else { Family::Classic }
    }
    pub fn name(self) -> &'static str {
        match self {
            Family::Classic => "classic",
            Family::Ng => "ng",
        }
    }
}

pub const Z_NO_FLUSH: c_int = 0;
pub const Z_SYNC_FLUSH: c_int = 2;
pub const Z_FULL_FLUSH: c_int = 3;
pub const Z_FINISH: c_int = 4;

// Both libraries share zlib's C API; libz-ng-sys links zlib-ng's `zng_` names,
// so classic zlib and zlib-ng live side by side in one program.
macro_rules! zlib_impl {
    ($m:ident, $name:ident) => {
        mod $name {
            use std::os::raw::{c_int, c_uint, c_void};
            use $m as z;

            extern "C" fn zalloc(_: *mut c_void, items: c_uint, size: c_uint) -> *mut c_void {
                unsafe { libc_calloc(items as usize, size as usize) }
            }
            extern "C" fn zfree(_: *mut c_void, p: *mut c_void) {
                unsafe { libc_free(p) }
            }
            unsafe extern "C" {
                #[link_name = "calloc"]
                fn libc_calloc(n: usize, s: usize) -> *mut c_void;
                #[link_name = "free"]
                fn libc_free(p: *mut c_void);
            }

            pub struct Stream(Box<z::z_stream>, bool);

            impl Stream {
                pub fn new(level: i32, wbits: i32, mem: i32, strategy: i32,
                           zdict: Option<&[u8]>) -> Option<Stream> {
                    let mut s = Box::new(z::z_stream {
                        next_in: std::ptr::null_mut(), avail_in: 0, total_in: 0,
                        next_out: std::ptr::null_mut(), avail_out: 0, total_out: 0,
                        msg: std::ptr::null_mut(), state: std::ptr::null_mut(),
                        zalloc, zfree, opaque: std::ptr::null_mut(),
                        data_type: 0, adler: 0, reserved: 0,
                    });
                    unsafe {
                        let r = z::deflateInit2_(&mut *s, level, 8, wbits, mem, strategy,
                                                 z::zlibVersion(),
                                                 std::mem::size_of::<z::z_stream>() as c_int);
                        if r != 0 { return None; }
                        let mut st = Stream(s, false);
                        if let Some(d) = zdict.filter(|d| !d.is_empty()) {
                            if z::deflateSetDictionary(&mut *st.0, d.as_ptr(), d.len() as _) != 0 {
                                return None;
                            }
                        }
                        Some(st)
                    }
                }

                /// Returns (status, consumed, produced).
                pub fn step(&mut self, input: &[u8], out: &mut [u8], flush: c_int) -> (c_int, usize, usize) {
                    let s = &mut *self.0;
                    s.next_in = input.as_ptr() as *mut u8;
                    s.avail_in = input.len() as _;
                    s.next_out = out.as_mut_ptr();
                    s.avail_out = out.len() as _;
                    let r = unsafe { z::deflate(s, flush) };
                    (r, input.len() - s.avail_in as usize, out.len() - s.avail_out as usize)
                }

                pub fn end(&mut self) {
                    if !self.1 {
                        self.1 = true;
                        unsafe { z::deflateEnd(&mut *self.0); }
                    }
                }
            }

            impl Drop for Stream {
                fn drop(&mut self) { self.end(); }
            }
        }
    };
}
zlib_impl!(libz_sys, classic);
zlib_impl!(libz_ng_sys, ng);

enum Inner {
    Classic(classic::Stream),
    Ng(ng::Stream),
}

/// A streaming compressor equivalent to Python's `zlib.compressobj`.
pub struct Deflater {
    inner: Inner,
}

impl Deflater {
    pub fn new(fam: Family, level: i32, wbits: i32, mem: i32, strategy: i32,
               zdict: Option<&[u8]>) -> Option<Deflater> {
        let inner = match fam {
            Family::Classic => Inner::Classic(classic::Stream::new(level, wbits, mem, strategy, zdict)?),
            Family::Ng => Inner::Ng(ng::Stream::new(level, wbits, mem, strategy, zdict)?),
        };
        Some(Deflater { inner })
    }

    /// Feeds data exactly like CPython's zlib module: output space is offered in
    /// blocks of 32 KB, 64 KB, 256 KB, 1 MB, ... and deflate is called until a call
    /// leaves space unused. zlib-ng's fast/medium strategies (levels 1-6) make
    /// block decisions that depend on this, so mirroring it keeps outputs exact.
    fn run(&mut self, data: &[u8], flush: c_int) -> Vec<u8> {
        const KB: usize = 1024;
        const MB: usize = 1024 * KB;
        const BLOCKS: [usize; 17] = [32 * KB, 64 * KB, 256 * KB, MB, 4 * MB, 8 * MB, 16 * MB,
            16 * MB, 32 * MB, 32 * MB, 32 * MB, 32 * MB, 64 * MB, 64 * MB, 128 * MB, 128 * MB,
            256 * MB];
        let mut out = Vec::new();
        let mut pos = 0usize;
        let mut bi = 0usize;
        let mut chunk: Vec<u8> = vec![0u8; BLOCKS[0]];
        let mut fill = 0usize; // bytes used in the current block
        loop {
            if fill == chunk.len() {
                out.extend_from_slice(&chunk);
                bi = (bi + 1).min(BLOCKS.len() - 1);
                chunk = vec![0u8; BLOCKS[bi]];
                fill = 0;
            }
            let (r, used, made) = match &mut self.inner {
                Inner::Classic(s) => s.step(&data[pos..], &mut chunk[fill..], flush),
                Inner::Ng(s) => s.step(&data[pos..], &mut chunk[fill..], flush),
            };
            pos += used;
            fill += made;
            if r != 0 && r != 1 && r != -5 {
                break; // error
            }
            if fill < chunk.len() {
                break; // deflate left space: it has nothing more to give for now
            }
        }
        out.extend_from_slice(&chunk[..fill]);
        out
    }

    pub fn compress(&mut self, data: &[u8]) -> Vec<u8> {
        self.run(data, Z_NO_FLUSH)
    }

    pub fn flush(&mut self, mode: c_int) -> Vec<u8> {
        let r = self.run(&[], mode);
        if mode == Z_FINISH {
            match &mut self.inner {
                Inner::Classic(s) => s.end(),
                Inner::Ng(s) => s.end(),
            }
        }
        r
    }
}

pub const PROBE: usize = 256 << 10;

/// Same two-piece feeding as the Python version (output must be reproducible).
/// None if the settings are invalid (e.g. read from a damaged archive).
pub fn deflate(data: &[u8], level: i32, mem: i32, wbits: i32, fam: Family) -> Option<Vec<u8>> {
    let mut c = Deflater::new(fam, level, wbits, mem, 0, None)?;
    let cut = data.len().min(PROBE);
    let mut out = c.compress(&data[..cut]);
    out.extend(c.compress(&data[cut..]));
    out.extend(c.flush(Z_FINISH));
    Some(out)
}

/// Block-parallel Deflate (LibreOffice / pigz style).
pub fn deflate_blocked(data: &[u8], level: i32, mem: i32, fam: Family, bs: usize,
                       flush: c_int) -> Option<Vec<u8>> {
    if bs == 0 {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let end = (i + bs).min(data.len());
        let zd = &data[i.saturating_sub(32768)..i];
        let mut c = Deflater::new(fam, level, -15, mem, 0, Some(zd))?;
        out.extend(c.compress(&data[i..end]));
        let last = i + bs >= data.len();
        out.extend(c.flush(if last { Z_FINISH } else { flush }));
        i = end;
    }
    Some(out)
}
