//! The published protocol vectors, `node/wiki/reference/protocol-vectors.json`,
//! worked out again from the wiki's formulas (wiki: reference/conformance).
//! Nothing here uses `ting_core`: this file holds the vectors to the
//! specification and `tests/protocol.rs` holds the code to the vectors, so the
//! code and the specification can only agree through them.
//!
//! After a deliberate change to the protocol, rewrite the file and review the
//! difference:
//!
//! ```text
//! TING_WRITE_VECTORS=1 cargo test --test vectors
//! ```
use num_bigint::BigUint;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

const HEADER_LEN: usize = 294;
const BLOCK_BASE: u64 = HEADER_LEN as u64 + 8;
const TRANSACTION_BASE: u64 = 81;
const RECORD_BASE: u64 = 36;
const EVENT_BASE: u64 = 36;
/// A SHA-256 over an `m`-byte message (wiki: resources/prices#hashing).
const fn sha(m: u64) -> u64 {
    100 + 900 * (m + 9).div_ceil(64)
}
/// A record key's hash, paid with every record read: "Ting/", the tag length,
/// "state-key", the namespace, the contract and the logical key.
const KEY: u64 = sha(5 + 2 + 9 + 1 + 32 + 32);
const TIMESTAMP: u64 = 1_700_000_000;

#[test]
fn the_published_vectors_follow_from_the_specification() {
    let mut text = String::new();
    vectors().write(0, &mut text);
    text.push('\n');
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("node/wiki/reference/protocol-vectors.json");
    if std::env::var_os("TING_WRITE_VECTORS").is_some() {
        fs::write(&path, &text).unwrap();
        return;
    }
    let published = fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
    if let Some(line) = (0..published.lines().count().max(text.lines().count()))
        .find(|i| published.lines().nth(*i) != text.lines().nth(*i))
    {
        panic!(
            "protocol-vectors.json differs from the specification at line {}:\n  published: {}\n  specified: {}\n\
             After a deliberate protocol change, rewrite it with TING_WRITE_VECTORS=1.",
            line + 1,
            published.lines().nth(line).unwrap_or("(end of file)"),
            text.lines().nth(line).unwrap_or("(end of file)")
        );
    }
}

#[test]
fn the_difficulty_curve_is_two_to_the_power_and_rounds_down() {
    // The cubic stays within 0.013% of 65536 * 2^(frac / 65536), never falls,
    // and starts at 65536 and stays below twice that.
    let mut last = 0;
    for frac in 0..65_536u128 {
        let f = factor(frac);
        let exact = 65_536.0 * 2f64.powf(frac as f64 / 65_536.0);
        assert!(
            (f as f64 - exact).abs() / exact < 1.3e-4,
            "{frac}: {f} vs {exact}"
        );
        assert!(f >= last);
        last = f;
    }
    assert_eq!(factor(0), 65_536);
    assert!(factor(65_535) < 131_072);
    // One second ahead of schedule with a 600 s half-life: the exponent is
    // floor(-65536 / 600) = -110, where aserti3-2d's truncation gives -109.
    let anchor = BigUint::from(1u32) << 200usize;
    let floored = (&anchor * factor(65_536 - 110)) >> 17usize;
    let truncated = (&anchor * factor(65_536 - 109)) >> 17usize;
    assert_ne!(floored, truncated);
    assert_eq!(asert(&anchor, 60, 600, 1, 59, &pow_limit()), floored);
}
#[test]
fn the_formulas_hold_at_their_edges() {
    let max = pow_limit();
    let one = BigUint::from(1u32);
    assert_eq!(asert(&one, 60, 600, 10, 1_000_000_000, &max), max);
    assert_eq!(asert(&max, 60, 600, 10, -1_000_000_000, &max), one);
    assert_eq!(work(&target()), BigUint::from(4u32));
    assert_eq!(work(&max), one);
    assert_eq!(direction(&[(2, 2), (1, 1)]), 2);
    for (value, signal, floor, expected) in [
        (100, 0, 90, 100),
        (100, 1, 90, 105),
        (100, 2, 80, 90),
        (100, 2, 95, 95),
        (109, 1, 90, 114),
        (91, 2, 90, 90),
        (1, 1, 1, 2),
        (19, 1, 1, 20),
        (20, 1, 1, 21),
        (40, 1, 1, 42),
        (9, 2, 1, 8),
        (u64::MAX, 1, 1, u64::MAX),
    ] {
        assert_eq!(adjust(value, signal, floor), expected);
    }
    // A present empty value is not an absent record, a single record is its
    // own leaf at any depth, and the root does not depend on the order in
    // which records are listed.
    let key = h("state-key", &[&[2][..], &[0x11; 32], &[0x22; 32]].concat());
    assert_ne!(state_root(&[(key, b"")]), state_root(&[]));
    assert_ne!(state_root(&[(key, b"a")]), state_root(&[(key, b"")]));
    assert_eq!(subtree(&[(key, b"a")], 200), state_root(&[(key, b"a")]));
    let mut last = [0u8; 32];
    last[31] = 1;
    let both: [Record; 2] = [([0; 32], b"a"), (last, b"b")];
    let reversed: [Record; 2] = [(last, b"b"), ([0; 32], b"a")];
    assert_eq!(state_root(&both), state_root(&reversed));
    assert_ne!(
        vector_root("transactions", &items(["a", "b"])),
        vector_root("transactions", &items(["b", "a"]))
    );
    assert_ne!(
        vector_root("transactions", &items(["a", "b", "c"])),
        vector_root("transactions", &items(["a", "b", "c", "c"]))
    );
}

