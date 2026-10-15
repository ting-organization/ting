# The reference node

`ting` is the reference node: one executable that validates and stores the chain,
talks to peers, mines, and serves the explorer, the API and this wiki.

## Consensus versus node

The node implements the consensus rules, and nothing it does beyond them is part of
those rules. Its database, file layout, peer protocol, mempool, CLI and REST API are
implementation choices. They can change at any time, before or after a freeze,
without changing what a valid block is. Another implementation can make entirely
different choices and still follow the same chain.

The same freedom covers execution. A node may interpret or compile programs, cache,
vectorize, batch state operations, compress its database or speculate on parallel
execution, as long as it produces the specified serial outcome and the reference
accounting. A function it recognizes as pure may be accelerated when its complete
observable behavior is preserved, including exhaustion and traps. Which functions
count as pure is the node's own judgment, not a consensus rule.

## How it is put together

- One dedicated thread, the chain thread, owns the VM and applies every change to
  the chain and its state, one request at a time. The node's own work, its
  connections to peers, its miner and the control channel, reaches it through a
  queue it serves first. Requests from outside come through a bounded queue in which
  peers that connected and REST each have 128 places: a request that finds its
  share full is refused at once, so a flood crowds out neither the node's own work
  nor the other kind of requester. A request not answered within 30 seconds fails.
  Between requests, and at least every 0.2 seconds, the thread ages the pool and
  admits due deferred transactions, and a reorganization goes on in steps of about
  50 milliseconds, so requests are answered while it runs.
- Reads of stored data do not go through the chain thread: REST's lookups of
  blocks, events, transactions, contracts and storage, and peers' header pages and
  block slices. Reader threads answer them from the database, which serves any
  number of readers beside its one writer; each read sees one commit whole. REST and
  peers each have their own, as many running at once as the machine has processor
  cores. A block larger than one slice is read from disk once while it is fetched
  slice by slice: the last such blocks read, up to 64 MiB, stay in memory. Status
  comes from a copy the chain thread publishes after every request it answers and
  at least every 0.2 seconds. Only submissions, the mempool and whether a
  transaction is pending still reach the chain thread from REST, so read traffic
  scales with the machine's cores without slowing the chain.
- State lives in an embedded database, so the whole state does not need to fit in
  memory. See [Storage and recovery](/wiki/node/storage).
- Peers come from the network's seed nodes or the configuration, and more are found
  through them. See [Peers and synchronization](/wiki/node/sync).
- Administration runs over an authenticated local control channel. REST is optional
  and public. See [Access and exposure](/wiki/node/security).
- Block assembly, the mempool and the built-in miner are described in
  [Mining and voting](/wiki/node/mining).

## Accepting a block

A block is accepted only after its header has been validated, its bytes have decoded
canonically, every transaction has executed in full and every commitment matches.
The checks run cheapest first:

1. The fixed 294-byte header is decoded and validated on its own. Its parent must
   already be stored: a block with an unknown parent is rejected, and synchronization
   fetches ancestors first. A valid header more than 7,200 seconds ahead of the
   node's clock waits: it is refused for now and accepted once the clock catches up.
2. The header's ancestry gives the capacities active at its height, and the block's
   size must be within the active data capacity before its body is decoded.
3. The body is decoded and executed against its parent's state, and the receipts,
   roots and resource totals must equal what the header claims.

Active capacities are derived epoch by epoch and memoized per branch, keyed by the
last block of each counted source epoch, so validation never recounts every vote
since genesis. The rules themselves are on [How nodes agree](/wiki/consensus) and
[Capacity voting](/wiki/consensus/capacity-voting).

## Choosing the branch

