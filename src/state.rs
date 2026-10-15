//! Journaled state and its incremental compact sparse Merkle tree.
use crate::hash::{Hash, hash, hash_parts};
use std::{cell::RefCell, collections::BTreeMap, fmt, rc::Rc, sync::OnceLock};

/// A subtree's place in the tree: its depth and the key bits that lead to it,
/// the rest cleared.
pub type Position = (u16, Hash);
/// A record value, shared rather than copied between the overlay, the journal
/// and the callers that read it.
pub type Value = Rc<Vec<u8>>;

/// A subtree as it is stored: a node over two or more records, or the leaf of
/// a single record with its key. An empty subtree is not stored, and nothing
/// is stored below a leaf (wiki: data/state-tree).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Node(Hash),
    Leaf(Hash, Hash),
}
impl Entry {
    pub fn hash(&self) -> Hash {
        match self {
            Entry::Node(hash) | Entry::Leaf(_, hash) => *hash,
        }
    }
}

/// A record written in this state: its value and leaf now, `None` when absent,
/// and the leaf it has in the reader.
#[derive(Clone)]
struct Dirty {
    value: Option<Value>,
    leaf: Option<Hash>,
    stored: Option<Hash>,
}

/// What one `set` replaced: the overlay entry (`None` when the record was still
/// the stored one) and the leaf. Rollback restores both without reading or
/// hashing any value.
#[derive(Clone)]
struct Undo {
    key: Hash,
    record: Option<Dirty>,
    leaf: Option<Hash>,
}

