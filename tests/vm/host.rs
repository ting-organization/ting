//! The host imports (rules §22): every span rule, getters
//! that report a length and copy only what fits, the nine context selectors,
//! storage reads, writes and deletions with their keys, leaves, accesses and
//! growth, the code hash of another program, and the three ways an invocation
//! ends.
use crate::{
    dsl::{escaped, import, invoke, memory, module, module_from, wasm},
    harness::{
        CHILD, Facts, GRANDCHILD, TARGET, context, deployed as case, deployed_code,
        deployed_entry_of as entry_of, deployment, k, warm_entry_of,
    },
};
use std::collections::BTreeSet;
use ting_core::{hash::hash, protocol::Status};
use vm_model::{
    BUDGET, KEY, Ledger, MANIFEST, RECORD_BASE, SMALL, TRANSACTION_BASE, Tier, call_size, cost,
    cost_at, deploy_size, installation, leaf, words,
};

fn dispatch(n: u64) -> Ledger {
    Ledger {
        dispatch: n,
        ..Default::default()
    }
}
fn has_result(host: &str) -> bool {
    matches!(
        host,
        "input" | "context" | "storage_get" | "code_hash" | "call" | "deploy" | "call_result"
    )
}
fn data(address: u32, bytes: &[u8]) -> String {
    format!("(data (i32.const {address}) {})", escaped(bytes))
}

#[test]
pub(crate) fn spans_are_validated_before_argument_bytes_are_charged() {
    // Each program calls one import with no compute left for copying its
    // arguments: a valid span fails on the copy debit, an invalid one traps
    // before any copy is attempted.
    let table: [(&str, &[i32], &[i32]); 6] = [
        ("storage_get", &[0, 0, 0], &[0, -1, 0]),
        ("storage_set", &[0, 0, 0], &[0, -1, 0]),
        ("storage_delete", &[0], &[65505]),
        ("call", &[0, 0, 0, 0, 0], &[0, -1, 0, 0, 0]),
        ("deploy", &[0, 0, 0, 0, 0], &[0, 0, -1, 0, 0]),
        ("code_hash", &[0, 0, 0], &[0, -1, 0]),
    ];
    for (host, valid, invalid) in table {
        for (args, status) in [(valid, Status::Compute), (invalid, Status::Trap)] {
            let constants: Vec<String> = args.iter().map(|a| format!("i32.const {a}")).collect();
            let calling = format!("{} call ${host}", constants.join(" "));
            let drop = if has_result(host) { " drop" } else { "" };
            let text = module(&[
                &import(host),
                &memory(1, 1),
                &invoke(&format!("{calling}{drop}")),
            ]);
            // The constants and the call's own price; the drop is never reached.
            let dispatch = cost(&calling);
            case(&text)
                .named(format!("{host} with a {status:?} span"))
                .call()
                .budget([
                    1 + entry_of(&text) + dispatch,
                    BUDGET[1],
                    BUDGET[2],
                    BUDGET[3],
                ])
                .expect(status)
                .charged(Ledger {
                    dispatch,
                    ..Default::default()
                })
                .run();
        }
    }
}

