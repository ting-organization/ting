# Proof of work and the heaviest chain

Each header names its parent, so blocks form a tree rooted at genesis. Usually that
tree is a single line. When two miners find blocks at about the same time it
branches, and every node has to choose a branch. Proof of work decides: a block
counts only if its miner did the hashing its target demands, and among fully valid
branches a node follows the one with the most accumulated work, not the most
blocks. The exact rules are in
[section 8 of the protocol rules](../../../ting-protocol-rules.md#8-proof-of-work-and-the-heaviest-chain).

## Valid proof of work

A header's identifier, the BlockId, is the [domain-separated hash](../data/encoding.md)
of its canonical bytes. Read as a big-endian 256-bit number, it must be at most the
header's target, so the lower the target, the more hashes a miner has to try. The
miner changes the nonce, and with it the hash, as often as it likes until one
qualifies. It cannot choose the target, which follows from the chain's history
([time and difficulty](difficulty.md)), and a hash far below the target earns
nothing extra.

## Work, not length

Each block contributes **work** according to its target: the number of hashes a
miner expects to try before one meets the target, 2^256 divided by (target + 1),
rounded down to a whole number. At the production network's initial target of
2^224 − 1, that is 2^32, about 4.3 billion hashes. A block's accumulated work is its parent's plus its
own, starting from zero at genesis.

A node follows the branch with the greatest accumulated work among the branches
that are fully valid and whose data is available. Counting blocks instead would be
wrong whenever two branches have different difficulty.

Headers alone never decide. A branch whose bodies or state transitions have not been
validated cannot become canonical, however much work its headers claim. Neither can
a branch that departs from the network's
[seed](../reference/conformance.md#the-seed): at a seed height, only the listed
block is valid.

## Close races

Which of two branches with nearly equal work to prefer is local policy, not a rule
of the protocol, so two nodes can briefly sit on different tips. The reference node
moves at once to a branch that grows its current tip with more work. Any other
branch must carry more work than the tip it would leave by more than half the work
of the branch's last block; otherwise the node keeps the branch it saw first. A
branch one block longer still wins at once.

The margin matters because a block's target follows its parent's timestamp: of two
branches of one length, the one stamped earlier carries slightly more work. Without
the margin, a miner withholding blocks could win every such race by stamping its
branch a second earlier.

## Reorganizations

When a competing branch overtakes the current one, the node restores the state at the
common ancestor, applies the competing blocks and switches its tip. Transactions and
events on the abandoned branch stop being canonical. The reference node returns the
displaced transactions to its mempool.

Difficulty, capacity history and pending capacity changes are all derived from the
branch itself, so they switch along with it. A node therefore has to be able to
restore the state of any ancestor and replay a competing branch from there; how the
reference node does it is described under [storage](../node/storage.md).

## No fixed finality

No confirmation count makes a block irreversible by rule. Every block mined on top
makes replacing it more expensive, and applications decide how much accumulated work
is enough for what they do.
