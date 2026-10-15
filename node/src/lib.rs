pub mod api;
pub mod chain;
pub mod config;
pub mod daemon;
pub mod discovery;
pub mod index;
pub mod mining;
pub mod peers;
pub mod pool;
pub mod presync;
pub mod reads;
pub mod store;
pub mod transport;
pub mod web;
pub mod wiki;
pub mod wire;

pub type Result<T> = std::result::Result<T, String>;
pub const FRAME_MAX: usize = 16 * 1024 * 1024;
pub fn unhex(s: &str, maximum: usize) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) || s.len() / 2 > maximum {
        return Err("invalid hex length".into());
    }
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| {
            let digit = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                _ => None,
            };
            Ok(digit(c[0]).ok_or("hex must be lowercase")? * 16
                + digit(c[1]).ok_or("hex must be lowercase")?)
        })
        .collect()
}
pub fn hash_arg(s: &str) -> Result<ting_core::hash::Hash> {
    unhex(s, 32)?
        .try_into()
        .map_err(|_| "expected 32-byte hash".into())
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
