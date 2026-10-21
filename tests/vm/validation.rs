//! Acceptance of modules (rules §17, §18): the shape the profile allows,
//! every structural bound at its edge, the twelve imports and nothing else,
//! the binary rules of minimal and complete encodings, and the typing rules
//! of the Core validation algorithm an own validator can get wrong. A module
//! is accepted or rejected as declared; a rejected one fails its deployment
//! having paid its code price, read from the section framing before
//! validation, and nothing more. An accepted one is deployed with compute
//! for its installation and its initializer's entry and nothing more, and
//! runs out exactly there.
use crate::{
    dsl::{escaped, import, invoke, memory, module, module_from, wasm},
    harness::{context, deployed as built, deployment_bytes},
    shape::shape,
};
use ting_core::{
    meter::Resources,
    protocol::{Payload, Status, Transaction},
    state::State,
    vm::{Runtime, profile},
};
use vm_model::{
    BUDGET, CONTRACT_ID, DEPLOY_ACCESSES, KEY, Ledger, MANIFEST, RECORD_BASE, ROOT,
    TRANSACTION_BASE, Tier, code_hash, code_price, cost_at, deploy_size, installation, words,
};

/// What a top-level deployment of invalid code of `price` consumes (rules
/// §23.1): the root dispatch, the ContractId, the existence
/// check of its manifest with its key and one access, then the code price and
/// the CodeHash in the one debit made before validation, which invalid code
/// keeps; and the envelope.
fn rejected_at(code: &[u8], price: u64) -> [u64; 4] {
    let n = code.len() as u64;
    [
        ROOT + CONTRACT_ID + KEY + price + code_hash(n),
        1,
        TRANSACTION_BASE + deploy_size(n, 0),
        0,
    ]
}

/// What a top-level deployment of valid code consumes up to its
/// initializer's first instruction: the root dispatch, the installation and
/// the initializer's entry, warm since validation prepared the code; the
/// existence check, the two writes and the entry's two reads; the envelope;
/// and both records' growth, which stands when they are rolled back.
fn accepted_at(code: &[u8]) -> [u64; 4] {
    let n = code.len() as u64;
    [
        ROOT + installation(code) + shape(code).entry().warm().compute(),
        DEPLOY_ACCESSES + 2,
        TRANSACTION_BASE + deploy_size(n, 0),
        RECORD_BASE + MANIFEST + RECORD_BASE + n,
    ]
}

/// `code` is accepted or rejected as declared. A rejected module's deployment
/// consumes what [`rejected_at`] states, at the model's code price. An
/// accepted one's, given compute up to its initializer's first instruction
/// and no more, ends there with status 3, the first priced instruction (or
/// the identifier, after an empty body) refused: what [`accepted_at`] states.
pub(crate) fn expect(name: &str, code: &[u8], accept: bool) {
    verdict(name, code, accept);
    if accept {
        let consumed = accepted_at(code);
        deployment_bytes(code)
            .named(name)
            .deploy()
            .budget([consumed[0], BUDGET[1], BUDGET[2], BUDGET[3]])
            .expect(Status::Compute)
            .consumed(consumed)
            .run();
    } else {
        deployment_bytes(code)
            .named(name)
            .deploy()
            .expect(Status::InvalidModule)
            .consumed(rejected_at(code, code_price(code)))
            .run();
    }
}
/// `code` is accepted or rejected as declared.
fn verdict(name: &str, code: &[u8], accept: bool) {
    let result = profile::validate(code);
    assert_eq!(result.is_ok(), accept, "{name}: {result:?}");
}
fn accepts(name: &str, text: &str) {
    expect(name, &wasm(text), true);
}
fn rejects(name: &str, text: &str) {
    expect(name, &wasm(text), false);
}
/// The decoder's reason for rejecting `code`.
pub(crate) fn reason(code: &[u8]) -> &'static str {
    profile::validate(code).unwrap_err().0
}
const SHAPE: &str = "module outside frozen VM profile";
pub(crate) const ENCODING: &str = "noncanonical Wasm encoding";
pub(crate) const TYPING: &str = "function body fails Core validation";

/// A module with the entry function's body alone.
fn body(instructions: &str) -> String {
    module(&[&invoke(instructions)])
}

// ---- Raw encodings, for what no text can express ---------------------------------------

pub(crate) fn leb(mut n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}
pub(crate) fn vec_of(items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = leb(items.len() as u64);
    for item in items {
        out.extend(item);
    }
    out
}
pub(crate) fn name(s: &str) -> Vec<u8> {
    let mut out = leb(s.len() as u64);
    out.extend(s.as_bytes());
    out
}
fn section(id: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![id];
    out.extend(leb(payload.len() as u64));
    out.extend(payload);
    out
}
pub(crate) fn assemble(parts: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"\0asm\x01\0\0\0".to_vec();
    for (id, payload) in parts {
        out.extend(section(*id, payload));
    }
    out
}
/// The smallest accepted module as sections: one type, one function, the
/// `invoke` export and a body of `nop`.
pub(crate) fn simple() -> Vec<(u8, Vec<u8>)> {
    simple_with(&[0x01])
}
pub(crate) fn simple_with(code: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut function = leb(0);
    function.extend(code);
    function.push(0x0b);
    let mut entry = leb(function.len() as u64);
    entry.extend(function);
    let mut export = name("invoke");
    export.extend([0x00, 0x00]);
    vec![
        (1, vec_of(&[vec![0x60, 0x00, 0x00]])),
        (3, vec_of(&[leb(0)])),
        (7, vec_of(&[export])),
        (10, vec_of(&[entry])),
    ]
}
fn with_section(mut parts: Vec<(u8, Vec<u8>)>, at: usize, id: u8, payload: Vec<u8>) -> Vec<u8> {
    parts.insert(at, (id, payload));
    assemble(&parts)
}

#[test]
pub(crate) fn the_structural_bounds_at_their_edges() {
    accepts("minimal", &body(""));
    for n in [32, 1000, 1001] {
        expect(
            &format!("{n} parameters"),
            &wasm(&module(&[
                &format!("(func (param {}))", "i32 ".repeat(n)),
                &invoke(""),
            ])),
            n <= 1000,
        );
    }
    for n in [1024, 16384, 16385] {
        expect(
            &format!("{n} locals"),
            &wasm(&body(&format!("(local {})", "i32 ".repeat(n)))),
            n <= 16384,
        );
    }
    for (locals, accept) in [(15384, true), (15385, false)] {
        expect(
            &format!("1000 parameters and {locals} locals"),
            &wasm(&module(&[
                &format!(
                    "(func (param {}) (local {}))",
                    "i32 ".repeat(1000),
                    "i32 ".repeat(locals)
                ),
                &invoke(""),
            ])),
            accept,
        );
    }
    for n in [1024, 1025] {
        let parts: Vec<String> = (0..n)
            .map(|_| "(global (mut i32) (i32.const 0))".to_string())
            .chain([invoke("")])
            .collect();
        accepts(&format!("{n} globals"), &module_from(&parts));
    }
    for n in [4096, 4097] {
        let types: Vec<String> = (0..n)
            .map(|_| "(type (func))".to_string())
            .chain([invoke("")])
            .collect();
        accepts(&format!("{n} types"), &module_from(&types));
        let functions: Vec<String> = (0..n)
            .map(|_| "(func)".to_string())
            .chain([invoke("")])
            .collect();
        accepts(&format!("{n} functions"), &module_from(&functions));
        let segments: Vec<String> = [memory(1, 1)]
            .into_iter()
            .chain((0..n).map(|_| "(data (i32.const 0) \"\")".to_string()))
            .chain([invoke("")])
            .collect();
        accepts(&format!("{n} data segments"), &module_from(&segments));
    }
    for depth in [128, 129, 2000] {
        accepts(
            &format!("nesting {depth}"),
            &body(&format!("{}{}", "(block ".repeat(depth), ")".repeat(depth))),
        );
    }
    for n in [32768, 32769] {
        expect(
            &format!("operand height {n}"),
            &wasm(&body(&format!(
                "{}{}",
                "i64.const 0 ".repeat(n),
                "drop ".repeat(n)
            ))),
            n <= 32768,
        );
    }
    for (height, accept) in [(16384, true), (16385, false)] {
        expect(
            &format!("16384 locals and height {height}"),
            &wasm(&body(&format!(
                "(local {}) {}{}",
                "i64 ".repeat(16384),
                "i64.const 0 ".repeat(height),
                "drop ".repeat(height)
            ))),
            accept,
        );
    }
}

