# Connect two nodes

A production node finds its peers by itself, through the network's seed nodes
([finding more peers](../node/sync.md#finding-more-peers)). On any other network you
name one or more peers, and a node finds further peers through them. This page runs
two nodes of a development chain on one machine, then explains discovery and what
changes across machines.

## Two nodes on one machine

Each node needs its own data directory and its own ports. Node A keeps the default
ports, the same as a production node, so stop any production node running on this
machine first. Relative directories such as `node-a` are resolved against your
current directory.

```sh
ting --data-dir node-a init --dev --api
ting --data-dir node-b init --dev --control 127.0.0.1:18540 --p2p 127.0.0.1:18542 --api-bind 127.0.0.1:18541 --api
ting --data-dir node-a start
ting --data-dir node-b start
ting --data-dir node-a add node 127.0.0.1:18542
ting --data-dir node-a mine --count 3
ting --data-dir node-b status
```

Node B gets other ports at init. `add node` tells node A where node B listens for
peers, and node A then mines three blocks. Every development chain starts from the
same genesis, so the two nodes are on one network. Within a few seconds node B
reports the same height and tip as node A.

## How synchronization works

Only one side needs to know the other. A node keeps a connection open to each of its
peers, and the connection carries traffic both ways:

- The peer announces each new tip, and the node fetches the headers and blocks it
  is missing.
- The node pushes its own new blocks to the peer.
- Both exchange pending and deferred transactions.

So a single configured connection synchronizes both ways, even when the node that
connects out cannot be reached from outside.

Every header is checked as it arrives, every block's body before it is stored, and
every block is executed before it is connected. A heavier valid branch from a peer
causes a reorganization; a lighter one is kept as a side branch in case it overtakes
later. A peer that sends invalid data is disconnected and, unless it is configured
or on the same machine, banned for a day.

`ting peers` shows each peer's last success or error, and `ting remove node ADDRESS`
removes a configured peer and ends the connection to it. With discovery on, the node
may still connect to that address again as a discovered peer, once it learns it from
the network. A node takes at most 32 configured peers.

## Finding more peers

A node does not need to know every other node. It asks its peers for the addresses
of nodes they have synchronized with, and keeps connections to up to eight peers in
all. Start a third node that knows only node B, and within a few seconds nodes A and
C synchronize with each other too: `ting peers` lists them with source `discovered`.
The addresses a node learns are kept in `peers.json` across restarts.

Set `"max_peers"` in `config.json` to keep more or fewer connections, or
`"discovery": false` to use only the configured peers. The details are in
[Finding more peers](../node/sync.md#finding-more-peers).

## Across machines

- On the production network the peer listener is reachable by default: it binds
  `0.0.0.0:18442`. The node finds its peers through the network's seed nodes. For
  another network, bind it to a reachable address when you initialize, for example
  `--p2p 0.0.0.0:18442`, and configure at least one peer by IP address and port with
  `ting add node IP:PORT`; the node finds more through it.
- A node that cannot be reached from outside still works: it connects out to its
  peers. Other nodes learn this node's address at the IP its connections come from
  and the port it listens on, so they can connect in only if that IP and port are
  open to them. Allowing the port through the firewall, and forwarding it through a
  NAT, makes the node one that others can reach, which helps the network.
- Every node must use an identical genesis. A production node has it built in. For
  another network, copy `genesis.bin` from the first node and initialize the others
  with `ting init --genesis genesis.bin`. Peers with a different ChainId are
  refused.

> [!WARNING]
> **Peer traffic is plain TCP.** It carries only public chain data, but it is neither
> encrypted nor authenticated. The control channel always stays on the loopback interface.

More detail: [Peers and synchronization](../node/sync.md).
