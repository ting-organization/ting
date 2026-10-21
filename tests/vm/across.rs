//! Programs and storage across blocks (seed section S7; rules §23,
//! §24, §22): a program called in later blocks reads its code and
//! manifest back from the committed state, pays its code price again in each
//! transaction and gives the same receipt every time; programs deployed in
//! different blocks call each other; a program deployed by a program is
//! called in a later block; and a record written in one block is read and
//! deleted in later ones.
use crate::{
    dsl::{escaped, import, invoke, memory, module, wasm},
    harness::{CHILD, TARGET, deployed as case, deployed_entry_of, k, warm_entry_of},
};
use vm_model::{IDENTIFIER, Ledger, MANIFEST, RECORD_BASE, SMALL, cost, installation, words};

/// Enough for every child here.
const SUBBUDGET: [u64; 4] = [1_000_000, 128, 4096, 4096];
fn budget_bytes(budget: [u64; 4]) -> Vec<u8> {
    budget.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn data(address: u32, bytes: &[u8]) -> String {
    format!("(data (i32.const {address}) {})", escaped(bytes))
}
/// What a program that returns "x" executes.
const RETURN_X: &str = "i32.const 0 i32.const 1 call $return";
fn returning() -> String {
    module(&[
        &import("return"),
        &memory(1, 1),
        &data(0, b"x"),
        &invoke(RETURN_X),
    ])
}

#[test]
pub(crate) fn the_same_call_in_later_blocks_gives_the_same_receipt() {
    // The deployment's block is committed before the first call: every call
    // reads the manifest and the code from state and pays the code price, the
    // first in its transaction. Twice in one block, then once in each of two
    // later blocks, with the same receipt every time.
    let ran = Ledger {
        dispatch: cost(RETURN_X),
        ..Default::default()
    };
    let mut c = case(&returning()).named("the same call across blocks");
    for again in [false, true, false, false] {
        if !again {
            c = c.next_block();
        }
        c = c.call_with(b"once more").result(b"x").charged(ran);
    }
    c.run();
}

#[test]
pub(crate) fn programs_deployed_in_different_blocks_call_each_other() {
    // The child, the caller and the call each in a block of its own.
    let body = "i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call drop
    i32.const 128 i32.const 1 call $call_result drop
    i32.const 128 i32.const 1 call $return";
    let caller = module(&[
        &import("call"),
        &import("call_result"),
        &import("return"),
        &memory(1, 1),
        &data(0, &CHILD),
        &data(64, &budget_bytes(SUBBUDGET)),
        &invoke(body),
    ]);
    let child = returning();
    case(&caller)
        .named("a call across blocks")
        .with_program(CHILD, &child)
        .apart()
        .call()
        .result(b"x")
        .charged(Ledger {
            dispatch: cost(body),
            copies: words(64) + words(1),
            nested: deployed_entry_of(&child) + cost(RETURN_X) + SMALL.bytes(1),
            accesses: 2,
            discarded: 1,
            ..Default::default()
        })
        .run();
}

#[test]
pub(crate) fn a_program_deployed_by_a_program_is_called_in_a_later_block() {
    // The factory deploys the child from its data; its initializer returns
    // "x", which is discarded with the identifier. In the next block the
    // child is called at the identifier the factory's transaction derived.
    let child = returning();
    let code = wasm(&child);
    let n = code.len() as u64;
    let body = format!(
        "i32.const 512 i32.const {n} i32.const 0 i32.const 0 i32.const 64 call $deploy drop"
    );
    let factory = module(&[
        &import("deploy"),
        &memory(1, 1),
        &data(64, &budget_bytes(SUBBUDGET)),
        &data(512, &code),
        &invoke(&body),
    ]);
    case(&factory)
        .named("a program deployed by a program")
        .call()
        .installs(0, &child)
        .charged(Ledger {
            dispatch: cost(&body),
            copies: words(32),
            nested: SMALL.bytes(n)
                + installation(&code)
                + warm_entry_of(&child)
                + cost(RETURN_X)
                + SMALL.bytes(1)
                + IDENTIFIER,
            accesses: 7,
            growth: RECORD_BASE + MANIFEST + RECORD_BASE + n,
            discarded: 1 + 32,
            ..Default::default()
        })
        .next_block()
        .call_deployed(0, 0, &child, b"later")
        .result(b"x")
        .charged(Ledger {
            dispatch: cost(RETURN_X),
            ..Default::default()
        })
        .run();
}

/// Every path: the length of the input, read without copying, and its test.
const TEST: &str = "i32.const 0 i32.const 0 call $input if";
/// With input: delete the record under the zero key and return nothing.
const DELETE: &str = "i32.const 0 call $storage_delete i32.const 0 i32.const 0 call $return";
/// Without: return the record's length, -1 when absent, copying nothing.
const LENGTH: &str = "i32.const 64 i32.const 0 i32.const 0 i32.const 0 call $storage_get i32.store
    i32.const 64 i32.const 4 call $return";

#[test]
pub(crate) fn a_record_written_in_one_block_is_read_and_deleted_in_later_ones() {
    // The deployment writes the record; the next block reads its length, the
    // one after deletes it, and the last finds it absent.
    let text = module(&[
        &import("input"),
        &import("storage_get"),
        &import("storage_delete"),
        &import("return"),
        &memory(1, 1),
        &invoke(&format!(
            "i32.const 0 i32.const 0 call $input
    (if (then {DELETE}))
    {LENGTH}"
        )),
    ]);
    let read = Ledger {
        dispatch: cost(TEST) + cost(LENGTH),
        copies: words(32),
        reads: 1,
        accesses: 1,
        ..Default::default()
    };
    case(&text)
        .named("a record across blocks")
        .with_storage(TARGET, [(k(0), b"kept")])
        .next_block()
        .call()
        .result(4u32.to_le_bytes())
        .charged(read)
        .next_block()
        .call_with(b"d")
        .no_records()
        .charged(Ledger {
            dispatch: cost(TEST) + cost(DELETE),
            copies: words(32),
            reads: 1,
            accesses: 2,
            ..Default::default()
        })
        .next_block()
        .call()
        .result(u32::MAX.to_le_bytes())
        .charged(read)
        .run();
}
