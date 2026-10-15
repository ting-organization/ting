//! Mining, split where a miner's fork of this node takes over.
//!
//! The node does the chain work: it assembles a *template* (a complete
//! candidate block whose header lacks only the nonce), accepts a *solution*
//! (a nonce for a template) and validates, commits and relays the block.
//! Two things are policy or hardware, and a fork replaces them by
//! implementing a trait and passing its own implementation to
//! [`crate::daemon::run_with`]:
//!
//! - [`Selector`], which transactions a template includes. The reference
//!   [`FirstIn`] takes them in arrival order; a fork that knows what the
//!   programs it cares about pay can build candidates and read their receipts.
//! - [`Search`], finding the nonce. The reference [`Cpu`] runs the plain
//!   loop on a few threads, hashing each attempt from a SHA-256 midstate; a
//!   fork drives a GPU or an ASIC.
//!
//! The built-in [`Miner`] is the loop between them: it fetches a template,
//! lets the search work on it for a slice, submits a solution and starts over
//! with a fresh template. It talks to the chain through the same `template`
//! and `solution` requests the control channel offers, so the chain thread
//! stays the only owner of state and external tools can do by hand what the
//! miner does in a loop.

use crate::{Result, chain::Handle, now};

/// The answer to a solution for a template the node no longer holds, as after
/// the tip moved.
pub const EXPIRED: &str = "unknown or expired template";
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};
use ting_core::{
    hash::hex,
    protocol::{Encode, HEADER_LEN, Header, Transaction, target_bytes},
    vm::Candidate,
};

/// Where the nonce sits in the 294-byte header: its last 16 bytes, little-endian
/// (wiki: data/blocks). Searching means rewriting these bytes and
/// hashing the header until its id is at or below the target.
pub const NONCE_OFFSET: usize = HEADER_LEN - 16;
pub const NONCE_BYTES: usize = 16;

/// Nonces tried per thread before the built-in miner asks for a fresh template,
/// so a new tip, new transactions and the clock reach the header it works on.
const SLICE: u64 = 1 << 24;

// ---- Selection -----------------------------------------------------------------