/// Physical storage is outside consensus. Readers must represent a stable view.
pub trait StateReader {
    fn record(&self, key: &Hash) -> Result<Option<Vec<u8>>, String>;
    /// A record's length without its value. A reader whose values are not in
    /// memory overrides it: a length query does no work in proportion to the
    /// value (wiki: vm/host).
    fn record_len(&self, key: &Hash) -> Result<Option<u64>, String> {
        Ok(self.record(key)?.map(|v| v.len() as u64))
    }
    /// The stored subtree at a position: `None` when it is empty or lies below
    /// a leaf.
    fn node(&self, position: &Position) -> Result<Option<Entry>, String>;
}
struct Empty;
impl StateReader for Empty {
    fn record(&self, _: &Hash) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
    fn node(&self, _: &Position) -> Result<Option<Entry>, String> {
        Ok(None)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordChange {
    pub key: Hash,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}
#[derive(Clone, Debug)]
pub struct Changes {
    pub records: Vec<RecordChange>,
    pub nodes: Vec<(Position, Option<Entry>)>,
    pub root: Hash,
    pub count: usize,
}

/// The hash of an empty subtree, at any depth.
fn empty() -> Hash {
    static VALUE: OnceLock<Hash> = OnceLock::new();
    *VALUE.get_or_init(|| hash("state-empty", &[]))
}
fn node(depth: u16, left: Hash, right: Hash) -> Hash {
    hash_parts("state-node", &[&depth.to_le_bytes(), &left, &right])
}
fn leaf(key: &Hash, value: &[u8]) -> Hash {
    hash_parts(
        "state-leaf",
        &[key, &(value.len() as u32).to_le_bytes(), value],
    )
}
/// Bit `depth` of a key, counted from the most significant bit of its first byte.
fn bit(key: &Hash, depth: u16) -> bool {
    key[usize::from(depth / 8)] >> (7 - depth % 8) & 1 == 1
}
fn prefix(mut key: Hash, depth: u16) -> Hash {
    if depth == 256 {
        return key;
    }
    let byte = usize::from(depth / 8);
    let bits = depth % 8;
    key[byte] &= if bits == 0 { 0 } else { 0xff << (8 - bits) };
    key[byte + 1..].fill(0);
    key
}
/// The subtree at `depth` on the key's path.
fn position(key: &Hash, depth: u16) -> Position {
    (depth, prefix(*key, depth))
}
/// The other child of the node above the subtree at `depth` on the key's path.
fn sibling(key: &Hash, depth: u16) -> Position {
    let mut bits = prefix(*key, depth);
    let above = depth - 1;
    bits[usize::from(above / 8)] ^= 1 << (7 - above % 8);
    (depth, bits)
}
/// The node at `depth` over the key's own subtree and the other one, in the
/// order the key's bit says.
fn join(key: &Hash, depth: u16, own: Hash, other: Hash) -> Hash {
    if bit(key, depth) {
        node(depth, other, own)
    } else {
        node(depth, own, other)
    }
}
/// Where a proof's path ends (wiki: data/state-tree#proofs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// The record's own leaf: it is present.
    Present,
    /// An empty subtree: it is absent.
    Empty,
    /// The leaf of another record whose key shares the path: it is absent.
    /// The record's key and value, from which the verifier hashes the leaf, so
    /// that the leaf is bound to that key.
    Other(Hash, Vec<u8>),
}
/// A proof that a record is present or absent: where its key's path ends, and
/// the hash of the other child at each level above that, from the end up to
/// the root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proof {
    pub end: End,
    pub siblings: Vec<Hash>,
}
/// The proof of the record at `key` in a stored tree.
pub fn prove(reader: &dyn StateReader, key: &Hash) -> Result<Proof, String> {
    let mut siblings = Vec::new();
    let mut depth = 0u16;
    let end = loop {
        match reader.node(&position(key, depth))? {
            None => break End::Empty,
            Some(Entry::Leaf(other, _)) if other == *key => break End::Present,
            Some(Entry::Leaf(other, _)) => {
                let value = reader
                    .record(&other)?
                    .ok_or("state tree leaf without its record")?;
                break End::Other(other, value);
            }
            Some(Entry::Node(_)) if depth == 256 => {
                return Err("state tree node below the deepest level".into());
            }
            Some(Entry::Node(_)) => {
                depth += 1;
                let other = reader.node(&sibling(key, depth))?;
                siblings.push(other.map_or_else(empty, |e| e.hash()));
            }
        }
    };
    siblings.reverse();
    Ok(Proof { end, siblings })
}
/// Whether `proof` shows the record at `key` to hold `value`, or with `None`
/// to be absent, in the state whose root is `root`.
pub fn verify(root: &Hash, key: &Hash, value: Option<&[u8]>, proof: &Proof) -> bool {
    let Ok(depth) = u16::try_from(proof.siblings.len()) else {
        return false;
    };
    if depth > 256 {
        return false;
    }
    let mut hash = match (&proof.end, value) {
        (End::Present, Some(value)) => leaf(key, value),
        (End::Empty, None) => empty(),
        (End::Other(other, value), None)
            if other != key && (0..depth).all(|d| bit(other, d) == bit(key, d)) =>
        {
            leaf(other, value)
        }
        _ => return false,
    };
    for (above, sibling) in (0..depth).rev().zip(&proof.siblings) {
        hash = join(key, above, hash, *sibling);
    }
    hash == *root
}
pub fn record_key(namespace: u8, contract: Hash, logical: Hash) -> Hash {
    hash_parts("state-key", &[&[namespace], &contract, &logical])
}
/// The manifest record of a contract (namespace 0).
pub fn manifest_key(contract: Hash) -> Hash {
    record_key(0, contract, [0; 32])
}
/// The single code record of a contract (namespace 1).
pub fn code_key(contract: Hash) -> Hash {
    record_key(1, contract, [0; 32])
}
/// Gross new allocation of a write of `new` bytes over a record of `old`
/// bytes, or over no record (wiki: resources/growth).
pub fn growth(old: Option<u64>, new: u64) -> u64 {
    old.map_or(36 + new, |old| new.saturating_sub(old))
}

#[derive(Clone)]
pub struct State {
    reader: Rc<dyn StateReader>,
    records: BTreeMap<Hash, Dirty>,
    /// Subtrees written in this state, `None` for one emptied.
    nodes: BTreeMap<Position, Option<Entry>>,
    /// Subtrees read from the reader, so that a path shared by many writes,
    /// and the final comparison against the reader, read each one once.
    stored: RefCell<BTreeMap<Position, Option<Entry>>>,
    journal: Vec<Undo>,
    count: usize,
    root: Hash,
    error: RefCell<Option<String>>,
}
impl Default for State {
    fn default() -> Self {
        Self::backed(Rc::new(Empty), 0, empty())
    }
}
impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("count", &self.count)
            .field("dirty_records", &self.records.len())
            .field("dirty_nodes", &self.nodes.len())
            .finish()
    }
}

