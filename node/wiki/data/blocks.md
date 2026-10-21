# Blocks, transactions and receipts

A block is a fixed-size header, which carries the proof of work and commits to
everything else, and a body with the transactions and one receipt for each. A
transaction asks the chain to run a program, and its receipt records what happened.
This page describes the fields that explorers show and integrators build and read.
The exact rules are in
[section 4 of the protocol rules](../../../ting-protocol-rules.md#4-blocks-transactions-and-receipts).

The types in the tables are those of [canonical bytes](encoding.md#types).

## The header

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

Every field has a fixed width, so every header is exactly 294 bytes. Header-first
synchronization and light clients rely on that: nodes can check headers, and their
proof of work, before they fetch bodies.

A block's BlockId is the hash of its header, `H("block", encode(Header))`, and it is
the BlockId that has to meet the target ([proof of work](../consensus/proof-of-work.md)).
The header commits to the rest of the block through four hashes:

- `transactions_root` and `receipts_root`, [ordered commitments](encoding.md#ordered-commitments)
  to the transactions and the receipts in the body;
- `producer_hash`, the hash `H("producer", producer_data)` of the miner's producer data;
- `state_root`, the root of the [state tree](state-tree.md) after the block's
  transactions have run.

The four votes are in resource-vector order: C, S, D, G. Each is `0` to hold, `1` to
increase or `2` to decrease that capacity; any other value makes the header invalid.
What the votes do is on [capacity voting](../consensus/capacity-voting.md).

### Header validity

A header must sit one height above its parent, name the network's ChainId and
carry the target its ancestry requires
([time and difficulty](../consensus/difficulty.md), which also covers timestamps).
Its three roots and `resources_used` must equal what the validator computes for
itself from the body and the parent state. The exact list is in
[section 4.2](../../../ting-protocol-rules.md#42-header-validity), and what a
validator checks for a whole block is on [how nodes agree](../consensus.md).

## The body

The body holds the miner's producer data, the number of transactions, the
transactions, and then one receipt per transaction in the same order, each
transaction and each receipt framed by its length
([the exact layout](../../../ting-protocol-rules.md#43-the-body)).

Producer data is whatever bytes the miner chooses, of any length. It is paid for as
block data, the header's `producer_hash` commits to it, and programs can read it
through the [host interface](../vm/host.md).

Receipts travel with the block for transport and proofs, but a validator derives its
own by execution and requires them to match exactly; it never trusts the supplied
bytes. An empty body, with no transactions, is valid.

Transactions execute once each, one after another in body order. The first runs
against the parent's state; each later one sees that state plus the successful
effects of the transactions before it. Receipt i belongs to transaction i.

## Transactions

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

The **budget** is a ceiling, not a payment. It may exceed the block's capacity,
because only what the block's transactions actually consume together has to fit
([block capacity](../resources/capacity.md)). Only the data budget has bounds of its
own: at least the transaction's own place in the block, and at most `2^32 − 1`
([the exact bounds](../../../ting-protocol-rules.md#123-d-data)). A miner may apply
stricter admission rules of its own, but these are policy, not validity.

```text
TransactionId = H("transaction", ChainId || encode(Transaction))
```

The hash covers the ChainId, so a transaction has a different ID on every network,
and the whole envelope, so changing even the budget changes the ID.

### No sender and no replay protection

A transaction says nothing about who sent or authorized it, and the protocol keeps no
list of transactions it has already seen: there is no global replay protection or
uniqueness rule. The same bytes can be included again, even twice in one block, and
each **occurrence** executes normally. That is why the API reports a list of
occurrences for an ID.

Authorization belongs to applications. An application can have its users sign a
separately encoded intent, which should bind the chain, the application, the action
and the application's own replay-protection data. Signing the complete envelope,
including the signature itself, is not required. See
[authentication and replay](../applications/authentication.md).

## Receipts

A receipt is the outcome of one transaction, as every validator computes it: the
TransactionId, a status, the resources actually used, the result bytes and the
events kept.

| Field | Type | Content |
|---|---|---|
| `transaction_id` | `bytes32` | The TransactionId |
| `status` | `u8` | The outcome, below |
| `resources_used` | `ResourceVector` | What the transaction consumed |
| `result` | `Bytes` | Result bytes of the top-level invocation |
| `event_count` | `u32` | The number of events, m |
| `events` | `Event[m]` | The retained events, in order |

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

No other status value is valid. A successful deployment's result is the ContractId
it created, as described in [deploy and invoke](../vm/lifecycle.md).

A failed transaction has no effect on state and keeps no events: a receipt with any
status other than 0 has zero events. Its receipt still records what it consumed,
along with any result bytes a revert returned. Diagnostic messages, stack traces,
error strings, timings and compilation details are never part of a receipt, so
implementations cannot disagree about them and they cannot affect any root.

Inside a program, the host's call and deploy functions report a child's outcome with
the same status codes, or with call error 128 when no child was run at all. 128 is a
value returned to the calling program, never a receipt status
([when it is returned](../../../ting-protocol-rules.md#227-calls-and-deployments)).

## Events

An event is the emitting ContractId plus bytes defined by the program. Events are
kept, in emission order, only from invocations that succeed and whose callers succeed
too.

| Field | Type | Content |
|---|---|---|
| `emitting_contract` | `bytes32` | The ContractId whose code emitted the event |
| `payload` | `Bytes` | Bytes chosen by the program |

An invocation that fails, whether by revert, trap or exhaustion, contributes no
events, and neither does anything it called. When the top-level invocation fails,
the receipt keeps none at all. Emitting an event still costs data even if it is
later discarded; see [what each counter measures](../resources/counters.md).
