//! Calls between programs and deployments from inside a program (wiki:
//! vm/calls, vm/lifecycle): a child's outcome, its retained charges and its
//! rolled-back effects; the last-result buffer; read-only mode, explicit and
//! inherited; the ordinal of internal deployments; code prepared once per
//! transaction, by a call or by a deployment, whatever becomes of either; and
//! depth bounded by budgets alone, whether as self-recursion or as a chain of
//! four hundred programs.
use crate::{
    dsl::{escaped, import, invoke, memory, module, wasm},
    harness::{
        CHILD, GRANDCHILD, TARGET, call_transaction, case, entry_of, identifier, warm_entry_of,
    },
};
use ting_core::{hash::Hash, protocol::Status};
use vm_model::{
    BUDGET, CONTRACT_ID, IDENTIFIER, KEY, Ledger, MANIFEST, PAGE, RECORD_BASE, SMALL, Tier,
    code_hash, code_price, cost, cost_at, installation, leaf, words,
};

/// A budget as the 32 bytes a program passes to `call` or `deploy`.
fn budget_bytes(budget: [u64; 4]) -> Vec<u8> {
    budget.iter().flat_map(|v| v.to_le_bytes()).collect()
}
/// Enough for every child here; entering one with a page of memory costs a
/// little over 100,000 C.
const SUBBUDGET: [u64; 4] = [1_000_000, 128, 4096, 4096];
/// More compute than the whole transaction has: refused as call error 128.
const EXCESS: [u64; 4] = [BUDGET[0] + 1, 1, 1, 1];
fn data(address: u32, bytes: &[u8]) -> String {
    format!("(data (i32.const {address}) {})", escaped(bytes))
}
fn dispatch(n: u64) -> Ledger {
    Ledger {
        dispatch: n,
        ..Default::default()
    }
}
/// The statuses of two calls as a program stores them side by side.
fn statuses(first: Status, second: Status) -> Vec<u8> {
    let mut bytes = (first as u32).to_le_bytes().to_vec();
    bytes.extend((second as u32).to_le_bytes());
    bytes
}

/// What a child that writes "x", emits it and ends with `ending` executes.
fn writing_body(ending: &str) -> String {
    format!(
        "i32.const 0 i32.const 32 i32.const 1 call $storage_set
    i32.const 32 i32.const 1 call $emit
    i32.const 32 i32.const 1 call ${ending}"
    )
}
fn writing_child(ending: &str) -> String {
    module(&[
        &import("storage_set"),
        &import("emit"),
        &import(ending),
        &memory(1, 1),
        &data(32, b"x"),
        &invoke(&writing_body(ending)),
    ])
}
/// Calls `CHILD` under `SUBBUDGET`, stores the status, copies the child's
/// result after it and ends with `last`: nine result bytes.
fn calling_body(last: &str) -> String {
    format!(
        "i32.const 128 i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call i32.store
    i32.const 132 i32.const 136 i32.const 1 call $call_result i32.store
    i32.const 128 i32.const 9 call ${last}"
    )
}
fn calling_parent(last: &str) -> String {
    module(&[
        &import("call"),
        &import("call_result"),
        &import(last),
        &memory(1, 1),
        &data(0, &CHILD),
        &data(64, &budget_bytes(SUBBUDGET)),
        &invoke(&calling_body(last)),
    ])
}

#[test]
fn a_childs_charges_stay_whatever_becomes_of_its_effects() {
    for (outcome, child_ending, parent_ending) in [
        ("child returns", "return", "return"),
        ("child reverts", "revert", "return"),
        ("parent reverts", "return", "revert"),
    ] {
        let child = writing_child(child_ending);
        let kept = outcome == "child returns";
        let mut result = u32::from(child_ending == "revert").to_le_bytes().to_vec();
        result.extend(1u32.to_le_bytes());
        result.push(b'x');
        // The child's entry and instructions, its write (the copy of key and
        // value, the record's key and the leaf of its one byte) and its ending
        // copy; its emit's word is counted from the event when the receipt
        // shows it, in `nested` when it was rolled back. Its growth and
        // accesses stay in every outcome, as does the event's data.
        let emitted = if kept { 0 } else { SMALL.bytes(1) };
        let mut c = case(&calling_parent(parent_ending))
            .named(outcome)
            .with_program(CHILD, &child)
            .call()
            .result(&result)
            .charged(Ledger {
                dispatch: cost(&calling_body(parent_ending)),
                copies: words(64) + words(1),
                nested: entry_of(&child)
                    + cost(&writing_body(child_ending))
                    + SMALL.bytes(32 + 1)
                    + KEY
                    + leaf(1)
                    + emitted
                    + SMALL.bytes(1),
                accesses: 2 + 2,
                growth: 37,
                discarded: 1 + if kept { 0 } else { 36 + 1 },
                ..Default::default()
            });
        c = match outcome {
            "child returns" => c
                .events_from(CHILD, [b"x"])
                .records_of(CHILD, [([0; 32], b"x")]),
            "child reverts" => c,
            _ => c.expect(Status::Revert),
        };
        c.run();
    }
}

