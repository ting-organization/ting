# VM tests

The reference VM is Ting's own code: `src/vm/decode.rs` decodes, validates
and compiles the profile, `src/vm/interp.rs` executes it with inline charging
at the prices of `src/vm/price.rs`, and `src/vm/host.rs` implements the host
imports. Its tests are Rust,
in `tests/vm/`, one binary:

```sh
cargo test --test vm
```

Each test states one rule of the VM, as the [protocol rules](../../ting-protocol-rules.md)
define it, as cases. A case is a program in
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

The test files import `deployed` as `case`: it takes the program, which a
transaction deploys before the case's steps, as the chain would. `call` adds a
step: a call of the program under the standard budget; `call_with` gives it
input, `budget` another budget, `deploy` and `then_call` deploy the program and
call what was deployed. The expectations belong to the last step: `expect`
names its status (success unless stated), `result` the bytes it returns,
`events` the payloads it emits, `records` the program's complete storage
afterwards (none unless stated), and `charged` what the program does on its
path. `run` executes the steps and compares every observable.

Every collected case has the shape of a chain: its programs are
deployed by transactions, never written into state.

- `deployed` takes the program the way the chain will: a transaction before the
  case's steps deploys it, and its steps name it as `TARGET`, which the harness
  replaces with the identifier the deployment derives.
- A deployment runs the program once as its initializer, so a program that traps
  or reverts on purpose could never be installed. `gate.rs` therefore puts a
  gate in front of the entry function: an invocation whose input is exactly
  seven bytes long returns at once. The gate reads that length with `input`,
  importing it when the program does not, and needs the program's memory for
  the call's empty span.
- The harness states each deployment's receipt itself and adds the gate's
  instructions to every call it checks, so a case declares only what its own
  program does. A test that works out an entry, a reservation or a code price
  itself uses `deployed_entry_of`, `deployed_shape` and `deployed_code`, which
  include the gate.
- `with_program` names another program by a placeholder (`CHILD`,
  `GRANDCHILD` or any other 32 bytes). It is deployed first, and its identifier
  replaces the placeholder wherever its bytes appear: in the code of the
  programs deployed after it, in inputs, results, events and records. A
  program that names itself, or two that name each other, cannot be deployed;
  a program reads its own identifier from the context instead.
- `with_storage` gives a program the records it starts with. Its deployment
  writes them, putting each key and value in memory and calling
  `storage_set` before the gate returns, and its receipt pays for them. A call
  never reaches that code.
- `result_from` states a result that only the run knows: an identifier the
  step's transaction derives, the transaction's own, or a value of its block,
  such as the height or the parent.
- `deployment` and `deployment_bytes` start from an empty state with a
  deployment of the program itself, for the rules of deployment and
  validation. A program without a memory, which the gate cannot serve, shows
  its behaviour in its own deployment.
- `next_block` puts the next step in a block of its own in the seed, after the
  block that holds the step before it; at the end of a case, the next case
  starts a block. `apart` gives each of a case's deployments a block of its
  own, and its steps a later one.
- `call_to` calls an identifier the case does not deploy, with its counters
  stated outright, and `call_deployed` the program an earlier step deployed
  from inside.

`harness::case`, which installs the program under a fixed identifier without a
transaction, remains for the harness's own tests, which show that a misstated
observable fails a case.

`seed.rs` builds the development seed from the cases, as a chain's seed is
built:
- It runs every registered semantic test, section by section, and keeps each
  case's transactions and receipts, every check passed. `SECTIONS` lists the
  tests in their sections, and a test fails when a file has one the list misses.
  The few tests that run on runtimes of their own (module verdicts too large for
  a block, rejection reasons, byte mutations, envelope rules) are listed as
  `REFERENCE` instead, and a listed test that gives the seed no case fails it.
- It opens with an empty block carrying producer data, then packs the
  transactions in order into blocks on the development genesis and mines them.
  Each section starts a block, and a block ends where its next transaction does
  not fit or a case asks for a block. A case that a later case repeats exactly
  is placed once. A block carries the producer data its cases ask for with
  `with_producer`.
- Every block receipt must equal the receipt its case produced on its own. A
  case with a `result_from` is checked against its expectations in the block it
  lands in instead. A case whose receipt depends on its block without saying
  so, such as one that returns the height as a fixed value, is named instead of
  trusted.
- At every block, a separate tree implementation hashes a plain map built from
  the cases' declared state changes and checks the header's state root. Neither
  the runtime's roots nor its incremental tree nodes supply this expectation.
