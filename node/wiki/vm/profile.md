# The execution profile

A deployed module must fit an exact profile. Anything outside it is rejected at
deployment; no node ever has to interpret it differently from another.

## Defined meaning

The meaning of every accepted instruction is written down in this section of the
wiki, instruction by instruction in the [instruction reference](/wiki/vm/instructions),
including the details where implementations most often disagree: integer
wrap-around, signed division and its traps, shift counts, effective addresses
that cannot wrap, sign extension on narrow loads, and branch targets.

The definitions are those of the integer subset of the
[WebAssembly Core 1.0 Recommendation of 5 December 2019](https://www.w3.org/TR/2019/REC-wasm-core-1-20191205/),
plus three additions taken from the
[WebAssembly Core Specification, Release 2.0](https://www.w3.org/TR/wasm-core-2/):
the sign-extension operators, `memory.copy` and `memory.fill`, and nothing else
of that release; and the four instructions of the
[wide-arithmetic proposal](https://github.com/WebAssembly/wide-arithmetic),
`i64.add128`, `i64.sub128`, `i64.mul_wide_s` and `i64.mul_wide_u`, whose meaning
the [instruction reference](/wiki/vm/instructions#wide-arithmetic) fixes in full.
Indirect calls, the table and element segments are Core 1.0 features. Ordinary
compilers therefore target the profile, and the official test suites apply to it.

The dated texts are the source of the definitions and the reference the
implementation is checked against. They are not a moving feature set: a
WebAssembly feature added later does not enter Ting because an engine supports
it, and neither engine defaults nor dependency upgrades decide what is accepted.
The rules on these pages are normative; where they are silent, the dated Core
text decides.

The reference implementation is Ting's own: it decodes, validates, executes,
charges and runs the host interface in a few thousand lines of safe Rust, with
no third-party WebAssembly code in the consensus build. A pinned independent
engine and validator are compiled in only behind the test-only `oracle` cargo
feature, to check the reference against on identical inputs
([conformance](/wiki/reference/conformance)).

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

That makes 115 accepted opcodes, listed with their exact behaviour in the
[instruction reference](/wiki/vm/instructions) and as byte ranges under
[the binary format](/wiki/vm/format#instruction-encodings). Parsing is not
enough: the module as a whole must also pass [validation](/wiki/vm/validation)
with only these features enabled.

Indirect calls are what compilers use for function pointers, closures and trait
objects; the table they go through is filled once when the module is instantiated
and can never be changed. Bulk copies let a program move or clear memory at the
byte price of each 32 bytes instead of paying for a loop of loads and stores. Wide
arithmetic lets cryptography multiply 64-bit limbs in one instruction instead of
four half-width products.

## What is rejected

Floating point, SIMD, reference-typed values, typed `select`, table instructions,
passive segments, `memory.init` and `data.drop`, saturating truncation, threads and
shared memory, 64-bit memories and custom page sizes, exceptions, start functions,
custom sections (debug information included), every import other than the twelve
host functions, and every other proposal. A program has no access to an operating
system, files, a network or a clock. Debug information stays a tooling concern:
it travels beside a module, never inside the deployed bytes.

## Module shape

- **Imports** are only functions from module `ting`, each named once, with exactly
  the signatures of the [host interface](/wiki/vm/host#signatures). Memories,
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
  types, globals and segments exist so that validation takes work proportional to
  the module's bytes ([validation](/wiki/vm/validation#bounds)). They are far above
  anything a compiler emits.

The bytes must also be the canonical encoding: every integer minimal and every
construct in its one accepted form ([the binary format](/wiki/vm/format)). A
decoder rejects anything else at the first unexpected byte, and the CodeHash
commits to the original bytes.

## Checking a module

The core's `ting-core inspect program.wasm` validates a file against the profile and
reports its functions, initial memory, globals and table. See
[Writing a program](/wiki/applications/writing-programs).

## References

- [WebAssembly Core 1.0, W3C Recommendation dated 5 December 2019](https://www.w3.org/TR/2019/REC-wasm-core-1-20191205/)
- [WebAssembly Core Specification, Release 2.0](https://www.w3.org/TR/wasm-core-2/),
  for the three accepted additions
- [The wide-arithmetic proposal](https://github.com/WebAssembly/wide-arithmetic),
  for its four instructions
- [WebAssembly execution specification](https://webassembly.github.io/spec/core/exec/instructions.html),
  for reviewing where Ting fixes a behaviour deterministically

These texts inform the definitions; later revisions of them are not incorporated
into Ting's consensus, and Ting claims no endorsement by their authors.
