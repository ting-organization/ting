//! Budget boundaries (wiki: resources/budgets, vm/metering, vm/lifecycle):
//! exactly enough succeeds, one unit less fails in that dimension with the
//! atomic debit that did not fit withheld, an instruction whose price does not
//! fit neither executes nor is charged, and the two record lookups of the
//! entry come before any code is loaded.
use crate::{
    dsl::{import, invoke, memory, module, wasm},
    harness::case,
    shape::shape,
};
use ting_core::protocol::Status;
use vm_model::{
    BUDGET, KEY, Ledger, PAGE, SMALL, TRANSACTION_BASE, Tier, call_size, cost, cost_at, leaf, words,
};

/// Adds two constants and returns the eight-byte result.
const ADDING: &str = "i32.const 0 i32.const -1 i32.const 1 i32.add i64.extend_i32_u i64.store i32.const 0 i32.const 8 call $return";
fn adding() -> String {
    module(&[&import("return"), &memory(1, 1), &invoke(ADDING)])
}
fn ledger() -> Ledger {
    dispatch(cost(ADDING))
}
fn dispatch(n: u64) -> Ledger {
    Ledger {
        dispatch: n,
        ..Default::default()
    }
}
fn with(dimension: usize, amount: u64) -> [u64; 4] {
    let mut budget = BUDGET;
    budget[dimension] = amount;
    budget
}
/// The compute a call consumes before the program's first instruction: the
/// root dispatch and the entry (the two lookups, the code price, the
/// preparation and the reservation of `invoke`).
fn before_first_instruction(text: &str) -> u64 {
    1 + shape(&wasm(text)).entry().compute()
}

#[test]
fn compute_runs_out_at_the_final_return_debit() {
    let text = adding();
    let needed = before_first_instruction(&text) + cost(ADDING) + SMALL.bytes(8);
    for extra in [0, 1] {
        case(&text)
            .named(format!("{extra} to spare"))
            .call()
            .budget(with(0, needed + extra))
            .result(0u64.to_le_bytes())
            .charged(ledger())
            .run();
    }
    // One unit short: the return's bytes are refused as a whole, so nothing of
    // the result is produced or charged while everything before it stands.
    case(&text)
        .named("one short")
        .call()
        .budget(with(0, needed - 1))
        .expect(Status::Compute)
        .charged(ledger())
        .run();
}

/// A straight run of instructions of every price class: loads at the smallest
/// tier, a multiplication, a division, a block and an `if` with their free
/// `end`s, a taken-not `br_if`, wide arithmetic, a comparison, a `select`, a
/// rotation, a store and the host call.
const STEPS: &[&str] = &[
    "i32.const 0",
    "i32.const 64",
    "i32.load",
    "i32.const 3",
    "i32.mul",
    "i32.const 4",
    "i32.div_u",
    "block",
    "i32.const 0",
    "br_if 0",
    "end",
    "i32.const 1",
    "if",
    "nop",
    "end",
    "i64.extend_i32_u",
    "i64.const -9",
    "i64.mul_wide_u",
    "i64.const 1",
    "i64.const 0",
    "i64.add128",
    "i64.xor",
    "i64.const 7",
    "i32.const 5",
    "i32.const 5",
    "i32.eq",
    "select",
    "i64.const 1",
    "i64.rotl",
    "i64.store",
    "i32.const 0",
    "i32.const 8",
    "call $return",
];
/// The program of [`STEPS`], with the 7 it loads placed at 64.
fn straight() -> String {
    module(&[
        &import("return"),
        &memory(1, 1),
        "(data (i32.const 64) \"\\07\")",
        &invoke(&STEPS.join(" ")),
    ])
}
/// What [`straight`] returns, in Rust's arithmetic.
fn straight_value() -> u64 {
    let quotient = 7u64 * 3 / 4;
    let product = u128::from(quotient) * u128::from(-9i64 as u64);
    let sum = product.wrapping_add(1);
    let (lo, hi) = (sum as u64, (sum >> 64) as u64);
    (lo ^ hi).rotate_left(1)
}

