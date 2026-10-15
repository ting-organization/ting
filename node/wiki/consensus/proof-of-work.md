# Proof of work and the heaviest chain

Each header names its parent, so blocks form a tree rooted at genesis. Usually that
tree is a single line. When two miners find blocks at about the same time it branches,
and every node has to choose a branch.

## Valid proof of work

A header's identifier, the BlockId, is the [domain-separated hash](/wiki/data/encoding)
of its canonical bytes. Read as a big-endian 256-bit number, it must not exceed the
header's target:

```text
BlockId   = H("block", encode(Header))
valid_pow = integer_big_endian(BlockId) <= target
```

The comparison is inclusive: an identifier equal to the target is valid. The miner
can change the nonce as often as it likes, but cannot choose the target, which is
derived from the chain's history (see [time and difficulty](/wiki/consensus/difficulty)).
A hash far below the target earns nothing extra.

## Work, not length

Each block contributes work according to its target, and a block's accumulated work
is its parent's plus its own. The genesis anchor's accumulated work is zero.

```text
work(target)      = floor(2^256 / (target + 1))
chain_work(block) = chain_work(parent) + work(target)
```

A harder target means more expected hashing, and so more work. A node follows the
branch with the greatest accumulated work among the branches that are fully valid
and whose data is available. Counting blocks instead would be wrong whenever two
branches have different difficulty.

Headers alone never decide. A branch whose bodies or state transitions have not been
validated cannot become canonical, however much work its headers claim. Which of two
branches with nearly equal work to prefer is local policy, so two nodes can briefly
sit on different tips. The reference node switches only to a branch whose work
exceeds its own by more than half the arriving block's work, and otherwise keeps the
branch it saw first. The margin matters because a block's target follows its
parent's timestamp: of two branches of one length, the one stamped earlier carries
slightly more work, and without the margin a miner withholding blocks could win
every such race by stamping its branch a second earlier. A branch one block longer
still wins at once.

## Reorganizations

When a competing branch overtakes the current one, the node restores the state at the
common ancestor, applies the competing blocks and switches its tip. Transactions and
events on the abandoned branch stop being canonical. The reference node returns the
displaced transactions to its mempool.

Difficulty, capacity history and pending capacity changes are all derived from the
branch itself, so they switch along with it. A node must therefore be able to restore
the state of any ancestor and replay a competing branch together with its difficulty
history, its capacity history and its pending capacity changes.

## No fixed finality

No confirmation count makes a block irreversible by rule. Every block mined on top
makes replacing it more expensive, and applications decide how much accumulated work
is enough for what they do.
