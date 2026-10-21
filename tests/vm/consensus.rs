//! The §7 vectors built on blocks and modules (wiki:
//! reference/conformance): the base, the first section of the development
//! seed, which the block groups build on; V3 headers and V4 blocks, each the
//! next block on the base with exactly one rule broken, beside the valid
//! blocks they are made from; and V7, modules at the structural bounds. A broken block
//! is re-mined after its fault, so that its proof of work holds unless the
//! fault is the proof of work. `tests/conformance.rs` checks the reference
//! implementation against them, and `seed.rs` that the base is the seed's
//! beginning.
//!
//! After a deliberate change, rewrite them and review the difference:
//!
//! ```text
//! TING_WRITE_VECTORS=1 cargo test --test vm consensus
//! ```
use crate::{
    dsl::{import, invoke, memory, module, wasm},
    harness::{self},
    seed::{SECTIONS, build},
    validation::{leb, name, simple, vec_of},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};
use ting_core::{
    consensus::{Ancestry, active_limits, commitments, valid_pow},
    hash::hex,
    meter::Resources,
    protocol::{BLOCK_BASE, Block, Encode, Genesis, Header, Payload, Status, Transaction},
    state::State,
    vm::{Context, Runtime},
};

/// The first section of the development seed: the empty block and the
/// primitives.
pub(crate) fn base() -> Vec<Block> {
    let (section, tests) = SECTIONS[0];
    let mut cases = Vec::new();
    for (_, test) in tests.iter() {
        let mut collected = harness::collect(test);
        for case in &mut collected {
            case.section = section.to_string();
        }
        cases.extend(collected);
    }
    build(&cases).0
}

/// A validator that has applied `blocks`, and their headers.
fn chain(blocks: &[Block]) -> (Runtime, Vec<Header>) {
    let g = Genesis::development();
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut history = Vec::new();
    for block in blocks {
        runtime
            .validate_and_apply_block_at(
                &g,
                &Ancestry::of(&history),
                block,
                active_limits(&g, &history),
            )
            .unwrap();
        history.push(block.header.clone());
    }
    (runtime, history)
}
fn mine(header: &mut Header) {
    header.nonce = 0;
    while !valid_pow(header) {
        header.nonce += 1;
    }
}
/// What the next block on `history` is made of.
#[derive(Default)]
struct Next<'a> {
    transactions: &'a [Transaction],
    timestamp: Option<u64>,
    votes: [u8; 4],
    /// Limits to assemble under instead of the active ones, for a block
    /// that goes past them.
    limits: Option<Resources>,
}
/// The next block on `history`, assembled on `runtime` and mined.
fn next(runtime: &Runtime, history: &[Header], n: Next) -> Block {
    let g = Genesis::development();
    let height = history.len() as u64 + 1;
    let timestamp = n
        .timestamp
        .unwrap_or(g.timestamp + u64::from(g.interval) * height);
    let limits = n.limits.unwrap_or_else(|| active_limits(&g, history));
    let mut candidate = runtime
        .candidate(
            &g,
            &Ancestry::of(history),
            limits,
            timestamp,
            n.votes,
            Vec::new(),
        )
        .unwrap();
    for tx in n.transactions {
        assert!(candidate.add(tx).unwrap().is_some(), "the transaction fits");
    }
    let mut block = candidate.finish().unwrap();
    mine(&mut block.header);
    block
}
/// The median the next timestamp must pass: of the last eleven, with the
/// genesis timestamp among them while there are fewer (rules
/// §9.1).
fn median(history: &[Header]) -> u64 {
    let mut times: Vec<u64> = history.iter().rev().take(11).map(|h| h.timestamp).collect();
    if times.len() < 11 {
        times.push(Genesis::development().timestamp);
    }
    times.sort_unstable();
    times[times.len() / 2]
}

