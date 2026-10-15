# Canonical bytes and hashing

Consensus hashes bytes, so every value needs exactly one byte representation.
Ting's encoding is deliberately plain.

## Encoding rules

- Integers have fixed widths and are little-endian. The exception is the 256-bit
  mining target, which is big-endian.
- Hashes and identifiers are opaque 32-byte strings.
- Byte strings carry a four-byte length prefix.
- Fields appear in a fixed order. There are no optional fields, alternative forms
  or extensions.

A decoder rejects unknown versions, unknown kinds, unknown fields, invalid lengths,
noncanonical representations, arithmetic overflow and trailing bytes. Lengths are
checked against the enclosing buffer and the active capacity before anything is
allocated, so a hostile peer cannot make a node reserve memory by sending a large
length field.

## Types

Every layout in the wiki is built from these types. Its fields are encoded in the
order listed, back to back, with nothing between them.

| Type | Bytes | Encoding |
|---|---:|---|
| `u8`, `u16`, `u32`, `u64`, `u128` | 1, 2, 4, 8, 16 | Unsigned integer of exactly that width, little-endian |
| `u256be` | 32 | Unsigned integer, big-endian; used for mining targets |
| `bytes32` | 32 | Opaque string: a hash or an identifier |
| `Bytes` | 4 + length | A `u32` length, then exactly that many bytes |
| `ResourceVector` | 32 | Four `u64` values in the order C, S, D, G |
| `T[n]` | n × item | n items of type `T`; n is fixed by the layout or given by an earlier count field |

```text
Bytes(x) = u32(len(x)) || x
```

`||` is concatenation, and `u16(n)`, `u32(n)` and `u64(n)` are the integer `n`
encoded at that width. `encode(v)` is the canonical encoding of a value `v`.

A `Bytes` item is at most `2^32 − 1` bytes long, and a vector has fewer than `2^32`
entries. These limits are the widths of the length and count fields, chosen
deliberately: no transaction, receipt, event or producer data needs more, and a block
is bounded by the active data capacity rather than by its encoding. See
[block capacity and bounds](/wiki/resources/capacity).

## Arithmetic

Consensus formulas are evaluated over mathematical integers. Nothing wraps around
and nothing is rounded or truncated unless the rule itself says so, for example
with an explicit `floor`. A result that does not fit the field it must be stored in
is invalid, never wrapped into it.

## Domain-separated hashing

Every hash in the protocol is SHA-256 over a domain tag and the data:

```text
H(tag, x) = SHA256(ASCII("Ting/") || u16(len(tag)) || ASCII(tag) || x)
```

The tag names what is being hashed: `block`, `transaction`, `contract`, `state-leaf`
and so on. A value hashed for one purpose can never be mistaken for one hashed for
another purpose, even when the bytes are equal.

- `len(tag)` is the tag's length in bytes, as a little-endian `u16`.
- Tags are literal, case-sensitive ASCII strings, written exactly as on these pages.
- When `x` is made of several fields, each one is either fixed-width or framed as
  `Bytes`, so there is only one way to split the input back into its fields.
- SHA-256 is the hash function standardized by NIST
  ([NIST hash functions](https://csrc.nist.gov/projects/hash-functions)). The
  function itself is fixed; later guidance does not change consensus.

`H` is the protocol's own commitment function. It is not offered to programs as a
host function: a program that needs a hash computes it in its own code, as described
in [Cryptography lives in programs](/wiki/decisions/crypto-in-programs).

The tags used by consensus:

| Tag | Hashes | Produces | Rules |
|---|---|---|---|
| `chain` | The genesis configuration | ChainId | [Genesis](/wiki/consensus/genesis) |
| `genesis` | The ChainId | Identifier of the genesis anchor | [Genesis](/wiki/consensus/genesis) |
| `block` | The header | BlockId | [Proof of work](/wiki/consensus/proof-of-work) |
| `producer` | The producer data | The header's producer hash | [Blocks](/wiki/data/blocks#the-header) |
| `transaction` | ChainId and the transaction | TransactionId | [Blocks](/wiki/data/blocks#transactions) |
| `contract` | ChainId, TransactionId and ordinal | ContractId | [Records](/wiki/data/records#identifiers) |
| `code` | A module's bytes | CodeHash | [Records](/wiki/data/records#identifiers) |
| `state-key` | Namespace, ContractId and logical key | RecordKey | [Records](/wiki/data/records#identifiers) |
| `state-leaf`, `state-node`, `state-empty` | Records and subtrees | State tree hashes | [State tree](/wiki/data/state-tree#hashes) |
| `transactions-leaf`, `-pad`, `-node`, `-root` | A block's transactions | Transactions root | [Below](#ordered-commitments) |
| `receipts-leaf`, `-pad`, `-node`, `-root` | A block's receipts | Receipts root | [Below](#ordered-commitments) |

## Ordered commitments

A block commits to its transactions, and to its receipts, as ordered lists. Each item
becomes a leaf that includes its position. Leaves are padded to a power of two and
combined in pairs, and the final root also includes the number of items. Changing,
moving, adding or removing an item changes the root.

Because the position is part of the leaf, the same item at another position has a
different leaf hash.

`VectorRoot(tag, items)` commits to the order, the number and the exact bytes of the
items. In the formulas, `tag + "-leaf"` is the tag with the suffix appended, such as
`transactions-leaf`.

```text
count = number of items

if count == 0:
    VectorRoot = H(tag + "-root", u32(0))

otherwise:
    width   = smallest power of two >= count
    leaf[i] = H(tag + "-leaf", u64(i) || Bytes(items[i]))     for 0 <= i < count
    leaf[i] = H(tag + "-pad",  u64(i))                        for count <= i < width
    parent  = H(tag + "-node", left || right)
    VectorRoot = H(tag + "-root", u32(count) || tree_root)
```

Each level pairs neighbouring hashes, positions `2k` and `2k + 1`, into the parent
on the next level up, until one hash remains: `tree_root`. With a single item,
`tree_root` is that item's leaf.

| Commitment | Tag | Items, in body order |
|---|---|---|
| Transactions root | `transactions` | The canonical encoding of each transaction |
| Receipts root | `receipts` | The canonical encoding of each receipt |

A committed list has fewer than `2^32` items, and the block's capacity bounds it
further.
