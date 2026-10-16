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

`init` creates a data directory with a configuration file, the production network's
genesis, which is built into the node, and a private control token. `start` launches
the node in the background and waits until it answers. Use `ting run` instead to keep
it in the foreground; Ctrl+C stops it, as does SIGTERM on Linux and macOS, which
service managers such as systemd and container runtimes send.

The node needs no peer address. It looks up the network's seed nodes at every start
and finds further peers through them
([finding more peers](/wiki/node/sync#finding-more-peers)), then downloads and
validates every block from genesis. `ting status` shows the height it has reached,
and `ting peers` the peers it is connected to.

The default directory is `~/.ting` (`%USERPROFILE%\.ting` on Windows), so every
command finds the same node from any working directory. Use
`--data-dir DIRECTORY` to work with another node.

## Reachability

The peer listener binds `0.0.0.0:18442` by default, every IPv4 address of the machine.
A node that cannot be reached from outside, behind a firewall or a NAT without a
forwarded port, still takes part fully: it connects out to its peers and exchanges
blocks and transactions over those connections. Opening TCP port 18442 to the
internet lets other nodes connect in as well, which helps the network. The control
channel always stays on the loopback interface, and REST stays off until you turn
it on.

## REST, the explorer and this wiki

```sh
ting api enable
ting docs
ting api disable
ting api enable --cors
ting stop
```

`ting api enable`, or `--api` at init, turns on the REST interface, which serves the
[explorer and the API](/wiki/start/explore) and this wiki, by default at
`127.0.0.1:18441`. The CLI talks to the node over a separate, authenticated loopback
connection, so it keeps working while REST is disabled. `ting docs` prints the
current wiki and API addresses, and `ting status` shows the ports actually bound.
`--cors` lets browser applications served from other origins, such as a wallet
running in a web page, call the API; leave it off unless you need that.

## Mining

A node validates and relays blocks without mining. `ting miner start` runs the
built-in CPU miner, on any network: it builds a block template on the node's tip,
searches nonces for it, submits a block when it finds one and starts over with a
fresh template, until `ting miner stop`. `--threads N` sets its search threads, and
the setting is kept in `config.json`, so a restarted node resumes mining. A template
tries every pending transaction in arrival order and includes each one the block
still has room for.

The protocol pays no reward. Programs that pay block producers read the block's
producer data, which `ting miner producer HEX` sets. Each block also carries its
miner's capacity votes, which `ting miner votes` sets and `ting votes` follows. Templates for external mining
software, producer data and capacity votes are covered in
[Mining and voting](/wiki/node/mining).

## Command reference

Every command accepts `--data-dir DIRECTORY` and prints its result as JSON, except
`ting run`, which prints `Ting ready:` and its listener addresses once the node
listens, and then any messages the node writes. A command that fails prints
`ting: ` and the error to standard error.

| Command | What it does |
|---|---|
| `ting init [--dev \| --genesis FILE] [--name N] [--control ADDR] [--p2p ADDR] [--api-bind ADDR] [--api]` | Creates the data directory for the production network, a development chain (`--dev`) or another network (`--genesis`); the addresses default to `127.0.0.1:18440`, `0.0.0.0:18442` on the production network or `127.0.0.1:18442` otherwise, and `127.0.0.1:18441` |
| `ting run` | Runs the node in the foreground |
| `ting start` | Runs it in the background, waiting up to 15 seconds for it to answer |
| `ting stop` | Shuts it down gracefully, waiting up to 15 seconds for it to finish |
| `ting status` | Chain status and the listener addresses actually bound |
| `ting peers` | Configured and discovered peers with their last success or error |
| `ting probe IP:PORT` | Says hello to the node at that address over the [peer protocol](/wiki/node/sync#messages), as a peer on this network, and shows its state: whether it answers, and its height, tip and software |
| `ting add node IP:PORT`, `ting remove node IP:PORT` | Adds or removes a configured peer |
| `ting api enable [--cors]`, `ting api disable` | Turns REST on or off |
| `ting docs` | The wiki, API page and OpenAPI addresses |
| `ting submit FILE [--not-before TIME]` | Submits a binary canonical transaction, deferred until a Unix time |
| `ting miner start [--threads N] [--spacing S]`, `ting miner stop`, `ting miner status` | Controls the built-in miner |
| `ting miner producer [HEX]` | Sets the producer data of the blocks this node builds, or clears it |
| `ting miner votes C S D G` | Sets the capacity votes of the blocks this node builds, each `hold`, `increase` or `decrease` ([capacity votes](/wiki/node/mining#capacity-votes)) |
| `ting votes` | Where capacity voting stands: this epoch's votes so far, the capacities already decided and this node's votes |
| `ting mine [--count N] [--attempts A]` | On a [development chain](#a-development-chain): mines up to N blocks (1 to 100, default 1), trying at most A nonces each (default 1,000,000) |
| `ting invalidate BLOCK`, `ting reconsider BLOCK` | Marks a block invalid, by id or height, or clears invalid marks ([overriding validity](/wiki/node/storage#overriding-validity)) |
| `ting template [--tx ID]…` | Prints a block template; each `--tx` names a pooled transaction, and the template then holds exactly those, in order |
| `ting solution ID NONCE` | Submits a nonce for a template |

Peer, REST, miner and producer settings can also be changed while the node is
stopped; the CLI then edits `config.json` itself. Details are on
[The reference node](/wiki/node), [Mining and voting](/wiki/node/mining) and
[Access and exposure](/wiki/node/security).

## The data directory

| File | Purpose |
|---|---|
| `config.json` | Name, listener addresses, peers, REST and miner settings, producer data and capacity votes |
| `genesis.bin` | The canonical genesis configuration, which defines the network |
| `control.token` | Secret for the CLI's control channel; keep it private |
| `blocks/` | Append-only block files |
| `chain.redb` | State, indexes and per-block changes |
| `deferred.json` | Transactions held until their time |
| `peers.json` | Addresses of other nodes, learned from seed nodes and peers |
| `runtime.json` | Actual listener addresses and process ID while the node runs |
| `node.log` | Output of a background node |
| `node.lock` | Locked by the running node |

The operating-system lock on `node.lock` stops two nodes from using the same
directory.

## A development chain

For experiments and program development, `ting init --dev` creates a chain of your
own. Its genesis is the development genesis, the
[experimental profile](/wiki/consensus/genesis#experimental-profile) with a two-week
voting epoch, whose target is easy enough that blocks appear almost at once, and its
peer listener stays on the loopback interface. Give it a data directory of its own:

```sh
ting --data-dir dev init --dev --api
ting --data-dir dev start
ting --data-dir dev mine --count 3
ting --data-dir dev status
```

`ting mine` runs a bounded nonce search in one request: a template, the search and
the solution, for each of up to N blocks. With no transactions pending it mines
empty blocks, which are still valid and still fully checked. To keep mining in the
background instead, run `ting --data-dir dev miner start --spacing 10`, which mines
at most one block every ten seconds, and `ting --data-dir dev miner stop` to end it.

A relative directory such as `dev` is resolved against your current directory. The
development node uses the same ports as a production node, so stop that one first,
or give this one others at init with `--control`, `--p2p` and `--api-bind`, as
[Connect two nodes](/wiki/start/network) does.
[Deploy a first program](/wiki/start/first-program) continues on this chain.

> [!NOTE]
> **Not a secured network.** A development chain has an easy mining target and is
> meant for experiments. It is not an economically secured network.
