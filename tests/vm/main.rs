//! The VM tests. Each module states one part of the protocol rules' VM as
//! cases: a program in WebAssembly text with its expected outcome and its
//! charges beside it.
//! `harness` runs a case on the reference VM; `vm-model` turns a case's
//! ledger into exact counters
//! without seeing the VM; `shape` reads from a module the facts the model
//! needs; `dsl` composes the common module shapes.
mod across;
mod budgets;
mod calls;
mod chain_histories;
mod compositions;
mod consensus;
mod control;
#[cfg(feature = "coverage")]
mod coverage;
mod deployment;
mod dsl;
mod gate;
mod generated;
mod harness;
mod heavy;
mod host;
mod isolation;
mod memory;
mod network;
mod numeric;
mod primitives;
mod programs;
mod reservations;
mod seed;
mod shape;
mod state_histories;
mod validation;
mod wide;
