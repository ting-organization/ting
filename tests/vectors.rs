//! The consensus vectors of the groups that are formulas and encodings,
//! worked out from the protocol rules without the code under test: V1
//! primitives, V2 envelopes, V5 difficulty, V6 voting, V8 work and V9 tables,
//! one JSON file per vector under `conformance/vectors/` (wiki:
//! reference/conformance). Nothing here uses `ting_core`: this file holds the
//! vectors to the protocol rules and `tests/conformance.rs` holds the code to
//! the vectors, so the code and the specification can only agree through them.
//! The groups built on blocks of the seed, V3, V4 and V7, come from
//! `tests/vm/consensus.rs`.
//!
//! After a deliberate change to the protocol, rewrite the files and review the
//! difference:
//!
//! ```text
//! TING_WRITE_VECTORS=1 cargo test --test vectors
//! ```
use num_bigint::BigUint;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};

const TRANSACTION_BASE: u64 = 81;
const TIMESTAMP: u64 = 1_700_000_000;
/// The groups this file writes; each directory holds its vectors alone.
const GROUPS: [&str; 6] = [
    "v1-primitives",
    "v2-envelopes",
    "v5-difficulty",
    "v6-voting",
    "v8-work",
    "v9-tables",
];

#[test]
fn the_published_vectors_follow_from_the_specification() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance/vectors");
    let mut written: BTreeMap<&str, BTreeMap<String, String>> = BTreeMap::new();
    for v in vectors() {
        let mut text = String::new();
        v.json().write(0, &mut text);
        text.push('\n');
        let previous = written
            .entry(v.group)
            .or_default()
            .insert(format!("{}.json", v.name), text);
        assert!(previous.is_none(), "{}/{} twice", v.group, v.name);
    }
    assert_eq!(written.len(), GROUPS.len());
    let rewrite = std::env::var_os("TING_WRITE_VECTORS").is_some();
    for (group, files) in &written {
        let dir = root.join(group);
        if rewrite {
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            for (name, text) in files {
                fs::write(dir.join(name), text).unwrap();
            }
            continue;
        }
        let mut published: Vec<String> = fs::read_dir(&dir)
            .unwrap_or_else(|_| panic!("{group} is missing; write it with TING_WRITE_VECTORS=1"))
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        published.sort();
        let specified: Vec<String> = files.keys().cloned().collect();
        assert_eq!(
            published, specified,
            "{group}: the files differ from the specification; after a deliberate \
             protocol change, rewrite them with TING_WRITE_VECTORS=1"
        );
        for (name, text) in files {
            let on_disk = fs::read_to_string(dir.join(name))
                .unwrap()
                .replace("\r\n", "\n");
            assert!(
                on_disk == *text,
                "{group}/{name} differs from the specification; after a deliberate \
                 protocol change, rewrite it with TING_WRITE_VECTORS=1"
            );
        }
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
}

