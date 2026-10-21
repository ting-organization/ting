//! Wide arithmetic (rules §19.7, §18,
//! §17, §15): `i64.add128`, `i64.sub128`,
//! `i64.mul_wide_s` and `i64.mul_wide_u` over edge operands, the order of
//! their two results, their typing, their encodings, their prices and the two
//! slots their results take. Expected values are Rust's `u128` and `i128`
//! arithmetic over the same operands, and a few stated by hand beside it; the
//! VM never computes an expectation.
use crate::{
    dsl::{import, invoke, memory, module, wasm},
    harness::{deployed as case, deployed_entry_of as entry_of},
    validation::{ENCODING, assemble, expect, reason, simple_with},
};
use ting_core::protocol::Status;
use vm_model::{BUDGET, Ledger, cost};

const MAX: u64 = u64::MAX;
const MIN: u64 = i64::MIN as u64;
const SIGNED_MAX: u64 = i64::MAX as u64;

#[derive(Clone, Copy, Debug)]
enum Wide {
    Add128,
    Sub128,
    MulWideS,
    MulWideU,
}
use Wide::*;
const ALL: [Wide; 4] = [Add128, Sub128, MulWideS, MulWideU];
impl Wide {
    fn name(self) -> &'static str {
        match self {
            Add128 => "i64.add128",
            Sub128 => "i64.sub128",
            MulWideS => "i64.mul_wide_s",
            MulWideU => "i64.mul_wide_u",
        }
    }
    /// The `fc` sub-opcode (rules §17).
    fn sub_opcode(self) -> u8 {
        match self {
            Add128 => 0x13,
            Sub128 => 0x14,
            MulWideS => 0x15,
            MulWideU => 0x16,
        }
    }
    fn operands(self) -> usize {
        match self {
            Add128 | Sub128 => 4,
            MulWideS | MulWideU => 2,
        }
    }
    /// The price from the price list.
    fn price(self) -> u64 {
        match self {
            Add128 | Sub128 => 2,
            MulWideS | MulWideU => 8,
        }
    }
    /// The 128-bit result: left low, left high, right low, right high for the
    /// sum and the difference modulo 2^128; the full product of two signed or
    /// two unsigned values.
    fn apply(self, x: &[u64]) -> u128 {
        match self {
            Add128 => join(x[0], x[1]).wrapping_add(join(x[2], x[3])),
            Sub128 => join(x[0], x[1]).wrapping_sub(join(x[2], x[3])),
            MulWideS => (i128::from(x[0] as i64) * i128::from(x[1] as i64)) as u128,
            MulWideU => u128::from(x[0]) * u128::from(x[1]),
        }
    }
}
fn join(low: u64, high: u64) -> u128 {
    u128::from(high) << 64 | u128::from(low)
}
fn halves(value: u128) -> (u64, u64) {
    (value as u64, (value >> 64) as u64)
}

/// Pushes `operands` as constants, runs `op`, sets the value on top into
/// local 1 and the one below into local 0, and returns local 0 then local 1
/// as sixteen bytes: the low half then the high half when the high half is on
/// top.
fn instructions(op: &str, operands: &[u64]) -> String {
    let pushes: Vec<String> = operands
        .iter()
        .map(|v| format!("i64.const {}", *v as i64))
        .collect();
    format!(
        "{} {op} local.set 1 local.set 0 i32.const 0 local.get 0 i64.store i32.const 8 local.get 1 i64.store i32.const 0 i32.const 16 call $return",
        pushes.join(" ")
    )
}
fn program(instructions: &str) -> String {
    module(&[
        &import("return"),
        &memory(1, 1),
        &invoke(&format!("(local i64 i64) {instructions}")),
    ])
}
fn sixteen(low: u64, high: u64) -> Vec<u8> {
    [low.to_le_bytes(), high.to_le_bytes()].concat()
}
fn dispatch(n: u64) -> Ledger {
    Ledger {
        dispatch: n,
        ..Default::default()
    }
}

/// `op` over `operands` returns the halves of Rust's result, low then high,
/// at the price of the instructions it ran.
fn computes(op: Wide, operands: &[u64]) {
    let (low, high) = halves(op.apply(operands));
    let text = instructions(op.name(), operands);
    case(&program(&text))
        .named(format!("{} {operands:#x?}", op.name()))
        .call()
        .result(sixteen(low, high))
        .charged(dispatch(cost(&text)))
        .run();
}
/// The same, with the halves also stated by hand.
fn computes_by_hand(op: Wide, cases: &[(&[u64], (u64, u64))]) {
    for (operands, by_hand) in cases {
        assert_eq!(
            halves(op.apply(operands)),
            *by_hand,
            "{} {operands:#x?}: Rust and the hand disagree",
            op.name()
        );
        computes(op, operands);
    }
}