/// The counts of the bounds table at their documented edges, and one function
/// body bounded by nothing but the module. Modules this large are past the
/// standard budget's data, so only the verdict is checked: what a rejected
/// deployment consumes is shown at small sizes above.
#[test]
pub(crate) fn memory_declarations() {
    for maximum in [0, 1024, 1025, 65536, 65537] {
        expect(
            &format!("maximum {maximum}"),
            &wasm(&module(&[&memory(0, maximum), &invoke("")])),
            maximum <= 65536,
        );
    }
    rejects(
        "minimum above maximum",
        &module(&[&memory(2, 1), &invoke("")]),
    );
    rejects(
        "no maximum",
        &module(&["(memory (export \"memory\") 1)", &invoke("")]),
    );
    rejects("not exported", &module(&["(memory 1 1)", &invoke("")]));
    rejects(
        "exported under another name",
        &module(&["(memory (export \"mem\") 1 1)", &invoke("")]),
    );
    let one = vec![0x01, 0x00, 0x01];
    expect(
        "two memories",
        &with_section(simple(), 2, 5, vec_of(&[one.clone(), one])),
        false,
    );
    expect(
        "shared memory",
        &with_section(simple(), 2, 5, vec_of(&[vec![0x03, 0x00, 0x01]])),
        false,
    );
    expect(
        "memory64",
        &with_section(simple(), 2, 5, vec_of(&[vec![0x05, 0x00, 0x01]])),
        false,
    );
    for (offset, payload, accept) in [
        (65536, "", true),
        (65536, "x", false),
        (65535, "x", true),
        (-1, "", false),
    ] {
        expect(
            &format!("data of {} bytes at {offset}", payload.len()),
            &wasm(&module(&[
                &memory(1, 1),
                &format!("(data (i32.const {offset}) \"{payload}\")"),
                &invoke(""),
            ])),
            accept,
        );
    }
    // Module size is bounded by the deploying transaction's data, not by a constant.
    let large = module(&[
        &memory(5, 5),
        &format!("(data (i32.const 0) {})", escaped(&vec![0; 300_000])),
        &invoke(""),
    ]);
    let profile = profile::validate(&wasm(&large)).unwrap();
    assert_eq!((profile.memory, profile.data_bytes), (5 * 65536, 300_000));
}

#[test]
pub(crate) fn imports_are_the_twelve_host_functions_with_their_exact_signatures() {
    const HOSTS: [&str; 12] = [
        "input",
        "context",
        "storage_get",
        "storage_set",
        "storage_delete",
        "code_hash",
        "call",
        "deploy",
        "call_result",
        "emit",
        "return",
        "revert",
    ];
    let all: Vec<String> = HOSTS
        .iter()
        .map(|h| import(h))
        .chain([invoke("")])
        .collect();
    accepts("all twelve", &module_from(&all));
    for host in HOSTS {
        // One parameter too many.
        let wrong = import(host).replace("(param ", "(param i32 ");
        rejects(
            &format!("{host} with a wrong signature"),
            &module(&[&wrong, &invoke("")]),
        );
    }
    rejects(
        "a duplicate import",
        &module(&[
            &import("input"),
            "(import \"ting\" \"input\" (func (param i32 i32) (result i32)))",
            &invoke(""),
        ]),
    );
    rejects(
        "an unknown name",
        &module(&["(import \"ting\" \"hash\" (func))", &invoke("")]),
    );
    rejects(
        "an unknown namespace",
        &module(&[
            "(import \"other\" \"input\" (func (param i32 i32) (result i32)))",
            &invoke(""),
        ]),
    );
    rejects(
        "a private meter",
        &module(&[
            "(import \"__ting_meter_v1\" \"charge\" (func (param i64)))",
            &invoke(""),
        ]),
    );
    rejects(
        "a non-function import",
        &module(&["(import \"ting\" \"memory\" (memory 1 1))", &invoke("")]),
    );
    rejects(
        "input without parameters",
        &module(&["(import \"ting\" \"input\" (func))", &invoke("")]),
    );
}

#[test]
pub(crate) fn consensus_extensions_are_rejected_even_when_the_program_never_uses_them() {
    // An unused import is still a protocol declaration. These concrete names
    // make plausible native/application features observable in the seed;
    // the frozen rule rejects every name outside the twelve-import list.
    for host in [
        "sha256",
        "verify_signature",
        "ed25519_verify",
        "secp256k1_verify",
        "balance",
        "transfer",
        "gas_left",
        "block_hash",
        "random",
        "clock",
        "upgrade",
        "feature",
        "version",
    ] {
        for parameters in 0..=8 {
            for result in [false, true] {
                let declaration = format!(
                    "(import \"ting\" \"{host}\" (func (param {}) {}))",
                    "i32 ".repeat(parameters),
                    if result { "(result i32)" } else { "" }
                );
                rejects(
                    &format!("freeze/unused-extension/{host}/{parameters}/{result}"),
                    &module(&[&declaration, &invoke("nop")]),
                );
            }
        }
    }
    for namespace in ["env", "wasi_snapshot_preview1", "ethereum", "ting.v2"] {
        rejects(
            &format!("freeze/namespace/{namespace}"),
            &module(&[
                &format!("(import \"{namespace}\" \"input\" (func (param i32 i32) (result i32)))"),
                &invoke("nop"),
            ]),
        );
    }
    for section in ["ting.version", "ting.features", "consensus"] {
        let mut parts = simple();
        let mut payload = name(section);
        payload.extend(b"version=2;optional=true");
        parts.insert(0, (0, payload));
        expect(
            &format!("freeze/opt-in-custom-section/{section}"),
            &assemble(&parts),
            false,
        );
    }
}

#[test]
pub(crate) fn exports_are_invoke_and_the_memory_alone() {
    rejects("no invoke", "(module (func (export \"other\")))");
    rejects(
        "a second function export",
        "(module (func (export \"invoke\")) (func (export \"other\")))",
    );
    rejects(
        "invoke with a parameter",
        "(module (func (export \"invoke\") (param i32)))",
    );
    rejects(
        "invoke with a result",
        "(module (func (export \"invoke\") (result i32) i32.const 0))",
    );
    rejects(
        "an import exported as invoke",
        "(module (import \"ting\" \"emit\" (func (param i32 i32))) (export \"invoke\" (func 0)))",
    );
}

#[test]
pub(crate) fn instructions_outside_the_profile_are_rejected() {
    for (what, instructions) in [
        ("a float constant", "f32.const 1 drop"),
        (
            "bulk memory without a memory",
            "i32.const 0 i32.const 0 i32.const 0 memory.copy",
        ),
        ("table.get", "i32.const 0 table.get 0 drop"),
        ("table.size", "table.size 0 drop"),
        ("table.grow", "ref.null func i32.const 1 table.grow 0 drop"),
        (
            "table.fill",
            "i32.const 0 ref.null func i32.const 1 table.fill 0",
        ),
        (
            "table.copy",
            "i32.const 0 i32.const 0 i32.const 0 table.copy 0 0",
        ),
        ("a reference", "ref.null func drop"),
        (
            "saturating truncation",
            "f32.const 0 i32.trunc_sat_f32_s drop",
        ),
        ("simd", "v128.const i32x4 0 0 0 0 drop"),
        (
            "typed select",
            "i32.const 1 i32.const 2 i32.const 0 select (result i32) drop",
        ),
        (
            "call_indirect without a table",
            "i32.const 0 call_indirect (type 0)",
        ),
        ("a label out of range", "br 1"),
        ("an operand missing", "drop"),
        (
            "an operand of the wrong type",
            "i64.const 0 i32.const 0 i32.add drop",
        ),
        ("a result left over", "i32.const 0"),
        ("an unknown local", "local.get 0 drop"),
    ] {
        rejects(what, &body(instructions));
    }
    rejects(
        "memory.init",
        &module(&[
            &memory(1, 1),
            "(data $d \"x\")",
            &invoke("i32.const 0 i32.const 0 i32.const 1 memory.init $d"),
        ]),
    );
    rejects(
        "data.drop",
        &module(&[&memory(1, 1), "(data $d \"x\")", &invoke("data.drop $d")]),
    );
    rejects(
        "a start function",
        "(module (func $s) (start $s) (func (export \"invoke\")))",
    );
    accepts(
        "sign extension",
        &body("i32.const 1 i32.extend8_s drop i64.const 1 i64.extend32_s drop"),
    );
    accepts(
        "bulk memory with a memory",
        &module(&[
            &memory(1, 65536),
            &invoke(
                "i32.const 0 i32.const 0 i32.const 8 memory.copy i32.const 0 i32.const 0 i32.const 8 memory.fill",
            ),
        ]),
    );
    accepts(
        "untyped select",
        &body("i32.const 1 i32.const 2 i32.const 0 select drop"),
    );
    accepts(
        "wide arithmetic",
        &body(
            "i64.const 1 i64.const 2 i64.const 3 i64.const 4 i64.add128 drop drop
            i64.const 1 i64.const 2 i64.const 3 i64.const 4 i64.sub128 drop drop
            i64.const 1 i64.const 2 i64.mul_wide_s drop drop
            i64.const 1 i64.const 2 i64.mul_wide_u drop drop",
        ),
    );
    // The first-byte whitelist and the six prefixed instructions: the two bulk
    // memory operations and the four of wide arithmetic.
    assert_eq!(
        (0u8..=255).filter(|b| profile::allowed_opcode(*b)).count(),
        110
    );
    assert_eq!(
        (0u32..=0x1_0000)
            .filter(|s| profile::allowed_prefixed(*s))
            .collect::<Vec<_>>(),
        [0x0a, 0x0b, 0x13, 0x14, 0x15, 0x16]
    );
    // Every other fc sub-opcode is rejected, those of other proposals and the
    // unassigned ones alike, 0x17 right after wide arithmetic included. Each
    // stands where two i64 operands are available and two values dropped
    // after it, so that only the sub-opcode is wrong.
    for sub in [
        0x00u8, 0x07, 0x08, 0x09, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x17, 0x18, 0x7f,
    ] {
        let code = assemble(&simple_with(&[
            0x42, 0x01, 0x42, 0x02, 0xfc, sub, 0x1a, 0x1a,
        ]));
        expect(&format!("fc {sub:#04x}"), &code, false);
    }
    // The same place accepts the four wide-arithmetic sub-opcodes.
    for (sub, operands) in [(0x13u8, 4), (0x14, 4), (0x15, 2), (0x16, 2)] {
        let mut code: Vec<u8> = (1..=operands).flat_map(|v| [0x42, v]).collect();
        code.extend([0xfc, sub, 0x1a, 0x1a]);
        expect(
            &format!("fc {sub:#04x}"),
            &assemble(&simple_with(&code)),
            true,
        );
    }
}

