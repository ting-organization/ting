//! What a node holds at once of the values a block deletes. The count this
//! file bounds, `store::gathered`, is the whole process's, so the file holds
//! one test.
mod support;
use serde_json::json;
use std::sync::{Arc, atomic::AtomicBool};
use support::Directory;
use ting_core::{
    consensus,
    hash::{Hash, hex},
    meter::Resources,
    protocol::{Block, Encode, Genesis, Header, Payload, Transaction},
    state::{State, record_key},
    vm::{Context, Runtime},
};
use ting_node::{
    chain::Chain,
    config,
    store::{self, PART_BYTES, Store},
};

/// The bytes of each record: four parts, the last one short.
const VALUE: usize = 3 * PART_BYTES + 4096;
/// The records one block deletes.
const COUNT: u8 = 4;
/// Byte 1 of the input names a record by the first byte of its key. Byte 0
/// says what happens to it: 1 writes `VALUE` bytes, each the record's name,
/// and anything else, the deployment's empty input included, deletes it.
const PROGRAM: &str = r#"(module
  (import "ting" "input" (func (param i32 i32) (result i32)))
  (import "ting" "storage_set" (func (param i32 i32 i32)))
  (import "ting" "storage_delete" (func (param i32)))
  (memory (export "memory") 50 50)
  (func (export "invoke")
    (drop (call 0 (i32.const 0) (i32.const 2)))
    (i32.store8 (i32.const 32) (i32.load8_u (i32.const 1)))
    (if (i32.eq (i32.load8_u (i32.const 0)) (i32.const 1))
      (then
        (memory.fill (i32.const 65536) (i32.load8_u (i32.const 1)) (i32.const 3149824))
        (call 1 (i32.const 32) (i32.const 65536) (i32.const 3149824)))
      (else (call 2 (i32.const 32))))))"#;

