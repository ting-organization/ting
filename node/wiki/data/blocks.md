# Blocks, transactions and receipts

A block's canonical bytes are its header followed by its body. All layouts on this
page use the types and rules of [canonical bytes and hashing](/wiki/data/encoding).

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

The fields total 294 bytes, so every header is exactly 294 bytes. Header-first
synchronization and light clients rely on that: nodes can check headers before they
fetch bodies.

The BlockId is the hash of the header, `H("block", encode(Header))`; how it meets the
target is on [proof of work](/wiki/consensus/proof-of-work).

```text
producer_hash     = H("producer", producer_data)
transactions_root = VectorRoot("transactions", transactions)
receipts_root     = VectorRoot("receipts", receipts)
```

`VectorRoot` is the [ordered commitment](/wiki/data/encoding#ordered-commitments)
and `state_root` is the root of the [state tree](/wiki/data/state-tree).

The four votes are in resource-vector order: C, S, D, G. Each is `0` to hold, `1` to
increase or `2` to decrease that capacity; any other value makes the header invalid.
What the votes do is on [capacity voting](/wiki/consensus/capacity-voting).

### Header validity

Besides decoding canonically, a header is valid only when:

- its `height` is its parent's height plus one;
- its `chain_id` is the ChainId of the pinned [genesis](/wiki/consensus/genesis);
- its `target` equals the target derived from its ancestry, as described in
  [time and difficulty](/wiki/consensus/difficulty), which also covers the
  timestamp rules;
- its three roots and `resources_used` equal the results the validator computes
  itself from the body and the parent state.

The complete order of checks is in [how nodes agree](/wiki/consensus).

## The body

| Field | Type | Content |
|---|---|---|
| `producer_data` | `Bytes` | Chosen by the miner |
| `transaction_count` | `u32` | The number of transactions, n |
| `transactions` | `Bytes[n]` | Each transaction's encoding, framed as `Bytes` |
| `receipts` | `Bytes[n]` | Each receipt's encoding, framed as `Bytes`, in transaction order |

The body holds the producer data, a count, the transactions, and then one receipt
per transaction. Producer data is chosen by the miner, has no fixed size, and is
paid for as block data; its hash must match the header's `producer_hash`. How block
data is counted is on [what each counter measures](/wiki/resources/counters).
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

Both kinds declare a **budget**. Each component may be any `u64`, including one larger
than the current block capacity. Only what the block's transactions actually consume
together must fit: a debit that would exceed the block's capacity makes the block
invalid, and a validator stops there (see
[block capacity and bounds](/wiki/resources/capacity)).
The data budget does have a floor, the envelope's own data cost, described on
[what each counter measures](/wiki/resources/counters), and a ceiling of `2^32 − 1`.
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
including the signature itself, is not required. See
[authentication and replay](/wiki/applications/authentication).

## Receipts

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
ContractId, as described in [deploy and invoke](/wiki/vm/lifecycle).

A failed transaction has no effect on state and keeps no events: a receipt with any
status other than 0 has zero events. Its receipt still records what it consumed,
along with any result bytes a revert returned. Diagnostic messages, stack traces,
error strings, timings and compilation details are never part of a receipt, so
implementations cannot disagree about them and they cannot affect any root.

Inside a program, the host's call and deploy functions report a child's outcome with
the same status codes, or with call error 128 when no child was run at all. 128 is a
value returned to the calling program, never a receipt status. Which problems return
128, which ones fail the executing frame, and in what order they are checked is on
[the host interface](/wiki/vm/host#calls-and-deployments).

## Events

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
[what each counter measures](/wiki/resources/counters).
