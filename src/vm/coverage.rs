//! What the VM exercised, for the seed's coverage gate (`tests/vm/coverage.rs`).
//! Compiled only with the `coverage` feature and never part of a consensus
//! build: each hook adds to the thread's tally and changes nothing the VM
//! does. [`take`] returns the tally and starts a new one.
use super::decode::{Import, Op};
use crate::protocol::Status;
use std::{cell::RefCell, collections::BTreeMap};

/// Counts of what ran.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    /// Instructions dispatched, by their text name.
    pub instructions: BTreeMap<&'static str, u64>,
    /// Host imports by name and how each ended: `returned`, `handed over` (a
    /// `call` or `deploy` whose child then ran) or the status it stopped the
    /// invocation with.
    pub imports: BTreeMap<(&'static str, String), u64>,
    /// Invocations by kind (`call` or `deploy`), depth (`top` or `nested`)
    /// and the status they ended with.
    pub invocations: BTreeMap<(&'static str, &'static str, String), u64>,
    /// Deployments of code the decoder rejected, by its reason.
    pub rejections: BTreeMap<&'static str, u64>,
    /// Memory sizes, in pages, instances started with or grew to.
    pub pages: BTreeMap<u64, u64>,
    /// Stack pages paid, by where: on entering an invocation's entry function
    /// or a called function.
    pub stack: BTreeMap<&'static str, u64>,
}

thread_local! {
    static TALLY: RefCell<Tally> = RefCell::new(Tally::default());
}
fn add<K: Ord>(map: &mut BTreeMap<K, u64>, key: K, n: u64) {
    *map.entry(key).or_insert(0) += n;
}

/// The tally so far; the next starts empty.
pub fn take() -> Tally {
    TALLY.with(|t| t.take())
}
pub(super) fn instruction(op: &Op) {
    TALLY.with(|t| add(&mut t.borrow_mut().instructions, name(op), 1));
}
pub(super) fn import(import: Import, ended: String) {
    TALLY.with(|t| add(&mut t.borrow_mut().imports, (import_name(import), ended), 1));
}
pub(super) fn invocation(kind: &'static str, top: bool, status: Status) {
    let depth = if top { "top" } else { "nested" };
    TALLY.with(|t| {
        add(
            &mut t.borrow_mut().invocations,
            (kind, depth, format!("{status:?}")),
            1,
        )
    });
}
pub(super) fn rejection(reason: &'static str) {
    TALLY.with(|t| add(&mut t.borrow_mut().rejections, reason, 1));
}
pub(super) fn pages(pages: u64) {
    TALLY.with(|t| add(&mut t.borrow_mut().pages, pages, 1));
}
pub(super) fn stack(place: &'static str, pages: u64) {
    if pages > 0 {
        TALLY.with(|t| add(&mut t.borrow_mut().stack, place, pages));
    }
}