#[test]
fn compute_runs_out_exactly_at_the_instruction_whose_price_does_not_fit() {
    let text = straight();
    let entry = before_first_instruction(&text);
    for (k, step) in STEPS.iter().enumerate() {
        let price = cost(step);
        // `end` is free: it always fits, even with nothing left, which the
        // instruction after each `end` shows.
        if price == 0 {
            continue;
        }
        // One C short of this instruction's price: it does not execute and
        // nothing of its price is charged; everything before it stands.
        let before = cost(&STEPS[..k].join(" "));
        case(&text)
            .named(format!("step {k}: {} C left before {step}", price - 1))
            .call()
            .budget(with(0, entry + before + price - 1))
            .expect(Status::Compute)
            .charged(dispatch(before))
            .run();
    }
    let all = cost(&STEPS.join(" "));
    case(&text)
        .named("one short of the result")
        .call()
        .budget(with(0, entry + all + SMALL.bytes(8) - 1))
        .expect(Status::Compute)
        .charged(dispatch(all))
        .run();
    case(&text)
        .named("exactly enough")
        .call()
        .budget(with(0, entry + all + SMALL.bytes(8)))
        .result(straight_value().to_le_bytes())
        .charged(dispatch(all))
        .run();
}

#[test]
fn seventeen_left_before_a_load_priced_eighteen() {
    // The price list: a load in a memory of at most 64 KiB costs 18, a
    // constant 1.
    assert_eq!((cost("i32.load"), cost("i32.const 0")), (18, 1));
    let text = straight();
    let entry = before_first_instruction(&text);
    // The two constants leave 17 C, one short of the load: status 3, with the
    // constants charged and nothing of the load.
    case(&text)
        .named("17 left")
        .call()
        .budget(with(0, entry + 2 + 17))
        .expect(Status::Compute)
        .charged(dispatch(2))
        .run();
    // With 18 the load executes, and the constant after it finds nothing left.
    case(&text)
        .named("18 left")
        .call()
        .budget(with(0, entry + 2 + 18))
        .expect(Status::Compute)
        .charged(dispatch(2 + 18))
        .run();
}

#[test]
fn a_loop_runs_out_part_way_through_an_iteration() {
    // The loop is paid once on entry; each iteration pays its constant and its
    // br_if, and the branch back does not pay for the loop again.
    let text = module(&[&invoke("(loop i32.const 1 br_if 0)")]);
    let entry = before_first_instruction(&text);
    let iteration = cost("i32.const 1 br_if 0");
    for n in [0, 1, 2, 1000] {
        // n iterations and the next constant, then 17 C for a br_if of 18.
        let spent = cost("loop") + n * iteration + cost("i32.const 1");
        case(&text)
            .named(format!("{n} iterations"))
            .call()
            .budget(with(0, entry + spent + cost("br_if 0") - 1))
            .expect(Status::Compute)
            .charged(dispatch(spent))
            .run();
    }
}

#[test]
fn a_reservation_that_does_not_fit_is_refused_whole() {
    let text = module(&[
        "(func $f (param i64) (local i64) i64.const 1 drop)",
        &invoke("i64.const 0 call $f"),
    ]);
    let entry = before_first_instruction(&text);
    let calling = cost("i64.const 0 call $f");
    let body = cost("i64.const 1 drop");
    // $f reserves its parameter, its local and its operand height of one.
    let slots = 1 + 1 + 1;
    // Short of the reservation: the call's price is charged, the reservation
    // is not.
    case(&text)
        .named("short of the reservation")
        .call()
        .budget(with(0, entry + calling + slots - 1))
        .expect(Status::Compute)
        .charged(dispatch(calling))
        .run();
    // The reservation fits; the callee's first instruction does not.
    case(&text)
        .named("short of the callee's first instruction")
        .call()
        .budget(with(0, entry + calling + slots))
        .expect(Status::Compute)
        .charged(Ledger {
            dispatch: calling,
            reservations: slots,
            ..Default::default()
        })
        .run();
    case(&text)
        .named("exactly enough")
        .call()
        .budget(with(0, entry + calling + slots + body))
        .charged(Ledger {
            dispatch: calling + body,
            reservations: slots,
            ..Default::default()
        })
        .run();
}

#[test]
fn the_entry_debits_come_in_order_and_are_each_refused_whole() {
    let text = adding();
    let entry = shape(&wasm(&text)).entry();
    let envelope = TRANSACTION_BASE + call_size(0);
    // The root dispatch, the manifest read and the code read, each with its
    // key, then the code price, the preparation and the reservation of
    // invoke. One C short of each: the counters stand where the previous
    // debit left them.
    let manifest = 1;
    let code = manifest + KEY;
    let code_price = code + KEY;
    let preparation = code_price + entry.code_price;
    let reservation = preparation + entry.preparation;
    for (what, before, accesses, debit) in [
        ("manifest read", manifest, 0, KEY),
        ("code read", code, 1, KEY),
        ("code price", code_price, 2, entry.code_price),
        ("preparation", preparation, 2, entry.preparation),
        ("reservation", reservation, 2, entry.slots),
    ] {
        case(&text)
            .named(what)
            .call()
            .budget(with(0, before + debit - 1))
            .expect(Status::Compute)
            .consumed([before, accesses, envelope, 0])
            .run();
    }
    // Everything of the entry fits; the first instruction does not.
    case(&text)
        .named("first instruction")
        .call()
        .budget(with(0, reservation + entry.slots))
        .expect(Status::Compute)
        .charged(Ledger::default())
        .run();
}

