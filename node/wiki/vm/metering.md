# Exact metering

Every node has to reach exactly the same counters for every transaction, including
the exact point at which a budget runs out, because the counters are part of the
receipts and the block header. So it is not enough to say roughly what an
instruction costs: Ting fixes when each charge is made and what has happened by
then. This page explains when a running program pays compute (C) and why the order
matters; [what each counter measures](../resources/counters.md) lists every counted
event across C, S, D and G, and [the price list](../resources/prices.md) gives the
amounts. The exact rules are in
[section 21 of the protocol rules](../../../ting-protocol-rules.md#21-exact-metering).

## Why the order matters

A budget runs out at one particular charge. Whatever happened before that charge
stays done and charged, and whatever would have come after it never happens. If two
nodes made the same charges in a different order, one might run out before writing a
byte and the other after, and they would disagree about the outcome. So every charge
is made at a fixed point, and every charge is atomic: it fits whole or is refused
whole ([atomic debits](../resources/budgets.md#atomic-debits)). A refused charge ends
the invocation with the status of the exhausted counter, and what it would have paid
for does not happen.

The general rule is to pay first, then act. An instruction pays before it executes,
so an instruction that traps has still paid its price. A copy pays before any byte
moves. A function pays for its stack slots before it is entered, and memory pays for
new pages before they exist.

## When each charge is made

- **Once per transaction**, 1 C of root dispatch, before any program work.
- **For every run of a program**, before its first instruction: reading its manifest
  and code record, the code price unless the transaction has already prepared that
  code, one charge for preparing the instance, and the reservation of `invoke`
  ([invocation](lifecycle.md#invocation)).
- **For every instruction**, its price, immediately before it executes. Skipped
  instructions cost nothing, and `else` and `end` are free. A branch back to a
  `loop` does not pay for the `loop` again, but every instruction of the body pays
  each time it runs. A load or store is priced by the memory's size at that moment.
- **For every function entry**, the function's reservation and any new stack pages,
  after the call instruction's own price ([function entry](#function-entry)).
- **For every byte moved**, after the instruction's price and before the move:
  `memory.copy` and `memory.fill` pay for their bytes before checking their bounds,
  and host functions pay for each copy before it happens ([the host interface](host.md)).
- **For every new page of memory**, 40,000 C, when `memory.grow` adds it
  ([memory growth](#memory-growth)).
- **For every deployment**, the code price and the price of hashing the CodeHash
  before validation, and 32 C with 32 D for the new ContractId once the initializer
  has succeeded ([deployment](lifecycle.md#deployment)).

The full list of charge points is in
[section 21.2 of the protocol rules](../../../ting-protocol-rules.md#212-where-each-charge-is-made).

## Function entry

Entering a defined function, `invoke` included, pays one C per slot of its
[reservation](profile.md#measured-for-execution): its parameters, its declared
locals and its maximum operand height. Each active function also keeps a record of
3 slots on the stack. The first 64 KiB of an invocation's stack, 8,192 slots of 8
bytes, come with its start price. Beyond that, the stack is priced as memory is:
each further 64 KiB that the stack's highest point so far passes costs 40,000 C
([stack](../resources/prices.md#stack)). Returning lowers the stack but not its
highest point, so stack paid for once is reused for free.

The reservation and the stack pages are one charge, made after the call
instruction's own price and, for `call_indirect`, after its table and type checks, so
a bad indirect call pays only its price. Calling a host import reserves nothing: it
pays 94 C for the host call, after an indirect call's own price when it goes through
the table, and then the host function's own charges. There is no cap on frames,
slots or nesting depth; the budget bounds them, as it bounds memory. The exact debit
is in
[section 21.4 of the protocol rules](../../../ting-protocol-rules.md#214-function-entry).

## Memory growth

`memory.grow` pays its price first. If the module's declared maximum forbids the
growth, it returns −1 and nothing more is charged. Otherwise it pays 40,000 C per new
page and then grows; running out of compute at that point fails the invocation
rather than returning −1.

There is no separate memory cap. Every page of memory and of stack, in a running
child and in its suspended parents, has been paid for in compute, so a program can
grow as far as its module declares and its budget pays. At 40,000 C per 64 KiB, a
transaction's live memory and stack together never exceed about 1.6 bytes per C of
its budget, beyond the 64 KiB of stack each invocation's start price covers, so a
node knows from a block's compute capacity how much memory the block can need.

## Counting differently

Implementations may count however they like, as long as outcomes and counters
agree, including the exact instruction at which a budget runs out. The reference
interpreter takes each instruction's price from a local allowance and settles with
the meter before any other charge, so nothing can tell it apart from charging every
instruction through the meter. A compiled implementation may charge a straight run
of instructions at once, provided it still stops at exactly the instruction whose
price does not fit. Fuel tables, timing, timeouts and cache state never define C,
and no inserted or compiled code is ever part of a deployed CodeHash.

## Block limits during execution

A node validating a block may stop as soon as a charge would push the block's totals
past an active capacity ([block totals](../resources/capacity.md#block-totals)). That
rejects the whole block. A transaction's own budget is checked first, so a charge the
transaction could not afford never counts toward the block, and a transaction run on
its own always behaves as these pages describe.