/// Chooses the transactions of a template from the pool.
///
/// `block` is a candidate on the tip: `add` executes a transaction into it and
/// returns its receipt, or `None` when the block would not fit it, and `undo`
/// takes the last one back out. A selector can judge transactions by their
/// receipts and events, such as what the programs it knows pay the producer.
/// The node itself knows nothing of fees or rewards.
pub trait Selector: Send {
    fn select(&mut self, pool: &[Transaction], block: &mut Candidate<'_>);
    fn describe(&self) -> Value;
}

/// The reference policy: first in, first served. Each transaction in arrival
/// order goes in when the block still fits it, so a template costs one
/// execution per pooled transaction.
#[derive(Default)]
pub struct FirstIn;
impl Selector for FirstIn {
    fn select(&mut self, pool: &[Transaction], block: &mut Candidate<'_>) {
        for tx in pool {
            // A transaction the block does not fit is left out.
            let _ = block.add(tx);
        }
    }
    fn describe(&self) -> Value {
        json!({"kind":"first-in"})
    }
}

// ---- Search --------------------------------------------------------------------

/// Finds a nonce for a header. `attempts` bounds one call: the miner calls
/// again with a fresh template, so a search never has to run on a stale one.
/// `start` lets successive calls cover new nonces; `stop` ends the call early;
/// `tried` counts attempts for the miner's status.
pub trait Search: Send + Sync {
    fn search(
        &self,
        header: &Header,
        start: u128,
        attempts: u64,
        stop: &AtomicBool,
        tried: &AtomicU64,
    ) -> Option<u128>;
    fn describe(&self) -> Value;
}

/// The reference search: the plain loop on `threads` threads with disjoint
/// nonce ranges.
pub struct Cpu {
    pub threads: usize,
}
impl Cpu {
    pub fn new(threads: u32) -> Self {
        Self {
            threads: (threads as usize).clamp(1, 64),
        }
    }
}
impl Search for Cpu {
    fn search(
        &self,
        header: &Header,
        start: u128,
        attempts: u64,
        stop: &AtomicBool,
        tried: &AtomicU64,
    ) -> Option<u128> {
        search_parallel(header, start, attempts, self.threads, stop, tried)
    }
    fn describe(&self) -> Value {
        json!({"kind":"cpu","threads":self.threads})
    }
}

/// Tries `attempts` nonces from `start` on one thread; `None` when the range is
/// exhausted or `stop` is raised. Counts every attempt into `tried`. Finds the
/// nonce `consensus::valid_pow` accepts first in the range.
pub fn search(
    header: &Header,
    start: u128,
    attempts: u64,
    stop: &AtomicBool,
    tried: &AtomicU64,
) -> Option<u128> {
    match Midstate::new(header) {
        Some(midstate) => midstate.search(start, attempts, stop, tried),
        None => unreachable_target(attempts, stop, tried),
    }
}

/// The same search spread over `threads` disjoint nonce ranges; the first
/// solution stops the others.
pub fn search_parallel(
    header: &Header,
    start: u128,
    attempts: u64,
    threads: usize,
    stop: &AtomicBool,
    tried: &AtomicU64,
) -> Option<u128> {
    let threads = threads.max(1);
    if threads == 1 {
        return search(header, start, attempts, stop, tried);
    }
    let Some(midstate) = Midstate::new(header) else {
        return unreachable_target(attempts.wrapping_mul(threads as u64), stop, tried);
    };
    let found = Mutex::new(None);
    let local_stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for i in 0..threads {
            let midstate = &midstate;
            let found = &found;
            let local_stop = &local_stop;
            scope.spawn(move || {
                let range_start = start.wrapping_add(u128::from(attempts) * i as u128);
                let mut budget = 0;
                while budget < attempts {
                    if stop.load(Ordering::Relaxed) || local_stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let step = SLICE.min(attempts - budget);
                    if let Some(nonce) = midstate.search(
                        range_start.wrapping_add(u128::from(budget)),
                        step,
                        local_stop,
                        tried,
                    ) {
                        *found.lock().unwrap() = Some(nonce);
                        local_stop.store(true, Ordering::Relaxed);
                        return;
                    }
                    budget += step;
                }
            });
        }
    });
    found.into_inner().unwrap()
}

/// A search under a target no id meets (zero, or wider than 256 bits, both of
/// which `valid_pow` refuses): every attempt fails, so none is hashed.
fn unreachable_target(attempts: u64, stop: &AtomicBool, tried: &AtomicU64) -> Option<u128> {
    if attempts > 0 && !stop.load(Ordering::Relaxed) {
        tried.fetch_add(attempts, Ordering::Relaxed);
    }
    None
}

// ---- Midstate ------------------------------------------------------------------

/// A header's id is SHA-256 over `"Ting/" || u16 LE tag length || "block"`
/// and the header bytes (`ting_core::hash`), padded the SHA-256 way: `0x80`,
/// zeros, and the message length in bits as a big-endian u64, up to whole
/// 64-byte blocks.
const PREFIX: [u8; 12] = *b"Ting/\x05\x00block";
const MESSAGE_LEN: usize = PREFIX.len() + HEADER_LEN;
const PADDED_LEN: usize = (MESSAGE_LEN + 9).div_ceil(64) * 64;
/// The blocks wholly before the nonce are the same for every attempt; only the
/// tail, from the block holding the nonce's first byte, changes.
const FIXED_LEN: usize = (PREFIX.len() + NONCE_OFFSET) / 64 * 64;
const TAIL_LEN: usize = PADDED_LEN - FIXED_LEN;
const TAIL_NONCE: usize = PREFIX.len() + NONCE_OFFSET - FIXED_LEN;

/// A header hashed up to its nonce: an attempt writes the nonce into a copy of
/// the tail and compresses only the tail blocks, without allocating.
struct Midstate {
    /// The SHA-256 state after the fixed blocks.
    state: [u32; 8],
    /// The padded tail, with the header's own nonce until one is written.
    tail: [u8; TAIL_LEN],
    /// The target as eight big-endian words: an id is at or below the target
    /// exactly when its state words, compared in order, are.
    target: [u32; 8],
}

