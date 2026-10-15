# Instruction reference

The 115 accepted instructions, by opcode byte. "Stack" shows what an instruction
pops and pushes, bottom to top, with `[t]` for a value that depends on the label
or function. Every instruction except `else` and `end` costs its price from
[the price list](/wiki/resources/prices) before it executes
([exact metering](/wiki/vm/metering)). Values are `i32` and `i64`. The
definitions are those of the dated WebAssembly Core 1.0 text, plus the
sign-extension operators and the two bulk memory operations of Release 2.0, and the
four instructions of the wide-arithmetic proposal
([the execution profile](/wiki/vm/profile#defined-meaning)); the reference
interpreter in `src/vm/interp.rs` implements them directly. The rules on this
page are normative; where they are silent, the dated texts decide.

## Control

| Byte | Instruction | Immediates | Stack | Meaning |
|---|---|---|---|---|
| `00` | `unreachable` | | | Traps. |
| `01` | `nop` | | | Does nothing. |
| `02` | `block` | block type | | Opens a region; a branch to it continues after its `end`. |
| `03` | `loop` | block type | | Opens a region; a branch to it continues at its first instruction. |
| `04` | `if` | block type | `i32 →` | Opens a region and runs the then-branch for a non-zero condition, otherwise the else-branch or nothing. |
| `05` | `else` | | | Separates the branches of an `if`; free. |
| `0b` | `end` | | | Closes a region, or the function; free. |
| `0c` | `br` | depth | `[t] →` | Branches to the depth-th enclosing label, carrying its value if it has one. |
| `0d` | `br_if` | depth | `[t] i32 → [t]` | Branches when the condition is non-zero; otherwise leaves the value in place. |
| `0e` | `br_table` | depths, default | `[t] i32 →` | Branches to the label at the unsigned index, or to the default when the index is out of range. |
| `0f` | `return` | | `[t] →` | Leaves the function with its result, if any. |
| `10` | `call` | function | `params → [result]` | Calls a function; a host import runs its host rule. |
| `11` | `call_indirect` | type, `00` | `params i32 → [result]` | Calls the entry of table 0 at the unsigned index; traps on an index outside the table, a null entry, or an entry whose type differs structurally (in its parameter and result types, not its type index) from the expected type. Otherwise exactly like `call`. |

## Parametric and variables

| Byte | Instruction | Immediates | Stack | Meaning |
|---|---|---|---|---|
| `1a` | `drop` | | `t →` | Discards a value. |
| `1b` | `select` | | `t t i32 → t` | Keeps the first value when the condition is non-zero, the second otherwise; all three are evaluated. |
| `20` | `local.get` | local | `→ t` | Reads a parameter or local. |
| `21` | `local.set` | local | `t →` | Writes a parameter or local. |
| `22` | `local.tee` | local | `t → t` | Writes and keeps the value. |
| `23` | `global.get` | global | `→ t` | Reads a global. |
| `24` | `global.set` | global | `t →` | Writes a mutable global. |

## Memory

Memory is a byte array of 64 KiB pages. Each access computes its effective
address as the unsigned 32-bit address plus the unsigned 32-bit offset, in
unbounded arithmetic, and traps when the address plus the access width exceeds
the current memory size: an address never wraps around into valid memory, and a
store that traps writes nothing. Bytes are little-endian. Narrow signed loads
sign-extend, narrow unsigned loads zero-extend, and narrow stores keep the low
bytes. Alignment immediates are hints and never change behaviour.

| Byte | Instruction | Stack | Meaning |
|---|---|---|---|
| `28` | `i32.load` | `i32 → i32` | 4 bytes. |
| `29` | `i64.load` | `i32 → i64` | 8 bytes. |
| `2c` | `i32.load8_s` | `i32 → i32` | 1 byte, sign-extended. |
| `2d` | `i32.load8_u` | `i32 → i32` | 1 byte, zero-extended. |
| `2e` | `i32.load16_s` | `i32 → i32` | 2 bytes, sign-extended. |
| `2f` | `i32.load16_u` | `i32 → i32` | 2 bytes, zero-extended. |
| `30` | `i64.load8_s` | `i32 → i64` | 1 byte, sign-extended. |
| `31` | `i64.load8_u` | `i32 → i64` | 1 byte, zero-extended. |
| `32` | `i64.load16_s` | `i32 → i64` | 2 bytes, sign-extended. |
| `33` | `i64.load16_u` | `i32 → i64` | 2 bytes, zero-extended. |
| `34` | `i64.load32_s` | `i32 → i64` | 4 bytes, sign-extended. |
| `35` | `i64.load32_u` | `i32 → i64` | 4 bytes, zero-extended. |
| `36` | `i32.store` | `i32 i32 →` | 4 bytes. |
| `37` | `i64.store` | `i32 i64 →` | 8 bytes. |
| `3a` | `i32.store8` | `i32 i32 →` | The low byte. |
| `3b` | `i32.store16` | `i32 i32 →` | The low 2 bytes. |
| `3c` | `i64.store8` | `i32 i64 →` | The low byte. |
| `3d` | `i64.store16` | `i32 i64 →` | The low 2 bytes. |
| `3e` | `i64.store32` | `i32 i64 →` | The low 4 bytes. |
| `3f` | `memory.size` | `→ i32` | The current size in pages. |
| `40` | `memory.grow` | `i32 → i32` | Appends that many zeroed pages and yields the previous size, or yields −1 with no effect when the new size would exceed the declared maximum. Growth by zero pages succeeds and yields the current size. Charges 40,000 C per new page; running out of compute there fails the invocation rather than yielding −1. |
| `fc 0a` | `memory.copy` | `i32 i32 i32 →` | Destination, source, length `n`: moves `n` bytes as if the source were first copied aside, so overlapping ranges move correctly. Charges `bytes(n)`, then traps before writing anything if any part of either range lies outside memory. |
| `fc 0b` | `memory.fill` | `i32 i32 i32 →` | Destination, value, length `n`: writes the low byte of the value `n` times. Same charging and bounds rule. |

A zero-length copy or fill whose addresses are at or below the end of memory is
in bounds.

## Constants, tests and comparisons

| Byte | Instruction | Immediates | Stack |
|---|---|---|---|
| `41` | `i32.const` | signed LEB128 | `→ i32` |
| `42` | `i64.const` | signed LEB128 | `→ i64` |
| `45` | `i32.eqz` | | `i32 → i32` |
| `46`–`4f` | `i32.eq`, `ne`, `lt_s`, `lt_u`, `gt_s`, `gt_u`, `le_s`, `le_u`, `ge_s`, `ge_u` | | `i32 i32 → i32` |
| `50` | `i64.eqz` | | `i64 → i32` |
| `51`–`5a` | `i64.eq`, `ne`, `lt_s`, `lt_u`, `gt_s`, `gt_u`, `le_s`, `le_u`, `ge_s`, `ge_u` | | `i64 i64 → i32` |

Results are 1 or 0. `_s` compares two's-complement signed values, `_u` unsigned.
`eqz` tests for zero.

## Arithmetic and bits

| i32 | i64 | Instruction | Meaning |
|---|---|---|---|
| `67` | `79` | `clz` | Leading zero bits; the width for zero. |
| `68` | `7a` | `ctz` | Trailing zero bits; the width for zero. |
| `69` | `7b` | `popcnt` | Set bits. |
| `6a` | `7c` | `add` | Wrapping. |
| `6b` | `7d` | `sub` | Wrapping. |
| `6c` | `7e` | `mul` | Wrapping, the low bits of the product. |
| `6d` | `7f` | `div_s` | Signed, truncating toward zero; traps on a zero divisor and on the minimum value divided by −1. |
| `6e` | `80` | `div_u` | Unsigned; traps on a zero divisor. |
| `6f` | `81` | `rem_s` | Signed remainder with the dividend's sign; traps on a zero divisor; the minimum value by −1 is 0. |
| `70` | `82` | `rem_u` | Unsigned; traps on a zero divisor. |
| `71` | `83` | `and` | Bitwise. |
| `72` | `84` | `or` | Bitwise. |
| `73` | `85` | `xor` | Bitwise. |
| `74` | `86` | `shl` | Shift left by the count modulo the width. |
| `75` | `87` | `shr_s` | Arithmetic shift right by the count modulo the width. |
| `76` | `88` | `shr_u` | Logical shift right by the count modulo the width. |
| `77` | `89` | `rotl` | Rotate left by the count modulo the width. |
| `78` | `8a` | `rotr` | Rotate right by the count modulo the width. |

Binary operators take the left operand first: `a b sub` is a − b, and the shift or
rotation count is the second operand. Shifts and rotations use only the low 5 bits
of the count for `i32` and the low 6 bits for `i64`. Unsigned division and
remainder are the ordinary unsigned operations.

## Conversions

| Byte | Instruction | Stack | Meaning |
|---|---|---|---|
| `a7` | `i32.wrap_i64` | `i64 → i32` | The low 32 bits. |
| `ac` | `i64.extend_i32_s` | `i32 → i64` | Sign-extended. |
| `ad` | `i64.extend_i32_u` | `i32 → i64` | Zero-extended. |
| `c0` | `i32.extend8_s` | `i32 → i32` | The low 8 bits, sign-extended. |
| `c1` | `i32.extend16_s` | `i32 → i32` | The low 16 bits, sign-extended. |
| `c2` | `i64.extend8_s` | `i64 → i64` | The low 8 bits, sign-extended. |
| `c3` | `i64.extend16_s` | `i64 → i64` | The low 16 bits, sign-extended. |
| `c4` | `i64.extend32_s` | `i64 → i64` | The low 32 bits, sign-extended. |

## Wide arithmetic

Four instructions compute with 128-bit integers held as two `i64` values, the low
half below the high half on the stack. They are the four instructions of the
WebAssembly wide-arithmetic proposal, with its opcodes and meaning.

| Bytes | Instruction | Stack | Meaning |
|---|---|---|---|
| `fc 13` | `i64.add128` | `i64 i64 i64 i64 → i64 i64` | Left low, left high, right low, right high: the low and high halves of the sum modulo 2^128. |
| `fc 14` | `i64.sub128` | `i64 i64 i64 i64 → i64 i64` | The same operands: the halves of left minus right modulo 2^128. |
| `fc 15` | `i64.mul_wide_s` | `i64 i64 → i64 i64` | The low and high halves of the full 128-bit product of two signed values. |
| `fc 16` | `i64.mul_wide_u` | `i64 i64 → i64 i64` | The same for two unsigned values. |

None of them traps. Each pushes two values, so it counts twice toward a function's
maximum operand height.

## Traps

A trap is a deterministic guest outcome: it ends the invocation with status
2 and no result, after the instruction's own dispatch charge and any byte
charge it made. The causes are
`unreachable`, a zero divisor, the signed-division overflow, an out-of-bounds
memory access, a bulk operation out of bounds, and a bad indirect call. Host
rules add their own causes ([host interface](/wiki/vm/host)). All traps are the
same status; nothing distinguishes them in a receipt.

## Everything else

No other opcode byte and no other `fc` sub-opcode is accepted. That excludes
floating point, SIMD, typed `select`, reference types, table instructions,
`memory.init` and `data.drop`, saturating truncation, exceptions, threads and
every other proposal.
