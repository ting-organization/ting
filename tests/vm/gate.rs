//! The deployment gate. Every case program is deployed by a transaction, and a
//! deployment runs the program once as its initializer, so a program that
//! traps or reverts on purpose could never be installed. The harness therefore
//! puts a gate in front of the entry function: an invocation whose input is
//! exactly [`MARK`] bytes long returns at once. The gate reads the input's
//! length with `input`, importing it when the program does not, and needs the
//! program's memory for that call's empty span. Every call pays the gate's
//! instructions, [`ON_CALL`], and the deployment those and its `return`; the
//! harness adds them to what a case declares.
//!
//! A program that starts with storage writes it there: before the gate
//! returns, the deployment puts each record's key and value in memory and
//! writes them with `storage_set`, imported as well when the program does not
//! import it. A call never reaches that code, so it pays the same [`ON_CALL`].
use std::convert::Infallible;
use vm_model::{Ledger, RECORD_BASE, Tier, cost_at, leaf, words};
use wasm_encoder::{
    BlockType, CodeSection, EntityType, Function, ImportSection, Instruction, MemArg, Module,
    SectionId, TypeSection, ValType,
    reencode::{self, Reencode},
};
use wasmparser::{ExternalKind, Parser, Payload, TypeRef};

/// The length of the deployment input that the gate answers.
pub const MARK: usize = 7;
/// What the gate executes on a call.
pub const ON_CALL: &str = "i32.const 0 i32.const 0 call $input i32.const 7 i32.eq if";

/// A record a program's deployment writes: its logical key and its value.
pub type Record = ([u8; 32], Vec<u8>);

/// A program with the gate in front of its entry function.
pub struct Gated {
    pub code: Vec<u8>,
    /// What its deployment does: the gate, the writes of its records and the
    /// return.
    pub deployment: Ledger,
}

/// `code` with the gate in front of its entry function, or `None` when the
/// module declares no memory, which the gate's `input` call needs.
pub fn gated(code: &[u8]) -> Option<Vec<u8>> {
    gated_with(code, &[]).map(|g| g.code)
}
/// The same, with a deployment that writes `records` before it returns.
pub fn gated_with(code: &[u8], records: &[Record]) -> Option<Gated> {
    let mut gate = Gate::scan(code, records)?;
    let deployment = gate.deployment(records);
    let mut module = Module::new();
    gate.parse_core_module(&mut module, Parser::new(0), code)
        .unwrap_or_else(|e| panic!("the gate cannot rewrite the module: {e:?}"));
    Some(Gated {
        code: module.finish(),
        deployment,
    })
}

/// One instruction of a deployment's writes.
#[derive(Clone, Copy)]
enum Op {
    I32(i32),
    I64(i64),
    Store64,
    Store8,
    Fill,
    Set,
}
impl Op {
    fn text(self) -> String {
        match self {
            Op::I32(v) => format!("i32.const {v}"),
            Op::I64(v) => format!("i64.const {v}"),
            Op::Store64 => "i64.store".into(),
            Op::Store8 => "i32.store8".into(),
            Op::Fill => "memory.fill".into(),
            Op::Set => "call $storage_set".into(),
        }
    }
    fn instruction(self, set: u32) -> Instruction<'static> {
        let at = |align| MemArg {
            offset: 0,
            align,
            memory_index: 0,
        };
        match self {
            Op::I32(v) => Instruction::I32Const(v),
            Op::I64(v) => Instruction::I64Const(v),
            Op::Store64 => Instruction::I64Store(at(3)),
            Op::Store8 => Instruction::I32Store8(at(0)),
            Op::Fill => Instruction::MemoryFill(0),
            Op::Set => Instruction::Call(set),
        }
    }
}
/// The instructions that put `bytes` in memory at `at`: a fill for each run
/// of sixteen equal bytes or more, eight bytes at a time otherwise, and single
/// bytes for the rest.
fn put(ops: &mut Vec<Op>, at: usize, bytes: &[u8]) {
    let mut i = 0;
    while i < bytes.len() {
        let run = bytes[i..].iter().take_while(|&&b| b == bytes[i]).count();
        let address = Op::I32((at + i) as i32);
        if run >= 16 {
            ops.extend([
                address,
                Op::I32(bytes[i].into()),
                Op::I32(run as i32),
                Op::Fill,
            ]);
            i += run;
        } else if i + 8 <= bytes.len() {
            let word = i64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
            ops.extend([address, Op::I64(word), Op::Store64]);
            i += 8;
        } else {
            ops.extend([address, Op::I32(bytes[i].into()), Op::Store8]);
            i += 1;
        }
    }
}
/// The writes of `records`: each key at 0 and value at 32, then `storage_set`.
fn writes(records: &[Record]) -> Vec<Op> {
    let mut ops = Vec::new();
    for (key, value) in records {
        put(&mut ops, 0, key);
        put(&mut ops, 32, value);
        ops.extend([
            Op::I32(0),
            Op::I32(32),
            Op::I32(value.len() as i32),
            Op::Set,
        ]);
    }
    ops
}