/// Edge operands: zero, one, two, every bit, the signed extremes, the 32-bit
/// boundaries and an irregular pattern.
const EDGES: [u64; 10] = [
    0,
    1,
    2,
    MAX,
    MAX - 1,
    SIGNED_MAX,
    MIN,
    0xffff_ffff,
    1 << 32,
    0x0123_4567_89ab_cdef,
];

/// A fixed pseudo-random sequence for operands beyond the edges.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
pub(crate) fn add128_carries_into_the_high_half_and_wraps_at_two_to_the_128() {
    computes_by_hand(
        Add128,
        &[
            (&[0, 0, 0, 0], (0, 0)),
            (&[1, 0, 1, 0], (2, 0)),
            // A carry out of the low half.
            (&[MAX, 0, 1, 0], (0, 1)),
            (&[MAX, 5, MAX, 7], (MAX - 1, 13)),
            (&[0, 1, 0, 1], (0, 2)),
            // A carry out of the high half is lost: the sum is modulo 2^128.
            (&[MAX, MAX, 1, 0], (0, 0)),
            (&[1, 0, MAX, MAX], (0, 0)),
            (&[MAX, MAX, MAX, MAX], (MAX - 1, MAX)),
            // The high half passes its sign bit without a trap.
            (&[0, SIGNED_MAX, 0, 1], (0, MIN)),
            (&[0, MIN, 0, MIN], (0, 0)),
        ],
    );
}
#[test]
pub(crate) fn sub128_borrows_from_the_high_half_and_wraps_below_zero() {
    computes_by_hand(
        Sub128,
        &[
            (&[0, 0, 0, 0], (0, 0)),
            (&[5, 7, 5, 7], (0, 0)),
            // A borrow from the high half.
            (&[0, 1, 1, 0], (MAX, 0)),
            // Below zero: two's complement modulo 2^128.
            (&[0, 0, 1, 0], (MAX, MAX)),
            (&[3, 0, 5, 0], (MAX - 1, MAX)),
            (&[1, 2, 3, 4], (MAX - 1, MAX - 2)),
            (&[0, 0, MAX, MAX], (1, 0)),
            (&[0, MIN, 1, 0], (MAX, SIGNED_MAX)),
            (&[MAX, MAX, MAX, MAX], (0, 0)),
            // Left minus right, not right minus left.
            (&[5, 0, 3, 0], (2, 0)),
        ],
    );
}
#[test]
pub(crate) fn mul_wide_u_keeps_the_whole_unsigned_product() {
    computes_by_hand(
        MulWideU,
        &[
            (&[0, MAX], (0, 0)),
            (&[1, MAX], (MAX, 0)),
            (&[2, MAX], (MAX - 1, 1)),
            (&[MAX, MAX], (1, MAX - 1)),
            (&[1 << 32, 1 << 32], (0, 1)),
            (&[0xffff_ffff, 0xffff_ffff], (0xffff_fffe_0000_0001, 0)),
            (&[MIN, 2], (0, 1)),
            (&[MIN, MIN], (0, 1 << 62)),
        ],
    );
}
#[test]
pub(crate) fn mul_wide_s_keeps_the_whole_signed_product_and_never_traps() {
    computes_by_hand(
        MulWideS,
        &[
            (&[0, MAX], (0, 0)),
            (&[1, MAX], (MAX, MAX)),
            (&[MAX, MAX], (1, 0)),
            (&[MAX - 1, 3], (MAX - 5, MAX)),
            (&[2, SIGNED_MAX], (MAX - 1, 0)),
            // The minimum times -1 is 2^63, which i64.mul wraps and i64.div_s
            // traps on; here it is simply the product.
            (&[MIN, MAX], (MIN, 0)),
            (&[MIN, MIN], (0, 1 << 62)),
            (&[MIN, SIGNED_MAX], (MIN, 0xc000_0000_0000_0000)),
            (&[SIGNED_MAX, SIGNED_MAX], (1, 0x3fff_ffff_ffff_ffff)),
            // The same bits as unsigned operands give another product.
            (&[MAX, 2], (MAX - 1, MAX)),
        ],
    );
}

