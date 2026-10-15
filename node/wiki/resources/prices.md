# The price list

Every compute (C) charge has a fixed price, set from measurements and frozen with the
rest of the protocol. One C is about the time a processor core takes for one integer
addition that has to wait for the one before it, about one clock cycle. Every other
price says how many of those additions an operation takes at worst. This page lists
the prices. When each one is debited, and in what order, is on
[exact metering](/wiki/vm/metering), [the host interface](/wiki/vm/host) and
[deploy and invoke](/wiki/vm/lifecycle).

## Instructions

An instruction's price is debited immediately before it executes.

| Price | Instructions |
|---:|---|
| 0 | `else`, `end` |
| 1 | `unreachable`, `nop`, `block`, `br`, `return`, `drop`, `local.get`, `local.set`, `local.tee`, `global.get`, `global.set`, `memory.size`, `memory.grow` (plus its pages), `i32.const`, `i64.const`; `add`, `sub`, `and`, `or`, `xor`, `shl`, `shr_s`, `shr_u` of both widths; `i32.wrap_i64`, `i64.extend_i32_s`, `i64.extend_i32_u` and the five sign-extension operators |
| 2 | `loop`, `rotl` and `rotr` of both widths, `i64.add128`, `i64.sub128` |
| 3 | `select` |
| 4 | `eqz` and every comparison of both widths; `clz`, `ctz`, `popcnt` of both widths |
| 5 | `mul` of both widths |
| 8 | `i64.mul_wide_s`, `i64.mul_wide_u` |
| 9 | `call` of a defined function, before the callee's reservation |
| 16 | `memory.copy`, `memory.fill`, before their bytes |
| 18 | `if`, `br_if` |
| 33 | `div_s`, `div_u`, `rem_s`, `rem_u` of both widths |
| 37 | `br_table` |
| 50 | `call_indirect`, before the callee's reservation, or before the host call's 94 when the table entry is a host import |
| 94 | `call` of a host import, before the import's own charges |
| tier | every load and store, below |

