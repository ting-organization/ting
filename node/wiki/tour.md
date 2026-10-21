# A tour of Ting

This page follows one transaction from the moment it is written until every node
agrees on its result. Each step links to a page that explains it in depth. You
don't need a running node to read it; [Deploy a first program](start/first-program.md)
repeats the same steps on yours.

![One transaction's way to agreement: written, held, mined, checked and settled](../diagrams/journey.svg)

## A program and a transaction

Programs are WebAssembly modules. This one is complete: when it runs, it stores
the byte `2a` under an all-zero key in its own storage.

```wat
(module
  (import "ting" "storage_set" (func (param i32 i32 i32)))
  (memory (export "memory") 1 1)
  (data (i32.const 32) "\2a")
  (func (export "invoke")
    i32.const 0    ;; key: 32 bytes at address 0, still zero
    i32.const 32   ;; value: the byte placed at address 32
    i32.const 1    ;; value length
    call 0))
```

To install it, someone writes a **Deploy** transaction. It holds:

- the module bytes;
- input for the program's first run;
- a **budget**: the most compute, state access, data and growth the transaction
  may use.

The other kind of transaction is a **Call**, which runs an installed program with
new input.

The transaction's identifier, its **TransactionId**, is a hash of the network's
ChainId and the encoded transaction. Notice what is missing: there is no sender,
signature or fee. Consensus doesn't care who submitted a transaction. Programs that
need authorization check it themselves.

Go deeper: [Blocks, transactions and receipts](data/blocks.md) ·
[Authentication and replay](applications/authentication.md)

## A node holds it for later

The transaction reaches a node through `ting submit` or `POST /v1/transactions`.
The node checks that it is well formed: canonical bytes, a known kind, and enough
data budget to cover the transaction itself. Then the transaction waits in the
mempool.

Nothing has executed yet. Admission is **node policy**, not consensus: each node
decides what to hold and relay, and a valid block can contain a transaction that
some node would have refused.

Go deeper: [Mining and voting](node/mining.md)

## A miner executes it

A miner picks pending transactions and executes them in order against its current
state, exactly as every validator will later. For this deployment the VM:

1. derives the new program's **ContractId** from the ChainId, the TransactionId
   and a per-transaction counter;
2. charges compute for every code byte and checks the module against the
   [execution profile](vm/profile.md);
3. installs the code as immutable records;
4. runs the exported `invoke` function, which writes `2a` into the contract's storage.

Every step is counted. Executed instructions and copied bytes cost **compute** (C).
Reading and writing records costs **state access** (S). The transaction and its
output cost **data** (D). New records cost **growth** (G). If any counter would
exceed the budget, execution stops and everything the transaction changed is rolled
back. The resources used up to that point stay used.

The outcome is a **receipt**: a status, the four amounts used, result bytes (here,
the new ContractId) and any events the program emitted.

Go deeper: [Deploy and invoke](vm/lifecycle.md) ·
[What each counter measures](resources/counters.md)

## The header seals the block

The miner builds a header containing:

- the parent block's identifier, the height, a timestamp and the required target;
- three commitments: to the ordered transactions, to their receipts and to the
  entire resulting state;
- the block's total resource use;
- the miner's capacity votes and a commitment to its producer data.

Then it searches for a nonce that makes the header's SHA-256 hash small enough.
Nothing a program can see depends on that nonce or on the final hash, so execution
happens once, before mining starts.

Go deeper: [Canonical bytes and hashing](data/encoding.md) ·
[The state tree](data/state-tree.md)

## Every node checks everything

A node that receives the block trusts none of it. It checks the header against the
chain, verifies the proof of work, then executes every transaction itself, starting
from the parent state. It must arrive at the same receipts, the same state root and
the same resource totals, byte for byte. Any difference makes the whole block invalid.

This is why the VM, the resource charges and the encodings are specified so exactly:
independent implementations have to agree on every counter, not only on the final result.

Go deeper: [Exact metering](vm/metering.md) ·
[Conformance and testing](reference/conformance.md)

## The heaviest valid chain wins

Two miners can find blocks at nearly the same time, creating competing branches.
Each node follows the fully validated branch with the most accumulated proof of
work. If another branch overtakes it, the node rolls its state back to the common
ancestor and applies the other branch.

A transaction becomes more settled as blocks pile up on top of it, but no rule
makes it irreversible at a fixed point. Applications choose how many confirmations
they wait for.

Go deeper: [Proof of work and the heaviest chain](consensus/proof-of-work.md)

## What you have seen

| In this walk-through | Layer |
|---|---|
| Transaction format, ContractId, execution, receipts, commitments, proof of work, chain selection | Consensus |
| How much a block may use | Capacity |
| Mempool admission, which transactions a miner picks | Node software |
| What the program stores, and who may call it | Application |

Try it yourself: [Start a node](start/node.md), then
[deploy this program](start/first-program.md).
