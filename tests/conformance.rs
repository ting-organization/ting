//! The reference implementation against the consensus vectors in
//! `conformance/vectors/` (wiki: reference/conformance): every file of every
//! group is read and checked, and a file the runner does not know fails it.
//! The vectors' expectations come from the specification (`tests/vectors.rs`
//! and `tests/vm/consensus.rs`), never from this code.
use num_bigint::BigUint;
use serde_json::Value;
use std::{fs, path::Path};
use ting_core::{
    consensus::{Ancestry, active_limits, adjust, asert, vote_direction, work},
    hash::{hash, vector_root},
    meter::{Exhausted, Meter, Resources},
    protocol::{Block, Genesis, Header, Transaction},
    state::{State, record_key},
    vm::{
        Context, Runtime,
        price::{Tier, code, sha, stack_pages},
        profile,
    },
};

fn bytes(v: &Value) -> Vec<u8> {
    let s = v.as_str().expect("hex");
    assert_eq!(s.len() % 2, 0);
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}
fn hash32(v: &Value) -> [u8; 32] {
    bytes(v).try_into().expect("32 bytes")
}
/// An integer written as a JSON number or, past 2^53, as decimal digits.
fn int(v: &Value) -> i128 {
    match v {
        Value::Number(n) => n.as_i64().map(i128::from).expect("an integer"),
        Value::String(s) => s.parse().expect("decimal digits"),
        _ => panic!("an integer: {v}"),
    }
}
fn uint(v: &Value) -> u64 {
    u64::try_from(int(v)).expect("an unsigned integer")
}
fn big(v: &Value) -> BigUint {
    match v {
        Value::Number(_) => BigUint::from(uint(v)),
        Value::String(s) => s.parse().expect("decimal digits"),
        _ => panic!("an integer: {v}"),
    }
}
fn target(v: &Value) -> BigUint {
    BigUint::from_bytes_be(&bytes(v))
}
fn counters(v: &Value) -> [u64; 4] {
    let list: Vec<u64> = v
        .as_array()
        .expect("four counters")
        .iter()
        .map(uint)
        .collect();
    list.try_into().expect("four counters")
}
fn profile(name: &Value) -> Genesis {
    match name.as_str() {
        Some("development") => Genesis::development(),
        Some("experimental") => Genesis::experimental(),
        _ => panic!("profile {name}"),
    }
}

/// The vectors of a group: their file names and contents.
fn group(name: &str) -> Vec<(String, Value)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("conformance/vectors")
        .join(name);
    let mut files: Vec<(String, Value)> = fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("{name} has no vectors"))
        .map(|e| {
            let path = e.unwrap().path();
            let text = fs::read_to_string(&path).unwrap();
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["group"], name, "{}", path.display());
            (
                path.file_stem().unwrap().to_string_lossy().into_owned(),
                value,
            )
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!files.is_empty(), "{name} has no vectors");
    files
}

#[test]
fn v1_primitives() {
    for (name, v) in group("v1-primitives") {
        let (input, expect) = (&v["input"], &v["expect"]);
        if name.starts_with("genesis-") {
            let config = bytes(&expect["config_hex"]);
            let g = Genesis::decode(&config).expect(&name);
            assert_eq!(g.timestamp, uint(&input["timestamp"]), "{name}");
            assert_eq!(
                g.initial_target,
                target(&input["initial_target_hex"]),
                "{name}"
            );
            assert_eq!(g.pow_limit, target(&input["pow_limit_hex"]), "{name}");
            assert_eq!(u64::from(g.interval), uint(&input["interval"]), "{name}");
            assert_eq!(u64::from(g.half_life), uint(&input["half_life"]), "{name}");
            assert_eq!(
                u64::from(g.vote_epoch),
                uint(&input["vote_epoch"]),
                "{name}"
            );
            assert_eq!(g.initial.0, counters(&input["initial"]), "{name}");
            assert_eq!(g.minimum.0, counters(&input["minimum"]), "{name}");
            use ting_core::protocol::Encode;
            assert_eq!(profile(&input["profile"]).encode(), config, "{name}");
            assert_eq!(g.chain_id(), hash32(&expect["chain_id"]), "{name}");
            assert_eq!(g.block_id(), hash32(&expect["genesis_block_id"]), "{name}");
        } else if name.starts_with("hash-") {
            let tag = input["tag"].as_str().unwrap();
            assert_eq!(
                hash(tag, &bytes(&input["bytes_hex"])),
                hash32(&expect["hash"]),
                "{name}"
            );
        } else if name.contains("-root-") {
            let tag = input["tag"].as_str().unwrap();
            let items: Vec<Vec<u8>> = input["items_hex"]
                .as_array()
                .unwrap()
                .iter()
                .map(bytes)
                .collect();
            assert_eq!(vector_root(tag, &items), hash32(&expect["root"]), "{name}");
        } else if name == "record-key" {
            let key = record_key(
                u8::try_from(uint(&input["namespace"])).unwrap(),
                hash32(&input["contract"]),
                hash32(&input["logical_key"]),
            );
            assert_eq!(key, hash32(&expect["key"]), "{name}");
        } else if name.starts_with("state-") {
            let mut state = State::default();
            for write in input["writes"].as_array().unwrap() {
                let value = if write["delete"] == true {
                    None
                } else {
                    Some(bytes(&write["value_hex"]))
                };
                state.set(hash32(&write["key"]), value);
            }
            assert_eq!(state.root(), hash32(&expect["root"]), "{name}");
        } else {
            panic!("an unknown v1 vector: {name}");
        }
    }
}