#[test]
fn call_checks_its_flags_before_copying_and_refuses_an_excessive_budget() {
    for (flags, budget, name) in [
        (0, SUBBUDGET, "an absent program"),
        (0, EXCESS, "an excessive budget"),
        (2, SUBBUDGET, "flags 2"),
        (-1, SUBBUDGET, "flags -1"),
    ] {
        let call = format!(
            "i32.const 128 i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const {flags} call $call"
        );
        let body = format!("{call} i32.store\n    i32.const 128 i32.const 4 call $return");
        let text = module(&[
            &import("call"),
            &import("return"),
            &memory(1, 1),
            &data(0, &CHILD),
            &data(64, &budget_bytes(budget)),
            &invoke(&body),
        ]);
        let c = case(&text).named(name).call();
        match (flags, budget == EXCESS) {
            // Nothing is installed as CHILD: the child's invocation reads its
            // manifest, with the record's key, and reports the missing status.
            (0, false) => c
                .result((Status::Missing as u32).to_le_bytes())
                .charged(Ledger {
                    dispatch: cost(&body),
                    copies: words(64),
                    nested: KEY,
                    accesses: 1,
                    ..Default::default()
                })
                .run(),
            // A sub-budget beyond the caller's is error 128: no child, no access.
            (0, true) => c
                .result(128u32.to_le_bytes())
                .charged(Ledger {
                    dispatch: cost(&body),
                    copies: words(64),
                    ..Default::default()
                })
                .run(),
            // An invalid flag traps at the call, after its dispatch and
            // before any copy.
            _ => c.expect(Status::Trap).charged(dispatch(cost(&call))).run(),
        }
    }
}

#[test]
fn read_only_is_refused_before_the_spans_of_a_mutation_are_checked() {
    let body = "i32.const 128 i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 1 call $call i32.store
    i32.const 128 i32.const 4 call $return";
    for (operation, arguments) in [
        ("storage_set", 3),
        ("storage_delete", 1),
        ("emit", 2),
        ("deploy", 5),
    ] {
        let attempt = format!("{}call ${operation}", "i32.const -1 ".repeat(arguments));
        let drop = if operation == "deploy" { " drop" } else { "" };
        let child = module(&[&import(operation), &invoke(&format!("{attempt}{drop}"))]);
        let text = module(&[
            &import("call"),
            &import("return"),
            &memory(1, 1),
            &data(0, &CHILD),
            &data(64, &budget_bytes(SUBBUDGET)),
            &invoke(body),
        ]);
        // The child pays its entry and its instructions up to the refused
        // import, and nothing for the spans.
        case(&text)
            .named(operation)
            .with_program(CHILD, &child)
            .call()
            .result((Status::ReadOnly as u32).to_le_bytes())
            .charged(Ledger {
                dispatch: cost(body),
                copies: words(64),
                nested: entry_of(&child) + cost(&attempt),
                accesses: 2,
                ..Default::default()
            })
            .run();
    }
}

#[test]
fn the_caller_identity_comes_from_the_vm() {
    let child_body = "i32.const 3 i32.const 0 i32.const 33 call $context drop i32.const 0 i32.const 33 call $return";
    let child = module(&[
        &import("context"),
        &import("return"),
        &memory(1, 1),
        &invoke(child_body),
    ]);
    let body = "i32.const 128 i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call i32.store
    i32.const 132 i32.const 136 i32.const 33 call $call_result i32.store
    i32.const 128 i32.const 41 call $return";
    let text = module(&[
        &import("call"),
        &import("call_result"),
        &import("return"),
        &memory(1, 1),
        &data(0, &CHILD),
        &data(64, &budget_bytes(SUBBUDGET)),
        &invoke(body),
    ]);
    let mut result = 0u32.to_le_bytes().to_vec();
    result.extend(33u32.to_le_bytes());
    result.push(1);
    result.extend(TARGET);
    case(&text)
        .with_program(CHILD, &child)
        .call()
        .result(result)
        .charged(Ledger {
            dispatch: cost(body),
            copies: words(64) + words(33),
            nested: entry_of(&child) + cost(child_body) + SMALL.bytes(33) + SMALL.bytes(33),
            accesses: 2,
            discarded: 33,
            ..Default::default()
        })
        .run();
}

