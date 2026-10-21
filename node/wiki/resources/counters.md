# What each counter measures

Each of the four counters is charged by a fixed list of logical events: an
instruction runs, a record is read, a byte of output is produced. Every node charges
the same events with the same amounts, whatever its hardware, cache or database, so
every node reaches the same counters and the same outcome. This page lists every
event with its amount, which is what a program author needs to estimate a
transaction. The exact rules are in
[section 12 of the protocol rules](../../../ting-protocol-rules.md#12-what-each-counter-measures);
the exact point at which each debit is made is in
[section 21](../../../ting-protocol-rules.md#212-where-each-charge-is-made), and the
steps of each host function in
[section 22](../../../ting-protocol-rules.md#22-the-host-interface).

## C, compute

C charges each event at its price on [the price list](prices.md). The
table uses two shorthands from that list:

- `bytes(n, s)` is the price of moving `n` bytes in `s` spans that hold a byte, at
  the executing instance's [memory prices](prices.md#memory-prices).
- `sha(m)` is the price of a SHA-256 over `m` input bytes ([hashing](prices.md#hashing)).

| Event | C |
|---|---:|
| Start a top-level call or deployment | 1 |
| Dispatch an executed instruction | its price, by instruction and, for loads and stores, by memory size |
| Enter a defined function | 1 per parameter, declared local and slot of its maximum operand height, and 40,000 per 64 KiB the entry adds to the invocation's [stack](prices.md#stack) beyond its mark |
| Reach a host import through `call_indirect` | 94 |
| Read a record, for its key | 1,900 |
| Write a value of `n` bytes to a record, for its leaf | `sha(n + 53)` |
| Prepare an instance | 40,000, plus 40,000 per page of initial memory, 40 per global, 4 per table entry, 12 per active data segment and `bytes(n, s)` for the `n` bytes of the active data segments, `s` of which hold a byte |
| `memory.grow` that adds `p` pages | 40,000 × `p` |
| `memory.copy` of `n` bytes | `bytes(n, s)` over its source and destination |
| `memory.fill` of `n` bytes | `bytes(n, s)` over its destination |
| Copy `n` bytes between a program and the host, in either direction | `bytes(n, s)` over the spans copied |
| Return, revert or emit `n` bytes | `bytes(n, s)` over the one span |
| Prepare code: validate a deployment, or load code its transaction has not yet prepared | the [code price](prices.md#code) |
| Hash deployed code of `n` bytes | `sha(n + 11)`, with the code price |
| Derive a deployment's ContractId | 1,900 |
| Produce the identifier of a successful deployment | 32 |

A few consequences are worth knowing when estimating:

- Skipped instructions cost nothing, and the delimiters `else` and `end` cost nothing.
- Calling a host function pays the `call` instruction's price for an import, 94,
  then the host function's own charges.
- Every invocation starts from a fresh instance, so instance preparation is paid on
  **every** run. A program that declares one 64 KiB page pays 80,000 C each time it
  runs, before its first instruction executes.
- An invocation pays its code's price only if its transaction has not yet prepared
  that CodeHash, by validating it in a deployment or paying for it in an earlier
  invocation. A deployment pays the code price and its CodeHash every time. Invalid
  code keeps the charge, and what a node keeps cached never changes it
  ([prepared code](../../../ting-protocol-rules.md#263-checkpoint-scope-and-prepared-code)).

Prices are fixed numbers, measured once so that one C stands for about the same work
whatever it is spent on. Nothing is ever measured while a transaction runs.

## S, state access

S charges one unit per logical record read or write, including reads of records
that turn out to be absent. Each unit stands for one random access and one path
through the [state tree](../data/state-tree.md). Every byte-proportional cost,
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
| Deploy: check that the deployment's ContractId is free by reading its manifest | 1 |
| Deploy: write the manifest and the code record, each like a storage write | 2 each |

Running a program reads its manifest and its code record, two units, on every
invocation. A node that already has the code compiled and cached still charges the
same: caches make nodes faster, never transactions cheaper. A deployment that
installs its code has made five accesses before its initial invocation adds those
two.

Maintaining the state tree has no separate charge. The hashes a node computes along
a record's path, about log₂ of the number of records and never more than 256, are
bounded work tied to the access, since a key has a fixed width. Two hashes are
compute instead, priced in C above: a record's key, which every access needs, and a
written value's leaf, whose size the program chooses.

### An example: writing a record

A program running with at most 64 KiB of memory calls `storage_set` to create a
record with a 32-byte value. The call itself costs:

| Step | C | S | G |
|---|---:|---:|---:|
| `call` of the host import | 94 | | |
| Copy the 32-byte key and the 32-byte value in, `bytes(64, 2)` | 8 | | |
| Read the old record, with its key's hash | 1,900 | 1 | |
| Write the record, with its leaf's hash `sha(85)` | 1,900 | 1 | 68 |
| **Total** | **3,902** | **2** | **68** |

The 68 G is the record's [growth](growth.md): 36 for the record plus its 32 bytes.
Writing the same record again with another 32-byte value costs the same C and S but
no G.

## D, data

D covers bytes the network must carry, or that programs generate.

| Event | D |
|---|---:|
| A transaction, before execution starts | `len(encode(Transaction)) + 81` |
| Return or revert `n` result bytes, by any invocation | `n` |
| Emit an event with an `n`-byte payload | `36 + n` |
| Produce the identifier of a successful deployment | 32 |

The transaction's data base covers its own [encoding](../data/encoding.md) and its
place in the [block body](../data/blocks.md):

```text
data base = len(encode(Transaction)) + 81
81        = 4      transaction's length prefix in the body
          + 4      receipt's length prefix in the body
          + 73     receipt's fixed fields
```

An event's 36 bytes are the emitting ContractId and the payload's length prefix. A
transaction must declare at least its data base in D, and at most `2^32 − 1`, or
its envelope is invalid.

Output is charged when it is generated, even if a failing caller later discards it,
so rollback cannot be used to recycle a data allowance. A child's result read with
`call_result` costs only its copy compute; if the parent returns those bytes again,
that is output of its own and is charged again. A deployment's initializer result is
discarded but stays charged. D is therefore an upper bound on what is published plus
the output that was discarded.

### The block's own bytes

Each block consumes D outside every transaction budget:

```text
block data = 302 + len(producer_data)
302        = 294    header
           + 4      producer data's length prefix
           + 4      transaction count
```

A block's D therefore equals its encoded length when the only output of its
transactions is their top-level results and the events their receipts keep. Any
other output, such as a nested call's result or output a failure discarded, was
charged D that no block carries, so D is larger. How the block total is formed is on
[Block capacity and bounds](capacity.md#block-totals).

## G, growth

G charges for gross persistent allocation. A record that did not exist costs 36
plus its value length, growing an existing value costs the added bytes, and deleting
or shrinking costs nothing. See [Persistent growth](growth.md).

## What is never counted

Elapsed time, physical disk reads, cache hits and misses, compiled-code size and
database layout are never counted. A node can change all of them without changing a
single counter.