#[test]
pub(crate) fn input_reports_its_length_and_copies_only_when_it_fits() {
    let hundred = [b'x'; 100];
    let table: [(&[u8], usize); 6] = [
        (b"", 0),
        (b"abc", 0),
        (b"abc", 2),
        (b"abc", 3),
        (b"abc", 4),
        (&hundred, 100),
    ];
    for (payload, capacity) in table {
        let n = payload.len();
        let sentinel = vec![b'Z'; n];
        let mut parts = vec![import("input"), import("return"), memory(1, 1)];
        if n > 0 {
            parts.push(data(4, &sentinel));
        }
        let body = format!(
            "i32.const 0 i32.const 4 i32.const {capacity} call $input i32.store i32.const 0 i32.const {} call $return",
            4 + n
        );
        parts.push(invoke(&body));
        let fits = capacity >= n;
        let mut result = (n as u32).to_le_bytes().to_vec();
        result.extend(if fits { payload } else { &sentinel });
        case(&module_from(&parts))
            .named(format!("{n} bytes into {capacity}"))
            .call_with(payload)
            .result(result)
            .charged(Ledger {
                dispatch: cost(&body),
                copies: if fits { words(n as u64) } else { 0 },
                ..Default::default()
            })
            .run();
    }
}
#[test]
pub(crate) fn a_getter_span_is_checked_in_full_even_for_an_empty_input() {
    for (ptr, capacity, ok) in [
        (65536, 0, true),
        (65537, 0, false),
        (65535, 2, false),
        (-1, 0, false),
    ] {
        let calling = format!("i32.const 0 i32.const {ptr} i32.const {capacity} call $input");
        let body = format!("{calling} i32.store i32.const 0 i32.const 4 call $return");
        let text = module(&[
            &import("input"),
            &import("return"),
            &memory(1, 1),
            &invoke(&body),
        ]);
        let c = case(&text).named(format!("span {ptr}+{capacity}")).call();
        if ok {
            c.result(0u32.to_le_bytes())
                .charged(dispatch(cost(&body)))
                .run();
        } else {
            c.expect(Status::Trap)
                .charged(dispatch(cost(&calling)))
                .run();
        }
    }
}
#[test]
pub(crate) fn a_zero_length_span_still_needs_a_memory() {
    // A program without a memory that makes the call on every path shows it
    // in its deployment: its initializer traps at the call, after the call's
    // dispatch, and nothing is installed.
    for host in ["input", "call_result", "return", "revert", "emit"] {
        let calling = format!("i32.const 0 i32.const 0 call ${host}");
        let drop = if has_result(host) { " drop" } else { "" };
        let text = module(&[&import(host), &invoke(&format!("{calling}{drop}"))]);
        let code = wasm(&text);
        let n = code.len() as u64;
        deployment(&text)
            .named(host)
            .deploy()
            .expect(Status::Trap)
            .consumed([
                1 + installation(&code) + warm_entry_of(&text) + cost(&calling),
                7,
                TRANSACTION_BASE + deploy_size(n, 0),
                RECORD_BASE + MANIFEST + RECORD_BASE + n,
            ])
            .run();
    }
}

#[test]
pub(crate) fn context_selectors_report_the_block_the_transaction_and_the_budget() {
    let calling = |selector: i32, n: usize| {
        format!("i32.const {selector} i32.const 0 i32.const {n} call $context")
    };
    let body = |selector: i32, n: usize| {
        format!(
            "{} drop i32.const 0 i32.const {n} call $return",
            calling(selector, n)
        )
    };
    let program = |selector: i32, n: usize| {
        module(&[
            &import("context"),
            &import("return"),
            &memory(1, 1),
            &invoke(&body(selector, n)),
        ])
    };
    // Selector 8 reports the frame's allowance before the getter's own copy
    // debit: after the root dispatch, the entry with its two record reads,
    // and the three constants and the call's own price.
    let remaining: Vec<u8> = {
        let entry = entry_of(&program(8, 32));
        [
            BUDGET[0] - (1 + entry + cost(&calling(8, 32))),
            BUDGET[1] - 2,
            BUDGET[2] - (TRANSACTION_BASE + call_size(0)),
            BUDGET[3],
        ]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect()
    };
    // Each value with its length: the chain, the transaction's identifier,
    // the program's own, the caller's (none at the top), then the block the
    // transaction lands in, and the budget. The block's producer data is the
    // case's, so that its length is known.
    let chain = context().chain;
    let producer = b"fixture miner".to_vec();
    type Read = Box<dyn Fn(&Facts) -> Vec<u8>>;
    let values: [(usize, Read); 9] = [
        (32, Box::new(move |_| chain.to_vec())),
        (32, Box::new(|f| f.tx_id().to_vec())),
        (32, Box::new(|f| f.id(TARGET).to_vec())),
        (33, Box::new(|_| vec![0; 33])),
        (8, Box::new(|f| f.block().height.to_le_bytes().to_vec())),
        (8, Box::new(|f| f.block().timestamp.to_le_bytes().to_vec())),
        (32, Box::new(|f| f.block().parent.to_vec())),
        (producer.len(), Box::new(|f| f.block().producer.clone())),
        (32, Box::new(move |_| remaining.clone())),
    ];
    for (selector, (n, value)) in values.into_iter().enumerate() {
        case(&program(selector as i32, n))
            .named(format!("selector {selector}"))
            .with_producer(&producer)
            .call()
            .result_from(value)
            .charged(Ledger {
                dispatch: cost(&body(selector as i32, n)),
                copies: words(n as u64),
                ..Default::default()
            })
            .run();
    }
    for selector in [9, -1] {
        let calling = calling(selector, 0);
        case(&module(&[
            &import("context"),
            &memory(1, 1),
            &invoke(&format!("{calling} drop")),
        ]))
        .named(format!("selector {selector}"))
        .call()
        .expect(Status::Trap)
        .charged(dispatch(cost(&calling)))
        .run();
    }
}
#[test]
pub(crate) fn producer_data_is_copied_only_when_it_fits() {
    let producer = [0x5a; 100];
    let calling = |address: i32, capacity: i32| {
        format!("i32.const 512 i32.const 7 i32.const {address} i32.const {capacity} call $context")
    };
    let body = |address: i32, capacity: i32| {
        format!(
            "{} i64.extend_i32_s i64.store
    i32.const 512 i32.const 8 call $return",
            calling(address, capacity)
        )
    };
    let query = |address: i32, capacity: i32| {
        module(&[
            &import("context"),
            &import("return"),
            &memory(1, 1),
            &invoke(&body(address, capacity)),
        ])
    };
    for capacity in [0, 31, 99, 100, 101, 4096] {
        case(&query(0, capacity))
            .named(format!("capacity {capacity}"))
            .with_producer(producer)
            .call()
            .result(100u64.to_le_bytes())
            .charged(Ledger {
                dispatch: cost(&body(0, capacity)),
                copies: if capacity >= 100 { words(100) } else { 0 },
                ..Default::default()
            })
            .run();
    }
    // An invalid span traps even for a length query.
    case(&query(65537, 0))
        .named("invalid span")
        .with_producer(producer)
        .call()
        .expect(Status::Trap)
        .charged(dispatch(cost(&calling(65537, 0))))
        .run();
}
#[test]
pub(crate) fn producer_length_queries_move_no_bytes() {
    // 20,000 length queries of a 128 KiB producer cost the loop once and its
    // body each round, the host call's own price among it, nothing else: an
    // implementation that copied the producer per query would move 2.6
    // gigabytes here.
    let round = "i32.const 7 i32.const 0 i32.const 0 call $context drop
      local.get $i i32.const 1 i32.add local.set $i
      local.get $i i32.const 20000 i32.lt_u br_if $again";
    let text = module(&[
        &import("context"),
        &memory(1, 1),
        &invoke(&format!(
            "(local $i i32)
    (loop $again
      {round})"
        )),
    ]);
    case(&text)
        .with_producer(vec![1; 128 << 10])
        .call()
        .charged(dispatch(cost("loop") + 20_000 * cost(round)))
        .run();
}