#[test]
fn read_only_is_inherited_through_an_ordinary_call() {
    // The parent calls CHILD read-only; CHILD calls GRANDCHILD with flags 0;
    // GRANDCHILD's write is still refused with status 11, which travels back.
    let grandchild_body = "i32.const -1 i32.const -1 i32.const -1 call $storage_set";
    let grandchild = module(&[&import("storage_set"), &invoke(grandchild_body)]);
    let child_body = "i32.const 128 i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call i32.store
    i32.const 128 i32.const 4 call $return";
    let child = module(&[
        &import("call"),
        &import("return"),
        &memory(1, 1),
        &data(0, &GRANDCHILD),
        &data(64, &budget_bytes([SUBBUDGET[0], 64, 4096, 4096])),
        &invoke(child_body),
    ]);
    let body = "i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 1 call $call drop
    i32.const 128 i32.const 4 call $call_result drop
    i32.const 128 i32.const 4 call $return";
    let text = module(&[
        &import("call"),
        &import("call_result"),
        &import("return"),
        &memory(1, 1),
        &data(0, &CHILD),
        &data(64, &budget_bytes([2 * SUBBUDGET[0], 128, 8192, 8192])),
        &invoke(body),
    ]);
    case(&text)
        .with_program(CHILD, &child)
        .with_program(GRANDCHILD, &grandchild)
        .call()
        .result((Status::ReadOnly as u32).to_le_bytes())
        .charged(Ledger {
            dispatch: cost(body),
            copies: words(64) + words(4),
            nested: entry_of(&child)
                + cost(child_body)
                + SMALL.bytes(64)
                + SMALL.bytes(4)
                + entry_of(&grandchild)
                + cost(grandchild_body),
            accesses: 4,
            discarded: 4,
            ..Default::default()
        })
        .run();
}

/// What a child that returns "x" executes.
const RETURN_X: &str = "i32.const 0 i32.const 1 call $return";
fn returning_child() -> String {
    module(&[
        &import("return"),
        &memory(1, 1),
        &data(0, b"x"),
        &invoke(RETURN_X),
    ])
}

#[test]
fn the_last_result_is_cleared_by_a_refused_call() {
    let child = returning_child();
    // A successful call leaves "x"; a call refused for its budget clears it;
    // the buffer then has length zero and copies nothing into the sentinel.
    let body = "i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call drop
    i32.const 32 i32.const 0 i32.const 0 i32.const 96 i32.const 0 call $call drop
    i32.const 128 i32.const 132 i32.const 1 call $call_result i32.store
    i32.const 128 i32.const 5 call $return";
    let text = module(&[
        &import("call"),
        &import("call_result"),
        &import("return"),
        &memory(1, 1),
        &data(0, &CHILD),
        &data(32, &GRANDCHILD),
        &data(64, &budget_bytes(SUBBUDGET)),
        &data(96, &budget_bytes(EXCESS)),
        &invoke(body),
    ]);
    case(&text)
        .with_program(CHILD, &child)
        .call()
        .result([0; 5])
        .charged(Ledger {
            dispatch: cost(body),
            copies: words(64) + words(64) + words(0),
            nested: entry_of(&child) + cost(RETURN_X) + SMALL.bytes(1),
            accesses: 2,
            discarded: 1,
            ..Default::default()
        })
        .run();
}

