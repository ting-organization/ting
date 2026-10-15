//! The oracle engine and validator, test-only behind the `oracle` feature.
//! Every case in this binary already runs on both engines and both
//! validators; here they meet on inputs no case states by hand: seeded random
//! programs that mix every instruction family, host imports, nested
//! self-calls and budgets that run out anywhere, and systematic byte
//! mutations of seed modules. Receipts and state roots must be identical.
//! `TING_FUZZ_PROGRAMS` and `TING_FUZZ_SEED` scale and move the batch.
use crate::{
    dsl::{import, invoke, memory, module, wasm},
    harness::context,
    shape::shape,
};
use std::fmt::Write as _;
use ting_core::{
    hash::hex,
    meter::Resources,
    protocol::{Encode, Payload, Status, Transaction},
    state::State,
    vm::{Runtime, profile},
};

/// Both validators agree on `module`; when both accept, on its profile too.
fn agree(module: &[u8], what: &str) -> bool {
    let own = profile::validate(module);
    let reference = profile::reference_validate(module);
    match (&own, &reference) {
        (Ok(a), Ok(b)) => assert_eq!(a, b, "{what}: profiles differ"),
        (Err(_), Err(_)) => {}
        _ => panic!("{what}: own {own:?}, reference {reference:?}"),
    }
    own.is_ok()
}

/// Every single-byte replacement from a fixed set, deletion, zero insertion
/// and truncation of each seed module.
#[test]
fn byte_mutations_of_seed_modules_agree() {
    let seeds = [
        (
            "indirect call",
            module(&[
                "(type $t (func (result i64)))",
                &import("return"),
                &memory(1, 1),
                "(func $ten (type $t) i64.const 10)",
                "(table 3 3 funcref)",
                "(elem (i32.const 0) $ten)",
                &invoke("i32.const 0 i32.const 0 call_indirect (type $t) i64.store i32.const 0 i32.const 8 call $return"),
            ]),
        ),
        (
            "bulk memory",
            module(&[
                &import("return"),
                &memory(1, 1),
                &invoke(
                    "i32.const 0 i32.const 171 i32.const 1000 memory.fill i32.const 2000 i32.const 0 i32.const 1000 memory.copy i32.const 2992 i32.const 8 call $return",
                ),
            ]),
        ),
        (
            "global",
            module(&[
                &import("return"),
                &memory(1, 1),
                "(global (mut i32) (i32.const 7))",
                &invoke("i32.const 0 global.get 0 i32.const 1 i32.add global.set 0 global.get 0 i64.extend_i32_u i64.store i32.const 0 i32.const 8 call $return"),
            ]),
        ),
        (
            "br_table",
            module(&[
                &import("return"),
                &memory(1, 1),
                &invoke("i32.const 0 (block (result i64) (block (result i64) i64.const 11 i32.const 0 br_table 0 1 1) i64.const 1 i64.add) i64.store i32.const 0 i32.const 8 call $return"),
            ]),
        ),
        (
            "loop",
            module(&[
                &import("return"),
                &memory(1, 1),
                &invoke("(local i32) i32.const 3 local.set 0 (block (loop local.get 0 i32.eqz br_if 1 local.get 0 i32.const 1 i32.sub local.set 0 br 0)) i32.const 0 local.get 0 i32.store i32.const 0 i32.const 4 call $return"),
            ]),
        ),
        (
            "memory.grow",
            module(&[
                &import("return"),
                &memory(1, 2),
                &invoke("i32.const 0 i32.const 0 memory.grow i32.store i32.const 4 memory.size i32.store i32.const 0 i32.const 8 call $return"),
            ]),
        ),
        (
            "select",
            module(&[
                &import("return"),
                &memory(1, 1),
                &invoke("i32.const 0 i64.const 17 i64.const 23 i32.const 0 select i64.store i32.const 0 i32.const 8 call $return"),
            ]),
        ),
        (
            "return",
            module(&[
                &import("return"),
                &memory(1, 1),
                "(func $forty_two (result i64) i64.const 42 return)",
                &invoke("i32.const 0 call $forty_two i64.store i32.const 0 i32.const 8 call $return"),
            ]),
        ),
        (
            "wide arithmetic",
            module(&[
                &import("return"),
                &memory(1, 1),
                &invoke("i32.const 0 i64.const -1 i64.const 3 i64.mul_wide_u i64.const 5 i64.const 7 i64.add128 i64.const 9 i64.const -2 i64.sub128 i64.mul_wide_s i64.xor i64.store i32.const 0 i32.const 8 call $return"),
            ]),
        ),
        (
            "imports",
            r#"(module
  (import "ting" "input" (func $input (param i32 i32) (result i32)))
  (import "ting" "storage_set" (func $set (param i32 i32 i32)))
  (memory (export "memory") 1 2) (data (i32.const 8) "abc")
  (global $g (mut i64) (i64.const 7))
  (func $f (param i32 i64) (result i32) (local i32) local.get 2)
  (func (export "invoke") (local i64)
    (i32.store8 offset=1 (i32.const 0) (call $f (i32.const 1) (i64.const 2)))
    (global.set $g (i64.extend_i32_s (call $input (i32.const 0) (i32.const 0))))
    (if (i32.eqz (i32.const 0)) (then (call $set (i32.const 0) (i32.const 0) (i32.const 1))))
    (block (result i32) (loop (br_if 1 (i32.const 1) (i32.const 0)) drop (br_if 0 (i32.const 0))) i32.const 3) drop))"#
                .to_string(),
        ),
    ];
    let (mut mutations, mut accepted) = (0u64, 0u64);
    for (id, source) in &seeds {
        let seed = wasm(source);
        assert!(agree(&seed, id), "seed {id} must be accepted");
        for position in 0..seed.len() {
            let original = seed[position];
            for replacement in [
                0x00,
                0x01,
                0x0b,
                0x7f,
                0x80,
                0xff,
                original ^ 0x80,
                original.wrapping_add(1),
                original.wrapping_sub(1),
            ] {
                if replacement == original {
                    continue;
                }
                let mut m = seed.clone();
                m[position] = replacement;
                mutations += 1;
                if agree(&m, &format!("{id} byte {position} -> {replacement:#04x}")) {
                    accepted += 1;
                }
            }
            let mut deleted = seed.clone();
            deleted.remove(position);
            mutations += 1;
            agree(&deleted, &format!("{id} without byte {position}"));
            let mut inserted = seed.clone();
            inserted.insert(position, 0x00);
            mutations += 1;
            agree(
                &inserted,
                &format!("{id} with a zero before byte {position}"),
            );
            mutations += 1;
            agree(&seed[..position], &format!("{id} truncated to {position}"));
        }
    }
    eprintln!("{mutations} mutations, {accepted} accepted by both validators");
    assert!(accepted > 0);
}

