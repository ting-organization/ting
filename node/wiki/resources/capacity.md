# Block capacity and bounds

## Block totals

A block's C, S and G are the sums of what its transactions actually used. Its D also
includes the header, the producer data and the transaction count, so for a block
whose only output is its transactions' top-level results and the events their
receipts keep, D equals the block's encoded size in bytes; any other output makes it
larger ([counters](/wiki/resources/counters#the-block-s-own-bytes)). All four totals
must fit the active capacity at that height.

```text
block_used[C, S, G] = sum(transaction_used[C, S, G])
block_used[D]       = 302 + len(producer_data) + sum(transaction_used[D])
```

Each transaction's used vector is what its receipt records, including everything
consumed before a failure; the 302 bytes are broken down under
[The block's own bytes](/wiki/resources/counters#the-block-s-own-bytes). Each of the
four totals must be at most the active capacity, derived independently from the
block's ancestry, and must equal the resources the header declares.

A block that exceeds a capacity is invalid as a whole. It does not change any
transaction's outcome to fit the remaining space, and since counters are never
restored by rollback, a revert cannot bring a total back under the limit. A
validator may stop executing as soon as a debit would push the block over a limit,
checking the transaction's own budgets first, so a debit the transaction could not
afford never counts toward the block. That early stop is only a way of rejecting the
block; it never produces an alternative receipt.

## Declared versus actual

A transaction's declared budget may exceed the current block capacity, because only
what it actually uses has to fit. Any value is a valid declaration. A transaction can
therefore carry generous headroom without its author knowing how full the block will
be, and simulating a transaction needs no assumption about how much block space
remains.

## Active capacity

Capacity starts at the genesis initial values and moves only through
[capacity voting](/wiki/consensus/capacity-voting), upward without limit and downward
to the genesis floor. Your node's current capacities appear on the explorer's overview
page and in `/v1/status`.

## Bounds that follow from the counters

Nothing a program does is capped by a fixed per-transaction number. Everything that
could be capped follows from the budgets instead:

| Quantity | Bounded by |
|---|---|
| Live guest memory | Compute: every 64 KiB page costs 40,000 C and is never refunded, about 1.6 bytes per C |
| Local stack | Compute: every function entry pays its parameters, locals and operand slots |
| Nesting depth | State access: every nested invocation reads two records before it runs |
| Record size | The memory that holds the value, and the compute to copy and hash it |
| Module size | 2^30 bytes, and the [code price](/wiki/resources/prices#code), about 60 C per byte, paid to validate it |
| Pending output and state journals | D, S and G: every byte and record they hold was charged |

A node therefore knows, from the active capacities alone, how much memory a block
can need. It may store journals however it likes, but local memory pressure can
never turn into a different consensus result.

## Fixed bounds

Only two kinds of fixed bound remain: the 32-bit address space of a single instance,
and the shape of a module, which exists so that validating a module takes work, and
validator memory, proportional to its bytes. The shape bounds are set far above what
compilers produce, and none of them is a capacity.

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

How a module is checked against them is on [Validation](/wiki/vm/validation). The
reasoning is in [Prices, not caps](/wiki/decisions/prices-not-caps).
