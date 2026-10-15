//! Programs are written in WebAssembly text and encoded to the canonical bytes
//! the profile accepts. These helpers compose the common module shapes; a case
//! may always give a complete module instead.
#![allow(dead_code)]
use wasm_encoder::{Module, RawSection};

/// Encodes WebAssembly text. Custom sections, which the profile excludes, are
/// dropped so that a program written here is exactly what is deployed.
pub fn wasm(text: &str) -> Vec<u8> {
    let raw = wat::parse_str(text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let mut module = Module::new();
    for payload in wasmparser::Parser::new(0).parse_all(&raw) {
        let payload = payload.unwrap();
        if let Some((id, range)) = payload.as_section()
            && id != 0
        {
            module.section(&RawSection {
                id,
                data: &raw[range],
            });
        }
    }
    module.finish()
}

/// A module from its parts, one per line.
pub fn module(parts: &[&str]) -> String {
    format!("(module\n  {}\n)", parts.join("\n  "))
}
/// The same, for parts built at run time.
pub fn module_from(parts: &[String]) -> String {
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    module(&parts)
}
/// The exported memory: `min` pages, growable to `max`.
pub fn memory(min: u32, max: u32) -> String {
    format!("(memory (export \"memory\") {min} {max})")
}
/// The entry function.
pub fn invoke(body: &str) -> String {
    format!("(func (export \"invoke\")\n    {body})")
}
/// The host import `name` with its signature (wiki: vm/host), bound to
/// `$name`.
pub fn import(name: &str) -> String {
    let (params, result) = match name {
        "input" | "call_result" => ("i32 i32", " (result i32)"),
        "context" | "storage_get" | "code_hash" => ("i32 i32 i32", " (result i32)"),
        "call" | "deploy" => ("i32 i32 i32 i32 i32", " (result i32)"),
        "storage_set" => ("i32 i32 i32", ""),
        "storage_delete" => ("i32", ""),
        "emit" | "return" | "revert" => ("i32 i32", ""),
        _ => panic!("{name} is not a host import"),
    };
    format!("(import \"ting\" \"{name}\" (func ${name} (param {params}){result}))")
}
/// Bytes as a WebAssembly text string literal.
pub fn escaped(bytes: &[u8]) -> String {
    let body: String = bytes.iter().map(|b| format!("\\{b:02x}")).collect();
    format!("\"{body}\"")
}
