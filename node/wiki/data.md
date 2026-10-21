# Blocks and state

A block is a header and a body. The small header carries the proof of work and,
through a few hashes, commits to everything the block contains and everything it
changed. The body carries the transactions and their receipts. The state is what the
chain holds between blocks: every program's code and storage, in one map whose root
hash is in every header. The exact rules are in
[sections 2 to 6 of the protocol rules](../../ting-protocol-rules.md#2-canonical-bytes-and-hashing).

![A header commits to its parent, to its body through two roots and the producer hash, and to the state after the block](../diagrams/block.svg)

The transactions root and the receipts root are each an
[ordered commitment](data/encoding.md#ordered-commitments) to a list in the body.
The producer hash commits to the bytes the miner chose to include, and the state
root is the root of the [state tree](data/state-tree.md) after the block.

The state is a single map from 32-byte keys to byte values. Its root hash in each
header lets two nodes confirm, in 32 bytes, that they hold exactly the same state.
With a [proof](data/state-tree.md#proofs), it also lets a client check one record
against a header without trusting whoever served it.

Two properties run through every format in this section:

- **One encoding per value.** The same logical data produces the same bytes on every
  node. Decoders reject anything else instead of guessing what a sender meant.
- **Commitments are checked, not trusted.** A node recomputes every commitment from
  data it validated itself. A matching header is the result of checking, never a
  reason to skip it.

The pages of this section explain each part: how values become
[canonical bytes and hashes](data/encoding.md), what
[blocks, transactions and receipts](data/blocks.md) contain, how
[contracts and records](data/records.md) are identified, and
[the state tree](data/state-tree.md) behind the state root.

The wire formats and the state model are Ting's own. They use the standard SHA-256
hash function, but are not taken from another project's design.
