use crate::{
    Result,
    config::{self, Config},
    hash_arg,
    index::{Index, Status as Kept},
    mining, now,
    pool::{POOL_AGE, Pool, Pooled},
    presync::{Outcome, Presync, Walk},
    reads,
    store::{self, Location, Store},
    unhex, wire,
};
use num_bigint::BigUint;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};
use ting_core::{
    consensus,
    hash::{Hash, hex},
    meter::Resources,
    protocol::{Block, Encode, Genesis, HEADER_LEN, Header, Status, Transaction},
    state::State,
    vm::{CodeCache, Context, Runtime},
};
use tokio::sync::oneshot;

/// Largest block slice a peer request returns; larger blocks travel in pieces.
pub const SLICE_MAX: usize = 1024 * 1024;
/// Most block ids a locator names.
pub const LOCATOR_MAX: usize = 32;

/// A block whose timestamp is further ahead of the clock than this is not
/// admitted until the clock catches up (rules §9): policy,
/// not validity, so the same block is accepted later.
pub const CLOCK_SLACK: u64 = 7_200;
pub const AHEAD_OF_CLOCK: &str = "timestamp ahead of the clock";
/// The answer to capacity votes that are not four of 0 (hold), 1 (increase)
/// and 2 (decrease).
const VOTES: &str = "votes are four of 0 (hold), 1 (increase) or 2 (decrease), in C, S, D, G order";
/// Refusals that are this node's policy rather than a peer's fault; a peer
/// connection carries on past them (wiki: node/sync).
pub const POOL_FULL: &str = "mempool is full";
pub const DEFERRED_FULL: &str = "deferred set is full";
pub const HORIZON: &str = "not_before is more than a year ahead";
pub const BEHIND: &str = "branch too far behind the tip";
/// A transaction whose bytes exceed the data capacity of the next block.
pub const TOO_LARGE: &str = "transaction larger than the data capacity";
/// A transaction fetched by id that is not the one with that id.
pub const NOT_ASKED: &str = "not the transaction asked for";
pub const INCLUDED: &str = "transaction already included";
/// A block the operator marked invalid, or one above it: refused, but not held
/// against the peer that sent it.
pub const MARKED: &str = "block marked invalid by this node's operator";
/// A transaction a peer relays is refused when a canonical block among the last
/// this many included it and it succeeded there: a pool keeps a transaction at
/// most a day, so a relayed one this recent is one a block just took, which
/// would otherwise travel back into the pools and be mined again.
pub const RELAY_RECENT: u64 = 1_440;
/// One that failed is refused only when a block among the last this many
/// included it, while that block's echo lasts: pools drop what a block takes,
/// so this stops it travelling back, and a miner who made it fail on purpose
/// cannot keep it from the network for a day.
pub const RELAY_ECHO: u64 = 10;
/// The start of every error that proves a peer sent invalid data: a header
/// or block that does not check (`invalid header: …`, `invalid block: …`).
/// Any other refusal, whatever its words, may be policy, timing, or this
/// node's own state, such as a storage error.
pub const INVALID: &str = "invalid ";
pub fn is_invalid(error: &str) -> bool {
    error.starts_with("invalid header: ") || error.starts_with("invalid block: ")
}
/// What changed in the chain, published after every request: the tip and the
/// size of the pool and the deferred set, which peers are told about, and the
/// number of stored blocks, which tells the node's own connections that more
/// blocks may be fetched.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub tip: Hash,
    pub height: u64,
    pub work: String,
    pub pending: usize,
    pub deferred: usize,
    pub stored: usize,
    /// The data capacity of the next block: the most bytes a transaction has.
    pub data: u64,
    /// The height of the best header, which the node is downloading toward.
    pub best: u64,
}
pub fn is_policy(error: &str) -> bool {
    matches!(
        error,
        AHEAD_OF_CLOCK
            | POOL_FULL
            | DEFERRED_FULL
            | HORIZON
            | BEHIND
            | TOO_LARGE
            | INCLUDED
            | MARKED
    )
}

/// A header chain is indexed once its work comes within this many blocks of
/// the tip's work (about a day); below that it is checked without being kept
/// (`presync`), so a cheap chain from far back costs no memory.
const THRESHOLD_BLOCKS: u32 = 1_440;
/// Headers per `headers` answer.
pub const HEADERS_PAGE: usize = 2_000;
/// Block download along the best header's branch: how far beyond the lowest
/// missing block, and how many with one peer at once. A block stays asked of
/// one peer for its `transfer_time`, and one that lapsed is not asked of that
/// peer again for `LAPSED_WAIT` seconds.
const WINDOW: usize = 1_024;
const PER_PEER: usize = 64;
const LAPSED_WAIT: u64 = 120;
/// The time a peer has to deliver a block or transaction of up to `bytes`, in
/// slices (wiki: node/sync): a minute, and ten seconds more for every slice,
/// so that size alone never makes an item fail, only a lack of progress.
pub fn transfer_time(bytes: u64) -> Duration {
    Duration::from_secs(
        60u64.saturating_add(10u64.saturating_mul(bytes.div_ceil(SLICE_MAX as u64))),
    )
}
/// Most blocks a reorganization disconnects, or connects by their stored
/// changes, in one database transaction.
const STEP: usize = 64;
/// The most canonical blocks settled in one database transaction.
const SETTLE_MAX: u64 = 65_536;
/// A reorganization that replaces at least this many blocks is logged as a
/// warning.
const DEEP_REORG: usize = 6;
/// A branch that failed validation here and carries at least this many blocks
/// of work more than the tip raises a warning.
const FORK_WARNING: u32 = 6;

/// Deferred-set policy: how many transactions a node holds for later, how far
/// ahead `not_before` may lie, how long one stays after that time without being
/// included, and how often a due one is re-executed when the tip has not moved.
/// Its bytes are bounded as the pool's are (`held_bytes`).
pub const DEFERRED_MAX: usize = 4096;
pub const DEFERRED_HORIZON: u64 = 366 * 86_400;
pub const DEFERRED_GRACE: u64 = 30 * 86_400;
const DEFERRED_RETRY: u64 = 30;
/// Seconds between passes over due deferred entries, each running at most
/// about one block's compute of trials.
const DEFERRED_PASS: u64 = 2;
/// Why a trial leaves a transaction out: it does not fit even a block that
/// holds nothing else.
const DOES_NOT_FIT: &str = "does not fit a block";
/// How long a change to the deferred set may wait before it is written.
const DEFERRED_SAVE: u64 = 5;

/// How a template picks transactions from the pool: the node's selector
/// (`mining::Selector`, first in unless a fork says otherwise), or exactly the
/// list a caller hands over.
pub enum Selection {
    Policy,
    Explicit(Vec<Hash>),
}

/// Templates kept for solutions: candidates on the current tip, by the id of
/// their nonce-less header.
const TEMPLATES_KEPT: usize = 8;

/// A transaction held until it is due and executes successfully on the tip,
/// then placed in the pool like any other (wiki: node/mining).
#[derive(Clone)]
pub struct Deferred {
    pub tx: Transaction,
    pub not_before: u64,
    pub expires: u64,
    /// Its encoded size, and the height when it was submitted: only a block
    /// above that height counts as including it, so the same envelope can be
    /// deferred again after it was mined once.
    bytes: usize,
    since: u64,
    /// Last admission attempt: when, against which tip, and why it failed.
    last_attempt: u64,
    last_tip: Option<Hash>,
    last_result: Option<String>,
    /// In the pool right now, awaiting a block.
    admitted: bool,
    /// The address of the peer that relayed it; none for the node's own API
    /// and CLI.
    source: Option<std::net::IpAddr>,
}
/// Most entries the deferred set keeps that count against one address.
pub const DEFERRED_PER_SOURCE: usize = 256;
/// Ids in one `inventory` page, and transactions in one `pending` or
/// `deferred` page.
pub const INVENTORY_PAGE: usize = 1_024;
pub const RELAY_PAGE: usize = 32;
/// The most bytes of transactions a `pending` or `deferred` answer carries
/// whole: one slice, so that the answer crosses a connection within the frame
/// deadline as a block's slice does, on the same links.
pub const RELAY_INLINE: usize = SLICE_MAX;
/// The address a submission counts against: the public REST client's, or the
/// relaying peer's; none for the node's own CLI and local clients.
fn source_of(p: &Value) -> Option<std::net::IpAddr> {
    if let Some(client) = p["client"].as_str() {
        return client.parse().ok();
    }
    if p["relayed"] != true {
        return None;
    }
    let peer = p["peer"].as_str()?;
    peer.parse::<std::net::SocketAddr>()
        .map(|a| a.ip())
        .or_else(|_| peer.parse())
        .ok()
}

/// A replay that rebuilds undo data below the window (wiki: node/storage).
struct Recovery {
    /// Undo data is rebuilt for canonical heights `down_to + 1 ..= upto`.
    down_to: u64,
    upto: u64,
    dir: PathBuf,
    thread: Option<std::thread::JoinHandle<Result<()>>>,
    /// The replay's database once it is done, and the next height to copy.
    replayed: Option<Store>,
    copied: u64,
}
/// Replays canonical `blocks` from genesis into a fresh database in `dir`,
/// from the block files of the data directory `data`, checking every block as
/// on first sight, and keeps the undo data of those above `down_to`.
fn replay(
    data: &Path,
    dir: &Path,
    genesis: &Genesis,
    blocks: &[(Hash, store::Location, Resources)],
    down_to: u64,
    shutdown: &AtomicBool,
    cache: u64,
) -> Result<()> {
    let store = Store::open(dir, genesis, cache)?;
    let code = CodeCache::default();
    let mut ancestry = consensus::Ancestry::default();
    for (i, (id, location, limits)) in blocks.iter().enumerate() {
        if shutdown.load(Ordering::Relaxed) {
            return Err("the node is stopping".into());
        }
        let height = i as u64 + 1;
        let bytes = store::frame_at(data, *location)?;
        let block = Block::decode(&bytes, bytes.len()).map_err(|e| e.to_string())?;
        if block.header.id() != *id {
            return Err("replayed block identity mismatch".into());
        }
        // A block executes against the last commit, through a view whose
        // tables open once, and its changes are written after.
        let validator = Runtime::with_cache(
            store.snapshot()?,
            Context::from_genesis(genesis),
            code.clone(),
        );
        validator
            .validate_and_apply_block_at(genesis, &ancestry, &block, *limits)
            .map_err(|e| format!("replay of block {height}: {e}"))?;
        if let Some(error) = validator.storage_error() {
            return Err(error);
        }
        let changes = validator.state_changes()?;
        drop(validator);
        let tx = Rc::new(store.begin()?);
        Store::apply(&tx, &changes)?;
        if height > down_to {
            let undo: Vec<(Hash, Option<Vec<u8>>)> = changes
                .records
                .iter()
                .map(|c| (c.key, c.before.clone()))
                .collect();
            Store::put_undo(&tx, *id, &undo)?;
        }
        let tx = Rc::try_unwrap(tx).map_err(|_| "replay transaction still borrowed")?;
        store.commit_state(tx)?;
        let mut recent = vec![block.header.timestamp];
        recent.extend(ancestry.recent.iter().take(10));
        ancestry = consensus::Ancestry {
            height,
            parent: Some(block.header),
            recent,
        };
    }
    Ok(())
}

