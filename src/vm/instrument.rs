//! Private execution instrumentation. Original code remains the committed code.
//!
//! Five private imports are inserted: `charge(i64)` debits an instruction's
//! price before every original instruction, `bytes(i32)` debits the bytes of a
//! bulk memory operation at the current memory tier, `access(i32)` debits a
//! load (0) or a store (1) at the current memory tier, and `enter(i64)` and
//! `leave(i64)` surround every entry of a defined function with its reserved
//! slots, so that the host debits the reservation and the stack pages the
//! entry adds, and lowers the stack's height again after it. Indirect calls go
//! through generated thunks so that the callee's entry is known without
//! knowing the target statically. The prices here are restated from the price list on wasmparser's
//! operators, independently of the reference interpreter's.
use super::profile::{Profile, charged_opcode};
use crate::protocol::Error;
use std::collections::BTreeMap;
use wasm_encoder::{
    reencode::{self, Reencode},
    *,
};

const METER: &str = "__ting_meter_v1";
pub const INSERTED: u32 = 5;

struct Instrument {
    profile: Profile,
    /// Original function type index of every function, imports included.
    function_types: Vec<u32>,
    /// Parameter count of every original type.
    param_counts: Vec<u32>,
    /// Original function index -> thunk function index (instrumented numbering).
    thunks: BTreeMap<u32, u32>,
    inserted: bool,
    bytes: Vec<u8>,
}
impl Instrument {
    fn imports(&mut self, section: &mut ImportSection) {
        section.import(METER, "charge", EntityType::Function(self.profile.types));
        section.import(METER, "bytes", EntityType::Function(self.profile.types + 1));
        section.import(
            METER,
            "access",
            EntityType::Function(self.profile.types + 1),
        );
        section.import(METER, "enter", EntityType::Function(self.profile.types));
        section.import(METER, "leave", EntityType::Function(self.profile.types));
        self.inserted = true;
    }
    fn access_import(&self) -> u32 {
        self.profile.imports + 2
    }
    fn enter_import(&self) -> u32 {
        self.profile.imports + 3
    }
    fn leave_import(&self) -> u32 {
        self.profile.imports + 4
    }
    /// The price of an original instruction other than a load or a store
    /// (wiki: resources/prices).
    fn price(&self, op: &wasmparser::Operator<'_>) -> i64 {
        use wasmparser::Operator as O;
        match op {
            O::Call { function_index } if *function_index < self.profile.imports => 94,
            O::Call { .. } => 9,
            O::CallIndirect { .. } => 50,
            O::MemoryCopy { .. } | O::MemoryFill { .. } => 16,
            O::Loop { .. }
            | O::I32Rotl
            | O::I32Rotr
            | O::I64Rotl
            | O::I64Rotr
            | O::I64Add128
            | O::I64Sub128 => 2,
            O::Select => 3,
            O::I32Eqz
            | O::I32Eq
            | O::I32Ne
            | O::I32LtS
            | O::I32LtU
            | O::I32GtS
            | O::I32GtU
            | O::I32LeS
            | O::I32LeU
            | O::I32GeS
            | O::I32GeU
            | O::I64Eqz
            | O::I64Eq
            | O::I64Ne
            | O::I64LtS
            | O::I64LtU
            | O::I64GtS
            | O::I64GtU
            | O::I64LeS
            | O::I64LeU
            | O::I64GeS
            | O::I64GeU
            | O::I32Clz
            | O::I32Ctz
            | O::I32Popcnt
            | O::I64Clz
            | O::I64Ctz
            | O::I64Popcnt => 4,
            O::I32Mul | O::I64Mul => 5,
            O::I64MulWideS | O::I64MulWideU => 8,
            O::If { .. } | O::BrIf { .. } => 18,
            O::I32DivS
            | O::I32DivU
            | O::I32RemS
            | O::I32RemU
            | O::I64DivS
            | O::I64DivU
            | O::I64RemS
            | O::I64RemU => 33,
            O::BrTable { .. } => 37,
            _ => 1,
        }
    }
    fn charge(&self) -> u32 {
        self.profile.imports
    }
    fn bytes_import(&self) -> u32 {
        self.profile.imports + 1
    }
    fn remap(&self, original: u32) -> u32 {
        if original >= self.profile.imports {
            original + INSERTED
        } else {
            original
        }
    }
    fn reservation(&self, original: u32) -> Option<u64> {
        if original < self.profile.imports {
            return None;
        }
        Some(self.profile.functions[original as usize].slots())
    }
    fn thunk_body(&self, original: u32) -> Function {
        let params = self.param_count(original);
        let mut f = Function::new([]);
        // Reached only through the table: a defined function's entry, or the
        // host call's price for an import.
        let reservation = self.reservation(original);
        match reservation {
            Some(slots) => {
                f.instruction(&Instruction::I64Const(slots as i64));
                f.instruction(&Instruction::Call(self.enter_import()));
            }
            None => {
                f.instruction(&Instruction::I64Const(94));
                f.instruction(&Instruction::Call(self.charge()));
            }
        }
        for i in 0..params {
            f.instruction(&Instruction::LocalGet(i));
        }
        f.instruction(&Instruction::Call(self.remap(original)));
        if let Some(slots) = reservation {
            f.instruction(&Instruction::I64Const(slots as i64));
            f.instruction(&Instruction::Call(self.leave_import()));
        }
        f.instruction(&Instruction::End);
        f
    }
    fn param_count(&self, original: u32) -> u32 {
        self.param_counts[self.function_types[original as usize] as usize]
    }
}
impl Reencode for Instrument {
    type Error = std::convert::Infallible;
    fn function_index(&mut self, index: u32) -> Result<u32, reencode::Error<Self::Error>> {
        Ok(self.remap(index))
    }
    fn parse_type_section(
        &mut self,
        types: &mut TypeSection,
        reader: wasmparser::TypeSectionReader<'_>,
    ) -> Result<(), reencode::Error<Self::Error>> {
        reencode::utils::parse_type_section(self, types, reader)?;
        types.ty().function([ValType::I64], []);
        types.ty().function([ValType::I32], []);
        Ok(())
    }
    fn parse_import_section(
        &mut self,
        imports: &mut ImportSection,
        reader: wasmparser::ImportSectionReader<'_>,
    ) -> Result<(), reencode::Error<Self::Error>> {
        reencode::utils::parse_import_section(self, imports, reader)?;
        self.imports(imports);
        Ok(())
    }
    fn intersperse_section_hook(
        &mut self,
        module: &mut Module,
        after: Option<SectionId>,
        before: Option<SectionId>,
    ) -> Result<(), reencode::Error<Self::Error>> {
        if !self.inserted && after == Some(SectionId::Type) && before != Some(SectionId::Import) {
            let mut imports = ImportSection::new();
            self.imports(&mut imports);
            module.section(&imports);
        }
        Ok(())
    }
    fn parse_function_section(
        &mut self,
        functions: &mut FunctionSection,
        reader: wasmparser::FunctionSectionReader<'_>,
    ) -> Result<(), reencode::Error<Self::Error>> {
        reencode::utils::parse_function_section(self, functions, reader)?;
        for original in self.thunks.keys() {
            functions.function(self.function_types[*original as usize]);
        }
        Ok(())
    }
    fn element_items<'a>(
        &mut self,
        items: wasmparser::ElementItems<'a>,
    ) -> Result<Elements<'a>, reencode::Error<Self::Error>> {
        let wasmparser::ElementItems::Functions(functions) = items else {
            unreachable!("validated profile has function element items");
        };
        let mut mapped = Vec::new();
        for f in functions {
            mapped.push(self.thunks[&f?]);
        }
        Ok(Elements::Functions(mapped.into()))
    }
    fn parse_code_section(
        &mut self,
        code: &mut CodeSection,
        section: wasmparser::CodeSectionReader<'_>,
    ) -> Result<(), reencode::Error<Self::Error>> {
        reencode::utils::parse_code_section(self, code, section)?;
        for original in self.thunks.keys().copied().collect::<Vec<_>>() {
            code.function(&self.thunk_body(original));
        }
        Ok(())
    }
    fn parse_function_body(
        &mut self,
        code: &mut CodeSection,
        body: wasmparser::FunctionBody<'_>,
    ) -> Result<(), reencode::Error<Self::Error>> {
        let mut locals = Vec::new();
        let mut declared = 0u32;
        for pair in body.get_locals_reader()? {
            let (count, ty) = pair?;
            declared += count;
            locals.push((count, self.val_type(ty)?));
        }
        // One private scratch local for bulk operations, outside reference accounting.
        locals.push((1, ValType::I32));
        let index = self.profile.imports + code.len();
        let scratch = self.param_count(index) + declared;
        let mut f = Function::new(locals);
        let mut reader = body.get_operators_reader()?;
        while !reader.eof() {
            let pos = reader.original_position();
            let op = reader.read()?;
            if charged_opcode(self.bytes[pos]) {
                use wasmparser::Operator as O;
                let access = match op {
                    O::I32Load { .. }
                    | O::I64Load { .. }
                    | O::I32Load8S { .. }
                    | O::I32Load8U { .. }
                    | O::I32Load16S { .. }
                    | O::I32Load16U { .. }
                    | O::I64Load8S { .. }
                    | O::I64Load8U { .. }
                    | O::I64Load16S { .. }
                    | O::I64Load16U { .. }
                    | O::I64Load32S { .. }
                    | O::I64Load32U { .. } => Some(0),
                    O::I32Store { .. }
                    | O::I64Store { .. }
                    | O::I32Store8 { .. }
                    | O::I32Store16 { .. }
                    | O::I64Store8 { .. }
                    | O::I64Store16 { .. }
                    | O::I64Store32 { .. } => Some(1),
                    _ => None,
                };
                match access {
                    Some(kind) => {
                        f.instruction(&Instruction::I32Const(kind));
                        f.instruction(&Instruction::Call(self.access_import()));
                    }
                    None => {
                        f.instruction(&Instruction::I64Const(self.price(&op)));
                        f.instruction(&Instruction::Call(self.charge()));
                    }
                }
            }
            let mut left = None;
            match op {
                wasmparser::Operator::Call { function_index } => {
                    if let Some(slots) = self.reservation(function_index) {
                        f.instruction(&Instruction::I64Const(slots as i64));
                        f.instruction(&Instruction::Call(self.enter_import()));
                        left = Some(slots);
                    }
                }
                wasmparser::Operator::MemoryCopy { .. }
                | wasmparser::Operator::MemoryFill { .. } => {
                    f.instruction(&Instruction::LocalTee(scratch));
                    f.instruction(&Instruction::LocalGet(scratch));
                    f.instruction(&Instruction::Call(self.bytes_import()));
                }
                _ => {}
            }
            f.instruction(&self.instruction(op)?);
            if let Some(slots) = left {
                f.instruction(&Instruction::I64Const(slots as i64));
                f.instruction(&Instruction::Call(self.leave_import()));
            }
        }
        code.function(&f);
        Ok(())
    }
}
impl Instrument {
    fn new(bytes: &[u8], profile: &Profile) -> Result<Self, Error> {
        let mut function_types = Vec::new();
        let mut param_counts = Vec::new();
        let mut referenced = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            match payload.map_err(|_| Error("instrumentation failure"))? {
                wasmparser::Payload::TypeSection(reader) => {
                    for ty in reader.into_iter_err_on_gc_types() {
                        let ty = ty.map_err(|_| Error("instrumentation failure"))?;
                        param_counts.push(ty.params().len() as u32);
                    }
                }
                wasmparser::Payload::ImportSection(reader) => {
                    for import in reader {
                        let import = import.map_err(|_| Error("instrumentation failure"))?;
                        if let wasmparser::TypeRef::Func(index) = import.ty {
                            function_types.push(index);
                        }
                    }
                }
                wasmparser::Payload::FunctionSection(reader) => {
                    for ty in reader {
                        function_types.push(ty.map_err(|_| Error("instrumentation failure"))?);
                    }
                }
                wasmparser::Payload::ElementSection(reader) => {
                    for e in reader {
                        let e = e.map_err(|_| Error("instrumentation failure"))?;
                        if let wasmparser::ElementItems::Functions(items) = e.items {
                            for item in items {
                                referenced
                                    .push(item.map_err(|_| Error("instrumentation failure"))?);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        referenced.sort_unstable();
        referenced.dedup();
        let defined = profile.functions.len() as u32 - profile.imports;
        let thunks = referenced
            .into_iter()
            .enumerate()
            .map(|(k, original)| (original, profile.imports + INSERTED + defined + k as u32))
            .collect();
        Ok(Self {
            profile: profile.clone(),
            function_types,
            param_counts,
            thunks,
            inserted: false,
            bytes: bytes.to_vec(),
        })
    }
}
pub fn instrument(bytes: &[u8], profile: &Profile) -> Result<Vec<u8>, Error> {
    let mut output = Module::new();
    let mut encoder = Instrument::new(bytes, profile)?;
    encoder
        .parse_core_module(&mut output, wasmparser::Parser::new(0), bytes)
        .map_err(|_| Error("instrumentation failure"))?;
    Ok(output.finish())
}
