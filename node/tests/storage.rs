mod support;
use redb::{Database, ReadableDatabase};
use std::{
    fs::{self, OpenOptions},
    process::{Command, Stdio},
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
};
use support::Directory;
use ting_core::{
    consensus,
    hash::{Hash, hex},
    meter::Resources,
    protocol::{Block, Encode, Genesis, Header, Payload, Status, Transaction},
    state::{State, record_key},
    vm::{Context, Runtime},
};
use ting_node::{
    chain::Chain,
    config,
    store::{DELTAS, META, PART_BYTES, PARTS, Store},
};
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
    let mut block = runtime
        .build_candidate(
            g,
            history,
            transactions,
            g.timestamp + 60 * (history.len() as u64 + 1),
            [0; 4],
            producer.as_bytes().to_vec(),
        )
        .unwrap();
    while !consensus::valid_pow(&block.header) {
        block.header.nonce += 1;
    }
    runtime
        .validate_and_apply_block(g, history, &block)
        .unwrap();
    history.push(block.header.clone());
    block
}
fn program() -> Transaction {
    Transaction {
        budget: Resources([1_000_000, 128, 16384, 16384]),
        payload: Payload::Deploy {
            input: vec![],
            code: wat::parse_str(
                r#"(module
      (import "ting" "input" (func (param i32 i32) (result i32)))
      (import "ting" "storage_set" (func (param i32 i32 i32)))
      (import "ting" "storage_delete" (func (param i32)))
      (import "ting" "revert" (func (param i32 i32)))
      (memory (export "memory") 1 1)
      (func (export "invoke") (local i32)
        (local.set 0 (call 0 (i32.const 0) (i32.const 8192)))
        (if (local.get 0) (then
          (if (i32.eq (i32.load8_u (i32.const 0)) (i32.const 2))
            (then (call 2 (i32.const 1)))
            (else (call 1 (i32.const 1) (i32.const 33) (i32.sub (local.get 0) (i32.const 33)))))
          (if (i32.eq (i32.load8_u (i32.const 0)) (i32.const 3))
            (then (call 3 (i32.const 0) (i32.const 0))))))))"#,
            )
            .unwrap(),
        },
    }
}
fn call(target: Hash, key: u8, op: u8, value: &[u8]) -> Transaction {
    let mut input = vec![op];
    input.extend([key; 32]);
    input.extend(value);
    Transaction {
        budget: Resources([1_000_000, 128, 16384, 16384]),
        payload: Payload::Call { target, input },
    }
}
fn value(chain: &mut Chain, target: Hash, key: u8) -> serde_json::Value {
    chain
        .request(
            "storage",
            serde_json::json!({"id":hex(&target),"key":hex(&[key;32])}),
        )
        .unwrap()["value_hex"]
        .clone()
}