#[test]
fn internal_deployments_advance_the_ordinal_only_when_begun() {
    let child = module(&[&invoke("nop")]);
    let code = wasm(&child);
    let n = code.len() as u64;
    for (first_fails_on_budget, outer_revert) in [(false, false), (false, true), (true, false)] {
        // The first attempt either names an excessive budget, which is refused
        // before an ordinal is taken, or deploys one invalid byte, which
        // takes ordinal 0 and fails validation. The second deploys the child.
        let (first, first_data, ordinal) = if first_fails_on_budget {
            (
                format!(
                    "i32.const 0 i32.const {n} i32.const 0 i32.const 0 i32.const 96 call $deploy drop"
                ),
                data(96, &budget_bytes(EXCESS)),
                0,
            )
        } else {
            (
                "i32.const 512 i32.const 1 i32.const 0 i32.const 0 i32.const 64 call $deploy drop"
                    .to_string(),
                data(512, &[0xff]),
                1,
            )
        };
        let last = if outer_revert { "revert" } else { "return" };
        let body = format!(
            "{first}
    i32.const 0 i32.const {n} i32.const 0 i32.const 0 i32.const 64 call $deploy drop
    i32.const 2048 i32.const 32 call $call_result drop
    i32.const 2048 i32.const 32 call ${last}"
        );
        let text = module(&[
            &import("deploy"),
            &import("call_result"),
            &import(last),
            &memory(1, 1),
            &data(0, &code),
            &data(64, &budget_bytes(SUBBUDGET)),
            &first_data,
            &invoke(&body),
        ]);
        // The identifier of the second deployment, which the parent returns.
        let tx_id = identifier(&call_transaction(TARGET, &[]), ordinal);
        // A refused attempt pays only for copying its budget: its code and
        // input are never received. A deployment that begins pays for its
        // budget, and receives its code and input in its own budget. The
        // invalid one also derives its ContractId, reads for an existing
        // manifest and pays the code price and CodeHash of its one byte. The second
        // installs the child as a top-level deployment does, runs its
        // initializer warm, since validation prepared the code, and produces
        // the identifier.
        let (first_copy, invalid) = if first_fails_on_budget {
            (words(32), 0)
        } else {
            (
                words(1 + 32),
                CONTRACT_ID + KEY + code_price(&[0xff]) + code_hash(1),
            )
        };
        let mut c = case(&text)
            .named(format!(
                "first attempt {}, then {last}",
                if first_fails_on_budget {
                    "refused"
                } else {
                    "invalid"
                }
            ))
            .call()
            .result(tx_id)
            .charged(Ledger {
                dispatch: cost(&body),
                copies: first_copy + words(n + 32) + words(32),
                nested: invalid
                    + installation(&code)
                    + warm_entry_of(&child)
                    + cost("nop")
                    + IDENTIFIER,
                accesses: 7 + u64::from(!first_fails_on_budget),
                growth: RECORD_BASE + MANIFEST + RECORD_BASE + n,
                discarded: 32,
                ..Default::default()
            });
        c = if outer_revert {
            c.expect(Status::Revert)
        } else {
            c.installs(ordinal, &child)
        };
        c.run();
    }
}

/// Calls the program named at 0 under the budget at 64, then the one named
/// at 32 under the budget at 96, and returns both statuses.
const TWO_CALLS: &str = "i32.const 128 i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call i32.store
    i32.const 132 i32.const 32 i32.const 0 i32.const 0 i32.const 96 i32.const 0 call $call i32.store
    i32.const 128 i32.const 8 call $return";
fn calling_twice(first: Hash, second: Hash, first_budget: [u64; 4]) -> String {
    module(&[
        &import("call"),
        &import("return"),
        &memory(1, 1),
        &data(0, &first),
        &data(32, &second),
        &data(64, &budget_bytes(first_budget)),
        &data(96, &budget_bytes(SUBBUDGET)),
        &invoke(TWO_CALLS),
    ])
}

#[test]
fn code_is_prepared_once_per_transaction() {
    // The first invocation of a CodeHash in a transaction pays its code
    // price; any later one, of the same program or of another with the same
    // code, is warm. Code that differs in one data byte is other code.
    let child = returning_child();
    let other = module(&[
        &import("return"),
        &memory(1, 1),
        &data(0, b"y"),
        &invoke(RETURN_X),
    ]);
    let run = cost(RETURN_X) + SMALL.bytes(1);
    for (name, second, second_text, second_entry) in [
        (
            "the same program twice",
            CHILD,
            &child,
            warm_entry_of(&child),
        ),
        (
            "another program with the same code",
            GRANDCHILD,
            &child,
            warm_entry_of(&child),
        ),
        (
            "another program with other code",
            GRANDCHILD,
            &other,
            entry_of(&other),
        ),
    ] {
        let mut c = case(&calling_twice(CHILD, second, SUBBUDGET))
            .named(name)
            .with_program(CHILD, &child);
        if second == GRANDCHILD {
            c = c.with_program(GRANDCHILD, second_text);
        }
        c.call()
            .result(statuses(Status::Success, Status::Success))
            .charged(Ledger {
                dispatch: cost(TWO_CALLS),
                copies: 2 * words(64),
                nested: entry_of(&child) + run + second_entry + run,
                accesses: 4,
                discarded: 2,
                ..Default::default()
            })
            .run();
    }
}

