//! The binary format and validation of the execution profile (rules §17, §18),
//! and the compilation of function bodies into the interpreter's code.
//!
//! One pass over the bytes decodes each section, validates typing with the
//! Core 1.0 validation algorithm, checks the profile's shape rules, measures
//! the operand height of every function for its reservation, and resolves the
//! targets of structured branches. Every integer is a minimally encoded
//! LEB128 and every construct has one encoding, so an accepted module has
//! exactly the bytes a canonical encoder would produce; nothing is re-encoded.
//! Every count is checked against the bytes that remain before anything is
//! allocated for it.
use super::profile::{
    FunctionInfo, IMPORTS, MAX_FUNCTIONS, MAX_GLOBALS, MAX_LOCALS, MAX_MODULE, MAX_PAGES,
    MAX_PARAMETERS, MAX_SEGMENTS, MAX_SLOTS, MAX_TYPES, Profile, allowed_opcode, allowed_prefixed,
};
use crate::protocol::Error;

type Result<T> = std::result::Result<T, Error>;
fn bad() -> Error {
    Error("module outside frozen VM profile")
}
fn noncanonical() -> Error {
    Error("noncanonical Wasm encoding")
}
/// A function body that fails the Core 1.0 typing rules: operand types, label
/// types, indices, mutability, memory and table presence, alignment.
fn mistyped() -> Error {
    Error("function body fails Core validation")
}

/// Bytes per memory page.
pub const PAGE: u64 = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValType {
    I32,
    I64,
}
use ValType::{I32, I64};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub result: Option<ValType>,
}