#[test]
fn undo_restores_updates_empty_values_deletes_and_deployments_across_restart() {
    let dir = Directory::new();
    dir.config(false, "undo");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let a = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut ah = vec![];
    let deployment = program();
    ting_core::vm::profile::validate(match &deployment.payload {
        Payload::Deploy { code, .. } => code,
        _ => unreachable!(),
    })
    .unwrap();
    let shared = mine(&a, &g, &mut ah, vec![deployment], "shared");
    assert_eq!(
        shared.receipts[0].status,
        Status::Success,
        "{:?}",
        shared.receipts[0]
    );
    let target: Hash = shared.receipts[0].result.clone().try_into().unwrap();
    chain.import(&shared.encode(), true).unwrap();
    let a2 = mine(
        &a,
        &g,
        &mut ah,
        vec![
            call(target, 1, 1, &[5]),
            call(target, 2, 1, &[]),
            call(target, 3, 1, &[8]),
            support::tx(),
        ],
        "A",
    );
    let removed: Hash = a2.receipts[3].result.clone().try_into().unwrap();
    chain.import(&a2.encode(), true).unwrap();
    let a3 = mine(
        &a,
        &g,
        &mut ah,
        vec![
            call(target, 1, 1, &[9]),
            call(target, 1, 1, &[10]),
            call(target, 2, 2, &[]),
            call(target, 3, 3, &[99]),
        ],
        "A",
    );
    assert_eq!(a3.receipts[3].status, Status::Revert);
    chain.import(&a3.encode(), true).unwrap();
    assert_eq!(value(&mut chain, target, 1), "0a");
    assert!(value(&mut chain, target, 2).is_null());
    assert_eq!(value(&mut chain, target, 3), "08");
    let before = chain.request("status", serde_json::json!({})).unwrap();
    drop(chain);
    {
        let db = Database::create(dir.0.join("chain.redb")).unwrap();
        let tx = db.begin_read().unwrap();
        let table = tx.open_table(DELTAS).unwrap();
        let mut key = a3.header.id().to_vec();
        key.extend(record_key(2, target, [1; 32]));
        assert_eq!(
            table.get(key.as_slice()).unwrap().unwrap().value(),
            &[1, 5],
            "one previous value for two successful writes"
        );
        let mut key = a3.header.id().to_vec();
        key.extend(record_key(2, target, [2; 32]));
        assert_eq!(
            table.get(key.as_slice()).unwrap().unwrap().value(),
            &[1],
            "an empty previous value differs from absence"
        );
        let mut key = a3.header.id().to_vec();
        key.extend(record_key(2, target, [3; 32]));
        assert!(
            table.get(key.as_slice()).unwrap().is_none(),
            "reverted write has no persisted change"
        );
        let mut key = a2.header.id().to_vec();
        key.extend(record_key(2, target, [2; 32]));
        assert_eq!(
            table.get(key.as_slice()).unwrap().unwrap().value(),
            &[0],
            "absent before"
        );
    }
    let mut chain = open(&dir);
    let b = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut bh = vec![];
    b.validate_and_apply_block(&g, &bh, &shared).unwrap();
    bh.push(shared.header.clone());
    let b2 = mine(
        &b,
        &g,
        &mut bh,
        vec![
            call(target, 1, 1, &[2]),
            call(target, 3, 2, &[]),
            call(target, 1, 1, &[4]),
        ],
        "B",
    );
    chain.import(&b2.encode(), true).unwrap();
    let b3 = mine(
        &b,
        &g,
        &mut bh,
        vec![call(target, 2, 1, &[]), call(target, 1, 2, &[])],
        "B",
    );
    chain.import(&b3.encode(), true).unwrap();
    assert_eq!(chain.tip(), a3.header.id(), "tie preserves the old state");
    assert_eq!(value(&mut chain, target, 1), "0a");
    drop(chain);
    let mut chain = open(&dir);
    assert_eq!(
        chain.request("status", serde_json::json!({})).unwrap()["state_root"],
        before["state_root"]
    );
    let b4 = mine(&b, &g, &mut bh, vec![call(target, 2, 1, &[7])], "B");
    let mut invalid = b4.clone();
    invalid.receipts[0].used.0[0] += 1;
    let size = fs::metadata(dir.0.join("blocks/00000000.dat"))
        .unwrap()
        .len();
    assert!(chain.import(&invalid.encode(), true).is_err());
    assert_eq!(chain.tip(), a3.header.id());
    assert_eq!(
        fs::metadata(dir.0.join("blocks/00000000.dat"))
            .unwrap()
            .len(),
        size
    );
    chain.import(&b4.encode(), true).unwrap();
    assert_eq!(chain.tip(), b4.header.id());
    assert!(value(&mut chain, target, 1).is_null());
    assert_eq!(value(&mut chain, target, 2), "07");
    assert!(value(&mut chain, target, 3).is_null());
    assert!(
        chain
            .request("contract", serde_json::json!({"id":hex(&removed)}))
            .is_err()
    );
    assert_eq!(
        chain.request("status", serde_json::json!({})).unwrap()["state_root"],
        hex(&b.state_root())
    );
    drop(chain);
    let mut chain = open(&dir);
    // Switch back through a persisted side branch; its ancestors use forward diffs.
    let a4 = mine(&a, &g, &mut ah, vec![], "A");
    chain.import(&a4.encode(), true).unwrap();
    assert_eq!(chain.tip(), b4.header.id());
    let a5 = mine(&a, &g, &mut ah, vec![], "A");
    chain.import(&a5.encode(), true).unwrap();
    assert_eq!(chain.tip(), a5.header.id());
    assert_eq!(value(&mut chain, target, 1), "0a");
    assert!(value(&mut chain, target, 2).is_null());
    assert_eq!(value(&mut chain, target, 3), "08");
    assert_eq!(value(&mut chain, removed, 0), "2a");
    assert_eq!(
        chain.request("status", serde_json::json!({})).unwrap()["state_root"],
        hex(&a.state_root())
    );
    drop(chain);
    let mut chain = open(&dir);
    assert_eq!(value(&mut chain, removed, 0), "2a");
}

