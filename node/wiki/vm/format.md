# The binary format

A program is a WebAssembly binary module in a strict form: the MVP binary format
with no custom sections, exactly one encoding per construct, and a small fixed set
of imports and exports. A node decodes it in one pass and rejects anything it
cannot account for. The accepted bytes are the deployed bytes: the CodeHash
commits to them, and nothing is ever re-encoded.

## Integers

All counts, indices, sizes and immediates are LEB128 integers, and every one of
them must be minimal at its specified width: 32 bits for every count, index,
length, limit, offset, alignment and `i32.const`, 64 bits for `i64.const`.

- An unsigned integer uses as few bytes as its value needs. A trailing `0x00`
  byte that only pads is rejected, and a final fifth byte may use only its low
  four bits.
- A signed integer, used by constants, has no redundant final byte: a final
  `0x00` after a byte with bit 6 clear, or a final `0x7f` after a byte with bit 6
  set, is rejected, and the unused high bits of a final byte must repeat the sign.

So `(i32.const 64)` is `41 c0 00`, `(i32.const -64)` is `41 40`, and no other
byte string means either. Names, of imports and exports, are a byte length and
the bytes.

## Module layout

A module starts with the eight bytes `00 61 73 6d 01 00 00 00`, the standard
WebAssembly magic and binary version 1. Sections follow, each as an id byte, a
size and the content, and the size must equal the content's length. A section
with nothing in it is left out: one whose vector has no entries is rejected. Only
these sections are accepted, each at most once, in this order:

| Id | Section | Content |
|---:|---|---|
| 1 | type | function types |
| 2 | import | host functions |
| 3 | function | the type of each defined function |
| 4 | table | at most one table |
| 5 | memory | at most one memory |
| 6 | global | integer globals |
| 7 | export | `invoke` and `memory` |
| 9 | element | the table's contents |
| 10 | code | function bodies |
| 11 | data | initial memory contents |

Custom sections (id 0), the start section (8), the data-count section (12) and
anything else are rejected, and nothing may follow the last section. Every vector
count is checked against the bytes left in its section before anything is
allocated for it, so a declared count can never cost more than the module's own
size.

## Sections in detail

**Types.** Each entry is `0x60`, a vector of parameter types and a vector of at
most one result type. The only value types are `0x7f` (i32) and `0x7e` (i64). A
type has at most 1,000 parameters.

