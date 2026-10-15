//! The VM's writes reach the node's store and survive a restart: a case in
//! the VM tests' standard runs against a redb snapshot instead of memory with
//! every check the harness makes, its changes are committed, and the reopened
//! store holds exactly the records the case declared.
#[path = "../../tests/vm/dsl.rs"]
mod dsl;
#[path = "../../tests/vm/harness.rs"]
mod harness;
#[path = "../../tests/vm/shape.rs"]
mod shape;
mod support;
use dsl::{import, invoke, memory, module};
use harness::{case, k};
use support::Directory;
use ting_core::{hash::hex, protocol::Genesis, state::State};
use ting_node::store::Store;
use vm_model::{Ledger, cost, leaf, words};

#[test]
fn storage_written_by_the_vm_persists_in_redb_across_a_restart() {
    let body = "i32.const 0 i32.const 32 i32.const 7 call $storage_set i32.const 32 i32.const 7 call $return";
    let text = module(&[
        &import("storage_set"),
        &import("return"),
        &memory(1, 1),
        "(data (i32.const 32) \"persist\")",
        &invoke(body),
    ]);
    let case = case(&text)
        .call()
        .result(b"persist")
        .records([(k(0), b"persist")])
        .charged(Ledger {
            dispatch: cost(body),
            copies: words(32 + 7),
            hashing: leaf(7),
            reads: 1,
            accesses: 2,
            growth: 36 + 7,
            ..Default::default()
        });
    let dir = Directory::new();
    let g = Genesis::experimental();
    let store = Store::open(&dir.0, &g, ting_node::store::CACHE_BYTES).unwrap();
    // The program is installed in the store before the case runs.
    let mut seeded = State::default();
    for (key, value) in case.initial_records() {
        seeded.set(key, Some(value));
    }
    seeded.commit();
    let tx = store.begin().unwrap();
    Store::apply(&tx, &seeded.changes().unwrap()).unwrap();
    tx.commit().unwrap();
    // The reader is an immutable redb snapshot; the VM's writes stay in the
    // overlay until they are applied.
    let runtime = case.run_on(store.snapshot().unwrap());
    let root = runtime.state_root();
    let changes = runtime.state_changes().unwrap();
    let tx = store.begin().unwrap();
    Store::apply(&tx, &changes).unwrap();
    tx.commit().unwrap();
    drop(runtime);
    drop(store);
    let reopened = Store::open(&dir.0, &g, ting_node::store::CACHE_BYTES).unwrap();
    let snapshot = reopened.snapshot().unwrap();
    assert_eq!(hex(&snapshot.root()), hex(&root), "root after the restart");
    assert_eq!(snapshot.len(), 3, "the manifest, the code and the record");
}
