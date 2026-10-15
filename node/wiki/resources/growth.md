# Persistent growth

Most resources are used up once a block has been processed. State is different: a
record written today has to be kept by every node for as long as it exists. G
measures how much a transaction asks the chain to allocate.

## The rule

A record's logical allocation is `36 + len(value)` bytes: its 32-byte key, a 4-byte
length and the value. Each write is charged the allocation it adds:

```text
growth(absent -> value) = 36 + len(value)
growth(old -> new)      = max(0, len(new) - len(old))
growth(delete)          = 0
```

Creating a record with an empty value still costs 36. The 36 bytes are a logical
unit, not a claim about how many bytes a database actually uses: how a node stores
records and tree nodes is its own business.

## When it is charged

Each positive allocation is charged when the write is attempted: after the
operation's required preflight checks and the read of the old record, in one atomic
debit together with the write's state access and the compute to hash the value,
and before the new value is installed. If that debit does not fit, nothing is
written. The exact order of a storage write is on
[The host interface](/wiki/vm/host).

The runtime's own records follow the same rule. Installing `n` bytes of code writes a
44-byte manifest (the CodeHash, the code length and the code price) and the code
record:

```text
manifest:  36 + 44
code:      36 + n
total:     116 + n     charged before the initializer runs
```

## Gross, not net

G counts allocation when it happens, not the final size of the state:

- creating, deleting and recreating a record pays for the creation twice;
- growing a value and then shrinking it still pays for the growth;
- allocation inside a call that later reverts stays charged.

A transaction cannot hide allocation work behind a small final result. Deleting a
record frees space but earns no credit.

> [!NOTE]
> G bounds how much a block may allocate. How large live state may become over time,
> and how long nodes keep history, are separate engineering questions.