#[test]
fn the_entry_reads_the_manifest_and_the_code_before_loading_anything() {
    let text = adding();
    for extra in [0, 1] {
        case(&text)
            .named(format!("{extra} to spare"))
            .call()
            .budget(with(1, 2 + extra))
            .result(0u64.to_le_bytes())
            .charged(ledger())
            .run();
    }
    // With one access the manifest is read and the code is not: the root
    // dispatch, the manifest's key, one access and the envelope are all that
    // is consumed.
    case(&text)
        .named("one access")
        .call()
        .budget(with(1, 1))
        .expect(Status::Access)
        .consumed([1 + KEY, 1, TRANSACTION_BASE + call_size(0), 0])
        .run();
    // With none, not even the manifest: its debit is refused whole, key
    // included.
    case(&text)
        .named("no access")
        .call()
        .budget(with(1, 0))
        .expect(Status::Access)
        .consumed([1, 0, TRANSACTION_BASE + call_size(0), 0])
        .run();
}
#[test]
fn data_runs_out_at_the_result_bytes() {
    let text = adding();
    let needed = TRANSACTION_BASE + call_size(0) + 8;
    for extra in [0, 1] {
        case(&text)
            .named(format!("{extra} to spare"))
            .call()
            .budget(with(2, needed + extra))
            .result(0u64.to_le_bytes())
            .charged(ledger())
            .run();
    }
    // The result's debit is one atomic C-and-D debit: refused, it leaves both
    // counters where they were.
    case(&text)
        .named("one short")
        .call()
        .budget(with(2, needed - 1))
        .expect(Status::Data)
        .charged(ledger())
        .run();
}

#[test]
fn a_transaction_whose_data_budget_cannot_hold_its_envelope_is_no_transaction() {
    use crate::harness::context;
    use ting_core::{
        meter::Resources,
        protocol::{Encode, Payload, Transaction},
        state::State,
        vm::Runtime,
    };
    let code = wasm(&module(&[&invoke("unreachable")]));
    let runtime = Runtime::new(State::default(), context());
    let mut tx = Transaction {
        budget: Resources([8192, 128, 1, 8192]),
        payload: Payload::Deploy {
            code: code.clone(),
            input: Vec::new(),
        },
    };
    // Rejected before anything runs: no receipt, nothing consumed.
    assert!(runtime.execute(&tx).is_err());
    assert_eq!(runtime.last_resources(), Resources::ZERO);
    // C, S and G have no ceiling. D has one, 2^32 - 1, so that the
    // transaction and its receipt each fit their u32 frames in the body.
    tx.budget = Resources([u64::MAX, u64::MAX, u32::MAX.into(), u64::MAX]);
    let receipt = runtime.execute(&tx).unwrap();
    assert_eq!(receipt.status, Status::Trap);
    assert_eq!(runtime.state_root(), State::default().root());
    tx.budget.0[2] += 1;
    assert!(runtime.execute(&tx).is_err());
    assert!(Transaction::decode(&tx.encode(), usize::MAX).is_err());
    assert_eq!(runtime.state_root(), State::default().root());
}
#[test]
fn a_result_debit_is_one_atomic_debit_of_compute_and_data() {
    const BODY: &str = "i32.const 0 i32.const 8 call $return";
    let text = module(&[
        &import("return"),
        &memory(1, 1),
        "(data (i32.const 0) \"12345678\")",
        &invoke(BODY),
    ]);
    let base = before_first_instruction(&text) + cost(BODY);
    let bytes = SMALL.bytes(8);
    let envelope = TRANSACTION_BASE + call_size(0);
    // Compute is checked before data, and neither counter moves when the
    // other falls short.
    for (c, d, status) in [
        (base + bytes - 1, envelope + 7, Status::Compute),
        (base + bytes - 1, envelope + 8, Status::Compute),
        (base + bytes, envelope + 7, Status::Data),
        (base + bytes, envelope + 8, Status::Success),
    ] {
        let mut k = case(&text)
            .named(format!("{status:?} with {c} C and {d} D"))
            .call()
            .budget([c, BUDGET[1], d, BUDGET[3]])
            .expect(status);
        if status == Status::Success {
            k = k.result(b"12345678");
        }
        k.charged(dispatch(cost(BODY))).run();
    }
}
#[test]
fn a_page_that_does_not_fit_fails_before_it_is_added() {
    const GROW: &str = "i32.const 0 i32.const 1 memory.grow";
    const AFTER: &str =
        "i32.store i32.const 4 memory.size i32.store i32.const 0 i32.const 8 call $return";
    let text = module(&[
        &import("return"),
        &memory(1, 2),
        &invoke(&format!("{GROW} {AFTER}")),
    ]);
    let before = before_first_instruction(&text) + cost(GROW);
    // The page's 40,000 C come after memory.grow's own price; one short, the
    // invocation fails with status 3 rather than memory.grow returning -1.
    case(&text)
        .named("one short of the page")
        .call()
        .budget(with(0, before + PAGE - 1))
        .expect(Status::Compute)
        .charged(dispatch(cost(GROW)))
        .run();
    // Exactly enough: everything after the growth runs at the tier of two
    // pages, the copy of the result included.
    let grown = Tier::pages(2);
    let needed = before + PAGE + cost_at(AFTER, grown) + grown.bytes(8);
    let mut result = 1u32.to_le_bytes().to_vec();
    result.extend(2u32.to_le_bytes());
    case(&text)
        .named("exactly enough")
        .call()
        .budget(with(0, needed))
        .result(result)
        .charged(Ledger {
            dispatch: cost(GROW) + cost_at(AFTER, grown),
            pages: 1,
            extra: grown.bytes(8) - SMALL.bytes(8),
            ..Default::default()
        })
        .run();
    case(&text)
        .named("one short of the result")
        .call()
        .budget(with(0, needed - 1))
        .expect(Status::Compute)
        .charged(Ledger {
            dispatch: cost(GROW) + cost_at(AFTER, grown),
            pages: 1,
            ..Default::default()
        })
        .run();
}

