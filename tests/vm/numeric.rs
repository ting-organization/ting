//! Integer instructions (wiki: vm/instructions): both widths, every operator over
//! edge operands, the trapping divisions, the conversions and the sign
//! extensions. Expected values are Rust's integer arithmetic over the same
//! operands; the VM never computes an expectation. Every instruction of a
//! program runs once, in a one-page memory, so its dispatch is the price of
//! its text (wiki: resources/prices).
use crate::{
    dsl::{import, invoke, memory, module},
    harness::case,
};
use ting_core::protocol::Status;
use vm_model::{Ledger, cost};

/// The instructions that store the i64 value of `expression` at address 0
/// and return those eight bytes: the address, the expression, the widening
/// of a 32-bit result, the store, the span and the return.
fn body(expression: &str, width: u32) -> String {
    let widen = if width == 32 { " i64.extend_i32_u" } else { "" };
    format!("i32.const 0 {expression}{widen} i64.store i32.const 0 i32.const 8 call $return")
}
fn program(expression: &str, width: u32) -> String {
    module(&[
        &import("return"),
        &memory(1, 2),
        &invoke(&body(expression, width)),
    ])
}
/// The value `expression` evaluates to, as the eight returned bytes; every
/// instruction of the body runs once.
fn evaluates(expression: &str, width: u32, value: u64) {
    case(&program(expression, width))
        .named(expression)
        .call()
        .result(value.to_le_bytes())
        .charged(Ledger {
            dispatch: cost(&body(expression, width)),
            ..Default::default()
        })
        .run();
}
/// `expression` traps at its last instruction, after that instruction's
/// price is debited: the address constant and the whole expression are paid.
fn traps(expression: &str, width: u32) {
    case(&program(expression, width))
        .named(expression)
        .call()
        .expect(Status::Trap)
        .charged(Ledger {
            dispatch: cost(&format!("i32.const 0 {expression}")),
            ..Default::default()
        })
        .run();
}

/// A fixed pseudo-random sequence for operands beyond the chosen edges.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}
fn pairs32() -> Vec<(i32, i32)> {
    let mut pairs = vec![
        (0, 0),
        (1, 0),
        (0, 1),
        (-1, 1),
        (1, -1),
        (-1, -1),
        (i32::MIN, -1),
        (i32::MIN, 1),
        (-7, 3),
        (7, -3),
        (1, 32),
        (1, 33),
        (-1, 2),
        (0x55AA_55AA, 0xAA55_AA55u32 as i32),
    ];
    // Operands the WebAssembly core test suite singles out: overflow edges,
    // every sign combination of a division, shift and rotation counts at and
    // past the width, and masks.
    pairs.extend([
        (0x7fff_ffff, 1),
        (0x3fff_ffff, 1),
        (0x4000_0000, 0x4000_0000),
        (0x1000_0000, 4096),
        (0x0123_4567, 0x7654_3210),
        (i32::MIN, 2),
        (-2_147_483_647, 1000),
        (5, 2),
        (-5, 2),
        (5, -2),
        (-5, -2),
        (7, 3),
        (-7, 3),
        (7, -3),
        (-7, -3),
        (11, 5),
        (17, 7),
        (0x8ff0_0ff0u32 as i32, 0x1_0001),
        (0xf0f0_ffffu32 as i32, 0xffff_f0f0u32 as i32),
        (0x7fff_ffff, -1),
        (-1, 0),
        (1, 31),
        (1, -1),
        (1, 0x7fff_ffff),
        (1, i32::MIN),
        (0x7fff_ffff, 31),
        (i32::MIN, 31),
        (i32::MIN, 1),
        (0xabcd_9876u32 as i32, 1),
        (0xfe00_dc00u32 as i32, 4),
        (0xb0c1_d2e3u32 as i32, 5),
        (0x0000_8000, 37),
        (0xb0c1_d2e3u32 as i32, 0xff05),
        (0x769a_bcdfu32 as i32, 0xffff_ffedu32 as i32),
        (0x769a_bcdfu32 as i32, 0x8000_000du32 as i32),
    ]);
    let mut rng = Rng(0x0C4A1);
    for _ in 0..4 {
        pairs.push((rng.next() as i32, rng.next() as i32));
    }
    pairs
}
fn pairs64() -> Vec<(i64, i64)> {
    let mut pairs = vec![
        (0, 0),
        (1, 0),
        (0, 1),
        (-1, 1),
        (1, -1),
        (-1, -1),
        (i64::MIN, -1),
        (i64::MIN, 1),
        (-7, 3),
        (7, -3),
        (1, 64),
        (1, 65),
        (-1, 2),
        (0x55AA_55AA_55AA_55AA, 0xAA55_AA55_AA55_AA55u64 as i64),
    ];
    pairs.extend([
        (0x7fff_ffff_ffff_ffff, 1),
        (0x3fff_ffff_ffff_ffff, 1),
        (0x4000_0000_0000_0000, 0x4000_0000_0000_0000),
        (0x1000_0000_0000_0000, 4096),
        (0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210u64 as i64),
        (i64::MIN, 2),
        (i64::MIN + 1, 1000),
        (5, 2),
        (-5, 2),
        (5, -2),
        (-5, -2),
        (7, 3),
        (-7, 3),
        (7, -3),
        (-7, -3),
        (11, 5),
        (17, 7),
        (0x8ff0_0ff0_0ff0_0ff0u64 as i64, 0x1_0000_0001),
        (
            0xf0f0_ffff_f0f0_ffffu64 as i64,
            0xffff_f0f0_ffff_f0f0u64 as i64,
        ),
        (i64::MAX, -1),
        (-1, 0),
        (1, 63),
        (1, -1),
        (1, i64::MAX),
        (1, i64::MIN),
        (i64::MAX, 63),
        (i64::MIN, 63),
        (i64::MIN, 1),
        (0xabcd_9876_1234_5678u64 as i64, 1),
        (0xfe00_dc00_0000_0000u64 as i64, 4),
        (0xb0c1_d2e3_f4a5_b6c7u64 as i64, 5),
        (0x0000_8000_0000_0000, 69),
        (0xb0c1_d2e3_f4a5_b6c7u64 as i64, 0xff05),
        (0x769a_bcdf_1234_5678u64 as i64, -19),
        (
            0x769a_bcdf_1234_5678u64 as i64,
            0x8000_0000_0000_000du64 as i64,
        ),
    ]);
    let mut rng = Rng(0x0C4A1);
    for _ in 0..4 {
        pairs.push((rng.next() as i64, rng.next() as i64));
    }
    pairs
}

