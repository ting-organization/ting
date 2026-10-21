# Seed scenarios

This guide maps the seed's interaction, edge and freeze scenarios to the tests
that state them, and describes how to reproduce the checks. The seed is built
from ordinary transactions on `Genesis::development()`. Its size follows from
the scenarios and the packing rules; it is not a target.

## Scenario lookup

Each listed function is registered in `seed.rs`. Its variants become real
deployments and calls, with exact expected receipts and logical state. Most
cases live beside the other tests of the same rule.

Interactions between programs, frames and blocks:

| File and test | Dimensions and observable evidence |
|---|---|
| `isolation::reentrant_storage_is_shared_but_instances_and_checkpoints_are_separate` | A→B→A with successful, reverted and trapped B. Inner A observes the outer write, starts with fresh memory/global values and reports its immediate caller and own identity. Outer A resumes its original instance and sees either the committed inner write or the restored outer write. |
| `isolation::sibling_and_grandchild_effects_rollback_to_their_own_checkpoints` | A successful sibling, a failing child with a successful grandchild, and another successful sibling, followed by parent success/revert. Child failures cover revert, trap and C/S/D/G exhaustion. The last sibling returns the old stored byte before overwriting it, proving restoration; exact event order and all retained charges are checked. |
| `isolation::identical_code_keeps_contract_storage_and_read_only_frames_separate` | Two contracts with identical code store different bytes under the same logical key. Interleaved ordinary calls, a refused read-only mutation, a reverted mutation and subsequent reads/writes prove storage and identity separation and restoration of ordinary mode after read-only calls. |
| `calls::failed_initializers_undo_descendants_without_rewinding_ordinals_or_preparation` | Initializers write storage, emit, deploy descendants and call an existing writer before revert, trap or failure at the final identifier C/D debit. The factory deploys again at ordinal 2 and calls the writer warm. Parent success/revert and later calls to all three derived identifiers check installation rollback, ordinal retention, preparation and persistent state. |
| `calls::last_results_survive_reads_and_are_replaced_by_every_child_attempt` | 1/33-page callers; returned/reverted lengths 0, 1, 31, 32, 33, 65; length queries, short reads and repeated reads after overwriting the destination. The next attempt covers missing calls, call/deploy receive failures, invalid/successful deployment and separately excessive C/S/D/G requests for calls and deployments. All copied bytes, sentinels, statuses and charges are checked. |
| `budgets::remaining_context_tracks_nested_consumption_and_unused_allowances_in_every_counter` | All four remaining counters are returned before/after a child and a following successful sibling. The first child succeeds, reverts, traps or exhausts C/S/D/G. Parent allowances are exact modeled totals; failed child consumption remains charged, unused allowance remains available and the sibling fills the remaining S/G where applicable. |
| `heavy::several_ordered_transactions_reach_each_exact_block_capacity` | Five ordered stateful calls mix successes, revert and trap; three successes have identical transaction bytes and distinct results. A sixth transaction fills C, S, D or G exactly. A later block repeats the mutating transaction and observes the committed state. Each exact-fit block has a separately mined one-unit-over rejection counterpart. |
| `chain_histories` and seed builder | Development timestamps include a block below its parent but exactly one second above median time, and positive/negative fractional schedule offsets. Every header target is checked with separate floor-division and polynomial arithmetic. A 33-block mined companion uses three-block epochs to activate increases, holds, decreases and floors after the full notice period. Unequal-work votes demonstrate that two of three increasing headers can still lose. |

Prices, spans and output at their edges:

| File and test | Dimensions and observable evidence |
|---|---|
| `calls::nested_deployment_output_boundaries_keep_charges_and_ordinals` | Direct and indirect deployment; initializer output lengths 0, 1, 31, 32, 33, 63, 64, 65; failure at the initializer output or final identifier debit; exact and spare data; compute/data precedence. Parent returns status, last-result length and the next deployment's identifier, checking ordinal consumption and installation rollback. |
| `calls::indirect_children_resume_with_results_and_live_operands` | Direct and indirect host calls; 1, 2, 16, 17, 32, 33, 48 pages; successful, reverted, trapped, compute-exhausted and access-exhausted children. Returned memory witnesses check status, result, suspended operands and continued execution. |
| `calls::deployment_receives_code_and_input_as_two_spans_rounded_together` | Code length residues 0, 1, 31 modulo 32; input lengths 0, 1, 31, 32, 33; overlapping and separate code/input spans; child budgets around receiving and completing deployment. Checks combined word rounding, separate span charges and whether deployment begins. |
| `calls::grown_parents_resume_at_their_own_prices_after_a_child` | Growth across 1→2, 16→17, 32→33, 47→48, 63→64, 127→128 pages; children with 1 or 33 pages; input lengths 0, 1, 31, 32, 33; returned and reverted output. Caller loads, stores, result copies and output must resume at the caller's current prices. |
| `calls::a_parent_failure_undoes_successful_child_writes_and_events_but_keeps_charges` | A child successfully writes, emits and returns before its parent traps or exhausts compute/data, at 1 and 33 pages. No child effects survive; exact child charges and generated data do. Existing cases also cover parent revert. |
| `host::input_and_producer_getters_copy_whole_or_nothing_across_price_steps` | Input and producer getters; 1, 2, 16, 17, 32, 33, 48, 64 pages; lengths 0, 1, 31, 32, 33, 63, 64, 65; destination capacity below, at and above the length. Returns sentinel bytes and reported length to distinguish no copy, partial copy and whole copy. |
| `host::storage_get_snapshots_an_aliased_key_before_copying_its_value` | The same memory/length boundaries, overlapping key and destination at offsets 0, 1, 16, 32, with short/exact/spare capacities. Returned memory checks both value and untouched bytes. |
| `host::code_hash_snapshots_aliased_targets_and_leaves_short_outputs_untouched` | Present and absent targets, overlapping destinations at offsets 0, 1, 16, 32, capacities 0, 31, 32, 33 and the getter memory tiers. Checks hash, reported length, sentinels and missing-target behavior. |
| `memory::every_affordable_memory_price_transition_has_a_byte_witness` | Both sides of every load/store/word/span price change reachable by this deployment-and-call fixture within the development compute capacity. The maximum is derived using the complete independent deployment/call model. Overlapping copy/fill and loads/stores return a byte witness. |
| `budgets::output_debits_are_atomic_at_every_memory_step_and_word_edge` | Return, revert and emit at eight representative memory sizes and eight byte lengths, with exact C/D, spare C/D, each one short and both one short where possible. Checks atomic refusal and compute-before-data precedence. |
| `generated::seeded_programs_of_statements_run_as_their_model_says` | 192 deterministic seeds include storage deletion as well as writes/reads, memory, locals, global, loops, events and child calls. Nontrapped programs also run at exact final C/D, C−1, D−1 and both−1. Independently derived child requests keep the same path admissible under the tight parent budgets. |
| `state_histories::thousands_of_records_survive_churn_failed_writes_and_later_reads` | Creates 4,096 live records in batches; reads ranges; deletes and recreates ranges; shrinks values to zero and regrows them; attempts reverted and trapped overwrites/new records; verifies later reads. A Rust map declares the entire expected storage after each step across blocks. |
| `heavy::random_small_copies_in_a_large_memory_fill_the_compute` | Random small copies in 64 MiB now write changing source values and return an independently predicted checksum of destination loads. Skipping copies can no longer pass because the memory was all zero. |
| `heavy::one_memory_growth_fills_the_compute` | Returns the grown page count and proves newly added memory is zero, writable and readable at its far edge. |
| `heavy::discarded_child_outputs_fill_data_even_when_the_block_is_small` | Near-full data consumption through repeated child results, or child events/results subsequently rolled back by the parent. Checks retained data charges, prepared-code reuse and the small surviving receipt. |

The freeze witnesses additionally cover:

- `validation::consensus_extensions_are_rejected_even_when_the_program_never_uses_them`:
  13 plausible native host names, each with 0–8 i32 parameters and with/without
  an i32 result, remain invalid even when unused. Extra import namespaces and
  custom sections claiming optional version/features also remain invalid.
  Exact InvalidModule receipts and charges make these extensions observable.
- `primitives::binary_data_and_zero_compute_remain_valid_without_application_policy`:
  non-UTF8 input, producer bytes, events and output remain valid. Zero compute
  and zero access allowances produce ordinary failure receipts; successful
  calls need no growth allowance. Identical unsigned calls repeat without a
  protocol nonce, signature or account requirement.

