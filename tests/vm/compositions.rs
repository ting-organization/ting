//! Compositions: seeded typed expression trees over the integer instructions,
//! evaluated independently in Rust and run as programs with the exact
//! dispatch price of the evaluated path. What the tables state one
//! instruction at a time, these state for nested expressions: operand order,
//! conditional arms, traps part way through, and the pricing of all of it.
use crate::{
    dsl::{import, invoke, memory, module},
    harness::case,
};
use ting_core::protocol::Status;
use vm_model::{Ledger, cost};

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
    fn mask(self, v: u64) -> u64 {
        match self {
            Ty::I32 => v & 0xffff_ffff,
            Ty::I64 => v,
        }
    }
}

enum Expr {
    Const(Ty, u64),
    Unary(Ty, &'static str, Box<Expr>),
    Convert(Ty, &'static str, Box<Expr>),
    Binary(Ty, &'static str, Box<Expr>, Box<Expr>),
    Compare(Ty, &'static str, Box<Expr>, Box<Expr>),
    Eqz(Ty, Box<Expr>),
    Select(Ty, Box<Expr>, Box<Expr>, Box<Expr>),
    If(Ty, Box<Expr>, Box<Expr>, Box<Expr>),
}
fn result_type(e: &Expr) -> Ty {
    match e {
        Expr::Const(t, _) | Expr::Unary(t, ..) | Expr::Convert(t, ..) | Expr::Binary(t, ..) => *t,
        Expr::Compare(..) | Expr::Eqz(..) => Ty::I32,
        Expr::Select(t, ..) | Expr::If(t, ..) => *t,
    }
}
fn text(e: &Expr) -> String {
    match e {
        Expr::Const(Ty::I32, v) => format!("(i32.const {})", *v as u32 as i32),
        Expr::Const(Ty::I64, v) => format!("(i64.const {})", *v as i64),
        Expr::Unary(t, op, a) => format!("({}.{op} {})", t.name(), text(a)),
        Expr::Convert(_, op, a) => format!("({op} {})", text(a)),
        Expr::Binary(t, op, a, b) => format!("({}.{op} {} {})", t.name(), text(a), text(b)),
        Expr::Compare(t, op, a, b) => format!("({}.{op} {} {})", t.name(), text(a), text(b)),
        Expr::Eqz(t, a) => format!("({}.eqz {})", t.name(), text(a)),
        Expr::Select(_, a, b, c) => format!("(select {} {} {})", text(a), text(b), text(c)),
        Expr::If(t, c, a, b) => format!(
            "(if (result {}) {} (then {}) (else {}))",
            t.name(),
            text(c),
            text(a),
            text(b)
        ),
    }
}

/// Core semantics of a binary operator over `t`; `None` traps.
fn binary(t: Ty, op: &str, a: u64, b: u64) -> Option<u64> {
    let v = match t {
        Ty::I32 => {
            let (a, b) = (a as u32, b as u32);
            let (sa, sb) = (a as i32, b as i32);
            u64::from(match op {
                "add" => a.wrapping_add(b),
                "sub" => a.wrapping_sub(b),
                "mul" => a.wrapping_mul(b),
                "div_s" => sa.checked_div(sb)? as u32,
                "div_u" => a.checked_div(b)?,
                "rem_s" => (b != 0).then(|| sa.wrapping_rem(sb))? as u32,
                "rem_u" => a.checked_rem(b)?,
                "and" => a & b,
                "or" => a | b,
                "xor" => a ^ b,
                "shl" => a.wrapping_shl(b),
                "shr_s" => sa.wrapping_shr(b) as u32,
                "shr_u" => a.wrapping_shr(b),
                "rotl" => a.rotate_left(b & 31),
                "rotr" => a.rotate_right(b & 31),
                _ => unreachable!("{op}"),
            })
        }
        Ty::I64 => {
            let (sa, sb) = (a as i64, b as i64);
            match op {
                "add" => a.wrapping_add(b),
                "sub" => a.wrapping_sub(b),
                "mul" => a.wrapping_mul(b),
                "div_s" => sa.checked_div(sb)? as u64,
                "div_u" => a.checked_div(b)?,
                "rem_s" => (b != 0).then(|| sa.wrapping_rem(sb))? as u64,
                "rem_u" => a.checked_rem(b)?,
                "and" => a & b,
                "or" => a | b,
                "xor" => a ^ b,
                "shl" => a.wrapping_shl(b as u32),
                "shr_s" => sa.wrapping_shr(b as u32) as u64,
                "shr_u" => a.wrapping_shr(b as u32),
                "rotl" => a.rotate_left(b as u32 & 63),
                "rotr" => a.rotate_right(b as u32 & 63),
                _ => unreachable!("{op}"),
            }
        }
    };
    Some(v)
}
fn compare(t: Ty, op: &str, a: u64, b: u64) -> u64 {
    let (sa, sb, ua, ub) = match t {
        Ty::I32 => (
            i64::from(a as u32 as i32),
            i64::from(b as u32 as i32),
            u64::from(a as u32),
            u64::from(b as u32),
        ),
        Ty::I64 => (a as i64, b as i64, a, b),
    };
    u64::from(match op {
        "eq" => ua == ub,
        "ne" => ua != ub,
        "lt_s" => sa < sb,
        "lt_u" => ua < ub,
        "gt_s" => sa > sb,
        "gt_u" => ua > ub,
        "le_s" => sa <= sb,
        "le_u" => ua <= ub,
        "ge_s" => sa >= sb,
        "ge_u" => ua >= ub,
        _ => unreachable!("{op}"),
    })
}
fn unary(t: Ty, op: &str, a: u64) -> u64 {
    match (t, op) {
        (Ty::I32, "clz") => u64::from((a as u32).leading_zeros()),
        (Ty::I32, "ctz") => u64::from((a as u32).trailing_zeros()),
        (Ty::I32, "popcnt") => u64::from((a as u32).count_ones()),
        (Ty::I32, "extend8_s") => u64::from(a as u8 as i8 as i32 as u32),
        (Ty::I32, "extend16_s") => u64::from(a as u16 as i16 as i32 as u32),
        (Ty::I64, "clz") => u64::from(a.leading_zeros()),
        (Ty::I64, "ctz") => u64::from(a.trailing_zeros()),
        (Ty::I64, "popcnt") => u64::from(a.count_ones()),
        (Ty::I64, "extend8_s") => a as u8 as i8 as i64 as u64,
        (Ty::I64, "extend16_s") => a as u16 as i16 as i64 as u64,
        (Ty::I64, "extend32_s") => a as u32 as i32 as i64 as u64,
        _ => unreachable!("{op}"),
    }
}
fn convert(op: &str, a: u64) -> u64 {
    match op {
        "i32.wrap_i64" => u64::from(a as u32),
        "i64.extend_i32_s" => a as u32 as i32 as i64 as u64,
        "i64.extend_i32_u" => u64::from(a as u32),
        _ => unreachable!("{op}"),
    }
}

/// The value and the instructions executed, as flat WebAssembly text, or the
/// instructions executed up to and including the one that traps, whose price
/// is charged before it traps. Operands evaluate left to right; `if` runs one
/// arm after its condition and its own dispatch, and the `else` and `end` it
/// passes are free.
fn eval(e: &Expr) -> Result<(u64, String), String> {
    fn after(prior: String, r: Result<(u64, String), String>) -> Result<(u64, String), String> {
        match r {
            Ok((v, path)) => Ok((v, format!("{prior} {path}"))),
            Err(path) => Err(format!("{prior} {path}")),
        }
    }
    match e {
        Expr::Const(_, v) => Ok((*v, text(e))),
        Expr::Unary(t, op, a) => {
            let (va, pa) = eval(a)?;
            Ok((t.mask(unary(*t, op, va)), format!("{pa} {}.{op}", t.name())))
        }
        Expr::Convert(t, op, a) => {
            let (va, pa) = eval(a)?;
            Ok((t.mask(convert(op, va)), format!("{pa} {op}")))
        }
        Expr::Binary(t, op, a, b) => {
            let (va, pa) = eval(a)?;
            let (vb, pb) = after(pa, eval(b))?;
            let path = format!("{pb} {}.{op}", t.name());
            match binary(*t, op, va, vb) {
                Some(v) => Ok((t.mask(v), path)),
                None => Err(path),
            }
        }
        Expr::Compare(t, op, a, b) => {
            let (va, pa) = eval(a)?;
            let (vb, pb) = after(pa, eval(b))?;
            Ok((compare(*t, op, va, vb), format!("{pb} {}.{op}", t.name())))
        }
        Expr::Eqz(t, a) => {
            let (va, pa) = eval(a)?;
            Ok((u64::from(t.mask(va) == 0), format!("{pa} {}.eqz", t.name())))
        }
        Expr::Select(_, a, b, c) => {
            let (va, pa) = eval(a)?;
            let (vb, pb) = after(pa, eval(b))?;
            let (vc, pc) = after(pb, eval(c))?;
            Ok((if vc as u32 != 0 { va } else { vb }, format!("{pc} select")))
        }
        Expr::If(_, c, a, b) => {
            let (vc, pc) = eval(c)?;
            let arm = if vc as u32 != 0 { a } else { b };
            after(format!("{pc} if"), eval(arm))
        }
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len() as u64) as usize]
    }
}
const BINARY: [&str; 15] = [
    "add", "sub", "mul", "div_s", "div_u", "rem_s", "rem_u", "and", "or", "xor", "shl", "shr_s",
    "shr_u", "rotl", "rotr",
];
const COMPARE: [&str; 10] = [
    "eq", "ne", "lt_s", "lt_u", "gt_s", "gt_u", "le_s", "le_u", "ge_s", "ge_u",
];
fn constant(rng: &mut Rng, t: Ty) -> Expr {
    let edges32: [u32; 8] = [0, 1, 2, 7, 31, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff];
    let edges64: [u64; 8] = [0, 1, 2, 63, 64, i64::MAX as u64, 1 << 63, u64::MAX];
    let v = match (t, rng.below(2)) {
        (Ty::I32, 0) => u64::from(*rng.pick(&edges32)),
        (Ty::I32, _) => u64::from(rng.next() as u32),
        (Ty::I64, 0) => *rng.pick(&edges64),
        (Ty::I64, _) => rng.next(),
    };
    Expr::Const(t, v)
}
fn generate(rng: &mut Rng, t: Ty, depth: u32) -> Expr {
    if depth >= 4 {
        return constant(rng, t);
    }
    let next = depth + 1;
    match rng.below(100) {
        0..=19 => constant(rng, t),
        20..=49 => Expr::Binary(
            t,
            rng.pick(&BINARY),
            Box::new(generate(rng, t, next)),
            Box::new(generate(rng, t, next)),
        ),
        50..=59 => {
            let ops: &[&str] = match t {
                Ty::I32 => &["clz", "ctz", "popcnt", "extend8_s", "extend16_s"],
                Ty::I64 => &[
                    "clz",
                    "ctz",
                    "popcnt",
                    "extend8_s",
                    "extend16_s",
                    "extend32_s",
                ],
            };
            Expr::Unary(t, rng.pick(ops), Box::new(generate(rng, t, next)))
        }
        60..=67 => {
            let (op, from) = match t {
                Ty::I32 => ("i32.wrap_i64", Ty::I64),
                Ty::I64 => (
                    *rng.pick(&["i64.extend_i32_s", "i64.extend_i32_u"]),
                    Ty::I32,
                ),
            };
            Expr::Convert(t, op, Box::new(generate(rng, from, next)))
        }
        68..=77 => {
            // A comparison or a test yields an i32; widened when an i64 is wanted.
            let w = *rng.pick(&[Ty::I32, Ty::I64]);
            let cmp = if rng.below(5) == 0 {
                Expr::Eqz(w, Box::new(generate(rng, w, next)))
            } else {
                Expr::Compare(
                    w,
                    rng.pick(&COMPARE),
                    Box::new(generate(rng, w, next)),
                    Box::new(generate(rng, w, next)),
                )
            };
            match t {
                Ty::I32 => cmp,
                Ty::I64 => Expr::Convert(Ty::I64, "i64.extend_i32_u", Box::new(cmp)),
            }
        }
        78..=87 => Expr::Select(
            t,
            Box::new(generate(rng, t, next)),
            Box::new(generate(rng, t, next)),
            Box::new(generate(rng, Ty::I32, next)),
        ),
        _ => Expr::If(
            t,
            Box::new(generate(rng, Ty::I32, next)),
            Box::new(generate(rng, t, next)),
            Box::new(generate(rng, t, next)),
        ),
    }
}

#[test]
fn seeded_expressions_evaluate_as_rust_does_with_exact_dispatch() {
    let (mut traps, mut conditionals) = (0, 0);
    for seed in 1..=128u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let t = *rng.pick(&[Ty::I32, Ty::I64]);
        let e = generate(&mut rng, t, 0);
        let widen = if result_type(&e) == Ty::I32 {
            " i64.extend_i32_u"
        } else {
            ""
        };
        conditionals += usize::from(text(&e).contains("(if "));
        let program = module(&[
            &import("return"),
            &memory(1, 1),
            &invoke(&format!(
                "i32.const 0 {}{widen} i64.store i32.const 0 i32.const 8 call $return",
                text(&e)
            )),
        ]);
        let c = case(&program).named(format!("seed {seed}")).call();
        match eval(&e) {
            Ok((value, path)) => c
                .result(value.to_le_bytes())
                .charged(Ledger {
                    dispatch: cost(&format!(
                        "i32.const 0 {path}{widen} i64.store i32.const 0 i32.const 8 call $return"
                    )),
                    ..Default::default()
                })
                .run(),
            Err(path) => {
                traps += 1;
                c.expect(Status::Trap)
                    .charged(Ledger {
                        dispatch: cost(&format!("i32.const 0 {path}")),
                        ..Default::default()
                    })
                    .run();
            }
        }
    }
    assert!(
        traps > 0 && conditionals > 0,
        "{traps} traps, {conditionals} conditionals"
    );
}