fn vector(group: &str, rule: &str, rules: &str, input: Value, expect: Value) -> Value {
    json!({ "group": group, "rule": rule, "rules": rules, "input": input, "expect": expect })
}
fn on(group: &str, rule: &str, rules: &str, height: usize, bytes: &[u8], valid: bool) -> Value {
    vector(
        group,
        rule,
        rules,
        json!({ "on": height, "block_hex": hex(bytes) }),
        json!({ "valid": valid }),
    )
}

/// A deployment of `text` with `input` under `budget`.
fn deployment(text: &str, input: &[u8], budget: [u64; 4]) -> Transaction {
    Transaction {
        budget: Resources(budget),
        payload: Payload::Deploy {
            code: wasm(text),
            input: input.to_vec(),
        },
    }
}

fn headers(base: &[Block]) -> Vec<(String, Value)> {
    const G: &str = "v3-headers";
    const VALIDITY: &str = "4.2";
    let mut v = Vec::new();
    let tip = base.len();
    let (runtime, history) = chain(base);
    let valid = next(&runtime, &history, Next::default());
    let encode = |b: &Block| b.encode();
    v.push((
        "an-empty-block".to_string(),
        on(
            G,
            "the next block, empty",
            VALIDITY,
            tip,
            &encode(&valid),
            true,
        ),
    ));
    let broken = |name: &str, rule: &str, fault: &dyn Fn(&mut Header), remine: bool| {
        let mut b = valid.clone();
        fault(&mut b.header);
        if remine {
            mine(&mut b.header);
        }
        (
            name.to_string(),
            on(G, rule, VALIDITY, tip, &b.encode(), false),
        )
    };
    v.push(broken(
        "another-chain",
        "the header names the chain",
        &|h| h.chain_id = [0x5a; 32],
        true,
    ));
    v.push(broken(
        "another-parent",
        "the header names its parent",
        &|h| h.parent = [0x5a; 32],
        true,
    ));
    v.push(broken(
        "a-height-too-high",
        "the height is the parent's plus one",
        &|h| h.height += 1,
        true,
    ));
    v.push(broken(
        "a-height-too-low",
        "the height is the parent's plus one",
        &|h| h.height -= 1,
        true,
    ));
    v.push(broken(
        "a-target-one-above",
        "the target is the one ASERT gives",
        &|h| h.target += 1u32,
        true,
    ));
    v.push(broken(
        "a-target-one-below",
        "the target is the one ASERT gives",
        &|h| h.target -= 1u32,
        true,
    ));
    v.push(broken(
        "a-vote-of-3",
        "each vote is 0, 1 or 2",
        &|h| h.votes = [3, 0, 0, 0],
        true,
    ));
    v.push(broken(
        "proof-of-work-above-the-target",
        "the block id, as a number, is at most the target",
        &|h| {
            h.nonce = 0;
            while valid_pow(h) {
                h.nonce += 1;
            }
        },
        false,
    ));
    let voting = next(
        &runtime,
        &history,
        Next {
            votes: [1, 2, 0, 1],
            ..Default::default()
        },
    );
    v.push((
        "votes-of-1-and-2".to_string(),
        on(
            G,
            "each vote is 0, 1 or 2",
            VALIDITY,
            tip,
            &voting.encode(),
            true,
        ),
    ));
    let mut version = valid.encode();
    version[0] = 2;
    v.push((
        "header-version-2".to_string(),
        on(
            G,
            "the header's version is 1",
            VALIDITY,
            tip,
            &version,
            false,
        ),
    ));
    // The median of the last eleven timestamps, with the genesis timestamp
    // among them early on: a timestamp at it fails and one past it holds,
    // even when it is earlier than the parent's.
    for height in [1, 2, 5, tip] {
        let (runtime, history) = chain(&base[..height]);
        let m = median(&history);
        for (name, timestamp, valid) in [("at", m, false), ("one-past", m + 1, true)] {
            let block = next(
                &runtime,
                &history,
                Next {
                    timestamp: Some(timestamp),
                    ..Default::default()
                },
            );
            v.push((
                format!("timestamp-{name}-the-median-after-{height}-blocks"),
                on(
                    G,
                    "the timestamp is past the median of the last eleven, the genesis timestamp among them while there are fewer",
                    "9.1",
                    height,
                    &block.encode(),
                    valid,
                ),
            ));
        }
    }
    v
}