// The published file's JSON: two-space indents, every list and object one
// entry per line, fields in the order written.

enum Json {
    Number(i128),
    Text(String),
    List(Vec<Json>),
    Object(Vec<(String, Json)>),
}
macro_rules! object {
    ($($key:literal: $value:expr),* $(,)?) => {
        Json::Object(vec![$(($key.to_string(), Json::from($value))),*])
    };
}
macro_rules! numbers {
    ($($t:ty),*) => {$(
        impl From<$t> for Json {
            fn from(n: $t) -> Self {
                Json::Number(n as i128)
            }
        }
    )*};
}
numbers!(u8, u32, i32, u64, usize);
impl From<i128> for Json {
    fn from(n: i128) -> Self {
        Json::Number(n)
    }
}
impl From<&str> for Json {
    fn from(text: &str) -> Self {
        Json::Text(text.into())
    }
}
impl From<Vec<Json>> for Json {
    fn from(items: Vec<Json>) -> Self {
        Json::List(items)
    }
}
impl From<[u64; 4]> for Json {
    fn from(counters: [u64; 4]) -> Self {
        Json::List(Vec::from(counters.map(Json::from)))
    }
}
fn hex(bytes: &[u8]) -> Json {
    Json::Text(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
impl Json {
    fn write(&self, depth: usize, out: &mut String) {
        let indent = |depth: usize| "  ".repeat(depth);
        match self {
            Json::Number(n) => out.push_str(&n.to_string()),
            Json::Text(text) => {
                assert!(
                    text.chars().all(|c| c.is_ascii_graphic() || c == ' ')
                        && !text.contains(['"', '\\'])
                );
                out.push('"');
                out.push_str(text);
                out.push('"');
            }
            Json::List(items) if items.is_empty() => out.push_str("[]"),
            Json::List(items) => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(",\n");
                    }
                    out.push_str(&indent(depth + 1));
                    item.write(depth + 1, out);
                }
                out.push('\n');
                out.push_str(&indent(depth));
                out.push(']');
            }
            Json::Object(fields) => {
                out.push_str("{\n");
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push_str(",\n");
                    }
                    out.push_str(&format!("{}\"{key}\": ", indent(depth + 1)));
                    value.write(depth + 1, out);
                }
                out.push('\n');
                out.push_str(&indent(depth));
                out.push('}');
            }
        }
    }
}

// The formulas, as the wiki states them.