/// Core semantics of the binary operators: `None` traps.
type Table<T> = Vec<(&'static str, fn(T, T) -> Option<T>)>;
fn binary32() -> Table<i32> {
    vec![
        ("add", |a, b| Some(a.wrapping_add(b))),
        ("sub", |a, b| Some(a.wrapping_sub(b))),
        ("mul", |a, b| Some(a.wrapping_mul(b))),
        ("div_s", |a, b| a.checked_div(b)),
        ("div_u", |a, b| {
            (a as u32).checked_div(b as u32).map(|v| v as i32)
        }),
        ("rem_s", |a, b| (b != 0).then(|| a.wrapping_rem(b))),
        ("rem_u", |a, b| {
            (a as u32).checked_rem(b as u32).map(|v| v as i32)
        }),
        ("and", |a, b| Some(a & b)),
        ("or", |a, b| Some(a | b)),
        ("xor", |a, b| Some(a ^ b)),
        ("shl", |a, b| Some(a.wrapping_shl(b as u32))),
        ("shr_s", |a, b| Some(a.wrapping_shr(b as u32))),
        ("shr_u", |a, b| {
            Some((a as u32).wrapping_shr(b as u32) as i32)
        }),
        ("rotl", |a, b| Some(a.rotate_left(b as u32 & 31))),
        ("rotr", |a, b| Some(a.rotate_right(b as u32 & 31))),
        ("eq", |a, b| Some((a == b) as i32)),
        ("ne", |a, b| Some((a != b) as i32)),
        ("lt_s", |a, b| Some((a < b) as i32)),
        ("lt_u", |a, b| Some(((a as u32) < (b as u32)) as i32)),
        ("gt_s", |a, b| Some((a > b) as i32)),
        ("gt_u", |a, b| Some(((a as u32) > (b as u32)) as i32)),
        ("le_s", |a, b| Some((a <= b) as i32)),
        ("le_u", |a, b| Some(((a as u32) <= (b as u32)) as i32)),
        ("ge_s", |a, b| Some((a >= b) as i32)),
        ("ge_u", |a, b| Some(((a as u32) >= (b as u32)) as i32)),
    ]
}
fn binary64() -> Table<i64> {
    vec![
        ("add", |a, b| Some(a.wrapping_add(b))),
        ("sub", |a, b| Some(a.wrapping_sub(b))),
        ("mul", |a, b| Some(a.wrapping_mul(b))),
        ("div_s", |a, b| a.checked_div(b)),
        ("div_u", |a, b| {
            (a as u64).checked_div(b as u64).map(|v| v as i64)
        }),
        ("rem_s", |a, b| (b != 0).then(|| a.wrapping_rem(b))),
        ("rem_u", |a, b| {
            (a as u64).checked_rem(b as u64).map(|v| v as i64)
        }),
        ("and", |a, b| Some(a & b)),
        ("or", |a, b| Some(a | b)),
        ("xor", |a, b| Some(a ^ b)),
        ("shl", |a, b| Some(a.wrapping_shl(b as u32))),
        ("shr_s", |a, b| Some(a.wrapping_shr(b as u32))),
        ("shr_u", |a, b| {
            Some((a as u64).wrapping_shr(b as u32) as i64)
        }),
        ("rotl", |a, b| Some(a.rotate_left(b as u32 & 63))),
        ("rotr", |a, b| Some(a.rotate_right(b as u32 & 63))),
        ("eq", |a, b| Some((a == b) as i64)),
        ("ne", |a, b| Some((a != b) as i64)),
        ("lt_s", |a, b| Some((a < b) as i64)),
        ("lt_u", |a, b| Some(((a as u64) < (b as u64)) as i64)),
        ("gt_s", |a, b| Some((a > b) as i64)),
        ("gt_u", |a, b| Some(((a as u64) > (b as u64)) as i64)),
        ("le_s", |a, b| Some((a <= b) as i64)),
        ("le_u", |a, b| Some(((a as u64) <= (b as u64)) as i64)),
        ("ge_s", |a, b| Some((a >= b) as i64)),
        ("ge_u", |a, b| Some(((a as u64) >= (b as u64)) as i64)),
    ]
}
/// Comparisons produce an i32 whatever the operand width.
fn comparison(name: &str) -> bool {
    matches!(
        name,
        "eq" | "ne" | "lt_s" | "lt_u" | "gt_s" | "gt_u" | "le_s" | "le_u" | "ge_s" | "ge_u"
    )
}

#[test]
fn i32_binary_operators_over_edge_operands() {
    for (name, f) in binary32() {
        for (a, b) in pairs32() {
            let expression = format!("i32.const {a} i32.const {b} i32.{name}");
            match f(a, b) {
                Some(v) => evaluates(&expression, 32, v as u32 as u64),
                None => traps(&expression, 32),
            }
        }
    }
}
#[test]
fn i64_binary_operators_over_edge_operands() {
    for (name, f) in binary64() {
        for (a, b) in pairs64() {
            let expression = format!("i64.const {a} i64.const {b} i64.{name}");
            match f(a, b) {
                Some(v) if comparison(name) => evaluates(&expression, 32, v as u64),
                Some(v) => evaluates(&expression, 64, v as u64),
                None => traps(&expression, 64),
            }
        }
    }
}
#[test]
fn unary_operators_count_bits() {
    for n in [0i32, 1, 2, -1, i32::MIN, 0x5555_5555] {
        evaluates(&format!("i32.const {n} i32.eqz"), 32, (n == 0) as u64);
        evaluates(
            &format!("i32.const {n} i32.clz"),
            32,
            n.leading_zeros().into(),
        );
        evaluates(
            &format!("i32.const {n} i32.ctz"),
            32,
            n.trailing_zeros().into(),
        );
        evaluates(
            &format!("i32.const {n} i32.popcnt"),
            32,
            n.count_ones().into(),
        );
    }
    for n in [0i64, 1, 2, -1, i64::MIN, 0x5555_5555_5555_5555] {
        evaluates(&format!("i64.const {n} i64.eqz"), 32, (n == 0) as u64);
        evaluates(
            &format!("i64.const {n} i64.clz"),
            64,
            n.leading_zeros().into(),
        );
        evaluates(
            &format!("i64.const {n} i64.ctz"),
            64,
            n.trailing_zeros().into(),
        );
        evaluates(
            &format!("i64.const {n} i64.popcnt"),
            64,
            n.count_ones().into(),
        );
    }
}
#[test]
fn conversions_wrap_and_extend() {
    for n in [0i64, 1, -1, i64::MIN, i64::MAX, 1 << 32, (1 << 32) - 1] {
        evaluates(&format!("i64.const {n} i32.wrap_i64"), 32, n as u32 as u64);
    }
    for n in [0i32, 1, -1, i32::MIN, i32::MAX] {
        evaluates(
            &format!("i32.const {n} i64.extend_i32_s"),
            64,
            n as i64 as u64,
        );
        evaluates(
            &format!("i32.const {n} i64.extend_i32_u"),
            64,
            n as u32 as u64,
        );
    }
}
#[test]
fn sign_extension_operators_follow_core_semantics() {
    for n in [
        0i32, 1, 0x7f, 0x80, 0xff, 0x100, 0x7fff, 0x8000, 0xffff, 0x1_0000, -1,
    ] {
        evaluates(
            &format!("i32.const {n} i32.extend8_s"),
            32,
            n as i8 as i32 as u32 as u64,
        );
        evaluates(
            &format!("i32.const {n} i32.extend16_s"),
            32,
            n as i16 as i32 as u32 as u64,
        );
    }
    for n in [
        0i64,
        1,
        0x7f,
        0x80,
        0xff,
        0x100,
        0x7fff,
        0x8000,
        0xffff,
        0x1_0000,
        0x7fff_ffff,
        0x8000_0000,
        0xffff_ffff,
        0x1_0000_0000,
        -1,
    ] {
        evaluates(
            &format!("i64.const {n} i64.extend8_s"),
            64,
            n as i8 as i64 as u64,
        );
        evaluates(
            &format!("i64.const {n} i64.extend16_s"),
            64,
            n as i16 as i64 as u64,
        );
        evaluates(
            &format!("i64.const {n} i64.extend32_s"),
            64,
            n as i32 as i64 as u64,
        );
    }
}
#[test]
fn expressions_are_not_folded_around_overflow_and_signedness() {
    // Identities an optimising implementation might wrongly apply; the
    // expected values are the same arithmetic in Rust.
    let min = i32::MIN;
    let cases: [(&str, u32, u64); 16] = [
        (
            "(i32.lt_s (i32.add (i32.const 2147483647) (i32.const 1)) (i32.add (i32.const 0) (i32.const 1)))",
            32,
            u64::from(i32::MAX.wrapping_add(1) < 1),
        ),
        (
            "(i32.gt_u (i32.add (i32.const -1) (i32.const 1)) (i32.add (i32.const 0) (i32.const 1)))",
            32,
            u64::from((-1i32).wrapping_add(1) as u32 > 1),
        ),
        (
            "(i32.div_s (i32.sub (i32.const 0) (i32.const -2147483648)) (i32.const 2))",
            32,
            (0i32.wrapping_sub(min) / 2) as u32 as u64,
        ),
        (
            "(i32.shr_u (i32.shl (i32.const -2147483648) (i32.const 1)) (i32.const 1))",
            32,
            ((min as u32).wrapping_shl(1) >> 1) as u64,
        ),
        (
            "(i32.rem_s (i32.const -2147483648) (i32.const -1))",
            32,
            min.wrapping_rem(-1) as u32 as u64,
        ),
        (
            "(i32.wrap_i64 (i64.extend_i32_s (i32.const -1)))",
            32,
            (-1i32 as i64) as u32 as u64,
        ),
        (
            "(i32.eq (i32.wrap_i64 (i64.extend_i32_u (i32.const -1))) (i32.const -1))",
            32,
            u64::from((-1i32 as u32 as u64) as u32 == -1i32 as u32),
        ),
        (
            "(i32.mul (i32.div_s (i32.const 7) (i32.const 2)) (i32.const 2))",
            32,
            ((7 / 2) * 2) as u64,
        ),
        (
            "(i32.and (i32.shr_s (i32.const -2147483648) (i32.const 31)) (i32.const 1))",
            32,
            ((min >> 31) & 1) as u64,
        ),
        (
            "(i32.xor (i32.rotl (i32.const 305419896) (i32.const 32)) (i32.const 305419896))",
            32,
            (305_419_896u32.rotate_left(32 & 31) ^ 305_419_896) as u64,
        ),
        (
            "(i32.ge_s (i32.sub (i32.const -2147483648) (i32.const 1)) (i32.const 0))",
            32,
            u64::from(min.wrapping_sub(1) >= 0),
        ),
        (
            "(i64.lt_s (i64.add (i64.const 9223372036854775807) (i64.const 1)) (i64.const 0))",
            32,
            u64::from(i64::MAX.wrapping_add(1) < 0),
        ),
        (
            "(i64.div_s (i64.sub (i64.const 0) (i64.const -9223372036854775808)) (i64.const 2))",
            64,
            (0i64.wrapping_sub(i64::MIN) / 2) as u64,
        ),
        (
            "(i64.shr_u (i64.shl (i64.const 1) (i64.const 63)) (i64.const 63))",
            64,
            (1u64 << 63) >> 63,
        ),
        (
            "(i64.ne (i64.extend_i32_u (i32.const -1)) (i64.const -1))",
            32,
            u64::from(u64::from(u32::MAX) != u64::MAX),
        ),
        (
            "(i32.wrap_i64 (i64.mul (i64.const 4294967296) (i64.const 4294967296)))",
            32,
            (4_294_967_296u64.wrapping_mul(4_294_967_296) as u32) as u64,
        ),
    ];
    for (expression, width, value) in cases {
        evaluates(expression, width, value);
    }
}
