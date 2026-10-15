# Exact metering

Every node has to reach exactly the same counters, so Ting defines precisely
when each unit is charged, not just roughly how expensive an instruction is.
This page fixes the compute (C) charges of running a program and the exact point
of each debit; [what each counter measures](/wiki/resources/counters) lists every
counted event across C, S, D and G.

## Prices

Every amount on this page is a price from [the price list](/wiki/resources/prices):
each instruction's price, the load and store prices and the byte price of the
executing instance's memory tier, the page price, the hashing prices, the start
price and the code price. `bytes(n)` is the byte price of `n` bytes at the current
tier. A debit of zero is permitted and has no effect.

## Where each charge is made

| Event | C | When |
|---|---:|---|
| Root dispatch of a transaction | 1 | once, before any top-level call or deployment work, whatever the program |
| Validating deployment code | the code price and the CodeHash | before validation and before any cache lookup |
| Loading code for an invocation | the code price, unless the transaction has prepared this CodeHash | after reading the manifest and the code record, even on a cache hit |
| Preparing an instance | see below | in one debit, before the instance exists |
| Entering `invoke` | its reservation | after preparation, before the first instruction |
| Dispatching an original instruction | its price | immediately before it executes |
| Entering a defined function by `call` or `call_indirect` | its reservation | after the call's dispatch and, for an indirect call, its table and type checks |
| Reaching a host import through `call_indirect` | 94 | after the indirect call's dispatch and its table and type checks |
| `memory.copy`, `memory.fill` of `n` bytes | `bytes(n)` | after dispatch, before the bounds check |
| `memory.grow` adding `p` pages | 40,000 × `p` | after dispatch and the maximum check, before growing |
| Host copies, record reads and writes, results, events | per [the host interface](/wiki/vm/host) | at the steps listed there |
| Producing a deployed ContractId | 32 | after the initializer succeeds, in one debit with 32 D |

Instance preparation is one atomic debit covering the start of the instance, its
initial memory, its globals and table, and copying the data segments, with nothing
extra for the `const` and `end` of initializers:

```text
C = 40,000 + 40,000 × initial_pages + 40 × global_count + 4 × table_size
  + 12 × active_data_segments + bytes(sum of the lengths of all active data segments)
```

The data bytes are priced at the tier of the initial memory.

## The price of an executed instruction

An instruction's price is charged immediately before it executes. Skipped
instructions cost nothing, and `else` and `end` are structural delimiters that
cost nothing, so the function's final `end` and its implicit return are free.
Entering a `block`, `loop` or `if` through its instruction costs its price;
branching back to a loop continues at its body without paying for the `loop`
again, while the branch instruction and every repeated instruction of the body are
charged. A load or store is priced by the tier of the memory's size at that moment.
A `call` is priced by its target: a defined function or a host import.

The charge comes before the instruction can trap. If the price does not fit, the
instruction does not execute, nothing of its price is charged, and the invocation
fails with status 3. `memory.copy` and `memory.fill` charge their price, then
`bytes(n)` for the `n` bytes their length operand names in one atomic debit, and
only then execute: an out-of-bounds range traps after both debits, and running out
of compute at the byte debit fails the invocation before any byte is written.

## Function entry

Entering a defined function reserves and charges one C per slot it can use, and
the [stack](/wiki/resources/prices#stack) it adds beyond the invocation's mark:

```text
reservation = parameters + declared_locals + maximum_typed_operand_height
height      = height before entry + reservation + 3
debit       = reservation + 40,000 × (stack_pages(max(height, mark)) − stack_pages(mark))
mark        = max(height, mark)
```

The 3 slots are the record every active function keeps. The invocation's height
and mark start at zero; entering its entry function is the first entry, so the
entry function's reservation and stack pages are debited the same way, at the
point the table above gives. Leaving a function lowers the height by its
reservation plus 3 and leaves the mark.

The maximum operand height is the validator's, unreachable regions included
([validation](/wiki/vm/validation#measured-for-execution)). Reservations, heights
and marks are static per-function maxima and their sums, not bytes of any engine's
stack. The reservation and the stack pages are one atomic debit, the same whether
the call is direct or indirect, and it comes after the
call's own price and, for an indirect call, after the table and type checks, so a
bad indirect call pays its price only. Calling an imported host function reserves
nothing; it pays the `call`'s price for an import and then its host charges. An
indirect call that reaches an import pays the indirect call's price, then, after
the table and type checks, 94 C for the host call, then its host charges.

There is no cap on frames, slots or nesting: each level costs its call, its slots
and the stack pages it adds, so the stack a transaction can build is bounded by its
compute budget as its memory is, and a node knows from the block's capacity how much
stack a block can need.

## Memory growth

`memory.grow` pays its price first. If the module's maximum forbids the growth, it
returns −1, as WebAssembly specifies, and nothing more is charged. Otherwise it
charges 40,000 C per new page and then grows and zeroes them; running out of compute
at that point fails the invocation with status 3 rather than returning −1. Growing
by zero pages has no page debit. There is no separate memory cap, per instance or
across them: suspended parents and the running child have each paid for their
memory in compute, so a program can grow as far as its module declares and its
budget pays for. At 40,000 C per 64 KiB page, of memory or of stack, a
transaction's live guest memory and stack together never exceed about 1.6 bytes per
unit of its C budget, beyond the 64 KiB of stack each invocation's start price
covers. An implementation that keeps
its own copy of each nested call's input or code, as the reference node does, holds
at most as much again, since every such copy comes from a caller's paid memory.

## How the reference does it

The reference interpreter charges inline. Before each original instruction it
takes the instruction's price from a local allowance equal to the largest compute
debit the executing frames and the block guard would accept; when the price does not
fit the allowance, it debits the price through the meter instead, which fails as
the rules above say. It settles what it has taken with the meter before any other
debit, before any host call and when it stops, so nothing can observe a difference
from charging each instruction through the meter. It keeps each invocation's
height and mark beside its stack. It keeps the load, store and byte
prices of the current tier with the instance and changes them when memory grows.
Function entries, memory pages and bulk bytes are single atomic debits at the
points described above; an indirect call checks the table and the type, then debits
the callee's reservation and enters it. No module is rewritten and no code is
inserted.

Other implementations can count differently, as long as outcomes and counters
agree, including the exact point at which a budget runs out. An engine may
instrument modules with private debit calls and thunks, as the test oracle does.
A compiled implementation may charge a straight-line run of instructions at once,
provided it finds the instruction whose price no longer fits exactly as charging
one at a time would, and leaves the same counters. Interpreter fuel tables,
intermediate instructions, timing, timeouts and cache state never define C, and
inserted or compiled code is never part of the deployed CodeHash.

## Block limits during execution

A validator may stop executing as soon as an otherwise permitted debit would push
the block's totals past an active capacity ([block totals](/wiki/resources/capacity#block-totals)).
The transaction's own budget and its frames are checked first, so a debit the
transaction could not afford never counts toward the block. Stopping is final: it
rejects the whole block, and no cancellation receipt or adjusted transaction
outcome can be accepted in its place. A revert cannot make an exceeded total fit,
because the counters only grow. Executing a transaction on its own, and every
accepted block, keep the transaction-local behaviour described on these pages.