/// The twelve host imports (rules §22).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Import {
    Input,
    Context,
    StorageGet,
    StorageSet,
    StorageDelete,
    CodeHash,
    Call,
    Deploy,
    CallResult,
    Emit,
    Return,
    Revert,
}
impl Import {
    pub fn named(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"input" => Self::Input,
            b"context" => Self::Context,
            b"storage_get" => Self::StorageGet,
            b"storage_set" => Self::StorageSet,
            b"storage_delete" => Self::StorageDelete,
            b"code_hash" => Self::CodeHash,
            b"call" => Self::Call,
            b"deploy" => Self::Deploy,
            b"call_result" => Self::CallResult,
            b"emit" => Self::Emit,
            b"return" => Self::Return,
            b"revert" => Self::Revert,
            _ => return None,
        })
    }
    /// Number of i32 parameters.
    pub fn params(self) -> usize {
        match self {
            Self::StorageDelete => 1,
            Self::Input | Self::CallResult | Self::Emit | Self::Return | Self::Revert => 2,
            Self::Context | Self::StorageGet | Self::CodeHash | Self::StorageSet => 3,
            Self::Call | Self::Deploy => 5,
        }
    }
    /// Whether the import returns an i32.
    pub fn result(self) -> bool {
        !matches!(
            self,
            Self::StorageSet | Self::StorageDelete | Self::Emit | Self::Return | Self::Revert
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Global {
    pub ty: ValType,
    pub mutable: bool,
    pub init: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub initial: u32,
    pub maximum: u32,
}

/// A structured branch, resolved: continue at `pc` with the operand stack cut
/// to `height` values above the frame's operand base, keeping the top value
/// when the label carries one.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub pc: u32,
    pub height: u32,
    pub keep: bool,
}

/// One original instruction, with immediates decoded and labels resolved.
/// Nested `end`s compile to nothing; `Else` is the jump over the else-branch
/// taken when the then-branch falls through; `End` is the function's end.
#[derive(Clone, Copy, Debug)]
pub enum Op {
    Unreachable,
    Nop,
    Block,
    Loop,
    If { skip: u32 },
    Else { end: u32 },
    End,
    Br(Target),
    BrIf(Target),
    BrTable(u32),
    Return,
    Call(u32),
    CallIndirect(u32),
    Drop,
    Select,
    LocalGet(u32),
    LocalSet(u32),
    LocalTee(u32),
    GlobalGet(u32),
    GlobalSet(u32),
    I32Load(u32),
    I64Load(u32),
    I32Load8S(u32),
    I32Load8U(u32),
    I32Load16S(u32),
    I32Load16U(u32),
    I64Load8S(u32),
    I64Load8U(u32),
    I64Load16S(u32),
    I64Load16U(u32),
    I64Load32S(u32),
    I64Load32U(u32),
    I32Store(u32),
    I64Store(u32),
    I32Store8(u32),
    I32Store16(u32),
    I64Store8(u32),
    I64Store16(u32),
    I64Store32(u32),
    MemorySize,
    MemoryGrow,
    I32Const(u32),
    I64Const(u64),
    I32Eqz,
    I32Eq,
    I32Ne,
    I32LtS,
    I32LtU,
    I32GtS,
    I32GtU,
    I32LeS,
    I32LeU,
    I32GeS,
    I32GeU,
    I64Eqz,
    I64Eq,
    I64Ne,
    I64LtS,
    I64LtU,
    I64GtS,
    I64GtU,
    I64LeS,
    I64LeU,
    I64GeS,
    I64GeU,
    I32Clz,
    I32Ctz,
    I32Popcnt,
    I32Add,
    I32Sub,
    I32Mul,
    I32DivS,
    I32DivU,
    I32RemS,
    I32RemU,
    I32And,
    I32Or,
    I32Xor,
    I32Shl,
    I32ShrS,
    I32ShrU,
    I32Rotl,
    I32Rotr,
    I64Clz,
    I64Ctz,
    I64Popcnt,
    I64Add,
    I64Sub,
    I64Mul,
    I64DivS,
    I64DivU,
    I64RemS,
    I64RemU,
    I64And,
    I64Or,
    I64Xor,
    I64Shl,
    I64ShrS,
    I64ShrU,
    I64Rotl,
    I64Rotr,
    I32WrapI64,
    I64ExtendI32S,
    I64ExtendI32U,
    I32Extend8S,
    I32Extend16S,
    I64Extend8S,
    I64Extend16S,
    I64Extend32S,
    MemoryCopy,
    MemoryFill,
    I64Add128,
    I64Sub128,
    I64MulWideS,
    I64MulWideU,
}

/// A `br_table`'s targets: each entry, the default last, is the index of its
/// label's target, so a label is resolved once however many entries name it.
#[derive(Clone, Debug, Default)]
pub struct BranchTable {
    pub entries: Vec<u32>,
    pub targets: Vec<Target>,
}

/// A defined function, compiled.
#[derive(Clone, Debug)]
pub struct Function {
    pub params: u32,
    /// Declared locals, zero-initialised on entry.
    pub locals: u32,
    pub result: bool,
    pub code: Vec<Op>,
    pub tables: Vec<BranchTable>,
    pub info: FunctionInfo,
}

/// A decoded, validated module.
#[derive(Clone, Debug)]
pub struct Module {
    pub types: Vec<FuncType>,
    pub imports: Vec<Import>,
    /// Type index of every function, imports first.
    pub function_types: Vec<u32>,
    pub functions: Vec<Function>,
    /// The table after its element segments are applied; `None` is null.
    pub table: Vec<Option<u32>>,
    pub memory: Option<Limits>,
    pub globals: Vec<Global>,
    pub data: Vec<(u32, Vec<u8>)>,
    pub invoke: u32,
    pub profile: Profile,
}

impl Module {
    /// Roughly the memory the decoded module holds: what a code cache counts,
    /// since compiled code is many times larger than the bytes it came from.
    pub fn weight(&self) -> u64 {
        use std::mem::size_of;
        let functions: usize = self
            .functions
            .iter()
            .map(|f| {
                let tables: usize = f
                    .tables
                    .iter()
                    .map(|t| {
                        size_of::<BranchTable>()
                            + t.entries.len() * size_of::<u32>()
                            + t.targets.len() * size_of::<Target>()
                    })
                    .sum();
                size_of::<Function>() + f.code.len() * size_of::<Op>() + tables
            })
            .sum();
        let types: usize = self
            .types
            .iter()
            .map(|t| size_of::<FuncType>() + t.params.len() * size_of::<ValType>())
            .sum();
        let data: usize = self
            .data
            .iter()
            .map(|(_, d)| size_of::<(u32, Vec<u8>)>() + d.len())
            .sum();
        (size_of::<Module>()
            + functions
            + types
            + data
            + self.function_types.len() * size_of::<u32>()
            + self.table.len() * size_of::<Option<u32>>()
            + self.globals.len() * size_of::<Global>()
            + self.profile.functions.len() * size_of::<FunctionInfo>()) as u64
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }
    fn eof(&self) -> bool {
        self.pos == self.bytes.len()
    }
    fn byte(&mut self) -> Result<u8> {
        let b = *self.bytes.get(self.pos).ok_or_else(bad)?;
        self.pos += 1;
        Ok(b)
    }
    fn expect(&mut self, b: u8) -> Result<()> {
        if self.byte()? == b {
            Ok(())
        } else {
            Err(bad())
        }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(bad());
        }
        let s = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn sub(&mut self, n: usize) -> Result<Reader<'a>> {
        Ok(Reader::new(self.take(n)?))
    }
    /// A minimally encoded unsigned LEB128 of at most 32 bits.
    fn u32(&mut self) -> Result<u32> {
        let mut value = 0u32;
        let mut shift = 0u32;
        loop {
            let b = self.byte()?;
            if shift == 28 && b & 0xf0 != 0 {
                return Err(bad());
            }
            value |= u32::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                if shift > 0 && b == 0 {
                    return Err(noncanonical());
                }
                return Ok(value);
            }
            shift += 7;
        }
    }
    /// A minimally encoded signed LEB128 of `bits` bits.
    fn sleb(&mut self, bits: u32) -> Result<i64> {
        let mut value = 0u64;
        let mut shift = 0u32;
        let mut previous = None;
        loop {
            let b = self.byte()?;
            let payload = u64::from(b & 0x7f);
            if shift + 7 >= bits {
                // The last permitted byte: no continuation, and the bits above
                // the value's width repeat its sign bit.
                if b & 0x80 != 0 {
                    return Err(bad());
                }
                let data = bits - shift;
                let sign = (payload >> (data - 1)) & 1;
                let expected = if sign == 1 {
                    (1u64 << (7 - data)) - 1
                } else {
                    0
                };
                if payload >> data != expected {
                    return Err(bad());
                }
                value |= payload << shift;
            } else {
                value |= payload << shift;
                if b & 0x80 != 0 {
                    shift += 7;
                    previous = Some(b);
                    continue;
                }
                if b & 0x40 != 0 {
                    value |= u64::MAX << (shift + 7);
                }
            }
            if let Some(p) = previous
                && ((b == 0x00 && p & 0x40 == 0) || (b == 0x7f && p & 0x40 != 0))
            {
                return Err(noncanonical());
            }
            return Ok(if bits == 32 {
                i64::from(value as u32 as i32)
            } else {
                value as i64
            });
        }
    }
    fn s32(&mut self) -> Result<i32> {
        Ok(self.sleb(32)? as i32)
    }
    fn s64(&mut self) -> Result<i64> {
        self.sleb(64)
    }
    /// A vector length: at most `bound`, and no larger than the bytes left,
    /// since every counted item occupies at least one byte.
    fn count(&mut self, bound: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n > bound || n > self.remaining() {
            return Err(bad());
        }
        Ok(n)
    }
    fn valtype(&mut self) -> Result<ValType> {
        match self.byte()? {
            0x7f => Ok(I32),
            0x7e => Ok(I64),
            _ => Err(bad()),
        }
    }
    fn name(&mut self) -> Result<&'a [u8]> {
        let n = self.count(usize::MAX)?;
        self.take(n)
    }
    /// `i32.const n end`, the only constant expression of the profile.
    fn const_i32(&mut self) -> Result<u32> {
        self.expect(0x41)?;
        let value = self.s32()? as u32;
        self.expect(0x0b)?;
        Ok(value)
    }
}

