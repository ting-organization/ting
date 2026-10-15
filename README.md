# Ting

Ting is a general-purpose proof-of-work blockchain whose consensus is small
enough to be frozen. It consists of blocks with SHA-256 commitments, a
deterministic integer WebAssembly VM implemented in this repository, four
independent resource counters, per-block difficulty adjustment and miner
capacity voting. Assets, accounts, fees and authentication are deployed
programs, not protocol features.

The production network launched on 13 October 2026. Its genesis is built into the
node, so `ting init` creates a production node, which finds its peers through the
network's seed nodes. Its rules are frozen: a different rule set is a different
chain ([Freeze consensus once](node/wiki/decisions/freeze.md)).

## Documentation

The [wiki](node/wiki/index.md) is the documentation and the specification. Every
node serves it at `/wiki`. Start with [Why Ting](node/wiki/index.md) and the
[tour](node/wiki/tour.md); the exact rules are in the sections on
[consensus](node/wiki/consensus.md), [blocks and state](node/wiki/data.md),
[resources](node/wiki/resources.md), [the VM](node/wiki/vm.md) and
[the reference node](node/wiki/node.md).

## Run a node

The Rust toolchain is pinned in `rust-toolchain.toml`; rustup installs it on the
first build.

```sh
cargo install --path node --locked
ting init
ting start
ting status
```

`ting init` creates a node for the production network in `~/.ting`
(`%USERPROFILE%\.ting` on Windows); it finds its peers through the network's seed
nodes. `ting api enable` turns on REST, and the node then serves a block explorer at
<http://127.0.0.1:18441/>, the wiki at <http://127.0.0.1:18441/wiki> and the REST
API reference at <http://127.0.0.1:18441/docs>. [Start a node](node/wiki/start/node.md)
covers the rest of the command line, and the development chain that
`ting init --dev` creates for experiments and program development.

## Repository

| Path | Contents |
|---|---|
| `src/` | `ting-core`: encoding, consensus, state and the VM (decoder, validator, interpreter, host interface) |
| `node/` | `ting-node`: the daemon and the `ting` command, peer protocol, storage, REST API, explorer, and the wiki in `node/wiki/` |
| `tests/` | Core tests; `tests/vm/` is the VM test suite, described in its [README](tests/vm/README.md) |
| `vm-model/` | The VM tests' charging model, independent of the VM |
| `programs/` | The reference programs: frozen, deployable modules for coins, identities and signature checks, described in the wiki's [Reference programs](node/wiki/programs.md) section; not part of the protocol |

## Tests

```sh
cargo test --workspace --locked
cargo test --package ting-core --features oracle --locked
```

The first command includes a test that works out the published
[protocol vectors](node/wiki/reference/protocol-vectors.json) again from the
specification, without the code under test. The second also runs every VM case
on a pinned independent WebAssembly engine, which is never part of the consensus
build. [Conformance and testing](node/wiki/reference/conformance.md) explains how
these fit together.

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
