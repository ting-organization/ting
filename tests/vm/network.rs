//! The chain the cases run on and the seed is built for: the development
//! chain, or, with `TING_SEED_CHAIN=production`, the production chain. A
//! case's identifiers and receipts follow its chain, and the heavy blocks its
//! capacity. The production seed is built once, at the launch
//! (`seed::the_production_seed_is_built_at_launch`); afterwards its published
//! bundle only replays.
use std::sync::OnceLock;
use ting_core::protocol::Genesis;

/// The production genesis's timestamp (rules §3), the moment its chain
/// begins.
pub(crate) const PRODUCTION_LAUNCH: u64 = 1_792_540_800;

/// Whether the cases and the seed are built for the production chain.
pub(crate) fn production() -> bool {
    static PRODUCTION: OnceLock<bool> = OnceLock::new();
    *PRODUCTION.get_or_init(|| match std::env::var("TING_SEED_CHAIN") {
        Ok(chain) if chain == "production" => true,
        Ok(chain) if chain == "development" || chain.is_empty() => false,
        Ok(chain) => panic!("TING_SEED_CHAIN names no chain: {chain}"),
        Err(_) => false,
    })
}
/// The genesis of the chain the cases run on.
pub(crate) fn genesis() -> Genesis {
    if production() {
        Genesis::production(PRODUCTION_LAUNCH)
    } else {
        Genesis::development()
    }
}
/// The chain's name, as its seed's files are named.
pub(crate) fn name() -> &'static str {
    if production() {
        "production"
    } else {
        "development"
    }
}
