# Prices, not caps

## Decision

Anything a counter can measure is priced, never capped, and every compute price is
measured. One C is about the time of one integer addition that waits for the one
before it; each instruction, each byte moved, each page of memory, each hash and
each program started costs what it takes at worst, in those units
([the price list](/wiki/resources/prices)). A record operation costs state accesses
regardless of the value's size, published bytes cost data, allocated bytes cost
growth. Call depth, the local stack, and the number and total size of a program's
records have no fixed limit. The only fixed bounds are those of the encoding, of
32-bit WebAssembly and of a module's shape: every byte string a block carries is
framed with a 32-bit length, a call's memory is at most 4 GiB, so a record value is
at most `2^32 − 2` bytes, and a module is at most 2^30 bytes (1 GiB), with bounded
counts of functions, types, globals and segments.

## Why

- **A cap in a frozen protocol is forever.** Capacity can be voted up, but a
  per-transaction constant cannot. A record size, a memory size or a call depth
  that looks generous at launch becomes the binding constraint the moment
  applications or hardware outgrow it, and no vote can lift it.
- **Prices already bound everything.** Every page of memory costs 40,000 C and is
  never refunded, so live memory never exceeds about 1.6 bytes per unit of the
  compute budget. Every function entry pays for its parameters, locals and operand
  slots, and every 64 KiB the stack grows past its highest point costs what a page
  of memory costs, so the stack is bounded the same way and from the same budget. Every nested invocation reads two
  records before it runs, so depth is bounded by state access. A value must fit the
  memory that holds it. That memory, 4 GiB in 32-bit WebAssembly, is the one size a
  program can meet besides a module's 1 GiB, and only per call, per value and per
  module: storage and calls are unbounded, and hashing a single gigabyte into the
  state costs about 15 billion C.
  Nothing is left for a cap to do.
- **One C must mean the same everywhere.** Capacity is voted for all of C at once,
  so the slowest work per C decides how much compute a block can safely hold. If
  hashing a byte, missing the cache on a load or starting a program cost more time
  per C than an addition, a block made of that work would take longer than its
  capacity allows, and honest programs would have to share a capacity sized for it.
  Measured prices keep every kind of work at about the same time per C, so the
  capacity buys the same time whatever fills it.
- **Measured against a fast engine.** The prices were measured against a compiling
  engine, not against the reference interpreter. Hashing, memory and disks do not
  get faster when software improves; instruction dispatch does. Prices measured
  against a fast engine stay right as implementations improve, and capacity votes
  pass the speed-up on.
- **Measured at the worst.** A price must hold for the program built to be slow:
  the chain of dependent loads, the branch that is never predicted, the hash of a
  record rewritten again and again. Real programs run well inside their prices.
- **State access stays honest.** A large value or a large module costs compute,
  never extra state accesses, so the state-access capacity miners vote measures
  random access and nothing else.

## What stays fixed

- The 32-bit address space of one instance: 65,536 pages, or 4 GiB. A transaction
  composes any number of instances through calls.
- The shape of a module: its size, parameters, locals, operand slots, and the counts
  of functions, types, globals and segments, set far above compiler output so that
  validating a module takes work proportional to its bytes.
- Encoding widths: 32-bit lengths and counts, enough for any item a single node can
  validate.

## Trade-offs

- A node must know how much memory a block can need. It does, from the active
  capacities alone: about 1.6 bytes of guest memory and stack per unit of compute
  capacity,
  and at most twice that with its own copies of nested inputs and code.
- Worst-case prices overcharge ordinary programs: compiled, the reference signature
  checks run in under a fifth of the time their C allows. That is the margin that
  keeps a block of the worst work inside its capacity.
- Prices are measured once, on the hardware of the time, and never change. If
  hardware shifts the balance between instructions, memory and hashing, capacity
  votes can follow the whole of C, but not one price alone.