#[derive(Default)]
struct Partial {
    types: Vec<FuncType>,
    imports: Vec<Import>,
    function_types: Vec<u32>,
    defined: usize,
    functions: Vec<Function>,
    table: Option<u32>,
    memory: Option<Limits>,
    globals: Vec<Global>,
    elements: Vec<(u32, Vec<u32>)>,
    data: Vec<(u32, Vec<u8>)>,
    data_bytes: u64,
    invoke: Option<u32>,
    memory_export: bool,
    code: Option<usize>,
}

pub fn decode(bytes: &[u8]) -> Result<Module> {
    if bytes.len() > MAX_MODULE || bytes.len() < 8 {
        return Err(bad());
    }
    let mut r = Reader::new(bytes);
    if r.take(8)? != b"\0asm\x01\0\0\0" {
        return Err(bad());
    }
    let mut m = Partial::default();
    let mut last = 0u8;
    while !r.eof() {
        let id = r.byte()?;
        let size = r.u32()? as usize;
        let mut s = r.sub(size)?;
        // Type, import, function, table, memory, global, export, element, code
        // and data, each at most once, in that order; nothing else. Every one
        // is a vector, and a section with no entries is left out.
        if id == 0 || id == 8 || id > 11 || id <= last {
            return Err(bad());
        }
        if s.bytes.first() == Some(&0) {
            return Err(noncanonical());
        }
        match id {
            1 => types(&mut s, &mut m)?,
            2 => imports(&mut s, &mut m)?,
            3 => functions(&mut s, &mut m)?,
            4 => table(&mut s, &mut m)?,
            5 => memory(&mut s, &mut m)?,
            6 => globals(&mut s, &mut m)?,
            7 => exports(&mut s, &mut m)?,
            9 => elements(&mut s, &mut m)?,
            10 => code(&mut s, &mut m)?,
            _ => data(&mut s, &mut m)?,
        }
        if !s.eof() {
            return Err(bad());
        }
        last = id;
    }
    let invoke = m.invoke.ok_or_else(bad)?;
    if (invoke as usize) < m.imports.len() || m.code.unwrap_or(0) != m.defined {
        return Err(bad());
    }
    let ty = &m.types[m.function_types[invoke as usize] as usize];
    if !ty.params.is_empty() || ty.result.is_some() || m.memory.is_some() != m.memory_export {
        return Err(bad());
    }
    let mut table = vec![None; m.table.unwrap_or(0) as usize];
    for (offset, items) in &m.elements {
        for (k, f) in items.iter().enumerate() {
            table[*offset as usize + k] = Some(*f);
        }
    }
    let mut infos = vec![FunctionInfo::default(); m.imports.len()];
    infos.extend(m.functions.iter().map(|f| f.info));
    let profile = Profile {
        functions: infos,
        imports: m.imports.len() as u32,
        types: m.types.len() as u32,
        invoke,
        memory: m.memory.map_or(0, |l| l.initial as usize * PAGE as usize),
        globals: m.globals.len() as u64,
        data_bytes: m.data_bytes,
        data_segments: m.data.len() as u64,
        data_spans: m.data.iter().filter(|(_, bytes)| !bytes.is_empty()).count() as u64,
        table: u64::from(m.table.unwrap_or(0)),
    };
    Ok(Module {
        types: m.types,
        imports: m.imports,
        function_types: m.function_types,
        functions: m.functions,
        table,
        memory: m.memory,
        globals: m.globals,
        data: m.data,
        invoke,
        profile,
    })
}