- A fresh validator replays the bundle from an empty state to the same blocks
  and the same state. Every receipt in the manifest is checked against its
  actual packed occurrence, including results that depend on block context.
- Only then is the bundle compared with the published one in
  `conformance/seed/`: `development.blocks`, each block framed with a u32 length,
  which the node carries and imports into every new development chain, and
  `development.json`, the manifest naming each block's section and each
  transaction's case and step. The test fails where they differ from the seed
  it builds; after a deliberate change, `TING_WRITE_VECTORS=1 cargo test --test vm
  seed` rewrites them.
- With `TING_SEED_CHAIN=production`, the cases run on the production chain and the
  heavy blocks follow its capacity. `the_production_seed_is_built_at_launch`,
  ignored since it mines at the production difficulty, built `production.blocks`
  and `production.json` that way once, before the chain began, leaving out the six
  cases no production block can hold. `the_published_production_seed_replays`
  replays them on every run: the production seed is the chain's history, and is
  never built again.

The builder also checks each header target against independent ASERT arithmetic
and uses valid irregular timestamps, including one below its parent but one
second above the median time. Four mixed-transaction blocks fill C, S, D and G
exactly; their counterparts one unit over are checked to be rejected, and written
under `seed/development/rejections/` in the target's temporary directory with
the coverage report. `chain_histories.rs` mines and replays a 33-block chain on a
short-epoch variant of the development genesis, whose votes activate increases,
holds, decreases and the floor after the full notice, with active capacities
computed independently; it is published as `conformance/chains/voting.blocks` and
`voting.json`. Unused declarations of native cryptography, account, transfer,
time, randomness and upgrade functions stay invalid, while binary data and
transactions that fail for lack of compute stay valid. A finite seed cannot
exclude a different rule that activates after its last block; changed rules
still mean a new genesis.

`coverage.rs` is the coverage gate. It needs the `coverage` feature, which
compiles counting hooks into the VM and is never part of a node:

```sh
cargo test --release --features coverage --test vm coverage
```

A fresh validator replays the development seed with the VM counting what it
runs, and the gate fails with the list of what the seed never exercised: an
instruction of the protocol rules' instruction reference, a way a host import can end, a
status of a top-level or nested call or deployment, a rejection reason, memory
on either side of a price step or on the curve, stack pages on entry or on a
call, or a program called in a later block than its deployment. CI runs it.
It writes `coverage.json`, the counts and the replayed block ids, to
`seed/development/` in the target's temporary directory. An instruction refused
at its dispatch debit does not count as executed. The gate is broad, not proof
that every edge is tested; the [seed scenarios guide](seed-coverage.md) maps the
detailed scenarios to their tests, and `mutation_check.py` checks that curated
wrong rules each fail a named test.

`Ledger` is the part a reader checks against the protocol rules: the price of
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

`with_producer` sets the block's producer data, `records_of` and
`events_from` declare another program's storage and events, and `installs`
declares a deployment made from inside.

## What `run` checks