The canonical branch is the one with the greatest accumulated target-derived work.
A competing branch replaces the tip only when its work exceeds the tip's by more than
half the arriving block's work ([proof of work](/wiki/consensus/proof-of-work)), and
headers alone never select the chain: only fully validated blocks count. Headers come
first and blocks are fetched along the best header's branch
([peers and synchronization](/wiki/node/sync)); a competing branch is stored and
executed only when it becomes the heaviest
([connecting the best branch](/wiki/node/storage#connecting-the-best-branch)).

A block that extends the canonical branch executes incrementally against the
database-backed state. A block on a side branch executes against its parent's state,
which the node rebuilds from the record changes it keeps for every block, checking
each restored state root on the way. Either way the new block is fully validated;
only greater accumulated work changes the canonical state. How the node stores and
publishes the result is on [Storage and recovery](/wiki/node/storage).

## The data directory

Every command works on one data directory. The default is `~/.ting`: `$HOME/.ting`,
or `%USERPROFILE%\.ting` on Windows, whatever the current working directory.

- `--data-dir DIRECTORY` selects another; a relative path is resolved against the
  caller's working directory.
- If the home variable is missing, empty or not an absolute path, the command fails
  with an error that says so, rather than silently selecting a different node.
- `ting init` refuses a directory that already holds `config.json` or `genesis.bin`.
- Every other command requires an initialized directory and, when `config.json` is
  missing, explains how to initialize the one it selected.

`ting init` writes `config.json`, the canonical `genesis.bin` and `control.token`, a
random 256-bit secret. Which genesis it writes:

- by default, the production network's, built into the node
  ([production profile](/wiki/consensus/genesis#production-profile));
- with `--dev`, a development chain for experiments: the
  [experimental profile](/wiki/consensus/genesis#experimental-profile) with its easy
  target and a two-day ASERT half-life, and a voting epoch of E = 20,160 blocks;
- with `--genesis FILE`, an explicit canonical genesis, to join another network.

Every peer must use the same genesis, and so the same ChainId. The files in the directory are listed on
[Start a node](/wiki/start/node#the-data-directory).

## Processes and listeners

`ting run` runs the node in the foreground. `ting start` launches it in the
background and waits for it to answer an authenticated status request. `ting stop`
requests a graceful shutdown. For its whole lifetime the running node holds an
operating-system lock on `node.lock`, so two processes never share a directory.

A node opens up to three TCP listeners:

| Listener | Default | Purpose |
|---|---|---|
| Control | `127.0.0.1:18440` | Authenticated administration by the CLI; always loopback |
| Peer | `0.0.0.0:18442` on the production network, `127.0.0.1:18442` otherwise | The [peer protocol](/wiki/node/sync) |
| REST | `127.0.0.1:18441` | The explorer, the API and this wiki; off unless enabled |

The CLI administers the node over the control listener, independent of REST, which
stays disabled unless requested with `ting init --api` or `ting api enable`.
Addresses are chosen at init with `--control`, `--p2p` and `--api-bind`; port zero
selects any available port. `runtime.json` records the addresses actually bound, the
process ID and the start time while the node runs, and is removed when it stops
cleanly. It is never treated as authority to kill a process. The control token is
excluded from status output, logs and peer messages.

## Configuration

`config.json` holds the node's settings:

| Field | Default | Meaning and bounds |
|---|---|---|
| `version` | `1` | Format of the file; must be 1 |
| `name` | `"Ting node"` | The node's name, at most 128 bytes; shown in status, never published in blocks |
| `control` | `127.0.0.1:18440` | Control listener; must be a loopback address |
| `p2p` | `0.0.0.0:18442` on the production network, `127.0.0.1:18442` otherwise | Peer listener |
| `api` | `127.0.0.1:18441` | REST listener |
| `api_enabled` | `false` | Whether REST is served |
| `api_cors` | `false` | Whether REST answers browser applications from any origin |
| `peers` | `[]` | Peers to synchronize with, as `IP:port`; at most 32 |
| `discovery` | `true` | Whether the node [finds more peers](/wiki/node/sync#finding-more-peers) through its peers |
| `max_peers` | `8` | Peers it keeps connections to, configured ones included; 1 to 64 |
| `undo_window` | `43200` | Canonical blocks whose undo data the node keeps, 30 days; `0` keeps everything ([per-block changes](/wiki/node/storage#per-block-changes)) |
| `db_cache_mib` | `256` | The database's page cache in MiB, at least 1 ([the database](/wiki/node/storage#the-database)) |
| `votes` | `[0, 0, 0, 0]` | Capacity votes in C, S, D, G order: 0 hold, 1 increase, 2 decrease |
| `miner_enabled` | `false` | Whether the built-in miner runs |
| `miner_threads` | `1` | Its search threads, 1 to 64 |
| `miner_spacing` | `0` | The least number of seconds between its own blocks |
| `producer_hex` | unset | Producer data for the blocks this node builds: lower-case hex, at most 4,096 bytes; none when unset |

A file with an unknown field or a value out of bounds is refused. `api_cors`, `discovery`,
`max_peers`, `undo_window`, `db_cache_mib`, the miner fields and `producer_hex` may be
left out and take their defaults. The node
writes the file atomically, through a synced temporary file that is then renamed.

The CLI changes peers, REST, the miner and producer data whether or not the node is
running: it asks a running node, and otherwise edits the file while holding
`node.lock` itself. Votes have no command; edit them while the node is stopped, as
described in [Mining and voting](/wiki/node/mining#capacity-votes).

## Limits of this implementation

Some of the reference node's choices favor a small, inspectable foundation over
scale:

- There are no snapshots: a new node validates every block from genesis, and every
  node keeps every canonical block and header. Only undo data older than the
  [undo window](/wiki/node/storage#per-block-changes) is pruned.
- Peers are `IP:port` addresses, from the network's seed nodes, the configuration
  or [discovery](/wiki/node/sync#finding-more-peers), and the transport, binary,
  has no encryption.

Snapshots and pruning of old blocks can each make a node more scalable without
touching consensus.
