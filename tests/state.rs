use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use ting_core::{
    hash::{Hash, hash, hash_parts},
    meter::Resources,
    protocol::{Genesis, Payload, Status, Transaction},
    state::{Entry, Position, State, StateReader, code_key, record_key},
    vm::{CodeCache, Context, Runtime},
};
struct Reader {
    records: BTreeMap<Hash, Vec<u8>>,
    nodes: BTreeMap<Position, Entry>,
}
impl StateReader for Reader {
    fn record(&self, key: &Hash) -> Result<Option<Vec<u8>>, String> {
        Ok(self.records.get(key).cloned())
    }
    fn node(&self, position: &Position) -> Result<Option<Entry>, String> {
        Ok(self.nodes.get(position).copied())
    }
}
/// A stored tree of `records`, as a node holds it.
fn stored(records: &[(Hash, Vec<u8>)]) -> (Hash, Reader) {
    let mut state = State::default();
    for (key, value) in records {
        state.set(*key, Some(value.clone()));
    }
    state.commit();
    let changes = state.changes().unwrap();
    let reader = Reader {
        records: changes
            .records
            .into_iter()
            .map(|c| (c.key, c.after.unwrap()))
            .collect(),
        nodes: changes
            .nodes
            .into_iter()
            .map(|(p, v)| (p, v.unwrap()))
            .collect(),
    };
    (state.root(), reader)
}
#[test]
fn proofs_show_records_present_and_absent_against_the_root() {
    use ting_core::state::{End, Proof, prove, verify};
    // An empty state: every key ends at the empty root.
    let (root, reader) = stored(&[]);
    let proof = prove(&reader, &[9; 32]).unwrap();
    assert_eq!(proof.end, End::Empty);
    assert!(verify(&root, &[9; 32], None, &proof));
    // One record: the root is its leaf.
    let (root, reader) = stored(&[([1; 32], vec![1, 2])]);
    let proof = prove(&reader, &[1; 32]).unwrap();
    assert_eq!((proof.end.clone(), proof.siblings.len()), (End::Present, 0));
    assert!(verify(&root, &[1; 32], Some(&[1, 2]), &proof));
    let absent = prove(&reader, &[2; 32]).unwrap();
    assert!(matches!(absent.end, End::Other(..)));
    assert!(verify(&root, &[2; 32], None, &absent));
    // Many records, an empty value among them, and two keys that differ only
    // in their last bit, whose leaves sit 256 levels deep.
    let mut records: Vec<(Hash, Vec<u8>)> = (0u32..500)
        .map(|i| (hash("key", &i.to_le_bytes()), i.to_le_bytes().to_vec()))
        .collect();
    records[7].1 = vec![];
    let mut twin = [0xab; 32];
    records.push((twin, vec![1]));
    twin[31] ^= 1;
    records.push((twin, vec![2]));
    let (root, reader) = stored(&records);
    for (key, value) in &records {
        let proof = prove(&reader, key).unwrap();
        assert_eq!(proof.end, End::Present);
        assert!(verify(&root, key, Some(value), &proof), "{key:?}");
        // A different value, absence or a changed sibling do not check.
        assert!(!verify(
            &root,
            key,
            Some(&[value.as_slice(), &[0]].concat()),
            &proof
        ));
        assert!(!verify(&root, key, None, &proof));
        let mut forged = proof.clone();
        if let Some(first) = forged.siblings.first_mut() {
            first[0] ^= 1;
            assert!(!verify(&root, key, Some(value), &forged));
        }
        // The record's own leaf passed off as a neighbour's, which shares the
        // path but for its last bit, does not show the record absent.
        let mut neighbour = *key;
        neighbour[31] ^= 1;
        let disguised = Proof {
            end: End::Other(neighbour, value.clone()),
            siblings: proof.siblings.clone(),
        };
        assert!(!verify(&root, key, None, &disguised), "{key:?}");
    }
    assert_eq!(prove(&reader, &twin).unwrap().siblings.len(), 256);
    let mut ends = BTreeSet::new();
    for i in 0u32..500 {
        let key = hash("absent", &i.to_le_bytes());
        let proof = prove(&reader, &key).unwrap();
        ends.insert(matches!(proof.end, End::Empty));
        assert!(verify(&root, &key, None, &proof));
        assert!(!verify(&root, &key, Some(&[]), &proof));
        // The other record comes with its true value, and with no other.
        if let End::Other(other, value) = &proof.end {
            assert_eq!(
                Some(value),
                records.iter().find(|r| r.0 == *other).map(|r| &r.1)
            );
            let changed = Proof {
                end: End::Other(*other, [value.as_slice(), &[0]].concat()),
                siblings: proof.siblings.clone(),
            };
            assert!(!verify(&root, &key, None, &changed));
        }
    }
    assert_eq!(ends.len(), 2, "both an empty end and another record's leaf");
    // Another record's leaf proves nothing for a key off its path.
    let other = prove(&reader, &hash("absent", &0u32.to_le_bytes())).unwrap();
    if let End::Other(..) = other.end {
        assert!(!verify(&root, &[0x55; 32], None, &other));
    }
}
#[test]
fn backed_state_preserves_roots_and_exports_only_net_changes() {
    let mut original = State::default();
    original.set([1; 32], Some(vec![5]));
    original.set([2; 32], Some(vec![]));
    original.commit();
    let base = original.changes().unwrap();
    let reader = Reader {
        records: base
            .records
            .into_iter()
            .map(|c| (c.key, c.after.unwrap()))
            .collect(),
        nodes: base
            .nodes
            .into_iter()
            .map(|(p, v)| (p, v.unwrap()))
            .collect(),
    };
    let mut backed = State::backed(Rc::new(reader), original.len(), original.root());
    for state in [&mut original, &mut backed] {
        state.set([1; 32], Some(vec![6]));
        state.set([1; 32], Some(vec![7]));
        state.set([2; 32], None);
        let checkpoint = state.checkpoint();
        state.set([3; 32], Some(vec![8]));
        state.rollback(checkpoint);
    }
    assert_eq!(backed.root(), original.root());
    assert_eq!(backed.len(), 1);
    let changes = backed.changes().unwrap();
    assert_eq!(changes.records.len(), 2);
    assert_eq!(changes.records[0].before, Some(vec![5]));
    assert_eq!(changes.records[0].after, Some(vec![7]));
    assert_eq!(changes.records[1].before, Some(vec![]));
    assert_eq!(changes.records[1].after, None);
    backed.rollback(0);
    assert_eq!(backed.len(), 2);
    assert!(backed.changes().unwrap().records.is_empty());
    assert!(backed.changes().unwrap().nodes.is_empty());
}
struct Broken;
impl StateReader for Broken {
    fn record(&self, _: &Hash) -> Result<Option<Vec<u8>>, String> {
        Err("disk read failed".into())
    }
    fn node(&self, _: &Position) -> Result<Option<Entry>, String> {
        Err("disk read failed".into())
    }
}
#[test]
fn storage_failure_never_becomes_a_valid_missing_contract_receipt() {
    let g = Genesis::experimental();
    let state = State::backed(Rc::new(Broken), 0, State::default().root());
    let runtime = Runtime::new(state, Context::from_genesis(&g));
    let tx = Transaction {
        budget: Resources([1_000_000, 10, 1000, 1000]),
        payload: Payload::Call {
            target: [1; 32],
            input: vec![],
        },
    };
    assert!(runtime.execute(&tx).is_err());
    assert_eq!(runtime.storage_error().as_deref(), Some("disk read failed"));
    assert!(
        runtime
            .build_candidate(&g, &[], vec![], g.timestamp + 1, [0; 4], vec![])
            .is_err()
    );
    assert!(runtime.state_changes().is_err());
}

