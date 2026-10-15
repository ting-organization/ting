# Mining and voting

## Templates and solutions

Mining is two jobs, and the node does one of them. It assembles a *template*: a
complete candidate block on its tip whose header lacks only the nonce. Whoever mines
searches for a nonce that makes the header's id meet the target and hands the
*solution* back; the node then validates the finished block like one from a peer,
commits it and relays it.

The proof of work is the block id, `H("block", header bytes)` read as a big-endian
integer, at or below the header's target; see
[Proof of work](/wiki/consensus/proof-of-work). The nonce is the header's last field:
16 bytes, little-endian, at offset 278 of the 294-byte header.

`ting template` asks for a template. The answer holds:

| Field | Content |
|---|---|
| `id` | The template's id, the id of its header with the nonce still zero |
| `height`, `parent`, `timestamp` | Where the block goes and the time it carries |
| `header_hex` | The complete header bytes |
| `bytes`, `used` | The block's encoded size and its C, S, D, G totals |
| `transactions` | The ids of the transactions it holds, in order |
| `mining` | `nonce_offset` (278), `nonce_bytes` (16), `nonce_endian` (`little`) and `target_hex` |

`ting solution ID NONCE` submits a nonce, as a decimal number, for a template. The
node checks the proof of work and then imports the block exactly as if a peer had
sent it, which validates and commits it and lets the peer loop relay it. A nonce that
misses the target is refused with `solution does not meet the target`.

Templates are kept per tip, the eight most recent at a time. All of them are
discarded when the tip moves or the miner settings change, so a solution for an old
template is refused with `unknown or expired template`.

## Assembling a candidate

To build a template the node:

1. opens a candidate on its tip: the block context set, the active capacities in
   force, and nothing executed yet;
2. adds the transactions one at a time, each executed into the candidate and kept
   only when the block still fits it: its selector picks from the pool (the
   reference selector tries every pooled transaction in arrival order), or
   `ting template --tx ID`, repeated, names exactly the pooled transactions to
   include, in order, bypassing the selector, and fails if one does not fit;
3. completes the receipts and the three roots, and restores its state and context
   from the journal. A candidate is never committed.

Each transaction tried is executed once, so a template costs at most the pool's
declared compute, four blocks' worth.

The timestamp is the current time, or one second past the median of the last eleven
timestamps when the clock is behind it. The producer data and the capacity votes come
from the configuration.

Assembly uses the same execution and the same C, S, D, G checks as validation, takes
no shortcut that validation would not, and changes no VM semantics. Once a nonce is
found, the normal validator executes the block again and commits it.

## What a mining fork replaces

A miner runs a fork of this node and replaces two pieces. Each is a trait in the
`mining` module whose reference implementation is the default; the fork passes its
own to `daemon::run_with` and keeps everything else — assembly, validation, storage,
synchronization and relay.

- **Selection** (`Selector`): which pending transactions a template includes. The
  selector is handed the candidate block: it adds a transaction and gets back its
  receipt, with its events, or nothing when the block would not fit it, and it can
  take the last one back out. So it can judge transactions by anything a receipt
  shows, such as what the programs it knows pay the producer. The reference
  `FirstIn` tries every pooled transaction first in, first served, and keeps each
  one the block fits. The node itself knows nothing about fees or rewards; those
  belong to programs.
- **Search** (`Search`): finding the nonce for a header within a bounded number of
  attempts. The reference `Cpu` hashes the header up to its nonce once, then each
  nonce from that SHA-256 midstate, on the configured threads over disjoint nonce
  ranges; a fork drives a GPU or an ASIC. A search is always bounded,
  so it never works long on a stale template.

## The built-in miner

`ting miner start [--threads N] [--spacing S]` runs the miner inside the node. It is
the loop between the two pieces: it fetches a template, lets the search try a slice
of 16,777,216 nonces per thread, about a second on a current processor core, submits
a solution when it finds one and fetches a
fresh template after every slice, so new transactions, a new tip and the clock reach
the header it works on. After an error, such as a template that expired, it waits
half a second and starts over. While the node is more than one block behind its best
header, as while it synchronizes, the miner waits instead, since a block on a tip
about to be replaced would be lost; its status shows how far behind.

- `--threads` sets the search threads, 1 to 64; `--spacing` the least number of
  seconds between its own blocks, useful on an easy development chain. Both default
  to the configured values.
- `ting miner stop` stops it.
- `ting miner status` reports whether it runs, the search in use, the spacing, when
  it started, templates fetched, nonces tried, blocks found, the height it works on
  and its last error.

Its state is kept in the configuration (`miner_enabled`, `miner_threads`,
`miner_spacing`), so a restarted node resumes mining if it was.

