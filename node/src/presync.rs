//! Checking a header chain that does not yet show enough work, without keeping
//! it (wiki: node/sync). Headers are validated as they stream past and only a
//! salted bit every `PERIOD` headers is kept, so a cheap chain costs memory in
//! proportion to elapsed time, not to its length. Once the chain reaches the
//! work threshold, the same headers are fetched again from where it forked,
//! matched against the bits, and only then indexed.
use crate::chain::INVALID;
use num_bigint::BigUint;
use std::collections::BTreeMap;
use ting_core::{
    consensus::{self, Ancestry, Tally},
    hash::{Hash, hash},
    meter::Resources,
    protocol::{Genesis, Header},
};

/// Headers between two kept bits.
pub const PERIOD: u64 = 600;
/// Headers held while re-fetching before any is indexed: 24 bits matched, so
/// a peer that swaps in another chain is caught before it costs memory.
pub const BUFFER: usize = 14_400;

/// What validating the next header of a branch needs, carried along as the
/// branch streams past: its ancestry, active limits and work, and the vote
/// tallies of the epochs that may still be counted.
#[derive(Clone)]
pub struct Walk {
    /// The chain's seed, whose heights take its blocks alone.
    pub seed: &'static [Hash],
    pub ancestry: Ancestry,
    pub limits: Resources,
    pub work: BigUint,
    pub tallies: BTreeMap<u64, Tally>,
}
impl Walk {
    /// Checks the next header and moves past it.
    pub fn step(&mut self, genesis: &Genesis, header: &Header) -> Result<(), String> {
        crate::seed::check(self.seed, header)?;
        consensus::validate_header_at(genesis, &self.ancestry, header, self.limits)
            .map_err(|e| format!("{INVALID}header: {e}"))?;
        let e = u64::from(genesis.vote_epoch);
        self.work += consensus::work(&header.target);
        self.tallies
            .entry((header.height - 1) / e)
            .or_default()
            .add(header);
        self.ancestry.recent.insert(0, header.timestamp);
        self.ancestry.recent.truncate(11);
        self.ancestry.height = header.height;
        self.ancestry.parent = Some(header.clone());
        // The next block opens epoch `height / e`: from epoch 3 on, its limits
        // follow the votes of the epoch three before.
        let height = header.height;
        if height.is_multiple_of(e) && height / e >= 3 {
            let source = height / e - 3;
            let tally = self.tallies.get(&source).cloned().unwrap_or_default();
            self.limits = consensus::next_limits_tally(genesis, self.limits, &tally);
            self.tallies.retain(|epoch, _| *epoch > source);
        }
        Ok(())
    }
}

/// A bit of a header id under a secret salt.
fn bit(salt: &[u8; 32], id: &Hash) -> bool {
    hash("presync", &[&salt[..], &id[..]].concat())[0] & 1 == 1
}

enum Phase {
    /// Checking and counting work.
    Presync,
    /// Fetching the same headers again: they are checked once more, matched
    /// against the bits, and held until enough bits have matched.
    Redownload {
        walk: Box<Walk>,
        count: u64,
        last: Hash,
        held: Vec<(Header, BigUint, Resources)>,
    },
}

/// One peer's header chain under check.
pub struct Presync {
    /// The indexed block the chain forks from.
    pub start: Hash,
    first: Walk,
    walk: Walk,
    /// The last header checked in the first pass, and how many there were.
    pub last: Hash,
    count: u64,
    salt: [u8; 32],
    offset: u64,
    bits: Vec<bool>,
    /// The most headers the clock allows: 6 a second of median-time advance
    /// since the fork, plus the two hours a timestamp may lead the clock.
    most: u64,
    threshold: BigUint,
    phase: Phase,
}

/// What a header did to a presync.
pub enum Outcome {
    /// Checked; more are wanted after the last.
    Continue,
    /// The chain reached the threshold: fetch it again from `start`.
    Refetch,
    /// Headers that are now indexed, with their work and limits, in order;
    /// `done` when the re-fetch has passed the first pass's last header.
    Index(Vec<(Header, BigUint, Resources)>, bool),
}

impl Presync {
    pub fn new(start: Hash, walk: Walk, threshold: BigUint, start_time: u64, now: u64) -> Self {
        let mut salt = [0; 32];
        let _ = getrandom::fill(&mut salt);
        let mut offset = [0; 8];
        let _ = getrandom::fill(&mut offset);
        let most = now
            .saturating_add(7_200)
            .saturating_sub(start_time)
            .saturating_mul(6)
            .saturating_add(PERIOD);
        Self {
            start,
            first: walk.clone(),
            walk,
            last: start,
            count: 0,
            salt,
            offset: u64::from_le_bytes(offset) % PERIOD,
            bits: Vec::new(),
            most,
            threshold,
            phase: Phase::Presync,
        }
    }
    /// Whether this header continues the chain under check.
    pub fn follows(&self, header: &Header) -> bool {
        match &self.phase {
            Phase::Presync => header.parent == self.last,
            Phase::Redownload { last, .. } => header.parent == *last,
        }
    }
    /// The header after which the next answer should start.
    pub fn next(&self) -> Hash {
        match &self.phase {
            Phase::Presync => self.last,
            Phase::Redownload { last, .. } => *last,
        }
    }
    pub fn refetching(&self) -> bool {
        matches!(self.phase, Phase::Redownload { .. })
    }
    /// Takes the next header. An error ends the presync: the peer sent a
    /// header that does not check, too many for the time, or, when fetched
    /// again, a different chain.
    pub fn add(&mut self, genesis: &Genesis, header: &Header) -> Result<Outcome, String> {
        let id = header.id();
        match &mut self.phase {
            Phase::Presync => {
                self.walk.step(genesis, header)?;
                if (self.count + self.offset).is_multiple_of(PERIOD) {
                    self.bits.push(bit(&self.salt, &id));
                }
                self.count += 1;
                self.last = id;
                if self.count > self.most {
                    return Err("more headers than the clock allows".into());
                }
                if self.walk.work >= self.threshold {
                    self.phase = Phase::Redownload {
                        walk: Box::new(self.first.clone()),
                        count: 0,
                        last: self.start,
                        held: Vec::new(),
                    };
                    return Ok(Outcome::Refetch);
                }
                Ok(Outcome::Continue)
            }
            Phase::Redownload {
                walk,
                count,
                last,
                held,
            } => {
                let limits = walk.limits;
                walk.step(genesis, header)?;
                if (*count + self.offset).is_multiple_of(PERIOD) {
                    let index = ((*count + self.offset) / PERIOD) as usize;
                    // The first pass kept bits from its own first multiple.
                    let first = usize::from(self.offset != 0);
                    let expected = index
                        .checked_sub(first)
                        .and_then(|i| self.bits.get(i))
                        .copied();
                    if expected.is_some_and(|b| b != bit(&self.salt, &id)) {
                        return Err("the chain fetched again differs".into());
                    }
                }
                *count += 1;
                *last = id;
                held.push((header.clone(), walk.work.clone(), limits));
                let done = *count >= self.count;
                if done && *last != self.last {
                    return Err("the chain fetched again differs".into());
                }
                if held.len() >= BUFFER || done {
                    return Ok(Outcome::Index(std::mem::take(held), done));
                }
                Ok(Outcome::Continue)
            }
        }
    }
}