// ---- A generator of typed programs ------------------------------------------------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len() as u64) as usize]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ty {
    I32,
    I64,
}
impl Ty {
    fn name(self) -> &'static str {
        match self {
            Ty::I32 => "i32",
            Ty::I64 => "i64",
        }
    }
}
struct Sig {
    params: Vec<Ty>,
    result: Option<Ty>,
}
/// What a function body may refer to while it is generated.
struct Scope {
    locals: Vec<Ty>,
    /// Enclosing labels, innermost last: the result a `br` must carry.
    labels: Vec<Option<Ty>>,
    result: Option<Ty>,
    /// Counter locals used by loops, so nested loops do not share one.
    counters: u32,
    /// Helpers this body may call (lower indices only, so no recursion).
    callable: usize,
}
struct Gen {
    rng: Rng,
    globals: Vec<Ty>,
    helpers: Vec<Sig>,
    table: usize,
    nesting: u32,
}

const I32_EDGES: [i32; 10] = [0, 1, -1, 2, 7, 31, 32, 255, i32::MIN, i32::MAX];
const I64_EDGES: [i64; 10] = [
    0,
    1,
    -1,
    63,
    64,
    255,
    1 << 32,
    i64::MIN,
    i64::MAX,
    -(1 << 31),
];

impl Gen {
    fn constant(&mut self, ty: Ty) -> String {
        match ty {
            Ty::I32 => {
                let v = if self.rng.chance(50) {
                    *self.rng.pick(&I32_EDGES)
                } else {
                    self.rng.next() as i32
                };
                format!("(i32.const {v})")
            }
            Ty::I64 => {
                let v = if self.rng.chance(50) {
                    *self.rng.pick(&I64_EDGES)
                } else {
                    self.rng.next() as i64
                };
                format!("(i64.const {v})")
            }
        }
    }
    /// A memory address: usually inside the first page, sometimes anywhere.
    fn address(&mut self, s: &mut Scope) -> String {
        if self.rng.chance(85) {
            format!("(i32.and {} (i32.const 65535))", self.expr(s, Ty::I32))
        } else {
            self.expr(s, Ty::I32)
        }
    }
    fn expr(&mut self, s: &mut Scope, ty: Ty) -> String {
        self.nesting += 1;
        let out = self.expr_inner(s, ty);
        self.nesting -= 1;
        out
    }
    fn expr_inner(&mut self, s: &mut Scope, ty: Ty) -> String {
        let t = ty.name();
        let leaf = self.nesting > 4 || self.rng.chance(30);
        if leaf {
            let locals: Vec<usize> = (0..s.locals.len()).filter(|i| s.locals[*i] == ty).collect();
            let globals: Vec<usize> = (0..self.globals.len())
                .filter(|i| self.globals[*i] == ty)
                .collect();
            return match self.rng.below(4) {
                0 if !locals.is_empty() => format!("(local.get {})", self.rng.pick(&locals)),
                1 if !globals.is_empty() => format!("(global.get {})", self.rng.pick(&globals)),
                _ => self.constant(ty),
            };
        }
        match self.rng.below(100) {
            0..=24 => {
                let ops = [
                    "add", "sub", "mul", "div_s", "div_u", "rem_s", "rem_u", "and", "or", "xor",
                    "shl", "shr_s", "shr_u", "rotl", "rotr",
                ];
                let op = *self.rng.pick(&ops);
                format!("({t}.{op} {} {})", self.expr(s, ty), self.expr(s, ty))
            }
            25..=32 => {
                let op = *self.rng.pick(&["clz", "ctz", "popcnt"]);
                format!("({t}.{op} {})", self.expr(s, ty))
            }
            33..=40 => {
                // Comparisons and tests yield i32; wrap them for i64.
                let other = *self.rng.pick(&[Ty::I32, Ty::I64]);
                let op = *self.rng.pick(&[
                    "eq", "ne", "lt_s", "lt_u", "gt_s", "gt_u", "le_s", "le_u", "ge_s", "ge_u",
                ]);
                let cmp = if self.rng.chance(15) {
                    format!("({}.eqz {})", other.name(), self.expr(s, other))
                } else {
                    format!(
                        "({}.{op} {} {})",
                        other.name(),
                        self.expr(s, other),
                        self.expr(s, other)
                    )
                };
                match ty {
                    Ty::I32 => cmp,
                    Ty::I64 => format!("(i64.extend_i32_u {cmp})"),
                }
            }
            41..=48 => match ty {
                Ty::I32 => {
                    let op = *self
                        .rng
                        .pick(&["i32.wrap_i64", "i32.extend8_s", "i32.extend16_s"]);
                    if op == "i32.wrap_i64" {
                        format!("({op} {})", self.expr(s, Ty::I64))
                    } else {
                        format!("({op} {})", self.expr(s, Ty::I32))
                    }
                }
                Ty::I64 => {
                    let op = *self.rng.pick(&[
                        "i64.extend_i32_s",
                        "i64.extend_i32_u",
                        "i64.extend8_s",
                        "i64.extend16_s",
                        "i64.extend32_s",
                    ]);
                    if op.starts_with("i64.extend_i32") {
                        format!("({op} {})", self.expr(s, Ty::I32))
                    } else {
                        format!("({op} {})", self.expr(s, Ty::I64))
                    }
                }
            },
            49..=58 => {
                let loads = match ty {
                    Ty::I32 => vec![
                        "i32.load",
                        "i32.load8_s",
                        "i32.load8_u",
                        "i32.load16_s",
                        "i32.load16_u",
                    ],
                    Ty::I64 => vec![
                        "i64.load",
                        "i64.load8_s",
                        "i64.load8_u",
                        "i64.load16_s",
                        "i64.load16_u",
                        "i64.load32_s",
                        "i64.load32_u",
                    ],
                };
                let op = *self.rng.pick(&loads);
                let offset = *self.rng.pick(&[0u32, 0, 1, 3, 8, 65_530, 4_294_967_295]);
                format!("({op} offset={offset} {})", self.address(s))
            }
            59..=64 => format!(
                "(select {} {} {})",
                self.expr(s, ty),
                self.expr(s, ty),
                self.expr(s, Ty::I32)
            ),
            65..=70 => {
                s.labels.push(Some(ty));
                let body = format!(
                    "(block (result {t}) (drop (br_if 0 {} {})) {})",
                    self.expr(s, ty),
                    self.expr(s, Ty::I32),
                    self.expr(s, ty)
                );
                s.labels.pop();
                body
            }
            71..=76 => {
                s.labels.push(Some(ty));
                let body = format!(
                    "(if (result {t}) {} (then {}) (else {}))",
                    self.expr(s, Ty::I32),
                    self.expr(s, ty),
                    self.expr(s, ty)
                );
                s.labels.pop();
                body
            }
            77..=84 => {
                let targets: Vec<usize> = (0..s.callable)
                    .filter(|k| self.helpers[*k].result == Some(ty))
                    .collect();
                if targets.is_empty() {
                    return self.constant(ty);
                }
                let k = *self.rng.pick(&targets);
                let params = self.helpers[k].params.clone();
                let args: Vec<String> = params.iter().map(|p| self.expr(s, *p)).collect();
                if self.table > 0 && self.rng.chance(40) {
                    format!(
                        "(call_indirect (type $t{k}) {} (i32.const {}))",
                        args.join(" "),
                        self.rng.below(self.table as u64 + 1)
                    )
                } else {
                    format!("(call $f{k} {})", args.join(" "))
                }
            }
            85..=88 => match ty {
                Ty::I32 => {
                    if self.rng.chance(50) {
                        "(memory.size)".into()
                    } else {
                        format!("(memory.grow (i32.const {}))", self.rng.below(3))
                    }
                }
                Ty::I64 => "(i64.extend_i32_u (memory.size))".to_string(),
            },
            89..=92 => {
                let locals: Vec<usize> =
                    (0..s.locals.len()).filter(|i| s.locals[*i] == ty).collect();
                if locals.is_empty() {
                    return self.constant(ty);
                }
                format!(
                    "(local.tee {} {})",
                    self.rng.pick(&locals),
                    self.expr(s, ty)
                )
            }
            93..=95 => {
                // A host getter's required length, as a value.
                let getter = *self.rng.pick(&["input", "call_result"]);
                let v = format!(
                    "(call ${getter} {} (i32.const {}))",
                    self.address(s),
                    self.rng.below(64)
                );
                match ty {
                    Ty::I32 => v,
                    Ty::I64 => format!("(i64.extend_i32_s {v})"),
                }
            }
            _ => {
                let wide = self.wide(s);
                match ty {
                    Ty::I32 => format!("(i32.wrap_i64 {wide})"),
                    Ty::I64 => wide,
                }
            }
        }
    }
    /// One of the four wide-arithmetic instructions, its two results made
    /// one i64: added, subtracted (the low half minus the high half), xored,
    /// the low half rotated by the high half, or the high half dropped.
    fn wide(&mut self, s: &mut Scope) -> String {
        let (op, n) = *self.rng.pick(&[
            ("i64.add128", 4),
            ("i64.sub128", 4),
            ("i64.mul_wide_s", 2),
            ("i64.mul_wide_u", 2),
        ]);
        let combine = self.rng.below(5);
        if combine == 4 {
            s.labels.push(Some(Ty::I64));
        }
        let args: Vec<String> = (0..n).map(|_| self.expr(s, Ty::I64)).collect();
        let pair = format!("({op} {})", args.join(" "));
        match combine {
            0 => format!("(i64.add {pair})"),
            1 => format!("(i64.sub {pair})"),
            2 => format!("(i64.xor {pair})"),
            3 => format!("(i64.rotl {pair})"),
            _ => {
                s.labels.pop();
                format!("(block (result i64) {pair} (drop))")
            }
        }
    }
    fn stmts(&mut self, s: &mut Scope, n: u64) -> String {
        let mut out = String::new();
        for _ in 0..n {
            out.push_str(&self.stmt(s));
            out.push(' ');
        }
        out
    }
    fn stmt(&mut self, s: &mut Scope) -> String {
        if self.nesting > 3 {
            let ty = *self.rng.pick(&[Ty::I32, Ty::I64]);
            return format!("(drop {})", self.expr(s, ty));
        }
        self.nesting += 1;
        let out = self.stmt_inner(s);
        self.nesting -= 1;
        out
    }
    fn stmt_inner(&mut self, s: &mut Scope) -> String {
        match self.rng.below(100) {
            0..=14 => {
                if s.locals.is_empty() {
                    return "(nop)".into();
                }
                let i = self.rng.below(s.locals.len() as u64) as usize;
                format!("(local.set {i} {})", self.expr(s, s.locals[i]))
            }
            15..=20 => {
                if self.globals.is_empty() {
                    return "(nop)".into();
                }
                let i = self.rng.below(self.globals.len() as u64) as usize;
                let ty = self.globals[i];
                format!("(global.set {i} {})", self.expr(s, ty))
            }
            21..=32 => {
                let (op, ty) = *self.rng.pick(&[
                    ("i32.store", Ty::I32),
                    ("i32.store8", Ty::I32),
                    ("i32.store16", Ty::I32),
                    ("i64.store", Ty::I64),
                    ("i64.store8", Ty::I64),
                    ("i64.store16", Ty::I64),
                    ("i64.store32", Ty::I64),
                ]);
                let offset = *self.rng.pick(&[0u32, 0, 2, 8, 65_532, 4_294_967_295]);
                format!(
                    "({op} offset={offset} {} {})",
                    self.address(s),
                    self.expr(s, ty)
                )
            }
            33..=38 => {
                let ty = *self.rng.pick(&[Ty::I32, Ty::I64]);
                format!("(drop {})", self.expr(s, ty))
            }
            39..=42 => {
                let n = *self.rng.pick(&[0u32, 1, 31, 32, 33, 1000, 65_536, 70_000]);
                if self.rng.chance(50) {
                    format!(
                        "(memory.copy {} {} (i32.const {n}))",
                        self.address(s),
                        self.address(s)
                    )
                } else {
                    format!(
                        "(memory.fill {} {} (i32.const {n}))",
                        self.address(s),
                        self.expr(s, Ty::I32)
                    )
                }
            }
            43..=50 => {
                // A counted loop with an exit block; branches inside may use both labels.
                let counter = s.locals.len();
                s.locals.push(Ty::I32);
                s.counters += 1;
                let iterations = self.rng.below(7);
                s.labels.push(None);
                s.labels.push(None);
                let n = 1 + self.rng.below(3);
                let body = self.stmts(s, n);
                s.labels.pop();
                s.labels.pop();
                format!(
                    "(local.set {counter} (i32.const {iterations})) (block (loop (br_if 1 (i32.eqz (local.get {counter}))) (local.set {counter} (i32.sub (local.get {counter}) (i32.const 1))) {body} (br 0)))"
                )
            }
            51..=58 => {
                s.labels.push(None);
                let (na, nb) = (1 + self.rng.below(3), self.rng.below(3));
                let a = self.stmts(s, na);
                let b = self.stmts(s, nb);
                s.labels.pop();
                format!("(if {} (then {a}) (else {b}))", self.expr(s, Ty::I32))
            }
            59..=63 => {
                // block A { block B { block C { br_table } inner } middle } outer
                let (ni, nm, no) = (1 + self.rng.below(2), self.rng.below(2), self.rng.below(2));
                s.labels.push(None);
                s.labels.push(None);
                let inner = self.stmts(s, ni);
                s.labels.pop();
                let middle = self.stmts(s, nm);
                s.labels.pop();
                let outer = self.stmts(s, no);
                format!(
                    "(block (block (block (br_table 0 1 2 1 {}) ) {inner}) {middle}) {outer}",
                    self.expr(s, Ty::I32)
                )
            }
            64..=67 => {
                // A branch to a random enclosing label that carries no value.
                let plain: Vec<usize> = (0..s.labels.len())
                    .filter(|i| s.labels[*i].is_none())
                    .collect();
                if plain.is_empty() {
                    return "(nop)".into();
                }
                let depth = s.labels.len() - 1 - self.rng.pick(&plain);
                if self.rng.chance(70) {
                    format!("(br_if {depth} {})", self.expr(s, Ty::I32))
                } else {
                    format!(
                        "(block (br_if 0 {}) (br {}))",
                        self.expr(s, Ty::I32),
                        depth + 1
                    )
                }
            }
            68..=72 => format!(
                "(call $emit {} (i32.const {}))",
                self.address(s),
                *self.rng.pick(&[0u64, 1, 5, 40, 100, 70_000])
            ),
            73..=78 => {
                let key = self.rng.below(4) * 32;
                match self.rng.below(3) {
                    0 => format!(
                        "(call $storage_set (i32.const {key}) {} (i32.const {}))",
                        self.address(s),
                        *self.rng.pick(&[0u64, 1, 8, 33, 300, 70_000])
                    ),
                    1 => format!(
                        "(drop (call $storage_get (i32.const {key}) {} (i32.const {})))",
                        self.address(s),
                        *self.rng.pick(&[0u64, 8, 64, 400])
                    ),
                    _ => format!("(call $storage_delete (i32.const {key}))"),
                }
            }
            79..=82 => format!(
                "(drop (call $context (i32.const {}) {} (i32.const {})))",
                self.rng.below(10),
                self.address(s),
                *self.rng.pick(&[0u64, 8, 32, 33, 64])
            ),
            83..=84 => format!(
                "(drop (call $code_hash (i32.const 1024) {} (i32.const {})))",
                self.address(s),
                *self.rng.pick(&[0u64, 31, 32])
            ),
            85..=87 => {
                // A nested call of this very contract. Its code is prepared
                // already, so its entry costs about 84,000 C: the budgets end
                // before it, about at it, and inside the child.
                let c = *self
                    .rng
                    .pick(&[0u64, 50, 5_000, 84_000, 90_000, 150_000, 400_000]);
                format!(
                    "(i64.store (i32.const 2048) (i64.const {c})) (i64.store offset=8 (i32.const 2048) (i64.const 4)) (i64.store offset=16 (i32.const 2048) (i64.const 2000)) (i64.store offset=24 (i32.const 2048) (i64.const 1000)) (drop (call $call (i32.const 1024) (i32.const 0) (i32.const {}) (i32.const 2048) (i32.const {})))",
                    self.rng.below(16),
                    self.rng.below(3)
                )
            }
            88..=90 => {
                let (name, len) = (
                    *self.rng.pick(&["return", "revert"]),
                    *self.rng.pick(&[0u64, 8, 32, 100, 70_000]),
                );
                format!("(call ${name} {} (i32.const {len}))", self.address(s))
            }
            91..=92 => match s.result {
                None => "(return)".into(),
                Some(t) => format!("(return {})", self.expr(s, t)),
            },
            93 => "(unreachable)".into(),
            94..=96 => {
                let targets: Vec<usize> = (0..s.callable).collect();
                if targets.is_empty() {
                    return "(nop)".into();
                }
                let k = *self.rng.pick(&targets);
                let params = self.helpers[k].params.clone();
                let args: Vec<String> = params.iter().map(|p| self.expr(s, *p)).collect();
                let call = format!("(call $f{k} {})", args.join(" "));
                if self.helpers[k].result.is_some() {
                    format!("(drop {call})")
                } else {
                    call
                }
            }
            _ => "(nop)".into(),
        }
    }
    fn body(
        &mut self,
        params: &[Ty],
        result: Option<Ty>,
        callable: usize,
        n: u64,
    ) -> (String, Vec<Ty>) {
        let mut s = Scope {
            locals: params.to_vec(),
            labels: vec![result],
            result,
            counters: 0,
            callable,
        };
        for _ in 0..self.rng.below(4) {
            s.locals.push(*self.rng.pick(&[Ty::I32, Ty::I64]));
        }
        let mut text = self.stmts(&mut s, n);
        if let Some(t) = result {
            let _ = write!(text, "{}", self.expr(&mut s, t));
        }
        let declared = s.locals[params.len()..].to_vec();
        (text, declared)
    }
}

