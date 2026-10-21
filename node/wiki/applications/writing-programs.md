# Writing a program

Any toolchain can produce Ting programs, as long as the final module fits the
[execution profile](../vm/profile.md). This page covers what to aim for.

## The minimum

A complete program is a WebAssembly module that:

1. imports the host functions it uses from module `ting`;
2. exports a function `invoke` with no parameters and no result;
3. exports its memory as `memory`, with an explicit maximum.

The [tour](../tour.md) shows one in a dozen lines of WebAssembly text.

## Choosing a toolchain

Hand-written WebAssembly text works well for small programs and tests. For larger
programs, use a compiler target limited to
[WebAssembly Core 1.0](https://www.w3.org/TR/2019/REC-wasm-core-1-20191205/) features,
such as Rust's `wasm32v1-none`. Enable the bulk-memory and sign-extension target
features of [Release 2.0](https://www.w3.org/TR/wasm-core-2/) and the wide-arithmetic
feature, and write integer-only code. Function pointers, closures and trait objects
compile to indirect calls through a table, which the profile accepts as long as the
table has a fixed size.

Linkers often add things the profile rejects: custom sections such as names and
debug information, and extra exports. Strip them before deploying.

The module must also be canonically encoded. Check it before deploying:

```sh
cargo run --locked -- inspect program.wasm
```

## Talking to the host

Pass byte strings as a pointer and a length. Either ask a getter for the length first,
or pass a buffer large enough for the largest value you accept.

Lengths are unsigned 32-bit numbers. Treat exactly `2^32 − 1` (−1 read as signed)
from `storage_get` or `code_hash` as absent, and any other value as a length.

End with `return` for results, or `revert` with an explanation in the result bytes for
errors. See [The host interface](../vm/host.md).

## Keeping it cheap

- **Declare little memory.** Every invocation pays 40,000 C for each page of initial
  memory, and loads, stores and copies cost more as memory grows past 64 KiB,
  1 MiB and 2 MiB, and more again the larger it gets
  ([memory prices](../resources/prices.md#memory-prices)). Keep the stack and the heap
  small, and grow memory at run time only when a transaction needs more.
- **Keep code small.** The first call to a program in a transaction pays to prepare
  its code, about 60 C per byte ([code](../resources/prices.md#code)). Later calls to
  the same code in that transaction do not, so batching related actions into one
  transaction saves it.
- **Prefer cheap instructions in hot loops.** Additions, shifts and logic cost 1 C,
  multiplication 5, comparisons 4, conditional branches 18 and division 33. Use the
  wide-arithmetic instructions for big-number arithmetic.
- **Store values whole.** One record holds a value of up to `2^32 − 2` bytes; splitting it into
  pieces only adds state accesses and key hashes.
- **Use bulk copies.** `memory.copy` and `memory.fill` cost far less than a loop of
  loads and stores.
- **Call shared programs; link small helpers.** A call costs about 84,000 C before
  the callee's first instruction, plus its code the first time in a transaction.
  Linking a small pure function into your own program avoids the call but makes
  every load of your program larger.
- **Size sub-budgets from the context.** The remaining-budget selector tells a
  program exactly what it can pass to a child.
- **Check cheap conditions first.** Work done before a failure is still paid for.
- **Leave headroom in budgets.** A transaction pays only for what it uses. A budget
  that is too tight fails and still pays.

## Testing

Run programs on a [development chain](../start/node.md#a-development-chain) and read
the receipts, which show the exact resources used. Every node computes the same
counters, so for the same state and input, the numbers you see locally are the
numbers the network charges.
