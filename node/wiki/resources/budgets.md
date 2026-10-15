# Budgets and exhaustion

## Budgets within budgets

A transaction declares its maximum C, S, D and G. When a program calls another
contract, it gives the child a sub-budget that must fit in what the caller has left.
Whatever the child consumes is also charged to every enclosing budget, and whatever it
doesn't use stays available to the caller.

Each of these is a frame: the transaction's top-level invocation, whose budget is
the declared one, and every nested call or deployment, whose budget is its
sub-budget. Every frame has a budget and a consumed vector that only ever grows. A
request for a sub-budget larger than the caller has left creates no child; see
[Calls, rollback and read-only](/wiki/vm/calls).

## Atomic debits

Before each counted event, the whole debit is checked against the current frame and
every frame around it. If it fits everywhere, it is charged everywhere and the event
happens. If not, nothing is charged, the event does not happen, and the invocation
that attempted it fails.

```text
debit d = [c, s, d, g] attempted by frame f:
    if consumed(x) + d <= budget(x) for f and every frame enclosing f:
        consumed(x) += d for all of those frames
        perform the event
    else:
        apply no part of d, do not perform the event, and fail f
```

Earlier debits that succeeded stay consumed. When an event would exceed more than
one allowance at once, the failure reports the first in the order C, S, D, G, so
every implementation reports the same status:

| First counter that does not fit | Status of the failed invocation |
|---|---:|
| C | 3 |
| S | 4 |
| D | 5 |
| G | 6 |

The failed invocation's effects are rolled back and its caller receives the status
and may continue; at top level it becomes the status in the
[receipt](/wiki/data/blocks). Which checks a host function makes before its debit,
and in what order, is on [The host interface](/wiki/vm/host).

## Nothing is refunded

Rollback restores state, never counters. A child that fails keeps its consumption
charged to its parent. A transaction that fails keeps everything it consumed, in its
receipt and in the block total. Unused allowance is simply not consumed; there is
nothing to refund because it was never spent.

A consumed vector can neither overflow nor become negative: it only grows, and only
by debits that fit under a budget.

The receipt records actual consumption, including all work done before a failure.
[Nothing is refunded](/wiki/decisions/no-refunds) explains why.

## Exhaustion is per transaction

A transaction runs out when its own budget is exhausted, never because the block is
filling up. Whether a block's transactions fit the block's capacity is a separate
check. A block that does not fit is invalid; its transactions do not get different
outcomes to make it fit. See [Block capacity and bounds](/wiki/resources/capacity).
