use num_bigint::BigUint;
use serde_json::Value;
use ting_core::{
    consensus,
    hash::{hash, hex, vector_root},
    meter::{Meter, Resources},
    protocol::{BLOCK_BASE, Block, Encode, Genesis, Header, Receipt, Transaction},
    state::{State, record_key},
    vm::{Context, Runtime},
};

fn vectors() -> Value {
    serde_json::from_str(include_str!("../node/wiki/reference/protocol-vectors.json")).unwrap()
}
fn bytes(s: &str) -> Vec<u8> {
    assert_eq!(s.len() % 2, 0);
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
fn raw(v: &Value) -> Vec<u8> {
    bytes(v.as_str().unwrap())
}

#[test]
fn canonical_vectors_roundtrip_and_execute() {
    let v = vectors();
    let genesis = Genesis::decode(&raw(&v["genesis"]["config_hex"])).unwrap();
    assert_eq!(genesis, Genesis::experimental());
    assert_eq!(hex(&genesis.chain_id()), v["genesis"]["chain_id"]);
    assert_eq!(hex(&genesis.block_id()), v["genesis"]["genesis_block_id"]);
    for sample in [
        &v["empty_block"],
        &v["producer_data_block"],
        &v["missing_program_transaction"]["block"],
    ] {
        let header = Header::decode(&raw(&sample["header_hex"])).unwrap();
        assert_eq!(hex(&header.id()), sample["block_id"]);
        assert!(consensus::valid_pow(&header));
        assert_eq!(consensus::work(&header.target), BigUint::from(4u32));
        let mut encoded = header.encode();
        encoded.extend(raw(&sample["body_hex"]));
        let block = Block::decode(&encoded, 1_048_576).unwrap();
        assert_eq!(block.encode(), encoded);
        assert_eq!(hex(&block.producer), sample["producer_hex"]);
        assert_eq!(hex(&header.producer_hash), sample["producer_hash"]);
        assert_eq!(
            block.header.used.0[2],
            BLOCK_BASE
                + block.producer.len() as u64
                + sample["resources_used_C_S_D_G"][2].as_u64().unwrap()
                - BLOCK_BASE
                - block.producer.len() as u64
        );
        let runtime = Runtime::new(State::default(), Context::from_genesis(&genesis));
        runtime
            .validate_and_apply_block(&genesis, &[], &block)
            .unwrap();
        assert_eq!(hex(&runtime.state_root()), sample["state_root"]);
    }
    let sample = &v["missing_program_transaction"];
    let t = Transaction::decode(&raw(&sample["transaction_hex"]), 1024).unwrap();
    let r = Receipt::decode(&raw(&sample["receipt_hex"]), 1024).unwrap();
    assert_eq!(r.transaction, t.id(genesis.chain_id()));
}
#[test]
fn domain_separation_and_vector_padding() {
    let v = vectors();
    assert_eq!(hex(&hash("example", &[])), v["hashes"]["H_example_empty"]);
    assert_eq!(hex(&hash("example", b"abc")), v["hashes"]["H_example_abc"]);
    assert_eq!(
        hex(&hash("producer", &[])),
        v["hashes"]["empty_producer_hash"]
    );
    assert_eq!(
        hex(&vector_root(
            "transactions",
            &[b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]
        )),
        v["hashes"]["transactions_three_items_61_62_63"]
    );
    assert_ne!(hash("ab", b"c"), hash("a", b"bc"));
    assert_ne!(
        vector_root("transactions", &[vec![1], vec![2]]),
        vector_root("transactions", &[vec![2], vec![1]])
    );
    assert_ne!(
        vector_root("transactions", &[vec![1], vec![2], vec![3]]),
        vector_root("transactions", &[vec![1], vec![2], vec![3], vec![3]])
    );
}
#[test]
fn incremental_tree_and_nested_journal() {
    let v = vectors();
    let mut state = State::default();
    assert_eq!(hex(&state.root()), v["hashes"]["empty_state_root"]);
    let key = record_key(2, [0x11; 32], [0x22; 32]);
    assert_eq!(hex(&key), v["state_tree"]["record_key"]);
    let root = state.root();
    let outer = state.checkpoint();
    state.set(key, Some(Vec::new()));
    assert_eq!(
        hex(&state.root()),
        v["state_tree"]["single_present_empty_value_root"]
    );
    assert_eq!(state.get(&key), Some(vec![]));
    let nested = state.checkpoint();
    state.set(key, Some(b"a".to_vec()));
    assert_eq!(hex(&state.root()), v["state_tree"]["single_value_61_root"]);
    state.rollback(nested);
    assert_eq!(state.get(&key), Some(vec![]));
    state.set([7; 32], Some(vec![3]));
    state.rollback(outer);
    assert_eq!(state.root(), root);
    assert_eq!(state.get(&key), None);
    state.commit();
}
#[test]
fn records_split_and_move_up_as_the_vectors_say() {
    let v = vectors();
    let tree = &v["state_tree"];
    let key = |name: &str| -> [u8; 32] { raw(&tree[name]).try_into().unwrap() };
    let (low, high, last) = (key("key_low"), key("key_high"), key("key_last_bit"));
    let mut state = State::default();
    state.set(low, Some(b"a".to_vec()));
    assert_eq!(hex(&state.root()), tree["low_root"]);
    state.set(high, Some(b"b".to_vec()));
    assert_eq!(hex(&state.root()), tree["low_high_root"]);
    state.set(last, Some(b"c".to_vec()));
    assert_eq!(hex(&state.root()), tree["low_high_last_bit_root"]);
    state.set(high, None);
    assert_eq!(hex(&state.root()), tree["after_deleting_high_root"]);
    state.set(last, None);
    assert_eq!(
        hex(&state.root()),
        tree["after_deleting_high_and_last_bit_root"]
    );
    assert_eq!(state.len(), 1);
}
#[test]
fn tree_is_insertion_order_independent_and_deletion_restores() {
    let mut a = State::default();
    let mut b = State::default();
    let keys = [[0; 32], [255; 32], [128; 32], [1; 32]];
    for key in keys {
        a.set(key, Some(vec![key[0]]));
    }
    for key in keys.into_iter().rev() {
        b.set(key, Some(vec![key[0]]));
    }
    assert_eq!(a.root(), b.root());
    for key in keys {
        a.set(key, None);
    }
    assert_eq!(a.root(), State::default().root());
}
#[test]
fn failed_atomic_debit_and_children_preserve_consumption() {
    let mut meter = Meter::new(Resources([10, 2, 10, 10]));
    meter.charge(Resources([2, 1, 0, 0])).unwrap();
    assert!(meter.push(Resources([5, 1, 5, 5])));
    meter.charge(Resources([3, 1, 2, 1])).unwrap();
    let before = meter.used();
    assert_eq!(meter.charge(Resources([3, 1, 0, 0])).unwrap_err().0, 0);
    assert_eq!(meter.used(), before);
    assert_eq!(meter.pop(), Resources([3, 1, 2, 1]));
    assert_eq!(meter.used(), Resources([5, 2, 2, 1]));
    assert!(!meter.push(Resources([6, 0, 0, 0])));
    let mut extreme = Meter::new(Resources([u64::MAX, 0, 0, 0]));
    extreme.charge(Resources::compute(u64::MAX)).unwrap();
    assert!(extreme.charge(Resources::compute(1)).is_err());
}
#[test]
fn difficulty_and_voting_vectors() {
    let v = vectors();
    for case in v["difficulty"]["cases"].as_array().unwrap() {
        let result = consensus::asert(
            &BigUint::from(case["anchor_target"].as_u64().unwrap()),
            case["interval"].as_u64().unwrap() as u32,
            case["half_life"].as_u64().unwrap() as u32,
            case["blocks"].as_u64().unwrap(),
            case["elapsed"].as_i64().unwrap() as i128,
            &BigUint::from(case["pow_limit"].as_u64().unwrap()),
        );
        assert_eq!(
            result,
            BigUint::from(case["new_target"].as_u64().unwrap()),
            "{case}"
        );
    }
    // The exponent is floored, not truncated as in aserti3-2d.
    let case = &v["difficulty"]["rounding_case"];
    let big = |key: &str| BigUint::from_bytes_be(&raw(&case[key]));
    assert_eq!(
        consensus::asert(
            &big("anchor_target_hex"),
            60,
            600,
            1,
            59,
            &big("pow_limit_hex")
        ),
        big("new_target_hex")
    );
    let limit = Genesis::experimental().pow_limit;
    assert_eq!(
        consensus::asert(&BigUint::from(1u32), 60, 600, 10, 1_000_000_000, &limit),
        limit
    );
    assert_eq!(
        consensus::asert(&limit, 60, 600, 10, -1_000_000_000, &limit),
        BigUint::from(1u32)
    );
    // Experimental genesis: on-schedule timestamps keep the initial target exactly.
    let g = Genesis::experimental();
    let base = Header::decode(&raw(&v["empty_block"]["header_hex"])).unwrap();
    let mut history = Vec::new();
    for h in 1..=5 {
        let mut header = base.clone();
        header.height = h;
        header.timestamp = g.timestamp + 60 * h;
        assert_eq!(consensus::expected_target(&g, &history), g.initial_target);
        history.push(header);
    }
    history[4].timestamp = g.timestamp + 60 * 5 + 172_800;
    assert_eq!(
        consensus::expected_target(&g, &history),
        &g.initial_target * 2u32
    );
    for (signals, direction) in [
        (&v["voting"]["weighted_threshold_example"], 1),
        (&v["voting"]["below_threshold_example"], 0),
        (&v["voting"]["majority_decrease_example"], 2),
        (&v["voting"]["exact_half_holds_example"], 0),
    ] {
        let signals: Vec<(BigUint, u8)> = signals["work_and_signal"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    BigUint::from(s[0].as_u64().unwrap()),
                    s[1].as_u64().unwrap() as u8,
                )
            })
            .collect();
        assert_eq!(consensus::vote_direction(&signals), direction);
    }
    assert_eq!(consensus::adjust(109, 1, 90), 114);
    assert_eq!(consensus::adjust(91, 2, 90), 90);
    assert_eq!(consensus::adjust(u64::MAX, 1, 1), u64::MAX);
    let mut g = Genesis::experimental();
    g.initial = Resources([100; 4]);
    g.minimum = Resources([90; 4]);
    let mut history = Vec::new();
    for h in 1..=20 {
        let mut header = base.clone();
        header.height = h;
        header.votes = if h <= 4 {
            [1; 4]
        } else if h <= 8 {
            [0; 4]
        } else {
            [2; 4]
        };
        history.push(header);
    }
    assert_eq!(
        consensus::active_limits(&g, &history[..12]),
        Resources([105; 4])
    );
    assert_eq!(
        consensus::active_limits(&g, &history[..16]),
        Resources([105; 4])
    );
    assert_eq!(consensus::active_limits(&g, &history), Resources([95; 4]));
    assert_eq!(
        consensus::active_limits(&g, &history[..8]),
        Resources([100; 4])
    );
}
#[test]
fn wire_rejects_truncation_trailing_data_and_allocation_bombs() {
    let v = vectors();
    let encoded = raw(&v["missing_program_transaction"]["transaction_hex"]);
    for n in 0..encoded.len() {
        assert!(Transaction::decode(&encoded[..n], 1024).is_err());
    }
    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(Transaction::decode(&trailing, 1024).is_err());
    let mut bomb = encoded;
    bomb[66..70].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Transaction::decode(&bomb, 1024).is_err());
    let mut header = raw(&v["empty_block"]["header_hex"]);
    header[242] = 3;
    assert!(Header::decode(&header).is_err());
    let mut g = Genesis::experimental();
    g.minimum.0[2] = BLOCK_BASE - 1;
    assert!(g.validate().is_err());
    let mut g = Genesis::experimental();
    g.half_life = 0;
    assert!(g.validate().is_err());
}

