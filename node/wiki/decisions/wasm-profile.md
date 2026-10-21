# A dated WebAssembly subset, implemented in-house

## Decision

Programs are modules in a precise integer subset of WebAssembly Core 1.0, the W3C
Recommendation dated 5 December 2019, including indirect calls through one immutable
table, plus:

- three additions from the WebAssembly 2.0 specification: the sign-extension
  operators, `memory.copy` and `memory.fill`;
- the four instructions of the wide-arithmetic proposal.

Ting takes the binary format and the meaning of each instruction from those
documents, specifies the byte whitelist, the charging, the host interface and the
failure order itself, and implements the whole machine in its own code.

## Why a WebAssembly subset

- **A shared, dated definition.** Arithmetic, memory and control flow are where
  implementations drift apart. A dated standard gives every implementation the same
  definition to test against, and official test suites to test with.
- **Compilers exist.** Rust, C and other toolchains emit the subset today; a chain
  that defined its own bytecode would also have to maintain its own compilers.
- **Fixed, not moving.** Pinning dated texts and an explicit whitelist means nothing
  can silently add instructions or change charges. Any change requires a
  specification change first.
- **Small enough to verify.** 115 opcodes, one memory, one table and twelve imports
  can be tested exhaustively, and every accepted opcode has a conformance witness.
- **Deterministic by construction.** Features with nondeterministic corners, such as
  threads, or floating point, whose NaN bit patterns WebAssembly allows to vary, are
  outside the profile.

## Why its own implementation

- **No dependency defines consensus.** The decoder, validator, interpreter and host
  are a few thousand lines in this repository, under the crate's ban on `unsafe`.
  No upgrade of a third-party engine can change what a block means, and nothing
  outside the protocol rules has to be audited to know what the chain does.
- **Charging where the rules say.** An interpreter that counts as it
  executes charges each unit at the exact point the rules name, without
  rewriting modules or inserting calls.
- **Faster, not slower.** Charging inline costs almost nothing, where a charge call
  before every instruction dominates an instrumented engine; on metered loops the
  reference runs an order of magnitude faster than an engine instrumented that way.
- **Portable.** A small interpreter with no platform assumptions runs the same in a
  node, in a wallet simulating a transaction, or in a browser.
- **Still checked.** Owning the implementation does not mean trusting it: it runs
  against its own test suite, whose expected counters come from a separate model of
  the rules, never from an engine, and every chain starts with that suite as its
  seed, which every node replays ([conformance](../reference/conformance.md)).

## Why these additions

Bulk copies and fills move memory far more cheaply than a metered loop of loads and
stores. Sign extension costs nothing to support. All three are deterministic and
precisely specified, and nothing else of Release 2.0 is taken. Indirect calls are
not an addition but part of Core 1.0, kept because compilers emit them for function
pointers, closures and dynamic dispatch; without them, a frozen chain would depend
on toolchains keeping a restricted mode alive for decades.

Wide arithmetic matters because cryptography lives in programs
([crypto in programs](crypto-in-programs.md)). Elliptic-curve and other
public-key arithmetic is built from 64-bit limbs multiplied into 128-bit products.
Without `i64.mul_wide_u`, each product takes four half-width multiplications and the
carries between them; with it, one instruction. A 256-bit multiplication then needs
about a third of the instructions and costs about a third as much. The four
instructions are deterministic, never trap, and have a final definition, so they
can be frozen like the rest.

## Trade-offs

- Compilers must target the Core 1.0 feature level plus exactly these additions and
  produce integer-only code.
- No table instructions means a table is fixed at instantiation; dynamic code loading
  is still a deployment.
- Anything left out, such as 64-bit memories or SIMD, stays out.
- Owning the implementation means earning trust continuously: every change to it must
  keep the VM tests passing and replay the seed to the same blocks.
