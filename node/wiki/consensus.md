# How nodes agree

Consensus answers two questions, which every node works out for itself: **is this
block valid**, and **which valid branch should I follow**? Proof of work only counts
for blocks that obey every other rule.

A block is valid when its header fits the chain it extends, its proof of work meets
the required target, and re-executing its transactions from the parent state
reproduces exactly the receipts, state root and resource totals it claims. Among all
valid branches, a node follows the one with the most accumulated work.

Both answers depend only on data in the chain: the genesis configuration, block
headers and block bodies. A node's clock, its peers or its speed can affect *when*
it learns about a block, but never *whether* that block is valid.

## Validating a block

To accept a block, a validator must establish all of the following:

1. The parent belongs to the pinned network ([genesis](/wiki/consensus/genesis)) and
   has a valid, available ancestry.
2. The header's fields, height, timestamp, target and vote values are valid
   ([the header](/wiki/data/blocks), [time and difficulty](/wiki/consensus/difficulty),
   [capacity voting](/wiki/consensus/capacity-voting)).
3. The header's [proof of work](/wiki/consensus/proof-of-work) is valid.
4. The body has canonical framing, its producer data matches the header's commitment
   to it, and its transactions match the transactions root.
5. Every transaction envelope satisfies the static rules and decodes within its
   bounds.
6. Executing the transactions in order from the parent state produces exactly the
   supplied receipts ([calls and failure](/wiki/vm/calls)).
7. The receipts root and the final state root match the header.
8. The computed resource totals match the header and fit every active capacity
   ([block capacity](/wiki/resources/capacity)).

## Rejecting early

Cheap bounds and header checks should come before expensive execution, and a
validator may reject an invalid block as soon as it finds the fault. Such a rejection
must follow from an objective rule of the protocol. A local simulation timeout, or
how fast a compiler happens to be, can never establish that a block is invalid.

Because the header has a fixed size and the active data capacity follows from the
block's ancestry, a validator can bound the size of a body before decoding it.

## The state transition

Validation comes down to one deterministic function:

```text
Execute(parent_state, stable_block_context, ordered_transactions)
    -> (next_state, ordered_receipts, consumed_resources)
```

The stable block context excludes the block's own identifier and its nonce. Every
payload, code, receipt and event byte the transition needs is either published in the
body or present in the validated parent state. A commitment to bytes does not make
them available: the bytes themselves must be published.

> [!NOTE]
> **Consensus layer.** The rules in this section are the ones meant to be frozen.
> The capacities themselves can move, but only through the voting rule described here.
