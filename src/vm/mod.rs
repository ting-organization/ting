//! Frozen Wasm profile, reference instruction metering, and host runtime.
#[cfg(feature = "coverage")]
pub mod coverage;
mod decode;
mod host;
mod interp;
pub mod price;
pub mod profile;
mod runtime;
pub use runtime::{Attempt, CODE_CACHE_BYTES, Candidate, CodeCache, Context, Runtime};
