//! Blocks around the VM: a candidate is built and reproduced by a validator,
//! an invalid block is cancelled at its capacity or rejected on its receipts
//! without a trace in state, a failed transaction's debit cannot cancel a
//! block, and producer data travels in the body under the header's commitment.
use ting_core::{
    consensus,
    hash::hash,
    meter::Resources,
    protocol::{
        BLOCK_BASE, Block, Encode, Genesis, Header, Payload, Receipt, Status, Transaction,
        producer_hash,
    },
    state::State,
    vm::{Context, Runtime},
};

fn looping() -> Vec<u8> {
    wat::parse_str("(module (func (export \"invoke\") (loop br 0)))").unwrap()
}
fn trivial() -> Vec<u8> {
    wat::parse_str("(module (memory (export \"memory\") 1 1) (func (export \"invoke\")))").unwrap()
}
fn deployment(code: Vec<u8>, budget: [u64; 4]) -> Transaction {
    Transaction {
        budget: Resources(budget),
        payload: Payload::Deploy {
            code,
            input: Vec::new(),
        },
    }
}
/// The compute of a call to a missing program: the root dispatch and the read
/// of the absent manifest with its key hash, a SHA-256 of two blocks (wiki:
/// resources/prices).
const MISSING: u64 = 1 + 1_900;
/// A genesis whose blocks hold little compute: fifty calls to a missing program.
fn capped() -> Genesis {
    let mut g = Genesis::experimental();
    g.minimum.0[0] = 10;
    g.initial.0[0] = 50 * MISSING;
    g.validate().unwrap();
    g
}
/// A block at height 1 with one transaction and its receipt, mined to the
/// genesis target.
fn block(
    g: &Genesis,
    tx: Transaction,
    receipt: Receipt,
    state_root: [u8; 32],
    used: Resources,
) -> Block {
    let (txroot, rcroot) =
        consensus::commitments(std::slice::from_ref(&tx), std::slice::from_ref(&receipt));
    let mut header = Header {
        chain_id: g.chain_id(),
        height: 1,
        parent: g.block_id(),
        timestamp: g.timestamp + 60,
        target: g.initial_target.clone(),
        transactions_root: txroot,
        receipts_root: rcroot,
        state_root,
        used,
        votes: [0; 4],
        producer_hash: producer_hash(&[]),
        nonce: 0,
    };
    while !consensus::valid_pow(&header) {
        header.nonce += 1;
    }
    Block {
        header,
        producer: Vec::new(),
        transactions: vec![tx],
        receipts: vec![receipt],
    }
}
fn mine(block: &mut Block) {
    while !consensus::valid_pow(&block.header) {
        block.header.nonce += 1;
    }
}