fn locals(tys: &[Ty]) -> String {
    tys.iter()
        .map(|t| format!("(local {})", t.name()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One random program as text.
fn program(seed: u64) -> String {
    let mut g = Gen {
        rng: Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1),
        globals: Vec::new(),
        helpers: Vec::new(),
        table: 0,
        nesting: 0,
    };
    for _ in 0..g.rng.below(4) {
        let t = *g.rng.pick(&[Ty::I32, Ty::I64]);
        g.globals.push(t);
    }
    for _ in 0..g.rng.below(4) {
        let n = g.rng.below(4);
        let params = (0..n).map(|_| *g.rng.pick(&[Ty::I32, Ty::I64])).collect();
        let result = *g.rng.pick(&[None, Some(Ty::I32), Some(Ty::I64)]);
        g.helpers.push(Sig { params, result });
    }
    let mut out = String::from("(module\n");
    for (k, h) in g.helpers.iter().enumerate() {
        let params: Vec<&str> = h.params.iter().map(|t| t.name()).collect();
        let _ = write!(out, "(type $t{k} (func");
        if !params.is_empty() {
            let _ = write!(out, " (param {})", params.join(" "));
        }
        if let Some(r) = h.result {
            let _ = write!(out, " (result {})", r.name());
        }
        out.push_str("))\n");
    }
    for name in [
        "input",
        "context",
        "storage_get",
        "storage_set",
        "storage_delete",
        "code_hash",
        "call",
        "call_result",
        "emit",
        "return",
        "revert",
    ] {
        let _ = writeln!(out, "{}", import(name));
    }
    let max_pages = 1 + g.rng.below(3);
    let _ = writeln!(out, "(memory (export \"memory\") 1 {max_pages})");
    if g.rng.chance(70) {
        let offset = g.rng.below(65_000);
        let len = g.rng.below(40);
        let mut data = String::new();
        for _ in 0..len {
            let _ = write!(data, "\\{:02x}", g.rng.next() as u8);
        }
        let _ = writeln!(out, "(data (i32.const {offset}) \"{data}\")");
    }
    for t in g.globals.clone() {
        let init = match t {
            Ty::I32 => format!("(i32.const {})", g.rng.next() as i32),
            Ty::I64 => format!("(i64.const {})", g.rng.next() as i64),
        };
        let _ = writeln!(out, "(global (mut {}) {init})", t.name());
    }
    // The table lists helpers in a random order with some null entries.
    let helpers = g.helpers.len();
    if helpers > 0 && g.rng.chance(70) {
        let size = 1 + g.rng.below(helpers as u64 + 2) as usize;
        g.table = size;
        let mut entries = Vec::new();
        for _ in 0..size {
            if g.rng.chance(80) {
                entries.push(format!("$f{}", g.rng.below(helpers as u64)));
            }
        }
        let _ = writeln!(out, "(table {size} {size} funcref)");
        if !entries.is_empty() {
            let _ = writeln!(out, "(elem (i32.const 0) {})", entries.join(" "));
        }
    }
    for k in 0..helpers {
        let params = g.helpers[k].params.clone();
        let result = g.helpers[k].result;
        let n = 1 + g.rng.below(4);
        let (body, declared) = g.body(&params, result, k, n);
        let _ = writeln!(
            out,
            "(func $f{k} (type $t{k}) {} {body})",
            locals(&declared)
        );
    }
    let n = 2 + g.rng.below(8);
    let (body, declared) = g.body(&[], None, helpers, n);
    let _ = writeln!(
        out,
        "(func (export \"invoke\") {} (if (i32.eqz (call $input (i32.const 0) (i32.const 0))) (then (return))) (drop (call $context (i32.const 2) (i32.const 1024) (i32.const 32))) {body})",
        locals(&declared)
    );
    out.push(')');
    out
}

fn deploy(runtime: &Runtime, code: &[u8]) -> Result<[u8; 32], Status> {
    let r = runtime
        .execute(&Transaction {
            budget: Resources([5_000_000, 64, 1 << 20, 1 << 20]),
            payload: Payload::Deploy {
                code: code.to_vec(),
                input: Vec::new(),
            },
        })
        .unwrap();
    if r.status == Status::Success {
        Ok(r.result.try_into().unwrap())
    } else {
        Err(r.status)
    }
}

#[test]
fn generated_programs_agree_between_engines() {
    let count: u64 = std::env::var("TING_FUZZ_PROGRAMS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400);
    let first: u64 = std::env::var("TING_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let (mut accepted, mut successes, mut traps, mut exhausted) = (0, 0, 0, 0);
    let mut wide = 0;
    let mut statuses = std::collections::BTreeMap::new();
    for seed in first..first + count {
        let source = program(seed);
        if ["add128", "sub128", "mul_wide"]
            .iter()
            .any(|op| source.contains(op))
        {
            wide += 1;
        }
        let code = wasm(&source);
        assert_eq!(
            profile::validate(&code),
            profile::reference_validate(&code),
            "seed {seed}: validators disagree\n{source}"
        );
        let own = Runtime::new(State::default(), context());
        let oracle = Runtime::with_oracle(State::default(), context());
        let (a, b) = (deploy(&own, &code), deploy(&oracle, &code));
        assert_eq!(a, b, "seed {seed}: deployment\n{source}");
        let id = a.unwrap_or_else(|status| {
            panic!("seed {seed}: deployment failed with {status:?}\n{source}")
        });
        accepted += 1;
        // A call's entry, with the root dispatch: compute budgets run out
        // before it, at it, or at any point after it.
        let entry = 1 + shape(&code).entry().compute();
        let mut rng = Rng(seed | 1);
        for step in 0..3 {
            let c = if rng.chance(15) {
                *rng.pick(&[50u64, 5_000, entry - 1])
            } else {
                entry + *rng.pick(&[0u64, 50, 120, 300, 800, 2_000, 20_000, 200_000, 2_000_000])
            };
            let s = *rng.pick(&[1u64, 3, 8, 64]);
            let d = *rng.pick(&[300u64, 600, 4_096, 100_000]);
            let g = *rng.pick(&[0u64, 40, 500, 100_000]);
            let input: Vec<u8> = (0..1 + rng.below(40)).map(|_| rng.next() as u8).collect();
            let tx = Transaction {
                budget: Resources([c, s, d, g]),
                payload: Payload::Call { target: id, input },
            };
            let (ra, rb) = (own.execute(&tx), oracle.execute(&tx));
            match (&ra, &rb) {
                (Ok(ra), Ok(rb)) => {
                    assert_eq!(
                        hex(&ra.encode()),
                        hex(&rb.encode()),
                        "seed {seed} step {step}: receipts differ\nown {ra:?}\noracle {rb:?}\n{source}"
                    );
                    assert_eq!(
                        own.state_root(),
                        oracle.state_root(),
                        "seed {seed} step {step}: state roots differ\n{source}"
                    );
                    *statuses.entry(ra.status as u8).or_insert(0u64) += 1;
                    match ra.status {
                        Status::Success => successes += 1,
                        Status::Trap => traps += 1,
                        Status::Compute => exhausted += 1,
                        _ => {}
                    }
                }
                (Err(a), Err(b)) => assert_eq!(a.0, b.0, "seed {seed} step {step}"),
                _ => panic!("seed {seed} step {step}: {ra:?} vs {rb:?}\n{source}"),
            }
            own.commit();
            oracle.commit();
        }
    }
    eprintln!(
        "{count} programs, {accepted} deployable, {wide} with wide arithmetic; {successes} successes, {traps} traps, {exhausted} compute exhaustions; statuses {statuses:?}"
    );
    assert_eq!(accepted, count);
    assert!(successes > 0 && traps > 0 && exhausted > 0);
    assert!(count < 50 || wide > 0, "no program uses wide arithmetic");
}
