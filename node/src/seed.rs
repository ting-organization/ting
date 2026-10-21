//! A chain's seed (wiki: reference/conformance#the-seed): its first blocks,
//! the conformance suite every node of the chain replays. The node holds
//! their ids and refuses any other block at their heights before anything is
//! executed, which also stops a competing chain mined from the same genesis.
//! A node imports its chain's seed when it is created, replaying every block
//! as one from a peer.
use crate::{Result, chain::INVALID, config};
use std::{
    path::Path,
    sync::{Arc, OnceLock, atomic::AtomicBool},
};
use ting_core::{
    hash::Hash,
    protocol::{Genesis, HEADER_LEN, Header},
};

/// The development seed, each block framed with a u32 length, as
/// `tests/vm/seed.rs` builds and publishes it.
const DEVELOPMENT: &[u8] = include_bytes!("../../conformance/seed/development.blocks");
/// The production seed, framed the same way.
const PRODUCTION: &[u8] = include_bytes!("../../conformance/seed/production.blocks");

/// The seed of the chain of `genesis`, if it has one.
fn bundle(genesis: &Genesis) -> Option<(&'static [u8], &'static OnceLock<Vec<Hash>>)> {
    static DEVELOPMENT_IDS: OnceLock<Vec<Hash>> = OnceLock::new();
    static PRODUCTION_IDS: OnceLock<Vec<Hash>> = OnceLock::new();
    let id = genesis.chain_id();
    if id == config::development_genesis().chain_id() {
        Some((DEVELOPMENT, &DEVELOPMENT_IDS))
    } else if config::production_genesis().is_some_and(|g| g.chain_id() == id) {
        Some((PRODUCTION, &PRODUCTION_IDS))
    } else {
        None
    }
}

/// The blocks of a bundle, each as its encoded bytes.
fn frames(bundle: &'static [u8]) -> impl Iterator<Item = &'static [u8]> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let length = bundle.get(at..at + 4)?;
        let n = u32::from_le_bytes(length.try_into().unwrap()) as usize;
        let frame = &bundle[at + 4..at + 4 + n];
        at += 4 + n;
        Some(frame)
    })
}

/// The seed's block ids of the chain of `genesis`, by height from 1; none for
/// a chain without a seed.
pub fn ids(genesis: &Genesis) -> &'static [Hash] {
    let Some((seed, ids)) = bundle(genesis) else {
        return &[];
    };
    ids.get_or_init(|| {
        frames(seed)
            .map(|block| {
                Header::decode(&block[..HEADER_LEN])
                    .expect("a seed's headers decode")
                    .id()
            })
            .collect()
    })
}

/// Refuses a header at one of the seed's heights that is not the seed's
/// block there.
pub fn check(seed: &[Hash], header: &Header) -> Result<()> {
    let listed = usize::try_from(header.height)
        .ok()
        .and_then(|h| h.checked_sub(1))
        .and_then(|i| seed.get(i));
    match listed {
        Some(id) if *id != header.id() => Err(format!(
            "{INVALID}header: not the seed's block at height {}",
            header.height
        )),
        _ => Ok(()),
    }
}

/// Imports the seed of the chain of `genesis` into the new chain in `dir`,
/// each block validated and executed as one from a peer would be; returns the
/// height reached, 0 for a chain without a seed.
pub fn import(dir: &Path, genesis: &Genesis) -> Result<u64> {
    let Some((seed, _)) = bundle(genesis) else {
        return Ok(0);
    };
    let mut chain = crate::chain::Chain::open(dir, Arc::new(AtomicBool::new(false)))?;
    for block in frames(seed) {
        chain.import(block, false)?;
    }
    Ok(chain.height() as u64)
}
