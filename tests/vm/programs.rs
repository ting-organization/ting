//! Whole programs over several transactions (rules §21, §22): a
//! counter that persists its value, and byte processing of inputs of every
//! size, each step with its exact charges.
use crate::{
    dsl::{import, invoke, memory, module},
    harness::{deployed as case, k},
};
use vm_model::{Ledger, cost, leaf, words};

/// The counter's instructions on every path: the read, the test of its
/// length, the input, the test of the input and the return.
const ALWAYS: &str = "i32.const 0 i32.const 32 i32.const 8 call $storage_get
    i32.const 8 i32.ne if
    i32.const 64 i32.const 64 call $input local.set 0
    local.get 0 if
    i32.const 32 i32.const 8 call $return";
/// Zeroing an absent value.
const ABSENT: &str = "i32.const 32 i64.const 0 i64.store";
/// Adding the first input byte and writing the value back.
const MUTATING: &str =
    "i32.const 32 i32.const 32 i64.load i32.const 64 i32.load8_u i64.extend_i32_u i64.add i64.store
      i32.const 0 i32.const 32 i32.const 8 call $storage_set";

/// Reads its value, zeroes it when absent, adds the first input byte when
/// there is one and writes the value back, and returns it.
fn counter() -> String {
    module(&[
        &import("storage_get"),
        &import("input"),
        &import("storage_set"),
        &import("return"),
        &memory(1, 1),
        &invoke(&format!(
            "(local i32)
    i32.const 0 i32.const 32 i32.const 8 call $storage_get
    i32.const 8 i32.ne (if (then {ABSENT}))
    i32.const 64 i32.const 64 call $input local.set 0
    local.get 0 (if (then
      {MUTATING}))
    i32.const 32 i32.const 8 call $return"
        )),
    ])
}
#[test]
pub(crate) fn a_counter_persists_across_transactions() {
    counter_steps(false).run();
}
#[test]
pub(crate) fn a_counter_persists_across_blocks() {
    // Each step in a block of its own: the value is read back from the state
    // the block before committed, then overwritten.
    counter_steps(true).named("the counter across blocks").run();
}
/// Five calls of the counter, each with its exact charges, in blocks of
/// their own when `apart`.
fn counter_steps(apart: bool) -> crate::harness::Case {
    let mut c = case(&counter());
    let (mut balance, mut exists) = (0u64, false);
    for input in [&b""[..], &[2], &[3], b"", &[0]] {
        if apart {
            c = c.next_block();
        }
        let mutating = !input.is_empty();
        // The read copies the key, reads the record with its key and, when
        // present, copies the value; the input is copied; the write copies
        // the key and the value, reads the old record with its key and hashes
        // the value into its leaf; a new record grows by its key, length and
        // value.
        let ledger = Ledger {
            dispatch: cost(ALWAYS)
                + if exists { 0 } else { cost(ABSENT) }
                + if mutating { cost(MUTATING) } else { 0 },
            copies: words(32)
                + if exists { words(8) } else { 0 }
                + words(input.len() as u64)
                + if mutating { words(40) } else { 0 },
            hashing: if mutating { leaf(8) } else { 0 },
            reads: 1 + u64::from(mutating),
            accesses: 1 + if mutating { 2 } else { 0 },
            growth: if mutating && !exists { 36 + 8 } else { 0 },
            ..Default::default()
        };
        if mutating {
            balance += u64::from(input[0]);
        }
        c = c
            .call_with(input)
            .result(balance.to_le_bytes())
            .charged(ledger);
        if mutating {
            c = c.records([(k(0), balance.to_le_bytes())]);
            exists = true;
        }
    }
    c
}

/// Before the loop: the input, the two initial values, and entering the
/// block and the loop.
const BEFORE: &str = "i32.const 256 i32.const 4096 call $input local.set 0
    i64.const 0 local.set 2
    i32.const 0 local.set 1
    block loop";
/// One iteration for one byte, the branch back to the loop included.
const ITERATION: &str = "local.get 1 local.get 0 i32.ge_u br_if 1
      local.get 2 i64.const 131 i64.mul
      i32.const 256 local.get 1 i32.add i32.load8_u i64.extend_i32_u i64.add local.set 2
      local.get 1 i32.const 1 i32.add local.set 1
      br 0";
/// Leaving the loop at the end of the input.
const EXIT: &str = "local.get 1 local.get 0 i32.ge_u br_if 1";
/// Storing and returning the digest.
const AFTER: &str = "i32.const 0 local.get 2 i64.store i32.const 0 i32.const 8 call $return";

/// h = h * 131 + byte over the input, modulo 2^64.
fn digest() -> String {
    module(&[
        &import("input"),
        &import("return"),
        &memory(1, 1),
        &invoke(&format!(
            "(local i32 i32 i64)
    {BEFORE}
      {ITERATION}
    end end
    {AFTER}"
        )),
    ])
}
#[test]
pub(crate) fn byte_processing_of_inputs_of_every_size() {
    let thirty_two: Vec<u8> = (0..32).collect();
    let kilobyte: Vec<u8> = (0..=255u8).cycle().take(1024).collect();
    let inputs: [&[u8]; 5] = [b"", b"hello", &thirty_two, &[0, 0xff, 0x80], &kilobyte];
    for input in inputs {
        let expected = input
            .iter()
            .fold(0u64, |h, &b| h.wrapping_mul(131).wrapping_add(u64::from(b)));
        // The loop is paid on entry only; each byte pays one iteration.
        case(&digest())
            .named(format!("{} bytes", input.len()))
            .call_with(input)
            .result(expected.to_le_bytes())
            .charged(Ledger {
                dispatch: cost(BEFORE)
                    + input.len() as u64 * cost(ITERATION)
                    + cost(EXIT)
                    + cost(AFTER),
                copies: words(input.len() as u64),
                ..Default::default()
            })
            .run();
    }
}
