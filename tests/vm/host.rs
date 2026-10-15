//! The host imports (wiki: vm/host): every span rule, getters
//! that report a length and copy only what fits, the nine context selectors,
//! storage reads, writes and deletions with their keys, leaves, accesses and
//! growth, the code hash of another program, and the three ways an invocation
//! ends.
use crate::{
    dsl::{escaped, import, invoke, memory, module, module_from, wasm},
    harness::{CHILD, GRANDCHILD, TARGET, call_transaction, case, context, entry_of, k},
};
use ting_core::{hash::hash, protocol::Status};
use vm_model::{BUDGET, KEY, Ledger, SMALL, TRANSACTION_BASE, call_size, cost, leaf, words};

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
fn spans_are_validated_before_argument_bytes_are_charged() {
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
fn input_reports_its_length_and_copies_only_when_it_fits() {
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
fn a_getter_span_is_checked_in_full_even_for_an_empty_input() {
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
fn a_zero_length_span_still_needs_a_memory() {
    for host in ["input", "call_result", "return", "revert", "emit"] {
        let calling = format!("i32.const 0 i32.const 0 call ${host}");
        let drop = if has_result(host) { " drop" } else { "" };
        case(&module(&[
            &import(host),
            &invoke(&format!("{calling}{drop}")),
        ]))
        .named(host)
        .call()
        .expect(Status::Trap)
        .charged(dispatch(cost(&calling)))
        .run();
    }
}

#[test]
fn context_selectors_report_the_block_the_transaction_and_the_budget() {
    let ctx = context();
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
    let values: [Vec<u8>; 9] = [
        ctx.chain.to_vec(),
        call_transaction(TARGET, &[]).id(ctx.chain).to_vec(),
        TARGET.to_vec(),
        vec![0; 33],
        ctx.height.to_le_bytes().to_vec(),
        ctx.timestamp.to_le_bytes().to_vec(),
        ctx.parent.to_vec(),
        ctx.producer.clone(),
        remaining,
    ];
    for (selector, value) in values.iter().enumerate() {
        let n = value.len();
        case(&program(selector as i32, n))
            .named(format!("selector {selector}"))
            .call()
            .result(value)
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
fn producer_data_is_copied_only_when_it_fits() {
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
fn producer_length_queries_move_no_bytes() {
    // 20,000 length queries of a 4 MiB producer cost the loop once and its
    // body each round, the host call's own price among it, nothing else: an
    // implementation that copied the producer per query would move eighty
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
        .with_producer(vec![1; 4 << 20])
        .call()
        .charged(dispatch(cost("loop") + 20_000 * cost(round)))
        .run();
}

#[test]
fn storage_get_reports_the_length_and_copies_only_when_it_fits() {
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
fn a_length_query_of_a_large_record_pays_its_key_and_its_access_only() {
    // 1,000 length queries of a 1 MiB record: each pays its instructions, the
    // copy of its key, the key's 1,900 C and one S, nothing in proportion to
    // the value.
    let round = "i32.const 0 i32.const 0 i32.const 0 call $storage_get drop
      local.get $i i32.const 1 i32.add local.tee $i
      i32.const 1000 i32.lt_u br_if $again";
    let text = module(&[
        &import("storage_get"),
        &memory(1, 1),
        &invoke(&format!(
            "(local $i i32)
    (loop $again
      {round})"
        )),
    ]);
    case(&text)
        .with_storage(TARGET, [(k(0), vec![7u8; 1 << 20])])
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
fn a_record_read_debits_its_key_with_its_access() {
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
fn storage_set_charges_the_value_per_word_and_growth_gross() {
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
fn storage_delete_pays_its_two_accesses_whether_or_not_the_record_exists() {
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
fn storage_set_fails_atomically_when_access_or_growth_runs_out() {
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
fn an_empty_record_is_present_until_deleted() {
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
fn code_hash_reads_another_programs_manifest() {
    let child = module(&[&invoke("nop")]);
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
            hash("code", &wasm(&child)).to_vec()
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
fn return_revert_and_emit_charge_their_payload_and_validate_their_span() {
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
