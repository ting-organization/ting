//! The reference interpreter: executes the compiled code of `decode` with the
//! charging rules (rules §21) applied inline.
//!
//! Values are `u64` slots; an i32 occupies the low 32 bits with the rest zero.
//! One value stack holds every activation's parameters, locals and operands,
//! so a function's reserved slots are exactly the stack it can use. Compute is
//! debited at each original instruction's price (rules §15)
//! through a local allowance that mirrors the frames and the block guard, and
//! settled with the meter before anything else can observe it.
use super::{
    decode::{Import, Module, Op, PAGE, Target},
    host::{Host, HostResult, Memory},
    price::{self, Tier},
    runtime::Progress,
};
use crate::{meter::Resources, protocol::Status};
use std::rc::Rc;

/// One function activation: where its locals and operands live on the stack.
#[derive(Clone, Copy)]
struct Activation {
    /// Index among defined functions.
    func: u32,
    pc: u32,
    /// Stack index of local 0.
    base: usize,
    /// Stack index of the first operand.
    operands: usize,
}

pub(super) struct Instance {
    module: Rc<Module>,
    host: Host,
    stack: Vec<u64>,
    frames: Vec<Activation>,
    /// The stack's height and its mark, in slots (rules §21.4).
    height: u64,
    mark: u64,
    memory: Option<Vec<u8>>,
    max_pages: u32,
    /// The prices of the current memory size's tier.
    tier: Tier,
    globals: Vec<u64>,
}

impl Instance {
    /// A fresh instance: initialised globals, the table, memory with its data
    /// segments, and the entry function's activation ready to run.
    pub fn new(module: Rc<Module>, host: Host) -> Self {
        let memory = module.memory.map(|limits| {
            let mut bytes = vec![0u8; limits.initial as usize * PAGE as usize];
            for (offset, data) in &module.data {
                bytes[*offset as usize..*offset as usize + data.len()].copy_from_slice(data);
            }
            bytes
        });
        let max_pages = module.memory.map_or(0, |l| l.maximum);
        let tier = Tier::of(memory.as_ref().map_or(0, |m| m.len() as u64));
        let globals = module.globals.iter().map(|g| g.init).collect();
        let entry = module.invoke - module.imports.len() as u32;
        let locals = module.functions[entry as usize].locals;
        // The entry function's reservation and stack pages are paid before
        // the instance exists, from a height of zero.
        let (_, height, mark) = price::entry(module.functions[entry as usize].info.slots(), 0, 0);
        #[cfg(feature = "coverage")]
        {
            super::coverage::stack("entry", price::stack_pages(mark));
            if let Some(memory) = &memory {
                super::coverage::pages(memory.len() as u64 / PAGE);
            }
        }
        let mut instance = Self {
            module,
            host,
            stack: vec![0; locals as usize],
            frames: vec![Activation {
                func: entry,
                pc: 0,
                base: 0,
                operands: locals as usize,
            }],
            height,
            mark,
            memory,
            max_pages,
            tier,
            globals,
        };
        instance.stack.reserve(64);
        instance
    }
    pub fn host(&self) -> &Host {
        &self.host
    }
    pub fn host_mut(&mut self) -> &mut Host {
        &mut self.host
    }
    pub fn start(&mut self) -> Progress {
        self.run()
    }
    /// Continues after a nested invocation with the child's status as the
    /// result of the `call` or `deploy` import.
    pub fn resume(&mut self, status: i32) -> Progress {
        self.stack.push(u64::from(status as u32));
        self.run()
    }
    /// Settles the compute debited by instruction dispatch since the last
    /// settlement; it always fits, since the allowance mirrors the meter.
    fn settle(&mut self, consumed: &mut u64) {
        if *consumed > 0 {
            self.host
                .runtime
                .charge(Resources::compute(*consumed))
                .expect("dispatch debits within the allowance");
            *consumed = 0;
        }
    }
    /// Calls a host import with the arguments on top of the stack.
    fn import(&mut self, import: Import) -> HostResult<Option<i32>> {
        let n = import.params();
        let at = self.stack.len() - n;
        let mut a = [0i32; 5];
        for (k, v) in self.stack.drain(at..).enumerate() {
            a[k] = v as u32 as i32;
        }
        let mut mem: Memory<'_> = self.memory.as_deref_mut();
        let host = &mut self.host;
        let result = match import {
            Import::Input => host.input(&mut mem, a[0], a[1]).map(Some),
            Import::Context => host.context(&mut mem, a[0], a[1], a[2]).map(Some),
            Import::StorageGet => host.storage_get(&mut mem, a[0], a[1], a[2]).map(Some),
            Import::StorageSet => host.storage_set(&mut mem, a[0], a[1], a[2]).map(|()| None),
            Import::StorageDelete => host.storage_delete(&mut mem, a[0]).map(|()| None),
            Import::CodeHash => host.code_hash(&mut mem, a[0], a[1], a[2]).map(Some),
            Import::Call => host.call(&mut mem, a[0], a[1], a[2], a[3], a[4]).map(Some),
            Import::Deploy => host
                .deploy(&mut mem, a[0], a[1], a[2], a[3], a[4])
                .map(Some),
            Import::CallResult => host.call_result(&mut mem, a[0], a[1]).map(Some),
            Import::Emit => host.emit(&mut mem, a[0], a[1]).map(|()| None),
            Import::Return => host
                .finish(&mut mem, Status::Success, a[0], a[1])
                .map(|()| None),
            Import::Revert => host
                .finish(&mut mem, Status::Revert, a[0], a[1])
                .map(|()| None),
        };
        #[cfg(feature = "coverage")]
        super::coverage::import(
            import,
            match (&result, &self.host.outcome, &self.host.pending) {
                (Ok(_), ..) => "returned".into(),
                (Err(_), Some((status, _)), _) => format!("{status:?}"),
                (Err(_), None, Some(_)) => "handed over".into(),
                (Err(_), None, None) => "interrupted".into(),
            },
        );
        result
    }

