mod support;
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    sync::{Arc, atomic::AtomicBool},
};
use support::Directory;
use ting_core::{
    consensus,
    hash::hex,
    protocol::{Block, Encode, Genesis, Header},
    state::State,
    vm::{Context, Runtime},
};
use ting_node::{chain::Chain, config, store::Store, wire};
fn open(dir: &Directory) -> Chain {
    Chain::open(&dir.0, Arc::new(AtomicBool::new(false))).unwrap()
}
fn mine(
    runtime: &Runtime,
    g: &Genesis,
    history: &mut Vec<Header>,
    transactions: Vec<ting_core::protocol::Transaction>,
    producer: &str,
) -> Block {
    let mut b = runtime
        .build_candidate(
            g,
            history,
            transactions,
            g.timestamp + 60 * (history.len() as u64 + 1),
            [0; 4],
            producer.as_bytes().to_vec(),
        )
        .unwrap();
    while !consensus::valid_pow(&b.header) {
        b.header.nonce += 1;
    }
    runtime.validate_and_apply_block(g, history, &b).unwrap();
    history.push(b.header.clone());
    b
}
#[test]
fn higher_work_reorganizes_state_and_restart_reproduces_it() {
    let dir = Directory::new();
    dir.config(false, "A");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let a = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut ah = vec![];
    let a1 = mine(&a, &g, &mut ah, vec![support::tx()], "A");
    let contract = hex(&a1.receipts[0].result);
    chain.import(&a1.encode(), true).unwrap();
    assert_eq!(
        chain
            .request("storage", json!({"id":contract,"key":hex(&[0;32])}))
            .unwrap()["value_hex"],
        "2a"
    );
    let b = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut bh = vec![];
    let b1 = mine(&b, &g, &mut bh, vec![], "B");
    chain.import(&b1.encode(), true).unwrap();
    assert_eq!(
        chain.tip(),
        a1.header.id(),
        "equal-work tie preserves existing tip"
    );
    let b2 = mine(&b, &g, &mut bh, vec![], "B");
    chain.import(&b2.encode(), true).unwrap();
    assert_eq!(chain.tip(), b2.header.id());
    assert!(chain.request("contract", json!({"id":contract})).is_err());
    assert_eq!(
        chain.request("mempool", json!({})).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    chain.mine(1, 10000).unwrap();
    assert_eq!(
        chain
            .request("storage", json!({"id":contract,"key":hex(&[0;32])}))
            .unwrap()["value_hex"],
        "2a"
    );
    let status = chain.request("status", json!({})).unwrap();
    drop(chain);
    let mut restarted = open(&dir);
    let again = restarted.request("status", json!({})).unwrap();
    for key in ["tip", "height", "state_root", "work", "stored_blocks"] {
        assert_eq!(again[key], status[key]);
    }
}
#[test]
fn duplicate_occurrences_are_valid_and_envelope_admission_is_bounded() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let tx = support::tx();
    let id = hex(&tx.id(chain.genesis.chain_id()));
    chain.submit(&tx.encode()).unwrap();
    chain.submit(&tx.encode()).unwrap();
    assert_eq!(
        chain.request("status", json!({})).unwrap()["mempool_count"],
        1
    );
    chain.mine(1, 10000).unwrap();
    chain.submit(&tx.encode()).unwrap();
    chain.mine(1, 10000).unwrap();
    let found = chain.request("transaction", json!({"id":id})).unwrap();
    assert_eq!(found["occurrences"].as_array().unwrap().len(), 2);
    assert_eq!(found["occurrences"][0]["receipt"]["status"], 0);
    assert_eq!(found["occurrences"][1]["receipt"]["status"], 10);
    let first = chain
        .request("transaction", json!({"id":id,"limit":1}))
        .unwrap();
    assert_eq!(first["occurrences"].as_array().unwrap().len(), 1);
    assert_eq!(first["has_more"], true);
    let second = chain
        .request(
            "transaction",
            json!({"id":id,"from":first["next"],"limit":1}),
        )
        .unwrap();
    assert_eq!(second["occurrences"][0]["receipt"]["status"], 10);
    assert_eq!(second["has_more"], false);
    let mut invalid = tx.clone();
    invalid.budget.0[2] = 1;
    assert!(chain.submit(&invalid.encode()).is_err());
    let mut bytes = tx.encode();
    bytes.push(0);
    assert!(chain.submit(&bytes).is_err());
    assert!(chain.request("blocks", json!({"limit":101})).is_err());
}
/// A chain of `n` valid blocks on genesis, from its own runtime.
fn branch(g: &Genesis, n: usize, producer: &str) -> Vec<Block> {
    let runtime = Runtime::new(State::default(), Context::from_genesis(g));
    let mut history = vec![];
    (0..n)
        .map(|_| mine(&runtime, g, &mut history, vec![], producer))
        .collect()
}
/// `n` blocks whose first has a wrong state root: every header checks, and
/// only executing the first block shows the branch invalid.
fn forged(g: &Genesis, n: usize) -> Vec<Block> {
    let mut blocks = branch(g, n, "F");
    let mut parent = g.block_id();
    for (i, block) in blocks.iter_mut().enumerate() {
        block.header.parent = parent;
        if i == 0 {
            block.header.state_root[0] ^= 1;
        }
        block.header.nonce = 0;
        while !consensus::valid_pow(&block.header) {
            block.header.nonce += 1;
        }
        parent = block.header.id();
    }
    blocks
}
#[test]
fn a_heavier_branch_that_fails_on_the_way_leaves_the_tip_where_it_was() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let a = branch(&g, 1, "A");
    chain.import(&a[0].encode(), true).unwrap();
    // Two blocks overtake the tip: it is disconnected, the first of them fails,
    // and the tip is connected again. Nothing was replaced.
    let b = forged(&g, 2);
    headers_from(&mut chain, &headers_of(&b)).unwrap();
    chain.import(&b[1].encode(), true).unwrap();
    chain.import(&b[0].encode(), true).unwrap();
    assert_eq!(chain.tip(), a[0].header.id());
    assert_eq!(
        chain.request("status", json!({})).unwrap()["reorganization"],
        serde_json::Value::Null
    );
}
#[test]
fn a_reorganization_that_fails_part_way_returns_to_the_tip_it_left() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let a = branch(&g, 3, "A");
    for block in &a {
        chain.import(&block.encode(), true).unwrap();
    }
    // B matches A's work at height 3, and its fourth block, whose state root
    // is wrong, overtakes A.
    let mut b = branch(&g, 4, "B");
    let last = b.last_mut().unwrap();
    last.header.state_root[0] ^= 1;
    while !consensus::valid_pow(&last.header) {
        last.header.nonce += 1;
    }
    headers_from(&mut chain, &headers_of(&b)).unwrap();
    for block in &b[..3] {
        chain.import(&block.encode(), true).unwrap();
    }
    assert_eq!(chain.tip(), a[2].header.id());
    // B1 to B3 connect and B4 fails: A, which B3 does not overtake, is
    // where the node goes back to.
    chain.import(&b[3].encode(), true).unwrap();
    assert_eq!(chain.tip(), a[2].header.id());
    assert_eq!(
        chain.request("status", json!({})).unwrap()["reorganization"],
        serde_json::Value::Null
    );
}
#[test]
fn a_reorganization_that_forks_again_reports_from_its_lowest_height() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let replay = |blocks: &[Block]| {
        let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
        let mut history = vec![];
        for block in blocks {
            runtime
                .validate_and_apply_block(&g, &history, block)
                .unwrap();
            history.push(block.header.clone());
        }
        (runtime, history)
    };
    // A to height 5; X forks at 3 and runs to 9, its last block failing; Y
    // forks from X at 6 and also reaches 9.
    let a = branch(&g, 5, "A");
    let (rx, mut hx) = replay(&a[..3]);
    let mut x: Vec<Block> = (0..6)
        .map(|_| mine(&rx, &g, &mut hx, vec![], "X"))
        .collect();
    let last = x.last_mut().unwrap();
    last.header.state_root[0] ^= 1;
    while !consensus::valid_pow(&last.header) {
        last.header.nonce += 1;
    }
    let mut prefix = a[..3].to_vec();
    prefix.extend(x[..3].iter().cloned());
    // Of two branches of equal work the one with the larger id is tried first:
    // X must be, so that Y forks off it after its last block fails.
    let y = (0..)
        .map(|n| {
            let (ry, mut hy) = replay(&prefix);
            (0..3)
                .map(|_| mine(&ry, &g, &mut hy, vec![], &format!("Y{n}")))
                .collect::<Vec<_>>()
        })
        .find(|y| y[2].header.id() < x[5].header.id())
        .unwrap();
    for block in &a {
        chain.import(&block.encode(), true).unwrap();
    }
    for blocks in [&x, &y] {
        headers_from(&mut chain, &headers_of(blocks)).unwrap();
    }
    // Every body but X's first, so that nothing is connected before it.
    for block in x[1..].iter().chain(&y).rev() {
        chain.import(&block.encode(), true).unwrap();
    }
    assert_eq!(chain.tip(), a[4].header.id());
    // One activation: back to 3, X up to 8, its last block fails, back to 6,
    // Y up to 9. Two blocks of the old chain were replaced, above height 3.
    chain.import(&x[0].encode(), true).unwrap();
    assert_eq!(chain.tip(), y[2].header.id());
    let reorganization = chain.request("status", json!({})).unwrap()["reorganization"].clone();
    assert_eq!(reorganization["replaced"], 2, "{reorganization}");
    assert_eq!(reorganization["fork_height"], 3);
    assert_eq!(reorganization["height"], 9);
}
/// A peer's page of headers, in their encodings one after another, as the
/// node takes it from a connection.
fn headers_from(chain: &mut Chain, headers: &[Header]) -> Result<serde_json::Value, String> {
    let bytes: Vec<u8> = headers.iter().flat_map(|h| h.encode()).collect();
    chain.request_with("headers_from", json!({"peer":"p"}), Some(bytes))
}
fn headers_of(blocks: &[Block]) -> Vec<Header> {
    blocks.iter().map(|b| b.header.clone()).collect()
}
/// A peer request the chain answers itself.
fn ask(chain: &mut Chain, request: wire::Request) -> Result<wire::Answer, String> {
    chain.answer_wire(&request)
}
#[test]
fn the_operator_marks_a_block_invalid_and_reconsiders_it() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let a = branch(&g, 4, "A");
    let b = branch(&g, 2, "B");
    for block in a[..3].iter().chain(&b) {
        chain.import(&block.encode(), true).unwrap();
    }
    assert_eq!(chain.tip(), a[2].header.id());
    // Marking a connected block leaves it and everything above it; the best
    // remaining branch is connected, a reorganization of all three blocks.
    let marked = chain
        .request("invalidate", json!({"id":hex(&a[1].header.id())}))
        .unwrap();
    assert_eq!(marked["known"], true);
    assert_eq!(chain.tip(), b[1].header.id());
    let status = chain.request("status", json!({})).unwrap();
    assert_eq!(status["marked"], json!([hex(&a[1].header.id())]));
    assert_eq!(status["reorganization"]["replaced"], 3);
    assert_eq!(status["reorganization"]["fork_height"], 0);
    assert_eq!(status["reorganization"]["deep"], false);
    // Peers are given the valid block below the mark, but neither the marked
    // block nor one above it, though both were executed and found valid.
    assert!(
        chain
            .request("raw", json!({"id":hex(&a[0].header.id())}))
            .is_ok()
    );
    for block in [&a[1], &a[2]] {
        assert_eq!(
            chain
                .request("raw", json!({"id":hex(&block.header.id())}))
                .unwrap_err(),
            "block not found"
        );
    }
    // The marked branch is refused from peers, but held against none.
    for block in [&a[2], &a[3]] {
        let error = chain
            .request(
                "import",
                json!({"hex":hex(&block.encode()),"peer":"10.0.0.9:1"}),
            )
            .unwrap_err();
        assert_eq!(error, ting_node::chain::MARKED);
        assert!(ting_node::chain::is_policy(&error) && !ting_node::chain::is_invalid(&error));
    }
    assert_eq!(
        chain.request("offenders", json!({})).unwrap(),
        json!({"peers":[]})
    );
    // The mark holds across a restart.
    drop(chain);
    let mut chain = open(&dir);
    assert_eq!(chain.tip(), b[1].header.id());
    // Reconsidered, the branch is followed again, by its stored changes.
    let back = chain
        .request("reconsider", json!({"id":hex(&a[2].header.id())}))
        .unwrap();
    assert_eq!(back["cleared"], 2);
    assert_eq!(chain.tip(), a[2].header.id());
    assert!(
        chain
            .request("raw", json!({"id":hex(&a[2].header.id())}))
            .is_ok()
    );
    chain.import(&a[3].encode(), true).unwrap();
    assert_eq!(chain.tip(), a[3].header.id());
    let status = chain.request("status", json!({})).unwrap();
    assert_eq!(status["marked"], json!([]));
    drop(chain);
    assert_eq!(open(&dir).tip(), a[3].header.id());
}
#[test]
fn a_block_not_yet_seen_can_be_marked_and_the_genesis_block_cannot() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let a = branch(&g, 2, "A");
    chain.import(&a[0].encode(), true).unwrap();
    let id = hex(&a[1].header.id());
    assert_eq!(
        chain.request("invalidate", json!({"id":id})).unwrap()["known"],
        false
    );
    assert_eq!(
        chain.import(&a[1].encode(), true).unwrap_err(),
        ting_node::chain::MARKED
    );
    assert_eq!(
        chain.request("reconsider", json!({"id":id})).unwrap()["cleared"],
        0
    );
    chain.import(&a[1].encode(), true).unwrap();
    assert_eq!(chain.height(), 2);
    // By height, a connected block.
    chain.request("invalidate", json!({"id":"2"})).unwrap();
    assert_eq!(chain.height(), 1);
    assert_eq!(
        chain
            .request("invalidate", json!({"id":hex(&g.block_id())}))
            .unwrap_err(),
        "the genesis block cannot be marked invalid"
    );
    assert_eq!(
        chain
            .request("reconsider", json!({"id":hex(&[7; 32])}))
            .unwrap_err(),
        "block not found"
    );
}
#[test]
fn a_heavier_branch_that_fails_raises_a_warning() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let forged = forged(&g, 8);
    headers_from(&mut chain, &headers_of(&forged)).unwrap();
    chain.import(&forged[0].encode(), true).unwrap();
    assert_eq!(chain.height(), 0);
    let warnings = chain.request("status", json!({})).unwrap()["warnings"].clone();
    assert!(
        warnings[0]
            .as_str()
            .unwrap()
            .contains("about 8 blocks heavier"),
        "{warnings}"
    );
    // Once this node's own chain comes within six blocks of it, the warning goes.
    for block in branch(&g, 3, "B") {
        chain.import(&block.encode(), true).unwrap();
    }
    assert_eq!(
        chain.request("status", json!({})).unwrap()["warnings"],
        json!([])
    );
}
#[test]
fn a_block_stored_before_its_parent_survives_a_restart() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let a = branch(&g, 2, "A");
    headers_from(&mut chain, &headers_of(&a)).unwrap();
    // The second block arrives first, and the node restarts before the
    // first: its header is gone, and the second waits for it.
    chain.import(&a[1].encode(), true).unwrap();
    drop(chain);
    let mut chain = open(&dir);
    assert_eq!(
        chain.request("status", json!({})).unwrap()["stored_blocks"],
        0
    );
    headers_from(&mut chain, &headers_of(&a)).unwrap();
    assert_eq!(
        chain.request("status", json!({})).unwrap()["stored_blocks"],
        1
    );
    chain.import(&a[0].encode(), true).unwrap();
    assert_eq!(chain.tip(), a[1].header.id());
}
#[test]
fn a_relayed_transaction_a_recent_block_included_is_refused() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let tx = hex(&support::tx().encode());
    chain.request("submit", json!({"hex":tx})).unwrap();
    chain.mine(1, 10000).unwrap();
    assert_eq!(
        chain
            .request("submit", json!({"hex":tx,"relayed":true}))
            .unwrap_err(),
        ting_node::chain::INCLUDED
    );
    // Submitted directly it is taken: the same transaction may be mined again.
    assert_eq!(
        chain.request("submit", json!({"hex":tx})).unwrap()["known"],
        false
    );
    // One that failed is refused only while its block's echo lasts, so that
    // a miner who made it fail on purpose cannot keep it from the network.
    let call_nothing = call(&hex(&[7; 32]), vec![]);
    let (failing, failing_id) = (
        hex(&call_nothing.encode()),
        hex(&call_nothing.id(chain.genesis.chain_id())),
    );
    chain.request("submit", json!({"hex":failing})).unwrap();
    chain.mine(1, 10000).unwrap();
    let block = chain.request("block", json!({"id":"2"})).unwrap();
    let receipt = block["receipts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["transaction"] == json!(failing_id))
        .expect("included")
        .clone();
    assert_ne!(receipt["status_name"], "Success");
    let relayed =
        |chain: &mut Chain, hex: &str| chain.request("submit", json!({"hex":hex,"relayed":true}));
    assert_eq!(
        relayed(&mut chain, &failing).unwrap_err(),
        ting_node::chain::INCLUDED
    );
    chain.mine(ting_node::chain::RELAY_ECHO, 10000).unwrap();
    assert_eq!(relayed(&mut chain, &failing).unwrap()["known"], false);
    // One that succeeded stays refused for the day.
    assert_eq!(
        relayed(&mut chain, &tx).unwrap_err(),
        ting_node::chain::INCLUDED
    );
}
#[test]
fn a_block_that_fails_names_the_peer_that_sent_it() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let r = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut h = vec![];
    let good = mine(&r, &g, &mut h, vec![], "A");
    let mut bad = mine(&r, &g, &mut h, vec![support::tx()], "A");
    // The body matches the header; only executing it shows the wrong state.
    bad.header.state_root[0] ^= 1;
    while !consensus::valid_pow(&bad.header) {
        bad.header.nonce += 1;
    }
    for (block, peer) in [(&good, "10.0.0.1:1"), (&bad, "10.0.0.2:2")] {
        chain
            .request("import", json!({"hex":hex(&block.encode()),"peer":peer}))
            .unwrap();
    }
    assert_eq!(chain.tip(), good.header.id());
    assert_eq!(
        chain.request("offenders", json!({})).unwrap(),
        json!({"peers":["10.0.0.2:2"]})
    );
    assert_eq!(
        chain.request("offenders", json!({})).unwrap(),
        json!({"peers":[]})
    );
    // Sent again, it is refused as invalid at once.
    assert_eq!(
        chain.import(&bad.encode(), true).unwrap_err(),
        "invalid block: it failed validation"
    );
    // Only executed, valid blocks are passed on to peers: not the failed one,
    // and not a competing block stored without being executed.
    let other = Runtime::new(State::default(), Context::from_genesis(&g));
    let side = mine(&other, &g, &mut vec![], vec![], "B");
    chain.import(&side.encode(), true).unwrap();
    assert_eq!(chain.tip(), good.header.id());
    for block in [&bad, &side] {
        assert_eq!(
            chain
                .request("raw", json!({"id":hex(&block.header.id())}))
                .unwrap_err(),
            "block not found"
        );
    }
    assert_eq!(
        chain
            .request("raw", json!({"id":hex(&good.header.id())}))
            .unwrap()["hex"],
        hex(&good.encode())
    );
}
#[test]
fn rejects_invalid_bodies_unknown_parents_and_header_work() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let r = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut h = vec![];
    let mut b = mine(&r, &g, &mut h, vec![support::tx()], "A");
    b.receipts[0].result[0] ^= 1;
    assert!(
        chain
            .import(&b.encode(), true)
            .unwrap_err()
            .contains("commitment")
    );
    assert_eq!(chain.height(), 0);
    b.header.parent = [1; 32];
    assert_eq!(
        chain.import(&b.encode(), true).unwrap_err(),
        "unknown parent"
    );
    b.header.parent = g.block_id();
    while consensus::valid_pow(&b.header) {
        b.header.nonce += 1;
    }
    assert_eq!(
        chain.import(&b.encode(), true).unwrap_err(),
        "invalid header: proof of work"
    );
    assert!(!dir.0.join("blocks").exists());
    assert!(dir.0.join("chain.redb").exists());
}
#[test]
fn truncated_tail_repairs_and_indexed_block_corruption_is_rejected_on_read() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    chain.mine(2, 10000).unwrap();
    let tip = chain.tip();
    drop(chain);
    let path = dir.0.join("blocks/00000000.dat");
    let size = fs::metadata(&path).unwrap().len();
    {
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&[0x2c, 1]).unwrap();
    }
    let chain = open(&dir);
    assert_eq!(chain.tip(), tip);
    drop(chain);
    assert_eq!(fs::metadata(&path).unwrap().len(), size);
    {
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&400u64.to_le_bytes()).unwrap();
        file.write_all(&[0; 80]).unwrap();
    }
    drop(open(&dir));
    assert_eq!(fs::metadata(&path).unwrap().len(), size);
    let mut bytes = fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(&path, bytes).unwrap();
    let mut chain = open(&dir);
    assert!(
        chain
            .request("raw", json!({"id":"2"}))
            .unwrap_err()
            .contains("checksum")
    );
}
#[test]
fn genesis_and_directory_lock_are_enforced() {
    let dir = Directory::new();
    dir.config(false, "test");
    let lock = config::lock(&dir.0).unwrap();
    assert!(config::lock(&dir.0).is_err());
    drop(lock);
    drop(config::lock(&dir.0).unwrap());
    let g = config::genesis(&dir.0).unwrap();
    drop(Store::open(&dir.0, &g, ting_node::store::CACHE_BYTES).unwrap());
    let mut other = g;
    other.timestamp += 1;
    assert!(
        Store::open(&dir.0, &other, ting_node::store::CACHE_BYTES)
            .err()
            .unwrap()
            .contains("mismatch")
    );
    assert!(config::initialize(&dir.0, &config::Config::default(), None).is_err());
}
#[test]
fn deferred_transactions_wait_until_they_succeed_and_survive_restart() {
    use ting_core::{
        meter::Resources,
        protocol::{Payload, Transaction},
    };
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let chain_id = chain.genesis.chain_id();
    let deploy = support::tx();
    // The id the deployment will get, so a call can be deferred before it exists.
    let contract = {
        let mut bytes = chain_id.to_vec();
        bytes.extend(deploy.id(chain_id));
        bytes.extend(0u64.to_le_bytes());
        ting_core::hash::hash("contract", &bytes)
    };
    let call = Transaction {
        budget: Resources([1_000_000, 128, 16_384, 16_384]),
        payload: Payload::Call {
            target: contract,
            input: vec![],
        },
    };
    let call_hex = hex(&call.encode());
    let call_id = hex(&call.id(chain_id));
    let now = ting_node::now();
    assert!(
        chain
            .request(
                "submit",
                json!({"hex":call_hex,"not_before":now + 400 * 86_400})
            )
            .is_err(),
        "a year is the horizon"
    );
    let admission = chain
        .request("submit", json!({"hex":call_hex,"not_before":now - 1}))
        .unwrap();
    assert_eq!(admission["deferred"], true);
    assert_eq!(admission["known"], false);
    let again = chain
        .request("submit", json!({"hex":call_hex,"not_before":now + 5}))
        .unwrap();
    assert_eq!(
        (again["known"].as_bool(), again["not_before"].as_u64()),
        (Some(true), Some(now - 1))
    );
    let status = chain.request("status", json!({})).unwrap();
    assert_eq!(
        (
            status["deferred_count"].as_u64(),
            status["mempool_count"].as_u64()
        ),
        (Some(1), Some(0))
    );
    // Due, but the contract is missing: tried, not admitted, nothing mined.
    chain.tick().unwrap();
    let lookup = chain.request("transaction", json!({"id":call_id})).unwrap();
    assert_eq!(lookup["pending"], false);
    assert_eq!(lookup["deferred"]["admitted"], false);
    assert_eq!(lookup["deferred"]["last_result"], "Missing");
    assert!(lookup["occurrences"].as_array().unwrap().is_empty());
    chain.mine(1, 10000).unwrap();
    assert_eq!(
        chain.request("status", json!({})).unwrap()["mempool_count"],
        0
    );
    // It is persisted: a restarted node still holds it.
    drop(chain);
    let mut chain = open(&dir);
    assert_eq!(
        chain.request("status", json!({})).unwrap()["deferred_count"],
        1
    );
    // Once the contract exists the call succeeds on the tip and enters the pool.
    chain.submit(&deploy.encode()).unwrap();
    chain.mine(1, 10000).unwrap();
    chain.tick().unwrap();
    let lookup = chain.request("transaction", json!({"id":call_id})).unwrap();
    assert_eq!(
        (
            lookup["pending"].as_bool(),
            lookup["deferred"]["admitted"].as_bool()
        ),
        (Some(true), Some(true))
    );
    assert_eq!(lookup["deferred"]["last_result"], serde_json::Value::Null);
    let Ok(wire::Answer::State(state)) = ask(&mut chain, wire::Request::State) else {
        panic!("no state");
    };
    assert!(
        state.pending.is_empty(),
        "peers get it as deferred, not pending"
    );
    assert_eq!(hex(&state.deferred[0]), call_id);
    let inventory = |chain: &mut Chain, deferred: bool| match ask(
        chain,
        wire::Request::Inventory {
            deferred,
            after: None,
        },
    ) {
        Ok(wire::Answer::Inventory { ids, next: None }) => ids,
        other => panic!("{other:?}"),
    };
    assert!(inventory(&mut chain, false).is_empty());
    let ids = inventory(&mut chain, true);
    assert_eq!(
        ids.iter().map(|id| hex(id)).collect::<Vec<_>>(),
        [call_id.as_str()]
    );
    let Ok(wire::Answer::Items(items)) = ask(&mut chain, wire::Request::Deferred { ids }) else {
        panic!("no items");
    };
    assert_eq!(
        (hex(&items[0].id), items[0].not_before),
        (call_id.clone(), Some(now - 1))
    );
    assert_eq!(
        chain.request("mempool", json!({})).unwrap()["items"][0]["deferred"]["admitted"],
        true
    );
    // Included: the entry is gone from the set and from the file.
    chain.mine(1, 10000).unwrap();
    chain.tick().unwrap();
    let lookup = chain.request("transaction", json!({"id":call_id})).unwrap();
    assert_eq!(lookup["occurrences"][0]["receipt"]["status"], 0);
    assert_eq!(lookup["deferred"], serde_json::Value::Null);
    assert_eq!(
        chain.request("status", json!({})).unwrap()["deferred_count"],
        0
    );
    // A future one is held without being tried.
    let future = Transaction {
        budget: call.budget,
        payload: Payload::Call {
            target: contract,
            input: vec![1],
        },
    };
    let later = chain
        .request(
            "submit",
            json!({"hex":hex(&future.encode()),"not_before":now + 86_400}),
        )
        .unwrap();
    chain.tick().unwrap();
    let lookup = chain
        .request("transaction", json!({"id":later["id"]}))
        .unwrap();
    assert_eq!(lookup["deferred"]["last_attempt"], serde_json::Value::Null);
    chain.mine(1, 10000).unwrap();
    assert!(lookup["occurrences"].as_array().unwrap().is_empty());
    assert_eq!(
        chain.request("status", json!({})).unwrap()["deferred_count"],
        1
    );
    // Closing the chain writes the set: the included entry is gone from the
    // file, the future one is in it.
    drop(chain);
    let file: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.0.join("deferred.json")).unwrap()).unwrap();
    let items = file["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["hex"], hex(&future.encode()));
}
#[test]
fn templates_are_solved_by_nonce_and_discarded_with_their_tip() {
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use ting_node::mining;
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let tx = support::tx();
    let tx_id = hex(&tx.id(chain.genesis.chain_id()));
    chain.submit(&tx.encode()).unwrap();
    // First in: the pooled transaction is included; the header has no nonce yet.
    let template = chain.request("template", json!({})).unwrap();
    assert_eq!(template["transactions"][0], tx_id);
    assert_eq!(template["height"], 1);
    assert_eq!(template["mining"]["nonce_offset"], 278);
    let header =
        Header::decode(&ting_node::unhex(template["header_hex"].as_str().unwrap(), 294).unwrap())
            .unwrap();
    assert_eq!(header.nonce, 0);
    // Asking again on the same tip and second gives the same template.
    assert_eq!(
        chain.request("template", json!({})).unwrap()["id"],
        template["id"]
    );
    // A nonce that misses the target is refused; a found one becomes the block.
    let tried = AtomicU64::new(0);
    let nonce = mining::search(&header, 0, 100_000, &AtomicBool::new(false), &tried).unwrap();
    let miss = (0u128..)
        .find(|n| {
            let mut h = header.clone();
            h.nonce = *n;
            !consensus::valid_pow(&h)
        })
        .unwrap();
    assert!(
        chain
            .request(
                "solution",
                json!({"template":template["id"],"nonce":miss.to_string()})
            )
            .unwrap_err()
            .contains("target")
    );
    let imported = chain
        .request(
            "solution",
            json!({"template":template["id"],"nonce":nonce.to_string()}),
        )
        .unwrap();
    assert_eq!(imported["canonical"], true);
    assert_eq!(chain.height(), 1);
    assert_eq!(
        chain.request("status", json!({})).unwrap()["mempool_count"],
        0
    );
    // The template went with the tip it was built on.
    assert!(
        chain
            .request(
                "solution",
                json!({"template":template["id"],"nonce":nonce.to_string()})
            )
            .is_err()
    );
    // Explicit selection: only pooled transactions, in the given order; an
    // empty list is an empty block.
    assert!(
        chain
            .request("template", json!({"transactions":[tx_id]}))
            .is_err()
    );
    let empty = chain
        .request("template", json!({"transactions":[]}))
        .unwrap();
    assert!(empty["transactions"].as_array().unwrap().is_empty());
    assert_eq!(empty["height"], 2);
}
#[test]
fn a_fork_replaces_selection_and_search_and_names_its_producer_data() {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };
    use ting_core::{protocol::Payload, vm::Candidate};
    use ting_node::{
        chain::Handle,
        mining::{self, Miner, Search, Selector},
    };
    // A fork's selection: the pool in reverse, each kept only if it succeeds.
    struct LastIn;
    impl Selector for LastIn {
        fn select(&mut self, pool: &[mining::Pending<'_>], block: &mut Candidate<'_>) {
            for pending in pool.iter().rev() {
                if let Ok(Some(receipt)) = block.add(pending.tx)
                    && receipt.status != ting_core::protocol::Status::Success
                {
                    block.undo();
                }
            }
        }
        fn describe(&self) -> serde_json::Value {
            json!({"kind":"last-in"})
        }
    }
    // A fork's search: counts the slices it is handed, then runs the plain loop.
    struct Counting(AtomicU64);
    impl Search for Counting {
        fn search(
            &self,
            header: &Header,
            start: u128,
            attempts: u64,
            stop: &AtomicBool,
            tried: &AtomicU64,
        ) -> Option<u128> {
            self.0.fetch_add(1, Ordering::Relaxed);
            mining::search(header, start, attempts, stop, tried)
        }
        fn describe(&self) -> serde_json::Value {
            json!({"kind":"counting"})
        }
    }
    let dir = Directory::new();
    dir.config(false, "fork");
    let shutdown = Arc::new(AtomicBool::new(false));
    let (handle, worker) =
        Handle::spawn_with(dir.0.clone(), shutdown.clone(), Box::new(LastIn)).unwrap();
    let chain_id = config::genesis(&dir.0).unwrap().chain_id();
    let first = support::tx();
    let mut second = support::tx();
    if let Payload::Deploy { input, .. } = &mut second.payload {
        *input = vec![1];
    }
    for tx in [&first, &second] {
        handle
            .call_blocking("submit", json!({"hex":hex(&tx.encode())}))
            .unwrap();
    }
    // Producer data comes from the configuration and must be hex.
    assert!(
        handle
            .call_blocking("miner_config", json!({"enabled":false,"producer":"xyz"}))
            .is_err()
    );
    let configured = handle
        .call_blocking(
            "miner_config",
            json!({"enabled":false,"producer":"6d696e6572"}),
        )
        .unwrap();
    assert_eq!(configured["producer_hex"], "6d696e6572");
    assert_eq!(configured["selection"]["kind"], "last-in");
    assert_eq!(
        config::Config::load(&dir.0).unwrap().producer(),
        b"miner".to_vec()
    );
    // The template takes the fork's order.
    let template = handle.call_blocking("template", json!({})).unwrap();
    assert_eq!(
        template["transactions"],
        json!([hex(&second.id(chain_id)), hex(&first.id(chain_id))])
    );
    // The built-in miner drives the fork's search and the chain's templates.
    let search = Arc::new(Counting(AtomicU64::new(0)));
    let miner = Miner::start(handle.clone(), search.clone(), 0);
    assert_eq!(miner.status()["search"]["kind"], "counting");
    let deadline = Instant::now() + Duration::from_secs(30);
    while miner.status()["blocks"].as_u64().unwrap_or(0) < 1 {
        assert!(Instant::now() < deadline, "no block: {}", miner.status());
        std::thread::sleep(Duration::from_millis(50));
    }
    miner.stop();
    assert!(search.0.load(Ordering::Relaxed) >= 1);
    let block = handle.call_blocking("block", json!({"id":"1"})).unwrap();
    assert_eq!(block["producer_hex"], "6d696e6572");
    assert_eq!(block["transactions"], template["transactions"]);
    assert_eq!(
        handle.call_blocking("status", json!({})).unwrap()["producer_hex"],
        "6d696e6572"
    );
    shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
    drop(handle);
    worker.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_go_on_beside_a_busy_chain_thread_and_rest_cannot_crowd_out_peers() {
    use std::{
        sync::{
            atomic::Ordering,
            mpsc::{Receiver, Sender, channel},
        },
        time::Duration,
    };
    use ting_core::vm::Candidate;
    use ting_node::{
        chain::Handle,
        mining::{self, Selector},
        reads,
    };
    // A selection that, once armed, holds the chain thread inside a template
    // until it is released: the thread is then busy with nothing else.
    struct Held {
        armed: Arc<AtomicBool>,
        entered: Sender<()>,
        release: Receiver<()>,
    }
    impl Selector for Held {
        fn select(&mut self, pool: &[mining::Pending<'_>], block: &mut Candidate<'_>) {
            if self.armed.load(Ordering::Relaxed) {
                let _ = self.entered.send(());
                let _ = self.release.recv();
            }
            for pending in pool {
                let _ = block.add(pending.tx);
            }
        }
        fn describe(&self) -> serde_json::Value {
            json!({"kind":"held"})
        }
    }
    let dir = Directory::new();
    dir.config(false, "held");
    let (armed, (entered_tx, entered), (release, release_rx)) =
        (Arc::new(AtomicBool::new(false)), channel(), channel());
    let shutdown = Arc::new(AtomicBool::new(false));
    let (handle, worker) = Handle::spawn_with(
        dir.0.clone(),
        shutdown.clone(),
        Box::new(Held {
            armed: armed.clone(),
            entered: entered_tx,
            release: release_rx,
        }),
    )
    .unwrap();
    let chain_id = config::genesis(&dir.0).unwrap().chain_id();
    // A block deploying a program, mined through a template and a solution.
    let tx = support::tx();
    handle
        .call("submit", json!({"hex":hex(&tx.encode())}))
        .await
        .unwrap();
    let template = handle.call("template", json!({})).await.unwrap();
    let header =
        Header::decode(&ting_node::unhex(template["header_hex"].as_str().unwrap(), 294).unwrap())
            .unwrap();
    let nonce = mining::search(
        &header,
        0,
        1_000_000,
        &AtomicBool::new(false),
        &Default::default(),
    )
    .unwrap();
    handle
        .call(
            "solution",
            json!({"template":template["id"],"nonce":nonce.to_string()}),
        )
        .await
        .unwrap();
    let contract =
        handle.call("block", json!({"id":"1"})).await.unwrap()["receipts"][0]["result_hex"]
            .as_str()
            .unwrap()
            .to_string();
    // The chain thread is held inside the next template.
    armed.store(true, Ordering::Relaxed);
    let held = tokio::spawn({
        let handle = handle.clone();
        async move { handle.call("template", json!({})).await }
    });
    tokio::task::spawn_blocking(move || entered.recv().unwrap())
        .await
        .unwrap();
    // Reads of stored data are answered meanwhile, from the database, and
    // as the thread itself answers them.
    let view = handle.reader().view().unwrap();
    let block = reads::answer(&view, "block", &json!({"id":"1"})).unwrap();
    assert_eq!(block["transactions"], json!([hex(&tx.id(chain_id))]));
    assert_eq!(block["canonical"], true);
    let stored = reads::answer(&view, "contract", &json!({"id":contract})).unwrap();
    assert_eq!(stored["id"], contract);
    let page = reads::answer(&view, "blocks", &json!({})).unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["tip"], block["id"]);
    // So is status, as the thread last published it.
    let status = handle.status();
    assert_eq!(status.status.as_ref().as_ref().unwrap()["height"], 1);
    // And a peer's header page and block slice, on a reader thread.
    let peers = reads::Readers::new(handle.reader().clone());
    let genesis = config::genesis(&dir.0).unwrap().block_id();
    let page = peers
        .answer("headers", json!({"locator":[hex(&genesis)]}))
        .await
        .unwrap();
    let headers = page["headers"].as_array().unwrap();
    assert_eq!(headers.len(), 1);
    let first =
        Header::decode(&ting_node::unhex(headers[0].as_str().unwrap(), 294).unwrap()).unwrap();
    assert_eq!(json!(hex(&first.id())), block["id"]);
    let raw = peers.answer("raw", json!({"id":"1"})).await.unwrap();
    assert_eq!(raw["offset"], 0);
    assert_eq!(raw["total"], block["bytes"]);
    // REST's share of the queue fills and refuses at once; peers keep theirs.
    let mut queued = Vec::new();
    for _ in 0..128 {
        let handle = handle.clone();
        queued.push(tokio::spawn(async move {
            handle.call_rest("mempool", json!({})).await
        }));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        handle.call_rest("mempool", json!({})),
    )
    .await
    .expect("refused at once, not queued");
    assert_eq!(refused.unwrap_err(), "node request queue unavailable");
    let peer = tokio::spawn({
        let handle = handle.clone();
        async move { handle.call_external_wire(wire::Request::State).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!peer.is_finished(), "a peer's request waits its turn");
    // Released, the thread answers every queued request.
    release.send(()).unwrap();
    held.await.unwrap().unwrap();
    assert!(peer.await.unwrap().is_ok());
    for request in queued {
        assert!(request.await.unwrap().is_ok());
    }
    shutdown.store(true, Ordering::Relaxed);
    drop(handle);
    tokio::task::spawn_blocking(move || worker.join().unwrap())
        .await
        .unwrap();
}

#[test]
fn blocks_ahead_of_the_clock_wait_for_it() {
    use ting_node::chain::{AHEAD_OF_CLOCK, CLOCK_SLACK};
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let r = Runtime::new(State::default(), Context::from_genesis(&g));
    let build = |timestamp: u64| {
        let mut b = r
            .build_candidate(&g, &[], vec![], timestamp, [0; 4], b"A".to_vec())
            .unwrap();
        while !consensus::valid_pow(&b.header) {
            b.header.nonce += 1;
        }
        b
    };
    let now = ting_node::now();
    // Valid, but further ahead than the slack: refused for now, not stored.
    let early = build(now + CLOCK_SLACK + 5);
    assert_eq!(
        chain.import(&early.encode(), true).unwrap_err(),
        AHEAD_OF_CLOCK
    );
    assert_eq!(
        chain
            .request_with(
                "check_header",
                serde_json::Value::Null,
                Some(early.header.encode())
            )
            .unwrap_err(),
        AHEAD_OF_CLOCK
    );
    assert_eq!(chain.height(), 0);
    assert_eq!(
        chain.request("status", json!({})).unwrap()["stored_blocks"],
        0
    );
    // Validity comes first: an invalid header from the future is invalid.
    let mut wrong = early.clone();
    wrong.header.parent = [1; 32];
    assert_eq!(
        chain.import(&wrong.encode(), true).unwrap_err(),
        "unknown parent"
    );
    // Within the slack it is an ordinary block.
    let soon = build(now + CLOCK_SLACK - 60);
    assert_eq!(
        chain.import(&soon.encode(), true).unwrap()["canonical"],
        true
    );
    assert_eq!(chain.height(), 1);
}

/// A transaction distinct from `support::tx()` by its input, and so by its id.
fn variant(input: Vec<u8>) -> ting_core::protocol::Transaction {
    let mut tx = support::tx();
    if let ting_core::protocol::Payload::Deploy { input: i, .. } = &mut tx.payload {
        *i = input;
    }
    tx
}
fn call(target: &str, input: Vec<u8>) -> ting_core::protocol::Transaction {
    ting_core::protocol::Transaction {
        budget: ting_core::meter::Resources([1_000_000, 128, 16_384, 16_384]),
        payload: ting_core::protocol::Payload::Call {
            target: ting_node::hash_arg(target).unwrap(),
            input,
        },
    }
}
/// Solves the chain's current template for exactly `ids` and imports it.
fn solve(chain: &mut Chain, ids: &[&str]) -> serde_json::Value {
    use std::sync::atomic::{AtomicBool, AtomicU64};
    let template = chain
        .request("template", json!({"transactions":ids}))
        .unwrap();
    let header =
        Header::decode(&ting_node::unhex(template["header_hex"].as_str().unwrap(), 294).unwrap())
            .unwrap();
    let nonce = ting_node::mining::search(
        &header,
        0,
        1_000_000,
        &AtomicBool::new(false),
        &AtomicU64::new(0),
    )
    .unwrap();
    chain
        .request(
            "solution",
            json!({"template":template["id"],"nonce":nonce.to_string()}),
        )
        .unwrap()
}

#[test]
fn status_headers_check_header_and_import_answer_exactly() {
    let dir = Directory::new();
    dir.config(false, "bounds");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let status = chain.request("status", json!({})).unwrap();
    assert_eq!(status["limits"], json!(g.initial.0));
    assert_eq!(status["resource_order"], json!(["C", "S", "D", "G"]));
    assert_eq!(status["genesis"], hex(&g.block_id()));
    assert_eq!(status["chain_id"], hex(&g.chain_id()));
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["mempool_bytes"], 0);
    assert_eq!(status["state_records"], 0);
    assert_eq!(status["name"], "bounds");
    let tx = support::tx();
    chain.submit(&tx.encode()).unwrap();
    assert_eq!(
        chain.request("status", json!({})).unwrap()["mempool_bytes"],
        tx.encode().len()
    );
    let mut metrics = chain.request("metrics", json!({})).unwrap();
    let mut status = chain.request("status", json!({})).unwrap();
    for document in [&mut metrics, &mut status] {
        document.as_object_mut().unwrap().remove("uptime_seconds");
    }
    assert_eq!(metrics, status);
    chain.mine(1, 100_000).unwrap();
    // The deployment wrote its manifest, its code and one record.
    assert_eq!(
        chain.request("status", json!({})).unwrap()["state_records"],
        3
    );
    // Headers: the locator's shape is checked before it is searched.
    for (params, error) in [
        (json!({}), "expected locator"),
        (json!({"locator":[]}), "locator limit"),
        (json!({"locator":vec![hex(&[0; 32]); 33]}), "locator limit"),
        (json!({"locator":[5]}), "expected locator hash"),
        (json!({"locator":[hex(&[9; 32])]}), "no common ancestor"),
    ] {
        assert_eq!(chain.request("headers", params).unwrap_err(), error);
    }
    let page = chain
        .request("headers", json!({"locator":[hex(&g.block_id())]}))
        .unwrap();
    let headers = page["headers"].as_array().unwrap();
    assert_eq!(headers.len(), 1);
    let first =
        Header::decode(&ting_node::unhex(headers[0].as_str().unwrap(), 294).unwrap()).unwrap();
    assert_eq!((first.height, first.id()), (1, chain.tip()));
    // A header the node stores is known without being validated again; a
    // stranger is validated, and a bad encoding is refused as such.
    let check = |chain: &mut Chain, bytes: Vec<u8>| {
        chain.request_with("check_header", serde_json::Value::Null, Some(bytes))
    };
    assert_eq!(
        check(&mut chain, first.encode()).unwrap(),
        json!({"id":hex(&chain.tip()),"known":true})
    );
    let mut orphan = first.clone();
    orphan.parent = [1; 32];
    orphan.height = 2;
    assert_eq!(
        check(&mut chain, orphan.encode()).unwrap_err(),
        "unknown parent"
    );
    // A header that does not decode is invalid data, as in a block.
    for bad in [vec![0; 293], vec![0xff; 294], vec![0; 295]] {
        assert!(
            check(&mut chain, bad)
                .unwrap_err()
                .starts_with("invalid block: ")
        );
    }
    // Import: too short to hold a header; a block already stored; a valid
    // header whose body is larger than the data capacity.
    assert_eq!(
        chain.request("import", json!({"hex":"00"})).unwrap_err(),
        "invalid block: truncated header"
    );
    let stored = chain.request("raw", json!({"id":"1"})).unwrap();
    assert_eq!(
        chain
            .request("import", json!({"hex":stored["hex"]}))
            .unwrap(),
        json!({"id":hex(&chain.tip()),"known":true,"canonical":true,"height":1})
    );
    let mut oversized = Header {
        height: 2,
        parent: first.id(),
        timestamp: first.timestamp + 1,
        target: consensus::expected_target(&g, std::slice::from_ref(&first)),
        used: ting_core::meter::Resources::ZERO,
        nonce: 0,
        ..first.clone()
    };
    while !consensus::valid_pow(&oversized) {
        oversized.nonce += 1;
    }
    let mut bytes = oversized.encode();
    bytes.resize(g.initial.0[2] as usize + 1, 0);
    assert_eq!(
        chain.import(&bytes, true).unwrap_err(),
        "invalid block: block byte limit"
    );
    assert_eq!(chain.height(), 1);
}

#[test]
fn submissions_answer_known_deferred_or_refused_and_the_pool_is_bounded() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let tx = support::tx();
    let encoded = hex(&tx.encode());
    for (params, error) in [
        (json!({"hex":"AB"}), "hex must be lowercase"),
        (json!({"hex":"abc"}), "invalid hex length"),
        (
            json!({"hex":encoded,"not_before":"soon"}),
            "expected not_before seconds",
        ),
        (
            json!({"hex":encoded,"not_before":-1}),
            "expected not_before seconds",
        ),
        (json!({"hex":"00".repeat(1_048_577)}), "invalid hex length"),
    ] {
        assert_eq!(chain.request("submit", params).unwrap_err(), error);
    }
    assert_eq!(chain.submit(&tx.encode()).unwrap()["known"], false);
    assert_eq!(chain.submit(&tx.encode()).unwrap()["known"], true);
    let id = hex(&tx.id(chain.genesis.chain_id()));
    assert_eq!(
        chain.submit_deferred(&tx.encode(), u64::MAX).unwrap(),
        json!({"id":id,"known":true,"deferred":false})
    );
    // A deferral: expiry thirty days after its time; the horizon is a year.
    let later = variant(vec![1]);
    let now = ting_node::now();
    let answer = chain.submit_deferred(&later.encode(), now + 1000).unwrap();
    assert_eq!(answer["deferred"], true);
    let expires = answer["expires"].as_u64().unwrap();
    assert!(
        (now + 1000 + ting_node::chain::DEFERRED_GRACE
            ..=now + 1001 + ting_node::chain::DEFERRED_GRACE)
            .contains(&expires)
    );
    assert_eq!(
        chain
            .submit_deferred(
                &variant(vec![2]).encode(),
                now + ting_node::chain::DEFERRED_HORIZON + 5
            )
            .unwrap_err(),
        ting_node::chain::HORIZON
    );
    // Submitting a deferred transaction again changes nothing, with or
    // without a time: it stays out of the pool until it is due.
    let plain = chain.submit(&later.encode()).unwrap();
    assert_eq!(
        (plain["known"].clone(), plain["deferred"].clone()),
        (json!(true), json!(true))
    );
    assert_eq!(
        chain.submit_deferred(&later.encode(), u64::MAX).unwrap()["known"],
        true
    );
    let status = chain.request("status", json!({})).unwrap();
    assert_eq!(
        (
            status["mempool_count"].clone(),
            status["deferred_count"].clone()
        ),
        (json!(1), json!(1))
    );
    assert_eq!(
        chain.request("template", json!({})).unwrap()["transactions"],
        json!([id])
    );
    // The pool holds 1,024 transactions whatever they declare: a template
    // runs each against what is left of its block. Then it refuses the next,
    // since none of them came from a peer that could give up its place. A
    // budget larger than a block is refused whatever the room.
    for i in 0..1_023u32 {
        let mut tx = variant(i.to_le_bytes().to_vec());
        if i < 5 {
            tx.budget.0[0] = chain.genesis.initial.0[0];
        }
        chain.submit(&tx.encode()).unwrap();
    }
    assert_eq!(
        chain.submit(&variant(vec![9; 9]).encode()).unwrap_err(),
        ting_node::chain::POOL_FULL
    );
    assert_eq!(
        chain.request("status", json!({})).unwrap()["mempool_count"],
        1_024
    );
}

#[test]
fn the_pool_shares_its_room_by_address() {
    use std::net::IpAddr;
    // A pool of eight, as an operator may configure it.
    let dir = Directory::new();
    dir.config(false, "test");
    let mut c = config::Config::load(&dir.0).unwrap();
    c.mempool_transactions = 8;
    c.save(&dir.0).unwrap();
    let mut chain = open(&dir);
    let address = |n: u8| -> IpAddr { format!("10.0.0.{n}").parse().unwrap() };
    let tx = |source: u8, i: u16| {
        let mut input = i.to_le_bytes().to_vec();
        input.push(source);
        variant(input)
    };
    let id = |tx: &ting_core::protocol::Transaction| {
        hex(&tx.id(config::genesis(&dir.0).unwrap().chain_id()))
    };
    let pooled = |chain: &mut Chain, tx: &ting_core::protocol::Transaction| {
        chain.request("transaction", json!({"id":id(tx)})).is_ok()
    };
    let count =
        |chain: &mut Chain| chain.request("status", json!({})).unwrap()["mempool_count"].clone();
    // One address alone may fill it: no share is held back.
    for i in 0..8 {
        chain
            .submit_from(&tx(1, i).encode(), Some(address(1)))
            .unwrap();
    }
    assert_eq!(count(&mut chain), 8);
    // Full: a newcomer takes the place of the most recent transaction of the
    // address holding the most, as long as that holds more than its own.
    for i in 0..4 {
        chain
            .submit_from(&tx(2, i).encode(), Some(address(2)))
            .unwrap();
        assert!(!pooled(&mut chain, &tx(1, 7 - i)));
    }
    assert_eq!(
        chain
            .submit_from(&tx(2, 4).encode(), Some(address(2)))
            .unwrap_err(),
        ting_node::chain::POOL_FULL
    );
    // A third address takes a place from one of the two holding four.
    chain
        .submit_from(&tx(3, 0).encode(), Some(address(3)))
        .unwrap();
    assert_eq!(count(&mut chain), 8);
    // One submitted directly counts against no address and takes a place too,
    // and never gives up its own.
    chain.submit(&variant(vec![7; 7]).encode()).unwrap();
    assert_eq!(count(&mut chain), 8);
    assert!(pooled(&mut chain, &variant(vec![7; 7])));
    // The byte bound is the configured MiB or four blocks' data, the larger.
    let status = chain.request("status", json!({})).unwrap();
    assert!(status["mempool_bytes"].as_u64().unwrap() < 4 * 1_048_576);
}
#[test]
fn a_fork_chooses_what_gives_way_in_a_full_pool() {
    use ting_core::vm::Candidate;
    use ting_node::{
        chain::Handle,
        mining::{self, Selector},
    };
    // A fork's policy: the oldest pooled transaction gives way, unless the
    // newcomer's input starts with 0, which is refused.
    struct OldestOut;
    impl Selector for OldestOut {
        fn select(&mut self, pool: &[mining::Pending<'_>], block: &mut Candidate<'_>) {
            for pending in pool {
                let _ = block.add(pending.tx);
            }
        }
        fn describe(&self) -> serde_json::Value {
            json!({"kind":"oldest-out"})
        }
        fn give_way(
            &mut self,
            pool: &[mining::Pending<'_>],
            incoming: &mining::Pending<'_>,
            _block: &mut Candidate<'_>,
        ) -> Option<usize> {
            let ting_core::protocol::Payload::Deploy { input, .. } = &incoming.tx.payload else {
                return None;
            };
            (input.first() != Some(&0) && !pool.is_empty()).then_some(0)
        }
    }
    let dir = Directory::new();
    dir.config(false, "fork");
    let mut c = config::Config::load(&dir.0).unwrap();
    c.mempool_transactions = 3;
    c.save(&dir.0).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let (handle, worker) =
        Handle::spawn_with(dir.0.clone(), shutdown.clone(), Box::new(OldestOut)).unwrap();
    let chain_id = config::genesis(&dir.0).unwrap().chain_id();
    let txs: Vec<_> = (1..=4u8).map(|i| variant(vec![i])).collect();
    for tx in &txs {
        handle
            .call_blocking("submit", json!({"hex":hex(&tx.encode())}))
            .unwrap();
    }
    let pooled = |tx: &ting_core::protocol::Transaction| {
        handle
            .call_blocking("transaction", json!({"id":hex(&tx.id(chain_id))}))
            .is_ok()
    };
    // The fourth took the first's place; one the policy refuses is refused.
    assert!(!pooled(&txs[0]));
    assert!(txs[1..].iter().all(pooled));
    assert_eq!(
        handle
            .call_blocking("submit", json!({"hex":hex(&variant(vec![0]).encode())}))
            .unwrap_err(),
        ting_node::chain::POOL_FULL
    );
    shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
    drop(handle);
    worker.join().unwrap();
}
#[test]
fn inventories_and_relay_pages_list_the_pool_and_the_deferred_set_by_id() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let now = ting_node::now();
    for i in 0..40u8 {
        chain.submit(&variant(vec![i]).encode()).unwrap();
        chain
            .submit_deferred(&variant(vec![i, 1]).encode(), now + 1_000)
            .unwrap();
    }
    for kind in ["pending", "deferred"] {
        let deferred = kind == "deferred";
        let Ok(wire::Answer::Inventory { ids, next }) = ask(
            &mut chain,
            wire::Request::Inventory {
                deferred,
                after: None,
            },
        ) else {
            panic!("no inventory");
        };
        assert_eq!((ids.len(), next), (40, None));
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "{kind} in id order");
        // By id: those named, and no more, whole.
        let named = vec![ids[3], ids[17]];
        let request = if deferred {
            wire::Request::Deferred { ids: named.clone() }
        } else {
            wire::Request::Pending { ids: named.clone() }
        };
        let Ok(wire::Answer::Items(items)) = ask(&mut chain, request) else {
            panic!("no items");
        };
        assert_eq!(items.iter().map(|i| i.id).collect::<Vec<_>>(), named);
        assert!(
            items
                .iter()
                .all(|i| matches!(i.body, wire::Body::Whole(_))
                    && i.not_before.is_some() == deferred)
        );
    }
}
#[test]
fn deferred_entries_stay_through_failed_inclusions_and_come_back_with_their_block() {
    use ting_core::{
        meter::Resources,
        protocol::{Payload, Transaction},
    };
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let chain_id = g.chain_id();
    let deploy = support::tx();
    let contract = {
        let mut bytes = chain_id.to_vec();
        bytes.extend(deploy.id(chain_id));
        bytes.extend(0u64.to_le_bytes());
        ting_core::hash::hash("contract", &bytes)
    };
    let call = Transaction {
        budget: Resources([1_000_000, 128, 16_384, 16_384]),
        payload: Payload::Call {
            target: contract,
            input: vec![],
        },
    };
    let call_id = hex(&call.id(chain_id));
    chain.submit_deferred(&call.encode(), 0).unwrap();
    let deferred = |chain: &mut Chain| {
        chain.request("status", json!({})).unwrap()["deferred_count"]
            .as_u64()
            .unwrap()
    };
    // A block that includes it while it fails, before the program exists,
    // leaves the entry in place.
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut history = vec![];
    let failed = mine(&runtime, &g, &mut history, vec![call.clone()], "A");
    assert_ne!(
        failed.receipts[0].status,
        ting_core::protocol::Status::Success
    );
    let deployed = mine(&runtime, &g, &mut history, vec![deploy.clone()], "A");
    let succeeded = mine(&runtime, &g, &mut history, vec![call.clone()], "A");
    assert_eq!(
        succeeded.receipts[0].status,
        ting_core::protocol::Status::Success
    );
    chain.import(&failed.encode(), true).unwrap();
    assert_eq!(deferred(&mut chain), 1);
    chain.import(&deployed.encode(), true).unwrap();
    // Included with success above its submission, it leaves.
    chain.import(&succeeded.encode(), true).unwrap();
    assert_eq!(deferred(&mut chain), 0);
    // A heavier branch without those blocks disconnects them: the entry
    // comes back to the deferred set, not into the pool.
    for block in branch(&g, 4, "B") {
        chain.import(&block.encode(), true).unwrap();
    }
    assert_eq!(chain.height(), 4);
    assert_eq!(deferred(&mut chain), 1);
    let lookup = chain.request("transaction", json!({"id":call_id})).unwrap();
    assert_eq!(lookup["pending"], false);
    assert_ne!(lookup["deferred"], serde_json::Value::Null);
}
#[test]
fn the_deferred_set_shares_its_room_by_address_and_keeps_what_is_due_first() {
    use std::net::IpAddr;
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let now = ting_node::now();
    let address = |n: u8| -> IpAddr { format!("10.0.1.{n}").parse().unwrap() };
    let tx = |source: u8, i: u16| {
        let mut input = i.to_le_bytes().to_vec();
        input.extend([source, 0xd]);
        variant(input)
    };
    // 256 from one address, then no more from it.
    for i in 0..256 {
        chain
            .submit_deferred_from(
                &tx(1, i).encode(),
                now + 1_000 + u64::from(i),
                Some(address(1)),
            )
            .unwrap();
    }
    assert_eq!(
        chain
            .submit_deferred_from(&tx(1, 256).encode(), now + 10, Some(address(1)))
            .unwrap_err(),
        ting_node::chain::DEFERRED_FULL
    );
    // Fifteen more addresses fill the set.
    for source in 2..=16u8 {
        for i in 0..256 {
            chain
                .submit_deferred_from(&tx(source, i).encode(), now + 500, Some(address(source)))
                .unwrap();
        }
    }
    assert_eq!(
        chain.request("status", json!({})).unwrap()["deferred_count"],
        4_096
    );
    // Full: an entry due earlier takes the place of the relayed one due
    // latest; one due later than all of them is refused.
    let latest = hex(&tx(1, 255).id(chain.genesis.chain_id()));
    chain
        .submit_deferred(&variant(vec![1, 2, 3]).encode(), now + 100)
        .unwrap();
    assert_eq!(
        chain
            .request("transaction", json!({"id":latest}))
            .unwrap_err(),
        "transaction not found"
    );
    assert_eq!(
        chain
            .submit_deferred(&variant(vec![4, 5, 6]).encode(), now + 5_000)
            .unwrap_err(),
        ting_node::chain::DEFERRED_FULL
    );
}
#[test]
fn only_header_and_block_checks_prove_invalid_data() {
    use ting_node::chain::is_invalid;
    for refusal in [
        "invalid hex length",
        "invalid block location",
        "invalid block slice",
        "invalid header data",
        "transaction already included",
    ] {
        assert!(!is_invalid(refusal), "{refusal}");
    }
    for proof in [
        "invalid header: proof of work",
        "invalid block: body commitment mismatch",
    ] {
        assert!(is_invalid(proof), "{proof}");
    }
}
#[test]
fn reconsidering_a_mark_clears_every_branch_above_it() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let a = branch(&g, 2, "A");
    // A second branch from the first block.
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    runtime.validate_and_apply_block(&g, &[], &a[0]).unwrap();
    let mut history = vec![a[0].header.clone()];
    let y: Vec<Block> = (0..2)
        .map(|_| mine(&runtime, &g, &mut history, vec![], "Y"))
        .collect();
    for block in a.iter().chain(&y[..1]) {
        chain.import(&block.encode(), true).unwrap();
    }
    chain
        .request("invalidate", json!({"id":hex(&a[0].header.id())}))
        .unwrap();
    assert_eq!(chain.height(), 0);
    // Reconsidered through one branch, the mark goes, and the other branch
    // is no longer invalid: its next block is taken.
    chain
        .request("reconsider", json!({"id":hex(&a[1].header.id())}))
        .unwrap();
    chain.import(&y[1].encode(), true).unwrap();
    assert_eq!(chain.tip(), y[1].header.id());
}
#[test]
fn the_pool_keeps_what_new_blocks_do_not_include() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let tx = support::tx();
    let id = hex(&tx.id(chain.genesis.chain_id()));
    chain.submit(&tx.encode()).unwrap();
    chain.mine(1, 100_000).unwrap();
    // Mined once, the same envelope may be submitted and mined again; an
    // empty block in between leaves it in the pool.
    assert_eq!(chain.submit(&tx.encode()).unwrap()["known"], false);
    solve(&mut chain, &[]);
    assert_eq!(chain.height(), 2);
    assert_eq!(
        chain.request("status", json!({})).unwrap()["mempool_count"],
        1
    );
    chain.mine(1, 100_000).unwrap();
    assert_eq!(
        chain.request("status", json!({})).unwrap()["mempool_count"],
        0
    );
    let lookup = chain.request("transaction", json!({"id":id})).unwrap();
    let occurrences = lookup["occurrences"].as_array().unwrap();
    assert_eq!(occurrences.len(), 2);
    assert_eq!(occurrences[0]["height"], 1);
    assert_eq!(occurrences[1]["height"], 3);
    assert_eq!(occurrences[0]["receipt"]["status"], 0);
    assert_eq!(occurrences[1]["receipt"]["status"], 10);
    assert_eq!(lookup["pending"], false);
}

