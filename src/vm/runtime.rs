//! The runtime: transaction execution, the driver of nested invocations,
//! resource charging against frames and the block guard, and block assembly
//! and validation. It follows the protocol rules (sections 20 to 24) and is
//! independent of the engine that executes instructions.
//!
//! Nested calls and deployments never recurse on the native stack: a host
//! function records the request and interrupts the guest, the driver loop
//! below runs the child, then resumes the parent with the child's status.
use super::{
    decode::{self, Module},
    host::{Host, Outcome, Pending},
    interp, price,
    profile::Profile,
};
use crate::{
    hash::{Hash, hash},
    meter::{Meter, Resources},
    protocol::{
        self, BLOCK_BASE, Encode, Event, Genesis, Payload, Receipt, Status, TRANSACTION_BASE,
        Transaction, producer_hash,
    },
    state::{Changes, State, Value, code_key, growth, manifest_key},
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

/// How much a default cache holds, counted in the memory decoded programs take.
pub const CODE_CACHE_BYTES: u64 = 256 << 20;

/// Validated programs by CodeHash. Runtimes given the same cache share it, so
/// a node decodes an installed program once instead of in every block. It is
/// bounded by the memory its decoded programs take, and the program used least
/// recently leaves first. What it holds never changes a result: a miss
/// only means the node decodes the program again.
#[derive(Clone)]
pub struct CodeCache(Rc<RefCell<Programs>>);
struct Programs {
    entries: BTreeMap<Hash, Entry>,
    /// The entries by when they were last used, the oldest first.
    order: BTreeMap<u64, Hash>,
    bytes: u64,
    limit: u64,
    clock: u64,
}
struct Entry {
    cached: Rc<Cached>,
    bytes: u64,
    used: u64,
}
impl CodeCache {
    /// A cache for decoded programs taking up to `limit` bytes.
    pub fn new(limit: u64) -> Self {
        Self(Rc::new(RefCell::new(Programs {
            entries: BTreeMap::new(),
            order: BTreeMap::new(),
            bytes: 0,
            limit,
            clock: 0,
        })))
    }
    pub fn clear(&self) {
        let mut p = self.0.borrow_mut();
        p.entries.clear();
        p.order.clear();
        p.bytes = 0;
    }
    fn get(&self, id: &Hash) -> Option<Rc<Cached>> {
        let mut p = self.0.borrow_mut();
        p.clock += 1;
        let clock = p.clock;
        let entry = p.entries.get_mut(id)?;
        let last = std::mem::replace(&mut entry.used, clock);
        let cached = entry.cached.clone();
        p.order.remove(&last);
        p.order.insert(clock, *id);
        Some(cached)
    }
    fn insert(&self, id: Hash, cached: Rc<Cached>, bytes: u64) {
        let mut p = self.0.borrow_mut();
        if bytes > p.limit {
            return;
        }
        if let Some(old) = p.entries.remove(&id) {
            p.order.remove(&old.used);
            p.bytes -= old.bytes;
        }
        while p.bytes + bytes > p.limit {
            let (_, oldest) = p.order.pop_first().expect("cached bytes belong to entries");
            let gone = p.entries.remove(&oldest).expect("ordered entries exist");
            p.bytes -= gone.bytes;
        }
        p.clock += 1;
        let used = p.clock;
        p.bytes += bytes;
        p.order.insert(used, id);
        p.entries.insert(
            id,
            Entry {
                cached,
                bytes,
                used,
            },
        );
    }
}
impl Default for CodeCache {
    fn default() -> Self {
        Self::new(CODE_CACHE_BYTES)
    }
}

#[derive(Clone, Debug)]
pub struct Context {
    pub chain: Hash,
    pub height: u64,
    pub timestamp: u64,
    pub parent: Hash,
    pub producer: Vec<u8>,
}
impl Context {
    pub fn from_genesis(g: &Genesis) -> Self {
        Self {
            chain: g.chain_id(),
            height: 1,
            timestamp: g.timestamp.saturating_add(g.interval.into()),
            parent: g.block_id(),
            producer: Vec::new(),
        }
    }
}
struct BlockGuard {
    committed: Resources,
    limits: Resources,
    invalid: bool,
}
pub(super) struct Machine {
    pub state: State,
    pub meter: Meter,
    pub context: Context,
    pub transaction: Hash,
    pub ordinal: u64,
    pub events: Vec<Event>,
    /// Code the transaction has prepared, by CodeHash: it paid its code price
    /// and is not charged again (rules §15.7). Held here, not
    /// in the node's cache, so that it stays prepared for the whole transaction.
    prepared: BTreeMap<Hash, Rc<Cached>>,
    block_guard: Option<BlockGuard>,
}

/// A validated module, ready to instantiate, cached by code hash.
struct Cached {
    profile: Profile,
    module: Rc<Module>,
}
/// One running or suspended invocation of a module.
type Instance = Box<interp::Instance>;
/// How an engine run ended: the entry function returned, the guest trapped,
/// or a host import interrupted it (see `Host::outcome` and `Host::pending`).
pub(super) enum Progress {
    Finished,
    Trapped,
    Interrupted,
}

#[derive(Clone)]
pub struct Runtime {
    pub(super) machine: Rc<RefCell<Machine>>,
    cache: CodeCache,
}

/// A deployment whose initializer is the running invocation.
struct Deploying {
    id: Hash,
    checkpoint: usize,
    events: usize,
}
struct Frame {
    instance: Instance,
    deploy: Option<Deploying>,
    checkpoint: usize,
    events: usize,
}
/// A parent frame whose guest is suspended inside a `call` or `deploy` host function.
struct Suspended {
    frame: Frame,
    pending: Pending,
}
enum Step {
    Done(Outcome),
    Suspended(Box<Suspended>),
}

impl Runtime {
    pub fn new(state: State, context: Context) -> Self {
        Self::with_cache(state, context, CodeCache::default())
    }
    /// A runtime that shares its validated programs with every other runtime
    /// given the same cache, such as a node's runtimes for successive blocks.
    pub fn with_cache(state: State, context: Context, cache: CodeCache) -> Self {
        Self {
            machine: Rc::new(RefCell::new(Machine {
                state,
                meter: Meter::new(Resources::ZERO),
                context,
                transaction: [0; 32],
                ordinal: 0,
                events: Vec::new(),
                prepared: BTreeMap::new(),
                block_guard: None,
            })),
            cache,
        }
    }
    pub fn state_root(&self) -> Hash {
        self.machine.borrow().state.root()
    }
    pub fn state_changes(&self) -> Result<Changes, String> {
        self.machine.borrow().state.changes()
    }
    pub fn storage_error(&self) -> Option<String> {
        self.machine.borrow().state.storage_error()
    }
    /// Read-only operational inspection; never charged or callable by a program.
    pub fn record_count(&self) -> usize {
        self.machine.borrow().state.len()
    }
    pub fn manifest(&self, contract: Hash) -> Option<Vec<u8>> {
        self.machine.borrow().state.get(&manifest_key(contract))
    }
    pub fn code(&self, contract: Hash) -> Option<Vec<u8>> {
        self.machine.borrow().state.get(&code_key(contract))
    }
    pub fn storage(&self, contract: Hash, key: Hash) -> Option<Vec<u8>> {
        self.machine
            .borrow()
            .state
            .get(&crate::state::record_key(2, contract, key))
    }
    pub fn clear_cache(&self) {
        self.cache.clear();
    }
    pub fn commit(&self) {
        self.machine.borrow_mut().state.commit();
    }
    pub fn last_resources(&self) -> Resources {
        self.machine.borrow().meter.used()
    }
    /// One atomic debit against every enclosing frame and the block guard.
    /// Frames are checked first: a debit rejected by its frame is not consumed
    /// by the block.
    pub(super) fn charge(&self, r: Resources) -> Result<(), Status> {
        let mut m = self.machine.borrow_mut();
        if m.block_guard.as_ref().is_some_and(|g| g.invalid) {
            return Err(Status::Structure);
        }
        m.meter.check(r).map_err(|e| {
            [
                Status::Compute,
                Status::Access,
                Status::Data,
                Status::Growth,
            ][e.0]
        })?;
        let used = m.meter.used();
        if let Some(g) = &mut m.block_guard {
            for i in 0..4 {
                if g.committed.0[i]
                    .checked_add(used.0[i])
                    .and_then(|v| v.checked_add(r.0[i]))
                    .is_none_or(|v| v > g.limits.0[i])
                {
                    g.invalid = true;
                    return Err(Status::Structure);
                }
            }
        }
        m.meter.charge(r).expect("checked debit");
        Ok(())
    }
    /// The largest compute debit `charge` would accept now: the interpreter
    /// debits dispatch against this allowance and settles it in one charge.
    pub(super) fn compute_headroom(&self) -> u64 {
        let m = self.machine.borrow();
        if m.block_guard.as_ref().is_some_and(|g| g.invalid) {
            return 0;
        }
        let mut n = m.meter.headroom(0);
        if let Some(g) = &m.block_guard {
            let spent = g.committed.0[0].saturating_add(m.meter.used().0[0]);
            n = n.min(g.limits.0[0].saturating_sub(spent));
        }
        n
    }
    /// A storage failure stops execution; the block is rejected, not given a receipt.
    fn stored<T>(&self, value: T) -> Result<T, Status> {
        if self.storage_error().is_some() {
            return Err(Status::Trap);
        }
        Ok(value)
    }
    /// One record read: its key's hash and one S.
    pub(super) fn read_record(&self, key: Hash) -> Result<Option<Value>, Status> {
        self.charge(Resources([price::KEY, 1, 0, 0]))?;
        let value = self.machine.borrow().state.value(&key);
        self.stored(value)
    }
    /// One record read, its key's hash and one S, of which only the length is
    /// needed: the value is not read, whatever its size.
    pub(super) fn read_len(&self, key: Hash) -> Result<Option<u64>, Status> {
        self.charge(Resources([price::KEY, 1, 0, 0]))?;
        let len = self.machine.borrow().state.value_len(&key);
        self.stored(len)
    }
    /// The value of a record whose read is already charged, for copying out.
    pub(super) fn record_value(&self, key: Hash) -> Result<Value, Status> {
        let value = self.machine.borrow().state.value(&key);
        Ok(self.stored(value)?.expect("the charged record exists"))
    }
    /// Old-record read, then one atomic debit for hashing the value into its
    /// leaf (C), the write (S) and gross allocation (G), then the mutation. Only
    /// the old record's length is needed.
    pub(super) fn write_record(&self, key: Hash, value: Option<Vec<u8>>) -> Result<(), Status> {
        let old = self.read_len(key)?;
        let (c, g) = value.as_ref().map_or((0, 0), |v| {
            let n = v.len() as u64;
            (price::leaf(n), growth(old, n))
        });
        self.charge(Resources([c, 1, 0, g]))?;
        self.machine.borrow_mut().state.set(key, value);
        Ok(())
    }
    /// The prepared program of an installed contract. Code this transaction has
    /// not yet prepared pays its code price first, then counts as prepared; on
    /// the node's cache miss the bytes are read for the price and decoded only
    /// after it is paid.
    /// The program a call runs. Unless the transaction has prepared it, its
    /// code price, which the manifest records, is debited before any of its
    /// bytes are read (rules §23).
    fn load(
        &self,
        id: Hash,
        code_hash: Hash,
        length: u64,
        price: u64,
    ) -> Result<Rc<Cached>, Status> {
        let warm = self.machine.borrow().prepared.get(&code_hash).cloned();
        if let Some(cached) = warm {
            return Ok(cached);
        }
        self.charge(Resources::compute(price))?;
        let cached = match self.cache.get(&code_hash) {
            Some(cached) => cached,
            None => {
                let code = self.record_value(code_key(id))?;
                assert_eq!(code.len() as u64, length, "state code length invariant");
                assert_eq!(price::code(&code), price, "state code price invariant");
                assert_eq!(hash("code", &code), code_hash, "state code hash invariant");
                self.prepare(code_hash, &code)
                    .expect("installed profile invariant")
            }
        };
        self.machine
            .borrow_mut()
            .prepared
            .insert(code_hash, cached.clone());
        Ok(cached)
    }
    /// The validated module of `code`, whose CodeHash is `id`: from the cache,
    /// or decoded and validated now.
    fn prepare(&self, id: Hash, code: &[u8]) -> Result<Rc<Cached>, Status> {
        if let Some(cached) = self.cache.get(&id) {
            return Ok(cached);
        }
        let module = decode::decode(code).map_err(|_e| {
            #[cfg(feature = "coverage")]
            super::coverage::rejection(_e.0);
            Status::InvalidModule
        })?;
        let weight = module.weight();
        let cached = Rc::new(Cached {
            profile: module.profile.clone(),
            module: Rc::new(module),
        });
        self.cache.insert(id, cached.clone(), weight);
        Ok(cached)
    }
    fn instantiate(&self, cached: &Cached, host: Host) -> Instance {
        Box::new(interp::Instance::new(cached.module.clone(), host))
    }
    fn rollback(&self, checkpoint: usize, events: usize) {
        let mut m = self.machine.borrow_mut();
        m.state.rollback(checkpoint);
        m.events.truncate(events);
    }
    pub fn execute(&self, tx: &Transaction) -> protocol::Result<Receipt> {
        let encoded = tx.encode();
        if tx.budget.0[2] < encoded.len() as u64 + TRANSACTION_BASE
            || tx.budget.0[2] > protocol::DATA_MAX
        {
            return Err(protocol::Error("invalid transaction envelope"));
        }
        let checkpoint = self.machine.borrow().state.checkpoint();
        {
            let mut m = self.machine.borrow_mut();
            m.meter = Meter::new(tx.budget);
            m.transaction = tx.id(m.context.chain);
            m.ordinal = 0;
            m.events.clear();
            m.prepared.clear();
        }
        let initial = self
            .charge(Resources::data(encoded.len() as u64 + TRANSACTION_BASE))
            .and_then(|()| self.charge(Resources::compute(price::ROOT)));
        let outcome = if let Err(status) = initial {
            (status, Vec::new())
        } else {
            let root = match &tx.payload {
                Payload::Call { target, input } => Pending::Call {
                    target: *target,
                    input: input.clone(),
                    readonly: false,
                },
                Payload::Deploy { code, input } => Pending::Deploy {
                    code: code.clone(),
                    input: input.clone(),
                },
            };
            self.run(root, None)
        };
        let mut m = self.machine.borrow_mut();
        if outcome.0 != Status::Success {
            m.state.rollback(checkpoint);
            m.events.clear();
        }
        if m.state.storage_error().is_some() {
            return Err(protocol::Error("state storage failure"));
        }
        Ok(Receipt {
            transaction: m.transaction,
            status: outcome.0,
            used: m.meter.used(),
            result: outcome.1,
            events: m.events.clone(),
        })
    }
    /// The driver: an explicit stack of suspended parents replaces native recursion.
    fn run(&self, root: Pending, caller: Option<Hash>) -> Outcome {
        #[cfg(feature = "coverage")]
        let kind = |p: &Pending| match p {
            Pending::Call { .. } => "call",
            Pending::Deploy { .. } => "deploy",
        };
        #[cfg(feature = "coverage")]
        let mut kinds = vec![kind(&root)];
        let mut parents: Vec<Frame> = Vec::new();
        let mut step = self.start(root, caller);
        loop {
            match step {
                Step::Done(outcome) => {
                    #[cfg(feature = "coverage")]
                    super::coverage::invocation(
                        kinds.pop().expect("a kind per invocation"),
                        parents.is_empty(),
                        outcome.0,
                    );
                    let Some(mut frame) = parents.pop() else {
                        return outcome;
                    };
                    self.machine.borrow_mut().meter.pop();
                    frame.instance.host_mut().last = Rc::new(outcome.1);
                    let progress = frame.instance.resume(outcome.0 as i32);
                    step = self.drive(frame, progress);
                }
                Step::Suspended(suspended) => {
                    let Suspended { frame, pending } = *suspended;
                    #[cfg(feature = "coverage")]
                    kinds.push(kind(&pending));
                    let caller = Some(frame.instance.host().id);
                    parents.push(frame);
                    step = self.start(pending, caller);
                }
            }
        }
    }
    fn start(&self, pending: Pending, caller: Option<Hash>) -> Step {
        match pending {
            Pending::Call {
                target,
                input,
                readonly,
            } => self.begin_call(target, input, caller, readonly, None),
            Pending::Deploy { code, input } => self.begin_deploy(&code, input, caller),
        }
    }
    fn begin_deploy(&self, code: &[u8], input: Vec<u8>, caller: Option<Hash>) -> Step {
        let checkpoint = self.machine.borrow().state.checkpoint();
        let events = self.machine.borrow().events.len();
        let installed = (|| -> Result<Hash, Status> {
            let id = {
                let mut m = self.machine.borrow_mut();
                let ordinal = m.ordinal;
                m.ordinal = m.ordinal.checked_add(1).ok_or(Status::Structure)?;
                let mut bytes = m.context.chain.to_vec();
                bytes.extend(m.transaction);
                bytes.extend(ordinal.to_le_bytes());
                hash("contract", &bytes)
            };
            self.charge(Resources::compute(price::CONTRACT_ID))?;
            if self.read_len(manifest_key(id))?.is_some() {
                return Err(Status::Exists);
            }
            // The code price and the CodeHash, before validation and before
            // any cache lookup; invalid code keeps the debit.
            let n = code.len() as u64;
            let code_price = price::code(code);
            self.charge(Resources::compute(
                code_price.saturating_add(price::code_hash(n)),
            ))?;
            let code_hash = hash("code", code);
            let cached = self.prepare(code_hash, code)?;
            self.machine.borrow_mut().prepared.insert(code_hash, cached);
            // The manifest records the code price, so that a call can charge it
            // before reading the code (rules §5).
            let mut manifest = code_hash.to_vec();
            manifest.extend((code.len() as u32).to_le_bytes());
            manifest.extend(code_price.to_le_bytes());
            self.write_record(manifest_key(id), Some(manifest))?;
            self.write_record(code_key(id), Some(code.to_vec()))?;
            Ok(id)
        })();
        match installed {
            Err(status) => {
                self.rollback(checkpoint, events);
                Step::Done((status, Vec::new()))
            }
            Ok(id) => self.begin_call(
                id,
                input,
                caller,
                false,
                Some(Deploying {
                    id,
                    checkpoint,
                    events,
                }),
            ),
        }
    }
    fn begin_call(
        &self,
        id: Hash,
        input: Vec<u8>,
        caller: Option<Hash>,
        readonly: bool,
        deploy: Option<Deploying>,
    ) -> Step {
        let checkpoint = self.machine.borrow().state.checkpoint();
        let events = self.machine.borrow().events.len();
        let prepared = (|| -> Result<Rc<Cached>, Status> {
            let manifest = self.read_record(manifest_key(id))?.ok_or(Status::Missing)?;
            assert_eq!(manifest.len(), 44, "state manifest invariant");
            let code_hash: Hash = manifest[..32].try_into().unwrap();
            let length = u64::from(u32::from_le_bytes(manifest[32..36].try_into().unwrap()));
            let code_price = u64::from_le_bytes(manifest[36..].try_into().unwrap());
            // The code record's read: its key's hash and one S, whether or not
            // the node holds the program ready.
            self.charge(Resources([price::KEY, 1, 0, 0]))?;
            let cached = self.load(id, code_hash, length, code_price)?;
            self.charge(Resources::compute(cached.profile.preparation()))?;
            let entry = cached.profile.functions[cached.profile.invoke as usize];
            let (debit, ..) = price::entry(entry.slots(), 0, 0);
            self.charge(Resources::compute(debit))?;
            Ok(cached)
        })();
        let cached = match prepared {
            Ok(cached) => cached,
            Err(status) => {
                return Step::Done(self.conclude(checkpoint, events, deploy, (status, Vec::new())));
            }
        };
        let host = Host::new(self.clone(), id, caller, readonly, input);
        let mut instance = self.instantiate(&cached, host);
        let progress = instance.start();
        let frame = Frame {
            instance,
            deploy,
            checkpoint,
            events,
        };
        self.drive(frame, progress)
    }
    /// Turns how the engine stopped into an outcome, or parks the frame while
    /// the nested invocation it requested runs.
    fn drive(&self, mut frame: Frame, progress: Progress) -> Step {
        let host = frame.instance.host_mut();
        let outcome = match progress {
            Progress::Finished => host.outcome.take().unwrap_or((Status::Success, Vec::new())),
            Progress::Trapped => host.outcome.take().unwrap_or((Status::Trap, Vec::new())),
            Progress::Interrupted => {
                if let Some(outcome) = host.outcome.take() {
                    outcome
                } else if let Some(pending) = host.pending.take() {
                    return Step::Suspended(Box::new(Suspended { frame, pending }));
                } else {
                    panic!("host interrupted the guest without a specified outcome");
                }
            }
        };
        let Frame {
            instance,
            deploy,
            checkpoint,
            events,
        } = frame;
        drop(instance);
        Step::Done(self.conclude(checkpoint, events, deploy, outcome))
    }
    /// Rolls back a failed invocation and finishes a deployment around it.
    fn conclude(
        &self,
        checkpoint: usize,
        events: usize,
        deploy: Option<Deploying>,
        outcome: Outcome,
    ) -> Outcome {
        if outcome.0 != Status::Success {
            self.rollback(checkpoint, events);
        }
        let Some(deploy) = deploy else {
            return outcome;
        };
        if outcome.0 != Status::Success {
            self.rollback(deploy.checkpoint, deploy.events);
            return outcome;
        }
        match self.charge(Resources([price::IDENTIFIER, 0, 32, 0])) {
            Ok(()) => (Status::Success, deploy.id.to_vec()),
            Err(status) => {
                self.rollback(deploy.checkpoint, deploy.events);
                (status, Vec::new())
            }
        }
    }
    /// Assemble under the same aggregate limits as validation, then restore the
    /// journal and context. Only validation of a mined block commits state.
    pub fn build_candidate(
        &self,
        g: &Genesis,
        history: &[crate::protocol::Header],
        transactions: Vec<Transaction>,
        timestamp: u64,
        votes: [u8; 4],
        producer: Vec<u8>,
    ) -> protocol::Result<crate::protocol::Block> {
        self.build_candidate_at(
            g,
            &crate::consensus::Ancestry::of(history),
            crate::consensus::active_limits(g, history),
            transactions,
            timestamp,
            votes,
            producer,
        )
    }
    /// `build_candidate` on a branch given by its ancestry and its active
    /// limits for the next block, which MUST equal `consensus::active_limits`
    /// of the branch. Every transaction must fit.
    #[allow(clippy::too_many_arguments)]
    pub fn build_candidate_at(
        &self,
        g: &Genesis,
        ancestry: &crate::consensus::Ancestry,
        limits: Resources,
        transactions: Vec<Transaction>,
        timestamp: u64,
        votes: [u8; 4],
        producer: Vec<u8>,
    ) -> protocol::Result<crate::protocol::Block> {
        let mut candidate = self.candidate(g, ancestry, limits, timestamp, votes, producer)?;
        for tx in &transactions {
            if candidate.add(tx)?.is_none() {
                return Err(protocol::Error("executed block resource limits"));
            }
        }
        candidate.finish()
    }
    /// A block to assemble one transaction at a time on this runtime's state,
    /// under the same aggregate limits as validation. The state and context
    /// are restored when the candidate is dropped; only validation of a mined
    /// block commits state.
    pub fn candidate<'a>(
        &'a self,
        g: &'a Genesis,
        ancestry: &crate::consensus::Ancestry,
        limits: Resources,
        timestamp: u64,
        votes: [u8; 4],
        producer: Vec<u8>,
    ) -> protocol::Result<Candidate<'a>> {
        use crate::protocol::Error;
        if votes.iter().any(|v| *v > 2) {
            return Err(Error("header fields"));
        }
        // The body frames producer data with a 32-bit length.
        if u32::try_from(producer.len()).is_err() {
            return Err(Error("producer data length"));
        }
        let parent_root = ancestry
            .parent
            .as_ref()
            .map_or_else(|| State::default().root(), |h| h.state_root);
        if self.state_root() != parent_root {
            return Err(Error("parent state mismatch"));
        }
        let used = Resources([0, 0, BLOCK_BASE + producer.len() as u64, 0]);
        if !used.fits(limits) {
            return Err(Error("block resource limits"));
        }
        let start = self.machine.borrow().state.checkpoint();
        let previous = self.machine.borrow().context.clone();
        let parent = ancestry.parent_id(g);
        {
            let mut m = self.machine.borrow_mut();
            m.context = Context {
                chain: g.chain_id(),
                height: ancestry.height + 1,
                timestamp,
                parent,
                producer: producer.clone(),
            };
            m.block_guard = Some(BlockGuard {
                committed: used,
                limits,
                invalid: false,
            });
        }
        Ok(Candidate {
            runtime: self,
            genesis: g,
            ancestry: ancestry.clone(),
            limits,
            timestamp,
            votes,
            producer,
            start,
            previous,
            used,
            marks: Vec::new(),
            transactions: Vec::new(),
            receipts: Vec::new(),
            discarded: 0,
            discard_limit: u64::MAX,
            attempts: Vec::new(),
        })
    }
    pub fn validate_and_apply_block(
        &self,
        g: &Genesis,
        history: &[crate::protocol::Header],
        block: &crate::protocol::Block,
    ) -> protocol::Result<()> {
        let limits = crate::consensus::active_limits(g, history);
        self.validate_and_apply_block_with(g, history, block, limits)
    }
    /// Block validation with active limits supplied by the caller; they MUST
    /// equal `consensus::active_limits(g, history)`.
    pub fn validate_and_apply_block_with(
        &self,
        g: &Genesis,
        history: &[crate::protocol::Header],
        block: &crate::protocol::Block,
        limits: Resources,
    ) -> protocol::Result<()> {
        self.validate_and_apply_block_at(g, &crate::consensus::Ancestry::of(history), block, limits)
    }
    /// Block validation on a branch given by its ancestry, with its active
    /// limits for the next block.
    pub fn validate_and_apply_block_at(
        &self,
        g: &Genesis,
        ancestry: &crate::consensus::Ancestry,
        block: &crate::protocol::Block,
        limits: Resources,
    ) -> protocol::Result<()> {
        crate::consensus::validate_header_at(g, ancestry, &block.header, limits)?;
        if producer_hash(&block.producer) != block.header.producer_hash {
            return Err(protocol::Error("producer data commitment"));
        }
        if self.storage_error().is_some() {
            return Err(protocol::Error("state storage failure"));
        }
        let parent_root = ancestry
            .parent
            .as_ref()
            .map_or_else(|| State::default().root(), |h| h.state_root);
        if self.state_root() != parent_root {
            return Err(protocol::Error("parent state mismatch"));
        }
        let checkpoint = self.machine.borrow().state.checkpoint();
        let previous = self.machine.borrow().context.clone();
        self.machine.borrow_mut().context = Context {
            chain: g.chain_id(),
            height: block.header.height,
            timestamp: block.header.timestamp,
            parent: block.header.parent,
            producer: block.producer.clone(),
        };
        let result = (|| {
            if block.transactions.len() != block.receipts.len() {
                return Err(protocol::Error("receipt count"));
            }
            if block.encode().len() as u64 > limits.0[2] {
                return Err(protocol::Error("serialized block data limit"));
            }
            let txroot = crate::hash::vector_root(
                "transactions",
                &block
                    .transactions
                    .iter()
                    .map(Encode::encode)
                    .collect::<Vec<_>>(),
            );
            if txroot != block.header.transactions_root {
                return Err(protocol::Error("transaction root"));
            }
            let mut used = Resources([0, 0, BLOCK_BASE + block.producer.len() as u64, 0]);
            let mut receipts = Vec::new();
            // A valid block uses exactly what its header declares, so the
            // guard stops at the declared totals as well as at the capacity:
            // a block that would pass them is invalid either way, and is found
            // so without running the rest of it.
            let declared = Resources(std::array::from_fn(|i| {
                limits.0[i].min(block.header.used.0[i])
            }));
            self.machine.borrow_mut().block_guard = Some(BlockGuard {
                committed: used,
                limits: declared,
                invalid: false,
            });
            for (tx, expected) in block.transactions.iter().zip(&block.receipts) {
                let receipt = self.execute(tx)?;
                if self.machine.borrow().block_guard.as_ref().unwrap().invalid {
                    return Err(protocol::Error("executed block resource limits"));
                }
                if receipt != *expected {
                    return Err(protocol::Error("execution commitment mismatch"));
                }
                for i in 0..4 {
                    used.0[i] = used.0[i]
                        .checked_add(receipt.used.0[i])
                        .ok_or(protocol::Error("resource total overflow"))?;
                }
                if !used.fits(declared) {
                    return Err(protocol::Error("executed block resource limits"));
                }
                self.machine
                    .borrow_mut()
                    .block_guard
                    .as_mut()
                    .unwrap()
                    .committed = used;
                receipts.push(receipt);
            }
            let root = crate::hash::vector_root(
                "receipts",
                &receipts.iter().map(Encode::encode).collect::<Vec<_>>(),
            );
            if receipts != block.receipts
                || root != block.header.receipts_root
                || self.state_root() != block.header.state_root
                || used != block.header.used
            {
                return Err(protocol::Error("execution commitment mismatch"));
            }
            Ok(())
        })();
        self.machine.borrow_mut().block_guard = None;
        if result.is_err() {
            self.machine.borrow_mut().state.rollback(checkpoint);
            self.machine.borrow_mut().context = previous;
        } else {
            self.commit();
        }
        result
    }
}

