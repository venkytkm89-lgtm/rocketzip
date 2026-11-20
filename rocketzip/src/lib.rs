//! RocketZip: a 7-Zip-style archiver with an ultra-compression format (.zfx).
pub mod api;
pub mod bench;
pub mod engines;
pub mod formats;
pub mod rarfmt;
pub mod sevenz;
pub mod tarfmt;
pub mod ultra;
pub mod util;
pub mod zipfmt;

pub use api::*;
pub use util::{human, Error, ProgressFn, Result};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// One item in an archive listing.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub size: Option<u64>,
    pub packed: Option<u64>,
    pub mtime: Option<f64>,
    pub is_dir: bool,
}