#[test]
fn crash_writer() {
    let Some(dir) = std::env::var_os("TING_CRASH_DIRECTORY") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let g = config::genesis(&dir).unwrap();
    let store = Store::open(&dir, &g, ting_node::store::CACHE_BYTES).unwrap();
    let runtime = Runtime::new(store.snapshot().unwrap(), Context::from_genesis(&g));
    let block = mine(&runtime, &g, &mut vec![], vec![support::tx()], "crash");
    let tx = Rc::new(store.begin().unwrap());
    let staged = Runtime::new(Store::state(tx.clone()).unwrap(), Context::from_genesis(&g));
    staged.validate_and_apply_block(&g, &[], &block).unwrap();
    let changes = staged.state_changes().unwrap();
    Store::apply(&tx, &changes).unwrap();
    let location = store.append(&block.encode()).unwrap();
    let work = consensus::work(&block.header.target);
    Store::save_data(&tx, &block.header, &work, location).unwrap();
    store
        .save_valid(&tx, &block, &work, &changes, g.initial)
        .unwrap();
    Store::canonical(&tx, 0, &[block.header.id()], block.header.id()).unwrap();
    drop(staged);
    if std::env::var_os("TING_CRASH_AFTER_COMMIT").is_some() {
        Rc::try_unwrap(tx).ok().unwrap().commit().unwrap();
    }
    // Exit without running destructors: durable file, unpublished database transaction.
    std::process::exit(73);
}
#[test]
fn process_crash_recovers_durable_block_and_unpublished_state_atomically() {
    for committed in [false, true] {
        let dir = Directory::new();
        dir.config(false, "crash");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "crash_writer", "--nocapture"])
            .env("TING_CRASH_DIRECTORY", &dir.0)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        if committed {
            command.env("TING_CRASH_AFTER_COMMIT", "1");
        }
        assert_eq!(command.status().unwrap().code(), Some(73));
        let g = config::genesis(&dir.0).unwrap();
        {
            let store = Store::open(&dir.0, &g, ting_node::store::CACHE_BYTES).unwrap();
            assert_eq!(
                store.snapshot().unwrap().root() == State::default().root(),
                !committed
            );
            assert_eq!(
                store.metadata().unwrap().canonical.len(),
                usize::from(committed)
            );
            assert_eq!(store.pending().unwrap().is_some(), !committed);
        }
        let mut chain = open(&dir);
        assert_eq!(chain.height(), 1);
        let found = chain
            .request(
                "transaction",
                serde_json::json!({"id":hex(&support::tx().id(g.chain_id()))}),
            )
            .unwrap();
        assert_eq!(found["occurrences"].as_array().unwrap().len(), 1);
        let target: Hash = ting_node::unhex(
            found["occurrences"][0]["receipt"]["result_hex"]
                .as_str()
                .unwrap(),
            32,
        )
        .unwrap()
        .try_into()
        .unwrap();
        assert_eq!(value(&mut chain, target, 0), "2a");
        drop(chain);
        assert_eq!(open(&dir).height(), 1);
    }
}
#[test]
fn unforced_writer() {
    let Some(dir) = std::env::var_os("TING_UNFORCED_DIRECTORY") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let mut chain = Chain::open(&dir, Arc::new(AtomicBool::new(false))).unwrap();
    chain.mine(5, 10_000).unwrap();
    let status = chain.request("status", serde_json::json!({})).unwrap();
    fs::write(dir.join("status.json"), status.to_string()).unwrap();
    // Exit without running destructors: only the first block's commit was
    // forced to disk; every block's frame was.
    std::process::exit(74);
}
#[test]
fn a_crash_replays_the_blocks_whose_commits_were_not_forced() {
    let dir = Directory::new();
    dir.config(false, "unforced");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "unforced_writer", "--nocapture"])
        .env("TING_UNFORCED_DIRECTORY", &dir.0)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    assert_eq!(command.status().unwrap().code(), Some(74));
    let before: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.0.join("status.json")).unwrap()).unwrap();
    assert_eq!(before["height"], 5);
    let g = config::genesis(&dir.0).unwrap();
    {
        // The database holds at most the first block, stored by the last
        // commit forced to disk, and the other frames wait after its file
        // cursor.
        let store = Store::open(&dir.0, &g, ting_node::store::CACHE_BYTES).unwrap();
        assert!(store.metadata().unwrap().canonical.len() <= 1);
        assert!(store.pending().unwrap().is_some());
    }
    let mut chain = open(&dir);
    let after = chain.request("status", serde_json::json!({})).unwrap();
    for key in ["tip", "height", "state_root", "work", "stored_blocks"] {
        assert_eq!(after[key], before[key], "{key}");
    }
    drop(chain);
    assert_eq!(open(&dir).height(), 5);
}
#[test]
fn segment_rotation_and_recovery_do_not_load_or_reexecute_old_bodies() {
    let dir = Directory::new();
    dir.config(false, "segments");
    let mut chain = open(&dir);
    chain.submit(&support::tx().encode()).unwrap();
    chain.mine(1, 10000).unwrap();
    let before = chain.request("status", serde_json::json!({})).unwrap();
    drop(chain);
    // Fill the unused segment suffix sparsely and advance the fixture's committed cursor.
    let path = dir.0.join("blocks/00000000.dat");
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(128 * 1024 * 1024)
        .unwrap();
    {
        let db = Database::create(dir.0.join("chain.redb")).unwrap();
        let tx = db.begin_write().unwrap();
        let mut cursor = 0u32.to_le_bytes().to_vec();
        cursor.extend((128 * 1024 * 1024u64).to_le_bytes());
        cursor.extend(0u64.to_le_bytes());
        tx.open_table(META)
            .unwrap()
            .insert(b"file_cursor".as_slice(), cursor.as_slice())
            .unwrap();
        tx.commit().unwrap();
    }
    let mut chain = open(&dir);
    assert_eq!(
        chain.request("status", serde_json::json!({})).unwrap()["state_root"],
        before["state_root"]
    );
    chain.mine(1, 10000).unwrap();
    assert!(dir.0.join("blocks/00000001.dat").exists());
    assert!(chain.request("raw", serde_json::json!({"id":"1"})).is_ok());
    drop(chain);
    assert_eq!(open(&dir).height(), 2);
}

