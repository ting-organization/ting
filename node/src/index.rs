//! The block index: every header this node has accepted, with what it holds of
//! each block (wiki: node/sync). It keeps the bookkeeping that choosing,
//! downloading and connecting the best chain needs; the chain decides.
//!
//! Recent blocks, side branches and headers not yet stored are held in memory.
//! Canonical blocks at or below the floor are settled: the index reads them from
//! the database when asked (wiki: node/storage), so that its memory does not
//! grow with the chain. The chain moves the floor up as the tip moves on, and
//! down before it changes anything at or below it.
use crate::{
    Result,
    store::{self, Reader},
};
use num_bigint::BigUint;
use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{BTreeSet, HashMap},
};
use ting_core::{hash::Hash, meter::Resources, protocol::Header};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// A valid header; the block is not stored.
    Header,
    /// The block is stored and its body matches its header, but it has not
    /// been executed.
    Data,
    /// Executed and valid: its state changes are stored.
    Valid,
    /// It, or an ancestor, failed validation or was marked invalid by the
    /// operator.
    Invalid,
}

#[derive(Clone)]
pub struct Entry {
    pub header: Header,
    pub work: BigUint,
    /// The active limits the block runs under.
    pub limits: Resources,
    pub status: Status,
    /// The status before it became invalid, to return to if reconsidered.
    held: Status,
    /// The block is stored, whatever its status.
    pub stored: bool,
    /// This block and every ancestor are stored and none is invalid: its
    /// branch could be connected.
    complete: bool,
}
impl Entry {
    /// Stored and not invalid.
    pub fn has_data(&self) -> bool {
        matches!(self.status, Status::Data | Status::Valid)
    }
}

pub struct Index {
    genesis: Hash,
    /// The entries held in memory.
    entries: HashMap<Hash, Entry>,
    children: HashMap<Hash, Vec<Hash>>,
    /// Entries held and not invalid, by work: the last is the best header.
    by_work: BTreeSet<(BigUint, Hash)>,
    /// Complete entries with no complete child, by work: the heads of the
    /// branches that could be connected.
    heads: BTreeSet<(BigUint, Hash)>,
    /// How many held entries are stored.
    stored: usize,
    /// The height at or below which canonical blocks are settled, and where
    /// they are read from.
    floor: u64,
    reader: Reader,
    /// The first failure to read a settled block.
    error: RefCell<Option<String>>,
}

