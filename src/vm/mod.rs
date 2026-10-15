//! Frozen Wasm profile, reference instruction metering, and host runtime.
mod decode;
mod host;
#[cfg(feature = "oracle")]
mod instrument;
mod interp;
#[cfg(feature = "oracle")]
mod oracle;
pub mod price;
pub mod profile;
#[cfg(feature = "oracle")]
mod reference;
mod runtime;
pub use runtime::{CODE_CACHE_BYTES, Candidate, CodeCache, Context, Runtime};
