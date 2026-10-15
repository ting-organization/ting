# What each counter measures

Each counter is charged by a fixed list of logical events, and every
implementation must reproduce those events exactly. This page lists every event
with its amount. The precise point at which each VM debit happens, and its order
relative to traps, is on [Exact metering](/wiki/vm/metering); the order of checks
inside each host function is on [The host interface](/wiki/vm/host).

## C, compute

C charges each event at its price on [the price list](/wiki/resources/prices). The
prices were measured once, so that one C stands for about the same work whatever it
is spent on; `bytes(n)` is the byte price of `n` bytes at the executing instance's
memory tier, and `sha(m)` the price of a SHA-256 over `m` input bytes.

| Event | C |
|---|---:|
| Start a top-level call or deployment, before any state access | 1 |
| Dispatch an executed instruction | its price, by instruction and, for loads and stores, by memory tier |
| Enter a defined function | 1 per parameter, declared local and slot of its maximum operand height |
| Read a record, for its key | 1,900, in the debit of the read's S |
| Write a value of `n` bytes to a record, for its leaf | `sha(n + 53)`, in the debit of the write's S |
| Prepare an instance | 40,000, plus 40,000 per page of initial memory, 40 per global, 4 per table entry, 12 per active data segment and `bytes(total segment length)` |
| `memory.grow` that adds `p` pages | 40,000 × `p`, after its dispatch |
| `memory.copy` or `memory.fill` of `n` bytes | `bytes(n)`, after its dispatch |
| Copy `n` bytes between a program and the host, in either direction | `bytes(n)` |
| Return, revert or emit `n` bytes | `bytes(n)` |
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
- Code is prepared at most once per transaction and CodeHash: the first invocation
  of a program in a transaction pays its code price, unless the transaction deployed
  that code or already invoked it; later invocations pay none. What a node keeps
  cached never changes the charge.
- The code price is charged before validation or any cache lookup, and invalid code
  keeps the charge.

Prices are fixed numbers. Nothing is ever measured while a transaction runs.

## S, state access

S charges one unit per logical record read or write, including reads of records
that turn out to be absent. Each unit stands for one random access and one path
through the [state tree](/wiki/data/state-tree). Every byte-proportional cost,
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

## D, data

D covers bytes the network must carry, or that programs generate.

| Event | D |
|---|---:|
| A transaction, before execution starts | `len(encode(Transaction)) + 81` |
| Return or revert `n` result bytes, by any invocation | `n` |
| Emit an event with an `n`-byte payload | `36 + n` |
| Produce the identifier of a successful deployment | 32 |

The transaction's data base covers its own [encoding](/wiki/data/encoding) and its
place in the [block body](/wiki/data/blocks):

```text
data base = len(encode(Transaction)) + 81
81        = 4      transaction's length prefix in the body
          + 4      receipt's length prefix in the body
          + 73     receipt's fixed fields
```

An event's 36 bytes are the emitting ContractId and the payload's length prefix. A
transaction whose declared D is smaller than its data base, or larger than
`2^32 − 1`, has an invalid envelope: its encoding and its receipt are each framed with
a 32-bit length in the block body ([blocks](/wiki/data/blocks#transactions)).

Output is charged when it is generated, even if a failing caller later discards it,
so rollback cannot be used to recycle a data allowance. A child's result read with
`call_result` costs only its copy compute; if the parent returns those bytes again,
that is new output and is charged again. A deployment's initializer result is
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

A block's D therefore equals its canonical encoded length only when the only output
of its transactions is their top-level results and the events their receipts keep.
Every other output is charged D that no block carries: a nested invocation's
result, a deployment initializer's result, the identifier a nested deployment
returns, and output a failure discarded. With any of these, D is larger. Framing used
to carry blocks between peers is not counted. How the block total is formed is on
[Block capacity and bounds](/wiki/resources/capacity#block-totals).

## G, growth

G charges for gross new persistent allocation. A new record costs 36 plus its value
length, growing an existing value costs the added bytes, and deleting or shrinking
costs nothing. See [Persistent growth](/wiki/resources/growth).

## What is never counted

Elapsed time, physical disk reads, cache hits and misses, compiled-code size and
database layout are never counted. A node can change all of them without changing a
single counter.