#[test]
fn v2_envelopes() {
    let chain = Genesis::development().chain_id();
    for (name, v) in group("v2-envelopes") {
        let tx = bytes(&v["input"]["transaction_hex"]);
        let decoded = Transaction::decode(&tx, usize::MAX);
        let valid = v["expect"]["valid"] == true;
        assert_eq!(decoded.is_ok(), valid, "{name}: {decoded:?}");
        if let Ok(t) = decoded {
            assert_eq!(
                t.id(chain),
                hash32(&v["expect"]["id_on_development"]),
                "{name}"
            );
        }
    }
}

#[test]
fn v5_difficulty() {
    for (name, v) in group("v5-difficulty") {
        let i = &v["input"];
        let next = asert(
            &target(&i["anchor_target_hex"]),
            u32::try_from(uint(&i["interval"])).unwrap(),
            u32::try_from(uint(&i["half_life"])).unwrap(),
            uint(&i["blocks"]),
            int(&i["elapsed"]),
            &target(&i["pow_limit_hex"]),
        );
        assert_eq!(next, target(&v["expect"]["target_hex"]), "{name}");
    }
}

#[test]
fn v6_voting() {
    for (name, v) in group("v6-voting") {
        let (input, expect) = (&v["input"], &v["expect"]);
        if name.starts_with("direction-") {
            let signals: Vec<(BigUint, u8)> = input["work_and_signal"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pair| (big(&pair[0]), u8::try_from(uint(&pair[1])).unwrap()))
                .collect();
            assert_eq!(
                u64::from(vote_direction(&signals)),
                uint(&expect["direction"]),
                "{name}"
            );
        } else if name.starts_with("step-") {
            let next = adjust(
                uint(&input["limit"]),
                u8::try_from(uint(&input["direction"])).unwrap(),
                uint(&input["floor"]),
            );
            assert_eq!(next, uint(&expect["limit"]), "{name}");
        } else if name.starts_with("activation-") {
            let g = profile(&input["profile"]);
            let headers: Vec<Header> = input["headers"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, h)| Header {
                    chain_id: g.chain_id(),
                    height: i as u64 + 1,
                    parent: [0; 32],
                    timestamp: g.timestamp,
                    target: target(&h["target_hex"]),
                    transactions_root: [0; 32],
                    receipts_root: [0; 32],
                    state_root: [0; 32],
                    used: Resources::ZERO,
                    votes: counters_u8(&h["votes"]),
                    producer_hash: [0; 32],
                    nonce: 0,
                })
                .collect();
            let epoch = g.vote_epoch as usize;
            for (e, limits) in expect["active_limits_after_each_epoch"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                assert_eq!(
                    active_limits(&g, &headers[..e * epoch]).0,
                    counters(limits),
                    "{name}, after epoch {e}"
                );
            }
        } else {
            panic!("an unknown v6 vector: {name}");
        }
    }
}
fn counters_u8(v: &Value) -> [u8; 4] {
    counters(v).map(|c| u8::try_from(c).unwrap())
}

#[test]
fn v8_work() {
    for (name, v) in group("v8-work") {
        let (input, expect) = (&v["input"], &v["expect"]);
        let computed = if let Some(targets) = input["targets_hex"].as_array() {
            targets.iter().map(|t| work(&target(t))).sum()
        } else {
            work(&target(&input["target_hex"]))
        };
        assert_eq!(computed, big(&expect["work"]), "{name}");
    }
}