/// A store that answers lengths and tree nodes but refuses to read the values
/// of some records.
struct Guarded {
    records: BTreeMap<Hash, Vec<u8>>,
    nodes: BTreeMap<Position, Entry>,
    refused: BTreeSet<Hash>,
}
impl StateReader for Guarded {
    fn record(&self, key: &Hash) -> Result<Option<Vec<u8>>, String> {
        if self.refused.contains(key) {
            return Err("a refused value was read".into());
        }
        Ok(self.records.get(key).cloned())
    }
    fn record_len(&self, key: &Hash) -> Result<Option<u64>, String> {
        Ok(self.records.get(key).map(|v| v.len() as u64))
    }
    fn node(&self, position: &Position) -> Result<Option<Entry>, String> {
        Ok(self.nodes.get(position).copied())
    }
}

/// Input 1 answers the length of the record at key zero with a zero-capacity
/// `storage_get`; any other input, the deployment's included, writes 1 MiB there.
const LARGE_RECORD: &str = r#"(module
  (import "ting" "input" (func (param i32 i32) (result i32)))
  (import "ting" "storage_get" (func (param i32 i32 i32) (result i32)))
  (import "ting" "storage_set" (func (param i32 i32 i32)))
  (import "ting" "return" (func (param i32 i32)))
  (memory (export "memory") 17 17)
  (func (export "invoke")
    (drop (call 0 (i32.const 0) (i32.const 1)))
    (if (i32.eq (i32.load8_u (i32.const 0)) (i32.const 1))
      (then
        (i32.store (i32.const 0) (call 1 (i32.const 32) (i32.const 0) (i32.const 0)))
        (call 3 (i32.const 0) (i32.const 4)))
      (else
        (call 2 (i32.const 32) (i32.const 65536) (i32.const 1048576))))))"#;