#[test]
fn invalid_undo_cannot_publish_a_partial_reorganization() {
    let dir = Directory::new();
    dir.config(false, "undo integrity");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    chain.submit(&support::tx().encode()).unwrap();
    chain.mine(1, 10000).unwrap();
    let old = chain.tip();
    let before = chain.request("status", serde_json::json!({})).unwrap();
    let b = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut history = vec![];
    let b1 = mine(&b, &g, &mut history, vec![], "B");
    chain.import(&b1.encode(), true).unwrap();
    let b2 = mine(&b, &g, &mut history, vec![], "B");
    drop(chain);
    {
        let db = Database::create(dir.0.join("chain.redb")).unwrap();
        let tx = db.begin_write().unwrap();
        {
            let mut table = tx.open_table(DELTAS).unwrap();
            let mut low = old.to_vec();
            low.extend([0; 32]);
            let mut high = old.to_vec();
            high.extend([255; 32]);
            use redb::ReadableTable;
            let (key, mut value) = {
                let (k, v) = table
                    .range(low.as_slice()..=high.as_slice())
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap();
                (k.value().to_vec(), v.value().to_vec())
            };
            *value.last_mut().unwrap() ^= 1;
            table.insert(key.as_slice(), value.as_slice()).unwrap();
        }
        tx.commit().unwrap();
    }
    let size = fs::metadata(dir.0.join("blocks/00000000.dat"))
        .unwrap()
        .len();
    let mut chain = open(&dir);
    // Damaged undo data is caught, whether it no longer decodes or restores
    // a root that is not the parent's, and nothing of the move is kept.
    let error = chain.import(&b2.encode(), true).unwrap_err();
    assert!(
        error.contains("root mismatch") || error.contains("change"),
        "{error}"
    );
    assert_eq!(chain.tip(), old);
    assert_eq!(
        chain.request("status", serde_json::json!({})).unwrap()["state_root"],
        before["state_root"]
    );
    // The competing block is kept, its branch not connected; a restart
    // that cannot connect it either stays on the old tip.
    assert!(
        fs::metadata(dir.0.join("blocks/00000000.dat"))
            .unwrap()
            .len()
            > size
    );
    drop(chain);
    assert_eq!(open(&dir).tip(), old);
}