#[test]
fn deferred_entries_outlive_restarts_but_not_expiry_corruption_or_inclusion() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let now = ting_node::now();
    let (a, b, c) = (variant(vec![1]), variant(vec![2]), variant(vec![3]));
    chain.submit_deferred(&a.encode(), now + 500).unwrap();
    chain.submit_deferred(&b.encode(), now + 500).unwrap();
    drop(chain);
    // On disk: a expires, b no longer decodes, one item is nonsense, c is fine.
    let path = dir.0.join("deferred.json");
    let mut file: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let items = file["items"].as_array_mut().unwrap();
    assert_eq!(items.len(), 2);
    items[0]["expires"] = json!(now - 1);
    items[1]["hex"] = json!("zz");
    items.push(json!({"hex":hex(&c.encode()),"not_before":now + 500,"expires":now + 600}));
    items.push(json!({"nonsense":1}));
    fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    let mut chain = open(&dir);
    assert_eq!(
        chain.request("status", json!({})).unwrap()["deferred_count"],
        1
    );
    let lookup = chain
        .request(
            "transaction",
            json!({"id":hex(&c.id(chain.genesis.chain_id()))}),
        )
        .unwrap();
    assert_eq!(lookup["deferred"]["not_before"], now + 500);
    assert_eq!(lookup["pending"], false);
    assert_eq!(
        lookup["pending_transaction"]["transaction"]["kind"],
        "deploy"
    );
    // A transaction mined once can be deferred again: only a block above the
    // height it was deferred at counts as including it.
    chain.submit(&support::tx().encode()).unwrap();
    chain.mine(1, 100_000).unwrap();
    let contract = chain
        .request(
            "transaction",
            json!({"id":hex(&support::tx().id(chain.genesis.chain_id()))}),
        )
        .unwrap()["occurrences"][0]["receipt"]["result_hex"]
        .as_str()
        .unwrap()
        .to_owned();
    let again = call(&contract, vec![]);
    let again_id = hex(&again.id(chain.genesis.chain_id()));
    chain.submit(&again.encode()).unwrap();
    chain.mine(1, 100_000).unwrap();
    assert_eq!(chain.height(), 2);
    assert_eq!(
        chain.submit_deferred(&again.encode(), now).unwrap()["deferred"],
        true
    );
    chain.tick().unwrap();
    let status = chain.request("status", json!({})).unwrap();
    assert_eq!(status["deferred_count"], 2);
    assert_eq!(status["mempool_count"], 1, "due and successful: pooled");
    chain.mine(1, 100_000).unwrap();
    chain.tick().unwrap();
    let status = chain.request("status", json!({})).unwrap();
    assert_eq!(
        status["deferred_count"], 1,
        "included above its height: gone"
    );
    assert_eq!(status["mempool_count"], 0);
    let lookup = chain
        .request("transaction", json!({"id":again_id}))
        .unwrap();
    assert_eq!(lookup["occurrences"].as_array().unwrap().len(), 2);
    assert_eq!(lookup["deferred"], serde_json::Value::Null);
}