## What the tests produce

The development seed is published in `conformance/seed/`: `development.blocks`,
the blocks framed with their lengths, and `development.json`, naming each
block's section and each transaction's case and step. The production seed beside
it, `production.blocks` and `production.json`, was built the same way once, on the
production genesis, and only replays. The test that builds the
seed compares both with what it builds and fails where they differ; after a
deliberate change, `TING_WRITE_VECTORS=1` rewrites them. In memory, the builder
also keeps for every transaction its id, its packed receipt and every source test
and variant it stands for, and checks them against the bundle; a variant ordinal
is local to its test's collection order, not a permanent case identifier.

The voting chain is published the same way in `conformance/chains/`:
`voting.blocks` and `voting.json`, with its complete genesis, the active
capacities computed independently at every height, each header's votes and
target, and each transaction's id and receipt. It is a separate valid chain with
a three-block epoch; the development genesis is unchanged by it.

The four exact-fit blocks each have a rejected counterpart one unit over in the
filled resource. Executing its transactions with that one unit more of room must
reproduce its stated receipts, resources and state, and ordinary validation must
reject it and leave the state as it was. The counterparts are written to
`seed/development/rejections/` in the target's temporary directory, beside
`coverage.json`, the coverage gate's counts and the block ids they came from.
An instruction refused at its dispatch debit does not count as executed.

Expected state roots are computed separately from the cases' declared changes,
installations and deletions included, with their own domain encoding, leaf
hashing and recursive compact-tree construction. They never read the runtime's
root, tree nodes or change list. The tree algorithm still implements the same
specification; it is no evidence that the specification itself is right.

## Reproduce

From the repository root:

```sh
cargo test --workspace --locked
cargo test --release --locked --test vm
cargo test --release --locked --features coverage --test vm coverage
cargo test --release --locked --package ting-node --test seed_replay -- --ignored
python tests/vm/mutation_check.py
```

CI runs the first four on Linux, Windows and macOS. The node replay imports the
published seed and the voting chain through the node's own import path with a
1 MiB database cache and no undo window, reopens the store every 17 blocks and
at the end, imports the same block again after each restart, then mines the next
block and reopens it.

`TING_CONFORMANCE_OUTPUT` sends the rejections and the coverage report to
another directory, for comparing a debug and a release run side by side.

The Python audit copies the tracked and untracked, non-ignored source files into
a temporary directory. It checks that each witness passes, then injects nine
faults one at a time: no span prices, words rounded down, prices kept from
before a memory growth, large-memory copies skipped, a refused child's result
kept, state-node children swapped, ordinals rewound on rollback, an earlier
sibling's event removed, and a new native host name admitted. Each must make
its witness fail. The sources get fresh timestamps and only the audit's own
packages are cleaned before the baseline, so a previously compiled mutant cannot
stand in for it. A compile error, a missing test or a surviving mutant fails the
audit. Logs and `report.json` are kept under `target/tmp/seed/mutations/`, build
products in a separate target directory; neither the real sources nor the seed
are changed.

## What a finite seed cannot show

The freeze witnesses cover both directions of drift: rejecting behavior that is
valid and accepting features that are forbidden. With the exact opcode, import
and module-profile tables, they make many changed rules fail on replay. They
cannot prove that an implementation has no extra rule, especially one that
activates only after the seed or on an input no case tries. The frozen
specification, and the rule that changed rules need a new genesis, stay
necessary; passing a finite seed is no permission to change validation at later
heights.

A valid chain also cannot hold a block that validators must reject, so invalid
blocks, non-canonical encodings and limits a block cannot reach stay with the
[consensus vectors](../../conformance/vectors). Direct state-tree vectors keep
the extreme paths too: storage keys are hashed, so a chosen long prefix or an
exact 256-level tree shape would need an infeasible preimage search, and the
4,096-record history tests realistic keys instead. Nine detected faults show
sensitivity to those nine faults only; new consensus-sensitive families should
add their own.

These are limits on the evidence, not reasons to keep reachable cases off the
chain: ordinary VM behavior belongs in transactions with observable witnesses
and independently stated charges.
