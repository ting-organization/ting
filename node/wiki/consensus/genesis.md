# Genesis and network identity

Every Ting network starts from a **genesis configuration**: a short, fixed list
of parameters that each node pins before it joins. Two nodes with different
configurations are on different networks, even when they run the same software,
because the configuration's hash, the ChainId, is part of every block and every
transaction. The exact rules are in
[section 3 of the protocol rules](../../../ting-protocol-rules.md#3-genesis-and-network-identity).

## What genesis fixes

| Parameter | Meaning |
|---|---|
| Protocol version | 1 for the current rules |
| Genesis timestamp | The anchor's time; the difficulty schedule starts here |
| Initial target, PoW limit | The first mining target, and the easiest target ever allowed |
| Target interval | Intended seconds between blocks |
| Half-life | How quickly difficulty reacts to the chain running ahead of or behind schedule |
| Voting epoch | Blocks over which capacity votes are counted |
| Initial and minimum capacities | One of each for C, S, D and G; there is no maximum |

Every other rule, from instructions and charges (the price list included) to
module-shape bounds and voting thresholds, belongs to the protocol version rather
than to genesis. Genesis chooses numbers; it cannot change behavior.

The initial and minimum capacities are part of the configuration, and so of the
ChainId. Benchmarking chooses capacity numbers; it never redefines what a unit of
C, S, D or G means. There is no maximum: [capacity voting](capacity-voting.md) can
raise capacities without limit. The voting epoch is counted in blocks, so its real
duration varies with block arrival and with changes in hashpower.

The configuration always encodes to the same 150 bytes, and it is valid only when
its numbers fit together: for example, every floor is positive and at most its
initial capacity, the initial target is no easier than the PoW limit, and the data
floor of at least 302 leaves room for an empty block. The byte layout and every
condition are in
[section 3.2](../../../ting-protocol-rules.md#32-the-configuration-bytes).

## ChainId

The **ChainId** is the [domain-separated hash](../data/encoding.md) of the
configuration's 150 bytes, and the genesis block's identifier, the GenesisBlockId,
is in turn a hash of the ChainId
([section 3.3](../../../ting-protocol-rules.md#33-chainid)). The ChainId identifies
the network everywhere: in every block header, inside every TransactionId and
ContractId, and in the first message between two peers. A transaction prepared for
one network has a different ID, and different effects, on another.

A node pins the configuration, or its digest, before it joins a network. A peer can
never choose a node's genesis for it.

## The genesis anchor

Genesis is not a mined block. It is a fixed anchor at height 0 whose identifier is
the GenesisBlockId. Its timestamp is the genesis timestamp, its target is the
initial target, its state is empty and its accumulated work is zero. It has no
transactions, no receipts, no capacity votes and no reward. The first mined block
has height 1 and names the GenesisBlockId as its parent.

## Production and development

`ting init` writes the production network's genesis: the production profile below
at the network's launch time, 21 October 2026, 00:00 UTC, built into the node.
`ting init --dev` writes the development genesis, for a chain to experiment on: the
experimental profile below with a voting epoch of 20,160 blocks.
`ting init --genesis FILE` imports any other configuration, which is how several
nodes join the same network.

The exact bytes of each configuration built into the node, with its ChainId where
it is fixed, are in
[section 28 of the protocol rules](../../../ting-protocol-rules.md#28-configurations-and-check-values).

## Production profile

| Item | Value |
|---|---:|
| Genesis timestamp | 1,792,540,800 (21 October 2026, 00:00:00 UTC) |
| Initial target | 2^224 − 1, about 4.3 billion hashes per block |
| PoW limit | 2^228 − 1, at least about 268 million hashes per block |
| Target interval | 60 seconds |
| Difficulty half-life | 43,200 seconds (12 hours) |
| Voting epoch | 20,160 blocks, about 14 days |
| Initial capacities | 450,000,000 C, 2,048 S, 262,144 D, 131,072 G per block |
| Capacity floors | 40,000,000 C, 64 S, 98,304 D, 98,304 G |
| ChainId | `15d6549cfacd816f835c632b4c67308d1c63e642f4cbf4049368430670705107` |
| Genesis block | `24b76a52f3825b921373b6c6a41ced2a74de7dbdb34015669c10594996887459` |

- **Capacities** were measured on the reference node: the worst possible block, full
  of the slowest compute per C or of state writes, takes about three seconds on a
  slow processor core, a twentieth of the block interval.
- **Floors** keep every [reference program](../programs.md) usable and deployable
  even when a majority votes capacity down: the largest reference transactions, such
  as a recovery and a savings cancel in one batch, or a sponsored cancel with P-256
  keys, need up to about 36 million C and 44 S.
- **Data and growth** bound the chain to at most about 138 GB of blocks and 69 GB of
  new state a year at the initial capacities; votes move both.
- **The half-life** was chosen by simulating the loss and the arrival of hashpower:
  12 hours keeps target noise near 3.5%, and after 90% of the hashpower leaves,
  block times are back under 90 seconds within about two days.
- **The PoW limit** lets one ordinary machine keep a nearly abandoned chain moving at
  about a block a minute.

## Experimental profile

The experimental profile is one reproducible configuration for prototypes and
tests. Every field is fixed, the timestamp and the initial target included, so it
always has the same ChainId, which several
[consensus vectors](../reference/conformance.md#consensus-vectors) use. Its values are not
production capacities, and its targets are easy, so that any computer mines a block
almost at once. The development genesis is this profile with a voting epoch of
20,160 blocks, about 14 days at the target rate, in place of 4.

Values marked *genesis* are configuration fields. Values marked *protocol* are rules
of the protocol version, listed here so that the whole profile is in one place.

| Item | Value | Set by |
|---|---:|---|
| Genesis timestamp | 1,700,000,000 (14 November 2023, 22:13:20 UTC) | genesis |
| Initial target | 2^254 − 1, about 4 hashes per block | genesis |
| PoW limit | 2^256 − 1, which every hash meets | genesis |
| Target interval | 60 seconds | genesis |
| Difficulty half-life | 172,800 seconds (two days) | genesis |
| Voting epoch | 4 blocks | genesis |
| Initial compute capacity | 100,000,000 C per block | genesis |
| Initial state-access capacity | 1,024 S per block | genesis |
| Initial data capacity | 1,048,576 D per block | genesis |
| Initial growth capacity | 524,288 G per block | genesis |
| Capacity floors | 100,000 C, 16 S, 1,024 D, 4,096 G | genesis |
| Increase | Two-thirds of epoch work, 5% step, no ceiling | protocol |
| Decrease | Majority of epoch work, 10% step, down to the genesis floor | protocol |
| Voting activation | Epoch *e* + 3 | protocol |
| Compute prices | [The price list](../resources/prices.md) | protocol |
| Linear memory of one instance | At most 65,536 pages of 64 KiB | protocol |
| Deployed module | At most 2^30 bytes | protocol |
| Function parameters | At most 1,000 | protocol |
| Parameters plus declared locals | At most 16,384 per function | protocol |
| Reserved slots per function | At most 32,768, including the maximum operand height | protocol |
| Functions, types, globals per module | At most 1,000,000 each | protocol |
| Data and element segments | At most 100,000 each | protocol |
| Table entries | At most the module's function count | protocol |

The voting rows are explained in [capacity voting](capacity-voting.md),
the prices on [the price list](../resources/prices.md) and the module bounds in
[what a module may contain](../vm/profile.md).
