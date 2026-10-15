# Blocks and state

A block is a header and a body. The small header carries the proof of work and,
through three hashes, commits to everything the block contains and everything it
changed. The body carries the transactions and their receipts.

The three hashes are the transactions root and the receipts root, each an
[ordered commitment](/wiki/data/encoding#ordered-commitments) to a list in the body,
and the state root, the root of the [state tree](/wiki/data/state-tree) after the
block. The exact layouts are on [blocks, transactions and receipts](/wiki/data/blocks).

Two properties run through every format in this section:

- **One encoding per value.** The same logical data produces the same bytes on every
  node. Decoders reject anything else instead of guessing what a sender meant.
- **Commitments are checked, not trusted.** A node recomputes every commitment from
  data it validated itself. A matching header is the result of checking, never a
  reason to skip it.

The state is a single map from 32-byte keys to byte values of any length, covering
every contract's code and storage. Its root hash in each header lets two nodes confirm, in
32 bytes, that they hold exactly the same state.

The wire formats and the state model are Ting's own. They use the standard SHA-256
hash function, but are not taken from another project's design.
