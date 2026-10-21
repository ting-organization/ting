//! What one invocation cannot see of another (rules §20, §24,
//! §23): every call instantiates the program afresh, events keep
//! their order across nested calls, a child with nothing to spend fails
//! before it starts, and an initializer's revert installs nothing.
use crate::{
    dsl::{escaped, import, invoke, memory, module, wasm},
    harness::{
        CHILD, GRANDCHILD, TARGET, deployed as case, deployed_entry_of, deployed_warm_entry_of,
        deployment, warm_entry_of,
    },
};
use ting_core::protocol::Status;
use vm_model::{
    KEY, Ledger, MANIFEST, RECORD_BASE, SMALL, TRANSACTION_BASE, cost, deploy_size, installation,
    words,
};

#[test]
pub(crate) fn reentrant_storage_is_shared_but_instances_and_checkpoints_are_separate() {
    for ending in ["return", "revert", "trap"] {
        let b_end = if ending == "trap" {
            "unreachable".into()
        } else {
            format!("i32.const 256 i32.const 77 call ${ending}")
        };
        let b_body = format!(
            "i32.const 0 i32.const 32 call $input drop
            i32.const 0 i32.const 128 i32.const 1 i32.const 64 i32.const 0 call $call drop
            i32.const 256 i32.const 77 call $call_result drop {b_end}"
        );
        let b = module(&[
            &import("input"),
            &import("call"),
            &import("call_result"),
            &import("return"),
            &import("revert"),
            &memory(1, 1),
            &data(64, &budget_bytes([1_000_000, 64, 2048, 2048])),
            &invoke(&b_body),
        ]);
        let test_input = "i32.const 0 i32.const 0 call $input if";
        let inner = "i32.const 0 i32.const 512 i32.const 4 call $storage_get drop
            i32.const 516 i32.const 128 i32.load8_u i32.store
            i32.const 520 global.get $g i32.store
            i32.const 3 i32.const 524 i32.const 33 call $context drop
            i32.const 2 i32.const 557 i32.const 32 call $context drop
            i32.const 64 i32.const 2 i32.store
            i32.const 0 i32.const 64 i32.const 4 call $storage_set
            i32.const 512 i32.const 77 call $return";
        let outer = "i32.const 128 i32.const 167 i32.store8 i32.const 9 global.set $g
            i32.const 64 i32.const 1 i32.store
            i32.const 0 i32.const 64 i32.const 4 call $storage_set
            i32.const 2 i32.const 192 i32.const 32 call $context drop
            i32.const 768 i32.const 256 i32.const 192 i32.const 32 i32.const 320 i32.const 0 call $call i32.store
            i32.const 772 i32.const 800 i32.const 77 call $call_result i32.store
            i32.const 0 i32.const 776 i32.const 4 call $storage_get drop
            i32.const 780 i32.const 128 i32.load8_u i32.store
            i32.const 784 global.get $g i32.store
            i32.const 768 i32.const 109 call $return";
        let a = module(&[
            &import("input"),
            &import("context"),
            &import("storage_get"),
            &import("storage_set"),
            &import("call"),
            &import("call_result"),
            &import("return"),
            &memory(1, 1),
            "(global $g (mut i32) (i32.const 3))",
            &data(128, &[90]),
            &data(256, &CHILD),
            &data(320, &budget_bytes([3_000_000, 128, 4096, 4096])),
            &invoke(&format!("{test_input} {inner} else {outer} end")),
        ]);
        let succeeds = ending == "return";
        let output = ending != "trap";
        let inner_c = deployed_warm_entry_of(&a)
            + cost(test_input)
            + cost(inner)
            + SMALL.bytes(1)
            + SMALL.bytes(32)
            + KEY
            + SMALL.bytes(4)
            + SMALL.bytes(33)
            + SMALL.bytes(32)
            + SMALL.bytes(36)
            + KEY
            + vm_model::leaf(4)
            + SMALL.bytes(77);
        let b_c = deployed_entry_of(&b) + cost(&b_body) + SMALL.bytes(32) // receive
            + SMALL.bytes(32) + SMALL.bytes(64) + inner_c + SMALL.bytes(77)
            + if output { SMALL.bytes(77) } else { 0 };
        case(&a)
            .named(format!("reentrant/{ending}"))
            .with_program(CHILD, &b)
            .call()
            .records([([0; 32], if succeeds { 2u32 } else { 1 }.to_le_bytes())])
            .result_from(move |facts| {
                let status = match ending {
                    "return" => Status::Success,
                    "revert" => Status::Revert,
                    _ => Status::Trap,
                };
                let mut result = vec![0; 109];
                result[..4].copy_from_slice(&(status as u32).to_le_bytes());
                result[4..8].copy_from_slice(&(if output { 77u32 } else { 0 }).to_le_bytes());
                result[8..12].copy_from_slice(&(if succeeds { 2u32 } else { 1 }).to_le_bytes());
                result[12..16].copy_from_slice(&167u32.to_le_bytes());
                result[16..20].copy_from_slice(&9u32.to_le_bytes());
                if output {
                    result[32..36].copy_from_slice(&1u32.to_le_bytes());
                    result[36..40].copy_from_slice(&90u32.to_le_bytes());
                    result[40..44].copy_from_slice(&3u32.to_le_bytes());
                    result[44] = 1;
                    result[45..77].copy_from_slice(&facts.id(CHILD));
                    result[77..109].copy_from_slice(&facts.id(TARGET));
                }
                result
            })
            .charged(Ledger {
                dispatch: cost(test_input) + cost(outer),
                copies: words(36)
                    + words(32)
                    + words(64)
                    + words(32)
                    + words(4)
                    + if output { words(77) } else { 0 },
                reads: 2,
                hashing: vm_model::leaf(4),
                accesses: 3 + 2 + 2 + 3,
                growth: RECORD_BASE + 4,
                nested: b_c,
                discarded: 77 + if output { 77 } else { 0 },
                ..Default::default()
            })
            .run();
    }
}