#[test]
fn the_formulas_hold_at_their_edges() {
    let max = pow_limit();
    let one = BigUint::from(1u32);
    assert_eq!(asert(&one, 60, 600, 10, 1_000_000_000, &max), max);
    assert_eq!(asert(&max, 60, 600, 10, -1_000_000_000, &max), one);
    assert_eq!(work(&target()), BigUint::from(4u32));
    assert_eq!(work(&max), one);
    // A present empty value is not an absent record, a single record is its
    // own leaf at any depth, and the root does not depend on the order in
    // which records are listed.
    let key = record_key(2, [0x11; 32], [0x22; 32]);
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

// A vector and its JSON: two-space indents, every list and object one entry
// per line, fields in the order written, and integers past 2^53 as strings.

#[derive(Clone)]
enum Json {
    Number(i128),
    Text(String),
    Bool(bool),
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
                Json::number(n as i128)
            }
        }
    )*};
}
numbers!(u8, u16, u32, i32, u64, usize, i64);
impl From<i128> for Json {
    fn from(n: i128) -> Self {
        Json::number(n)
    }
}
impl From<bool> for Json {
    fn from(b: bool) -> Self {
        Json::Bool(b)
    }
}
impl From<&str> for Json {
    fn from(text: &str) -> Self {
        Json::Text(text.into())
    }
}
impl From<String> for Json {
    fn from(text: String) -> Self {
        Json::Text(text)
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
impl From<&BigUint> for Json {
    fn from(n: &BigUint) -> Self {
        Json::Text(n.to_string())
    }
}
fn hex(bytes: &[u8]) -> Json {
    Json::Text(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
impl Json {
    /// A number, or past 2^53 the decimal digits as a string.
    fn number(n: i128) -> Self {
        if n.unsigned_abs() > 1 << 53 {
            Json::Text(n.to_string())
        } else {
            Json::Number(n)
        }
    }
    fn write(&self, depth: usize, out: &mut String) {
        let indent = |depth: usize| "  ".repeat(depth);
        match self {
            Json::Number(n) => out.push_str(&n.to_string()),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Text(text) => {
                assert!(
                    text.chars().all(|c| c.is_ascii_graphic() || c == ' ')
                        && !text.contains(['"', '\\']),
                    "{text}"
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

struct Vector {
    group: &'static str,
    name: String,
    rule: String,
    rules: &'static str,
    input: Json,
    expect: Json,
}
impl Vector {
    fn json(&self) -> Json {
        object! {
            "group": self.group,
            "rule": self.rule.as_str(),
            "rules": self.rules,
            "input": self.input.clone(),
            "expect": self.expect.clone(),
        }
    }
}
fn vector(
    group: &'static str,
    name: impl Into<String>,
    rule: impl Into<String>,
    rules: &'static str,
    input: Json,
    expect: Json,
) -> Vector {
    Vector {
        group,
        name: name.into(),
        rule: rule.into(),
        rules,
        input,
        expect,
    }
}

// The formulas, as the protocol rules state them.

/// `H(tag, bytes) = SHA-256("Ting/" ‖ u16 tag length ‖ tag ‖ bytes)` (rules §2).
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
/// one node, and the count bound into the root (rules §2).
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
/// The state key of a record: its namespace, its program and its logical key
/// (rules §5).
fn record_key(namespace: u8, contract: [u8; 32], logical: [u8; 32]) -> [u8; 32] {
    h(
        "state-key",
        &[&[namespace][..], &contract, &logical].concat(),
    )
}
/// A record's key and value.
type Record<'a> = ([u8; 32], &'a [u8]);
/// The hash of a set of records whose keys agree on their first `depth` bits:
/// empty, a single record's leaf, or a node over the records split by bit
/// `depth` (rules §6).
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
/// `2^256 / (target + 1)` (rules §8).
fn work(target: &BigUint) -> BigUint {
    (BigUint::from(1u32) << 256usize) / (target + 1u32)
}
/// ASERT in exact integers (rules §9).
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
/// `factor` approximates `65536 × 2^(frac / 65536)` (rules §9).
fn factor(frac: u128) -> u128 {
    ((195_766_423_245_049 * frac + 971_821_376 * frac.pow(2) + 5_127 * frac.pow(3) + (1 << 47))
        >> 48)
        + 65_536
}
/// Increase with two-thirds of the work, decrease with more than half
/// (rules §10).
fn direction(signals: &[(BigUint, u8)]) -> u8 {
    let total: BigUint = signals.iter().map(|s| &s.0).sum();
    let up: BigUint = signals.iter().filter(|s| s.1 == 1).map(|s| &s.0).sum();
    let down: BigUint = signals.iter().filter(|s| s.1 == 2).map(|s| &s.0).sum();
    if total == BigUint::from(0u32) {
        0
    } else if up * 3u32 >= &total * 2u32 {
        1
    } else if down * 2u32 > total {
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
/// The prices of a memory of `pages` pages (rules §15.2):
/// three steps to 2 MiB, then the curve over `L`, log₂ of the pages in
/// sixteenths of a doubling, rounded down.
fn memory_prices(pages: u64) -> [u64; 4] {
    match pages {
        0..=1 => [18, 1, 4, 0],
        2..=16 => [31, 1, 8, 0],
        17..=32 => [80, 1, 8, 0],
        _ => {
            let b = u64::from(63 - pages.leading_zeros());
            let l = 16 * b + ((16 * pages) >> b) - 16;
            let d = (l - 80) * (l - 80);
            let store = 16 + d.div_ceil(72);
            [770 + d.div_ceil(48), store, 12, store]
        }
    }
}
/// A SHA-256 the runtime computes over an `m`-byte message (rules
/// §15.5).
fn sha(m: u64) -> u64 {
    100 + 900 * (m + 9).div_ceil(64)
}
/// Stack pages paid for a height of `slots` (rules §21).
fn stack_pages(slots: u64) -> u64 {
    slots.div_ceil(8_192).saturating_sub(1)
}

/// The budgets of one transaction as the protocol rules state them (rules
/// §11): a stack of frames, each with its budget and its own
/// use. A debit fits when every frame can take it, checked C, S, D, G in
/// order, and a refused one changes nothing; a nested budget opens only
/// within what its parent has left; closing a frame returns its use. Sums are
/// exact, so nothing here wraps near 2^64.
struct Budgets(Vec<([u64; 4], [u64; 4])>);
enum Step {
    Charge([u64; 4]),
    Open([u64; 4]),
    Close,
}
impl Budgets {
    fn run(&mut self, step: &Step) -> Json {
        match step {
            Step::Charge(debit) => {
                for i in 0..4 {
                    if self.0.iter().any(|(budget, used)| {
                        u128::from(used[i]) + u128::from(debit[i]) > u128::from(budget[i])
                    }) {
                        return Json::from(["C", "S", "D", "G"][i]);
                    }
                }
                for (_, used) in &mut self.0 {
                    for i in 0..4 {
                        used[i] += debit[i];
                    }
                }
                Json::from("ok")
            }
            Step::Open(budget) => {
                let (top, used) = self.0.last().unwrap();
                if (0..4).all(|i| budget[i] <= top[i] - used[i]) {
                    self.0.push((*budget, [0; 4]));
                    Json::from("opened")
                } else {
                    Json::from("refused")
                }
            }
            Step::Close => Json::from(self.0.pop().unwrap().1),
        }
    }
}

// The vectors.

const GENESIS: &str = "3";

struct Profile {
    name: &'static str,
    vote_epoch: u32,
}
const INITIAL: [u64; 4] = [100_000_000, 1024, 1_048_576, 524_288];
const MINIMUM: [u64; 4] = [100_000, 16, 1024, 4096];
fn config(vote_epoch: u32) -> Vec<u8> {
    [
        &1u16.to_le_bytes()[..],
        &TIMESTAMP.to_le_bytes(),
        &be32(&target()),
        &be32(&pow_limit()),
        &60u32.to_le_bytes(),
        &172_800u32.to_le_bytes(),
        &vote_epoch.to_le_bytes(),
        &resources(INITIAL),
        &resources(MINIMUM),
    ]
    .concat()
}
/// The development chain's identifier, which transaction identifiers bind.
fn development() -> [u8; 32] {
    h("chain", &config(20_160))
}

fn vectors() -> Vec<Vector> {
    let mut all = Vec::new();
    all.extend(primitives());
    all.extend(envelopes());
    all.extend(difficulty());
    all.extend(voting());
    all.extend(work_vectors());
    all.extend(tables());
    all
}

fn primitives() -> Vec<Vector> {
    const G: &str = "v1-primitives";
    let mut v = Vec::new();
    for p in [
        Profile {
            name: "development",
            vote_epoch: 20_160,
        },
        Profile {
            name: "experimental",
            vote_epoch: 4,
        },
    ] {
        let config = config(p.vote_epoch);
        assert_eq!(config.len(), 150);
        let id = h("chain", &config);
        v.push(vector(
            G,
            format!("genesis-{}", p.name),
            "the genesis configuration encodes to 150 bytes; ChainId = H('chain', them); the genesis anchor is H('genesis', ChainId)",
            GENESIS,
            object! {
                "profile": p.name,
                "timestamp": TIMESTAMP,
                "initial_target_hex": hex(&be32(&target())),
                "pow_limit_hex": hex(&be32(&pow_limit())),
                "interval": 60,
                "half_life": 172_800,
                "vote_epoch": p.vote_epoch,
                "initial": INITIAL,
                "minimum": MINIMUM,
            },
            object! {
                "config_hex": hex(&config),
                "chain_id": hex(&id),
                "genesis_block_id": hex(&h("genesis", &id)),
            },
        ));
    }
    let long: Vec<u8> = (0..100u8).collect();
    for (name, tag, bytes) in [
        ("hash-empty-tag", "", &b""[..]),
        ("hash-example-empty", "example", b""),
        ("hash-example-abc", "example", b"abc"),
        ("hash-example-100-bytes", "example", &long),
        ("hash-block-abc", "block", b"abc"),
    ] {
        v.push(vector(
            G,
            name,
            "H(tag, bytes) = SHA-256('Ting/' || u16 tag length || tag || bytes)",
            "2.4",
            object! { "tag": tag, "bytes_hex": hex(bytes) },
            object! { "hash": hex(&h(tag, bytes)) },
        ));
    }
    let letters = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];
    for (tag, n) in [
        ("transactions", 0),
        ("transactions", 1),
        ("transactions", 2),
        ("transactions", 3),
        ("transactions", 4),
        ("transactions", 5),
        ("transactions", 8),
        ("transactions", 9),
        ("receipts", 0),
        ("receipts", 3),
    ] {
        let list: Vec<Vec<u8>> = letters[..n].iter().map(|t| t.as_bytes().to_vec()).collect();
        v.push(vector(
            G,
            format!("{tag}-root-{n}"),
            "an ordered commitment: indexed leaves padded to a power of two, paired to one node, with the count in the root",
            "2.5",
            object! {
                "tag": tag,
                "items_hex": list.iter().map(|x| hex(x)).collect::<Vec<_>>(),
            },
            object! { "root": hex(&vector_root(tag, &list)) },
        ));
    }
    let (contract, logical) = ([0x11; 32], [0x22; 32]);
    v.push(vector(
        G,
        "record-key",
        "a record's state key is H('state-key', namespace || program || logical key)",
        "5.1",
        object! {
            "namespace": 2,
            "contract": hex(&contract),
            "logical_key": hex(&logical),
        },
        object! { "key": hex(&record_key(2, contract, logical)) },
    ));
    // State trees after a sequence of writes and deletions: an empty value
    // against none, splits at the first and at the last bit, records moving
    // up after a deletion, an overwrite, and the same set in another order.
    let key = record_key(2, contract, logical);
    let low = [0u8; 32];
    let mut high = [0u8; 32];
    high[0] = 0x80;
    let mut last = [0u8; 32];
    last[31] = 1;
    type Op<'a> = ([u8; 32], Option<&'a [u8]>);
    let trees: [(&str, Vec<Op>); 10] = [
        ("state-empty", vec![]),
        ("state-empty-value", vec![(key, Some(b""))]),
        ("state-one-value", vec![(key, Some(b"a"))]),
        (
            "state-written-then-deleted",
            vec![(key, Some(b"a")), (key, None)],
        ),
        (
            "state-split-at-the-first-bit",
            vec![(low, Some(b"a")), (high, Some(b"b"))],
        ),
        (
            "state-split-at-the-last-bit",
            vec![(low, Some(b"a")), (high, Some(b"b")), (last, Some(b"c"))],
        ),
        (
            "state-written-in-another-order",
            vec![(last, Some(b"c")), (high, Some(b"b")), (low, Some(b"a"))],
        ),
        (
            "state-a-record-moves-up",
            vec![
                (low, Some(b"a")),
                (high, Some(b"b")),
                (last, Some(b"c")),
                (high, None),
            ],
        ),
        (
            "state-two-records-move-up",
            vec![
                (low, Some(b"a")),
                (high, Some(b"b")),
                (last, Some(b"c")),
                (high, None),
                (last, None),
            ],
        ),
        (
            "state-overwritten",
            vec![(low, Some(b"a")), (high, Some(b"b")), (low, Some(b"z"))],
        ),
    ];
    for (name, ops) in trees {
        let mut set: BTreeMap<[u8; 32], &[u8]> = BTreeMap::new();
        for (k, value) in &ops {
            match value {
                Some(value) => set.insert(*k, value),
                None => set.remove(k),
            };
        }
        let records: Vec<Record> = set.into_iter().collect();
        v.push(vector(
            G,
            name,
            "the state root of the records present after the writes, in the tree the protocol rules define",
            "6.2",
            object! {
                "writes": ops.iter().map(|(k, value)| match value {
                    Some(value) => object! { "key": hex(k), "value_hex": hex(value) },
                    None => object! { "key": hex(k), "delete": true },
                }).collect::<Vec<_>>(),
            },
            object! { "root": hex(&state_root(&records)) },
        ));
    }
    v
}

/// A call to `[0x11; 32]` with no input under `budget`.
fn call(version: u8, kind: u8, budget: [u64; 4]) -> Vec<u8> {
    [
        &[version, kind][..],
        &resources(budget),
        &[0x11; 32],
        &blob(b""),
    ]
    .concat()
}
fn envelopes() -> Vec<Vector> {
    const G: &str = "v2-envelopes";
    const RULE: &str = "4.4";
    let budget = [10_000, 10, 1024, 4096];
    let ok = call(1, 0, budget);
    assert_eq!(ok.len(), 70);
    let n = ok.len() as u64;
    let deploy = [
        &[1, 1][..],
        &resources(budget),
        &blob(b"\0asm"),
        &blob(b"in"),
    ]
    .concat();
    let with_d = |d: u64| {
        let mut b = budget;
        b[2] = d;
        call(1, 0, b)
    };
    let mut past_end = ok.clone();
    past_end[66..70].copy_from_slice(&5u32.to_le_bytes());
    let mut code_past_end = deploy.clone();
    code_past_end[34..38].copy_from_slice(&100u32.to_le_bytes());
    let cases: Vec<(&str, Vec<u8>, bool, &str)> = vec![
        (
            "a-call",
            ok.clone(),
            true,
            "a call: version 1, kind 0, the budget, the target and the input",
        ),
        (
            "a-deployment",
            deploy.clone(),
            true,
            "a deployment: version 1, kind 1, the budget, the code and the input",
        ),
        ("version-0", call(0, 0, budget), false, "the version is 1"),
        ("version-2", call(2, 0, budget), false, "the version is 1"),
        (
            "kind-2",
            call(1, 2, budget),
            false,
            "the kind is 0, a call, or 1, a deployment",
        ),
        ("empty", Vec::new(), false, "a transaction is not empty"),
        (
            "cut-after-the-version",
            ok[..1].to_vec(),
            false,
            "every field is complete",
        ),
        (
            "cut-inside-the-budget",
            ok[..20].to_vec(),
            false,
            "every field is complete",
        ),
        (
            "cut-inside-the-target",
            ok[..50].to_vec(),
            false,
            "every field is complete",
        ),
        (
            "cut-inside-the-input-length",
            ok[..68].to_vec(),
            false,
            "every field is complete",
        ),
        (
            "input-length-past-the-end",
            past_end,
            false,
            "a length does not run past the end",
        ),
        (
            "code-length-past-the-end",
            code_past_end,
            false,
            "a length does not run past the end",
        ),
        (
            "a-trailing-byte",
            [&ok[..], &[0]].concat(),
            false,
            "nothing follows the last field",
        ),
        (
            "data-one-below-the-envelope",
            with_d(n + TRANSACTION_BASE - 1),
            false,
            "D covers the transaction's bytes and the data base of 81",
        ),
        (
            "data-exactly-the-envelope",
            with_d(n + TRANSACTION_BASE),
            true,
            "D covers the transaction's bytes and the data base of 81",
        ),
        (
            "data-at-its-frame",
            with_d(u64::from(u32::MAX)),
            true,
            "D is at most 2^32 - 1",
        ),
        (
            "data-past-its-frame",
            with_d(1 << 32),
            false,
            "D is at most 2^32 - 1",
        ),
    ];
    cases
        .into_iter()
        .map(|(name, bytes, valid, rule)| {
            let expect = if valid {
                let id = h("transaction", &[&development()[..], &bytes].concat());
                object! { "valid": true, "id_on_development": hex(&id) }
            } else {
                object! { "valid": false }
            };
            vector(
                G,
                name,
                rule,
                RULE,
                object! { "transaction_hex": hex(&bytes) },
                expect,
            )
        })
        .collect()
}

fn difficulty() -> Vec<Vector> {
    const G: &str = "v5-difficulty";
    const RULE: &str = "the target of the next block: anchor * 2^((elapsed - interval * blocks) / half-life), the exponent floored in 1/65536 steps, its fraction by the cubic, clamped to [1, limit]";
    let small = |n: u32| BigUint::from(n);
    // A small anchor shows the arithmetic; the development genesis on, ahead
    // of and behind schedule; saturation at both ends; and a second ahead
    // where flooring the exponent differs from truncating it.
    let mut cases: Vec<(String, BigUint, i128, i128, i128, BigUint)> = [
        (0, 0),
        (10, 600),
        (10, 1200),
        (10, 0),
        (10, 1800),
        (10, 900),
        (10, 300),
        (10, 599),
        (10, 601),
        (10, 100_000),
        (10, -100_000),
        (1, -59),
    ]
    .into_iter()
    .map(|(blocks, elapsed)| {
        (
            format!("small-anchor-{blocks}-blocks-{elapsed}-s").replace("--", "-minus-"),
            small(1000),
            600,
            blocks,
            elapsed,
            small(10_000),
        )
    })
    .collect();
    for (name, blocks, elapsed) in [
        ("development-on-schedule", 1000, 60_000),
        ("development-a-half-life-late", 1000, 60_000 + 172_800),
        ("development-a-half-life-early", 1000, 60_000 - 172_800),
        ("development-a-second-early", 1000, 59_999),
    ] {
        cases.push((name.into(), target(), 172_800, blocks, elapsed, pow_limit()));
    }
    cases.push((
        "saturated-at-the-limit".into(),
        small(1),
        600,
        10,
        1_000_000_000,
        pow_limit(),
    ));
    cases.push((
        "saturated-at-one".into(),
        pow_limit(),
        600,
        10,
        -1_000_000_000,
        pow_limit(),
    ));
    cases.push((
        "flooring-not-truncating".into(),
        BigUint::from(1u32) << 200usize,
        600,
        1,
        59,
        pow_limit(),
    ));
    cases
        .into_iter()
        .map(|(name, anchor, half_life, blocks, elapsed, limit)| {
            let next = asert(&anchor, 60, half_life, blocks, elapsed, &limit);
            vector(
                G,
                name,
                RULE,
                "9.3",
                object! {
                    "anchor_target_hex": hex(&be32(&anchor)),
                    "interval": 60,
                    "half_life": half_life,
                    "blocks": blocks,
                    "elapsed": elapsed,
                    "pow_limit_hex": hex(&be32(&limit)),
                },
                object! { "target_hex": hex(&be32(&next)) },
            )
        })
        .collect()
}

fn voting() -> Vec<Vector> {
    const G: &str = "v6-voting";
    const COUNTING: &str = "10.2";
    let mut v = Vec::new();
    let big = |n: u32| BigUint::from(n);
    let directions: Vec<(&str, Vec<(BigUint, u8)>)> = vec![
        (
            "weighted-increase",
            vec![(big(4), 1), (big(1), 0), (big(1), 0)],
        ),
        (
            "below-two-thirds-holds",
            vec![(big(2), 1), (big(1), 0), (big(1), 0)],
        ),
        (
            "exactly-two-thirds-increases",
            vec![(big(2), 1), (big(1), 0)],
        ),
        (
            "just-below-two-thirds-holds",
            vec![(big(199), 1), (big(101), 0)],
        ),
        ("majority-decreases", vec![(big(2), 2), (big(1), 1)]),
        ("exact-half-holds", vec![(big(1), 2), (big(1), 0)]),
        ("no-signals-hold", vec![]),
        (
            "work-past-2-to-the-64",
            vec![
                (BigUint::from(1u32) << 255usize, 1),
                (BigUint::from(1u32) << 254usize, 2),
            ],
        ),
    ];
    for (name, signals) in directions {
        v.push(vector(
            G,
            format!("direction-{name}"),
            "an increase needs two-thirds of the epoch's work, a decrease more than half; otherwise the limit holds",
            COUNTING,
            object! {
                "work_and_signal": signals.iter().map(|(w, s)| Json::List(vec![w.into(), (*s).into()])).collect::<Vec<_>>(),
            },
            object! { "direction": direction(&signals) },
        ));
    }
    for (value, signal, floor) in [
        (100u64, 0u8, 90u64),
        (100, 1, 90),
        (100, 2, 80),
        (100, 2, 95),
        (109, 1, 90),
        (91, 2, 90),
        (1, 1, 1),
        (19, 1, 1),
        (20, 1, 1),
        (40, 1, 1),
        (9, 2, 1),
        (u64::MAX, 1, 1),
        (u64::MAX - 1, 1, 1),
    ] {
        v.push(vector(
            G,
            format!("step-{value}-signal-{signal}-floor-{floor}"),
            "an increase adds 5% and at least 1, saturating at 2^64 - 1; a decrease takes 10% and at least 1, down to the floor",
            "10.4",
            object! { "limit": value, "direction": signal, "floor": floor },
            object! { "limit": adjust(value, signal, floor) },
        ));
    }
    // Activation: epochs of four blocks on the experimental genesis, the votes
    // of epoch e deciding the limits from epoch e + 3, each block weighing its
    // work. The headers give their target and their votes.
    let easy = target();
    let hard = (BigUint::from(1u32) << 250usize) - 1u32;
    type Headers = Vec<(BigUint, [u8; 4])>;
    let scenarios: Vec<(&str, Headers)> = vec![
        (
            "epochs-up-none-down",
            [
                vec![(easy.clone(), [1, 1, 0, 2]); 4],
                vec![(easy.clone(), [0, 0, 0, 0]); 4],
                vec![(easy.clone(), [2, 0, 1, 0]); 4],
                vec![(easy.clone(), [0, 0, 0, 0]); 12],
            ]
            .concat(),
        ),
        (
            "work-outweighs-count",
            [
                vec![
                    (hard.clone(), [1, 0, 0, 0]),
                    (easy.clone(), [2, 0, 0, 0]),
                    (easy.clone(), [2, 0, 0, 0]),
                    (easy.clone(), [2, 0, 0, 0]),
                ],
                vec![(easy.clone(), [0, 0, 0, 0]); 12],
            ]
            .concat(),
        ),
    ];
    for (name, headers) in scenarios {
        let mut active = Vec::new();
        for k in (0..=headers.len()).step_by(4) {
            let mut limits = INITIAL;
            for activation in 3..=k / 4 {
                let source = &headers[(activation - 3) * 4..(activation - 2) * 4];
                for (i, limit) in limits.iter_mut().enumerate() {
                    let signals: Vec<(BigUint, u8)> = source
                        .iter()
                        .map(|(t, votes)| (work(t), votes[i]))
                        .collect();
                    *limit = adjust(*limit, direction(&signals), MINIMUM[i]);
                }
            }
            active.push(Json::from(limits));
        }
        v.push(vector(
            G,
            format!("activation-{name}"),
            "the votes of epoch e, weighed by each block's work, set the limits from epoch e + 3",
            "10.3",
            object! {
                "profile": "experimental",
                "headers": headers.iter().map(|(t, votes)| object! {
                    "target_hex": hex(&be32(t)),
                    "votes": Json::List(votes.iter().map(|v| Json::from(*v)).collect()),
                }).collect::<Vec<_>>(),
            },
            object! { "active_limits_after_each_epoch": active },
        ));
    }
    v
}

fn work_vectors() -> Vec<Vector> {
    const G: &str = "v8-work";
    let one = BigUint::from(1u32);
    let pow2 = |n: usize| BigUint::from(1u32) << n;
    let mut v: Vec<Vector> = [
        ("the-pow-limit", pow2(256) - &one),
        ("half-the-pow-limit", pow2(255) - &one),
        ("the-development-target", pow2(254) - &one),
        ("the-production-target", pow2(224) - &one),
        ("an-uneven-target", pow2(200) * 3u32),
        ("target-one", one.clone()),
    ]
    .into_iter()
    .map(|(name, t)| {
        vector(
            G,
            name,
            "a block's work is 2^256 / (target + 1), rounded down",
            "8.2",
            object! { "target_hex": hex(&be32(&t)) },
            object! { "work": &work(&t) },
        )
    })
    .collect();
    let chain = [
        pow2(254) - &one,
        pow2(250) - &one,
        pow2(224) - &one,
        pow2(254) - &one,
    ];
    let total: BigUint = chain.iter().map(work).sum();
    v.push(vector(
        G,
        "accumulated-work",
        "a branch's work is the sum of its blocks' work",
        "8.2",
        object! {
            "targets_hex": chain.iter().map(|t| hex(&be32(t))).collect::<Vec<_>>(),
        },
        object! { "work": &total },
    ));
    v
}

fn tables() -> Vec<Vector> {
    const G: &str = "v9-tables";
    let mut v = Vec::new();
    // Memory prices for every page count from 0 to 65,536, as the runs of
    // page counts that share them.
    let mut runs: Vec<(u64, u64, [u64; 4])> = Vec::new();
    for pages in 0..=65_536u64 {
        let prices = memory_prices(pages);
        match runs.last_mut() {
            Some((_, to, p)) if *p == prices => *to = pages,
            _ => runs.push((pages, pages, prices)),
        }
    }
    v.push(vector(
        G,
        "memory-prices",
        "load, store, word and span prices for every memory size from 0 to 65,536 pages: three steps to 2 MiB, then the curve",
        "15.2",
        object! { "pages": "0 to 65536" },
        object! {
            "runs": runs.iter().map(|(from, to, [load, store, word, span])| object! {
                "pages_from": *from,
                "pages_to": *to,
                "load": *load,
                "store": *store,
                "word": *word,
                "span": *span,
            }).collect::<Vec<_>>(),
        },
    ));
    let lengths: Vec<u64> = vec![0, 1, 54, 55, 56, 119, 120, 1 << 20, u64::from(u32::MAX)];
    v.push(vector(
        G,
        "hashing",
        "a SHA-256 over m bytes costs 100 + 900 * ceil((m + 9) / 64)",
        "15.5",
        object! { "lengths": lengths.iter().map(|m| Json::from(*m)).collect::<Vec<_>>() },
        object! { "compute": lengths.iter().map(|m| Json::from(sha(*m))).collect::<Vec<_>>() },
    ));
    // Debit sequences near the counters' limits: each step's outcome and the
    // root's use after it.
    let max = u64::MAX;
    let sequences: Vec<(&str, [u64; 4], Vec<Step>)> = vec![
        (
            "meter-a-root-near-2-to-the-64",
            [max; 4],
            vec![
                Step::Charge([max - 1, 0, 0, 0]),
                Step::Charge([1, 0, 0, 0]),
                Step::Charge([1, 0, 0, 0]),
                Step::Charge([0, max, max, max]),
                Step::Charge([0, 1, 0, 0]),
            ],
        ),
        (
            "meter-nested-frames",
            [10, 2, 10, 10],
            vec![
                Step::Charge([2, 1, 0, 0]),
                Step::Open([5, 1, 5, 5]),
                Step::Charge([3, 1, 2, 1]),
                Step::Charge([3, 1, 0, 0]),
                Step::Close,
                Step::Open([6, 0, 0, 0]),
                Step::Open([5, 0, 8, 9]),
                Step::Close,
            ],
        ),
        (
            "meter-each-insufficiency-at-once",
            [5, 5, 5, 5],
            vec![
                Step::Charge([6, 6, 6, 6]),
                Step::Charge([5, 6, 6, 6]),
                Step::Charge([5, 5, 6, 6]),
                Step::Charge([5, 5, 5, 6]),
                Step::Charge([5, 5, 5, 5]),
                Step::Charge([0, 0, 0, 0]),
                Step::Charge([0, 0, 0, 1]),
            ],
        ),
        (
            "meter-a-child-budget-at-and-past-what-is-left",
            [100, 100, 100, 100],
            vec![
                Step::Charge([40, 30, 20, 10]),
                Step::Open([61, 70, 80, 90]),
                Step::Open([60, 71, 80, 90]),
                Step::Open([60, 70, 81, 90]),
                Step::Open([60, 70, 80, 91]),
                Step::Open([60, 70, 80, 90]),
                Step::Charge([60, 70, 80, 90]),
                Step::Open([0, 0, 0, 0]),
                Step::Charge([0, 0, 0, 1]),
                Step::Close,
                Step::Close,
            ],
        ),
        (
            "meter-a-child-of-a-root-near-2-to-the-64",
            [max; 4],
            vec![
                Step::Charge([max - 5, 0, 0, 0]),
                Step::Open([6, max, max, max]),
                Step::Open([5, max, max, max]),
                Step::Charge([5, max, 0, 0]),
                Step::Charge([1, 0, 0, 0]),
                Step::Close,
            ],
        ),
    ];
    for (name, budget, steps) in sequences {
        let mut budgets = Budgets(vec![(budget, [0; 4])]);
        let outcomes: Vec<Json> = steps
            .iter()
            .map(|step| {
                let outcome = budgets.run(step);
                let (what, debit) = match step {
                    Step::Charge(d) => ("charge", Json::from(*d)),
                    Step::Open(b) => ("open", Json::from(*b)),
                    Step::Close => ("close", Json::Bool(true)),
                };
                Json::Object(vec![
                    (what.to_string(), debit),
                    ("outcome".to_string(), outcome),
                    ("root_used_after".to_string(), Json::from(budgets.0[0].1)),
                ])
            })
            .collect();
        v.push(vector(
            G,
            name,
            "a debit fits every open frame or changes nothing, C, S, D, G checked in order; a nested budget opens within what is left; closing a frame returns its use",
            "11.2",
            object! { "root_budget": budget },
            object! { "steps": outcomes },
        ));
    }
    let heights: Vec<u64> = vec![0, 1, 8_192, 8_193, 16_384, 16_385, 1 << 32, u64::MAX];
    v.push(vector(
        G,
        "stack-pages",
        "a stack of h slots pays ceil(h / 8192) - 1 pages, none for the first 64 KiB",
        "15.4",
        object! { "heights": heights.iter().map(|x| Json::from(*x)).collect::<Vec<_>>() },
        object! { "pages": heights.iter().map(|x| Json::from(stack_pages(*x))).collect::<Vec<_>>() },
    ));
    v
}
