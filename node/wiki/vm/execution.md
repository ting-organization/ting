# How a program runs

A program runs on a small abstract machine: a stack of 64-bit slots, structured
branches, a fresh instance of its module, and calls out to the host. The machine
says what state exists, how instructions act on it and what a node observes. Any
implementation that produces the same outcomes and counters is correct, however it
is built, and Ting's reference interpreter is one of them. The exact rules are in
[section 20 of the protocol rules](../../../ting-protocol-rules.md#20-the-abstract-machine).

## Values and the stack

The only values are `i32` and `i64`. Each invocation has one stack, which holds, for
every function currently active, its parameters, then its declared locals, which
start at zero, then the operands it is working on. A call takes its arguments from
the top of the caller's operands and makes them the callee's first locals. A return
copies the result to where the callee's slots began and drops the rest.

A function can use only the slots its
[reservation](profile.md#measured-for-execution) paid for, so the stack's height
follows from the functions that are active. A deeper stack is paid for the way
memory is ([function entry](metering.md#function-entry)), which is why recursion
has no fixed limit: the budget bounds it.

## Branches

There are no jumps to arbitrary addresses. `block`, `loop` and `if` open regions, and
a branch names an enclosing region by how many levels out it is, the innermost being
0. A branch cuts the operand stack back to the height it had when the region opened,
then:

- for a `block` or an `if`, continues after the region's `end`, carrying the
  region's result if it has one;
- for a `loop`, continues at the first instruction of the loop's body, carrying
  nothing. This is how loops repeat; the `loop` instruction itself does not run
  again.

A counting loop, for example, is a `loop` whose body ends with `br_if 0`: while the
condition holds, execution goes back to the top of the body, and when it fails,
execution falls through to the `end`. Falling off the end of a region is not a
branch; its result simply stays on the stack. `return`, and the function's final
`end`, leave the function the way a branch to its outermost region would.

## Instances

Each invocation runs a fresh **instance** of the contract's module: globals set to
their initial values, the table filled from the element segments and fixed from
then on, and memory of the declared initial size, zeroed and then overlaid with
the data segments. Memory can grow up to the module's declared maximum. Nothing of
an instance outlives the invocation: two calls to the same contract in one
transaction share no memory and no globals, and anything that should last goes into
storage.

## Host calls

A call to one of the twelve imports leaves the machine and runs a host function
([the host interface](host.md)). Most return a value and execution goes on; `return`
and `revert` end the invocation.

`call` and `deploy` **suspend** the invocation while a child runs: the parent's whole
state is kept, the child runs to completion in its own fresh instance, and the
parent then continues with the child's status as the import's result. When no child
can start, the import returns at once
([calls and deployments](host.md#calls-and-deployments)). A child sees nothing of
its parent except through storage ([calls and rollback](calls.md)). Implementations
run this nesting without relying on the processor's own stack, so how deep calls go
never depends on a node's native limits.

## How an invocation ends

An invocation ends in one of these ways, each a deterministic outcome:

| How | Status | Result |
|---|---|---|
| `return` | 0, success | the returned bytes |
| falling off the end of `invoke` | 0, success | empty |
| `revert` | 1, revert | the returned bytes |
| a trap | 2, trap | empty |
| a budget running out | 3, 4, 5 or 6, for C, S, D or G | empty |
| breaking a host rule | 11 for a write in read-only mode, otherwise 2 | empty |

A failed invocation's state changes and events are rolled back to its start, but
everything it consumed stays charged ([checkpoints](calls.md#checkpoints)).

## Determinism

The machine has no clock, no randomness, no addresses of host memory and nothing
that depends on how a node allocates. Every trap is a deterministic outcome of the
program, and nothing else may stand in for one: if a node runs out of its own memory
or its compiler fails, that is a failure of the node, never an outcome written into
a receipt. The budget has already paid for every byte a program may touch, so a node
equipped for the block's capacity can always provide it. Implementations must also
never narrow a size by accident: a program has 32-bit addresses, but the state of a
transaction's nested instances and stacks together is not capped at 2^32 bytes.

## How the reference does it

The reference, `src/vm/decode.rs` and `src/vm/interp.rs`, compiles each validated
function once into a flat list of operations with immediates decoded and branches
resolved, so the loop that runs them is little more than a jump table. The compiled
form is a node's own business: the CodeHash commits to the original bytes, and a
cold start and a cache hit produce identical outcomes and counters. The decoder,
validator, interpreter and host are a few thousand lines of safe Rust in Ting's own
repository, with no third-party WebAssembly code, so no dependency upgrade can widen
what is accepted or change what it costs.

## How it is checked

The VM test suite, described in `tests/vm/README.md` and under
[conformance](../reference/conformance.md), is one Rust test binary whose cases
state each program's exact receipt and state, with counters from a model that does
not use the VM. Its cases are the chain's
[seed](../reference/conformance.md#the-seed), the first blocks every node replays.
They cover every accepted instruction family and its edge cases, branches and
loops, direct and indirect calls, bulk memory, instance reset, exact exhaustion,
memory growth, rejected modules, rollback, deep nesting, large records, and
identical results from a cold start and a code-cache hit.

```text
cargo test --package ting-core --test vm --locked
```
