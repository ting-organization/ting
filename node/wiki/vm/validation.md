# Validation

A module is validated once, at deployment, and never again: the manifest records
its CodeHash and every node accepts the same bytes. Validation decides the
module's shape ([binary format](/wiki/vm/format)), types every instruction, and
measures what each function will need, so that execution runs without checks
that could fail.

## Typing

Decoding an accepted opcode is not enough: the entire module must pass Core
typing validation, the validation algorithm of the dated WebAssembly Core 1.0
text ([the execution profile](/wiki/vm/profile#defined-meaning)) with only the
MVP integer types, mutable integer globals, the sign-extension operators, the
two bulk memory operations and the four wide-arithmetic instructions enabled.
Specialised to two value types and single block and function results, it works
like this:

- An **operand stack** of types grows and shrinks as instructions are read. An
  instruction pops its operands and pushes its results; a mismatch is an error.
- A **control stack** holds one frame per open `block`, `loop`, `if` or `else`,
  plus one for the function. A frame remembers its result type, the operand
  height when it opened, and whether the code after an unconditional branch is
  unreachable.
- After `unreachable`, `br`, `br_table` or `return`, the rest of the frame is
  **unreachable code**: the operand stack is cut to the frame's height and a pop
  from an empty frame yields an *unknown* type that matches anything. Unreachable
  code is still typed, so a module cannot hide a type error behind `unreachable`.
- `end` closes a frame: the frame's result must be on top and nothing else above
  the frame's height. An `if` without `else` may not declare a result. `else`
  closes the then-frame and opens an else-frame with the same result.
- A branch to a label must provide the label's type: the result type of a
  `block` or `if`, nothing for a `loop`. All labels of a `br_table`, the default
  included, must have the same type, in unreachable code too; Core 1.0 asks
  for exactly that, and the relaxed rule of later WebAssembly releases is not
  part of the profile.
- `select` needs two operands of one type, or one known and one unknown, and an
  i32 condition.
- `call` and `call_indirect` take their parameters from the stack in order and
  push the result, if any; `call_indirect` also needs an i32 index and a table.
- `i64.add128` and `i64.sub128` pop four i64 operands, `i64.mul_wide_s` and
  `i64.mul_wide_u` two, and each pushes two i64 results. They are the only
  instructions with two results; blocks and functions still have at most one.
- Memory instructions need a memory; `global.set` needs a mutable global; every
  local, global, function, type and label index must exist.

## Measured for execution

While typing, the validator records for each function the number of parameters
plus declared locals and the **maximum operand height**, the highest the
validator's operand stack reaches anywhere in the body, unreachable regions
included, with each unknown pushed there counting as one slot. The function's
final `end` pushes its result, if it has one, like any other block's `end`, so a
function with a result reaches a height of at least one even when its body ends
unreachable. Their sum is the function's reservation, charged on every entry
([exact metering](/wiki/vm/metering#function-entry)). It also resolves every branch to the
position it continues at and the stack height it returns to, so the interpreter
never searches for an `end`.

## Bounds

These limits keep a validator's allocation proportional to the module's bytes
and stay below the limits of common engines. They are far above what compilers
emit and are not capacity limits:

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
module size. Every declared count is checked against its section's own remaining
length before an array is allocated for it or a validator that may allocate one
is run.

## Outcome

Validation either accepts the module, yielding its profile (type count, function
reservations, import count, entry function, initial memory, globals, data segments,
data bytes and table size), or rejects it. A rejected deployment fails with status 9,
deployment module invalid, after paying its [code price](/wiki/resources/prices#code),
and installs nothing. The reference decoder and validator is one pass in
`src/vm/decode.rs`, whose work grows with the module's bytes and its declared
functions and segments, never with declared counts that cost no bytes, such as the
locals a function declares with one entry. Its acceptance is checked against the VM
test suite's validation cases and, under the `oracle` feature, against an
independent validator on tens of thousands of mutated modules
([conformance](/wiki/reference/conformance)).
