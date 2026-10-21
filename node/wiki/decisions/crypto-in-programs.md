# Cryptography lives in programs

## Decision

SHA-256 is the only cryptography in consensus, used for commitments and proof of work.
No hash or signature function is offered to programs by the host. Programs that need
cryptography implement it in ordinary metered WebAssembly, or call another program
that does.

## Why

- **A frozen protocol would freeze its algorithms.** Signature schemes and hash
  functions weaken over time, and some will need replacing, for example against
  quantum attacks. Built into a frozen protocol, a scheme could never be removed or
  joined by a better one.
- **Programs can migrate.** A new scheme is a deployment. Applications adopt it when
  they choose, several schemes can coexist, and the protocol does not change.
- **No special cases.** Every instruction is charged by the same rules. There is no
  table of precompiled functions with hand-picked prices to maintain.

## Speed without built-ins

Cryptography in programs costs more compute today. The protocol leaves three ways to
improve that without changing any rule:

- **Faster implementations.** A node may compile, cache, or recognize a well-known pure
  function and execute it natively, provided the result, every failure and every
  counter match the reference exactly. The units charged stay the same; validating
  them takes less time.
- **More capacity.** As nodes validate faster, miners can vote compute capacity up,
  without any ceiling.
- **Better programs.** Faster algorithms and libraries can be deployed at any time.

## Trade-offs

Signed transactions use much more compute than they would with a native verifier, so
signature-heavy workloads have lower throughput at a given capacity.