impl State {
    pub fn backed(reader: Rc<dyn StateReader>, count: usize, root: Hash) -> Self {
        Self {
            reader,
            records: BTreeMap::new(),
            nodes: BTreeMap::new(),
            stored: RefCell::new(BTreeMap::new()),
            journal: Vec::new(),
            count,
            root,
            error: RefCell::new(None),
        }
    }
    /// Storage failures are sticky: the runtime must reject execution, not emit a receipt.
    pub fn storage_error(&self) -> Option<String> {
        self.error.borrow().clone()
    }
    fn fail(&self, error: String) {
        self.error.borrow_mut().get_or_insert(error);
    }
    fn read<T>(&self, result: Result<Option<T>, String>) -> Option<T> {
        match result {
            Ok(value) => value,
            Err(error) => {
                self.fail(error);
                None
            }
        }
    }
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    /// A copy of a record's value.
    pub fn get(&self, key: &Hash) -> Option<Vec<u8>> {
        match self.records.get(key) {
            Some(dirty) => dirty.value.as_deref().cloned(),
            None => self.base_record(key),
        }
    }
    /// A record's value, shared with the state when it was written in it.
    pub fn value(&self, key: &Hash) -> Option<Value> {
        match self.records.get(key) {
            Some(dirty) => dirty.value.clone(),
            None => self.base_record(key).map(Rc::new),
        }
    }
    /// A record's length, without reading its value.
    pub fn value_len(&self, key: &Hash) -> Option<u64> {
        match self.records.get(key) {
            Some(dirty) => dirty.value.as_ref().map(|v| v.len() as u64),
            None => self.read(self.reader.record_len(key)),
        }
    }
    fn base_record(&self, key: &Hash) -> Option<Vec<u8>> {
        self.read(self.reader.record(key))
    }
    fn base_node(&self, position: &Position) -> Option<Entry> {
        let cached = self.stored.borrow().get(position).copied();
        if let Some(entry) = cached {
            return entry;
        }
        let entry = self.read(self.reader.node(position));
        if self.storage_error().is_none() {
            self.stored.borrow_mut().insert(*position, entry);
        }
        entry
    }
    pub fn root(&self) -> Hash {
        self.root
    }
    /// The net changes against the reader. A record whose leaf is unchanged is
    /// unchanged, so no value is read or compared for it.
    pub fn changes(&self) -> Result<Changes, String> {
        let mut records = Vec::new();
        for (key, dirty) in &self.records {
            if dirty.leaf == dirty.stored {
                continue;
            }
            records.push(RecordChange {
                key: *key,
                before: dirty.stored.and_then(|_| self.base_record(key)),
                after: dirty.value.as_deref().cloned(),
            });
        }
        let nodes = self
            .nodes
            .iter()
            .filter(|(position, entry)| **entry != self.base_node(position))
            .map(|(position, entry)| (*position, *entry))
            .collect();
        if let Some(error) = self.storage_error() {
            return Err(error);
        }
        Ok(Changes {
            records,
            nodes,
            root: self.root,
            count: self.count,
        })
    }
    pub fn checkpoint(&self) -> usize {
        self.journal.len()
    }
    pub fn commit(&mut self) {
        self.journal.clear();
    }
    pub fn set(&mut self, key: Hash, value: Option<Vec<u8>>) {
        let new = value.as_deref().map(|v| leaf(&key, v));
        let replaced = self.update(key, new);
        let stored = self
            .records
            .get(&key)
            .map_or(replaced, |dirty| dirty.stored);
        let record = self.records.insert(
            key,
            Dirty {
                value: value.map(Rc::new),
                leaf: new,
                stored,
            },
        );
        self.journal.push(Undo {
            key,
            record,
            leaf: replaced,
        });
    }
    pub fn rollback(&mut self, checkpoint: usize) {
        assert!(checkpoint <= self.journal.len());
        while self.journal.len() > checkpoint {
            let Undo { key, record, leaf } = self.journal.pop().unwrap();
            match record {
                Some(dirty) => self.records.insert(key, dirty),
                None => self.records.remove(&key),
            };
            self.update(key, leaf);
        }
    }
    /// Sets the leaf of `key`, `None` to remove the record, and hashes its path
    /// again up to the root (wiki: data/state-tree). Returns the leaf it
    /// replaced; the record count follows the leaves.
    fn update(&mut self, key: Hash, leaf: Option<Hash>) -> Option<Hash> {
        // The key's path ends at an empty subtree or at a single leaf.
        let mut depth = 0;
        let end = loop {
            match self.get_node(&position(&key, depth)) {
                Some(Entry::Node(_)) if depth < 256 => depth += 1,
                Some(Entry::Node(_)) => {
                    self.fail("a state-tree node below the deepest level".into());
                    return None;
                }
                end => break end,
            }
        };
        if self.storage_error().is_some() {
            return None;
        }
        let replaced = match end {
            Some(Entry::Leaf(other, hash)) if other == key => Some(hash),
            _ => None,
        };
        if replaced == leaf {
            return replaced;
        }
        match (replaced, leaf) {
            (None, Some(_)) => self.count += 1,
            (Some(_), None) => self.count -= 1,
            _ => {}
        }
        match (leaf, end) {
            (Some(new), Some(Entry::Leaf(other, theirs))) if other != key => {
                // Both records move down to the first bit where their keys
                // differ, under nodes with one empty side.
                let split = (depth..256)
                    .find(|&d| bit(&key, d) != bit(&other, d))
                    .expect("distinct keys differ in some bit");
                self.put(
                    position(&other, split + 1),
                    Some(Entry::Leaf(other, theirs)),
                );
                self.put(position(&key, split + 1), Some(Entry::Leaf(key, new)));
                let mut current = join(&key, split, new, theirs);
                self.put(position(&key, split), Some(Entry::Node(current)));
                for d in (depth..split).rev() {
                    current = join(&key, d, current, empty());
                    self.put(position(&key, d), Some(Entry::Node(current)));
                }
                self.rehash(&key, depth, current);
            }
            (Some(new), _) => {
                self.put(position(&key, depth), Some(Entry::Leaf(key, new)));
                self.rehash(&key, depth, new);
            }
            (None, _) => {
                // The record's place empties, and a record left alone beside
                // it moves up for as long as it stays alone.
                self.put(position(&key, depth), None);
                let mut lone = None;
                while depth > 0 {
                    let beside = sibling(&key, depth);
                    match (lone, self.get_node(&beside)) {
                        (None, Some(other @ Entry::Leaf(..))) => {
                            self.put(beside, None);
                            lone = Some(other);
                        }
                        (Some(_), None) => self.put(position(&key, depth), None),
                        _ => break,
                    }
                    depth -= 1;
                }
                self.put(position(&key, depth), lone);
                self.rehash(&key, depth, lone.map_or(empty(), |entry| entry.hash()));
            }
        }
        replaced
    }
    /// Hashes the nodes on the key's path above `depth` again, from the
    /// subtree's hash `current` up to the root.
    fn rehash(&mut self, key: &Hash, mut depth: u16, mut current: Hash) {
        while depth > 0 {
            let other = self
                .get_node(&sibling(key, depth))
                .map_or(empty(), |entry| entry.hash());
            depth -= 1;
            current = join(key, depth, current, other);
            self.put(position(key, depth), Some(Entry::Node(current)));
        }
        self.root = current;
    }
    fn get_node(&self, position: &Position) -> Option<Entry> {
        match self.nodes.get(position) {
            Some(entry) => *entry,
            None => self.base_node(position),
        }
    }
    fn put(&mut self, position: Position, entry: Option<Entry>) {
        self.nodes.insert(position, entry);
    }
}