/// Whether a block whose branch has accumulated `work` replaces a tip with
/// `tip` work: only by more than half the block's own work. Of two branches of
/// one length, the one stamped earlier carries slightly more work under ASERT;
/// without the margin a withholding miner would win every such race
/// (rules §8).
fn overtakes(work: &BigUint, header: &Header, tip: &BigUint) -> bool {
    *work > tip + consensus::work(&header.target) / 2u32
}
/// Block ids by height along a branch from genesis. Those at or below the
/// index's floor are settled canonical blocks, which the index reads from the
/// database; the rest are held here.
#[derive(Default)]
struct Branch {
    floor: usize,
    ids: Vec<Hash>,
}
impl Branch {
    fn len(&self) -> usize {
        self.floor + self.ids.len()
    }
    /// The block at `i`, height `i + 1`.
    fn get(&self, i: usize, index: &Index) -> Option<Hash> {
        match i.checked_sub(self.floor) {
            Some(held) => self.ids.get(held).copied(),
            None => index.settled_at(i as u64 + 1),
        }
    }
    fn last(&self, index: &Index) -> Option<Hash> {
        self.get(self.len().checked_sub(1)?, index)
    }
    /// The blocks held, from height `floor + 1`.
    fn held(&self) -> &[Hash] {
        &self.ids
    }
    /// Keeps the first `len` blocks; none settled may go.
    fn truncate(&mut self, len: usize) {
        assert!(len >= self.floor, "a settled block cannot leave its branch");
        self.ids.truncate(len - self.floor);
    }
    /// The blocks from `at` on, which leave the branch; none settled may go.
    fn split_off(&mut self, at: usize) -> Vec<Hash> {
        assert!(at >= self.floor, "a settled block cannot leave its branch");
        self.ids.split_off(at - self.floor)
    }
    fn extend(&mut self, ids: impl IntoIterator<Item = Hash>) {
        self.ids.extend(ids);
    }
    /// The floor rises to `floor`: the blocks below it are settled.
    fn settle(&mut self, floor: usize) {
        self.ids.drain(..floor - self.floor);
        self.floor = floor;
    }
    /// The floor falls to `floor`: `ids`, the settled blocks above it, are
    /// held again.
    fn unsettle(&mut self, floor: usize, ids: &[Hash]) {
        self.ids.splice(0..0, ids.iter().copied());
        self.floor = floor;
    }
}
pub struct Chain {
    pub genesis: Genesis,
    /// The ids of the chain's seed blocks, by height (wiki:
    /// reference/conformance#the-seed).
    seed: &'static [Hash],
    pub config: Config,
    directory: PathBuf,
    store: Store,
    /// Every accepted header, with what this node holds of its block.
    index: Index,
    /// The connected branch, block ids by height.
    canonical: Branch,
    /// The best header's branch, block ids by height: blocks are downloaded
    /// along it. Its settled blocks are the canonical ones.
    best: Branch,
    /// The lowest height on `best` whose block may not be stored yet.
    frontier: usize,
    /// The tip a move toward another branch began from, with its work, while
    /// the move goes on.
    moving_from: Option<(Hash, BigUint)>,
    /// The height up to which canonical blocks have no undo data, and the
    /// replay that restores it when a branch forks below that.
    pruned: u64,
    recovery: Option<Recovery>,
    /// How long one call may work toward the best branch (none: until done),
    /// and whether steps are left, with whether disconnected transactions
    /// return to the pool.
    step_time: Option<Duration>,
    activating: Option<bool>,
    /// The limits after a vote boundary, by the parent before it.
    boundaries: std::cell::RefCell<BTreeMap<Hash, Resources>>,
    /// Where capacity voting stood at a tip, as last asked.
    voting: std::cell::RefCell<Option<(Hash, Value)>>,
    /// The transaction last served in slices, so that its next slice is not
    /// encoded again.
    served_transaction: Option<(Hash, std::rc::Rc<Vec<u8>>)>,
    /// Blocks asked of a peer, and when; and blocks a peer let expire, which
    /// are not given back to that peer for a while.
    in_flight: BTreeMap<Hash, (String, u64)>,
    expired: BTreeMap<Hash, (String, u64)>,
    /// Header chains under check, by peer.
    presync: BTreeMap<String, Presync>,
    /// The peer each stored, not yet connected block came from, and the peers
    /// whose blocks have since failed: they sent invalid data.
    sources: BTreeMap<Hash, String>,
    offenders: Vec<String>,
    /// Blocks the operator marked invalid, whether seen yet or not.
    marks: BTreeSet<Hash>,
    /// Stored blocks whose parent is not indexed, by parent, as after a restart
    /// in the middle of a download: indexed once the parent's header is.
    orphans: BTreeMap<Hash, Vec<store::Stored>>,
    /// The most work of a branch that failed validation here, and whether the
    /// warning it raised was logged.
    failed_work: BigUint,
    warned: bool,
    /// The reorganization under way, as the tip and height it started from
    /// and the lowest height it reached; the last one completed.
    unwinding: Option<(Hash, usize, usize)>,
    reorganization: Option<Value>,
    runtime: Runtime,
    /// Validated programs, shared by every runtime the chain builds, so that
    /// a program is decoded once rather than in every block.
    code: CodeCache,
    pool: Pool,
    deferred: BTreeMap<Hash, Deferred>,
    /// Deferred entries that connected blocks included successfully, by the
    /// block and its height, to restore if that block is disconnected.
    taken: BTreeMap<Hash, (u64, Vec<(Hash, Deferred)>)>,
    /// When the deferred set first changed since it was last written.
    deferred_dirty: Option<u64>,
    /// When the next pass over due deferred entries may run, and the tip of
    /// the last one: a new tip runs a pass at once.
    deferred_next: u64,
    deferred_tip: Option<Hash>,
    templates: VecDeque<(Hash, Block)>,
    selector: Box<dyn mining::Selector>,
    started: u64,
    shutdown: Arc<AtomicBool>,
    failed: Option<String>,
}
impl Chain {
    pub fn open(dir: &Path, shutdown: Arc<AtomicBool>) -> Result<Self> {
        Self::open_with(dir, shutdown, Box::new(mining::FirstIn::default()))
    }
    /// Opens the chain with a fork's own transaction selection.
    pub fn open_with(
        dir: &Path,
        shutdown: Arc<AtomicBool>,
        selector: Box<dyn mining::Selector>,
    ) -> Result<Self> {
        let genesis = config::genesis(dir)?;
        let config = Config::load(dir)?;
        let store = Store::open(dir, &genesis, config.cache_bytes())?;
        let metadata = store.metadata()?;
        let code = CodeCache::default();
        let runtime = Runtime::with_cache(
            store.snapshot()?,
            Context::from_genesis(&genesis),
            code.clone(),
        );
        let index = Index::new(genesis.block_id(), store.reader(), metadata.floor);
        let floor = usize::try_from(metadata.floor).map_err(|e| e.to_string())?;
        let mut chain = Self {
            seed: crate::seed::ids(&genesis),
            genesis,
            config,
            directory: dir.into(),
            store,
            index,
            canonical: Branch {
                floor,
                ids: metadata.canonical,
            },
            best: Branch {
                floor,
                ids: Vec::new(),
            },
            frontier: 0,
            moving_from: None,
            pruned: 0,
            recovery: None,
            step_time: None,
            activating: None,
            boundaries: Default::default(),
            voting: Default::default(),
            served_transaction: None,
            in_flight: BTreeMap::new(),
            expired: BTreeMap::new(),
            presync: BTreeMap::new(),
            sources: BTreeMap::new(),
            offenders: Vec::new(),
            marks: BTreeSet::new(),
            orphans: BTreeMap::new(),
            failed_work: BigUint::default(),
            warned: false,
            unwinding: None,
            reorganization: None,
            runtime,
            code,
            pool: Pool::default(),
            deferred: BTreeMap::new(),
            taken: BTreeMap::new(),
            deferred_dirty: None,
            deferred_next: 0,
            deferred_tip: None,
            templates: VecDeque::new(),
            selector,
            started: now(),
            shutdown,
            failed: None,
        };
        if chain.tip() != metadata.tip {
            return Err("persisted tip mismatch".into());
        }
        // The index, parents first. A stored block whose parent was only a
        // header waits until that header comes again.
        let mut blocks = metadata.blocks;
        blocks.sort_by_key(|(_, b)| b.header.height);
        for (_, stored) in blocks {
            let parent = stored.header.parent;
            if parent != chain.genesis.block_id() && !chain.index.contains(&parent) {
                chain.orphans.entry(parent).or_default().push(stored);
                continue;
            }
            chain.index_stored(stored)?;
        }
        // The held canonical blocks follow from the highest settled one.
        let mut parent = match floor {
            0 => chain.genesis.block_id(),
            _ => chain
                .index
                .settled_at(floor as u64)
                .ok_or("missing settled canonical block")?,
        };
        for (i, id) in chain.canonical.held().iter().enumerate() {
            let entry = chain.index.get(id).ok_or("missing canonical header")?;
            if entry.header.parent != parent
                || entry.header.height != (floor + i) as u64 + 1
                || entry.status != Kept::Valid
            {
                return Err("persisted canonical ancestry mismatch".into());
            }
            parent = *id;
        }
        if chain.runtime.state_root() != chain.root_at(chain.tip())? {
            return Err("persisted tip state mismatch".into());
        }
        // The operator's marks hold in memory only: what is stored of a marked
        // block stays as it was, for when it is reconsidered.
        chain.pruned = chain.store.pruned()?;
        let _ = std::fs::remove_dir_all(chain.directory.join("replay"));
        chain.marks = chain.store.marks()?.into_iter().collect();
        for id in chain.marks.clone() {
            chain.hold(&id)?;
            chain.index.set_invalid(&id);
        }
        chain.sync_best();
        // Frames written after the database's last commit are stored again; a
        // frame whose block can no longer be indexed, such as one whose parent
        // was only a header, is passed over and its block fetched again.
        let mut waiting = Vec::new();
        while let Some((bytes, location)) = chain.store.pending()? {
            if let Err(error) = chain.accept(&bytes, Some(location)) {
                if chain.failed.is_some() {
                    return Err(error);
                }
                chain.store.file_progress(location)?;
                waiting.push((bytes, location));
            }
        }
        // A frame written before its parent's is tried again once the others
        // are in, for as long as that indexes more.
        loop {
            let before = waiting.len();
            waiting.retain(|(bytes, location)| chain.accept(bytes, Some(*location)).is_err());
            if chain.failed.is_some() {
                return Err(chain.failed.clone().unwrap_or_default());
            }
            if waiting.len() == before {
                break;
            }
        }
        // A stored branch with more work than the tip, as after a crash in the
        // middle of a reorganization, is connected now, and a tip the operator
        // marked invalid while the node was down is left. A branch that cannot
        // be connected, such as one whose way back runs through damaged undo
        // data, leaves the node on its tip, not stopped.
        if let Err(error) = chain
            .leave_invalid(false)
            .and_then(|()| chain.activate(false))
        {
            eprintln!("the best stored branch is not connected: {error}");
        }
        // What is below the held heights settles, all of it at the first
        // start of a node that held every block, and old side branches go.
        while chain.settle()? {}
        chain.forget_old_branches()?;
        chain.fill_limits()?;
        // What that moved is on disk before the node serves anyone.
        chain.store.flush()?;
        chain.load_deferred()?;
        Ok(chain)
    }
    pub fn height(&self) -> usize {
        self.canonical.len()
    }
    /// Indexes a stored block whose parent is indexed: its height, proof of
    /// work and work follow from its parent, and its limits from its branch.
    fn index_stored(&mut self, stored: store::Stored) -> Result<()> {
        let parent = stored.header.parent;
        let (height, work) = if parent == self.genesis.block_id() {
            (0, BigUint::default())
        } else {
            let p = self.index.get(&parent).ok_or("missing stored parent")?;
            (p.header.height, p.work.clone())
        };
        if Some(stored.header.height) != height.checked_add(1)
            || !consensus::valid_pow(&stored.header)
            || stored.work != work + consensus::work(&stored.header.target)
        {
            return Err("persisted block work mismatch".into());
        }
        let limits = self.limits_after(parent)?;
        let status = match stored.kept {
            store::Kept::Data => Kept::Data,
            store::Kept::Valid => Kept::Valid,
            store::Kept::Invalid => Kept::Invalid,
        };
        let id = stored.header.id();
        self.index
            .insert(stored.header, stored.work, limits, status);
        if self.marks.contains(&id) {
            self.index.set_invalid(&id);
        }
        Ok(())
    }
    /// Indexes a checked header, then the stored blocks that waited for it.
    fn index_header(&mut self, header: Header, work: BigUint, limits: Resources) {
        let id = header.id();
        self.index.insert(header, work, limits, Kept::Header);
        if self.marks.contains(&id) {
            self.index.set_invalid(&id);
        }
        let mut stack = vec![id];
        while let Some(parent) = stack.pop() {
            for stored in self.orphans.remove(&parent).unwrap_or_default() {
                let child = stored.header.id();
                match self.index_stored(stored) {
                    Ok(()) => stack.push(child),
                    Err(error) => eprintln!("stored block {} not indexed: {error}", hex(&child)),
                }
            }
        }
    }
    /// Whether an invalid block is invalid only because the operator marked it
    /// or a block below it: refusing it then says nothing about the peer.
    fn by_operator(&self, id: &Hash) -> bool {
        let mut id = *id;
        loop {
            if self.marks.contains(&id) {
                return true;
            }
            match self.index.get(&id) {
                Some(entry) if entry.status == Kept::Invalid => id = entry.header.parent,
                _ => return false,
            }
        }
    }
    /// A branch with `work` failed validation here. One well ahead of the tip
    /// means that this node and much of the network disagree on the rules.
    fn failed_branch(&mut self, work: BigUint) {
        if work > self.failed_work {
            self.failed_work = work;
        }
        match self.fork_warning() {
            Some(warning) if !self.warned => {
                eprintln!("warning: {warning}");
                self.warned = true;
            }
            Some(_) => {}
            None => self.warned = false,
        }
    }
    fn fork_warning(&self) -> Option<String> {
        let target = match self.index.get(&self.tip()) {
            Some(entry) => entry.header.target.clone(),
            None => self.genesis.initial_target.clone(),
        };
        let block = consensus::work(&target);
        let tip = self.tip_work();
        if self.failed_work <= tip.clone() + block.clone() * FORK_WARNING {
            return None;
        }
        let ahead = (self.failed_work.clone() - tip) / block;
        Some(format!(
            "a branch about {ahead} blocks heavier than this node's chain failed validation here; \
             this node or much of the network follows other rules: check for an update"
        ))
    }
    pub fn event(&self) -> Event {
        Event {
            tip: self.tip(),
            height: self.height() as u64,
            work: self.tip_work().to_string(),
            pending: self.pool.len(),
            deferred: self.deferred.len(),
            stored: self.index.stored(),
            data: self.tip_limits().map_or(u64::MAX, |limits| limits.0[2]),
            best: self.best.len() as u64,
        }
    }
    /// Which network the node is on, by its genesis: the production network,
    /// a development chain, or another.
    fn network(&self) -> &'static str {
        let id = self.genesis.chain_id();
        if crate::config::production_genesis().is_some_and(|g| g.chain_id() == id) {
            "production"
        } else if crate::config::development_genesis().chain_id() == id {
            "development"
        } else {
            "other"
        }
    }
    pub fn tip(&self) -> Hash {
        self.canonical
            .last(&self.index)
            .unwrap_or(self.genesis.block_id())
    }
    fn tip_work(&self) -> BigUint {
        self.index
            .get(&self.tip())
            .map_or_else(BigUint::default, |b| b.work.clone())
    }
    fn entry(&self, id: &Hash) -> std::borrow::Cow<'_, crate::index::Entry> {
        self.index.get(id).expect("an indexed block")
    }
    /// Whether an indexed block is on the connected branch: one lookup by height.
    fn is_canonical(&self, id: &Hash) -> bool {
        self.index.get(id).is_some_and(|b| {
            usize::try_from(b.header.height)
                .ok()
                .and_then(|h| self.canonical.get(h.checked_sub(1)?, &self.index))
                == Some(*id)
        })
    }
    /// The branch ending at `tip` against the connected one: how many connected
    /// blocks they share, and `tip`'s own blocks after those, oldest first.
    /// The walk goes only as deep as the branches differ.
    fn fork(&self, mut tip: Hash) -> Result<(usize, Vec<Hash>)> {
        let mut ids = Vec::new();
        let common = loop {
            if tip == self.genesis.block_id() {
                break 0;
            }
            if self.is_canonical(&tip) {
                break self.entry(&tip).header.height as usize;
            }
            ids.push(tip);
            tip = self.index.get(&tip).ok_or("unknown parent")?.header.parent;
        };
        ids.reverse();
        Ok((common, ids))
    }
    /// What validating the block after `parent` needs of its branch: its
    /// height, its last header and its last eleven timestamps.
    fn ancestry(&self, parent: Hash) -> Result<consensus::Ancestry> {
        if parent == self.genesis.block_id() {
            return Ok(consensus::Ancestry::default());
        }
        let header = self
            .index
            .get(&parent)
            .ok_or("unknown parent")?
            .header
            .clone();
        let mut recent = Vec::with_capacity(11);
        let mut id = parent;
        while recent.len() < 11 && id != self.genesis.block_id() {
            let b = self.entry(&id);
            recent.push(b.header.timestamp);
            id = b.header.parent;
        }
        Ok(consensus::Ancestry {
            height: header.height,
            parent: Some(header),
            recent,
        })
    }
    fn root_at(&self, id: Hash) -> Result<Hash> {
        if id == self.genesis.block_id() {
            Ok(State::default().root())
        } else {
            Ok(self
                .index
                .get(&id)
                .ok_or("unknown state root")?
                .header
                .state_root)
        }
    }
    /// The active limits of the block after `parent` (rules
    /// §10): within an epoch those of `parent`; at the
    /// first block of epoch 3 or later, the limits of the epoch before, moved by
    /// the votes of the source epoch three before, read along the branch.
    fn limits_after(&self, parent: Hash) -> Result<Resources> {
        let e = u64::from(self.genesis.vote_epoch);
        let height = if parent == self.genesis.block_id() {
            0
        } else {
            self.index
                .get(&parent)
                .ok_or("unknown parent")?
                .header
                .height
        };
        let epoch = height / e;
        if epoch < 3 {
            return Ok(self.genesis.initial);
        }
        let previous = self.entry(&parent).limits;
        if height % e != 0 {
            return Ok(previous);
        }
        // At a boundary the votes of a whole epoch are counted, once per
        // parent: every child of the same block reuses the result.
        if let Some(limits) = self.boundaries.borrow().get(&parent) {
            return Ok(*limits);
        }
        // The source epoch holds heights (epoch - 3) * e + 1 ..= (epoch - 2) * e.
        let mut id = parent;
        while self.entry(&id).header.height > (epoch - 2) * e {
            id = self.entry(&id).header.parent;
        }
        let mut tally = consensus::Tally::default();
        for _ in 0..e {
            let b = self.entry(&id);
            tally.add(&b.header);
            id = b.header.parent;
        }
        let limits = consensus::next_limits_tally(&self.genesis, previous, &tally);
        let mut boundaries = self.boundaries.borrow_mut();
        if boundaries.len() >= 1_024 {
            boundaries.clear();
        }
        boundaries.insert(parent, limits);
        Ok(limits)
    }
    /// The state a header check along the branch after `parent` starts from:
    /// its ancestry, the next block's limits, its work, and the tallies of the
    /// epochs a later boundary may still count.
    fn walk_from(&self, parent: Hash) -> Result<Walk> {
        let e = u64::from(self.genesis.vote_epoch);
        let (height, work) = if parent == self.genesis.block_id() {
            (0, BigUint::default())
        } else {
            let p = self.index.get(&parent).ok_or("unknown parent")?;
            (p.header.height, p.work.clone())
        };
        let first_source = ((height + e) / e).saturating_sub(3);
        let mut tallies: BTreeMap<u64, consensus::Tally> = BTreeMap::new();
        let mut id = parent;
        while id != self.genesis.block_id() {
            let b = self.entry(&id);
            if b.header.height < first_source * e + 1 {
                break;
            }
            tallies
                .entry((b.header.height - 1) / e)
                .or_default()
                .add(&b.header);
            id = b.header.parent;
        }
        Ok(Walk {
            seed: self.seed,
            ancestry: self.ancestry(parent)?,
            limits: self.limits_after(parent)?,
            work,
            tallies,
        })
    }
    fn tip_limits(&self) -> Result<Resources> {
        self.limits_after(self.tip())
    }
    /// Where capacity voting stands at the tip (rules
    /// §10): the epoch the next block belongs to, the
    /// share of the work of its blocks so far that votes each way and what
    /// that would decide, and the capacities of this epoch and the next two,
    /// which the votes of earlier epochs already decided. Counted once per tip.
    fn voting(&self) -> Result<Value> {
        let tip = self.tip();
        if let Some((at, answer)) = &*self.voting.borrow()
            && *at == tip
        {
            return Ok(answer.clone());
        }
        let e = u64::from(self.genesis.vote_epoch);
        let height = self.height() as u64;
        let epoch = height / e;
        // The votes of this epoch so far and of the two before it, along the
        // canonical chain.
        let lowest = epoch.saturating_sub(2) * e + 1;
        let mut tallies: BTreeMap<u64, consensus::Tally> = BTreeMap::new();
        let mut id = tip;
        while id != self.genesis.block_id() {
            let b = self.entry(&id);
            if b.header.height < lowest {
                break;
            }
            tallies
                .entry((b.header.height - 1) / e)
                .or_default()
                .add(&b.header);
            id = b.header.parent;
        }
        let mut limits = self.tip_limits()?;
        let mut capacities = vec![json!({"epoch":epoch,"from":epoch * e + 1,"limits":limits.0})];
        for next in epoch + 1..=epoch + 2 {
            if next >= 3 {
                let source = tallies.get(&(next - 3)).cloned().unwrap_or_default();
                limits = consensus::next_limits_tally(&self.genesis, limits, &source);
            }
            capacities.push(json!({"epoch":next,"from":next * e + 1,"limits":limits.0}));
        }
        let current = tallies.remove(&epoch).unwrap_or_default();
        let percent = |part: &BigUint| -> f64 {
            if current.total.bits() == 0 {
                return 0.0;
            }
            let basis = u64::try_from(&(part * 10_000u32 / &current.total)).unwrap_or(10_000);
            basis as f64 / 100.0
        };
        let outcome = |i: usize| match current.direction(i) {
            1 => "increase",
            2 => "decrease",
            _ => "hold",
        };
        let answer = json!({"height":height,"epoch":epoch,"epoch_blocks":e,"next_epoch_from":(epoch + 1) * e + 1,
            "this_epoch":{"blocks":height - epoch * e,
                "increase_percent":current.up.iter().map(percent).collect::<Vec<_>>(),
                "decrease_percent":current.down.iter().map(percent).collect::<Vec<_>>(),
                "outcome_so_far":(0..4).map(outcome).collect::<Vec<_>>(),
                "takes_effect_from":(epoch + 3) * e + 1},
            "capacities":capacities});
        *self.voting.borrow_mut() = Some((tip, answer.clone()));
        Ok(answer)
    }
    /// The work a header chain needs before it is indexed: within
    /// `THRESHOLD_BLOCKS` of the tip's work.
    fn threshold(&self) -> BigUint {
        let block = self.index.get(&self.tip()).map_or_else(
            || consensus::work(&self.genesis.initial_target),
            |b| consensus::work(&b.header.target),
        );
        let window = block * THRESHOLD_BLOCKS;
        let tip = self.tip_work();
        if tip > window {
            tip - window
        } else {
            BigUint::default()
        }
    }
    fn checked_header(&self, header: &Header) -> Result<(consensus::Ancestry, Resources)> {
        // At a seed height, the seed's block alone, before anything else.
        crate::seed::check(self.seed, header)?;
        let ancestry = self.ancestry(header.parent)?;
        // Every check but the capacity first, the proof of work among them, so
        // that a header nobody worked for costs no count of an epoch's votes.
        consensus::validate_header_at(&self.genesis, &ancestry, header, Resources([u64::MAX; 4]))
            .map_err(|e| format!("{INVALID}header: {e}"))?;
        let limits = self.limits_after(header.parent)?;
        consensus::validate_header_at(&self.genesis, &ancestry, header, limits)
            .map_err(|e| format!("{INVALID}header: {e}"))?;
        Ok((ancestry, limits))
    }
    /// A valid header is admitted only once its timestamp is within
    /// `CLOCK_SLACK` of the clock.
    fn timely(header: &Header) -> Result<()> {
        if header.timestamp > now().saturating_add(CLOCK_SLACK) {
            return Err(AHEAD_OF_CLOCK.into());
        }
        Ok(())
    }
    /// A failure the node cannot continue past: reported, remembered as the
    /// answer to every further request, and the node stops.
    fn fail(&mut self, error: String) -> String {
        eprintln!("chain stopped: {error}");
        self.failed = Some(error.clone());
        self.shutdown.store(true, Ordering::Relaxed);
        error
    }
    /// Keeps `best` the branch of the best header, and the download frontier
    /// no higher than where the branch changed.
    fn sync_best(&mut self) {
        // With nothing held but invalid blocks, the best header is the
        // highest settled block, which `best` keeps.
        let Some(best) = self.index.best_header() else {
            let floor = self.best.floor;
            self.best.truncate(floor);
            self.frontier = self.frontier.min(floor);
            return;
        };
        if self.best.last(&self.index) == Some(best) {
            return;
        }
        let mut path = Vec::new();
        let mut id = best;
        let keep = loop {
            if id == self.genesis.block_id() {
                break 0;
            }
            let height = self.entry(&id).header.height as usize;
            if self.best.get(height - 1, &self.index) == Some(id) {
                break height;
            }
            path.push(id);
            id = self.entry(&id).header.parent;
        };
        // A best branch that forks below the floor holds what it leaves first.
        if keep < self.best.floor
            && let Err(error) = self.unsettle((keep as u64).saturating_sub(1))
        {
            self.fail(error);
            return;
        }
        self.best.truncate(keep);
        path.reverse();
        self.best.extend(path);
        self.frontier = self.frontier.min(keep);
    }
    fn advance_frontier(&mut self) {
        self.frontier = self.frontier.max(self.best.floor);
        while let Some(id) = self.best.get(self.frontier, &self.index) {
            if !self.entry(&id).has_data() {
                break;
            }
            self.frontier += 1;
        }
    }
    /// How many of the highest canonical blocks the index holds in memory at
    /// least: the last three voting epochs, whose votes a header check
    /// counts, and the undo window.
    fn held_heights(&self) -> u64 {
        (3 * u64::from(self.genesis.vote_epoch) + 11).max(self.config.undo_window)
    }
    /// Settles canonical blocks below the held heights, at least `STEP` at a
    /// time and at most `SETTLE_MAX` (wiki: node/storage): their limits and
    /// the floor are written, and they leave memory. Returns whether any did.
    /// Stores the limits of every executed block the index holds, once, for a
    /// database from before every executed block kept them: settled blocks'
    /// are there already.
    fn fill_limits(&mut self) -> Result<()> {
        if self.store.limits_filled()? {
            return Ok(());
        }
        let limits: Vec<(Hash, Resources)> = self
            .index
            .held_entries()
            .filter(|(_, entry)| entry.status == Kept::Valid)
            .map(|(id, entry)| (*id, entry.limits))
            .collect();
        let tx = self.store.begin()?;
        Store::put_limits(&tx, &limits)?;
        Store::set_limits_filled(&tx)?;
        if let Err(error) = self.store.commit_state(tx) {
            return Err(self.fail(format!("database commit failed; restart required: {error}")));
        }
        Ok(())
    }
    fn settle(&mut self) -> Result<bool> {
        if self.recovery.is_some() || self.activating.is_some() {
            return Ok(false);
        }
        let floor = self.index.floor();
        let Some(target) = (self.height() as u64).checked_sub(self.held_heights()) else {
            return Ok(false);
        };
        if target < floor.saturating_add(STEP as u64) {
            return Ok(false);
        }
        let target = target.min(floor + SETTLE_MAX);
        let mut blocks = Vec::new();
        let mut limits = Vec::new();
        for height in floor + 1..=target {
            let i = height as usize - 1;
            let (Some(id), Some(child)) = (
                self.canonical.get(i, &self.index),
                self.canonical.get(i + 1, &self.index),
            ) else {
                break;
            };
            // Only what nothing can change: a held block, executed and valid,
            // on the best branch too, and not marked.
            let entry = self.entry(&id);
            if !self.index.holds(&id)
                || entry.status != Kept::Valid
                || self.marks.contains(&id)
                || self.best.get(i, &self.index) != Some(id)
            {
                break;
            }
            limits.push((id, entry.limits));
            blocks.push((id, child));
        }
        if blocks.is_empty() {
            return Ok(false);
        }
        let floor = floor + blocks.len() as u64;
        let tx = self.store.begin()?;
        Store::settle(&tx, &limits, floor)?;
        if let Err(error) = self.store.commit_state(tx) {
            return Err(self.fail(format!("database commit failed; restart required: {error}")));
        }
        self.index.settle(&blocks, floor);
        self.canonical.settle(floor as usize);
        self.best.settle(floor as usize);
        self.frontier = self.frontier.max(floor as usize);
        self.forget_old_branches()?;
        Ok(true)
    }
    /// Forgets the side branches all of whose blocks lie at or below the
    /// settled height (wiki: node/storage): far behind the tip, they leave the
    /// index and the database, their frames left unreferenced in their files.
    /// Should one ever be extended past the tip, it is fetched again like any
    /// other branch.
    fn forget_old_branches(&mut self) -> Result<()> {
        let floor = self.index.floor();
        let mut old: Vec<(u64, Hash)> = self
            .index
            .held_entries()
            .filter(|(id, e)| {
                let height = e.header.height;
                height <= floor
                    && self.best.get(height as usize - 1, &self.index) != Some(**id)
                    && !self.is_canonical(id)
            })
            .map(|(id, e)| (e.header.height, *id))
            .collect();
        if old.is_empty() {
            return Ok(());
        }
        // Highest first: a block goes only with every block above it.
        old.sort_unstable_by(|a, b| b.cmp(a));
        let mut gone = std::collections::HashSet::new();
        for (_, id) in &old {
            if self.index.children(id).iter().all(|c| gone.contains(c)) {
                gone.insert(*id);
            }
        }
        if gone.is_empty() {
            return Ok(());
        }
        let tx = self.store.begin()?;
        for id in &gone {
            if self.index.get(id).is_some_and(|e| e.stored) {
                self.store.forget_side(&tx, id)?;
            }
        }
        if let Err(error) = self.store.commit_state(tx) {
            return Err(self.fail(format!("database commit failed; restart required: {error}")));
        }
        self.index.forget(&gone);
        self.sources.retain(|id, _| !gone.contains(id));
        self.presync.retain(|_, p| !gone.contains(&p.start));
        Ok(())
    }
    /// Holds a settled block in memory again, with its parent, before the
    /// operator's mark or reconsideration changes it.
    fn hold(&mut self, id: &Hash) -> Result<()> {
        if self.index.holds(id) {
            return Ok(());
        }
        match self.index.get(id).map(|e| e.header.height) {
            Some(height) => self.unsettle(height.saturating_sub(2)),
            None => Ok(()),
        }
    }
    /// Holds the settled blocks above `floor` in memory again (wiki:
    /// node/storage), before anything at or below the current floor changes:
    /// a branch forking there is followed, or a block there is marked or
    /// reconsidered. The lower floor is written first.
    fn unsettle(&mut self, floor: u64) -> Result<()> {
        let old = self.index.floor();
        if floor >= old {
            return Ok(());
        }
        let mut blocks = Vec::new();
        let mut ids = Vec::new();
        {
            let view = self.store.view()?;
            for height in floor + 1..=old {
                let id = view
                    .canonical(height)?
                    .ok_or("missing settled canonical block")?;
                let child = view
                    .canonical(height + 1)?
                    .ok_or("missing canonical block above the floor")?;
                let stored = view.stored(&id)?.ok_or("missing settled header")?;
                let limits = view
                    .limits(&id)?
                    .ok_or("missing limits of a settled block")?;
                blocks.push((stored, limits, child));
                ids.push(id);
            }
        }
        let tx = self.store.begin()?;
        Store::settle(&tx, &[], floor)?;
        if let Err(error) = self.store.commit_state(tx) {
            return Err(self.fail(format!("database commit failed; restart required: {error}")));
        }
        self.index.unsettle(blocks, floor);
        self.canonical.unsettle(floor as usize, &ids);
        self.best.unsettle(floor as usize, &ids);
        Ok(())
    }
    /// Checks a header against its indexed parent and indexes it. A header
    /// whose chain stays below the work threshold is refused here: such a
    /// chain is checked by `headers_from` without being kept.
    fn accept_header(&mut self, header: &Header, recovered: bool) -> Result<()> {
        let id = header.id();
        if self.index.contains(&id) {
            return Ok(());
        }
        if self.marks.contains(&id) {
            return Err(MARKED.into());
        }
        let parent_work = if header.parent == self.genesis.block_id() {
            BigUint::default()
        } else {
            let parent = self.index.get(&header.parent).ok_or("unknown parent")?;
            let work = parent.work.clone();
            if parent.status == Kept::Invalid {
                if self.by_operator(&header.parent) {
                    return Err(MARKED.into());
                }
                // A valid header on a failed branch: its work counts toward
                // the warning that the node may follow other rules.
                self.checked_header(header)?;
                self.failed_branch(work + consensus::work(&header.target));
                return Err(format!("{INVALID}header: its parent is invalid"));
            }
            work
        };
        let (_, limits) = self.checked_header(header)?;
        if !recovered {
            Self::timely(header)?;
        }
        let work = parent_work + consensus::work(&header.target);
        if !recovered && work < self.threshold() {
            return Err(BEHIND.into());
        }
        self.index_header(header.clone(), work, limits);
        self.sync_best();
        Ok(())
    }
    pub fn import(&mut self, bytes: &[u8], requeue: bool) -> Result<Value> {
        self.import_from(bytes, requeue, None)
    }
    /// `import` of a block a peer sent, remembered as its source until the
    /// block is connected or fails.
    pub fn import_from(
        &mut self,
        bytes: &[u8],
        requeue: bool,
        peer: Option<&str>,
    ) -> Result<Value> {
        let result = self.accept(bytes, None).and_then(|(id, known)| {
            if let Some(peer) = peer
                && !known
            {
                if self.sources.len() >= 65_536 {
                    self.sources.clear();
                }
                self.sources.insert(id, peer.to_string());
            }
            self.activate(requeue).map(|()| (id, known))
        });
        if result.is_err() && self.store.has_unindexed() {
            self.fail("unindexed durable block tail; restart required".into());
        }
        let (id, known) = result?;
        Ok(
            json!({"id":hex(&id),"known":known,"canonical":self.is_canonical(&id),"height":self.height()}),
        )
    }
    /// Stores a block whose header is, or now becomes, indexed and whose body
    /// matches it; nothing is executed until its branch is connected
    /// (`activate`). Returns its id and whether it was stored already.
    fn accept(&mut self, bytes: &[u8], recovered: Option<Location>) -> Result<(Hash, bool)> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        // The fixed-size header first: the active data capacity at this height
        // bounds the body, and is derived from the header's ancestry.
        let header = Header::decode(
            bytes
                .get(..HEADER_LEN)
                .ok_or(format!("{INVALID}block: truncated header"))?,
        )
        .map_err(|e| format!("{INVALID}block: {e}"))?;
        let id = header.id();
        match self.index.get(&id) {
            Some(entry) if entry.status == Kept::Invalid => {
                if self.by_operator(&id) {
                    return Err(MARKED.into());
                }
                return Err(format!("{INVALID}block: it failed validation"));
            }
            Some(entry) if entry.has_data() => {
                if self.store.block(&id)?.encode() != bytes {
                    return Err(format!("{INVALID}block: a stored header with another body"));
                }
                if let Some(location) = recovered {
                    self.store.file_progress(location)?;
                }
                return Ok((id, true));
            }
            Some(_) => {
                // A block durable on disk was admitted once; recovery keeps it
                // whatever the clock says now.
                if recovered.is_none() {
                    Self::timely(&header)?;
                }
            }
            None => self.accept_header(&header, recovered.is_some())?,
        }
        let entry = self.entry(&id);
        let (limits, work) = (entry.limits, entry.work.clone());
        if bytes.len() as u64 > limits.0[2] {
            return Err(format!("{INVALID}block: block byte limit"));
        }
        let block =
            Block::decode(bytes, bytes.len()).map_err(|e| format!("{INVALID}block: {e}"))?;
        // The body's own commitments, before it is stored.
        let (transactions_root, receipts_root) =
            consensus::commitments(&block.transactions, &block.receipts);
        if block.transactions.len() != block.receipts.len()
            || transactions_root != header.transactions_root
            || receipts_root != header.receipts_root
            || ting_core::protocol::producer_hash(&block.producer) != header.producer_hash
        {
            return Err(format!("{INVALID}block: body commitment mismatch"));
        }
        let location = match recovered {
            Some(location) => location,
            None => self.store.append(bytes)?,
        };
        let tx = self.store.begin()?;
        Store::save_data(&tx, &header, &work, location)?;
        // A failed commit may have reached disk. Stop instead of continuing with stale RAM.
        if let Err(error) = self.store.commit_block(tx, location) {
            return Err(self.fail(format!("database commit failed; restart required: {error}")));
        }
        // Connecting it next reads it from memory.
        self.store.keep_recent(id, bytes);
        self.index.set_data(&id);
        self.in_flight.remove(&id);
        self.advance_frontier();
        Ok((id, false))
    }
    /// Connects the best branch that can be connected (wiki: node/storage):
    /// while a branch whose blocks are all stored has more work than the tip
    /// by the fork-choice margin, the tip moves toward it, a step at a time,
    /// each in one database transaction: disconnecting blocks, connecting
    /// blocks by their stored changes, or executing one block.
    fn activate(&mut self, requeue: bool) -> Result<()> {
        let result = self.activate_steps(requeue);
        if self.activating.is_none() {
            self.reorganized();
            if self.recovery.is_none() {
                self.moving_from = None;
            }
        }
        result?;
        // Once the tip has moved on, what fell below the held heights settles.
        self.settle()?;
        Ok(())
    }
    fn activate_steps(&mut self, requeue: bool) -> Result<()> {
        let deadline = self.step_time.map(|t| std::time::Instant::now() + t);
        self.activating = None;
        // The tip the move began from, kept across paused steps: the branch
        // the margin defends. One the operator has since marked invalid
        // defends nothing.
        let (start, start_work) = match &self.moving_from {
            Some((start, work))
                if self
                    .index
                    .get(start)
                    .is_none_or(|e| e.status != Kept::Invalid) =>
            {
                (*start, work.clone())
            }
            _ => (self.tip(), self.tip_work()),
        };
        self.moving_from = Some((start, start_work.clone()));
        let mut first = true;
        while !self.shutdown.load(Ordering::Relaxed) {
            // Out of time with steps left, and at least one taken: the chain
            // thread answers requests and comes back (`resume_activation`).
            if !first && deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                self.activating = Some(requeue);
                break;
            }
            first = false;
            let Some(best) = self.choose(start, &start_work) else {
                break;
            };
            if best == self.tip() {
                break;
            }
            let (common, branch) = self.fork(best)?;
            // The blocks a branch forking at or below the floor leaves, and
            // the one it forks from, are held first.
            if self.canonical.len() > common && common as u64 <= self.index.floor() {
                self.unsettle((common as u64).saturating_sub(1))?;
            }
            if self.canonical.len() > common {
                // Below the window the undo data must be rebuilt first: the
                // replay runs, and the steps go on when it is done.
                if (common as u64) < self.pruned {
                    self.recover_undo(common as u64)?;
                    break;
                }
                let n = (self.canonical.len() - common).min(STEP);
                self.disconnect(n, requeue)?;
            } else {
                self.connect(&branch, requeue)?;
            }
        }
        Ok(())
    }
    /// The branch to move toward (wiki: node/storage): of the heads of the
    /// branches that could be connected, most work first, the first that is
    /// the tip the move began from, grows it with more work, or clears the
    /// fork-choice margin against it. A move that fails part way therefore
    /// returns to where it began rather than stopping on a lighter branch,
    /// and a heavier branch that does not clear the margin hides no growth of
    /// the tip.
    fn choose(&self, start: Hash, start_work: &BigUint) -> Option<Hash> {
        self.index.heads().copied().find(|head| {
            let entry = self.entry(head);
            *head == start
                || overtakes(&entry.work, &entry.header, start_work)
                || (entry.work > *start_work && self.descends_from(*head, start))
        })
    }
    /// Whether `ancestor` is `id` or one of its ancestors.
    fn descends_from(&self, mut id: Hash, ancestor: Hash) -> bool {
        if ancestor == self.genesis.block_id() {
            return true;
        }
        let Some(floor) = self.index.get(&ancestor).map(|e| e.header.height) else {
            return false;
        };
        loop {
            if id == ancestor {
                return true;
            }
            match self.index.get(&id) {
                Some(entry) if entry.header.height > floor => id = entry.header.parent,
                _ => return false,
            }
        }
    }
    /// Limits how long one call works toward the best branch; the rest of the
    /// steps run between requests.
    pub fn bound_steps(&mut self, time: Duration) {
        self.step_time = Some(time);
    }
    /// Whether steps toward the best branch are left.
    pub fn activating(&self) -> bool {
        self.activating.is_some() || self.recovery.as_ref().is_some_and(|r| r.thread.is_none())
    }
    /// Takes the next steps toward the best branch, for at most the step time.
    pub fn resume_activation(&mut self) {
        if let Err(error) = self.poll_recovery() {
            eprintln!("rebuilding undo data: {error}");
        }
        if let Some(requeue) = self.activating
            && let Err(error) = self.activate(requeue)
        {
            eprintln!("the best stored branch is not connected: {error}");
            self.activating = None;
        }
    }
    /// Reports a reorganization once the tip has settled (wiki: node/storage).
    fn reorganized(&mut self) {
        let Some((tip, from, lowest)) = self.unwinding.take() else {
            return;
        };
        // Back where it started, as when the other branch failed: nothing
        // was replaced.
        if tip == self.tip() {
            return;
        }
        let replaced = from - lowest;
        let deep = replaced >= DEEP_REORG;
        eprintln!(
            "{}reorganization: {replaced} blocks above height {lowest} replaced; tip {} at height {}",
            if deep { "warning: deep " } else { "" },
            hex(&self.tip()),
            self.height()
        );
        self.reorganization = Some(json!({"replaced":replaced,"fork_height":lowest,
            "height":self.height(),"tip":hex(&self.tip()),"time":now(),"deep":deep}));
    }
    /// Disconnects connected blocks above the lowest invalid one, as when the
    /// operator marks a connected block invalid.
    fn leave_invalid(&mut self, requeue: bool) -> Result<()> {
        if self.marks.is_empty() {
            return Ok(());
        }
        // A marked block is held, and so is every block above it.
        let Some(lowest) = self
            .canonical
            .held()
            .iter()
            .position(|id| self.entry(id).status == Kept::Invalid)
            .map(|i| i + self.canonical.floor)
        else {
            return Ok(());
        };
        if (lowest as u64) < self.pruned {
            return self.recover_undo(lowest as u64);
        }
        while self.canonical.len() > lowest {
            let n = (self.canonical.len() - lowest).min(STEP);
            self.disconnect(n, requeue)?;
        }
        Ok(())
    }
    fn disconnect(&mut self, n: usize, requeue: bool) -> Result<()> {
        let keep = self.canonical.len() - n;
        // The lowest height reached, not the last: a reorganization may
        // connect blocks above it and then disconnect some of them again.
        let (tip, from, lowest) =
            self.unwinding
                .unwrap_or((self.tip(), self.canonical.len(), keep));
        self.unwinding = Some((tip, from, lowest.min(keep)));
        let tx = Rc::new(self.store.begin()?);
        for id in self.canonical.held()[keep - self.canonical.floor..]
            .iter()
            .rev()
        {
            let header = &self.entry(id).header;
            let (parent, height) = (header.parent, header.height);
            Store::undo_block(tx.clone(), *id, height, self.root_at(parent)?)?;
        }
        let tip = if keep == 0 {
            self.genesis.block_id()
        } else {
            self.canonical
                .get(keep - 1, &self.index)
                .ok_or("missing canonical block")?
        };
        Store::canonical(&tx, keep, &[], tip)?;
        let tx = Rc::try_unwrap(tx).map_err(|_| "state transaction still borrowed")?;
        if let Err(error) = self.store.commit_state(tx) {
            return Err(self.fail(format!("database commit failed; restart required: {error}")));
        }
        let gone = self.canonical.split_off(keep);
        self.published(&[], &gone, requeue)
    }
    fn connect(&mut self, branch: &[Hash], requeue: bool) -> Result<()> {
        let common = self.canonical.len();
        let tx = Rc::new(self.store.begin()?);
        let mut connected = Vec::new();
        for id in branch.iter().take(STEP) {
            let entry = self.entry(id);
            if entry.status == Kept::Valid
                && Store::redo_block(tx.clone(), *id, entry.header.state_root)?
            {
                connected.push(*id);
                continue;
            }
            // A block to execute is a step of its own.
            if !connected.is_empty() {
                break;
            }
            let block = match self.store.block(id) {
                Ok(block) => block,
                Err(error) => {
                    drop(tx);
                    return self.forget_body(*id, &error);
                }
            };
            let (limits, work, parent) = (entry.limits, entry.work.clone(), entry.header.parent);
            let ancestry = self.ancestry(parent)?;
            // Nothing is written in this transaction before a block executes,
            // so the block runs against the last commit, through a view whose
            // tables open once, and its changes are written after.
            let validator = Runtime::with_cache(
                self.store.snapshot()?,
                Context::from_genesis(&self.genesis),
                self.code.clone(),
            );
            let result =
                validator.validate_and_apply_block_at(&self.genesis, &ancestry, &block, limits);
            if let Some(error) = validator.storage_error() {
                return Err(error);
            }
            if let Err(error) = result {
                drop(validator);
                drop(tx);
                // A seed block is canonical: when this node's execution
                // rejects it, the fault is the node's, not the peer's.
                if block.header.height <= self.seed.len() as u64 {
                    let report = self.nonconformance(&block, &ancestry, limits, &error.to_string());
                    return Err(self.fail(report));
                }
                return self.invalidate(*id, &error.to_string());
            }
            let changes = validator.state_changes()?;
            drop(validator);
            Store::apply(&tx, &changes)?;
            self.store
                .save_valid(&tx, &block, &work, &changes, limits)?;
            connected.push(*id);
            break;
        }
        let tip = *connected.last().ok_or("nothing to connect")?;
        Store::canonical(&tx, common, &connected, tip)?;
        let pruned = self.prune(&tx, common, &connected)?;
        let tx = Rc::try_unwrap(tx).map_err(|_| "state transaction still borrowed")?;
        if let Err(error) = self.store.commit_state(tx) {
            return Err(self.fail(format!("database commit failed; restart required: {error}")));
        }
        self.pruned = pruned;
        for id in &connected {
            self.index.set_valid(id);
            self.sources.remove(id);
        }
        self.canonical.extend(connected.iter().copied());
        let blocks = connected
            .iter()
            .map(|id| self.store.block(id))
            .collect::<Result<Vec<_>>>()?;
        self.published(&blocks, &[], requeue)
    }
    /// What to report when this node's execution rejects a seed block: the
    /// block, the reason, and the first transaction whose receipt differs
    /// from the seed's when the transactions run again one by one.
    fn nonconformance(
        &self,
        block: &Block,
        ancestry: &consensus::Ancestry,
        limits: Resources,
        error: &str,
    ) -> String {
        let chain = self.genesis.chain_id();
        let mut detail = "every receipt agrees, so the state root or the totals differ".to_string();
        let runtime = self.store.snapshot().map(|snapshot| {
            Runtime::with_cache(
                snapshot,
                Context::from_genesis(&self.genesis),
                self.code.clone(),
            )
        });
        if let Ok(runtime) = &runtime
            && let Ok(mut candidate) = runtime.candidate(
                &self.genesis,
                ancestry,
                limits,
                block.header.timestamp,
                block.header.votes,
                block.producer.clone(),
            )
        {
            for (i, (tx, seed)) in block.transactions.iter().zip(&block.receipts).enumerate() {
                let differs = match candidate.add(tx) {
                    Ok(Some(receipt)) if receipt == seed => continue,
                    Ok(Some(receipt)) => format!(
                        "status {:?} using {:?} where the seed has {:?} using {:?}",
                        receipt.status, receipt.used.0, seed.status, seed.used.0
                    ),
                    Ok(None) => "it does not fit the block".to_string(),
                    Err(e) => e.to_string(),
                };
                detail = format!(
                    "transaction {i} of the block, {}, differs: {differs}",
                    hex(&tx.id(chain))
                );
                break;
            }
        }
        format!(
            "this node does not conform: its own execution rejects seed block {} ({error}); {detail}. \
             The seed's blocks are canonical, so the fault is this node's, not its peers'",
            block.header.height
        )
    }
    /// Deletes, in the connecting transaction, the undo data of canonical
    /// blocks that the window leaves behind, a step's worth at a time, and the
    /// undo and redo data of disconnected blocks as old. Returns how far undo
    /// data is pruned then. Nothing is pruned while a replay restores it.
    fn prune(&self, tx: &redb::WriteTransaction, common: usize, connected: &[Hash]) -> Result<u64> {
        let window = self.config.undo_window;
        let height = (common + connected.len()) as u64;
        if window == 0 || self.recovery.is_some() || height <= window {
            return Ok(self.pruned);
        }
        let floor = height - window;
        let to = floor.min(self.pruned + STEP as u64);
        let id_at = |h: u64| -> Result<Hash> {
            let h = h as usize;
            if h <= common {
                self.canonical
                    .get(h - 1, &self.index)
                    .ok_or_else(|| "missing canonical block".into())
            } else {
                Ok(connected[h - common - 1])
            }
        };
        let ids: Vec<Hash> = (self.pruned + 1..=to).map(id_at).collect::<Result<_>>()?;
        Store::prune_undo(tx, &ids, to)?;
        Store::prune_redo(tx, floor)?;
        Ok(to)
    }
    /// A stored block whose body cannot be read is forgotten, its header kept,
    /// so that it is fetched again; the best branch that is left is connected.
    fn forget_body(&mut self, id: Hash, error: &str) -> Result<()> {
        eprintln!(
            "block {} cannot be read and is fetched again: {error}",
            hex(&id)
        );
        let tx = self.store.begin()?;
        self.store.forget_block(&tx, &id)?;
        if let Err(error) = self.store.commit_state(tx) {
            return Err(self.fail(format!("database commit failed; restart required: {error}")));
        }
        self.index.forget_data(&id);
        self.sync_best();
        self.frontier = 0;
        Ok(())
    }
    /// Starts rebuilding the undo data of canonical blocks above `down_to`, up
    /// to the pruned height, by replaying the chain from genesis in another
    /// thread (wiki: node/storage#deep-reorganizations).
    fn recover_undo(&mut self, down_to: u64) -> Result<()> {
        if self.recovery.is_some() {
            return Ok(());
        }
        let upto = self.pruned;
        let mut blocks = Vec::with_capacity(upto as usize);
        for i in 0..upto as usize {
            let id = self
                .canonical
                .get(i, &self.index)
                .ok_or("missing canonical block")?;
            blocks.push((id, self.store.location(&id)?, self.entry(&id).limits));
        }
        let dir = self.directory.join("replay");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        eprintln!(
            "a branch forks at height {down_to}, below the undo window: replaying {upto} blocks from genesis to rebuild their undo data"
        );
        let (data, replay_dir, genesis, shutdown, cache) = (
            self.directory.clone(),
            dir.clone(),
            self.genesis.clone(),
            self.shutdown.clone(),
            self.config.cache_bytes(),
        );
        let thread = std::thread::spawn(move || {
            replay(
                &data,
                &replay_dir,
                &genesis,
                &blocks,
                down_to,
                &shutdown,
                cache,
            )
        });
        self.recovery = Some(Recovery {
            down_to,
            upto,
            dir,
            thread: Some(thread),
            replayed: None,
            copied: down_to + 1,
        });
        Ok(())
    }
    /// Moves a finished replay's undo data into the database, a step at a
    /// time, then lets the reorganization go on.
    fn poll_recovery(&mut self) -> Result<()> {
        let Some(recovery) = self.recovery.as_mut() else {
            return Ok(());
        };
        if let Some(thread) = &recovery.thread {
            if !thread.is_finished() {
                return Ok(());
            }
            let result = recovery
                .thread
                .take()
                .expect("checked above")
                .join()
                .unwrap_or_else(|_| Err("the replay thread panicked".into()));
            let opened = result.and_then(|()| {
                Store::open(&recovery.dir, &self.genesis, self.config.cache_bytes())
            });
            match opened {
                Ok(store) => recovery.replayed = Some(store),
                Err(error) => {
                    eprintln!("rebuilding undo data failed: {error}");
                    let dir = recovery.dir.clone();
                    self.recovery = None;
                    let _ = std::fs::remove_dir_all(dir);
                    return Ok(());
                }
            }
        }
        let recovery = self.recovery.as_mut().expect("present");
        let replayed = recovery.replayed.as_ref().expect("opened above");
        let end = (recovery.copied + STEP as u64 - 1).min(recovery.upto);
        let tx = self.store.begin()?;
        for height in recovery.copied..=end {
            let id = self
                .canonical
                .get(height as usize - 1, &self.index)
                .ok_or("missing canonical block")?;
            Store::put_undo(&tx, id, &replayed.undo_of(id)?)?;
        }
        let done = end == recovery.upto;
        if done {
            Store::set_pruned(&tx, recovery.down_to)?;
        }
        if let Err(error) = self.store.commit_state(tx) {
            return Err(self.fail(format!("database commit failed; restart required: {error}")));
        }
        recovery.copied = end + 1;
        if done {
            let (down_to, dir) = (recovery.down_to, recovery.dir.clone());
            self.recovery = None;
            let _ = std::fs::remove_dir_all(dir);
            self.pruned = down_to;
            eprintln!("undo data rebuilt above height {down_to}; the reorganization goes on");
            self.leave_invalid(true)?;
            self.activating = Some(true);
        }
        Ok(())
    }
    /// A stored block failed: it is stored as invalid, and it and everything
    /// above it are never connected; at a restart, what is above it follows
    /// from it.
    fn invalidate(&mut self, id: Hash, reason: &str) -> Result<()> {
        eprintln!("block {} is invalid: {reason}", hex(&id));
        let invalid = self.index.set_invalid(&id);
        let tx = self.store.begin()?;
        let entry = self.entry(&id);
        Store::save_invalid(&tx, &entry.header, &entry.work)?;
        if let Err(error) = self.store.commit_state(tx) {
            return Err(self.fail(format!("database commit failed; restart required: {error}")));
        }
        let mut most = BigUint::default();
        for id in &invalid {
            if let Some(peer) = self.sources.remove(id) {
                self.offenders.push(peer);
            }
            most = most.max(self.entry(id).work.clone());
        }
        self.failed_branch(most);
        self.sync_best();
        Ok(())
    }
    /// The block a request names: a canonical height, or an id, seen or not.
    fn named(&self, text: &str) -> Result<Hash> {
        if text.len() != 64 && !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
            return self.block_id(text);
        }
        hash_arg(text)
    }
    /// The operator marks a block invalid (wiki: node/storage): the node
    /// leaves it and every block above it, never connects them, and refuses
    /// them from peers without holding them against the peers. A block not yet
    /// seen is refused when it comes. The mark is kept across restarts; what
    /// is stored of the block is not changed.
    pub fn mark_invalid(&mut self, id: Hash) -> Result<Value> {
        if id == self.genesis.block_id() {
            return Err("the genesis block cannot be marked invalid".into());
        }
        let tx = self.store.begin()?;
        Store::mark(&tx, &id, true)?;
        tx.commit().map_err(|e| e.to_string())?;
        self.marks.insert(id);
        let known = self.index.contains(&id);
        self.hold(&id)?;
        self.index.set_invalid(&id);
        self.sync_best();
        eprintln!("block {} marked invalid by the operator", hex(&id));
        self.leave_invalid(true)?;
        self.activate(true)?;
        Ok(json!({"id":hex(&id),"known":known,"tip":hex(&self.tip()),"height":self.height()}))
    }
    /// The operator reconsiders a block (wiki: node/storage): it, every block
    /// above it and every block below it are no longer invalid, whether marked
    /// or failed, and the best branch is connected; a block that fails again
    /// is invalid again.
    pub fn reconsider(&mut self, id: Hash) -> Result<Value> {
        let known = self.index.contains(&id);
        if !known && !self.marks.contains(&id) {
            return Err("block not found".into());
        }
        self.hold(&id)?;
        let mut cleared = self.index.reconsider(&id);
        let unmarked: BTreeSet<Hash> = cleared
            .iter()
            .chain([&id])
            .filter(|i| self.marks.contains(*i))
            .copied()
            .collect();
        // A mark that goes clears every branch above it, not only the one
        // through the reconsidered block: they were invalid for it alone.
        for mark in &unmarked {
            cleared.extend(self.index.reconsider(mark));
        }
        let cleared: Vec<Hash> = {
            let mut seen = BTreeSet::new();
            cleared.into_iter().filter(|id| seen.insert(*id)).collect()
        };
        let tx = self.store.begin()?;
        for mark in &unmarked {
            Store::mark(&tx, mark, false)?;
        }
        for cleared in &cleared {
            let entry = self.entry(cleared);
            if entry.stored {
                let kept = if entry.status == Kept::Valid {
                    store::Kept::Valid
                } else {
                    store::Kept::Data
                };
                Store::save_kept(&tx, &entry.header, &entry.work, kept)?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        for mark in &unmarked {
            self.marks.remove(mark);
        }
        self.failed_work = BigUint::default();
        self.warned = false;
        self.sync_best();
        eprintln!(
            "block {} reconsidered by the operator: {} blocks no longer invalid",
            hex(&id),
            cleared.len()
        );
        self.activate(true)?;
        Ok(
            json!({"id":hex(&id),"cleared":cleared.len(),"tip":hex(&self.tip()),"height":self.height()}),
        )
    }
    /// After the tip moved: a fresh view of state, no templates, and the pool
    /// and the deferred set following the blocks connected and disconnected.
    /// What newly connected blocks include leaves the pool, and the deferred set
    /// when included above the height it was deferred at; a transaction mined
    /// earlier may be pooled and mined again (wiki: node/mining).
    fn published(
        &mut self,
        connected: &[Block],
        disconnected: &[Hash],
        requeue: bool,
    ) -> Result<()> {
        self.templates.clear();
        let result = (|| -> Result<()> {
            self.runtime = Runtime::with_cache(
                self.store.snapshot()?,
                Context::from_genesis(&self.genesis),
                self.code.clone(),
            );
            let chain_id = self.genesis.chain_id();
            let mut included: BTreeMap<Hash, u64> = BTreeMap::new();
            // A deferred entry leaves only for a successful inclusion: one that
            // failed, such as one before its time, does not cancel it.
            let mut succeeded: BTreeMap<Hash, (u64, Hash)> = BTreeMap::new();
            for b in connected {
                for (tx, receipt) in b.transactions.iter().zip(&b.receipts) {
                    let id = tx.id(chain_id);
                    let at = included.entry(id).or_default();
                    *at = (*at).max(b.header.height);
                    if receipt.status == Status::Success {
                        succeeded.insert(id, (b.header.height, b.header.id()));
                    }
                }
            }
            self.pool.retain(|p| !included.contains_key(&p.id));
            let leaving: Vec<Hash> = self
                .deferred
                .iter()
                .filter(|(id, d)| succeeded.get(*id).is_some_and(|(at, _)| *at > d.since))
                .map(|(id, _)| *id)
                .collect();
            let mut changed = !leaving.is_empty();
            for id in leaving {
                let entry = self.deferred.remove(&id).expect("listed above");
                let (height, block) = succeeded[&id];
                let taken = self.taken.entry(block).or_insert((height, Vec::new()));
                taken.1.push((id, entry));
            }
            // An entry a disconnected block had taken comes back, with its
            // time, instead of entering the pool.
            let current = now();
            let mut restored = BTreeSet::new();
            for old in disconnected {
                for (id, entry) in self.taken.remove(old).map(|(_, e)| e).unwrap_or_default() {
                    if entry.expires >= current {
                        self.deferred.insert(id, entry);
                        restored.insert(id);
                        changed = true;
                    }
                }
            }
            let floor = (self.height() as u64).saturating_sub(RELAY_RECENT);
            self.taken.retain(|_, (height, _)| *height > floor);
            if changed {
                self.deferred_changed();
            }
            if requeue {
                for old in disconnected {
                    for tx in self.store.block(old)?.transactions {
                        let id = tx.id(chain_id);
                        if !included.contains_key(&id) && !restored.contains(&id) {
                            let _ = self.submit(&tx.encode());
                        }
                    }
                }
            }
            for (id, entry) in self.deferred.iter_mut() {
                entry.admitted = self.pool.contains(id);
            }
            Ok(())
        })();
        result.map_err(|error| self.fail(error))
    }
    /// Headers a peer sent, in order (wiki: node/sync). Each is indexed when
    /// its chain comes within the work threshold of the tip; below it, the
    /// chain is checked by a presync for that peer, fetched again once it
    /// reaches the threshold, and indexed then. Returns where the peer's next
    /// answer should start, and whether the chain is being fetched again.
    fn headers_from(&mut self, peer: &str, headers: &[Header]) -> Result<Value> {
        let mut indexed = 0usize;
        let mut next: Option<Hash> = None;
        for header in headers {
            let id = header.id();
            if let Some(mut state) = self.presync.remove(peer)
                && state.follows(header)
            {
                // Held to the clock as an indexed header is: the chain is
                // taken up to it, and from it once the clock catches up.
                if Self::timely(header).is_err() {
                    self.presync.insert(peer.into(), state);
                    break;
                }
                match state.add(&self.genesis, header)? {
                    Outcome::Continue => {
                        next = Some(state.next());
                        self.presync.insert(peer.into(), state);
                    }
                    Outcome::Refetch => {
                        let start = state.start;
                        self.presync.insert(peer.into(), state);
                        return Ok(json!({"next":hex(&start),"refetch":true,"indexed":indexed}));
                    }
                    Outcome::Index(held, done) => {
                        for (h, work, limits) in held {
                            if !self.index.contains(&h.id()) {
                                self.index_header(h, work, limits);
                                indexed += 1;
                            }
                        }
                        self.sync_best();
                        next = Some(state.next());
                        if !done {
                            self.presync.insert(peer.into(), state);
                        }
                    }
                }
                continue;
            }
            // Otherwise the peer moved on to another chain, and any presync ended.
            if self.index.contains(&id) {
                next = Some(id);
                continue;
            }
            match self.accept_header(header, false) {
                Ok(()) => {
                    indexed += 1;
                    next = Some(id);
                }
                Err(error) if error == BEHIND => {
                    let start = header.parent;
                    let start_time = if start == self.genesis.block_id() {
                        self.genesis.timestamp
                    } else {
                        self.index
                            .get(&start)
                            .ok_or("unknown parent")?
                            .header
                            .timestamp
                    };
                    let mut state = Presync::new(
                        start,
                        self.walk_from(start)?,
                        self.threshold(),
                        start_time,
                        now(),
                    );
                    let outcome = state.add(&self.genesis, header)?;
                    next = Some(state.next());
                    let refetch = matches!(outcome, Outcome::Refetch);
                    self.presync.insert(peer.into(), state);
                    if refetch {
                        return Ok(json!({"next":hex(&start),"refetch":true,"indexed":indexed}));
                    }
                }
                Err(error) if is_policy(&error) => break,
                Err(error) => return Err(error),
            }
        }
        Ok(json!({"next":next.map(|id| hex(&id)),"refetch":false,"indexed":indexed}))
    }
    /// Blocks for a peer to fetch, along the best header's branch (wiki:
    /// node/sync): from the lowest one not yet stored, within `WINDOW` of it,
    /// at most `PER_PEER` asked of this peer at once, none of two peers at once
    /// until its transfer time, by its data capacity, passes.
    fn blocks_wanted(&mut self, peer: &str) -> Vec<Hash> {
        let current = now();
        let mut lapsed = Vec::new();
        let index = &self.index;
        self.in_flight.retain(|id, (holder, since)| {
            let capacity = index.get(id).map_or(0, |e| e.limits.0[2]);
            let keep = since.saturating_add(transfer_time(capacity).as_secs()) > current;
            if !keep {
                lapsed.push((*id, std::mem::take(holder)));
            }
            keep
        });
        for (id, holder) in lapsed {
            self.expired.insert(id, (holder, current));
        }
        self.expired
            .retain(|_, (_, at)| at.saturating_add(LAPSED_WAIT) > current);
        self.advance_frontier();
        let mut wanted = Vec::new();
        let mut asked = 0;
        let along: Vec<Hash> = (self.frontier..self.best.len())
            .take(WINDOW)
            .filter_map(|i| self.best.get(i, &self.index))
            .collect();
        for id in &along {
            if asked >= PER_PEER {
                break;
            }
            if self.entry(id).has_data() {
                continue;
            }
            if self
                .expired
                .get(id)
                .is_some_and(|(holder, _)| holder == peer)
            {
                continue;
            }
            match self.in_flight.get(id) {
                Some((holder, _)) if holder != peer => continue,
                Some(_) => {}
                None => {
                    self.in_flight.insert(*id, (peer.to_string(), current));
                }
            }
            wanted.push(*id);
            asked += 1;
        }
        wanted
    }
    /// Pool admission (local policy, wiki: node/mining): a canonical envelope
    /// within the tip's data capacity and room in the pool. Nothing is
    /// executed here; a template that finds the transaction does not fit even
    /// a block holding nothing else drops it from the pool.
    pub fn submit(&mut self, bytes: &[u8]) -> Result<Value> {
        self.submit_from(bytes, None)
    }
    /// `submit` of a transaction relayed from `source`, which counts against
    /// that address (wiki: node/mining).
    pub fn submit_from(&mut self, bytes: &[u8], source: Option<std::net::IpAddr>) -> Result<Value> {
        let tx = self.decode_envelope(bytes)?;
        let id = tx.id(self.genesis.chain_id());
        if let Some(entry) = self.deferred.get(&id) {
            return Ok(
                json!({"id":hex(&id),"known":true,"deferred":true,"not_before":entry.not_before,"expires":entry.expires}),
            );
        }
        if self.pool.contains(&id) {
            return Ok(json!({"id":hex(&id),"known":true}));
        }
        self.admit(tx, id, bytes.len(), source)?;
        Ok(json!({"id":hex(&id),"known":false}))
    }
    /// Refuses a relayed transaction that a recent canonical block included.
    fn recent(&mut self, bytes: &[u8]) -> Result<()> {
        let id = self.decode_envelope(bytes)?.id(self.genesis.chain_id());
        if self.pool.contains(&id) {
            return Ok(());
        }
        let height = self.height() as u64;
        if self
            .store
            .included_above(id, height.saturating_sub(RELAY_ECHO))?
            || self
                .store
                .succeeded_above(id, height.saturating_sub(RELAY_RECENT))?
        {
            return Err(INCLUDED.into());
        }
        Ok(())
    }
    fn decode_envelope(&mut self, bytes: &[u8]) -> Result<Transaction> {
        let limit = usize::try_from(self.tip_limits()?.0[2]).unwrap_or(usize::MAX);
        Transaction::decode(bytes, limit).map_err(|e| e.to_string())
    }
    fn admit(
        &mut self,
        tx: Transaction,
        id: Hash,
        bytes: usize,
        source: Option<std::net::IpAddr>,
    ) -> Result<()> {
        // Any declared budget is taken, as consensus takes it: only what a
        // transaction uses has to fit a block (rules §14), and
        // a template keeps a transaction only when its use fits.
        let pooled = Pooled {
            tx,
            id,
            bytes,
            arrived: now(),
            source,
            tried: 0,
        };
        if !self.make_room(&pooled)? {
            return Err(POOL_FULL.into());
        }
        self.pool.push(pooled);
        Ok(())
    }
    /// Room in the pool for `incoming`: room that is free, or in a full pool
    /// the room the selection policy makes, a transaction at a time
    /// (`mining::Selector::give_way`, wiki: node/mining). Whether there is room.
    fn make_room(&mut self, incoming: &Pooled) -> Result<bool> {
        let (count, most) = (self.config.mempool_transactions, self.held_bytes());
        if incoming.bytes > most {
            return Ok(false);
        }
        if self.pool.fits(incoming.bytes, count, most) {
            return Ok(true);
        }
        let ancestry = self.ancestry(self.tip())?;
        let limits = self.tip_limits()?;
        let timestamp = self.next_timestamp(&ancestry)?;
        let room = {
            let mut candidate = self
                .runtime
                .candidate(
                    &self.genesis,
                    &ancestry,
                    limits,
                    timestamp,
                    self.config.votes,
                    self.config.producer(),
                )
                .map_err(|e| e.to_string())?;
            candidate.limit_discarded(limits.0[0]);
            loop {
                let pending = self.pool.pending();
                let gives = self
                    .selector
                    .give_way(&pending, &incoming.pending(), &mut candidate)
                    .filter(|at| *at < pending.len());
                drop(pending);
                let Some(at) = gives else {
                    break false;
                };
                self.pool.remove_at(at);
                if self.pool.fits(incoming.bytes, count, most) {
                    break true;
                }
            }
        };
        if let Some(error) = self.runtime.storage_error() {
            return Err(error);
        }
        Ok(room)
    }
    /// Runs a transaction alone in a candidate on the tip: whether it fits a
    /// block and succeeds, or why not, and the compute it ran. A storage
    /// failure is an error.
    fn trial(&self, tx: &Transaction) -> Result<(std::result::Result<(), String>, u64)> {
        let ancestry = self.ancestry(self.tip())?;
        let limits = self.tip_limits()?;
        let timestamp = self.next_timestamp(&ancestry)?;
        let outcome = {
            let mut candidate = self
                .runtime
                .candidate(
                    &self.genesis,
                    &ancestry,
                    limits,
                    timestamp,
                    self.config.votes,
                    self.config.producer(),
                )
                .map_err(|e| e.to_string())?;
            let outcome = match candidate.add(tx) {
                Ok(Some(r)) if r.status == Status::Success => Ok(()),
                Ok(Some(r)) => Err(format!("{:?}", r.status)),
                Ok(None) => Err(DOES_NOT_FIT.into()),
                Err(e) => Err(e.to_string()),
            };
            let compute = candidate.attempts().first().map_or(0, |a| a.used.0[0]);
            (outcome, compute)
        };
        if let Some(error) = self.runtime.storage_error() {
            return Err(error);
        }
        Ok(outcome)
    }
    /// Deferred submission: the transaction waits in the deferred set until
    /// `not_before` has passed and it executes successfully on the tip, then
    /// enters the pool. It stays in the set, through restarts and
    /// reorganizations, until a canonical block includes it or it expires.
    pub fn submit_deferred(&mut self, bytes: &[u8], not_before: u64) -> Result<Value> {
        self.submit_deferred_from(bytes, not_before, None)
    }
    /// `submit_deferred` of a transaction relayed from `source`.
    pub fn submit_deferred_from(
        &mut self,
        bytes: &[u8],
        not_before: u64,
        source: Option<std::net::IpAddr>,
    ) -> Result<Value> {
        let tx = self.decode_envelope(bytes)?;
        let id = tx.id(self.genesis.chain_id());
        let current = now();
        if let Some(entry) = self.deferred.get(&id) {
            return Ok(
                json!({"id":hex(&id),"known":true,"deferred":true,"not_before":entry.not_before,"expires":entry.expires}),
            );
        }
        if self.pool.contains(&id) {
            return Ok(json!({"id":hex(&id),"known":true,"deferred":false}));
        }
        if not_before > current.saturating_add(DEFERRED_HORIZON) {
            return Err(HORIZON.into());
        }
        if source.is_some()
            && self
                .deferred
                .values()
                .filter(|d| d.source == source)
                .count()
                >= DEFERRED_PER_SOURCE
        {
            return Err(DEFERRED_FULL.into());
        }
        // A full set takes an entry due earlier in place of the relayed entry
        // due latest.
        while self.deferred.len() >= DEFERRED_MAX
            || self.deferred_bytes().saturating_add(bytes.len()) > self.held_bytes()
        {
            let latest = self
                .deferred
                .iter()
                .filter(|(_, d)| d.source.is_some())
                .max_by_key(|(_, d)| d.not_before)
                .map(|(id, d)| (*id, d.not_before));
            match latest {
                Some((id, due)) if due > not_before => {
                    self.deferred.remove(&id);
                }
                _ => return Err(DEFERRED_FULL.into()),
            }
        }
        let expires = not_before.max(current).saturating_add(DEFERRED_GRACE);
        self.deferred.insert(
            id,
            Deferred {
                tx,
                not_before,
                expires,
                bytes: bytes.len(),
                since: self.height() as u64,
                last_attempt: 0,
                last_tip: None,
                last_result: None,
                admitted: false,
                source,
            },
        );
        self.deferred_changed();
        Ok(
            json!({"id":hex(&id),"known":false,"deferred":true,"not_before":not_before,"expires":expires}),
        )
    }
    fn deferred_bytes(&self) -> usize {
        self.deferred.values().map(|d| d.bytes).sum()
    }
    /// The most bytes of encoded transactions the pool holds, and the deferred
    /// set as well (wiki: node/mining): `mempool_mib`, or the data of four
    /// blocks at the next block's capacity when that is more, so that a
    /// transaction of any size a block can hold always fits.
    fn held_bytes(&self) -> usize {
        let capacity = self.tip_limits().map_or(0, |limits| limits.0[2]);
        let configured =
            usize::try_from(self.config.mempool_mib.saturating_mul(1 << 20)).unwrap_or(usize::MAX);
        usize::try_from(capacity.saturating_mul(4))
            .unwrap_or(usize::MAX)
            .max(configured)
    }
    /// The deferred set changed: `tick` writes it within `DEFERRED_SAVE`
    /// seconds, and closing the chain writes it at once.
    fn deferred_changed(&mut self) {
        self.deferred_dirty.get_or_insert(now());
    }
    fn flush_deferred(&mut self, force: bool) -> Result<()> {
        if let Some(since) = self.deferred_dirty
            && (force || now() >= since.saturating_add(DEFERRED_SAVE))
        {
            self.save_deferred()?;
            self.deferred_dirty = None;
        }
        Ok(())
    }
    fn deferred_path(&self) -> PathBuf {
        self.directory.join("deferred.json")
    }
    fn save_deferred(&self) -> Result<()> {
        let items: Vec<Value> = self
            .deferred
            .values()
            .map(|d| json!({"hex":hex(&d.tx.encode()),"not_before":d.not_before,"expires":d.expires,"since":d.since,"source":d.source}))
            .collect();
        config::atomic_write(
            &self.deferred_path(),
            &serde_json::to_vec(&json!({"version":1,"items":items})).map_err(|e| e.to_string())?,
        )
    }
    fn load_deferred(&mut self) -> Result<()> {
        let path = self.deferred_path();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        // A file that does not parse, as a damaged disk may leave it, is set
        // aside for the operator, and the node starts with an empty set.
        let file: Value = match serde_json::from_slice::<Value>(&bytes) {
            Ok(file) if file["items"].is_array() => file,
            parsed => {
                let aside = path.with_extension("json.corrupt");
                let reason = parsed.map_or_else(|e| e.to_string(), |_| "no items".into());
                eprintln!(
                    "invalid deferred set ({reason}); moved to {} and started empty",
                    aside.display()
                );
                std::fs::rename(&path, &aside)
                    .map_err(|e| format!("cannot move aside {}: {e}", path.display()))?;
                return Ok(());
            }
        };
        let current = now();
        for item in file["items"].as_array().into_iter().flatten() {
            // An entry the node can no longer decode, such as a transaction
            // above a data capacity that was voted down since, is dropped
            // rather than keeping the node from starting.
            let loaded = (|| -> Result<(Vec<u8>, u64, u64)> {
                Ok((
                    unhex(str_param(item, "hex")?, usize::MAX)?,
                    item["not_before"].as_u64().ok_or("expected not_before")?,
                    item["expires"].as_u64().ok_or("expected expires")?,
                ))
            })()
            .and_then(|(bytes, not_before, expires)| {
                Ok((
                    self.decode_envelope(&bytes)?,
                    bytes.len(),
                    not_before,
                    expires,
                ))
            });
            let (tx, bytes, not_before, expires) = match loaded {
                Ok(loaded) => loaded,
                Err(error) => {
                    eprintln!("deferred transaction dropped: {error}");
                    continue;
                }
            };
            let since = item["since"].as_u64().unwrap_or(0);
            if expires < current
                || self.deferred.len() >= DEFERRED_MAX
                || self.deferred_bytes().saturating_add(bytes) > self.held_bytes()
                || self
                    .store
                    .succeeded_above(tx.id(self.genesis.chain_id()), since)?
            {
                continue;
            }
            self.deferred.insert(
                tx.id(self.genesis.chain_id()),
                Deferred {
                    tx,
                    not_before,
                    expires,
                    bytes,
                    since,
                    last_attempt: 0,
                    last_tip: None,
                    last_result: None,
                    admitted: false,
                    source: item["source"].as_str().and_then(|s| s.parse().ok()),
                },
            );
        }
        Ok(())
    }
    /// The timestamp the next block would carry: the clock, or just past the
    /// median of recent timestamps when the clock is behind.
    fn next_timestamp(&self, ancestry: &consensus::Ancestry) -> Result<u64> {
        let mut times = ancestry.recent.clone();
        if times.len() < 11 {
            times.push(self.genesis.timestamp);
        }
        times.sort_unstable();
        Ok(now().max(
            times[times.len() / 2]
                .checked_add(1)
                .ok_or("timestamp overflow")?,
        ))
    }
    /// Keeps the pool and the deferred set moving, with bounded work per call:
    /// it runs on a clock and before every block is assembled.
    ///
    /// - Pooled transactions past their age leave.
    /// - Expired deferred transactions leave. Due ones run on the tip, the
    ///   least recently tried first, at most about one block's compute every
    ///   `DEFERRED_PASS` seconds and on every new tip; those that succeed enter
    ///   the pool, and those that do not fit even a block holding nothing else
    ///   leave.
    pub fn tick(&mut self) -> Result<()> {
        let current = now();
        let tip = self.tip();
        let limits = self.tip_limits()?;
        // Pooled transactions leave with age; a declared budget is never a
        // reason, since only use has to fit a block.
        self.pool
            .retain(|p| p.arrived.saturating_add(POOL_AGE) >= current);
        if !self.deferred.is_empty()
            && (current >= self.deferred_next || self.deferred_tip != Some(tip))
        {
            self.deferred_next = current.saturating_add(DEFERRED_PASS);
            self.deferred_tip = Some(tip);
            let before = self.deferred.len();
            self.deferred.retain(|_, d| d.expires >= current);
            if self.deferred.len() != before {
                self.deferred_changed();
            }
            let mut due: Vec<(u64, Hash)> = self
                .deferred
                .iter()
                .filter(|(id, d)| {
                    !self.pool.contains(id)
                        && d.not_before <= current
                        && (d.last_tip != Some(tip)
                            || d.last_attempt.saturating_add(DEFERRED_RETRY) <= current)
                })
                .map(|(id, d)| (d.last_attempt, *id))
                .collect();
            // The entries tried least recently first, until about a block's
            // compute has run: the trial that crosses it still finishes, and
            // no trial runs past a block.
            due.sort_unstable();
            let mut allowance = limits.0[0];
            for (_, id) in due {
                if allowance == 0 {
                    break;
                }
                let tx = self.deferred[&id].tx.clone();
                let bytes = self.deferred[&id].bytes;
                let (outcome, compute) = self.trial(&tx)?;
                allowance = allowance.saturating_sub(compute);
                if outcome.as_ref().is_err_and(|e| e == DOES_NOT_FIT) {
                    self.deferred.remove(&id);
                    self.deferred_changed();
                    continue;
                }
                let source = self.deferred[&id].source;
                if outcome.is_ok() {
                    let pooled = Pooled {
                        tx,
                        id,
                        bytes,
                        arrived: current,
                        source,
                        tried: 0,
                    };
                    if self.make_room(&pooled)? {
                        self.pool.push(pooled);
                    }
                }
                let entry = self
                    .deferred
                    .get_mut(&id)
                    .ok_or("deferred entry vanished")?;
                entry.last_attempt = current;
                entry.last_tip = Some(tip);
                entry.last_result = outcome.err();
            }
            for (id, entry) in self.deferred.iter_mut() {
                entry.admitted = self.pool.contains(id);
            }
        }
        // Commits made while no block arrives, such as a reorganization's or
        // an operator's, reach the disk within the interval too.
        if let Err(error) = self.store.flush_due() {
            return Err(self.fail(format!("database commit failed; restart required: {error}")));
        }
        self.flush_deferred(false)
    }
    /// Whether a transaction is pooled or deferred, and its envelope if so.
    fn pending_json(&self, id: Hash) -> Value {
        let pending_tx = self.pool.get(&id);
        let pending = pending_tx.is_some();
        let pending_tx = pending_tx.or_else(|| self.deferred.get(&id).map(|d| &d.tx));
        json!({"pending":pending,"deferred":self.deferred_json(id),
            "pending_transaction":pending_tx.map(|tx|json!({"hex":reads::inline_hex(tx),"transaction":reads::transaction_summary(tx)}))})
    }
    fn deferred_json(&self, id: Hash) -> Option<Value> {
        self.deferred.get(&id).map(|d| {
            json!({"not_before":d.not_before,"expires":d.expires,"admitted":d.admitted,
                "last_attempt":(d.last_attempt > 0).then_some(d.last_attempt),"last_result":d.last_result})
        })
    }
    /// A candidate block on the current tip, complete except for the nonce:
    /// the node's half of mining. The template is kept so that a solution can
    /// be matched to it; a new tip discards every template.
    pub fn template(&mut self, selection: Selection) -> Result<Value> {
        self.tick()?;
        let ancestry = self.ancestry(self.tip())?;
        let limits = self.tip_limits()?;
        let timestamp = self.next_timestamp(&ancestry)?;
        let producer = self.config.producer();
        let chain_id = self.genesis.chain_id();
        let mut attempts = Vec::new();
        let block = {
            let mut candidate = self
                .runtime
                .candidate(
                    &self.genesis,
                    &ancestry,
                    limits,
                    timestamp,
                    self.config.votes,
                    producer,
                )
                .map_err(|e| e.to_string())?;
            match selection {
                Selection::Policy => {
                    // At most about a block's compute thrown away per template.
                    candidate.limit_discarded(limits.0[0]);
                    self.selector.select(&self.pool.pending(), &mut candidate);
                    attempts = candidate.attempts().to_vec();
                }
                Selection::Explicit(ids) => {
                    for id in ids {
                        let tx = self.pool.get(&id).cloned().ok_or_else(|| {
                            format!("transaction {} is not in the pool", hex(&id))
                        })?;
                        if candidate.add(&tx).map_err(|e| e.to_string())?.is_none() {
                            return Err(format!("transaction {} does not fit the block", hex(&id)));
                        }
                    }
                }
            }
            candidate.finish().map_err(|e| e.to_string())?
        };
        if let Some(error) = self.runtime.storage_error() {
            return Err(error);
        }
        // Every transaction the template ran waits its turn again; one that did
        // not fit a block holding nothing else leaves the pool, and the
        // deferred set if it came from there.
        let current = now();
        for attempt in &attempts {
            if !attempt.fit && attempt.alone {
                self.pool.remove(&attempt.id);
                if self.deferred.remove(&attempt.id).is_some() {
                    self.deferred_changed();
                }
            } else {
                self.pool.tried(&attempt.id, current);
            }
        }
        let id = block.header.id();
        if !self.templates.iter().any(|(t, _)| *t == id) {
            self.templates.push_back((id, block.clone()));
            while self.templates.len() > TEMPLATES_KEPT {
                self.templates.pop_front();
            }
        }
        Ok(
            json!({"id":hex(&id),"height":block.header.height,"parent":hex(&block.header.parent),"timestamp":block.header.timestamp,
            "header_hex":hex(&block.header.encode()),"bytes":block.encode().len(),"used":block.header.used.0,
            "transactions":block.transactions.iter().map(|tx| hex(&tx.id(chain_id))).collect::<Vec<_>>(),
            "mining":mining::describe(&block.header)}),
        )
    }
    /// A nonce for a template: checked against the target, then the block is
    /// imported like any other, which validates it fully and lets the peer
    /// loop relay it.
    pub fn solution(&mut self, id: Hash, nonce: u128) -> Result<Value> {
        let position = self
            .templates
            .iter()
            .position(|(t, _)| *t == id)
            .ok_or(mining::EXPIRED)?;
        let mut block = self.templates[position].1.clone();
        block.header.nonce = nonce;
        if !consensus::valid_pow(&block.header) {
            return Err("solution does not meet the target".into());
        }
        self.import(&block.encode(), true)
    }
    /// Development mining in one call: template, CPU search, solution, all
    /// on the calling thread. The daemon's `mine` searches off the chain
    /// thread instead (`daemon::Node::mine`).
    pub fn mine(&mut self, count: u64, attempts: u64) -> Result<Value> {
        if !(1..=100).contains(&count) || !(1..=10_000_000).contains(&attempts) {
            return Err("count must be 1..100 and attempts 1..10000000".into());
        }
        let mut mined = Vec::new();
        let tried = AtomicU64::new(0);
        for _ in 0..count {
            let template = self.template(Selection::Policy)?;
            let id = hash_arg(str_param(&template, "id")?)?;
            let header = self
                .templates
                .iter()
                .find(|(t, _)| *t == id)
                .map(|(_, b)| b.header.clone())
                .ok_or("template vanished")?;
            let Some(nonce) = mining::search(&header, 0, attempts, &self.shutdown, &tried) else {
                if self.shutdown.load(Ordering::Relaxed) {
                    return Err("node is stopping".into());
                }
                return Ok(json!({"blocks":mined,"exhausted":true}));
            };
            mined.push(self.solution(id, nonce)?);
        }
        Ok(json!({"blocks":mined,"exhausted":false}))
    }
    /// The best header's branch back from its tip, the ten most recent and
    /// then at gaps that double, then up to eight stored side branches, most
    /// work first, then genesis.
    fn locator(&self) -> Vec<Hash> {
        let mut ids = Vec::new();
        let mut height = self.best.len();
        let mut step = 1;
        while height > 0 && ids.len() < 23 {
            if let Some(id) = self.best.get(height - 1, &self.index) {
                ids.push(id);
            }
            height = height.saturating_sub(step);
            if ids.len() > 8 {
                step *= 2;
            }
        }
        let on_best = |id: &Hash| {
            let height = self.entry(id).header.height as usize;
            self.best.get(height - 1, &self.index) == Some(*id)
        };
        ids.extend(self.index.heads().filter(|id| !on_best(id)).take(8));
        ids.push(self.genesis.block_id());
        ids
    }
    /// A height in decimal digits, or a 64-character block id.
    fn block_id(&self, text: &str) -> Result<Hash> {
        if text.len() != 64 && !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
            let height = text.parse::<usize>().map_err(|_| "block not found")?;
            return self
                .canonical
                .get(
                    height
                        .checked_sub(1)
                        .ok_or("genesis has no ordinary block body")?,
                    &self.index,
                )
                .ok_or("block not found".into());
        }
        let id = hash_arg(text)?;
        if !self.index.get(&id).is_some_and(|e| e.stored) {
            return Err("block not found".into());
        }
        Ok(id)
    }
    pub fn request(&mut self, method: &str, p: Value) -> Result<Value> {
        self.request_with(method, p, None)
    }
    /// A chain that failed, or whose storage did, answers nothing more.
    fn healthy(&self) -> Result<()> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        if let Some(error) = self.runtime.storage_error() {
            return Err(error);
        }
        if let Some(error) = self.index.storage_error() {
            return Err(error);
        }
        Ok(())
    }
    /// A peer's request that the chain answers itself (wiki:
    /// reference/peer-protocol): its state, the ids of its pooled and deferred
    /// transactions, those transactions, and slices of one. Blocks and
    /// headers come from the reader threads, and what a peer pushes arrives
    /// as `import` and `submit`.
    pub fn answer_wire(&mut self, request: &wire::Request) -> Result<wire::Answer> {
        self.healthy()?;
        match request {
            wire::Request::Hello(_) | wire::Request::State => Ok(wire::Answer::State(self.state())),
            wire::Request::Inventory { deferred, after } => {
                let ids: Vec<Hash> = if *deferred {
                    use std::ops::Bound;
                    let low = after.map_or(Bound::Unbounded, Bound::Excluded);
                    self.deferred
                        .range((low, Bound::Unbounded))
                        .take(INVENTORY_PAGE + 1)
                        .map(|(id, _)| *id)
                        .collect()
                } else {
                    self.pool
                        .ids_after(*after)
                        .filter(|id| !self.deferred.contains_key(*id))
                        .take(INVENTORY_PAGE + 1)
                        .copied()
                        .collect()
                };
                let next = (ids.len() > INVENTORY_PAGE).then(|| ids[INVENTORY_PAGE - 1]);
                Ok(wire::Answer::Inventory {
                    ids: ids.into_iter().take(INVENTORY_PAGE).collect(),
                    next,
                })
            }
            wire::Request::Pending { ids } => Ok(wire::Answer::Items(self.relayed(ids, false))),
            wire::Request::Deferred { ids } => Ok(wire::Answer::Items(self.relayed(ids, true))),
            wire::Request::TransactionRaw { id, offset, length } => {
                self.transaction_slice(id, *offset, *length)
            }
            _ => Err(wire::UNKNOWN_REQUEST.into()),
        }
    }
    /// What this node tells its peers of itself, but for its run's nonce,
    /// which the connection adds.
    pub fn state(&self) -> wire::State {
        wire::State {
            version: wire::VERSION,
            chain_id: self.genesis.chain_id(),
            agent: wire::AGENT.into(),
            features: Vec::new(),
            nonce: 0,
            height: self.height() as u64,
            tip: self.tip(),
            work: self.tip_work(),
            locator: self.locator(),
            pending: self
                .pool
                .iter()
                .map(|p| p.id)
                .filter(|id| !self.deferred.contains_key(id))
                .take(wire::STATE_IDS)
                .collect(),
            deferred: self
                .deferred
                .keys()
                .take(wire::STATE_IDS)
                .copied()
                .collect(),
        }
    }
    /// Of `ids`, the pooled transactions, deferred ones left out, or with
    /// `deferred` the deferred ones, as a `pending` or `deferred` answer lists
    /// them: whole while all of them fit `RELAY_INLINE`, one slice, by size
    /// alone otherwise, for the peer to fetch in slices.
    fn relayed(&self, ids: &[Hash], deferred: bool) -> Vec<wire::Item> {
        let mut inline = 0usize;
        let mut items = Vec::new();
        for id in ids {
            let (tx, not_before) = if deferred {
                match self.deferred.get(id) {
                    Some(d) => (&d.tx, Some(d.not_before)),
                    None => continue,
                }
            } else {
                match self.pool.get(id) {
                    Some(tx) if !self.deferred.contains_key(id) => (tx, None),
                    _ => continue,
                }
            };
            let bytes = tx.encode();
            let body = if inline.saturating_add(bytes.len()) <= RELAY_INLINE {
                inline += bytes.len();
                wire::Body::Whole(bytes)
            } else {
                wire::Body::Total(bytes.len() as u64)
            };
            items.push(wire::Item {
                id: *id,
                not_before,
                body,
            });
        }
        items
    }
    /// A slice of a pooled or deferred transaction (wiki: node/sync), for a
    /// peer fetching one listed by its size alone. The one last served stays
    /// encoded for its next slice.
    fn transaction_slice(&mut self, id: &Hash, offset: u64, length: u32) -> Result<wire::Answer> {
        if !self.pool.contains(id) && !self.deferred.contains_key(id) {
            return Err("transaction not found".into());
        }
        let bytes = match &self.served_transaction {
            Some((served, bytes)) if served == id => bytes.clone(),
            _ => {
                let tx = self
                    .pool
                    .get(id)
                    .or_else(|| self.deferred.get(id).map(|d| &d.tx))
                    .ok_or("transaction not found")?;
                let bytes = std::rc::Rc::new(tx.encode());
                self.served_transaction = Some((*id, bytes.clone()));
                bytes
            }
        };
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        if start > bytes.len() {
            return Err("slice offset out of range".into());
        }
        let end = start.saturating_add(length as usize).min(bytes.len());
        Ok(wire::Answer::Slice {
            offset,
            total: bytes.len() as u64,
            bytes: bytes[start..end].to_vec(),
        })
    }
    /// A request whose payload may come as bytes rather than hex, as a block
    /// fetched from a peer does.
    pub fn request_with(
        &mut self,
        method: &str,
        p: Value,
        bytes: Option<Vec<u8>>,
    ) -> Result<Value> {
        self.healthy()?;
        match method {
            "status" | "metrics" => {
                let limits = self.tip_limits()?.0;
                Ok(
                    json!({"name":self.config.name,"version":env!("CARGO_PKG_VERSION"),"network":self.network(),
                    "chain_id":hex(&self.genesis.chain_id()),"genesis":hex(&self.genesis.block_id()),"height":self.height(),
                    "tip":hex(&self.tip()),"work":self.tip_work().to_string(),"state_root":hex(&self.runtime.state_root()),
                    "limits":limits,
                    "resource_order":["C","S","D","G"],"mempool_count":self.pool.len(),"mempool_bytes":self.pool.bytes(),"deferred_count":self.deferred.len(),
                    "producer_hex":hex(&self.config.producer()),
                    "stored_blocks":self.index.stored(),"best_height":self.best.len(),"state_records":self.runtime.record_count(),"uptime_seconds":now().saturating_sub(self.started),
                    "undo_pruned":self.pruned,"settled_height":self.index.floor(),"held_headers":self.index.held(),"replaying":self.recovery.is_some(),
                    "marked":self.marks.iter().map(|id| hex(id)).collect::<Vec<_>>(),"reorganization":self.reorganization,
                    "warnings":self.fork_warning().into_iter().collect::<Vec<_>>()}),
                )
            }
            "config" => serde_json::to_value(&self.config).map_err(|e| e.to_string()),
            "votes" => self.voting(),
            // Operator overrides, from the control channel only.
            "invalidate" => self.mark_invalid(self.named(str_param(&p, "id")?)?),
            "reconsider" => self.reconsider(self.named(str_param(&p, "id")?)?),
            "api_config" => {
                let mut c = self.config.clone();
                c.api_enabled = p["enabled"].as_bool().ok_or("expected enabled boolean")?;
                c.api_cors = p["cors"].as_bool().unwrap_or(false);
                c.save(&self.directory)?;
                self.config = c;
                Ok(json!({"enabled":self.config.api_enabled,"cors":self.config.api_cors}))
            }
            "add_peer" | "remove_peer" => {
                let address = str_param(&p, "address")?
                    .parse()
                    .map_err(|_| "expected IP:port")?;
                let mut c = self.config.clone();
                if method == "remove_peer" {
                    c.peers.retain(|a| *a != address);
                } else if !c.peers.contains(&address) {
                    c.peers.push(address);
                }
                c.save(&self.directory)?;
                self.config = c;
                Ok(json!({"peers":self.config.peers}))
            }
            // Read from the database, as the reader threads read it.
            "headers" => reads::answer(&self.store.view()?, method, &p),
            // A header a peer pushes a block under, in its encoding.
            "check_header" => {
                let h = Header::decode(bytes.as_deref().ok_or("expected a header")?)
                    .map_err(|e| format!("{INVALID}block: {e}"))?;
                let known = self.index.get(&h.id()).is_some_and(|e| e.stored);
                if known {
                    return Ok(json!({"id":hex(&h.id()),"known":true}));
                }
                let (_, limits) = self.checked_header(&h)?;
                Self::timely(&h)?;
                // The block's data capacity bounds its size.
                Ok(json!({"id":hex(&h.id()),"known":false,"max":limits.0[2]}))
            }
            // Sync's own requests, from this node's peer connections only.
            // A peer's page of headers, in their encodings one after another.
            "headers_from" => {
                let peer = str_param(&p, "peer")?.to_string();
                let bytes = bytes.ok_or("expected headers")?;
                if !bytes.len().is_multiple_of(HEADER_LEN)
                    || bytes.len() / HEADER_LEN > HEADERS_PAGE
                {
                    return Err("header response limit".into());
                }
                let headers = bytes
                    .as_chunks::<HEADER_LEN>()
                    .0
                    .iter()
                    .map(|h| Header::decode(h).map_err(|e| format!("{INVALID}header: {e}")))
                    .collect::<Result<Vec<_>>>()?;
                self.headers_from(&peer, &headers)
            }
            "blocks_wanted" => {
                let peer = str_param(&p, "peer")?;
                let wanted = self.blocks_wanted(peer);
                let ids: Vec<String> = wanted.iter().map(|id| hex(id)).collect();
                // The most bytes each can have: its data capacity.
                let max: Vec<u64> = wanted.iter().map(|id| self.entry(id).limits.0[2]).collect();
                Ok(json!({"ids":ids,"max":max}))
            }
            // A peer that could not give a block, or whose connection ended: what
            // it was asked goes back to the others.
            "release" => {
                let peer = str_param(&p, "peer")?.to_string();
                match p["id"].as_str() {
                    Some(id) => {
                        let id = hash_arg(id)?;
                        if self
                            .in_flight
                            .get(&id)
                            .is_some_and(|(holder, _)| *holder == peer)
                        {
                            self.in_flight.remove(&id);
                        }
                    }
                    None => {
                        self.in_flight.retain(|_, (holder, _)| *holder != peer);
                        self.presync.remove(&peer);
                    }
                }
                Ok(json!({}))
            }
            // Transport frames bound peer input; reassembled slices are bounded by
            // the header's active data capacity before the body is decoded.
            "import" => {
                let bytes = match bytes {
                    Some(bytes) => bytes,
                    None => unhex(str_param(&p, "hex")?, usize::MAX)?,
                };
                self.import_from(&bytes, true, p["peer"].as_str())
            }
            // Peers whose blocks failed since the last time this was asked.
            "offenders" => Ok(json!({"peers":std::mem::take(&mut self.offenders)})),
            "submit" => {
                let limit = usize::try_from(self.tip_limits()?.0[2]).unwrap_or(usize::MAX);
                let bytes = match bytes {
                    Some(bytes) if bytes.len() > limit => return Err(TOO_LARGE.into()),
                    Some(bytes) => bytes,
                    None => unhex(str_param(&p, "hex")?, limit)?,
                };
                // Fetched by id from a peer: it must be what was asked for.
                if let Some(expected) = p["id"].as_str() {
                    let tx = self.decode_envelope(&bytes).map_err(|_| NOT_ASKED)?;
                    if hex(&tx.id(self.genesis.chain_id())) != expected {
                        return Err(NOT_ASKED.into());
                    }
                }
                if p["relayed"] == true {
                    self.recent(&bytes)?;
                }
                let source = source_of(&p);
                match &p["not_before"] {
                    Value::Null => self.submit_from(&bytes, source),
                    value => {
                        let not_before = value.as_u64().ok_or("expected not_before seconds")?;
                        self.submit_deferred_from(&bytes, not_before, source)
                    }
                }
            }
            "template" => {
                let selection = match p["transactions"].as_array() {
                    Some(ids) => Selection::Explicit(
                        ids.iter()
                            .map(|v| hash_arg(v.as_str().ok_or("expected transaction id")?))
                            .collect::<Result<Vec<_>>>()?,
                    ),
                    None => Selection::Policy,
                };
                self.template(selection)
            }
            "solution" => {
                let id = hash_arg(str_param(&p, "template")?)?;
                let nonce = match &p["nonce"] {
                    Value::Number(n) => u128::from(n.as_u64().ok_or("expected nonce")?),
                    Value::String(s) => s.parse::<u128>().map_err(|_| "expected nonce")?,
                    _ => return Err("expected nonce".into()),
                };
                self.solution(id, nonce)
            }
            "miner_config" => {
                let mut c = self.config.clone();
                c.miner_enabled = p["enabled"].as_bool().ok_or("expected enabled boolean")?;
                if let Some(threads) = p["threads"].as_u64() {
                    c.miner_threads =
                        u32::try_from(threads).map_err(|_| "threads must be 1..64")?;
                }
                if let Some(spacing) = p["spacing"].as_u64() {
                    c.miner_spacing = spacing;
                }
                match &p["producer"] {
                    Value::Null => {}
                    Value::String(producer) if producer.is_empty() => c.producer_hex = None,
                    Value::String(producer) => c.producer_hex = Some(producer.clone()),
                    _ => return Err("expected producer hex".into()),
                }
                match &p["votes"] {
                    Value::Null => {}
                    Value::Array(votes) if votes.len() == 4 => {
                        for (vote, value) in c.votes.iter_mut().zip(votes) {
                            *vote = value
                                .as_u64()
                                .and_then(|v| u8::try_from(v).ok())
                                .filter(|v| *v <= 2)
                                .ok_or(VOTES)?;
                        }
                    }
                    _ => return Err(VOTES.into()),
                }
                c.save(&self.directory)?;
                self.config = c;
                self.templates.clear();
                Ok(
                    json!({"enabled":self.config.miner_enabled,"threads":self.config.miner_threads,"spacing":self.config.miner_spacing,
                    "producer_hex":hex(&self.config.producer()),"votes":self.config.votes,"selection":self.selector.describe()}),
                )
            }
            "block" | "blocks" | "events" | "contract" | "storage" | "storage_batch" | "raw" => {
                reads::answer(&self.store.view()?, method, &p)
            }
            "mempool" => {
                let mut items = Vec::new();
                let mut size = 0;
                // Peers receive pooled deferred transactions through `deferred`,
                // so that they too hold them until due instead of mining them early.
                for pooled in self.pool.iter().take(1024) {
                    let (id, tx) = (pooled.id, &pooled.tx);
                    let item = json!({"id":hex(&id),"bytes":pooled.bytes,"budget":tx.budget.0,"deferred":self.deferred_json(id)});
                    if !reads::push_bounded(&mut items, item, &mut size)? {
                        break;
                    }
                }
                let mut deferred = Vec::new();
                for (id, d) in &self.deferred {
                    let item = json!({"id":hex(id),"bytes":d.tx.encode().len(),"budget":d.tx.budget.0,"deferred":self.deferred_json(*id)});
                    if !reads::push_bounded(&mut deferred, item, &mut size)? {
                        break;
                    }
                }
                Ok(json!({"items":items,"bytes":self.pool.bytes(),"deferred":deferred}))
            }
            "transaction" => {
                let id = hash_arg(str_param(&p, "id")?)?;
                let stored = reads::answer(&self.store.view()?, method, &p)?;
                reads::finish(stored, self.pending_json(id))
            }
            // What a transaction lookup off this thread adds to its stored part.
            "transaction_pending" => Ok(self.pending_json(hash_arg(str_param(&p, "id")?)?)),
            _ => Err("unknown method".into()),
        }
    }
}
impl Drop for Chain {
    /// A deferred set not yet written is written on the way out.
    fn drop(&mut self) {
        if let Err(error) = self.flush_deferred(true) {
            eprintln!("deferred set not saved: {error}");
        }
    }
}
pub fn str_param<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p[key]
        .as_str()
        .ok_or_else(|| format!("expected string {key}"))
}
/// An optional non-negative integer parameter; anything else is refused
/// rather than silently taking the default.
pub fn u64_param(p: &Value, key: &str) -> Result<Option<u64>> {
    match &p[key] {
        Value::Null => Ok(None),
        Value::Number(n) if n.as_u64().is_some() => Ok(n.as_u64()),
        _ => Err(format!("expected integer {key}")),
    }
}
struct Request {
    method: String,
    params: Value,
    bytes: Option<Vec<u8>>,
    /// A peer's request the chain answers itself, in place of the rest.
    wire: Option<wire::Request>,
    reply: oneshot::Sender<Result<Reply>>,
    /// The place an outside request holds in its requester's share of the
    /// queue, until it is answered or dropped.
    _place: Option<Place>,
}
/// The chain thread's answer: JSON, or a peer request's answer as it goes on
/// the wire.
enum Reply {
    Json(Value),
    Wire(wire::Answer),
}
impl Reply {
    fn json(self) -> Result<Value> {
        match self {
            Reply::Json(value) => Ok(value),
            Reply::Wire(_) => Err("unexpected answer".into()),
        }
    }
    fn wire(self) -> Result<wire::Answer> {
        match self {
            Reply::Wire(answer) => Ok(answer),
            Reply::Json(_) => Err("unexpected answer".into()),
        }
    }
}
/// The places in the outside queue that each kind of requester has: peers
/// that connected, and REST. A request that finds its share full is refused
/// at once, so a flood of one kind crowds out neither the node's own work
/// nor the other kind.
const SHARE: usize = 128;
struct Place(Arc<AtomicUsize>);
impl Place {
    fn take(taken: &Arc<AtomicUsize>) -> Option<Self> {
        taken
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < SHARE).then_some(n + 1)
            })
            .ok()?;
        Some(Self(taken.clone()))
    }
}
impl Drop for Place {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
/// The node's status as the chain thread last published it, and when: it is
/// renewed after every request the thread answers and at least every 0.2
/// seconds, so that status reads never wait for the thread.
#[derive(Clone)]
pub struct Published {
    pub at: std::time::Instant,
    pub status: Arc<Result<Value>>,
}
#[derive(Clone)]
pub struct Handle {
    /// The node's own requests (its connections, miner, supervisor and
    /// operator), served first and never refused for a full queue.
    own: mpsc::Sender<Request>,
    /// Requests from outside, from peers that connected and from REST: a
    /// bounded queue in which each has its share (`SHARE`).
    shared: mpsc::SyncSender<Request>,
    peers: Arc<AtomicUsize>,
    rest: Arc<AtomicUsize>,
    events: tokio::sync::watch::Receiver<Event>,
    status: tokio::sync::watch::Receiver<Published>,
    reader: store::Reader,
}
/// How long the chain thread waits for an outside request before it looks
/// at its own queue again, and how often it ticks whatever the traffic.
const POLL: Duration = Duration::from_millis(10);
const TICK: Duration = Duration::from_millis(200);
/// How long the chain thread works toward the best branch before it answers
/// the next request.
const STEP_TIME: Duration = Duration::from_millis(50);
impl Handle {
    pub fn spawn(
        dir: PathBuf,
        shutdown: Arc<AtomicBool>,
    ) -> Result<(Self, std::thread::JoinHandle<()>)> {
        Self::spawn_with(dir, shutdown, Box::new(mining::FirstIn::default()))
    }
    /// The chain thread with a fork's own transaction selection.
    pub fn spawn_with(
        dir: PathBuf,
        shutdown: Arc<AtomicBool>,
        selector: Box<dyn mining::Selector>,
    ) -> Result<(Self, std::thread::JoinHandle<()>)> {
        let (own, own_requests) = mpsc::channel::<Request>();
        let (shared, shared_requests) = mpsc::sync_channel::<Request>(2 * SHARE);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (events_tx, events) = tokio::sync::watch::channel(Event::default());
        let published = |chain: &mut Chain| Published {
            at: std::time::Instant::now(),
            status: Arc::new(chain.request("status", json!({}))),
        };
        let (status_tx, status) = tokio::sync::watch::channel(Published {
            at: std::time::Instant::now(),
            status: Arc::new(Err("the node is starting".into())),
        });
        let worker = std::thread::spawn(move || {
            let mut chain = match Chain::open_with(&dir, shutdown.clone(), selector) {
                Ok(mut c) => {
                    events_tx.send_replace(c.event());
                    status_tx.send_replace(published(&mut c));
                    let _ = ready_tx.send(Ok(c.store.reader()));
                    c
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            chain.bound_steps(STEP_TIME);
            let mut ticked = std::time::Instant::now();
            while !shutdown.load(Ordering::Relaxed) {
                let request = match own_requests.try_recv() {
                    Ok(r) => Some(r),
                    Err(mpsc::TryRecvError::Disconnected) => break,
                    // With steps left, no waiting: they continue at once.
                    Err(mpsc::TryRecvError::Empty) if chain.activating() => {
                        shared_requests.try_recv().ok()
                    }
                    Err(mpsc::TryRecvError::Empty) => shared_requests.recv_timeout(POLL).ok(),
                };
                if let Some(r) = request {
                    let result = match &r.wire {
                        Some(request) => chain.answer_wire(request).map(Reply::Wire),
                        None => chain
                            .request_with(&r.method, r.params, r.bytes)
                            .and_then(reads::fits)
                            .map(Reply::Json),
                    };
                    // Published before the answer, so that a status read after
                    // it sees what the request changed.
                    status_tx.send_replace(published(&mut chain));
                    let _ = r.reply.send(result);
                }
                chain.resume_activation();
                // Deferred admission, pool aging and the deferred set's save
                // run on a clock, not only when the queue is idle.
                if ticked.elapsed() >= TICK {
                    ticked = std::time::Instant::now();
                    if let Err(error) = chain.tick() {
                        eprintln!("deferred transactions: {error}");
                    }
                    status_tx.send_replace(published(&mut chain));
                }
                let event = chain.event();
                events_tx.send_if_modified(|current| {
                    let changed = *current != event;
                    if changed {
                        *current = event;
                    }
                    changed
                });
            }
        });
        let reader = ready_rx.recv().map_err(|e| e.to_string())??;
        Ok((
            Self {
                own,
                shared,
                peers: Arc::new(AtomicUsize::new(0)),
                rest: Arc::new(AtomicUsize::new(0)),
                events,
                status,
                reader,
            },
            worker,
        ))
    }
    /// The node's tip and pool as they change.
    pub fn events(&self) -> tokio::sync::watch::Receiver<Event> {
        self.events.clone()
    }
    /// The status last published.
    pub fn status(&self) -> Published {
        self.status.borrow().clone()
    }
    /// A reader of the stored chain, for reads off the chain thread.
    pub fn reader(&self) -> &store::Reader {
        &self.reader
    }
    /// A request of the node's own.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_bytes(method, params, None).await
    }
    /// The same with a payload as bytes.
    pub async fn call_bytes(
        &self,
        method: &str,
        params: Value,
        bytes: Option<Vec<u8>>,
    ) -> Result<Value> {
        self.call_own(method.into(), params, bytes, None)
            .await?
            .json()
    }
    /// A peer request of the node's own that the chain answers, as for its
    /// outbound connections.
    pub async fn call_wire(&self, request: wire::Request) -> Result<wire::Answer> {
        self.call_own(request.method().into(), Value::Null, None, Some(request))
            .await?
            .wire()
    }
    async fn call_own(
        &self,
        method: String,
        params: Value,
        bytes: Option<Vec<u8>>,
        wire: Option<wire::Request>,
    ) -> Result<Reply> {
        let (reply, receiver) = oneshot::channel();
        self.own
            .send(Request {
                method,
                params,
                bytes,
                wire,
                reply,
                _place: None,
            })
            .map_err(|_| "node worker stopped")?;
        Self::answer(receiver).await
    }
    /// A request from a peer that connected: refused at once when the peers'
    /// share of the queue is full.
    pub async fn call_external(&self, method: &str, params: Value) -> Result<Value> {
        self.call_external_bytes(method, params, None).await
    }
    /// The same with a payload as bytes.
    pub async fn call_external_bytes(
        &self,
        method: &str,
        params: Value,
        bytes: Option<Vec<u8>>,
    ) -> Result<Value> {
        self.call_shared(&self.peers, method.into(), params, bytes, None)
            .await?
            .json()
    }
    /// A request of a peer that connected, that the chain answers itself.
    pub async fn call_external_wire(&self, request: wire::Request) -> Result<wire::Answer> {
        self.call_shared(
            &self.peers,
            request.method().into(),
            Value::Null,
            None,
            Some(request),
        )
        .await?
        .wire()
    }
    /// A request from REST: refused at once when REST's share of the queue is
    /// full.
    pub async fn call_rest(&self, method: &str, params: Value) -> Result<Value> {
        self.call_shared(&self.rest, method.into(), params, None, None)
            .await?
            .json()
    }
    async fn call_shared(
        &self,
        share: &Arc<AtomicUsize>,
        method: String,
        params: Value,
        bytes: Option<Vec<u8>>,
        wire: Option<wire::Request>,
    ) -> Result<Reply> {
        let place = Place::take(share).ok_or("node request queue unavailable")?;
        let (reply, receiver) = oneshot::channel();
        self.shared
            .try_send(Request {
                method,
                params,
                bytes,
                wire,
                reply,
                _place: Some(place),
            })
            .map_err(|_| "node request queue unavailable")?;
        Self::answer(receiver).await
    }
    async fn answer(receiver: oneshot::Receiver<Result<Reply>>) -> Result<Reply> {
        tokio::time::timeout(Duration::from_secs(30), receiver)
            .await
            .map_err(|_| "node request timed out")?
            .map_err(|_| "node worker stopped")?
    }
    /// The same request from a plain thread, such as the built-in miner's.
    pub fn call_blocking(&self, method: &str, params: Value) -> Result<Value> {
        let (reply, receiver) = oneshot::channel();
        self.own
            .send(Request {
                method: method.into(),
                params,
                bytes: None,
                wire: None,
                reply,
                _place: None,
            })
            .map_err(|_| "node worker stopped")?;
        receiver
            .blocking_recv()
            .map_err(|_| "node worker stopped")??
            .json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ting_core::{
        meter::Resources,
        protocol::{Payload, Transaction},
        state::State,
    };

    /// A listed seed block that this node's execution rejects stops the node
    /// with a report naming the block and the transaction, instead of marking
    /// the block invalid and blaming its source.
    #[test]
    fn a_seed_block_this_node_rejects_stops_it_and_names_the_transaction() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".node-tests")
            .join(format!("seed-unit-{}-{}", std::process::id(), crate::now()));
        let g = Genesis::experimental();
        let c = Config {
            control: "127.0.0.1:0".parse().unwrap(),
            p2p: "127.0.0.1:0".parse().unwrap(),
            api: "127.0.0.1:0".parse().unwrap(),
            ..Default::default()
        };
        config::initialize_empty(&dir, &c, g.clone()).unwrap();
        let mut chain = Chain::open(&dir, Arc::new(AtomicBool::new(false))).unwrap();
        // A block whose one receipt claims a C more than the deployment uses,
        // its commitments and totals following the receipt.
        let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
        let tx = Transaction {
            budget: Resources([1_000_000, 16, 4096, 4096]),
            payload: Payload::Deploy {
                code: wat::parse_str("(module (func (export \"invoke\")))").unwrap(),
                input: Vec::new(),
            },
        };
        let mut candidate = runtime
            .candidate(
                &g,
                &consensus::Ancestry::default(),
                g.initial,
                g.timestamp + 60,
                [0; 4],
                Vec::new(),
            )
            .unwrap();
        candidate.add(&tx).unwrap().unwrap();
        let mut block = candidate.finish().unwrap();
        block.receipts[0].used.0[0] += 1;
        block.header.receipts_root = consensus::commitments(&block.transactions, &block.receipts).1;
        block.header.used.0[0] += 1;
        while !consensus::valid_pow(&block.header) {
            block.header.nonce += 1;
        }
        chain.seed = Vec::leak(vec![block.header.id()]);
        let error = chain.import(&block.encode(), false).unwrap_err();
        assert!(
            error.contains("does not conform")
                && error.contains("seed block 1")
                && error.contains("transaction 0 of the block"),
            "{error}"
        );
        assert!(chain.failed.is_some());
        assert!(chain.offenders.is_empty(), "the source is not blamed");
        drop(chain);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