#[test]
pub(crate) fn sibling_and_grandchild_effects_rollback_to_their_own_checkpoints() {
    let write = "i32.const 0 i32.const 32 i32.const 1 call $storage_set";
    let emit = "i32.const 32 i32.const 1 call $emit";
    let leaf_body = format!(
        "i32.const 32 i32.const 1 call $input drop
        i32.const 0 i32.const 96 i32.const 1 call $storage_get drop {write} {emit}
        i32.const 96 i32.const 1 call $return"
    );
    let leaf = module(&[
        &import("input"),
        &import("storage_set"),
        &import("storage_get"),
        &import("emit"),
        &import("return"),
        &memory(1, 1),
        &invoke(&leaf_body),
    ]);
    for fail in ["revert", "trap", "compute", "data", "access", "growth"] {
        for parent_fails in [false, true] {
            let tail = match fail {
                "revert" => "i32.const 0 i32.const 0 call $revert",
                "trap" => "unreachable",
                "compute" => "nop", // no C remains for this dispatch
                "data" => "i32.const 32 i32.const 1 call $emit",
                "access" => "i32.const 0 call $storage_delete",
                _ => "i32.const 1 i32.const 32 i32.const 1 call $storage_set",
            };
            let middle_prefix = format!(
                "{write} {emit}
                i32.const 0 i32.const 64 i32.const 1 i32.const 128 i32.const 0 call $call drop"
            );
            // The first sibling already prepared the leaf's code and created
            // its record. An exact grandchild request fits every tight frame.
            let leaf_run = cost(&leaf_body)
                + SMALL.bytes(1) * 5
                + SMALL.bytes(32)
                + SMALL.bytes(33)
                + 2 * KEY
                + vm_model::leaf(1);
            let middle = module(&[
                &import("storage_set"),
                &import("storage_delete"),
                &import("emit"),
                &import("call"),
                &import("revert"),
                &memory(1, 1),
                &data(0, &GRANDCHILD),
                &data(32, b"b"),
                &data(64, b"c"),
                &data(
                    128,
                    &budget_bytes([deployed_warm_entry_of(&leaf) + leaf_run, 5, 38, 0]),
                ),
                &invoke(&format!("{middle_prefix} {tail}")),
            ]);
            let middle_before_tail = deployed_entry_of(&middle)
                + cost(&middle_prefix)
                + SMALL.bytes(33)
                + KEY
                + vm_model::leaf(1)
                + SMALL.bytes(1)
                + SMALL.bytes(64)
                + deployed_warm_entry_of(&leaf)
                + leaf_run;
            let mut sub = [3_000_000, 128, 4096, 4096];
            let (extra_c, extra_s) = match fail {
                "compute" => {
                    sub[0] = middle_before_tail;
                    (0, 0)
                }
                "data" => {
                    sub[2] = 75;
                    (cost(tail), 0)
                }
                "access" => {
                    sub[1] = 9;
                    (cost(tail) + SMALL.bytes(32), 0)
                }
                "growth" => {
                    sub[3] = 37;
                    (cost(tail) + SMALL.bytes(33) + KEY, 1)
                }
                _ => (cost(tail), 0),
            };
            let middle_total = middle_before_tail + extra_c;
            let one =
                "i32.const 0 i32.const 64 i32.const 1 i32.const 128 i32.const 0 call $call drop";
            let two = "i32.const 256 i32.const 32 i32.const 0 i32.const 0 i32.const 160 i32.const 0 call $call i32.store";
            let three =
                "i32.const 0 i32.const 65 i32.const 1 i32.const 128 i32.const 0 call $call drop";
            let finish = if parent_fails {
                "i32.const 256 i32.const 8 call $revert"
            } else {
                "i32.const 256 i32.const 8 call $return"
            };
            let body = format!(
                "i32.const 66 i32.const 1 call $emit {one} {two} {three}
                i32.const 260 i32.const 1 call $call_result drop
                i32.const 67 i32.const 1 call $emit {finish}"
            );
            let parent = module(&[
                &import("call"),
                &import("call_result"),
                &import("emit"),
                &import("revert"),
                &import("return"),
                &memory(1, 1),
                &data(0, &GRANDCHILD),
                &data(32, &CHILD),
                &data(64, b"adpz"),
                &data(128, &budget_bytes(SUBBUDGET)),
                &data(160, &budget_bytes(sub)),
                &invoke(&body),
            ]);
            let retained = if parent_fails { 0 } else { 4 };
            let mut c = case(&parent)
                .named(format!("checkpoints/{fail}/parent-reverts-{parent_fails}"))
                .with_program(GRANDCHILD, &leaf)
                .with_program(CHILD, &middle)
                .call()
                .expect(if parent_fails {
                    Status::Revert
                } else {
                    Status::Success
                })
                .result(
                    (match fail {
                        "revert" => Status::Revert,
                        "trap" => Status::Trap,
                        "compute" => Status::Compute,
                        "data" => Status::Data,
                        "access" => Status::Access,
                        _ => Status::Growth,
                    } as u32)
                        .to_le_bytes()
                        .into_iter()
                        .chain([b'a', 0, 0, 0])
                        .collect::<Vec<_>>(),
                )
                .charged(Ledger {
                    dispatch: cost(&body),
                    copies: 3 * words(64) + 2 + words(1),
                    nested: deployed_entry_of(&leaf)
                        + deployed_warm_entry_of(&leaf)
                        + 2 * leaf_run
                        + middle_total
                        - SMALL.bytes(1) // first lookup finds no value to copy
                        - retained * SMALL.bytes(1),
                    accesses: 5 + 9 + extra_s + 5,
                    growth: 37 + 37,
                    discarded: (6 - retained) * 37 + 3,
                    ..Default::default()
                });
            if !parent_fails {
                c = c
                    .records_of(GRANDCHILD, [([0; 32], b"d")])
                    .events([b"p"])
                    .events_from(GRANDCHILD, [b"a", b"d"])
                    .events([b"z"]);
            }
            c.run();
        }
    }
}
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
pub(crate) fn identical_code_keeps_contract_storage_and_read_only_frames_separate() {
    let prefix = "i32.const 32 i32.const 2 call $input local.set $n local.get $n if";
    let write = "i32.const 0 i32.const 32 i32.const 1 call $storage_set";
    let test_revert = "local.get $n i32.const 2 i32.eq if";
    let revert = "i32.const 0 i32.const 0 call $revert";
    let read = "i32.const 64 i32.const 0 i32.const 68 i32.const 1 call $storage_get i32.store
        i32.const 2 i32.const 72 i32.const 32 call $context drop
        i32.const 64 i32.const 40 call $return";
    let leaf = module(&[
        &import("input"),
        &import("storage_set"),
        &import("storage_get"),
        &import("context"),
        &import("return"),
        &import("revert"),
        &memory(1, 1),
        &invoke(&format!(
            "(local $n i32) {prefix} {write} {test_revert} {revert} end end {read}"
        )),
    ]);
    // target offset, input offset/length, flag, status, final byte on success.
    let sequence = [
        (0, 96, 1, 0, Status::Success, b'a'),
        (32, 97, 1, 0, Status::Success, b'b'),
        (0, 98, 1, 1, Status::ReadOnly, 0),
        (0, 96, 0, 0, Status::Success, b'a'),
        (32, 98, 2, 0, Status::Revert, 0),
        (32, 96, 0, 0, Status::Success, b'b'),
        (0, 100, 1, 0, Status::Success, b'd'),
    ];
    let mut body = String::new();
    let (mut nested, mut accesses, mut discarded) = (0, 0, 0);
    for (index, &(target, ptr, n, flag, status, _)) in sequence.iter().enumerate() {
        let at = 512 + index * 48;
        body.push_str(&format!("i32.const {at} i32.const {target} i32.const {ptr} i32.const {n} i32.const 64 i32.const {flag} call $call i32.store
            i32.const {} i32.const {} i32.const 40 call $call_result i32.store ",at+4,at+8));
        nested += if index == 0 {
            deployed_entry_of(&leaf)
        } else {
            deployed_warm_entry_of(&leaf)
        };
        nested += SMALL.bytes(n) * 2 + cost(prefix);
        accesses += 2;
        if n != 0 {
            nested += cost(write);
            if flag == 0 {
                nested += SMALL.bytes(33) + KEY + vm_model::leaf(1) + cost(test_revert);
                accesses += 2;
            }
        }
        if status == Status::Revert {
            nested += cost(revert);
        }
        if status == Status::Success {
            nested += cost(read)
                + SMALL.bytes(32)
                + KEY
                + SMALL.bytes(1)
                + SMALL.bytes(32)
                + SMALL.bytes(40);
            accesses += 1;
            discarded += 40;
        }
    }
    body.push_str("i32.const 512 i32.const 336 call $return");
    let parent = module(&[
        &import("call"),
        &import("call_result"),
        &import("return"),
        &memory(1, 1),
        &data(0, &CHILD),
        &data(32, &GRANDCHILD),
        &data(64, &budget_bytes(SUBBUDGET)),
        &data(96, b"abcxd"),
        &invoke(&body),
    ]);
    case(&parent)
        .with_program(CHILD, &leaf)
        .with_program(GRANDCHILD, &leaf)
        .call()
        .records_of(CHILD, [([0; 32], b"d")])
        .records_of(GRANDCHILD, [([0; 32], b"b")])
        .result_from(move |facts| {
            let mut result = vec![0; 336];
            for (i, &(target, _, _, _, status, value)) in sequence.iter().enumerate() {
                let at = i * 48;
                result[at..at + 4].copy_from_slice(&(status as u32).to_le_bytes());
                if status == Status::Success {
                    result[at + 4..at + 8].copy_from_slice(&40u32.to_le_bytes());
                    result[at + 8..at + 12].copy_from_slice(&1u32.to_le_bytes());
                    result[at + 12] = value;
                    result[at + 16..at + 48].copy_from_slice(&facts.id(if target == 0 {
                        CHILD
                    } else {
                        GRANDCHILD
                    }));
                }
            }
            result
        })
        .charged(Ledger {
            dispatch: cost(&body),
            copies: 7 * words(64) + 5 * words(40),
            nested,
            accesses,
            discarded,
            growth: 2 * 37,
            ..Default::default()
        })
        .run();
}

#[test]
pub(crate) fn every_call_runs_a_fresh_instance_even_within_one_transaction() {
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
            nested: deployed_entry_of(&child) + deployed_warm_entry_of(&child) + 2 * run,
            accesses: 2 * 2,
            discarded: 2,
            ..Default::default()
        })
        .run();
}

#[test]
pub(crate) fn events_keep_their_order_across_a_nested_call() {
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
            nested: deployed_entry_of(&child) + cost(child_body),
            accesses: 2,
            ..Default::default()
        })
        .run();
}

#[test]
pub(crate) fn a_child_with_nothing_to_spend_fails_before_it_starts() {
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
            .with_program(CHILD, &module(&[&memory(1, 1), &invoke("nop")]))
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
pub(crate) fn an_initializer_that_reverts_fails_the_deployment_with_its_bytes() {
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