The miner is only a client of the template and solution requests, which the
[control channel](/wiki/node/security#the-control-channel) offers as well, so the
chain thread stays the only owner of state and a program outside the node can do by
hand what the miner does in a loop. Mining has no REST surface.

`ting mine --count N --attempts A` is the shortcut for a
[development chain](/wiki/start/node#a-development-chain): template, search and
solution in one request, for up to N blocks (1 to 100, default 1) with at most A
nonces tried for each (1 to 10,000,000, default 1,000,000). The search runs beside the
chain thread, as the miner's does, so the node answers meanwhile; a template that the
tip moves past is replaced by a new one. The answer lists the blocks imported and
says whether the attempts ran out first.

## Producer data

Each block carries producer data in its body, under the header's commitment. The
node writes the bytes set with `ting miner producer HEX` (lower-case hex, at most
4,096 bytes) and kept as `producer_hex`, and none when it is unset, so nothing about
the node is published unless its operator chooses; `ting miner producer` with no
argument clears it. The node attaches no meaning to the
data. Programs read it, and a program that compensates block producers can take the
destination of its payments from there. It proves nothing about who mined.

## Capacity votes

The node signals the votes in the `votes` field of `config.json`: four numbers in
C, S, D, G order, each 0 for hold, 1 for increase or 2 for decrease.

```json
"votes": [1, 0, 0, 0]
```

Edit the file while the node is stopped; the votes apply to every block it builds
after the next start. How votes are counted is covered in
[Capacity voting](/wiki/consensus/capacity-voting).

## The mempool

Mempool rules are local policy, not consensus:

- submission checks canonical encoding, that the data budget covers the envelope's
  own base data charge, and a size within the data capacity active at the tip; a
  larger transaction is refused with `transaction larger than the data capacity`. Any
  declared budget is taken, however large, since only what a transaction uses has to
  fit a block ([declared versus actual](/wiki/resources/capacity#declared-versus-actual)):
  a template keeps a transaction only when its use fits. Nothing executes until a
  block is assembled or validated;
- at most 1,024 transactions, and 16 MiB of encoded transactions or the data of
  four blocks at the next block's capacity, whichever is more, so that a
  transaction of any size a block can hold fits. A template runs
  each pooled transaction against what is left of its block, so the compute pooled
  transactions declare does not bound the pool: one that cannot fit is refused at
  its first charge past the block's room;
- a transaction a peer relays counts against the peer's IP address, and at most 64
  of one address's are pooled at once. When the pool is full, a submission takes the
  place of the most recent transaction of the address holding the most, if that
  address holds more than the submitter's; transactions submitted through the API
  or the CLI count against no address, so they can take such a place and never give
  up theirs. Otherwise the submission is refused with `mempool is full`;
- first in, first out, and a transaction whose id is already pooled is not added
  again;
- a transaction a peer relays is refused with `transaction already included` when
  one of the last 1,440 canonical blocks included it and it succeeded there: a pool
  keeps a transaction at most a day, so a relayed one this recent is one a block just
  took. One that failed is refused only while one of the last 10 blocks includes it,
  as the block's echo lasts: pools drop what a block takes, so it does not travel
  back, and a miner who made it fail on purpose cannot keep it from the network for
  a day; sent again, it spreads as before. Submitted through the API or the CLI, it
  is taken;
- transactions included in a new canonical block leave the pool, and so do those
  that have waited a day. A live reorganization returns displaced transactions that
  the new branch does not include, within the same limits;
- the pool is not saved: a node starts, and recovers, with an empty pool.

Acceptance into the pool means a transaction is well formed, not that it will
succeed. Authentication, assets and replay protection are decided by programs when
the transaction runs. The same transaction id may also appear in more than one
block; every such occurrence is consensus-valid, and lookups page through them.

## Local policy

Mempool acceptance, fee selection, transaction simulation, relay queues, peer
selection, proof-of-submission work and willingness to accept particular assets are
all local policies. Consensus never requires any node to execute every request that
has not been mined, and a transaction that some node's policy rejects can still
appear in a valid block.

## Deferred transactions

A submission may carry `not_before`, a Unix time. Such a transaction is deferred: the
node keeps it in a separate deferred set instead of the pool.

- Once `not_before` has passed, the node executes it on its tip, at least every 0.2
  seconds and before assembling each block, about one block's worth of declared
  compute at a time, each entry counting its declared compute but at most a block's,
  the entries tried least recently first, passing over one that does not fit what is
  left; and moves it into the pool only when that execution fits a block and
  succeeds.
- A failing one is tried again when the tip changes, or after thirty seconds.
- An entry stays in the set, through restarts (it is saved in `deferred.json`
  within five seconds of a change, and when the node stops) and reorganizations,
  until a canonical block above the height it was submitted at includes the
  transaction **with a successful receipt**, or until it expires thirty days after
  `not_before` (or after its submission, if that came later). An inclusion that
  failed, such as one before the transaction's time, leaves the entry in place, and
  if the block that removed an entry is disconnected, the entry comes back instead
  of entering the pool. An envelope mined once can therefore be deferred again. An
  entry the node can no longer decode when it starts, such as a transaction above a
  data capacity voted down since, is dropped with a note on standard error.
- The set holds at most 4,096 transactions, and as many bytes as the pool, at most
  256 of them
  relayed from one IP address, and `not_before` may be at most a year (366 days)
  ahead. When the set is full, a submission due earlier takes the place of the
  relayed entry due latest; otherwise it is refused with `deferred set is full`.
- Submitting a transaction that is already deferred or already pooled changes
  nothing; the answer says it was known.

This is how a program's time-dependent call, such as a scheduled action, is handed
to the node when it is created: the node, not the submitter, has to be there when it
falls due. Peers exchange deferred transactions with their times, so any of them may
be the one mining when it does. The deferral is policy; the program's own checks
still decide whether the transaction succeeds when a block includes it.
