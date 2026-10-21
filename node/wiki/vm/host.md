# The host interface

A program talks to the chain only through twelve functions it imports from module
`ting`. With them it reads its input and context, uses its own storage, learns which
code another contract runs, calls or creates contracts, and produces output.
Anything more, such as checking signatures, moving money or charging fees, is the
business of programs, not of the interface. This page is the program author's guide
to the twelve functions. The exact rules, including the order of every check and
charge, are in
[section 22 of the protocol rules](../../../ting-protocol-rules.md#22-the-host-interface).

| Purpose | Imports |
|---|---|
| Read invocation data | `input`, `context` |
| Use its own storage | `storage_get`, `storage_set`, `storage_delete` |
| Learn about other contracts | `code_hash` |
| Call or create contracts | `call`, `deploy`, `call_result` |
| Produce output | `emit`, `return`, `revert` |

## Conventions

- **Spans are checked.** Every argument is a 32-bit integer, read as unsigned. A
  byte string is passed as a pointer and a length into the program's exported
  memory, and every such span is checked against the memory before anything is
  copied. A span outside it traps the invocation.
- **Bytes are opaque.** Input, results, events and storage values have no protocol
  format. Each program defines its own encodings.
- **Bytes cost compute.** Every copy across the interface, in either direction, costs
  `bytes(n, s)`: the word price of the program's memory size for each started 32
  bytes, and the span price for each span that holds a byte
  ([memory prices](../resources/prices.md#memory-prices)). Up to 2 MiB of memory the
  span price is zero; above that it is the price of a store.
- **Records cost compute and S.** Every record read costs 1,900 C, for hashing the
  record's key, and one S, and every value written also costs the price of hashing it
  into its leaf ([hashing](../resources/prices.md#hashing)).
- **A charge fits whole or not at all.** A charge that does not fit ends the
  invocation with the status of the exhausted counter, and a refused charge moves no
  bytes.

## Signatures

Every parameter and every ordinary result is `i32`, and an import's type must match
its row exactly ([module shape](profile.md#module-shape)).

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
ordinary call and 1 for a [read-only](calls.md#read-only-mode) one.

## Getters

`input`, `context`, `storage_get`, `code_hash` and `call_result` copy a value into an
output buffer, given as a pointer and a capacity, and return the value's length as an
unsigned 32-bit number:

- **The full length is returned.** If the value fits the capacity, it is copied and
  the copy is charged. If it does not, nothing is copied and no copy is charged, so a
  program can ask with a small buffer, learn the length and call again with a larger
  one.
- **Absent is not empty.** `storage_get` returns `2^32 − 1`, every bit set (−1 read
  as signed), for a record that does not exist, and 0 for a record that exists and is
  empty. `code_hash` returns `2^32 − 1` for a contract that does not exist. No value
  is ever that long: a record value is at most `2^32 − 2` bytes
  ([storage writes](#storage-writes)).
- **Every call pays again.** A getter's charges repeat each time it is called;
  nothing is discounted for having been read before.

The output span is checked in full even when the value turns out to be absent or
shorter. `storage_get` reads a record in the executing contract's own storage
([namespaces](../data/records.md#namespaces)). It pays for copying its 32-byte key,
1,900 C and one S for the read, and the copy of the value when it fits. The read
needs only the record's length until the value is copied, so a length query does no
work in proportion to the value.

## Storage writes

`storage_set` sets a record of the executing contract to a value of any length;
`storage_delete` deletes one. Both fail with status 11 in a
[read-only](calls.md#read-only-mode) invocation. Each pays:

1. the copy of its arguments: the 32-byte key and, for `storage_set`, the value;
2. 1,900 C and one S for the key and for reading the old record, of which only the
   length is needed;
3. for `storage_set`, the price of hashing the value into its leaf,
   `100 + 900 × ceil((n + 62) / 64)` C for `n` bytes, and for either, a second S and
   the record's [growth](../resources/growth.md) in G, which is zero for a deletion.

Both S units are charged even for a write that changes nothing and for deleting a
record that does not exist. The key is 32 bytes and a value at most `2^32 − 2`
bytes, one fewer than memory allows, so that no length can be mistaken for absence.
There is no other size limit. The runtime's own writes of a manifest and a code
record during [deployment](lifecycle.md#deployment) are priced the same way.

## Output

`return` and `revert` end the invocation, with status Success or Revert, and their
bytes become its result either way. Each result byte costs one D, plus the compute
of copying it out. `revert` rolls back the invocation's effects
([checkpoints](calls.md#checkpoints)), so a program can fail with an explanation its
caller can read.

`emit` appends an event holding the executing ContractId and the payload bytes
([events](../data/blocks.md#events)). An event of `n` payload bytes costs `36 + n` D,
plus the copy. `emit` fails with status 11 in a read-only invocation.

## Calls and deployments

`call` runs another contract as a child with a sub-budget the caller chooses, and
`deploy` runs a [deployment](lifecycle.md#deployment) the same way: it installs
new code and runs its initializer. Both go through the same stages:

1. A `call` with flags other than 0 or 1 traps, and a `deploy` in a read-only
   invocation fails with status 11. Otherwise the caller pays for copying the
   32-byte budget, and for a `call` the 32-byte target too, and its last-result
   buffer is emptied.
2. If the requested budget is larger, in any counter, than what the caller has left,
   the import returns 128 and no child is created. The caller simply continues.
3. Otherwise the child opens with that budget and pays, out of it, for receiving its
   input and, for a `deploy`, its code. If it cannot, it ends with status 3 having
   consumed nothing, and the import returns 3.
4. Otherwise the child runs, read-only when the call's flags are 1 or the caller is
   itself read-only, and the import returns the child's status.

The child's status is a [receipt status code](../data/blocks.md#receipts); 128 is
only ever a host return value, never a receipt status. A call to a contract that does
not exist returns 8. After a `call`, the last-result buffer holds the child's result
bytes. After a `deploy` it holds the new 32-byte ContractId on success, or the failed
initializer's result bytes, if any. `call_result` is a getter over that buffer:
reading it costs the copy but no D, and the parent pays the ordinary D only if it
returns those bytes itself.

Because a child pays for receiving its own input and code, a caller that hands on
everything it has left, less a fixed reserve for its own work afterwards, can pass an
input of any size: the copy comes out of the child's share, never out of the reserve.
What the child consumes stays charged to every enclosing budget, and what it leaves
unused is available to the caller again ([calls and rollback](calls.md)). The exact
stages and their charges are in
[section 22.7 of the protocol rules](../../../ting-protocol-rules.md#227-calls-and-deployments).

## Context

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
| 7 | Producer data | its original bytes, exactly as the block body carries them ([the body](../data/blocks.md#the-body)) |
| 8 | Remaining budget | four u64, little-endian, C/S/D/G, of the executing frame |

Any other selector traps. A top-level invocation has no calling contract; an
initializer's calling contract is its deployer, or none for a top-level deployment.

Selector 8 reports the executing frame's budget minus what it has consumed so far,
taken before the getter's own copy is charged. It lets a program give a child
exactly what it can afford instead of guessing, and it is deterministic, since it
follows from the declared budget and the consumption so far. A program that forwards
it to a child must first subtract what it will spend before the call.

The current block's own ID, its nonce, the current state root, the wall clock, local
resource timings and other contracts' storage are deliberately unavailable. A
candidate block's execution therefore does not depend on the nonce search or on the
block's eventual hash.

## Code hashes

`code_hash` takes a ContractId and returns that contract's CodeHash. It costs
`bytes(32, 1)` for copying the target, 1,900 C and one S for reading its manifest,
and `bytes(32, 1)` to copy the hash out when the buffer holds it. A program uses it
to verify which code another contract runs, for example to accept only known
implementations before calling or trusting one.

## Failures

Besides the [instruction traps](instructions.md#traps), the host traps, with status
2, for an invalid span, a `storage_set` value of `2^32 − 1` bytes, an unknown context
selector and invalid call flags. A write, deployment or event attempted in read-only
mode fails with status 11 instead, and a charge that does not fit fails with the
status of its exhausted counter. Each of these ends the invocation that made the
host call, without a result; its caller receives the status and continues. A
sub-budget that does not fit (128) and a child that cannot pay for its input or code
(3) are not failures of the caller at all: the import returns, and the caller
decides. When several faults apply at once, the one checked first decides the status
and the counters; the order is in
[section 22.10 of the protocol rules](../../../ting-protocol-rules.md#2210-failures-and-their-order).
