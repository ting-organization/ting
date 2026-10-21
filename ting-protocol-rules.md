# Ting protocol rules

This document states every consensus rule of Ting in one place: the canonical
formats, the state transition, the virtual machine, every price, the order in which
things fail, block validity and the rules derived from a branch. It is written for
implementers. Together with a network's genesis configuration and its
[seed blocks](node/wiki/reference/conformance.md#the-seed), it defines the consensus
completely and is everything needed to build a node that agrees with every other: no
Rust code, no wiki page, no WebAssembly engine and no separate WebAssembly
specification. Even SHA-256 is defined, in the last section.

Networking, databases, APIs, mining strategy, mempool admission and application
cryptography are outside the consensus transition and outside this document. The
wiki's pages under [Running a node](node/wiki/node.md) describe the reference node's
choices.

This document and a network's seed blocks define the consensus; nothing else
does. The [wiki](node/wiki/index.md), which every node serves at `/wiki` with this
document among its pages, explains the rules topic by topic, with the reasons
behind them and examples, and defers to this document: where the two differ, this
document decides and the wiki is corrected. Beyond the rules themselves, this
document gives two algorithms an implementer needs in full, type checking
([section 25](#25-complete-type-checking-algorithm)) and exact execution
([section 26](#26-exact-execution-algorithms-and-boundary-clarifications)), and
reproduces the configurations built into the reference node
([section 28](#28-configurations-and-check-values)).

| Sections | Explained on |
|---|---|
| 2 to 6 | [Blocks and state](node/wiki/data.md) |
| 7 to 10 | [How nodes agree](node/wiki/consensus.md) |
| 11 to 15 | [Four resources](node/wiki/resources.md) |
| 16 to 24 | [Programs and the VM](node/wiki/vm.md) |
| 27 | [Conformance and testing](node/wiki/reference/conformance.md) |

## 1. Authority and network binding

In this document, **must** is a consensus requirement and **may** permits equivalent
implementation strategies. Statements labeled local policy or implementation advice
do not change block validity.

A network is bound by three things, published together:

| Binding | What it is |
|---|---|
| The genesis configuration | Its exact 150 bytes, which hash to the ChainId and the GenesisBlockId ([section 3](#3-genesis-and-network-identity)) |
| The seed list | The BlockId at every seed height, 1 to N ([section 27](#271-the-seed-list)) |
| The seed bundle | The seed blocks themselves, in order, each framed by its length ([section 27](#271-the-seed-list)) |

The production and development networks' configurations, lists and bundles are
published in the repository and built into the reference node
([conformance](node/wiki/reference/conformance.md#the-seed)). The configurations built
into the reference node are reproduced in
[section 28](#28-configurations-and-check-values).

Genesis time is a consensus input: it changes the ChainId, and with it every
TransactionId, ContractId and seed commitment. Header hashes cannot be inverted to
recover genesis parameters, so an implementation needs the configuration itself,
not only its ChainId.

The seed is executable evidence for the cases it contains, and its bytes are what
the network accepted: an implementation must reproduce them, and a disagreement
between the seed and a reading of these rules is a conformance fault to
investigate, never permission to rewrite the history. The rules also define every
input the seed does not contain, including malformed blocks, invalid envelopes,
arbitrary future programs and capacities larger than the launch capacity. Passing a
finite seed alone does not prove correctness for all inputs, and a finite example
never defines an unstated general exception. Later engine defaults, dependency
versions, benchmarks or edits to external standards never change these rules.

## 2. Canonical bytes and hashing

Consensus hashes bytes, so every value needs exactly one byte representation.
Ting's encoding is deliberately plain.

### 2.1. Encoding rules

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

### 2.2. Types

Every layout in this document is built from these types. Its fields are encoded in the
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
[block capacity and bounds](#14-block-capacity-and-bounds).

### 2.3. Arithmetic

Consensus formulas are evaluated over mathematical integers. Nothing wraps around
and nothing is rounded or truncated unless the rule itself says so, for example
with an explicit `floor`. A result that does not fit the field it must be stored in
is invalid, never wrapped into it.

### 2.4. Domain-separated hashing

Every hash in the protocol is SHA-256 over a domain tag and the data:

```text
H(tag, x) = SHA256(ASCII("Ting/") || u16(len(tag)) || ASCII(tag) || x)
```

The tag names what is being hashed: `block`, `transaction`, `contract`, `state-leaf`
and so on. A value hashed for one purpose can never be mistaken for one hashed for
another purpose, even when the bytes are equal.

- `len(tag)` is the tag's length in bytes, as a little-endian `u16`.
- Tags are literal, case-sensitive ASCII strings, written exactly as in this document.
- When `x` is made of several fields, each one is either fixed-width or framed as
  `Bytes`, so there is only one way to split the input back into its fields.
- SHA-256 is the algorithm defined in [section 29](#29-sha-256-definition); later
  revisions of the standard do not change it.

`H` is the protocol's own commitment function. It is not offered to programs as a
host function: a program that needs a hash computes it in its own code under the ordinary instruction and resource rules.

The tags used by consensus:

| Tag | Hashes | Produces | Rules |
|---|---|---|---|
| `chain` | The genesis configuration | ChainId | [Genesis](#3-genesis-and-network-identity) |
| `genesis` | The ChainId | Identifier of the genesis anchor | [Genesis](#3-genesis-and-network-identity) |
| `block` | The header | BlockId | [Proof of work](#8-proof-of-work-and-the-heaviest-chain) |
| `producer` | The producer data | The header's producer hash | [Blocks](#41-the-header) |
| `transaction` | ChainId and the transaction | TransactionId | [Blocks](#44-transactions) |
| `contract` | ChainId, TransactionId and ordinal | ContractId | [Records](#51-identifiers) |
| `code` | A module's bytes | CodeHash | [Records](#51-identifiers) |
| `state-key` | Namespace, ContractId and logical key | RecordKey | [Records](#51-identifiers) |
| `state-leaf`, `state-node`, `state-empty` | Records and subtrees | State tree hashes | [State tree](#62-hashes) |
| `transactions-leaf`, `-pad`, `-node`, `-root` | A block's transactions | Transactions root | [Below](#25-ordered-commitments) |
| `receipts-leaf`, `-pad`, `-node`, `-root` | A block's receipts | Receipts root | [Below](#25-ordered-commitments) |

### 2.5. Ordered commitments

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

Each level pairs neighboring hashes, positions `2k` and `2k + 1`, into the parent
on the next level up, until one hash remains: `tree_root`. With a single item,
`tree_root` is that item's leaf.

| Commitment | Tag | Items, in body order |
|---|---|---|
| Transactions root | `transactions` | The canonical encoding of each transaction |
| Receipts root | `receipts` | The canonical encoding of each receipt |

A committed list has fewer than `2^32` items, and the block's capacity bounds it
further.

## 3. Genesis and network identity

Every Ting network starts from a **genesis configuration**: a short, fixed list
of parameters that each node pins before it joins. Two nodes with different
configurations are on different networks, even when they run the same software.

### 3.1. What genesis fixes

| Parameter | Meaning |
|---|---|
| Protocol version | 1 for the current rules |
| Genesis timestamp | The anchor's time; the difficulty schedule starts here |
| Initial target, PoW limit | The first mining target, and the easiest target ever allowed |
| Target interval | Intended seconds between blocks |
| Half-life | How quickly difficulty reacts to the chain running ahead of or behind schedule |
| Voting epoch | Blocks over which capacity votes are counted |
| Initial and minimum capacities | One of each for C, S, D and G; there is no maximum |

Every other rule, from instructions and charges (the price list included) to
module-shape bounds and voting thresholds, belongs to the protocol version rather
than to genesis. Genesis chooses numbers; it cannot change behavior.

### 3.2. The configuration bytes

The configuration is [canonically encoded](#2-canonical-bytes-and-hashing) with its fields in
this order, which always takes 150 bytes:

```text
GenesisConfig {
    protocol_version:   u16 = 1
    genesis_timestamp:  u64              // seconds
    initial_target:     u256be
    pow_limit:          u256be
    target_interval:    u32              // seconds
    half_life:          u32              // seconds
    vote_epoch_length:  u32              // blocks
    initial_limits:     ResourceVector
    minimum_limits:     ResourceVector
}
```

A `ResourceVector` is four `u64` values in the order C, S, D, G, one per
[resource counter](#12-what-each-counter-measures). A configuration is valid only when:

- every capacity, initial and minimum, is positive, and each minimum is at most its
  initial value. There is no maximum: [capacity voting](#10-capacity-voting)
  can raise capacities without limit;
- the initial target lies between 1 and `pow_limit`, inclusive;
- the target interval, the half-life and the voting epoch length are positive;
- the minimum D capacity is at least 302, so that an empty block with empty producer
  data always fits its data allowance.

### 3.3. ChainId

The canonical bytes of the configuration hash to the **ChainId**. It identifies the
network everywhere: in every block header, inside every TransactionId and ContractId,
and in the first message between two peers. A transaction prepared for one network
has a different ID, and different effects, on another.

```text
ChainId        = H("chain", encode(GenesisConfig))
GenesisBlockId = H("genesis", ChainId)
```

`H` is the [domain-separated hash](#2-canonical-bytes-and-hashing). A node pins the
configuration, or its digest, before it joins a network. A peer can never choose a
node's genesis for it.

### 3.4. The genesis anchor

Genesis is not a mined block. It is a fixed anchor at height 0 whose identifier is
the GenesisBlockId. Its timestamp is the genesis timestamp, its target is the
initial target, its state is empty and its accumulated work is zero. It has no
transactions, no receipts, no capacity votes and no reward. The first mined block
has height 1 and names the GenesisBlockId as its parent.

## 4. Blocks, transactions and receipts

A block's canonical bytes are its header followed by its body. All layouts in this
section use the types and rules of [canonical bytes and hashing](#2-canonical-bytes-and-hashing).

### 4.1. The header

| Field | Type | Bytes | Content |
|---|---|---:|---|
| `protocol_version` | `u16` | 2 | Always 1 |
| `chain_id` | `bytes32` | 32 | The ChainId of the network |
| `height` | `u64` | 8 | Position in the chain |
| `parent_id` | `bytes32` | 32 | The parent's BlockId |
| `timestamp` | `u64` | 8 | Time context for the block |
| `target` | `u256be` | 32 | The proof of work required |
| `transactions_root` | `bytes32` | 32 | The ordered transactions |
| `receipts_root` | `bytes32` | 32 | The ordered execution outcomes |
| `state_root` | `bytes32` | 32 | Every record after the block |
| `resources_used` | `ResourceVector` | 32 | Total C, S, D and G for the block |
| `votes` | `u8[4]` | 4 | One capacity vote per resource |
| `producer_hash` | `bytes32` | 32 | A commitment to the miner's producer data |
| `nonce` | `u128` | 16 | Varied while mining |

The fields total 294 bytes, so every header is exactly 294 bytes. Header-first
synchronization and light clients rely on that: nodes can check headers before they
fetch bodies.

The BlockId is the hash of the header, `H("block", encode(Header))`; how it meets the
target is on [proof of work](#8-proof-of-work-and-the-heaviest-chain).

```text
producer_hash     = H("producer", producer_data)
transactions_root = VectorRoot("transactions", transactions)
receipts_root     = VectorRoot("receipts", receipts)
```

`VectorRoot` is the [ordered commitment](#25-ordered-commitments)
and `state_root` is the root of the [state tree](#6-the-state-tree).

The four votes are in resource-vector order: C, S, D, G. Each is `0` to hold, `1` to
increase or `2` to decrease that capacity; any other value makes the header invalid.
What the votes do is on [capacity voting](#10-capacity-voting).

### 4.2. Header validity

Besides decoding canonically, a header is valid only when:

- its `height` is its parent's height plus one;
- its `chain_id` is the ChainId of the pinned [genesis](#3-genesis-and-network-identity);
- its `target` equals the target derived from its ancestry, as described in
  [time and difficulty](#9-time-and-difficulty), which also covers the
  timestamp rules;
- its three roots and `resources_used` equal the results the validator computes
  itself from the body and the parent state;
- at a seed height, its BlockId is the one the seed lists
  ([section 27](#271-the-seed-list)).

What a validator must establish for a whole block is listed under
[validating a block](#71-validating-a-block); the order in which it checks does not
change which blocks are valid.

### 4.3. The body

| Field | Type | Content |
|---|---|---|
| `producer_data` | `Bytes` | Chosen by the miner |
| `transaction_count` | `u32` | The number of transactions, n |
| `transactions` | `Bytes[n]` | Each transaction's encoding, framed as `Bytes` |
| `receipts` | `Bytes[n]` | Each receipt's encoding, framed as `Bytes`, in transaction order |

The body holds the producer data, a count, the transactions, and then one receipt
per transaction. Producer data is chosen by the miner, has no fixed size, and is
paid for as block data; its hash must match the header's `producer_hash`. How block
data is counted is on [what each counter measures](#12-what-each-counter-measures).
Receipts travel with the block for transport and proofs, but a validator derives its
own by execution and requires them to match exactly; it never trusts the supplied
bytes. An empty body, with no transactions, is valid.

Transactions execute once each, one after another in body order. The first runs
against the parent's state; each later one sees that state plus the successful
effects of the transactions before it. Receipt i belongs to transaction i.

### 4.4. Transactions

There are two kinds:

- **Call** names a target ContractId and carries input bytes.
- **Deploy** carries WebAssembly code and input for its first run.

| Field | Type | Content |
|---|---|---|
| `envelope_version` | `u8` | Always 1 |
| `kind` | `u8` | `0` for Call, `1` for Deploy; any other value is invalid |
| `budget` | `ResourceVector` | The maximum C, S, D and G the transaction may consume |
| payload | | The fields of the kind, below |

| Kind | Field | Type | Content |
|---|---|---|---|
| 0, Call | `target` | `bytes32` | The ContractId to invoke |
| | `input` | `Bytes` | Input for the invocation |
| 1, Deploy | `code` | `Bytes` | The WebAssembly module |
| | `initial_input` | `Bytes` | Input for the first run |

Both kinds declare a **budget**. Each component may be any `u64`, including one larger
than the current block capacity. Only what the block's transactions actually consume
together must fit: a debit that would exceed the block's capacity makes the block
invalid, and a validator stops there (see
[block capacity and bounds](#14-block-capacity-and-bounds)).
The data budget does have a floor, the envelope's own data cost, described on
[what each counter measures](#12-what-each-counter-measures), and a ceiling of `2^32 − 1`.
The body frames the transaction and its receipt each as `Bytes`, and every byte of
both is charged as D, so a transaction that declares no more always fits its frames.
A miner may apply stricter
admission rules of its own, but these are policy, not validity.

```text
TransactionId = H("transaction", ChainId || encode(Transaction))
```

The TransactionId hashes the ChainId together with the whole transaction, so changing
the budget changes the ID.

A transaction says nothing about who sent or authorized it, and the protocol keeps no
list of transactions it has already seen: there is no global replay protection or
uniqueness rule. The same bytes can be included again, even twice in one block, and
each **occurrence** executes normally. That is why the API reports a list of
occurrences for an ID.

Authorization belongs to applications. An application can have its users sign a
separately encoded intent, which should bind the chain, the application, the action
and the application's own replay-protection data. Signing the complete envelope,
including the signature itself, is not required. These are application conventions, not additional envelope validity rules.

### 4.5. Receipts

A receipt records the TransactionId, a status, the resources actually used, result
bytes and the events emitted.

| Field | Type | Bytes | Content |
|---|---|---:|---|
| `transaction_id` | `bytes32` | 32 | The TransactionId |
| `status` | `u8` | 1 | The outcome, below |
| `resources_used` | `ResourceVector` | 32 | What the transaction consumed |
| `result` | `Bytes` | 4 + length | Result bytes of the top-level invocation |
| `event_count` | `u32` | 4 | The number of events, m |
| `events` | `Event[m]` | 36 + payload length, each | The retained events, in order |

Without its result bytes and events, a receipt is 73 bytes. Events follow one another
directly; only their payloads carry a length prefix.

| Status | Meaning |
|---|---|
| 0 | Success |
| 1 | Explicit revert |
| 2 | VM or host trap |
| 3 | Compute allowance exceeded |
| 4 | State-access allowance exceeded |
| 5 | Data allowance exceeded |
| 6 | Growth allowance exceeded |
| 7 | Structural execution bound exceeded: deployment ordinal overflow |
| 8 | Target program absent |
| 9 | Deployment module invalid |
| 10 | Deployment identifier already exists |
| 11 | Read-only operation prohibited |

No other status value is valid. A successful deployment's result is the new
ContractId, as described in [deploy and invoke](#23-deploy-and-invoke).

A failed transaction has no effect on state and keeps no events: a receipt with any
status other than 0 has zero events. Its receipt still records what it consumed,
along with any result bytes a revert returned. Diagnostic messages, stack traces,
error strings, timings and compilation details are never part of a receipt, so
implementations cannot disagree about them and they cannot affect any root.

Inside a program, the host's call and deploy functions report a child's outcome with
the same status codes, or with call error 128 when no child was run at all. 128 is a
value returned to the calling program, never a receipt status. Which problems return
128, which ones fail the executing frame, and in what order they are checked is on
[the host interface](#227-calls-and-deployments).

### 4.6. Events

An event is the emitting ContractId plus bytes defined by the program. Events are
kept, in emission order, only from invocations that succeed and whose callers succeed too.

| Field | Type | Content |
|---|---|---|
| `emitting_contract` | `bytes32` | The ContractId whose code emitted the event |
| `payload` | `Bytes` | Bytes chosen by the program |

An invocation that fails, whether by revert, trap or exhaustion, contributes no
events, and neither does anything it called.
When the top-level invocation fails, the receipt keeps none at all. Emitting an event
still costs data even if it is later discarded; see
[what each counter measures](#12-what-each-counter-measures).

## 5. Contracts and records

All state lives in one flat map of **records**: 32-byte keys to byte values.
Contracts, their code and their storage are all records in that map,
told apart by how their keys are derived.

### 5.1. Identifiers

```text
ContractId = H("contract",  ChainId || TransactionId || u64(ordinal))
CodeHash   = H("code",      code)
RecordKey  = H("state-key", u8(namespace) || ContractId || logical_key)
```

`H` is the [domain-separated hash](#24-domain-separated-hashing).
`ChainId` comes from [genesis](#3-genesis-and-network-identity) and `TransactionId` from the
[transaction](#44-transactions) that deploys the contract. `code` is the
complete original module bytes, `namespace` is one byte from the table below, and
`logical_key` is exactly 32 bytes.

The **deployment ordinal** counts deployment attempts within one transaction,
starting at zero. Every deployment, the transaction's own or one started by a
program, takes the counter's current value as its ordinal when it begins and then
increments the counter. The counter never goes back, even when a deployment fails or
a call is rolled back, so every attempt gets a distinct identifier. If incrementing
would overflow the 64-bit counter, that deployment fails with status 7, structural
bound exceeded. Exactly when an attempt begins is part of the deployment sequence
[below](#52-deployment).

None of these identifiers depends on the current block's hash, its timestamp or its
nonce: the same deployment attempt of a transaction gets the same ContractId
whichever block the transaction lands in.

### 5.2. Deployment

A deploy transaction and a deployment started by a program are the same operation.
It fails when its ContractId already exists (status 10), when its module is invalid
(status 9), or when its first invocation fails. A failed deployment rolls back its
code and every state change it made. The full sequence, with its charges, is on
[deploy and invoke](#23-deploy-and-invoke).

Code is installed once and never changes: there is no replacement, no deletion, no
account ownership in the protocol and no running of one contract's code against
another contract's storage. Using another contract always means an explicit call, and
a program can check which code it is about to call with the `code_hash` host
function.

### 5.3. Namespaces

| Namespace | Holds | Logical key | Written by |
|---|---|---|---|
| 0 | The manifest | 32 zero bytes | The runtime |
| 1 | The code, as one record | 32 zero bytes | The runtime |
| 2 | The contract's storage | Any 32 bytes | The program |

A manifest is exactly 44 bytes:

```text
manifest = CodeHash || u32(code_length) || u64(code_price)
```

`code_price` is the code's [code price](#157-code), worked out once
when it is deployed, so that a call can charge it before reading any of the code.

The code record holds the complete original module bytes, exactly as deployed. It is
at most `2^30` bytes long. A deploy transaction carries its code,
so its data budget bounds the code too. Code a program deploys comes from that
program's memory and is paid for in compute and growth; no data is charged for its
bytes ([deploy and invoke](#23-deploy-and-invoke)).

A contract's manifest and code record always agree, and installed code always
satisfies the [execution profile](#16-the-execution-profile). These are invariants of the
state, not outcomes a program can observe: a node that found otherwise would be
holding corrupt state.

A program reads and writes only its own namespace-2 records, through the
[host interface](#22-the-host-interface). The runtime alone manages manifests and code.

### 5.4. Present, empty and absent

An empty value is a present record. It differs from an absent record, and programs
can tell them apart. Deleting a record makes it absent.

Storage values have length 0 through `2^32 − 2`; `2^32 − 1` is the absent-value
sentinel returned by storage getters. Code records also obey the module-size and
validation rules. There is no smaller fixed limit on a storage value: budgets and
the guest's memory constrain writes.

## 6. The state tree

The state root in each header commits to every record. It is the root of a binary
**compact sparse Merkle tree** over the records' keys: the tree splits only as far as
it must to separate records, and a part of the tree that holds a single record is
just that record's leaf.

### 6.1. Shape

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

### 6.2. Hashes

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

`H` is the [domain-separated hash](#24-domain-separated-hashing),
`RecordKey` is the record's [key](#51-identifiers), and `value` is its
byte string, possibly empty. An empty state has the root `EMPTY`; a state with one
record has that record's leaf as its root. Keys are distinct, so any two records
differ in some bit and every node has a depth `d` of at most 255.

The internal hashes are derived from the records. They are not records themselves and
are not part of the state map.

### 6.3. Changing a record

Only one path changes when a record does:

- **Writing an existing record** replaces its leaf.
- **Writing a new record** takes the empty subtree where its path ends. If its path
  instead ends at another record's leaf, both move down to the first bit where their
  keys differ, and nodes fill the levels between, each with one empty side.
- **Deleting a record** empties its place. If its parent's other child is then a
  single leaf, that record moves up, and keeps moving up for as long as it is alone.

Then the nodes on the path are hashed again up to the root: about log₂ of the
number of records of them, and never more than 256.

## 7. How nodes agree

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

### 7.1. Validating a block

To accept a block, a validator must establish all of the following:

1. At a seed height, the block's BlockId is the one the seed lists
   ([section 27](#271-the-seed-list)).
2. The parent belongs to the pinned network ([genesis](#3-genesis-and-network-identity)) and
   has a valid, available ancestry.
3. The header's fields, height, timestamp, target and vote values are valid
   ([the header](#4-blocks-transactions-and-receipts), [time and difficulty](#9-time-and-difficulty),
   [capacity voting](#10-capacity-voting)).
4. The header's [proof of work](#8-proof-of-work-and-the-heaviest-chain) is valid.
5. The body has canonical framing, its producer data matches the header's commitment
   to it, and its transactions match the transactions root.
6. Every transaction envelope satisfies the static rules and decodes within its
   bounds.
7. Executing the transactions in order from the parent state produces exactly the
   supplied receipts ([calls and failure](#24-calls-rollback-and-read-only)).
8. The receipts root and the final state root match the header.
9. The computed resource totals match the header and fit every active capacity
   ([block capacity](#14-block-capacity-and-bounds)).

### 7.2. Rejecting early

Cheap bounds and header checks should come before expensive execution, and a
validator may reject an invalid block as soon as it finds the fault. Such a rejection
must follow from an objective rule of the protocol. A local simulation timeout, or
how fast a compiler happens to be, can never establish that a block is invalid.

Because the header has a fixed size and the active data capacity follows from the
block's ancestry, a validator can bound the size of a body before decoding it.

### 7.3. The state transition

Validation comes down to one deterministic function:

```text
Execute(parent_state, stable_block_context, ordered_transactions)
    -> (next_state, ordered_receipts, consumed_resources)
```

The stable block context is what a program can see of its block: the ChainId, the
height, the timestamp, the parent BlockId and the producer data
([context](#228-context)). It excludes the block's own identifier, its nonce, its
target, its votes, its roots and its totals. Every
payload, code, receipt and event byte the transition needs is either published in the
body or present in the validated parent state. A commitment to bytes does not make
them available: the bytes themselves must be published.

## 8. Proof of work and the heaviest chain

Each header names its parent, so blocks form a tree rooted at genesis. Usually that
tree is a single line. When two miners find blocks at about the same time it branches,
and every node has to choose a branch.

### 8.1. Valid proof of work

A header's identifier, the BlockId, is the [domain-separated hash](#2-canonical-bytes-and-hashing)
of its canonical bytes. Read as a big-endian 256-bit number, it must not exceed the
header's target:

```text
BlockId   = H("block", encode(Header))
valid_pow = integer_big_endian(BlockId) <= target
```

The comparison is inclusive: an identifier equal to the target is valid. The miner
can change the nonce as often as it likes, but cannot choose the target, which is
derived from the chain's history (see [time and difficulty](#9-time-and-difficulty)).
A hash far below the target earns nothing extra.

### 8.2. Work, not length

Each block contributes work according to its target, and a block's accumulated work
is its parent's plus its own. The genesis anchor's accumulated work is zero.

```text
work(target)      = floor(2^256 / (target + 1))
chain_work(block) = chain_work(parent) + work(target)
```

A harder target means more expected hashing, and so more work. A node follows the
branch with the greatest accumulated work among the branches that are fully valid
and whose data is available. Seed heights take only the listed blocks
([section 27](#271-the-seed-list)), so a branch that departs from the seed is never
valid, however much work it carries. Counting blocks instead would be wrong whenever two
branches have different difficulty.

Headers alone never decide. A branch whose bodies or state transitions have not been
validated cannot become canonical, however much work its headers claim. Which of two
branches with nearly equal work to prefer is local policy, so two nodes can briefly
sit on different tips. The reference node's local policy: it moves to a branch that grows its
current tip as soon as that branch has more work, and to any other branch only when
`candidate_work > start_work + floor(work(candidate_head.target) / 2)`, where
`start_work` is the work of the tip it began moving from and the candidate head is
that branch's last block; it tries candidate heads most work first. This margin is
local fork-choice policy, not a validity rule or a restriction on targets. The margin matters because a block's target follows its
parent's timestamp: of two branches of one length, the one stamped earlier carries
slightly more work, and without the margin a miner withholding blocks could win
every such race by stamping its branch a second earlier. A branch one block longer
still wins at once.

### 8.3. Reorganizations

When a competing branch overtakes the current one, the node restores the state at the
common ancestor, applies the competing blocks and switches its tip. Transactions and
events on the abandoned branch stop being canonical. The reference node returns the
displaced transactions to its mempool.

Difficulty, capacity history and pending capacity changes are all derived from the
branch itself, so they switch along with it. A node must therefore be able to restore
the state of any ancestor and replay a competing branch together with its difficulty
history, its capacity history and its pending capacity changes.

### 8.4. No fixed finality

No confirmation count makes a block irreversible by rule. Every block mined on top
makes replacing it more expensive, and applications decide how much accumulated work
is enough for what they do.

## 9. Time and difficulty

Difficulty keeps blocks arriving near the target interval as hashpower comes and goes.
It depends only on timestamps recorded in the chain, never on a node's own clock.

### 9.1. Timestamps

A block's timestamp must be greater than the median of the timestamps of the blocks
before it: the previous eleven, or, while fewer than eleven blocks exist, all of them
plus the genesis timestamp. With an even number of samples, the upper of the two
middle values is used. For the block at height *h*:

```text
samples = timestamps at heights h − 1, h − 2, …, max(0, h − 11)   // height 0 is genesis
sorted  = samples in ascending order                            // count = min(h, 11)
median  = sorted[floor(count / 2)]                              // counted from 0
valid   = timestamp > median
```

That is the only consensus rule about time. A node should defer a header whose
timestamp is more than 7,200 seconds (two hours) ahead of its local clock, but that
is admission policy, not invalidity: the same block becomes acceptable once the clock
catches up. Difficulty and capacity calculations use only data in the branch.

Timestamps, like producer data, are context chosen by the miner. They authenticate no
external identity and are not unpredictable randomness. Nor are they precise, and
programs should not treat them as any of these.

### 9.2. The retarget

Every block's target follows from one anchor, genesis, and one question: how far
is the chain ahead of or behind its schedule? The schedule says the parent of the
block at height *h* should have arrived `interval × (h − 1)` seconds after genesis.

```text
behind = (parent timestamp − genesis timestamp) − interval × (h − 1)
target = initial target × 2^(behind / half-life)
```

If the chain is one half-life behind schedule, the target doubles and mining gets
easier; one half-life ahead halves it. The exponent is evaluated with exact integer
arithmetic in steps of 1/65536, with a fixed cubic polynomial for the fractional
part, so every node computes the same target. There is no averaging window and no
limit on how far one block can move the target, and nothing makes mining easier just because a long time has passed since the last
block: being behind schedule is what matters, and a block that is early tightens
the target again.

### 9.3. Exact arithmetic

The interval, half-life, genesis timestamp, initial target and PoW limit come from
[genesis](#3-genesis-and-network-identity). For the block at height *h* with parent *p*:

```text
elapsed  = timestamp(p) − genesis_timestamp                 // signed
expected = target_interval × (h − 1)
exponent = floor(((elapsed − expected) × 65536) / half_life)
shifts   = floor(exponent / 65536)
frac     = exponent − shifts × 65536                        // 0 ≤ frac < 65536
factor   = ((195766423245049 × frac + 971821376 × frac² + 5127 × frac³ + 2^47) >> 48) + 65536
target   = clamp(floor(initial_target × factor × 2^(shifts − 16)), 1, pow_limit)
```

- Every value is a mathematical integer. Bounds, products and quotients are evaluated
  without any intermediate fixed-width overflow, and `floor` rounds toward negative
  infinity, for negative values too.
- `factor` is a cubic approximation of `65536 × 2^(frac / 65536)` in 16.48 fixed
  point; `>> 48` is floor division by 2^48.
- The factor `2^(shifts − 16)` is an exact multiplication when `shifts − 16` is
  zero or positive and a floor division by `2^(16 − shifts)` when it is negative.
  When `shifts − 16` is 256 or more the result saturates at `pow_limit`, and when
  it is −274 or less it saturates at 1.
- `clamp` raises a result below 1 to 1 and lowers a result above `pow_limit` to
  `pow_limit`.
- The first mined block (*h* = 1) has genesis as its parent, so
  `elapsed = expected = 0` and its target is the initial target.

A header's target must equal this value exactly. There is no minimum-difficulty
exception triggered by the clock or by a long gap between blocks.

## 10. Capacity voting

Block capacity is how much compute, state access, data and growth one block may use.
It is the one aspect of execution that can still change after a freeze. Miners change
it by voting, through a rule that is itself frozen. There is no ceiling: capacity can
follow hardware for as long as the chain exists.

### 10.1. Signals

Every header carries four votes, one per resource in C, S, D, G order: 0 for hold,
1 for increase or 2 for decrease. Any other value makes the header invalid. A vote is
weighted by the [work](#82-work-not-length) of the block
that carries it, `work(target)`, so voting costs exactly as much hashpower as mining.
There is no registry of miners, no signature and no identity. Only blocks on the
branch being evaluated count; stale blocks and blocks on other branches signal
nothing. The genesis anchor carries no votes.

### 10.2. Counting

Votes are counted per **epoch** of E blocks, where E is the voting epoch length fixed
at [genesis](#3-genesis-and-network-identity), and separately for each resource. If blocks
carrying at least two-thirds of the epoch's work signal increase, the decision is
increase. If blocks carrying more than half of the work signal decrease, the decision
is decrease. Otherwise capacity holds.

```text
epoch(h) = floor((h − 1) / E)          // for a mined height h ≥ 1

W = work of all blocks in the epoch
U = work of the blocks that signal increase
L = work of the blocks that signal decrease

direction = increase   if 3 × U ≥ 2 × W
            decrease   if 2 × L > W
            hold       otherwise
```

Both cannot pass in one epoch: an increase leaves at most a third of the work to
signal decrease, which is not a majority.

The two thresholds differ on purpose. Growth needs broad, sustained agreement;
braking needs only a majority and moves twice as fast. That asymmetry is what
stands in for a ceiling: a single large miner or a rented supermajority can win one small
step, and any majority can reverse it.

### 10.3. Notice

A decision made in epoch *e* takes effect at the start of epoch *e* + 3, so two
complete epochs pass between the vote and the change. The first three epochs use the
initial capacities. At the start of every later epoch, the direction scheduled for it
is applied to the previous epoch's capacity with the steps below, for each resource
independently:

```text
limit(e) = initial_limits                           for e = 0, 1, 2
limit(e) = apply(direction(e − 3), limit(e − 1))    for e ≥ 3
```

A block at height *h* must fit `limit(epoch(h))`, its
[active capacity](#14-block-capacity-and-bounds). Pending changes are derived from the
ancestry and roll back with it; no block can override them. The production genesis
uses epochs of 20,160 blocks, about two weeks at the target rate.

### 10.4. Steps and the floor

An increase moves a capacity up by 5%, rounded down but at least 1, with no ceiling
but the 64-bit range: it stops at 2^64 − 1. A decrease moves it down by 10%, rounded down but at
least 1, never below the floor fixed at genesis. A hold leaves it unchanged.

```text
increase(x) = min(2^64 − 1, x + max(1, floor(x / 20)))
decrease(x) = max(minimum, x − max(1, floor(x / 10)))
hold(x)     = x
```

At one increase per two-week epoch, capacity can double in about six and a half
months and grow by a factor of about 3.6 per year, indefinitely. Decreases in every
epoch halve it in about three months.

### 10.5. What votes cannot change

Votes change capacity and nothing else. They cannot change what a counter measures,
what any operation costs, what an instruction means, any format, hashing or other
cryptography, the difficulty rule, or whether a program's actions are valid. Capacity
history is derived from the branch, so a reorganization switches it along with
everything else.

## 11. Budgets and exhaustion

### 11.1. Budgets within budgets

A transaction declares its maximum C, S, D and G. When a program calls another
contract, it gives the child a sub-budget that must fit in what the caller has left.
Whatever the child consumes is also charged to every enclosing budget, and whatever it
doesn't use stays available to the caller.

Each of these is a frame: the transaction's top-level invocation, whose budget is
the declared one, and every nested call or deployment, whose budget is its
sub-budget. Every frame has a budget and a consumed vector that only ever grows. A
request for a sub-budget larger than the caller has left creates no child; see
[Calls, rollback and read-only](#24-calls-rollback-and-read-only).

### 11.2. Atomic debits

Before each counted event, the whole debit is checked against the current frame and
every frame around it. If it fits everywhere, it is charged everywhere and the event
happens. If not, nothing is charged, the event does not happen, and the invocation
that attempted it fails.

```text
debit v = [c, s, d, g] attempted by frame f:
    if consumed(x) + v <= budget(x) for f and every frame enclosing f:
        consumed(x) += v for all of those frames
        perform the event
    else:
        apply no part of v, do not perform the event, and fail f
```

Earlier debits that succeeded stay consumed. When an event would exceed more than
one allowance at once, the failure reports the first in the order C, S, D, G, so
every implementation reports the same status:

| First counter that does not fit | Status of the failed invocation |
|---|---:|
| C | 3 |
| S | 4 |
| D | 5 |
| G | 6 |

The failed invocation's effects are rolled back and its caller receives the status
and may continue; at top level it becomes the status in the
[receipt](#4-blocks-transactions-and-receipts). Which checks a host function makes before its debit,
and in what order, is on [The host interface](#22-the-host-interface).

### 11.3. Nothing is refunded

Rollback restores state, never counters. A child that fails keeps its consumption
charged to its parent. A transaction that fails keeps everything it consumed, in its
receipt and in the block total. Unused allowance is simply not consumed; there is
nothing to refund because it was never spent.

A consumed vector can neither overflow nor become negative: it only grows, and only
by debits that fit under a budget.

The receipt records actual consumption, including all work done before a failure.

### 11.4. Exhaustion is per transaction

A transaction runs out when its own budget is exhausted, never because the block is
filling up. Whether a block's transactions fit the block's capacity is a separate
check. A block that does not fit is invalid; its transactions do not get different
outcomes to make it fit. See [Block capacity and bounds](#14-block-capacity-and-bounds).

## 12. What each counter measures

Each counter is charged by a fixed list of logical events, and every
implementation must reproduce those events exactly. This section lists every event
with its amount. The precise point at which each VM debit happens, and its order
relative to traps, is on [Exact metering](#21-exact-metering); the order of checks
inside each host function is on [The host interface](#22-the-host-interface).

### 12.1. C, compute

C charges each event at its price on [the price list](#15-the-price-list). The
prices were measured once, so that one C stands for about the same work whatever it
is spent on; `bytes(n, s)` is the price of moving `n` bytes in `s` spans that hold a
byte at the executing instance's [memory prices](#152-memory-prices),
and `sha(m)` the price of a SHA-256 over `m` input bytes.

| Event | C |
|---|---:|
| Start a top-level call or deployment, before any state access | 1 |
| Dispatch an executed instruction | its price, by instruction and, for loads and stores, by memory size |
| Enter a defined function | 1 per parameter, declared local and slot of its maximum operand height, and 40,000 per 64 KiB the entry adds to the invocation's [stack](#154-stack) beyond its mark |
| Reach a host import through `call_indirect` | 94, after the indirect call's table and type checks |
| Read a record, for its key | 1,900, in the debit of the read's S |
| Write a value of `n` bytes to a record, for its leaf | `sha(n + 53)`, in the debit of the write's S |
| Prepare an instance | 40,000, plus 40,000 per page of initial memory, 40 per global, 4 per table entry, 12 per active data segment and `bytes(n, s)` for the `n` bytes of the active data segments, `s` of which hold a byte |
| `memory.grow` that adds `p` pages | 40,000 × `p`, after its dispatch |
| `memory.copy` of `n` bytes | `bytes(n, s)` over its source and destination, after its dispatch |
| `memory.fill` of `n` bytes | `bytes(n, s)` over its destination, after its dispatch |
| Copy `n` bytes between a program and the host, in either direction | `bytes(n, s)` over the spans copied |
| Return, revert or emit `n` bytes | `bytes(n, s)` over the one span |
| Prepare code: validate a deployment, or load code its transaction has not yet prepared | the code price |
| Hash deployed code of `n` bytes | `sha(n + 11)`, with the code price |
| Derive a deployment's ContractId | 1,900 |
| Produce the identifier of a successful deployment | 32 |

- Skipped instructions cost nothing, and the delimiters `else` and `end` cost nothing.
- The top-level unit does not depend on the program's instructions; it is the
  transaction's first compute debit.
- Calling a host function pays the `call` instruction's price for an import, then the
  host function's own charges. Host functions reserve no slots. Return, revert and
  emit pay for their bytes once, in the same debit as their D.
- The preparation charges of an instance are one atomic debit, taken after the code
  is loaded and before the entry function's reservation.
- Instance preparation happens on **every** invocation, because every invocation
  starts from a fresh instance. A program that declares one 64 KiB page pays 80,000 C
  each time it runs, before its first instruction executes.
- An invocation pays its code's price unless the transaction has already prepared
  that CodeHash. Code becomes prepared when a deployment's validation accepts it or
  when an invocation's code-price debit succeeds, and stays prepared whatever fails
  later. A deployment pays the code price and its CodeHash every time, prepared or
  not ([section 26.3](#263-checkpoint-scope-and-prepared-code)). What a node keeps
  cached never changes the charge.
- The code price is charged before validation or any cache lookup, and invalid code
  keeps the charge.

Prices are fixed numbers. Nothing is ever measured while a transaction runs.

### 12.2. S, state access

S charges one unit per logical record read or write, including reads of records
that turn out to be absent. Each unit stands for one random access and one path
through the [state tree](#6-the-state-tree). Every byte-proportional cost,
including the bytes of a value, is compute, so a large value or a large module
costs C, never extra accesses. That keeps S an honest measure of random access.

| Event | S |
|---|---:|
| Read one record, present or absent | 1 |
| Write or delete one record | 1 |
| `storage_get` | 1 |
| `storage_set` or `storage_delete`, including a no-op and the deletion of an absent record | 2: a read of the old record, then the write |
| `code_hash`: read the target's manifest | 1 |
| Start an invocation: read its manifest and its code record | 2 |
| Deploy: check that the new ContractId is free by reading its manifest | 1 |
| Deploy: write the manifest and the code record, each like a storage write | 2 each |

Running a program reads its manifest and its code record, two units, on every
invocation. A node that already has the code compiled and cached still charges the
same: caches make nodes faster, never transactions cheaper. A deployment that
installs its code has made five accesses before its initial invocation adds those
two.

Maintaining the state tree has no separate charge. The node hashes on a record's
path, about log₂ of the number of records and never more than 256, are bounded
work tied to the access, since a key has a fixed width.
The hashes whose size a program chooses or that every access needs, a record's key
and a written value's leaf, are compute, priced in C above. Hashing transaction
envelopes and commitment vectors is bounded by their serialized size and entry
count. No host function does tree or hashing work outside these bounds.

### 12.3. D, data

D covers bytes the network must carry, or that programs generate.

| Event | D |
|---|---:|
| A transaction, before execution starts | `len(encode(Transaction)) + 81` |
| Return or revert `n` result bytes, by any invocation | `n` |
| Emit an event with an `n`-byte payload | `36 + n` |
| Produce the identifier of a successful deployment | 32 |

The transaction's data base covers its own [encoding](#2-canonical-bytes-and-hashing) and its
place in the [block body](#4-blocks-transactions-and-receipts):

```text
data base = len(encode(Transaction)) + 81
81        = 4      transaction's length prefix in the body
          + 4      receipt's length prefix in the body
          + 73     receipt's fixed fields
```

An event's 36 bytes are the emitting ContractId and the payload's length prefix. A
transaction whose declared D is smaller than its data base, or larger than
`2^32 − 1`, has an invalid envelope: its encoding and its receipt are each framed with
a 32-bit length in the block body ([blocks](#44-transactions)).

Output is charged when it is generated, even if a failing caller later discards it,
so rollback cannot be used to recycle a data allowance. A child's result read with
`call_result` costs only its copy compute; if the parent returns those bytes again,
that is new output and is charged again. A deployment's initializer result is
discarded but stays charged. D is therefore an upper bound on what is published plus
the output that was discarded.

### 12.4. The block's own bytes

Each block consumes D outside every transaction budget:

```text
block data = 302 + len(producer_data)
302        = 294    header
           + 4      producer data's length prefix
           + 4      transaction count
```

A block's D therefore equals its canonical encoded length only when the only output
of its transactions is their top-level results and the events their receipts keep.
Every other output is charged D that no block carries: a nested invocation's
result, a deployment initializer's result, the identifier a nested deployment
returns, and output a failure discarded. With any of these, D is larger. Framing used
to carry blocks between peers is not counted. How the block total is formed is on
[Block capacity and bounds](#141-block-totals).

### 12.5. G, growth

G charges for gross new persistent allocation. A new record costs 36 plus its value
length, growing an existing value costs the added bytes, and deleting or shrinking
costs nothing. See [Persistent growth](#13-persistent-growth).

### 12.6. What is never counted

Elapsed time, physical disk reads, cache hits and misses, compiled-code size and
database layout are never counted. A node can change all of them without changing a
single counter.

## 13. Persistent growth

Most resources are used up once a block has been processed. State is different: a
record written today has to be kept by every node for as long as it exists. G
measures how much a transaction asks the chain to allocate.

### 13.1. The rule

A record's logical allocation is `36 + len(value)` bytes: its 32-byte key, a 4-byte
length and the value. Each write is charged the allocation it adds:

```text
growth(absent -> value) = 36 + len(value)
growth(old -> new)      = max(0, len(new) - len(old))
growth(delete)          = 0
```

Creating a record with an empty value still costs 36. The 36 bytes are a logical
unit, not a claim about how many bytes a database actually uses: how a node stores
records and tree nodes is its own business.

### 13.2. When it is charged

Each positive allocation is charged when the write is attempted: after the
operation's required preflight checks and the read of the old record, in one atomic
debit together with the write's state access and the compute to hash the value,
and before the new value is installed. If that debit does not fit, nothing is
written. The exact order of a storage write is on
[The host interface](#22-the-host-interface).

The runtime's own records follow the same rule. Installing `n` bytes of code writes a
44-byte manifest (the CodeHash, the code length and the code price) and the code
record:

```text
manifest:  36 + 44
code:      36 + n
total:     116 + n     charged before the initializer runs
```

### 13.3. Gross, not net

G counts allocation when it happens, not the final size of the state:

- creating, deleting and recreating a record pays for the creation twice;
- growing a value and then shrinking it still pays for the growth;
- allocation inside a call that later reverts stays charged.

A transaction cannot hide allocation work behind a small final result. Deleting a
record frees space but earns no credit.

## 14. Block capacity and bounds

### 14.1. Block totals

A block's C, S and G are the sums of what its transactions actually used. Its D also
includes the header, the producer data and the transaction count, so for a block
whose only output is its transactions' top-level results and the events their
receipts keep, D equals the block's encoded size in bytes; any other output makes it
larger ([counters](#12-what-each-counter-measures)). All four totals
must fit the active capacity at that height.

```text
block_used[C, S, G] = sum(transaction_used[C, S, G])
block_used[D]       = 302 + len(producer_data) + sum(transaction_used[D])
```

Each transaction's used vector is what its receipt records, including everything
consumed before a failure; the 302 bytes are broken down under
[the block's own bytes](#124-the-blocks-own-bytes). Each of the
four totals must be at most the active capacity, derived independently from the
block's ancestry, and must equal the resources the header declares.

A block that exceeds a capacity is invalid as a whole. It does not change any
transaction's outcome to fit the remaining space, and since counters are never
restored by rollback, a revert cannot bring a total back under the limit. A
validator may stop executing as soon as a debit would push the block over a limit,
checking the transaction's own budgets first, so a debit the transaction could not
afford never counts toward the block. That early stop is only a way of rejecting the
block; it never produces an alternative receipt.

### 14.2. Declared versus actual

A transaction's declared budget may exceed the current block capacity, because only
what it actually uses has to fit. C, S and G may be any u64 declaration; D must also satisfy its envelope floor and u32 ceiling. A transaction can
therefore carry generous headroom without its author knowing how full the block will
be, and simulating a transaction needs no assumption about how much block space
remains.

### 14.3. Active capacity

Capacity starts at the genesis initial values and moves only through
[capacity voting](#10-capacity-voting), upward without limit and downward
to the genesis floor. Their derivation is independent of any local node setting.

### 14.4. Bounds that follow from the counters

Nothing a program does is capped by a fixed per-transaction number. Everything that
could be capped follows from the budgets instead:

| Quantity | Bounded by |
|---|---|
| Live guest memory | Compute: every 64 KiB page costs 40,000 C and is never refunded, about 1.6 bytes per C |
| Local stack | Compute: every function entry pays its parameters, locals and operand slots |
| Nesting depth | State access: every nested invocation reads two records before it runs |
| Record size | The memory that holds the value, and the compute to copy and hash it |
| Module size | 2^30 bytes, and the [code price](#157-code), about 60 C per byte, paid to validate it |
| Pending output and state journals | D, S and G: every byte and record they hold was charged |

A node therefore knows, from the active capacities alone, how much memory a block
can need. It may store journals however it likes, but local memory pressure can
never turn into a different consensus result.

### 14.5. Fixed bounds

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

How a module is checked against them is in [Validation](#18-validation).

## 15. The price list

Every compute (C) charge has a fixed price, set from measurements and frozen with the
rest of the protocol. One C is about the time a processor core takes for one integer
addition that has to wait for the one before it, about one clock cycle. Every other
price says how many of those additions an operation was priced to cover. This section lists
the prices. When each one is debited, and in what order, is on
[exact metering](#21-exact-metering), [the host interface](#22-the-host-interface) and
[deploy and invoke](#23-deploy-and-invoke).

### 15.1. Instructions

An instruction's price is debited immediately before it executes.

| Price | Instructions |
|---:|---|
| 0 | `else`, `end` |
| 1 | `unreachable`, `nop`, `block`, `br`, `return`, `drop`, `local.get`, `local.set`, `local.tee`, `global.get`, `global.set`, `memory.size`, `memory.grow` (plus its pages), `i32.const`, `i64.const`; `add`, `sub`, `and`, `or`, `xor`, `shl`, `shr_s`, `shr_u` of both widths; `i32.wrap_i64`, `i64.extend_i32_s`, `i64.extend_i32_u` and the five sign-extension operators |
| 2 | `loop`, `rotl` and `rotr` of both widths, `i64.add128`, `i64.sub128` |
| 3 | `select` |
| 4 | `eqz` and every comparison of both widths; `clz`, `ctz`, `popcnt` of both widths |
| 5 | `mul` of both widths |
| 8 | `i64.mul_wide_s`, `i64.mul_wide_u` |
| 9 | `call` of a defined function, before the callee's reservation |
| 18 | `if`, `br_if` |
| 20 | `memory.copy`, `memory.fill`, before their bytes |
| 33 | `div_s`, `div_u`, `rem_s`, `rem_u` of both widths |
| 37 | `br_table` |
| 50 | `call_indirect`, before the callee's reservation, or before the host call's 94 when the table entry is a host import |
| 94 | `call` of a host import, before the import's own charges |
| by memory size | every load and store, below |

Entering a defined function also debits its reservation, one C per parameter,
declared local and slot of its maximum operand height
([function entry](#214-function-entry)), and 40,000 C for every
64 KiB the entry adds to the invocation's [stack](#154-stack) beyond its highest point
so far. A `call_indirect` whose table
entry turns out to be a host import debits 94, the price of a host call, after its
table and type checks, where a defined function's reservation would be.

### 15.2. Memory prices

A load waits for memory, and how long depends on how much memory the program has. A
small memory stays in the processor's caches. Past the processor's own cache, 2 MiB
on the slowest machine the prices are set for, an access can be a trip to RAM, and a
very large memory also misses the processor's address-translation caches. Loads,
stores, the bytes a program moves and the spans it moves them in are therefore priced
by the executing instance's current memory size, in 64 KiB pages.

Up to 2 MiB the prices step with the caches:

| Memory size | Load | Store | Word | Span |
|---|---:|---:|---:|---:|
| at most 64 KiB, 1 page | 18 | 1 | 4 | 0 |
| at most 1 MiB, 16 pages | 31 | 1 | 8 | 0 |
| at most 2 MiB, 32 pages | 80 | 1 | 8 | 0 |

Above 2 MiB they follow a curve over the size, in whole numbers only:

```text
b     = the position of the highest set bit of pages     2^b ≤ pages < 2^(b+1)
L     = 16 × b + floor(16 × pages / 2^b) − 16            log₂ of pages, in sixteenths
load  = 770 + ceil((L − 80)² / 48)
store = 16 + ceil((L − 80)² / 72)
word  = 12
span  = store
```

| Memory size | Pages | L | Load | Store and span |
|---|---:|---:|---:|---:|
| 2 MiB + 64 KiB | 33 | 80 | 770 | 16 |
| 3 MiB | 48 | 88 | 772 | 17 |
| 16 MiB | 256 | 128 | 818 | 48 |
| 64 MiB | 1,024 | 160 | 904 | 105 |
| 256 MiB | 4,096 | 192 | 1,032 | 191 |
| 1 GiB | 16,384 | 224 | 1,202 | 304 |
| 4 GiB | 65,536 | 256 | 1,416 | 447 |

The load and store prices are the whole price of those instructions. Moving bytes
costs the word price for every started 32 bytes, and the span price for every
**span** that holds at least one byte. A span is a stretch of memory an operation
reads or writes in one piece. A `memory.copy` has two, its source and its
destination, and a `memory.fill` has one. The host has one for each argument it
reads and one for an output it writes ([the host interface](#22-the-host-interface)). Each
span can be a trip to RAM on its own, however few bytes it holds, which is why above
2 MiB the span price is that of a store.

```text
words(n)    = ceil(n / 32)
bytes(n, s) = word × words(n) + span × s      n bytes in s spans that hold a byte
```

`bytes(n, s)` is the price of a `memory.copy` or `memory.fill`, of every copy
between a program and the host in either direction, of the bytes a program returns,
reverts with or emits, and of copying the data segments into a new instance. A
successful `memory.grow` changes the prices for the instructions after it. Instance
preparation copies the data segments at the prices of the initial memory, one span
for each active data segment that holds a byte.

### 15.3. Memory

Each 64 KiB page costs **40,000 C**: every page of an instance's initial memory, at
preparation, and every page `memory.grow` adds. The price covers zeroing the page and
the operating system's first touch of it. It also bounds memory: one C buys at most
about 1.6 bytes of it, so a node knows from the compute capacity alone how much memory
a block can need, about twice that if it keeps its own copies of nested inputs and
code ([metering](#215-memory-growth)).

### 15.4. Stack

An invocation's stack holds, for every function active in it, the slots of its
reservation, 8 bytes each, and a record of 3 slots. Its **height** is the sum, over
the active functions, of their reservations plus 3. The first 64 KiB, 8,192 slots,
of every invocation's stack are part of its start price. Beyond them, the stack is
priced as memory is, **40,000 C** per 64 KiB, for every 64 KiB its highest point so
far, its **mark**, passes:

```text
stack pages of a height h = ⌈h / 8,192⌉ − 1, and 0 for h ≤ 8,192
```

Entering a function that takes the height above the mark pays the stack pages of
the new height minus those of the mark, with its reservation, and the new height
becomes the mark. Returning lowers the height but never the mark, so a stack paid for
once is used again for free. Memory and stack together therefore hold at most about
1.6 bytes per C of the budget ([metering](#214-function-entry)).

### 15.5. Hashing

Every SHA-256 the runtime computes on a program's behalf is priced by the length `m`
of its input, the [domain-separated](#2-canonical-bytes-and-hashing) message of 7 bytes plus the
tag and the hashed bytes:

```text
sha(m) = 100 + 900 × ceil((m + 9) / 64)
```

| Hash | `m` | Price |
|---|---:|---:|
| A record's key, with every record read | 81 | 1,900 |
| A deployment's ContractId | 87 | 1,900 |
| A value of `n` bytes written to a record, into its leaf | `n + 53` | `100 + 900 × ceil((n + 62) / 64)` |
| A deployment's code of `n` bytes, its CodeHash | `n + 11` | `100 + 900 × ceil((n + 20) / 64)` |

The node hashes on a record's path through the
[state tree](#6-the-state-tree) are part of the record's state access, S, not of C.

### 15.6. Invocations

Every run of a program, a top-level call, a nested call or a deployment's
initializer, starts a fresh instance. Its preparation is one debit:

```text
preparation = 40,000
            + 40,000 × pages of initial memory
            + 40 × globals
            + 4 × table entries
            + 12 × active data segments
            + bytes(data bytes, data spans)
```

The data bytes are the total length of the active data segments, and the data spans
are the number of those segments that hold at least one byte.

The 40,000 is the start price: setting up and tearing down an instance, whatever the
program. The rest pays for the instance's memory, its globals and table, and copying
its data.

### 15.7. Code

Preparing code to run means decoding and validating it, the same work whether a
deployment installs it or a call finds it not yet prepared. Its **code price** is read
from the module's section framing alone, before any validation:

```text
code price = 14,400
           + the module's bytes, each at its rate below
           + 1,215 × functions each function section declares
           + 446 × segments each data section declares
```

| Bytes | Rate, C per byte |
|---|---:|
| Contents of the element section | 175 |
| Contents of the global section | 100 |
| Contents of the data section | 6 |
| Every other byte: the preamble, every section's id and size, every other section's contents | 60 |

The framing is read from the 8-byte preamble on as a sequence of sections, each
an id byte, a size as an unsigned LEB128 of at most five bytes whose value fits 32
bits, minimal or not ([section 26.1](#261-code-price-scan-including-malformed-bytes)),
and that many bytes of contents. The scan stops at the first section whose id or
size cannot be read or whose contents run past the end; every byte from there on is
priced at 60. A declared count is the first such LEB128 of its section's contents,
zero if it cannot be read; every section with id 3 adds its count, and every section
with id 11 its own, however many such sections a module has. Nothing else of the
module is inspected, so the price is known before validation, and invalid code pays
it too.

The price is an exact number, however large: a module of repeated function sections
can declare more than 2^64 − 1 C of it. Like any price beyond what is left of the
budget, it is refused whole, and the deployment fails for compute. A deployment is
always charged something before its code price, so from 2^64 − 1 C on a price is
always refused, and an implementation may count it up to there and stop.

A deployment pays the code price for validation, whether or not its CodeHash is
prepared, and records it in the contract's [manifest](#53-namespaces). A call pays
it when its transaction has not yet prepared that CodeHash. Code becomes prepared
when a deployment's validation accepts it or when an invocation's code-price debit
succeeds, and stays prepared whatever fails later; further invocations of the same
code in the same transaction pay none. Every transaction starts with nothing
prepared ([section 26.3](#263-checkpoint-scope-and-prepared-code)).

### 15.8. Fixed charges

| Charge | Price |
|---|---:|
| A transaction's root dispatch | 1 C |
| Producing a successful deployment's identifier | 32 C and 32 D |

## 16. The execution profile

A deployed module must fit an exact profile. Anything outside it is rejected at
deployment; no node ever has to interpret it differently from another.

### 16.1. Defined meaning

The instruction tables, binary grammar, complete typing algorithm and metering
rules in this document define the accepted machine. Its integer behavior derives from
the WebAssembly Core 1.0 Recommendation of 5 December 2019, with the sign-extension
operators, `memory.copy` and `memory.fill` from Release 2.0 and the four
wide-arithmetic instructions, and this document states that behavior in full. Only the
encodings and features listed here are accepted: no external text adds an acceptance
rule. Should this document ever be silent on the meaning of an accepted instruction, the
dated Core text decides. An engine's default feature set is never the protocol.

### 16.2. What is accepted

- Integer constants, arithmetic, comparisons, bit operations, shifts and rotations
  on `i32` and `i64`
- 128-bit addition and subtraction, and the full 128-bit product of two `i64`
  values, through the four wide-arithmetic instructions
- Conversions between `i32` and `i64`, and the five sign-extension operators
- Locals and integer globals, mutable or immutable
- Structured control flow (`block`, `loop`, `if`, branches and `return`), direct
  calls, and indirect calls through one table
- Integer loads and stores, `memory.size`, `memory.grow`, `memory.copy` and `memory.fill`

That makes 115 accepted opcodes, listed with their exact behavior in the
[instruction reference](#19-instruction-reference) and as byte ranges under
[the binary format](#174-instruction-encodings). Parsing is not
enough: the module as a whole must also pass [validation](#18-validation)
with only these features enabled.

Indirect calls are what compilers use for function pointers, closures and trait
objects; the table they go through is filled once when the module is instantiated
and can never be changed. Bulk copies let a program move or clear memory at the
byte price of each 32 bytes instead of paying for a loop of loads and stores. Wide
arithmetic lets cryptography multiply 64-bit limbs in one instruction instead of
four half-width products.

### 16.3. What is rejected

Floating point, SIMD, reference-typed values, typed `select`, table instructions,
passive segments, `memory.init` and `data.drop`, saturating truncation, threads and
shared memory, 64-bit memories and custom page sizes, exceptions, start functions,
custom sections (debug information included), every import other than the twelve
host functions, and every other proposal. A program has no access to an operating
system, files, a network or a clock. Debug information stays a tooling concern:
it travels beside a module, never inside the deployed bytes.

### 16.4. Module shape

- **Imports** are only functions from module `ting`, each named once, with exactly
  the signatures of the [host interface](#222-signatures). Memories,
  tables and globals cannot be imported.
- **Exports** are exactly one defined function `invoke`, with no parameters and no
  result, plus the module's memory as `memory` when it has one. Nothing else is
  exported.
- **Memory** is at most one, defined by the module, with an explicit maximum of
  no more than 65,536 pages of 64 KiB (4 GiB, the whole 32-bit address space) and
  a minimum no larger than the maximum.
- **A table**, if present, holds function references, has a fixed size (maximum
  equal to minimum) no larger than the module's function count, imports included,
  and is filled once by active element segments.
- **Globals** are integers initialized by a single constant, and **data segments**
  are active, target memory 0 and must fit in the initial memory.
- **Bounds** on parameters, locals, operand slots and the counts of functions,
  types, globals and segments exist so that validation takes work proportional to
  the module's bytes ([validation](#183-bounds)). They are far above
  anything a compiler emits.

The bytes must also be the canonical encoding: every integer minimal and every
construct in its one accepted form ([the binary format](#17-the-binary-format)). A
decoder rejects anything else at the first unexpected byte, and the CodeHash
commits to the original bytes.

## 17. The binary format

A program is a WebAssembly binary module in a strict form: the MVP binary format
with no custom sections, exactly one encoding per construct, and a small fixed set
of imports and exports. A node decodes it in one pass and rejects anything it
cannot account for. The accepted bytes are the deployed bytes: the CodeHash
commits to them, and nothing is ever re-encoded.

### 17.1. Integers

All counts, indices, sizes and immediates are LEB128 integers, and every one of
them must be minimal at its specified width: 32 bits for every count, index,
length, limit, offset, alignment and `i32.const`, 64 bits for `i64.const`.

- An unsigned integer uses as few bytes as its value needs. A trailing `0x00`
  byte that only pads is rejected, and a final fifth byte may use only its low
  four bits.
- A signed integer, used by constants, has no redundant final byte: a final
  `0x00` after a byte with bit 6 clear, or a final `0x7f` after a byte with bit 6
  set, is rejected, and the unused high bits of a final byte must repeat the sign.

So `(i32.const 64)` is `41 c0 00`, `(i32.const -64)` is `41 40`, and no other
byte string means either. Names, of imports and exports, are a byte length and
the bytes.

### 17.2. Module layout

A module starts with the eight bytes `00 61 73 6d 01 00 00 00`, the standard
WebAssembly magic and binary version 1. Sections follow, each as an id byte, a
size and the content, and the size must equal the content's length. A section
with nothing in it is left out: one whose vector has no entries is rejected. Only
these sections are accepted, each at most once, in this order:

| Id | Section | Content |
|---:|---|---|
| 1 | type | function types |
| 2 | import | host functions |
| 3 | function | the type of each defined function |
| 4 | table | at most one table |
| 5 | memory | at most one memory |
| 6 | global | integer globals |
| 7 | export | `invoke` and `memory` |
| 9 | element | the table's contents |
| 10 | code | function bodies |
| 11 | data | initial memory contents |

Custom sections (id 0), the start section (8), the data-count section (12) and
anything else are rejected, and after the final section no unparsed bytes may remain; any further section must itself be one of the permitted later sections and be complete. Every vector
count is checked against the bytes left in its section before anything is
allocated for it, so a declared count can never cost more than the module's own
size.

### 17.3. Sections in detail

**Types.** Each entry is `0x60`, a vector of parameter types and a vector of at
most one result type. The only value types are `0x7f` (i32) and `0x7e` (i64). A
type has at most 1,000 parameters.

**Imports.** Each entry names module `ting`, one of the twelve host functions,
kind `0x00` (function) and a type index whose type is exactly the import's
signature: all-i32 parameters and an i32 result where the
[host interface](#222-signatures) says so. A name may appear once. No
other kind of import exists: memories, tables and globals are never imported,
and any other module or name is rejected, including the private metering
functions an instrumenting engine adds to its own copy of a module.

**Functions.** A type index per defined function. The function index space is
the imports first, then the defined functions, at most 1,000,000 in all.

**Table.** At most one: element type `0x70` (funcref), limits flag `0x01`, and a
minimum equal to the maximum, no larger than the number of functions, imports
included. It is filled at instantiation and never changes: there are no table
instructions, and a table is never exported or imported.

**Memory.** At most one: limits flag `0x01`, a minimum and a maximum in 64 KiB
pages, the maximum at most 65,536 and the minimum at most the maximum.

**Globals.** Each is a value type, a mutability byte (`0x00` immutable, `0x01`
mutable) and an initializer that is exactly `i32.const n end` or
`i64.const n end` of the global's type.

**Exports.** At most two: `invoke` as a function (kind `0x00`) that is defined,
not imported, and has no parameters and no result; and `memory` as memory 0 (kind
`0x02`), present exactly when the module declares a memory. No other export is
accepted and no name may repeat.

**Elements.** Each segment is the flag `0x00` (active, table 0 implied), the
offset `i32.const n end` and a vector of function indices, which may name imports
as well as defined functions. The segment must fit the table, and a module
without a table has no element segments. Segments apply in order; entries not
listed are null.

**Code.** One body per defined function, in order, each with its size. A body
starts with a vector of local groups, each a count and a value type: every count
is at least 1, and two neighboring groups never have the same type, so each run of
locals of one type is one group. Parameters plus declared locals are at most
16,384. The instructions follow, up to and
including the function's final `end`, and must use the body's size exactly. The
[instruction reference](#19-instruction-reference) gives every accepted encoding.

**Data.** Each segment is the flag `0x00` (active, memory 0 implied), the offset
`i32.const n end` and a vector of bytes. The segment must fit the initial memory, so a module without a
memory has no data segments. Overlapping segments apply in order.

### 17.4. Instruction encodings

Only these instruction bytes are accepted, 115 instructions in all; the
[instruction reference](#19-instruction-reference) defines each one.

| Bytes, inclusive | Instructions |
|---|---|
| `00`–`05` | `unreachable`, `nop`, `block`, `loop`, `if`, `else` |
| `0b`–`11` | `end`, `br`, `br_if`, `br_table`, `return`, `call`, `call_indirect` |
| `1a`–`1b` | `drop`, untyped `select` |
| `20`–`24` | `local.get`, `local.set`, `local.tee`, `global.get`, `global.set` |
| `28`–`29` | `i32.load`, `i64.load` |
| `2c`–`35` | integer narrow loads |
| `36`–`37` | `i32.store`, `i64.store` |
| `3a`–`3e` | integer narrow stores |
| `3f`–`40` | `memory.size`, `memory.grow` |
| `41`–`42` | `i32.const`, `i64.const` |
| `45`–`5a` | integer `eqz` and comparisons |
| `67`–`8a` | integer bit counts, arithmetic, bit operations, shifts, rotations |
| `a7`, `ac`, `ad` | `i32.wrap_i64`, `i64.extend_i32_s`, `i64.extend_i32_u` |
| `c0`–`c4` | `i32.extend8_s`, `i32.extend16_s`, `i64.extend8_s`, `i64.extend16_s`, `i64.extend32_s` |
| `fc 0a`, `fc 0b` | `memory.copy` (memory 0 to memory 0), `memory.fill` (memory 0) |
| `fc 13`–`fc 16` | `i64.add128`, `i64.sub128`, `i64.mul_wide_s`, `i64.mul_wide_u` |

Every other byte is rejected, and so is every other `fc` sub-opcode (saturating
truncation, `memory.init`, `data.drop`, the table operations). Within the
accepted instructions:

- Block types are one byte: `0x40` for no result, `0x7f` or `0x7e` for one.
- Branch depths and local, global, function and type indices are unsigned.
- `br_table` carries a vector of depths followed by the default depth.
- Memory instructions carry an alignment exponent and an offset, both unsigned;
  the alignment may not exceed the access width's natural alignment.
- `memory.size` and `memory.grow` carry one `0x00` byte; `call_indirect` carries
  its type index and then one `0x00` byte, so it always names table 0;
  `memory.copy` is `fc 0a 00 00` and `memory.fill` is `fc 0b 00`.
- The four wide-arithmetic instructions carry no immediates: each is `fc` and its
  sub-opcode, `13` to `16`.
- The `fc` sub-opcode is an unsigned LEB128 and, like every other integer, minimal.

## 18. Validation

A module's validity is established at deployment: the manifest records its CodeHash
and every node accepts the same bytes. An implementation may decode and validate
installed code again when it runs it; that adds no debit beyond the code price an
invocation pays when its transaction has not yet prepared the code
([code](#157-code)), and no acceptance rule. Validation decides the
module's shape ([binary format](#17-the-binary-format)), types every instruction, and
measures what each function will need, so that execution runs without checks
that could fail due to an ill-typed instruction; the specified dynamic traps still apply.

### 18.1. Typing

Decoding an accepted opcode is not enough: the entire module must pass the
type-checking algorithm of [section 25](#25-complete-type-checking-algorithm), which
specifies the stack operations and the unreachable-code rules in full. In summary:

- An **operand stack** of types grows and shrinks as instructions are read. An
  instruction pops its operands and pushes its results; a mismatch is an error.
- A **control stack** holds one frame per open `block`, `loop`, `if` or `else`,
  plus one for the function. A frame remembers its result type, the operand
  height when it opened, and whether the code after an unconditional branch is
  unreachable.
- After `unreachable`, `br`, `br_table` or `return`, the rest of the frame is
  **unreachable code**: the operand stack is cut to the frame's height and a pop
  from an empty frame yields an *unknown* type that matches anything. Unreachable
  code is still typed, so a module cannot hide a type error behind `unreachable`.
- `end` closes a frame: the frame's result must be on top and nothing else above
  the frame's height. An `if` without `else` may not declare a result. `else`
  closes the then-frame and opens an else-frame with the same result.
- A branch to a label must provide the label's type: the result type of a
  `block` or `if`, nothing for a `loop`. All labels of a `br_table`, the default
  included, must have the same type, in unreachable code too; Core 1.0 asks
  for exactly that, and the relaxed rule of later WebAssembly releases is not
  part of the profile.
- `select` needs two operands of one type, or one known and one unknown, and an
  i32 condition.
- `call` and `call_indirect` take their parameters from the stack in order and
  push the result, if any; `call_indirect` also needs an i32 index and a table.
- `i64.add128` and `i64.sub128` pop four i64 operands, `i64.mul_wide_s` and
  `i64.mul_wide_u` two, and each pushes two i64 results. They are the only
  instructions with two results; blocks and functions still have at most one.
- Memory instructions need a memory; `global.set` needs a mutable global; every
  local, global, function, type and label index must exist.

### 18.2. Measured for execution

While typing, the validator records for each function the number of parameters
plus declared locals and the **maximum operand height**, the highest the
validator's operand stack reaches anywhere in the body, unreachable regions
included, with each unknown pushed there counting as one slot. The function's
final `end` pushes its result, if it has one, like any other block's `end`, so a
function with a result reaches a height of at least one even when its body ends
unreachable. Their sum is the function's reservation, charged on every entry
([exact metering](#214-function-entry)). It also resolves every branch to the
position it continues at and the stack height it returns to, so the interpreter
never searches for an `end`.

### 18.3. Bounds

These limits keep a validator's allocation proportional to the module's bytes
and stay below the limits of common engines. They are far above what compilers
emit and are not capacity limits:

| Bound | Value |
|---|---:|
| Module size | 2^30 bytes |
| Parameters of one type | 1,000 |
| Parameters plus locals of one function | 16,384 |
| Reserved slots of one function (parameters, locals and maximum operand height) | 32,768 |
| Functions, including imports | 1,000,000 |
| Types | 1,000,000 |
| Globals | 1,000,000 |
| Element segments, data segments | 100,000 each |
| Memory maximum | 65,536 pages |

Nesting depth is unbounded, and a single function body is bounded only by the
module size. Every declared count is checked against its section's own remaining
length before an array is allocated for it or a validator that may allocate one
is run.

### 18.4. Outcome

Validation either accepts the module, yielding its profile (type count, function
reservations, import count, entry function, initial memory, globals, data segments,
data bytes, nonempty data-segment count and table size), or rejects it. A rejected deployment fails with status 9,
deployment module invalid, after paying its [code price](#157-code),
and installs nothing. A validator's work can grow with the module's bytes and its
declared functions and segments, never with declared counts that cost no bytes,
such as the locals a function declares with one entry; the reference decoder and
validator is a single pass. Acceptance is exercised by the seed's validation section
and by the structural vectors ([conformance](node/wiki/reference/conformance.md)).

## 19. Instruction reference

The 115 accepted instructions, by opcode byte. "Stack" shows what an instruction
pops and pushes, bottom to top, with `[t]` for a value that depends on the label
or function. Every instruction except `else` and `end` costs its price from
[the price list](#15-the-price-list) before it executes
([exact metering](#21-exact-metering)). Values are `i32` and `i64`. The
definitions below, together with the typing algorithm of section 25 and the
metering rules of section 21, are sufficient to implement the accepted subset. No
unlisted external feature is enabled.

### 19.1. Control

| Byte | Instruction | Immediates | Stack | Meaning |
|---|---|---|---|---|
| `00` | `unreachable` | | | Traps. |
| `01` | `nop` | | | Does nothing. |
| `02` | `block` | block type | | Opens a region; a branch to it continues after its `end`. |
| `03` | `loop` | block type | | Opens a region; a branch to it continues at its first instruction. |
| `04` | `if` | block type | `i32 →` | Opens a region and runs the then-branch for a non-zero condition, otherwise the else-branch or nothing. |
| `05` | `else` | | | Separates the branches of an `if`; free. |
| `0b` | `end` | | | Closes a region, or the function; free. |
| `0c` | `br` | depth | `[t] →` | Branches to the depth-th enclosing label, carrying its value if it has one. |
| `0d` | `br_if` | depth | `[t] i32 → [t]` | Branches when the condition is non-zero; otherwise leaves the value in place. |
| `0e` | `br_table` | depths, default | `[t] i32 →` | Branches to the label at the unsigned index, or to the default when the index is out of range. |
| `0f` | `return` | | `[t] →` | Leaves the function with its result, if any. |
| `10` | `call` | function | `params → [result]` | Calls a function; a host import runs its host rule. |
| `11` | `call_indirect` | type, `00` | `params i32 → [result]` | Calls the entry of table 0 at the unsigned index; traps on an index outside the table, a null entry, or an entry whose type differs structurally (in its parameter and result types, not its type index) from the expected type. Otherwise exactly like `call`. |

### 19.2. Parametric and variables

| Byte | Instruction | Immediates | Stack | Meaning |
|---|---|---|---|---|
| `1a` | `drop` | | `t →` | Discards a value. |
| `1b` | `select` | | `t t i32 → t` | Keeps the first value when the condition is non-zero, the second otherwise; all three are evaluated. |
| `20` | `local.get` | local | `→ t` | Reads a parameter or local. |
| `21` | `local.set` | local | `t →` | Writes a parameter or local. |
| `22` | `local.tee` | local | `t → t` | Writes and keeps the value. |
| `23` | `global.get` | global | `→ t` | Reads a global. |
| `24` | `global.set` | global | `t →` | Writes a mutable global. |

### 19.3. Memory

Memory is a byte array of 64 KiB pages. Each access computes its effective
address as the unsigned 32-bit address plus the unsigned 32-bit offset, in
unbounded arithmetic, and traps when the address plus the access width exceeds
the current memory size: an address never wraps around into valid memory, and a
store that traps writes nothing. Bytes are little-endian. Narrow signed loads
sign-extend, narrow unsigned loads zero-extend, and narrow stores keep the low
bytes. Alignment immediates are hints and never change behavior.

| Byte | Instruction | Stack | Meaning |
|---|---|---|---|
| `28` | `i32.load` | `i32 → i32` | 4 bytes. |
| `29` | `i64.load` | `i32 → i64` | 8 bytes. |
| `2c` | `i32.load8_s` | `i32 → i32` | 1 byte, sign-extended. |
| `2d` | `i32.load8_u` | `i32 → i32` | 1 byte, zero-extended. |
| `2e` | `i32.load16_s` | `i32 → i32` | 2 bytes, sign-extended. |
| `2f` | `i32.load16_u` | `i32 → i32` | 2 bytes, zero-extended. |
| `30` | `i64.load8_s` | `i32 → i64` | 1 byte, sign-extended. |
| `31` | `i64.load8_u` | `i32 → i64` | 1 byte, zero-extended. |
| `32` | `i64.load16_s` | `i32 → i64` | 2 bytes, sign-extended. |
| `33` | `i64.load16_u` | `i32 → i64` | 2 bytes, zero-extended. |
| `34` | `i64.load32_s` | `i32 → i64` | 4 bytes, sign-extended. |
| `35` | `i64.load32_u` | `i32 → i64` | 4 bytes, zero-extended. |
| `36` | `i32.store` | `i32 i32 →` | 4 bytes. |
| `37` | `i64.store` | `i32 i64 →` | 8 bytes. |
| `3a` | `i32.store8` | `i32 i32 →` | The low byte. |
| `3b` | `i32.store16` | `i32 i32 →` | The low 2 bytes. |
| `3c` | `i64.store8` | `i32 i64 →` | The low byte. |
| `3d` | `i64.store16` | `i32 i64 →` | The low 2 bytes. |
| `3e` | `i64.store32` | `i32 i64 →` | The low 4 bytes. |
| `3f` | `memory.size` | `→ i32` | The current size in pages. |
| `40` | `memory.grow` | `i32 → i32` | Appends that many zeroed pages and yields the previous size, or yields −1 with no effect when the new size would exceed the declared maximum. Growth by zero pages succeeds and yields the current size. Charges 40,000 C per new page; running out of compute there fails the invocation rather than yielding −1. |
| `fc 0a` | `memory.copy` | `i32 i32 i32 →` | Destination, source, length `n`: moves `n` bytes as if the source were first copied aside, so overlapping ranges move correctly. Charges `bytes(n, 2)` when n is nonzero and `bytes(0, 0)` otherwise, then traps before writing anything if any part of either range lies outside memory. |
| `fc 0b` | `memory.fill` | `i32 i32 i32 →` | Destination, value, length `n`: writes the low byte of the value `n` times. Charges `bytes(n, 1)` when n is nonzero and `bytes(0, 0)` otherwise, then checks the destination bounds before writing. |

A zero-length copy or fill whose addresses are at or below the end of memory is
in bounds.

### 19.4. Constants, tests and comparisons

| Byte | Instruction | Immediates | Stack |
|---|---|---|---|
| `41` | `i32.const` | signed LEB128 | `→ i32` |
| `42` | `i64.const` | signed LEB128 | `→ i64` |
| `45` | `i32.eqz` | | `i32 → i32` |
| `46`–`4f` | `i32.eq`, `ne`, `lt_s`, `lt_u`, `gt_s`, `gt_u`, `le_s`, `le_u`, `ge_s`, `ge_u` | | `i32 i32 → i32` |
| `50` | `i64.eqz` | | `i64 → i32` |
| `51`–`5a` | `i64.eq`, `ne`, `lt_s`, `lt_u`, `gt_s`, `gt_u`, `le_s`, `le_u`, `ge_s`, `ge_u` | | `i64 i64 → i32` |

Results are 1 or 0. `_s` compares two's-complement signed values, `_u` unsigned.
`eqz` tests for zero.

### 19.5. Arithmetic and bits

| i32 | i64 | Instruction | Meaning |
|---|---|---|---|
| `67` | `79` | `clz` | Leading zero bits; the width for zero. |
| `68` | `7a` | `ctz` | Trailing zero bits; the width for zero. |
| `69` | `7b` | `popcnt` | Set bits. |
| `6a` | `7c` | `add` | Wrapping. |
| `6b` | `7d` | `sub` | Wrapping. |
| `6c` | `7e` | `mul` | Wrapping, the low bits of the product. |
| `6d` | `7f` | `div_s` | Signed, truncating toward zero; traps on a zero divisor and on the minimum value divided by −1. |
| `6e` | `80` | `div_u` | Unsigned; traps on a zero divisor. |
| `6f` | `81` | `rem_s` | Signed remainder with the dividend's sign; traps on a zero divisor; the minimum value by −1 is 0. |
| `70` | `82` | `rem_u` | Unsigned; traps on a zero divisor. |
| `71` | `83` | `and` | Bitwise. |
| `72` | `84` | `or` | Bitwise. |
| `73` | `85` | `xor` | Bitwise. |
| `74` | `86` | `shl` | Shift left by the count modulo the width. |
| `75` | `87` | `shr_s` | Arithmetic shift right by the count modulo the width. |
| `76` | `88` | `shr_u` | Logical shift right by the count modulo the width. |
| `77` | `89` | `rotl` | Rotate left by the count modulo the width. |
| `78` | `8a` | `rotr` | Rotate right by the count modulo the width. |

Unary bit counts have stack `t → t`; arithmetic, shifts and bitwise operations have stack `t t → t`, with t the instruction's width. Values are bit patterns of their declared width; ordinary add/subtract/multiply and left shift retain only that many low bits. Binary operators take the left operand first: `a b sub` is a − b, and the shift or
rotation count is the second operand. Shifts and rotations use only the low 5 bits
of the count for `i32` and the low 6 bits for `i64`. Unsigned division and
remainder are the ordinary unsigned operations.

### 19.6. Conversions

| Byte | Instruction | Stack | Meaning |
|---|---|---|---|
| `a7` | `i32.wrap_i64` | `i64 → i32` | The low 32 bits. |
| `ac` | `i64.extend_i32_s` | `i32 → i64` | Sign-extended. |
| `ad` | `i64.extend_i32_u` | `i32 → i64` | Zero-extended. |
| `c0` | `i32.extend8_s` | `i32 → i32` | The low 8 bits, sign-extended. |
| `c1` | `i32.extend16_s` | `i32 → i32` | The low 16 bits, sign-extended. |
| `c2` | `i64.extend8_s` | `i64 → i64` | The low 8 bits, sign-extended. |
| `c3` | `i64.extend16_s` | `i64 → i64` | The low 16 bits, sign-extended. |
| `c4` | `i64.extend32_s` | `i64 → i64` | The low 32 bits, sign-extended. |

### 19.7. Wide arithmetic

Four instructions compute with 128-bit integers held as two `i64` values, the low
half below the high half on the stack. They are the four instructions of the
WebAssembly wide-arithmetic proposal, with its opcodes and meaning.

| Bytes | Instruction | Stack | Meaning |
|---|---|---|---|
| `fc 13` | `i64.add128` | `i64 i64 i64 i64 → i64 i64` | Left low, left high, right low, right high: the low and high halves of the sum modulo 2^128. |
| `fc 14` | `i64.sub128` | `i64 i64 i64 i64 → i64 i64` | The same operands: the halves of left minus right modulo 2^128. |
| `fc 15` | `i64.mul_wide_s` | `i64 i64 → i64 i64` | The low and high halves of the full 128-bit product of two signed values. |
| `fc 16` | `i64.mul_wide_u` | `i64 i64 → i64 i64` | The same for two unsigned values. |

None of them traps. Each pushes two values, so it counts twice toward a function's
maximum operand height.

### 19.8. Traps

A trap is a deterministic guest outcome: it ends the invocation with status
2 and no result, after the instruction's own dispatch charge and any byte
charge it made. The causes are
`unreachable`, a zero divisor, the signed-division overflow, an out-of-bounds
memory access, a bulk operation out of bounds, and a bad indirect call. Host
rules add their own causes ([host interface](#22-the-host-interface)). All traps are the
same status; nothing distinguishes them in a receipt.

### 19.9. Everything else

No other opcode byte and no other `fc` sub-opcode is accepted. That excludes
floating point, SIMD, typed `select`, reference types, table instructions,
`memory.init` and `data.drop`, saturating truncation, exceptions, threads and
every other proposal.

## 20. The abstract machine

This section defines how a program runs: what state exists, how instructions act
on it, and what a node observes. The reference interpreter is one implementation
of this machine; any implementation that produces the same outcomes and counters
is correct.

### 20.1. Values and the stack

The only values are i32 and i64, held in 64-bit slots. One stack per invocation
holds, for every active function, its parameters, then its declared locals, then
its operands. A call takes its arguments from the top of the caller's operands and
makes them the callee's first locals; declared locals start at zero. A return
copies the result to where the callee's slots began and discards the rest. The
slots a function can use are the ones its reservation paid for; with the record
each active function keeps, they make up the stack's height, whose growth is paid
for as memory's is ([function entry](#214-function-entry)).

### 20.2. Labels and branches

`block`, `loop` and `if` open regions; each region is a label a branch can
target, counted outward from the innermost. A branch to the n-th enclosing
region cuts the operand stack back to the height the region had when it opened
and continues:

- after the region's `end` for a `block` or an `if`, carrying the region's result
  if it declares one;
- at the first instruction of the body for a `loop`, carrying nothing, which is
  how loops repeat. The `loop` instruction itself is not executed again.

Falling off the end of a region is not a branch; its result is simply left on the
stack. `return`, and the function's final `end`, leave the function the same way a
branch to its outermost label would.

### 20.3. Instances

An invocation runs a fresh **instance** of the contract's module: globals set to
their initializers, the table filled from the element segments and never changed
afterwards, and memory of the declared initial size, zeroed and then overlaid with
the data segments. Memory can grow up to the declared maximum. Nothing of an
instance survives the invocation; two invocations of one contract in one
transaction do not share memory or globals.

### 20.4. Host calls

A call to one of the twelve imports leaves the machine and runs the host rule
([host interface](#22-the-host-interface)). Most return a value and execution continues.
`return` and `revert` end the invocation. `call` and `deploy` **suspend** it when
they run a child: the machine's whole state is kept, a new instance runs the child to
completion, and then the parent continues with the child's status as the import's
result. When no child runs, because the sub-budget does not fit (128) or the child
cannot pay for receiving its input or code (3), the result comes back at once. The
runtime drives this with an explicit loop, so nesting never depends on a node's
native stack, and a parent's state is not visible to its child except through
storage ([calls and rollback](#24-calls-rollback-and-read-only)).

### 20.5. Ending

An invocation ends in one of these ways, each a deterministic outcome:

| How | Status | Result |
|---|---|---|
| `return` | 0, success | the returned bytes |
| falling off the end of `invoke` | 0, success | empty |
| `revert` | 1, revert | the returned bytes |
| a trap | 2, trap | empty |
| a budget running out | 3, 4, 5 or 6, for C, S, D or G | empty |
| breaking a host rule | 11 for a write in read-only mode, otherwise 2 | empty |

A failed invocation's state changes and events are rolled back to its start; its
consumed resources remain charged.

### 20.6. Determinism

The machine has no clock, no randomness, no addresses of host memory and no
dependence on how a node allocates. A trap is a deterministic guest outcome, and
nothing else may stand in for one: a node's allocation, compiler or backend
failure is a failure of that node and is never turned into a guest outcome.
Memory growth that a node cannot satisfy is such a node failure: the budget
already paid for every byte the guest may touch, so a node that cannot provide
it is misconfigured for the block's capacity.

A conforming implementation must represent every legal index, byte count and live aggregate without accidental narrowing. A guest has 32-bit addresses; aggregate state, suspended instances and execution stacks are not thereby capped at 2^32 bytes or slots. A host platform limitation is not a guest trap.

## 21. Exact metering

Every node has to reach exactly the same counters, so Ting defines precisely
when each unit is charged, not just roughly how expensive an instruction is.
This section fixes the compute (C) charges of running a program and the exact point
of each debit; [what each counter measures](#12-what-each-counter-measures) lists every
counted event across C, S, D and G.

### 21.1. Prices

Every amount in this section is a price from [the price list](#15-the-price-list):
each instruction's price, the load, store, word and span prices of the executing
instance's memory size, the page price, the hashing prices, the start price and the
code price. `bytes(n, s)` is the price of moving `n` bytes in `s` spans at the
current memory size ([memory prices](#152-memory-prices)); a span
that holds no byte is not counted. A debit of zero is permitted and has no effect.

### 21.2. Where each charge is made

| Event | C | When |
|---|---:|---|
| Root dispatch of a transaction | 1 | once, before any top-level call or deployment work, whatever the program |
| Validating deployment code | the code price plus the price of hashing the CodeHash | before validation and before any cache lookup |
| Loading code for an invocation | the code price, unless the transaction has prepared this CodeHash | after reading the manifest and the code record, even on a cache hit |
| Preparing an instance | see below | in one debit, before the instance exists |
| Entering `invoke` | its reservation | after preparation, before the first instruction |
| Dispatching an original instruction | its price | immediately before it executes |
| Entering a defined function by `call` or `call_indirect` | its reservation | after the call's dispatch and, for an indirect call, its table and type checks |
| Reaching a host import through `call_indirect` | 94 | after the indirect call's dispatch and its table and type checks |
| `memory.copy`, `memory.fill` of `n` bytes | `bytes(n, s)`, over the source and destination of a copy or the destination of a fill | after dispatch, before the bounds check |
| `memory.grow` adding `p` pages | 40,000 × `p` | after dispatch and the maximum check, before growing |
| Host copies, record reads and writes, results, events | per [the host interface](#22-the-host-interface) | at the steps listed there |
| Producing a deployed ContractId | 32 | after the initializer succeeds, in one debit with 32 D |

Instance preparation is one atomic debit covering the start of the instance, its
initial memory, its globals and table, and copying the data segments, with nothing
extra for the `const` and `end` of initializers:

```text
C = 40,000 + 40,000 × initial_pages + 40 × global_count + 4 × table_size
  + 12 × active_data_segments + bytes(data_bytes, data_spans)
```

`data_bytes` is the sum of the lengths of all active data segments and `data_spans`
the number of them that hold at least one byte, priced at the initial memory's size.

### 21.3. The price of an executed instruction

An instruction's price is charged immediately before it executes. Skipped
instructions cost nothing, and `else` and `end` are structural delimiters that
cost nothing, so the function's final `end` and its implicit return are free.
Entering a `block`, `loop` or `if` through its instruction costs its price;
branching back to a loop continues at its body without paying for the `loop`
again, while the branch instruction and every repeated instruction of the body are
charged. A load or store is priced by the memory's size at that moment.
A `call` is priced by its target: a defined function or a host import.

The charge comes before the instruction can trap. If the price does not fit, the
instruction does not execute, nothing of its price is charged, and the invocation
fails with status 3. `memory.copy` and `memory.fill` make two debits
before they execute: first their price, then, in one atomic debit, `bytes(n, s)`
for the `n` bytes their length operand names, over two spans for a copy and one for
a fill when `n` is not 0. Only then do they execute: an out-of-bounds range traps after both debits, and running out
of compute at the byte debit fails the invocation before any byte is written.

### 21.4. Function entry

Entering a defined function reserves and charges one C per slot it can use, and
the [stack](#154-stack) it adds beyond the invocation's mark:

```text
reservation = parameters + declared_locals + maximum_typed_operand_height
height      = height before entry + reservation + 3
debit       = reservation + 40,000 × (stack_pages(max(height, mark)) − stack_pages(mark))
mark        = max(height, mark)
```

The 3 slots are the record every active function keeps. The invocation's height
and mark start at zero; entering its entry function is the first entry, so the
entry function's reservation and stack pages are debited the same way, at the
point the table above gives. Leaving a function lowers the height by its
reservation plus 3 and leaves the mark.

The maximum operand height is the validator's, unreachable regions included
([validation](#182-measured-for-execution)). Reservations, heights
and marks are static per-function maxima and their sums, not bytes of any engine's
stack. The reservation and the stack pages are one atomic debit, the same whether
the call is direct or indirect, and it comes after the
call's own price and, for an indirect call, after the table and type checks, so a
bad indirect call pays its price only. Calling an imported host function reserves
nothing; it pays the `call`'s price for an import and then its host charges. An
indirect call that reaches an import pays the indirect call's price, then, after
the table and type checks, 94 C for the host call, then its host charges.

There is no cap on frames, slots or nesting: each level costs its call, its slots
and the stack pages it adds, so the stack a transaction can build is bounded by its
compute budget as its memory is, and a node knows from the block's capacity how much
stack a block can need.

### 21.5. Memory growth

`memory.grow` pays its price first. If the module's maximum forbids the growth, it
returns −1, as WebAssembly specifies, and nothing more is charged. Otherwise it
charges 40,000 C per new page and then grows and zeroes them; running out of compute
at that point fails the invocation with status 3 rather than returning −1. Growing
by zero pages has no page debit. There is no separate memory cap, per instance or
across them: suspended parents and the running child have each paid for their
memory in compute, so a program can grow as far as its module declares and its
budget pays for. At 40,000 C per 64 KiB page, of memory or of stack, a
transaction's live guest memory and stack together never exceed about 1.6 bytes per
unit of its C budget, beyond the 64 KiB of stack each invocation's start price
covers. An implementation that keeps
its own copy of each nested call's input or code, as the reference node does, holds
at most as much again, since every such copy comes from a caller's paid memory.

### 21.6. Block limits during execution

A validator may stop executing as soon as an otherwise permitted debit would push
the block's totals past an active capacity ([block totals](#141-block-totals)).
The transaction's own budget and its frames are checked first, so a debit the
transaction could not afford never counts toward the block. Stopping is final: it
rejects the whole block, and no cancellation receipt or adjusted transaction
outcome can be accepted in its place. A revert cannot make an exceeded total fit,
because the counters only grow. Executing a transaction on its own, and every
accepted block, keep the transaction-local behavior these rules describe.

## 22. The host interface

A program talks to the chain only through twelve functions imported from module
`ting`. Every argument is a 32-bit integer; byte strings are passed as a pointer
and a length into the program's memory. The interface adds no cryptographic
verification, no money transfers and no fees: those are programs' business.

| Purpose | Imports |
|---|---|
| Read invocation data | `input`, `context` |
| Use its own storage | `storage_get`, `storage_set`, `storage_delete` |
| Learn about other contracts | `code_hash` |
| Call or create contracts | `call`, `deploy`, `call_result` |
| Produce output | `emit`, `return`, `revert` |

### 22.1. Conventions

- **Spans are checked.** Every pointer and length is checked against the exported
  memory before anything is copied. An out-of-range span traps the invocation.
- **Getters report the size.** `input`, `context`, `storage_get`, `code_hash` and
  `call_result` take an output buffer and return the full length of the value, as an
  unsigned 32-bit number. If the
  buffer is too small, nothing is copied and no copy is charged, so the program can
  call again with a larger buffer.
- **Absent is not empty.** `storage_get` returns `2^32 − 1`, every bit set (−1 read
  as signed), for an absent record, and 0 for a present empty one. `code_hash` returns
  `2^32 − 1` for a contract that does not exist. No record is that long: a record
  value is at most `2^32 − 2` bytes ([storage writes](#225-storage-writes)).
- **Bytes are opaque.** Input, results, events and storage values have no protocol
  format. Each program defines its own encodings.
- **Bytes cost compute.** Every copy across the interface costs `bytes(n, s)`
  ([memory prices](#152-memory-prices)): the word price of the
  program's memory size for each started 32 bytes, and the span price for each span
  of at least one byte that the copy reads or writes, which above 2 MiB is that of a
  store.
- **Records cost compute too.** Every record read is debited together with 1,900 C
  for hashing the record's key, and every value written with the price of hashing it
  into its leaf ([hashing](#155-hashing)).

### 22.2. Signatures

Every parameter and every ordinary result is `i32`. An import's type must match
its row exactly ([the binary format](#173-sections-in-detail)).

| Import | Parameters | Result |
|---|---|---|
| `input` | out_ptr, capacity | required length |
| `context` | selector, out_ptr, capacity | required length |
| `storage_get` | key_ptr, out_ptr, capacity | length, or `2^32 − 1` when absent |
| `storage_set` | key_ptr, value_ptr, value_len | none |
| `storage_delete` | key_ptr | none |
| `code_hash` | target_ptr, out_ptr, capacity | 32, or `2^32 − 1` when absent |
| `call` | target_ptr, input_ptr, input_len, budget_ptr, flags | child status, or 128 |
| `deploy` | code_ptr, code_len, input_ptr, input_len, budget_ptr | child status, or 128 |
| `call_result` | out_ptr, capacity | length of the last child result |
| `emit` | payload_ptr, payload_len | none |
| `return` | result_ptr, result_len | none |
| `revert` | result_ptr, result_len | none |

`return` and `revert` never come back to the program, although their type has
no result. Keys and targets are 32-byte spans. A budget is a 32-byte span holding
four little-endian u64 fields in C, S, D, G order. Call flags are 0 for an
ordinary call and 1 for a [read-only](#243-read-only-mode) one.

### 22.3. Spans and copying

- Pointers, lengths, capacities and selectors are read as unsigned 32-bit values, and
  a length a getter returns is one too. A program's memory is at most 4 GiB, so no
  span, and no value a program writes or receives, is longer than `2^32 − 1` bytes.
- A span of `ptr` and `len` is valid when the module exports a memory and
  `ptr + len`, computed in mathematical addition without wrapping, is at most the
  memory's current size. A zero-length span still needs an exported memory and an
  end in range.
- An operation with several argument spans validates all of them before it
  charges or copies any argument byte. It then debits `bytes(total length, s)` C,
  `s` being the argument spans that hold a byte, in one atomic step and copies. `call` and `deploy` are the exception: the caller
  pays only for the target and the budget, and the child pays for receiving its
  input and code (below).
- Copying `n` bytes back into guest memory debits `bytes(n, s)` C, one span when `n`
  is not 0.
- A debit written `[c, s, d, g]` charges the four counters C, S, D and G in one
  atomic step.

### 22.4. Getters

`input`, `context`, `storage_get`, `code_hash` and `call_result`:

1. validate the whole output span, `out_ptr` and `capacity`, even when the value
   turns out to be absent or shorter;
2. find the value, with the import's own argument copies and reads (below);
3. if the value fits the capacity, debit `bytes(length, s)` C and copy it; if not,
   copy nothing and charge no output copy;
4. return the value's full length, or `2^32 − 1` for an absent record or contract.

A getter validates its span and debits its copy before any byte moves, and
copies straight from the host's own data, so a length query or a short buffer
does no work in proportion to the value. Calling a getter again repeats its
charges; nothing is discounted for having been read before.

`storage_get` validates the output span, then validates and copies its 32-byte
key (`bytes(32, 1)`), then reads the record in the executing contract's storage
([namespaces](#53-namespaces)), debiting `[1,900, 1, 0, 0]` for the
key and the read, then copies the value as above. The read needs only the record's
length until the value is copied. `code_hash` does the same with a 32-byte target,
reading that contract's manifest; when the contract exists it copies its 32-byte
CodeHash.

### 22.5. Storage writes

`storage_set` sets a record of any length; `storage_delete` deletes one. Both:

1. fail with status 11 in a read-only invocation, before any span check;
2. validate the key span and, for `storage_set`, the value span, which must also be
   shorter than `2^32 − 1` bytes, then debit `bytes(32 + value_len, s)` C, two spans
   or one for an empty value, or `bytes(32, 1)` for a deletion, and copy;
3. debit `[1,900, 1, 0, 0]` for the key and the read of the old record, of which
   only the length is needed;
4. debit, in one atomic step,

   ```text
   [ C = 100 + 900 × ceil((len(value) + 62) / 64), S = 1, D = 0, G = growth ]
   ```

   the price of hashing the value into its leaf, with no hash and zero growth for a
   deletion ([growth](#13-persistent-growth));
5. apply the change.

Both S units are charged for a write that changes nothing and for deleting an
absent record. The key is 32 bytes and the value at most `2^32 − 2` bytes, one
fewer than memory allows, so that no record's length can be read as absence; a
value span of `2^32 − 1` bytes traps like an invalid span. There is no other size
limit. The runtime's own writes of a
manifest and a code record during deployment follow the same rule.

### 22.6. Output

`return` and `revert` validate their span, debit `[bytes(n, s), 0, n, 0]` atomically
for `n` result bytes, copy the bytes and end the invocation, with status Success
or Revert. `revert` rolls back the invocation's effects
([checkpoints](#242-checkpoints)); the bytes are its result either way.

`emit` fails with status 11 in a read-only invocation, then validates its span,
debits `[bytes(n, s), 0, 36 + n, 0]` atomically and appends an event holding the
executing ContractId and the `n` payload bytes ([events](#46-events)).

### 22.7. Calls and deployments

`call`:

1. traps unless flags is 0 or 1;
2. validates the target, input and budget spans, debits `bytes(32 + 32, 2)` C and
   copies the target and the budget;
3. empties the last-result buffer;
4. returns 128 if the requested budget exceeds, in any counter, what the caller
   has left; no child is created;
5. otherwise opens the child with that budget and debits `bytes(input_len, s)` C,
   at the caller's memory prices, as the child's first debit. If it does not fit, the
   child ends with status 3 having consumed nothing, and `call` returns 3;
6. otherwise copies the input and runs the child, read-only when flags is 1 or the
   caller is itself read-only, and returns the child's status, keeping its result
   bytes in the last-result buffer.

`deploy`:

1. fails with status 11 in a read-only invocation, before any span check;
2. validates the code, input and budget spans, debits `bytes(32, 1)` C and copies
   the budget;
3. empties the last-result buffer;
4. returns 128 if the requested budget does not fit, as for `call`; no ordinal
   is taken and nothing is installed;
5. otherwise opens the deployment with that budget and debits
   `bytes(code_len + input_len, s)` C, `s` being the code and input spans that hold
   a byte, at the caller's memory prices, as its first debit. If it does not fit, the deployment ends with status 3 having consumed
   nothing, before taking an ordinal, and `deploy` returns 3;
6. otherwise copies the code and the input and runs the
   [deployment](#231-deployment) with that budget and returns its
   status. On success the last-result buffer holds the new 32-byte ContractId; on
   failure, the failed initializer's result bytes, or nothing when the deployment
   failed before its initializer ran or after it succeeded, at the identifier debit.

A child pays for receiving its own input and code, so a caller that hands on
everything it has left, less a fixed reserve for its own work afterwards, can pass
an input of any size: the copy comes out of the child's share, never out of the
reserve.

The child's status is a receipt status code ([receipts](#45-receipts)).
128 is only ever a host return value, never a receipt status. The last-result
buffer is empty when an invocation starts and is replaced by every `call` or
`deploy` attempt. `call_result` is a getter over it: reading it costs copy C but
no second D, and returning it from the parent charges the parent's ordinary
result D. What the child consumed stays charged to every enclosing budget, and
what it left unused is available to the caller again
([calls and rollback](#24-calls-rollback-and-read-only)).

### 22.8. Context

`context` copies one field, chosen by a selector:

| Selector | Field | Encoding |
|---:|---|---|
| 0 | ChainId | 32 bytes |
| 1 | TransactionId | 32 bytes |
| 2 | Executing ContractId | 32 bytes |
| 3 | Calling ContractId | 33 bytes: tag 0 and 32 zero bytes when there is none, or tag 1 and the identifier |
| 4 | Height | u64, little-endian |
| 5 | Block timestamp | u64, little-endian |
| 6 | Parent BlockId | 32 bytes |
| 7 | Producer data | its original bytes, exactly as the block body carries them ([the body](#43-the-body)) |
| 8 | Remaining budget | four u64, little-endian, C/S/D/G, of the executing frame |

Any other selector traps. A top-level invocation has no calling contract; an
initializer's calling contract is its deployer, or none for a top-level
deployment.

Selector 8 reports the executing frame's budget minus its consumption at the
moment the getter runs, before the getter's own copy debit. It lets a program
give a child exactly what it can afford instead of guessing, and it is
deterministic, since it follows from the declared budget and the consumption so
far. A program that forwards it to a child must first subtract what it will
spend before the call.

The current block's own ID, its nonce, the current state root, the wall clock,
local resource timings and other contracts' storage are deliberately unavailable.
A candidate block's execution therefore does not depend on the nonce search or on
the block's eventual hash.

### 22.9. Code hashes

`code_hash` takes a ContractId and returns that contract's CodeHash: `bytes(32, 1)`
for copying the target, 1,900 C and one S for reading its manifest, and `bytes(32, 1)` to
copy the hash out when the buffer holds it. A program uses it to verify which code another contract
runs, for example to accept only known implementations before calling or trusting
one.

### 22.10. Failures and their order

| Failure | When it is checked | Outcome |
|---|---|---|
| A writing operation in read-only mode | before any span check | the executing invocation fails with status 11 |
| An invalid span, a stored value of `2^32 − 1` bytes, an unknown selector or an invalid call flag | before any charge for the spans | the executing invocation traps, status 2 |
| A debit that does not fit | at that debit | the invocation fails with the first exhausted counter in C, S, D, G order ([atomic debits](#112-atomic-debits)) |
| A sub-budget larger than what is left | after copying the target and the budget | `call` or `deploy` returns 128; the caller continues |
| A child that cannot pay for receiving its input or code | at that debit, the child's first | the child ends with status 3 having consumed nothing; the caller continues |

The first three end the invocation that made the host call, without a result;
its caller receives the status and continues. Besides the instruction traps
([traps](#198-traps)), invalid spans, a stored value of `2^32 − 1`
bytes, unknown selectors and invalid call flags are the host's only causes of a trap.

## 23. Deploy and invoke

A deployment installs immutable code and runs it once as its initializer; an
invocation runs installed code. Both happen the same way whether a transaction
or another program asks for them, and both follow a fixed order of steps, each
with its debit, so that every node fails at the same step with the same
counters. Every debit below is atomic ([budgets](#112-atomic-debits)):
if it does not fit, nothing of it is charged and the step fails with the status
of the exhausted counter.

### 23.1. Deployment

A deployment from a program has first passed the `deploy` import's own checks:
read-only mode, the spans, the sub-budget, and receiving its code and input within
that sub-budget ([host interface](#227-calls-and-deployments)). Then, and
for a top-level deployment directly:

1. **Ordinal.** Take the transaction's current deployment ordinal, which starts at
   zero, and advance the counter. Rollback never restores it, so a failed
   deployment still uses its ordinal. An overflowing counter fails the deployment
   with status 7.
2. **Identifier.** Debit 1,900 C for deriving the ContractId from the chain, the
   transaction and the ordinal ([identifiers](#51-identifiers)),
   then read its manifest record, `[1,900, 1, 0, 0]`. If it exists, the deployment
   fails with status 10.
3. **Validation.** Debit the code's [code price](#157-code) plus
   the price of hashing it into its CodeHash, in one debit before validation and
   before any cache lookup, then [validate](#18-validation). Invalid code fails
   with status 9; the debit stays consumed. Valid code counts as prepared for the
   rest of the transaction.
4. **Installation.** Write the manifest, which records the code price debited in
   step 3, and the code record. Each write follows
   the storage-write rule ([host interface](#225-storage-writes)): 1,900 C
   and one S for the key and the read of the old record, then one atomic debit of
   the price of hashing the value into its leaf, one S and the record's
   [growth](#13-persistent-growth).
5. **Initializer.** Invoke the new contract as below, with the initial input, in
   ordinary mode, with the deploying contract as its caller, or none at the top
   level. It can write the new contract's storage, call other contracts and emit
   events.
6. **Identifier result.** When the initializer succeeds, debit 32 C and 32 D for
   producing the identifier. The deployment's result is the 32-byte ContractId and
   nothing else: the initializer's own result bytes are discarded, though they
   were charged as data when it returned them. A top-level deployment's receipt
   carries the ContractId as its result; a program reads it with `call_result`.

If any step fails, including the identifier debit, the installation and
everything the initializer and its descendants did are rolled back. The ordinal
stays used and every resource consumed stays charged. A failed initializer's
status and result bytes are the deployment's outcome, so a revert's bytes reach
the deployer; a failure before the initializer has an empty result.

### 23.2. Invocation

Before its top-level call or deployment, a transaction pays its data base
([counters](#123-d-data)) and then one root dispatch C, the
same whatever the program does. Every run of a program, top-level or nested,
then works the same way:

1. **Code.** Read the manifest, `[1,900, 1, 0, 0]`; for a call to a contract that
   does not exist, the invocation ends here with status 8. Read the code record,
   `[1,900, 1, 0, 0]`. Then, unless the transaction has already prepared code with
   this CodeHash, debit its [code price](#157-code) for loading
   it, even when the node holds a compiled form; from then on the code counts as
   prepared for the rest of the transaction, whatever later fails. The price is the
   one the manifest records, and no byte of the code is read before it is paid: the
   code record's read needs only the length the manifest gives.
2. **Preparation.** Debit, in one atomic step before the instance exists,

   ```text
   C = 40,000 + 40,000 × initial_pages + 40 × global_count + 4 × table_size
     + 12 × active_data_segments + bytes(data_bytes, data_spans)
   ```

   `data_bytes` is the sum of the lengths of all active data segments and
   `data_spans` the number of them that hold at least one byte
   ([memory prices](#152-memory-prices)).

   which pays for starting the instance, its initial memory, setting the globals
   and the table, and copying the data segments
   ([invocations](#156-invocations)). The initializer expressions
   themselves cost nothing more. Then build the fresh instance: globals set to their constants,
   the table filled from the element segments, memory zeroed and the data
   segments copied in source order ([the abstract machine](#203-instances)).
3. **Entry.** Debit the reservation of `invoke` ([function entry](#214-function-entry)).
4. **Run.** Execute `invoke`. The program reads its input and context through host
   functions and ends with `return`, with `revert`, with a trap, by exhausting a
   budget, or by simply finishing, which is a successful empty return.

Nothing survives an invocation except storage records, retained events and the
result. Globals, the table and memory are initialized from the immutable code
every time, so two calls to the same contract within one transaction each start
from scratch, and the only persistent program state is what the storage
functions write.

### 23.3. Installed code is always valid

Invalid deployable code is an ordinary deployment failure with its charges.
Installed code, by contrast, was validated when it was deployed, and its manifest
and code record always agree. Finding them otherwise is a violation of the state
invariant, a fault of the node that holds that state, and never an alternative
outcome of the VM.

### 23.4. Compiled code is a node's business

A node may compile and cache modules however it likes. A cache hit must produce
exactly the outcome and counters of a cold start, which is why validation, code
reads and code loading are charged even when the code is cached. Whether code is
prepared within a transaction is part of the transaction's own history, not of any
node's cache.

## 24. Calls, rollback and read-only

### 24.1. Calling another contract

`call` takes a target, input bytes, a sub-budget and a read-only flag. The child runs
a fresh instance of the target's code, sees every earlier successful write in the
transaction, and returns a status. Its result bytes wait in the caller's
**last-result buffer** until read with `call_result`; the next call or deployment
replaces them. A call to a contract that does not exist returns status 8.

The sub-budget may be no greater, in any counter, than what the caller has left once
the call has copied the target and the budget. If it is greater, `call` returns 128 and no
child is created; the caller can treat that like any other failed call and
continue. A program that wants to pass on everything it has can read its
remaining budget from the context and subtract what it will spend itself: the
call's copy of the target and the budget, and its own work afterwards. The child
pays for receiving its input out of its own budget, so the size of the input never
eats into what the caller kept back. What the child consumes counts against its
own budget and every enclosing one; what it leaves unused is available to the
caller again ([budgets within budgets](#111-budgets-within-budgets)).
The exact order of copying, checks and charges is on
[the host interface](#227-calls-and-deployments).

### 24.2. Checkpoints

Every invocation begins with a checkpoint of state and events.

- When it succeeds, its changes and events stay.
- When it fails, whether by revert, trap or exhaustion, everything since its
  checkpoint is undone, including the effects of children that had succeeded.
- The caller receives the failure status and the child's result bytes, and decides
  what to do. A failed child does not fail its caller. If the caller later fails
  itself, everything since its own checkpoint is undone, its children's retained
  effects included.

At the top level, a failed transaction restores the state from before the
transaction and drops all its events. Its receipt still records the failure
status, the top-level result bytes where there are any, such as a revert's, and
the resources consumed ([receipts](#45-receipts)). Consumed
resources stay charged at every level: rollback restores state, never counters.

### 24.3. Read-only mode

A read-only invocation can compute and read storage, but cannot set or delete records,
deploy code or emit events. Attempting any of these fails the invocation with
status 11 before any span is checked and before anything changes. Read-only mode
passes to every nested call, and a callee cannot switch it off: an inherited
restriction overrides a call made with flags 0. A caller can use it to query
another contract with the guarantee that the call changes nothing.

### 24.4. Depth

There is no fixed nesting limit. Every nested invocation reads two records and pays
for its own instance, so budgets bound how deep a transaction can go. Implementations
must run nested invocations without depending on the host processor's stack; the
reference runtime drives them with an explicit loop rather than recursion
([host calls](#204-host-calls)).

### 24.5. Reentrancy

A contract can be called again while an earlier invocation of it is still active.
Each invocation gets a fresh instance but sees the same transactional state.
Guarding against reentrancy, like authorization, is the program's job. No caller
identity is ever inferred from input: a program learns its immediate caller only
from the context.

### 24.6. Deploying from a program

`deploy` works like `call`, but installs new code first and runs it as its
initializer ([deployment](#231-deployment)). Installation is
transactional: a failed initializer rolls back the code, the manifest and every
effect of its descendants. When it succeeds, the new ContractId is available
through `call_result`.

### 24.7. Failures that are not outcomes

Only the guest outcomes above can appear in a receipt. Malformed transaction
framing is not one: it makes the block invalid ([transactions](#44-transactions)).
Invalid deployable code is an ordinary deployment failure, status 9, after its
charges. Installed code that is not valid would violate the state invariant and
is never an alternative VM result ([installed code](#233-installed-code-is-always-valid)).

## 25. Complete type-checking algorithm

This section replaces any need to consult a separate WebAssembly validation
algorithm. First decode the complete canonical module and check its shape and indices. Validate every defined function, including functions never invoked and regions that will not execute. Do not run the program to discover its reservation.

### 25.1. Types and validator state

An operand type is `i32`, `i64` or `unknown`. Unknown is an internal validation marker, not a guest value type and not an encoding. Each function has an operand-type stack V, initially empty, and a control-frame stack K, initially containing its implicit function frame. A control frame stores its kind (function, block, loop, if-then or if-else), optional result type, opening operand height, and unreachable flag. The function frame has height zero and unreachable=false. Function parameters followed by declared locals form the indexed local types; parameters are not initially operands.

Use these exact operations:

```text
pop_any():
    f = K.top
    if len(V) == f.height:
        if f.unreachable: return unknown without changing V
        otherwise: reject
    return V.pop()

pop_expected(t):
    actual = pop_any()
    reject if actual is neither unknown nor t
    return actual

pop_result(optional_t):
    if a type is present: pop_expected(t)

push_result(optional_t):
    if a type is present: V.push(t)

make_unreachable():
    truncate V to K.top.height
    K.top.unreachable = true

open_frame(kind, result):
    K.push(kind, result, height=len(V), unreachable=false)

close_frame():
    pop_result(K.top.result)
    reject unless len(V) == K.top.height
    return K.pop()

label(depth):
    reject unless depth < len(K)
    return K[len(K)-1-depth]

label_result(frame):
    none for a loop; otherwise frame.result
```

A newly opened nested frame starts reachable for typing purposes even when its parent frame is marked unreachable. Popping at the new frame's empty height must therefore reject until that frame itself becomes unreachable. Known values pushed in unreachable code still have their actual types and cannot satisfy mismatched operations. A stack pop never reaches below the current control frame's opening height.

### 25.2. Control instructions

Apply the following rules in binary instruction order. Every immediate must decode canonically even when the instruction is unreachable.

| Instruction | Validation operation |
|---|---|
| `unreachable` | `make_unreachable()` |
| `nop` | no stack change |
| `block`, `loop` | decode empty/i32/i64 block result; `open_frame(kind,result)` |
| `if` | decode result; pop i32 condition; open if-then frame |
| `else` | close the current frame, require its kind is if-then, then open if-else with the same result and opening height; no then-result is pushed into the else arm |
| `end` | close current frame; reject if it is if-then with a result (no else); push its result if any into V. Closing the function frame finishes the body; require no trailing body bytes |
| `br depth` | resolve label, pop its result if any, then make current frame unreachable |
| `br_if depth` | resolve label; pop i32 condition, pop the label's result if any, then push that declared result type back. Do not mark unreachable |
| `br_table` | decode target vector and default; pop i32 selector; resolve every label and require exactly equal optional label result types, including in unreachable code; pop that result if any, then make current frame unreachable |
| `return` | pop enclosing function's result if any; make current frame unreachable |
| `call f` | require f exists; pop declared parameter types in reverse order, then push declared result if any |
| `call_indirect type 0` | require type and table exist; pop i32 table index, then validate parameters/result as for call. Table entry existence and structural signature equality are runtime checks |

`else` is legal only once for its immediately open if-then frame. `end` cannot close a nonexistent frame. A loop's branch label has no result even when its normal completion does. The implicit function label is a valid branch target. An unconditional branch does not make surrounding frames unreachable; it marks only the current validation frame.

### 25.3. Other instruction types

For every ordinary typed stack row in the instruction tables, pop its inputs right-to-left using `pop_expected`, then push its declared output types left-to-right. Narrow loads and stores use the declared value width, not the memory access width. Constants push their known width. Integer comparisons always push i32; i64 bit counts push i64. Wide add/sub pop four i64s and push two i64s; wide multiplication pops two i64s and pushes two i64s.

Additional rules:

- `drop` uses `pop_any()` and pushes nothing.
- `select` pops an i32 condition, then two types with `pop_any()`. Reject two different known types. Push the known type if either is known, otherwise push one actual `unknown` marker onto V.
- Local accesses require an existing index. `local.get` pushes that local's known type; `local.set` pops it; `local.tee` pops it then pushes the known local type, even if the popped operand was unknown.
- Global accesses require an existing index. Get pushes its known type; set requires mutable and pops that type.
- All memory instructions require declared memory, including memory.size/grow and bulk operations. A memory access's alignment exponent must not exceed log2(access bytes). Its offset is a u32.
- Bulk copy/fill each pop three i32s and push nothing. memory.grow pops i32 and pushes i32; memory.size pushes i32.
- Host imports have their ordinary declared function signatures. In particular, a call to the non-returning `ting.return` or `ting.revert` is still typed as a call with no result, **not** as a validation-unreachable instruction. Their runtime termination does not authorize otherwise ill-typed following code.
- Signature equality means the complete ordered parameter types and optional result; duplicate equal function types at different indices are allowed.

### 25.4. Maximum operand height and reservation

After processing **each original instruction**, including `end`, update `maximum = max(maximum, len(V))`. Count each actual unknown marker as one slot, but synthetic pops at an unreachable frame's empty height do not add slots. The final function `end` pushes its declared result for this measurement even though the function control frame has just closed. Thus an unreachable function returning i64 still has maximum at least one. All arms and all unreachable regions contribute.

`reservation = parameter_count + declared_local_count + maximum`. Reject when parameters plus locals exceed 16,384, or reservation exceeds 32,768. Local groups are canonical independently of the parameter list: the first declared local group may have the same type as the final parameter. Duplicate function-type entries and equivalent programs with different valid instruction sequences are not deduplicated or normalized. Canonical encoding provides one encoding of a given construct, not a unique encoding of every semantically equivalent program.

These reservations are consensus data used by function-entry metering. A compiler optimization, actual runtime operand height or a different unreachable-code analysis must not reduce them.

## 26. Exact execution algorithms and boundary clarifications

### 26.1. Code-price scan, including malformed bytes

The code-price scan is deliberately weaker than the validating decoder. It does not check magic, version, section order, allowed sections, minimum LEB representation, function typing or whether declared entries exist. Never use a validating parser's early rejection as the price of invalid code.

`read_price_u32` consumes at most five LEB128 bytes. It succeeds if a terminating byte appears within those five and the accumulated unsigned value fits u32; redundant zero groups are allowed. It fails on truncation, a continuation in byte five, or value overflow. This permissiveness applies only to pricing; canonical module validation still rejects nonminimal integers.

```text
code_price(code):
    total = 14400
    at = min(8, len(code))
    total += 60 * at                         // even an invalid or short preamble
    while at < len(code):
        start = at
        id = code[at]; at += 1
        try size = read_price_u32(code, at), advancing at
        if failure or at + size > len(code):
            total += 60 * (len(code) - start)
            stop
        payload = code[at : at+size]
        rate = 175 if id==9 else 100 if id==6 else 6 if id==11 else 60
        total += 60*(at-start) + rate*size
        if id==3 or id==11:
            count = read_price_u32(payload, 0), or 0 on failure
            total += (1215 if id==3 else 446) * count
        at += size
    return total
```

An empty complete data section still gets its content rate and readable declared count. Repeated sections each contribute, including repeated function/data count addends. If a section overruns, its id, length bytes and all remaining bytes get the default rate and its count contributes nothing. Later apparent sections inside that tail are never rescanned. Exact mathematical prices may exceed u64. Refuse an unaffordable combined deployment price atomically; never wrap it or partially consume it. Saturating at u64 max is equivalent only when it is already proved that every such debit is unaffordable at this stage (the earlier root or host work leaves less than u64 max remaining).

### 26.2. Top-level transaction algorithm

```text
validate canonical transaction and its D floor/ceiling
transaction_id = H("transaction", ChainId || encode(transaction))
create root budget frame with used=[0,0,0,0]
ordinal_counter = 0
prepared_code_hashes = empty
events = empty
save state checkpoint
debit [0,0,len(encode(transaction))+81,0]
debit [1,0,0,0]
run ordinary-mode call or deployment with no caller
if status != 0: restore state checkpoint and discard every event
receipt = transaction_id, status, root used, result, retained events
```

A failed root compute debit leaves the data base consumed. Top-level envelope input/code has no extra child-receive copy debit. Deployments still pay their validation/hash price. Context height/time/parent/producer and ChainId belong to the candidate block, not to a prior simulation. TransactionId is shared by all frames of that occurrence. A repeated transaction occurrence starts with fresh counters, ordinal and prepared set and runs against its own current state.

### 26.3. Checkpoint scope and prepared code

Checkpoints restore persistent records and truncate the ordered event stream. They never restore consumed counters, the transaction's deployment ordinal or its prepared-CodeHash set. A successful child merges into the parent's checkpoint scope, so a later parent failure undoes it. Memory, globals, table and last-result belong to their own invocation and disappear with it. Suspended parent instance state is preserved until resumed.

A valid deployment marks its CodeHash prepared immediately after successful validation, before either installation write. A later G/S exhaustion, initializer failure, outer revert or failed identifier-result debit does not remove that prepared entry. Deployment always pays validation plus CodeHash again even for a prepared or cached CodeHash. An ordinary call charges the manifest's code price only if that hash is not already prepared; once paid and prepared, a later instance-preparation or execution failure leaves it prepared. A code-price debit that fails does not mark it prepared. At the next transaction occurrence, the set is empty again.

The initializer uses the deployment's existing budget frame, not an additional sub-budget frame. Installation precedes invocation, so the initializer can observe/call its own newly installed code. Its manifest and code reads are still charged. On success its returned bytes are discarded after being charged; the fixed identifier debit `[32,0,32,0]` produces the deployment result. Failure at that last debit rolls back installation and initializer effects and produces an empty result. A reverting initializer propagates its revert bytes and status instead of an identifier.

### 26.4. First-failure and copy details

Instruction dispatch always precedes host-internal preflight. Read-only checks precede mutation spans. `call` validates flags before its spans. `context` validates its selector before its output span; selector 8 reports the executing frame's budget minus its consumption, never capped by what the block has left, taken before its output copy and after prior instruction and host dispatch charges. Storage getters validate the output capacity before copying the key and reading the record. Invalid value length `2^32−1` traps before storage_set copies or reads anything. These details preserve counters even where different faults have the same trap status.

Bulk operations debit fixed dispatch, then combined word/span cost, then test bounds; host copies validate all required ranges before their copy debit. In either case a refused debit moves no bytes. Equal or overlapping source/destination ranges still count two nonempty logical spans for memory.copy. Host argument spans similarly count separately even when aliased. Zero length counts no span, but an empty host span still requires exported memory and a valid endpoint. At exactly 4 GiB, the mathematical endpoint can be 2^32 although the pointer and length fields are individually u32.

Data initialization rounds **the sum** of active segment lengths once for its word charge. Each nonempty segment counts one span and every active segment, including empty, adds 12 C. Overlap changes the final bytes according to source order but removes no charged bytes/spans. All are charged in the preparation debit at initial memory prices. A table's size, not the number of non-null entries or element-segment count, determines its 4-C-per-entry preparation component; element initializers add no instruction dispatches.

Caller arguments and child receive are separate debits. Call caller-copy is `bytes(64,2)`; deploy caller-copy is `bytes(32,1)`. Child receive uses **caller current memory size**, but consumes the child's sub-budget and all enclosing budgets. A deployment's code+input are rounded once together and use 0/1/2 nonempty spans. The child's own instance preparation, loads, stores and later output use its own memory size. Parent growth and child growth never change each other's prices.

A requested child budget is compared after successful caller copying and last-result clearing. Refusal returns 128 with an empty last-result, no child, no ordinal and no child work. Failure of child receive returns status 3 with empty result and zero child consumption; previous caller work remains. A failed preflight or caller debit instead ends the caller frame; it is not a returned 128. Successful copied input is a snapshot and cannot be changed by the suspended caller.

For every combined debit, test dimensions C, S, D, G for failure, and apply the whole vector only if all dimensions fit all active frames. Check transaction frames before the block capacity guard: an unaffordable transaction debit never increases the block total. A local optimization may batch only when it reproduces the exact accepted instruction prefix and first refused debit.

### 26.5. Reachability, status and implementation failures

Status 11 can occur in a read-only child, whose caller can encode it in its own result and continue. Top-level calls and initializers start ordinary; there is no top-level readonly envelope flag. All statuses 0..11 remain valid receipt byte encodings, but decoding a status does not prove that any arbitrary receipt containing it is a valid execution outcome. Status 128 is never a receipt status.

An ordinal is taken only when deployment starts after any nested receive debit. If the counter equals u64 max, reject the attempt with status 7 before deriving or charging an identifier, and do not increment it. Otherwise take its current value and increment. The counter is never rewound. Reaching overflow through ordinary execution is constrained by the u64 compute budget; the rule is specified even if no valid seed can witness it.

Invalid deployable bytes cause status 9 only if the preceding prices and checks succeeded. A too-expensive invalid module can fail with status 3 before validation; a collision can fail with status 10 before code pricing. Valid code can fail installation for G or fail preparation for C without being an invalid module.

Disk failure, unavailable state, inconsistent manifest/code, allocation failure, compiler bugs, native stack overflow and local timeouts are failures to validate, not alternate guest outcomes. Do not commit a receipt for them or mark a block objectively invalid solely because local resources were insufficient. Installed code is always valid under the frozen profile by the state-transition invariant.

### 26.6. Binary integers and segment endpoints

The fixed-width protocol encoding and the variable-width module encoding are different. All block, receipt, transaction and genesis integers use their fixed widths, never LEB128. Within a module, every vector, name and byte-string length uses minimal unsigned 32-bit LEB128. Each function body and section is length-delimited, and parsing must consume exactly its own bytes. A vector is its count followed by exactly that many encoded entries; names are a byte count followed by those bytes. Import/export names must equal the listed ASCII byte strings exactly. A section's leading vector count must be nonzero; parameter/result vectors, function local-group vectors, branch tables and individual data/element payloads may be empty when their other rules permit it.

For a width w, LEB128 contains successive low-to-high groups of seven payload bits in bytes. Bit 7 means another byte follows. To decode, accumulate `(byte & 0x7f) * 2^(7*i)` for byte index i, until the first byte with bit 7 clear. Reject truncation or more than `ceil(w/7)` bytes. Unsigned u32 requires the mathematical value in 0..2^32−1 and no redundant final zero group. For signed i32/i64, subtract `2^(7*k)` when the final byte has bit 6 set, where k is the number of bytes; require the resulting signed value to fit w bits and require the unused high bits of the last permitted payload to agree with its width-specific sign bit. Minimal signed encoding has no removable final `00` after a previous byte whose bit 6 is clear, and no removable final `7f` after a previous byte whose bit 6 is set. Equivalently, re-encoding the decoded value by the algorithm below must reproduce the bytes exactly:

```text
encode_unsigned(n):
    repeat:
        payload = n mod 128
        n = floor(n/128)
        emit payload + (128 if n != 0 else 0)
    until n == 0

encode_signed(n):
    repeat:
        payload = n mod 128               // remainder in 0..127, also for negative n
        n = floor(n/128)                   // arithmetic shift, toward negative infinity
        done = (n == 0 and (payload & 64) == 0)
            or (n == -1 and (payload & 64) != 0)
        emit payload + (0 if done else 128)
    until done
```

The literal section IDs, value types, mutability and limits flags, segment flags and required reserved zero bytes are single bytes in the specified grammar; do not accept a multi-byte spelling of them. The `fc` instruction sub-opcode is a minimal unsigned LEB128. Block types accept only the three single bytes `40`, `7f`, `7e`; a signed type index is not accepted. Local/function/type/global/label indices and all limits, alignment exponents and offsets are u32. Constant immediates alone use signed LEB128 at their value width.

Data and element offsets are the **unsigned 32-bit interpretation** of their `i32.const` bit pattern. Check `offset + length <= initial_memory_bytes` for data, or `offset + item_count <= table_size` for elements, with nonwrapping mathematical addition. Empty segments are valid at the end but not beyond it; they still require the corresponding declared memory/table. Negative-looking offsets are large unsigned offsets. Table and memory initial sizes may be zero; their limits declaration remains mandatory when the object is present. The fixed table maximum equals its minimum, including zero. A declaration of 65,536 maximum pages alone does not allocate or charge those pages; initial size and successful growth determine allocation.

### 26.7. Header and body boundary conditions

Every mined header must have version 1, the pinned ChainId, height exactly parent height+1 without u64 overflow, and `parent_id` equal to that parent's actual identifier (the GenesisBlockId at height 1). Its target must be the exact ancestry-derived value and its hash, interpreted big-endian, must be at most that value. Its four votes must each be 0,1 or 2. A valid target is at least 1 and at most the genesis PoW limit. A timestamp need not exceed its immediate parent's timestamp; it must exceed the specified median. There is no consensus wall-clock upper bound. Header resource values must each fit active capacity before execution and must then equal recomputed totals.

The body encodes all n framed transactions first, then all n framed receipts; it is not an interleaving of transaction/receipt pairs. There is no separate receipt count. Both framing lengths and nested field lengths must be exact and all data must be consumed. An event has only its payload length prefix, not an outer event-length prefix. The ordered commitment's `Bytes(item)` prefix is part of the commitment algorithm, while its item is the raw canonical transaction or receipt encoding; do not pass the body-framed item and add its length twice. Producer hashing likewise hashes raw producer bytes without their body length prefix. Record and code hashes use exactly the inputs in their definitions, not a generic serialization of an enclosing object.

Before allocating vectors, counts must fit the available bytes: each body transaction/receipt pair occupies at least 123 bytes, and each event occupies at least 36 bytes. These are necessary early bounds, not sufficient validation. The entire encoded block length cannot exceed active D capacity because every published byte is charged. The converse does not hold: discarded output can make actual D exceed encoded length. Resource sums, vote-work sums and chain work must not wrap; block resource totals exceeding u64 or active capacity are invalid, while cumulative chain work and voting arithmetic use arbitrary-size integers.

## 27. Seed replay and implementation acceptance

### 27.1. The seed list

A network's seed is its first N blocks, and the network pins the exact BlockId at
every seed height 1 to N. A node holds that list with the genesis configuration and
these rules; a peer never supplies it, since accepting a peer's list would let the
peer choose the history. At a seed height, a header whose BlockId is not the listed
one is refused as an invalid header, before any other check and even when it would
otherwise be a valid block, however much work its branch carries.

Every listed block must still pass normal decoding, target, proof of work, capacity,
execution, receipt and root validation. There is no privileged test VM, preloaded
seed state or bypass of charging, and a listed BlockId never authorizes accepting
execution results without recomputing them. When an implementation's own execution
rejects a listed block, the fault is the implementation's, not the block's: it must
not treat the block as invalid, and the reference node stops, naming the first
transaction whose receipt differs. Blocks after N follow the ordinary validity, work
and voting rules; nothing beyond the listed prefix is fixed.

The seed is distributed as a bundle: for each block in order, its length as a
little-endian `u32`, then its canonical bytes. A released bundle has exactly N
entries and no trailing bytes. The framing is not part of a block's hash or its D
charge, and it is a distribution format only: legal block sizes come from block
capacity and the block's own fields. The production and development networks'
bundles are `conformance/seed/production.blocks` and
`conformance/seed/development.blocks` in the repository, with `production.json` and
`development.json` naming each block's section and each transaction's case.

### 27.2. Replay procedure

1. Validate the chosen exact GenesisConfig; compute and compare its ChainId and GenesisBlockId. Initialize the empty record map and its root, work zero, no votes, and the genesis timestamp anchor.
2. For each seed height in order, parse one complete block and check its BlockId against the authenticated list. Validate its ancestry, median timestamp, exact target, PoW and active capacity.
3. Check canonical body framing and envelopes. Recompute producer and transaction commitments. Execute every occurrence from the previous validated state with the block's actual stable context.
4. Compare every complete receipt byte-for-byte, then the receipt root, state root and all four header totals. A failed transaction is normal when that is its independently defined outcome; an invalid block never becomes acceptable through a failed-transaction receipt.
5. Retain state plus enough branch history to derive median time, difficulty, work and capacity decisions. Verify cold replay, restarts and local cache changes give identical results. A checkpoint at N is an optimization only after its state has been verified against the seed.
6. For later blocks apply exactly the same transition and branch rules. The seed's particular timestamps, producer values, voting patterns and program IDs are examples, not new restrictions on all future blocks.

A discrepancy should report the earliest differing height, transaction occurrence and field. An identical TransactionId may occur multiple times, so identify it by `(height,index)` as well. Matching final state alone is insufficient: an implementation can have wrong counters or discarded-output accounting yet the same storage. Matching receipts alone is insufficient: storage roots and record contents must also agree.

### 27.3. What conformance must additionally reject

The explicit rules above require rejecting wrong versions and framing, invalid envelope D, extra or missing receipts, wrong producer/transaction/receipt/state commitments, wrong used totals, block capacity overflow, incorrect ancestry/median/target/PoW and invalid votes. A valid on-chain seed cannot contain examples of these invalid blocks. The [consensus vectors](node/wiki/reference/conformance.md#consensus-vectors) hold such
blocks, each with exactly one rule broken; the rules in this document define their
verdicts.

Likewise, direct state-tree vectors can supply chosen raw keys whose paths split at every depth. Those are primitive tests of the specified root function; they do not authorize programs to bypass hashed RecordKeys. Formula tests across all 0..65,536 memory-page counts supplement reachable seed executions. Full-range arithmetic agreement does not itself prove large-memory address arithmetic or copy integration.

### 27.4. Matters that do not add consensus rules

There is no native coin, fee, account ownership, signature scheme, sender field, reward, privileged miner invocation, transaction nonce or protocol-wide replay ban. Application code may implement these. Producer data is arbitrary bytes, not an authenticated producer identity. No reference contract or cryptographic algorithm inside a deployed program receives special execution treatment.

Peer message formats, compression, APIs, proof-response formats, database layout, snapshots, mempool policy, compiler strategy and cache eviction do not alter canonical consensus bytes. A node can defer far-future timestamps or choose a local tie/hysteresis policy without declaring otherwise valid blocks invalid. Capacity votes change only the four limits, not any instruction price or meaning. Future larger capacity does not enable an unlisted opcode or add another memory/table.

## 28. Configurations and check values

The configurations built into the reference node, complete, so an implementation can
start without reading source code. The development network uses the first, and its
seed is built on it. Several consensus vectors use the second. The third is the
production network's, the production profile of
[genesis](node/wiki/consensus/genesis.md#production-profile) at its launch time.

### 28.1. Development

| Field | Exact value |
|---|---|
| Protocol version | 1 |
| Genesis timestamp | 1700000000 |
| Initial target, big-endian hex | `3fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff` |
| PoW limit, big-endian hex | `ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff` |
| Target interval | 60 |
| Half-life | 172800 |
| Voting epoch length | 20160 |
| Initial C/S/D/G | 100000000, 1024, 1048576, 524288 |
| Minimum C/S/D/G | 100000, 16, 1024, 4096 |
| ChainId | `8578787146be1ef0aacc797754001290272915e89cd9e5784fe1368e4c2131bc` |
| GenesisBlockId | `ae99e1c9c68f6fe3e527ab9952a1a5ee3876a996ddb7b1149682b320b80ec0bb` |

Canonical GenesisConfig, exactly 150 bytes (hexadecimal; line breaks below are presentation only):

```text
010000f15365000000003fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
ffffffffffffffffffffffffffffffffffffffffffffffff3c00000000a30200c04e000000e1f50500000000000400000000
000000001000000000000000080000000000a086010000000000100000000000000000040000000000000010000000000000
```

### 28.2. Experimental

| Field | Exact value |
|---|---|
| Protocol version | 1 |
| Genesis timestamp | 1700000000 |
| Initial target, big-endian hex | `3fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff` |
| PoW limit, big-endian hex | `ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff` |
| Target interval | 60 |
| Half-life | 172800 |
| Voting epoch length | 4 |
| Initial C/S/D/G | 100000000, 1024, 1048576, 524288 |
| Minimum C/S/D/G | 100000, 16, 1024, 4096 |
| ChainId | `15b2b480e8b3bb449479c1aef1dbf1e5a27c907d49fc876842d4b83332facd53` |
| GenesisBlockId | `2a25002d2674c05cc1e310c81fa3be635c66e6164923041418d47c2ffc84018f` |

Canonical GenesisConfig, exactly 150 bytes (hexadecimal; line breaks below are presentation only):

```text
010000f15365000000003fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
ffffffffffffffffffffffffffffffffffffffffffffffff3c00000000a302000400000000e1f50500000000000400000000
000000001000000000000000080000000000a086010000000000100000000000000000040000000000000010000000000000
```

### 28.3. Production

| Field | Exact value |
|---|---|
| Protocol version | 1 |
| Genesis timestamp | 1792540800 (2026-10-21 00:00:00 UTC) |
| Initial target, big-endian hex | `00000000ffffffffffffffffffffffffffffffffffffffffffffffffffffffff` |
| PoW limit, big-endian hex | `0000000fffffffffffffffffffffffffffffffffffffffffffffffffffffffff` |
| Target interval | 60 |
| Half-life | 43200 |
| Voting epoch length | 20160 |
| Initial C/S/D/G | 450000000, 2048, 262144, 131072 |
| Minimum C/S/D/G | 40000000, 64, 98304, 98304 |
| ChainId | `15d6549cfacd816f835c632b4c67308d1c63e642f4cbf4049368430670705107` |
| GenesisBlockId | `24b76a52f3825b921373b6c6a41ced2a74de7dbdb34015669c10594996887459` |
| Seed length N | 325 blocks |
| Last seed BlockId | `00000000e04237d25842f99dc925e087198affc2da93817ef30dd2a3527a9b32` |

Canonical GenesisConfig, exactly 150 bytes (hexadecimal; line breaks below are presentation only):

```text
01008000d86a0000000000000000ffffffffffffffffffffffffffffffffffffffffffffffffffffffff0000000fffffffff
ffffffffffffffffffffffffffffffffffffffffffffffff3c000000c0a80000c04e00008074d21a00000000000800000000
000000000400000000000000020000000000005a620200000000400000000000000000800100000000000080010000000000
```

### 28.4. Primitive commitments

All values below use the domain separation specified above; inputs labeled empty contain zero bytes.

| Value | Hexadecimal digest |
|---|---|
| Empty state root | `1afb449a10b29cdbe8a0b60a98e26b413a52f8bc35af49a1fc2d4fff669f5889` |
| Empty producer data hash | `51976778d26056c36803109ea49beaec0c618c49fc39c63fb1f10e51bf56592c` |
| Empty transactions root | `8c2beadecc1824b6e7f88cd39fff6d8ffdc9e8a41e93254d7f7109ba7d218f49` |
| Empty receipts root | `de3aa3b4c7df30035eb3ddc2d357d2a54521c27a4742438a9ab4561cb516ee6b` |

### 28.5. Arithmetic checks

| Calculation | Expected result |
|---|---|
| Call envelope with empty input | 70 bytes; data base 151 |
| Deploy envelope with empty code and input | 42 bytes; data base 123; code is still invalid when validation is reached |
| Empty receipt | 73 bytes |
| Empty block with empty producer | 302 encoded bytes and D=302 |
| Code price of zero bytes | 14,400 C |
| Code price of only the 8-byte preamble | 14,880 C |
| CodeHash charge for zero code bytes | 1,000 C |
| Record key hash charge | 1,900 C |
| Value leaf hash at lengths 2 and 3 | 1,000 and 1,900 C respectively |
| Initializer installation G before any storage writes | 116 + module byte length |
| Memory at 32 pages: load/store/word/span | 80 / 1 / 8 / 0 |
| Memory at 33 pages: load/store/word/span | 770 / 16 / 12 / 16 |
| Memory at 48 pages: load/store/word/span | 772 / 17 / 12 / 17 |
| Memory at 65,536 pages: load/store/word/span | 1,416 / 447 / 12 / 447 |
| 1-byte copy at 48 pages | 46 C dynamic debit plus 20 C dispatch: 66 C for the instruction |
| 1-byte fill at 48 pages | 29 C dynamic debit plus 20 C dispatch: 49 C for the instruction |
| Zero-length copy or fill | 20 C dispatch, zero dynamic copy cost; bounds checks still apply |
| 32-byte key + 1-byte value copied at 48 pages | 58 C: two words and two spans |
| Stack pages at heights 8,192 and 8,193 | 0 and 1 |
| Work at target 1 | 2^255 |
| Work at target 2^256−1 | 1 |
| Voting with exactly two-thirds increase work | increase |
| Voting with exactly one-half decrease work | hold |

The illustrative envelope sizes include all fields exactly once; block framing contributes through the 81-byte data-base addend. A receipt comparison must include discarded-output consumption even though the discarded bytes are absent from its encoding.

## 29. SHA-256 definition

SHA-256 maps a byte string to a 32-byte digest. The following byte-input definition is included for completeness; using a correctly implemented cryptographic library is equivalent. It is independent of the VM's guest-program hashing code and does not create a host import.

### 29.1. Padding and initial state

Let l be the input length in bits. Append byte `80`, then zero bytes until the length is congruent to 56 modulo 64, then the original l as an unsigned 64-bit **big-endian** integer. All consensus hash messages fit the SHA-256 domain l<2^64. Split the padded message into 64-byte blocks. The eight initial 32-bit state words are:

```text
6a09e667 bb67ae85 3c6ef372 a54ff53a 510e527f 9b05688c 1f83d9ab 5be0cd19
```

All additions in the SHA compression algorithm are modulo 2^32. NOT is bitwise complement within 32 bits. `ROTR(x,n)` rotates a 32-bit word right; `SHR(x,n)` is logical right shift.

```text
Ch(x,y,z)  = (x AND y) XOR ((NOT x) AND z)
Maj(x,y,z) = (x AND y) XOR (x AND z) XOR (y AND z)
S0(x) = ROTR(x,2) XOR ROTR(x,13) XOR ROTR(x,22)
S1(x) = ROTR(x,6) XOR ROTR(x,11) XOR ROTR(x,25)
s0(x) = ROTR(x,7) XOR ROTR(x,18) XOR SHR(x,3)
s1(x) = ROTR(x,17) XOR ROTR(x,19) XOR SHR(x,10)
```

### 29.2. Compression

For each message block, read its sixteen 32-bit words in big-endian order as W[0..15]. Extend to 64 words by `W[t]=s1(W[t−2])+W[t−7]+s0(W[t−15])+W[t−16]` for t=16..63. Copy the current eight state words to a,b,c,d,e,f,g,h. For t=0..63:

```text
T1 = h + S1(e) + Ch(e,f,g) + K[t] + W[t]
T2 = S0(a) + Maj(a,b,c)
(a,b,c,d,e,f,g,h) = (T1+T2, a, b, c, d+T1, e, f, g)
```

The assignment is simultaneous. Add a..h respectively into the eight previous state words. After the final message block, concatenate those state words in big-endian order to get the digest. K[0..63], in order, is:

```text
428a2f98 71374491 b5c0fbcf e9b5dba5 3956c25b 59f111f1 923f82a4 ab1c5ed5
d807aa98 12835b01 243185be 550c7dc3 72be5d74 80deb1fe 9bdc06a7 c19bf174
e49b69c1 efbe4786 0fc19dc6 240ca1cc 2de92c6f 4a7484aa 5cb0a9dc 76f988da
983e5152 a831c66d b00327c8 bf597fc7 c6e00bf3 d5a79147 06ca6351 14292967
27b70a85 2e1b2138 4d2c6dfc 53380d13 650a7354 766a0abb 81c2c92e 92722c85
a2bfe8a1 a81a664b c24b8b70 c76c51a3 d192e819 d6990624 f40e3585 106aa070
19a4c116 1e376c08 2748774c 34b0bcb5 391c0cb3 4ed8aa4a 5b9cca4f 682e6ff3
748f82ee 78a5636f 84c87814 8cc70208 90befffa a4506ceb bef9a3f7 c67178f2
```

Raw SHA-256 of the empty byte string is `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`. Raw SHA-256 of ASCII `abc` is `ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad`. Protocol hashes first prepend the `Ting/` domain header and tag; neither raw test digest is the empty state root.
