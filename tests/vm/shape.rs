//! What the model needs to know about a module, read from its bytes with
//! wasmparser and the Core validation algorithm's stack discipline, never from
//! the decoder under test: the code price, the preparation of an instance,
//! the tier of its initial memory and the slots each function reserves (wiki:
//! vm/lifecycle, vm/metering, resources/prices).
use vm_model::{DATA_SEGMENT, Entry, GLOBAL, PAGE, START, TABLE_ENTRY, Tier, code_price};
use wasmparser::{
    BlockType, DataKind, ExternalKind, FunctionBody, Operator, Parser, Payload, TypeRef,
};

pub struct Shape {
    pub code_price: u64,
    /// Start, initial pages, globals, table entries, data segments and data bytes.
    pub preparation: u64,
    /// The tier of the initial memory.
    pub tier: Tier,
    /// Reserved slots per function index; imports reserve nothing.
    pub slots: Vec<u64>,
    /// The index of the exported `invoke`.
    pub invoke: u32,
}
impl Shape {
    /// The entry costs of invoking the module the first time in a transaction.
    pub fn entry(&self) -> Entry {
        Entry {
            code_price: self.code_price,
            preparation: self.preparation,
            slots: self.slots[self.invoke as usize],
            tier: self.tier,
            cold: true,
        }
    }
}

/// Parameter and result counts of a function type.
type Sig = (u64, u64);

