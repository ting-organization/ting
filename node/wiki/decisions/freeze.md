# Freeze consensus once

## Decision

Consensus semantics and resource accounting are defined once and then frozen. After
the freeze, applications evolve through program code, implementations improve on
their own, and block capacity changes only through a voting rule that is itself fixed.

## Why

- **Applications rest on the rules.** A program deployed on a frozen protocol keeps
  its meaning and its costs. No later upgrade can reprice it, change an instruction
  under it or remove a feature it uses.
- **Nothing in consensus needs to evolve.** Signatures, assets, fees and account
  models change over time, so none of them is in consensus. What remains (ordering,
  execution, commitments and counting) can be finished.
- **Implementations get a fixed target.** A complete specification with conformance
  vectors lets independent implementations prove they agree, and lets them keep
  getting faster without coordinating with anyone.

## What it takes

Freezing only works if the specification is complete. That includes the exact
accepted instruction bytes, the exact moment every resource unit is charged, the
order in which failures are detected, and portable test vectors for all of it. It
also means choosing the production network's parameters once, from measurement and
simulation, rather than adjusting them afterwards
([production profile](/wiki/consensus/genesis#production-profile)).

## After the freeze

- **Versions stay at 1.** A header's `protocol_version` and a transaction's
  `envelope_version` are always 1, and a decoder rejects anything else. There is no
  upgrade path inside a chain: a different rule set is a different chain, with its
  own genesis and so its own ChainId.
- **History wins.** If the reference node is found to differ from this specification
  after the freeze, the node's behavior stands and the specification is corrected
  to describe it. Accepted blocks may already depend on that behavior, and changing
  it would split the chain.
- **Flaws start a new chain.** A flaw that makes the chain unsafe or unusable is not
  patched in place. It is answered by a new chain with its own genesis, which can
  carry the old chain's state over from a chosen block.

## Trade-offs

- A mistake in a frozen rule cannot be fixed by an upgrade. The cost moves up front,
  into specification and testing.
- Anything that would be cheaper built in, such as native signature verification,
  costs more as a program.