fn blocks(base: &[Block]) -> Vec<(String, Value)> {
    const G: &str = "v4-blocks";
    const BODY: &str = "4.3";
    let mut v = Vec::new();
    let tip = base.len();
    let (runtime, history) = chain(base);
    let g = Genesis::development();
    // A call of nothing, a deployment and a call of what it deployed.
    let nop = module(&[&memory(1, 1), &invoke("nop")]);
    let deploy = deployment(&nop, b"block vectors", [1_000_000, 16, 4096, 4096]);
    let deployed = harness::identifier(&deploy, 0);
    let call = |target| Transaction {
        budget: Resources([1_000_000, 16, 4096, 4096]),
        payload: Payload::Call {
            target,
            input: Vec::new(),
        },
    };
    let transactions = [call([0x5a; 32]), deploy, call(deployed)];
    let valid = next(
        &runtime,
        &history,
        Next {
            transactions: &transactions,
            ..Default::default()
        },
    );
    assert_eq!(valid.receipts[0].status, Status::Missing);
    assert_eq!(valid.receipts[2].status, Status::Success);
    v.push((
        "three-transactions".to_string(),
        on(
            G,
            "the next block, with transactions",
            BODY,
            tip,
            &valid.encode(),
            true,
        ),
    ));
    let bytes = valid.encode();
    let plain = |name: &str, rule: &str, bytes: Vec<u8>| {
        (name.to_string(), on(G, rule, BODY, tip, &bytes, false))
    };
    v.push(plain(
        "cut-by-a-byte",
        "every field is complete",
        bytes[..bytes.len() - 1].to_vec(),
    ));
    v.push(plain(
        "a-trailing-byte",
        "nothing follows the last receipt",
        [&bytes[..], &[0]].concat(),
    ));
    // The count follows the header and the producer data's length.
    let mut count = bytes.clone();
    count[294 + 4..294 + 8].copy_from_slice(&4u32.to_le_bytes());
    v.push(plain(
        "one-transaction-more-than-the-body-holds",
        "the count is the number of transactions and of receipts",
        count,
    ));
    let mut producer = valid.clone();
    producer.producer = b"not committed".to_vec();
    v.push(plain(
        "producer-data-not-committed",
        "the header commits to the producer data",
        producer.encode(),
    ));
    // Faults the header carries, re-mined; for a receipt, its commitment and
    // the header's total follow it, so that only the execution disagrees.
    let header_fault = |fault: &dyn Fn(&mut Block)| {
        let mut b = valid.clone();
        fault(&mut b);
        mine(&mut b.header);
        b.encode()
    };
    let recommit = |b: &mut Block| {
        let (t, r) = commitments(&b.transactions, &b.receipts);
        b.header.transactions_root = t;
        b.header.receipts_root = r;
        let mut used = Resources([0, 0, BLOCK_BASE + b.producer.len() as u64, 0]);
        for receipt in &b.receipts {
            for i in 0..4 {
                used.0[i] += receipt.used.0[i];
            }
        }
        b.header.used = used;
    };
    let fault = |name: &str, rule: &str, rules: &str, bytes: Vec<u8>| {
        (name.to_string(), on(G, rule, rules, tip, &bytes, false))
    };
    v.push(fault(
        "another-transactions-root",
        "the header commits to the transactions",
        "2.5",
        header_fault(&|b| b.header.transactions_root = [0x5a; 32]),
    ));
    v.push(fault(
        "another-receipts-root",
        "the header commits to the receipts",
        "2.5",
        header_fault(&|b| b.header.receipts_root = [0x5a; 32]),
    ));
    v.push(fault(
        "another-state-root",
        "the header commits to the state after the block",
        "6.2",
        header_fault(&|b| b.header.state_root = [0x5a; 32]),
    ));
    v.push(fault(
        "one-more-compute-used",
        "the header's resources used are the receipts' totals with the block's data",
        BODY,
        header_fault(&|b| b.header.used.0[0] += 1),
    ));
    v.push(fault(
        "a-receipt-charging-one-more",
        "each receipt is the one executing its transaction gives",
        "4.5",
        header_fault(&|b| {
            b.receipts[1].used.0[0] += 1;
            recommit(b);
        }),
    ));
    v.push(fault(
        "a-receipt-with-another-status",
        "each receipt is the one executing its transaction gives",
        "4.5",
        header_fault(&|b| {
            b.receipts[0].status = Status::Trap;
            recommit(b);
        }),
    ));
    v.push(fault(
        "a-receipt-with-a-result",
        "each receipt is the one executing its transaction gives",
        "4.5",
        header_fault(&|b| {
            b.receipts[2].result = b"x".to_vec();
            recommit(b);
        }),
    ));
    // Blocks whose transactions together go past the capacity in one
    // resource, assembled under larger limits and consistent otherwise.
    let active = active_limits(&g, &history);
    let unlimited = Resources([u64::MAX; 4]);
    let burner = module(&[
        &memory(1, 1),
        &invoke("(loop i32.const 0 i32.const 0 i32.const 65536 memory.fill br 0)"),
    ]);
    let compute: Vec<Transaction> = (0..2u8)
        .map(|i| deployment(&burner, &[i], [active.0[0] / 10 * 6, 16, 4096, 4096]))
        .collect();
    let accesses: Vec<Transaction> = (0..=(active.0[1] / 7) as u32)
        .map(|i| deployment(&nop, &i.to_le_bytes(), [1_000_000, 16, 4096, 4096]))
        .collect();
    let emitter = module(&[
        &import("emit"),
        &memory(1, 1),
        &invoke("i32.const 0 i32.const 60000 call $emit unreachable"),
    ]);
    let data: Vec<Transaction> = (0..=(active.0[2] / 60_000) as u8)
        .map(|i| deployment(&emitter, &[i], [10_000_000, 16, 70_000, 4096]))
        .collect();
    let writer = module(&[
        &import("storage_set"),
        &memory(2, 2),
        &invoke("i32.const 0 i32.const 32 i32.const 120000 call $storage_set"),
    ]);
    let growth: Vec<Transaction> = (0..=(active.0[3] / 120_000) as u8)
        .map(|i| deployment(&writer, &[i], [100_000_000, 16, 4096, 130_000]))
        .collect();
    for (name, transactions, i) in [
        ("compute-past-the-capacity", compute, 0),
        ("accesses-past-the-capacity", accesses, 1),
        ("data-past-the-capacity", data, 2),
        ("growth-past-the-capacity", growth, 3),
    ] {
        let block = next(
            &runtime,
            &history,
            Next {
                transactions: &transactions,
                limits: Some(unlimited),
                ..Default::default()
            },
        );
        assert!(block.header.used.0[i] > active.0[i], "{name}");
        assert!(
            (0..4).all(|j| j == i || block.header.used.0[j] <= active.0[j]),
            "{name} goes past one resource alone"
        );
        v.push(fault(
            name,
            "a block's resources used stay within the active capacity",
            "14.1",
            block.encode(),
        ));
    }
    v
}