#[test]
fn lookups_pages_and_slices_hold_their_bounds() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    // Block 1 deploys the storing program; block 2 deploys and calls an
    // emitter, so there are events to page.
    chain.submit(&support::tx().encode()).unwrap();
    chain.mine(1, 100_000).unwrap();
    let emitter = ting_core::protocol::Transaction {
        budget: ting_core::meter::Resources([1_000_000, 128, 16_384, 16_384]),
        payload: ting_core::protocol::Payload::Deploy {
            code: wat::parse_str(
                r#"(module
  (import "ting" "input" (func (param i32 i32) (result i32)))
  (import "ting" "emit" (func (param i32 i32)))
  (memory (export "memory") 1 1)
  (func (export "invoke") (call 1 (i32.const 0) (call 0 (i32.const 0) (i32.const 64)))))"#,
            )
            .unwrap(),
            input: vec![],
        },
    };
    let mut bytes = g.chain_id().to_vec();
    bytes.extend(emitter.id(g.chain_id()));
    bytes.extend(0u64.to_le_bytes());
    let emitter_id = hex(&ting_core::hash::hash("contract", &bytes));
    let shout = call(&emitter_id, b"hi".to_vec());
    chain.submit(&emitter.encode()).unwrap();
    chain.submit(&shout.encode()).unwrap();
    chain.mine(1, 100_000).unwrap();
    // Block identifiers: a height in digits, or a block id.
    for (id, error) in [
        ("0", "genesis has no ordinary block body"),
        ("9", "block not found"),
        ("+1", "hex must be lowercase"),
        (&hex(&[9; 32]), "block not found"),
    ] {
        assert_eq!(
            chain.request("block", json!({"id":id})).unwrap_err(),
            error,
            "{id}"
        );
    }
    let one = chain.request("block", json!({"id":"1"})).unwrap();
    assert_eq!(
        chain.request("block", json!({"id":one["id"]})).unwrap(),
        one
    );
    assert_eq!(one["canonical"], true);
    // A validated side-branch block is found by its id and says it is not canonical.
    let other = Runtime::new(State::default(), Context::from_genesis(&g));
    let side = mine(&other, &g, &mut vec![], vec![], "side");
    assert_eq!(
        chain.import(&side.encode(), true).unwrap()["canonical"],
        false
    );
    let found = chain
        .request("block", json!({"id":hex(&side.header.id())}))
        .unwrap();
    assert_eq!(
        (found["canonical"].clone(), found["height"].clone()),
        (json!(false), json!(1))
    );
    assert_eq!(
        chain.request("block", json!({"id":"1"})).unwrap()["id"],
        one["id"]
    );
    // Raw slices.
    let whole = chain.request("raw", json!({"id":"2"})).unwrap();
    let total = whole["total"].as_u64().unwrap();
    assert_eq!(
        chain
            .request("raw", json!({"id":"2","offset":total}))
            .unwrap()["hex"],
        ""
    );
    for (params, error) in [
        (
            json!({"id":"2","offset":total + 1}),
            "slice offset out of range",
        ),
        (
            json!({"id":"2","length":ting_node::chain::SLICE_MAX + 1}),
            "slice length limit",
        ),
        (json!({"id":"2","offset":"5"}), "expected integer offset"),
        (json!({"id":"2","length":-1}), "expected integer length"),
    ] {
        assert_eq!(chain.request("raw", params).unwrap_err(), error);
    }
    let slice = chain
        .request("raw", json!({"id":"2","offset":10,"length":5}))
        .unwrap();
    assert_eq!(slice["hex"], whole["hex"].as_str().unwrap()[20..30]);
    assert_eq!(slice["offset"], 10);
    // Pages: `from` starts at 1, past the tip a page is empty, `limit` may
    // be 0 and at most 100, and both must be integers.
    for method in ["blocks", "events"] {
        let page = chain.request(method, json!({"from":0})).unwrap();
        assert_eq!(page["from"], 1);
        assert_eq!(page["items"].as_array().unwrap().len(), 2);
        assert_eq!(page["next"], 3);
        assert_eq!(page["tip"], hex(&chain.tip()));
        let beyond = chain.request(method, json!({"from":7})).unwrap();
        assert_eq!(
            (beyond["items"].clone(), beyond["next"].clone()),
            (json!([]), json!(7))
        );
        assert_eq!(
            chain.request(method, json!({"limit":0})).unwrap()["items"],
            json!([])
        );
        assert_eq!(
            chain.request(method, json!({"limit":101})).unwrap_err(),
            "pagination limit is 100"
        );
        assert_eq!(
            chain.request(method, json!({"limit":"500"})).unwrap_err(),
            "expected integer limit"
        );
        assert_eq!(
            chain.request(method, json!({"from":1.5})).unwrap_err(),
            "expected integer from"
        );
    }
    let events = chain.request("events", json!({"from":2})).unwrap();
    let block_events = &events["items"][0];
    assert_eq!(block_events["height"], 2);
    assert_eq!(
        block_events["timestamp"],
        chain.request("block", json!({"id":"2"})).unwrap()["timestamp"]
    );
    let listed = block_events["events"].as_array().unwrap();
    // The initializer emitted its empty input; the call emitted "hi".
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["transaction_index"], 0);
    assert_eq!(listed[0]["payload_hex"], "");
    assert_eq!(listed[1]["transaction_index"], 1);
    assert_eq!(listed[1]["contract"], emitter_id);
    assert_eq!(listed[1]["payload_hex"], "6869");
    assert_eq!(listed[1]["transaction"], hex(&shout.id(g.chain_id())));
    // Transactions, contracts and storage.
    assert_eq!(
        chain
            .request("transaction", json!({"id":hex(&[9; 32])}))
            .unwrap_err(),
        "transaction not found"
    );
    assert_eq!(
        chain
            .request("transaction", json!({"id":hex(&[9; 32]),"limit":101}))
            .unwrap_err(),
        "pagination limit is 100"
    );
    let shout_id = hex(&shout.id(g.chain_id()));
    let far = chain
        .request("transaction", json!({"id":shout_id,"from":5}))
        .unwrap();
    assert_eq!(
        (far["occurrences"].clone(), far["has_more"].clone()),
        (json!([]), json!(false))
    );
    assert_eq!(
        chain
            .request("transaction", json!({"id":"abc"}))
            .unwrap_err(),
        "invalid hex length"
    );
    assert_eq!(
        chain
            .request("contract", json!({"id":hex(&[9; 32])}))
            .unwrap_err(),
        "contract not found"
    );
    let manifest = chain.request("contract", json!({"id":emitter_id})).unwrap();
    if let ting_core::protocol::Payload::Deploy { code, .. } = &emitter.payload {
        assert_eq!(
            manifest["code_hash"],
            hex(&ting_core::hash::hash("code", code))
        );
        assert_eq!(manifest["code_bytes"], code.len());
        assert_eq!(manifest["code_price"], vm_model::code_price(code));
    }
    assert_eq!(
        manifest["state_root"],
        chain.request("status", json!({})).unwrap()["state_root"]
    );
    for (key, error) in [
        ("zz", "hex must be lowercase"),
        ("00", "expected 32-byte hash"),
    ] {
        assert_eq!(
            chain
                .request("storage", json!({"id":emitter_id,"key":key}))
                .unwrap_err(),
            error
        );
    }
    assert_eq!(
        chain
            .request("storage", json!({"id":emitter_id,"key":hex(&[0; 32])}))
            .unwrap()["value_hex"],
        serde_json::Value::Null
    );
    assert_eq!(
        chain.request("nonsense", json!({})).unwrap_err(),
        "unknown method"
    );
}

