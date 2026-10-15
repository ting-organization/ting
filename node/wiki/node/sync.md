# Peers and synchronization

The reference node synchronizes with the peers it finds through the network's seed
nodes, those named in its configuration, and those it learns of from both. It
follows the branch with the most work, at any depth:
there is no point beyond which it stops reorganizing, so a network that split heals
by itself once its parts reconnect.

![How a node follows the chain: headers first, then blocks along the best header's branch, connected once all are stored](../diagrams/sync.svg)

The transport is a small binary protocol over TCP ([messages](#messages)): blocks,
headers and transactions travel in their consensus encodings, framed with the few
fields each message needs. It is separate from the consensus rules and can be
replaced without affecting them.

## Connections

A node keeps one connection open to each of its peers: every configured peer, and
the discovered peers it currently keeps (see [Finding more peers](#finding-more-peers)).
It skips its own peer address. Over each connection the peer tells the node when its
tip changes, and the node follows:

![One connection: the node follows the peer's chain, hears of its new tips, pushes its own and swaps pools](../diagrams/connection.svg)

1. **Hello.** The node sends `hello` with its protocol version, its software, its
   features, its ChainId, its nonce, `announce`, and with discovery on, its
   listening port. The peer answers with its own hello. A version below 1 or another
   ChainId ends the connection ([versions](#versions)).
2. **Rounds.** The node then works in rounds, each doing what is due, in this order:
   - **Headers**, at the first round, when the peer announces a new tip, and at every
     keepalive: the node sends its locator with `headers`, and keeps asking from the
     last header received while full pages of 2,000 come back, up to eight pages a
     round. Each header is checked as it arrives and indexed, or checked without
     being kept ([headers first](#headers-first)).
   - **Blocks**: it asks the chain which blocks to fetch from this peer, up to 16 at
     a time along the best header's branch, fetches each with `raw`, slice by slice,
     and stores it ([blocks](#blocks)). A peer that does not have one is passed over
     until it announces a new tip, and the others are asked.
   - **Push**, when this node's tip has changed and has more work than the peer's:
     from the peer's locator, asked with `state`, with the last block the peer took
     from this connection's pushes put first, it finds their best common block and
     sends up to 32 blocks after it, in order, with `import`: a block that fits one
     1 MiB slice whole, a larger one [slice by slice](#blocks). The last block taken
     matters past a deep fork, where a locator thinned out by its doubling gaps may
     name no shared block but genesis.
   - **Pending and deferred transactions**, when either pool has changed and at
     every keepalive: for each, it asks the peer for its `inventory`, the ids it
     holds, page by page, and compares it with its own. It submits to the peer the
     transactions the peer lacks, deferred ones with their `not_before` times, and
     fetches the ones it lacks itself by id, 32 at a time, and submits those locally
     as relayed by that peer. A transaction larger than one 1 MiB slice travels in
     slices both ways ([transactions in slices](#transactions-in-slices)).
   - **Addresses**, with discovery on: it asks for `addresses` and adds what it
     receives to its address book, every ten seconds while it has fewer peers than it
     wants, otherwise every ten minutes.
3. **Waiting.** Between rounds the connection waits for a change: at this node a new
   tip, pool or stored block; at the peer an announcement. After 30 seconds without
   one, ten while the node looks for more peers, the keepalive runs a round anyway, so
   the connection stays open and nothing is missed. A round that left header pages or
   blocks to push is followed by the next at once; rounds start at least 0.1 seconds
   apart.

**Announcements.** A peer that asks for them in its `hello` is told, between answers,
whenever the answering node's tip or pool sizes change, at most once per 0.2 seconds:
its tip, height and accumulated work, and the sizes of its pool and deferred set.

So over one connection a new tip travels both ways at once: the answering node
announces its own, and the connecting node pushes its own. Configuring one side is
enough. A node that cannot be reached from outside, behind a NAT without a forwarded
port, takes part fully this way: it connects out, hears its peers' tips over its own
connections, and pushes its blocks over them. Its announced listening address is
never confirmed by a connection, so it is never shared ([finding more
peers](#finding-more-peers)).

**Failures.** A connection whose request fails, or gets no reply within 30 seconds,
ends. The blocks it was asked for go back to the other connections, and `ting peers`
shows each peer's last success, or its last attempt and error. A configured peer is
connected again a minute after the failed attempt; a discovered one waits a minute,
doubling with each further failure up to an hour, and is replaced meanwhile.

A refusal that is one node's policy is not a failure, and the connection stays open
past it:

- A block on a branch more than a day's work behind the tip, whose header the node
  has not indexed, is answered with `branch too far behind the tip` and neither
  stored nor executed ([headers first](#headers-first)). The pushing node stops
  pushing.
- A block whose timestamp is more than 7,200 seconds ahead of the receiving node's
  clock is answered with `timestamp ahead of the clock` and not stored, as
  [Time and difficulty](/wiki/consensus/difficulty#timestamps) asks. The pushing
  node stops pushing, since the block's descendants would wait too; a node checks its
  own clock the same way before it takes a header. The block is offered again later
  and accepted once the clock has caught up.
- A block the operator marked invalid, or one above it, is answered with
  `block marked invalid by this node's operator`, and its header the same way
  ([overriding validity](/wiki/node/storage#overriding-validity)). It is not stored,
  and held against no peer.
- A transaction either side does not take, such as one refused with
  `mempool is full`, is skipped and the exchange continues with the next one. A
  transaction relayed by a peer that a recent block included is refused with
  `transaction already included` ([the mempool](/wiki/node/mining#the-mempool)), so
  a transaction just mined does not travel back into the pools: for a day if it
  succeeded, for ten blocks if it failed.

Deferred transactions travel only to peers that keep a deferred set, because a peer
without one would pool them at once. For the same reason, pooled transactions that
are still in the deferred set are left out of the pending exchange.

## Invalid data

A header that does not check, a block whose body does not match its header, and a
block whose execution fails prove that the peer that sent them does not follow the
rules: an honest node never relays them. The answer to such data starts with
`invalid `, as in `invalid header: proof of work`, and the connection it came over
ends. A block stored first and failing only when its branch is connected is traced
back to the peer it came from. Since a peer is held to every block it sends, a node
serves with `raw` only blocks it has executed and found valid, as Bitcoin does: an
honest node never passes on a block it has not checked, even one it stored. Nor does
it pass on a block its operator marked invalid, or one above it.

That peer is also banned for a day, with its address range: its IPv4 address, or
the /64 of its IPv6 address, which one subscriber usually holds whole. Addresses in
the range leave the address book and are not learned again, connections from it are
refused, open connections from it end at once, and connections to it within two
seconds. Two kinds of peer are disconnected but never banned: configured peers,
which the operator chose, with their ranges, and peers on the same machine, since a
ban by address would cover every node on it. Configuring a peer lifts a ban on its
range. Bans are kept in memory, at most 4,096
at once, the one ending soonest giving way to a new one, so a restart ends them.

A peer that keeps sending headers on a branch that failed here may be following
other rules than this node; when that branch carries much more work than the tip, the
node [warns](/wiki/node/storage#warnings) that it may be the one out of step.

Some failures look like misbehavior without proving it, and cost the peer nothing
beyond the request: headers that do not follow the chain they continue, a chain that
differs when fetched again, as after a reorganization on the peer, and transactions
either side does not take.

## A stale tip

When the tip has not moved for ten minutes, the node may be cut off from the rest of
the network by peers that do not relay blocks. It then connects one discovered peer
more than it otherwise keeps, until the tip moves again; then the discovered peer
whose last new block came longest ago is dropped. This follows Bitcoin; on a quiet
network it costs one extra connection.

## Finding more peers

A node on the production network needs no configured peer: the node carries the
names of the network's seed nodes, looks them up as it starts, and learns from
them. On any network, one configured peer is enough. A node learns further
addresses from its peers and keeps connections to up to `max_peers` of them, eight
unless configured otherwise. This follows the address exchange of Bitcoin, kept small: no
separate discovery protocol, no distributed hash table, nothing beyond the
connections above.

**Where addresses come from.** A node keeps an address book of other nodes'
listening addresses, filled from four sources:

- on the production network, its seed nodes: `host:port` names built into the node,
  each looked up for at most five seconds at every start; a name that does not
  resolve is passed over;
- its configured peers;
- the listening port each connecting peer announces in its `hello`, taken at the IP
  address the connection came from;
- the `addresses` its peers share, at most 32 from one answer, each only if the
  sending peer could reach it (below).

**What a node shares.** An `addresses` answer holds at most 32 addresses in random
order, and only addresses the node has itself completed a round with in the last 24
hours, without a failure since. It never includes the asking peer's own announced
address. An address reaches only peers that could reach it too:

| Address | Shared with and accepted from |
|---|---|
| Public | Any peer |
| Private: 10/8, 172.16/12, 192.168/16, 100.64/10, 169.254/16, fc00::/7, fe80::/10 | Peers at private or loopback addresses |
| Loopback | Peers at loopback addresses |
| Unspecified, multicast, broadcast, documentation, port 0 | Nobody |

**Choosing peers.** The node connects to every configured peer other than its own
listening address, whatever `max_peers` says, and again a minute after a failed
attempt. Up to `max_peers` connections in all, the node adds discovered peers from
its book: those it has completed a round with first, most recent first, then untried
ones in random order. Among public addresses it keeps at most one discovered peer
per network group (an IPv4 /16, an IPv6 /32), so a single operator's address range
cannot take every slot. A discovered peer whose connection fails is replaced, and
its address is not tried again for a minute, doubling with each further failure up
to an hour.

**Probing.** When every slot is taken, the node tests one untried address every two
minutes with a connection that only exchanges `hello`. An address that answers can
then be shared, so new nodes become known even in a full network.

**Itself.** Each run of a node picks a random 64-bit nonce and sends it in `hello`.
A peer that receives its own nonce refuses the connection, and a node that gets its
own nonce back drops the address: under some other address, it reached itself. It
does not connect to that address again until it restarts, even a configured one.

**The book.** It holds at most 1,024 addresses and lives in `peers.json` in the data
directory, saved every two seconds when it has changed. When it is full, the address
least worth keeping makes room: never a configured one, an untried one before one
with a completed round, the most failed first. The file holds only hints; if it is
missing or unreadable, the node starts with an empty book. A peer removed from the
configuration is forgotten, and connected again only if a peer shares its address
later.

**Turning it off.** With `"discovery": false` in `config.json`, read at start, the
node announces no listening port, asks for and shares no addresses, and keeps
connections to its configured peers only; it does not look up the seed nodes
either. Other nodes can still learn its address: one that has it configured,
completes rounds with it and has discovery on shares it like any other.

## Headers first

A node learns of blocks from their headers, which are small and cheap to check, and
fetches bodies only for the chain they show to be the heaviest, as Bitcoin does. A
peer's claimed height and work are informational only.

Every header is checked as it arrives, against its parent: proof of work, the
difficulty target, the timestamp rules and the clock, the votes and the capacity
limits of its branch. What happens next depends on the work its chain carries:

- **Within a day's work of the tip** (1,440 blocks of the tip's work, or more) the
  header goes into the node's header tree, held in memory until its block settles
  ([the block index](/wiki/node/storage#the-block-index)).
- **Further behind**, its chain is checked by a *presync* for that peer instead. The
  headers keep streaming in and are checked one by one, but none is kept: the node
  holds only the running state a check needs (the last header, the last eleven
  timestamps, the work so far and the vote tallies of the last voting epochs) and,
  every 600 headers, one bit of a salted hash of the header. When the chain reaches
  the threshold, the node asks the peer for the same headers again from where the
  chain forked, checks them again, compares the kept bits, and indexes them, 14,400
  at a time, once enough bits have matched. A peer that sends a different chain the
  second time is caught by a bit that does not match.

A chain from far back, such as one forked near genesis and kept at the easiest
difficulty, can be made cheaply, header by header. Presync lets a node check such a
chain at the cost of the headers' checks alone, without its memory growing with the
chain: what a presync keeps grows with the time since the fork, never with the
number of headers, and the clock bounds the headers it accepts at six a second of
that time. A real competing chain, however long, is indexed once it shows its work,
so there is no depth limit and no checkpoint.

A `headers` answer starts after the highest locator entry on the answering node's
canonical branch, wherever that entry sits in the list, so two nodes on competing
branches keep finding their best common block. Headers in the tree but not yet
stored are not saved: after a restart a node asks for them again.

## Blocks

The **best header** is the indexed header with the most work that is not invalid. A
node downloads the blocks of its branch that it does not store:

- in order of height, from the lowest one missing, at most 1,024 beyond it;
- from several peers at once, each connection asking for its own, up to 16 per peer
  at a time and none of two peers at once; a block asked of a peer and not delivered
  within its transfer time, or one the peer does not have, goes back to the others,
  and is not asked of a peer that let it lapse again for two minutes;
- in slices of at most 1 MiB, so a block of any size crosses the transport, within
  its transfer time for the whole block. Its header is known before it is asked for,
  so a peer that answers with another block, or claims one larger than the data
  capacity at its height, has sent [invalid data](#invalid-data) and is caught at the
  first slice.

An item's **transfer time** is a minute, and ten seconds more for every 1 MiB slice
it may span: for a block, its size once the first slice tells it, and until then the
data capacity at its height. Size alone therefore never makes a download fail; a
peer that delivers less than a slice every ten seconds or so does.

A body is stored once its transaction and receipt roots, its receipt count and its
producer data commitment match its header, and before anything is executed. Blocks
arrive out of order and wait on disk until their parents are there. Executing them
happens only when their branch is connected, as
[storage](/wiki/node/storage#connecting-the-best-branch) describes, so a competing
branch costs nothing to execute until it is the heaviest.

A block pushed by a peer, or offered to the API, is taken on the same terms: its
header must be within a day's work of the tip, or already indexed. A block from
further behind is answered with `branch too far behind the tip`.

A peer pushes a block larger than one slice in slices, one item at a time on a
connection. The slice at offset 0 holds the header, which is checked first: a block
the node already stores is answered as `known` at once, and one whose `total` exceeds
the data capacity at its height is invalid data. The other slices follow in order,
each where the last ended, within the block's transfer time from the first; one that
does not ends that block, not the connection. Each slice but the last is answered
with `received`, and the pusher stops unless that is where the slice ended. The last
slice imports the block.

### Transactions in slices

A transaction's size is bounded only by the data capacity, so one may exceed what a
frame carries. A `pending` or `deferred` answer lists a transaction larger than one
slice, or more than the answer has room for, by its size, `total`, instead of whole; the node fetches it with
`transaction_raw`, slice by slice within its transfer time, if it is no larger than
the next block's data capacity, and submits it locally only if its id is the one
listed. A node pushes such a transaction with `submit` in slices, as a block is
pushed: the slice at offset 0 sets `total`, which must not exceed the next block's
data capacity, and carries `not_before` for a deferred one; the last submits it as
relayed by that peer.

## Framing

The protocol runs over TCP. Each message is a frame: a `u32` little-endian length,
then that many bytes, 1 to 16 MiB. The first byte is the frame's kind:

| Kind | Frame |
|---|---|
| 1 | A request: its tag, then its fields |
| 2 | An answer: its tag, then its fields |
| 3 | An error answer: its text |
| 4 | An announcement |

Fields are written one after another, with nothing between them:

| Field | Encoding |
|---|---|
| `u8`, `u16`, `u32`, `u64` | Little-endian, of that width |
| flag | One byte, 0 or 1 |
| id | A block or transaction id, or a ChainId: its 32 bytes |
| bytes | A `u32` length, then that many bytes |
| list | A `u16` count, then that many items |
| name | A `u16` length, then that many bytes of printable ASCII |
| text | A `u16` length, then that many bytes of UTF-8 |
| option | A flag, then the value if it is 1 |
| work | A `u8` length of at most 64, then the number's bytes, most significant first, with no leading zero byte; zero has none |
| address | `4` and an IPv4 address's 4 bytes, or `6` and an IPv6 address's 16, then the port as a `u16`. An IPv4 address is never written mapped into IPv6 |
| header | A block header in its consensus encoding, 294 bytes |
| block, transaction | In its consensus encoding, as bytes |

A frame is read strictly: every count and length within its bound, every flag 0 or
1, work in its shortest form, and no byte left over. A message therefore has exactly
one encoding, and a frame that breaks any of this, or names a kind or tag this node
does not know, is no message: the connection ends without an answer. Blocks, headers
and transactions travel in their consensus encodings, so a node never converts them
to cross the wire.

A connection attempt has five seconds, and so has every frame once its length has
arrived; an answer has 30 seconds.

## Messages

The first request on a connection must be `hello` with a version of at least 1 and
the receiving node's ChainId, in a frame of at most 4 KiB; anything else is
answered with the error `peer version or genesis mismatch` and the connection
closes. After it, the connection stays open as long as requests keep coming: one
idle for 120 seconds is closed. Requests are counted by cost, a block slice or a
block 16, a page of headers 8, `state`, `inventory`, `pending` and `deferred` 2, any
other 1, and beyond 200 a second, after a burst of 2,000, they wait their turn. An
error answer is an answer like any other, such as a block the node does not have,
and the connection stays open; it ends for a second `hello` and for
[invalid data](#invalid-data). Requests are answered in the order they were sent.

| Tag | Request | Fields | Answer |
|---|---|---|---|
| 0 | `hello` | `version` `u32`, `chain_id` id, `agent` name of at most 64 bytes, `features` list of at most 16 names of at most 32 bytes, `nonce` `u64`, `announce` flag (to be told of changes), `listen` option of a `u16` (the sender's listening port) | The node's state |
| 1 | `state` | none | The node's state, as it is now |
| 2 | `headers` | `locator`: a list of 1 to 32 ids | Headers: up to 2,000 canonical headers following the highest locator entry on the canonical chain |
| 3 | `raw` | `id`, `offset` `u64`, `length` `u32` of 1 byte to 1 MiB | A slice of a block the node has executed and found valid, canonical or not, unless the operator marked it or a block below it invalid. Any other block, stored or not, is answered with `block not found` |
| 4 | `import` | `offset` `u64`, `total` `u64`, the block's full length, and bytes of at most 1 MiB: a block whole, or a slice of one | Taken; for a slice before the last, received |
| 5 | `transaction_raw` | `id` of a pooled or deferred transaction, `offset` `u64`, `length` `u32` of 1 byte to 1 MiB | A slice of the transaction; `transaction not found` for one the node does not hold |
| 6 | `submit` | `offset`, `total` and bytes as for `import`, of a transaction; `not_before` option of a `u64`, for a deferred one | Taken; for a slice before the last, received. Refused with `transaction already included` when one of the node's last 1,440 canonical blocks included it and it succeeded, or one of the last 10 included it at all |
| 7 | `inventory` | `deferred` flag; `after` option of an id | Inventory: up to 1,024 ids of pooled transactions, deferred ones left out, or with the flag of deferred ones, in id order after `after` |
| 8 | `pending` | `ids`: a list of 1 to 32 ids | Items: those of the pooled transactions, leaving out deferred ones |
| 9 | `deferred` | `ids`, as for `pending` | Items: those of the deferred transactions, with their times |
| 10 | `addresses` | none | Addresses: up to 32 peer addresses; none when discovery is off |

| Tag | Answer | Fields |
|---|---|---|
| 0 | State | `version` `u32`, `chain_id` id, `agent` name, `features` list of names, `nonce` `u64`, `height` `u64`, `tip` id, `work` work, `locator` list of at most 32 ids, `pending` list of at most 128 ids, `deferred` list of at most 128 ids |
| 1 | Headers | A list of at most 2,000 headers |
| 2 | Slice | `offset` `u64`, `total` `u64`, the item's full length, and bytes of at most 1 MiB |
| 3 | Received | `u64`: the bytes of a push taken so far |
| 4 | Taken | `id` of the block or transaction, and `known` flag: the node held it already |
| 5 | Inventory | A list of at most 1,024 ids, and `next` option of an id: where the next page starts, or none at the end |
| 6 | Items | A list of at most 32 items: `id`, `not_before` option of a `u64` (set for deferred ones), then `0` and the transaction as bytes, or `1` and its size `total` as a `u64` |
| 7 | Addresses | A list of at most 32 addresses |

An error answer's text is at most 1,024 bytes, cut at a character boundary.

A node's state carries:

| Field | Content |
|---|---|
| `version`, `chain_id` | 1, and the node's ChainId |
| `agent` | The node's software and its version, such as `ting-node/0.1.0` |
| `features` | The optional messages it supports beyond its version: none are defined |
| `nonce` | The node's nonce for this run |
| `height`, `tip`, `work` | The canonical height, tip and accumulated work |
| `locator` | At most 32 block ids: the best header's branch back from its tip, the ten most recent and then at gaps that double each time, up to 23 in all; then up to eight stored side branches, most work first; then the genesis block |
| `pending` | Up to 128 pooled transaction ids, leaving out deferred ones |
| `deferred` | Up to 128 deferred transaction ids |

A hello whose `nonce` is the receiving node's own is answered with
`connected to itself`.

A connection whose hello set `announce` also receives announcements between
answers, never in the middle of one, whenever the answering node's tip or pool sizes
change ([connections](#connections)): `tip` id, `height` `u64`, `work` work,
`pending` `u64` and `deferred` `u64`, the sizes of its pool and deferred set.

An `items` answer carries transactions whole while each fits a slice and all of them
together 8 MiB; the others it lists by size alone, and the asking node fetches them
with `transaction_raw` ([transactions in slices](#transactions-in-slices)).

### Versions

The protocol's version is 1. A later version keeps every message of the earlier
ones and only adds, so a node takes a peer of any version from 1 up and speaks the
messages both know; a new optional message is listed in `features` first, and a
node uses it only with a peer that lists it. Unknown features are ignored. `agent`
is shown by `ting peers`; nothing depends on it.

A `headers` request with no locator entry on the canonical chain fails with
`no common ancestor`; the genesis block always matches. Transactions are bounded by
the data capacity active at the tip. Answers beyond the bounds above are no
messages, and the asking side ends the connection.

## Limits

- At most 32 configured peers, and connections to up to `max_peers` peers (1 to 64,
  default 8; one more while the tip is stale)
- 125 inbound connections at once, at most 8 of them from one address range (an IPv4
  address or an IPv6 /64; this machine excepted). With every slot taken, a new
  connection takes the place of the newest one of the range holding the most, if
  that range holds at least two more than the newcomer's; otherwise it is closed. A
  frame's buffer grows as its bytes arrive, so a length alone reserves nothing
- 200 request units a second per inbound connection after a burst of 2,000, and
  120 seconds without a request before it is closed
- A `hello` frame of at most 4 KiB, and every field within its bound
  ([framing](#framing))
- A block download of at most its height's data capacity, and a transaction of at
  most the next block's, within its transfer time
- An address book of at most 1,024 addresses, and at most 32 addresses in or from one
  `addresses` answer
- Five seconds per connection attempt and per frame, 30 seconds per reply
- Frames of at most 16 MiB; blocks and transactions larger than 1 MiB travel in
  slices of at most 1 MiB

No message reaches administration: peer configuration, mining and shutdown exist only
on the [control channel](/wiki/node/security#the-control-channel). Shared addresses
are only hints; whom to connect to stays each node's own choice. There is no NAT
traversal or transport encryption: a node that cannot be reached connects out.