#[test]
pub(crate) fn typing_rules_of_the_core_validation_algorithm() {
    let cases: &[(&str, bool)] = &[
        // Polymorphic stacks after unreachable, br and return.
        ("unreachable i32.add drop", true),
        ("unreachable i64.add i32.add drop", false),
        ("(block (result i32) unreachable) drop", true),
        ("(block (result i32) br 0) drop", false),
        (
            "(block (result i32) i32.const 1 br 0 i64.const 2) drop",
            false,
        ),
        (
            "(block (result i32) i32.const 1 br 0 i64.const 2 drop) drop",
            true,
        ),
        ("(block (result i32) return i64.const 2) drop", false),
        ("(block (result i64) i32.const 1 br 0) drop", false),
        ("unreachable i32.const 0 select drop", true),
        ("unreachable i32.const 1 i32.const 0 select drop", true),
        (
            "unreachable i64.const 1 i32.const 0 select i32.add drop",
            false,
        ),
        (
            "unreachable i64.const 1 i32.const 0 select i64.add drop",
            true,
        ),
        (
            "unreachable i32.const 1 i32.const 0 select i32.add drop",
            true,
        ),
        // br_table: every label has the default's type, Core 1.0, in
        // unreachable code too.
        (
            "(block (result i32) (block (result i32) i32.const 1 i32.const 0 br_table 0 1)) drop",
            true,
        ),
        (
            "(block (result i32) (block i32.const 1 i32.const 0 br_table 0 1) i32.const 2) drop",
            false,
        ),
        (
            "(block (result i64) (block (result i32) unreachable br_table 0 1) drop i64.const 1) drop",
            false,
        ),
        (
            "(block (result i64) (block (result i32) unreachable br_table 1 0) drop i64.const 1) drop",
            false,
        ),
        (
            "(block (result i64) (block (result i32) i32.const 1 i32.const 0 br_table 0 1) drop i64.const 1) drop",
            false,
        ),
        (
            "(block (result i32) (block (result i32) unreachable br_table 0 1 0)) drop",
            true,
        ),
        ("(block (block i32.const 0 br_table 0 1 0 1 0))", true),
        ("i32.const 0 br_table 0", true),
        ("(block (loop unreachable br_table 0 1))", true),
        (
            "(block (result i32) (loop unreachable br_table 0 1)) drop",
            false,
        ),
        (
            "(block (result i32) (block (result i32) i32.const 1 i32.const 0 br_table 0 1 1)) drop",
            true,
        ),
        (
            "(block (result i32) (block (result i32) unreachable br_table 0 1) drop i32.const 1)",
            false,
        ),
        // select operand types.
        ("i32.const 1 i64.const 2 i32.const 0 select drop", false),
        // if and else results.
        (
            "i32.const 1 (if (result i32) (then i32.const 1)) drop",
            false,
        ),
        (
            "i32.const 1 (if (result i32) (then i32.const 1) (else i32.const 2)) drop",
            true,
        ),
        (
            "i32.const 1 (if (result i32) (then i32.const 1) (else i64.const 2)) drop",
            false,
        ),
        ("i32.const 1 (if (then) (else))", true),
        ("i32.const 1 (if (then nop) (else nop))", true),
        ("i32.const 1 (if (then nop))", true),
        // Loops branch to their start and carry no value there.
        ("(loop (result i32) i32.const 1 br 0) drop", true),
        (
            "(loop (result i32) i32.const 1 br_if 0 i32.const 2) drop",
            true,
        ),
        ("(loop (result i32) i32.const 1) drop", true),
        ("(block (result i32)) drop", false),
        // Function-level labels.
        ("br 0", true),
        ("i32.const 1 br 0", true),
        // An else needs an if; a loop label carries nothing.
        (
            "(block (result i32) (loop (result i32) i32.const 1 br 0)) drop",
            true,
        ),
    ];
    for (instructions, accept) in cases {
        expect(instructions, &wasm(&body(instructions)), *accept);
    }
    let modules: &[(&str, &str, bool)] = &[
        (
            "a helper's label result",
            "(module (func $f (result i32) i32.const 1 br 0) (func (export \"invoke\") call $f drop))",
            true,
        ),
        (
            "a helper's label without its result",
            "(module (func $f (result i32) br 0) (func (export \"invoke\") call $f drop))",
            false,
        ),
        (
            "a helper's early return of the wrong type",
            "(module (func $f (result i32) i32.const 1 return i64.const 2) (func (export \"invoke\") call $f drop))",
            false,
        ),
        (
            "a br_table to the function label with the right type",
            "(module (func $f (result i32) (block (result i32) i32.const 1 i32.const 0 br_table 0 1)) (func (export \"invoke\") call $f drop))",
            true,
        ),
        (
            "a br_table mixing an i32 block and an i64 function label",
            "(module (func $f (result i64) (block (result i32) unreachable br_table 0 1) drop i64.const 1) (func (export \"invoke\") call $f drop))",
            false,
        ),
        (
            "a local of the wrong type",
            "(module (func (export \"invoke\") (local i64) i32.const 1 local.set 0))",
            false,
        ),
        (
            "a tee of the right type",
            "(module (func (export \"invoke\") (local i64) i64.const 1 local.tee 0 drop))",
            true,
        ),
        (
            "writing an immutable global",
            "(module (global i32 (i32.const 1)) (func (export \"invoke\") i32.const 2 global.set 0))",
            false,
        ),
        (
            "writing a mutable global",
            "(module (global (mut i32) (i32.const 1)) (func (export \"invoke\") i32.const 2 global.set 0))",
            true,
        ),
        (
            "a global initialised with the wrong type",
            "(module (global (mut i64) (i32.const 1)) (func (export \"invoke\")))",
            false,
        ),
        (
            "a natural alignment",
            "(module (memory (export \"memory\") 1 1) (func (export \"invoke\") i32.const 0 i32.load align=4 drop))",
            true,
        ),
        (
            "an alignment above the natural one",
            "(module (memory (export \"memory\") 1 1) (func (export \"invoke\") i32.const 0 i32.load align=8 drop))",
            false,
        ),
        (
            "a byte load aligned to two",
            "(module (memory (export \"memory\") 1 1) (func (export \"invoke\") i32.const 0 i64.load8_s align=2 drop))",
            false,
        ),
        (
            "the largest offset",
            "(module (memory (export \"memory\") 1 1) (func (export \"invoke\") i32.const 0 i32.load offset=4294967295 drop))",
            true,
        ),
        (
            "a load without a memory",
            "(module (func (export \"invoke\") i32.const 0 i32.load drop))",
            false,
        ),
        (
            "memory.size without a memory",
            "(module (func (export \"invoke\") memory.size drop))",
            false,
        ),
        (
            "call_indirect of a type the table does not hold",
            "(module (type (func (param i32))) (table 1 1 funcref) (func (export \"invoke\") i32.const 0 call_indirect (type 0)))",
            false,
        ),
        (
            "call_indirect with its argument",
            "(module (type (func (param i32))) (table 1 1 funcref) (func (export \"invoke\") i32.const 1 i32.const 0 call_indirect (type 0)))",
            true,
        ),
        (
            "call_indirect through another table",
            "(module (table 1 1 funcref) (elem (i32.const 0) 0) (func (export \"invoke\") i32.const 0 call_indirect 1 (type 0)))",
            false,
        ),
        (
            "conversions in the wrong direction",
            "(module (func (export \"invoke\") i64.const 1 i32.wrap_i64 i64.extend_i32_s i64.extend_i32_u drop drop))",
            false,
        ),
        (
            "extending an i64",
            "(module (func (export \"invoke\") i64.const 1 i64.extend_i32_s drop))",
            false,
        ),
        (
            "adding across widths",
            "(module (func (export \"invoke\") i32.const 1 i64.const 2 i32.add drop))",
            false,
        ),
        (
            "testing an i32 as an i64",
            "(module (func (export \"invoke\") i32.const 1 i32.eqz i64.eqz drop))",
            false,
        ),
        (
            "an empty type section",
            "(module (type (func)) (func (export \"invoke\")))",
            true,
        ),
        (
            "an empty table",
            "(module (func (export \"invoke\")) (table 0 0 funcref))",
            true,
        ),
        (
            "an empty memory",
            "(module (func (export \"invoke\")) (memory (export \"memory\") 0 0))",
            true,
        ),
    ];
    for (what, text, accept) in modules {
        expect(what, &wasm(text), *accept);
    }
}