fn open(dir: &Directory) -> Chain {
    Chain::open(&dir.0, Arc::new(AtomicBool::new(false))).unwrap()
}
fn mine(
    runtime: &Runtime,
    g: &Genesis,
    history: &mut Vec<Header>,
    transactions: Vec<Transaction>,
    producer: &str,
) -> Block {
    let mut b = runtime
        .build_candidate(
            g,
            history,
            transactions,
            g.timestamp + 60 * (history.len() as u64 + 1),
            [0; 4],
            producer.as_bytes().to_vec(),
        )
        .unwrap();
    while !consensus::valid_pow(&b.header) {
        b.header.nonce += 1;
    }
    runtime.validate_and_apply_block(g, history, &b).unwrap();
    history.push(b.header.clone());
    b
}
/// A runtime and history at the tip of `blocks`.
fn replayed(g: &Genesis, blocks: &[Block]) -> (Runtime, Vec<Header>) {
    let runtime = Runtime::new(State::default(), Context::from_genesis(g));
    let mut history = vec![];
    for block in blocks {
        runtime
            .validate_and_apply_block(g, &history, block)
            .unwrap();
        history.push(block.header.clone());
    }
    (runtime, history)
}
fn call(target: Hash, input: Vec<u8>) -> Transaction {
    Transaction {
        budget: Resources([1_000_000_000, 16, 4_096, 1 << 22]),
        payload: Payload::Call { target, input },
    }
}
fn state_root(chain: &mut Chain) -> String {
    chain.request("status", json!({})).unwrap()["state_root"]
        .as_str()
        .unwrap()
        .to_owned()
}
/// Imports `blocks` and waits for the chain to reach the last one, through a
/// replay if the branch forks below the undo window.
fn follow(chain: &mut Chain, blocks: &[Block]) {
    for block in blocks {
        chain.import(&block.encode(), true).unwrap();
    }
    let tip = blocks.last().unwrap().header.id();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while chain.tip() != tip {
        assert!(std::time::Instant::now() < deadline, "no reorganization");
        chain.resume_activation();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn a_block_that_deletes_large_records_moves_their_values_a_part_at_a_time() {
    assert_eq!(VALUE, 3_149_824, "the program writes this many bytes");
    // A chain whose blocks have room to write the records in one block: what
    // an attacker gathers over many blocks under production capacities.
    let dir = Directory::new();
    let g = Genesis {
        initial: Resources([4_000_000_000, 1_024, 1 << 20, 1 << 24]),
        ..support::test_genesis()
    };
    let c = config::Config {
        name: "memory".into(),
        control: "127.0.0.1:0".parse().unwrap(),
        p2p: "127.0.0.1:0".parse().unwrap(),
        api: "127.0.0.1:0".parse().unwrap(),
        undo_window: 3,
        ..Default::default()
    };
    config::initialize_with(&dir.0, &c, g.clone()).unwrap();
    let mut chain = open(&dir);

    // Block 1 deploys the program, block 2 writes the records, block 3
    // deletes them all.
    let a = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut ah = vec![];
    let deploy = Transaction {
        budget: Resources([100_000_000, 16, 4_096, 4_096]),
        payload: Payload::Deploy {
            code: wat::parse_str(PROGRAM).unwrap(),
            input: vec![],
        },
    };
    let a1 = mine(&a, &g, &mut ah, vec![deploy], "A");
    assert_eq!(a1.receipts[0].status as u8, 0, "{:?}", a1.receipts[0]);
    let program: Hash = a1.receipts[0].result.as_slice().try_into().unwrap();
    let all = |op: u8| (0..COUNT).map(|n| call(program, vec![op, n])).collect();
    let a2 = mine(&a, &g, &mut ah, all(1), "A");
    assert!(a2.receipts.iter().all(|r| r.status as u8 == 0));
    let written = hex(&a.state_root());
    let a3 = mine(&a, &g, &mut ah, all(0), "A");
    assert!(a3.receipts.iter().all(|r| r.status as u8 == 0));
    let deleted = hex(&a.state_root());
    assert_ne!(written, deleted);
    follow(&mut chain, &[a1.clone(), a2.clone()]);
    assert_eq!(state_root(&mut chain), written);

    // From here on no step may read a value whole: connecting the block
    // that deletes the records keeps their values as its undo data;
    let before = store::gathered();
    follow(&mut chain, std::slice::from_ref(&a3));
    assert_eq!(state_root(&mut chain), deleted);
    assert_eq!(store::gathered(), before, "connecting gathered values");

    // a heavier branch from block 2 disconnects it, which gives the records
    // their values back;
    let (b, mut bh) = replayed(&g, &[a1.clone(), a2.clone()]);
    let b_blocks: Vec<Block> = (0..2).map(|_| mine(&b, &g, &mut bh, vec![], "B")).collect();
    follow(&mut chain, &b_blocks);
    assert_eq!(state_root(&mut chain), written);
    assert_eq!(store::gathered(), before, "disconnecting gathered values");

    // and a longer A connects it again, by its redo data.
    let longer: Vec<Block> = (0..2).map(|_| mine(&a, &g, &mut ah, vec![], "A")).collect();
    follow(&mut chain, &longer);
    assert_eq!(state_root(&mut chain), deleted);
    assert_eq!(store::gathered(), before, "reconnecting gathered values");

    // The same across a restart, and once the block's undo data is pruned:
    // four more blocks leave it below the window of three, and a branch
    // that forks under it is reached by a replay from genesis, which
    // executes the block in a database of its own, keeps its undo data
    // there, and copies it back.
    let more: Vec<Block> = (0..4).map(|_| mine(&a, &g, &mut ah, vec![], "A")).collect();
    follow(&mut chain, &more);
    assert_eq!(
        chain.request("status", json!({})).unwrap()["undo_pruned"],
        6
    );
    drop(chain);
    let mut chain = open(&dir);
    assert_eq!(state_root(&mut chain), deleted);
    let more: Vec<Block> = (0..8).map(|_| mine(&b, &g, &mut bh, vec![], "B")).collect();
    follow(&mut chain, &more);
    assert_eq!(chain.height(), 12);
    assert_eq!(state_root(&mut chain), written);
    assert_eq!(store::gathered(), before, "the replay gathered values");

    // The records are back as they were written, byte for byte.
    drop(chain);
    let store = Store::open(&dir.0, &g, store::CACHE_BYTES).unwrap();
    let state = store.snapshot().unwrap();
    for n in 0..COUNT {
        let mut key = [0; 32];
        key[0] = n;
        let value = state.get(&record_key(2, program, key)).unwrap();
        assert!(value.len() == VALUE && value.iter().all(|byte| *byte == n));
    }
    assert_eq!(
        store::gathered(),
        before + u64::from(COUNT) * VALUE as u64,
        "a read of a record gathers its value"
    );
}
