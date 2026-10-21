# Programs and the VM

A program on Ting, called a **contract**, is immutable WebAssembly code plus the
records it stores. A deployment installs it, and it runs whenever a transaction or
another contract calls it. Every node runs every program and must reach exactly the
same result and the same resource counters, so Ting's VM is defined more tightly
than an ordinary WebAssembly engine: which modules it accepts, what each
instruction does, what each step costs and what a failure undoes are all fixed in
advance. The exact rules are in
[sections 16 to 24 of the protocol rules](../../ting-protocol-rules.md#16-the-execution-profile).

What sets the VM apart:

- It accepts a precise integer subset of WebAssembly, which ordinary compilers can
  target, and rejects everything else at deployment.
- Every instruction has one meaning and a measured price, and every charge is made
  at a fixed point, so every node runs out of budget at the same instruction.
- Every invocation starts from scratch, with fresh memory and fresh globals.
  Anything that should last goes into storage through the host interface.
- Programs cannot reach a filesystem, a network, a clock or a source of randomness.
  Given the same state and input, a program does the same thing on every node.
- The whole machine, from the bytes of a module to the last counter of a receipt,
  is Ting's own code, so no outside engine decides what is accepted or what it costs.

## The pages

- [What a module may contain](vm/profile.md): the accepted subset, the module's
  shape, its one encoding and how it is validated.
- [Instruction reference](vm/instructions.md): the 115 instructions and what each
  does.
- [How a program runs](vm/execution.md): values, the stack, branches, instances and
  how an invocation ends.
- [Deploy and invoke](vm/lifecycle.md): the steps of installing code and of every
  run, with their charges.
- [The host interface](vm/host.md): the twelve functions a program uses to talk to
  the chain.
- [Calls, rollback and read-only](vm/calls.md): nested invocations and what a
  failure undoes.
- [Exact metering](vm/metering.md): when each charge is made, and why the order
  matters.