#[test]
fn length_queries_overwrites_and_cached_code_read_no_values() {
    let g = Genesis::experimental();
    let budget = Resources([1_000_000_000, 100, 1 << 24, 1 << 24]);
    let cache = CodeCache::default();
    let first = Runtime::with_cache(State::default(), Context::from_genesis(&g), cache.clone());
    let deployed = first
        .execute(&Transaction {
            budget,
            payload: Payload::Deploy {
                code: wat::parse_str(LARGE_RECORD).unwrap(),
                input: vec![],
            },
        })
        .unwrap();
    assert_eq!(deployed.status, Status::Success);
    first.commit();
    let id: Hash = deployed.result.as_slice().try_into().unwrap();
    let base = first.state_changes().unwrap();
    let reader: Rc<dyn StateReader> = Rc::new(Guarded {
        records: base
            .records
            .iter()
            .map(|c| (c.key, c.after.clone().unwrap()))
            .collect(),
        nodes: base.nodes.iter().map(|(p, v)| (*p, v.unwrap())).collect(),
        refused: [record_key(2, id, [0; 32]), code_key(id)].into(),
    });
    let call = |mode: u8| Transaction {
        budget,
        payload: Payload::Call {
            target: id,
            input: vec![mode],
        },
    };
    // The cache shared with the deploying runtime holds the program, so the
    // code record's bytes are never fetched; the length query and the
    // same-size overwrite read only the stored length.
    let warm = Runtime::with_cache(
        State::backed(reader.clone(), base.count, base.root),
        Context::from_genesis(&g),
        cache,
    );
    let query = warm.execute(&call(1)).unwrap();
    assert_eq!(query.status, Status::Success);
    assert_eq!(query.result, (1u32 << 20).to_le_bytes());
    let overwrite = warm.execute(&call(2)).unwrap();
    assert_eq!(overwrite.status, Status::Success);
    assert_eq!(overwrite.used.0[3], 0);
    assert_eq!(warm.storage_error(), None);
    // Without the program in its cache, a runtime has to read the code.
    let cold = Runtime::new(
        State::backed(reader, base.count, base.root),
        Context::from_genesis(&g),
    );
    assert!(cold.execute(&call(1)).is_err());
    assert_eq!(
        cold.storage_error().as_deref(),
        Some("a refused value was read")
    );
}

#[test]
fn the_tree_ignores_writes_that_change_nothing_and_nests_checkpoints() {
    let mut s = State::default();
    let empty = s.root();
    s.set([1; 32], Some(vec![5]));
    let (root, len) = (s.root(), s.len());
    let before = s.changes().unwrap();
    // The same value again, and deleting an absent key, change nothing.
    s.set([1; 32], Some(vec![5]));
    s.set([2; 32], None);
    assert_eq!((s.root(), s.len()), (root, len));
    let after = s.changes().unwrap();
    assert_eq!(after.records.len(), before.records.len());
    assert_eq!(after.nodes.len(), before.nodes.len());
    // Three nested checkpoints unwind one at a time, or all at once.
    let outer = s.checkpoint();
    s.set([2; 32], Some(vec![1]));
    let inner = s.checkpoint();
    s.set([3; 32], Some(vec![2]));
    let deepest = s.checkpoint();
    s.set([4; 32], Some(vec![3]));
    assert_eq!(s.len(), 4);
    s.rollback(deepest);
    assert_eq!((s.len(), s.get(&[4; 32])), (3, None));
    s.rollback(inner);
    assert_eq!((s.len(), s.get(&[3; 32])), (2, None));
    assert_eq!(s.get(&[2; 32]), Some(vec![1]));
    s.rollback(outer);
    assert_eq!((s.root(), s.len()), (root, len));
    let now = s.checkpoint();
    s.rollback(now);
    assert_eq!(s.root(), root);
    s.commit();
    assert_eq!(s.checkpoint(), 0);
    // Keys that differ only in their last bit are distinct records, and
    // deleting back to nothing restores the empty root.
    let mut last = [0u8; 32];
    last[31] = 1;
    let mut s = State::default();
    s.set([0; 32], Some(vec![1]));
    let only_zero = s.root();
    s.set(last, Some(vec![1]));
    assert_ne!(s.root(), only_zero);
    assert_eq!(s.len(), 2);
    s.set([0; 32], None);
    let mut t = State::default();
    t.set(last, Some(vec![1]));
    assert_eq!(s.root(), t.root());
    s.set(last, None);
    assert_eq!((s.root(), s.len()), (empty, 0));
}