#[test]
pub(crate) fn host_copies_pay_a_span_each_above_2_mib() {
    // At 3 MiB every span the host copies out of or into memory pays 17
    // besides its words, when it holds a byte: an empty value's write moves
    // only its key, a full one its key and its value, a read its key and the
    // value it copies out, and an event its payload, which the model counts
    // from the receipt.
    let body = "i32.const 0 i32.const 32 i32.const 0 call $storage_set
    i32.const 0 i32.const 32 i32.const 5 call $storage_set
    i32.const 0 i32.const 64 i32.const 5 call $storage_get drop
    i32.const 32 i32.const 5 call $emit
    i32.const 64 i32.const 5 call $return";
    let text = module(&[
        &import("storage_set"),
        &import("storage_get"),
        &import("emit"),
        &import("return"),
        &memory(48, 48),
        &data(32, b"hello"),
        &invoke(body),
    ]);
    case(&text)
        .call()
        .result(b"hello")
        .records([(k(0), b"hello")])
        .events([b"hello"])
        .charged(Ledger {
            dispatch: cost_at(body, Tier::pages(48)),
            copies: words(32) + words(32 + 5) + words(32) + words(5),
            spans: 1 + 2 + 1 + 1,
            hashing: leaf(0) + leaf(5),
            reads: 3,
            accesses: 2 + 2 + 1,
            growth: 36 + 5,
            ..Default::default()
        })
        .run();
}
#[test]
pub(crate) fn storage_get_reports_the_length_and_copies_only_when_it_fits() {
    let olds: [Option<Vec<u8>>; 5] = [
        None,
        Some(Vec::new()),
        Some(b"abc".to_vec()),
        Some((0..=255u8).cycle().take(4096).collect()),
        Some((0..=255u8).cycle().take(10240).collect()),
    ];
    for old in olds {
        let n = old.as_ref().map_or(0, Vec::len);
        let mut capacities = vec![0, n.saturating_sub(1), n];
        capacities.dedup();
        for capacity in capacities {
            let sentinel = vec![b'Z'; n];
            let mut parts = vec![import("storage_get"), import("return"), memory(1, 1)];
            if n > 0 {
                parts.push(data(36, &sentinel));
            }
            // The key is the 32 zero bytes at address 0.
            let body = format!(
                "i32.const 32 i32.const 0 i32.const 36 i32.const {capacity} call $storage_get i32.store
    i32.const 32 i32.const {} call $return",
                4 + n
            );
            parts.push(invoke(&body));
            let copied = old.is_some() && capacity >= n;
            let mut result = match &old {
                Some(_) => (n as u32).to_le_bytes().to_vec(),
                None => u32::MAX.to_le_bytes().to_vec(),
            };
            result.extend(match &old {
                Some(old) if copied => old.clone(),
                _ => sentinel,
            });
            let mut c = case(&module_from(&parts)).named(format!(
                "{} into {capacity}",
                old.as_ref()
                    .map_or("absent".to_string(), |o| format!("{} bytes", o.len()))
            ));
            if let Some(old) = &old {
                c = c.with_storage(TARGET, [(k(0), old.clone())]);
            }
            // The key's copy and the read with its key price always; the
            // value's copy only when it fits.
            c.call()
                .result(result)
                .charged(Ledger {
                    dispatch: cost(&body),
                    copies: words(32) + if copied { words(n as u64) } else { 0 },
                    reads: 1,
                    accesses: 1,
                    ..Default::default()
                })
                .run();
        }
    }
}
#[test]
pub(crate) fn a_length_query_of_a_large_record_pays_its_key_and_its_access_only() {
    // 1,000 length queries of a 96 KiB record, as large as one block's growth
    // allows: each pays its instructions, the copy of its key, the key's
    // 1,900 C and one S, nothing in proportion to the value. The program that
    // wrote the record needs two pages to hold it.
    let round = "i32.const 0 i32.const 0 i32.const 0 call $storage_get drop
      local.get $i i32.const 1 i32.add local.tee $i
      i32.const 1000 i32.lt_u br_if $again";
    let text = module(&[
        &import("storage_get"),
        &memory(2, 2),
        &invoke(&format!(
            "(local $i i32)
    (loop $again
      {round})"
        )),
    ]);
    case(&text)
        .with_storage(TARGET, [(k(0), vec![7u8; 96 << 10])])
        .call()
        .charged(Ledger {
            dispatch: cost("loop") + 1_000 * cost(round),
            copies: 1_000 * words(32),
            reads: 1_000,
            accesses: 1_000,
            ..Default::default()
        })
        .run();
}
#[test]
pub(crate) fn a_record_read_debits_its_key_with_its_access() {
    // The read of the record debits [1,900, 1, 0, 0] in one step after the
    // key's copy: one C short of it fails on compute, one S short on access,
    // and neither charges any part of the read.
    let calling = "i32.const 0 i32.const 0 i32.const 0 call $storage_get";
    let body = format!("{calling} drop");
    let text = module(&[&import("storage_get"), &memory(1, 1), &invoke(&body)]);
    let before_read = 1 + entry_of(&text) + cost(calling) + SMALL.bytes(32);
    let complete = before_read + KEY + cost("drop");
    for (c, s, status) in [
        (complete, 3, Status::Success),
        (before_read + KEY - 1, 3, Status::Compute),
        (complete, 2, Status::Access),
    ] {
        let read = status == Status::Success;
        case(&text)
            .named(format!("C {c}, S {s}"))
            .call()
            .budget([c, s, BUDGET[2], BUDGET[3]])
            .expect(status)
            .charged(Ledger {
                dispatch: if read { cost(&body) } else { cost(calling) },
                copies: words(32),
                reads: u64::from(read),
                accesses: u64::from(read),
                ..Default::default()
            })
            .run();
    }
}
#[test]
pub(crate) fn storage_set_charges_the_value_per_word_and_growth_gross() {
    // The value is copied with its key at the tier's price per word and
    // hashed into its leaf: one SHA-256 block up to two bytes, two up to 66,
    // three from 67.
    let big: Vec<u8> = (0..=255u8).cycle().take(19968).chain([1, 2]).collect();
    type Variant<'a> = (&'a str, Option<&'a [u8]>, Vec<u8>);
    let variants: [Variant; 12] = [
        ("create empty", None, Vec::new()),
        ("create one byte", None, b"x".to_vec()),
        ("create two bytes", None, b"xy".to_vec()),
        ("create three bytes", None, b"xyz".to_vec()),
        ("create 66", None, vec![b'x'; 66]),
        ("create 67", None, vec![b'x'; 67]),
        ("create 4096", None, vec![b'x'; 4096]),
        ("create 4097", None, vec![b'y'; 4097]),
        ("create 19970", None, big),
        ("no-op", Some(b"xyz"), b"xyz".to_vec()),
        ("shrink", Some(b"xyz"), b"x".to_vec()),
        ("grow", Some(b"x"), b"xyz".to_vec()),
    ];
    for (name, old, new) in variants {
        let n = new.len();
        let mut parts = vec![import("storage_set"), import("return"), memory(1, 1)];
        if n > 0 {
            parts.push(data(32, &new));
        }
        let body = format!(
            "i32.const 0 i32.const 32 i32.const {n} call $storage_set i32.const 0 i32.const 0 call $return"
        );
        parts.push(invoke(&body));
        // A new record grows by its key and length plus its value; an
        // existing one by what its value gains, and never shrinks the counter.
        let growth = match old {
            None => 36 + n as u64,
            Some(old) => n.saturating_sub(old.len()) as u64,
        };
        let mut c = case(&module_from(&parts)).named(name);
        if let Some(old) = old {
            c = c.with_storage(TARGET, [(k(0), old)]);
        }
        c.call()
            .records([(k(0), new.clone())])
            .charged(Ledger {
                dispatch: cost(&body),
                copies: words(32 + n as u64),
                hashing: leaf(n as u64),
                reads: 1,
                accesses: 2,
                growth,
                ..Default::default()
            })
            .run();
    }
}
#[test]
pub(crate) fn storage_delete_pays_its_two_accesses_whether_or_not_the_record_exists() {
    // The key's copy, the read of the old record with its key price, and the
    // write's access, with no leaf to hash.
    let body = "i32.const 0 call $storage_delete i32.const 0 i32.const 0 call $return";
    let text = module(&[
        &import("storage_delete"),
        &import("return"),
        &memory(1, 1),
        &invoke(body),
    ]);
    for present in [false, true] {
        let mut c = case(&text).named(if present { "present" } else { "absent" });
        if present {
            c = c.with_storage(TARGET, [(k(0), b"x")]);
        }
        c.call()
            .no_records()
            .charged(Ledger {
                dispatch: cost(body),
                copies: words(32),
                reads: 1,
                accesses: 2,
                ..Default::default()
            })
            .run();
    }
}
#[test]
pub(crate) fn storage_set_fails_atomically_when_access_or_growth_runs_out() {
    let calling = "i32.const 0 i32.const 32 i32.const 1 call $storage_set";
    let body = format!("{calling} i32.const 0 i32.const 0 call $return");
    let text = module(&[
        &import("storage_set"),
        &import("return"),
        &memory(1, 1),
        &data(32, b"x"),
        &invoke(&body),
    ]);
    // The write needs two accesses after the entry's two and 37 bytes of growth.
    for (s, g) in [(4, 36), (3, 37), (3, 36), (4, 37), (5, 38)] {
        let c = case(&text)
            .named(format!("S {s}, G {g}"))
            .call()
            .budget([BUDGET[0], s, BUDGET[2], g]);
        if s >= 4 && g >= 37 {
            c.records([(k(0), b"x")])
                .charged(Ledger {
                    dispatch: cost(&body),
                    copies: words(33),
                    hashing: leaf(1),
                    reads: 1,
                    accesses: 2,
                    growth: 37,
                    ..Default::default()
                })
                .run();
        } else {
            // The old-record read succeeded with its key price; the
            // mutation's debit of the leaf, S and G was refused as a whole,
            // so neither the hashing nor the growth shows.
            c.expect(if s < 4 {
                Status::Access
            } else {
                Status::Growth
            })
            .charged(Ledger {
                dispatch: cost(calling),
                copies: words(33),
                reads: 1,
                accesses: 1,
                ..Default::default()
            })
            .run();
        }
    }
}
#[test]
pub(crate) fn an_empty_record_is_present_until_deleted() {
    // get of an absent key, set of an empty value, get, delete, get: the
    // lengths are -1, 0 and -1, the empty record costs its 36 bytes of
    // growth and the leaf of an empty value, every call reads one record,
    // and gross growth is never refunded.
    let body = "i32.const 64 i32.const 0 i32.const 128 i32.const 0 call $storage_get i32.store
    i32.const 0 i32.const 128 i32.const 0 call $storage_set
    i32.const 68 i32.const 0 i32.const 128 i32.const 0 call $storage_get i32.store
    i32.const 0 call $storage_delete
    i32.const 72 i32.const 0 i32.const 128 i32.const 0 call $storage_get i32.store
    i32.const 64 i32.const 12 call $return";
    let text = module(&[
        &import("storage_get"),
        &import("storage_set"),
        &import("storage_delete"),
        &import("return"),
        &memory(1, 1),
        &invoke(body),
    ]);
    let mut result = u32::MAX.to_le_bytes().to_vec();
    result.extend(0u32.to_le_bytes());
    result.extend(u32::MAX.to_le_bytes());
    case(&text)
        .call()
        .result(result)
        .charged(Ledger {
            dispatch: cost(body),
            copies: 5 * words(32),
            hashing: leaf(0),
            reads: 5,
            accesses: 1 + 2 + 1 + 2 + 1,
            growth: 36,
            ..Default::default()
        })
        .run();
}

