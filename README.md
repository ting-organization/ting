# Ting

Ting is a general-purpose proof-of-work blockchain built as a permanent
foundation. Its consensus is a small set of rules, written once and fixed for good:
no developer, miner or foundation can change them, and there are no upgrades to
wait for or fear. Coins, accounts, identities and fees are programs that anyone can
deploy on top, and that others can build on in turn, on rules meant to stay the same
for generations.

Underneath are blocks with SHA-256 commitments, a deterministic integer WebAssembly
VM implemented in this repository, four independent resource counters, per-block
difficulty adjustment and miner capacity voting.

The production network began on 21 October 2026 at 00:00 UTC, with ChainId
`15d6549cfacd816f835c632b4c67308d1c63e642f4cbf4049368430670705107`. Its rules are
frozen: a different rule set is a different chain
([Freeze consensus once](node/wiki/decisions/freeze.md)). `ting init --dev` creates
a development chain to run, explore and build on.

## Documentation

The [protocol rules](ting-protocol-rules.md) and a network's seed blocks define
Ting's consensus, and nothing else does. The rules are one self-contained document,
from the canonical bytes to SHA-256; the seed blocks are the conformance suite every
node replays as the chain's first blocks.

The [wiki](node/wiki/index.md) explains it all for readers, and every node serves it
at `/wiki`. Start with [Why Ting](node/wiki/index.md) and the
[tour](node/wiki/tour.md); the sections on [consensus](node/wiki/consensus.md),
[blocks and state](node/wiki/data.md), [resources](node/wiki/resources.md) and
[the VM](node/wiki/vm.md) explain the rules, and the pages on
[the reference node](node/wiki/node.md) describe how this implementation runs them.
Where the wiki and the protocol rules differ, the rules decide.

## Run a node

The Rust toolchain is pinned in `rust-toolchain.toml`; rustup installs it on the
first build.

```sh
cargo install --path node --locked
ting init --dev --api
ting start
ting mine --count 3
ting status
```

`ting init --dev --api` creates a node for a development chain in `~/.ting`
(`%USERPROFILE%\.ting` on Windows), with REST turned on. The chain starts with its
seed blocks, and `ting mine` adds blocks on top. The node serves a block explorer at
<http://127.0.0.1:18441/>, the wiki at <http://127.0.0.1:18441/wiki> and the REST
API reference at <http://127.0.0.1:18441/docs>. Plain `ting init` creates a node
for the production network instead: it replays the network's seed blocks and finds
its peers through the network's seed nodes. [Start a node](node/wiki/start/node.md)
covers the rest of the command line.

## Repository

| Path | Contents |
|---|---|
| `ting-protocol-rules.md` | The protocol rules: every consensus rule in one document |
| `src/` | `ting-core`: encoding, consensus, state and the VM (decoder, validator, interpreter, host interface) |
| `node/` | `ting-node`: the daemon and the `ting` command, peer protocol, storage, REST API, explorer, and the wiki in `node/wiki/` |
| `tests/` | Core tests; `tests/vm/` is the VM test suite, described in its [README](tests/vm/README.md) |
| `vm-model/` | The VM tests' charging model, independent of the VM |
| `conformance/` | The consensus vectors, one JSON file each, the production and development seeds and a mined voting chain, described in [Conformance and testing](node/wiki/reference/conformance.md) |
| `programs/` | The reference programs: frozen, deployable modules for coins, identities and signature checks, described in the wiki's [Reference programs](node/wiki/programs.md) section; not part of the protocol |

## Tests

```sh
cargo test --workspace --locked
```

It runs the VM tests, whose expectations never come from the VM, and builds from
them the [development seed](conformance/seed), the first blocks every development
chain starts with, and replays the production seed; it works the [consensus vectors](conformance/vectors) out again
from the protocol rules, without the code under test, and checks the reference
against them. [Conformance and testing](node/wiki/reference/conformance.md) explains
how these fit together.

## AI usage

Much of Ting is written with AI coding assistants, working under its maintainer's
direction, and the project is a good fit for that. A consensus meant to be frozen is
not a product that keeps gaining features: the work is to state a small set of rules
exactly, implement them once, and then show case by case that the implementation
follows them. Rules written down exactly make a precise brief, against which both the
code and its tests can be checked. Tests at the depth a frozen consensus needs, every
instruction at its edge operands, every price on both sides of each step, every way a
call can fail, are work people rarely finish by hand; an assistant writes them
readily and, asked to, covers more edges than a person comfortably would. The
consensus core is about 5,500 lines of Rust with more than three times as many lines
of tests, and a coverage gate fails the build if any instruction, host-function outcome,
status or price step goes unexercised.

None of this asks anyone to trust who wrote the code, a person or a model. Every
expected result comes from the written rules and from a model of the charges that
does not use the VM, an expectation is never adjusted to match what the code
produces, and the consensus vectors are worked out again without the code under
test. The suite then becomes part of the chain, as the seed blocks every node
replays, so any implementation, however it was written, is held to the same
evidence. Locked code also plays to the method's strength: generated code is hardest
to keep right where a design keeps shifting, and here the design is fixed, so tests
written once stay valid for as long as the chain runs.

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
