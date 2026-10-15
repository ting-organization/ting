# Genesis and network identity

Every Ting network starts from a **genesis configuration**: a short, fixed list
of parameters that each node pins before it joins. Two nodes with different
configurations are on different networks, even when they run the same software.

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

## The configuration bytes

The configuration is [canonically encoded](/wiki/data/encoding) with its fields in
this order, which always takes 150 bytes:

```text
GenesisConfig {
    protocol_version:   u16 = 1
    genesis_timestamp:  u64              // seconds
    initial_target:     u256be
    pow_limit:          u256be
    target_interval:    u32              // seconds
    half_life:          u32              // seconds
    vote_epoch_length:  u32              // blocks
    initial_limits:     ResourceVector
    minimum_limits:     ResourceVector
}
```

A `ResourceVector` is four `u64` values in the order C, S, D, G, one per
[resource counter](/wiki/resources/counters). A configuration is valid only when:

- every capacity, initial and minimum, is positive, and each minimum is at most its
  initial value. There is no maximum: [capacity voting](/wiki/consensus/capacity-voting)
  can raise capacities without limit;
- the initial target lies between 1 and `pow_limit`, inclusive;
- the target interval, the half-life and the voting epoch length are positive;
- the minimum D capacity is at least 302, so that an empty block with empty producer
  data always fits its data allowance.

## ChainId

The canonical bytes of the configuration hash to the **ChainId**. It identifies the
network everywhere: in every block header, inside every TransactionId and ContractId,
and in the first message between two peers. A transaction prepared for one network
has a different ID, and different effects, on another.

```text
ChainId        = H("chain", encode(GenesisConfig))
GenesisBlockId = H("genesis", ChainId)
```

`H` is the [domain-separated hash](/wiki/data/encoding). A node pins the
configuration, or its digest, before it joins a network. A peer can never choose a
node's genesis for it.

## The genesis anchor

Genesis is not a mined block. It is a fixed anchor at height 0 whose identifier is
the GenesisBlockId. Its timestamp is the genesis timestamp, its target is the
initial target, its state is empty and its accumulated work is zero. It has no
transactions, no receipts, no capacity votes and no reward. The first mined block
has height 1 and names the GenesisBlockId as its parent.

## Production and development

`ting init` writes the production network's genesis, which is built into the node
and uses the production profile below. `ting init --dev` writes the development
genesis, for a chain to experiment on: the experimental profile below with a voting
epoch of 20,160 blocks. `ting init --genesis FILE` imports any other configuration,
which is how several nodes join the same network.

## Production profile

| Item | Value |
|---|---:|
| Genesis timestamp | 1,791,908,344 (13 October 2026, 16:19:04 UTC), the launch time |
| ChainId | `26520bbe40340aabafdc0da89c128e4849146d4b3e59edabd7899d6782a6e653` |
| Genesis block | `c1e449324bd2397d4738955265e17612d97ce2c5d35f660adab96318021670f4` |
| Initial target | 2^224 − 1, about 4.3 billion hashes per block |
| PoW limit | 2^228 − 1, at least about 268 million hashes per block |
| Target interval | 60 seconds |
| Difficulty half-life | 43,200 seconds (12 hours) |
| Voting epoch | 20,160 blocks, about 14 days |
| Initial capacities | 150,000,000 C, 1,024 S, 262,144 D, 131,072 G per block |
| Capacity floors | 40,000,000 C, 64 S, 98,304 D, 98,304 G |

- **Capacities** were measured so that the worst possible block, full of the slowest
  compute per C and of state writes, takes about a second on a slow processor core
  with the reference node.
- **Floors** keep every [reference program](/wiki/programs) usable and deployable
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
always has the same ChainId; it is the genesis of the
[protocol vectors](/wiki/reference/conformance#protocol-vectors). Its values are not
production capacities, and its targets are easy, so that any computer mines a block
almost at once. The development genesis is this profile with a voting epoch of
20,160 blocks, about 14 days at the target rate, in place of 4. Values marked
*genesis* are configuration fields. Values marked *protocol* are rules of the
protocol version, listed here so that the whole profile is in one place.

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
| Compute prices | [The price list](/wiki/resources/prices) | protocol |
| Linear memory of one instance | At most 65,536 pages of 64 KiB | protocol |
| Deployed module | At most 2^30 bytes | protocol |
| Function parameters | At most 1,000 | protocol |
| Parameters plus declared locals | At most 16,384 per function | protocol |
| Reserved slots per function | At most 32,768, including the maximum operand height | protocol |
| Functions, types, globals per module | At most 1,000,000 each | protocol |
| Data and element segments | At most 100,000 each | protocol |
| Table entries | At most the module's function count | protocol |

The voting rows are explained in [capacity voting](/wiki/consensus/capacity-voting),
the prices on [the price list](/wiki/resources/prices) and the module bounds in
[validation](/wiki/vm/validation).

Initial and minimum capacities form a single explicit commitment in genesis, and so
in the ChainId. Benchmarking chooses capacity numbers; it never redefines what a unit
of C, S, D or G means. The real duration of an epoch varies with block arrival and
with changes in hashpower.