fn types(s: &mut Reader<'_>, m: &mut Partial) -> Result<()> {
    let n = s.count(MAX_TYPES as usize)?;
    for _ in 0..n {
        s.expect(0x60)?;
        let np = s.count(MAX_PARAMETERS)?;
        let mut params = Vec::with_capacity(np);
        for _ in 0..np {
            params.push(s.valtype()?);
        }
        let result = match s.count(1)? {
            0 => None,
            _ => Some(s.valtype()?),
        };
        m.types.push(FuncType { params, result });
    }
    Ok(())
}
fn imports(s: &mut Reader<'_>, m: &mut Partial) -> Result<()> {
    let n = s.count(IMPORTS as usize)?;
    for _ in 0..n {
        let module = s.name()?;
        let import = Import::named(s.name()?).ok_or_else(bad)?;
        s.expect(0x00)?;
        let ty = s.u32()?;
        let ft = m.types.get(ty as usize).ok_or_else(bad)?;
        let result = if import.result() { Some(I32) } else { None };
        if module != b"ting"
            || m.imports.contains(&import)
            || ft.params.len() != import.params()
            || ft.params.iter().any(|t| *t != I32)
            || ft.result != result
        {
            return Err(bad());
        }
        m.imports.push(import);
        m.function_types.push(ty);
    }
    Ok(())
}
fn functions(s: &mut Reader<'_>, m: &mut Partial) -> Result<()> {
    let n = s.count(MAX_FUNCTIONS.saturating_sub(m.imports.len() as u32) as usize)?;
    for _ in 0..n {
        let ty = s.u32()?;
        if ty as usize >= m.types.len() {
            return Err(bad());
        }
        m.function_types.push(ty);
    }
    m.defined = n;
    Ok(())
}
fn table(s: &mut Reader<'_>, m: &mut Partial) -> Result<()> {
    for _ in 0..s.count(1)? {
        s.expect(0x70)?;
        s.expect(0x01)?;
        let min = s.u32()?;
        let max = s.u32()?;
        if min != max || u64::from(min) > m.function_types.len() as u64 {
            return Err(bad());
        }
        m.table = Some(min);
    }
    Ok(())
}
fn memory(s: &mut Reader<'_>, m: &mut Partial) -> Result<()> {
    for _ in 0..s.count(1)? {
        s.expect(0x01)?;
        let initial = s.u32()?;
        let maximum = s.u32()?;
        if u64::from(maximum) > MAX_PAGES || initial > maximum {
            return Err(bad());
        }
        m.memory = Some(Limits { initial, maximum });
    }
    Ok(())
}
fn globals(s: &mut Reader<'_>, m: &mut Partial) -> Result<()> {
    let n = s.count(MAX_GLOBALS as usize)?;
    for _ in 0..n {
        let ty = s.valtype()?;
        let mutable = match s.byte()? {
            0 => false,
            1 => true,
            _ => return Err(bad()),
        };
        let init = match (ty, s.byte()?) {
            (I32, 0x41) => u64::from(s.s32()? as u32),
            (I64, 0x42) => s.s64()? as u64,
            _ => return Err(bad()),
        };
        s.expect(0x0b)?;
        m.globals.push(Global { ty, mutable, init });
    }
    Ok(())
}
fn exports(s: &mut Reader<'_>, m: &mut Partial) -> Result<()> {
    for _ in 0..s.count(2)? {
        let name = s.name()?;
        let kind = s.byte()?;
        let index = s.u32()?;
        match (name, kind) {
            (b"invoke", 0x00) => {
                if m.invoke.is_some() || index as usize >= m.function_types.len() {
                    return Err(bad());
                }
                m.invoke = Some(index);
            }
            (b"memory", 0x02) => {
                if m.memory_export || index != 0 || m.memory.is_none() {
                    return Err(bad());
                }
                m.memory_export = true;
            }
            _ => return Err(bad()),
        }
    }
    Ok(())
}
fn elements(s: &mut Reader<'_>, m: &mut Partial) -> Result<()> {
    let n = s.count(MAX_SEGMENTS as usize)?;
    for _ in 0..n {
        let table = m.table.ok_or_else(bad)?;
        s.expect(0x00)?;
        let offset = s.const_i32()?;
        let count = s.count(usize::MAX)?;
        if u64::from(offset) + count as u64 > u64::from(table) {
            return Err(bad());
        }
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            let f = s.u32()?;
            if f as usize >= m.function_types.len() {
                return Err(bad());
            }
            items.push(f);
        }
        m.elements.push((offset, items));
    }
    Ok(())
}
fn code(s: &mut Reader<'_>, m: &mut Partial) -> Result<()> {
    let n = s.count(MAX_FUNCTIONS as usize)?;
    if n != m.defined {
        return Err(bad());
    }
    m.code = Some(n);
    for i in 0..n {
        let size = s.u32()? as usize;
        let mut b = s.sub(size)?;
        let type_index = m.function_types[m.imports.len() + i];
        let function = body(&mut b, m, type_index)?;
        m.functions.push(function);
    }
    Ok(())
}
fn data(s: &mut Reader<'_>, m: &mut Partial) -> Result<()> {
    let n = s.count(MAX_SEGMENTS as usize)?;
    for _ in 0..n {
        let memory = m.memory.ok_or_else(bad)?;
        s.expect(0x00)?;
        let offset = s.const_i32()?;
        let len = s.count(usize::MAX)?;
        let bytes = s.take(len)?;
        if u64::from(offset) + len as u64 > u64::from(memory.initial) * PAGE {
            return Err(bad());
        }
        m.data_bytes += len as u64;
        m.data.push((offset, bytes.to_vec()));
    }
    Ok(())
}

// ---- Function bodies: the Core 1.0 validation algorithm, emitting code ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Function,
    Block,
    Loop,
    If,
    Else,
}
/// A branch whose target is the end of a frame still open: an op, or a target
/// of a branch table.
#[derive(Clone, Copy)]
enum Fixup {
    Op(usize),
    Table(usize, usize),
}
struct Ctrl {
    kind: Kind,
    result: Option<ValType>,
    /// Operand stack height at entry.
    height: usize,
    unreachable: bool,
    /// Code index of the first body instruction, the target of a loop label.
    start: u32,
    /// The `If` op waiting for its skip target.
    if_op: usize,
    fixups: Vec<Fixup>,
}

struct Body<'m> {
    m: &'m Partial,
    /// The types of parameters and declared locals as runs: each entry is the
    /// index one past its run and the run's type. Declaring many locals at once
    /// costs one entry, not one per local.
    locals: Vec<(u32, ValType)>,
    /// Parameters plus declared locals.
    total: u32,
    result: Option<ValType>,
    vals: Vec<Option<ValType>>,
    ctrls: Vec<Ctrl>,
    ops: Vec<Op>,
    tables: Vec<BranchTable>,
    max: usize,
}