/// An empty block on `history`, mined to its target and applied to `runtime`.
fn mined(runtime: &Runtime, g: &Genesis, history: &[Header], timestamp: u64) -> Block {
    let mut block = runtime
        .build_candidate(g, history, vec![], timestamp, [0; 4], vec![])
        .unwrap();
    while !consensus::valid_pow(&block.header) {
        block.header.nonce += 1;
    }
    block
}
/// A named change to a value, for tests of what validation refuses.
type Fault<T> = (&'static str, fn(&mut T));
type HeaderFault = (&'static str, Box<dyn Fn(&mut Header)>);
fn remine(header: &mut Header) {
    while !consensus::valid_pow(header) {
        header.nonce += 1;
    }
}

#[test]
fn header_validation_rejects_each_fault() {
    let g = Genesis::experimental();
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut history = Vec::new();
    // At height 1 the only earlier timestamp is the genesis's: equal is too early.
    let mut first = mined(&runtime, &g, &history, g.timestamp + 1).header;
    assert!(consensus::validate_header(&g, &history, &first).is_ok());
    first.timestamp = g.timestamp;
    remine(&mut first);
    assert_eq!(
        consensus::validate_header(&g, &history, &first)
            .unwrap_err()
            .0,
        "median timestamp"
    );
    for (step, timestamp) in [(1, g.timestamp + 60), (2, g.timestamp + 300)] {
        let block = mined(&runtime, &g, &history, timestamp);
        assert_eq!(block.header.height, step);
        runtime
            .validate_and_apply_block(&g, &history, &block)
            .unwrap();
        history.push(block.header);
    }
    let good = mined(&runtime, &g, &history, g.timestamp + 360).header;
    assert!(consensus::validate_header(&g, &history, &good).is_ok());
    let limits = consensus::active_limits(&g, &history);
    let mut other = g.clone();
    other.timestamp += 1;
    let (t1, t2) = (history[0].timestamp, history[1].timestamp);
    let faults: Vec<HeaderFault> = vec![
        (
            "header ancestry",
            Box::new(move |h| h.chain_id = other.chain_id()),
        ),
        ("header ancestry", Box::new(|h| h.parent = [1; 32])),
        ("header ancestry", Box::new(|h| h.height += 1)),
        // Three samples (genesis, t1, t2): the median is t1, and the header
        // must be strictly later than it.
        ("median timestamp", Box::new(move |h| h.timestamp = t1)),
        ("difficulty target", Box::new(|h| h.target += 1u32)),
        ("header fields", Box::new(|h| h.votes = [0, 0, 0, 3])),
        (
            "block resource limits",
            Box::new(move |h| h.used.0[0] = limits.0[0] + 1),
        ),
    ];
    for (expected, fault) in faults {
        let mut header = good.clone();
        fault(&mut header);
        remine(&mut header);
        assert_eq!(
            consensus::validate_header(&g, &history, &header)
                .unwrap_err()
                .0,
            expected
        );
    }
    // Later than the median is enough, even earlier than the parent.
    let mut early = good.clone();
    early.timestamp = t1 + 1;
    assert!(t1 + 1 < t2);
    remine(&mut early);
    assert!(consensus::validate_header(&g, &history, &early).is_ok());
    // The proof of work itself.
    let mut unsolved = good.clone();
    while consensus::valid_pow(&unsolved) {
        unsolved.nonce += 1;
    }
    assert_eq!(
        consensus::validate_header(&g, &history, &unsolved)
            .unwrap_err()
            .0,
        "proof of work"
    );
    assert!(!consensus::valid_pow(&Header {
        target: BigUint::ZERO,
        ..good.clone()
    }));
    assert!(!consensus::valid_pow(&Header {
        target: BigUint::from(1u32) << 256usize,
        ..good.clone()
    }));
    assert_eq!(
        consensus::work(&((BigUint::from(1u32) << 256usize) - 1u32)),
        BigUint::from(1u32)
    );
}

#[test]
fn wire_formats_hold_their_versions_bounds_and_counts() {
    use ting_core::{
        meter::Resources,
        protocol::{Event, GENESIS_LEN, HEADER_LEN, Payload, Status},
    };
    let v = vectors();
    // The reader's byte limit is exact.
    let call = Transaction {
        budget: Resources([0, 0, 1000, 0]),
        payload: Payload::Call {
            target: [7; 32],
            input: vec![1, 2, 3],
        },
    };
    let encoded = call.encode();
    assert_eq!(Transaction::decode(&encoded, encoded.len()).unwrap(), call);
    assert_eq!(
        Transaction::decode(&encoded, encoded.len() - 1)
            .unwrap_err()
            .0,
        "wire byte limit"
    );
    // Versions and kinds.
    for (byte, value, error) in [
        (0, 0, "transaction version"),
        (0, 2, "transaction version"),
        (1, 2, "transaction kind"),
    ] {
        let mut bad = encoded.clone();
        bad[byte] = value;
        assert_eq!(Transaction::decode(&bad, 1024).unwrap_err().0, error);
    }
    let deploy = Transaction {
        budget: Resources([5, 6, 1000, 8]),
        payload: Payload::Deploy {
            code: vec![1, 2, 3],
            input: vec![4],
        },
    };
    let encoded = deploy.encode();
    assert_eq!(Transaction::decode(&encoded, 1024).unwrap(), deploy);
    for n in 0..encoded.len() {
        assert!(Transaction::decode(&encoded[..n], 1024).is_err(), "{n}");
    }
    // The header: fixed length, versioned, and every field where the wiki
    // puts it, in C, S, D, G order.
    let base = Header::decode(&raw(&v["empty_block"]["header_hex"])).unwrap();
    let header = Header {
        votes: [1, 2, 0, 1],
        used: Resources([1, 2, 3, 4]),
        nonce: u128::MAX,
        ..base.clone()
    };
    let bytes = header.encode();
    assert_eq!(bytes.len(), HEADER_LEN);
    assert_eq!(Header::decode(&bytes).unwrap(), header);
    assert_eq!(bytes[..2], [1, 0]);
    for i in 0..4 {
        let field = &bytes[210 + 8 * i..218 + 8 * i];
        assert_eq!(u64::from_le_bytes(field.try_into().unwrap()), i as u64 + 1);
    }
    assert_eq!(bytes[242..246], [1, 2, 0, 1]);
    assert_eq!(bytes[278..294], u128::MAX.to_le_bytes());
    let mut long = bytes.clone();
    long.push(0);
    assert!(Header::decode(&long).is_err());
    assert!(Header::decode(&bytes[..HEADER_LEN - 1]).is_err());
    let mut versioned = bytes.clone();
    versioned[0] = 2;
    assert_eq!(Header::decode(&versioned).unwrap_err().0, "header version");
    // Genesis: every validation arm, and the exact data floor.
    let faults: [Fault<Genesis>; 6] = [
        ("interval", |g| g.interval = 0),
        ("vote epoch", |g| g.vote_epoch = 0),
        ("zero target", |g| g.initial_target = BigUint::ZERO),
        ("target above limit", |g| {
            g.initial_target = g.pow_limit.clone() + 1u32
        }),
        ("state floor", |g| g.minimum.0[1] = 0),
        ("floor above initial", |g| {
            g.minimum.0[3] = g.initial.0[3] + 1
        }),
    ];
    for (name, fault) in faults {
        let mut g = Genesis::experimental();
        fault(&mut g);
        assert!(g.validate().is_err(), "{name}");
    }
    let mut floor = Genesis::experimental();
    floor.minimum.0[2] = BLOCK_BASE;
    floor.validate().unwrap();
    let g = Genesis::experimental();
    let bytes = g.encode();
    assert_eq!(bytes.len(), GENESIS_LEN);
    assert_eq!(Genesis::decode(&bytes).unwrap(), g);
    assert!(Genesis::decode(&bytes[..GENESIS_LEN - 1]).is_err());
    let mut long = bytes.clone();
    long.push(0);
    assert!(Genesis::decode(&long).is_err());
    let mut versioned = bytes.clone();
    versioned[0] = 2;
    assert!(Genesis::decode(&versioned).is_err());
    // Receipts: a result and events round-trip; a failed receipt carries no
    // events, no status byte beyond the twelve exists, and an event count
    // larger than the bytes left is refused before anything is allocated.
    let receipt = Receipt {
        transaction: [9; 32],
        status: Status::Success,
        used: Resources([1, 2, 3, 4]),
        result: vec![5, 6],
        events: vec![
            Event {
                contract: [1; 32],
                payload: vec![],
            },
            Event {
                contract: [2; 32],
                payload: vec![7; 3],
            },
        ],
    };
    let bytes = receipt.encode();
    assert_eq!(Receipt::decode(&bytes, 1024).unwrap(), receipt);
    for byte in 0..12u8 {
        assert_eq!(Status::from_byte(byte).unwrap() as u8, byte);
    }
    assert_eq!(Status::from_byte(12).unwrap_err().0, "receipt status");
    let mut bad = bytes.clone();
    bad[32] = 12;
    assert_eq!(Receipt::decode(&bad, 1024).unwrap_err().0, "receipt status");
    let mut failed = bytes.clone();
    failed[32] = Status::Revert as u8;
    assert_eq!(
        Receipt::decode(&failed, 1024).unwrap_err().0,
        "failed receipt events"
    );
    let bare = Receipt {
        events: vec![],
        ..receipt.clone()
    }
    .encode();
    let mut bomb = bare.clone();
    let n = bomb.len();
    bomb[n - 4..].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(Receipt::decode(&bomb, 1024).unwrap_err().0, "event count");
    // Blocks: a body with transactions and receipts round-trips; trailing
    // bytes, truncation, an impossible count and a padded inner frame fail.
    let producer = b"fixture miner".to_vec();
    let block = Block {
        header: Header {
            producer_hash: ting_core::protocol::producer_hash(&producer),
            ..base
        },
        producer,
        transactions: vec![call.clone()],
        receipts: vec![Receipt {
            transaction: call.id(g.chain_id()),
            ..receipt
        }],
    };
    let bytes = block.encode();
    assert_eq!(Block::decode(&bytes, 1 << 20).unwrap(), block);
    assert_eq!(
        Block::decode(&bytes, bytes.len() - 1).unwrap_err().0,
        "wire byte limit"
    );
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(
        Block::decode(&trailing, 1 << 20).unwrap_err().0,
        "trailing bytes"
    );
    for n in [HEADER_LEN, HEADER_LEN + 4, bytes.len() - 1] {
        assert!(Block::decode(&bytes[..n], 1 << 20).is_err(), "{n}");
    }
    let count = HEADER_LEN + 4 + 13;
    let mut bomb = bytes.clone();
    bomb[count..count + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        Block::decode(&bomb, 1 << 20).unwrap_err().0,
        "transaction count"
    );
    let frame = count + 4;
    let mut padded = bytes.clone();
    let inner = u32::from_le_bytes(padded[frame..frame + 4].try_into().unwrap());
    padded[frame..frame + 4].copy_from_slice(&(inner + 1).to_le_bytes());
    padded.insert(frame + 4 + inner as usize, 0);
    assert_eq!(
        Block::decode(&padded, 1 << 20).unwrap_err().0,
        "trailing bytes"
    );
}

#[test]
fn voting_weighs_work_per_resource_and_activates_on_epoch_boundaries() {
    let v = vectors();
    let base = Header::decode(&raw(&v["empty_block"]["header_hex"])).unwrap();
    // Work, not head count, decides.
    let w = |n: u32| BigUint::from(n);
    assert_eq!(consensus::vote_direction(&[]), 0);
    assert_eq!(
        consensus::vote_direction(&[(w(1), 1), (w(1), 1), (w(10), 0)]),
        0
    );
    assert_eq!(
        consensus::vote_direction(&[(w(10), 1), (w(1), 0), (w(1), 0)]),
        1
    );
    assert_eq!(consensus::vote_direction(&[(w(10), 2), (w(9), 0)]), 2);
    assert_eq!(consensus::vote_direction(&[(w(10), 2), (w(10), 0)]), 0);
    // Steps round down but never vanish, and the floor holds.
    for (value, vote, minimum, expected) in [
        (1, 1, 1, 2),
        (19, 1, 1, 20),
        (20, 1, 1, 21),
        (39, 1, 1, 40),
        (40, 1, 1, 42),
        (100, 0, 90, 100),
        (100, 2, 90, 90),
        (100, 2, 80, 90),
        (9, 2, 1, 8),
        (10, 2, 1, 9),
        (19, 2, 1, 18),
        (20, 2, 1, 18),
        (2, 2, 1, 1),
        (1, 2, 1, 1),
    ] {
        assert_eq!(
            consensus::adjust(value, vote, minimum),
            expected,
            "{value} {vote}"
        );
    }
    // Each resource is voted separately, and the first activation is the
    // fourth epoch: a history of 11 blocks is still on the initial limits.
    let mut g = Genesis::experimental();
    g.initial = Resources([100; 4]);
    g.minimum = Resources([90, 90, 99, 90]);
    let history: Vec<Header> = (1..=20)
        .map(|h| Header {
            height: h,
            votes: [1, 0, 2, 0],
            ..base.clone()
        })
        .collect();
    assert_eq!(
        consensus::active_limits(&g, &history[..11]),
        Resources([100; 4])
    );
    assert_eq!(
        consensus::active_limits(&g, &history[..12]),
        Resources([105, 100, 99, 100])
    );
    assert_eq!(
        consensus::active_limits(&g, &history[..15]),
        Resources([105, 100, 99, 100])
    );
    assert_eq!(
        consensus::active_limits(&g, &history[..16]),
        Resources([110, 100, 99, 100])
    );
    assert_eq!(
        consensus::active_limits(&g, &history[..19]),
        Resources([110, 100, 99, 100])
    );
    assert_eq!(
        consensus::active_limits(&g, &history),
        Resources([115, 100, 99, 100])
    );
}
#[test]
fn the_production_profile_is_the_one_the_wiki_states() {
    let g = Genesis::production(1_800_000_000);
    g.validate().unwrap();
    let one = BigUint::from(1u32);
    assert_eq!(g.initial_target, (&one << 224usize) - &one);
    assert_eq!(g.pow_limit, (&one << 228usize) - &one);
    assert_eq!(
        (g.interval, g.half_life, g.vote_epoch),
        (60, 43_200, 20_160)
    );
    assert_eq!(g.initial.0, [150_000_000, 1_024, 262_144, 131_072]);
    assert_eq!(g.minimum.0, [40_000_000, 64, 98_304, 98_304]);
}