/// `H(tag, bytes) = SHA-256("Ting/" ‖ u16 tag length ‖ tag ‖ bytes)` (wiki: data/encoding).
fn h(tag: &str, bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"Ting/");
    hasher.update((tag.len() as u16).to_le_bytes());
    hasher.update(tag);
    hasher.update(bytes);
    hasher.finalize().into()
}
fn items<const N: usize>(texts: [&str; N]) -> Vec<Vec<u8>> {
    texts.map(|t| t.as_bytes().to_vec()).to_vec()
}
fn blob(bytes: &[u8]) -> Vec<u8> {
    [&(bytes.len() as u32).to_le_bytes()[..], bytes].concat()
}
fn resources(counters: [u64; 4]) -> Vec<u8> {
    counters.iter().flat_map(|c| c.to_le_bytes()).collect()
}
fn be32(n: &BigUint) -> [u8; 32] {
    let bytes = n.to_bytes_be();
    let mut out = [0; 32];
    out[32 - bytes.len()..].copy_from_slice(&bytes);
    out
}
/// A committed list: leaves by index, padded to a power of two, paired up to
/// one node, and the count bound into the root (wiki: data/encoding).
fn vector_root(tag: &str, items: &[Vec<u8>]) -> [u8; 32] {
    let count = (items.len() as u32).to_le_bytes();
    if items.is_empty() {
        return h(&format!("{tag}-root"), &count);
    }
    let mut nodes: Vec<[u8; 32]> = items
        .iter()
        .enumerate()
        .map(|(i, x)| {
            h(
                &format!("{tag}-leaf"),
                &[&(i as u64).to_le_bytes()[..], &blob(x)].concat(),
            )
        })
        .collect();
    for i in nodes.len()..items.len().next_power_of_two() {
        nodes.push(h(&format!("{tag}-pad"), &(i as u64).to_le_bytes()));
    }
    while nodes.len() > 1 {
        nodes = nodes
            .chunks(2)
            .map(|pair| h(&format!("{tag}-node"), &[pair[0], pair[1]].concat()))
            .collect();
    }
    h(&format!("{tag}-root"), &[&count[..], &nodes[0]].concat())
}
/// A record's key and value.
type Record<'a> = ([u8; 32], &'a [u8]);
/// The hash of a set of records whose keys agree on their first `depth` bits:
/// empty, a single record's leaf, or a node over the records split by bit
/// `depth` (wiki: data/state-tree).
fn subtree(records: &[Record], depth: u16) -> [u8; 32] {
    match records {
        [] => h("state-empty", b""),
        [(key, value)] => h("state-leaf", &[&key[..], &blob(value)].concat()),
        _ => {
            let (right, left): (Vec<_>, Vec<_>) = records
                .iter()
                .partition(|(key, _)| key[usize::from(depth / 8)] >> (7 - depth % 8) & 1 == 1);
            let pair = [subtree(&left, depth + 1), subtree(&right, depth + 1)].concat();
            h("state-node", &[&depth.to_le_bytes()[..], &pair].concat())
        }
    }
}
fn state_root(records: &[Record]) -> [u8; 32] {
    subtree(records, 0)
}
fn target() -> BigUint {
    (BigUint::from(1u32) << 254usize) - 1u32
}
fn pow_limit() -> BigUint {
    (BigUint::from(1u32) << 256usize) - 1u32
}
/// `2^256 / (target + 1)` (wiki: consensus/proof-of-work).
fn work(target: &BigUint) -> BigUint {
    (BigUint::from(1u32) << 256usize) / (target + 1u32)
}
/// ASERT in exact integers (wiki: consensus/difficulty).
fn asert(
    anchor: &BigUint,
    interval: i128,
    half_life: i128,
    blocks: i128,
    elapsed: i128,
    limit: &BigUint,
) -> BigUint {
    // floor(a / b) for b > 0, rounding toward negative infinity.
    let floor = |a: i128, b: i128| {
        let q = a / b;
        if a % b != 0 && a < 0 { q - 1 } else { q }
    };
    let exponent = floor((elapsed - interval * blocks) * 65_536, half_life);
    let shifts = floor(exponent, 65_536);
    let factor = factor((exponent - shifts * 65_536) as u128);
    // 2^(shifts - 16): saturating at pow_limit from 2^256 and at 1 from 2^-274.
    let target = match shifts - 16 {
        s if s >= 256 => limit.clone(),
        s if s <= -274 => BigUint::from(1u32),
        s if s >= 0 => anchor * factor * (BigUint::from(1u32) << s as usize),
        s => anchor * factor / (BigUint::from(1u32) << (-s) as usize),
    };
    target.clamp(BigUint::from(1u32), limit.clone())
}
/// `factor` approximates `65536 × 2^(frac / 65536)` (wiki: consensus/difficulty).
fn factor(frac: u128) -> u128 {
    ((195_766_423_245_049 * frac + 971_821_376 * frac.pow(2) + 5_127 * frac.pow(3) + (1 << 47))
        >> 48)
        + 65_536
}
/// Increase with two-thirds of the work, decrease with more than half
/// (wiki: consensus/capacity-voting).
fn direction(signals: &[(u64, u8)]) -> u8 {
    let total: u64 = signals.iter().map(|s| s.0).sum();
    let up: u64 = signals.iter().filter(|s| s.1 == 1).map(|s| s.0).sum();
    let down: u64 = signals.iter().filter(|s| s.1 == 2).map(|s| s.0).sum();
    if 3 * up >= 2 * total {
        1
    } else if 2 * down > total {
        2
    } else {
        0
    }
}
/// Up 5% without a ceiling, down 10% to the floor, at least one each time.
fn adjust(value: u64, signal: u8, floor: u64) -> u64 {
    match signal {
        1 => value.saturating_add((value / 20).max(1)),
        2 => value.saturating_sub((value / 10).max(1)).max(floor),
        _ => value,
    }
}