fn body(r: &mut Reader<'_>, m: &Partial, type_index: u32) -> Result<Function> {
    let ty = &m.types[type_index as usize];
    let params = ty.params.len() as u32;
    fn run(locals: &mut Vec<(u32, ValType)>, end: u32, t: ValType) {
        match locals.last_mut() {
            Some(last) if last.1 == t => last.0 = end,
            _ => locals.push((end, t)),
        }
    }
    let mut locals: Vec<(u32, ValType)> = Vec::new();
    let mut total = 0u32;
    for t in &ty.params {
        total += 1;
        run(&mut locals, total, *t);
    }
    // One group per run of locals of one type: no empty group, and no two
    // neighbours of the same type (rules §17).
    let mut previous = None;
    for _ in 0..r.count(usize::MAX)? {
        let n = r.u32()?;
        let t = r.valtype()?;
        if n == 0 || previous == Some(t) {
            return Err(noncanonical());
        }
        previous = Some(t);
        total = total.checked_add(n).ok_or_else(bad)?;
        if total > MAX_LOCALS {
            return Err(bad());
        }
        run(&mut locals, total, t);
    }
    let mut b = Body {
        m,
        locals,
        total,
        result: ty.result,
        vals: Vec::new(),
        ctrls: vec![Ctrl {
            kind: Kind::Function,
            result: ty.result,
            height: 0,
            unreachable: false,
            start: 0,
            if_op: 0,
            fixups: Vec::new(),
        }],
        ops: Vec::new(),
        tables: Vec::new(),
        max: 0,
    };
    loop {
        if r.eof() {
            return Err(bad());
        }
        let opcode = r.byte()?;
        if !allowed_opcode(opcode) {
            return Err(bad());
        }
        let finished = b.instruction(opcode, r)?;
        b.max = b.max.max(b.vals.len());
        if b.total as usize + b.max > MAX_SLOTS as usize {
            return Err(bad());
        }
        if finished {
            break;
        }
    }
    if !r.eof() {
        return Err(bad());
    }
    Ok(Function {
        params,
        locals: total - params,
        result: ty.result.is_some(),
        code: b.ops,
        tables: b.tables,
        info: FunctionInfo {
            locals: total,
            operands: b.max as u32,
        },
    })
}

