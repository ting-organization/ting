//! Valid header histories supplement the VM cases: irregular miner time and
//! a fully mined short-epoch chain whose votes actually activate capacities.
use num_bigint::BigUint;
use num_traits::{One, Zero};
use ting_core::protocol::{Genesis, Header};

/// A reproducible valid miner schedule. Height 13 is below its parent but
/// exactly one second above the eleven-block median. Later blocks alternate
/// ahead/behind schedule, including fractional negative ASERT exponents.
pub(crate) fn timestamp(g: &Genesis, history: &[Header]) -> u64 {
    let height = history.len() as u64 + 1;
    if height == 13 {
        let mut times: Vec<_> = history.iter().rev().take(11).map(|h| h.timestamp).collect();
        times.sort_unstable();
        return times[times.len() / 2] + 1;
    }
    let jitter = if height <= 2 {
        0
    } else if height <= 12 {
        1
    } else {
        [-1i64, 0, 1, 59, 61, 120, -60][height as usize % 7]
    };
    (g.timestamp + u64::from(g.interval) * height)
        .checked_add_signed(jitter)
        .unwrap()
}

/// Specification arithmetic, without consensus::asert/expected_target. Floor
/// negative divisions explicitly and evaluate the cubic in Horner form.
pub(crate) fn target(g: &Genesis, history: &[Header]) -> BigUint {
    let floor = |n: i128, d: i128| if n < 0 { -((-n + d - 1) / d) } else { n / d };
    let elapsed = history
        .last()
        .map_or(0, |h| i128::from(h.timestamp) - i128::from(g.timestamp));
    let exponent = floor(
        (elapsed - i128::from(g.interval) * history.len() as i128) * 65536,
        i128::from(g.half_life),
    );
    let shift = floor(exponent, 65536);
    let fraction = (exponent - shift * 65536) as u128;
    let polynomial = (((5127 * fraction + 971821376) * fraction + 195766423245049) * fraction
        + (1u128 << 47))
        >> 48;
    let product = &g.initial_target * BigUint::from(65536 + polynomial);
    let value = if shift >= 272 {
        g.pow_limit.clone()
    } else if shift <= -258 {
        BigUint::zero()
    } else if shift >= 16 {
        product << ((shift - 16) as usize)
    } else {
        product >> ((16 - shift) as usize)
    };
    value.max(BigUint::one()).min(g.pow_limit.clone())
}

/// Work-weighted decisions and delayed changes, independently evaluated over
/// actual ancestors. No consensus voting or capacity helpers derive these.
fn limits(g: &Genesis, history: &[Header]) -> [u64; 4] {
    let epoch = g.vote_epoch as usize;
    let mut result = g.initial.0;
    for activation in 3..=history.len() / epoch {
        let source = &history[(activation - 3) * epoch..(activation - 2) * epoch];
        let weights: Vec<_> = source
            .iter()
            .map(|h| (BigUint::one() << 256usize) / (&h.target + BigUint::one()))
            .collect();
        let total: BigUint = weights.iter().sum();
        for (d, value) in result.iter_mut().enumerate() {
            let up: BigUint = weights
                .iter()
                .zip(source)
                .filter(|(_, h)| h.votes[d] == 1)
                .map(|(w, _)| w)
                .sum();
            let down: BigUint = weights
                .iter()
                .zip(source)
                .filter(|(_, h)| h.votes[d] == 2)
                .map(|(w, _)| w)
                .sum();
            if up * 3u32 >= &total * 2u32 {
                *value = value.saturating_add((*value / 20).max(1));
            } else if down * 2u32 > total {
                *value = value
                    .saturating_sub((*value / 10).max(1))
                    .max(g.minimum.0[d]);
            }
        }
    }
    result
}