**Imports.** Each entry names module `ting`, one of the twelve host functions,
kind `0x00` (function) and a type index whose type is exactly the import's
signature: all-i32 parameters and an i32 result where the
[host interface](/wiki/vm/host#signatures) says so. A name may appear once. No
other kind of import exists: memories, tables and globals are never imported,
and any other module or name is rejected, including the private metering
functions an instrumenting engine adds to its own copy of a module.

**Functions.** A type index per defined function. The function index space is
the imports first, then the defined functions, at most 1,000,000 in all.

**Table.** At most one: element type `0x70` (funcref), limits flag `0x01`, and a
minimum equal to the maximum, no larger than the number of functions, imports
included. It is filled at instantiation and never changes: there are no table
instructions, and a table is never exported or imported.

**Memory.** At most one: limits flag `0x01`, a minimum and a maximum in 64 KiB
pages, the maximum at most 65,536 and the minimum at most the maximum.

**Globals.** Each is a value type, a mutability byte (`0x00` immutable, `0x01`
mutable) and an initializer that is exactly `i32.const n end` or
`i64.const n end` of the global's type.

**Exports.** At most two: `invoke` as a function (kind `0x00`) that is defined,
not imported, and has no parameters and no result; and `memory` as memory 0 (kind
`0x02`), present exactly when the module declares a memory. No other export is
accepted and no name may repeat.

**Elements.** Each segment is the flag `0x00` (active, table 0 implied), the
offset `i32.const n end` and a vector of function indices, which may name imports
as well as defined functions. The segment must fit the table, and a module
without a table has no element segments. Segments apply in order; entries not
listed are null.

**Code.** One body per defined function, in order, each with its size. A body
starts with a vector of local groups, each a count and a value type: every count
is at least 1, and two neighbouring groups never have the same type, so each run of
locals of one type is one group. Parameters plus declared locals are at most
16,384. The instructions follow, up to and
including the function's final `end`, and must use the body's size exactly. The
[instruction reference](/wiki/vm/instructions) gives every accepted encoding.

**Data.** Each segment is the flag `0x00` (active, memory 0 implied), the offset
`i32.const n end` and a vector of bytes. The segment must fit the initial memory, so a module without a
memory has no data segments. Overlapping segments apply in order.

## Instruction encodings

Only these instruction bytes are accepted, 115 instructions in all; the
[instruction reference](/wiki/vm/instructions) defines each one.

| Bytes, inclusive | Instructions |
|---|---|
| `00`–`05` | `unreachable`, `nop`, `block`, `loop`, `if`, `else` |
| `0b`–`11` | `end`, `br`, `br_if`, `br_table`, `return`, `call`, `call_indirect` |
| `1a`–`1b` | `drop`, untyped `select` |
| `20`–`24` | `local.get`, `local.set`, `local.tee`, `global.get`, `global.set` |
| `28`–`29` | `i32.load`, `i64.load` |
| `2c`–`35` | integer narrow loads |
| `36`–`37` | `i32.store`, `i64.store` |
| `3a`–`3e` | integer narrow stores |
| `3f`–`40` | `memory.size`, `memory.grow` |
| `41`–`42` | `i32.const`, `i64.const` |
| `45`–`5a` | integer `eqz` and comparisons |
| `67`–`8a` | integer bit counts, arithmetic, bit operations, shifts, rotations |
| `a7`, `ac`, `ad` | `i32.wrap_i64`, `i64.extend_i32_s`, `i64.extend_i32_u` |
| `c0`–`c4` | `i32.extend8_s`, `i32.extend16_s`, `i64.extend8_s`, `i64.extend16_s`, `i64.extend32_s` |
| `fc 0a`, `fc 0b` | `memory.copy` (memory 0 to memory 0), `memory.fill` (memory 0) |
| `fc 13`–`fc 16` | `i64.add128`, `i64.sub128`, `i64.mul_wide_s`, `i64.mul_wide_u` |

Every other byte is rejected, and so is every other `fc` sub-opcode (saturating
truncation, `memory.init`, `data.drop`, the table operations). Within the
accepted instructions:

- Block types are one byte: `0x40` for no result, `0x7f` or `0x7e` for one.
- Branch depths and local, global, function and type indices are unsigned.
- `br_table` carries a vector of depths followed by the default depth.
- Memory instructions carry an alignment exponent and an offset, both unsigned;
  the alignment may not exceed the access width's natural alignment.
- `memory.size` and `memory.grow` carry one `0x00` byte; `call_indirect` carries
  its type index and then one `0x00` byte, so it always names table 0;
  `memory.copy` is `fc 0a 00 00` and `memory.fill` is `fc 0b 00`.
- The four wide-arithmetic instructions carry no immediates: each is `fc` and its
  sub-opcode, `13` to `16`.
- The `fc` sub-opcode is an unsigned LEB128 and, like every other integer, minimal.

## Why so strict

One encoding per module means one CodeHash per program and nothing a node has
to normalise. The reference decoder, `src/vm/decode.rs`, rejects every other
encoding directly, so the accepted bytes are exactly what a canonical encoder
produces. A decoder that rejects at the first unexpected byte is small,
allocates in proportion to its input and behaves the same everywhere. Linkers add
things the profile forbids, such as custom sections and extra exports; a packing
step after linking removes them and encodes canonically, and `ting-core inspect`
tells you whether a file is deployable ([the execution profile](/wiki/vm/profile)).
