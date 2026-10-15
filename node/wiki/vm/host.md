# The host interface

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

## Conventions

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
  value is at most `2^32 − 2` bytes ([storage writes](#storage-writes)).
- **Bytes are opaque.** Input, results, events and storage values have no protocol
  format. Each program defines its own encodings.
- **Bytes cost compute.** Every copy across the interface costs `bytes(n)`, the byte
  price of the program's [memory tier](/wiki/resources/prices#memory-tiers) for
  each started 32 bytes.
- **Records cost compute too.** Every record read is debited together with 1,900 C
  for hashing the record's key, and every value written with the price of hashing it
  into its leaf ([hashing](/wiki/resources/prices#hashing)).

## Signatures

Every parameter and every ordinary result is `i32`. An import's type must match
its row exactly ([the binary format](/wiki/vm/format#sections-in-detail)).

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
ordinary call and 1 for a [read-only](/wiki/vm/calls#read-only-mode) one.

## Spans and copying

- Pointers, lengths, capacities and selectors are read as unsigned 32-bit values, and
  a length a getter returns is one too. A program's memory is at most 4 GiB, so no
  span, and no value a program writes or receives, is longer than `2^32 − 1` bytes.
- A span of `ptr` and `len` is valid when the module exports a memory and
  `ptr + len`, computed in mathematical addition without wrapping, is at most the
  memory's current size. A zero-length span still needs an exported memory and an
  end in range.
- An operation with several argument spans validates all of them before it
  charges or copies any argument byte. It then debits `bytes(total length)` C in
  one atomic step and copies. `call` and `deploy` are the exception: the caller
  pays only for the target and the budget, and the child pays for receiving its
  input and code (below).
- Copying `n` bytes back into guest memory debits `bytes(n)` C.
- A debit written `[c, s, d, g]` charges the four counters C, S, D and G in one
  atomic step.

## Getters

`input`, `context`, `storage_get`, `code_hash` and `call_result`:

1. validate the whole output span, `out_ptr` and `capacity`, even when the value
   turns out to be absent or shorter;
2. find the value, with the import's own argument copies and reads (below);
3. if the value fits the capacity, debit `bytes(length)` C and copy it; if not,
   copy nothing and charge no output copy;
4. return the value's full length, or `2^32 − 1` for an absent record or contract.

A getter validates its span and debits its copy before any byte moves, and
copies straight from the host's own data, so a length query or a short buffer
does no work in proportion to the value. Calling a getter again repeats its
charges; nothing is discounted for having been read before.

`storage_get` validates the output span, then validates and copies its 32-byte
key (`bytes(32)`), then reads the record in the executing contract's storage
([namespaces](/wiki/data/records#namespaces)), debiting `[1,900, 1, 0, 0]` for the
key and the read, then copies the value as above. The read needs only the record's
length until the value is copied. `code_hash` does the same with a 32-byte target,
reading that contract's manifest; when the contract exists it copies its 32-byte
CodeHash.

## Storage writes

`storage_set` sets a record of any length; `storage_delete` deletes one. Both:

1. fail with status 11 in a read-only invocation, before any span check;
2. validate the key span and, for `storage_set`, the value span, which must also be
   shorter than `2^32 − 1` bytes, then debit `bytes(32 + value_len)` C, or
   `bytes(32)` for a deletion, and copy;
3. debit `[1,900, 1, 0, 0]` for the key and the read of the old record, of which
   only the length is needed;
4. debit, in one atomic step,

   ```text
   [ C = 100 + 900 × ceil((len(value) + 62) / 64), S = 1, D = 0, G = growth ]
   ```

   the price of hashing the value into its leaf, with no hash and zero growth for a
   deletion ([growth](/wiki/resources/growth));
5. apply the change.

Both S units are charged for a write that changes nothing and for deleting an
absent record. The key is 32 bytes and the value at most `2^32 − 2` bytes, one
fewer than memory allows, so that no record's length can be read as absence; a
value span of `2^32 − 1` bytes traps like an invalid span. There is no other size
limit. The runtime's own writes of a
manifest and a code record during deployment follow the same rule.

## Output

`return` and `revert` validate their span, debit `[bytes(n), 0, n, 0]` atomically
for `n` result bytes, copy the bytes and end the invocation, with status Success
or Revert. `revert` rolls back the invocation's effects
([checkpoints](/wiki/vm/calls#checkpoints)); the bytes are its result either way.

`emit` fails with status 11 in a read-only invocation, then validates its span,
debits `[bytes(n), 0, 36 + n, 0]` atomically and appends an event holding the
executing ContractId and the `n` payload bytes ([events](/wiki/data/blocks#events)).

## Calls and deployments

`call`:

1. traps unless flags is 0 or 1;
2. validates the target, input and budget spans, debits `bytes(32 + 32)` C and
   copies the target and the budget;
3. empties the last-result buffer;
4. returns 128 if the requested budget exceeds, in any counter, what the caller
   has left; no child is created;
5. otherwise opens the child with that budget and debits `bytes(input_len)` C, at
   the caller's memory tier, as the child's first debit. If it does not fit, the
   child ends with status 3 having consumed nothing, and `call` returns 3;
6. otherwise copies the input and runs the child, read-only when flags is 1 or the
   caller is itself read-only, and returns the child's status, keeping its result
   bytes in the last-result buffer.

`deploy`:

1. fails with status 11 in a read-only invocation, before any span check;
2. validates the code, input and budget spans, debits `bytes(32)` C and copies the
   budget;
3. empties the last-result buffer;
4. returns 128 if the requested budget does not fit, as for `call`; no ordinal
   is taken and nothing is installed;
5. otherwise opens the deployment with that budget and debits
   `bytes(code_len + input_len)` C, at the caller's memory tier, as its first
   debit. If it does not fit, the deployment ends with status 3 having consumed
   nothing, before taking an ordinal, and `deploy` returns 3;
6. otherwise copies the code and the input and runs the
   [deployment](/wiki/vm/lifecycle#deployment) with that budget and returns its
   status. On success the last-result buffer holds the new 32-byte ContractId; on
   failure, the failed initializer's result bytes, or nothing when the deployment
   failed before its initializer ran or after it succeeded, at the identifier debit.

A child pays for receiving its own input and code, so a caller that hands on
everything it has left, less a fixed reserve for its own work afterwards, can pass
an input of any size: the copy comes out of the child's share, never out of the
reserve.

The child's status is a receipt status code ([receipts](/wiki/data/blocks#receipts)).
128 is only ever a host return value, never a receipt status. The last-result
buffer is empty when an invocation starts and is replaced by every `call` or
`deploy` attempt. `call_result` is a getter over it: reading it costs copy C but
no second D, and returning it from the parent charges the parent's ordinary
result D. What the child consumed stays charged to every enclosing budget, and
what it left unused is available to the caller again
([calls and rollback](/wiki/vm/calls)).

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
| 7 | Producer data | its original bytes, exactly as the block body carries them ([the body](/wiki/data/blocks#the-body)) |
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

## Code hashes

`code_hash` takes a ContractId and returns that contract's CodeHash: `bytes(32)` for
copying the target, 1,900 C and one S for reading its manifest, and `bytes(32)` to
copy the hash out when the buffer holds it. A program uses it to verify which code another contract
runs, for example to accept only known implementations before calling or trusting
one.

## Failures and their order

| Failure | When it is checked | Outcome |
|---|---|---|
| A writing operation in read-only mode | before any span check | the executing invocation fails with status 11 |
| An invalid span, a stored value of `2^32 − 1` bytes, an unknown selector or an invalid call flag | before any charge for the spans | the executing invocation traps, status 2 |
| A debit that does not fit | at that debit | the invocation fails with the first exhausted counter in C, S, D, G order ([atomic debits](/wiki/resources/budgets#atomic-debits)) |
| A sub-budget larger than what is left | after copying the target and the budget | `call` or `deploy` returns 128; the caller continues |
| A child that cannot pay for receiving its input or code | at that debit, the child's first | the child ends with status 3 having consumed nothing; the caller continues |

The first three end the invocation that made the host call, without a result;
its caller receives the status and continues. Besides the instruction traps
([traps](/wiki/vm/instructions#traps)), invalid spans, a stored value of `2^32 − 1`
bytes, unknown selectors and invalid call flags are the host's only causes of a trap.
