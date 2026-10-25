# Peer protocol

This page specifies how to talk to a reference node over its peer protocol, exactly
enough for another implementation to sync from reference nodes, serve them, and
relay blocks and transactions between them. The protocol is not consensus: it only
carries blocks, headers and transactions, in the consensus encodings of the
[protocol rules](../../../ting-protocol-rules.md), and a node may speak any other
transport besides. [Peers and synchronization](../node/sync.md) describes what the
reference node does with it.

## Endpoints

| Port | Service |
|---|---|
| 18442 | The peer protocol, over TCP, on every address unless configured otherwise |
| 18441 | The REST API over HTTP, when the node's operator turns it on; its reference is served at `/docs` |
| 18440 | The control channel, on the node's own machine only, never for peers ([security](../node/security.md#the-control-channel)) |

The production network's seed nodes are `node1.ting-node.com:18442` and
`node2.ting-node.com:18442`. A node learns of the others through the
[`addresses`](#requests) request.

## A connection

One TCP connection carries requests in one direction: the side that connected asks,
and the side that accepted answers. The reference node keeps one connection to each
of its peers and does all its asking over the connections it opened, pushing its own
blocks included; a peer that wants to ask a reference node opens its own.

1. **Connect.** The connection has five seconds to open.
2. **Hello.** The first frame must be a [`hello`](#requests) request, whole within
   five seconds and at most 4 KiB, with a version of at least 1 and the receiving
   node's ChainId. The receiving node answers with its [state](#the-state). Any
   other first frame, another version or another ChainId is answered with the error
   `peer version or genesis mismatch`, and a hello carrying the receiving node's
   own nonce with `connected to itself`; then the connection closes.
3. **Requests.** The connecting side then sends requests, and may send the next
   before the answer to the last has come: the reference node pipelines up to 64.
   Answers come in the order of the requests. A connection whose hello set
   `announce` also receives [announcements](#announcements) between answers, never
   in the middle of one.
4. **Keeping it open.** A connection without a request for 120 seconds is closed;
   the reference node sends at least one every 30 seconds.
5. **Closing.** Either side closes the TCP connection when it is done. There is no
   goodbye.

Requests are counted by cost, and requests beyond the allowance wait rather than
fail:

- a page of headers: 8;
- `state`, `inventory`, `pending` and `deferred`: 2;
- any other: 1;
- one that carries a block or a transaction: 1 more for every 64 KiB of it, a slice
  asked for with `raw` or `transaction_raw` counted once it is answered.

The allowance is 200 a second, after a burst of 2,000, so a connection draws at most
about 12 MiB of blocks a second, as many small ones or few large.

An error answer is an answer like any other, such as for a block the node does not
have, and the connection stays open after it. The connection ends without an answer
for a frame that is no message ([framing](#framing)), a request tag the node does not
know among them, and after the error answer to a second `hello` or to
[invalid data](#invalid-data).

## Framing

Each message is a frame: a `u32` little-endian length, then that many bytes, 1 to
16 MiB. The first byte is the frame's kind:

| Kind | Frame |
|---|---|
| 1 | A request: its tag, then its fields |
| 2 | An answer: its tag, then its fields |
| 3 | An error answer: its text |
| 4 | An announcement: its fields |

Fields are written one after another, with nothing between them:

| Field | Encoding |
|---|---|
| `u8`, `u16`, `u32`, `u64` | Little-endian, of that width |
| flag | One byte, 0 or 1 |
| id | A block or transaction id, or a ChainId: its 32 bytes |
| bytes | A `u32` length, then that many bytes |
| list | A `u16` count, then that many items |
| name | A `u16` length, then that many bytes of printable ASCII (0x20 to 0x7e) |
| text | A `u16` length, then that many bytes of UTF-8 |
| option | A flag, then the value if it is 1 |
| work | A `u8` length of at most 64, then the number's bytes, most significant first, with no leading zero byte; zero has none |
| address | `4` and an IPv4 address's 4 bytes, or `6` and an IPv6 address's 16, then the port as a `u16`. An IPv4 address is never written mapped into IPv6 |
| header | A block header in its consensus encoding, 294 bytes |
| block, transaction | In its consensus encoding, as bytes |

A frame is read strictly: every count and length within its bound, every flag 0 or
1, work in its shortest form, and no byte left over. A message therefore has exactly
one encoding, and a frame that breaks any of this, or names a kind or tag the node
does not know, is no message. Every frame has five seconds to arrive once its
length has, and every answer 30 seconds from its request.

## Requests

| Tag | Request | Fields, in order | Answer |
|---|---|---|---|
| 0 | `hello` | `version` `u32`, `chain_id` id, `agent` name of at most 64 bytes, `features` list of at most 16 names of at most 32 bytes, `nonce` `u64`, `announce` flag (to be told of changes), `listen` option of a `u16` (the sender's listening port) | State |
| 1 | `state` | none | State, as it is now |
| 2 | `headers` | `locator`: a list of 1 to 32 ids | Headers: up to 2,000 canonical headers following the highest locator entry on the canonical chain |
| 3 | `raw` | `id`, `offset` `u64`, `length` `u32` of 1 byte to 1 MiB | Slice of a block |
| 4 | `import` | `offset` `u64`, `total` `u64`, the block's full length, and bytes of at most 1 MiB: a block whole, or a slice of one | Taken; for a slice before the last, Received |
| 5 | `transaction_raw` | `id` of a pooled or deferred transaction, `offset` `u64`, `length` `u32` of 1 byte to 1 MiB | Slice of the transaction |
| 6 | `submit` | `offset`, `total` and bytes as for `import`, of a transaction; `not_before` option of a `u64`, for a deferred one | Taken; for a slice before the last, Received |
| 7 | `inventory` | `deferred` flag; `after` option of an id | Inventory |
| 8 | `pending` | `ids`: a list of 1 to 32 ids | Items: those of the pooled transactions, leaving out deferred ones |
| 9 | `deferred` | `ids`, as for `pending` | Items: those of the deferred transactions, with their times |
| 10 | `addresses` | none | Addresses: up to 32 peer addresses; none when the node's discovery is off |

- **`headers`.** The locator names blocks the asker has, most recent first; the
  genesis block's id always matches. With no entry on the answering node's
  canonical chain, it fails with `no common ancestor`.
- **`raw`.** Serves a block the node has executed and found valid, canonical or not,
  unless its operator marked it or a block below it invalid. Any other block, stored
  or not, is answered with `block not found`.
- **`inventory`.** Up to 1,024 ids of pooled transactions, deferred ones left out,
  or with the flag of deferred ones, in id order after `after`. When more follow,
  `next` is the last id listed.
- **`submit`.** Refused with `transaction already included` when one of the node's
  last 1,440 canonical blocks included the transaction and it succeeded there, or
  one of the last 10 included it at all.

## Answers

| Tag | Answer | Fields, in order |
|---|---|---|
| 0 | State | `version` `u32`, `chain_id` id, `agent` name, `features` list of names, `nonce` `u64`, `height` `u64`, `tip` id, `work` work, `locator` list of at most 32 ids, `pending` list of at most 128 ids, `deferred` list of at most 128 ids |
| 1 | Headers | A list of at most 2,000 headers |
| 2 | Slice | `offset` `u64`, `total` `u64`, the item's full length, and bytes of at most 1 MiB |
| 3 | Received | `u64`: the bytes of a push taken so far |
| 4 | Taken | `id` of the block or transaction, and `known` flag: the node held it already |
| 5 | Inventory | A list of at most 1,024 ids, and `next` option of an id: the last id listed, the `after` of the next page, or none at the end |
| 6 | Items | A list of at most 32 items: `id`, `not_before` option of a `u64` (set for deferred ones), then `0` and the transaction as bytes, or `1` and its size `total` as a `u64` |
| 7 | Addresses | A list of at most 32 addresses |

An error answer's text is at most 1,024 bytes, cut at a character boundary.

An Items answer carries transactions whole while all of them together fit one
slice, 1 MiB, so that it arrives within its five seconds wherever a block's slice
does; the others it lists by size alone, to be fetched with `transaction_raw`.

### The state

| Field | Content |
|---|---|
| `version`, `chain_id` | 1, and the node's ChainId |
| `agent` | The node's software and its version, such as `ting-node/0.1.0` |
| `features` | The optional messages it supports beyond its version: none are defined |
| `nonce` | The node's nonce for this run, which keeps it from connecting to itself |
| `height`, `tip`, `work` | The canonical height, tip and accumulated work |
| `locator` | At most 32 block ids: the best header's branch back from its tip, the ten most recent and then at gaps that double each time, up to 23 in all; then up to eight stored side branches, most work first; then the genesis block |
| `pending` | Up to 128 pooled transaction ids, leaving out deferred ones |
| `deferred` | Up to 128 deferred transaction ids |

### Announcements

Whenever the answering node's tip or pool sizes change, at most once every 0.2
seconds, a connection whose hello set `announce` receives: `tip` id, `height` `u64`,
`work` work, `pending` `u64` and `deferred` `u64`, the sizes of its pool and
deferred set.

## Example frames

On the production network, a hello from a node running `ting-node/0.1.0` with nonce
`0x0123456789abcdef`, asking for announcements and listening on port 18442, is these
73 bytes, its length first (hexadecimal; line breaks are presentation only):

```text
45000000 01 00 01000000
15d6549cfacd816f835c632b4c67308d1c63e642f4cbf4049368430670705107
0f00 74696e672d6e6f64652f302e312e30 0000
efcdab8967452301 01 01 0a48
```

The first `headers` request of a node that holds only the genesis block names it as
its locator:

```text
24000000 01 02 0100
24b76a52f3825b921373b6c6a41ced2a74de7dbdb34015669c10594996887459
```

And the first slice of the block at height 1, a whole MiB asked for:

```text
2e000000 01 03
000000004eba8c8bd3488debecab68017d3720b9f5a0da24ab80ffce4b57d1ea
0000000000000000 00001000
```

## Syncing from a reference node

1. Open a connection and say hello, with `announce` set to hear of new tips.
2. Ask for `headers` with a locator of the blocks you have; keep asking from the last
   header received while full pages of 2,000 come back. Check each header as the
   rules say, its proof of work and target included, before asking for more.
3. Fetch each block along the best branch with `raw`, from offset 0 with a length of
   1 MiB; the Slice's `total` is the block's full length, and the next request
   starts where the bytes so far end. Check the first slice's header against the id
   asked for.
4. Validate and execute each block by the rules. The first blocks of the production
   and development chains are their [seed](conformance.md#the-seed).
5. On an announcement, or every 30 seconds, ask for headers again.

## Pushing blocks and transactions

A block or transaction of at most 1 MiB goes whole in one `import` or `submit`, with
`offset` 0 and `total` its length; larger ones go in slices of at most 1 MiB, in
order, each starting where the last ended. The node answers Received with the bytes
so far after each slice before the last, and Taken after the whole item. Every slice
must name the same `total`, and a slice that is empty, out of order or of another
item ends the push with `slice out of order` or `empty slice`.

- A block's first slice holds its header, which sets its bound: the data capacity at
  its height. A larger `total` is invalid data.
- A transaction is bounded by the data capacity of the next block, and a larger one
  is refused with `transaction larger than the data capacity`.
- The whole item must arrive within its transfer time: a minute, and ten seconds
  more for every 1 MiB slice it spans.

A deferred transaction travels with its `not_before` time, only to a node that
keeps a deferred set, and is pooled once due and successful
([deferred transactions](../node/mining.md#deferred-transactions)).

## Exchanging pools

To learn what a node holds, page through `inventory` from no `after` until `next` is
none, once for pooled and once for deferred transactions. Ask for the ones you lack
with `pending` or `deferred`, 32 ids at a time, and fetch any listed by size alone
with `transaction_raw`. The reference node submits a peer's transactions to its own
pool as relayed by that peer's address ([the mempool](../node/mining.md#the-mempool)).
It reads a peer's inventory only until it holds as many ids as its own pool or
deferred set can, and sends the peer nothing past the last id it read, since it
cannot tell what the peer holds there. It sends each transaction to a peer, and
fetches each from it, once a connection, whether or not it was taken; one refused
with `mempool is full` or `deferred set is full` is tried again once its own tip
has moved, at most the next block's data capacity of them each way at each tip.

## Errors

| Error | Meaning |
|---|---|
| `peer version or genesis mismatch` | The hello was missing, of another version or of another chain; the connection closes |
| `connected to itself` | The hello carried the node's own nonce; the connection closes |
| `unknown peer method` | A second hello; the connection closes |
| `invalid header: …`, `invalid block: …` | The header or block breaks the rules: [invalid data](#invalid-data) |
| `block not found` | `raw` of a block the node does not serve |
| `transaction not found` | `transaction_raw` of a transaction the node does not hold |
| `no common ancestor` | `headers` whose locator names no canonical block |
| `branch too far behind the tip` | A block on a branch more than a day's work behind the tip, whose header the node has not indexed |
| `timestamp ahead of the clock` | A block more than 7,200 seconds ahead of the node's clock; offer it again later |
| `block marked invalid by this node's operator` | A block the operator marked invalid, or one above it; held against no peer |
| `mempool is full`, `deferred set is full` | No room for the transaction |
| `not_before is more than a year ahead` | A deferred transaction scheduled too far ahead |
| `transaction larger than the data capacity` | A transaction beyond the next block's data capacity |
| `transaction already included` | A recent block included the transaction |
| `slice out of order`, `empty slice` | A push broke its order |

Other texts may come from the node's own state, such as a storage error; they are
answers too, and the connection stays open.

## Invalid data

An answer `invalid header: …` or `invalid block: …` proves that the asking side sent
data breaking the consensus rules, as does a peer's answer that does. The reference
node then disconnects and, unless the peer is configured or on the same machine,
bans its address for a day: a banned address's connections close at once, before
any frame. A block that this node's own execution rejects at a seed height is the
node's fault, not the peer's, and holds nothing against it
([conformance](conformance.md#the-seed)).

## Serving a reference node

A reference node that connects to you sends, after its hello, in rounds:

- `headers` with its locator, at the first round, after your announcements and
  every 30 seconds, paging while full pages come back;
- `raw` for the blocks it wants along the best header's branch, up to 64 slices in
  flight;
- `state`, then `import` of up to 32 of its blocks past your locator, when its tip
  has more work than yours;
- `inventory`, `pending`, `deferred`, `transaction_raw` and `submit` to exchange
  pooled and deferred transactions;
- `addresses` every ten seconds while it looks for peers, otherwise every ten
  minutes, when its discovery is on.

It waits 30 seconds for each answer, ends the connection on a frame that is no
message or an answer beyond its bounds, and treats invalid data as above.

## Versions

The protocol's version is 1. A later version keeps every message of the earlier
ones and only adds, so a node takes a peer of any version from 1 up and speaks the
messages both know; a new optional message is listed in `features` first, and a node
uses it only with a peer that lists it. Unknown features are ignored. `agent` is
shown by `ting peers`; nothing depends on it.

## Limits

- Frames of 1 byte to 16 MiB, a `hello` frame of at most 4 KiB, and every field
  within its bound. A frame's buffer grows as its bytes arrive, so a length alone
  reserves nothing
- Blocks and transactions larger than 1 MiB travel in slices of at most 1 MiB; a
  block of at most its height's data capacity and a transaction of at most the next
  block's, within its transfer time
- 125 inbound connections at once, at most 8 of them from one address range (an IPv4
  address or an IPv6 /64; the node's own machine excepted). With every slot taken, a
  new connection takes the place of the newest one of the network group holding the
  most (an IPv4 /16 or an IPv6 /32, a local address its own range), if that group
  holds at least two more than the newcomer's; otherwise it is closed. Many
  addresses of one operator's network therefore cannot hold every slot
- 200 request units a second per inbound connection after a burst of 2,000, and 120
  seconds without a request before it is closed
- At most 32 addresses in or from one `addresses` answer
- Five seconds per connection attempt and per frame, 30 seconds per answer

No message reaches administration: peer configuration, mining and shutdown exist
only on the control channel. Shared addresses are hints; whom to connect to stays
each node's own choice. There is no NAT traversal or transport encryption: a node
that cannot be reached connects out.
