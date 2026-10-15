# Four counters, not one price

## Decision

Execution is measured with four independent counters: compute, state access, data and
growth. They are never converted into one another or combined. Each has its own
transaction budget and its own block capacity.

## Why

- **They are different bottlenecks.** Compute is processor time, state access is
  database latency, data is bandwidth and block size, and growth is storage that every
  node keeps for the long term. Hardware improves at different rates for each.
- **One number needs exchange rates.** Combining them requires deciding how much
  compute one storage read is worth. That rate depends on hardware and changes with
  it, and on other chains it has been revised by protocol upgrades (Ethereum's
  EIP-2929, for example, raised the cost of state access relative to computation).
  Separate counters need no exchange rate, so no rate goes stale.
- **Capacity can follow each resource.** Miners vote each capacity separately, so
  more compute does not have to mean more state growth.

## What is counted

Counters describe logical events: an instruction dispatched, a record read, a byte
generated, a byte allocated. They never describe elapsed time, cache behavior or disk
operations, which is what makes them identical on every node. See
[What each counter measures](/wiki/resources/counters).

## Trade-offs

- A transaction declares four budgets instead of one.
- Filling a block is a four-dimensional packing problem.
- There is no single "cost" number to compare transactions by. Any pricing is up to
  miners and applications.
