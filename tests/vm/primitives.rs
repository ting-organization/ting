//! What every chain has before any program does anything (seed section S1;
//! rules §4, §23): blocks of every small number of
//! transactions, whose commitments pad to a power of two, the same
//! transaction twice in one block, and a call of an identifier nothing is
//! deployed under. The seed itself opens with an empty block carrying
//! producer data.
use crate::{
    dsl::{import, invoke, memory, module},
    harness::{deployed as case, deployment},
};
use ting_core::{hash::Hash, protocol::Status};
use vm_model::{BUDGET, KEY, Ledger, ROOT, TRANSACTION_BASE, call_size, cost, words};

/// An identifier no deployment derives.
const NOWHERE: Hash = [0x5a; 32];

fn nop() -> String {
    module(&[&memory(1, 1), &invoke("nop")])
}
fn ran_nop() -> Ledger {
    Ledger {
        dispatch: cost("nop"),
        ..Default::default()
    }
}

#[test]
pub(crate) fn binary_data_and_zero_compute_remain_valid_without_application_policy() {
    // Consensus imposes no text encoding, account/nonce, signature or minimum
    // compute allowance. Failed execution is still an ordinary transaction.
    let bytes = [0xff, 0xfe, 0xc0, 0xaf, 0, 0x80];
    let body = "i32.const 64 i32.const 6 call $input drop
        i32.const 64 i32.const 6 call $emit i32.const 64 i32.const 6 call $return";
    let text = module(&[
        &import("input"),
        &import("emit"),
        &import("return"),
        &memory(1, 1),
        &invoke(body),
    ]);
    let envelope = TRANSACTION_BASE + call_size(bytes.len() as u64);
    let mut test = case(&text)
        .named("freeze/binary-data-and-no-application-policy")
        .with_producer([0xff, 0, 0x80, 0xfe, 0xc0, 0xaf])
        .call_with(&bytes)
        .budget([0, 0, envelope, 0])
        .expect(Status::Compute)
        .consumed([0, 0, envelope, 0])
        .call_with(&bytes)
        .budget([BUDGET[0], 0, envelope, 0])
        .expect(Status::Access)
        .consumed([1, 0, envelope, 0]);
    for _ in 0..2 {
        test = test
            .call_with(&bytes)
            .budget([BUDGET[0], 2, envelope + 36 + 12, 0])
            .result(bytes)
            .events([bytes])
            .charged(Ledger {
                dispatch: cost(body),
                copies: words(6),
                ..Default::default()
            });
    }
    test.run();
}

#[test]
pub(crate) fn blocks_of_one_to_nine_transactions() {
    // Seven blocks of 1, 2, 3, 4, 5, 8 and 9 calls, on both sides of each
    // power of two the transaction and receipt commitments pad to; the
    // deployment before them shares the block before. Each call has its own
    // input, and so its own identifier.
    let mut c = case(&nop()).named("blocks of 1 to 9 transactions");
    let mut input = 0u8;
    for n in [1, 2, 3, 4, 5, 8, 9] {
        c = c.next_block();
        for _ in 0..n {
            c = c.call_with(&[input]).charged(ran_nop());
            input += 1;
        }
    }
    c.next_block().run();
}

#[test]
pub(crate) fn the_same_transaction_twice_in_one_block() {
    // A call that changes nothing, twice in a block of its own: the same
    // identifier and the same receipt both times.
    case(&nop())
        .named("the same call twice in a block")
        .next_block()
        .call_with(b"again")
        .charged(ran_nop())
        .call_with(b"again")
        .charged(ran_nop())
        .next_block()
        .run();
}

#[test]
pub(crate) fn a_call_of_an_identifier_nothing_is_deployed_under() {
    // The root dispatch and the manifest read with its key and one access
    // find nothing: status 8, and only the envelope besides.
    deployment(&nop())
        .named("a call of nothing")
        .call_to(NOWHERE, &[])
        .expect(Status::Missing)
        .consumed([ROOT + KEY, 1, TRANSACTION_BASE + call_size(0), 0])
        .run();
}
