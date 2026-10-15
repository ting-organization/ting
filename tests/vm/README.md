# VM tests

The reference VM is Ting's own code: `src/vm/decode.rs` decodes, validates
and compiles the profile, `src/vm/interp.rs` executes it with inline charging
at the prices of `src/vm/price.rs`, and `src/vm/host.rs` implements the host
imports. Its tests are Rust,
in `tests/vm/`, one binary:

```sh
cargo test --test vm
```

Each test states one rule of the VM, as the [wiki's VM pages](../../node/wiki/vm.md) define it, as cases. A case is a program in
WebAssembly text with its expected outcome and its charges beside it; nothing
in a case comes from running the VM.

## A case

```rust
#[test]
fn br_if_carries_the_block_result_when_taken() {
    case(&returning("(block (result i64) i64.const 17 i32.const 1 br_if 0 drop i64.const 23)"))
        .call()
        .result(17u64.to_le_bytes())
        .charged(Ledger { dispatch: cost("block i64.const 17 i32.const 1 br_if 0") + cost(AROUND), ..Default::default() })
        .run();
}
```

`case` takes the program, installed under a fixed identifier. `call` adds a
step: a call of the program under the standard budget; `call_with` gives it
input, `budget` another budget, `deploy` and `then_call` deploy the program and
call what was deployed. The expectations belong to the last step: `expect`
names its status (success unless stated), `result` the bytes it returns,
`events` the payloads it emits, `records` the program's complete storage
afterwards (none unless stated), and `charged` what the program does on its
path. `run` executes the steps and compares every observable.

`Ledger` is the part a reader checks against the specification: the price of
the instructions dispatched, written as the executed instruction text priced
by `vm_model::cost` ([the price list](../../node/wiki/resources/prices.md)),
words copied at the program's memory tier, record values hashed into their
leaves, record reads and their key hashes, slots reserved by called functions
and the stack pages their entries add past the invocation's mark,
pages added by growth, accesses beyond the entry's two lookups, state growth,
the compute of nested invocations, nested deployments' code prices, and the
data of results and events the receipt does not show. Everything that follows
from the module or from the declared outcome is derived rather than declared:
the entry costs (the two lookups with their keys, the code price on the first
invocation of the code in the transaction, the instance's preparation, the
entry function's reservation and stack pages) are read from the module's shape, and the debits
of the declared result and events are added by the model. The few failures that happen before
a program is entered state their exact counters with `consumed`.

`with_program` installs further programs for calls between programs,
`with_storage` seeds a program's storage, `with_producer` sets the block's
producer data, `records_of` and `events_from` declare another program's
storage and events, and `installs` declares a deployment made from inside.

## What `run` checks

For every step: the status, the result, the events, the complete logical state
(what was installed, with each declared program's storage replaced and the
step's deployments added), and the exact C/S/D/G counters. A step that leaves
the state unchanged is run again with a warm code cache and again after the
cache is cleared, and must produce the identical receipt. Under the `oracle`
feature the whole case runs on the oracle engine as well, and the receipts of
the two engines must be identical byte for byte. The harness's own tests prove
that a wrong status, result, ledger or record fails a case.

## Where expectations come from

- `vm-model/`: the price list and the charging rules of [metering](../../node/wiki/vm/metering.md) and the [host interface](../../node/wiki/vm/host.md) as arithmetic
  over a `Ledger` and an `Entry`, including the code price read from section
  framing. The crate has no dependencies, and
  `ting-core` is not one of them, so an expected counter can never come
  from the code under test.
- `tests/vm/shape.rs`: the code price, the preparation, the initial memory's
  tier and each function's reserved slots, read from the module bytes with `wasmparser` and
  the Core validation algorithm's stack discipline, never with the decoder.
- Instruction semantics: Rust's integer arithmetic over the same operands in
  the numeric tables, and values stated by hand elsewhere. The integer files of
  the WebAssembly core test suite were read and their edge cases written as
  cases in this form; no suite files or converted data are kept.
- Never adjust an expected value to what the VM produces. When a case fails on
  its counters, the VM, the model or the reading of the specification is
  wrong, and the specification decides which. Correct a case with its
  explanation, or treat the disagreement as a defect.

## The modules

| Module | States |
|---|---|
| `numeric` | both widths, every operator over the edge operands and the pairs the core test suite singles out, the trapping divisions, conversions, sign extension, identities that must not be folded |
| `compositions` | seeded typed expression trees evaluated independently in Rust, run with the exact dispatch of the evaluated path, traps part way through included |
| `control` | select, br_if, if, br_table, loops, locals, globals, function exits, indirect calls, bulk memory; operands evaluated left to right; branches and returns that unwind operands |
| `memory` | every load and store width, extension and truncation, every edge of memory for every access, offsets and alignment hints, copy over every small overlap and across words, fill, zero-length ranges, growth, declared memory, data segments, prices by memory tier as memory grows across tier boundaries |
| `host` | span rules, getters that copy only what fits, the nine context selectors, storage reads with their key hashes, writes with their leaf hashes at the block boundaries, deletions, length queries of large records, the code hash, return, revert and emit |
| `calls` | a child's outcome and retained charges, the last-result buffer, read-only mode explicit and inherited, internal deployments and ordinals, code prepared once per transaction, recursion and a chain of four hundred programs |
| `deployment` | validation at the code price, records, the identifier, every budget boundary, collisions, failing initializers |
| `programs` | a persistent counter and byte processing over several transactions |
| `reservations` | parameters, locals and operand height reserved at every entry, depth bounded by the budget alone |
| `budgets` | each boundary of each counter, an instruction whose price does not fit refused whole, the entry's debits in order, atomic debits refused whole, rollback keeping its charges |
| `validation` | acceptance: structural bounds at their edges, imports, exports, instructions outside the profile, Core typing rules, every control construct against every body shape, typing in dead code, tables, segments, every immediate in every LEB form, sections complete, in order and unique, the three rejection reasons, the code price read from framing and kept by invalid code, accepted byte mutations |
| `wide` | the four wide-arithmetic instructions over edge and seeded operands against Rust's 128-bit arithmetic, the order of their two results, their prices, typing and encodings |
| `oracle` (feature) | seeded random programs and systematic byte mutations on both engines and both validators |

`harness.rs` runs a case, `dsl.rs` composes module text, `shape.rs` reads a
module's shape, and `vm-model` turns a ledger into counters. The node runs a
case of the same standard against a redb snapshot and a restart in
`node/tests/vm_conformance.rs`.

## The oracle

The consensus build carries no third-party WebAssembly code. The `oracle`
cargo feature adds, for tests only, the pinned `wasmi` engine behind the same
host layer on instrumented modules and the `wasmparser`-based validator with
its own label tracker for the Core 1.0 `br_table` rule. With the feature on,
every case runs on both engines, every acceptance table is checked against
both validators, and the `oracle` module adds generated programs that mix
every instruction family, host imports, nested self-calls and budgets that run
out at arbitrary points, plus byte replacements, deletions, insertions and
truncations of seed modules. `TING_FUZZ_PROGRAMS` and
`TING_FUZZ_SEED` scale and move the batch; 400 programs run by default.

## Running

```sh
cargo test --workspace --locked
cargo test --package ting-core --test vm --features oracle --locked
```

`.github/workflows/ci.yml` runs the same on Linux, Windows and macOS, the VM
tests in debug and release, and fails when a WebAssembly crate reaches the
consensus build.

## Changing the VM

A change to instruction behaviour, charging, the host interface or failure
order is written into the wiki's VM pages first. Then the cases that state the rule are
added or corrected, with their ledgers, before the code changes. The change is
ready when the VM tests pass with and without the oracle feature.