impl Body<'_> {
    fn emit(&mut self, op: Op) {
        self.ops.push(op);
    }
    fn push(&mut self, t: ValType) {
        self.vals.push(Some(t));
    }
    fn push_type(&mut self, t: Option<ValType>) {
        if let Some(t) = t {
            self.push(t);
        }
    }
    /// Pops an operand; in unreachable code an empty frame yields an unknown.
    fn pop(&mut self) -> Result<Option<ValType>> {
        let c = self.ctrls.last().ok_or_else(mistyped)?;
        if self.vals.len() == c.height {
            return if c.unreachable {
                Ok(None)
            } else {
                Err(mistyped())
            };
        }
        Ok(self.vals.pop().unwrap())
    }
    fn pop_expect(&mut self, t: ValType) -> Result<Option<ValType>> {
        match self.pop()? {
            Some(actual) if actual != t => Err(mistyped()),
            actual => Ok(actual),
        }
    }
    fn pop_type(&mut self, t: Option<ValType>) -> Result<()> {
        if let Some(t) = t {
            self.pop_expect(t)?;
        }
        Ok(())
    }
    fn unreachable(&mut self) {
        let c = self.ctrls.last_mut().expect("open frame");
        self.vals.truncate(c.height);
        c.unreachable = true;
    }
    fn push_ctrl(&mut self, kind: Kind, result: Option<ValType>, if_op: usize) {
        self.ctrls.push(Ctrl {
            kind,
            result,
            height: self.vals.len(),
            unreachable: false,
            start: self.ops.len() as u32,
            if_op,
            fixups: Vec::new(),
        });
    }
    fn pop_ctrl(&mut self) -> Result<Ctrl> {
        let result = self.ctrls.last().ok_or_else(mistyped)?.result;
        self.pop_type(result)?;
        if self.vals.len() != self.ctrls.last().unwrap().height {
            return Err(mistyped());
        }
        Ok(self.ctrls.pop().unwrap())
    }
    fn label(&self, depth: u32) -> Result<usize> {
        let n = self.ctrls.len();
        if depth as usize >= n {
            return Err(mistyped());
        }
        Ok(n - 1 - depth as usize)
    }
    fn label_type(&self, idx: usize) -> Option<ValType> {
        let c = &self.ctrls[idx];
        if c.kind == Kind::Loop { None } else { c.result }
    }
    /// The target of a branch to frame `idx`; a forward target is fixed up
    /// when the frame ends.
    fn target(&mut self, idx: usize, fixup: Fixup) -> Target {
        let c = &mut self.ctrls[idx];
        let height = c.height as u32;
        if c.kind == Kind::Loop {
            Target {
                pc: c.start,
                height,
                keep: false,
            }
        } else {
            c.fixups.push(fixup);
            Target {
                pc: 0,
                height,
                keep: c.result.is_some(),
            }
        }
    }
    fn patch(&mut self, fixups: Vec<Fixup>, pc: u32) {
        for f in fixups {
            match f {
                Fixup::Op(i) => match &mut self.ops[i] {
                    Op::Br(t) | Op::BrIf(t) => t.pc = pc,
                    Op::Else { end } => *end = pc,
                    _ => unreachable!("fixup of a non-branch"),
                },
                Fixup::Table(t, e) => self.tables[t].targets[e].pc = pc,
            }
        }
    }
    /// The type of parameter or local `i`.
    fn local(&self, i: u32) -> Result<ValType> {
        if i >= self.total {
            return Err(mistyped());
        }
        let k = self.locals.partition_point(|(end, _)| *end <= i);
        Ok(self.locals[k].1)
    }
    fn block_type(r: &mut Reader<'_>) -> Result<Option<ValType>> {
        match r.byte()? {
            0x40 => Ok(None),
            0x7f => Ok(Some(I32)),
            0x7e => Ok(Some(I64)),
            _ => Err(bad()),
        }
    }
    fn memory(&self) -> Result<()> {
        if self.m.memory.is_some() {
            Ok(())
        } else {
            Err(mistyped())
        }
    }
    fn memarg(&self, r: &mut Reader<'_>, natural: u32) -> Result<u32> {
        self.memory()?;
        if r.u32()? > natural {
            return Err(mistyped());
        }
        r.u32()
    }
    fn load(
        &mut self,
        r: &mut Reader<'_>,
        natural: u32,
        t: ValType,
        op: fn(u32) -> Op,
    ) -> Result<()> {
        let offset = self.memarg(r, natural)?;
        self.pop_expect(I32)?;
        self.push(t);
        self.emit(op(offset));
        Ok(())
    }
    fn store(
        &mut self,
        r: &mut Reader<'_>,
        natural: u32,
        t: ValType,
        op: fn(u32) -> Op,
    ) -> Result<()> {
        let offset = self.memarg(r, natural)?;
        self.pop_expect(t)?;
        self.pop_expect(I32)?;
        self.emit(op(offset));
        Ok(())
    }
    fn unary(&mut self, t: ValType, op: Op) -> Result<()> {
        self.pop_expect(t)?;
        self.push(t);
        self.emit(op);
        Ok(())
    }
    fn binary(&mut self, t: ValType, op: Op) -> Result<()> {
        self.pop_expect(t)?;
        self.pop_expect(t)?;
        self.push(t);
        self.emit(op);
        Ok(())
    }
    fn compare(&mut self, t: ValType, op: Op) -> Result<()> {
        self.pop_expect(t)?;
        self.pop_expect(t)?;
        self.push(I32);
        self.emit(op);
        Ok(())
    }
    fn convert(&mut self, from: ValType, to: ValType, op: Op) -> Result<()> {
        self.pop_expect(from)?;
        self.push(to);
        self.emit(op);
        Ok(())
    }
    /// Pops a call's parameters and pushes its result. In unreachable code,
    /// once the frame's operands are used up every further pop is an unknown
    /// that matches anything, so the rest are not walked: the work follows the
    /// operands present, not the parameters declared.
    fn call_type(&mut self, type_index: u32) -> Result<()> {
        let index = type_index as usize;
        for i in (0..self.m.types[index].params.len()).rev() {
            let c = self.ctrls.last().ok_or_else(mistyped)?;
            if c.unreachable && self.vals.len() == c.height {
                break;
            }
            let p = self.m.types[index].params[i];
            self.pop_expect(p)?;
        }
        let result = self.m.types[index].result;
        self.push_type(result);
        Ok(())
    }
    /// Validates and compiles one instruction; `Ok(true)` at the function's end.
    fn instruction(&mut self, opcode: u8, r: &mut Reader<'_>) -> Result<bool> {
        match opcode {
            0x00 => {
                self.emit(Op::Unreachable);
                self.unreachable();
            }
            0x01 => self.emit(Op::Nop),
            0x02 => {
                let bt = Self::block_type(r)?;
                self.emit(Op::Block);
                self.push_ctrl(Kind::Block, bt, 0);
            }
            0x03 => {
                let bt = Self::block_type(r)?;
                self.emit(Op::Loop);
                self.push_ctrl(Kind::Loop, bt, 0);
            }
            0x04 => {
                let bt = Self::block_type(r)?;
                self.pop_expect(I32)?;
                let if_op = self.ops.len();
                self.emit(Op::If { skip: 0 });
                self.push_ctrl(Kind::If, bt, if_op);
            }
            0x05 => {
                let frame = self.pop_ctrl()?;
                if frame.kind != Kind::If {
                    return Err(mistyped());
                }
                let else_op = self.ops.len();
                self.emit(Op::Else { end: 0 });
                let after_else = self.ops.len() as u32;
                let Op::If { skip } = &mut self.ops[frame.if_op] else {
                    unreachable!("if frame points at its if");
                };
                *skip = after_else;
                self.push_ctrl(Kind::Else, frame.result, 0);
                let mut fixups = frame.fixups;
                fixups.push(Fixup::Op(else_op));
                self.ctrls.last_mut().unwrap().fixups = fixups;
            }
            0x0b => {
                let frame = self.pop_ctrl()?;
                let end = self.ops.len() as u32;
                if frame.kind == Kind::If {
                    if frame.result.is_some() {
                        return Err(mistyped());
                    }
                    let Op::If { skip } = &mut self.ops[frame.if_op] else {
                        unreachable!("if frame points at its if");
                    };
                    *skip = end;
                }
                self.patch(frame.fixups, end);
                self.push_type(frame.result);
                if frame.kind == Kind::Function {
                    self.emit(Op::End);
                    return Ok(true);
                }
            }
            0x0c => {
                let idx = self.label(r.u32()?)?;
                self.pop_type(self.label_type(idx))?;
                let op = self.ops.len();
                let target = self.target(idx, Fixup::Op(op));
                self.emit(Op::Br(target));
                self.unreachable();
            }
            0x0d => {
                let idx = self.label(r.u32()?)?;
                self.pop_expect(I32)?;
                let lt = self.label_type(idx);
                self.pop_type(lt)?;
                self.push_type(lt);
                let op = self.ops.len();
                let target = self.target(idx, Fixup::Op(op));
                self.emit(Op::BrIf(target));
            }
            0x0e => {
                let n = r.count(usize::MAX)?;
                let mut labels = Vec::with_capacity(n + 1);
                for _ in 0..=n {
                    labels.push(r.u32()?);
                }
                self.pop_expect(I32)?;
                let default = self.label(labels[n])?;
                let result = self.label_type(default);
                let table = self.tables.len();
                let mut t = BranchTable {
                    entries: Vec::with_capacity(n + 1),
                    targets: Vec::new(),
                };
                // Each distinct label is checked and resolved once; an entry
                // only records which of them it names.
                let mut slots = std::collections::BTreeMap::new();
                for l in labels {
                    let idx = self.label(l)?;
                    let slot = match slots.get(&idx) {
                        Some(slot) => *slot,
                        None => {
                            // Core 1.0: every label has the default label's
                            // type, in reachable and unreachable code alike.
                            if self.label_type(idx) != result {
                                return Err(mistyped());
                            }
                            let slot = t.targets.len();
                            t.targets.push(self.target(idx, Fixup::Table(table, slot)));
                            slots.insert(idx, slot);
                            slot
                        }
                    };
                    t.entries.push(slot as u32);
                }
                self.pop_type(result)?;
                self.tables.push(t);
                self.emit(Op::BrTable(table as u32));
                self.unreachable();
            }
            0x0f => {
                self.pop_type(self.result)?;
                self.emit(Op::Return);
                self.unreachable();
            }
            0x10 => {
                let f = r.u32()?;
                let ty = *self.m.function_types.get(f as usize).ok_or_else(mistyped)?;
                self.call_type(ty)?;
                self.emit(Op::Call(f));
            }
            0x11 => {
                let ty = r.u32()?;
                if ty as usize >= self.m.types.len() || self.m.table.is_none() {
                    return Err(mistyped());
                }
                r.expect(0x00)?;
                self.pop_expect(I32)?;
                self.call_type(ty)?;
                self.emit(Op::CallIndirect(ty));
            }
            0x1a => {
                self.pop()?;
                self.emit(Op::Drop);
            }
            0x1b => {
                self.pop_expect(I32)?;
                let t1 = self.pop()?;
                let t2 = self.pop()?;
                match (t1, t2) {
                    (Some(a), Some(b)) if a != b => return Err(mistyped()),
                    _ => self.vals.push(t1.or(t2)),
                }
                self.emit(Op::Select);
            }
            0x20 => {
                let i = r.u32()?;
                let t = self.local(i)?;
                self.push(t);
                self.emit(Op::LocalGet(i));
            }
            0x21 => {
                let i = r.u32()?;
                let t = self.local(i)?;
                self.pop_expect(t)?;
                self.emit(Op::LocalSet(i));
            }
            0x22 => {
                let i = r.u32()?;
                let t = self.local(i)?;
                self.pop_expect(t)?;
                self.push(t);
                self.emit(Op::LocalTee(i));
            }
            0x23 => {
                let i = r.u32()?;
                let g = *self.m.globals.get(i as usize).ok_or_else(mistyped)?;
                self.push(g.ty);
                self.emit(Op::GlobalGet(i));
            }
            0x24 => {
                let i = r.u32()?;
                let g = *self.m.globals.get(i as usize).ok_or_else(mistyped)?;
                if !g.mutable {
                    return Err(mistyped());
                }
                self.pop_expect(g.ty)?;
                self.emit(Op::GlobalSet(i));
            }
            0x28 => self.load(r, 2, I32, Op::I32Load)?,
            0x29 => self.load(r, 3, I64, Op::I64Load)?,
            0x2c => self.load(r, 0, I32, Op::I32Load8S)?,
            0x2d => self.load(r, 0, I32, Op::I32Load8U)?,
            0x2e => self.load(r, 1, I32, Op::I32Load16S)?,
            0x2f => self.load(r, 1, I32, Op::I32Load16U)?,
            0x30 => self.load(r, 0, I64, Op::I64Load8S)?,
            0x31 => self.load(r, 0, I64, Op::I64Load8U)?,
            0x32 => self.load(r, 1, I64, Op::I64Load16S)?,
            0x33 => self.load(r, 1, I64, Op::I64Load16U)?,
            0x34 => self.load(r, 2, I64, Op::I64Load32S)?,
            0x35 => self.load(r, 2, I64, Op::I64Load32U)?,
            0x36 => self.store(r, 2, I32, Op::I32Store)?,
            0x37 => self.store(r, 3, I64, Op::I64Store)?,
            0x3a => self.store(r, 0, I32, Op::I32Store8)?,
            0x3b => self.store(r, 1, I32, Op::I32Store16)?,
            0x3c => self.store(r, 0, I64, Op::I64Store8)?,
            0x3d => self.store(r, 1, I64, Op::I64Store16)?,
            0x3e => self.store(r, 2, I64, Op::I64Store32)?,
            0x3f => {
                r.expect(0x00)?;
                self.memory()?;
                self.push(I32);
                self.emit(Op::MemorySize);
            }
            0x40 => {
                r.expect(0x00)?;
                self.memory()?;
                self.pop_expect(I32)?;
                self.push(I32);
                self.emit(Op::MemoryGrow);
            }
            0x41 => {
                let v = r.s32()? as u32;
                self.push(I32);
                self.emit(Op::I32Const(v));
            }
            0x42 => {
                let v = r.s64()? as u64;
                self.push(I64);
                self.emit(Op::I64Const(v));
            }
            0x45 => self.convert(I32, I32, Op::I32Eqz)?,
            0x46 => self.compare(I32, Op::I32Eq)?,
            0x47 => self.compare(I32, Op::I32Ne)?,
            0x48 => self.compare(I32, Op::I32LtS)?,
            0x49 => self.compare(I32, Op::I32LtU)?,
            0x4a => self.compare(I32, Op::I32GtS)?,
            0x4b => self.compare(I32, Op::I32GtU)?,
            0x4c => self.compare(I32, Op::I32LeS)?,
            0x4d => self.compare(I32, Op::I32LeU)?,
            0x4e => self.compare(I32, Op::I32GeS)?,
            0x4f => self.compare(I32, Op::I32GeU)?,
            0x50 => self.convert(I64, I32, Op::I64Eqz)?,
            0x51 => self.compare(I64, Op::I64Eq)?,
            0x52 => self.compare(I64, Op::I64Ne)?,
            0x53 => self.compare(I64, Op::I64LtS)?,
            0x54 => self.compare(I64, Op::I64LtU)?,
            0x55 => self.compare(I64, Op::I64GtS)?,
            0x56 => self.compare(I64, Op::I64GtU)?,
            0x57 => self.compare(I64, Op::I64LeS)?,
            0x58 => self.compare(I64, Op::I64LeU)?,
            0x59 => self.compare(I64, Op::I64GeS)?,
            0x5a => self.compare(I64, Op::I64GeU)?,
            0x67 => self.unary(I32, Op::I32Clz)?,
            0x68 => self.unary(I32, Op::I32Ctz)?,
            0x69 => self.unary(I32, Op::I32Popcnt)?,
            0x6a => self.binary(I32, Op::I32Add)?,
            0x6b => self.binary(I32, Op::I32Sub)?,
            0x6c => self.binary(I32, Op::I32Mul)?,
            0x6d => self.binary(I32, Op::I32DivS)?,
            0x6e => self.binary(I32, Op::I32DivU)?,
            0x6f => self.binary(I32, Op::I32RemS)?,
            0x70 => self.binary(I32, Op::I32RemU)?,
            0x71 => self.binary(I32, Op::I32And)?,
            0x72 => self.binary(I32, Op::I32Or)?,
            0x73 => self.binary(I32, Op::I32Xor)?,
            0x74 => self.binary(I32, Op::I32Shl)?,
            0x75 => self.binary(I32, Op::I32ShrS)?,
            0x76 => self.binary(I32, Op::I32ShrU)?,
            0x77 => self.binary(I32, Op::I32Rotl)?,
            0x78 => self.binary(I32, Op::I32Rotr)?,
            0x79 => self.unary(I64, Op::I64Clz)?,
            0x7a => self.unary(I64, Op::I64Ctz)?,
            0x7b => self.unary(I64, Op::I64Popcnt)?,
            0x7c => self.binary(I64, Op::I64Add)?,
            0x7d => self.binary(I64, Op::I64Sub)?,
            0x7e => self.binary(I64, Op::I64Mul)?,
            0x7f => self.binary(I64, Op::I64DivS)?,
            0x80 => self.binary(I64, Op::I64DivU)?,
            0x81 => self.binary(I64, Op::I64RemS)?,
            0x82 => self.binary(I64, Op::I64RemU)?,
            0x83 => self.binary(I64, Op::I64And)?,
            0x84 => self.binary(I64, Op::I64Or)?,
            0x85 => self.binary(I64, Op::I64Xor)?,
            0x86 => self.binary(I64, Op::I64Shl)?,
            0x87 => self.binary(I64, Op::I64ShrS)?,
            0x88 => self.binary(I64, Op::I64ShrU)?,
            0x89 => self.binary(I64, Op::I64Rotl)?,
            0x8a => self.binary(I64, Op::I64Rotr)?,
            0xa7 => self.convert(I64, I32, Op::I32WrapI64)?,
            0xac => self.convert(I32, I64, Op::I64ExtendI32S)?,
            0xad => self.convert(I32, I64, Op::I64ExtendI32U)?,
            0xc0 => self.unary(I32, Op::I32Extend8S)?,
            0xc1 => self.unary(I32, Op::I32Extend16S)?,
            0xc2 => self.unary(I64, Op::I64Extend8S)?,
            0xc3 => self.unary(I64, Op::I64Extend16S)?,
            0xc4 => self.unary(I64, Op::I64Extend32S)?,
            0xfc => {
                let sub = r.u32()?;
                if !allowed_prefixed(sub) {
                    return Err(bad());
                }
                match sub {
                    0x0a | 0x0b => {
                        r.expect(0x00)?;
                        if sub == 0x0a {
                            r.expect(0x00)?;
                        }
                        self.memory()?;
                        self.pop_expect(I32)?;
                        self.pop_expect(I32)?;
                        self.pop_expect(I32)?;
                        self.emit(if sub == 0x0a {
                            Op::MemoryCopy
                        } else {
                            Op::MemoryFill
                        });
                    }
                    // Wide arithmetic: four or two i64 operands, two i64 results.
                    _ => {
                        let (operands, op) = match sub {
                            0x13 => (4, Op::I64Add128),
                            0x14 => (4, Op::I64Sub128),
                            0x15 => (2, Op::I64MulWideS),
                            _ => (2, Op::I64MulWideU),
                        };
                        for _ in 0..operands {
                            self.pop_expect(I64)?;
                        }
                        self.push(I64);
                        self.push(I64);
                        self.emit(op);
                    }
                }
            }
            _ => return Err(bad()),
        }
        Ok(false)
    }
}