#[test]
fn v9_tables() {
    for (name, v) in group("v9-tables") {
        let (input, expect) = (&v["input"], &v["expect"]);
        match name.as_str() {
            "memory-prices" => {
                let mut next = 0;
                for run in expect["runs"].as_array().unwrap() {
                    let (from, to) = (uint(&run["pages_from"]), uint(&run["pages_to"]));
                    assert_eq!(from, next, "the runs cover every page count");
                    for pages in from..=to {
                        let tier = Tier::of(pages * 65_536);
                        let stated =
                            [&run["load"], &run["store"], &run["word"], &run["span"]].map(uint);
                        assert_eq!(
                            [tier.load, tier.store, tier.word, tier.span],
                            stated,
                            "{pages} pages"
                        );
                    }
                    next = to + 1;
                }
                assert_eq!(next, 65_537);
            }
            "hashing" => {
                for (m, c) in input["lengths"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .zip(expect["compute"].as_array().unwrap())
                {
                    assert_eq!(sha(uint(m)), uint(c), "sha of {m}");
                }
            }
            "stack-pages" => {
                for (h, p) in input["heights"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .zip(expect["pages"].as_array().unwrap())
                {
                    assert_eq!(stack_pages(uint(h)), uint(p), "height {h}");
                }
            }
            _ if name.starts_with("meter-") => {
                let mut meter = Meter::new(Resources(counters(&input["root_budget"])));
                for (k, step) in expect["steps"].as_array().unwrap().iter().enumerate() {
                    let outcome = &step["outcome"];
                    if step.get("charge").is_some() {
                        let got = match meter.charge(Resources(counters(&step["charge"]))) {
                            Ok(()) => "ok",
                            Err(Exhausted(i)) => ["C", "S", "D", "G"][i],
                        };
                        assert_eq!(outcome, got, "{name}, step {k}");
                    } else if step.get("open").is_some() {
                        let opened = meter.push(Resources(counters(&step["open"])));
                        assert_eq!(
                            outcome,
                            if opened { "opened" } else { "refused" },
                            "{name}, step {k}"
                        );
                    } else {
                        assert_eq!(meter.pop().0, counters(outcome), "{name}, step {k}");
                    }
                    assert_eq!(
                        meter.used().0,
                        counters(&step["root_used_after"]),
                        "{name}, step {k}"
                    );
                }
            }
            _ => panic!("an unknown v9 vector: {name}"),
        }
    }
}

/// The base the header and block vectors build on: the blocks of the
/// development seed's first section.
fn base() -> Vec<Block> {
    let (_, v) = group("base").into_iter().next().unwrap();
    assert_eq!(v["input"]["profile"], "development");
    v["expect"]["blocks_hex"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| Block::decode(&bytes(b), usize::MAX).unwrap())
        .collect()
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
            .unwrap_or_else(|e| panic!("base block {}: {}", block.header.height, e.0));
        history.push(block.header.clone());
    }
    (runtime, history)
}
/// Each vector of a block group is the next block on the first `on` blocks
/// of the base: decoded within the active data capacity and validated, it
/// is accepted or rejected as stated, and a rejected one leaves the state.
fn blocks_of(name: &str) {
    let base = base();
    let g = Genesis::development();
    for (vector, v) in group(name) {
        let height = usize::try_from(uint(&v["input"]["on"])).unwrap();
        let (runtime, history) = chain(&base[..height]);
        let limits = active_limits(&g, &history);
        let before = runtime.state_root();
        let block = bytes(&v["input"]["block_hex"]);
        let outcome = Block::decode(&block, limits.0[2] as usize).and_then(|block| {
            runtime.validate_and_apply_block_at(&g, &Ancestry::of(&history), &block, limits)
        });
        let valid = v["expect"]["valid"] == true;
        assert_eq!(outcome.is_ok(), valid, "{vector}: {outcome:?}");
        if !valid {
            assert_eq!(
                runtime.state_root(),
                before,
                "{vector}: the state after a rejection"
            );
        }
    }
}

#[test]
fn the_base_is_a_valid_beginning_of_the_development_chain() {
    let base = base();
    assert!(!base.is_empty());
    chain(&base);
}
#[test]
fn v3_headers() {
    blocks_of("v3-headers");
}
#[test]
fn v4_blocks() {
    blocks_of("v4-blocks");
}

/// A module from its recipe: the preamble, then each section's id, its
/// payload's length and the payload, made of bytes and of bytes repeated.
fn module(recipe: &Value) -> Vec<u8> {
    let mut out = bytes(&recipe["preamble_hex"]);
    for section in recipe["sections"].as_array().unwrap() {
        let mut payload = Vec::new();
        for part in section["payload"].as_array().unwrap() {
            if part.get("hex").is_some() {
                payload.extend(bytes(&part["hex"]));
            } else {
                let item = bytes(&part["repeat_hex"]);
                for _ in 0..uint(&part["times"]) {
                    payload.extend(&item);
                }
            }
        }
        out.push(u8::try_from(uint(&section["id"])).unwrap());
        let mut n = payload.len() as u64;
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
        out.extend(payload);
    }
    out
}

#[test]
fn v7_structural() {
    for (name, v) in group("v7-structural") {
        let module = module(&v["input"]["module"]);
        let verdict = profile::validate(&module);
        let expect = &v["expect"];
        assert_eq!(
            verdict.is_ok(),
            expect["accepted"] == true,
            "{name}: {verdict:?}"
        );
        if let Ok(p) = verdict {
            assert_eq!(
                code(&module),
                uint(&expect["code_price"]),
                "{name}: code price"
            );
            assert_eq!(
                p.preparation(),
                uint(&expect["preparation"]),
                "{name}: preparation"
            );
            assert_eq!(
                p.functions[p.invoke as usize].slots(),
                uint(&expect["entry_slots"]),
                "{name}: the entry's slots"
            );
        }
    }
}