#[test]
pub(crate) fn code_hash_reads_another_programs_manifest() {
    let child = module(&[&memory(1, 1), &invoke("nop")]);
    for (target, capacity, present) in
        [(CHILD, 32, true), (CHILD, 0, true), (GRANDCHILD, 32, false)]
    {
        let body = format!(
            "i32.const 0 i32.const 32 i32.const {capacity} call $code_hash local.set 0
    i32.const 64 local.get 0 i32.store
    i32.const 32 i32.const 36 call $return"
        );
        let text = module(&[
            &import("code_hash"),
            &import("return"),
            &memory(1, 1),
            &data(0, &target),
            &invoke(&format!("(local i32)\n    {body}")),
        ]);
        let copied = present && capacity >= 32;
        let mut expected = if copied {
            hash("code", &deployed_code(&child)).to_vec()
        } else {
            vec![0; 32]
        };
        expected.extend(if present { 32u32 } else { u32::MAX }.to_le_bytes());
        // The target's copy, the manifest's read with its key price, and the
        // hash's copy when it exists and fits.
        case(&text)
            .named(format!(
                "{} into {capacity}",
                if present { "present" } else { "absent" }
            ))
            .with_program(CHILD, &child)
            .call()
            .result(expected)
            .charged(Ledger {
                dispatch: cost(&body),
                copies: words(32) + if copied { words(32) } else { 0 },
                reads: 1,
                accesses: 1,
                ..Default::default()
            })
            .run();
    }
}

