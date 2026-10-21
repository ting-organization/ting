# The tests are the first blocks

## Decision

Every Ting chain begins with its conformance suite. Before anything else happens on
it, the chain's first blocks, its **seed**, deploy and call programs over every
instruction and host function, every price step and every outcome a program can
reach, each transaction with the receipt the rules give it. Nodes hold the ids of
these blocks and accept no others at those heights, so following the chain means
replaying the whole suite first.

Together with the [protocol rules](../../../ting-protocol-rules.md), the seed defines
the consensus: the rules say what every input means, and the seed shows, transaction
by transaction, what the network accepted.

## Why

- **Agreement is checked, not assumed.** A frozen protocol is only as good as the
  agreement between the programs that implement it. Elsewhere, a conformance suite is
  something an implementer may or may not run. Here it is part of the chain's
  history: a node that has synced has run it, and passed.
- **Every implementation gets the same fixed test.** A new implementation, written
  from the protocol rules in any language, proves itself the same way the reference
  node does: by syncing the seed. There is no separate certification and no one to
  ask.
- **A wrong node finds out at once.** If a node's own execution ever disagrees with a
  seed block, the fault is the node's: the reference node stops, naming the seed
  block and the first transaction whose receipt differs, instead of quietly
  following a different chain later.
- **The suite cannot drift.** Test files in a repository can be edited, skipped or
  lost. Blocks in a chain cannot: the seed is fixed with the network, and every node
  checks it against the same list.
- **A standard workload.** The seed includes blocks that fill each resource to the
  capacity, so it is also the same benchmark on every machine and in every
  implementation: how long a node takes to validate full blocks of each kind.
- **Nothing to gain from it.** Ting has no native coin, so the seed's blocks reward
  no one, and they come before any application exists on the chain.

## How it is made

The seed is built from the VM tests, which state every rule as cases whose expected
outcomes and charges come from the protocol rules and a separate model of the
prices, never from running the VM. The cases are packed into blocks, mined, and
replayed by a fresh validator to the same blocks and state. A coverage gate fails the
build if any instruction, host-function outcome, status or price step is missing,
and deliberately wrong rules are checked to make a named test fail. The production
chain's seed was cut and mined on its frozen genesis before the chain began, and the
development chain's is built anew from the cases; both are published in the
repository ([conformance and testing](../reference/conformance.md)).

## What it cannot show

- **Inputs it never tries.** A finite suite cannot rule out a rule that differs only
  on an input no case contains, or one that would activate after the seed's last
  block. That is why the protocol rules, not the seed alone, define what every
  input means.
- **Blocks a chain must reject.** A valid chain holds only valid blocks, so malformed
  blocks, broken commitments and over-capacity blocks are covered by portable
  [consensus vectors](../reference/conformance.md#consensus-vectors) instead.
- **Capacity.** Agreement is not speed: how much a block may hold comes from
  measurement and from [capacity voting](../consensus/capacity-voting.md).

## Trade-offs

- Every node replays the seed before it follows the chain, which costs a few seconds
  of validation, once.
- What the seed records is history. If it ever turns out to record a behavior the
  rules did not intend, the accepted behavior stands and the rules are corrected to
  describe it ([freeze consensus once](freeze.md)). The cost moves up front: the seed
  must be complete and reviewed before a network launches.
