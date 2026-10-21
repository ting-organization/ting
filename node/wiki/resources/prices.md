# The price list

Every compute (C) charge has a fixed price, set from measurements and frozen with the
rest of the protocol. One C is about the time a processor core takes for one integer
addition that has to wait for the one before it, about one clock cycle. Every other
price says how many of those additions an operation was priced to cover, so a
division costs 33 and a load from a large memory costs hundreds. Because the prices
are fixed numbers, every node charges the same amounts however fast it really runs.
This page lists the prices a program author needs to estimate a program's compute.
The exact rules are in
[section 15 of the protocol rules](../../../ting-protocol-rules.md#15-the-price-list),
and when each price is debited is in
[section 21](../../../ting-protocol-rules.md#21-exact-metering).

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
| 18 | `if`, `br_if` |
| 20 | `memory.copy`, `memory.fill`, before their bytes |
| 33 | `div_s`, `div_u`, `rem_s`, `rem_u` of both widths |
| 37 | `br_table` |
| 50 | `call_indirect`, before the callee's reservation, or before a host call's 94 when the table entry is a host import |
| 94 | `call` of a host import, before the import's own charges |
| by memory size | every load and store, below |

Entering a defined function also pays for its reservation: one C per parameter,
declared local and slot of its maximum operand height, plus any [stack](#stack) it
adds; the exact debit is in
[function entry](../../../ting-protocol-rules.md#214-function-entry). A conditional
branch costs 18, far more than an addition, because it is priced as the branch the
processor never predicts.

## Memory prices

A load waits for memory, and how long depends on how much memory the program has. A
small memory stays in the processor's caches. Past the processor's own cache, 2 MiB
on the slowest machine the prices are set for, an access can be a trip to RAM, and a
very large memory also misses the processor's address-translation caches. Loads,
stores, the bytes a program moves and the spans it moves them in are therefore
priced by the executing instance's current memory size, in 64 KiB pages.

Each memory size has four prices. The load and store prices are the whole price of
those instructions. Moving bytes costs the **word** price for every started 32 bytes,
and the **span** price for every span that holds at least one byte:

```text
words(n)    = ceil(n / 32)
bytes(n, s) = word × words(n) + span × s      n bytes in s spans that hold a byte
```

A span is a stretch of memory an operation reads or writes in one piece. A
`memory.copy` has two, its source and its destination, and a `memory.fill` has one.
The host has one for each argument it reads and one for an output it writes. Each
span can be a trip to RAM on its own, however few bytes it holds, which is why above
2 MiB the span price is that of a store. `bytes(n, s)` is the price of a
`memory.copy` or `memory.fill`, of every copy between a program and the host in
either direction, of the bytes a program returns, reverts with or emits, and of
copying the data segments into a fresh instance.

Up to 2 MiB the prices step with the caches:

| Memory size | Load | Store | Word | Span |
|---|---:|---:|---:|---:|
| at most 64 KiB, 1 page | 18 | 1 | 4 | 0 |
| at most 1 MiB, 16 pages | 31 | 1 | 8 | 0 |
| at most 2 MiB, 32 pages | 80 | 1 | 8 | 0 |

Above 2 MiB they follow a curve over the size. `L` is the memory size on a
logarithmic scale, in whole sixteenths of a doubling: 80 at 32 pages, and 16 more
for every doubling, up to 256 at 4 GiB.

```text
load  = 770 + (L − 80)² / 48, rounded up
store = 16 + (L − 80)² / 72, rounded up
word  = 12
span  = store
```

| Memory size | Pages | L | Load | Store and span |
|---|---:|---:|---:|---:|
| 2 MiB + 64 KiB | 33 | 80 | 770 | 16 |
| 3 MiB | 48 | 88 | 772 | 17 |
| 16 MiB | 256 | 128 | 818 | 48 |
| 64 MiB | 1,024 | 160 | 904 | 105 |
| 256 MiB | 4,096 | 192 | 1,032 | 191 |
| 1 GiB | 16,384 | 224 | 1,202 | 304 |
| 4 GiB | 65,536 | 256 | 1,416 | 447 |

How `L` is computed between doublings, in integers only, is in
[memory prices](../../../ting-protocol-rules.md#152-memory-prices). A successful
`memory.grow` moves the prices for the instructions after it, so a program that keeps
its memory at or under 64 KiB gets the cheapest loads and free spans.

## Memory

Each 64 KiB page costs **40,000 C**: every page of an instance's initial memory, at
preparation, and every page `memory.grow` adds. The price covers zeroing the page and
the operating system's first touch of it. It also bounds memory: one C buys at most
about 1.6 bytes of it, so a node knows from the compute capacity alone how much memory
a block can need.

## Stack

An invocation's stack holds, for every function active in it, the slots of its
reservation, 8 bytes each, and a record of 3 slots. The first 64 KiB, 8,192 slots,
of every invocation's stack are part of its start price. Beyond them, the stack is
priced like memory: **40,000 C** for every 64 KiB that its highest point so far, its
**mark**, passes.

```text
stack pages of a height h = ⌈h / 8,192⌉ − 1, and 0 for h ≤ 8,192
```

A call that takes the stack above the mark pays for the pages it adds, and the mark
moves up. Returning lowers the stack but never the mark, so a stack paid for once is
used again for free. A program whose calls stay within the first 64 KiB of stack pays
nothing for it; deep recursion pays as memory does, and memory and stack together
hold at most about 1.6 bytes per C of the budget.

## Hashing

Every SHA-256 the runtime computes on a program's behalf is priced by the length `m`
of its input: the whole [domain-separated](../data/encoding.md) message, 7 bytes plus
the tag plus the hashed bytes.

```text
sha(m) = 100 + 900 × ceil((m + 9) / 64)
```

| Hash | `m` | Price |
|---|---:|---:|
| A record's key, with every record read | 81 | 1,900 |
| A deployment's ContractId | 87 | 1,900 |
| A value of `n` bytes written to a record, into its leaf | `n + 53` | `100 + 900 × ceil((n + 62) / 64)` |
| A deployment's code of `n` bytes, its CodeHash | `n + 11` | `100 + 900 × ceil((n + 20) / 64)` |

The hashes a node computes along a record's path through the
[state tree](../data/state-tree.md) are part of the record's state access, S, not of C.

## Invocations

Every run of a program, a top-level call, a nested call or a deployment's
initializer, starts a fresh instance. Its preparation is one debit:

```text
preparation = 40,000
            + 40,000 × pages of initial memory
            + 40 × globals
            + 4 × table entries
            + 12 × active data segments
            + bytes(data bytes, data spans)
```

The data bytes are the total length of the active data segments, and the data spans
are the number of those segments that hold at least one byte, priced at the initial
memory's size.

The 40,000 is the start price: setting up and tearing down an instance, whatever the
program. The rest pays for the instance's memory, its globals and table, and copying
its data.

## Code

Preparing code to run means decoding and validating it, the same work whether a
deployment installs it or a call finds it not yet prepared. Its **code price** is read
from the module's section framing alone, so it is known before validation and invalid
code pays it too:

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

A 20 KiB module with no data, global or element section costs
14,400 + 20,480 × 60 = 1,243,200 C, plus 1,215 C for each function it declares.
How the framing is
scanned, including for malformed modules, is in
[code](../../../ting-protocol-rules.md#157-code) and
[the code-price scan](../../../ting-protocol-rules.md#261-code-price-scan-including-malformed-bytes).

A deployment pays the code price for validation every time, and records it in the
contract's [manifest](../data/records.md#namespaces). A call pays it when its
transaction has not yet prepared that CodeHash; further invocations of the same code
in the same transaction pay none. Every transaction starts with nothing prepared, so
a node's compiled-code cache never makes a transaction cheaper.

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
uniform is on [prices, not caps](../decisions/prices-not-caps.md).