impl Index {
    pub fn new(genesis: Hash, reader: Reader, floor: u64) -> Self {
        Self {
            genesis,
            entries: HashMap::new(),
            children: HashMap::new(),
            by_work: BTreeSet::new(),
            heads: BTreeSet::new(),
            stored: 0,
            floor,
            reader,
            error: RefCell::new(None),
        }
    }
    /// How many blocks are stored: those held and the settled ones.
    pub fn stored(&self) -> usize {
        self.stored
            .saturating_add(usize::try_from(self.floor).unwrap_or(usize::MAX))
    }
    /// The height at or below which canonical blocks are settled.
    pub fn floor(&self) -> u64 {
        self.floor
    }
    /// A failure to read a settled block from the database, if one happened.
    pub fn storage_error(&self) -> Option<String> {
        self.error.borrow().clone()
    }
    fn failed<T>(&self, result: Result<Option<T>>) -> Option<T> {
        result.unwrap_or_else(|error| {
            self.error
                .borrow_mut()
                .get_or_insert(format!("block index: {error}"));
            None
        })
    }
    /// An entry, held or settled.
    pub fn get(&self, id: &Hash) -> Option<Cow<'_, Entry>> {
        if let Some(entry) = self.entries.get(id) {
            return Some(Cow::Borrowed(entry));
        }
        if self.floor == 0 {
            return None;
        }
        self.failed(self.settled(id)).map(Cow::Owned)
    }
    /// A settled block's entry, read from the database: executed, valid and
    /// canonical, with every ancestor.
    fn settled(&self, id: &Hash) -> Result<Option<Entry>> {
        let view = self.reader.view()?;
        let Some(stored) = view.stored(id)? else {
            return Ok(None);
        };
        let height = stored.header.height;
        if height > self.floor || view.canonical(height)? != Some(*id) {
            return Ok(None);
        }
        if stored.kept != store::Kept::Valid {
            return Err("a settled block is not valid".into());
        }
        let limits = view
            .limits(id)?
            .ok_or("missing limits of a settled block")?;
        Ok(Some(Entry {
            header: stored.header,
            work: stored.work,
            limits,
            status: Status::Valid,
            held: Status::Valid,
            stored: true,
            complete: true,
        }))
    }
    /// The settled canonical block at `height`, from 1 to the floor.
    pub fn settled_at(&self, height: u64) -> Option<Hash> {
        if height == 0 || height > self.floor {
            return None;
        }
        self.failed(self.reader.view().and_then(|view| view.canonical(height)))
    }
    pub fn contains(&self, id: &Hash) -> bool {
        self.get(id).is_some()
    }
    /// Whether the entry is held in memory.
    pub fn holds(&self, id: &Hash) -> bool {
        self.entries.contains_key(id)
    }
    /// How many entries are held in memory.
    pub fn held(&self) -> usize {
        self.entries.len()
    }
    /// The entries held in memory.
    pub fn held_entries(&self) -> impl Iterator<Item = (&Hash, &Entry)> {
        self.entries.iter()
    }
    /// Forgets held entries, each with every descendant among them: an old
    /// side branch the node no longer keeps. A parent left behind may be a
    /// branch's head again.
    pub fn forget(&mut self, ids: &std::collections::HashSet<Hash>) {
        let mut parents = Vec::new();
        for id in ids {
            let Some(entry) = self.entries.remove(id) else {
                continue;
            };
            if entry.stored {
                self.stored -= 1;
            }
            self.by_work.remove(&(entry.work.clone(), *id));
            self.heads.remove(&(entry.work, *id));
            self.children.remove(id);
            parents.push(entry.header.parent);
        }
        for parent in parents {
            if let Some(children) = self.children.get_mut(&parent) {
                children.retain(|c| !ids.contains(c));
                if children.is_empty() {
                    self.children.remove(&parent);
                }
            }
            if let Some(p) = self.entries.get(&parent)
                && p.complete
                && !self
                    .children(&parent)
                    .iter()
                    .any(|c| self.entries.get(c).is_some_and(|e| e.complete))
            {
                self.heads.insert((p.work.clone(), parent));
            }
        }
    }
    pub fn children(&self, id: &Hash) -> &[Hash] {
        self.children.get(id).map_or(&[], Vec::as_slice)
    }
    /// Whether the chain up to and including `id` (or genesis) holds every
    /// block and none is invalid.
    fn complete(&self, id: &Hash) -> bool {
        *id == self.genesis || self.get(id).is_some_and(|e| e.complete)
    }
    /// Settles canonical blocks, `(id, canonical child)` from the floor up: they
    /// leave memory, the database holding what they are (`Store::settle`).
    /// Each is executed, valid and complete, so neither it nor its canonical
    /// child is a branch's head.
    pub fn settle(&mut self, blocks: &[(Hash, Hash)], floor: u64) {
        for (id, child) in blocks {
            let entry = self.entries.remove(id).expect("a held canonical block");
            self.stored -= 1;
            self.by_work.remove(&(entry.work, *id));
            if let Some(children) = self.children.get_mut(id) {
                children.retain(|c| c != child);
                if children.is_empty() {
                    self.children.remove(id);
                }
            }
        }
        self.floor = floor;
    }
    /// Holds settled blocks in memory again, from the lowest up, each with its
    /// canonical child, before anything at or below the old floor changes.
    pub fn unsettle(&mut self, blocks: Vec<(store::Stored, Resources, Hash)>, floor: u64) {
        for (stored, limits, child) in blocks {
            let id = stored.header.id();
            self.stored += 1;
            self.by_work.insert((stored.work.clone(), id));
            let children = self.children.entry(id).or_default();
            if !children.contains(&child) {
                children.push(child);
            }
            self.entries.insert(
                id,
                Entry {
                    header: stored.header,
                    work: stored.work,
                    limits,
                    status: Status::Valid,
                    held: Status::Valid,
                    stored: true,
                    complete: true,
                },
            );
        }
        self.floor = floor;
    }
    /// Adds a header whose parent is genesis or already indexed; any status
    /// but `Header` means the block is stored. A header below an invalid block
    /// is invalid too.
    pub fn insert(&mut self, header: Header, work: BigUint, limits: Resources, status: Status) {
        let id = header.id();
        let parent = header.parent;
        let stored = status != Status::Header;
        if stored {
            self.stored += 1;
        }
        // A block stored as invalid failed itself: reconsidered, it is data
        // to execute again.
        let held = if status == Status::Invalid {
            Status::Data
        } else {
            status
        };
        let status = if self
            .get(&parent)
            .is_some_and(|p| p.status == Status::Invalid)
        {
            Status::Invalid
        } else {
            status
        };
        let complete = matches!(status, Status::Data | Status::Valid) && self.complete(&parent);
        self.children.entry(parent).or_default().push(id);
        if status != Status::Invalid {
            self.by_work.insert((work.clone(), id));
        }
        if complete {
            self.heads.insert((work.clone(), id));
            self.unhead(&parent);
        }
        self.entries.insert(
            id,
            Entry {
                header,
                work,
                limits,
                status,
                held,
                stored,
                complete,
            },
        );
    }
    /// The block is stored: its branch, and stored branches above it, may now
    /// be complete.
    pub fn set_data(&mut self, id: &Hash) {
        let Some(entry) = self.entries.get_mut(id) else {
            return;
        };
        if entry.status != Status::Header {
            return;
        }
        entry.status = Status::Data;
        entry.stored = true;
        self.stored += 1;
        let parent = entry.header.parent;
        if self.complete(&parent) {
            self.mark_complete(*id);
        }
    }
    fn mark_complete(&mut self, id: Hash) {
        let mut stack = vec![id];
        while let Some(id) = stack.pop() {
            let entry = self.entries.get_mut(&id).expect("indexed");
            if entry.complete || !entry.has_data() {
                continue;
            }
            entry.complete = true;
            let (work, parent) = (entry.work.clone(), entry.header.parent);
            self.heads.insert((work, id));
            self.unhead(&parent);
            stack.extend(self.children(&id).iter().copied());
        }
    }
    pub fn set_valid(&mut self, id: &Hash) {
        if let Some(entry) = self.entries.get_mut(id)
            && entry.status == Status::Data
        {
            entry.status = Status::Valid;
        }
    }
    /// The block failed validation, or was marked invalid: it and every
    /// descendant are invalid. Returns those that became invalid now.
    pub fn set_invalid(&mut self, id: &Hash) -> Vec<Hash> {
        let mut invalid = Vec::new();
        let mut stack = vec![*id];
        while let Some(id) = stack.pop() {
            let Some(entry) = self.entries.get_mut(&id) else {
                continue;
            };
            if entry.status == Status::Invalid {
                continue;
            }
            invalid.push(id);
            entry.held = entry.status;
            entry.status = Status::Invalid;
            entry.complete = false;
            self.by_work.remove(&(entry.work.clone(), id));
            self.heads.remove(&(entry.work.clone(), id));
            stack.extend(self.children(&id).iter().copied());
        }
        // The block's parent may be a branch's head again.
        if let Some(parent) = self.entries.get(id).map(|e| e.header.parent)
            && let Some(p) = self.entries.get(&parent)
            && p.complete
            && !self
                .children(&parent)
                .iter()
                .any(|c| self.entries[c].complete)
        {
            self.heads.insert((p.work.clone(), parent));
        }
        invalid
    }
    /// The block's body is gone, as when its frame cannot be read: its header
    /// stays, and no branch through it can be connected until it is stored
    /// again.
    pub fn forget_data(&mut self, id: &Hash) {
        let Some(entry) = self.entries.get_mut(id) else {
            return;
        };
        if !entry.has_data() {
            return;
        }
        entry.status = Status::Header;
        entry.stored = false;
        self.stored -= 1;
        let parent = entry.header.parent;
        let mut stack = vec![*id];
        while let Some(id) = stack.pop() {
            let entry = self.entries.get_mut(&id).expect("indexed");
            if !entry.complete {
                continue;
            }
            entry.complete = false;
            self.heads.remove(&(entry.work.clone(), id));
            stack.extend(self.children(&id).iter().copied());
        }
        if let Some(p) = self.entries.get(&parent)
            && p.complete
            && !self
                .children(&parent)
                .iter()
                .any(|c| self.entries[c].complete)
        {
            self.heads.insert((p.work.clone(), parent));
        }
    }
    /// A complete entry with a complete child is no branch's head.
    fn unhead(&mut self, id: &Hash) {
        if let Some(entry) = self.entries.get(id) {
            self.heads.remove(&(entry.work.clone(), *id));
        }
    }
    /// The heads of the branches that could be connected, most work first.
    pub fn heads(&self) -> impl Iterator<Item = &Hash> {
        self.heads.iter().rev().map(|(_, id)| id)
    }
    /// Clears the invalid status of a block, of every descendant and of every
    /// ancestor, each returning to what it was before. Returns those cleared,
    /// parents first.
    pub fn reconsider(&mut self, id: &Hash) -> Vec<Hash> {
        let mut cleared = Vec::new();
        let mut up = self.entries.get(id).map(|e| e.header.parent);
        while let Some(parent) = up {
            match self.entries.get(&parent) {
                Some(entry) if entry.status == Status::Invalid => {
                    cleared.push(parent);
                    up = Some(entry.header.parent);
                }
                _ => up = None,
            }
        }
        cleared.reverse();
        let mut stack = vec![*id];
        while let Some(id) = stack.pop() {
            let Some(entry) = self.entries.get(&id) else {
                continue;
            };
            if entry.status == Status::Invalid {
                cleared.push(id);
            }
            stack.extend(self.children(&id).iter().copied());
        }
        cleared.sort_by_key(|id| self.entries[id].header.height);
        for id in &cleared {
            let entry = self.entries.get_mut(id).expect("indexed");
            entry.status = entry.held;
            self.by_work.insert((entry.work.clone(), *id));
        }
        for id in &cleared {
            let parent = self.entries[id].header.parent;
            if self.complete(&parent) {
                self.mark_complete(*id);
            }
        }
        cleared
    }
    /// The header with the most work that is not invalid.
    pub fn best_header(&self) -> Option<Hash> {
        self.by_work.last().map(|(_, id)| *id)
    }
}