#[test]
pub(crate) fn return_revert_and_emit_charge_their_payload_and_validate_their_span() {
    for host in ["return", "revert", "emit"] {
        for n in [0usize, 1, 8, 33, 100] {
            let payload: Vec<u8> = (0..n as u8).collect();
            let mut parts = vec![import(host), memory(1, 1)];
            if n > 0 {
                parts.push(data(0, &payload));
            }
            let body = format!("i32.const 0 i32.const {n} call ${host}");
            parts.push(invoke(&body));
            let c = case(&module_from(&parts))
                .named(format!("{host} of {n} bytes"))
                .call();
            let c = match host {
                "return" => c.result(&payload),
                "revert" => c.expect(Status::Revert).result(&payload),
                _ => c.events([&payload]),
            };
            c.charged(dispatch(cost(&body))).run();
        }
        for (ptr, len) in [(65536, 0), (65537, 0), (-1, 0), (65535, 2)] {
            let body = format!("i32.const {ptr} i32.const {len} call ${host}");
            let text = module(&[&import(host), &memory(1, 1), &invoke(&body)]);
            let c = case(&text)
                .named(format!("{host} of span {ptr}+{len}"))
                .call();
            let c = if ptr == 65536 && len == 0 {
                match host {
                    "return" => c,
                    "revert" => c.expect(Status::Revert),
                    _ => c.events([b""]),
                }
            } else {
                c.expect(Status::Trap)
            };
            c.charged(dispatch(cost(&body))).run();
        }
    }
}