#[test]
fn a_failed_first_call_leaves_its_code_prepared() {
    // A child that reverts both times: the second invocation is warm.
    let reverting = module(&[
        &import("revert"),
        &memory(1, 1),
        &data(0, b"x"),
        &invoke("i32.const 0 i32.const 1 call $revert"),
    ]);
    let run = cost("i32.const 0 i32.const 1 call $revert") + SMALL.bytes(1);
    case(&calling_twice(CHILD, CHILD, SUBBUDGET))
        .named("reverted")
        .with_program(CHILD, &reverting)
        .call()
        .result(statuses(Status::Revert, Status::Revert))
        .charged(Ledger {
            dispatch: cost(TWO_CALLS),
            copies: 2 * words(64),
            nested: entry_of(&reverting) + run + warm_entry_of(&reverting) + run,
            accesses: 4,
            discarded: 2,
            ..Default::default()
        })
        .run();
    // A first call whose compute covers its two record reads and its code
    // price runs out at its preparation, with the code prepared: the second
    // is warm. One C less and the code price is refused as a whole, nothing
    // is prepared, and the second call pays it.
    let child = returning_child();
    let price = code_price(&wasm(&child));
    let run = cost(RETURN_X) + SMALL.bytes(1);
    for (name, compute, first, second) in [
        (
            "out of compute after the code price",
            2 * KEY + price,
            2 * KEY + price,
            warm_entry_of(&child),
        ),
        (
            "out of compute at the code price",
            2 * KEY + price - 1,
            2 * KEY,
            entry_of(&child),
        ),
    ] {
        case(&calling_twice(
            CHILD,
            CHILD,
            [compute, SUBBUDGET[1], SUBBUDGET[2], SUBBUDGET[3]],
        ))
        .named(name)
        .with_program(CHILD, &child)
        .call()
        .result(statuses(Status::Compute, Status::Success))
        .charged(Ledger {
            dispatch: cost(TWO_CALLS),
            copies: 2 * words(64),
            nested: first + second + run,
            accesses: 4,
            discarded: 1,
            ..Default::default()
        })
        .run();
    }
}

#[test]
fn a_deployment_prepares_its_code_and_pays_validation_regardless() {
    let child = returning_child();
    let code = wasm(&child);
    let n = code.len() as u64;
    let run = cost(RETURN_X) + SMALL.bytes(1);
    // A deployment from inside: its installation as at the top level, the
    // initializer warm and the identifier.
    let deployment = installation(&code) + warm_entry_of(&child) + run + IDENTIFIER;
    // Deploys the child, reads its identifier and calls it: the call is warm.
    let deploy_then_call = format!(
        "i32.const 128 i32.const 512 i32.const {n} i32.const 0 i32.const 0 i32.const 64 call $deploy i32.store
    i32.const 160 i32.const 32 call $call_result drop
    i32.const 132 i32.const 160 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call i32.store
    i32.const 128 i32.const 8 call $return"
    );
    // Calls CHILD, installed with the same code, then deploys it: validation
    // pays the code price although the call has prepared the code.
    let call_then_deploy = format!(
        "i32.const 128 i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call i32.store
    i32.const 132 i32.const 512 i32.const {n} i32.const 0 i32.const 0 i32.const 64 call $deploy i32.store
    i32.const 128 i32.const 8 call $return"
    );
    for (name, body, copies, nested) in [
        (
            "deployed, then called",
            &deploy_then_call,
            words(n + 32) + words(32) + words(64),
            deployment + warm_entry_of(&child) + run,
        ),
        (
            "called, then deployed",
            &call_then_deploy,
            words(64) + words(n + 32),
            entry_of(&child) + run + deployment,
        ),
    ] {
        let text = module(&[
            &import("call"),
            &import("call_result"),
            &import("deploy"),
            &import("return"),
            &memory(1, 1),
            &data(0, &CHILD),
            &data(64, &budget_bytes(SUBBUDGET)),
            &data(512, &code),
            &invoke(body),
        ]);
        case(&text)
            .named(name)
            .with_program(CHILD, &child)
            .call()
            .result(statuses(Status::Success, Status::Success))
            .installs(0, &child)
            .charged(Ledger {
                dispatch: cost(body),
                copies,
                nested,
                // The deployment's existence check, its two writes and its
                // initializer's lookups, and the call's.
                accesses: 7 + 2,
                growth: RECORD_BASE + MANIFEST + RECORD_BASE + n,
                // The two results of "x" and the identifier.
                discarded: 1 + 1 + 32,
                ..Default::default()
            })
            .run();
    }
}