#[test]
pub(crate) fn tables_and_element_segments() {
    for (what, table, accept) in [
        ("no maximum", vec![0x70, 0x00, 0x00], false),
        ("maximum above minimum", vec![0x70, 0x01, 0x01, 0x02], false),
        (
            "more entries than functions",
            vec![0x70, 0x01, 0x02, 0x02],
            false,
        ),
        ("exact", vec![0x70, 0x01, 0x01, 0x01], true),
        ("empty", vec![0x70, 0x01, 0x00, 0x00], true),
        ("externref", vec![0x6f, 0x01, 0x01, 0x01], false),
    ] {
        expect(
            what,
            &with_section(simple(), 2, 4, vec_of(&[table])),
            accept,
        );
    }
    let exact = vec![0x70, 0x01, 0x01, 0x01];
    expect(
        "two tables",
        &with_section(simple(), 2, 4, vec_of(&[exact.clone(), exact])),
        false,
    );
    rejects(
        "an element past the table",
        "(module (table 1 1 funcref) (elem (i32.const 1) 0) (func (export \"invoke\")))",
    );
    accepts(
        "two segments",
        "(module (table 2 2 funcref) (elem (i32.const 0) 0) (elem (i32.const 1) 1) (func) (func (export \"invoke\")))",
    );
    rejects(
        "a table larger than the function count",
        "(module (table 2 2 funcref) (elem (i32.const 1) 0) (func (export \"invoke\")))",
    );
    accepts(
        "a table as large as the function count",
        "(module (table 2 2 funcref) (elem (i32.const 1) 0) (func) (func (export \"invoke\")))",
    );
    rejects(
        "a segment past a full table",
        "(module (table 2 2 funcref) (elem (i32.const 1) 0 0) (func (export \"invoke\")))",
    );
    accepts(
        "an empty segment in an empty table",
        "(module (table 0 0 funcref) (elem (i32.const 0)) (func (export \"invoke\")))",
    );
    accepts(
        "an import in the table",
        "(module (import \"ting\" \"input\" (func (param i32 i32) (result i32))) (table 1 1 funcref) (elem (i32.const 0) 0) (func (export \"invoke\")))",
    );
    rejects(
        "a passive segment",
        "(module (table 1 1 funcref) (elem func 0) (func (export \"invoke\")))",
    );
    rejects(
        "a segment of expressions",
        "(module (table 1 1 funcref) (elem (i32.const 0) funcref (ref.func 0)) (func (export \"invoke\")))",
    );
    // An active segment naming table zero explicitly is the same segment in
    // a second encoding, which the profile excludes.
    let mut parts = simple();
    parts.insert(2, (4, vec_of(&[vec![0x70, 0x01, 0x01, 0x01]])));
    let mut segment = vec![0x02, 0x00, 0x41, 0x00, 0x0b, 0x00];
    segment.extend(vec_of(&[leb(0)]));
    expect(
        "an explicit table index",
        &with_section(parts, 4, 9, vec_of(&[segment])),
        false,
    );
}

#[test]
pub(crate) fn globals_and_data_segments() {
    rejects(
        "a global with a non-constant initializer",
        "(module (global i32 (global.get 0)) (func (export \"invoke\")))",
    );
    rejects(
        "a float global",
        "(module (global f32 (f32.const 0)) (func (export \"invoke\")))",
    );
    accepts(
        "constant integer globals",
        "(module (global i64 (i64.const -1)) (global (mut i32) (i32.const -64)) (func (export \"invoke\")))",
    );
    rejects(
        "a passive data segment",
        &module(&[&memory(1, 1), "(data \"x\")", &invoke("")]),
    );
    accepts(
        "an empty segment in an empty memory",
        &module(&[&memory(0, 1), "(data (i32.const 0) \"\")", &invoke("")]),
    );
    // An active segment naming memory zero explicitly: the second encoding.
    let mut parts = simple();
    parts.insert(2, (5, vec_of(&[vec![0x01, 0x01, 0x01]])));
    let mut invoke_export = name("invoke");
    invoke_export.extend([0x00, 0x00]);
    let mut memory_export = name("memory");
    memory_export.extend([0x02, 0x00]);
    parts[3] = (7, vec_of(&[invoke_export, memory_export]));
    let mut segment = vec![0x02, 0x00, 0x41, 0x00, 0x0b];
    segment.extend(vec_of(&[vec![b'x']]));
    parts.push((11, vec_of(&[segment])));
    expect("an explicit memory index", &assemble(&parts), false);
}

#[test]
pub(crate) fn binary_encodings_are_minimal_complete_and_in_order() {
    for (what, code) in [
        ("a non-minimal i32 constant", vec![0x41, 0x81, 0x00, 0x1a]),
        (
            "a non-minimal negative i64 constant",
            vec![0x42, 0xff, 0x7f, 0x1a],
        ),
        ("a non-minimal call index", vec![0x10, 0x80, 0x00]),
    ] {
        let module = assemble(&simple_with(&code));
        expect(what, &module, false);
        assert_eq!(reason(&module), ENCODING, "{what}");
    }
    let parts = simple();
    let mut padded = b"\0asm\x01\0\0\0".to_vec();
    padded.push(1);
    padded.extend([parts[0].1.len() as u8 | 0x80, 0x00]);
    padded.extend(&parts[0].1);
    for (id, payload) in &parts[1..] {
        padded.extend(section(*id, payload));
    }
    expect("a non-minimal section length", &padded, false);
    assert_eq!(reason(&padded), ENCODING);
    expect(
        "an i32 immediate that overflows",
        &assemble(&simple_with(&[0x41, 0xff, 0xff, 0xff, 0xff, 0x1f, 0x1a])),
        false,
    );
    let mut bomb = b"\0asm\x01\0\0\0".to_vec();
    bomb.extend(section(1, &[0xff, 0xff, 0xff, 0xff, 0x0f]));
    expect("a type count of four billion", &bomb, false);
    let mut short = b"\0asm\x01\0\0\0".to_vec();
    short.extend(section(1, &[0x05, 0x60, 0x00, 0x00]));
    expect("a count above its section", &short, false);
    let baseline = assemble(&simple());
    for length in 0..baseline.len() {
        expect(
            &format!("truncated to {length} bytes"),
            &baseline[..length],
            false,
        );
    }
    let mut magic = baseline.clone();
    magic[0] = b'x';
    expect("a wrong magic", &magic, false);
    let mut version = baseline.clone();
    version[4] = 2;
    expect("a wrong version", &version, false);
    let mut duplicate = baseline.clone();
    duplicate.extend(section(1, &vec_of(&[vec![0x60, 0x00, 0x00]])));
    expect("a duplicate section", &duplicate, false);
    for (what, id) in [
        ("a custom section", 0),
        ("a start section", 8),
        ("a data count section", 12),
    ] {
        expect(what, &with_section(simple(), 2, id, vec![0x00]), false);
    }
    expect(
        "an element section before the exports",
        &with_section(simple(), 2, 9, vec![0x00]),
        false,
    );
}

#[test]
pub(crate) fn a_rejection_names_its_reason() {
    assert_eq!(reason(&wasm(&body("f32.const 1 drop"))), SHAPE);
    assert_eq!(
        reason(&wasm(&body("i64.const 0 i32.const 0 i32.add drop"))),
        TYPING
    );
    assert_eq!(
        reason(&assemble(&simple_with(&[0x41, 0x81, 0x00, 0x1a]))),
        ENCODING
    );
}

// ---- The code price, paid before validation ---------------------------------------------

const PREAMBLE: &[u8] = b"\0asm\x01\0\0\0";
/// The code price's terms (rules §15.7), stated by hand here
/// so that the model's reading of the framing is checked as well.
const CODE_BASE: u64 = 14_400;
const DECLARED_FUNCTION: u64 = 1_215;
const DECLARED_SEGMENT: u64 = 446;
/// `n` bytes at the common rate of 60 C.
fn plain(n: usize) -> u64 {
    60 * n as u64
}
/// `code` is invalid, the model reads `price` from its framing as stated by
/// hand, and its deployment pays exactly that price with the CodeHash before
/// validation rejects it.
fn pays(name: &str, code: &[u8], price: u64) {
    assert!(profile::validate(code).is_err(), "{name} must be invalid");
    assert_eq!(code_price(code), price, "{name}: the model's code price");
    deployment_bytes(code)
        .named(name)
        .deploy()
        .expect(Status::InvalidModule)
        .consumed(rejected_at(code, price))
        .run();
}

