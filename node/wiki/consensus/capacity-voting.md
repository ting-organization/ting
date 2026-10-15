# Capacity voting

Block capacity is how much compute, state access, data and growth one block may use.
It is the one aspect of execution that can still change after a freeze. Miners change
it by voting, through a rule that is itself frozen. There is no ceiling: capacity can
follow hardware for as long as the chain exists.

## Signals

Every header carries four votes, one per resource in C, S, D, G order: 0 for hold,
1 for increase or 2 for decrease. Any other value makes the header invalid. A vote is
weighted by the [work](/wiki/consensus/proof-of-work#work-not-length) of the block
that carries it, `work(target)`, so voting costs exactly as much hashpower as mining.
There is no registry of miners, no signature and no identity. Only blocks on the
branch being evaluated count; stale blocks and blocks on other branches signal
nothing. The genesis anchor carries no votes.

## Counting

Votes are counted per **epoch** of E blocks, where E is the voting epoch length fixed
at [genesis](/wiki/consensus/genesis), and separately for each resource. If blocks
carrying at least two-thirds of the epoch's work signal increase, the decision is
increase. If blocks carrying more than half of the work signal decrease, the decision
is decrease. Otherwise capacity holds.

```text
epoch(h) = floor((h − 1) / E)          // for a mined height h ≥ 1

W = work of all blocks in the epoch
U = work of the blocks that signal increase
L = work of the blocks that signal decrease

direction = increase   if 3 × U ≥ 2 × W
            decrease   if 2 × L > W
            hold       otherwise
```

Both cannot pass in one epoch: an increase leaves at most a third of the work to
signal decrease, which is not a majority.

The two thresholds differ on purpose. Growth needs broad, sustained agreement;
braking needs only a majority and moves twice as fast. That asymmetry is what
stands in for a ceiling: a single large miner or a rented supermajority can win one small
step, and any majority can reverse it.

## Notice

A decision made in epoch *e* takes effect at the start of epoch *e* + 3, so two
complete epochs pass between the vote and the change. The first three epochs use the
initial capacities. At the start of every later epoch, the direction scheduled for it
is applied to the previous epoch's capacity with the steps below, for each resource
independently:

```text
limit(e) = initial_limits                           for e = 0, 1, 2
limit(e) = apply(direction(e − 3), limit(e − 1))    for e ≥ 3
```

A block at height *h* must fit `limit(epoch(h))`, its
[active capacity](/wiki/resources/capacity). Pending changes are derived from the
ancestry and roll back with it; no block can override them. The production genesis
uses epochs of 20,160 blocks, about two weeks at the target rate.

## Steps and the floor

An increase moves a capacity up by 5%, rounded down but at least 1, with no upper
limit (it saturates at 2^64 − 1). A decrease moves it down by 10%, rounded down but at
least 1, never below the floor fixed at genesis. A hold leaves it unchanged.

```text
increase(x) = min(2^64 − 1, x + max(1, floor(x / 20)))
decrease(x) = max(minimum, x − max(1, floor(x / 10)))
hold(x)     = x
```

At one increase per two-week epoch, capacity can double in about six and a half
months and grow by a factor of about 3.6 per year, indefinitely. Decreases in every
epoch halve it in about three months.

## What votes cannot change

Votes change capacity and nothing else. They cannot change what a counter measures,
what any operation costs, what an instruction means, any format, hashing or other
cryptography, the difficulty rule, or whether a program's actions are valid. Capacity
history is derived from the branch, so a reorganization switches it along with
everything else.
