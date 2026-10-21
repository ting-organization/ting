# Block capacity and bounds

Each block has a capacity for each of the four counters, and the block's totals must
fit all four. The capacities decide how much work a block may ask of every node, and
so how quickly a modest machine can keep up with the chain. Apart from them, Ting
fixes almost no limits: everything a program does is bounded by what it pays. This
page explains the block totals, where the capacity comes from, which bounds follow
from the counters, and the few fixed bounds. The exact rules are in
[section 14 of the protocol rules](../../../ting-protocol-rules.md#14-block-capacity-and-bounds).

## Block totals

A block's C, S and G are the sums of what its transactions actually used. Its D also
includes the header, the producer data and the transaction count, so for a block
whose only output is its transactions' top-level results and the events their
receipts keep, D equals the block's encoded size in bytes; any other output makes it
larger ([the block's own bytes](counters.md#the-blocks-own-bytes)).

```text
block_used[C, S, G] = sum(transaction_used[C, S, G])
block_used[D]       = 302 + len(producer_data) + sum(transaction_used[D])
```

Each transaction's used vector is what its receipt records, including everything
consumed before a failure. Each of the four totals must be at most the active
capacity at that height and must equal the resources the header declares.

A block that exceeds a capacity is invalid as a whole. It does not change any
transaction's outcome to fit the remaining space, and since counters are never
restored by rollback, a revert cannot bring a total back under the limit. A
validator may stop executing such a block early, as
[block limits during execution](../../../ting-protocol-rules.md#216-block-limits-during-execution)
allows, but only to reject it.

## Declared versus actual

A transaction's declared budget may exceed the current block capacity, because only
what it actually uses has to fit. C, S and G may be any `u64`; D must also lie
between the envelope's own data cost and `2^32 − 1`
([data](counters.md#d-data)). A transaction can
therefore carry generous headroom without its author knowing how full the block will
be, and simulating a transaction needs no assumption about how much block space
remains.

## Active capacity

Capacity starts at the genesis initial values and moves only through
[capacity voting](../consensus/capacity-voting.md), upward without limit and downward
to the genesis floor. Every node derives it from the chain alone, never from a local
setting. The production values are on [genesis](../consensus/genesis.md), and your
node's current capacities appear on the explorer's overview page and in `/v1/status`.

## Bounds that follow from the counters

Nothing a program does is capped by a fixed per-transaction number. Everything that
could be capped follows from the budgets instead:

| Quantity | Bounded by |
|---|---|
| Live guest memory | Compute: every 64 KiB page costs 40,000 C and is never refunded, about 1.6 bytes per C |
| Local stack | Compute: every function entry pays its parameters, locals and operand slots |
| Nesting depth | State access: every nested invocation reads two records before it runs |
| Record size | The memory that holds the value, and the compute to copy and hash it |
| Module size | 2^30 bytes, and the [code price](prices.md#code), about 60 C per byte, paid to validate it |
| Pending output and state journals | D, S and G: every byte and record they hold was charged |

A node therefore knows, from the active capacities alone, how much memory a block
can need. It may store journals however it likes, but local memory pressure can
never turn into a different consensus result.

## Fixed bounds

There are only two kinds of fixed bound: the 32-bit address space of a single
instance, and the shape of a module. The shape bounds exist so that validating a
module takes work, and validator memory, proportional to its bytes. They are set far
above what compilers produce, and none of them is a capacity.

| Item | At most |
|---|---:|
| Linear memory of one instance | 65,536 pages of 64 KiB (4 GiB) |
| Module | 2^30 bytes |
| Parameters of one function type | 1,000 |
| Parameters plus declared locals of one function | 16,384 |
| Reserved slots of one function, including its maximum operand height | 32,768 |
| Functions per module, imports included | 1,000,000 |
| Types per module | 1,000,000 |
| Globals per module | 1,000,000 |
| Data segments per module | 100,000 |
| Element segments per module | 100,000 |
| Table entries | The module's function count |

How a module is checked against them is on [What a module may contain](../vm/profile.md). The
reasoning is in [Prices, not caps](../decisions/prices-not-caps.md).