/// Records by key, as the definition sees them.
type Records = BTreeMap<Hash, Vec<u8>>;

/// The state tree straight from its definition (wiki: data/state-tree): the
/// root, and the entry a store holds at each position.
fn definition(records: &Records) -> (Hash, BTreeMap<Position, Entry>) {
    fn bit(key: &Hash, depth: u16) -> bool {
        key[usize::from(depth / 8)] >> (7 - depth % 8) & 1 == 1
    }
    fn walk(
        records: &[(Hash, Vec<u8>)],
        depth: u16,
        prefix: Hash,
        entries: &mut BTreeMap<Position, Entry>,
    ) -> Hash {
        match records {
            [] => hash("state-empty", &[]),
            [(key, value)] => {
                let length = (value.len() as u32).to_le_bytes();
                let leaf = hash_parts("state-leaf", &[key, &length, value]);
                entries.insert((depth, prefix), Entry::Leaf(*key, leaf));
                leaf
            }
            _ => {
                let (right, left): (Vec<_>, Vec<_>) = records
                    .iter()
                    .cloned()
                    .partition(|(key, _)| bit(key, depth));
                let mut right_prefix = prefix;
                right_prefix[usize::from(depth / 8)] |= 1 << (7 - depth % 8);
                let left = walk(&left, depth + 1, prefix, entries);
                let right = walk(&right, depth + 1, right_prefix, entries);
                let node = hash_parts("state-node", &[&depth.to_le_bytes(), &left, &right]);
                entries.insert((depth, prefix), Entry::Node(node));
                node
            }
        }
    }
    let mut entries = BTreeMap::new();
    let records: Vec<_> = records.iter().map(|(k, v)| (*k, v.clone())).collect();
    let root = walk(&records, 0, [0; 32], &mut entries);
    (root, entries)
}