#[test]
fn a_child_grows_memory_against_compute_not_against_a_shared_cap() {
    let grow = "i32.const 0 i32.const 1023 memory.grow";
    let after = "i64.extend_i32_s i64.store i32.const 0 i32.const 8 call $return";
    let child = module(&[
        &import("return"),
        &memory(1, 1024),
        &invoke(&format!("{grow} {after}")),
    ]);
    let body = "i32.const 128 i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call i32.store
    i32.const 132 i32.const 136 i32.const 8 call $call_result i32.store
    i32.const 128 i32.const 16 call $return";
    let parent = |budget: [u64; 4]| {
        module(&[
            &import("call"),
            &import("call_result"),
            &import("return"),
            &memory(1, 1),
            &data(0, &CHILD),
            &data(64, &budget_bytes(budget)),
            &invoke(body),
        ])
    };
    // 1,023 pages cost 40,920,000 C: under a million the child runs out at
    // the grow, having paid its entry and three instructions.
    let mut result = (Status::Compute as u32).to_le_bytes().to_vec();
    result.extend(0u32.to_le_bytes());
    result.extend([0; 8]);
    case(&parent(SUBBUDGET))
        .named("exhausted")
        .with_program(CHILD, &child)
        .call()
        .result(result)
        .charged(Ledger {
            dispatch: cost(body),
            copies: words(64) + words(0),
            nested: entry_of(&child) + cost(grow),
            accesses: 2,
            ..Default::default()
        })
        .run();
    // With the compute, 64 MiB is live in one instance while the parent
    // waits; the store after the growth and the result's copy are priced at
    // the tier of 64 MiB.
    let grown = Tier::pages(1024);
    let mut result = 0u32.to_le_bytes().to_vec();
    result.extend(8u32.to_le_bytes());
    result.extend(1u64.to_le_bytes());
    case(&parent([45_000_000, 128, 4096, 4096]))
        .named("grown")
        .with_program(CHILD, &child)
        .call()
        .result(result)
        .charged(Ledger {
            dispatch: cost(body),
            copies: words(64) + words(8),
            nested: entry_of(&child)
                + cost(grow)
                + 1023 * PAGE
                + cost_at(after, grown)
                + grown.bytes(8),
            accesses: 2,
            discarded: 8,
            ..Default::default()
        })
        .run();
}

/// A level reads its input, the remaining depth, and branches on it.
const ENTER: &str = "i32.const 0 i32.const 1 call $input drop
    i32.const 0 i32.load8_u
    if";
/// A recursing level writes the depth less one as the child's input, reads
/// its own identifier as the child's target, and reads its remaining budget.
const DESCEND: &str = "i32.const 100 i32.const 0 i32.load8_u i32.const 1 i32.sub i32.store8
      i32.const 2 i32.const 128 i32.const 32 call $context drop
      i32.const 8 i32.const 64 i32.const 32 call $context";
/// It then subtracts `spent` from the remaining compute and calls itself with
/// the rest.
fn handing_on(spent: u64) -> String {
    format!(
        "drop i32.const 64 i32.const 64 i64.load i64.const {spent} i64.sub i64.store
      i32.const 200 i32.const 128 i32.const 100 i32.const 1 i32.const 64 i32.const 0 call $call"
    )
}
/// And returns the child's status.
const PASS_BACK: &str = "i32.store i32.const 200 i32.const 4 call $return";
/// The last level returns nothing.
const BOTTOM: &str = "end
    i32.const 0 i32.const 0 call $return";