// The vectors.

struct Chain {
    id: [u8; 32],
    genesis: [u8; 32],
    empty_state: [u8; 32],
}

/// A block at height 1 with the given body, mined to the example target.
fn block(
    c: &Chain,
    transactions: &[Vec<u8>],
    receipts: &[Vec<u8>],
    used: [u64; 4],
    producer: &[u8],
) -> Json {
    let (txroot, rcroot) = (
        vector_root("transactions", transactions),
        vector_root("receipts", receipts),
    );
    let mut body = blob(producer);
    body.extend((transactions.len() as u32).to_le_bytes());
    for item in transactions.iter().chain(receipts) {
        body.extend(blob(item));
    }
    let prefix = [
        &1u16.to_le_bytes()[..],
        &c.id,
        &1u64.to_le_bytes(),
        &c.genesis,
        &(TIMESTAMP + 60).to_le_bytes(),
        &be32(&target()),
        &txroot,
        &rcroot,
        &c.empty_state,
        &resources(used),
        &[0; 4],
        &h("producer", producer),
    ]
    .concat();
    let (nonce, header, id) = (0..10_000u128)
        .map(|nonce| {
            let header = [&prefix[..], &nonce.to_le_bytes()].concat();
            let id = h("block", &header);
            (nonce, header, id)
        })
        .find(|(_, _, id)| BigUint::from_bytes_be(id) <= target())
        .expect("the example proof of work");
    assert_eq!(header.len(), HEADER_LEN);
    assert_eq!((header.len() + body.len()) as u64, used[2]);
    object! {
        "header_hex": hex(&header),
        "body_hex": hex(&body),
        "block_id": hex(&id),
        "nonce": nonce as u64,
        "header_length": header.len(),
        "block_length": header.len() + body.len(),
        "work": u64::try_from(work(&target())).unwrap(),
        "target_hex": hex(&be32(&target())),
        "resources_used_C_S_D_G": used,
        "producer_hex": hex(producer),
        "producer_hash": hex(&h("producer", producer)),
        "transactions_root": hex(&txroot),
        "receipts_root": hex(&rcroot),
        "state_root": hex(&c.empty_state),
    }
}

