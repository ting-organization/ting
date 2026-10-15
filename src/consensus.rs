//! Branch-derived proof of work, difficulty, and resource voting.
use crate::{
    hash::{Hash, vector_root},
    meter::Resources,
    protocol::{Encode, Error, Genesis, Header, Result},
};
use num_bigint::BigUint;
use num_traits::{One, Zero};

pub fn work(target: &BigUint) -> BigUint {
    (BigUint::one() << 256usize) / (target + BigUint::one())
}
pub fn valid_pow(header: &Header) -> bool {
    !header.target.is_zero()
        && header.target.bits() <= 256
        && BigUint::from_bytes_be(&header.id()) <= header.target
}
/// Exact integer ASERT (wiki: consensus/difficulty): the target of the block whose
/// parent is `blocks` intervals after genesis and whose parent timestamp is
/// `elapsed` seconds after the genesis timestamp.
///
/// ```text
/// target = anchor × 2^((elapsed − interval × blocks) / half_life)
/// ```
///
/// The exponent is taken in 1/65536 steps with floor division; its fractional
/// part is approximated by a cubic polynomial in 16.48 fixed point, and the
/// integer part is applied as a shift. The result is clamped to `[1, limit]`.
pub fn asert(
    anchor: &BigUint,
    interval: u32,
    half_life: u32,
    blocks: u64,
    elapsed: i128,
    limit: &BigUint,
) -> BigUint {
    assert!(half_life > 0);
    let expected = i128::from(interval) * i128::from(blocks);
    let exponent = ((elapsed - expected) << 16).div_euclid(i128::from(half_life));
    let shifts = exponent >> 16;
    let frac = (exponent - (shifts << 16)) as u128;
    let factor = ((195_766_423_245_049 * frac
        + 971_821_376 * frac * frac
        + 5_127 * frac * frac * frac
        + (1 << 47))
        >> 48)
        + 65_536;
    // anchor × factor < 2^273; shifts beyond ±274 saturate at the bounds.
    let product = anchor * BigUint::from(factor);
    let shift = shifts - 16;
    let next = if shift >= 256 {
        limit.clone()
    } else if shift <= -274 {
        BigUint::one()
    } else if shift >= 0 {
        product << (shift as usize)
    } else {
        product >> ((-shift) as usize)
    };
    next.max(BigUint::one()).min(limit.clone())
}
/// Increase needs two-thirds of the epoch's work; decrease needs a strict majority.
pub fn vote_direction(signals: &[(BigUint, u8)]) -> u8 {
    let total: BigUint = signals.iter().map(|(w, _)| w).sum();
    if total.is_zero() {
        return 0;
    }
    let up: BigUint = signals
        .iter()
        .filter(|(_, v)| *v == 1)
        .map(|(w, _)| w)
        .sum();
    let down: BigUint = signals
        .iter()
        .filter(|(_, v)| *v == 2)
        .map(|(w, _)| w)
        .sum();
    if up * 3u32 >= &total * 2u32 {
        1
    } else if down * 2u32 > total {
        2
    } else {
        0
    }
}
/// Increases move by 5% without a ceiling; decreases move by 10% down to the floor.
pub fn adjust(value: u64, vote: u8, minimum: u64) -> u64 {
    match vote {
        1 => value.saturating_add((value / 20).max(1)),
        2 => value.saturating_sub((value / 10).max(1)).max(minimum),
        _ => value,
    }
}
/// What validating the next block needs of a branch: its height, its last
/// header (none at genesis) and the timestamps of its last eleven headers,
/// newest first. A node can supply it without holding the whole branch.
#[derive(Clone, Debug, Default)]
pub struct Ancestry {
    pub height: u64,
    pub parent: Option<Header>,
    pub recent: Vec<u64>,
}
impl Ancestry {
    /// The ancestry of a branch given in full, excluding the genesis anchor.
    pub fn of(history: &[Header]) -> Self {
        Self {
            height: history.len() as u64,
            parent: history.last().cloned(),
            recent: history.iter().rev().take(11).map(|h| h.timestamp).collect(),
        }
    }
    /// The id of the branch's last block, or the genesis block id.
    pub fn parent_id(&self, genesis: &Genesis) -> Hash {
        self.parent.as_ref().map_or(genesis.block_id(), Header::id)
    }
}
/// Ordered ancestry excludes the distinguished genesis anchor.
pub fn expected_target(genesis: &Genesis, history: &[Header]) -> BigUint {
    expected_target_at(genesis, &Ancestry::of(history))
}
pub fn expected_target_at(genesis: &Genesis, ancestry: &Ancestry) -> BigUint {
    let Some(parent) = &ancestry.parent else {
        return genesis.initial_target.clone();
    };
    asert(
        &genesis.initial_target,
        genesis.interval,
        genesis.half_life,
        ancestry.height,
        i128::from(parent.timestamp) - i128::from(genesis.timestamp),
        &genesis.pow_limit,
    )
}
/// The limits that one complete source epoch schedules, applied to the limits
/// of the epoch before its activation epoch (wiki: consensus/capacity-voting).
pub fn next_limits(genesis: &Genesis, previous: Resources, source: &[Header]) -> Resources {
    next_limits_tally(genesis, previous, &Tally::of(source))
}
/// `next_limits` from the source epoch's tally.
pub fn next_limits_tally(genesis: &Genesis, previous: Resources, source: &Tally) -> Resources {
    let mut limits = previous;
    for i in 0..4 {
        limits.0[i] = adjust(limits.0[i], source.direction(i), genesis.minimum.0[i]);
    }
    limits
}
/// The work-weighted votes of an epoch for each resource: all that
/// `next_limits` reads of its source epoch, kept as running sums so that a
/// node can count headers as they pass instead of holding them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub total: BigUint,
    pub up: [BigUint; 4],
    pub down: [BigUint; 4],
}
impl Tally {
    pub fn of(headers: &[Header]) -> Self {
        let mut tally = Self::default();
        for header in headers {
            tally.add(header);
        }
        tally
    }
    pub fn add(&mut self, header: &Header) {
        let w = work(&header.target);
        for i in 0..4 {
            match header.votes[i] {
                1 => self.up[i] += &w,
                2 => self.down[i] += &w,
                _ => {}
            }
        }
        self.total += w;
    }
    /// The vote's outcome for resource `i`, as `vote_direction` decides it.
    pub fn direction(&self, i: usize) -> u8 {
        if self.total.is_zero() {
            0
        } else if &self.up[i] * 3u32 >= &self.total * 2u32 {
            1
        } else if &self.down[i] * 2u32 > self.total {
            2
        } else {
            0
        }
    }
}
pub fn active_limits(genesis: &Genesis, history: &[Header]) -> Resources {
    let e = genesis.vote_epoch as usize;
    let epoch = history.len() / e;
    let mut limits = genesis.initial;
    for activation in 3..=epoch {
        let source = activation - 3;
        limits = next_limits(genesis, limits, &history[source * e..(source + 1) * e]);
    }
    limits
}
pub fn validate_header(genesis: &Genesis, history: &[Header], header: &Header) -> Result<()> {
    validate_header_with(genesis, history, header, active_limits(genesis, history))
}
/// Header validation with the active limits supplied by the caller, which may
/// derive them incrementally; they MUST equal `active_limits(genesis, history)`.
pub fn validate_header_with(
    genesis: &Genesis,
    history: &[Header],
    header: &Header,
    limits: Resources,
) -> Result<()> {
    validate_header_at(genesis, &Ancestry::of(history), header, limits)
}
/// Header validation against a branch's ancestry; `limits` MUST be the branch's
/// active limits for the next block.
pub fn validate_header_at(
    genesis: &Genesis,
    ancestry: &Ancestry,
    header: &Header,
    limits: Resources,
) -> Result<()> {
    if header.chain_id != genesis.chain_id()
        || header.parent != ancestry.parent_id(genesis)
        || header.height != ancestry.height + 1
    {
        return Err(Error("header ancestry"));
    }
    assert_eq!(
        ancestry.recent.len() as u64,
        ancestry.height.min(11),
        "an ancestry carries the last eleven timestamps"
    );
    let mut times = ancestry.recent.clone();
    if times.len() < 11 {
        times.push(genesis.timestamp);
    }
    times.sort_unstable();
    if header.timestamp <= times[times.len() / 2] {
        return Err(Error("median timestamp"));
    }
    if header.target != expected_target_at(genesis, ancestry) {
        return Err(Error("difficulty target"));
    }
    if header.votes.iter().any(|v| *v > 2) {
        return Err(Error("header fields"));
    }
    if !valid_pow(header) {
        return Err(Error("proof of work"));
    }
    if !header.used.fits(limits) {
        return Err(Error("block resource limits"));
    }
    Ok(())
}
pub fn commitments(transactions: &[impl Encode], receipts: &[impl Encode]) -> (Hash, Hash) {
    (
        vector_root(
            "transactions",
            &transactions.iter().map(Encode::encode).collect::<Vec<_>>(),
        ),
        vector_root(
            "receipts",
            &receipts.iter().map(Encode::encode).collect::<Vec<_>>(),
        ),
    )
}