#[test]
pub(crate) fn a_copy_that_does_not_fit_stops_the_import_after_its_own_price() {
    // One C short of the copy that follows the host call's own price, for a
    // getter that copies eight or thirty-two bytes out and an event that
    // copies eight in: status 3, the call charged and nothing of the copy.
    for (host, calling, n, input) in [
        (
            "input",
            "i32.const 0 i32.const 8 call $input",
            8,
            &b"12345678"[..],
        ),
        (
            "context",
            "i32.const 0 i32.const 0 i32.const 32 call $context",
            32,
            b"",
        ),
        ("emit", "i32.const 0 i32.const 8 call $emit", 8, b""),
    ] {
        let drop = if has_result(host) { " drop" } else { "" };
        let text = module(&[
            &import(host),
            &memory(1, 1),
            &invoke(&format!("{calling}{drop}")),
        ]);
        case(&text)
            .named(format!("{host} one C short of its copy"))
            .call_with(input)
            .budget([
                1 + entry_of(&text) + cost(calling) + SMALL.bytes(n) - 1,
                BUDGET[1],
                BUDGET[2],
                BUDGET[3],
            ])
            .expect(Status::Compute)
            .charged(dispatch(cost(calling)))
            .run();
    }
}
#[test]
pub(crate) fn call_result_stops_when_its_copy_does_not_fit() {
    // The child returns "x" under a sub-budget of exactly what it needs; the
    // parent is one C short of copying the byte out.
    let child = module(&[
        &import("return"),
        &memory(1, 1),
        &data(0, b"x"),
        &invoke("i32.const 0 i32.const 1 call $return"),
    ]);
    let child_needs =
        entry_of(&child) + cost("i32.const 0 i32.const 1 call $return") + SMALL.bytes(1);
    let calling = "i32.const 0 i32.const 0 i32.const 0 i32.const 64 i32.const 0 call $call drop
    i32.const 128 i32.const 1 call $call_result";
    let budget: Vec<u8> = [child_needs, 2, 1, 0]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let text = module(&[
        &import("call"),
        &import("call_result"),
        &memory(1, 1),
        &data(0, &CHILD),
        &data(64, &budget),
        &invoke(&format!("{calling} drop")),
    ]);
    let before_copy = 1 + entry_of(&text) + cost(calling) + SMALL.bytes(64) + child_needs;
    case(&text)
        .named("call_result one C short of its copy")
        .with_program(CHILD, &child)
        .call()
        .budget([
            before_copy + SMALL.bytes(1) - 1,
            BUDGET[1],
            BUDGET[2],
            BUDGET[3],
        ])
        .expect(Status::Compute)
        .charged(Ledger {
            dispatch: cost(calling),
            copies: words(64),
            nested: child_needs,
            accesses: 2,
            discarded: 1,
            ..Default::default()
        })
        .run();
}
#[test]
pub(crate) fn a_record_lookup_that_does_not_fit_stops_the_import() {
    // With the entry's two accesses alone, the record read of a deletion and
    // of a code hash is refused whole, after the copy of the key or target;
    // with one more, the deletion reads the absent record and its write's
    // access is refused.
    let delete = "i32.const 0 call $storage_delete";
    let code_hash = "i32.const 0 i32.const 32 i32.const 32 call $code_hash";
    for (host, calling, s, read) in [
        ("storage_delete", delete, 2, false),
        ("storage_delete", delete, 3, true),
        ("code_hash", code_hash, 2, false),
    ] {
        let drop = if has_result(host) { " drop" } else { "" };
        let text = module(&[
            &import(host),
            &memory(1, 1),
            &invoke(&format!("{calling}{drop}")),
        ]);
        case(&text)
            .named(format!("{host} with {s} accesses"))
            .call()
            .budget([BUDGET[0], s, BUDGET[2], BUDGET[3]])
            .expect(Status::Access)
            .charged(Ledger {
                dispatch: cost(calling),
                copies: words(32),
                reads: u64::from(read),
                accesses: u64::from(read),
                ..Default::default()
            })
            .run();
    }
}