/// A module as its sections, each payload as parts: bytes, or bytes
/// repeated, so that a module of megabytes is a few lines.
struct Recipe(Vec<(u8, Vec<Part>)>);
enum Part {
    Bytes(Vec<u8>),
    Repeat(Vec<u8>, usize),
}
impl Recipe {
    fn module(&self) -> Vec<u8> {
        let mut out = b"\0asm\x01\0\0\0".to_vec();
        for (id, parts) in &self.0 {
            let mut payload: Vec<u8> = Vec::new();
            for part in parts {
                match part {
                    Part::Bytes(b) => payload.extend(b),
                    Part::Repeat(b, n) => {
                        for _ in 0..*n {
                            payload.extend(b);
                        }
                    }
                }
            }
            out.push(*id);
            out.extend(leb(payload.len() as u64));
            out.extend(payload);
        }
        out
    }
    fn json(&self) -> Value {
        json!({
            "preamble_hex": "0061736d01000000",
            "sections": self.0.iter().map(|(id, parts)| json!({
                "id": id,
                "payload": parts.iter().map(|p| match p {
                    Part::Bytes(b) => json!({ "hex": hex(b) }),
                    Part::Repeat(b, n) => json!({ "repeat_hex": hex(b), "times": n }),
                }).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }
}

/// Modules at the profile's structural bounds and one past each, whose
/// verdict follows from the bound; an accepted one with its code price, read
/// from its framing by the model, its preparation, stated from the rule, and
/// its entry function's reserved slots.
fn structural() -> Vec<(String, Value)> {
    const G: &str = "v7-structural";
    const BOUNDS: &str = "18.3";
    let whole = |payload: Vec<u8>| vec![Part::Bytes(payload)];
    let simple = || -> Vec<(u8, Vec<Part>)> {
        simple()
            .into_iter()
            .map(|(id, p)| (id, vec![Part::Bytes(p)]))
            .collect()
    };
    // A vector of `n` copies of `item`.
    let many =
        |item: &[u8], n: usize| vec![Part::Bytes(leb(n as u64)), Part::Repeat(item.to_vec(), n)];
    let mut v = Vec::new();
    let mut add = |name: String, rule: &str, recipe: Recipe, accepted: bool, preparation: u64| {
        let module = recipe.module();
        let expect = if accepted {
            json!({
                "accepted": true,
                "code_price": vm_model::code_price(&module),
                "preparation": preparation,
                "entry_slots": 0,
            })
        } else {
            json!({ "accepted": false })
        };
        v.push((
            name,
            vector(G, rule, BOUNDS, json!({ "module": recipe.json() }), expect),
        ));
    };
    let start = vm_model::START;
    for n in [1_000_000, 1_000_001] {
        let mut sections = simple();
        sections[0].1 = many(&[0x60, 0x00, 0x00], n);
        add(
            format!("types-{n}"),
            "a module declares at most 1,000,000 types",
            Recipe(sections),
            n <= 1_000_000,
            start,
        );
    }
    // Functions count the imports: one import and `n` defined functions, the
    // last the entry.
    let (params, result) = ting_core::vm::profile::signature("revert").unwrap();
    let mut revert = vec![0x60];
    revert.extend(leb(params as u64));
    revert.extend(vec![0x7f; params]);
    revert.extend(if result { vec![0x01, 0x7f] } else { vec![0x00] });
    let mut import = name("ting");
    import.extend(name("revert"));
    import.extend([0x00, 0x01]);
    for n in [999_999, 1_000_000] {
        let mut export = name("invoke");
        export.push(0x00);
        export.extend(leb(n as u64));
        let sections = vec![
            (1, whole(vec_of(&[vec![0x60, 0x00, 0x00], revert.clone()]))),
            (2, whole(vec_of(std::slice::from_ref(&import)))),
            (3, many(&[0x00], n)),
            (7, whole(vec_of(&[export]))),
            (10, many(&[0x02, 0x00, 0x0b], n)),
        ];
        add(
            format!("an-import-and-{n}-functions"),
            "a module has at most 1,000,000 functions, its imports among them",
            Recipe(sections),
            n < 1_000_000,
            start,
        );
    }
    for n in [1_000_000, 1_000_001] {
        let mut sections = simple();
        sections.insert(2, (6, many(&[0x7f, 0x00, 0x41, 0x00, 0x0b], n)));
        add(
            format!("globals-{n}"),
            "a module declares at most 1,000,000 globals",
            Recipe(sections),
            n <= 1_000_000,
            start + vm_model::GLOBAL * n as u64,
        );
    }
    let mut invoke = name("invoke");
    invoke.extend([0x00, 0x00]);
    let mut exported_memory = name("memory");
    exported_memory.extend([0x02, 0x00]);
    for n in [100_000, 100_001] {
        let mut sections = simple();
        sections.insert(2, (5, whole(vec_of(&[vec![0x01, 0x01, 0x01]]))));
        sections[3].1 = whole(vec_of(&[invoke.clone(), exported_memory.clone()]));
        sections.push((11, many(&[0x00, 0x41, 0x00, 0x0b, 0x00], n)));
        add(
            format!("data-segments-{n}"),
            "a module declares at most 100,000 data segments",
            Recipe(sections),
            n <= 100_000,
            start + vm_model::PAGE + vm_model::DATA_SEGMENT * n as u64,
        );
    }
    for n in [100_000, 100_001] {
        let mut sections = simple();
        sections.insert(2, (4, whole(vec_of(&[vec![0x70, 0x01, 0x01, 0x01]]))));
        sections.insert(4, (9, many(&[0x00, 0x41, 0x00, 0x0b, 0x01, 0x00], n)));
        add(
            format!("element-segments-{n}"),
            "a module declares at most 100,000 element segments",
            Recipe(sections),
            n <= 100_000,
            start + vm_model::TABLE_ENTRY,
        );
    }
    // A body of 7,654,322 bytes: its local declarations, nops and the end.
    let nops = 7_654_320;
    let mut head = leb(1);
    head.extend(leb(1 + nops as u64 + 1));
    head.extend(leb(0));
    let mut sections = simple();
    sections[3].1 = vec![
        Part::Bytes(head),
        Part::Repeat(vec![0x01], nops),
        Part::Bytes(vec![0x0b]),
    ];
    add(
        "a-function-body-of-7654322-bytes".to_string(),
        "a function body may be 7,654,322 bytes long",
        Recipe(sections),
        true,
        start,
    );
    v
}

#[test]
fn the_block_vectors_follow_from_the_base() {
    let base = base();
    let mut groups: BTreeMap<&str, Vec<(String, Value)>> = BTreeMap::new();
    groups.insert(
        "base",
        vec![(
            "development".to_string(),
            vector(
                "base",
                "the first section of the development seed, the history the header and block vectors build on",
                "27",
                json!({ "profile": "development" }),
                json!({ "blocks_hex": base.iter().map(|b| hex(&b.encode())).collect::<Vec<_>>() }),
            ),
        )],
    );
    groups.insert("v3-headers", headers(&base));
    groups.insert("v4-blocks", blocks(&base));
    groups.insert("v7-structural", structural());
    write_or_check(&groups);
}

/// Writes the groups' files with `TING_WRITE_VECTORS`, and otherwise fails
/// where a file differs from what the generator makes.
pub(crate) fn write_or_check(groups: &BTreeMap<&str, Vec<(String, Value)>>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/vectors");
    let rewrite = std::env::var_os("TING_WRITE_VECTORS").is_some();
    for (group, vectors) in groups {
        let dir = root.join(group);
        let files: BTreeMap<String, String> = vectors
            .iter()
            .map(|(name, value)| {
                (
                    format!("{name}.json"),
                    serde_json::to_string_pretty(value).unwrap() + "\n",
                )
            })
            .collect();
        assert_eq!(files.len(), vectors.len(), "{group}: a name twice");
        if rewrite {
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            for (name, text) in &files {
                fs::write(dir.join(name), text).unwrap();
            }
            continue;
        }
        let mut published: Vec<String> = fs::read_dir(&dir)
            .unwrap_or_else(|_| panic!("{group} is missing; write it with TING_WRITE_VECTORS=1"))
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        published.sort();
        assert_eq!(
            published,
            files.keys().cloned().collect::<Vec<_>>(),
            "{group}: the files differ; after a deliberate change, rewrite them with TING_WRITE_VECTORS=1"
        );
        for (name, text) in &files {
            let on_disk = fs::read_to_string(dir.join(name))
                .unwrap()
                .replace("\r\n", "\n");
            assert!(
                on_disk == *text,
                "{group}/{name} differs; after a deliberate change, rewrite it with TING_WRITE_VECTORS=1"
            );
        }
    }
}