/// Reads the input's length without copying it, then writes one byte under
/// key zero, emits it and reverts.
const READ: &str = "i32.const 128 i32.const 0 call $input";
const WRITE: &str = "i32.const 0 i32.const 32 i32.const 1 call $storage_set";
const EMIT: &str = "i32.const 32 i32.const 1 call $emit";
const REVERT: &str = "i32.const 0 i32.const 0 call $revert";

#[test]
fn a_reverted_write_keeps_its_growth_accesses_and_data() {
    let text = module(&[
        &import("input"),
        &import("storage_set"),
        &import("emit"),
        &import("revert"),
        &memory(1, 1),
        "(data (i32.const 32) \"\\01\")",
        &invoke(&format!(
            "{READ}\n    (if (then\n      {WRITE}\n      {EMIT}\n      {REVERT}))"
        )),
    ]);
    // The input of one byte does not fit the empty buffer: no copy. The
    // write copies its key and value, reads the old record with its key and
    // hashes the value into its leaf; the event copies its byte.
    let to_write = cost(READ) + cost("if") + cost(WRITE);
    let to_emit = to_write + cost(EMIT);
    // The write and the event are rolled back; their growth, accesses and
    // data were consumed and stay consumed.
    case(&text)
        .named("reverted")
        .call_with(b"x")
        .expect(Status::Revert)
        .charged(Ledger {
            dispatch: to_emit + cost(REVERT),
            copies: words(33) + words(1),
            hashing: leaf(1),
            reads: 1,
            accesses: 2,
            growth: 36 + 1,
            discarded: 36 + 1,
            ..Default::default()
        })
        .run();
    // Short of an access or of growth, the write's mutation is refused whole
    // after its old-record read.
    for (s, g, status) in [
        (3, BUDGET[3], Status::Access),
        (BUDGET[1], 36, Status::Growth),
    ] {
        case(&text)
            .named(format!("{status:?}"))
            .call_with(b"x")
            .budget([BUDGET[0], s, BUDGET[2], g])
            .expect(status)
            .charged(Ledger {
                dispatch: to_write,
                copies: words(33),
                reads: 1,
                accesses: 1,
                ..Default::default()
            })
            .run();
    }
    // Data for the envelope but not for the event: the write stood, the
    // emit's debit of its copy and its data is refused whole.
    case(&text)
        .named("Data")
        .call_with(b"x")
        .budget([
            BUDGET[0],
            BUDGET[1],
            TRANSACTION_BASE + call_size(1) + 8,
            BUDGET[3],
        ])
        .expect(Status::Data)
        .charged(Ledger {
            dispatch: to_emit,
            copies: words(33),
            hashing: leaf(1),
            reads: 1,
            accesses: 2,
            growth: 36 + 1,
            ..Default::default()
        })
        .run();
}