fn vectors() -> Json {
    let initial = [100_000_000, 1024, 1_048_576, 524_288];
    let minimum = [100_000, 16, 1024, 4096];
    let config = [
        &1u16.to_le_bytes()[..],
        &TIMESTAMP.to_le_bytes(),
        &be32(&target()),
        &be32(&pow_limit()),
        &60u32.to_le_bytes(),
        &172_800u32.to_le_bytes(),
        &4u32.to_le_bytes(),
        &resources(initial),
        &resources(minimum),
    ]
    .concat();
    assert_eq!(config.len(), 150);
    let id = h("chain", &config);
    let c = Chain {
        id,
        genesis: h("genesis", &id),
        empty_state: state_root(&[]),
    };

    // A call to a program that does not exist: status 8, after the root
    // dispatch and the read of the absent manifest with its key, one S.
    let tx = [
        &[1, 0][..],
        &resources([10_000, 10, 1024, 4096]),
        &[0x11; 32],
        &blob(b""),
    ]
    .concat();
    let txid = h("transaction", &[&c.id[..], &tx].concat());
    let tx_used = [1 + KEY, 1, tx.len() as u64 + TRANSACTION_BASE, 0];
    let receipt = [
        &txid[..],
        &[8],
        &resources(tx_used),
        &blob(b""),
        &0u32.to_le_bytes(),
    ]
    .concat();
    assert_eq!((tx.len(), receipt.len(), tx_used[2]), (70, 73, 151));
    let missing_block = block(
        &c,
        std::slice::from_ref(&tx),
        std::slice::from_ref(&receipt),
        [
            1 + KEY,
            1,
            BLOCK_BASE + 4 + tx.len() as u64 + 4 + receipt.len() as u64,
            0,
        ],
        b"",
    );

    // ASERT with an anchor target of 1,000, a 60-second interval, a 600-second
    // half-life and a limit of 10,000.
    let rounding_anchor = BigUint::from(1u32) << 200usize;
    let difficulty: Vec<Json> = [
        (0, 0, 1000),
        (10, 600, 1000),
        (10, 1200, 2000),
        (10, 0, 500),
        (10, 1800, 4000),
        (10, 900, 1414),
        (10, 300, 707),
        (10, 599, 998),
        (10, 601, 1001),
        (10, 100_000, 10_000),
        (10, -100_000, 1),
        (1, -59, 871),
    ]
    .into_iter()
    .map(|(blocks, elapsed, expected): (i128, i128, u32)| {
        let anchor = BigUint::from(1000u32);
        let limit = BigUint::from(10_000u32);
        assert_eq!(
            asert(&anchor, 60, 600, blocks, elapsed, &limit),
            BigUint::from(expected),
            "{blocks} blocks, {elapsed} s"
        );
        object! {
            "anchor_target": 1000,
            "interval": 60,
            "half_life": 600,
            "blocks": blocks,
            "elapsed": elapsed,
            "pow_limit": 10_000,
            "new_target": expected,
        }
    })
    .collect();

    let example = |signals: &[(u64, u8)], expected: u8| {
        assert_eq!(direction(signals), expected);
        object! {
            "work_and_signal": signals.iter().map(|&(w, s)| Json::List(vec![w.into(), s.into()])).collect::<Vec<_>>(),
            "direction": expected,
        }
    };
    // Epochs of four blocks: the votes of epoch e take effect in epoch e + 3.
    let sources = [1, 0, 2];
    let mut active = 100;
    let epochs: Vec<Json> = (0..6u64)
        .map(|e| {
            if e >= 3 {
                active = adjust(active, sources[(e - 3) as usize], 90);
            }
            object! { "epoch": e, "first_height": e * 4 + 1, "active_limit": active }
        })
        .collect();
    assert_eq!(epochs.len(), 6);
    assert_eq!(active, 95);

    // Growth: a record created empty, grown to two bytes, shrunk to one,
    // deleted and created again with one byte; growth is never refunded.
    let growth = [RECORD_BASE, 2, 0, 0, RECORD_BASE + 1];
    // Output discarded by a revert: an event of 100 bytes and a nested
    // result of 8, paid for and not carried by the block.
    let event_d = EVENT_BASE + 100;
    let total_d = tx_used[2] + event_d + 8;
    let key = h("state-key", &[&[2][..], &[0x11; 32], &[0x22; 32]].concat());
    // Three keys: two that differ in their first bit, and one that differs
    // from the first only in its last bit.
    let low = [0u8; 32];
    let mut high = [0u8; 32];
    high[0] = 0x80;
    let mut last = [0u8; 32];
    last[31] = 1;
    let (va, vb, vc): (&[u8], &[u8], &[u8]) = (b"a", b"b", b"c");

    object! {
        "scope": "Checked design examples; not a complete VM conformance suite or production genesis.",
        "resource_order": vec![Json::from("C"), "S".into(), "D".into(), "G".into()],
        "genesis": object! {
            "config_hex": hex(&config),
            "config_length": config.len(),
            "chain_id": hex(&c.id),
            "genesis_block_id": hex(&c.genesis),
            "timestamp": TIMESTAMP,
            "target_interval": 60,
            "half_life": 172_800,
            "vote_epoch_length": 4,
            "initial_limits": initial,
            "minimum_limits": minimum,
        },
        "hashes": object! {
            "H_example_empty": hex(&h("example", b"")),
            "H_example_abc": hex(&h("example", b"abc")),
            "empty_state_root": hex(&state_root(&[])),
            "empty_transactions_root": hex(&vector_root("transactions", &[])),
            "empty_receipts_root": hex(&vector_root("receipts", &[])),
            "transactions_three_items_61_62_63": hex(&vector_root("transactions", &items(["a", "b", "c"]))),
            "empty_producer_hash": hex(&h("producer", b"")),
        },
        "state_tree": object! {
            "record_key": hex(&key),
            "single_present_empty_value_root": hex(&state_root(&[(key, b"")])),
            "single_value_61_root": hex(&state_root(&[(key, va)])),
            "after_delete_root": hex(&state_root(&[])),
            "key_low": hex(&low),
            "key_high": hex(&high),
            "key_last_bit": hex(&last),
            "values": "61 at key_low, 62 at key_high, 63 at key_last_bit",
            "low_root": hex(&state_root(&[(low, va)])),
            "low_high_root": hex(&state_root(&[(low, va), (high, vb)])),
            "low_high_last_bit_root": hex(&state_root(&[(low, va), (high, vb), (last, vc)])),
            "after_deleting_high_root": hex(&state_root(&[(low, va), (last, vc)])),
            "after_deleting_high_and_last_bit_root": hex(&state_root(&[(low, va)])),
        },
        "empty_block": block(&c, &[], &[], [0, 0, BLOCK_BASE, 0], b""),
        "producer_data_block": block(&c, &[], &[], [0, 0, BLOCK_BASE + 13, 0], b"fixture miner"),
        "missing_program_transaction": object! {
            "transaction_hex": hex(&tx),
            "transaction_id": hex(&txid),
            "receipt_hex": hex(&receipt),
            "receipt_status": 8,
            "transaction_length": tx.len(),
            "receipt_length": receipt.len(),
            "transaction_usage": tx_used,
            "block": missing_block,
        },
        "difficulty": object! {
            "rule": "ASERT, wiki consensus/difficulty",
            "cases": difficulty,
            "rounding_case": object! {
                "note": "one second ahead of schedule: the exponent is floor(-65536 / 600) = -110, where truncation would give -109",
                "anchor_target_hex": hex(&be32(&rounding_anchor)),
                "interval": 60,
                "half_life": 600,
                "blocks": 1,
                "elapsed": 59,
                "pow_limit_hex": hex(&be32(&pow_limit())),
                "new_target_hex": hex(&be32(&asert(&rounding_anchor, 60, 600, 1, 59, &pow_limit()))),
            },
        },
        "voting": object! {
            "weighted_threshold_example": example(&[(4, 1), (1, 0), (1, 0)], 1),
            "below_threshold_example": example(&[(2, 1), (1, 0), (1, 0)], 0),
            "majority_decrease_example": example(&[(2, 2), (1, 1)], 2),
            "exact_half_holds_example": example(&[(1, 2), (1, 0)], 0),
            "epoch_activation": object! {
                "epoch_length": 4,
                "initial_limit": 100,
                "minimum": 90,
                "source_directions": object! { "0": sources[0], "1": sources[1], "2": sources[2] },
                "epochs": epochs,
            },
        },
        "growth": object! {
            "operations": Vec::from(
                ["create empty value", "expand to 2 bytes", "shrink to 1 byte", "delete", "recreate 1 byte"]
                    .map(Json::from),
            ),
            "G_debits": Vec::from(growth.map(Json::from)),
            "total_G": growth.iter().sum::<u64>(),
            "final_live_record_allocation": RECORD_BASE + 1,
            "G_after_enclosing_revert": growth.iter().sum::<u64>(),
            "live_allocation_after_enclosing_revert": 0,
        },
        "discarded_output": object! {
            "transaction_base_D": tx_used[2],
            "event_payload_len": 100,
            "event_D": event_d,
            "nested_result_D": 8,
            "root_revert_result_len": 0,
            "total_transaction_D": total_d,
            "retained_events": 0,
            "actual_encoded_block_length": 453,
            "block_D": BLOCK_BASE + total_d,
        },
    }
}
