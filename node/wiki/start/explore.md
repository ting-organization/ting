# Explore the chain

With REST enabled, a node serves three views from one address, by default
`http://127.0.0.1:18441`.

| View | Address | What it is for |
|---|---|---|
| Node | [/](/) | The blockchain explorer |
| Wiki | [/wiki](/wiki) | This guide |
| API | [/docs](/docs) | Interactive reference for the REST API |

All three are bundled into the node and work offline, and each has its own
navigation.

## The explorer

- **Overview** shows the height, accumulated work, pending transactions, the active
  capacity for each counter, recent blocks, the ChainId and the current state root.
- **Blocks** lists the canonical chain. A block page shows its header, its
  transactions with their receipts and resource use, and their events.
- **Pending transactions** lists what the node holds in its mempool.
- A **transaction** page shows every canonical occurrence of that ID, with a summary
  of its envelope, since the same transaction can be included more than once; while
  it is queued, it shows the pending envelope instead.
- A **contract** page shows the immutable manifest, meaning the code hash, size and
  price, and looks up any 32-byte storage key.

Search accepts a block height, block hash, transaction ID or contract ID. The
explorer's pages are `/`, `/blocks`, `/blocks/{id}`, `/transactions/{id}`,
`/contracts/{id}` and `/mempool`.

## The REST API

The same data is available as JSON under `/v1/`. Identifiers are 64-character
lower-case hex, binary fields are lower-case hex, and resource vectors are in C, S,
D, G order.

| Method | Path | Answer |
|---|---|---|
| GET | `/health` | `ready`, with the height and tip, once the node answers |
| GET | `/v1/status` | Name, version, network (`production`, `development` or `other`), ChainId, genesis block, height, tip, accumulated work, state root, active capacities, pool and deferred counts, producer data, stored blocks, state records and uptime |
| GET | `/v1/blocks?from=1&limit=20` | Canonical blocks from height `from` |
| GET | `/v1/blocks/{height-or-hash}` | Block summary: header fields, accumulated work, whether canonical, producer data, size, transaction ids and receipts |
| GET | `/v1/blocks/{height-or-hash}/raw?offset=0&length=1048576` | The bytes of a block the node has executed, canonical or on a side branch, as hex, whole or sliced, with `offset` and the block's `total` length |
| GET | `/v1/transactions/{id}?from=0&limit=20` | Pending or deferred status, the pending envelope, and a page of canonical occurrences with their receipts |
| POST | `/v1/transactions` | Submit `{"hex": "…"}`, a canonical transaction; with `"not_before"`, deferred |
| GET | `/v1/mempool` | Pending ids, sizes and budgets, the pool's total bytes, and the deferred set |
| GET | `/v1/events?from=1&limit=20` | Events grouped by canonical block, each block with its height, id and timestamp |
| GET | `/v1/contracts/{id}` | The immutable manifest: code hash, code size and code price |
| GET | `/v1/contracts/{id}/storage/{key}?proof=false` | The current canonical record, `null` if absent, with the state root and tip it was read at; with `proof=true` also its Merkle proof |
| POST | `/v1/storage` | Many records from one state: `{"reads": [{"contract": "…", "key": "…"}, …]}`, 1 to 1,024 of them, answered in that order with the state root and canonical tip they were all read at; with `"proof": true` each with its Merkle proof |
| GET | `/v1/metrics` | The status document: height, pool and state record counts, uptime |

A height selects a canonical block; a hash selects any block the node has validated,
on a side branch too. Manifests and storage records come with the state root they
were read from. Records that belong together, such as the parts of one identity,
are best read with one `POST /v1/storage`: separate reads may fall on either side of
a new block. A submission answers with the transaction `id` and `known`, true when
the node already held it; a deferred one also with `deferred`, `not_before` and
`expires`. Admission says only that the transaction is well formed: see
[the mempool](/wiki/node/mining#the-mempool) and
[deferred transactions](/wiki/node/mining#deferred-transactions).

The schema is served at [/openapi.json](/openapi.json) for SDKs and integrations,
and [/docs](/docs) renders it with "Try it out" requests against your node. Mining has
no REST surface; it runs over the [control channel](/wiki/node/security#the-control-channel).

## Pages and errors

- `limit` is at most 100 and defaults to 20. Block and event pages start at height
  `from`, default 1; occurrence pages start at offset `from`, default 0, and say with
  `has_more` whether another page follows. These pages and `/raw` refuse any other
  query parameter with `400`; the other endpoints ignore query parameters.
- A JSON page stops before 16 MiB, so it may hold fewer entries than `limit`; follow
  `next` to continue. An entry too large for any page is refused with an error that
  points to the raw block.
- `/raw` returns a block whole only when it fits one response, about 8 MiB. Any
  block can be read in slices with `offset` and `length`, at most 1 MiB each;
  every answer carries its `offset` and the block's `total` length.
- A transaction lookup gives each envelope's `hex` up to 1 MiB; a larger one has
  `hex` null and its size in the summary's `encoded_bytes`, and a canonical one is
  read from its block's slices.
- Pages report the canonical `tip` they were read at.

Errors are JSON with an `error` string:

| Status | Meaning |
|---|---|
| `400` | A malformed request: a bad identifier, page or transaction |
| `404` | No such block, transaction, contract or endpoint |
| `413` | A submission's body over twice the data capacity of the next block and 4 KiB more, the most a transaction in hex can need; any other body over 16 MiB |
| `415`, `422` | A submission that is not JSON, or JSON of the wrong shape |
| `503` | The node cannot serve the request now: REST's share of its request queue or its HTTP slots (32 per processor core) are full, its pool or deferred set is full, or it has stopped |
| `504` | The request took more than 35 seconds (a submission, ten more for every MiB its body may have), or the node's own 30-second request deadline passed |

## What the API can and cannot tell you

The API reports what this node has validated; it does not replace validation.
A storage response includes the state root it was read from, and asked for, the
record's [Merkle proof](/wiki/data/state-tree#proofs): a client that checks it
against the state root of a header it trusts no longer trusts the node for the
value. Without a proof, it does. Reorganizations can change recent history,
and each view tells you which canonical tip it reflects.

## The wiki

The wiki is built into the executable, works offline and uses no external assets. It
leads from why Ting exists, through a tour and hands-on guides, to concept pages with
sub-pages and the design decisions; the API reference is one view beside it, not its
purpose. Pages are served at `/wiki` and `/wiki/{path}`, such as
[/wiki/consensus/difficulty](/wiki/consensus/difficulty), and a path with a trailing
slash redirects to the page. The worked protocol vectors are served as
[protocol-vectors.json](/wiki/reference/protocol-vectors.json).

> [!NOTE]
> **Public by design.** Reads and transaction submission need no token: programs
> decide who may do what. Mining, peers, shutdown and API toggling are only available
> through the authenticated CLI. See [Access and exposure](/wiki/node/security).