impl Midstate {
    /// `None` when no id can meet the target: `valid_pow` refuses a zero target
    /// and one wider than 256 bits.
    fn new(header: &Header) -> Option<Self> {
        if !(1..=256).contains(&header.target.bits()) {
            return None;
        }
        let mut message = [0u8; PADDED_LEN];
        message[..PREFIX.len()].copy_from_slice(&PREFIX);
        message[PREFIX.len()..MESSAGE_LEN].copy_from_slice(&header.encode());
        message[MESSAGE_LEN] = 0x80;
        message[PADDED_LEN - 8..].copy_from_slice(&(MESSAGE_LEN as u64 * 8).to_be_bytes());
        let mut state = IV;
        for block in message[..FIXED_LEN].as_chunks::<64>().0 {
            compress(&mut state, block);
        }
        let target = target_bytes(&header.target);
        Some(Self {
            state,
            tail: message[FIXED_LEN..].try_into().expect("tail length"),
            target: std::array::from_fn(|i| u32::from_be_bytes(target.as_chunks::<4>().0[i])),
        })
    }

    /// The id of the header with `nonce`, as the final SHA-256 state. The nonce
    /// is written into `tail`, a copy of [`Self::tail`].
    fn id(&self, tail: &mut [u8; TAIL_LEN], nonce: u128) -> [u32; 8] {
        tail[TAIL_NONCE..TAIL_NONCE + NONCE_BYTES].copy_from_slice(&nonce.to_le_bytes());
        let mut state = self.state;
        for block in tail.as_chunks::<64>().0 {
            compress(&mut state, block);
        }
        state
    }

    /// [`search`] on this header.
    fn search(
        &self,
        start: u128,
        attempts: u64,
        stop: &AtomicBool,
        tried: &AtomicU64,
    ) -> Option<u128> {
        let mut tail = self.tail;
        for offset in 0..attempts {
            if offset.is_multiple_of(1024) && stop.load(Ordering::Relaxed) {
                tried.fetch_add(offset, Ordering::Relaxed);
                return None;
            }
            let nonce = start.wrapping_add(u128::from(offset));
            if self.id(&mut tail, nonce) <= self.target {
                tried.fetch_add(offset + 1, Ordering::Relaxed);
                return Some(nonce);
            }
        }
        tried.fetch_add(attempts, Ordering::Relaxed);
        None
    }
}

/// The SHA-256 initial state (FIPS 180-4, 5.3.3).
const IV: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// The SHA-256 compression of one 64-byte block into `state`, on the
/// processor's SHA instructions where it has them.
fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    sha2::compress256(
        state,
        std::slice::from_ref(sha2::digest::generic_array::GenericArray::from_slice(block)),
    );
}

// ---- The built-in miner ----------------------------------------------------------

/// What the built-in miner reports.
#[derive(Default)]
struct Stats {
    started: u64,
    templates: u64,
    attempts: AtomicU64,
    blocks: u64,
    height: u64,
    last_error: Option<String>,
}

/// The loop between templates and a [`Search`]: fetch, search a slice, submit.
pub struct Miner {
    stop: Arc<AtomicBool>,
    stats: Arc<Mutex<Stats>>,
    search: Arc<dyn Search>,
    spacing: u64,
    worker: Option<JoinHandle<()>>,
}