Entering a defined function also debits its reservation, one C per parameter,
declared local and slot of its maximum operand height
([function entry](/wiki/vm/metering#function-entry)), and 40,000 C for every
64 KiB the entry adds to the invocation's [stack](#stack) beyond its highest point
so far. A `call_indirect` whose table
entry turns out to be a host import debits 94, the price of a host call, after its
table and type checks, where a defined function's reservation would be.

## Memory tiers

A load waits for memory, and how long depends on how much memory the program has: a
small memory stays in the processor's caches, a large one does not. Loads, stores
and every byte a program moves are therefore priced by the **tier** of the executing
instance's current memory size, its pages times 64 KiB.

| Memory size | Load | Store | Bytes, per 32-byte word |
|---|---:|---:|---:|
| at most 64 KiB | 18 | 1 | 4 |
| at most 1 MiB | 31 | 1 | 8 |
| at most 2 MiB | 51 | 1 | 8 |
| at most 64 MiB | 690 | 50 | 12 |
| at most 256 MiB | 770 | 90 | 12 |
| at most 1 GiB | 1,020 | 250 | 12 |
| more | 1,300 | 400 | 12 |

The load and store prices are the whole price of those instructions. The byte price
applies to `words(n)`, the started 32-byte words of `n` bytes:

```text
words(n) = ceil(n / 32)
bytes(n) = byte price of the tier × words(n)
```

`bytes(n)` is the price of the `n` bytes of a `memory.copy` or `memory.fill`, of every
copy between a program and the host in either direction, and of the bytes a program
returns, reverts with or emits. A successful `memory.grow` changes the tier for the
instructions after it. Instance preparation copies the data segments at the tier of
the initial memory.

## Memory

Each 64 KiB page costs **40,000 C**: every page of an instance's initial memory, at
preparation, and every page `memory.grow` adds. The price covers zeroing the page and
the operating system's first touch of it. It also bounds memory: one C buys at most
about 1.6 bytes of it, so a node knows from the compute capacity alone how much memory
a block can need, about twice that if it keeps its own copies of nested inputs and
code ([metering](/wiki/vm/metering#memory-growth)).

## Stack

An invocation's stack holds, for every function active in it, the slots of its
reservation, 8 bytes each, and a record of 3 slots. Its **height** is the sum, over
the active functions, of their reservations plus 3. The first 64 KiB, 8,192 slots,
of every invocation's stack are part of its start price. Beyond them, the stack is
priced as memory is, **40,000 C** per 64 KiB, for every 64 KiB its highest point so
far, its **mark**, passes:

```text
stack pages of a height h = ⌈h / 8,192⌉ − 1, and 0 for h ≤ 8,192
```

Entering a function that takes the height above the mark pays the stack pages of
the new height minus those of the mark, with its reservation, and the new height
becomes the mark. Returning lowers the height but never the mark, so a stack paid for
once is used again for free. Memory and stack together therefore hold at most about
1.6 bytes per C of the budget ([metering](/wiki/vm/metering#function-entry)).

## Hashing

Every SHA-256 the runtime computes on a program's behalf is priced by the length `m`
of its input, the [domain-separated](/wiki/data/encoding) message of 7 bytes plus the
tag and the hashed bytes:

```text
sha(m) = 100 + 900 × ceil((m + 9) / 64)
```

| Hash | `m` | Price |
|---|---:|---:|
| A record's key, with every record read | 81 | 1,900 |
| A deployment's ContractId | 87 | 1,900 |
| A value of `n` bytes written to a record, into its leaf | `n + 53` | `100 + 900 × ceil((n + 62) / 64)` |
| A deployment's code of `n` bytes, its CodeHash | `n + 11` | `100 + 900 × ceil((n + 20) / 64)` |

The node hashes on a record's path through the
[state tree](/wiki/data/state-tree) are part of the record's state access, S, not of C.

## Invocations

Every run of a program, a top-level call, a nested call or a deployment's
initializer, starts a fresh instance. Its preparation is one debit:

```text
preparation = 40,000
            + 40,000 × pages of initial memory
            + 40 × globals
            + 4 × table entries
            + 12 × active data segments
            + bytes(total length of the active data segments)
```

The 40,000 is the start price: setting up and tearing down an instance, whatever the
program. The rest pays for the instance's memory, its globals and table, and copying
its data.

## Code

Preparing code to run means decoding and validating it, the same work whether a
deployment installs it or a call finds it not yet prepared. Its **code price** is read
from the module's section framing alone, before any validation:

```text
code price = 14,400
           + the module's bytes, each at its rate below
           + 1,215 × functions each function section declares
           + 446 × segments each data section declares
```

| Bytes | Rate, C per byte |
|---|---:|
| Contents of the element section | 175 |
| Contents of the global section | 100 |
| Contents of the data section | 6 |
| Every other byte: the preamble, every section's id and size, every other section's contents | 60 |

The framing is read from the 8-byte preamble on as a sequence of sections, each
an id byte, a size as an unsigned LEB128 of at most five bytes whose value fits 32
bits, and that many bytes of contents. The scan stops at the first section whose id or
size cannot be read or whose contents run past the end; every byte from there on is
priced at 60. A declared count is the first such LEB128 of its section's contents,
zero if it cannot be read; every section with id 3 adds its count, and every section
with id 11 its own, however many such sections a module has. Nothing else of the
module is inspected, so the price is known before validation, and invalid code pays
it too.

The price is an exact number, however large: a module of repeated function sections
can declare more than 2^64 − 1 C of it. Like any price beyond what is left of the
budget, it is refused whole, and the deployment fails for compute. A deployment is
always charged something before its code price, so from 2^64 − 1 C on a price is
always refused, and an implementation may count it up to there and stop.

A deployment pays the code price once, for validation, and records it in the
contract's [manifest](/wiki/data/records#namespaces). A call pays it the first time
a transaction invokes a CodeHash that the transaction has not yet prepared, whether
by deploying it or by an earlier call, failed or not; further invocations of the same
code in the same transaction pay none. Every transaction starts with nothing prepared.

## Fixed charges

| Charge | Price |
|---|---:|
| A transaction's root dispatch | 1 C |
| Producing a successful deployment's identifier | 32 C and 32 D |

## Where the numbers come from

The prices were measured on the three kinds of core of one laptop processor, a
performance core, an efficiency core and a low-power core with no third-level cache,
against a compiling WebAssembly engine with metering. On each core, each operation was
timed in the worst form a program can give it, such as a chain of loads that each wait
for the previous one or branches with random outcomes, and expressed in that core's own
dependent additions. The price is the highest of the three, rounded up. Hashing is
priced at the speed of software SHA-256 on the low-power core, as on a processor
without SHA instructions, and code at the speed of the reference decoder.

Real programs run well inside their prices: compiled, the reference signature checks
and hashes take 6 to 19 percent of the time their C allows. Only a program built to be
slow reaches its price, and none exceeds it. Why prices are measured rather than
uniform is on [prices, not caps](/wiki/decisions/prices-not-caps).