#[test]
pub(crate) fn every_pair_of_edges() {
    for (i, &a) in EDGES.iter().enumerate() {
        for (j, &b) in EDGES.iter().enumerate() {
            computes(MulWideS, &[a, b]);
            computes(MulWideU, &[a, b]);
            // The low halves over every pair, the high halves drawn from the
            // edges so that carries, borrows and wraps meet every sign.
            let (c, d) = (EDGES[(i + 3 * j) % 10], EDGES[(7 * i + j + 1) % 10]);
            computes(Add128, &[a, c, b, d]);
            computes(Sub128, &[a, c, b, d]);
        }
    }
}
#[test]
pub(crate) fn seeded_operands() {
    let mut rng = Rng(0x5eed_0128);
    for _ in 0..24 {
        let x: Vec<u64> = (0..4).map(|_| rng.next()).collect();
        computes(Add128, &x);
        computes(Sub128, &x);
        computes(MulWideS, &x[..2]);
        computes(MulWideU, &x[2..]);
    }
}

#[test]
pub(crate) fn the_low_half_is_pushed_first_and_the_high_half_is_on_top() {
    // Dropping the top leaves the low half; keeping the top keeps the high
    // half. Each result has halves that differ.
    for (op, operands, low, high) in [
        ("i64.mul_wide_u", "i64.const -1 i64.const -1", 1, MAX - 1),
        ("i64.mul_wide_s", "i64.const -1 i64.const 2", MAX - 1, MAX),
        (
            "i64.add128",
            "i64.const -1 i64.const 0 i64.const 1 i64.const 0",
            0,
            1,
        ),
        (
            "i64.sub128",
            "i64.const 0 i64.const 1 i64.const 1 i64.const 0",
            MAX,
            0,
        ),
    ] {
        for (keep, value) in [("drop", low), ("local.set 0 drop local.get 0", high)] {
            let text = format!(
                "i32.const 0 {operands} {op} {keep} i64.store i32.const 0 i32.const 8 call $return"
            );
            case(&module(&[
                &import("return"),
                &memory(1, 1),
                &invoke(&format!("(local i64) {text}")),
            ]))
            .named(format!("{op} then {keep}"))
            .call()
            .result(value.to_le_bytes())
            .charged(dispatch(cost(&text)))
            .run();
        }
    }
}

#[test]
pub(crate) fn they_are_priced_two_and_eight() {
    for op in ALL {
        // The model reads the same price list.
        assert_eq!(cost(op.name()), op.price(), "{}", op.name());
        let operands = vec![3; op.operands()];
        let text = program(&instructions(op.name(), &operands));
        let entry = 1 + entry_of(&text);
        let pushes = cost(&"i64.const 3 ".repeat(op.operands()));
        // One C short of its price: it does not execute and nothing of it is
        // charged.
        case(&text)
            .named(format!("{} one short", op.name()))
            .call()
            .budget([
                entry + pushes + op.price() - 1,
                BUDGET[1],
                BUDGET[2],
                BUDGET[3],
            ])
            .expect(Status::Compute)
            .charged(dispatch(pushes))
            .run();
        // Exactly its price: it executes, and the local.set after it finds
        // nothing left.
        case(&text)
            .named(format!("{} exactly", op.name()))
            .call()
            .budget([entry + pushes + op.price(), BUDGET[1], BUDGET[2], BUDGET[3]])
            .expect(Status::Compute)
            .charged(dispatch(pushes + op.price()))
            .run();
    }
}

#[test]
pub(crate) fn two_results_take_two_slots_of_the_operand_height() {
    // $w's operand height peaks at four: the two halves of the product with
    // its two arguments pushed over them for the sum. Its reservation is its
    // two parameters and those four slots; were the halves one slot, three.
    const W: &str =
        "local.get 0 local.get 1 i64.mul_wide_u local.get 0 local.get 1 i64.add128 i64.xor";
    const INVOKE: &str = "i32.const 0 i64.const -1 i64.const 3 call $w i64.store i32.const 0 i32.const 8 call $return";
    let text = module(&[
        &import("return"),
        &memory(1, 1),
        &format!("(func $w (param i64 i64) (result i64) {W})"),
        &invoke(INVOKE),
    ]);
    let (x, y) = (MAX, 3);
    let product = u128::from(x) * u128::from(y);
    let (low, high) = halves(product.wrapping_add(join(x, y)));
    case(&text)
        .call()
        .result((low ^ high).to_le_bytes())
        .charged(Ledger {
            dispatch: cost(INVOKE) + cost(W),
            reservations: 2 + 4,
            ..Default::default()
        })
        .run();
}

