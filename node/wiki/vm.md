# Programs and the VM

A program on Ting, called a **contract**, is immutable WebAssembly code plus
the records it stores. A deployment installs it, and it runs whenever a transaction
or another contract calls it.

The VM is unusual less for what it can do than for how exactly it is defined:

- It accepts a precise integer subset of WebAssembly Core 1.0, with three additions
  taken from Release 2.0 and the four wide-arithmetic instructions, and rejects
  everything else.
- Each instruction has one exactly defined meaning, written down in this section
  instruction by instruction, and a measured price. The definitions follow the
  dated WebAssembly standard so ordinary compilers target the profile; Ting adds
  rules only for what a program may import, how resources are charged, and how
  calls and failures behave.
- The whole machine, from the bytes of a module to the last counter of a receipt,
  is implemented in Ting's own code and checked against independent evidence.
- Every invocation starts from scratch, with fresh memory and fresh globals. Anything
  that should last goes into storage through the host interface.
- Programs cannot reach a filesystem, a network, a clock or a source of randomness.
  Given the same state and input, a program does the same thing on every node.

The pages of this section define the VM in full: the [profile](/wiki/vm/profile)
and the [binary format](/wiki/vm/format) a module must fit, how it is
[validated](/wiki/vm/validation), what every [instruction](/wiki/vm/instructions)
does, how the [abstract machine](/wiki/vm/execution) runs it, what happens at
[deployment and invocation](/wiki/vm/lifecycle), the [host interface](/wiki/vm/host),
[calls and rollback](/wiki/vm/calls), and [exact metering](/wiki/vm/metering).
Where another page of the wiki describes the VM in passing, these pages decide.