#[test]
fn a_mined_short_epoch_chain_activates_work_weighted_votes_after_the_notice() {
    use crate::{
        dsl::{invoke, memory, module, wasm},
        harness::installation,
        seed::{bundle, declared_root},
        shape,
    };
    use std::{collections::BTreeMap, fs};
    use ting_core::{
        consensus::{Ancestry, active_limits, valid_pow},
        hash::{hash, hex},
        meter::Resources,
        protocol::{Block, Encode, Payload, Status, Transaction},
        state::State,
        vm::{Context, Runtime},
    };
    let mut g = Genesis::development();
    g.vote_epoch = 3;
    g.half_life = 60;
    g.initial = Resources([4_000_000, 64, 8192, 2048]);
    g.minimum = Resources([3_900_000, 60, 7500, 1900]);
    let text = module(&[&memory(1, 1), &invoke("nop")]);
    let code = wasm(&text);
    let deploy = Transaction {
        budget: g.initial,
        payload: Payload::Deploy {
            code: code.clone(),
            input: vec![],
        },
    };
    let mut identifier = g.chain_id().to_vec();
    identifier.extend(deploy.id(g.chain_id()));
    identifier.extend(0u64.to_le_bytes());
    let id = hash("contract", &identifier);
    let records: BTreeMap<_, _> = installation(id, &code).into();
    let state_root = declared_root(&records);
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut blocks: Vec<Block> = Vec::new();
    let mut headers = Vec::new();
    let mut stated = Vec::new();
    for height in 1..=33u64 {
        let expected = limits(&g, &headers);
        assert_eq!(active_limits(&g, &headers).0, expected);
        let votes = match (height - 1) / 3 {
            0 => [1, 0, 2, 1],
            2 => [2, 2, 0, 2],
            3 => {
                if height == 10 {
                    [0; 4]
                } else {
                    [1; 4]
                }
            }
            4 => [1; 4],
            5..=9 => [2; 4],
            _ => [0; 4],
        };
        // Epoch 3 has work [4,1,1]: two headers vote up, but only a third
        // of the work does. Those votes must not activate an increase.
        let time = g.timestamp + height * 60 + if height == 10 || height == 11 { 120 } else { 0 };
        let tx = if height == 1 {
            deploy.clone()
        } else {
            Transaction {
                budget: g.initial,
                payload: Payload::Call {
                    target: id,
                    input: height.to_le_bytes().to_vec(),
                },
            }
        };
        let ledger = vm_model::Ledger {
            dispatch: vm_model::cost("nop"),
            ..Default::default()
        };
        let used = if height == 1 {
            vm_model::deploy(&code, 0, shape::shape(&code).entry(), ledger, &[])
        } else {
            vm_model::call(8, shape::shape(&code).entry(), ledger, 0, &[])
        };
        let mut block = runtime
            .build_candidate(&g, &headers, vec![tx], time, votes, vec![])
            .unwrap();
        assert_eq!(block.receipts.len(), 1);
        assert_eq!(block.receipts[0].status, Status::Success);
        assert_eq!(block.receipts[0].used.0, used);
        assert_eq!(
            block.receipts[0].result,
            if height == 1 { id.to_vec() } else { vec![] }
        );
        assert_eq!(block.header.state_root, state_root);
        assert_eq!(block.header.target, target(&g, &headers));
        while !valid_pow(&block.header) {
            block.header.nonce += 1;
        }
        runtime
            .validate_and_apply_block_at(&g, &Ancestry::of(&headers), &block, Resources(expected))
            .unwrap();
        stated.push(serde_json::json!({"height":height,"id":hex(&block.header.id()),"limits":expected,
            "votes":votes,"target":hex(&ting_core::protocol::target_bytes(&block.header.target)),
            "transactions":[{"id":hex(&block.transactions[0].id(g.chain_id())),"receipt":hex(&block.receipts[0].encode())}]}));
        headers.push(block.header.clone());
        blocks.push(block);
    }
    assert_eq!(stated[8]["limits"], serde_json::json!(g.initial.0));
    assert_eq!(
        stated[9]["limits"],
        serde_json::json!([4_200_000, 64, 7500, 2150])
    );
    assert_eq!(
        stated[15]["limits"],
        serde_json::json!([3_900_000, 60, 7500, 1935])
    );
    assert_eq!(
        stated[18]["limits"], stated[15]["limits"],
        "minority-work votes must hold"
    );
    assert_eq!(stated[30]["limits"], serde_json::json!(g.minimum.0));
    let fresh = Runtime::new(State::default(), Context::from_genesis(&g));
    let mut history = Vec::new();
    for block in &blocks {
        fresh
            .validate_and_apply_block_at(
                &g,
                &Ancestry::of(&history),
                block,
                Resources(limits(&g, &history)),
            )
            .unwrap();
        history.push(block.header.clone());
    }
    assert_eq!(fresh.state_root(), state_root);
    // Published beside the seed, for other implementations and the node's
    // replay test; rewritten only on purpose.
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("conformance/chains");
    let bytes = bundle(&blocks);
    let mut text = serde_json::to_string_pretty(&serde_json::json!({
        "genesis": hex(&g.encode()),
        "blocks": stated,
    }))
    .unwrap();
    text.push('\n');
    if std::env::var_os("TING_WRITE_VECTORS").is_some() {
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("voting.blocks"), &bytes).unwrap();
        fs::write(dir.join("voting.json"), &text).unwrap();
    }
    assert!(
        fs::read(dir.join("voting.blocks")).unwrap_or_default() == bytes,
        "conformance/chains/voting.blocks differs from the chain; after a deliberate \
         change, rewrite it with TING_WRITE_VECTORS=1"
    );
    assert!(
        fs::read_to_string(dir.join("voting.json"))
            .unwrap_or_default()
            .replace("\r\n", "\n")
            == text,
        "conformance/chains/voting.json differs from the chain; after a deliberate \
         change, rewrite it with TING_WRITE_VECTORS=1"
    );
}
