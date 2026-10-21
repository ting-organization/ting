//! The published chains through the node's own import path (wiki:
//! reference/conformance): the development seed and the voting chain, block by
//! block, with a 1 MiB database cache and no undo window, reopening the store
//! every 17 blocks and importing the same block again after each restart, then
//! an ordinary next block mined on top and reopened.
mod support;
use serde_json::json;
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
use ting_core::{
    hash::hex,
    protocol::{Block, Genesis},
};
use ting_node::{
    chain::Chain,
    config::{self, Config},
    seed,
};

/// The repository's `conformance/` folder.
fn conformance() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance")
}

#[test]
#[ignore = "imports the whole seed; CI runs it in release"]
fn the_development_seed_survives_small_cache_restarts_and_continues() {
    let genesis = Genesis::development();
    let bytes = std::fs::read(conformance().join("seed/development.blocks")).unwrap();
    let blocks = replay(&genesis, &bytes);
    // The node's list is the published seed.
    assert_eq!(blocks, seed::ids(&genesis));
}

#[test]
#[ignore = "imports a mined chain; CI runs it in release"]
fn the_voting_chain_survives_small_cache_restarts_and_continues() {
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(conformance().join("chains/voting.json")).unwrap())
            .unwrap();
    let encoded = manifest["genesis"].as_str().unwrap();
    let genesis = Genesis::decode(
        &(0..encoded.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&encoded[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let bytes = std::fs::read(conformance().join("chains/voting.blocks")).unwrap();
    let blocks = replay(&genesis, &bytes);
    let stated: Vec<_> = manifest["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(blocks.iter().map(|id| hex(id)).collect::<Vec<_>>(), stated);
}

/// Imports the blocks of `bundle` into a fresh node on `genesis`, and returns
/// their ids.
fn replay(genesis: &Genesis, bundle: &[u8]) -> Vec<ting_core::hash::Hash> {
    let dir = support::Directory::new();
    let config = Config {
        db_cache_mib: 1,
        undo_window: 0,
        ..Config::default()
    };
    config::initialize_empty(&dir.0, &config, genesis.clone()).unwrap();
    let open = || Chain::open(&dir.0, Arc::new(AtomicBool::new(false))).unwrap();
    let mut chain = open();
    let mut ids = Vec::new();
    let mut at = 0;
    while at < bundle.len() {
        let len = u32::from_le_bytes(bundle[at..at + 4].try_into().unwrap()) as usize;
        let encoded = &bundle[at + 4..at + 4 + len];
        at += 4 + len;
        let block = Block::decode(encoded, len).unwrap();
        chain
            .import_from(encoded, false, Some("conformance-peer"))
            .unwrap();
        ids.push(block.header.id());
        let height = ids.len();
        assert_eq!(chain.height(), height);
        assert_eq!(chain.tip(), block.header.id());
        let status = chain.request("status", json!({})).unwrap();
        assert_eq!(status["state_root"], hex(&block.header.state_root));
        if height % 17 == 0 || at == bundle.len() {
            drop(chain);
            chain = open();
            assert_eq!(chain.height(), height);
            assert_eq!(chain.tip(), block.header.id());
            assert_eq!(
                chain.request("status", json!({})).unwrap()["state_root"],
                status["state_root"]
            );
            // The same block again after a restart adds no height.
            chain.import(encoded, false).unwrap();
            assert_eq!(chain.height(), height);
        }
    }
    assert_eq!(at, bundle.len(), "unaccounted bundle bytes");
    chain.mine(1, 100_000).unwrap();
    assert_eq!(chain.height(), ids.len() + 1);
    let status = chain.request("status", json!({})).unwrap();
    drop(chain);
    let persisted = open().request("status", json!({})).unwrap();
    for field in ["height", "tip", "state_root", "work"] {
        assert_eq!(persisted[field], status[field]);
    }
    ids
}