/// One `Candidate::add`: the transaction's id, what its execution used,
/// whether the block had room for it, whether the block still holds it (not
/// once taken back out), and whether the block held no other transaction when
/// it was tried, so that not fitting then means not fitting any block on this
/// parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub id: Hash,
    pub used: Resources,
    pub fit: bool,
    pub kept: bool,
    pub alone: bool,
}

/// A block being assembled (`Runtime::candidate`).
pub struct Candidate<'a> {
    runtime: &'a Runtime,
    genesis: &'a Genesis,
    ancestry: crate::consensus::Ancestry,
    limits: Resources,
    timestamp: u64,
    votes: [u8; 4],
    producer: Vec<u8>,
    /// The state journal before the candidate, and the context to restore.
    start: usize,
    previous: Context,
    used: Resources,
    /// For each transaction added, the journal and the block's use before it.
    marks: Vec<(usize, Resources)>,
    transactions: Vec<Transaction>,
    receipts: Vec<Receipt>,
    /// Compute run by attempts the block did not keep and by transactions
    /// taken back out, and the most of it the candidate allows.
    discarded: u64,
    discard_limit: u64,
    attempts: Vec<Attempt>,
}
impl Candidate<'_> {
    /// Adds a transaction when the block still fits with it, and returns its
    /// receipt; `None` when it does not fit, and then nothing of it stays.
    /// Once the allowance for discarded work is spent (`limit_discarded`), it
    /// returns `None` without running anything. An invalid envelope or a
    /// storage failure is an error.
    pub fn add(&mut self, tx: &Transaction) -> protocol::Result<Option<&Receipt>> {
        if self.discarded >= self.discard_limit {
            return Ok(None);
        }
        let alone = self.transactions.is_empty();
        let id = tx.id(self.genesis.chain_id());
        let checkpoint = self.runtime.machine.borrow().state.checkpoint();
        let receipt = self.runtime.execute(tx)?;
        let invalid = self
            .runtime
            .machine
            .borrow()
            .block_guard
            .as_ref()
            .is_some_and(|g| g.invalid);
        let mut used = self.used;
        let mut fits = !invalid;
        for i in 0..4 {
            match used.0[i].checked_add(receipt.used.0[i]) {
                Some(v) => used.0[i] = v,
                None => fits = false,
            }
        }
        let fits = fits && used.fits(self.limits);
        let mut m = self.runtime.machine.borrow_mut();
        let guard = m
            .block_guard
            .as_mut()
            .expect("a candidate holds the block guard");
        if !fits {
            guard.invalid = false;
            guard.committed = self.used;
            m.state.rollback(checkpoint);
            drop(m);
            self.discarded = self.discarded.saturating_add(receipt.used.0[0]);
            self.attempts.push(Attempt {
                id,
                used: receipt.used,
                fit: false,
                kept: false,
                alone,
            });
            return Ok(None);
        }
        guard.committed = used;
        drop(m);
        self.attempts.push(Attempt {
            id,
            used: receipt.used,
            fit: true,
            kept: true,
            alone,
        });
        self.marks.push((checkpoint, self.used));
        self.used = used;
        self.transactions.push(tx.clone());
        self.receipts.push(receipt);
        Ok(self.receipts.last())
    }
    /// Takes the last transaction added back out, with everything it did.
    pub fn undo(&mut self) -> bool {
        let Some((checkpoint, used)) = self.marks.pop() else {
            return false;
        };
        let mut m = self.runtime.machine.borrow_mut();
        m.state.rollback(checkpoint);
        m.block_guard
            .as_mut()
            .expect("a candidate holds the block guard")
            .committed = used;
        self.used = used;
        self.transactions.pop();
        if let Some(receipt) = self.receipts.pop() {
            self.discarded = self.discarded.saturating_add(receipt.used.0[0]);
        }
        if let Some(attempt) = self.attempts.iter_mut().rev().find(|a| a.kept) {
            attempt.kept = false;
        }
        true
    }
    /// Stops running transactions once the compute of attempts the block did
    /// not keep, and of transactions taken back out, reaches `compute`: a
    /// node's bound on the work one candidate may throw away. The attempt that
    /// reaches it still finishes, so at most `compute` plus one block's worth
    /// is discarded.
    pub fn limit_discarded(&mut self, compute: u64) {
        self.discard_limit = compute;
    }
    /// Whether the allowance for discarded work is spent, so that `add` runs
    /// nothing more.
    pub fn spent(&self) -> bool {
        self.discarded >= self.discard_limit
    }
    /// Every transaction `add` ran, in order.
    pub fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }
    /// The transactions added so far.
    pub fn transactions(&self) -> &[Transaction] {
        &self.transactions
    }
    /// The block with the transactions added, complete except for its nonce.
    pub fn finish(mut self) -> protocol::Result<crate::protocol::Block> {
        use crate::{
            consensus,
            protocol::{Block, Error, Header},
        };
        if self.runtime.storage_error().is_some() {
            return Err(Error("state storage failure"));
        }
        let transactions = std::mem::take(&mut self.transactions);
        let receipts = std::mem::take(&mut self.receipts);
        let (transactions_root, receipts_root) = consensus::commitments(&transactions, &receipts);
        let g = self.genesis;
        let block = Block {
            header: Header {
                chain_id: g.chain_id(),
                height: self.ancestry.height + 1,
                parent: self.ancestry.parent_id(g),
                timestamp: self.timestamp,
                target: consensus::expected_target_at(g, &self.ancestry),
                transactions_root,
                receipts_root,
                state_root: self.runtime.state_root(),
                used: self.used,
                votes: self.votes,
                producer_hash: producer_hash(&self.producer),
                nonce: 0,
            },
            producer: std::mem::take(&mut self.producer),
            transactions,
            receipts,
        };
        if block.encode().len() as u64 > self.limits.0[2] {
            return Err(Error("serialized block data limit"));
        }
        Ok(block)
    }
}
impl Drop for Candidate<'_> {
    fn drop(&mut self) {
        let mut m = self.runtime.machine.borrow_mut();
        m.block_guard = None;
        m.state.rollback(self.start);
        m.context = self.previous.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(functions: usize) -> (Rc<Cached>, u64) {
        let body = "(func)".repeat(functions);
        let code = wat::parse_str(format!("(module {body} (func (export \"invoke\")))")).unwrap();
        let module = decode::decode(&code).unwrap();
        let weight = module.weight();
        let cached = Rc::new(Cached {
            profile: module.profile.clone(),
            module: Rc::new(module),
        });
        (cached, weight)
    }

    #[test]
    fn the_cache_counts_decoded_programs_and_evicts_the_least_recently_used() {
        let (a, b, c) = (program(10), program(20), program(30));
        assert!(a.1 < b.1 && b.1 < c.1, "decoded weight grows with the code");
        let cache = CodeCache::new(a.1 + c.1);
        cache.insert([1; 32], a.0.clone(), a.1);
        cache.insert([2; 32], b.0.clone(), b.1);
        // A used again, so B is the least recently used and leaves for C.
        assert!(cache.get(&[1; 32]).is_some());
        cache.insert([3; 32], c.0.clone(), c.1);
        assert!(cache.get(&[2; 32]).is_none());
        assert!(cache.get(&[1; 32]).is_some() && cache.get(&[3; 32]).is_some());
        // Inserting a program again replaces it, counted once.
        cache.insert([3; 32], c.0.clone(), c.1);
        assert!(cache.get(&[1; 32]).is_some());
        // A program larger than the whole cache is not kept.
        let tiny = CodeCache::new(a.1 - 1);
        tiny.insert([1; 32], a.0, a.1);
        assert!(tiny.get(&[1; 32]).is_none());
    }
}
