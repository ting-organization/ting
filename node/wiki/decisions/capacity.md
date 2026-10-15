# Capacity moves, rules do not

## Decision

The per-block capacities for C, S, D and G are the only aspect of execution that can
change after a freeze. They move through work-weighted miner votes: a two-thirds
threshold and 5% steps to increase, a majority and 10% steps to decrease, two epochs
of notice, floors fixed at genesis and no ceiling. The voting rule itself is frozen.

## Why

- **Throughput has to be able to change.** Hardware and implementations improve. With
  the charge for each operation fixed forever, more throughput can only come from
  more capacity.
- **It must be able to change forever.** A ceiling chosen before launch would become
  the binding constraint the moment hardware passed it, and no vote could lift it.
  Without a ceiling, the ability to reverse growth is the safeguard instead,
  which is why braking needs only a majority and moves twice as fast as growth.
- **Braking must be harder to block than growing is to win.** Votes are counted
  from canonical blocks, and a selfish miner wins a larger share of those than its
  share of hashpower. With a two-thirds threshold in both directions, one miner
  holding roughly 27–33% of the work could veto every decrease. With a majority
  brake, blocking a decrease takes 37–50%, while winning an increase still takes
  about 44% even with selfish mining. Capacity voting is never easier to capture
  than the chain itself.
- **Capacity is safe to adjust.** Changing how much fits in a block does not change
  what any program does or what any operation costs, so it cannot break applications.
- **Slow and visible.** Small steps, a supermajority of work and advance notice mean
  every participant sees a change coming, and no single short burst of hashpower can
  push through more than one small step.
- **Fast enough to follow a faster engine.** An implementation that runs programs
  several times faster, such as a compiler in place of an interpreter, should be
  usable within a year or two, not after half a decade. A 5% step allows about 3.6×
  a year. What users actually want to run, and how fast miners and nodes are willing
  to let the chain grow, stay the real limits.

## How throughput grows

1. **Implementations get faster.** Validating the same counted work takes less time.
2. **Miners vote capacity up.** More counted work fits in each block.
3. **Programs get better.** More efficient algorithms do the same job with fewer units.

None of these steps changes a rule.

## Trade-offs

- Growth is gradual: at most one 5% step per resource per epoch, about 3.6× per
  year at the production network's epoch of 20,160 blocks.
- Hashpower decides. Miners with a large share of the work have a large share of
  the vote, and no voting rule protects against a miner that already controls the
  chain itself.
- There is no rent and no expiry, so the growth capacity miners vote is the only
  bound on how much state the network commits to keeping forever.