#[test]
pub(crate) fn invalid_code_keeps_a_code_price_read_from_its_framing_alone() {
    // Nothing to frame: every byte at 60.
    pays("no bytes", &[], CODE_BASE);
    pays("the preamble alone", PREAMBLE, CODE_BASE + plain(8));
    pays("half the preamble", &PREAMBLE[..4], CODE_BASE + plain(4));
    // The type, function and export sections are read whole; the code
    // section runs past the end of the module, so the scan stops at it and
    // every byte from its id on is priced at 60. The function section's
    // declared function counts.
    let simple_module = assemble(&simple());
    let cut = &simple_module[..simple_module.len() - 2];
    pays(
        "cut inside the code section",
        cut,
        CODE_BASE + plain(cut.len()) + DECLARED_FUNCTION,
    );
    // A data section is priced at 6 C a byte and 446 per declared segment
    // while it is whole; cut by one byte, its contents run past the end and
    // its bytes fall back to 60, its segment uncounted.
    let mut parts = simple();
    parts.insert(2, (5, vec_of(&[vec![0x01, 0x01, 0x01]])));
    let mut invoke_export = name("invoke");
    invoke_export.extend([0x00, 0x00]);
    let mut memory_export = name("memory");
    memory_export.extend([0x02, 0x00]);
    parts[3] = (7, vec_of(&[invoke_export, memory_export]));
    let mut segment = vec![0x00, 0x41, 0x00, 0x0b];
    segment.extend(leb(200));
    segment.extend([0xab; 200]);
    let data = vec_of(&[segment]);
    parts.push((11, data.clone()));
    let whole = assemble(&parts);
    assert!(profile::validate(&whole).is_ok());
    assert_eq!(
        code_price(&whole),
        CODE_BASE
            + plain(whole.len() - data.len())
            + 6 * data.len() as u64
            + DECLARED_FUNCTION
            + DECLARED_SEGMENT,
        "the whole module with data"
    );
    // Deployed whole, it pays the same price and runs its `nop`.
    deployment_bytes(&whole)
        .named("whole with data")
        .deploy()
        .charged(Ledger {
            dispatch: 1,
            ..Default::default()
        })
        .run();
    let cut = &whole[..whole.len() - 1];
    pays(
        "cut inside the data section",
        cut,
        CODE_BASE + plain(cut.len()) + DECLARED_FUNCTION,
    );
    // A whole data section followed by a section that runs past the end.
    let mut m = PREAMBLE.to_vec();
    m.extend(section(11, &data));
    m.extend([10, 0x05, 0x01]);
    pays(
        "a data section, then a cut one",
        &m,
        CODE_BASE + plain(8 + 3) + 6 * data.len() as u64 + DECLARED_SEGMENT + plain(3),
    );
    // A section size is any LEB128 of at most five bytes that fits 32 bits,
    // minimal or not: a padded size still frames its section, whose bytes
    // keep their rate, though validation rejects the encoding.
    let small = {
        let mut segment = vec![0x00, 0x41, 0x00, 0x0b];
        segment.extend(name("abc"));
        vec_of(&[segment])
    };
    let mut m = PREAMBLE.to_vec();
    m.extend([11, 0x80 | small.len() as u8, 0x00]);
    m.extend(&small);
    pays(
        "a padded data section size",
        &m,
        CODE_BASE + plain(8 + 3) + 6 * small.len() as u64 + DECLARED_SEGMENT,
    );
    // A size beyond 32 bits cannot be read: the scan stops at its section.
    let mut m = PREAMBLE.to_vec();
    m.extend([11, 0xff, 0xff, 0xff, 0xff, 0x1f]);
    m.extend(&small);
    pays(
        "a data section size beyond 32 bits",
        &m,
        CODE_BASE + plain(m.len()),
    );
    // A count that cannot be read counts zero.
    let mut m = PREAMBLE.to_vec();
    let unreadable = [0x80, 0x80, 0x80, 0x80, 0x80, 0x00];
    m.extend(section(11, &unreadable));
    pays(
        "a data count of six bytes",
        &m,
        CODE_BASE + plain(8 + 2) + 6 * unreadable.len() as u64,
    );
    let mut m = PREAMBLE.to_vec();
    m.extend(section(1, &vec_of(&[vec![0x60, 0x00, 0x00]])));
    m.extend(section(3, &[0xff, 0xff, 0xff, 0xff, 0x1f]));
    pays(
        "a function count beyond 32 bits",
        &m,
        CODE_BASE + plain(m.len()),
    );
    // A declared count is priced whether or not its entries follow.
    let mut m = PREAMBLE.to_vec();
    m.extend(section(1, &vec_of(&[vec![0x60, 0x00, 0x00]])));
    m.extend(section(3, &leb(5_000)));
    pays(
        "five thousand functions declared, none given",
        &m,
        CODE_BASE + plain(m.len()) + 5_000 * DECLARED_FUNCTION,
    );
    // The element section at 175 C a byte and the global section at 100,
    // here around a global whose initializer has the wrong type.
    let table = vec_of(&[vec![0x70, 0x01, 0x01, 0x01]]);
    let mut segment = vec![0x00, 0x41, 0x00, 0x0b];
    segment.extend(vec_of(&[leb(0)]));
    let element = vec_of(&[segment]);
    for (what, init, valid) in [
        ("a mistyped global", 0x41u8, false),
        ("a global", 0x42, true),
    ] {
        let global = vec_of(&[vec![0x7e, 0x01, init, 0x00, 0x0b]]);
        let mut parts = simple();
        parts.insert(2, (4, table.clone()));
        parts.insert(3, (6, global.clone()));
        parts.insert(5, (9, element.clone()));
        let m = assemble(&parts);
        let price = CODE_BASE
            + plain(m.len() - global.len() - element.len())
            + 100 * global.len() as u64
            + 175 * element.len() as u64
            + DECLARED_FUNCTION;
        if valid {
            assert_eq!(code_price(&m), price, "{what}");
            deployment_bytes(&m)
                .named(what)
                .deploy()
                .charged(Ledger {
                    dispatch: 1,
                    ..Default::default()
                })
                .run();
        } else {
            pays(what, &m, price);
        }
    }
}

#[test]
pub(crate) fn a_code_price_beyond_the_budget_is_refused_whole_before_validation() {
    // Eighty thousand declared functions cost 97,200,000 C to prepare. One C
    // short of it and the CodeHash, the debit is refused whole and the
    // deployment fails with status 3, after the ContractId and the existence
    // check and before validation could reject the module; with exactly
    // enough, validation rejects it.
    let mut m = PREAMBLE.to_vec();
    m.extend(section(1, &vec_of(&[vec![0x60, 0x00, 0x00]])));
    m.extend(section(3, &leb(80_000)));
    let price = CODE_BASE + plain(m.len()) + 80_000 * DECLARED_FUNCTION;
    assert_eq!(code_price(&m), price);
    let n = m.len() as u64;
    let before = ROOT + CONTRACT_ID + KEY;
    let envelope = TRANSACTION_BASE + deploy_size(n, 0);
    let debit = price + code_hash(n);
    for (what, c, status) in [
        ("one short", before + debit - 1, Status::Compute),
        ("exactly enough", before + debit, Status::InvalidModule),
    ] {
        let step = deployment_bytes(&m)
            .named(what)
            .deploy()
            .budget([c, BUDGET[1], BUDGET[2], BUDGET[3]])
            .expect(status);
        if status == Status::Compute {
            step.consumed([before, 1, envelope, 0]).run();
        } else {
            step.consumed(rejected_at(&m, price)).run();
        }
    }
    // A count of 2^32 - 1 still fits 32 bits and is priced in full, far past
    // the standard budget.
    let mut m = PREAMBLE.to_vec();
    m.extend(section(1, &vec_of(&[vec![0x60, 0x00, 0x00]])));
    m.extend(section(3, &[0xff, 0xff, 0xff, 0xff, 0x0f]));
    let n = m.len() as u64;
    assert_eq!(
        code_price(&m),
        CODE_BASE + plain(m.len()) + u64::from(u32::MAX) * DECLARED_FUNCTION
    );
    deployment_bytes(&m)
        .named("four billion functions declared")
        .deploy()
        .expect(Status::Compute)
        .consumed([before, 1, TRANSACTION_BASE + deploy_size(n, 0), 0])
        .run();
    // Every function section adds its count: 3,534,953 sections of seven
    // bytes, each declaring 2^32 - 1 functions, take the exact price past
    // 2^64 - 1 C. It is refused like any price beyond the budget, the same in
    // every build, never wrapped around to a price the budget could pay. The
    // module, 24.7 MB, is larger than any block, so a program builds it in
    // its memory, doubling its one section with `memory.copy`, and deploys
    // it from inside: the deployment receives the code, derives its
    // ContractId, finds no manifest and is refused the price.
    let sections = 3_534_953u64;
    let one = section(3, &[0xff, 0xff, 0xff, 0xff, 0x0f]);
    let mut m = PREAMBLE.to_vec();
    for _ in 0..sections {
        m.extend(&one);
    }
    assert!(vm_model::exact_code_price(&m) > u128::from(u64::MAX));
    assert_eq!(code_price(&m), u64::MAX);
    assert_eq!(ting_core::vm::price::code(&m), u64::MAX);
    let n = m.len() as u64;
    let mut doubling = String::new();
    let (mut copies, mut have) = (Vec::new(), 1u64);
    while have < sections {
        let more = have.min(sections - have);
        doubling += &format!(
            "i32.const {} i32.const 8 i32.const {} memory.copy
    ",
            8 + 7 * have,
            7 * more
        );
        copies.push(7 * more);
        have += more;
    }
    let (budget_at, result_at) = (n.next_multiple_of(8), n.next_multiple_of(8) + 32);
    let pages = (result_at + 4).div_ceil(65_536);
    let mut first = PREAMBLE.to_vec();
    first.extend(&one);
    let body = format!(
        "{doubling}i32.const {result_at} i32.const 0 i32.const {n} i32.const 0 i32.const 0 i32.const {budget_at} call $deploy i32.store
    i32.const {result_at} i32.const 4 call $return"
    );
    let builder = module(&[
        &import("deploy"),
        &import("return"),
        &memory(pages as u32, pages as u32),
        &format!("(data (i32.const 0) {})", escaped(&first)),
        &format!(
            "(data (i32.const {budget_at}) {})",
            escaped(
                &[10_000_000u64, 128, 4096, 4096]
                    .map(u64::to_le_bytes)
                    .concat()
            )
        ),
        &invoke(&body),
    ]);
    let tier = Tier::pages(pages);
    built(&builder)
        .named("a code price past 2^64 - 1")
        .call()
        .result((Status::Compute as u32).to_le_bytes())
        .charged(Ledger {
            dispatch: cost_at(&body, tier),
            copies: copies.iter().map(|&c| words(c)).sum::<u64>() + words(32),
            spans: 2 * copies.len() as u64 + 1,
            nested: tier.bytes(n) + tier.spans(1) + CONTRACT_ID + KEY,
            accesses: 1,
            ..Default::default()
        })
        .run();
}

