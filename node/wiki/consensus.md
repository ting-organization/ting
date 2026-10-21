# How nodes agree

Consensus answers two questions, which every node works out for itself: **is this
block valid**, and **which valid branch should I follow**? A node answers the first
by checking everything a block claims, down to re-running its transactions, and the
second by following the valid branch with the most accumulated proof of work. Proof
of work only counts for blocks that obey every other rule. The exact rules are in
[section 7 of the protocol rules](../../ting-protocol-rules.md#7-how-nodes-agree).

The pages of this section explain each part: the
[genesis configuration](consensus/genesis.md) a network starts from, how
[proof of work](consensus/proof-of-work.md) picks one branch, the
[timestamp and difficulty rules](consensus/difficulty.md), and how miners move
block capacity by [voting](consensus/capacity-voting.md).

## Validating a block

A block is valid when its header fits the chain it extends, its proof of work meets
the required target, and re-executing its transactions from the parent state
reproduces exactly the receipts, state root and resource totals it claims. In
outline, a validator checks that:

- the block extends a valid block of the pinned network
  ([genesis](consensus/genesis.md)), and at a seed height it is the block the
  chain's [seed](reference/conformance.md#the-seed) lists;
- the header is well formed: its height is one above its parent's, its timestamp is
  past the recent median, its target is exactly the one the chain's history
  requires, and its capacity votes are valid ([the header](data/blocks.md),
  [time and difficulty](consensus/difficulty.md),
  [capacity voting](consensus/capacity-voting.md));
- the header's hash meets that target ([proof of work](consensus/proof-of-work.md));
- the body's producer data and transactions match the header's commitments to them,
  and every transaction is well formed;
- running the transactions in order from the parent state gives exactly the
  receipts, the state root and the resource totals the header declares, and those
  totals fit the active capacity ([calls and failure](vm/calls.md),
  [block capacity](resources/capacity.md)).

The complete list is in
[section 7.1](../../ting-protocol-rules.md#71-validating-a-block).

## Rejecting early

A validator may check in any order and reject a block as soon as it finds a fault,
so cheap checks on the header and on sizes come before expensive execution. Because
the header has a fixed size and the active data capacity follows from the block's
ancestry, a node can bound the size of a body before decoding it. A rejection must
still follow from a rule of the protocol: a local timeout, or how fast a compiler
happens to be, never makes a block invalid.

## The state transition

Validation comes down to one deterministic function: from the parent state, the
block's stable context and its transactions in order, it computes the next state,
the receipts and the resources used. The stable context is what a program can see
of its block: the ChainId, the height, the timestamp, the parent BlockId and the
producer data ([context](vm/host.md#context)). It excludes the block's own
identifier, its nonce, its target, its votes, its roots and its totals.

Everything the function needs is either published in the block or present in the
validated parent state. A commitment to bytes does not make them available: the
bytes themselves must be published. The exact definition is in
[section 7.3](../../ting-protocol-rules.md#73-the-state-transition).

## Why only chain data decides

Both answers depend only on data in the chain: the genesis configuration, block
headers and block bodies. A node's clock, its peers or its speed can affect *when*
it learns about a block, but never *whether* that block is valid. Two correct nodes
that have seen the same blocks therefore reach the same verdict on every one of
them.

> [!NOTE]
> **Frozen rules.** The rules this section explains are the ones meant to be
> [frozen](decisions/freeze.md). The capacities themselves can move, but only
> through the voting rule described in
> [capacity voting](consensus/capacity-voting.md).
