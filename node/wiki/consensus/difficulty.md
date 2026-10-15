# Time and difficulty

Difficulty keeps blocks arriving near the target interval as hashpower comes and goes.
It depends only on timestamps recorded in the chain, never on a node's own clock.

## Timestamps

A block's timestamp must be greater than the median of the timestamps of the blocks
before it: the previous eleven, or, while fewer than eleven blocks exist, all of them
plus the genesis timestamp. With an even number of samples, the upper of the two
middle values is used. For the block at height *h*:

```text
samples = timestamps at heights h − 1, h − 2, …, max(0, h − 11)   // height 0 is genesis
sorted  = samples in ascending order                            // count = min(h, 11)
median  = sorted[floor(count / 2)]                              // counted from 0
valid   = timestamp > median
```

That is the only consensus rule about time. A node should defer a header whose
timestamp is more than 7,200 seconds (two hours) ahead of its local clock, but that
is admission policy, not invalidity: the same block becomes acceptable once the clock
catches up. Difficulty and capacity calculations use only data in the branch.

Timestamps, like producer data, are context chosen by the miner. They authenticate no
external identity and are not unpredictable randomness. Nor are they precise, and
programs should not treat them as any of these.

## The retarget

Every block's target follows from one anchor, genesis, and one question: how far
is the chain ahead of or behind its schedule? The schedule says the parent of the
block at height *h* should have arrived `interval × (h − 1)` seconds after genesis.

```text
behind = (parent timestamp − genesis timestamp) − interval × (h − 1)
target = initial target × 2^(behind / half-life)
```

If the chain is one half-life behind schedule, the target doubles and mining gets
easier; one half-life ahead halves it. The exponent is evaluated with exact integer
arithmetic in steps of 1/65536, with a fixed cubic polynomial for the fractional
part, so every node computes the same target. There is no averaging window and no
limit on how far one block can move the target, and nothing makes mining easier just because a long time has passed since the last
block: being behind schedule is what matters, and a block that is early tightens
the target again.

## Exact arithmetic

The interval, half-life, genesis timestamp, initial target and PoW limit come from
[genesis](/wiki/consensus/genesis). For the block at height *h* with parent *p*:

```text
elapsed  = timestamp(p) − genesis_timestamp                 // signed
expected = target_interval × (h − 1)
exponent = floor(((elapsed − expected) × 65536) / half_life)
shifts   = floor(exponent / 65536)
frac     = exponent − shifts × 65536                        // 0 ≤ frac < 65536
factor   = ((195766423245049 × frac + 971821376 × frac² + 5127 × frac³ + 2^47) >> 48) + 65536
target   = clamp(floor(initial_target × factor × 2^(shifts − 16)), 1, pow_limit)
```

- Every value is a mathematical integer. Bounds, products and quotients are evaluated
  without any intermediate fixed-width overflow, and `floor` rounds toward negative
  infinity, for negative values too.
- `factor` is a cubic approximation of `65536 × 2^(frac / 65536)` in 16.48 fixed
  point; `>> 48` is floor division by 2^48.
- The factor `2^(shifts − 16)` is an exact multiplication when `shifts − 16` is
  zero or positive and a floor division by `2^(16 − shifts)` when it is negative.
  When `shifts − 16` is 256 or more the result saturates at `pow_limit`, and when
  it is −274 or less it saturates at 1.
- `clamp` raises a result below 1 to 1 and lowers a result above `pow_limit` to
  `pow_limit`.
- The first mined block (*h* = 1) has genesis as its parent, so
  `elapsed = expected = 0` and its target is the initial target.

A header's target must equal this value exactly. There is no minimum-difficulty
exception triggered by the clock or by a long gap between blocks.

## Where the rule comes from

This is the ASERT rule ("absolutely scheduled exponentially rising targets") that
Bitcoin Cash has used since November 2020 as `aserti3-2d`. It was chosen because a
window-based retarget oscillates on any chain that is not the biggest user of its
hash function: hashpower arrives when difficulty is low and leaves when it rises.
ASERT responds every block and cannot oscillate.

The polynomial and its fixed-point form are taken from the
[aserti3-2d specification](https://upgradespecs.bitcoincashnode.org/2020-11-15-asert/),
restated here with the genesis anchor. The arithmetic on this page is the rule; later
changes to that document do not change Ting. Ting's results are not interchangeable
with aserti3-2d's, for two reasons:

- **Rounding.** aserti3-2d truncates the exponent's division toward zero; Ting
  floors it. For a block ahead of schedule, whose exponent is negative, the two
  differ by one 1/65536 step whenever the division is not exact, and Ting's target is
  then about 0.001% lower.
- **Precision.** aserti3-2d stores targets in the compact `nBits` form, which rounds
  them. A Ting header carries the exact 256-bit target.

The production genesis uses a 60-second interval and a 12-hour half-life, chosen by
simulating the loss and the arrival of hashpower
([production profile](/wiki/consensus/genesis#production-profile)); the experimental
profile of development chains uses a two-day half-life.
