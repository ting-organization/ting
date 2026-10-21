# Nothing is refunded

## Decision

Rollback restores state, never consumed resources:

- a failed call or transaction keeps everything it consumed;
- G counts gross allocation, so deleting a record earns nothing back and recreating
  it pays again;
- D counts output when it is generated, even if a failing caller discards it.

## Why

- **The work was done.** Every validator executed those instructions, read those
  records and produced those bytes. Counters describe work, and the work does not
  disappear on rollback.
- **No recycling.** If rollback restored counters, a transaction could allocate, revert
  and allocate again, or generate output and discard it, doing far more work than its
  budget shows. Monotonic counters make the receipt an honest bound on what validating
  the transaction cost.
- **Simple, checkable totals.** Block totals are plain sums of what transactions used,
  which every node can verify independently.

## Consequences

- Failing late is expensive, so programs should check cheap conditions first.
- A transaction that runs out of budget still pays for everything up to that point,
  and its receipt says so.

## Trade-offs

Patterns that use temporary storage as scratch space pay full growth for it. Programs
keep temporary data in memory instead.
