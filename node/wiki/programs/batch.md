# Batch

> [!NOTE]
> A reference program, not part of the protocol: the chain runs it like any other
> deployment. See [Reference programs](/wiki/programs).

`batch` runs several calls in one transaction, in order, all or none. A transaction
is a single call ([transactions](/wiki/data/blocks#transactions)), so this is how
operations that belong together travel together: above all, an operation that pays
no miner with a payment from another account
([paying for a transaction](/wiki/programs/conventions#paying-for-a-transaction)).

- A cancel from a time-locked savings account, which may not pay a fee, followed by
  a zero transfer from another account that carries the fee.
- A guardian's recovery approval, or any other operation of the
  [identity registry](/wiki/programs/identity), which pays nothing itself, followed
  by a fee.
- Several signed actions of different accounts or different coins at once, all or
  none for whoever submits them. None of them depends on the others, though: each
  stays valid alone.

The calls go out from `batch`, so the called programs see `batch` as their caller.
Everything the reference programs authorize with a signature works regardless of the
caller. Each call stays valid on its own, so a call that pays must be a
[bound action](/wiki/programs/conventions#bound-actions): bound to the batch and to
the calls before it, it cannot be taken out and submitted alone, nor attached to
other calls. It commits only to the calls before it, so it pays for one or more calls
and never comes first.

## Input and result

```text
input  = call*                   call = target(32) ‖ Bytes(input)
result = Bytes(result)*          one per call, in order
```

The input is any number of calls back to back, up to its end. Each names the
ContractId to call and the input to call it with. Integers are little-endian and
`Bytes(x)` is the length-prefixed encoding of the
[conventions](/wiki/programs/conventions#encoding).

1. Input that is not an exact sequence of calls reverts `bad input`.
2. The calls run one after another as ordinary calls, each with the child budget of
   the [conventions](/wiki/programs/conventions#the-budget-reserve): the remaining
   budget minus the standard reserve R. A remaining budget below R in any counter,
   or a call that returns 128 because its budget did not fit, reverts
   `insufficient budget`.
3. The first call that does not succeed reverts the whole batch, undoing every call
   before it ([checkpoints](/wiki/vm/calls#checkpoints)), with that call's own result
   bytes as the batch's result, unchanged: the revert reason of a reference program,
   or nothing when the call traps, runs out of budget or names no program. The
   batch's own status is a revert in every case, so a receipt does not tell these
   apart. Since the bytes come from the called program, a reason proves nothing
   about which program gave it.
4. When every call succeeds, the result is each call's result as `Bytes`, in order.

Empty input is a batch of no calls: it succeeds with an empty result and writes
nothing, which is the program's
[canonical deployment](/wiki/programs#canonical-deployment).

## The prefix

While a batch runs, its record `00…00` holds the prefix of the call it is making: a
hash of every call before it in the same batch.

```text
prefix(0)     = H("app/batch/prefix", "")
prefix(i + 1) = H("app/batch/prefix", prefix(i) ‖ target(i) ‖ Bytes(input(i)))
```

The batch writes `prefix(i)` just before it makes call `i`, so the record names only
calls that have completed. When the batch ends successfully it restores the record
as it found it: a batch inside a batch puts back the outer batch's prefix, and an
outermost batch deletes the record. A batch that fails is rolled back with
everything else. The record therefore exists only while a batch is running.

The one-byte input `00` is a query: it returns the 32-byte prefix of the call the
running batch is making, or reverts `not running` when no batch is running. It
writes nothing, and no call list is one byte long.

A payment that pays for the calls before it is bound to the batch and to `prefix(k)`
for its own position `k`, and goes last. The application that pays checks that the
batch is its caller and asks the batch for the prefix
([bound actions](/wiki/programs/conventions#bound-actions)). The payment is then
valid only as call `k` of a batch whose first `k` calls were exactly the committed
ones, and since a batch is all or none, only when those calls stay in effect. Calls
after the payment are not committed to, nor calls that a program makes before or
after calling the batch; whoever pays puts the payment last. A call bound at the first
position commits to nothing, since every batch starts from `prefix(0)`.

`batch` writes its record during every run with calls, so it cannot run read-only:
called read-only, it fails at its first write
([read-only mode](/wiki/vm/calls#read-only-mode)). The query and empty input write
nothing and succeed read-only too, and input that does not decode reverts
`bad input` before any write.

## Never send it assets

> [!WARNING]
> Anyone can make `batch` call anything, so `batch` acts for whoever submits it.
> Its [contract account](/wiki/programs/conventions#contract-accounts) in an
> application, `H("app/account-v1/contract", batch)`, therefore belongs to everyone:
> coins sent to it can be taken by the next transaction. Never send assets to it, and
> never make it a minter, a guardian's key or any other authority.

## Costs

Each call costs what the called program spends plus an invocation of its own, about
84,000 C and 2 S, and its code price the first time a transaction runs that program
([the cost of an invocation](/wiki/programs/conventions#the-cost-of-an-invocation)).
It holds back one more reserve R while it runs. `batch` itself costs about 415,000 C
and 7 S with one call, mostly its code price, and for each further call about
35,000 C and 2 S: the copying of the input and the result at the
[byte price](/wiki/resources/prices#memory-tiers), one record write and one prefix
hash. The batch hashes in its own code, at about 130 C per byte of a short input and
more for a long one, and hashes nothing for the last call, which no call follows, so
a payment that goes last adds no hash. A sponsored savings cancel, the cancel and a
bound payment from another account, costs about 25 million C and 44 S: two signature
checks and the work around them.