/// What a level spends from the moment the budget getter reads its remaining
/// budget to the call's budget check: the getter's copy, the instructions
/// that hand the budget on and the copy of the call's arguments.
fn spent() -> u64 {
    SMALL.bytes(32) + cost(&handing_on(0)) + SMALL.bytes(32 + 1 + 32)
}
/// Calls itself to the depth its input names, each level passing exactly its
/// remaining budget to the next.
fn recursive() -> String {
    module(&[
        &import("input"),
        &import("context"),
        &import("call"),
        &import("return"),
        &memory(1, 1),
        &invoke(&format!(
            "{ENTER}
      {DESCEND}
      {}
      {PASS_BACK}
    {BOTTOM}",
            handing_on(spent())
        )),
    ])
}
#[test]
fn self_recursion_is_bounded_by_the_budget_each_level_passes_on() {
    let text = recursive();
    let recursing = format!("{ENTER} {DESCEND} {} {PASS_BACK}", handing_on(0));
    // A recursing level: its instructions, the input copy, two context
    // copies, the call's arguments and its four result bytes. The last level:
    // its instructions and the input copy. Every level below the top runs
    // code the top's invocation has prepared.
    let level = warm_entry_of(&text)
        + cost(&recursing)
        + SMALL.bytes(1)
        + 2 * SMALL.bytes(32)
        + SMALL.bytes(32 + 1 + 32)
        + SMALL.bytes(4);
    let last = warm_entry_of(&text) + cost(&format!("{ENTER} {BOTTOM}")) + SMALL.bytes(1);
    for depth in [1u64, 70, 200] {
        case(&text)
            .named(format!("depth {depth}"))
            .call_with(&[depth as u8])
            .result(0u32.to_le_bytes())
            .charged(Ledger {
                dispatch: cost(&recursing),
                copies: words(1) + words(32) + words(32) + words(32 + 1 + 32),
                nested: (depth - 1) * level + last,
                accesses: 2 * depth,
                discarded: 4 * (depth - 1),
                ..Default::default()
            })
            .run();
    }
}

fn level_id(level: u32) -> Hash {
    let mut id = [0x20; 32];
    id[30..].copy_from_slice(&(level as u16).to_be_bytes());
    id
}
/// Forwards to its target under its budget and returns whatever it returned.
const FORWARD: &str = "i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call drop
    i32.const 128 i32.const 64 call $call_result local.set 0
    i32.const 128 local.get 0 call $return";
fn forwarder(target: Hash, budget: [u64; 4]) -> String {
    module(&[
        &import("call"),
        &import("call_result"),
        &import("return"),
        &memory(1, 1),
        &data(0, &target),
        &data(64, &budget_bytes(budget)),
        &invoke(&format!("(local i32)\n    {FORWARD}")),
    ])
}
#[test]
fn four_hundred_nested_programs_do_not_recurse_on_the_native_stack() {
    let returning = "i32.const 0 i32.const 4 call $return";
    let bottom = module(&[
        &import("return"),
        &memory(1, 1),
        &data(0, b"leaf"),
        &invoke(returning),
    ]);
    // What one forwarder spends itself: its entry, the same for every
    // target and budget, its instructions, the call's arguments and the
    // four result bytes it reads and returns. Each level passes on a budget
    // that covers every level below it with one level's worth to spare.
    let own = entry_of(&forwarder(level_id(0), [0; 4]))
        + cost(FORWARD)
        + SMALL.bytes(64)
        + 2 * SMALL.bytes(4);
    let budget = |level: u64| [(level + 1) * own, 200 + level * 4, 8000, 8000];
    let mut c = case(&forwarder(level_id(399), budget(400))).with_program(level_id(0), &bottom);
    let mut nested = entry_of(&bottom) + cost(returning) + SMALL.bytes(4);
    for level in 1..=399u32 {
        let text = forwarder(level_id(level - 1), budget(level.into()));
        nested += entry_of(&text) + cost(FORWARD) + SMALL.bytes(64) + 2 * SMALL.bytes(4);
        c = c.with_program(level_id(level), &text);
    }
    c.call()
        .budget([BUDGET[0], 2048, 8192, 8192])
        .result(b"leaf")
        .charged(Ledger {
            dispatch: cost(FORWARD),
            copies: words(64) + words(4),
            nested,
            accesses: 2 * 400,
            discarded: 4 * 400,
            ..Default::default()
        })
        .run();
}

/// A child that answers the length of its input, read with a zero-capacity
/// `input`, as four bytes.
const LENGTH_BODY: &str = "i32.const 0 i32.const 0 i32.const 0 call $input i32.store i32.const 0 i32.const 4 call $return";
fn length_child() -> String {
    module(&[
        &import("input"),
        &import("return"),
        &memory(1, 1),
        &invoke(LENGTH_BODY),
    ])
}