#[test]
fn invalid_block_cancels_at_capacity_and_restores_state() {
    let g = capped();
    let tx = deployment(looping(), [1_000_000, 256, 8192, 8192]);
    let receipt = Receipt {
        transaction: tx.id(g.chain_id()),
        status: Status::Compute,
        used: Resources([0, 0, tx.encode().len() as u64 + 81, 0]),
        result: Vec::new(),
        events: Vec::new(),
    };
    let mut used = receipt.used;
    used.0[2] += BLOCK_BASE;
    let block = block(&g, tx, receipt, State::default().root(), used);
    let r = Runtime::new(State::default(), Context::from_genesis(&g));
    assert_eq!(
        r.validate_and_apply_block(&g, &[], &block).unwrap_err().0,
        "executed block resource limits"
    );
    assert!(r.last_resources().0[0] <= g.initial.0[0]);
    assert_eq!(r.state_root(), State::default().root());
    let missing = Transaction {
        budget: Resources([10_000, 10, 1024, 0]),
        payload: Payload::Call {
            target: [7; 32],
            input: Vec::new(),
        },
    };
    assert_eq!(r.execute(&missing).unwrap().status, Status::Missing);
}
#[test]
fn failed_transaction_debit_cannot_trigger_false_block_cancellation() {
    let g = capped();
    let r = Runtime::new(State::default(), Context::from_genesis(&g));
    let tx = deployment(looping(), [0, 256, 8192, 8192]);
    let receipt = r.execute(&tx).unwrap();
    assert_eq!(receipt.status, Status::Compute);
    assert_eq!(receipt.used.0[0], 0);
    let mut used = receipt.used;
    used.0[2] += BLOCK_BASE;
    let block = block(&g, tx, receipt, State::default().root(), used);
    r.validate_and_apply_block(&g, &[], &block).unwrap();
}
#[test]
fn a_block_whose_receipts_do_not_match_is_rejected_without_a_trace() {
    let g = Genesis::experimental();
    let builder = Runtime::new(State::default(), Context::from_genesis(&g));
    let tx = deployment(
        wat::parse_str("(module (func (export \"invoke\")))").unwrap(),
        [1_000_000, 256, 8192, 8192],
    );
    let receipt = builder.execute(&tx).unwrap();
    let mut used = receipt.used;
    used.0[2] += BLOCK_BASE;
    let mut block = block(&g, tx, receipt, builder.state_root(), used);
    let validator = Runtime::new(State::default(), Context::from_genesis(&g));
    validator.validate_and_apply_block(&g, &[], &block).unwrap();
    assert_eq!(validator.state_root(), builder.state_root());
    // One bit of the receipt: the block fails and the state stays empty.
    block.receipts[0].result[0] ^= 1;
    block.header.receipts_root = consensus::commitments(&block.transactions, &block.receipts).1;
    mine(&mut block);
    let validator = Runtime::new(State::default(), Context::from_genesis(&g));
    assert!(validator.validate_and_apply_block(&g, &[], &block).is_err());
    assert_eq!(validator.state_root(), State::default().root());
}
#[test]
fn producer_data_travels_in_the_body_under_a_header_commitment() {
    let g = capped();
    let r = Runtime::new(State::default(), Context::from_genesis(&g));
    let data = vec![7u8; 1000];
    let mut block = r
        .build_candidate(&g, &[], vec![], g.timestamp + 60, [0; 4], data.clone())
        .unwrap();
    mine(&mut block);
    assert_eq!(block.producer, data);
    assert_eq!(block.header.producer_hash, producer_hash(&data));
    assert_eq!(block.header.used.0[2], BLOCK_BASE + 1000);
    assert_eq!(block.encode().len() as u64, block.header.used.0[2]);
    let mut forged = block.clone();
    forged.producer[0] ^= 1;
    assert!(Block::decode(&forged.encode(), 1 << 20).is_err());
    assert_eq!(
        r.validate_and_apply_block(&g, &[], &forged).unwrap_err().0,
        "producer data commitment"
    );
}
#[test]
fn candidate_is_reversible_and_validator_reproduces_receipts() {
    let g = Genesis::experimental();
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let empty = runtime.state_root();
    let tx = deployment(trivial(), [1_000_000, 128, 16_384, 16_384]);
    let candidate = runtime
        .build_candidate(&g, &[], vec![tx.clone()], g.timestamp + 60, [0; 4], vec![])
        .unwrap();
    assert_eq!(runtime.state_root(), empty);
    assert_eq!(candidate.receipts[0].status, Status::Success);
    assert_eq!(
        candidate.header.used.0[2],
        candidate.receipts[0].used.0[2] + BLOCK_BASE
    );
    assert_ne!(candidate.header.state_root, empty);
    let mut again = runtime
        .build_candidate(&g, &[], vec![tx], g.timestamp + 60, [0; 4], vec![])
        .unwrap();
    assert_eq!(candidate.encode(), again.encode());
    mine(&mut again);
    runtime.validate_and_apply_block(&g, &[], &again).unwrap();
    assert_eq!(runtime.state_root(), candidate.header.state_root);
}
#[test]
fn failed_assembly_restores_state_and_context() {
    let mut g = Genesis::experimental();
    g.initial.0[0] = 100;
    g.minimum.0[0] = 1;
    let runtime = Runtime::new(State::default(), Context::from_genesis(&g));
    let empty = runtime.state_root();
    assert!(
        runtime
            .build_candidate(
                &g,
                &[],
                vec![deployment(trivial(), [1_000_000, 128, 16_384, 16_384])],
                g.timestamp + 999_999,
                [0; 4],
                vec![]
            )
            .is_err()
    );
    assert_eq!(runtime.state_root(), empty);
    let block = runtime
        .build_candidate(&g, &[], vec![], g.timestamp + 60, [0; 4], vec![])
        .unwrap();
    assert_eq!(block.header.used, Resources([0, 0, BLOCK_BASE, 0]));
    assert_eq!(block.header.state_root, empty);
    // The context the runtime was built with is intact: a program reading
    // the block timestamp sees the genesis's first interval.
    let clock = wat::parse_str(
        r#"(module
  (import "ting" "context" (func (param i32 i32 i32) (result i32)))
  (import "ting" "return" (func (param i32 i32)))
  (memory (export "memory") 1 1)
  (func (export "invoke") i32.const 5 i32.const 0 i32.const 8 call 0 drop i32.const 0 i32.const 8 call 1))"#,
    )
    .unwrap();
    let tx = deployment(clock, [1_000_000, 128, 16_384, 16_384]);
    let deployed = runtime.execute(&tx).unwrap();
    assert_eq!(deployed.status, Status::Success);
    let call = Transaction {
        budget: tx.budget,
        payload: Payload::Call {
            target: deployed.result.try_into().unwrap(),
            input: vec![],
        },
    };
    assert_eq!(
        runtime.execute(&call).unwrap().result,
        (g.timestamp + g.interval as u64).to_le_bytes()
    );
}

