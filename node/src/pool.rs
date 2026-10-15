//! The transaction pool: what this node offers its miner, in arrival order,
//! within fixed bounds (wiki: node/mining). Policy, not consensus.
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    net::IpAddr,
};
use ting_core::{hash::Hash, protocol::Transaction};

/// Most transactions the pool holds; its bytes are bounded by the chain, by
/// the data capacity. A template runs each pooled transaction against what is
/// left of its block, so the compute they declare needs no bound of its own.
pub const POOL_MAX: usize = 1024;
/// Most transactions one peer address's relaying may hold in the pool.
pub const POOL_PER_SOURCE: usize = 64;
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
    sources: BTreeMap<IpAddr, usize>,
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
    fn count_of(&self, source: Option<IpAddr>) -> usize {
        source.map_or(0, |s| self.sources.get(&s).copied().unwrap_or(0))
    }
    /// Makes room for a transaction of `bytes` from `source` in a pool of at
    /// most `most` bytes, if the rules allow (wiki: node/mining): under its
    /// source's share, and when the pool is full, in place of the most recent
    /// transaction of the address holding the most, if that address holds
    /// more than `source`.
    pub fn make_room(&mut self, bytes: usize, source: Option<IpAddr>, most: usize) -> bool {
        if self.count_of(source) >= POOL_PER_SOURCE || bytes > most {
            return false;
        }
        while self.items.len() >= POOL_MAX || self.bytes.saturating_add(bytes) > most {
            let Some((&largest, &held)) = self.sources.iter().max_by_key(|(_, n)| **n) else {
                return false;
            };
            if held <= self.count_of(source) {
                return false;
            }
            let at = self
                .items
                .iter()
                .rposition(|p| p.source == Some(largest))
                .expect("counted sources hold transactions");
            let gone = self.items.remove(at).expect("found above");
            self.forget(&gone);
        }
        true
    }
    fn forget(&mut self, gone: &Pooled) {
        self.ids.remove(&gone.id);
        self.bytes -= gone.bytes;
        if let Some(source) = gone.source
            && let Some(n) = self.sources.get_mut(&source)
        {
            *n -= 1;
            if *n == 0 {
                self.sources.remove(&source);
            }
        }
    }
    pub fn push(&mut self, item: Pooled) {
        self.bytes += item.bytes;
        if let Some(source) = item.source {
            *self.sources.entry(source).or_default() += 1;
        }
        self.ids.insert(item.id);
        self.items.push_back(item);
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&Pooled) -> bool) {
        self.items.retain(|p| keep(p));
        self.ids = self.items.iter().map(|p| p.id).collect();
        self.bytes = self.items.iter().map(|p| p.bytes).sum();
        self.sources.clear();
        for source in self.items.iter().filter_map(|p| p.source) {
            *self.sources.entry(source).or_default() += 1;
        }
    }
}
