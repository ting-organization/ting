//! The wasmparser-based validator of the execution profile, kept behind the
//! `oracle` feature as a comparison oracle for `decode`. It validates with
//! wasmparser's Core typing, checks the profile's shape rules, and enforces
//! canonical encodings by re-encoding with wasm-encoder and comparing bytes.
use super::profile::{
    FunctionInfo, IMPORTS, MAX_FUNCTIONS, MAX_GLOBALS, MAX_LOCALS, MAX_MODULE, MAX_PAGES,
    MAX_PARAMETERS, MAX_SEGMENTS, MAX_SLOTS, MAX_TYPES, Profile, allowed_opcode,
};
use crate::protocol::Error;
use wasm_encoder::{
    Module,
    reencode::{Reencode, RoundtripReencoder},
};
use wasmparser::{
    DataKind, ElementItems, ElementKind, ExternalKind, Operator, Parser, Payload, RefType, TypeRef,
    ValType, ValidPayload, Validator, WasmFeatures,
};

fn signature(name: &str) -> Option<(Vec<ValType>, Vec<ValType>)> {
    let (args, result) = super::profile::signature(name)?;
    Some((
        vec![ValType::I32; args],
        if result { vec![ValType::I32] } else { vec![] },
    ))
}
fn allowed(op: &Operator<'_>) -> bool {
    match op {
        Operator::CallIndirect { table_index, .. } => *table_index == 0,
        Operator::MemoryCopy { dst_mem, src_mem } => *dst_mem == 0 && *src_mem == 0,
        Operator::MemoryFill { mem } => *mem == 0,
        Operator::I64Add128
        | Operator::I64Sub128
        | Operator::I64MulWideS
        | Operator::I64MulWideU => true,
        _ => false,
    }
}
/// Core 1.0 requires every label of a `br_table`, including the default, to
/// have the same result type, in unreachable code too; wasmparser applies the
/// relaxed rule of later releases, so this tracker restores the pinned one.
fn br_table_labels_agree(bytes: &[u8]) -> Result<bool, Error> {
    let mut types: Vec<wasmparser::FuncType> = Vec::new();
    let mut function_types: Vec<u32> = Vec::new();
    let mut code_index = 0usize;
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(|_| bad())? {
            Payload::TypeSection(reader) => {
                for ty in reader.into_iter_err_on_gc_types() {
                    types.push(ty.map_err(|_| bad())?);
                }
            }
            Payload::ImportSection(reader) => {
                for import in reader {
                    if let TypeRef::Func(index) = import.map_err(|_| bad())?.ty {
                        function_types.push(index);
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                for ty in reader {
                    function_types.push(ty.map_err(|_| bad())?);
                }
            }
            Payload::CodeSectionEntry(body) => {
                let defined = function_types.iter().skip_while(|_| false).count()
                    - (function_types.len() - function_types.len());
                let _ = defined;
                let imports = function_types.len() - count_defined(bytes)?;
                let ty = &types[function_types[imports + code_index] as usize];
                code_index += 1;
                let result = ty.results().first().copied();
                // (is loop, label result) per open frame; the function is the outermost.
                let mut frames: Vec<(bool, Option<ValType>)> = vec![(false, result)];
                let mut reader = body.get_operators_reader().map_err(|_| bad())?;
                while !reader.eof() {
                    match reader.read().map_err(|_| bad())? {
                        Operator::Block { blockty } | Operator::If { blockty } => {
                            frames.push((false, block_result(blockty)?));
                        }
                        Operator::Loop { blockty } => frames.push((true, block_result(blockty)?)),
                        Operator::End => {
                            frames.pop();
                        }
                        Operator::BrTable { targets } => {
                            let label = |depth: u32| -> Option<Option<ValType>> {
                                let frame =
                                    frames.get(frames.len().checked_sub(1 + depth as usize)?)?;
                                Some(if frame.0 { None } else { frame.1 })
                            };
                            let Some(default) = label(targets.default()) else {
                                return Ok(false);
                            };
                            for target in targets.targets() {
                                if label(target.map_err(|_| bad())?) != Some(default) {
                                    return Ok(false);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(true)
}
fn count_defined(bytes: &[u8]) -> Result<usize, Error> {
    for payload in Parser::new(0).parse_all(bytes) {
        if let Payload::FunctionSection(reader) = payload.map_err(|_| bad())? {
            return Ok(reader.count() as usize);
        }
    }
    Ok(0)
}
fn block_result(blockty: wasmparser::BlockType) -> Result<Option<ValType>, Error> {
    match blockty {
        wasmparser::BlockType::Empty => Ok(None),
        wasmparser::BlockType::Type(t) => Ok(Some(t)),
        wasmparser::BlockType::FuncType(_) => Err(bad()),
    }
}
fn integer(t: ValType) -> bool {
    matches!(t, ValType::I32 | ValType::I64)
}
fn bad() -> Error {
    Error("module outside frozen VM profile")
}
fn expr_i32(expr: wasmparser::ConstExpr<'_>) -> Result<u32, Error> {
    let mut reader = expr.get_operators_reader();
    let value = match reader.read().map_err(|_| bad())? {
        Operator::I32Const { value } => value as u32,
        _ => return Err(bad()),
    };
    if !matches!(reader.read().map_err(|_| bad())?, Operator::End) || !reader.eof() {
        return Err(bad());
    }
    Ok(value)
}
/// Declared section counts are checked against the section's own length before
/// any array is allocated: every counted item occupies at least one byte.
/// A section's declared count: within its bound and its bytes, and not zero,
/// since a section with no entries must be left out.
fn preflight(count: u32, range: &std::ops::Range<usize>, bound: u32) -> Result<(), Error> {
    if count == 0 || count > bound || count as usize > range.len() {
        return Err(bad());
    }
    Ok(())
}

pub fn validate(bytes: &[u8]) -> Result<Profile, Error> {
    if bytes.len() > MAX_MODULE || bytes.len() < 8 {
        return Err(bad());
    }
    let features = WasmFeatures::MVP
        .union(WasmFeatures::MUTABLE_GLOBAL)
        .union(WasmFeatures::BULK_MEMORY)
        .union(WasmFeatures::SIGN_EXTENSION)
        .union(WasmFeatures::WIDE_ARITHMETIC)
        .difference(WasmFeatures::FLOATS);
    let mut validator = Validator::new_with_features(features);
    let mut types = Vec::new();
    let mut function_types = Vec::new();
    let mut functions = Vec::new();
    let mut imports = 0u32;
    let mut invoke = None;
    let mut memory = None;
    let mut memory_export = false;
    let mut globals = 0u64;
    let mut data_bytes = 0u64;
    let mut data_segments = 0u64;
    let mut table = None;
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|_| bad())?;
        match &payload {
            Payload::TypeSection(r) => preflight(r.count(), &r.range(), MAX_TYPES)?,
            Payload::ImportSection(r) => preflight(r.count(), &r.range(), IMPORTS)?,
            Payload::FunctionSection(r) => {
                preflight(r.count(), &r.range(), MAX_FUNCTIONS.saturating_sub(imports))?
            }
            Payload::TableSection(r) => preflight(r.count(), &r.range(), 1)?,
            Payload::MemorySection(r) => preflight(r.count(), &r.range(), 1)?,
            Payload::GlobalSection(r) => preflight(r.count(), &r.range(), MAX_GLOBALS)?,
            Payload::ExportSection(r) => preflight(r.count(), &r.range(), 2)?,
            Payload::ElementSection(r) => preflight(r.count(), &r.range(), MAX_SEGMENTS)?,
            Payload::DataSection(r) => preflight(r.count(), &r.range(), MAX_SEGMENTS)?,
            Payload::CodeSectionStart { count, range, .. } => {
                preflight(*count, range, MAX_FUNCTIONS)?
            }
            Payload::Version {
                num: 1,
                encoding: wasmparser::Encoding::Module,
                ..
            }
            | Payload::End(_)
            | Payload::CodeSectionEntry(_) => {}
            _ => return Err(bad()),
        }
        let valid = validator.payload(&payload).map_err(|_| bad())?;
        match payload {
            Payload::Version { .. } | Payload::End(_) | Payload::CodeSectionStart { .. } => {}
            Payload::TypeSection(reader) => {
                for ty in reader.into_iter_err_on_gc_types() {
                    let ty = ty.map_err(|_| bad())?;
                    if ty.params().len() > MAX_PARAMETERS
                        || ty.results().len() > 1
                        || !ty.params().iter().chain(ty.results()).all(|t| integer(*t))
                    {
                        return Err(bad());
                    }
                    types.push(ty);
                }
            }
            Payload::ImportSection(reader) => {
                let mut names = std::collections::BTreeSet::new();
                for import in reader {
                    let import = import.map_err(|_| bad())?;
                    let TypeRef::Func(index) = import.ty else {
                        return Err(bad());
                    };
                    let expected = signature(import.name).ok_or_else(bad)?;
                    let ty = types.get(index as usize).ok_or_else(bad)?;
                    if import.module != "ting"
                        || !names.insert(import.name.to_owned())
                        || ty.params() != expected.0
                        || ty.results() != expected.1
                    {
                        return Err(bad());
                    }
                    imports += 1;
                    function_types.push(index);
                    functions.push(FunctionInfo::default());
                }
            }
            Payload::FunctionSection(reader) => {
                for ty in reader {
                    function_types.push(ty.map_err(|_| bad())?);
                }
                functions.resize(function_types.len(), FunctionInfo::default());
            }
            Payload::TableSection(reader) => {
                for t in reader {
                    let t = t.map_err(|_| bad())?;
                    let ty = t.ty;
                    if ty.element_type != RefType::FUNCREF
                        || ty.table64
                        || ty.shared
                        || ty.maximum != Some(ty.initial)
                        || ty.initial > function_types.len() as u64
                        || !matches!(t.init, wasmparser::TableInit::RefNull)
                    {
                        return Err(bad());
                    }
                    table = Some(ty.initial);
                }
            }
            Payload::MemorySection(reader) => {
                for m in reader {
                    let m = m.map_err(|_| bad())?;
                    if m.memory64
                        || m.shared
                        || m.page_size_log2.is_some()
                        || m.maximum.is_none_or(|v| v > MAX_PAGES)
                        || m.initial > m.maximum.unwrap()
                    {
                        return Err(bad());
                    }
                    memory = Some(m.initial as usize * 65536);
                }
            }
            Payload::GlobalSection(reader) => {
                for g in reader {
                    let g = g.map_err(|_| bad())?;
                    if !integer(g.ty.content_type) {
                        return Err(bad());
                    }
                    let mut e = g.init_expr.get_operators_reader();
                    let op = e.read().map_err(|_| bad())?;
                    if !matches!(op, Operator::I32Const { .. } | Operator::I64Const { .. })
                        || !matches!(e.read().map_err(|_| bad())?, Operator::End)
                        || !e.eof()
                    {
                        return Err(bad());
                    }
                    globals += 1;
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export.map_err(|_| bad())?;
                    match (export.name, export.kind) {
                        ("invoke", ExternalKind::Func) => {
                            if invoke.replace(export.index).is_some() {
                                return Err(bad());
                            }
                        }
                        ("memory", ExternalKind::Memory) if export.index == 0 => {
                            memory_export = true
                        }
                        _ => return Err(bad()),
                    }
                }
            }
            Payload::ElementSection(reader) => {
                for e in reader {
                    let e = e.map_err(|_| bad())?;
                    let ElementKind::Active {
                        table_index: None,
                        offset_expr,
                    } = e.kind
                    else {
                        return Err(bad());
                    };
                    let ElementItems::Functions(items) = e.items else {
                        return Err(bad());
                    };
                    let offset = expr_i32(offset_expr)? as u64;
                    if offset
                        .checked_add(u64::from(items.count()))
                        .is_none_or(|end| end > table.unwrap_or(0))
                    {
                        return Err(bad());
                    }
                    for item in items {
                        item.map_err(|_| bad())?;
                    }
                }
            }
            Payload::DataSection(reader) => {
                for d in reader {
                    let d = d.map_err(|_| bad())?;
                    let DataKind::Active {
                        memory_index: 0,
                        offset_expr,
                    } = d.kind
                    else {
                        return Err(bad());
                    };
                    let offset = expr_i32(offset_expr)? as usize;
                    if offset
                        .checked_add(d.data.len())
                        .is_none_or(|end| end > memory.unwrap_or(0))
                    {
                        return Err(bad());
                    }
                    data_bytes += d.data.len() as u64;
                    data_segments += 1;
                }
            }
            Payload::CodeSectionEntry(body) => {
                let ValidPayload::Func(to_validate, _) = valid else {
                    return Err(bad());
                };
                let index = to_validate.index;
                let params = types
                    .get(*function_types.get(index as usize).ok_or_else(bad)? as usize)
                    .ok_or_else(bad)?
                    .params()
                    .len() as u32;
                let mut fv = to_validate.into_validator(Default::default());
                let mut count = params;
                let mut last_type = None;
                for local in body.get_locals_reader().map_err(|_| bad())? {
                    let (n, t) = local.map_err(|_| bad())?;
                    count = count.checked_add(n).ok_or_else(bad)?;
                    // A group per run: none empty, no neighbours of one type.
                    if count > MAX_LOCALS || !integer(t) || n == 0 || last_type == Some(t) {
                        return Err(bad());
                    }
                    last_type = Some(t);
                }
                fv.read_locals(&mut body.get_binary_reader())
                    .map_err(|_| bad())?;
                let mut reader = body.get_operators_reader().map_err(|_| bad())?;
                let mut max = 0;
                while !reader.eof() {
                    let pos = reader.original_position();
                    let opcode = bytes[pos];
                    if !allowed_opcode(opcode) {
                        return Err(bad());
                    }
                    let op = reader.read().map_err(|_| bad())?;
                    if (opcode == 0xfc || opcode == 0x11) && !allowed(&op) {
                        return Err(bad());
                    }
                    fv.op(pos, &op).map_err(|_| bad())?;
                    max = max.max(fv.operand_stack_height());
                    if count + max > MAX_SLOTS {
                        return Err(bad());
                    }
                }
                if fv.control_stack_height() != 0 {
                    return Err(bad());
                }
                functions[index as usize] = FunctionInfo {
                    locals: count,
                    operands: max,
                };
            }
            _ => return Err(bad()),
        }
    }
    let invoke = invoke.ok_or_else(bad)?;
    if invoke < imports {
        return Err(bad());
    }
    let ty = types
        .get(*function_types.get(invoke as usize).ok_or_else(bad)? as usize)
        .ok_or_else(bad)?;
    if !ty.params().is_empty() || !ty.results().is_empty() || memory.is_some() != memory_export {
        return Err(bad());
    }
    if !br_table_labels_agree(bytes)? {
        return Err(bad());
    }
    // Roundtrip enforces canonical MVP encodings, including minimal LEB fields.
    let mut canonical = Module::new();
    RoundtripReencoder
        .parse_core_module(&mut canonical, Parser::new(0), bytes)
        .map_err(|_| bad())?;
    if canonical.finish() != bytes {
        return Err(Error("noncanonical Wasm encoding"));
    }
    Ok(Profile {
        functions,
        imports,
        types: types.len() as u32,
        invoke,
        memory: memory.unwrap_or(0),
        globals,
        data_bytes,
        data_segments,
        table: table.unwrap_or(0),
    })
}
