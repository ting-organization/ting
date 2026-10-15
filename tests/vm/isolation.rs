//! What one invocation cannot see of another (wiki: vm/execution, vm/calls,
//! vm/lifecycle): every call instantiates the program afresh, events keep
//! their order across nested calls, a child with nothing to spend fails
//! before it starts, and an initializer's revert installs nothing.
use crate::{
    dsl::{escaped, import, invoke, memory, module, wasm},
    harness::{CHILD, TARGET, case, deployment, entry_of, warm_entry_of},
};
use ting_core::protocol::Status;
use vm_model::{
    KEY, Ledger, MANIFEST, RECORD_BASE, SMALL, TRANSACTION_BASE, cost, deploy_size, installation,
    words,
};

/// A sub-budget the children here fit in with room to spare: a child's entry
/// alone, its two keys, code price and one-page preparation, is over 100,000 C.
const SUBBUDGET: [u64; 4] = [1_000_000, 128, 4096, 4096];
fn budget_bytes(budget: [u64; 4]) -> Vec<u8> {
    budget.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn data(address: u32, bytes: &[u8]) -> String {
    format!("(data (i32.const {address}) {})", escaped(bytes))
}

#[test]
fn every_call_runs_a_fresh_instance_even_within_one_transaction() {
    // The child returns the byte at address 0 and overwrites it; a second
    // call in the same transaction still finds the original.
    let child_body = "i32.const 10 i32.const 0 i32.load8_u i32.store8
    i32.const 0 i32.const 122 i32.store8
    i32.const 10 i32.const 1 call $return";
    let child = module(&[
        &import("return"),
        &memory(1, 1),
        &data(0, b"a"),
        &invoke(child_body),
    ]);
    let body = "i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call drop
    i32.const 128 i32.const 1 call $call_result drop
    i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call drop
    i32.const 129 i32.const 1 call $call_result drop
    i32.const 128 i32.const 2 call $return";
    let parent = module(&[
        &import("call"),
        &import("call_result"),
        &import("return"),
        &memory(1, 1),
        &data(0, &CHILD),
        &data(64, &budget_bytes(SUBBUDGET)),
        &invoke(body),
    ]);
    // Twice: the call's arguments, the child's entry, its instructions and
    // its one-byte return, the result copied out, and the child's result
    // discarded from the parent's receipt. Each call prepares a fresh
    // instance, but only the first pays the child's code price: the second
    // finds the code prepared in this transaction.
    let run = cost(child_body) + SMALL.bytes(1);
    case(&parent)
        .with_program(CHILD, &child)
        .call()
        .result(b"aa")
        .charged(Ledger {
            dispatch: cost(body),
            copies: 2 * (words(64) + words(1)),
            nested: entry_of(&child) + warm_entry_of(&child) + 2 * run,
            accesses: 2 * 2,
            discarded: 2,
            ..Default::default()
        })
        .run();
}

#[test]
fn events_keep_their_order_across_a_nested_call() {
    // The child's event is in the receipt, so the model charges its byte
    // from there.
    let child_body = "i32.const 0 i32.const 1 call $emit";
    let child = module(&[
        &import("emit"),
        &memory(1, 1),
        &data(0, b"b"),
        &invoke(child_body),
    ]);
    let body = "i32.const 128 i32.const 1 call $emit
    i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call drop
    i32.const 129 i32.const 1 call $emit";
    let parent = module(&[
        &import("call"),
        &import("emit"),
        &memory(1, 1),
        &data(0, &CHILD),
        &data(64, &budget_bytes(SUBBUDGET)),
        &data(128, b"ac"),
        &invoke(body),
    ]);
    case(&parent)
        .with_program(CHILD, &child)
        .call()
        .events_from(TARGET, [b"a"])
        .events_from(CHILD, [b"b"])
        .events_from(TARGET, [b"c"])
        .charged(Ledger {
            dispatch: cost(body),
            copies: words(64),
            nested: entry_of(&child) + cost(child_body),
            accesses: 2,
            ..Default::default()
        })
        .run();
}

#[test]
fn a_child_with_nothing_to_spend_fails_before_it_starts() {
    // A budget too small for anything is not excessive, so the call
    // proceeds; the child's first charge, the key and the access that read
    // its manifest, [1,900, 1, 0, 0], is refused as a whole: on compute for
    // a zero budget, on access for one with the key's compute alone. With
    // one access as well, the manifest is read and the code record's read
    // is refused, and what the child consumed stays charged.
    let body = "i32.const 128 i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call i32.store
    i32.const 128 i32.const 4 call $return";
    for (budget, status, consumed) in [
        ([0; 4], Status::Compute, [0, 0]),
        ([KEY, 0, 0, 0], Status::Access, [0, 0]),
        ([KEY, 1, 0, 0], Status::Compute, [KEY, 1]),
    ] {
        let parent = module(&[
            &import("call"),
            &import("return"),
            &memory(1, 1),
            &data(0, &CHILD),
            &data(64, &budget_bytes(budget)),
            &invoke(body),
        ]);
        case(&parent)
            .named(format!("sub-budget {budget:?}"))
            .with_program(CHILD, &module(&[&invoke("nop")]))
            .call()
            .result((status as u32).to_le_bytes())
            .charged(Ledger {
                dispatch: cost(body),
                copies: words(64),
                nested: consumed[0],
                accesses: consumed[1],
                ..Default::default()
            })
            .run();
    }
}

#[test]
fn an_initializer_that_reverts_fails_the_deployment_with_its_bytes() {
    let body = "i32.const 0 i32.const 1 call $revert";
    let text = module(&[
        &import("revert"),
        &memory(1, 1),
        &data(0, b"r"),
        &invoke(body),
    ]);
    let code = wasm(&text);
    let n = code.len() as u64;
    // The installation (the ContractId, the existence check, validation with
    // the CodeHash, both records' keys and leaves), the initializer's entry,
    // warm since the deployment prepared its code (the two keys, the memory
    // page and its data, the operand slots), its instructions and the
    // one-byte revert; the records are rolled back, their growth and the
    // reverted byte are paid, and no identifier is.
    deployment(&text)
        .deploy()
        .expect(Status::Revert)
        .result(b"r")
        .consumed([
            1 + installation(&code) + warm_entry_of(&text) + cost(body) + SMALL.bytes(1),
            7,
            TRANSACTION_BASE + deploy_size(n, 0) + 1,
            RECORD_BASE + MANIFEST + RECORD_BASE + n,
        ])
        .run();
}
