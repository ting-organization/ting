# What a module may contain

A program is a WebAssembly module, but Ting accepts only a strict, integer-only
subset of WebAssembly: a fixed list of instructions, a fixed module shape and one
encoding for every construct. A deployment checks all of it before anything is
installed and rejects whatever falls outside, so no node ever has to run code that
another node might read differently, and installed code never needs checking again.
This page explains what is accepted, what is rejected and why. The exact rules are in
[sections 16 to 18 of the protocol rules](../../../ting-protocol-rules.md#16-the-execution-profile),
with the complete type-checking algorithm in
[section 25](../../../ting-protocol-rules.md#25-complete-type-checking-algorithm).

## Where the definitions come from

The accepted instructions come from three dated texts:

- the integer subset of the
  [WebAssembly Core 1.0 Recommendation of 5 December 2019](https://www.w3.org/TR/2019/REC-wasm-core-1-20191205/);
- three additions from the
  [WebAssembly Core Specification, Release 2.0](https://www.w3.org/TR/wasm-core-2/):
  the sign-extension operators, `memory.copy` and `memory.fill`, and nothing else
  of that release;
- the four instructions of the
  [wide-arithmetic proposal](https://github.com/WebAssembly/wide-arithmetic),
  `i64.add128`, `i64.sub128`, `i64.mul_wide_s` and `i64.mul_wide_u`.

Because these are standard WebAssembly, ordinary compilers target the profile. The
protocol rules state the behavior of every accepted instruction in full, and the
dated Core text decides only where they are silent. A WebAssembly feature added later
does not enter Ting because an engine supports it: neither engine defaults nor
dependency upgrades decide what is accepted. The reference implementation decodes,
validates, executes and charges in Ting's own code, with no third-party WebAssembly
code ([conformance](../reference/conformance.md)). Why Ting chose this is on
[A dated WebAssembly subset](../decisions/wasm-profile.md).

## What is accepted

- Integer constants, arithmetic, comparisons, bit operations, shifts and rotations
  on `i32` and `i64`
- 128-bit addition and subtraction, and the full 128-bit product of two `i64`
  values, through the four wide-arithmetic instructions
- Conversions between `i32` and `i64`, and the five sign-extension operators
- Locals and integer globals, mutable or immutable
- Structured control flow (`block`, `loop`, `if`, branches and `return`), direct
  calls, and indirect calls through one table
- Integer loads and stores, `memory.size`, `memory.grow`, `memory.copy` and `memory.fill`

That makes 115 accepted instructions, listed with what each does in the
[instruction reference](instructions.md). Accepted instructions are not enough on
their own: the module as a whole must also pass [validation](#validation).

Indirect calls are what compilers use for function pointers, closures and trait
objects; the table they go through is filled once when the module is instantiated
and can never be changed. Bulk copies let a program move or clear memory at the
byte price of each 32 bytes instead of paying for a loop of loads and stores. Wide
arithmetic lets cryptography multiply 64-bit limbs in one instruction instead of
four half-width products.

## What is rejected

Everything else is rejected: floating point, SIMD, reference-typed values, typed
`select`, table instructions, passive segments, `memory.init` and `data.drop`,
saturating truncation, threads and shared memory, 64-bit memories and custom page
sizes, exceptions, start functions, custom sections (debug information included),
every import other than the twelve host functions, and every other proposal. A
program has no access to an operating system, files, a network or a clock. Debug
information stays a tooling concern: it travels beside a module, never inside the
deployed bytes.

## Module shape

- **Imports** are only functions from module `ting`, each named once, with exactly
  the signatures of the [host interface](host.md#signatures). Memories,
  tables and globals cannot be imported.
- **Exports** are exactly one defined function `invoke`, with no parameters and no
  result, plus the module's memory as `memory` when it has one. Nothing else is
  exported.
- **Memory** is at most one, defined by the module, with an explicit maximum of
  no more than 65,536 pages of 64 KiB (4 GiB, the whole 32-bit address space) and
  a minimum no larger than the maximum.
- **A table**, if present, holds function references, has a fixed size (maximum
  equal to minimum) no larger than the module's function count, imports included,
  and is filled once by active element segments.
- **Globals** are integers initialized by a single constant, and **data segments**
  are active, target memory 0 and must fit in the initial memory.
- **Bounds** on parameters, locals, operand slots and the counts of functions,
  types, globals and segments keep validation proportional to the module's bytes
  ([bounds](#bounds)). They are far above anything a compiler emits.

## One encoding

A module uses the standard WebAssembly binary format in a strict form. Its sections
are type, import, function, table, memory, global, export, element, code and data,
each at most once and in that order; a section with nothing in it is left out, and
custom sections, the start section and the data-count section are rejected. Every
integer is a LEB128 number in its shortest form, and every other construct has one
accepted spelling. `(i32.const 64)` is `41 c0 00`, for example, and no other bytes
mean the same.

So a given module can be written in bytes only one way. Its CodeHash commits to the
bytes as deployed, and no node ever re-encodes or normalizes anything. (The
encoding is unique, not the program: two different programs that compute the same
thing still have different bytes.) A decoder can reject at the first unexpected
byte, and it checks every declared count against the bytes left before allocating
anything, so its work stays proportional to the module's size. The
[exact binary format](../../../ting-protocol-rules.md#17-the-binary-format) gives
every byte, including the accepted
[instruction encodings](../../../ting-protocol-rules.md#174-instruction-encodings).

Linkers add things the profile forbids, such as custom sections and extra exports.
A packing step after linking removes them and encodes the module canonically
([Writing a program](../applications/writing-programs.md)).

## Validation

Decoding is not enough: every function must type-check, including code that never
runs. The validator reads each function body once and keeps a stack of value types
beside a stack of open blocks. Each instruction pops the types it needs and pushes
the types it produces, and a mismatch rejects the module. A branch must carry what
its target expects, the end of a block must leave exactly the block's result, and
code after an unconditional branch is still typed, so a module cannot hide a type
error behind `unreachable`. Every index must exist, `global.set` needs a mutable
global and memory instructions need a memory.
[Section 18 of the protocol rules](../../../ting-protocol-rules.md#18-validation)
states what validation checks, and section 25 gives the type-checking algorithm in
full.

Validity is established once, at deployment. A rejected deployment fails with
status 9, deployment module invalid, after paying its
[code price](../resources/prices.md#code), and installs nothing. Installed code is
therefore always valid, and running it never checks an operand's type. Only the
dynamic [traps](instructions.md#traps) of individual instructions remain, such as
a zero divisor or an indirect call to a table entry of the wrong type.

## Measured for execution

While typing, the validator also records what each function will need at run time:
its parameters plus declared locals, and its **maximum operand height**, the highest
the type stack reaches anywhere in the body, unreachable code included. Their sum is
the function's **reservation**, the number of stack slots it can use, and every
entry to the function pays one C per slot ([function entry](metering.md#function-entry)).
Because the number comes from the validator, never from running the code or from a
compiler's analysis, every node charges the same
([how it is measured](../../../ting-protocol-rules.md#254-maximum-operand-height-and-reservation)).
The validator also resolves every branch to the place it continues at, so the
interpreter never searches for an `end`.

## Bounds

These limits keep a validator's memory and work proportional to the module's bytes.
They are far above what compilers emit and are not capacity limits:

| Bound | Value |
|---|---:|
| Module size | 2^30 bytes |
| Parameters of one type | 1,000 |
| Parameters plus locals of one function | 16,384 |
| Reserved slots of one function (parameters, locals and maximum operand height) | 32,768 |
| Functions, including imports | 1,000,000 |
| Types | 1,000,000 |
| Globals | 1,000,000 |
| Element segments, data segments | 100,000 each |
| Memory maximum | 65,536 pages |

Nesting depth is unbounded, and a single function body is bounded only by the
module size.

## Checking a module

The core's `ting-core inspect program.wasm` validates a file against the profile and
reports its functions, initial memory, globals and table. See
[Writing a program](../applications/writing-programs.md).

## References

- [WebAssembly Core 1.0, W3C Recommendation dated 5 December 2019](https://www.w3.org/TR/2019/REC-wasm-core-1-20191205/)
- [WebAssembly Core Specification, Release 2.0](https://www.w3.org/TR/wasm-core-2/),
  for the three accepted additions
- [The wide-arithmetic proposal](https://github.com/WebAssembly/wide-arithmetic),
  for its four instructions

Later revisions of these texts are not part of Ting's consensus, and Ting claims no
endorsement by their authors.