#[test]
fn templates_mining_and_configuration_requests_hold_their_bounds() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    for (count, attempts) in [(0, 1), (101, 1), (1, 0), (1, 10_000_001)] {
        assert_eq!(
            chain.mine(count, attempts).unwrap_err(),
            "count must be 1..100 and attempts 1..10000000"
        );
    }
    // Mining is the daemon's, which searches off the chain thread.
    assert_eq!(
        chain.request("mine", json!({})).unwrap_err(),
        "unknown method"
    );
    assert_eq!(
        chain
            .request("template", json!({"transactions":[5]}))
            .unwrap_err(),
        "expected transaction id"
    );
    // Eight templates are kept: the ninth distinct one forgets the first.
    let ids: Vec<String> = (0..4u8)
        .map(|i| {
            let tx = variant(vec![i]);
            chain.submit(&tx.encode()).unwrap();
            hex(&tx.id(chain.genesis.chain_id()))
        })
        .collect();
    let subsets: [&[usize]; 9] = [
        &[],
        &[0],
        &[1],
        &[2],
        &[3],
        &[0, 1],
        &[0, 2],
        &[0, 3],
        &[1, 2],
    ];
    let mut templates = Vec::new();
    for subset in subsets {
        let chosen: Vec<&str> = subset.iter().map(|i| ids[*i].as_str()).collect();
        let template = chain
            .request("template", json!({"transactions":chosen}))
            .unwrap();
        assert_eq!(template["transactions"], json!(chosen));
        templates.push(template);
    }
    let nonce_for = |template: &serde_json::Value| {
        use std::sync::atomic::{AtomicBool, AtomicU64};
        let header = Header::decode(
            &ting_node::unhex(template["header_hex"].as_str().unwrap(), 294).unwrap(),
        )
        .unwrap();
        ting_node::mining::search(
            &header,
            0,
            1_000_000,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
        )
        .unwrap() as u64
    };
    assert_eq!(
        chain
            .request(
                "solution",
                json!({"template":templates[0]["id"],"nonce":nonce_for(&templates[0]).to_string()})
            )
            .unwrap_err(),
        "unknown or expired template"
    );
    // The nonce may be a number or a decimal string; anything else is refused.
    for (params, error) in [
        (json!({"nonce":1}), "expected string template"),
        (
            json!({"template":templates[8]["id"],"nonce":"abc"}),
            "expected nonce",
        ),
        (
            json!({"template":templates[8]["id"],"nonce":true}),
            "expected nonce",
        ),
        (json!({"template":templates[8]["id"]}), "expected nonce"),
    ] {
        assert_eq!(chain.request("solution", params).unwrap_err(), error);
    }
    // Changing the miner's configuration discards the templates.
    chain
        .request("miner_config", json!({"enabled":false,"producer":"00ff"}))
        .unwrap();
    assert_eq!(
        chain
            .request(
                "solution",
                json!({"template":templates[8]["id"],"nonce":nonce_for(&templates[8])})
            )
            .unwrap_err(),
        "unknown or expired template"
    );
    let fresh = chain
        .request("template", json!({"transactions":[ids[1]]}))
        .unwrap();
    let imported = chain
        .request(
            "solution",
            json!({"template":fresh["id"],"nonce":nonce_for(&fresh)}),
        )
        .unwrap();
    assert_eq!(
        (imported["canonical"].clone(), imported["height"].clone()),
        (json!(true), json!(1))
    );
    assert_eq!(
        chain.request("block", json!({"id":"1"})).unwrap()["producer_hex"],
        "00ff"
    );
    // Miner configuration bounds, and an empty producer means the name.
    for (params, error) in [
        (json!({}), "expected enabled boolean"),
        (
            json!({"enabled":false,"threads":0}),
            "invalid configuration",
        ),
        (
            json!({"enabled":false,"threads":65}),
            "invalid configuration",
        ),
        (
            json!({"enabled":false,"producer":5}),
            "expected producer hex",
        ),
        (
            json!({"enabled":false,"producer":"0G"}),
            "invalid configuration",
        ),
        (
            json!({"enabled":false,"producer":"0"}),
            "invalid configuration",
        ),
    ] {
        assert!(
            chain
                .request("miner_config", params.clone())
                .unwrap_err()
                .contains(error),
            "{params}"
        );
    }
    let cleared = chain
        .request("miner_config", json!({"enabled":false,"producer":""}))
        .unwrap();
    // Cleared, a node publishes no producer data, not its name.
    assert_eq!(cleared["producer_hex"], "");
    assert_eq!(config::Config::load(&dir.0).unwrap().producer_hex, None);
    // Peers: the address must parse, duplicates are ignored, 32 is the most.
    assert_eq!(
        chain
            .request("add_peer", json!({"address":"nonsense"}))
            .unwrap_err(),
        "expected IP:port"
    );
    for port in 1..=32 {
        chain
            .request("add_peer", json!({"address":format!("10.0.0.1:{port}")}))
            .unwrap();
    }
    assert_eq!(
        chain
            .request("add_peer", json!({"address":"10.0.0.1:32"}))
            .unwrap()["peers"]
            .as_array()
            .unwrap()
            .len(),
        32
    );
    assert!(
        chain
            .request("add_peer", json!({"address":"10.0.0.1:33"}))
            .unwrap_err()
            .contains("maximum 32 peers")
    );
    assert_eq!(
        chain
            .request("remove_peer", json!({"address":"10.0.0.1:99"}))
            .unwrap()["peers"]
            .as_array()
            .unwrap()
            .len(),
        32
    );
    assert_eq!(config::Config::load(&dir.0).unwrap().peers.len(), 32);
    // The REST flags are saved as given.
    assert_eq!(
        chain
            .request("api_config", json!({"cors":true}))
            .unwrap_err(),
        "expected enabled boolean"
    );
    chain
        .request("api_config", json!({"enabled":true,"cors":true}))
        .unwrap();
    let saved = config::Config::load(&dir.0).unwrap();
    assert!(saved.api_enabled && saved.api_cors);
}

