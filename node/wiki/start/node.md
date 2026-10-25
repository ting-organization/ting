# Start a node

`ting` is both the node and the command-line tool that controls it. This page
installs it and runs a node on the production network. A
[development chain](#a-development-chain), for experiments and program development,
is described at the end.

## Install

From a source checkout, build and install the CLI:

```sh
cargo install --path node --locked
```

The Rust toolchain is pinned in `rust-toolchain.toml`; rustup installs it on the
first build.

## Initialize and start

```sh
ting init
ting start
ting status
```

`ting init` creates a data directory holding a configuration file, the production
network's genesis (which is built into the node) and a private control token, and
imports the network's [seed](../reference/conformance.md#the-seed): its first
blocks, the conformance suite, each checked and executed. The
default directory is `~/.ting` (`%USERPROFILE%\.ting` on Windows), so every command
finds the same node from any working directory. Use `--data-dir DIRECTORY` to work
with another node.

`ting start` launches the node in the background and waits until it answers;
`ting stop` shuts it down. To keep the node in the foreground instead, use
`ting run`. Ctrl+C stops it, and so does SIGTERM on Linux and macOS, which service
managers such as systemd and container runtimes send.

The node needs no peer address. It looks up the network's seed nodes at every start
and finds further peers through them
([finding more peers](../node/sync.md#finding-more-peers)), then downloads and
validates every block from genesis. `ting status` shows the height it has reached,
and `ting peers` the peers it is connected to. Its log, `node.log` in the data
directory, follows the download as it goes: the peers it connects to, its progress
every ten seconds and the moment it catches up
([what the node writes](#what-the-node-writes)).

## Reachability

By default the peer listener binds `0.0.0.0:18442`, which is every IPv4 address of
the machine. Your node does not have to be reachable from outside, though. A node
behind a firewall, or behind a NAT without a forwarded port, still takes part fully:
it connects out to its peers and exchanges blocks and transactions over those
connections. Opening TCP port 18442 to the internet lets other nodes connect in as
well, which helps the network.

The control channel always stays on the loopback interface, and REST stays off until
you turn it on.

## REST, the explorer and this wiki

```sh
ting api enable
ting docs
```

`ting api enable`, or `--api` at init, turns on the REST interface. It serves the
[explorer and the API](explore.md) and this wiki, by default at
`127.0.0.1:18441`. `ting docs` prints the current wiki and API addresses to open in a
browser, and `ting status` shows the ports actually bound.

`ting api disable` turns REST off again. The CLI talks to the node over a separate,
authenticated loopback connection, so it keeps working while REST is disabled.

`ting api enable --cors` lets browser applications served from other origins, such
as a wallet running in a web page, call the API. Leave it off unless you need that.

## Mining

A node validates and relays blocks without mining. To mine, run `ting miner start`,
which works on any network. The built-in CPU miner builds a block template on the
node's tip and searches nonces for it. When it finds a valid nonce, it submits the
block and starts over with a fresh template. It keeps going until `ting miner stop`.

`--threads N` sets the number of search threads. The miner's state and settings are
kept in `config.json`, so a restarted node resumes mining. A template tries the
pending transactions in turn, the earliest first, and includes each one the block
still has room for.

The protocol pays no reward. Programs that pay block producers read the block's
producer data, which `ting miner producer HEX` sets. Each block also carries its
miner's capacity votes, which `ting miner votes` sets and `ting votes` reports on.
Templates for external mining software, producer data and capacity votes are covered
in [Mining and voting](../node/mining.md).

## Command reference

Every command accepts `--data-dir DIRECTORY` and `--json`. Every command except
`ting run` prints its result as text for a person to read; with `--json` it prints
the result itself as JSON instead, the form scripts and mining software read, with
every field. `ting run` writes the node's log ([what the node writes](#what-the-node-writes)).
A command that fails prints `ting: ` and the error to standard error.

| Command | What it does |
|---|---|
| `ting init [--dev \| --genesis FILE] [--name N] [--control ADDR] [--p2p ADDR] [--api-bind ADDR] [--api]` | Creates the data directory for the production network, a development chain (`--dev`) or another network (`--genesis`); `--control` defaults to `127.0.0.1:18440`, `--p2p` to `0.0.0.0:18442` on the production network or `127.0.0.1:18442` otherwise, and `--api-bind` to `127.0.0.1:18441` |
| `ting run` | Runs the node in the foreground |
| `ting start` | Runs it in the background, waiting up to 15 seconds for it to answer |
| `ting stop` | Shuts it down gracefully, waiting up to 15 seconds for it to finish |
| `ting status` | Chain status and the listener addresses actually bound |
| `ting peers` | Configured and discovered peers with their last success or error |
| `ting probe IP:PORT` | Says hello to the node at that address over the [peer protocol](../reference/peer-protocol.md), as a peer on this network, and shows its state: whether it answers, and its height, tip and software |
| `ting add node IP:PORT`, `ting remove node IP:PORT` | Adds or removes a configured peer |
| `ting api enable [--cors]`, `ting api disable` | Turns REST on or off |
| `ting docs` | The wiki, API page and OpenAPI addresses |
| `ting submit FILE [--not-before TIME]` | Submits a binary canonical transaction; with `--not-before`, deferred until that Unix time |
| `ting miner start [--threads N] [--spacing S]`, `ting miner stop`, `ting miner status` | Controls the built-in miner; `--spacing S` leaves at least S seconds between its own blocks |
| `ting miner producer [HEX]` | Sets the producer data of the blocks this node builds, or clears it |
| `ting miner votes C S D G` | Sets the capacity votes of the blocks this node builds, each `hold`, `increase` or `decrease` ([capacity votes](../node/mining.md#capacity-votes)) |
| `ting votes` | Where capacity voting stands: this epoch's votes so far, the capacities already decided and this node's votes |
| `ting mine [--count N] [--attempts A]` | On a [development chain](#a-development-chain): mines up to N blocks (1 to 100, default 1), trying at most A nonces each (default 1,000,000) |
| `ting invalidate BLOCK`, `ting reconsider BLOCK` | Marks a block invalid, by id or height, or clears invalid marks ([overriding validity](../node/storage.md#overriding-validity)) |
| `ting template [--tx ID]…` | Prints a block template; each `--tx` names a pooled transaction, and the template then holds exactly those, in order |
| `ting solution ID NONCE` | Submits a nonce for a template |

Peer, REST, miner and producer settings can also be changed while the node is
stopped; the CLI then edits `config.json` itself. Details are on
[The reference node](../node.md), [Mining and voting](../node/mining.md) and
[Access and exposure](../node/security.md).

## What the node writes

`ting run` writes the node's log to the terminal, and a node started with
`ting start` appends it to `node.log` in the data directory, where
`tail -f ~/.ting/node.log` follows it. Each line begins with the time in UTC. A
new node on the production network writes, for example:

```text
2026-10-25T13:10:12Z Ting ready: control=127.0.0.1:18440 peers=0.0.0.0:18442 REST=disabled
2026-10-25T13:10:12Z production chain at height 325, tip 00000000e04237d2…
2026-10-25T13:10:12Z seed node node1.ting-node.com:18442: 1 address
2026-10-25T13:10:14Z connected to 203.0.113.4:18442: ting-node/0.1.0, at height 5427
2026-10-25T13:10:15Z downloading blocks from height 326, best header so far 2325
2026-10-25T13:10:25Z height 1687 of 5427 (31%), 136 blocks/s, about 27s left
2026-10-25T13:10:35Z height 4252 of 5427 (78%), 196 blocks/s, about 5s left
2026-10-25T13:10:40Z caught up at height 5427, tip 00000000c64a9a39…: 5102 blocks in 24s
2026-10-25T13:10:40Z block 5428 00000002251ebfe8…
```

The log shows the full block ids. It holds:

- `Ting ready:` and the listener addresses once the node listens, then where the
  chain stands: its network, height and tip. On the production network, each seed
  node and the addresses it resolved to.
- Each connection to a peer as it opens, with the peer's software and height, and
  as it ends, with the reason. A configured peer that cannot be reached is written
  once for each new reason; discovered addresses are tried quietly, as many never
  answer.
- While the node is more than two blocks below its best header, it is downloading:
  a line every ten seconds with the height, the share done, the speed and the time
  left, and a line when it catches up. Its best header can rise during the download
  as further headers arrive.
- Once caught up, each new tip.
- With no peer connected for 30 seconds, while the node has addresses to try or is
  on the production network, a line saying so, and another every ten minutes for as
  long as none is.
- Invalid blocks and the peers banned for them, reorganizations, and anything that
  stops the chain.

## The data directory

| File | Purpose |
|---|---|
| `config.json` | Name, listener addresses, peers, REST and miner settings, producer data and capacity votes, the database cache and the mempool's bounds |
| `genesis.bin` | The canonical genesis configuration, which defines the network |
| `control.token` | Secret for the CLI's control channel; keep it private |
| `blocks/` | Append-only block files |
| `chain.redb` | State, indexes and per-block changes |
| `deferred.json` | Transactions held until their time |
| `peers.json` | Addresses of other nodes, learned from seed nodes and peers |
| `runtime.json` | Actual listener addresses and process ID while the node runs |
| `node.log` | The log of a background node ([what the node writes](#what-the-node-writes)) |
| `node.lock` | Locked by the running node |

The operating-system lock on `node.lock` stops two nodes from using the same
directory.

## A development chain

For experiments and program development, `ting init --dev` creates a chain of your
own:

- Its genesis is the development genesis: the
  [experimental profile](../consensus/genesis.md#experimental-profile) with a
  two-week voting epoch. Its target is easy enough that blocks appear almost at once.
- Its peer listener stays on the loopback interface.
- It starts with its [seed](../reference/conformance.md#the-seed). `ting init --dev`
  imports the chain's first blocks, the conformance suite, and checks and executes
  every one, so the chain begins at the seed's last block.

> [!NOTE]
> **Not a secured network.** A development chain has an easy mining target and is
> meant for experiments. It is not an economically secured network.

Give the chain a data directory of its own. A relative directory such as `dev` is
resolved against your current directory. The development node uses the same ports as
a production node, so stop that one first, or give this one others at init with
`--control`, `--p2p` and `--api-bind`, as [Connect two nodes](network.md)
does.

```sh
ting --data-dir dev init --dev --api
ting --data-dir dev start
ting --data-dir dev mine --count 3
ting --data-dir dev status
```

`ting mine` runs a bounded nonce search in one request: a template, the search and
the solution, for each of up to N blocks. With no transactions pending it mines
empty blocks, which are still valid and still fully checked. `ting status` then
shows the height the chain has reached.

To keep mining in the background instead, run
`ting --data-dir dev miner start --spacing 10`, which mines at most one block every
ten seconds, and `ting --data-dir dev miner stop` to end it.

Next, [Deploy a first program](first-program.md) continues on this chain.
