# The abstract machine

This page defines how a program runs: what state exists, how instructions act
on it, and what a node observes. The reference interpreter is one implementation
of this machine; any implementation that produces the same outcomes and counters
is correct.

## Values and the stack

The only values are i32 and i64, held in 64-bit slots. One stack per invocation
holds, for every active function, its parameters, then its declared locals, then
its operands. A call takes its arguments from the top of the caller's operands and
makes them the callee's first locals; declared locals start at zero. A return
copies the result to where the callee's slots began and discards the rest. The
slots a function can use are the ones its reservation paid for; with the record
each active function keeps, they make up the stack's height, whose growth is paid
for as memory's is ([function entry](/wiki/vm/metering#function-entry)).

## Labels and branches

`block`, `loop` and `if` open regions; each region is a label a branch can
target, counted outward from the innermost. A branch to the n-th enclosing
region cuts the operand stack back to the height the region had when it opened
and continues:

- after the region's `end` for a `block` or an `if`, carrying the region's result
  if it declares one;
- at the first instruction of the body for a `loop`, carrying nothing, which is
  how loops repeat. The `loop` instruction itself is not executed again.

Falling off the end of a region is not a branch; its result is simply left on the
stack. `return`, and the function's final `end`, leave the function the same way a
branch to its outermost label would.

## Instances

An invocation runs a fresh **instance** of the contract's module: globals set to
their initializers, the table filled from the element segments and never changed
afterwards, and memory of the declared initial size, zeroed and then overlaid with
the data segments. Memory can grow up to the declared maximum. Nothing of an
instance survives the invocation; two invocations of one contract in one
transaction do not share memory or globals.

## Host calls

A call to one of the twelve imports leaves the machine and runs the host rule
([host interface](/wiki/vm/host)). Most return a value and execution continues.
`return` and `revert` end the invocation. `call` and `deploy` **suspend** it when
they run a child: the machine's whole state is kept, a new instance runs the child to
completion, and then the parent continues with the child's status as the import's
result. When no child runs, because the sub-budget does not fit (128) or the child
cannot pay for receiving its input or code (3), the result comes back at once. The
runtime drives this with an explicit loop, so nesting never depends on a node's
native stack, and a parent's state is not visible to its child except through
storage ([calls and rollback](/wiki/vm/calls)).

## Ending

An invocation ends in one of these ways, each a deterministic outcome:

| How | Status | Result |
|---|---|---|
| `return` | 0, success | the returned bytes |
| falling off the end of `invoke` | 0, success | empty |
| `revert` | 1, revert | the returned bytes |
| a trap | 2, trap | empty |
| a budget running out | 3, 4, 5 or 6, for C, S, D or G | empty |
| breaking a host rule | 11 for a write in read-only mode, otherwise 2 | empty |

A failed invocation's state changes and events are rolled back to its start; its
consumed resources remain charged.

## Determinism

The machine has no clock, no randomness, no addresses of host memory and no
dependence on how a node allocates. A trap is a deterministic guest outcome, and
nothing else may stand in for one: a node's allocation, compiler or backend
failure is a failure of that node and is never turned into a guest outcome.
Memory growth that a node cannot satisfy is such a node failure: the budget
already paid for every byte the guest may touch, so a node that cannot provide
it is misconfigured for the block's capacity.

The reference assumes a 64-bit host. Stack positions and byte counts are
host-sized and never narrowed to 32 bits, so a budget large enough to keep more
than 2^32 slots or bytes live at once runs correctly there and nowhere smaller.

## How the reference does it

The reference, `src/vm/decode.rs` and `src/vm/interp.rs`, compiles each validated
function once into a flat list of operations with immediates decoded and branches
resolved to a position and a stack height, so the loop that runs them is little
more than a jump table. Nested `end`s compile to nothing, `else` to a jump, and
the function's `end` to a return. The compiled form is a node's own business: the
CodeHash commits to the original bytes, and a cold start and a cache hit produce
identical outcomes and counters.

No dependency defines what is accepted or what it costs. The decoder, validator,
interpreter and host all live in Ting's own repository, a few thousand lines of
safe Rust with no third-party WebAssembly code in the consensus build, so no
upgrade of a third-party crate can widen the accepted instructions or alter
charging. An instruction's byte encoding, meaning, acceptance rules, meter events
and boundary cases are written on these pages before any implementation accepts
it.

## How it is checked

Instruction semantics are verified two ways, described in `tests/vm/README.md`
and under [conformance](/wiki/reference/conformance):

- the VM test suite, one Rust test binary whose cases state each program's exact
  receipt and state, with counters from a model that does not use the VM;
- differential execution against a pinned third-party engine, on the same cases
  and on generated programs, under the test-only `oracle` cargo feature.

Together the cases cover every accepted integer family, signed edge cases,
truncation, extension and sign extension, effective-address bounds, structured
branches and loops, direct and indirect call returns and traps, bulk copies and
fills, instance reset, exact dispatch and exhaustion, memory growth, profile
rejection compared with an independent validator, resource rollback, deeply
nested calls, records larger than a page, equivalence of generated programs
between engines, and identical results from a cold start and a code-cache hit.

```text
cargo test --package ting-core --test vm --locked
cargo test --package ting-core --test vm --features oracle --locked
```
