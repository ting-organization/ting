# The state tree

The state root in each header commits to every record. It is the root of a binary
**compact sparse Merkle tree** over the records' keys: the tree splits only as far as
it must to separate records, and a part of the tree that holds a single record is
just that record's leaf.

## Shape

A record key's bits, most significant first, choose a path from the root: left for 0,
right for 1. Every subtree is one of three things:

- **empty**, when no record's key follows its path;
- **a leaf**, when exactly one record's key does;
- **a node**, when two or more do. Its two children split them by the next bit.

A record therefore sits at the shallowest depth where no other record shares its
path. Record keys are hashes, so that is about log₂ of the number of records deep:
17 levels for 100,000 records, 30 for a billion. Two keys that differ only in their
last bit put their leaves at depth 256, the deepest a tree can be.

A record's position depends only on its key and on which other keys exist, so the
root depends only on the set of records, never on the order in which they were
written.

## Hashes

Bits are counted from the most significant bit of the key's first byte, which is bit 0.
For a set `R` of records whose keys agree on their first `d` bits:

```text
hash(R, d) = EMPTY                                       when R is empty
           = H("state-leaf", RecordKey || Bytes(value))  when R holds one record
           = H("state-node", u16(d) || hash(R0, d + 1) || hash(R1, d + 1))
                                                         otherwise
             where R0 and R1 are the records of R whose key bit d is 0 and 1

EMPTY      = H("state-empty", "")
state_root = hash(all records, 0)
```

`H` is the [domain-separated hash](/wiki/data/encoding#domain-separated-hashing),
`RecordKey` is the record's [key](/wiki/data/records#identifiers), and `value` is its
byte string, possibly empty. An empty state has the root `EMPTY`; a state with one
record has that record's leaf as its root. Keys are distinct, so any two records
differ in some bit and every node has a depth `d` of at most 255.

The internal hashes are derived from the records. They are not records themselves and
are not part of the state map.

## Changing a record

Only one path changes when a record does:

- **Writing an existing record** replaces its leaf.
- **Writing a new record** takes the empty subtree where its path ends. If its path
  instead ends at another record's leaf, both move down to the first bit where their
  keys differ, and nodes fill the levels between, each with one empty side.
- **Deleting a record** empties its place. If its parent's other child is then a
  single leaf, that record moves up, and keeps moving up for as long as it is alone.

Then the nodes on the path are hashed again up to the root: about log₂ of the
number of records of them, and never more than 256.

## Why this shape

- **Bounded work per record.** A change touches one path, of at most 256 nodes and
  usually far fewer. Keys are hashes, so making two keys share their first `b` bits
  takes about 2^b attempts: a deep path is expensive to build and still bounded. A
  record operation is one random access and one path, which is exactly what the
  state-access counter counts. Hashing the value itself into its leaf is byte work,
  charged as compute when the record is written.
- **Incremental updates.** Changing one record recomputes one path, not the whole state.
- **Short proofs.** A proof carries one hash per level of the record's path.
- **Freedom underneath.** How a node stores the tree, whether it caches, batches
  writes or uses another database, is its own business, as long as the root comes
  out the same. None of it is consensus-visible: a cache miss or a physical disk read
  never costs any resource. See [what each counter measures](/wiki/resources/counters).

## Proofs

A proof that a record is present gives its value and the hashes of the other child at
each level of its path, from its leaf up to the root. The verifier hashes the leaf,
combines it with each of those hashes in the order the key's bits say, and compares
the result with the state root. A proof that a record is absent ends the key's path
at an empty subtree, or at the leaf of a different record whose key shares that path.

The reference node's storage reads give a record's proof when asked
([the REST API](/wiki/start/explore#the-rest-api)), beside its value and the state
root and canonical tip it was read at:

| Field | Content |
|---|---|
| `record_key` | The record's key, which the verifier computes for itself from the contract and the storage key |
| `end` | Where the key's path ends: `present`, the record's own leaf; `empty`, an empty subtree; `other`, another record's leaf |
| `other_key`, `other_leaf` | For `other`: that record's key and its leaf hash |
| `siblings` | The other child's hash at each level of the path, from its end up to the root |

With `n` siblings the path ends at depth `n`. The verifier starts from the leaf of
the record and its value for `present`, from `EMPTY` for `empty`, and from
`other_leaf` for `other`, after checking that `other_key` differs from the key and
shares its first `n` bits. Then, for `d` from `n − 1` down to 0, it combines the hash
so far with the next sibling as the node at depth `d`, the hash so far on the side
the key's bit `d` names, and compares the result with the state root. A proof checks
only against a root the client trusts: one in a header it has checked, on the chain
with the most work.
