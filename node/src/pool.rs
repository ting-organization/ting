//! The transaction pool: what this node offers its miner, in arrival order
//! (wiki: node/mining). Its bounds are the node's configuration, and which
//! transaction gives way when it is full is the selection policy's
//! (`mining::Selector`). Policy, not consensus.
use std::{
    collections::{BTreeSet, VecDeque},
    net::IpAddr,
};
use ting_core::{hash::Hash, protocol::Transaction};

/// How long a transaction stays without being included.
pub const POOL_AGE: u64 = 86_400;

pub struct Pooled {
    pub tx: Transaction,
    pub id: Hash,
    pub bytes: usize,
    pub arrived: u64,
    /// The address of the peer that relayed it; none for the node's own API,
    /// CLI and deferred set.
    pub source: Option<IpAddr>,
}

#[derive(Default)]
pub struct Pool {
    items: VecDeque<Pooled>,
    ids: BTreeSet<Hash>,
    bytes: usize,
}

impl Pool {
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn contains(&self, id: &Hash) -> bool {
        self.ids.contains(id)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Pooled> {
        self.items.iter()
    }
    /// Pooled ids in order, after `after`.
    pub fn ids_after(&self, after: Option<Hash>) -> impl Iterator<Item = &Hash> {
        use std::ops::Bound;
        let low = after.map_or(Bound::Unbounded, Bound::Excluded);
        self.ids.range((low, Bound::Unbounded))
    }
    pub fn get(&self, id: &Hash) -> Option<&Transaction> {
        if !self.contains(id) {
            return None;
        }
        self.items.iter().find(|p| p.id == *id).map(|p| &p.tx)
    }
    /// Whether a transaction of `bytes` fits beside the rest, within `count`
    /// transactions and `most` bytes.
    pub fn fits(&self, bytes: usize, count: usize, most: usize) -> bool {
        self.items.len() < count && self.bytes.saturating_add(bytes) <= most
    }
    /// Takes out the transaction at `at`, in arrival order.
    pub fn remove_at(&mut self, at: usize) -> Option<Pooled> {
        let gone = self.items.remove(at)?;
        self.ids.remove(&gone.id);
        self.bytes -= gone.bytes;
        Some(gone)
    }
    pub fn push(&mut self, item: Pooled) {
        self.bytes += item.bytes;
        self.ids.insert(item.id);
        self.items.push_back(item);
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&Pooled) -> bool) {
        self.items.retain(|p| keep(p));
        self.ids = self.items.iter().map(|p| p.id).collect();
        self.bytes = self.items.iter().map(|p| p.bytes).sum();
    }
    /// The pool as a selection policy sees it, in arrival order.
    pub fn pending(&self) -> Vec<crate::mining::Pending<'_>> {
        self.items.iter().map(Pooled::pending).collect()
    }
}
impl Pooled {
    pub fn pending(&self) -> crate::mining::Pending<'_> {
        crate::mining::Pending {
            id: self.id,
            tx: &self.tx,
            bytes: self.bytes,
            arrived: self.arrived,
            source: self.source,
        }
    }
}
