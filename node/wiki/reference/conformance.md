# Conformance and testing

Ting depends on independent implementations agreeing on every byte and every
counter. That agreement is shown with worked protocol vectors any implementation can
check, with VM tests whose expectations never come from the VM, and with
side-by-side runs against an independent engine. It is never taken on trust from the
reference.

## Protocol vectors

[`protocol-vectors.json`](protocol-vectors.json) holds worked examples of:

- the genesis encoding and ChainId;
- domain-separated hashes and ordered commitments, including empty and padded lists;
- state-tree roots for present, empty and deleted records, for records split at the
  first and at the last bit, and for a record moving up after a deletion;
- an empty block, a block with producer data, and a block with a missing-program
  transaction and its receipt;
- ASERT targets on schedule, early, late, at fractional half-lives and at saturation,
  and one where flooring the exponent differs from truncating it;
- the two-thirds and majority vote thresholds, and activation across epochs;
- growth accounting through create, expand, shrink, delete and recreate;
- data accounting for output that is generated and then discarded.

The test `tests/vectors.rs` works every vector out again from the formulas on these
pages, without the code under test, and fails if the published file differs; the
reference implementation is checked against the same file. They are checked design
examples, not a complete VM conformance suite.

## Required coverage

The reference implementation must publish portable input and output vectors covering:

- canonical encoding, domain separation, empty roots and vector padding;
- state-tree insertion, splits, updates, deletion with records moving up, and empty
  values versus absent records;
- inclusive proof-of-work target checks, and accumulated work at boundary targets;
- ASERT targets on schedule, early, late, at fractional half-lives and at saturation;
- work-weighted voting, the two-thirds and majority thresholds, activation
  boundaries, saturation, floors and reorganizations;
- missing programs, duplicate transaction occurrences and deployment collisions;
- deployment attempts whose enclosing calls later revert;
- successful nested calls, caught failures, outer failures, inherited read-only mode,
  and deep nesting bounded only by budgets;
- instruction, state-access, output and growth exhaustion, with prior debits
  unchanged;
- initial memory, deterministic memory growth, global and table reset, bulk copies
  and fills, indirect calls and their traps, and sign extension;
- records larger than one page, and generated data and allocated state later
  discarded by rollback;
- identical results and resource vectors with cold caches, warm caches and compiled
  execution.

These vectors establish agreement, not capacity: throughput and safe capacities come
from adversarial workload measurements and hashpower and difficulty simulations, as
for the [production profile](/wiki/consensus/genesis#production-profile).

## The VM tests

The VM tests are Rust, one binary, `tests/vm/`. Each test states one rule of the
[VM pages](/wiki/vm) as cases: a program in WebAssembly text with its expected
outcome and its charges beside it. A case declares what the program does on its
path, as a ledger of instructions dispatched, bytes copied, records accessed, slots
reserved and state grown; a model crate with no dependency on the VM turns the ledger into the
exact C/S/D/G counters the receipt must show, and the module's entry costs are read
from its shape, never from the decoder. The harness checks the status, the result,
the events, the complete storage and the counters of every step, runs an unchanged
step again warm and after a cleared code cache, and must see the identical receipt.
How to write and read a case is described in `tests/vm/README.md` in the repository.

The cases cover every integer instruction over edge operands, every load and store
width, control flow, the host imports with their span and copy rules, calls between
programs with their retained charges and rolled-back effects, deployments at every
budget boundary, reservations, budgets, and module acceptance down to minimal
encodings and the Core typing rules. The edge cases of the WebAssembly core test
suite's integer files are written in the same form. Accounting must match exactly,
not only results, and an expected value is never adjusted to what the VM produces.

## Comparing against an independent engine

The consensus build has no third-party WebAssembly code. For testing, the `oracle`
cargo feature adds a pinned independent engine and validator. With the feature on,
every case runs on both engines and their receipts must be identical byte for byte,
every acceptance table is checked against both validators, and seeded random
programs that mix every instruction family, host imports, nested self-calls and
budgets that run out at arbitrary points run on both, as do systematic byte
mutations of seed modules.

```sh
cargo test --package ting-core --test vm --features oracle --locked
```

## Reference test suites

The Rust test suites add:

- blocks around the VM: candidates, cancellation at capacity, rejected receipts,
  producer data commitments;
- the protocol vectors executed against the reference;
- a state reader behind the VM, and a storage failure that never becomes a receipt;
- invalid-block cancellation.

On the node side, they cover reorganizations, restarts, crash recovery, corrupted
files, two-node synchronization, the API, and a VM case run against redb state and a
durable restart.

## Running them

```sh
cargo test --workspace --locked
```

`TING_WRITE_VECTORS=1 cargo test --test vectors` writes the published vectors from
the specification instead of comparing them.

## How the reference changes

- **Rules first.** A change to instruction behavior, charging, the host interface or
  failure order is written into the rules on these pages before any code: its bytes,
  semantics, meter events and boundary cases.
- **Own code, independent evidence.** The VM is implemented here, and every change
  to it must keep the VM tests passing, with and without the oracle feature. Expected
  values are never regenerated from the implementation under test.
- **Nothing implicit.** Dependency defaults never define the accepted profile. Unknown
  instructions, imports and features are rejected.
- **Exact tests.** Every change comes with cases that state its outcome and its exact
  accounting.
- **No physical accounting.** CPU timing, interpreter fuel and cache state never define a counter.