#[test]
fn a_child_pays_for_receiving_its_input() {
    // The caller pays for copying the target and the budget; the child pays
    // for receiving its input as its first debit. A child that cannot pay
    // ends with status 3 having consumed nothing, and the caller continues.
    let n: u64 = 40_000;
    let receipt = SMALL.bytes(n);
    let body = format!(
        "i32.const 128 i32.const 0 i32.const 1024 i32.const {n} i32.const 64 i32.const 0 call $call i32.store
    i32.const 132 i32.const 4 call $call_result drop
    i32.const 128 i32.const 8 call $return"
    );
    for pays in [true, false] {
        let budget = if pays {
            SUBBUDGET
        } else {
            [receipt - 1, 128, 4096, 4096]
        };
        let text = module(&[
            &import("call"),
            &import("call_result"),
            &import("return"),
            &memory(1, 1),
            &data(0, &CHILD),
            &data(64, &budget_bytes(budget)),
            &invoke(&body),
        ]);
        let child = length_child();
        let (status, length) = if pays {
            (Status::Success, n as u32)
        } else {
            (Status::Compute, 0)
        };
        let mut result = (status as u32).to_le_bytes().to_vec();
        result.extend(length.to_le_bytes());
        let ledger = if pays {
            Ledger {
                dispatch: cost(&body),
                copies: words(64) + words(4),
                nested: receipt + entry_of(&child) + cost(LENGTH_BODY) + SMALL.bytes(4),
                accesses: 2,
                discarded: 4,
                ..Default::default()
            }
        } else {
            Ledger {
                dispatch: cost(&body),
                copies: words(64),
                ..Default::default()
            }
        };
        case(&text)
            .named(if pays {
                "the child pays"
            } else {
                "one C short"
            })
            .with_program(CHILD, &child)
            .call()
            .result(result)
            .charged(ledger)
            .run();
    }
}

#[test]
fn everything_left_less_a_reserve_passes_an_input_of_any_size() {
    // The caller hands on what it has left, less a reserve of 2,000 C for its
    // own work, with an input whose copy costs 7,500 C. The copy comes out of
    // the child's share, so the request still fits.
    let n: u64 = 60_000;
    assert!(SMALL.bytes(n) > 2_000);
    let body = format!(
        "i32.const 8 i32.const 64 i32.const 32 call $context drop
    i32.const 64 i32.const 64 i64.load i64.const 2000 i64.sub i64.store
    i32.const 128 i32.const 0 i32.const 1024 i32.const {n} i32.const 64 i32.const 0 call $call i32.store
    i32.const 132 i32.const 4 call $call_result drop
    i32.const 128 i32.const 8 call $return"
    );
    let text = module(&[
        &import("context"),
        &import("call"),
        &import("call_result"),
        &import("return"),
        &memory(1, 1),
        &data(0, &CHILD),
        &invoke(&body),
    ]);
    let child = length_child();
    let mut result = (Status::Success as u32).to_le_bytes().to_vec();
    result.extend((n as u32).to_le_bytes());
    case(&text)
        .with_program(CHILD, &child)
        .call()
        .result(result)
        .charged(Ledger {
            dispatch: cost(&body),
            copies: words(32) + words(64) + words(4),
            nested: SMALL.bytes(n) + entry_of(&child) + cost(LENGTH_BODY) + SMALL.bytes(4),
            accesses: 2,
            discarded: 4,
            ..Default::default()
        })
        .run();
}

#[test]
fn a_deployment_that_cannot_receive_its_code_takes_no_ordinal() {
    // The first deployment's budget is one C short of receiving its code: it
    // ends with status 3 before taking an ordinal, having consumed nothing.
    // The second therefore takes ordinal 0.
    let child = module(&[&invoke("nop")]);
    let code = wasm(&child);
    let n = code.len() as u64;
    assert!(n < 64);
    let body = format!(
        "i32.const 2048 i32.const 0 i32.const {n} i32.const 0 i32.const 0 i32.const 96 call $deploy i32.store
    i32.const 0 i32.const {n} i32.const 0 i32.const 0 i32.const 64 call $deploy drop
    i32.const 2052 i32.const 32 call $call_result drop
    i32.const 2048 i32.const 36 call $return"
    );
    let text = module(&[
        &import("deploy"),
        &import("call_result"),
        &import("return"),
        &memory(1, 1),
        &data(0, &code),
        &data(64, &budget_bytes(SUBBUDGET)),
        &data(96, &budget_bytes([SMALL.bytes(n) - 1, 128, 4096, 4096])),
        &invoke(&body),
    ]);
    let mut result = (Status::Compute as u32).to_le_bytes().to_vec();
    result.extend(identifier(&call_transaction(TARGET, &[]), 0));
    case(&text)
        .call()
        .result(result)
        .charged(Ledger {
            dispatch: cost(&body),
            copies: words(32) + words(32) + words(32),
            nested: SMALL.bytes(n)
                + installation(&code)
                + warm_entry_of(&child)
                + cost("nop")
                + IDENTIFIER,
            accesses: 7,
            growth: RECORD_BASE + MANIFEST + RECORD_BASE + n,
            discarded: 32,
            ..Default::default()
        })
        .installs(0, &child)
        .run();
}