#[test]
pub(crate) fn accepted_byte_mutations_execute_deterministically_within_their_budget() {
    // Every single-byte replacement of five small modules: whatever the
    // decoder accepts must run to the same receipt twice, within the budget,
    // and leave nothing behind when it fails. The budget pays for a seed's
    // code price, installation and initializer, so that mutations deploy as
    // well as fail.
    let seeds = [
        body("i32.const 7 i32.const 3 i32.add drop"),
        module(&[&memory(0, 1), &invoke("i32.const 0 memory.grow drop")]),
        body("(block (loop i32.const 0 br_if 0))"),
        "(module (table 1 1 funcref) (elem (i32.const 0) 0) (func (export \"invoke\") (call_indirect (i32.const 0))))".to_string(),
        body("i64.const 7 i64.const -3 i64.mul_wide_s i64.const 1 i64.const 2 i64.add128 drop drop"),
    ];
    let (mut accepted, mut deployed) = (0, 0);
    for source in &seeds {
        let original = wasm(source);
        for position in 0..original.len() {
            for replacement in 0u8..=255 {
                if replacement == original[position] {
                    continue;
                }
                let mut mutated = original.clone();
                mutated[position] = replacement;
                if let Ok(profile) = profile::validate(&mutated) {
                    assert!(profile.memory as u64 <= profile::MAX_PAGES * 65536);
                } else {
                    continue;
                }
                accepted += 1;
                let tx = Transaction {
                    budget: Resources([500_000, 128, 4096, 8192]),
                    payload: Payload::Deploy {
                        code: mutated,
                        input: Vec::new(),
                    },
                };
                let a = Runtime::new(State::default(), context());
                let b = Runtime::new(State::default(), context());
                let (first, second) = (a.execute(&tx).unwrap(), b.execute(&tx).unwrap());
                assert_eq!(
                    first, second,
                    "byte {position} as {replacement:#04x} of\n{source}"
                );
                assert!(first.used.fits(tx.budget));
                assert_eq!(a.state_root(), b.state_root());
                if first.status == Status::Success {
                    deployed += 1;
                } else {
                    assert_eq!(a.state_root(), State::default().root());
                    assert!(first.events.is_empty());
                }
            }
        }
    }
    assert!(accepted >= 100, "{accepted} accepted mutations");
    assert!(
        deployed > 0,
        "none of {accepted} accepted mutations deployed"
    );
}

#[test]
pub(crate) fn an_if_without_else_may_not_carry_a_result_even_in_dead_code() {
    rejects(
        "an unreachable then arm with a result",
        &body("i32.const 1 (if (result i32) (then unreachable)) drop"),
    );
    accepts(
        "an unreachable then arm with an else",
        &body("i32.const 1 (if (result i32) (then unreachable) (else i32.const 1)) drop"),
    );
    rejects(
        "an if without else inside dead code",
        &body(
            "(block (result i32) unreachable i32.const 1 (if (result i32) (then i32.const 1))) drop",
        ),
    );
    // An else that closes a block, and an else at function level.
    expect(
        "else after a block",
        &assemble(&simple_with(&[0x02, 0x40, 0x05, 0x0b])),
        false,
    );
    expect(
        "else without an if",
        &assemble(&simple_with(&[0x05])),
        false,
    );
}

// ---- Every immediate in every encoding -------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Form {
    Minimal,
    /// One continuation byte more than needed.
    Padded,
    /// Two more.
    PaddedTwice,
    /// Every byte the width allows, plus one.
    Overlong,
    /// The last permitted byte carries bits above the width.
    Overflow,
}
const FORMS: [Form; 5] = [
    Form::Minimal,
    Form::Padded,
    Form::PaddedTwice,
    Form::Overlong,
    Form::Overflow,
];
fn unsigned(value: u32, form: Form) -> Vec<u8> {
    let mut out = leb(u64::from(value));
    let pad = |out: &mut Vec<u8>| {
        let last = out.len() - 1;
        out[last] |= 0x80;
        out.push(0x00);
    };
    match form {
        Form::Minimal => {}
        Form::Padded => pad(&mut out),
        Form::PaddedTwice => {
            pad(&mut out);
            pad(&mut out);
        }
        Form::Overlong => {
            out = (0..5)
                .map(|k| ((value >> (7 * k)) & 0x7f) as u8 | 0x80)
                .collect();
            out.push(0x00);
        }
        Form::Overflow => {
            out = (0..4)
                .map(|k| ((value >> (7 * k)) & 0x7f) as u8 | 0x80)
                .collect();
            out.push(((value >> 28) & 0x0f) as u8 | 0x10);
        }
    }
    out
}
fn signed(mut value: i64, bits: u32, form: Form) -> Vec<u8> {
    let negative = value < 0;
    let mut out = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        out.push(if done { byte } else { byte | 0x80 });
        if done {
            break;
        }
    }
    let pad = |out: &mut Vec<u8>| {
        let last = out.len() - 1;
        out[last] |= 0x80;
        out.push(if negative { 0x7f } else { 0x00 });
    };
    let bytes = bits.div_ceil(7) as usize;
    match form {
        Form::Minimal => {}
        Form::Padded => pad(&mut out),
        Form::PaddedTwice => {
            pad(&mut out);
            pad(&mut out);
        }
        Form::Overlong => {
            // The value's two's complement over `bytes` continuation bytes,
            // then one byte more carrying the sign.
            let bits_value = value_bits(out.clone(), bits);
            out = (0..bytes)
                .map(|k| ((bits_value >> (7 * k)) & 0x7f) as u8 | 0x80)
                .collect();
            out.push(if negative { 0x7f } else { 0x00 });
        }
        Form::Overflow => {
            // Every permitted byte, the last one with the bits above the
            // width wrong: set for a non-negative value, clear for a negative.
            let bits_value = value_bits(out.clone(), bits);
            out = (0..bytes - 1)
                .map(|k| ((bits_value >> (7 * k)) & 0x7f) as u8 | 0x80)
                .collect();
            let data = bits - 7 * (bytes as u32 - 1);
            let low = ((bits_value >> (7 * (bytes - 1))) & ((1 << data) - 1)) as u8;
            let above = ((0x7fu32 >> data) << data) as u8;
            out.push(if negative { low } else { low | above });
        }
    }
    out
}
/// The `bits`-bit two's complement pattern of a minimal signed encoding.
fn value_bits(minimal: Vec<u8>, bits: u32) -> u64 {
    let mut value: i64 = 0;
    let mut shift = 0;
    for byte in &minimal {
        value |= i64::from(byte & 0x7f) << shift;
        shift += 7;
    }
    if shift < 64 && minimal.last().unwrap() & 0x40 != 0 {
        value |= -1i64 << shift;
    }
    if bits == 32 {
        value as u32 as u64
    } else {
        value as u64
    }
}

