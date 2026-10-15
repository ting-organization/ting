//! Top-level deployments (wiki: vm/lifecycle, vm/host): the ContractId and the
//! existence check, validation at the code price, the manifest and code
//! records with their reads, leaves and growth, the initializer on code the
//! deployment prepared, the identifier's atomic debit, every budget boundary,
//! a collision, a later call, and initializers that trap, run out or return
//! something.
use crate::{
    dsl::{escaped, import, invoke, memory, module, wasm},
    harness::{deployment, warm_entry_of},
};
use ting_core::protocol::Status;
use vm_model::{
    BUDGET, CONTRACT_ID, IDENTIFIER, KEY, Ledger, MANIFEST, RECORD_BASE, TRANSACTION_BASE,
    code_hash, code_price, cost, deploy_size, installation, leaf, words,
};

fn nop() -> String {
    module(&[&invoke("nop")])
}
fn dispatch(n: u64) -> Ledger {
    Ledger {
        dispatch: n,
        ..Default::default()
    }
}

#[test]
fn a_deployment_at_each_budget_boundary() {
    let text = nop();
    let code = wasm(&text);
    let n = code.len() as u64;
    let envelope = TRANSACTION_BASE + deploy_size(n, 0);
    let both_records = RECORD_BASE + MANIFEST + RECORD_BASE + n;
    // Everything before the initializer: the root, the ContractId, the
    // existence check, validation, and the writes of the manifest and the code.
    let installed = 1 + installation(&code);
    // The initializer's entry, warm since validation prepared the code, and
    // the nop.
    let initialized = installed + warm_entry_of(&text) + cost("nop");
    // What a successful deployment of a nop needs: the above and the
    // identifier; the existence check, two writes of two accesses and the
    // entry's two reads; the envelope and the identifier; both records' growth.
    let needed = [initialized + IDENTIFIER, 7, envelope + 32, both_records];
    for dimension in 0..4 {
        for extra in [0, 1] {
            let mut budget = BUDGET;
            budget[dimension] = needed[dimension] + extra;
            deployment(&text)
                .named(format!("dimension {dimension}, {extra} to spare"))
                .deploy()
                .budget(budget)
                .charged(dispatch(cost("nop")))
                .run();
        }
        let mut budget = BUDGET;
        budget[dimension] = needed[dimension] - 1;
        let (status, consumed) = match dimension {
            // The identifier's C-and-D debit is refused as a whole; the
            // records were written and rolled back, their growth stands.
            0 => (Status::Compute, [initialized, 7, envelope, both_records]),
            // The initializer's manifest read is the sixth access and its
            // code-record read the seventh, which does not fit: the records
            // stand and no instance is prepared.
            1 => (Status::Access, [installed + KEY, 6, envelope, both_records]),
            2 => (Status::Data, [initialized, 7, envelope, both_records]),
            // The code record's growth does not fit after its old-record
            // read, the fourth access, so its leaf is never charged; the
            // manifest's growth stands.
            _ => (
                Status::Growth,
                [
                    1 + CONTRACT_ID
                        + KEY
                        + code_price(&code)
                        + code_hash(n)
                        + KEY
                        + leaf(MANIFEST)
                        + KEY,
                    4,
                    envelope,
                    RECORD_BASE + MANIFEST,
                ],
            ),
        };
        deployment(&text)
            .named(format!("dimension {dimension}, one short"))
            .deploy()
            .budget(budget)
            .expect(status)
            .consumed(consumed)
            .run();
    }
}

#[test]
fn the_same_transaction_again_collides_and_the_program_stays_callable() {
    let text = nop();
    let n = wasm(&text).len() as u64;
    deployment(&text)
        .deploy()
        .charged(dispatch(cost("nop")))
        // The identifier exists: the root, the ContractId and one access that
        // finds the manifest.
        .deploy()
        .expect(Status::Exists)
        .consumed([
            1 + CONTRACT_ID + KEY,
            1,
            TRANSACTION_BASE + deploy_size(n, 0),
            0,
        ])
        // The deployed program runs in a later transaction, which has
        // prepared nothing: its entry pays the code price again, and the nop.
        .then_call()
        .charged(dispatch(cost("nop")))
        .run();
}

#[test]
fn an_initializer_that_traps_fails_the_deployment_and_installs_nothing() {
    let text = module(&[&invoke("nop unreachable nop")]);
    let code = wasm(&text);
    let n = code.len() as u64;
    // The installation, the initializer's warm entry and two instructions
    // stand; the records are rolled back and their growth is not.
    deployment(&text)
        .deploy()
        .expect(Status::Trap)
        .consumed([
            1 + installation(&code) + warm_entry_of(&text) + cost("nop unreachable"),
            7,
            TRANSACTION_BASE + deploy_size(n, 0),
            RECORD_BASE + MANIFEST + RECORD_BASE + n,
        ])
        .run();
}
#[test]
fn an_initializer_out_of_compute_stops_exactly_at_its_limit() {
    let text = module(&[&invoke("nop unreachable nop")]);
    let code = wasm(&text);
    let n = code.len() as u64;
    // Everything up to the initializer's entry, then one instruction fits and
    // the next does not.
    let limit = 1 + installation(&code) + warm_entry_of(&text) + cost("nop");
    deployment(&text)
        .deploy()
        .budget([limit, BUDGET[1], BUDGET[2], BUDGET[3]])
        .expect(Status::Compute)
        .consumed([
            limit,
            7,
            TRANSACTION_BASE + deploy_size(n, 0),
            RECORD_BASE + MANIFEST + RECORD_BASE + n,
        ])
        .run();
}
#[test]
fn an_initializers_input_and_result_are_charged_and_the_result_discarded() {
    let body = "i32.const 0 i32.const 3 call $return";
    let text = module(&[
        &import("return"),
        &memory(1, 1),
        &format!("(data (i32.const 0) {})", escaped(b"abc")),
        &invoke(body),
    ]);
    // The result is the identifier; the initializer's three bytes cost their
    // word and their data all the same.
    deployment(&text)
        .deploy_with(b"in")
        .charged(Ledger {
            dispatch: cost(body),
            copies: words(3),
            discarded: 3,
            ..Default::default()
        })
        .run();
}
#[test]
fn the_identifier_depends_on_the_chain_the_transaction_and_the_ordinal_alone() {
    // The harness expects the identifier derived from those three; a block
    // with other producer data must yield the same one.
    deployment(&nop())
        .with_producer(b"another block")
        .deploy()
        .charged(dispatch(cost("nop")))
        .run();
}