#[test]
pub(crate) fn operands_must_be_i64_and_both_results_must_be_consumed() {
    let body = |instructions: &str| wasm(&module(&[&invoke(instructions)]));
    for op in ALL {
        let name = op.name();
        let operands = "i64.const 1 ".repeat(op.operands());
        // An i32 in any operand position.
        for wrong in 0..op.operands() {
            let typed: Vec<&str> = (0..op.operands())
                .map(|k| {
                    if k == wrong {
                        "i32.const 1"
                    } else {
                        "i64.const 1"
                    }
                })
                .collect();
            expect(
                &format!("{name} with an i32 as operand {wrong}"),
                &body(&format!("{} {name} drop drop", typed.join(" "))),
                false,
            );
        }
        for (what, instructions, accept) in [
            (
                "both results dropped",
                format!("{operands}{name} drop drop"),
                true,
            ),
            (
                "both results added",
                format!("{operands}{name} i64.add drop"),
                true,
            ),
            (
                "one operand missing",
                format!(
                    "{}{name} drop drop",
                    "i64.const 1 ".repeat(op.operands() - 1)
                ),
                false,
            ),
            (
                "results used as i32",
                format!("{operands}{name} i32.add drop"),
                false,
            ),
            (
                "one result left over",
                format!("{operands}{name} drop"),
                false,
            ),
            (
                "three results dropped",
                format!("{operands}{name} drop drop drop"),
                false,
            ),
            (
                "both results as a block's one",
                format!("(block (result i64) {operands}{name}) drop"),
                false,
            ),
            (
                "the low half as a block's result",
                format!("(block (result i64) {operands}{name} drop) drop"),
                true,
            ),
            (
                "unknown operands in dead code",
                format!("unreachable {name} drop drop"),
                true,
            ),
            (
                "an i64 result tested as i32 in dead code",
                format!("unreachable {name} i32.eqz drop drop"),
                false,
            ),
            (
                "a known i32 operand in dead code",
                format!("unreachable i32.const 1 {name} drop drop"),
                false,
            ),
        ] {
            expect(&format!("{name}: {what}"), &body(&instructions), accept);
        }
        // A function has at most one result.
        expect(
            &format!("{name}: both results as a function's one"),
            &wasm(&module(&[
                &format!("(func $f (result i64) {operands}{name})"),
                &invoke("call $f drop"),
            ])),
            false,
        );
        expect(
            &format!("{name}: the low half as a function's result"),
            &wasm(&module(&[
                &format!("(func $f (result i64) {operands}{name} drop)"),
                &invoke("call $f drop"),
            ])),
            true,
        );
    }
    // The results of one feed the next.
    expect(
        "a product's halves as the left operand of a sum, and its halves multiplied",
        &body(
            "i64.const 1 i64.const 2 i64.mul_wide_u i64.const 3 i64.const 4 i64.add128 i64.mul_wide_s drop drop",
        ),
        true,
    );
}

#[test]
pub(crate) fn each_is_fc_and_its_sub_opcode_without_immediates() {
    for op in ALL {
        let name = op.name();
        let mut code: Vec<u8> = (1..=op.operands() as u8).flat_map(|v| [0x42, v]).collect();
        let pushes: Vec<String> = (1..=op.operands())
            .map(|v| format!("i64.const {v}"))
            .collect();
        let mut canonical = code.clone();
        canonical.extend([0xfc, op.sub_opcode(), 0x1a, 0x1a]);
        let module_bytes = assemble(&simple_with(&canonical));
        expect(name, &module_bytes, true);
        // The text encoder writes the same bytes.
        assert_eq!(
            module_bytes,
            wasm(&module(&[&invoke(&format!(
                "{} {name} drop drop",
                pushes.join(" ")
            ))])),
            "{name}"
        );
        // A padded sub-opcode is a second encoding of the same instruction.
        code.extend([0xfc, 0x80 | op.sub_opcode(), 0x00, 0x1a, 0x1a]);
        let padded = assemble(&simple_with(&code));
        expect(&format!("{name} with a padded sub-opcode"), &padded, false);
        assert_eq!(reason(&padded), ENCODING, "{name}");
    }
}