/// A module with every kind of immediate, each encoded minimally except the
/// one `site`, which takes `form`.
struct Immediates {
    site: &'static str,
    form: Form,
}
impl Immediates {
    fn u(&self, site: &str, value: u32) -> Vec<u8> {
        unsigned(
            value,
            if site == self.site {
                self.form
            } else {
                Form::Minimal
            },
        )
    }
    fn s(&self, site: &str, value: i64, bits: u32) -> Vec<u8> {
        signed(
            value,
            bits,
            if site == self.site {
                self.form
            } else {
                Form::Minimal
            },
        )
    }
    fn section(&self, site: &str, id: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![id];
        out.extend(self.u(site, payload.len() as u32));
        out.extend(payload);
        out
    }
    fn module(&self) -> Vec<u8> {
        let mut types = self.u("type count", 2);
        types.push(0x60);
        types.extend(self.u("parameter count", 1));
        types.extend([0x7f, 0x00]);
        types.extend([0x60, 0x00, 0x00]);
        let mut functions = self.u("function count", 2);
        functions.extend(self.u("function type index", 0));
        functions.extend(self.u("function type index", 1));
        let mut table = vec![0x01, 0x70, 0x01];
        table.extend(self.u("table minimum", 1));
        table.extend(self.u("table maximum", 1));
        let mut memory = vec![0x01, 0x01];
        memory.extend(self.u("memory minimum", 1));
        memory.extend(self.u("memory maximum", 1));
        let mut global = vec![0x01, 0x7f, 0x01, 0x41];
        global.extend(self.s("global initializer", 0, 32));
        global.push(0x0b);
        let mut exports = self.u("export count", 2);
        exports.extend(self.u("export name length", 6));
        exports.extend(b"invoke");
        exports.push(0x00);
        exports.extend(self.u("export index", 1));
        exports.extend([6]);
        exports.extend(b"memory");
        exports.extend([0x02, 0x00]);
        let mut elements = self.u("element count", 1);
        elements.extend([0x00, 0x41]);
        elements.extend(self.s("element offset", 0, 32));
        elements.push(0x0b);
        elements.extend(self.u("element function count", 1));
        elements.extend(self.u("element function index", 0));
        // $f (param i32) (local i32): local.get 0 local.set 1.
        let mut f = self.u("local declaration count", 1);
        f.extend(self.u("local count", 1));
        f.push(0x7f);
        f.push(0x20);
        f.extend(self.u("local index", 0));
        f.push(0x21);
        f.extend(self.u("local index", 1));
        f.push(0x0b);
        let mut invoke = vec![0x00, 0x02, 0x40, 0x41];
        invoke.extend(self.s("i32.const", 5, 32));
        invoke.push(0x10);
        invoke.extend(self.u("call index", 0));
        invoke.extend([0x41, 0x00, 0x0d]);
        invoke.extend(self.u("br_if depth", 0));
        invoke.extend([0x41, 0x00, 0x41, 0x00, 0x36]);
        invoke.extend(self.u("memarg align", 2));
        invoke.extend(self.u("memarg offset", 4));
        invoke.extend([0x41, 0x00, 0x28, 0x02, 0x00, 0x1a]);
        invoke.push(0x23);
        invoke.extend(self.u("global index", 0));
        invoke.extend([0x24, 0x00, 0x42]);
        invoke.extend(self.s("i64.const", -3, 64));
        invoke.extend([0x1a, 0x41, 0x00, 0x0e]);
        invoke.extend(self.u("br_table count", 1));
        invoke.extend(self.u("br_table label", 0));
        invoke.extend([0x00, 0x0b, 0x0b]);
        let mut code = self.u("code count", 2);
        code.extend(self.u("body size", f.len() as u32));
        code.extend(f);
        code.extend(self.u("body size", invoke.len() as u32));
        code.extend(invoke);
        let mut data = self.u("data count", 1);
        data.extend([0x00, 0x41]);
        data.extend(self.s("data offset", 0, 32));
        data.push(0x0b);
        data.extend(self.u("data length", 1));
        data.push(b'x');
        let mut out = b"\0asm\x01\0\0\0".to_vec();
        out.extend(self.section("section length", 1, &types));
        out.extend(self.section("", 3, &functions));
        out.extend(self.section("", 4, &table));
        out.extend(self.section("", 5, &memory));
        out.extend(self.section("", 6, &global));
        out.extend(self.section("", 7, &exports));
        out.extend(self.section("", 9, &elements));
        out.extend(self.section("", 10, &code));
        out.extend(self.section("", 11, &data));
        out
    }
}
const UNSIGNED_SITES: [&str; 25] = [
    "section length",
    "type count",
    "parameter count",
    "function count",
    "function type index",
    "table minimum",
    "table maximum",
    "memory minimum",
    "memory maximum",
    "export count",
    "export name length",
    "export index",
    "element count",
    "element function count",
    "element function index",
    "local declaration count",
    "local count",
    "local index",
    "call index",
    "br_if depth",
    "memarg align",
    "memarg offset",
    "global index",
    "br_table count",
    "br_table label",
];
const SIGNED_SITES: [(&str, u32); 5] = [
    ("global initializer", 32),
    ("element offset", 32),
    ("i32.const", 32),
    ("i64.const", 64),
    ("data offset", 32),
];

#[test]
pub(crate) fn every_immediate_is_accepted_minimally_encoded_and_in_no_other_form() {
    let minimal = Immediates {
        site: "",
        form: Form::Minimal,
    }
    .module();
    expect("every immediate minimal", &minimal, true);
    for site in UNSIGNED_SITES.iter().copied().chain([
        "code count",
        "body size",
        "data count",
        "data length",
    ]) {
        for form in FORMS {
            let module = Immediates { site, form }.module();
            if form == Form::Minimal {
                assert_eq!(module, minimal, "{site}");
                continue;
            }
            expect(&format!("{site} {form:?}"), &module, false);
            if matches!(form, Form::Padded | Form::PaddedTwice) {
                assert_eq!(reason(&module), ENCODING, "{site} {form:?}");
            }
        }
    }
    for (site, _) in SIGNED_SITES {
        for form in FORMS {
            let module = Immediates { site, form }.module();
            if form == Form::Minimal {
                assert_eq!(module, minimal, "{site}");
                continue;
            }
            expect(&format!("{site} {form:?}"), &module, false);
            if matches!(form, Form::Padded | Form::PaddedTwice) {
                assert_eq!(reason(&module), ENCODING, "{site} {form:?}");
            }
        }
    }
    // The signed encoder itself, against known forms.
    assert_eq!(signed(5, 32, Form::Minimal), [0x05]);
    assert_eq!(signed(-3, 64, Form::Minimal), [0x7d]);
    assert_eq!(signed(5, 32, Form::Padded), [0x85, 0x00]);
    assert_eq!(signed(-3, 64, Form::Padded), [0xfd, 0x7f]);
    assert_eq!(signed(64, 32, Form::Minimal), [0xc0, 0x00]);
    assert_eq!(signed(-64, 32, Form::Minimal), [0x40]);
    assert_eq!(
        unsigned(5, Form::Overlong),
        [0x85, 0x80, 0x80, 0x80, 0x80, 0x00]
    );
}

#[test]
pub(crate) fn one_encoding_per_construct_no_empty_section_and_one_group_per_run() {
    // A section with no entries is left out: each section the simple module
    // can do without, present but empty, is rejected.
    for id in [2u8, 4, 5, 6, 9, 11] {
        let mut parts = simple();
        let at = parts
            .iter()
            .position(|(i, _)| *i > id)
            .unwrap_or(parts.len());
        parts.insert(at, (id, vec![0x00]));
        let code = assemble(&parts);
        expect(&format!("empty section {id}"), &code, false);
        assert_eq!(reason(&code), ENCODING, "empty section {id}");
    }
    // Local groups: each run of one type is one group of at least one local.
    let with_locals = |groups: &[(u32, u8)]| {
        let groups: Vec<Vec<u8>> = groups
            .iter()
            .map(|(n, t)| {
                let mut group = leb(u64::from(*n));
                group.push(*t);
                group
            })
            .collect();
        let mut function = vec_of(&groups);
        function.extend([0x01, 0x0b]);
        let mut entry = leb(function.len() as u64);
        entry.extend(function);
        let mut parts = simple();
        parts[3] = (10, vec_of(&[entry]));
        assemble(&parts)
    };
    expect(
        "runs of i32, i64 and i32 again",
        &with_locals(&[(2, 0x7f), (1, 0x7e), (1, 0x7f)]),
        true,
    );
    for (what, groups) in [
        ("an empty group", vec![(0, 0x7f)]),
        (
            "an empty group between runs",
            vec![(1, 0x7f), (0, 0x7e), (1, 0x7e)],
        ),
        ("a run split in two", vec![(1, 0x7f), (1, 0x7f)]),
    ] {
        let code = with_locals(&groups);
        expect(what, &code, false);
        assert_eq!(reason(&code), ENCODING, "{what}");
    }
}
#[test]
pub(crate) fn sections_are_complete_in_order_and_unique() {
    let base = Immediates {
        site: "",
        form: Form::Minimal,
    };
    let parts: Vec<(u8, Vec<u8>)> = {
        // Reparse the minimal module into (id, payload) pairs.
        let bytes = base.module();
        let mut parts = Vec::new();
        let mut i = 8;
        while i < bytes.len() {
            let id = bytes[i];
            let (mut len, mut shift, mut j) = (0usize, 0, i + 1);
            loop {
                let b = bytes[j];
                len |= usize::from(b & 0x7f) << shift;
                shift += 7;
                j += 1;
                if b & 0x80 == 0 {
                    break;
                }
            }
            parts.push((id, bytes[j..j + len].to_vec()));
            i = j + len;
        }
        parts
    };
    assert_eq!(
        parts.iter().map(|p| p.0).collect::<Vec<_>>(),
        [1, 3, 4, 5, 6, 7, 9, 10, 11]
    );
    expect("reassembled", &assemble(&parts), true);
    for k in 0..parts.len() {
        let id = parts[k].0;
        // Twice.
        let mut twice = parts.clone();
        twice.insert(k + 1, parts[k].clone());
        expect(&format!("section {id} twice"), &assemble(&twice), false);
        // Before its predecessor.
        if k > 0 {
            let mut swapped = parts.clone();
            swapped.swap(k - 1, k);
            expect(
                &format!("section {id} before section {}", parts[k - 1].0),
                &assemble(&swapped),
                false,
            );
        }
        // Declared one byte longer and one byte shorter than it is.
        for delta in [1i64, -1] {
            let mut bytes = b"\0asm\x01\0\0\0".to_vec();
            for (j, (sid, payload)) in parts.iter().enumerate() {
                bytes.push(*sid);
                let declared = if j == k {
                    (payload.len() as i64 + delta) as u64
                } else {
                    payload.len() as u64
                };
                bytes.extend(leb(declared));
                bytes.extend(payload);
            }
            expect(
                &format!("section {id} declared {delta:+} bytes"),
                &bytes,
                false,
            );
        }
        // A count one above and one below the entries.
        for delta in [1i64, -1] {
            let payload = &parts[k].1;
            let count = payload[0] as i64 + delta;
            if count < 0 {
                continue;
            }
            let mut altered = parts.clone();
            let mut p = leb(count as u64);
            p.extend(&payload[1..]);
            altered[k].1 = p;
            expect(
                &format!("section {id} count {delta:+}"),
                &assemble(&altered),
                false,
            );
        }
    }
    // A section nothing depends on may be left out, but not kept empty: the
    // body reads the global, so that section stays.
    for id in [4u8, 9, 11] {
        let mut emptied = parts.clone();
        let k = emptied.iter().position(|p| p.0 == id).unwrap();
        emptied[k].1 = vec![0x00];
        let mut omitted = parts.clone();
        omitted.retain(|p| p.0 != id);
        if id == 4 {
            // Without a table the indirect-call-free module still stands,
            // but the element segment needs one: drop it too.
            emptied.retain(|p| p.0 != 9);
            omitted.retain(|p| p.0 != 9);
        }
        expect(&format!("section {id} empty"), &assemble(&emptied), false);
        expect(&format!("section {id} left out"), &assemble(&omitted), true);
    }
    // Unknown and custom sections, anywhere.
    for id in [0u8, 12, 13, 14, 127, 255] {
        for at in [0, 3, parts.len()] {
            let mut with = parts.clone();
            with.insert(at, (id, vec![0x00]));
            expect(&format!("section {id} at {at}"), &assemble(&with), false);
        }
    }
}