    fn run(&mut self) -> Progress {
        let module = self.module.clone();
        let imports = module.imports.len() as u32;
        let mut act = *self.frames.last().expect("an activation to run");
        let mut code: &[Op] = &module.functions[act.func as usize].code;
        let mut fuel = self.host.runtime.compute_headroom();
        let mut consumed = 0u64;

        macro_rules! exit {
            ($progress:expr) => {{
                self.settle(&mut consumed);
                if let Some(frame) = self.frames.last_mut() {
                    frame.pc = act.pc;
                }
                return $progress;
            }};
        }
        macro_rules! trap {
            () => {
                exit!(Progress::Trapped)
            };
        }
        /// One atomic compute debit outside dispatch: reservations, zeroing, bulk bytes.
        macro_rules! debit {
            ($n:expr) => {{
                let n: u64 = $n;
                if n > 0 {
                    self.settle(&mut consumed);
                    match self.host.runtime.charge(Resources::compute(n)) {
                        Ok(()) => fuel = self.host.runtime.compute_headroom(),
                        Err(status) => {
                            self.host.outcome = Some((status, Vec::new()));
                            exit!(Progress::Interrupted);
                        }
                    }
                }
            }};
        }
        macro_rules! pop {
            () => {
                self.stack.pop().expect("validated operand")
            };
        }
        macro_rules! push {
            ($v:expr) => {
                self.stack.push($v)
            };
        }
        macro_rules! push32 {
            ($v:expr) => {
                self.stack.push(u64::from($v as u32))
            };
        }
        macro_rules! bin32 {
            ($f:expr) => {{
                let b = pop!() as u32;
                let a = pop!() as u32;
                push32!($f(a, b));
            }};
        }
        macro_rules! bin64 {
            ($f:expr) => {{
                let b = pop!();
                let a = pop!();
                push!($f(a, b));
            }};
        }
        macro_rules! cmp32 {
            ($f:expr) => {{
                let b = pop!() as u32;
                let a = pop!() as u32;
                push32!(u32::from($f(a, b)));
            }};
        }
        macro_rules! cmp64 {
            ($f:expr) => {{
                let b = pop!();
                let a = pop!();
                push32!(u32::from($f(a, b)));
            }};
        }
        macro_rules! un32 {
            ($f:expr) => {{
                let a = pop!() as u32;
                push32!($f(a));
            }};
        }
        macro_rules! un64 {
            ($f:expr) => {{
                let a = pop!();
                push!($f(a));
            }};
        }
        /// Effective address of a `$width`-byte access, or a trap.
        macro_rules! address {
            ($offset:expr, $width:expr) => {{
                let addr = pop!() as u32;
                let ea = u64::from(addr) + u64::from($offset);
                let len = self.memory.as_ref().expect("validated memory").len() as u64;
                if ea + $width > len {
                    trap!();
                }
                ea as usize
            }};
        }
        macro_rules! load {
            ($offset:expr, $width:expr, $read:expr) => {{
                let i = address!($offset, $width);
                let mem = self.memory.as_ref().expect("validated memory");
                let bytes: [u8; $width] = mem[i..i + $width].try_into().unwrap();
                push!($read(bytes));
            }};
        }
        macro_rules! store {
            ($offset:expr, $width:expr, $write:expr) => {{
                let v = pop!();
                let i = address!($offset, $width);
                let bytes: [u8; $width] = $write(v);
                self.memory.as_mut().expect("validated memory")[i..i + $width]
                    .copy_from_slice(&bytes);
            }};
        }
        /// Takes a structured branch: cut the operand stack, keep the label's value.
        macro_rules! branch {
            ($t:expr) => {{
                let t: Target = $t;
                let keep = act.operands + t.height as usize;
                if t.keep {
                    let v = *self.stack.last().expect("label value");
                    self.stack.truncate(keep);
                    push!(v);
                } else {
                    self.stack.truncate(keep);
                }
                act.pc = t.pc;
            }};
        }
        /// Leaves the current function with its result; `true` when it was the entry.
        macro_rules! leave {
            () => {{
                let f = &module.functions[act.func as usize];
                self.height -= f.info.slots() + price::ACTIVATION;
                let result = if f.result { Some(pop!()) } else { None };
                self.stack.truncate(act.base);
                if let Some(v) = result {
                    push!(v);
                }
                self.frames.pop();
                match self.frames.last() {
                    None => true,
                    Some(parent) => {
                        act = *parent;
                        code = &module.functions[act.func as usize].code;
                        false
                    }
                }
            }};
        }
        /// Enters defined function `$defined`: reservation debit, then a new activation.
        macro_rules! enter {
            ($defined:expr) => {{
                let defined: u32 = $defined;
                let f = &module.functions[defined as usize];
                let (debit, height, mark) = price::entry(f.info.slots(), self.height, self.mark);
                debit!(debit);
                #[cfg(feature = "coverage")]
                super::coverage::stack(
                    "call",
                    price::stack_pages(mark) - price::stack_pages(self.mark),
                );
                (self.height, self.mark) = (height, mark);
                self.frames.last_mut().expect("caller").pc = act.pc;
                let base = self.stack.len() - f.params as usize;
                self.stack.resize(self.stack.len() + f.locals as usize, 0);
                act = Activation {
                    func: defined,
                    pc: 0,
                    base,
                    operands: base + (f.params + f.locals) as usize,
                };
                self.frames.push(act);
                code = &f.code;
            }};
        }
        /// Calls function `$f` of the whole index space: a host import or a defined function.
        macro_rules! call {
            ($f:expr) => {{
                let f: u32 = $f;
                if f < imports {
                    self.settle(&mut consumed);
                    self.frames.last_mut().expect("caller").pc = act.pc;
                    match self.import(module.imports[f as usize]) {
                        Ok(Some(v)) => push32!(v),
                        Ok(None) => {}
                        Err(_) => return Progress::Interrupted,
                    }
                    fuel = self.host.runtime.compute_headroom();
                } else {
                    enter!(f - imports);
                }
            }};
        }

        loop {
            let op = code[act.pc as usize];
            act.pc += 1;
            // Structural delimiters: no dispatch debit.
            match op {
                Op::Else { end } => {
                    #[cfg(feature = "coverage")]
                    super::coverage::instruction(&op);
                    act.pc = end;
                    continue;
                }
                Op::End => {
                    #[cfg(feature = "coverage")]
                    super::coverage::instruction(&op);
                    if leave!() {
                        exit!(Progress::Finished);
                    }
                    continue;
                }
                _ => {}
            }
            // The instruction's price, from the allowance while it fits; once
            // it does not, through the meter, which charges all of it or none.
            let cost = price::instruction(&op, imports, self.tier);
            if fuel < cost {
                debit!(cost);
            } else {
                fuel -= cost;
                consumed += cost;
            }
            // Encountering an opcode whose dispatch is refused is not an
            // execution witness for that opcode.
            #[cfg(feature = "coverage")]
            super::coverage::instruction(&op);
            match op {
                Op::Else { .. } | Op::End => unreachable!("handled above"),
                Op::Unreachable => trap!(),
                Op::Nop | Op::Block | Op::Loop => {}
                Op::If { skip } => {
                    if pop!() as u32 == 0 {
                        act.pc = skip;
                    }
                }
                Op::Br(t) => branch!(t),
                Op::BrIf(t) => {
                    if pop!() as u32 != 0 {
                        branch!(t);
                    }
                }
                Op::BrTable(table) => {
                    let i = pop!() as u32 as usize;
                    let t = &module.functions[act.func as usize].tables[table as usize];
                    let slot = t.entries[i.min(t.entries.len() - 1)];
                    branch!(t.targets[slot as usize]);
                }
                Op::Return => {
                    if leave!() {
                        exit!(Progress::Finished);
                    }
                }
                Op::Call(f) => call!(f),
                Op::CallIndirect(ty) => {
                    let i = pop!() as u32 as usize;
                    let Some(entry) = module.table.get(i) else {
                        trap!();
                    };
                    let Some(f) = *entry else {
                        trap!();
                    };
                    if module.types[module.function_types[f as usize] as usize]
                        != module.types[ty as usize]
                    {
                        trap!();
                    }
                    // An import reached through the table pays the host call's
                    // price where a defined function's reservation would be.
                    if f < imports {
                        debit!(price::HOST_CALL);
                    }
                    call!(f);
                }
                Op::Drop => {
                    pop!();
                }
                Op::Select => {
                    let c = pop!() as u32;
                    let v2 = pop!();
                    let v1 = pop!();
                    push!(if c != 0 { v1 } else { v2 });
                }
                Op::LocalGet(i) => push!(self.stack[act.base + i as usize]),
                Op::LocalSet(i) => {
                    let v = pop!();
                    self.stack[act.base + i as usize] = v;
                }
                Op::LocalTee(i) => {
                    let v = *self.stack.last().expect("validated operand");
                    self.stack[act.base + i as usize] = v;
                }
                Op::GlobalGet(i) => push!(self.globals[i as usize]),
                Op::GlobalSet(i) => {
                    let v = pop!();
                    self.globals[i as usize] = v;
                }
                Op::I32Load(o) => load!(o, 4, |b| u64::from(u32::from_le_bytes(b))),
                Op::I64Load(o) => load!(o, 8, u64::from_le_bytes),
                Op::I32Load8S(o) => load!(o, 1, |b: [u8; 1]| u64::from(b[0] as i8 as i32 as u32)),
                Op::I32Load8U(o) => load!(o, 1, |b: [u8; 1]| u64::from(b[0])),
                Op::I32Load16S(o) => {
                    load!(o, 2, |b| u64::from(i16::from_le_bytes(b) as i32 as u32))
                }
                Op::I32Load16U(o) => load!(o, 2, |b| u64::from(u16::from_le_bytes(b))),
                Op::I64Load8S(o) => load!(o, 1, |b: [u8; 1]| b[0] as i8 as i64 as u64),
                Op::I64Load8U(o) => load!(o, 1, |b: [u8; 1]| u64::from(b[0])),
                Op::I64Load16S(o) => load!(o, 2, |b| i16::from_le_bytes(b) as i64 as u64),
                Op::I64Load16U(o) => load!(o, 2, |b| u64::from(u16::from_le_bytes(b))),
                Op::I64Load32S(o) => load!(o, 4, |b| i32::from_le_bytes(b) as i64 as u64),
                Op::I64Load32U(o) => load!(o, 4, |b| u64::from(u32::from_le_bytes(b))),
                Op::I32Store(o) => store!(o, 4, |v: u64| (v as u32).to_le_bytes()),
                Op::I64Store(o) => store!(o, 8, |v: u64| v.to_le_bytes()),
                Op::I32Store8(o) | Op::I64Store8(o) => store!(o, 1, |v: u64| [v as u8]),
                Op::I32Store16(o) | Op::I64Store16(o) => {
                    store!(o, 2, |v: u64| (v as u16).to_le_bytes())
                }
                Op::I64Store32(o) => store!(o, 4, |v: u64| (v as u32).to_le_bytes()),
                Op::MemorySize => {
                    let pages = self.memory.as_ref().expect("validated memory").len() as u64 / PAGE;
                    push32!(pages);
                }
                Op::MemoryGrow => {
                    let delta = pop!() as u32;
                    let current = (self.memory.as_ref().expect("validated memory").len() as u64
                        / PAGE) as u32;
                    if u64::from(current) + u64::from(delta) > u64::from(self.max_pages) {
                        push32!(u32::MAX);
                    } else {
                        debit!(price::PAGE * u64::from(delta));
                        let bytes = u64::from(delta) * PAGE;
                        let mem = self.memory.as_mut().expect("validated memory");
                        mem.resize(mem.len() + bytes as usize, 0);
                        self.tier = Tier::of(mem.len() as u64);
                        #[cfg(feature = "coverage")]
                        super::coverage::pages(mem.len() as u64 / PAGE);
                        push32!(current);
                    }
                }
                Op::I32Const(v) => push32!(v),
                Op::I64Const(v) => push!(v),
                Op::I32Eqz => un32!(|a: u32| u32::from(a == 0)),
                Op::I32Eq => cmp32!(|a, b| a == b),
                Op::I32Ne => cmp32!(|a, b| a != b),
                Op::I32LtS => cmp32!(|a, b| (a as i32) < (b as i32)),
                Op::I32LtU => cmp32!(|a, b| a < b),
                Op::I32GtS => cmp32!(|a, b| (a as i32) > (b as i32)),
                Op::I32GtU => cmp32!(|a, b| a > b),
                Op::I32LeS => cmp32!(|a, b| (a as i32) <= (b as i32)),
                Op::I32LeU => cmp32!(|a, b| a <= b),
                Op::I32GeS => cmp32!(|a, b| (a as i32) >= (b as i32)),
                Op::I32GeU => cmp32!(|a, b| a >= b),
                Op::I64Eqz => {
                    let a = pop!();
                    push32!(u32::from(a == 0));
                }
                Op::I64Eq => cmp64!(|a, b| a == b),
                Op::I64Ne => cmp64!(|a, b| a != b),
                Op::I64LtS => cmp64!(|a, b| (a as i64) < (b as i64)),
                Op::I64LtU => cmp64!(|a, b| a < b),
                Op::I64GtS => cmp64!(|a, b| (a as i64) > (b as i64)),
                Op::I64GtU => cmp64!(|a, b| a > b),
                Op::I64LeS => cmp64!(|a, b| (a as i64) <= (b as i64)),
                Op::I64LeU => cmp64!(|a, b| a <= b),
                Op::I64GeS => cmp64!(|a, b| (a as i64) >= (b as i64)),
                Op::I64GeU => cmp64!(|a, b| a >= b),
                Op::I32Clz => un32!(|a: u32| a.leading_zeros()),
                Op::I32Ctz => un32!(|a: u32| a.trailing_zeros()),
                Op::I32Popcnt => un32!(|a: u32| a.count_ones()),
                Op::I32Add => bin32!(u32::wrapping_add),
                Op::I32Sub => bin32!(u32::wrapping_sub),
                Op::I32Mul => bin32!(u32::wrapping_mul),
                Op::I32DivS => {
                    let b = pop!() as u32 as i32;
                    let a = pop!() as u32 as i32;
                    if b == 0 || (a == i32::MIN && b == -1) {
                        trap!();
                    }
                    push32!(a.wrapping_div(b) as u32);
                }
                Op::I32DivU => {
                    let b = pop!() as u32;
                    let a = pop!() as u32;
                    if b == 0 {
                        trap!();
                    }
                    push32!(a / b);
                }
                Op::I32RemS => {
                    let b = pop!() as u32 as i32;
                    let a = pop!() as u32 as i32;
                    if b == 0 {
                        trap!();
                    }
                    push32!(a.wrapping_rem(b) as u32);
                }
                Op::I32RemU => {
                    let b = pop!() as u32;
                    let a = pop!() as u32;
                    if b == 0 {
                        trap!();
                    }
                    push32!(a % b);
                }
                Op::I32And => bin32!(|a, b| a & b),
                Op::I32Or => bin32!(|a, b| a | b),
                Op::I32Xor => bin32!(|a, b| a ^ b),
                Op::I32Shl => bin32!(|a: u32, b: u32| a.wrapping_shl(b)),
                Op::I32ShrS => bin32!(|a: u32, b: u32| (a as i32).wrapping_shr(b) as u32),
                Op::I32ShrU => bin32!(|a: u32, b: u32| a.wrapping_shr(b)),
                Op::I32Rotl => bin32!(|a: u32, b: u32| a.rotate_left(b & 31)),
                Op::I32Rotr => bin32!(|a: u32, b: u32| a.rotate_right(b & 31)),
                Op::I64Clz => un64!(|a: u64| u64::from(a.leading_zeros())),
                Op::I64Ctz => un64!(|a: u64| u64::from(a.trailing_zeros())),
                Op::I64Popcnt => un64!(|a: u64| u64::from(a.count_ones())),
                Op::I64Add => bin64!(u64::wrapping_add),
                Op::I64Sub => bin64!(u64::wrapping_sub),
                Op::I64Mul => bin64!(u64::wrapping_mul),
                Op::I64DivS => {
                    let b = pop!() as i64;
                    let a = pop!() as i64;
                    if b == 0 || (a == i64::MIN && b == -1) {
                        trap!();
                    }
                    push!(a.wrapping_div(b) as u64);
                }
                Op::I64DivU => {
                    let b = pop!();
                    let a = pop!();
                    if b == 0 {
                        trap!();
                    }
                    push!(a / b);
                }
                Op::I64RemS => {
                    let b = pop!() as i64;
                    let a = pop!() as i64;
                    if b == 0 {
                        trap!();
                    }
                    push!(a.wrapping_rem(b) as u64);
                }
                Op::I64RemU => {
                    let b = pop!();
                    let a = pop!();
                    if b == 0 {
                        trap!();
                    }
                    push!(a % b);
                }
                Op::I64And => bin64!(|a, b| a & b),
                Op::I64Or => bin64!(|a, b| a | b),
                Op::I64Xor => bin64!(|a, b| a ^ b),
                Op::I64Shl => bin64!(|a: u64, b: u64| a.wrapping_shl(b as u32)),
                Op::I64ShrS => bin64!(|a: u64, b: u64| (a as i64).wrapping_shr(b as u32) as u64),
                Op::I64ShrU => bin64!(|a: u64, b: u64| a.wrapping_shr(b as u32)),
                Op::I64Rotl => bin64!(|a: u64, b: u64| a.rotate_left((b & 63) as u32)),
                Op::I64Rotr => bin64!(|a: u64, b: u64| a.rotate_right((b & 63) as u32)),
                Op::I32WrapI64 => {
                    let a = pop!();
                    push32!(a as u32);
                }
                Op::I64ExtendI32S => {
                    let a = pop!() as u32;
                    push!(a as i32 as i64 as u64);
                }
                Op::I64ExtendI32U => {
                    let a = pop!() as u32;
                    push!(u64::from(a));
                }
                Op::I32Extend8S => un32!(|a: u32| a as u8 as i8 as i32 as u32),
                Op::I32Extend16S => un32!(|a: u32| a as u16 as i16 as i32 as u32),
                Op::I64Extend8S => un64!(|a: u64| a as u8 as i8 as i64 as u64),
                Op::I64Extend16S => un64!(|a: u64| a as u16 as i16 as i64 as u64),
                Op::I64Extend32S => un64!(|a: u64| a as u32 as i32 as i64 as u64),
                Op::MemoryCopy => {
                    let n = pop!() as u32;
                    let s = pop!() as u32;
                    let d = pop!() as u32;
                    // A source and a destination span, when there is a byte.
                    debit!(self.tier.bytes(u64::from(n), 2 * u64::from(n > 0)));
                    let mem = self.memory.as_mut().expect("validated memory");
                    let len = mem.len() as u64;
                    if u64::from(s) + u64::from(n) > len || u64::from(d) + u64::from(n) > len {
                        trap!();
                    }
                    let (s, d, n) = (s as usize, d as usize, n as usize);
                    mem.copy_within(s..s + n, d);
                }
                Op::MemoryFill => {
                    let n = pop!() as u32;
                    let v = pop!() as u32;
                    let d = pop!() as u32;
                    debit!(self.tier.bytes(u64::from(n), u64::from(n > 0)));
                    let mem = self.memory.as_mut().expect("validated memory");
                    if u64::from(d) + u64::from(n) > mem.len() as u64 {
                        trap!();
                    }
                    mem[d as usize..d as usize + n as usize].fill(v as u8);
                }
                Op::I64Add128 | Op::I64Sub128 => {
                    let b_hi = pop!();
                    let b_lo = pop!();
                    let a_hi = pop!();
                    let a_lo = pop!();
                    let a = u128::from(a_lo) | (u128::from(a_hi) << 64);
                    let b = u128::from(b_lo) | (u128::from(b_hi) << 64);
                    let r = if matches!(op, Op::I64Add128) {
                        a.wrapping_add(b)
                    } else {
                        a.wrapping_sub(b)
                    };
                    push!(r as u64);
                    push!((r >> 64) as u64);
                }
                Op::I64MulWideS => {
                    let b = pop!() as i64;
                    let a = pop!() as i64;
                    let r = i128::from(a) * i128::from(b);
                    push!(r as u64);
                    push!((r >> 64) as u64);
                }
                Op::I64MulWideU => {
                    let b = pop!();
                    let a = pop!();
                    let r = u128::from(a) * u128::from(b);
                    push!(r as u64);
                    push!((r >> 64) as u64);
                }
            }
        }
    }
}