/// A deterministic xorshift sequence.
struct Sequence(u64);
impl Sequence {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Keys of which every second one shares a random number of leading bits
/// with the one before, so that records split deep and move up across many
/// levels; some share all but the last bit.
fn keys(sequence: &mut Sequence, count: usize) -> Vec<Hash> {
    let mut keys: Vec<Hash> = Vec::new();
    for i in 0..count {
        let mut key = [0u8; 32];
        key.iter_mut().for_each(|b| *b = sequence.next() as u8);
        if i % 2 == 1 {
            let shared = match i % 6 {
                1 => 255,
                3 => sequence.below(32),
                _ => sequence.below(256),
            };
            let before = keys[i - 1];
            for bit in 0..=shared {
                let mask = 1 << (7 - bit % 8);
                let set = (before[bit / 8] & mask != 0) != (bit == shared);
                key[bit / 8] = if set {
                    key[bit / 8] | mask
                } else {
                    key[bit / 8] & !mask
                };
            }
        }
        keys.push(key);
    }
    keys
}

/// Random writes, overwrites, deletions and nested rollbacks on `states`,
/// with the expected records in `model`; every state's root and count are
/// checked against the definition.
fn exercise(
    sequence: &mut Sequence,
    pool: &[Hash],
    states: &mut [&mut State],
    model: &mut Records,
    steps: usize,
) {
    let mut saved: Vec<(Vec<usize>, Records)> = Vec::new();
    for step in 0..steps {
        match sequence.below(10) {
            0 if !saved.is_empty() => {
                let (checkpoints, records) = saved.pop().unwrap();
                for (state, checkpoint) in states.iter_mut().zip(checkpoints) {
                    state.rollback(checkpoint);
                }
                *model = records;
            }
            1 if saved.len() < 4 => saved.push((
                states.iter().map(|state| state.checkpoint()).collect(),
                model.clone(),
            )),
            2..=4 => {
                let key = pool[sequence.below(pool.len())];
                states.iter_mut().for_each(|state| state.set(key, None));
                model.remove(&key);
            }
            _ => {
                let key = pool[sequence.below(pool.len())];
                let value = vec![sequence.next() as u8; sequence.below(3)];
                states
                    .iter_mut()
                    .for_each(|state| state.set(key, Some(value.clone())));
                model.insert(key, value);
            }
        }
        for state in states.iter() {
            assert_eq!(state.len(), model.len());
            assert_eq!(state.storage_error(), None);
        }
        // The definition rehashes every record, so it is checked every few steps.
        if step % 4 == 3 || step + 1 == steps {
            let root = definition(model).0;
            assert!(states.iter().all(|state| state.root() == root));
        }
    }
    states.iter_mut().for_each(|state| state.commit());
}

#[test]
fn the_incremental_tree_matches_its_definition() {
    let mut sequence = Sequence(0x9e37_79b9_7f4a_7c15);
    let pool = keys(&mut sequence, 48);
    // In memory: the stored entries are exactly the compact tree's.
    let mut memory = State::default();
    let mut model = BTreeMap::new();
    exercise(&mut sequence, &pool, &mut [&mut memory], &mut model, 1_000);
    let (root, entries) = definition(&model);
    let base = memory.changes().unwrap();
    assert_eq!(base.root, root);
    let stored: BTreeMap<Position, Entry> = base
        .nodes
        .iter()
        .map(|(position, entry)| (*position, entry.unwrap()))
        .collect();
    assert_eq!(stored, entries);
    // Behind a reader holding those entries: the same roots, and changes that
    // turn the stored tree into the compact tree of the final records.
    let reader = Reader {
        records: base
            .records
            .iter()
            .map(|c| (c.key, c.after.clone().unwrap()))
            .collect(),
        nodes: stored.clone(),
    };
    let mut backed = State::backed(Rc::new(reader), memory.len(), memory.root());
    exercise(
        &mut sequence,
        &pool,
        &mut [&mut memory, &mut backed],
        &mut model,
        1_000,
    );
    let changes = backed.changes().unwrap();
    let mut applied = stored;
    for (position, entry) in changes.nodes {
        match entry {
            Some(entry) => applied.insert(position, entry),
            None => applied.remove(&position),
        };
    }
    assert_eq!(applied, definition(&model).1);
    assert_eq!(changes.root, definition(&model).0);
}

#[test]
fn a_call_that_cannot_pay_the_code_price_reads_none_of_the_code() {
    let g = Genesis::experimental();
    let budget = Resources([1_000_000_000, 100, 1 << 24, 1 << 24]);
    let first = Runtime::new(State::default(), Context::from_genesis(&g));
    let deployed = first
        .execute(&Transaction {
            budget,
            payload: Payload::Deploy {
                code: wat::parse_str(LARGE_RECORD).unwrap(),
                input: vec![],
            },
        })
        .unwrap();
    assert_eq!(deployed.status, Status::Success);
    first.commit();
    let id: Hash = deployed.result.as_slice().try_into().unwrap();
    let base = first.state_changes().unwrap();
    // A store that refuses to hand out the code, and a runtime with nothing
    // cached: the manifest's code price is charged before the code is read,
    // so a budget that cannot pay it ends with status 3 and no read.
    let reader: Rc<dyn StateReader> = Rc::new(Guarded {
        records: base
            .records
            .iter()
            .map(|c| (c.key, c.after.clone().unwrap()))
            .collect(),
        nodes: base.nodes.iter().map(|(p, v)| (*p, v.unwrap())).collect(),
        refused: [code_key(id)].into(),
    });
    let cold = Runtime::new(
        State::backed(reader, base.count, base.root),
        Context::from_genesis(&g),
    );
    let call = Transaction {
        budget: Resources([10_000, 100, 1 << 24, 1 << 24]),
        payload: Payload::Call {
            target: id,
            input: vec![1],
        },
    };
    let receipt = cold.execute(&call).unwrap();
    assert_eq!(receipt.status, Status::Compute);
    assert_eq!(cold.storage_error(), None);
}