#[test]
fn active_capacities_follow_votes_epoch_by_epoch_through_a_reorganization() {
    use ting_core::meter::Resources;
    // The experimental profile as written: epochs of four blocks.
    let dir = Directory::new();
    let g = Genesis::experimental();
    let path = dir.0.join("genesis.src");
    fs::write(&path, g.encode()).unwrap();
    let c = config::Config {
        name: "voter".into(),
        control: "127.0.0.1:0".parse().unwrap(),
        p2p: "127.0.0.1:0".parse().unwrap(),
        api: "127.0.0.1:0".parse().unwrap(),
        votes: [1, 0, 0, 0],
        ..Default::default()
    };
    config::initialize(&dir.0, &c, Some(&path)).unwrap();
    let mut chain = open(&dir);
    assert_eq!(chain.genesis.vote_epoch, 4);
    let history = |chain: &mut Chain| -> Vec<Header> {
        let page = chain
            .request("headers", json!({"locator":[hex(&g.block_id())]}))
            .unwrap();
        page["headers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| Header::decode(&ting_node::unhex(h.as_str().unwrap(), 294).unwrap()).unwrap())
            .collect()
    };
    let limits = |chain: &mut Chain| -> Resources {
        let status = chain.request("status", json!({})).unwrap();
        Resources(serde_json::from_value(status["limits"].clone()).unwrap())
    };
    let mut seen = std::collections::BTreeSet::new();
    for height in 1..=20 {
        chain.mine(1, 100_000).unwrap();
        assert_eq!(chain.height(), height);
        let active = limits(&mut chain);
        assert_eq!(
            active,
            consensus::active_limits(&g, &history(&mut chain)),
            "{height}"
        );
        seen.insert(active.0[0]);
    }
    // The first increase is active from height 12, the second from 16, the
    // third from 20: four distinct compute capacities so far.
    assert_eq!(seen.len(), 4);
    assert_eq!(
        limits(&mut chain).0[0],
        100_000_000 + 5_000_000 + 5_250_000 + 5_512_500
    );
    // A heavier branch voting the other way replaces the chain, and the
    // active capacities follow the new history, not the memo of the old.
    let other = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut branch = Vec::new();
    for _ in 0..24 {
        let mut b = other
            .build_candidate(
                &g,
                &branch,
                vec![],
                g.timestamp + 60 * (branch.len() as u64 + 1),
                [2, 0, 0, 0],
                b"other".to_vec(),
            )
            .unwrap();
        while !consensus::valid_pow(&b.header) {
            b.header.nonce += 1;
        }
        other.validate_and_apply_block(&g, &branch, &b).unwrap();
        branch.push(b.header.clone());
        chain.import(&b.encode(), true).unwrap();
    }
    assert_eq!(chain.tip(), branch.last().unwrap().id());
    let active = limits(&mut chain);
    assert_eq!(active, consensus::active_limits(&g, &branch));
    assert!(active.0[0] < 100_000_000);
    assert_eq!(active.0[1..], g.initial.0[1..]);
    // The next block the node builds is bounded by the new capacities.
    let template = chain.request("template", json!({})).unwrap();
    assert_eq!(template["height"], 25);
    assert_eq!(
        limits(&mut chain),
        consensus::active_limits(&g, &history(&mut chain))
    );
}

#[test]
fn a_branch_below_the_undo_window_is_reached_by_replaying_from_genesis() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut c = config::Config::load(&dir.0).unwrap();
    c.undo_window = 3;
    c.save(&dir.0).unwrap();
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    // Eight blocks, every other one changing state: the window keeps the undo
    // data of the last three, so that of heights 1 to 5 is gone.
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut history = vec![];
    let a: Vec<Block> = (0..8u8)
        .map(|i| {
            let txs = if i % 2 == 0 {
                vec![variant(vec![i])]
            } else {
                vec![]
            };
            mine(&runtime, &g, &mut history, txs, "A")
        })
        .collect();
    for block in &a {
        chain.import(&block.encode(), true).unwrap();
    }
    assert_eq!(chain.tip(), a[7].header.id());
    assert_eq!(
        chain.request("status", json!({})).unwrap()["undo_pruned"],
        5
    );
    drop(chain);
    // Across a restart too: a branch from genesis with more work.
    let mut chain = open(&dir);
    let b = branch(&g, 10, "B");
    for block in &b {
        chain.import(&block.encode(), true).unwrap();
    }
    // The node stays on its tip while the replay rebuilds the undo data, then
    // carries out the reorganization.
    assert_eq!(chain.tip(), a[7].header.id());
    assert_eq!(
        chain.request("status", json!({})).unwrap()["replaying"],
        true
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while chain.tip() != b[9].header.id() {
        assert!(std::time::Instant::now() < deadline, "no reorganization");
        chain.resume_activation();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(chain.height(), 10);
    assert!(!dir.0.join("replay").exists());
    let status = chain.request("status", json!({})).unwrap();
    assert_eq!(status["reorganization"]["replaced"], 8);
    assert_eq!(status["state_records"], 0);
    // And back to a longer A: B's undo data below the window is rebuilt the
    // same way, and A's blocks come back by the redo data their disconnect
    // kept.
    let longer: Vec<Block> = (0..3)
        .map(|_| mine(&runtime, &g, &mut history, vec![], "A"))
        .collect();
    for block in &longer {
        chain.import(&block.encode(), true).unwrap();
    }
    while chain.tip() != longer[2].header.id() {
        assert!(std::time::Instant::now() < deadline, "no way back");
        chain.resume_activation();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(chain.height(), 11);
}
#[test]
fn settled_blocks_leave_memory_and_branches_below_them_are_still_followed() {
    // The experimental profile, epochs of four blocks, and an undo window of
    // three: the index holds the last 23 canonical blocks at least, three
    // voting epochs and eleven more.
    let dir = Directory::new();
    let path = dir.0.join("genesis.src");
    fs::write(&path, Genesis::experimental().encode()).unwrap();
    let c = config::Config {
        name: "settler".into(),
        control: "127.0.0.1:0".parse().unwrap(),
        p2p: "127.0.0.1:0".parse().unwrap(),
        api: "127.0.0.1:0".parse().unwrap(),
        undo_window: 3,
        ..Default::default()
    };
    config::initialize(&dir.0, &c, Some(&path)).unwrap();
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    assert_eq!(g.vote_epoch, 4);
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut history = vec![];
    let a: Vec<Block> = (0..120u8)
        .map(|i| {
            let txs = if i % 10 == 0 {
                vec![variant(vec![i])]
            } else {
                vec![]
            };
            mine(&runtime, &g, &mut history, txs, "A")
        })
        .collect();
    for block in &a {
        chain.import(&block.encode(), true).unwrap();
    }
    // At height 87 the 64 blocks below the held ones settled; the next 64
    // wait until there are as many.
    let status = chain.request("status", json!({})).unwrap();
    assert_eq!(status["settled_height"], 64);
    assert_eq!(status["held_headers"], 56);
    assert_eq!(status["stored_blocks"], 120);
    // A settled block is still known, and still served.
    assert_eq!(chain.import(&a[9].encode(), true).unwrap()["known"], true);
    assert_eq!(
        chain.request("block", json!({"id":"10"})).unwrap()["id"],
        hex(&a[9].header.id())
    );
    assert!(
        chain
            .request("raw", json!({"id":hex(&a[9].header.id())}))
            .is_ok()
    );
    // A restart reads only the held blocks into memory.
    drop(chain);
    let mut chain = open(&dir);
    let again = chain.request("status", json!({})).unwrap();
    for field in [
        "settled_height",
        "held_headers",
        "stored_blocks",
        "tip",
        "state_root",
    ] {
        assert_eq!(again[field], status[field], "{field}");
    }
    // A heavier branch forking at height 30, below the floor: what it leaves
    // is held again, the undo data below the window is rebuilt, and the node
    // moves to it.
    let fork = |at: usize, n: usize, producer: &str| -> Vec<Block> {
        let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
        let mut history = vec![];
        for block in &a[..at] {
            runtime
                .validate_and_apply_block(&g, &history, block)
                .unwrap();
            history.push(block.header.clone());
        }
        (0..n)
            .map(|_| mine(&runtime, &g, &mut history, vec![], producer))
            .collect()
    };
    let b = fork(30, 100, "B");
    for block in &b {
        chain.import(&block.encode(), true).unwrap();
    }
    let settle = |chain: &mut Chain, tip: ting_core::hash::Hash| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while chain.tip() != tip || chain.activating() {
            assert!(std::time::Instant::now() < deadline, "no reorganization");
            chain.resume_activation();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        chain.request("status", json!({})).unwrap()
    };
    let status = settle(&mut chain, b[99].header.id());
    assert_eq!(status["height"], 130);
    assert_eq!(status["reorganization"]["replaced"], 90);
    // B settles in turn up to 23 below its tip; A's 90 blocks above the fork
    // stay held as a side branch.
    assert_eq!(status["settled_height"], 107);
    assert_eq!(status["held_headers"], 23 + 90);
    // The three programs deployed below the fork, three records each.
    assert_eq!(status["state_records"], 9);
    // The operator marks a settled block of B: it is held again and left,
    // and A, the heaviest branch left, is followed.
    chain
        .request("invalidate", json!({"id":hex(&b[19].header.id())}))
        .unwrap();
    let status = settle(&mut chain, a[119].header.id());
    assert_eq!(status["height"], 120);
    assert_eq!(status["state_records"], 36);
    // Reconsidered, B is followed again.
    chain
        .request("reconsider", json!({"id":hex(&b[19].header.id())}))
        .unwrap();
    let status = settle(&mut chain, b[99].header.id());
    assert_eq!(status["marked"], json!([]));
    // And a restart opens on what all that left.
    drop(chain);
    let mut chain = open(&dir);
    let again = chain.request("status", json!({})).unwrap();
    for field in ["settled_height", "held_headers", "tip", "state_root"] {
        assert_eq!(again[field], status[field], "{field}");
    }
}
#[test]
fn a_node_that_held_every_block_settles_the_old_ones_as_it_opens() {
    // Epochs of four blocks; the default undo window of 30 days holds all
    // 120 blocks in memory.
    let dir = Directory::new();
    let path = dir.0.join("genesis.src");
    fs::write(&path, Genesis::experimental().encode()).unwrap();
    let c = config::Config {
        name: "upgrade".into(),
        control: "127.0.0.1:0".parse().unwrap(),
        p2p: "127.0.0.1:0".parse().unwrap(),
        api: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };
    config::initialize(&dir.0, &c, Some(&path)).unwrap();
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    for block in branch(&g, 120, "A") {
        chain.import(&block.encode(), true).unwrap();
    }
    let before = chain.request("status", json!({})).unwrap();
    assert_eq!(before["settled_height"], 0);
    assert_eq!(before["held_headers"], 120);
    drop(chain);
    // With an undo window of three, the node holds the last 23 and settles
    // the 97 below them as it opens, in one go.
    let mut c = config::Config::load(&dir.0).unwrap();
    c.undo_window = 3;
    c.save(&dir.0).unwrap();
    let mut chain = open(&dir);
    let after = chain.request("status", json!({})).unwrap();
    assert_eq!(after["settled_height"], 97);
    assert_eq!(after["held_headers"], 23);
    for field in ["tip", "state_root", "stored_blocks", "work"] {
        assert_eq!(after[field], before[field], "{field}");
    }
    // A header page and a template still come out right.
    let page = chain
        .request("headers", json!({"locator":[hex(&g.block_id())]}))
        .unwrap();
    assert_eq!(page["headers"].as_array().unwrap().len(), 120);
    assert_eq!(chain.request("template", json!({})).unwrap()["height"], 121);
}
#[test]
fn side_branches_below_the_settled_height_are_forgotten() {
    let dir = Directory::new();
    let path = dir.0.join("genesis.src");
    fs::write(&path, Genesis::experimental().encode()).unwrap();
    let c = config::Config {
        name: "forgetter".into(),
        control: "127.0.0.1:0".parse().unwrap(),
        p2p: "127.0.0.1:0".parse().unwrap(),
        api: "127.0.0.1:0".parse().unwrap(),
        undo_window: 3,
        ..Default::default()
    };
    config::initialize(&dir.0, &c, Some(&path)).unwrap();
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    // B, three blocks deploying a program, is connected first; A, 120 blocks,
    // replaces it; C, two headers above A's tenth block, is never stored.
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut history = vec![];
    let b: Vec<Block> = (0..3)
        .map(|i| {
            let txs = if i == 0 {
                vec![variant(vec![99])]
            } else {
                vec![]
            };
            mine(&runtime, &g, &mut history, txs, "B")
        })
        .collect();
    let deployed = hex(&b[0].transactions[0].id(g.chain_id()));
    for block in &b {
        chain.import(&block.encode(), true).unwrap();
    }
    assert!(chain.request("transaction", json!({"id":deployed})).is_ok());
    let a = branch(&g, 120, "A");
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut history = vec![];
    for block in &a[..10] {
        runtime
            .validate_and_apply_block(&g, &history, block)
            .unwrap();
        history.push(block.header.clone());
    }
    let side: Vec<Block> = (0..2)
        .map(|_| mine(&runtime, &g, &mut history, vec![], "C"))
        .collect();
    for (i, block) in a.iter().enumerate() {
        chain.import(&block.encode(), true).unwrap();
        if i == 9 {
            headers_from(&mut chain, &headers_of(&side)).unwrap();
        }
    }
    assert_eq!(chain.tip(), a[119].header.id());
    // At height 87 the blocks up to 64 settled, and B and C, wholly below,
    // went with them: from memory, and B from the database too.
    let status = chain.request("status", json!({})).unwrap();
    assert_eq!(status["settled_height"], 64);
    assert_eq!(status["held_headers"], 56);
    assert_eq!(status["stored_blocks"], 120);
    assert_eq!(
        chain
            .request("block", json!({"id":hex(&b[0].header.id())}))
            .unwrap_err(),
        "block not found"
    );
    // B's transaction went back to the pool when B was left; it occurs in no
    // block the node keeps.
    let lookup = chain
        .request("transaction", json!({"id":deployed}))
        .unwrap();
    assert_eq!(
        (lookup["pending"].clone(), lookup["occurrences"].clone()),
        (json!(true), json!([]))
    );
    // Sent again, B's first block is a side block like any other, until the
    // node next opens.
    assert_eq!(chain.import(&b[0].encode(), true).unwrap()["known"], false);
    assert_eq!(
        chain.request("status", json!({})).unwrap()["stored_blocks"],
        121
    );
    drop(chain);
    let mut chain = open(&dir);
    let status = chain.request("status", json!({})).unwrap();
    assert_eq!(status["stored_blocks"], 120);
    assert_eq!(status["held_headers"], 56);
    assert_eq!(chain.tip(), a[119].header.id());
}
/// A proof as the API gives it, the values of other records from the
/// answer's `others`.
fn proof_of(answer: &serde_json::Value, others: &serde_json::Value) -> ting_core::state::Proof {
    use ting_core::state::{End, Proof};
    let hash = |v: &serde_json::Value| ting_node::hash_arg(v.as_str().unwrap()).unwrap();
    let proof = &answer["proof"];
    Proof {
        end: match proof["end"].as_str().unwrap() {
            "present" => End::Present,
            "empty" => End::Empty,
            "other" => {
                let key = proof["other_key"].as_str().unwrap();
                let value = others[key].as_str().unwrap();
                End::Other(
                    ting_node::hash_arg(key).unwrap(),
                    ting_node::unhex(value, usize::MAX).unwrap(),
                )
            }
            end => panic!("unknown end {end}"),
        },
        siblings: proof["siblings"]
            .as_array()
            .unwrap()
            .iter()
            .map(hash)
            .collect(),
    }
}
#[test]
fn storage_reads_carry_merkle_proofs_on_request() {
    use ting_core::state::{record_key, verify};
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    // Several programs, each storing 0x2a at key zero.
    for i in 0..5u8 {
        chain
            .request("submit", json!({"hex":hex(&variant(vec![i]).encode())}))
            .unwrap();
    }
    chain.mine(1, 10000).unwrap();
    let block = chain.request("block", json!({"id":"1"})).unwrap();
    let contract = block["receipts"][0]["result_hex"]
        .as_str()
        .unwrap()
        .to_string();
    let id = ting_node::hash_arg(&contract).unwrap();
    let check = |answer: &serde_json::Value, of: &serde_json::Value, key: [u8; 32]| {
        let root = ting_node::hash_arg(of["state_root"].as_str().unwrap()).unwrap();
        let record = record_key(2, id, key);
        assert_eq!(answer["proof"]["record_key"], json!(hex(&record)));
        let value = answer["value_hex"]
            .as_str()
            .map(|v| ting_node::unhex(v, usize::MAX).unwrap());
        let proof = proof_of(answer, &of["others"]);
        assert!(verify(&root, &record, value.as_deref(), &proof), "{answer}");
        // The same proof shows no other value.
        assert!(!verify(&root, &record, Some(&[1, 2, 3]), &proof));
    };
    // Without asking, no proof.
    let plain = chain
        .request("storage", json!({"id":contract,"key":hex(&[0; 32])}))
        .unwrap();
    assert!(plain.get("proof").is_none());
    assert!(plain.get("others").is_none());
    for key in [[0; 32], [1; 32]] {
        let answer = chain
            .request(
                "storage",
                json!({"id":contract,"key":hex(&key),"proof":true}),
            )
            .unwrap();
        assert_eq!(answer["tip"], block["id"]);
        check(&answer, &answer, key);
    }
    let batch = chain
        .request(
            "storage_batch",
            json!({"reads":[{"contract":contract,"key":hex(&[0; 32])},{"contract":contract,"key":hex(&[1; 32])}],"proof":true}),
        )
        .unwrap();
    for (answer, key) in batch["values"]
        .as_array()
        .unwrap()
        .iter()
        .zip([[0; 32], [1; 32]])
    {
        check(answer, &batch, key);
    }
    assert_eq!(batch["values"][0]["value_hex"], "2a");
    assert_eq!(batch["values"][1]["value_hex"], serde_json::Value::Null);
    // Absent records: their paths end both at empty subtrees and at other
    // records' leaves, which come with those records' values.
    let keys: Vec<[u8; 32]> = (2..66u8).map(|i| [i; 32]).collect();
    let reads: Vec<_> = keys
        .iter()
        .map(|k| json!({"contract":contract,"key":hex(k)}))
        .collect();
    let batch = chain
        .request("storage_batch", json!({"reads":reads,"proof":true}))
        .unwrap();
    let mut ends = std::collections::BTreeSet::new();
    for (answer, key) in batch["values"].as_array().unwrap().iter().zip(&keys) {
        check(answer, &batch, *key);
        ends.insert(answer["proof"]["end"].as_str().unwrap().to_string());
    }
    assert_eq!(ends, ["empty", "other"].map(String::from).into());
    // Each other record's value is listed once, and only those proofs name.
    let named: std::collections::BTreeSet<_> = batch["values"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["proof"]["other_key"].as_str())
        .collect();
    let listed: std::collections::BTreeSet<_> = batch["others"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(named, listed);
    assert!(named.len() < keys.len(), "proofs share other records");
}
#[test]
fn a_batch_read_too_large_for_one_answer_is_refused_by_its_lengths() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    // A program storing 10,000 bytes at key zero.
    let deploy = ting_core::protocol::Transaction {
        budget: ting_core::meter::Resources([50_000_000, 1_024, 65_536, 65_536]),
        payload: ting_core::protocol::Payload::Deploy {
            code: wat::parse_str(
                r#"(module
            (import "ting" "storage_set" (func (param i32 i32 i32)))
            (memory (export "memory") 1 1)
            (func (export "invoke") i32.const 0 i32.const 32 i32.const 10000 call 0))"#,
            )
            .unwrap(),
            input: vec![],
        },
    };
    chain.submit(&deploy.encode()).unwrap();
    chain.mine(1, 10000).unwrap();
    let block = chain.request("block", json!({"id":"1"})).unwrap();
    let contract = block["receipts"][0]["result_hex"].as_str().unwrap();
    let read = json!({"contract":contract,"key":hex(&[0; 32])});
    // Named 1,024 times, its value alone is 20 MB of hex, past one answer.
    let refused = chain
        .request("storage_batch", json!({"reads":vec![read.clone(); 1_024]}))
        .unwrap_err();
    assert!(refused.contains("JSON response size limit"), "{refused}");
    // Named 512 times, it is read once and given at each place.
    let batch = chain
        .request(
            "storage_batch",
            json!({"reads":vec![read; 512],"proof":true}),
        )
        .unwrap();
    let values = batch["values"].as_array().unwrap();
    assert_eq!(values.len(), 512);
    assert_eq!(values[0]["value_hex"], json!(hex(&[0; 10_000])));
    assert!(values.iter().all(|v| v == &values[0]));
}
#[test]
fn a_restart_reads_side_blocks_from_their_table_which_older_databases_fill_once() {
    use redb::{ReadableDatabase, ReadableTableMetadata, TableDefinition};
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    // A, four blocks, wins; B, three, was connected first and is a side
    // branch now, and so is C, one block stored but never connected.
    let b = branch(&g, 3, "B");
    for block in &b {
        chain.import(&block.encode(), true).unwrap();
    }
    let a = branch(&g, 4, "A");
    for block in &a {
        chain.import(&block.encode(), true).unwrap();
    }
    let c = branch(&g, 1, "C");
    chain.import(&c[0].encode(), true).unwrap();
    assert_eq!(chain.tip(), a[3].header.id());
    let before = chain.request("status", json!({})).unwrap();
    assert_eq!(before["stored_blocks"], 8);
    drop(chain);
    let side_table: TableDefinition<&[u8], u64> = TableDefinition::new("side_blocks");
    let side = || {
        let db = redb::Database::open(dir.0.join("chain.redb")).unwrap();
        let tx = db.begin_read().unwrap();
        tx.open_table(side_table).unwrap().len().unwrap()
    };
    assert_eq!(side(), 4);
    // As a database from before the table: it is filled again as the node
    // opens, and the side blocks are indexed as before.
    {
        let db = redb::Database::open(dir.0.join("chain.redb")).unwrap();
        let tx = db.begin_write().unwrap();
        tx.delete_table(side_table).unwrap();
        let meta: TableDefinition<&[u8], &[u8]> = TableDefinition::new("metadata");
        tx.open_table(meta)
            .unwrap()
            .remove(b"side_blocks".as_slice())
            .unwrap();
        tx.commit().unwrap();
    }
    let mut chain = open(&dir);
    let after = chain.request("status", json!({})).unwrap();
    for field in ["stored_blocks", "held_headers", "tip", "state_root"] {
        assert_eq!(after[field], before[field], "{field}");
    }
    // B is still a branch the node can follow: two more blocks overtake A.
    let longer = branch(&g, 5, "B");
    chain.import(&longer[3].encode(), true).unwrap();
    chain.import(&longer[4].encode(), true).unwrap();
    assert_eq!(chain.tip(), longer[4].header.id());
    drop(chain);
    assert_eq!(side(), 5, "A's four blocks and C");
}
#[test]
fn a_stored_block_that_cannot_be_read_is_forgotten_and_taken_again() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let a = branch(&g, 3, "A");
    headers_from(&mut chain, &headers_of(&a)).unwrap();
    // The second block's body is stored before the first's, then damaged on
    // disk before it is ever executed.
    chain.import(&a[1].encode(), true).unwrap();
    let path = dir.0.join("blocks/00000000.dat");
    let mut bytes = fs::read(&path).unwrap();
    let body = a[1].encode();
    let at = bytes
        .windows(64)
        .position(|w| w == &body[..64])
        .expect("the frame is in the file");
    bytes[at + body.len() / 2] ^= 1;
    fs::write(&path, &bytes).unwrap();
    // A block just stored is kept in memory; after a restart it is read from
    // its file. Connecting reaches it: it is forgotten and the tip stays on
    // the first.
    drop(chain);
    let mut chain = open(&dir);
    chain.import(&a[0].encode(), true).unwrap();
    assert_eq!(chain.tip(), a[0].header.id());
    // Sent again, it is stored and connected, with the third.
    chain.import(&a[2].encode(), true).unwrap();
    chain.import(&a[1].encode(), true).unwrap();
    assert_eq!(chain.tip(), a[2].header.id());
}
#[test]
fn recovered_frames_whose_parent_comes_later_are_stored_too() {
    // Three blocks whose frames reached the file, 2 before 1, but no commit
    // did: recovery stores 1, then 2 and 3 on the next pass, and connects all.
    let dir = Directory::new();
    dir.config(false, "test");
    let g = config::genesis(&dir.0).unwrap();
    let a = branch(&g, 3, "A");
    let mut file = b"TINGBLK ".to_vec();
    file.extend(g.chain_id());
    for block in [&a[1], &a[0], &a[2]] {
        let bytes = block.encode();
        file.extend((bytes.len() as u64).to_le_bytes());
        file.extend(&bytes);
        file.extend(ting_core::hash::hash("node-block", &bytes));
    }
    fs::create_dir_all(dir.0.join("blocks")).unwrap();
    fs::write(dir.0.join("blocks/00000000.dat"), file).unwrap();
    let chain = open(&dir);
    assert_eq!(chain.tip(), a[2].header.id());
    drop(chain);
    assert_eq!(open(&dir).height(), 3);
}
#[test]
fn a_damaged_unindexed_tail_is_truncated_at_start() {
    // A complete frame with a wrong checksum, or zeros as a power loss may
    // leave: nothing refers to either, so the node opens on the block it had
    // indexed, cuts the tail off and appends after it.
    for tail in [
        [400u64.to_le_bytes().to_vec(), vec![0; 400], vec![1; 32]].concat(),
        vec![0; 1_000],
    ] {
        let dir = Directory::new();
        dir.config(false, "test");
        let mut chain = open(&dir);
        chain.mine(1, 100_000).unwrap();
        let tip = chain.tip();
        drop(chain);
        let path = dir.0.join("blocks/00000000.dat");
        let indexed = fs::metadata(&path).unwrap().len();
        {
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(&tail).unwrap();
        }
        let mut chain = Chain::open(&dir.0, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(chain.tip(), tip);
        assert_eq!(fs::metadata(&path).unwrap().len(), indexed);
        chain.mine(1, 100_000).unwrap();
        assert_eq!(chain.height(), 2);
    }
}

#[test]
fn a_branch_of_equal_length_stamped_earlier_does_not_replace_the_tip() {
    // A genesis hard enough for one second of timestamp to show in a block's
    // work: 2^12 hashes per block and a 600-second half-life.
    let dir = Directory::new();
    let mut g = Genesis::experimental();
    g.initial_target = (num_bigint::BigUint::from(1u32) << 244usize) - 1u32;
    g.half_life = 600;
    let path = dir.0.join("genesis.src");
    fs::write(&path, g.encode()).unwrap();
    let c = config::Config {
        name: "margin".into(),
        control: "127.0.0.1:0".parse().unwrap(),
        p2p: "127.0.0.1:0".parse().unwrap(),
        api: "127.0.0.1:0".parse().unwrap(),
        ..Default::default()
    };
    config::initialize(&dir.0, &c, Some(&path)).unwrap();
    let mine_at = |runtime: &Runtime, history: &mut Vec<Header>, timestamp: u64| -> Block {
        let mut b = runtime
            .build_candidate(&g, history, vec![], timestamp, [0; 4], vec![])
            .unwrap();
        while !consensus::valid_pow(&b.header) {
            b.header.nonce += 1;
        }
        runtime.validate_and_apply_block(&g, history, &b).unwrap();
        history.push(b.header.clone());
        b
    };
    let work = |history: &[Header]| -> num_bigint::BigUint {
        history.iter().map(|h| consensus::work(&h.target)).sum()
    };
    let a = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut ah = vec![];
    let a1 = mine_at(&a, &mut ah, g.timestamp + 60);
    let a2 = mine_at(&a, &mut ah, g.timestamp + 120);
    // The same length, its first block stamped one second earlier: the second
    // block's target is a little harder, so the branch has a little more work.
    let b = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut bh = vec![];
    let b1 = mine_at(&b, &mut bh, g.timestamp + 59);
    let b2 = mine_at(&b, &mut bh, g.timestamp + 120);
    assert!(work(&bh) > work(&ah));
    assert!(work(&bh) - work(&ah) < consensus::work(&b2.header.target) / 2u32);
    let mut chain = open(&dir);
    for block in [&a1, &a2, &b1, &b2] {
        chain.import(&block.encode(), true).unwrap();
    }
    assert_eq!(
        chain.tip(),
        a2.header.id(),
        "a hair more work does not take over"
    );
    // A stored side branch with a little more work than the tip is consistent.
    drop(chain);
    let mut chain = open(&dir);
    assert_eq!(chain.tip(), a2.header.id());
    // One more block does.
    let b3 = mine_at(&b, &mut bh, g.timestamp + 180);
    chain.import(&b3.encode(), true).unwrap();
    assert_eq!(chain.tip(), b3.header.id());
    drop(chain);
    assert_eq!(open(&dir).tip(), b3.header.id());
}

#[test]
fn a_chain_far_behind_is_checked_without_being_kept_until_it_overtakes() {
    let dir = Directory::new();
    dir.config(false, "presync");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    // Two branches from genesis, both mined on schedule, so every block
    // carries the initial target's work: A, 1,450 blocks, connected first;
    // B, 1,460, which overtakes it. A header chain is indexed once within
    // 1,440 blocks of the tip's work, so B's first headers are not.
    let build = |n: usize, producer: &str| -> Vec<Block> {
        let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
        let mut history = vec![];
        (0..n)
            .map(|_| mine(&runtime, &g, &mut history, vec![], producer))
            .collect()
    };
    let a = build(1_450, "A");
    let b = build(1_460, "B");
    for block in &a {
        chain.import(&block.encode(), true).unwrap();
    }
    assert_eq!(chain.tip(), a[1_449].header.id());
    let status = |chain: &mut Chain| chain.request("status", json!({})).unwrap();
    let before = status(&mut chain);
    // B's first block is too far behind to be stored.
    assert_eq!(
        chain.import(&b[0].encode(), true).unwrap_err(),
        ting_node::chain::BEHIND
    );
    let headers = |range: std::ops::Range<usize>| headers_of(&b[range]);
    // Checked as it streams, nothing kept: five headers do not reach the
    // threshold, the next five do, and the peer is asked again from genesis.
    let first = headers_from(&mut chain, &headers(0..5)).unwrap();
    assert_eq!(
        (first["indexed"].clone(), first["refetch"].clone()),
        (json!(0), json!(false))
    );
    // A header beyond the clock's slack is not taken, and the chain under
    // check stays where it was.
    let early = {
        let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
        let mut history = vec![];
        for block in &b[..5] {
            runtime
                .validate_and_apply_block(&g, &history, block)
                .unwrap();
            history.push(block.header.clone());
        }
        let mut early = runtime
            .build_candidate(
                &g,
                &history,
                vec![],
                ting_node::now() + ting_node::chain::CLOCK_SLACK + 3_600,
                [0; 4],
                vec![],
            )
            .unwrap();
        while !consensus::valid_pow(&early.header) {
            early.header.nonce += 1;
        }
        early
    };
    let held = headers_from(&mut chain, std::slice::from_ref(&early.header)).unwrap();
    assert_eq!(held["next"], serde_json::Value::Null);
    let reached = headers_from(&mut chain, &headers(5..20)).unwrap();
    assert_eq!(reached["refetch"], true);
    assert_eq!(reached["next"], hex(&g.block_id()));
    let after = status(&mut chain);
    assert_eq!(after["best_height"], before["best_height"]);
    assert_eq!(after["stored_blocks"], before["stored_blocks"]);
    // Fetched again from genesis: matched, then indexed, and the headers
    // after them indexed directly.
    let indexed = headers_from(&mut chain, &headers(0..1_460)).unwrap();
    assert_eq!(indexed["indexed"], 1_460);
    assert_eq!(status(&mut chain)["best_height"], 1_460);
    // The blocks are asked along B, sixty-four at a time of one peer, and
    // connected once B has more work than A.
    let wanted = chain.request("blocks_wanted", json!({"peer":"p"})).unwrap();
    let ids = wanted["ids"].as_array().unwrap();
    assert_eq!(ids.len(), 64);
    assert_eq!(ids[0], hex(&b[0].header.id()));
    for block in &b {
        chain.import(&block.encode(), true).unwrap();
    }
    assert_eq!(chain.tip(), b[1_459].header.id());
    assert_eq!(chain.height(), 1_460);
    // A is kept, valid but not connected; a restart keeps B.
    drop(chain);
    let chain = open(&dir);
    assert_eq!(chain.tip(), b[1_459].header.id());
}
#[test]
fn a_damaged_deferred_set_is_set_aside_and_the_node_starts() {
    let dir = Directory::new();
    dir.config(false, "test");
    drop(open(&dir));
    fs::write(dir.0.join("deferred.json"), b"{\"items\":[{\"hex\":\"00").unwrap();
    let mut chain = open(&dir);
    assert_eq!(
        chain.request("status", json!({})).unwrap()["deferred_count"],
        0
    );
    assert!(!dir.0.join("deferred.json").exists());
    assert_eq!(
        fs::read(dir.0.join("deferred.json.corrupt")).unwrap(),
        b"{\"items\":[{\"hex\":\"00"
    );
}
#[test]
fn a_transfer_has_a_minute_and_ten_seconds_a_slice() {
    use std::time::Duration;
    use ting_node::chain::transfer_time;
    assert_eq!(transfer_time(0), Duration::from_secs(60));
    assert_eq!(transfer_time(256 * 1024), Duration::from_secs(70));
    assert_eq!(transfer_time(100 << 20), Duration::from_secs(1_060));
    assert_eq!(
        transfer_time(u64::MAX),
        Duration::from_secs(60 + 10 * (1 << 44))
    );
}
#[test]
fn a_transaction_fetched_by_id_must_be_that_one_and_is_served_in_slices() {
    use ting_node::chain::NOT_ASKED;
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let tx = support::tx();
    let bytes = tx.encode();
    let id = hex(&tx.id(chain.genesis.chain_id()));
    // Another transaction, or bytes that are none, under an id is refused.
    for wrong in [bytes.clone(), vec![1, 2, 3]] {
        assert_eq!(
            chain
                .request_with("submit", json!({"id":hex(&[0; 32])}), Some(wrong))
                .unwrap_err(),
            NOT_ASKED
        );
    }
    let taken = chain
        .request_with("submit", json!({"id":id}), Some(bytes.clone()))
        .unwrap();
    assert_eq!(taken["known"], false);
    // Held, it is served in slices, like a block.
    let slice = ask(
        &mut chain,
        wire::Request::TransactionRaw {
            id: ting_node::hash_arg(&id).unwrap(),
            offset: 1,
            length: 10,
        },
    )
    .unwrap();
    assert_eq!(
        slice,
        wire::Answer::Slice {
            offset: 1,
            total: bytes.len() as u64,
            bytes: bytes[1..11].to_vec()
        }
    );
    assert_eq!(
        ask(
            &mut chain,
            wire::Request::TransactionRaw {
                id: [0; 32],
                offset: 0,
                length: 10
            }
        )
        .unwrap_err(),
        "transaction not found"
    );
}
#[test]
fn status_names_the_network_by_its_genesis() {
    // A development chain, created with its seed imported: its tip is the
    // seed's last block.
    let dir = Directory::new();
    let g = config::development_genesis();
    config::initialize_with(&dir.0, &config::Config::default(), g.clone()).unwrap();
    let mut chain = open(&dir);
    assert_eq!(
        chain.request("status", json!({})).unwrap()["network"],
        "development"
    );
    let seed = ting_node::seed::ids(&g);
    assert!(!seed.is_empty());
    assert_eq!(chain.height(), seed.len());
    assert_eq!(chain.tip(), *seed.last().unwrap());
    let other = Directory::new();
    let mut g = config::development_genesis();
    g.initial.0[2] += 1;
    config::initialize_with(&other.0, &config::Config::default(), g).unwrap();
    let mut chain = open(&other);
    assert_eq!(
        chain.request("status", json!({})).unwrap()["network"],
        "other"
    );
}
#[test]
fn a_budget_larger_than_a_block_is_taken_and_mined_when_its_use_fits() {
    // Consensus takes any declared budget; only what a transaction uses has to
    // fit (rules §14). Nested program calls need such budgets.
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let chain_id = g.chain_id();
    let huge = |mut tx: ting_core::protocol::Transaction| {
        tx.budget = ting_core::meter::Resources(g.initial.0.map(|c| c * 10));
        tx
    };
    let deploy = huge(support::tx());
    chain.submit(&deploy.encode()).unwrap();
    chain.mine(1, 10_000).unwrap();
    let block = chain.request("block", json!({"id":"1"})).unwrap();
    assert_eq!(block["transactions"], json!([hex(&deploy.id(chain_id))]));
    assert_eq!(block["receipts"][0]["status_name"], "Success");
    // Deferred the same: due, tried, pooled and mined.
    let contract = ting_core::hash::hash(
        "contract",
        &[&chain_id[..], &deploy.id(chain_id), &0u64.to_le_bytes()].concat(),
    );
    let call = huge(ting_core::protocol::Transaction {
        budget: ting_core::meter::Resources([0; 4]),
        payload: ting_core::protocol::Payload::Call {
            target: contract,
            input: vec![],
        },
    });
    chain.submit_deferred(&call.encode(), 0).unwrap();
    chain.tick().unwrap();
    chain.mine(1, 10_000).unwrap();
    let block = chain.request("block", json!({"id":"2"})).unwrap();
    assert_eq!(block["transactions"], json!([hex(&call.id(chain_id))]));
    assert_eq!(block["receipts"][0]["status_name"], "Success");
}
/// A program that emits its input, up to 64 bytes, as an event: its
/// deployment emits the empty input.
fn emitter() -> ting_core::protocol::Transaction {
    ting_core::protocol::Transaction {
        budget: ting_core::meter::Resources([1_000_000, 128, 16_384, 16_384]),
        payload: ting_core::protocol::Payload::Deploy {
            code: wat::parse_str(
                r#"(module
  (import "ting" "input" (func (param i32 i32) (result i32)))
  (import "ting" "emit" (func (param i32 i32)))
  (memory (export "memory") 1 1)
  (func (export "invoke") (call 1 (i32.const 0) (call 0 (i32.const 0) (i32.const 64)))))"#,
            )
            .unwrap(),
            input: vec![],
        },
    }
}
/// A page's events as height, transaction, place and payload.
fn events_in(page: &serde_json::Value) -> Vec<(u64, u64, u64, String)> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|item| {
            let height = item["height"].as_u64().unwrap();
            item["events"].as_array().unwrap().iter().map(move |e| {
                (
                    height,
                    e["transaction_index"].as_u64().unwrap(),
                    e["event_index"].as_u64().unwrap(),
                    e["payload_hex"].as_str().unwrap().to_string(),
                )
            })
        })
        .collect()
}
#[test]
fn events_are_read_by_program_and_topic_on_the_canonical_chain() {
    use redb::TableDefinition;
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let g = chain.genesis.clone();
    let deploy = emitter();
    let mut bytes = g.chain_id().to_vec();
    bytes.extend(deploy.id(g.chain_id()));
    bytes.extend(0u64.to_le_bytes());
    let program = hex(&ting_core::hash::hash("contract", &bytes));
    let (a, b) = ([0xaa; 32], [0xbb; 32]);
    let said = |topic: [u8; 32], word: &[u8]| [&topic[..], word].concat();
    // Block 1: the deployment's own event, then A, B and A again; block 2
    // has none; block 3 has A once more.
    chain.submit(&deploy.encode()).unwrap();
    for payload in [said(a, b"1"), said(b, b"2"), said(a, b"3")] {
        chain.submit(&call(&program, payload).encode()).unwrap();
    }
    chain.mine(1, 100_000).unwrap();
    chain.mine(1, 100_000).unwrap();
    chain
        .submit(&call(&program, said(a, b"4")).encode())
        .unwrap();
    chain.mine(1, 100_000).unwrap();
    let all = chain
        .request("events", json!({"contract":program}))
        .unwrap();
    assert_eq!(
        events_in(&all),
        vec![
            (1, 0, 0, String::new()),
            (1, 1, 0, hex(&said(a, b"1"))),
            (1, 2, 0, hex(&said(b, b"2"))),
            (1, 3, 0, hex(&said(a, b"3"))),
            (3, 0, 0, hex(&said(a, b"4"))),
        ]
    );
    assert_eq!((&all["next"], &all["height"]), (&json!(4), &json!(3)));
    let topic = |chain: &mut Chain, topic: [u8; 32], more: serde_json::Value| {
        let mut query = json!({"contract":program,"topic":hex(&topic)});
        for (key, value) in more.as_object().unwrap() {
            query[key] = value.clone();
        }
        chain.request("events", query)
    };
    let of_a = topic(&mut chain, a, json!({})).unwrap();
    assert_eq!(
        events_in(&of_a)
            .iter()
            .map(|e| (e.0, e.1))
            .collect::<Vec<_>>(),
        vec![(1, 1), (1, 3), (3, 0)]
    );
    let of_b = topic(&mut chain, b, json!({})).unwrap();
    assert_eq!(events_in(&of_b), vec![(1, 2, 0, hex(&said(b, b"2")))]);
    // Pages hold whole blocks; next skips the blocks without any.
    let first = topic(&mut chain, a, json!({"limit":1})).unwrap();
    assert_eq!((events_in(&first).len(), &first["next"]), (2, &json!(2)));
    let second = topic(&mut chain, a, json!({"limit":1,"from":2})).unwrap();
    assert_eq!(events_in(&second), vec![(3, 0, 0, hex(&said(a, b"4")))]);
    assert_eq!(second["next"], 4);
    let beyond = topic(&mut chain, a, json!({"from":9})).unwrap();
    assert_eq!((events_in(&beyond).len(), &beyond["next"]), (0, &json!(9)));
    let unknown = chain
        .request("events", json!({"contract":hex(&[7; 32])}))
        .unwrap();
    assert_eq!(
        (events_in(&unknown).len(), &unknown["next"]),
        (0, &json!(4))
    );
    // Every event page gives each event's place and the height read at.
    let page = chain.request("events", json!({"from":1})).unwrap();
    assert_eq!(page["items"][0]["events"][3]["event_index"], 0);
    assert_eq!(page["height"], 3);
    for (query, error) in [
        (
            json!({"topic":hex(&a)}),
            "a topic is read with its program: give contract too",
        ),
        (json!({"contract":"zz"}), "hex must be lowercase"),
        (json!({"contract":program,"topic":7}), "expected topic"),
        (
            json!({"contract":program,"limit":101}),
            "pagination limit is 100",
        ),
    ] {
        let found = chain.request("events", query.clone()).unwrap_err();
        assert!(found.contains(error), "{query}: {found}");
    }
    // A heavier branch from genesis deploys the same program and says B
    // once: the events of the branch left behind are no longer read.
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut history = vec![];
    let mut x = vec![mine(
        &runtime,
        &g,
        &mut history,
        vec![deploy.clone(), call(&program, said(b, b"x"))],
        "X",
    )];
    for _ in 0..3 {
        x.push(mine(&runtime, &g, &mut history, vec![], "X"));
    }
    for block in &x {
        chain.import(&block.encode(), true).unwrap();
    }
    assert_eq!(chain.tip(), x[3].header.id());
    let check = |chain: &mut Chain| {
        assert_eq!(events_in(&topic(chain, a, json!({})).unwrap()), vec![]);
        let of_b = topic(chain, b, json!({})).unwrap();
        assert_eq!(events_in(&of_b), vec![(1, 1, 0, hex(&said(b, b"x")))]);
        assert_eq!(of_b["items"][0]["block"], hex(&x[0].header.id()));
        assert_eq!((&of_b["next"], &of_b["height"]), (&json!(5), &json!(4)));
        let all = chain
            .request("events", json!({"contract":program}))
            .unwrap();
        assert_eq!(events_in(&all).len(), 2);
    };
    check(&mut chain);
    // So after a restart, and after a database from before the event
    // tables fills them as the node opens.
    drop(chain);
    let mut chain = open(&dir);
    check(&mut chain);
    drop(chain);
    {
        let db = redb::Database::open(dir.0.join("chain.redb")).unwrap();
        let tx = db.begin_write().unwrap();
        for table in ["program_events", "program_event_topics"] {
            tx.delete_table(TableDefinition::<&[u8], &[u8]>::new(table))
                .unwrap();
        }
        let meta: TableDefinition<&[u8], &[u8]> = TableDefinition::new("metadata");
        tx.open_table(meta)
            .unwrap()
            .remove(b"events".as_slice())
            .unwrap()
            .unwrap();
        tx.commit().unwrap();
    }
    let mut chain = open(&dir);
    check(&mut chain);
}
#[test]
fn voting_shows_this_epochs_votes_and_the_capacities_already_decided() {
    // The experimental profile: epochs of four blocks. The node votes to
    // increase compute.
    let dir = Directory::new();
    let g = Genesis::experimental();
    let path = dir.0.join("genesis.src");
    fs::write(&path, g.encode()).unwrap();
    let c = config::Config {
        name: "voter".into(),
        control: "127.0.0.1:0".parse().unwrap(),
        p2p: "127.0.0.1:0".parse().unwrap(),
        api: "127.0.0.1:0".parse().unwrap(),
        votes: [1, 0, 0, 0],
        ..Default::default()
    };
    config::initialize(&dir.0, &c, Some(&path)).unwrap();
    let mut chain = open(&dir);
    let votes = |chain: &mut Chain| chain.request("votes", json!({})).unwrap();
    let start = votes(&mut chain);
    assert_eq!(
        (&start["epoch"], &start["next_epoch_from"]),
        (&json!(0), &json!(5))
    );
    assert_eq!(start["this_epoch"]["blocks"], 0);
    assert_eq!(
        start["this_epoch"]["outcome_so_far"],
        json!(["hold", "hold", "hold", "hold"])
    );
    // Epoch 0, all four blocks for an increase, decides epoch 3.
    chain.mine(4, 100_000).unwrap();
    let mut raised = g.initial;
    raised.0[0] += raised.0[0] / 20;
    let four = votes(&mut chain);
    assert_eq!(four["epoch"], 1);
    assert_eq!(
        four["capacities"],
        json!([
            {"epoch":1,"from":5,"limits":g.initial.0},
            {"epoch":2,"from":9,"limits":g.initial.0},
            {"epoch":3,"from":13,"limits":raised.0},
        ])
    );
    // The running node takes new votes at once: decrease compute, increase
    // growth. Anything but four of 0, 1 or 2 is refused.
    for bad in [json!([3, 0, 0, 0]), json!([0, 0, 0]), json!("increase")] {
        assert!(
            chain
                .request("miner_config", json!({"enabled":false,"votes":bad}))
                .is_err()
        );
    }
    let set = chain
        .request(
            "miner_config",
            json!({"enabled":false,"votes":[2, 0, 0, 1]}),
        )
        .unwrap();
    assert_eq!(set["votes"], json!([2, 0, 0, 1]));
    assert_eq!(config::Config::load(&dir.0).unwrap().votes, [2, 0, 0, 1]);
    chain.mine(2, 100_000).unwrap();
    assert_eq!(
        chain.request("block", json!({"id":"6"})).unwrap()["votes"],
        json!([2, 0, 0, 1])
    );
    let six = votes(&mut chain);
    let this = &six["this_epoch"];
    assert_eq!((&six["epoch"], &this["blocks"]), (&json!(1), &json!(2)));
    assert_eq!(this["decrease_percent"], json!([100.0, 0.0, 0.0, 0.0]));
    assert_eq!(this["increase_percent"], json!([0.0, 0.0, 0.0, 100.0]));
    assert_eq!(
        this["outcome_so_far"],
        json!(["decrease", "hold", "hold", "increase"])
    );
    assert_eq!(this["takes_effect_from"], 17);
    // Once epoch 1 is complete, epoch 4's capacities are decided: less
    // compute than epoch 3, more growth.
    chain.mine(2, 100_000).unwrap();
    let decided = votes(&mut chain)["capacities"][2].clone();
    assert_eq!(
        (&decided["epoch"], &decided["from"]),
        (&json!(4), &json!(17))
    );
    let limits: Vec<u64> = serde_json::from_value(decided["limits"].clone()).unwrap();
    assert!(limits[0] < raised.0[0] && limits[3] > g.initial.0[3]);
    assert_eq!(limits[1..3], g.initial.0[1..3]);
    // And they are what the chain applies when epoch 4 begins.
    chain.mine(8, 100_000).unwrap();
    let status = chain.request("status", json!({})).unwrap();
    assert_eq!(
        (&status["height"], &status["limits"]),
        (&json!(16), &decided["limits"])
    );
    assert_eq!(votes(&mut chain)["capacities"][0], decided);
    // Each block's summary gives the capacity it had to fit, so its use reads
    // against that, and so after a database from before every executed block
    // kept its limits fills them in as the node opens.
    let check = |chain: &mut Chain| {
        let limits = |chain: &mut Chain, id: &str| {
            chain.request("block", json!({"id":id})).unwrap()["limits"].clone()
        };
        assert_eq!(limits(chain, "12"), json!(g.initial.0));
        assert_eq!(limits(chain, "13"), json!(raised.0));
    };
    check(&mut chain);
    drop(chain);
    {
        use redb::{ReadableTable, TableDefinition};
        let db = redb::Database::open(dir.0.join("chain.redb")).unwrap();
        let tx = db.begin_write().unwrap();
        tx.delete_table(TableDefinition::<&[u8], &[u8]>::new("block_limits"))
            .unwrap();
        let meta: TableDefinition<&[u8], &[u8]> = TableDefinition::new("metadata");
        let mut table = tx.open_table(meta).unwrap();
        assert!(table.get(b"block_limits".as_slice()).unwrap().is_some());
        table.remove(b"block_limits".as_slice()).unwrap();
        drop(table);
        tx.commit().unwrap();
    }
    let mut chain = open(&dir);
    check(&mut chain);
}
/// A deployment whose initializer loops: it runs until a block's compute is
/// gone and never fits one. `n` makes each one a different transaction.
fn endless(n: u8) -> ting_core::protocol::Transaction {
    ting_core::protocol::Transaction {
        budget: ting_core::meter::Resources([u64::MAX, 128, 16_384, 16_384]),
        payload: ting_core::protocol::Payload::Deploy {
            code: wat::parse_str("(module (func (export \"invoke\") (loop br 0)))").unwrap(),
            input: vec![n],
        },
    }
}
/// The templates the pool takes to include `id`, each asserted to hold
/// nothing before it.
fn templates_until(chain: &mut Chain, id: &str) -> usize {
    for n in 1..=10 {
        let template = chain.request("template", json!({})).unwrap();
        if template["transactions"] == json!([id]) {
            return n;
        }
        assert_eq!(template["transactions"], json!([]));
    }
    panic!("{id} never got into a template");
}
#[test]
fn a_transaction_that_fits_no_block_leaves_the_pool_and_the_next_one_gets_in() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let chain_id = chain.genesis.chain_id();
    let junk = endless(0);
    let honest = support::tx();
    chain.submit(&junk.encode()).unwrap();
    chain.submit(&honest.encode()).unwrap();
    // The loop runs first, alone in the block, uses the whole block's compute
    // and does not fit: it leaves the pool, and with it the template's
    // allowance for discarded work is spent. The next template takes the
    // transaction after it.
    let first = chain.request("template", json!({})).unwrap();
    assert_eq!(first["transactions"], json!([]));
    assert_eq!(
        chain
            .request("transaction", json!({"id":hex(&junk.id(chain_id))}))
            .unwrap_err(),
        "transaction not found"
    );
    assert_eq!(templates_until(&mut chain, &hex(&honest.id(chain_id))), 1);
}
#[test]
fn a_template_throws_away_about_a_block_of_compute_at_most() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let chain_id = chain.genesis.chain_id();
    let honest = support::tx();
    for n in 0..3 {
        chain.submit(&endless(n).encode()).unwrap();
    }
    chain.submit(&honest.encode()).unwrap();
    // Every loop costs a template its whole allowance, and leaves: three
    // templates run one each, and the fourth holds the honest transaction.
    assert_eq!(templates_until(&mut chain, &hex(&honest.id(chain_id))), 4);
    assert_eq!(
        chain.request("status", json!({})).unwrap()["mempool_count"],
        1
    );
}
#[test]
fn an_address_whose_transaction_was_just_tried_lets_the_others_go_first() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let chain_id = chain.genesis.chain_id();
    let attacker: std::net::IpAddr = "203.0.113.7".parse().unwrap();
    let other: std::net::IpAddr = "198.51.100.4".parse().unwrap();
    for n in 0..5 {
        chain
            .submit_from(&endless(n).encode(), Some(attacker))
            .unwrap();
    }
    let honest = support::tx();
    chain.submit_from(&honest.encode(), Some(other)).unwrap();
    // The attacker's loops arrived first, and the first of them takes the
    // first template's allowance. Then the other address has its turn before
    // the attacker's next loop.
    assert_eq!(templates_until(&mut chain, &hex(&honest.id(chain_id))), 2);
}
#[test]
fn a_transaction_a_fork_takes_back_out_stays_pooled() {
    use ting_core::vm::Candidate;
    use ting_node::mining::{Pending, Selector};
    // A fork's selection that runs each transaction, reads its receipt and
    // takes it back out, as one ranking what transactions pay would.
    struct Looks;
    impl Selector for Looks {
        fn select(&mut self, pool: &[Pending<'_>], block: &mut Candidate<'_>) {
            for pending in pool {
                if let Ok(Some(_)) = block.add(pending.tx) {
                    block.undo();
                }
            }
        }
        fn describe(&self) -> serde_json::Value {
            json!({"kind":"looks"})
        }
    }
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain =
        Chain::open_with(&dir.0, Arc::new(AtomicBool::new(false)), Box::new(Looks)).unwrap();
    chain.submit(&support::tx().encode()).unwrap();
    // It fit a block holding nothing else, so it is not dropped.
    for _ in 0..3 {
        let template = chain.request("template", json!({})).unwrap();
        assert_eq!(template["transactions"], json!([]));
    }
    assert_eq!(
        chain.request("status", json!({})).unwrap()["mempool_count"],
        1
    );
}
#[test]
fn a_deferred_transaction_that_fits_no_block_leaves_the_set() {
    let dir = Directory::new();
    dir.config(false, "test");
    let mut chain = open(&dir);
    let now = ting_node::now();
    chain
        .submit_deferred(&endless(0).encode(), now - 1)
        .unwrap();
    assert_eq!(
        chain.request("status", json!({})).unwrap()["deferred_count"],
        1
    );
    chain.tick().unwrap();
    let status = chain.request("status", json!({})).unwrap();
    assert_eq!(
        (
            status["deferred_count"].clone(),
            status["mempool_count"].clone()
        ),
        (json!(0), json!(0))
    );
}
#[test]
fn a_pending_answer_carries_one_slice_whole_and_lists_the_rest_by_size() {
    // Four pooled transactions of 300 kB: three fit one slice whole, and the
    // fourth is listed by its size, to be fetched in slices, so that the
    // answer is a frame no larger than a block's slice.
    use ting_core::{
        meter::Resources,
        protocol::{Payload, Transaction},
    };
    use ting_node::chain::{RELAY_PAGE, SLICE_MAX};
    let dir = Directory::new();
    dir.config(false, "relay");
    let mut chain = open(&dir);
    let chain_id = chain.genesis.chain_id();
    let large: Vec<Transaction> = (1..=4)
        .map(|byte| Transaction {
            budget: Resources([1_000_000, 1, 400_000, 1]),
            payload: Payload::Call {
                target: [1; 32],
                input: vec![byte; 300_000],
            },
        })
        .collect();
    for tx in &large {
        chain.submit(&tx.encode()).unwrap();
    }
    let ids: Vec<_> = large.iter().map(|tx| tx.id(chain_id)).collect();
    let wire::Answer::Items(items) = chain
        .answer_wire(&wire::Request::Pending { ids: ids.clone() })
        .unwrap()
    else {
        panic!("not items");
    };
    assert_eq!(items.iter().map(|i| i.id).collect::<Vec<_>>(), ids);
    let whole: Vec<usize> = items
        .iter()
        .filter_map(|i| match &i.body {
            wire::Body::Whole(bytes) => Some(bytes.len()),
            wire::Body::Total(_) => None,
        })
        .collect();
    assert_eq!(whole.len(), 3);
    assert!(whole.iter().sum::<usize>() <= SLICE_MAX);
    assert_eq!(
        items[3].body,
        wire::Body::Total(large[3].encode().len() as u64)
    );
    let frame = wire::Frame::Answer(wire::Answer::Items(items)).encode();
    assert!(
        frame.len() <= SLICE_MAX + 64 * RELAY_PAGE,
        "{}",
        frame.len()
    );
    // The one listed by size comes whole in one slice of its own.
    let wire::Answer::Slice { bytes, total, .. } = chain
        .answer_wire(&wire::Request::TransactionRaw {
            id: ids[3],
            offset: 0,
            length: SLICE_MAX as u32,
        })
        .unwrap()
    else {
        panic!("not a slice");
    };
    assert_eq!(bytes, large[3].encode());
    assert_eq!(total, bytes.len() as u64);
}
