# Contracts and records

All state lives in one flat map of **records**: 32-byte keys to values of any length.
Contracts, their code and their storage are all records in that map,
told apart by how their keys are derived.

## Identifiers

```text
ContractId = H("contract",  ChainId || TransactionId || u64(ordinal))
CodeHash   = H("code",      code)
RecordKey  = H("state-key", u8(namespace) || ContractId || logical_key)
```

`H` is the [domain-separated hash](/wiki/data/encoding#domain-separated-hashing).
`ChainId` comes from [genesis](/wiki/consensus/genesis) and `TransactionId` from the
[transaction](/wiki/data/blocks#transactions) that deploys the contract. `code` is the
complete original module bytes, `namespace` is one byte from the table below, and
`logical_key` is exactly 32 bytes.

The **deployment ordinal** counts deployment attempts within one transaction,
starting at zero. Every deployment, the transaction's own or one started by a
program, takes the counter's current value as its ordinal when it begins and then
increments the counter. The counter never goes back, even when a deployment fails or
a call is rolled back, so every attempt gets a distinct identifier. If incrementing
would overflow the 64-bit counter, that deployment fails with status 7, structural
bound exceeded. Exactly when an attempt begins is part of the deployment sequence
[below](#deployment).

None of these identifiers depends on the current block's hash, its timestamp or its
nonce: the same deployment attempt of a transaction gets the same ContractId
whichever block the transaction lands in.

## Deployment

A deploy transaction and a deployment started by a program are the same operation.
It fails when its ContractId already exists (status 10), when its module is invalid
(status 9), or when its first invocation fails. A failed deployment rolls back its
code and every state change it made. The full sequence, with its charges, is on
[deploy and invoke](/wiki/vm/lifecycle).

Code is installed once and never changes: there is no replacement, no deletion, no
account ownership in the protocol and no running of one contract's code against
another contract's storage. Using another contract always means an explicit call, and
a program can check which code it is about to call with the `code_hash` host
function.

## Namespaces

| Namespace | Holds | Logical key | Written by |
|---|---|---|---|
| 0 | The manifest | 32 zero bytes | The runtime |
| 1 | The code, as one record | 32 zero bytes | The runtime |
| 2 | The contract's storage | Any 32 bytes | The program |

A manifest is exactly 44 bytes:

```text
manifest = CodeHash || u32(code_length) || u64(code_price)
```

`code_price` is the code's [code price](/wiki/resources/prices#code), worked out once
when it is deployed, so that a call can charge it before reading any of the code.

The code record holds the complete original module bytes, exactly as deployed. Its
length is at least 1 and at most `2^30` bytes. A deploy transaction carries its code,
so its data budget bounds the code too. Code a program deploys comes from that
program's memory and is paid for in compute and growth; no data is charged for its
bytes ([deploy and invoke](/wiki/vm/lifecycle)).

A contract's manifest and code record always agree, and installed code always
satisfies the [execution profile](/wiki/vm/profile). These are invariants of the
state, not outcomes a program can observe: a node that found otherwise would be
holding corrupt state.

A program reads and writes only its own namespace-2 records, through the
[host interface](/wiki/vm/host). The runtime alone manages manifests and code.

## Present, empty and absent

An empty value is a present record. It differs from an absent record, and programs
can tell them apart. Deleting a record makes it absent.

## Values of any size

A record has no size cap. Reading one costs one state access and writing one two,
a read of the old record and the write, regardless of its length; each read also
pays 1,900 C for hashing the record's key. The bytes themselves are paid as compute:
copying a value into or out of a program at the byte price of the program's memory
tier, and hashing it into the state tree when it is written at about 14 C per byte
([the price list](/wiki/resources/prices)). A large key, a list or a post-quantum
signature is therefore a single record, bounded only by the memory a program can
afford to hold it in, and that memory is paid for in compute. See
[what each counter measures](/wiki/resources/counters).
