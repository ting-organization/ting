//! The VM tests. Each module states one part of the wiki's VM rules as
//! cases: a program in WebAssembly text with its expected outcome and its
//! charges beside it.
//! `harness` runs a case on the reference VM, and on the oracle engine under
//! the `oracle` feature; `vm-model` turns a case's ledger into exact counters
//! without seeing the VM; `shape` reads from a module the facts the model
//! needs; `dsl` composes the common module shapes.
mod budgets;
mod calls;
mod compositions;
mod control;
mod deployment;
mod dsl;
mod harness;
mod host;
mod isolation;
mod memory;
mod numeric;
#[cfg(feature = "oracle")]
mod oracle;
mod programs;
mod reservations;
mod shape;
mod validation;
mod wide;