pub fn shape(bytes: &[u8]) -> Shape {
    let mut types: Vec<Sig> = Vec::new();
    let mut funcs: Vec<Sig> = Vec::new();
    let mut declared: Vec<u32> = Vec::new();
    let mut slots: Vec<u64> = Vec::new();
    let mut invoke = None;
    let (mut globals, mut table, mut memory, mut data) = (0u64, 0u64, 0u64, 0u64);
    let mut segments = 0u64;
    let mut bodies = 0usize;
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.unwrap() {
            Payload::TypeSection(r) => {
                for t in r.into_iter_err_on_gc_types() {
                    let t = t.unwrap();
                    types.push((t.params().len() as u64, t.results().len() as u64));
                }
            }
            Payload::ImportSection(r) => {
                for i in r {
                    match i.unwrap().ty {
                        TypeRef::Func(t) => {
                            funcs.push(types[t as usize]);
                            slots.push(0);
                        }
                        other => panic!("a fixture imports {other:?}"),
                    }
                }
            }
            Payload::FunctionSection(r) => {
                for t in r {
                    let t = t.unwrap();
                    declared.push(t);
                    funcs.push(types[t as usize]);
                }
            }
            Payload::TableSection(r) => {
                for t in r {
                    table += t.unwrap().ty.initial;
                }
            }
            Payload::MemorySection(r) => {
                for m in r {
                    memory += m.unwrap().initial * 65_536;
                }
            }
            Payload::GlobalSection(r) => globals += r.count() as u64,
            Payload::ExportSection(r) => {
                for e in r {
                    let e = e.unwrap();
                    if e.name == "invoke" && e.kind == ExternalKind::Func {
                        invoke = Some(e.index);
                    }
                }
            }
            Payload::DataSection(r) => {
                for d in r {
                    let d = d.unwrap();
                    if matches!(d.kind, DataKind::Active { .. }) {
                        data += d.data.len() as u64;
                        segments += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                let (params, results) = funcs[slots.len()];
                let locals: u64 = body
                    .get_locals_reader()
                    .unwrap()
                    .into_iter()
                    .map(|l| u64::from(l.unwrap().0))
                    .sum();
                slots.push(params + locals + max_height(&body, &funcs, &types, results));
                bodies += 1;
            }
            _ => {}
        }
    }
    assert_eq!(bodies, declared.len(), "function and code sections differ");
    let tier = Tier::of(memory);
    Shape {
        code_price: code_price(bytes),
        preparation: START
            + PAGE * (memory / 65_536)
            + GLOBAL * globals
            + TABLE_ENTRY * table
            + DATA_SEGMENT * segments
            + tier.bytes(data),
        tier,
        slots,
        invoke: invoke.expect("the fixture exports invoke"),
    }
}

/// The maximum typed operand-stack height of a body, following the Core
/// validation algorithm: a block records the height it opened at, a branch or
/// `unreachable` makes the rest of the block polymorphic, and an operand popped
/// from a polymorphic region counts as nothing.
fn max_height(body: &FunctionBody, funcs: &[Sig], types: &[Sig], results: u64) -> u64 {
    struct Frame {
        base: u64,
        results: u64,
        polymorphic: bool,
    }
    let mut frames = vec![Frame {
        base: 0,
        results,
        polymorphic: false,
    }];
    let (mut height, mut max) = (0u64, 0u64);
    let block_results = |ty: BlockType| match ty {
        BlockType::Empty => 0,
        BlockType::Type(_) => 1,
        BlockType::FuncType(_) => panic!("a fixture block has a function type"),
    };
    let mut ops = body.get_operators_reader().unwrap();
    while !ops.eof() {
        let op = ops.read().unwrap();
        macro_rules! pop {
            ($n:expr) => {
                for _ in 0..$n {
                    let frame = frames.last().unwrap();
                    if height > frame.base {
                        height -= 1;
                    } else {
                        assert!(frame.polymorphic, "operand stack underflow in a fixture");
                    }
                }
            };
        }
        macro_rules! push {
            ($n:expr) => {
                height += $n
            };
        }
        macro_rules! unreachable_rest {
            () => {{
                let frame = frames.last_mut().unwrap();
                height = frame.base;
                frame.polymorphic = true;
            }};
        }
        use Operator::*;
        match op {
            Unreachable => unreachable_rest!(),
            Nop => {}
            Block { blockty } | Loop { blockty } => frames.push(Frame {
                base: height,
                results: block_results(blockty),
                polymorphic: false,
            }),
            If { blockty } => {
                pop!(1);
                frames.push(Frame {
                    base: height,
                    results: block_results(blockty),
                    polymorphic: false,
                });
            }
            Else => {
                let frame = frames.last_mut().unwrap();
                frame.polymorphic = false;
                height = frame.base;
            }
            End => {
                let frame = frames.pop().unwrap();
                height = frame.base + frame.results;
            }
            Br { .. } | Return => unreachable_rest!(),
            BrIf { .. } => pop!(1),
            BrTable { .. } => {
                pop!(1);
                unreachable_rest!();
            }
            Call { function_index } => {
                let (p, r) = funcs[function_index as usize];
                pop!(p);
                push!(r);
            }
            CallIndirect { type_index, .. } => {
                pop!(1);
                let (p, r) = types[type_index as usize];
                pop!(p);
                push!(r);
            }
            Drop => pop!(1),
            Select => {
                pop!(3);
                push!(1);
            }
            LocalGet { .. } | GlobalGet { .. } | I32Const { .. } | I64Const { .. } => push!(1),
            LocalSet { .. } | GlobalSet { .. } => pop!(1),
            LocalTee { .. } => {
                pop!(1);
                push!(1);
            }
            I32Load { .. }
            | I64Load { .. }
            | I32Load8S { .. }
            | I32Load8U { .. }
            | I32Load16S { .. }
            | I32Load16U { .. }
            | I64Load8S { .. }
            | I64Load8U { .. }
            | I64Load16S { .. }
            | I64Load16U { .. }
            | I64Load32S { .. }
            | I64Load32U { .. } => {
                pop!(1);
                push!(1);
            }
            I32Store { .. }
            | I64Store { .. }
            | I32Store8 { .. }
            | I32Store16 { .. }
            | I64Store8 { .. }
            | I64Store16 { .. }
            | I64Store32 { .. } => pop!(2),
            MemorySize { .. } => push!(1),
            MemoryGrow { .. } => {
                pop!(1);
                push!(1);
            }
            I32Eqz | I64Eqz | I32Clz | I32Ctz | I32Popcnt | I64Clz | I64Ctz | I64Popcnt
            | I32WrapI64 | I64ExtendI32S | I64ExtendI32U | I32Extend8S | I32Extend16S
            | I64Extend8S | I64Extend16S | I64Extend32S => {
                pop!(1);
                push!(1);
            }
            I32Eq | I32Ne | I32LtS | I32LtU | I32GtS | I32GtU | I32LeS | I32LeU | I32GeS
            | I32GeU | I64Eq | I64Ne | I64LtS | I64LtU | I64GtS | I64GtU | I64LeS | I64LeU
            | I64GeS | I64GeU | I32Add | I32Sub | I32Mul | I32DivS | I32DivU | I32RemS
            | I32RemU | I32And | I32Or | I32Xor | I32Shl | I32ShrS | I32ShrU | I32Rotl
            | I32Rotr | I64Add | I64Sub | I64Mul | I64DivS | I64DivU | I64RemS | I64RemU
            | I64And | I64Or | I64Xor | I64Shl | I64ShrS | I64ShrU | I64Rotl | I64Rotr => {
                pop!(2);
                push!(1);
            }
            MemoryCopy { .. } | MemoryFill { .. } => pop!(3),
            I64Add128 | I64Sub128 => {
                pop!(4);
                push!(2);
            }
            I64MulWideS | I64MulWideU => {
                pop!(2);
                push!(2);
            }
            other => panic!("a fixture uses {other:?}, which is outside the profile"),
        }
        max = max.max(height);
    }
    max
}