const COPY_PAGES: &[u32] = &[1, 2, 16, 17, 32, 33, 48, 64];
const COPY_LENGTHS: &[usize] = &[0, 1, 31, 32, 33, 63, 64, 65];

#[test]
pub(crate) fn code_hash_snapshots_aliased_targets_and_leaves_short_outputs_untouched() {
    let child = module(&[&memory(1, 1), &invoke("nop")]);
    let expected_hash = hash("code", &deployed_code(&child));
    for &pages in COPY_PAGES {
        let tier = Tier::pages(pages.into());
        for present in [false, true] {
            let target = if present { CHILD } else { [0x44; 32] };
            for offset in [0usize, 1, 16, 32] {
                for cap in [0usize, 31, 32, 33] {
                    let mut initial = vec![0xa5; 96];
                    initial[..32].copy_from_slice(&target);
                    let body = format!("i32.const 96 i32.const 0 i32.const {offset} i32.const {cap} call $code_hash i32.store
                        i32.const 0 i32.const 100 call $return");
                    let text = module(&[
                        &import("code_hash"),
                        &import("return"),
                        &memory(pages, pages),
                        &data(0, &initial),
                        &invoke(&body),
                    ]);
                    let copied = present && cap >= 32;
                    case(&text).named(format!("aliased-code-hash/pages-{pages}/present-{present}/offset-{offset}/cap-{cap}"))
                        .with_program(CHILD, &child).call().result_from(move |facts| {
                            let mut expected = vec![0xa5; 96];
                            expected[..32].copy_from_slice(&facts.id(target));
                            if copied { expected[offset..offset + 32].copy_from_slice(&expected_hash); }
                            expected.extend(if present { 32u32 } else { u32::MAX }.to_le_bytes());
                            expected
                        }).charged(Ledger {
                            dispatch: cost_at(&body, tier), copies: 1 + u64::from(copied),
                            spans: 1 + u64::from(copied), reads: 1, accesses: 1,
                            ..Default::default()
                        }).run();
                }
            }
        }
    }
}

#[test]
pub(crate) fn input_and_producer_getters_copy_whole_or_nothing_across_price_steps() {
    for &pages in COPY_PAGES {
        let tier = Tier::pages(pages.into());
        for host in ["input", "context"] {
            for &n in COPY_LENGTHS {
                let payload: Vec<u8> = (0..n).map(|i| (i * 37 + 19) as u8).collect();
                let capacities: BTreeSet<_> = [n.saturating_sub(1), n, n + 1].into();
                for cap in capacities {
                    let args = if host == "input" {
                        format!("i32.const 128 i32.const {cap}")
                    } else {
                        format!("i32.const 7 i32.const 128 i32.const {cap}")
                    };
                    let body = format!(
                        "i32.const 124 {args} call ${host} i32.store
                        i32.const 124 i32.const 100 call $return"
                    );
                    let sentinel = [0xa5u8; 96];
                    let text = module(&[
                        &import(host),
                        &import("return"),
                        &memory(pages, pages),
                        &data(128, &sentinel),
                        &invoke(&body),
                    ]);
                    let copied = cap >= n;
                    let mut result = (n as u32).to_le_bytes().to_vec();
                    let mut expected = sentinel;
                    if copied {
                        expected[..n].copy_from_slice(&payload);
                    }
                    result.extend(expected);
                    let test =
                        case(&text).named(format!("getter/{host}/pages-{pages}/len-{n}/cap-{cap}"));
                    let test = if host == "input" {
                        test.call_with(&payload)
                    } else {
                        test.with_producer(&payload).call()
                    };
                    test.result(result)
                        .charged(Ledger {
                            dispatch: cost_at(&body, tier),
                            copies: if copied { words(n as u64) } else { 0 },
                            spans: u64::from(copied && n != 0),
                            ..Default::default()
                        })
                        .run();
                }
            }
        }
    }
}

#[test]
pub(crate) fn storage_get_snapshots_an_aliased_key_before_copying_its_value() {
    let key = [0x43u8; 32];
    for &pages in COPY_PAGES {
        let tier = Tier::pages(pages.into());
        for &n in COPY_LENGTHS {
            let value: Vec<u8> = (0..n).map(|i| (i * 13 + 17) as u8).collect();
            for offset in [0usize, 1, 16, 32] {
                let capacities: BTreeSet<_> = [n.saturating_sub(1), n, n + 1].into();
                for cap in capacities {
                    let mut initial = vec![0xa5u8; 128];
                    initial[..32].copy_from_slice(&key);
                    let body = format!("i32.const 128 i32.const 0 i32.const {offset} i32.const {cap} call $storage_get i32.store
                        i32.const 0 i32.const 132 call $return");
                    let text = module(&[
                        &import("storage_get"),
                        &import("return"),
                        &memory(pages, pages),
                        &data(0, &initial),
                        &invoke(&body),
                    ]);
                    let fits = cap >= n;
                    if fits {
                        initial[offset..offset + n].copy_from_slice(&value);
                    }
                    initial.extend((n as u32).to_le_bytes());
                    case(&text)
                        .named(format!(
                            "aliased-get/pages-{pages}/len-{n}/offset-{offset}/cap-{cap}"
                        ))
                        .with_storage(TARGET, [(key, &value)])
                        .call()
                        .result(initial)
                        .charged(Ledger {
                            dispatch: cost_at(&body, tier),
                            copies: words(32) + if fits { words(n as u64) } else { 0 },
                            spans: 1 + u64::from(fits && n != 0),
                            reads: 1,
                            accesses: 1,
                            ..Default::default()
                        })
                        .run();
                }
            }
        }
    }
}
