//! Specification-driven Ting reference core.

// A program's memory may reach 4 GiB, and sizes are computed in `usize`.
#[cfg(not(target_pointer_width = "64"))]
compile_error!("Ting requires a 64-bit platform");

pub mod consensus;
pub mod hash;
pub mod meter;
pub mod protocol;
pub mod state;
pub mod vm;
