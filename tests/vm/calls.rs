//! Calls between programs and deployments from inside a program (rules
//! §24, §23): a child's outcome, its retained charges and its
//! rolled-back effects; the last-result buffer; read-only mode, explicit and
//! inherited; the ordinal of internal deployments; code prepared once per
//! transaction, by a call or by a deployment, whatever becomes of either; and
//! depth bounded by budgets alone, whether as self-recursion or as a chain of
//! four hundred programs.
use crate::{
    dsl::{escaped, import, invoke, memory, module, wasm},
    harness::{
        CHILD, GRANDCHILD, TARGET, deployed as case, deployed_code, deployed_entry_of,
        deployed_warm_entry_of, warm_entry_of,
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

#[test]
pub(crate) fn grown_parents_resume_at_their_own_prices_after_a_child() {
    for (before, after) in [
        (1u32, 2u32),
        (16, 17),
        (32, 33),
        (47, 48),
        (63, 64),
        (127, 128),
    ] {
        let initial = Tier::pages(before.into());
        let grown = Tier::pages(after.into());
        for child_pages in [1u32, 33] {
            let child_tier = Tier::pages(child_pages.into());
            for input_len in [0u64, 1, 31, 32, 33] {
                for reverts in [false, true] {
                    let end = if reverts { "revert" } else { "return" };
                    let child_body = format!("i32.const 0 i32.const 1 call ${end}");
                    let child = module(&[
                        &import(end),
                        &memory(child_pages, child_pages),
                        &data(0, b"x"),
                        &invoke(&child_body),
                    ]);
                    let grow = format!("i32.const {} memory.grow drop", after - before);
                    let body = format!("i32.const 256 i32.const 0 i32.const 128 i32.const {input_len} i32.const 64 i32.const 0 call $call i32.store
                        i32.const 260 i32.const 264 i32.const 1 call $call_result i32.store
                        i32.const 268 i32.const 264 i32.load8_u i32.store
                        i32.const 256 i32.const 16 call $return");
                    let text = module(&[
                        &import("call"),
                        &import("call_result"),
                        &import("return"),
                        &memory(before, after),
                        &data(0, &CHILD),
                        &data(64, &budget_bytes([4_000_000, 128, 4096, 4096])),
                        &data(128, &[0x41; 33]),
                        &invoke(&format!("{grow} {body}")),
                    ]);
                    let mut result = u32::from(reverts).to_le_bytes().to_vec();
                    result.extend(1u32.to_le_bytes());
                    result.extend([b'x', 0, 0, 0]);
                    result.extend(u32::from(b'x').to_le_bytes());
                    case(&text).named(format!("grown-resume/{before}-{after}/child-{child_pages}/input-{input_len}/revert-{reverts}"))
                        .with_program(CHILD, &child).call().result(result).charged(Ledger {
                            dispatch: cost_at(&grow, initial) + cost_at(&body, grown),
                            pages: u64::from(after - before),
                            nested: grown.bytes(input_len) + grown.spans(u64::from(input_len != 0))
                                + deployed_entry_of(&child) + cost_at(&child_body, child_tier)
                                + child_tier.bytes(1) + child_tier.spans(1),
                            extra: grown.bytes(64) + grown.spans(2) + grown.bytes(1) + grown.spans(1)
                                + grown.bytes(16) + grown.spans(1) - initial.bytes(16) - initial.spans(1),
                            accesses: 2, discarded: 1, ..Default::default()
                        }).run();
                }
            }
        }
    }
}
fn data(address: u32, bytes: &[u8]) -> String {
    format!("(data (i32.const {address}) {})", escaped(bytes))
}

#[test]
pub(crate) fn failed_initializers_undo_descendants_without_rewinding_ordinals_or_preparation() {
    let descendant = module(&[&memory(1, 1), &invoke("nop")]);
    let descendant_code = wasm(&descendant);
    let child_n = descendant_code.len() as u64;
    let child_c = SMALL.bytes(child_n)
        + installation(&descendant_code)
        + warm_entry_of(&descendant)
        + cost("nop")
        + IDENTIFIER;
    let writer = writing_child("return");
    let writer_work =
        cost(&writing_body("return")) + SMALL.bytes(33) + KEY + leaf(1) + 2 * SMALL.bytes(1);
    let writer_c = deployed_entry_of(&writer) + writer_work;
    for failure in [Status::Revert, Status::Trap, Status::Compute, Status::Data] {
        for parent_reverts in [false, true] {
            let ending = match failure {
                Status::Revert => "i32.const 64 i32.const 1 call $revert",
                Status::Trap => "unreachable",
                _ => "i32.const 64 i32.const 1 call $return",
            };
            let init_body = format!("i32.const 512 i32.const 64 i32.const 1 call $storage_set
                i32.const 64 i32.const 1 call $emit
                i32.const 1024 i32.const {child_n} i32.const 0 i32.const 0 i32.const 160 call $deploy drop
                i32.const 0 i32.const 0 i32.const 0 i32.const 128 i32.const 0 call $call drop {ending}");
            let initializer = module(&[
                &import("storage_set"),
                &import("emit"),
                &import("deploy"),
                &import("call"),
                &import("return"),
                &import("revert"),
                &memory(1, 1),
                &data(0, &CHILD),
                &data(64, b"r"),
                &data(128, &budget_bytes([writer_c, 4, 38, 37])),
                &data(160, &budget_bytes([child_c, 7, 32, 116 + child_n])),
                &data(1024, &descendant_code),
                &invoke(&init_body),
            ]);
            let init_code = wasm(&initializer);
            let n = init_code.len() as u64;
            let output = failure != Status::Trap;
            let init_c = SMALL.bytes(n)
                + installation(&init_code)
                + warm_entry_of(&initializer)
                + cost(&init_body)
                + SMALL.bytes(33)
                + KEY
                + leaf(1)
                + SMALL.bytes(1)
                + SMALL.bytes(32)
                + child_c
                + SMALL.bytes(64)
                + writer_c
                + if output { SMALL.bytes(1) } else { 0 };
            let init_d = 37 + 32 + 38 + u64::from(output);
            let mut limit = [init_c + IDENTIFIER + 100, 128, 4096, 4096];
            if failure == Status::Compute {
                limit[0] = init_c + IDENTIFIER - 1;
            }
            if failure == Status::Data {
                limit[2] = init_d + 31;
            }
            let end = if parent_reverts { "revert" } else { "return" };
            let body = format!("i32.const 256 i32.const 2048 i32.const {n} i32.const 0 i32.const 0 i32.const 64 call $deploy i32.store
                i32.const 260 i32.const 264 i32.const 1 call $call_result i32.store
                i32.const 268 i32.const 1024 i32.const {child_n} i32.const 0 i32.const 0 i32.const 96 call $deploy i32.store
                i32.const 272 i32.const 32 call $call_result drop
                i32.const 0 i32.const 0 i32.const 0 i32.const 128 i32.const 0 call $call drop
                i32.const 256 i32.const 48 call ${end}");
            let text = module(&[
                &import("deploy"),
                &import("call"),
                &import("call_result"),
                &import(end),
                &memory(1, 1),
                &data(0, &CHILD),
                &data(64, &budget_bytes(limit)),
                &data(96, &budget_bytes([child_c, 7, 32, 116 + child_n])),
                &data(128, &budget_bytes([writer_c, 4, 38, 37])),
                &data(1024, &descendant_code),
                &data(2048, &init_code),
                &invoke(&body),
            ]);
            let mut test = case(&text)
                .named(format!(
                    "initializer-descendants/{failure:?}/parent-reverts-{parent_reverts}"
                ))
                .with_program(CHILD, &writer)
                .call()
                .expect(if parent_reverts {
                    Status::Revert
                } else {
                    Status::Success
                })
                .installs(2, &descendant)
                .result_from(move |facts| {
                    let mut result = vec![0; 48];
                    result[..4].copy_from_slice(&(failure as u32).to_le_bytes());
                    if failure == Status::Revert {
                        result[4..8].copy_from_slice(&1u32.to_le_bytes());
                        result[8] = b'r';
                    }
                    result[16..].copy_from_slice(&facts.deployed(2));
                    result
                })
                .charged(Ledger {
                    dispatch: cost(&body),
                    copies: 2 * words(32)
                        + words(64)
                        + words(32)
                        + u64::from(failure == Status::Revert),
                    nested: init_c + child_c + deployed_warm_entry_of(&writer) + writer_work
                        - if parent_reverts { 0 } else { SMALL.bytes(1) },
                    accesses: 7 + 2 + 7 + 4 + 7 + 4,
                    growth: 116 + n + 37 + (116 + child_n) * 2 + 37 * 2,
                    discarded: init_d + 32 + 1 + if parent_reverts { 37 } else { 0 },
                    ..Default::default()
                });
            if !parent_reverts {
                test = test
                    .records_of(CHILD, [([0; 32], b"x")])
                    .events_from(CHILD, [b"x"]);
            }
            for ordinal in [0, 1] {
                test = test
                    .next_block()
                    .call_deployed(0, ordinal, &descendant, b"gone")
                    .expect(Status::Missing)
                    .consumed([
                        1 + KEY,
                        1,
                        vm_model::TRANSACTION_BASE + vm_model::call_size(4),
                        0,
                    ]);
            }
            test = test.next_block().call_deployed(0, 2, &descendant, b"later");
            if parent_reverts {
                test = test.expect(Status::Missing).consumed([
                    1 + KEY,
                    1,
                    vm_model::TRANSACTION_BASE + vm_model::call_size(5),
                    0,
                ]);
            } else {
                test = test.charged(dispatch(cost("nop")));
            }
            test.run();
        }
    }
}
fn dispatch(n: u64) -> Ledger {
    Ledger {
        dispatch: n,
        ..Default::default()
    }
}

#[test]
pub(crate) fn last_results_survive_reads_and_are_replaced_by_every_child_attempt() {
    let deployed = module(&[&invoke("nop")]);
    let code = wasm(&deployed);
    let code_n = code.len() as u64;
    let mut attempts = vec![
        ("missing", 0),
        ("receive-call", 0),
        ("invalid-deploy", 0),
        ("receive-deploy", 0),
        ("success-deploy", 0),
    ];
    for dimension in 0..4 {
        attempts.extend([("refuse-call", dimension), ("refuse-deploy", dimension)]);
    }
    for pages in [1u32, 33] {
        let tier = Tier::pages(pages.into());
        for reverts in [false, true] {
            for n in [0u64, 1, 31, 32, 33, 65] {
                let end = if reverts { "revert" } else { "return" };
                let child_body = format!("i32.const 0 i32.const {n} call ${end}");
                let payload: Vec<u8> = (0..n).map(|i| (i * 23 + 7) as u8).collect();
                let child = module(&[
                    &import(end),
                    &memory(1, 1),
                    &data(0, &payload),
                    &invoke(&child_body),
                ]);
                for &(attempt, dimension) in &attempts {
                    let deploy = attempt.ends_with("deploy");
                    let refused = attempt.starts_with("refuse");
                    let success = attempt == "success-deploy";
                    let receive = attempt.starts_with("receive");
                    let mut request = [1_000_000, 32, 512, 4096];
                    if refused {
                        request = [0; 4];
                        request[dimension] = u64::MAX;
                    }
                    if receive {
                        request[0] = 0;
                    }
                    let second = if deploy {
                        format!(
                            "i32.const 2048 i32.const {} i32.const 0 i32.const 0 i32.const 96 call $deploy",
                            if success { code_n } else { 1 }
                        )
                    } else {
                        format!(
                            "i32.const {} i32.const 192 i32.const {} i32.const 96 i32.const 0 call $call",
                            if attempt == "missing" { 32 } else { 0 },
                            u32::from(receive)
                        )
                    };
                    let body = format!("i32.const 512 i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call i32.store
                        i32.const 516 i32.const 544 i32.const 0 call $call_result i32.store
                        i32.const 520 i32.const 640 i32.const {} call $call_result i32.store
                        i32.const 524 i32.const 544 i32.const {n} call $call_result i32.store
                        i32.const 544 i32.const 9 i32.const {n} memory.fill
                        i32.const 528 i32.const 544 i32.const {n} call $call_result i32.store
                        i32.const 532 {second} i32.store
                        i32.const 536 i32.const 736 i32.const 32 call $call_result i32.store
                        i32.const 512 i32.const 256 call $return",n.saturating_sub(1));
                    let text = module(&[
                        &import("call"),
                        &import("deploy"),
                        &import("call_result"),
                        &import("return"),
                        &memory(pages, pages),
                        &data(0, &CHILD),
                        &data(32, &[0x5a; 32]),
                        &data(64, &budget_bytes([1_000_000, 32, 512, 4096])),
                        &data(96, &budget_bytes(request)),
                        &data(544, &[0xa5; 224]),
                        &data(2048, if success { &code } else { &[0xff] }),
                        &invoke(&body),
                    ]);
                    let status = if refused {
                        128u32
                    } else if receive {
                        Status::Compute as u32
                    } else if success {
                        0
                    } else if deploy {
                        Status::InvalidModule as u32
                    } else {
                        Status::Missing as u32
                    };
                    let second_c = if refused || receive {
                        0
                    } else if success {
                        tier.bytes(code_n)
                            + tier.spans(1)
                            + installation(&code)
                            + warm_entry_of(&deployed)
                            + cost("nop")
                            + IDENTIFIER
                    } else if deploy {
                        tier.bytes(1)
                            + tier.spans(1)
                            + CONTRACT_ID
                            + KEY
                            + code_price(&[0xff])
                            + code_hash(1)
                    } else {
                        KEY
                    };
                    let second_s = if refused || receive {
                        0
                    } else if success {
                        7
                    } else {
                        1
                    };
                    let payload = payload.clone();
                    let mut test = case(&text).named(format!("result-lifecycle/pages-{pages}/revert-{reverts}/len-{n}/{attempt}/{dimension}"))
                        .with_program(CHILD,&child).call().result_from(move |facts| {
                            let mut expected = vec![0;256];
                            expected[..4].copy_from_slice(&u32::from(reverts).to_le_bytes());
                            for at in [4,8,12,16] { expected[at..at+4].copy_from_slice(&(n as u32).to_le_bytes()); }
                            expected[20..24].copy_from_slice(&status.to_le_bytes());
                            expected[32..].fill(0xa5);
                            expected[32..32+n as usize].copy_from_slice(&payload);
                            if success {
                                expected[24..28].copy_from_slice(&32u32.to_le_bytes());
                                expected[224..256].copy_from_slice(&facts.deployed(0));
                            }
                            expected
                        }).charged(Ledger {
                            dispatch: cost_at(&body,tier),
                            copies: words(64) + 3 * words(n) + words(if deploy {32} else {64}) + if success {words(32)} else {0},
                            spans: 2 + 3 * u64::from(n != 0) + if deploy {1} else {2} + u64::from(success),
                            nested: deployed_entry_of(&child) + cost(&child_body) + SMALL.bytes(n) + second_c,
                            accesses: 2 + second_s, discarded: n + if success {32} else {0},
                            growth: if success {116+code_n} else {0}, ..Default::default()
                        });
                    if success {
                        test = test.installs(0, &deployed);
                    }
                    test.run();
                }
            }
        }
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

#[test]
pub(crate) fn a_parent_failure_undoes_successful_child_writes_and_events_but_keeps_charges() {
    let child = writing_child("return");
    let child_compute = deployed_entry_of(&child)
        + cost(&writing_body("return"))
        + SMALL.bytes(33)
        + KEY
        + leaf(1)
        + 2 * SMALL.bytes(1);
    for pages in [1u32, 33] {
        let tier = Tier::pages(pages.into());
        for status in [Status::Trap, Status::Compute, Status::Data] {
            let tail = if status == Status::Trap {
                "unreachable"
            } else {
                "i32.const 0 i32.const 1 call $return"
            };
            let body = format!(
                "i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call drop {tail}"
            );
            let text = module(&[
                &import("call"),
                &import("return"),
                &memory(pages, pages),
                &data(0, &CHILD),
                &data(64, &budget_bytes([child_compute, 4, 38, 37])),
                &invoke(&body),
            ]);
            let mut budget = BUDGET;
            if status == Status::Compute {
                budget[0] = 1
                    + deployed_entry_of(&text)
                    + cost_at(&body, tier)
                    + tier.bytes(64)
                    + tier.spans(2)
                    + child_compute
                    + tier.bytes(1)
                    + tier.spans(1)
                    - 1;
            }
            if status == Status::Data {
                budget[2] = vm_model::TRANSACTION_BASE + vm_model::call_size(0) + 38;
            }
            case(&text)
                .named(format!("parent-failure/pages-{pages}/{status:?}"))
                .with_program(CHILD, &child)
                .call()
                .budget(budget)
                .expect(status)
                .charged(Ledger {
                    dispatch: cost_at(&body, tier),
                    copies: 2,
                    spans: 2,
                    nested: child_compute,
                    accesses: 4,
                    growth: 37,
                    discarded: 38,
                    ..Default::default()
                })
                .run();
        }
    }
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
pub(crate) fn a_childs_charges_stay_whatever_becomes_of_its_effects() {
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
                nested: deployed_entry_of(&child)
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
pub(crate) fn call_checks_its_flags_before_copying_and_refuses_an_excessive_budget() {
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
pub(crate) fn read_only_is_refused_before_the_spans_of_a_mutation_are_checked() {
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
        let child = module(&[
            &import(operation),
            &memory(1, 1),
            &invoke(&format!("{attempt}{drop}")),
        ]);
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
                nested: deployed_entry_of(&child) + cost(&attempt),
                accesses: 2,
                ..Default::default()
            })
            .run();
    }
}

#[test]
pub(crate) fn the_caller_identity_comes_from_the_vm() {
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
            nested: deployed_entry_of(&child)
                + cost(child_body)
                + SMALL.bytes(33)
                + SMALL.bytes(33),
            accesses: 2,
            discarded: 33,
            ..Default::default()
        })
        .run();
}

#[test]
pub(crate) fn a_call_pays_its_spans_at_the_callers_memory_size() {
    // A caller of 3 MiB pays 17 for each of the target and budget spans, and
    // its child, as its first debit, 17 for the span of its input, at the
    // caller's prices; the child, in one page, pays no span for its result.
    let body = "i32.const 0 i32.const 96 i32.const 4 i32.const 64 i32.const 0 call $call drop
    i32.const 0 i32.const 0 call $return";
    let text = module(&[
        &import("call"),
        &import("return"),
        &memory(48, 48),
        &data(0, &CHILD),
        &data(64, &budget_bytes(SUBBUDGET)),
        &invoke(body),
    ]);
    let child = returning_child();
    let caller = Tier::pages(48);
    case(&text)
        .with_program(CHILD, &child)
        .call()
        .charged(Ledger {
            dispatch: cost_at(body, caller),
            copies: words(64),
            spans: 2,
            nested: caller.bytes(4)
                + caller.spans(1)
                + deployed_entry_of(&child)
                + cost(RETURN_X)
                + SMALL.bytes(1),
            accesses: 2,
            discarded: 1,
            ..Default::default()
        })
        .run();
}
#[test]
pub(crate) fn read_only_is_inherited_through_an_ordinary_call() {
    // The parent calls CHILD read-only; CHILD calls GRANDCHILD with flags 0;
    // GRANDCHILD's write is still refused with status 11, which travels back.
    let grandchild_body = "i32.const -1 i32.const -1 i32.const -1 call $storage_set";
    let grandchild = module(&[
        &import("storage_set"),
        &memory(1, 1),
        &invoke(grandchild_body),
    ]);
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
            nested: deployed_entry_of(&child)
                + cost(child_body)
                + SMALL.bytes(64)
                + SMALL.bytes(4)
                + deployed_entry_of(&grandchild)
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
pub(crate) fn the_last_result_is_cleared_by_a_refused_call() {
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
            nested: deployed_entry_of(&child) + cost(RETURN_X) + SMALL.bytes(1),
            accesses: 2,
            discarded: 1,
            ..Default::default()
        })
        .run();
}

#[test]
pub(crate) fn internal_deployments_advance_the_ordinal_only_when_begun() {
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
            .result_from(move |f| f.deployed(ordinal).to_vec())
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
pub(crate) fn code_is_prepared_once_per_transaction() {
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
            deployed_warm_entry_of(&child),
        ),
        (
            "another program with the same code",
            GRANDCHILD,
            &child,
            deployed_warm_entry_of(&child),
        ),
        (
            "another program with other code",
            GRANDCHILD,
            &other,
            deployed_entry_of(&other),
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
                nested: deployed_entry_of(&child) + run + second_entry + run,
                accesses: 4,
                discarded: 2,
                ..Default::default()
            })
            .run();
    }
}

#[test]
pub(crate) fn a_failed_first_call_leaves_its_code_prepared() {
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
            nested: deployed_entry_of(&reverting) + run + deployed_warm_entry_of(&reverting) + run,
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
    let price = code_price(&deployed_code(&child));
    let run = cost(RETURN_X) + SMALL.bytes(1);
    for (name, compute, first, second) in [
        (
            "out of compute after the code price",
            2 * KEY + price,
            2 * KEY + price,
            deployed_warm_entry_of(&child),
        ),
        (
            "out of compute at the code price",
            2 * KEY + price - 1,
            2 * KEY,
            deployed_entry_of(&child),
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
pub(crate) fn a_deployment_prepares_its_code_and_pays_validation_regardless() {
    // The program deploys the code CHILD was deployed with, gate and all;
    // its initializer gets no input and so runs the child's own body.
    let child = returning_child();
    let code = deployed_code(&child);
    let n = code.len() as u64;
    let run = cost(RETURN_X) + SMALL.bytes(1);
    // A deployment from inside: its installation as at the top level, the
    // initializer warm and the identifier.
    let deployment = installation(&code) + deployed_warm_entry_of(&child) + run + IDENTIFIER;
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
            deployment + deployed_warm_entry_of(&child) + run,
        ),
        (
            "called, then deployed",
            &call_then_deploy,
            words(64) + words(n + 32),
            deployed_entry_of(&child) + run + deployment,
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
            .installs_code(0, &code)
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
pub(crate) fn a_child_grows_memory_against_compute_not_against_a_shared_cap() {
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
            nested: deployed_entry_of(&child) + cost(grow),
            accesses: 2,
            ..Default::default()
        })
        .run();
    // With the compute, 64 MiB is live in one instance while the parent
    // waits; the store after the growth and the result's copy, with its span,
    // are priced at 64 MiB.
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
            nested: deployed_entry_of(&child)
                + cost(grow)
                + 1023 * PAGE
                + cost_at(after, grown)
                + grown.bytes(8)
                + grown.spans(1),
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
pub(crate) fn self_recursion_is_bounded_by_the_budget_each_level_passes_on() {
    let text = recursive();
    let recursing = format!("{ENTER} {DESCEND} {} {PASS_BACK}", handing_on(0));
    // A recursing level: its instructions, the input copy, two context
    // copies, the call's arguments and its four result bytes. The last level:
    // its instructions and the input copy. Every level below the top runs
    // code the top's invocation has prepared.
    let level = deployed_warm_entry_of(&text)
        + cost(&recursing)
        + SMALL.bytes(1)
        + 2 * SMALL.bytes(32)
        + SMALL.bytes(32 + 1 + 32)
        + SMALL.bytes(4);
    let last = deployed_warm_entry_of(&text) + cost(&format!("{ENTER} {BOTTOM}")) + SMALL.bytes(1);
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
pub(crate) fn four_hundred_nested_programs_do_not_recurse_on_the_native_stack() {
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
    let own = deployed_entry_of(&forwarder(level_id(0), [0; 4]))
        + cost(FORWARD)
        + SMALL.bytes(64)
        + 2 * SMALL.bytes(4);
    let budget = |level: u64| [(level + 1) * own, 200 + level * 4, 8000, 8000];
    let mut c = case(&forwarder(level_id(399), budget(400))).with_program(level_id(0), &bottom);
    let mut nested = deployed_entry_of(&bottom) + cost(returning) + SMALL.bytes(4);
    for level in 1..=399u32 {
        let text = forwarder(level_id(level - 1), budget(level.into()));
        nested += deployed_entry_of(&text) + cost(FORWARD) + SMALL.bytes(64) + 2 * SMALL.bytes(4);
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
pub(crate) fn a_child_pays_for_receiving_its_input() {
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
                nested: receipt + deployed_entry_of(&child) + cost(LENGTH_BODY) + SMALL.bytes(4),
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
pub(crate) fn everything_left_less_a_reserve_passes_an_input_of_any_size() {
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
            nested: SMALL.bytes(n) + deployed_entry_of(&child) + cost(LENGTH_BODY) + SMALL.bytes(4),
            accesses: 2,
            discarded: 4,
            ..Default::default()
        })
        .run();
}

#[test]
pub(crate) fn a_deployment_pays_its_spans_at_the_callers_memory_size() {
    // A caller of 3 MiB pays 17 for its budget's span, and the deployment, as
    // its first debit, 17 for the span of its code, at the caller's prices;
    // its empty input is no span.
    let child = module(&[&invoke("nop")]);
    let code = wasm(&child);
    let n = code.len() as u64;
    let body = format!(
        "i32.const 0 i32.const {n} i32.const 0 i32.const 0 i32.const 64 call $deploy drop
    i32.const 0 i32.const 0 call $return"
    );
    let text = module(&[
        &import("deploy"),
        &import("return"),
        &memory(48, 48),
        &data(0, &code),
        &data(64, &budget_bytes(SUBBUDGET)),
        &invoke(&body),
    ]);
    let caller = Tier::pages(48);
    case(&text)
        .call()
        .charged(Ledger {
            dispatch: cost_at(&body, caller),
            copies: words(32),
            spans: 1,
            nested: caller.bytes(n)
                + caller.spans(1)
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
#[test]
pub(crate) fn a_deployment_that_cannot_receive_its_code_takes_no_ordinal() {
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
    case(&text)
        .call()
        .result_from(|f| {
            let mut result = (Status::Compute as u32).to_le_bytes().to_vec();
            result.extend(f.deployed(0));
            result
        })
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

/// Calls `CHILD` under the budget at 64 and returns its status.
const STATUS_OF_CALL: &str = "i32.const 128 i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call i32.store
    i32.const 128 i32.const 4 call $return";

#[test]
pub(crate) fn a_child_traps_or_runs_out_of_data_or_growth_and_its_parent_continues() {
    let trapping = module(&[&memory(1, 1), &invoke("unreachable")]);
    let returning = returning_child();
    let writing = writing_child("return");
    let write = "i32.const 0 i32.const 32 i32.const 1 call $storage_set";
    // A trap after its dispatch; the one-byte result, refused whole under a
    // sub-budget of no data; the write, whose old-record read stands and
    // whose growth is refused under a sub-budget of none.
    for (name, child, budget, status, nested, accesses) in [
        (
            "the child traps",
            &trapping,
            SUBBUDGET,
            Status::Trap,
            deployed_entry_of(&trapping) + cost("unreachable"),
            2,
        ),
        (
            "the child has no data for its result",
            &returning,
            [SUBBUDGET[0], 128, 0, 4096],
            Status::Data,
            deployed_entry_of(&returning) + cost(RETURN_X),
            2,
        ),
        (
            "the child has no growth for its write",
            &writing,
            [SUBBUDGET[0], 128, 4096, 0],
            Status::Growth,
            deployed_entry_of(&writing) + cost(write) + SMALL.bytes(32 + 1) + KEY,
            3,
        ),
    ] {
        let text = module(&[
            &import("call"),
            &import("return"),
            &memory(1, 1),
            &data(0, &CHILD),
            &data(64, &budget_bytes(budget)),
            &invoke(STATUS_OF_CALL),
        ]);
        case(&text)
            .named(name)
            .with_program(CHILD, child)
            .call()
            .result((status as u32).to_le_bytes())
            .charged(Ledger {
                dispatch: cost(STATUS_OF_CALL),
                copies: words(64),
                nested,
                accesses,
                ..Default::default()
            })
            .run();
    }
}

#[test]
pub(crate) fn an_internal_deployment_reverts_traps_or_runs_out_of_access_or_growth() {
    let reverting = module(&[
        &import("revert"),
        &memory(1, 1),
        &data(0, b"r"),
        &invoke("i32.const 0 i32.const 1 call $revert"),
    ]);
    let trapping = module(&[&invoke("unreachable")]);
    let nop = module(&[&invoke("nop")]);
    for (name, child, budget, status) in [
        (
            "the initializer reverts",
            &reverting,
            SUBBUDGET,
            Status::Revert,
        ),
        ("the initializer traps", &trapping, SUBBUDGET, Status::Trap),
        (
            "no access for the existence check",
            &nop,
            [SUBBUDGET[0], 0, 4096, 4096],
            Status::Access,
        ),
        (
            "no growth for the manifest",
            &nop,
            [SUBBUDGET[0], 128, 4096, 0],
            Status::Growth,
        ),
    ] {
        let code = wasm(child);
        let n = code.len() as u64;
        let body = format!(
            "i32.const 128 i32.const 512 i32.const {n} i32.const 0 i32.const 0 i32.const 64 call $deploy i32.store
    i32.const 128 i32.const 4 call $return"
        );
        let text = module(&[
            &import("deploy"),
            &import("return"),
            &memory(1, 1),
            &data(64, &budget_bytes(budget)),
            &data(512, &code),
            &invoke(&body),
        ]);
        // The deployment receives its code, takes an ordinal and derives its
        // ContractId. An initializer that fails runs after the installation,
        // whose two records are rolled back with their growth kept, and a
        // reverted byte is data the receipt does not show. Without an access,
        // the existence check is refused; without growth, the manifest's
        // write is, after its old-record read.
        let receiving = SMALL.bytes(n) + CONTRACT_ID;
        let records = RECORD_BASE + MANIFEST + RECORD_BASE + n;
        let ran = |body: &str, result: u64| {
            installation(&code) + warm_entry_of(child) + cost(body) + SMALL.bytes(result)
        };
        let (nested, accesses, growth, discarded) = match status {
            Status::Revert => (
                SMALL.bytes(n) + ran("i32.const 0 i32.const 1 call $revert", 1),
                7,
                records,
                1,
            ),
            Status::Trap => (SMALL.bytes(n) + ran("unreachable", 0), 7, records, 0),
            Status::Access => (receiving, 0, 0, 0),
            _ => (
                receiving + KEY + code_price(&code) + code_hash(n) + KEY,
                2,
                0,
                0,
            ),
        };
        case(&text)
            .named(name)
            .call()
            .result((status as u32).to_le_bytes())
            .charged(Ledger {
                dispatch: cost(&body),
                copies: words(32),
                nested,
                accesses,
                growth,
                discarded,
                ..Default::default()
            })
            .run();
    }
}

#[test]
pub(crate) fn nested_deployment_output_boundaries_keep_charges_and_ordinals() {
    let second = module(&[&invoke("nop")]);
    let second_code = wasm(&second);
    let second_c = SMALL.bytes(second_code.len() as u64)
        + installation(&second_code)
        + warm_entry_of(&second)
        + cost("nop")
        + IDENTIFIER;
    for indirect in [false, true] {
        for n in [0u64, 1, 31, 32, 33, 63, 64, 65] {
            let payload: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            let ending = format!("i32.const 0 i32.const {n} call $return");
            let child = module(&[
                &import("return"),
                &memory(1, 1),
                &data(0, &payload),
                &invoke(&ending),
            ]);
            let code = wasm(&child);
            let prefix = SMALL.bytes(code.len() as u64)
                + installation(&code)
                + warm_entry_of(&child)
                + cost(&ending);
            let returned = prefix + SMALL.bytes(n);
            let mut limits = vec![
                (SUBBUDGET[0], n),
                (SUBBUDGET[0], n + 31),
                (SUBBUDGET[0], n + 32),
                (SUBBUDGET[0], n + 33),
                (returned + IDENTIFIER - 1, n + 32),
                (returned + IDENTIFIER - 1, n + 31),
                (returned + IDENTIFIER, n + 32),
            ];
            if n != 0 {
                limits.push((SUBBUDGET[0], n - 1));
            }
            for (c, d) in limits {
                let success = c >= returned + IDENTIFIER && d >= n + 32;
                let status = if d < n {
                    Status::Data
                } else if c < returned + IDENTIFIER {
                    Status::Compute
                } else if success {
                    Status::Success
                } else {
                    Status::Data
                };
                let first_c = if d < n {
                    prefix
                } else if success {
                    returned + IDENTIFIER
                } else {
                    returned
                };
                let first_d = if d < n {
                    0
                } else {
                    n + if success { 32 } else { 0 }
                };
                let call = if indirect {
                    "i32.const 0 call_indirect (param i32 i32 i32 i32 i32) (result i32)"
                } else {
                    "call $deploy"
                };
                let body = format!(
                    "i32.const 2048 i32.const 512 i32.const {} i32.const 0 i32.const 0 i32.const 64 {call} i32.store
                     i32.const 2052 i32.const 2056 i32.const 32 call $call_result i32.store
                     i32.const 1024 i32.const {} i32.const 0 i32.const 0 i32.const 96 call $deploy drop
                     i32.const 2088 i32.const 32 call $call_result drop
                     i32.const 2048 i32.const 72 call $return", code.len(), second_code.len());
                let table = if indirect {
                    "(table 1 1 funcref) (elem (i32.const 0) $deploy)"
                } else {
                    ""
                };
                let text = module(&[
                    &import("deploy"),
                    &import("call_result"),
                    &import("return"),
                    &memory(1, 1),
                    table,
                    &data(64, &budget_bytes([c, 128, d, 4096])),
                    &data(96, &budget_bytes(SUBBUDGET)),
                    &data(512, &code),
                    &data(1024, &second_code),
                    &invoke(&body),
                ]);
                let mut test = case(&text)
                    .named(format!(
                        "nested-output/indirect-{indirect}/len-{n}/C-{c}/D-{d}"
                    ))
                    .call()
                    .result_from(move |f| {
                        let mut r = (status as u32).to_le_bytes().to_vec();
                        r.extend((if success { 32u32 } else { 0 }).to_le_bytes());
                        r.extend(if success { f.deployed(0) } else { [0; 32] });
                        r.extend(f.deployed(1));
                        r
                    })
                    .charged(Ledger {
                        dispatch: cost(&body) + if indirect { 94 } else { 0 },
                        copies: 2 * words(32) + words(32) + if success { words(32) } else { 0 },
                        nested: first_c + second_c,
                        accesses: 14,
                        growth: 2 * (2 * RECORD_BASE + MANIFEST)
                            + code.len() as u64
                            + second_code.len() as u64,
                        discarded: first_d + 32,
                        ..Default::default()
                    })
                    .installs(1, &second);
                if success {
                    test = test.installs(0, &child);
                }
                // The failed identifier must not leave callable code behind;
                // installation expectations also check both complete records.
                test.run();
            }
        }
    }
}

#[test]
pub(crate) fn indirect_children_resume_with_results_and_live_operands() {
    for pages in [1u32, 2, 16, 17, 32, 33, 48] {
        let tier = Tier::pages(pages.into());
        for indirect in [false, true] {
            for status in [
                Status::Success,
                Status::Revert,
                Status::Trap,
                Status::Compute,
                Status::Access,
            ] {
                let ending = match status {
                    Status::Revert => "i32.const 0 i32.const 1 call $revert",
                    Status::Trap => "unreachable",
                    _ => "i32.const 0 i32.const 1 call $return",
                };
                let child = module(&[
                    &import("return"),
                    &import("revert"),
                    &memory(1, 1),
                    &data(0, b"x"),
                    &invoke(ending),
                ]);
                let requested = match status {
                    Status::Compute => [0, 128, 4096, 4096],
                    Status::Access => [SUBBUDGET[0], 0, 4096, 4096],
                    _ => SUBBUDGET,
                };
                let call = if indirect {
                    "i32.const 0 call_indirect (param i32 i32 i32 i32 i32) (result i32)"
                } else {
                    "call $call"
                };
                let body = format!(
                    "i32.const 128 i32.const 0 i32.const 100 i32.const 3 i32.const 64 i32.const 0 {call} i32.store
                     i32.const 132 i32.const 136 i32.const 4 call $call_result i32.store
                     i32.const 140 i32.const 305419896 i32.store
                     i32.const 128 i32.const 16 call $return");
                let table = if indirect {
                    "(table 1 1 funcref) (elem (i32.const 0) $call)"
                } else {
                    ""
                };
                let text = module(&[
                    &import("call"),
                    &import("call_result"),
                    &import("return"),
                    &memory(pages, pages),
                    table,
                    &data(0, &CHILD),
                    &data(64, &budget_bytes(requested)),
                    &data(100, b"abc"),
                    &invoke(&body),
                ]);
                let output = matches!(status, Status::Success | Status::Revert);
                let entered = !matches!(status, Status::Compute | Status::Access);
                let receive = tier.bytes(3) + tier.spans(1);
                let nested = match status {
                    Status::Compute => 0,
                    Status::Access => receive,
                    _ => {
                        receive
                            + deployed_entry_of(&child)
                            + cost(ending)
                            + if output { SMALL.bytes(1) } else { 0 }
                    }
                };
                let mut result = (status as u32).to_le_bytes().to_vec();
                result.extend(u32::from(output).to_le_bytes());
                result.extend(if output { [b'x', 0, 0, 0] } else { [0; 4] });
                result.extend(305419896u32.to_le_bytes());
                case(&text)
                    .named(format!(
                        "continuation/pages-{pages}/indirect-{indirect}/{status:?}"
                    ))
                    .with_program(CHILD, &child)
                    .call()
                    .result(result)
                    .charged(Ledger {
                        dispatch: cost_at(&body, tier) + if indirect { 94 } else { 0 },
                        copies: words(64) + u64::from(output),
                        spans: 2 + u64::from(output),
                        nested,
                        accesses: if entered { 2 } else { 0 },
                        discarded: u64::from(output),
                        ..Default::default()
                    })
                    .run();
            }
        }
    }
}

#[test]
pub(crate) fn deployment_receives_code_and_input_as_two_spans_rounded_together() {
    for pages in [1u32, 2, 16, 17, 32, 33, 48] {
        let tier = Tier::pages(pages.into());
        for remainder in [0usize, 1, 31] {
            // Select actual module bytes on either side of a word boundary;
            // padding is an ordinary active data segment, never a custom section.
            let child = (0..96)
                .map(|pad| module(&[&memory(1, 1), &data(256, &vec![0x3d; pad]), &invoke("nop")]))
                .find(|text| wasm(text).len() % 32 == remainder)
                .unwrap();
            let code = wasm(&child);
            let n = code.len() as u64;
            let run = installation(&code) + warm_entry_of(&child) + cost("nop");
            for input in [0u64, 1, 31, 32, 33] {
                let receive = tier.bytes(n + input) + tier.spans(1 + u64::from(input != 0));
                let needed = receive + run + IDENTIFIER;
                for overlap in [false, true] {
                    for limit in [receive - 1, receive, needed - 1, needed, needed + 1] {
                        let success = limit >= needed;
                        let entered = limit >= needed - 1;
                        let nested = if limit < receive {
                            0
                        } else if !entered {
                            receive
                        } else if success {
                            needed
                        } else {
                            receive + run
                        };
                        let ptr = if overlap { 1024 } else { 8192 };
                        let body = format!("i32.const 256 i32.const 1024 i32.const {n} i32.const {ptr} i32.const {input} i32.const 64 call $deploy i32.store
                            i32.const 260 i32.const 264 i32.const 32 call $call_result i32.store
                            i32.const 256 i32.const 40 call $return");
                        let b: Vec<u8> = [limit, 128, 4096, 4096]
                            .iter()
                            .flat_map(|v| v.to_le_bytes())
                            .collect();
                        let text = module(&[
                            &import("deploy"),
                            &import("call_result"),
                            &import("return"),
                            &memory(pages, pages),
                            &data(64, &b),
                            &data(1024, &code),
                            &data(8192, &[0x71; 33]),
                            &invoke(&body),
                        ]);
                        let mut test = case(&text)
                            .named(format!("receive/pages-{pages}/code-mod-{remainder}/input-{input}/alias-{overlap}/C-{limit}"))
                            .call().result_from(move |f| {
                                let status = if success { Status::Success } else { Status::Compute };
                                let mut r = (status as u32).to_le_bytes().to_vec();
                                r.extend((if success { 32u32 } else { 0 }).to_le_bytes());
                                r.extend(if success { f.deployed(0) } else { [0; 32] });
                                r
                            }).charged(Ledger {
                                dispatch: cost_at(&body, tier),
                                copies: words(32) + u64::from(success),
                                spans: 1 + u64::from(success), nested,
                                accesses: if entered { 7 } else { 0 },
                                growth: if entered { 2 * RECORD_BASE + MANIFEST + n } else { 0 },
                                discarded: if success { 32 } else { 0 },
                                ..Default::default()
                            });
                        if success {
                            test = test.installs(0, &child);
                        }
                        test.run();
                    }
                }
            }
        }
    }
}