struct Gate {
    /// Function imports before the gate's own, which shift the defined
    /// functions by the imports the gate adds.
    imports: u32,
    /// The indices of `input` and `storage_set` after rewriting.
    input: u32,
    set: u32,
    /// The imports the gate adds, with their types.
    adds: Vec<(&'static str, u32)>,
    /// Types the gate appends to the type section.
    new_types: Vec<(Vec<ValType>, Vec<ValType>)>,
    /// The entry function's original index, and the code entries seen so far.
    invoke: u32,
    bodies: u32,
    import_written: bool,
    /// The deployment's writes, and the tier of the initial memory.
    writes: Vec<Op>,
    tier: Tier,
}
impl Gate {
    fn scan(code: &[u8], records: &[Record]) -> Option<Self> {
        let (mut types, mut imports, mut found, mut invoke, mut pages) =
            (Vec::new(), 0u32, Vec::new(), None, None);
        for payload in Parser::new(0).parse_all(code) {
            match payload.expect("a case program decodes") {
                Payload::TypeSection(r) => {
                    for ty in r.into_iter_err_on_gc_types() {
                        let ty = ty.expect("a case program decodes");
                        types.push((ty.params().to_vec(), ty.results().to_vec()));
                    }
                }
                Payload::ImportSection(r) => {
                    for import in r {
                        let import = import.expect("a case program decodes");
                        if let TypeRef::Func(_) = import.ty {
                            found.push((import.name.to_string(), imports));
                            imports += 1;
                        }
                    }
                }
                Payload::MemorySection(r) => {
                    for memory in r {
                        pages = Some(memory.expect("a case program decodes").initial);
                    }
                }
                Payload::ExportSection(r) => {
                    for export in r {
                        let export = export.expect("a case program decodes");
                        if export.name == "invoke" && export.kind == ExternalKind::Func {
                            invoke = Some(export.index);
                        }
                    }
                }
                _ => {}
            }
        }
        let pages = pages?;
        let needed = records.iter().map(|(_, v)| 32 + v.len() as u64).max();
        assert!(
            needed.is_none_or(|n| n <= pages * 65_536),
            "a program's initial memory must hold each record it starts with"
        );
        let mut keys: Vec<_> = records.iter().map(|(k, _)| k).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(
            keys.len(),
            records.len(),
            "a program starts with one record per key"
        );
        use wasmparser::ValType::I32;
        let mut needs = vec![("input", vec![I32, I32], vec![I32])];
        if !records.is_empty() {
            needs.push(("storage_set", vec![I32, I32, I32], vec![]));
        }
        let mut gate = Self {
            imports,
            input: 0,
            set: 0,
            adds: Vec::new(),
            new_types: Vec::new(),
            invoke: invoke.expect("a case program exports invoke"),
            bodies: 0,
            import_written: false,
            writes: writes(records),
            tier: Tier::pages(pages),
        };
        for (name, params, results) in needs {
            let index = match found.iter().find(|(n, _)| n == name) {
                Some((_, index)) => *index,
                None => {
                    let signature = (params, results);
                    let ty = match types.iter().position(|t| *t == signature) {
                        Some(ty) => ty as u32,
                        None => {
                            types.push(signature.clone());
                            let convert = |v: &[wasmparser::ValType]| -> Vec<ValType> {
                                v.iter().map(|_| ValType::I32).collect()
                            };
                            gate.new_types
                                .push((convert(&signature.0), convert(&signature.1)));
                            types.len() as u32 - 1
                        }
                    };
                    gate.adds.push((name, ty));
                    imports + gate.adds.len() as u32 - 1
                }
            };
            match name {
                "input" => gate.input = index,
                _ => gate.set = index,
            }
        }
        Some(gate)
    }
    /// What the deployment does: the gate, the writes and the return, at the
    /// prices of the initial memory, and for each record the copy of its key
    /// and value, its leaf, its read, its two accesses and its growth.
    fn deployment(&self, records: &[Record]) -> Ledger {
        let text: Vec<String> = self.writes.iter().map(|op| op.text()).collect();
        let executed = format!("{ON_CALL} {} return", text.join(" "));
        let mut ledger = Ledger {
            dispatch: cost_at(&executed, self.tier),
            ..Default::default()
        };
        for window in self.writes.windows(2) {
            if let [Op::I32(n), Op::Fill] = window {
                ledger.copies += words(*n as u64);
                ledger.spans += u64::from(*n > 0);
            }
        }
        for (_, value) in records {
            let n = value.len() as u64;
            ledger.copies += words(32 + n);
            ledger.spans += 1 + u64::from(n > 0);
            ledger.hashing += leaf(n);
            ledger.reads += 1;
            ledger.accesses += 2;
            ledger.growth += RECORD_BASE + n;
        }
        ledger
    }
    fn write_imports(&mut self, imports: &mut ImportSection) {
        for (name, ty) in &self.adds {
            imports.import("ting", name, EntityType::Function(*ty));
        }
        self.import_written = true;
    }
}
impl Reencode for Gate {
    type Error = Infallible;
    fn function_index(&mut self, index: u32) -> Result<u32, reencode::Error<Infallible>> {
        Ok(if index >= self.imports {
            index + self.adds.len() as u32
        } else {
            index
        })
    }
    fn parse_type_section(
        &mut self,
        types: &mut TypeSection,
        reader: wasmparser::TypeSectionReader<'_>,
    ) -> Result<(), reencode::Error<Infallible>> {
        reencode::utils::parse_type_section(self, types, reader)?;
        for (params, results) in &self.new_types {
            types
                .ty()
                .function(params.iter().copied(), results.iter().copied());
        }
        Ok(())
    }
    fn parse_import_section(
        &mut self,
        imports: &mut ImportSection,
        reader: wasmparser::ImportSectionReader<'_>,
    ) -> Result<(), reencode::Error<Infallible>> {
        reencode::utils::parse_import_section(self, imports, reader)?;
        self.write_imports(imports);
        Ok(())
    }
    fn intersperse_section_hook(
        &mut self,
        module: &mut Module,
        after: Option<SectionId>,
        before: Option<SectionId>,
    ) -> Result<(), reencode::Error<Infallible>> {
        // A module without imports gets an import section after its types.
        if !self.adds.is_empty()
            && !self.import_written
            && after == Some(SectionId::Type)
            && before != Some(SectionId::Import)
        {
            let mut imports = ImportSection::new();
            self.write_imports(&mut imports);
            module.section(&imports);
        }
        Ok(())
    }
    fn parse_function_body(
        &mut self,
        code: &mut CodeSection,
        body: wasmparser::FunctionBody<'_>,
    ) -> Result<(), reencode::Error<Infallible>> {
        let original = self.imports + self.bodies;
        self.bodies += 1;
        if original != self.invoke {
            return reencode::utils::parse_function_body(self, code, body);
        }
        let mut locals = Vec::new();
        for pair in body.get_locals_reader()? {
            let (count, ty) = pair?;
            locals.push((count, self.val_type(ty)?));
        }
        let mut f = Function::new(locals);
        for instruction in [
            Instruction::I32Const(0),
            Instruction::I32Const(0),
            Instruction::Call(self.input),
            Instruction::I32Const(MARK as i32),
            Instruction::I32Eq,
            Instruction::If(BlockType::Empty),
        ] {
            f.instruction(&instruction);
        }
        for op in &self.writes {
            f.instruction(&op.instruction(self.set));
        }
        f.instruction(&Instruction::Return);
        f.instruction(&Instruction::End);
        let mut reader = body.get_operators_reader()?;
        while !reader.eof() {
            let op = reader.read()?;
            f.instruction(&self.instruction(op)?);
        }
        code.function(&f);
        Ok(())
    }
}
