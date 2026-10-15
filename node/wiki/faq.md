# Common questions

## Is there a Ting coin?

No. The protocol has no native currency, balance field, fee asset or account type.
An asset is a program that keeps its own records and defines its own transfer rules.
Any number of them can exist side by side, and none is privileged by the chain. The
reference [coin](/wiki/programs/coin) program is one implementation for all of them:
anyone issues a coin by deploying it with their own name, supply and rewards.

## Then why would anyone mine?

The protocol does not issue a mining reward. Each block carries **producer data**
of any size, committed by the header's proof of work, which programs can read from
their execution context. Programs give it meaning: a coin can pay block rewards and
fees to the account a block's producer data names. Producer data says what a miner
wrote, not who mined. Miners are paid in the assets of programs, and value them by
their own policy; the protocol needs no currency for it.

## How do transactions get into blocks without fees?

Which transactions a miner includes is its own policy. The reference node tries
every pending transaction in arrival order and includes each one the block still has
room for. Fee arrangements, where wanted, are defined by applications rather than by
the protocol, and a miner can rank transactions by what they pay it
([mining](/wiki/node/mining#what-a-mining-fork-replaces)).

## Who signed my transaction?

As far as consensus is concerned, nobody. A transaction contains no sender or
signature, and the same bytes can be submitted by anyone. A program that needs
authorization verifies a signed message inside its input, using whichever signature
scheme it chooses. See [Authentication and replay](/wiki/applications/authentication).

## What happens if a signature scheme is broken?

Applications deploy a verifier for a stronger scheme and move to it on their own
schedule. Because no signature scheme is built into consensus, none has to be removed
from it, and the protocol does not change. That includes a future move to
post-quantum signatures.

## Wouldn't built-in cryptography be faster?

Today, yes. But anything built into a frozen protocol stays there forever, weaknesses
included. Ting lets nodes get faster without changing the rules instead: an
implementation may compile, cache, or recognize a well-known pure function and run it
natively, as long as the result, every failure and every resource counter are exactly
what the reference would produce. See
[Cryptography lives in programs](/wiki/decisions/crypto-in-programs).

## Can the same transaction run twice?

Yes. Identical bytes can appear in several blocks, or twice in one block, and each
occurrence executes. The protocol keeps no set of used transaction IDs. Applications
that need one-time execution store a sequence number or a used marker.

## Can I upgrade a deployed program?

Not in place. Installed code is immutable and cannot be replaced or deleted. A new
version is a new deployment, and applications that plan for change keep routing or
migration logic in ordinary state. See [Upgrades through programs](/wiki/applications/upgrades).

## Is there randomness, or a clock?

There is no source of random bytes and no wall clock. Programs can read the height,
the block timestamp and the parent block ID. The timestamp is chosen by the miner
within consensus bounds, and none of these values is unpredictable.

## When is a transaction final?

No fixed number of blocks makes a transaction irreversible by rule. A heavier
competing branch can replace recent history. Applications choose how many
confirmations they need for the value at stake.

## How is this different from Bitcoin or Ethereum?

Like Bitcoin, Ting uses SHA-256 proof of work and follows the branch with the
most work. Like Ethereum, it runs general programs with persistent storage. Unlike
both, it has no native currency or account model, keeps signature schemes out of
consensus, and counts execution with four independent counters rather than a single
combined unit. Its rules are meant to be frozen once, rather than upgraded over time.

## Why WebAssembly, and why only integers?

The VM accepts a small integer subset of WebAssembly Core 1.0, a standard dated
5 December 2019, indirect calls through a fixed table included, plus three additions
from WebAssembly 2.0, the sign-extension operators, `memory.copy` and `memory.fill`,
and the four wide-arithmetic instructions for 128-bit products and sums.
Every instruction's meaning comes from those fixed documents rather than from code
written for Ting, so independent implementations can be checked against the same
definition. Floating point, threads, SIMD and other later WebAssembly features are
not part of the profile. See
[A dated WebAssembly subset](/wiki/decisions/wasm-profile).

## How do I join the network?

Install `ting` and run `ting init` and `ting start`. The node joins the production
network, finds its peers through the network's seed nodes and validates every block
itself; it does not mine unless you start its miner. For experiments and program
development, `ting init --dev` creates a private development chain with an easy
target instead. See [Start a node](/wiki/start/node).