#[test]
fn torn_unindexed_segment_header_is_repaired_and_inconsistent_state_is_rejected() {
    let dir = Directory::new();
    dir.config(false, "torn header");
    drop(open(&dir));
    fs::create_dir_all(dir.0.join("blocks")).unwrap();
    fs::write(dir.0.join("blocks/00000000.dat"), b"OCBL").unwrap();
    let mut chain = open(&dir);
    assert_eq!(
        fs::metadata(dir.0.join("blocks/00000000.dat"))
            .unwrap()
            .len(),
        40
    );
    chain.mine(1, 10000).unwrap();
    drop(chain);
    {
        let db = Database::create(dir.0.join("chain.redb")).unwrap();
        let tx = db.begin_write().unwrap();
        tx.open_table(META)
            .unwrap()
            .insert(b"root".as_slice(), [99; 32].as_slice())
            .unwrap();
        tx.commit().unwrap();
    }
    assert!(
        Chain::open(&dir.0, Arc::new(AtomicBool::new(false)))
            .err()
            .unwrap()
            .contains("Merkle root")
    );
}
#[test]
fn the_first_commit_after_opening_is_forced_and_later_ones_are_flushed() {
    let dir = Directory::new();
    dir.config(false, "durable");
    let g = config::genesis(&dir.0).unwrap();
    let store = Store::open(&dir.0, &g, ting_node::store::CACHE_BYTES).unwrap();
    // Nothing forced yet: a state commit, as a startup activation makes, is.
    assert!(store.due());
    store.commit_state(store.begin().unwrap()).unwrap();
    assert!(!store.due() && !store.lazy());
    // The next within the interval is not, until a flush forces it.
    store.commit_state(store.begin().unwrap()).unwrap();
    assert!(store.lazy());
    store.flush_due().unwrap();
    assert!(store.lazy());
    store.flush().unwrap();
    assert!(!store.lazy());
}
#[test]
fn values_larger_than_a_part_are_kept_in_parts_and_leave_with_their_rows() {
    use redb::ReadableTableMetadata;
    let dir = Directory::new();
    dir.config(false, "parts");
    let g = config::genesis(&dir.0).unwrap();
    let store = Store::open(&dir.0, &g, ting_node::store::CACHE_BYTES).unwrap();
    let parts = |tx: &redb::WriteTransaction| tx.open_table(PARTS).unwrap().len().unwrap();
    let (key, block, other) = ([7; 32], [9; 32], [8; 32]);
    let big: Vec<u8> = (0..2 * PART_BYTES + 5).map(|i| (i % 251) as u8).collect();
    let set = |tx: &Rc<redb::WriteTransaction>, value: Option<Vec<u8>>| {
        let mut state = Store::state(tx.clone()).unwrap();
        state.set(key, value);
        let changes = state.changes().unwrap();
        drop(state);
        Store::apply(tx, &changes).unwrap();
        changes.root
    };
    let get = |tx: &Rc<redb::WriteTransaction>| Store::state(tx.clone()).unwrap().get(&key);
    let tx = Rc::new(store.begin().unwrap());
    let empty = Store::state(tx.clone()).unwrap().root();
    // A record of three parts, and a block whose undo data deletes it.
    let root = set(&tx, Some(big.clone()));
    Store::put_undo(&tx, block, &[(key, None)]).unwrap();
    assert_eq!(parts(&tx), 3);
    assert_eq!(get(&tx).as_ref(), Some(&big));
    // Undone, the value moves to the block's redo data, still in parts; done
    // again, back to the record.
    Store::undo_block(tx.clone(), block, 1, empty).unwrap();
    assert_eq!(get(&tx), None);
    assert_eq!(parts(&tx), 3);
    assert!(Store::redo_block(tx.clone(), block, root).unwrap());
    assert_eq!(get(&tx).as_ref(), Some(&big));
    assert_eq!(parts(&tx), 3);
    // A large previous value in undo data is kept in parts too.
    Store::put_undo(&tx, other, &[(key, Some(big.clone()))]).unwrap();
    assert_eq!(parts(&tx), 6);
    // A smaller value leaves fewer parts, one that fits a row none.
    set(&tx, Some(big[..PART_BYTES + 1].to_vec()));
    assert_eq!(parts(&tx), 5);
    set(&tx, Some(big[..10].to_vec()));
    assert_eq!(parts(&tx), 3);
    set(&tx, Some(big.clone()));
    store
        .commit_state(Rc::try_unwrap(tx).ok().unwrap())
        .unwrap();
    // Read back through a committed view, and from undo data.
    assert_eq!(store.snapshot().unwrap().get(&key).as_ref(), Some(&big));
    assert_eq!(
        store.undo_of(other).unwrap(),
        vec![(key, Some(big.clone()))]
    );
    // Pruned undo data and a deleted record take their parts with them.
    let tx = Rc::new(store.begin().unwrap());
    Store::prune_undo(&tx, &[other, block], 0).unwrap();
    assert_eq!(parts(&tx), 3);
    set(&tx, None);
    assert_eq!(parts(&tx), 0);
}