fn import_name(import: Import) -> &'static str {
    match import {
        Import::Input => "input",
        Import::Context => "context",
        Import::StorageGet => "storage_get",
        Import::StorageSet => "storage_set",
        Import::StorageDelete => "storage_delete",
        Import::CodeHash => "code_hash",
        Import::Call => "call",
        Import::Deploy => "deploy",
        Import::CallResult => "call_result",
        Import::Emit => "emit",
        Import::Return => "return",
        Import::Revert => "revert",
    }
}
/// The text name of a decoded instruction.
fn name(op: &Op) -> &'static str {
    match op {
        Op::Unreachable => "unreachable",
        Op::Nop => "nop",
        Op::Block => "block",
        Op::Loop => "loop",
        Op::If { .. } => "if",
        Op::Else { .. } => "else",
        Op::End => "end",
        Op::Br(_) => "br",
        Op::BrIf(_) => "br_if",
        Op::BrTable(_) => "br_table",
        Op::Return => "return",
        Op::Call(_) => "call",
        Op::CallIndirect(_) => "call_indirect",
        Op::Drop => "drop",
        Op::Select => "select",
        Op::LocalGet(_) => "local.get",
        Op::LocalSet(_) => "local.set",
        Op::LocalTee(_) => "local.tee",
        Op::GlobalGet(_) => "global.get",
        Op::GlobalSet(_) => "global.set",
        Op::I32Load(_) => "i32.load",
        Op::I64Load(_) => "i64.load",
        Op::I32Load8S(_) => "i32.load8_s",
        Op::I32Load8U(_) => "i32.load8_u",
        Op::I32Load16S(_) => "i32.load16_s",
        Op::I32Load16U(_) => "i32.load16_u",
        Op::I64Load8S(_) => "i64.load8_s",
        Op::I64Load8U(_) => "i64.load8_u",
        Op::I64Load16S(_) => "i64.load16_s",
        Op::I64Load16U(_) => "i64.load16_u",
        Op::I64Load32S(_) => "i64.load32_s",
        Op::I64Load32U(_) => "i64.load32_u",
        Op::I32Store(_) => "i32.store",
        Op::I64Store(_) => "i64.store",
        Op::I32Store8(_) => "i32.store8",
        Op::I32Store16(_) => "i32.store16",
        Op::I64Store8(_) => "i64.store8",
        Op::I64Store16(_) => "i64.store16",
        Op::I64Store32(_) => "i64.store32",
        Op::MemorySize => "memory.size",
        Op::MemoryGrow => "memory.grow",
        Op::I32Const(_) => "i32.const",
        Op::I64Const(_) => "i64.const",
        Op::I32Eqz => "i32.eqz",
        Op::I32Eq => "i32.eq",
        Op::I32Ne => "i32.ne",
        Op::I32LtS => "i32.lt_s",
        Op::I32LtU => "i32.lt_u",
        Op::I32GtS => "i32.gt_s",
        Op::I32GtU => "i32.gt_u",
        Op::I32LeS => "i32.le_s",
        Op::I32LeU => "i32.le_u",
        Op::I32GeS => "i32.ge_s",
        Op::I32GeU => "i32.ge_u",
        Op::I64Eqz => "i64.eqz",
        Op::I64Eq => "i64.eq",
        Op::I64Ne => "i64.ne",
        Op::I64LtS => "i64.lt_s",
        Op::I64LtU => "i64.lt_u",
        Op::I64GtS => "i64.gt_s",
        Op::I64GtU => "i64.gt_u",
        Op::I64LeS => "i64.le_s",
        Op::I64LeU => "i64.le_u",
        Op::I64GeS => "i64.ge_s",
        Op::I64GeU => "i64.ge_u",
        Op::I32Clz => "i32.clz",
        Op::I32Ctz => "i32.ctz",
        Op::I32Popcnt => "i32.popcnt",
        Op::I32Add => "i32.add",
        Op::I32Sub => "i32.sub",
        Op::I32Mul => "i32.mul",
        Op::I32DivS => "i32.div_s",
        Op::I32DivU => "i32.div_u",
        Op::I32RemS => "i32.rem_s",
        Op::I32RemU => "i32.rem_u",
        Op::I32And => "i32.and",
        Op::I32Or => "i32.or",
        Op::I32Xor => "i32.xor",
        Op::I32Shl => "i32.shl",
        Op::I32ShrS => "i32.shr_s",
        Op::I32ShrU => "i32.shr_u",
        Op::I32Rotl => "i32.rotl",
        Op::I32Rotr => "i32.rotr",
        Op::I64Clz => "i64.clz",
        Op::I64Ctz => "i64.ctz",
        Op::I64Popcnt => "i64.popcnt",
        Op::I64Add => "i64.add",
        Op::I64Sub => "i64.sub",
        Op::I64Mul => "i64.mul",
        Op::I64DivS => "i64.div_s",
        Op::I64DivU => "i64.div_u",
        Op::I64RemS => "i64.rem_s",
        Op::I64RemU => "i64.rem_u",
        Op::I64And => "i64.and",
        Op::I64Or => "i64.or",
        Op::I64Xor => "i64.xor",
        Op::I64Shl => "i64.shl",
        Op::I64ShrS => "i64.shr_s",
        Op::I64ShrU => "i64.shr_u",
        Op::I64Rotl => "i64.rotl",
        Op::I64Rotr => "i64.rotr",
        Op::I32WrapI64 => "i32.wrap_i64",
        Op::I64ExtendI32S => "i64.extend_i32_s",
        Op::I64ExtendI32U => "i64.extend_i32_u",
        Op::I32Extend8S => "i32.extend8_s",
        Op::I32Extend16S => "i32.extend16_s",
        Op::I64Extend8S => "i64.extend8_s",
        Op::I64Extend16S => "i64.extend16_s",
        Op::I64Extend32S => "i64.extend32_s",
        Op::MemoryCopy => "memory.copy",
        Op::MemoryFill => "memory.fill",
        Op::I64Add128 => "i64.add128",
        Op::I64Sub128 => "i64.sub128",
        Op::I64MulWideS => "i64.mul_wide_s",
        Op::I64MulWideU => "i64.mul_wide_u",
    }
}