impl Miner {
    /// `spacing` is the least number of seconds between the miner's own
    /// blocks: zero mines as fast as the target allows, which on an easy
    /// development chain is faster than anyone wants.
    pub fn start(handle: Handle, search: Arc<dyn Search>, spacing: u64) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Mutex::new(Stats {
            started: now(),
            ..Default::default()
        }));
        let worker = {
            let stop = stop.clone();
            let stats = stats.clone();
            let search = search.clone();
            std::thread::Builder::new()
                .name("ting-miner".into())
                .spawn(move || run(handle, search, spacing, stop, stats))
                .expect("miner thread")
        };
        Self {
            stop,
            stats,
            search,
            spacing,
            worker: Some(worker),
        }
    }
    pub fn status(&self) -> Value {
        let s = self.stats.lock().unwrap();
        json!({"running":true,"search":self.search.describe(),"spacing":self.spacing,"since":s.started,"templates":s.templates,
            "attempts":s.attempts.load(Ordering::Relaxed),"blocks":s.blocks,"height":s.height,"last_error":s.last_error})
    }
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(
    handle: Handle,
    search: Arc<dyn Search>,
    spacing: u64,
    stop: Arc<AtomicBool>,
    stats: Arc<Mutex<Stats>>,
) {
    let attempts = AtomicU64::new(0);
    let mut nonce_start: u128 = 0;
    let mut last_block = 0u64;
    let events = handle.events();
    while !stop.load(Ordering::Relaxed) {
        if spacing > 0 && now() < last_block.saturating_add(spacing) {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        // Far behind its best header, the node's tip is about to be replaced
        // and a block on it with it: the miner waits until it catches up.
        let (height, best) = {
            let event = events.borrow();
            (event.height, event.best)
        };
        if best > height + 1 {
            stats.lock().unwrap().last_error = Some(format!(
                "waiting: the node is {} blocks behind its best header",
                best - height
            ));
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        let outcome = (|| -> Result<Option<Value>> {
            let template = handle.call_blocking("template", json!({}))?;
            let header = Header::decode(&crate::unhex(
                template["header_hex"].as_str().ok_or("invalid template")?,
                HEADER_LEN,
            )?)
            .map_err(|e| e.to_string())?;
            {
                let mut s = stats.lock().unwrap();
                s.templates += 1;
                s.height = header.height;
            }
            // A fresh template per slice: the timestamp moves on and new
            // transactions get in, while a solved one is handed back at once.
            let found = search.search(&header, nonce_start, SLICE, &stop, &attempts);
            nonce_start = nonce_start.wrapping_add(u128::from(SLICE) * 64);
            stats
                .lock()
                .unwrap()
                .attempts
                .store(attempts.load(Ordering::Relaxed), Ordering::Relaxed);
            match found {
                Some(nonce) => Ok(Some(handle.call_blocking(
                    "solution",
                    json!({"template":template["id"],"nonce":nonce.to_string()}),
                )?)),
                None => Ok(None),
            }
        })();
        let mut s = stats.lock().unwrap();
        match outcome {
            Ok(Some(imported)) => {
                if imported["canonical"] == true {
                    s.blocks += 1;
                    last_block = now();
                }
                s.last_error = None;
            }
            Ok(None) => s.last_error = None,
            Err(error) => {
                // A stale template or a busy node: note it and try again shortly.
                s.last_error = Some(error);
                drop(s);
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

/// The parts of a template a search needs next to the header bytes.
pub fn describe(header: &Header) -> Value {
    json!({"nonce_offset":NONCE_OFFSET,"nonce_bytes":NONCE_BYTES,"nonce_endian":"little",
        "target_hex":hex(&target_bytes(&header.target))})
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_bigint::BigUint;
    use ting_core::{hash::hash, meter::Resources};

    #[test]
    fn the_midstate_id_is_the_header_id() {
        for seed in 0..8u8 {
            let bytes = |field: u8| hash("midstate-test", &[seed, field]);
            let mut header = Header {
                chain_id: bytes(0),
                height: u64::from(seed) << 40,
                parent: bytes(1),
                timestamp: 1_800_000_000 + u64::from(seed),
                // Targets of several widths; the target is part of the header.
                target: BigUint::from_bytes_be(&bytes(2)[..4 + usize::from(seed) * 4]),
                transactions_root: bytes(3),
                receipts_root: bytes(4),
                state_root: bytes(5),
                used: Resources([u64::from(seed), 2, 3, u64::MAX]),
                votes: [seed % 3, 0, 1, 2],
                producer_hash: bytes(6),
                nonce: 0,
            };
            let midstate = Midstate::new(&header).unwrap();
            let mut tail = midstate.tail;
            let random =
                (0..64).map(|i| u128::from_le_bytes(bytes(100 + i)[..16].try_into().unwrap()));
            let edges = [
                0,
                1,
                0xffff,
                (1 << 64) - 1,
                1 << 64,
                u128::MAX >> 1,
                u128::MAX,
            ];
            for nonce in random.chain(edges) {
                header.nonce = nonce;
                let id: Vec<u8> = midstate
                    .id(&mut tail, nonce)
                    .iter()
                    .flat_map(|w| w.to_be_bytes())
                    .collect();
                assert_eq!(id, header.id(), "seed {seed} nonce {nonce}");
            }
        }
    }
}