/// A program that stores one byte when its record is absent (its initializer
/// does) and returns the stored byte when it is present (a later call does).
/// Imports are called by index: names would add a custom section, which the
/// profile excludes.
fn keeper() -> Vec<u8> {
    wat::parse_str(
        r#"(module
  (import "ting" "storage_get" (func (param i32 i32 i32) (result i32)))
  (import "ting" "storage_set" (func (param i32 i32 i32)))
  (import "ting" "return" (func (param i32 i32)))
  (memory (export "memory") 1 1)
  (data (i32.const 32) "\2a")
  (func (export "invoke")
    (if (i32.ge_s (call 0 (i32.const 0) (i32.const 64) (i32.const 1)) (i32.const 0))
      (then (call 2 (i32.const 64) (i32.const 1)))
      (else (call 1 (i32.const 0) (i32.const 32) (i32.const 1))))))"#,
    )
    .unwrap()
}
/// A program whose initializer does nothing and whose calls emit ten bytes
/// and then revert with three: output a block carries nothing of.
fn loud_reverter() -> Vec<u8> {
    wat::parse_str(
        r#"(module
  (import "ting" "input" (func (param i32 i32) (result i32)))
  (import "ting" "emit" (func (param i32 i32)))
  (import "ting" "revert" (func (param i32 i32)))
  (memory (export "memory") 1 1)
  (func (export "invoke")
    (if (i32.gt_s (call 0 (i32.const 0) (i32.const 0)) (i32.const 0))
      (then
        (call 1 (i32.const 0) (i32.const 10))
        (call 2 (i32.const 0) (i32.const 3))))))"#,
    )
    .unwrap()
}
const BUDGET: [u64; 4] = [1_000_000, 128, 16_384, 16_384];
fn call(target: [u8; 32], input: Vec<u8>) -> Transaction {
    Transaction {
        budget: Resources(BUDGET),
        payload: Payload::Call { target, input },
    }
}
/// The identifier a transaction's deployment at `ordinal` receives (wiki: vm/lifecycle).
fn contract_id(g: &Genesis, tx: &Transaction, ordinal: u64) -> [u8; 32] {
    let mut bytes = g.chain_id().to_vec();
    bytes.extend(tx.id(g.chain_id()));
    bytes.extend(ordinal.to_le_bytes());
    hash("contract", &bytes)
}
fn fresh(g: &Genesis) -> Runtime {
    Runtime::new(State::default(), Context::from_genesis(g))
}
fn candidate(g: &Genesis, transactions: Vec<Transaction>) -> Block {
    let mut block = fresh(g)
        .build_candidate(g, &[], transactions, g.timestamp + 60, [0; 4], vec![])
        .unwrap();
    mine(&mut block);
    block
}
/// A named change to a block, for tests of what validation refuses.
type Tamper = (&'static str, fn(&mut Block));
fn sum_of(block: &Block) -> Resources {
    let mut used = Resources([0, 0, BLOCK_BASE + block.producer.len() as u64, 0]);
    for r in &block.receipts {
        for i in 0..4 {
            used.0[i] += r.used.0[i];
        }
    }
    used
}

#[test]
fn a_contract_deployed_in_a_block_is_callable_later_in_the_same_block() {
    let g = Genesis::experimental();
    let deploy = deployment(keeper(), BUDGET);
    let id = contract_id(&g, &deploy, 0);
    // Deploy, then call: the call reads what the initializer wrote.
    let block = candidate(&g, vec![deploy.clone(), call(id, vec![])]);
    assert_eq!(block.receipts[0].status, Status::Success);
    assert_eq!(block.receipts[0].result, id);
    assert_eq!(block.receipts[1].status, Status::Success);
    assert_eq!(block.receipts[1].result, vec![0x2a]);
    assert_eq!(block.header.used, sum_of(&block));
    let validator = fresh(&g);
    validator.validate_and_apply_block(&g, &[], &block).unwrap();
    assert_eq!(validator.storage(id, [0; 32]), Some(vec![0x2a]));
    assert_eq!(validator.code(id), Some(keeper()));
    // Call, then deploy: the call finds nothing and the deployment still lands
    // under the same identifier, which depends on the transaction, not its place.
    let reversed = candidate(&g, vec![call(id, vec![]), deploy.clone()]);
    assert_eq!(reversed.receipts[0].status, Status::Missing);
    assert_eq!(reversed.receipts[1].status, Status::Success);
    assert_eq!(reversed.receipts[1].result, id);
    assert_eq!(reversed.header.used, sum_of(&reversed));
    fresh(&g)
        .validate_and_apply_block(&g, &[], &reversed)
        .unwrap();
    // The same deployment twice in one block: the second finds its identifier
    // taken, and the block is valid with that receipt.
    let twice = candidate(&g, vec![deploy.clone(), deploy]);
    assert_eq!(twice.receipts[0].status, Status::Success);
    assert_eq!(twice.receipts[1].status, Status::Exists);
    assert!(twice.receipts[1].used.0[0] < twice.receipts[0].used.0[0]);
    assert_eq!(twice.header.used, sum_of(&twice));
    fresh(&g).validate_and_apply_block(&g, &[], &twice).unwrap();
}

#[test]
fn every_commitment_is_checked_and_a_mismatch_leaves_no_trace() {
    let g = Genesis::experimental();
    let nop = deployment(
        wat::parse_str("(module (func (export \"invoke\")))").unwrap(),
        BUDGET,
    );
    let block = candidate(&g, vec![deployment(trivial(), BUDGET), nop]);
    fresh(&g).validate_and_apply_block(&g, &[], &block).unwrap();
    let tampers: [Tamper; 3] = [
        ("state root", |b| b.header.state_root[0] ^= 1),
        ("resource totals", |b| b.header.used.0[0] += 1),
        ("receipts root", |b| b.header.receipts_root[0] ^= 1),
    ];
    for (name, tamper) in tampers {
        let mut bad = block.clone();
        tamper(&mut bad);
        mine(&mut bad);
        let validator = fresh(&g);
        assert_eq!(
            validator
                .validate_and_apply_block(&g, &[], &bad)
                .unwrap_err()
                .0,
            "execution commitment mismatch",
            "{name}"
        );
        assert_eq!(validator.state_root(), State::default().root(), "{name}");
    }
    // Declared totals below what the transactions use stop execution where
    // they are passed, before the rest of the block runs; a receipt that
    // differs stops it as soon as that transaction ends.
    let first = block.receipts[0].used;
    let early: [Tamper; 2] = [
        ("compute declared below the first transaction's", |b| {
            b.header.used.0[0] = b.receipts[0].used.0[0] - 1;
        }),
        ("the first receipt", |b| b.receipts[0].result.push(0)),
    ];
    for (name, tamper) in early {
        let mut bad = block.clone();
        tamper(&mut bad);
        if name == "the first receipt" {
            bad.header.receipts_root = ting_core::hash::vector_root(
                "receipts",
                &bad.receipts.iter().map(Encode::encode).collect::<Vec<_>>(),
            );
        }
        mine(&mut bad);
        let validator = fresh(&g);
        let error = validator
            .validate_and_apply_block(&g, &[], &bad)
            .unwrap_err()
            .0;
        let expected = if name == "the first receipt" {
            "execution commitment mismatch"
        } else {
            "executed block resource limits"
        };
        assert_eq!(error, expected, "{name}");
        assert_eq!(validator.state_root(), State::default().root(), "{name}");
    }
    assert!(first.0[0] > 0);
    // The transactions themselves are committed too: swapping two of them
    // under the same header fails before anything executes.
    let mut swapped = block.clone();
    swapped.transactions.swap(0, 1);
    assert_eq!(
        fresh(&g)
            .validate_and_apply_block(&g, &[], &swapped)
            .unwrap_err()
            .0,
        "transaction root"
    );
    // A body with fewer receipts than transactions is refused, not encoded.
    let mut short = block.clone();
    short.receipts.pop();
    assert_eq!(
        fresh(&g)
            .validate_and_apply_block(&g, &[], &short)
            .unwrap_err()
            .0,
        "receipt count"
    );
}

#[test]
fn blocks_chain_through_history_and_the_parent_state_is_checked() {
    let g = Genesis::experimental();
    let runtime = fresh(&g);
    let b1 = candidate(&g, vec![deployment(trivial(), BUDGET)]);
    runtime.validate_and_apply_block(&g, &[], &b1).unwrap();
    let history = vec![b1.header.clone()];
    let mut b2 = runtime
        .build_candidate(&g, &history, vec![], g.timestamp + 120, [0; 4], vec![])
        .unwrap();
    mine(&mut b2);
    assert_eq!(b2.header.height, 2);
    assert_eq!(b2.header.parent, b1.header.id());
    assert_eq!(b2.header.state_root, b1.header.state_root);
    runtime.validate_and_apply_block(&g, &history, &b2).unwrap();
    // Without block 1's state, neither validation nor assembly of block 2 proceeds.
    let other = fresh(&g);
    assert_eq!(
        other
            .validate_and_apply_block(&g, &history, &b2)
            .unwrap_err()
            .0,
        "parent state mismatch"
    );
    assert_eq!(
        other
            .build_candidate(&g, &history, vec![], g.timestamp + 120, [0; 4], vec![])
            .unwrap_err()
            .0,
        "parent state mismatch"
    );
}

#[test]
fn block_totals_accumulate_across_transactions_up_to_the_capacity() {
    // Compute: fifty calls to a missing program fit a capacity of fifty such
    // calls exactly, and the fifty-first cancels the block.
    let g = capped();
    let missing = call([7; 32], vec![]);
    let full = candidate(&g, vec![missing.clone(); 50]);
    assert_eq!(full.header.used.0[0], 50 * MISSING);
    assert!(full.receipts.iter().all(|r| r.status == Status::Missing));
    fresh(&g).validate_and_apply_block(&g, &[], &full).unwrap();
    assert_eq!(
        fresh(&g)
            .build_candidate(&g, &[], vec![missing; 51], g.timestamp + 60, [0; 4], vec![])
            .unwrap_err()
            .0,
        "executed block resource limits"
    );
    // Two transactions that each fit alone do not fit together: the block's
    // running total, not each receipt, is what the capacity bounds.
    let one = fresh(&Genesis::experimental())
        .execute(&deployment(trivial(), BUDGET))
        .unwrap()
        .used
        .0[0];
    let mut exact = Genesis::experimental();
    exact.initial.0[0] = one;
    exact.minimum.0[0] = one.min(exact.minimum.0[0]);
    let single = candidate(&exact, vec![deployment(trivial(), BUDGET)]);
    assert_eq!(single.header.used.0[0], one);
    let pair = vec![deployment(trivial(), BUDGET); 2];
    assert_eq!(
        fresh(&exact)
            .build_candidate(
                &exact,
                &[],
                pair.clone(),
                exact.timestamp + 60,
                [0; 4],
                vec![]
            )
            .unwrap_err()
            .0,
        "executed block resource limits"
    );
    let mut forged = single.clone();
    forged.transactions = pair;
    forged.receipts.push(forged.receipts[0].clone());
    forged.header.transactions_root =
        consensus::commitments(&forged.transactions, &forged.receipts).0;
    mine(&mut forged);
    let validator = fresh(&exact);
    assert_eq!(
        validator
            .validate_and_apply_block(&exact, &[], &forged)
            .unwrap_err()
            .0,
        "executed block resource limits"
    );
    assert_eq!(validator.state_root(), State::default().root());
    // Data: producer data up to the capacity less the block's base fits;
    // one byte more is refused before anything executes.
    let g = Genesis::experimental();
    let room = (g.initial.0[2] - BLOCK_BASE) as usize;
    let block = fresh(&g)
        .build_candidate(&g, &[], vec![], g.timestamp + 60, [0; 4], vec![1; room])
        .unwrap();
    assert_eq!(block.header.used.0[2], g.initial.0[2]);
    assert_eq!(
        fresh(&g)
            .build_candidate(&g, &[], vec![], g.timestamp + 60, [0; 4], vec![1; room + 1])
            .unwrap_err()
            .0,
        "block resource limits"
    );
}

#[test]
fn assembly_refuses_bad_votes_and_envelopes_below_their_own_base() {
    let g = Genesis::experimental();
    let runtime = fresh(&g);
    assert_eq!(
        runtime
            .build_candidate(&g, &[], vec![], g.timestamp + 60, [3, 0, 0, 0], vec![])
            .unwrap_err()
            .0,
        "header fields"
    );
    // A transaction whose data budget cannot pay for its own envelope never
    // decodes, and a constructed one is refused by assembly and validation.
    let mut thin = call([7; 32], vec![]);
    let base = thin.encode().len() as u64 + 81;
    thin.budget = Resources([1000, 10, base, 0]);
    assert_eq!(Transaction::decode(&thin.encode(), 1 << 20).unwrap(), thin);
    thin.budget.0[2] = base - 1;
    assert_eq!(
        Transaction::decode(&thin.encode(), 1 << 20).unwrap_err().0,
        "data budget below envelope base"
    );
    assert_eq!(
        runtime
            .build_candidate(
                &g,
                &[],
                vec![thin.clone()],
                g.timestamp + 60,
                [0; 4],
                vec![]
            )
            .unwrap_err()
            .0,
        "invalid transaction envelope"
    );
    assert_eq!(runtime.state_root(), State::default().root());
    let receipt = Receipt {
        transaction: thin.id(g.chain_id()),
        status: Status::Data,
        used: Resources::ZERO,
        result: vec![],
        events: vec![],
    };
    let forged = block(
        &g,
        thin,
        receipt,
        State::default().root(),
        Resources([0, 0, BLOCK_BASE, 0]),
    );
    assert_eq!(
        fresh(&g)
            .validate_and_apply_block(&g, &[], &forged)
            .unwrap_err()
            .0,
        "invalid transaction envelope"
    );
}

#[test]
fn the_serialized_block_is_bounded_before_execution() {
    let mut g = Genesis::experimental();
    g.initial.0[2] = 1024;
    g.minimum.0[2] = 1024;
    g.validate().unwrap();
    // The header's declared total fits; the body does not.
    let producer = vec![7u8; 800];
    let (txroot, rcroot) = consensus::commitments(&[] as &[Transaction], &[] as &[Receipt]);
    let mut block = Block {
        header: Header {
            chain_id: g.chain_id(),
            height: 1,
            parent: g.block_id(),
            timestamp: g.timestamp + 60,
            target: g.initial_target.clone(),
            transactions_root: txroot,
            receipts_root: rcroot,
            state_root: State::default().root(),
            used: Resources([0, 0, BLOCK_BASE, 0]),
            votes: [0; 4],
            producer_hash: producer_hash(&producer),
            nonce: 0,
        },
        producer,
        transactions: vec![],
        receipts: vec![],
    };
    mine(&mut block);
    assert!(block.encode().len() > 1024);
    assert_eq!(
        fresh(&g)
            .validate_and_apply_block(&g, &[], &block)
            .unwrap_err()
            .0,
        "serialized block data limit"
    );
}

#[test]
fn discarded_output_is_charged_to_the_block_but_not_carried_by_it() {
    let g = Genesis::experimental();
    let deploy = deployment(loud_reverter(), BUDGET);
    let id = contract_id(&g, &deploy, 0);
    let block = candidate(&g, vec![deploy, call(id, vec![1])]);
    let reverted = &block.receipts[1];
    assert_eq!(reverted.status, Status::Revert);
    assert_eq!(reverted.result, vec![0; 3]);
    assert!(reverted.events.is_empty());
    // Everything in the block's data total is in its bytes, except the ten
    // emitted bytes and their event base, paid for and then thrown away.
    assert_eq!(
        block.header.used.0[2] - block.encode().len() as u64,
        36 + 10
    );
    let quiet = candidate(&g, vec![deployment(loud_reverter(), BUDGET)]);
    assert_eq!(quiet.header.used.0[2], quiet.encode().len() as u64);
    fresh(&g).validate_and_apply_block(&g, &[], &block).unwrap();
}