For every step: the status, the result, the events, the complete logical state
(what was deployed, with each declared program's storage replaced and the
step's deployments added), and the exact C/S/D/G counters. A step that leaves
the state unchanged is run again with a warm code cache and again after the
cache is cleared, and must produce the identical receipt. The harness's own tests prove
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
  its counters, the VM, the model or the reading of the protocol rules is
  wrong, and the rules decide which. Correct a case with its
  explanation, or treat the disagreement as a defect.

## The modules

| Module | States |
|---|---|
| `generated` | seeded programs of statements over locals, a global, memory, storage, events, calls of a child and counted loops, each run against a model of it in Rust that states its result, records, events, trap and every charge |
| `state_histories` | 4,096 live records, deletion and recreation, shrink and regrowth, failed writes and later reads, with a declared map after every transaction across forced block boundaries |
| `heavy` | blocks one transaction fills: compute with random small copies in 64 MiB and with one memory growth, accesses with record writes and with calls nested 511 deep, data with events and with one input, and one record grown over eight blocks to about 1 MiB |
| `primitives` | blocks of one to nine transactions, the same transaction twice in a block, a call of an identifier nothing is deployed under |
| `across` | the same call in later blocks, programs deployed in different blocks calling each other, a program deployed by a program called later, a record written, read and deleted across blocks |
| `numeric` | both widths, every operator over the edge operands and the pairs the core test suite singles out, the trapping divisions, conversions, sign extension, identities that must not be folded |
| `compositions` | seeded typed expression trees evaluated independently in Rust, run with the exact dispatch of the evaluated path, traps part way through included |
| `control` | select, br_if, if, br_table, loops, locals, globals, function exits, indirect calls, bulk memory; operands evaluated left to right; branches and returns that unwind operands |
| `memory` | every load and store width, extension and truncation, every edge of memory for every access, offsets and alignment hints, copy over every small overlap and across words, fill, zero-length ranges, growth, declared memory, data segments, memory prices in their steps as memory grows across them and on the curve above 2 MiB, a span price for every span a copy, a fill or a data segment moves |
| `host` | span rules, getters that copy only what fits, a zero-length span without a memory, copies and record lookups that do not fit, the nine context selectors against the block, storage reads with their key hashes, writes with their leaf hashes at the block boundaries, deletions, length queries of large records, the code hash, return, revert and emit, the span each copy pays above 2 MiB |
| `calls` | a child's outcome and retained charges, a child or an internal deployment ending each way it can, the last-result buffer, read-only mode explicit and inherited, internal deployments and ordinals, code prepared once per transaction, recursion and a chain of four hundred programs |
| `deployment` | validation at the code price, records, the identifier, every budget boundary, collisions, failing initializers |
| `programs` | a persistent counter, in one block and across blocks, and byte processing over several transactions |
| `reservations` | parameters, locals and operand height reserved at every entry, depth bounded by the budget alone |
| `budgets` | each boundary of each counter, an instruction whose price does not fit refused whole, the entry's debits in order, atomic debits of returned and reverted results refused whole, rollback keeping its charges |
| `validation` | acceptance: structural bounds at their edges, imports, exports, instructions outside the profile, Core typing rules, every control construct against every body shape, typing in dead code, tables, segments, every immediate in every LEB form, sections complete, in order and unique, the three rejection reasons, the code price read from framing and kept by invalid code, a price past 2^64 − 1 for a module built in memory and deployed from inside, accepted byte mutations. A rejected module is deployed and fails with status 9; an accepted one is deployed with compute for its installation and its initializer's entry alone, and runs out at its first instruction |
| `wide` | the four wide-arithmetic instructions over edge and seeded operands against Rust's 128-bit arithmetic, the order of their two results, their prices, typing and encodings |

The seed's sections, in order: S1 `primitives`; S2 `validation`; S3
`numeric`, `control` and `wide`; S4 `memory` and `reservations`; S5 `host`; S6
`deployment`, `calls` and `isolation`; S7 `programs`, `across` and
`state_histories`; S8 `budgets`;
S9 `compositions` and `generated`; S10 `heavy`.

`harness.rs` runs a case, `seed.rs` builds the seed, `coverage.rs` checks what
it exercises, `consensus.rs` writes the consensus vectors built on the seed's
first section (wiki: reference/conformance), `chain_histories.rs` the voting
chain, `dsl.rs` composes module text, `shape.rs` reads a module's shape, and
`vm-model` turns a ledger into counters. The node carries the published seed,
imports it into every new development chain, and syncs it from a peer through a
restart in `node/tests/daemon.rs`. `node/tests/seed_replay.rs` imports the whole
seed and the voting chain through the node's own import path with a 1 MiB
database cache, reopening the store every 17 blocks and importing the same block
again after each restart, then mines the next block and reopens it.

## No outside engine

The consensus build carries no third-party WebAssembly code, and neither do the
tests' judgements: every expectation is stated by a case, through `vm-model`,
never taken from another engine. The tests use `wat` to write programs as text
and `wasmparser` to read a module's shape without the decoder under test.

## Running

Set `TING_CONFORMANCE_OUTPUT` to an absolute directory to write core test
artifacts there instead of `target/tmp/seed/`. This permits independent debug
and release runs with byte-for-byte bundle comparisons. The persistent node
replay tests consume the default directory produced by the commands below.

```sh
cargo test --workspace --locked
```

`.github/workflows/ci.yml` runs the same on Linux, Windows and macOS, the VM
tests in debug and release, and fails when a WebAssembly crate reaches the
consensus build.

## Changing the VM

A change to instruction behavior, charging, the host interface or failure
order is written into the protocol rules first, and the wiki's VM pages explain it. Then the cases that state the rule are
added or corrected, with their ledgers, before the code changes. The change is
ready when the VM tests pass.