// ---- Control typing, construct by construct ----------------------------------------------

/// Bodies for a frame with result `R`, and whether `block`, `loop`,
/// `if` without else, the then arm and the else arm of an `if` with else
/// accept them. `O` is the other width.
const RESULT_BODIES: &[(&str, [bool; 5])] = &[
    ("R.const 1", [true, true, false, true, true]),
    ("", [false, false, false, false, false]),
    ("O.const 1", [false, false, false, false, false]),
    ("R.const 1 R.const 2", [false, false, false, false, false]),
    ("R.const 1 br 0", [true, true, false, true, true]),
    ("br 0", [false, true, false, false, false]),
    ("unreachable", [true, true, false, true, true]),
    ("unreachable R.const 1", [true, true, false, true, true]),
    ("unreachable O.const 1", [false, false, false, false, false]),
    (
        "unreachable R.const 1 R.const 2",
        [false, false, false, false, false],
    ),
    (
        "R.const 1 i32.const 1 br_if 0",
        [true, true, false, true, true],
    ),
    ("i32.const 1 br_if 0", [false, false, false, false, false]),
    ("R.const 1 return", [true, true, false, true, true]),
    ("R.const 1 drop", [false, false, false, false, false]),
    (
        "R.const 1 R.const 2 i32.const 0 select",
        [true, true, false, true, true],
    ),
    (
        "R.const 1 O.const 2 i32.const 0 select",
        [false, false, false, false, false],
    ),
    (
        "(block (result R) R.const 1)",
        [true, true, false, true, true],
    ),
    (
        "(block (result O) O.const 1)",
        [false, false, false, false, false],
    ),
    (
        "(loop (result R) R.const 1 br 1)",
        [true, true, false, true, true],
    ),
    (
        "unreachable drop R.const 1",
        [true, true, false, true, true],
    ),
    ("unreachable R.add", [true, true, false, true, true]),
    (
        "unreachable O.const 1 R.add",
        [false, false, false, false, false],
    ),
    ("R.const 1 unreachable", [true, true, false, true, true]),
    (
        "R.const 1 R.const 1 unreachable",
        [true, true, false, true, true],
    ),
];
/// Bodies for a frame without a result.
const PLAIN_BODIES: &[(&str, bool)] = &[
    ("", true),
    ("i32.const 1", false),
    ("i64.const 1 drop", true),
    ("br 0", true),
    // A branch is stack-polymorphic: operands below the label's values are discarded.
    ("i32.const 1 br 0", true),
    ("i32.const 1 br_if 0", true),
    ("unreachable", true),
    ("unreachable i32.const 1", false),
    ("unreachable drop", true),
    ("return", true),
    ("i32.const 1 return", true),
];

#[test]
pub(crate) fn control_constructs_accept_exactly_the_bodies_core_validation_allows() {
    for (r, o) in [("i32", "i64"), ("i64", "i32")] {
        for (template, accept) in RESULT_BODIES {
            let b = template
                .replace("R.", &format!("{r}."))
                .replace("O.", &format!("{o}."));
            let b = b
                .replace("(result R)", &format!("(result {r})"))
                .replace("(result O)", &format!("(result {o})"));
            let constructs = [
                format!("(block (result {r}) {b}) drop"),
                format!("(loop (result {r}) {b}) drop"),
                format!("i32.const 1 (if (result {r}) (then {b})) drop"),
                format!("i32.const 1 (if (result {r}) (then {b}) (else {r}.const 9)) drop"),
                format!("i32.const 1 (if (result {r}) (then {r}.const 9) (else {b})) drop"),
            ];
            for (construct, expected) in constructs.iter().zip(accept) {
                expect(construct, &wasm(&body(construct)), *expected);
            }
        }
    }
    for (b, accept) in PLAIN_BODIES {
        for construct in [
            format!("(block {b})"),
            format!("(loop {b})"),
            format!("i32.const 1 (if (then {b}))"),
            format!("i32.const 1 (if (then {b}) (else nop))"),
            format!("i32.const 1 (if (then nop) (else {b}))"),
        ] {
            expect(&construct, &wasm(&body(&construct)), *accept);
        }
    }
}

#[test]
pub(crate) fn dead_code_is_typed_where_its_operands_are_known() {
    // (operation, operands of the right types, one operand of a wrong width,
    // the values it leaves). Each runs after `unreachable`: with unknown
    // operands it is accepted, with the right known ones accepted, with a
    // wrong known one rejected.
    let table: &[(&str, &str, &str, usize)] = &[
        (
            "i32.add",
            "i32.const 1 i32.const 2",
            "i64.const 1 i32.const 2",
            1,
        ),
        (
            "i64.mul",
            "i64.const 1 i64.const 2",
            "i32.const 1 i64.const 2",
            1,
        ),
        ("i32.eqz", "i32.const 1", "i64.const 1", 1),
        ("i64.eqz", "i64.const 1", "i32.const 1", 1),
        ("i32.wrap_i64", "i64.const 1", "i32.const 1", 1),
        ("i64.extend_i32_u", "i32.const 1", "i64.const 1", 1),
        ("i32.extend8_s", "i32.const 1", "i64.const 1", 1),
        ("i32.load", "i32.const 0", "i64.const 0", 1),
        (
            "i64.store",
            "i32.const 0 i64.const 0",
            "i32.const 0 i32.const 0",
            0,
        ),
        (
            "select",
            "i32.const 1 i32.const 2 i32.const 0",
            "i32.const 1 i64.const 2 i32.const 0",
            1,
        ),
        ("call $f", "i32.const 1", "i64.const 1", 0),
        ("local.set 0", "i32.const 1", "i64.const 1", 0),
        ("global.set 0", "i32.const 1", "i64.const 1", 0),
        ("memory.grow", "i32.const 1", "i64.const 1", 1),
        (
            "i32.store8",
            "i32.const 0 i32.const 0",
            "i32.const 0 i64.const 0",
            0,
        ),
        (
            "memory.copy",
            "i32.const 0 i32.const 0 i32.const 0",
            "i32.const 0 i64.const 0 i32.const 0",
            0,
        ),
        (
            "i64.add128",
            "i64.const 1 i64.const 2 i64.const 3 i64.const 4",
            "i64.const 1 i64.const 2 i32.const 3 i64.const 4",
            2,
        ),
        (
            "i64.sub128",
            "i64.const 1 i64.const 2 i64.const 3 i64.const 4",
            "i32.const 1 i64.const 2 i64.const 3 i64.const 4",
            2,
        ),
        (
            "i64.mul_wide_s",
            "i64.const 1 i64.const 2",
            "i64.const 1 i32.const 2",
            2,
        ),
        (
            "i64.mul_wide_u",
            "i64.const 1 i64.const 2",
            "i32.const 1 i64.const 2",
            2,
        ),
    ];
    for (operation, right, wrong, results) in table {
        let drop = " drop".repeat(*results);
        for (what, operands, accept) in [
            ("unknown operands", "", true),
            ("known operands", *right, true),
            ("a known operand of the wrong width", *wrong, false),
        ] {
            let text = module(&[
                &memory(1, 1),
                "(global (mut i32) (i32.const 0))",
                "(func $f (param i32))",
                &invoke(&format!(
                    "(local i32) unreachable {operands} {operation}{drop}"
                )),
            ]);
            expect(
                &format!("{operation} after unreachable with {what}"),
                &wasm(&text),
                accept,
            );
        }
    }
    // br_if in dead code: a known wrong condition, and a known wrong label value.
    expect(
        "br_if with a known i64 condition in dead code",
        &wasm(&body("(block unreachable i64.const 0 br_if 0)")),
        false,
    );
    expect(
        "br_if with a known wrong label value in dead code",
        &wasm(&body(
            "(block (result i32) unreachable i64.const 0 i32.const 0 br_if 0) drop",
        )),
        false,
    );
    expect(
        "br_if with an unknown label value in dead code",
        &wasm(&body(
            "(block (result i32) unreachable i32.const 0 br_if 0) drop",
        )),
        true,
    );
}
