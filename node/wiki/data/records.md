# Contracts and records

All of Ting's state is one flat map of **records**: 32-byte keys to byte values. A
deployed program's manifest, its code and its storage are all records in that map,
told apart by how their keys are derived. There are no accounts and no native coin
in the protocol, only programs and the records they keep. The exact rules are in
[section 5 of the protocol rules](../../../ting-protocol-rules.md#5-contracts-and-records).

## Identifiers

Three identifiers name everything in the state. Each is a
[domain-separated hash](encoding.md#domain-separated-hashing) of the parts listed:

| Identifier | Names | Hash of |
|---|---|---|
| ContractId | A deployed program | The ChainId, the TransactionId of the deploying transaction, and the deployment's ordinal |
| CodeHash | A module | The module's complete original bytes |
| RecordKey | A record | A namespace byte, a ContractId and a 32-byte logical key |

The **deployment ordinal** counts the deployment attempts within one transaction,
starting at zero, whether the transaction or a program starts them. An attempt that
fails, or is rolled back with its caller, keeps its number, so every attempt gets a
distinct ContractId.

None of these identifiers depends on the block a transaction lands in: not its hash,
its timestamp or its nonce. A deploy transaction's own deployment has ordinal 0, so
anyone can work out its ContractId from the transaction alone, before it is mined.
If the same transaction is included again after its deployment succeeded, the
deployment finds its ContractId taken and fails with status 10. The exact
derivations are in [section 5.1 of the protocol rules](../../../ting-protocol-rules.md#51-identifiers).

## Deployment

A deploy transaction and a deployment started by a program are the same operation.
It fails when its ContractId is already taken (status 10), when its module is invalid
(status 9), or when its first run fails, and a failed deployment rolls back its code
and every state change it made. The full sequence, with its charges, is on
[deploy and invoke](../vm/lifecycle.md).

Code is installed once and never changes: there is no replacement, no deletion, no
account ownership in the protocol and no running of one contract's code against
another contract's storage. Using another contract always means an explicit call, and
a program can check which code it is about to call with the `code_hash` host
function.

## Namespaces

Each contract's records fall into three namespaces, told apart by the namespace
byte hashed into their record keys:

| Namespace | Holds | Logical key | Written by |
|---|---|---|---|
| 0 | The manifest | 32 zero bytes | The runtime |
| 1 | The code, as one record | 32 zero bytes | The runtime |
| 2 | The contract's storage | Any 32 bytes | The program |

The manifest is 44 bytes: the CodeHash, the code's length and its
[code price](../resources/prices.md). The price is worked out once, when the code is
deployed, so that every call can charge it before reading any of the code. The code
record holds the module's original bytes, exactly as deployed, at most `2^30` bytes.
The runtime alone writes these two records. They always agree with each other, and
installed code always satisfies the [execution profile](../vm/profile.md).

A program reads and writes only its own storage, the namespace-2 records under its
own ContractId, through the [host interface](../vm/host.md).

## Present, empty and absent

A record with an empty value is present, with zero bytes. That differs from an
absent record, and programs can tell the two apart: a storage getter returns a
value's length, 0 for an empty value, and `2^32 − 1` for an absent record. Deleting
a record makes it absent. Creating a record counts as
[growth](../resources/growth.md) even when its value is empty.

## Values of any size

A storage record has no size cap beyond its 32-bit length: a value can hold up to
`2^32 − 2` bytes, one fewer than the marker a getter returns for an absent record
([the exact limits](../../../ting-protocol-rules.md#54-present-empty-and-absent)).
Its state-access cost does not depend on its length: reading one costs one state
access, and writing one costs two, a read of the old record and the write. Every
read, a write's read of the old record included, also pays 1,900 C for hashing the
record's key.

The bytes themselves are paid as compute:

- copying a value into or out of a program, at the program's
  [memory prices](../resources/prices.md);
- hashing a value into the state tree when it is written, at about 14 C per byte.

Bytes added to the state also count as [growth](../resources/growth.md). A large key,
a list or a post-quantum signature is therefore a single record, bounded only by the
memory a program can afford to hold it in, and that memory is paid for in compute.
See [what each counter measures](../resources/counters.md).
