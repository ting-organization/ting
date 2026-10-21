<div class="hero">
<p class="kicker">The Ting wiki</p>
<h1>A foundation that doesn't move</h1>
<p class="hero-lead">Most programmable blockchains keep changing their own rules. Ting writes a small, simple set of consensus rules once and fixes them for good: no developer, miner or foundation can change them, and every node proves it follows them by replaying a test suite stored in the chain's own first blocks. Everything else is a program that anyone can deploy and others can build on, on rules meant to stay the same for generations.</p>
<div class="hero-actions"><a class="button" href="tour.md">Take the tour</a><a href="start/node.md">Start a node</a><a href="decisions.md">Read the design commitments</a><a href="../../ting-protocol-rules.md">Read the protocol rules</a></div>
</div>

## The problem

On most programmable chains, a new signature scheme, opcode, fee rule or token
standard is a protocol upgrade. Every node has to change together, and each change
can alter what existing applications do or what they cost. Whatever cryptography
is built in stays built in until the next upgrade replaces it.

Over time the base layer collects special cases: built-in accounts, built-in assets,
precompiled functions, repriced instructions. The rules that applications depend
on are never quite settled.

## The approach

Ting's consensus does four things, and nothing else.

<ol class="pillars">
<li><strong>Order</strong><span>Transactions are ordered in blocks secured by SHA-256 proof of work.</span></li>
<li><strong>Execute</strong><span>Programs run deterministically in a small, exactly specified WebAssembly profile.</span></li>
<li><strong>Commit</strong><span>Every block commits to its transactions, their receipts and the resulting state.</span></li>
<li><strong>Count</strong><span>Four independent counters bound the compute, state access, data and storage growth of every transaction.</span></li>
</ol>

Accounts, signatures, coins, fees, replay protection and upgrades are not on that
list. They are programs, deployed and used through ordinary transactions. What is
left is small: two kinds of transaction, 115 instructions, twelve host functions and
four counters, with no accounts, no native coin and no fees anywhere in it.

## What that gives you

<div class="benefits">
<div><strong>Rules that stay put</strong><p>Once the protocol is frozen, every instruction keeps its meaning and its resource charge for as long as the chain runs.</p></div>
<div><strong>Cryptography you can replace</strong><p>A new signature scheme or hash function is deployed as a program. Applications move to it when they choose, without a protocol change.</p></div>
<div><strong>No privileged application</strong><p>There is no native coin, account model or fee asset. The chain does not pick winners among the programs built on it.</p></div>
<div><strong>Costs that don't drift</strong><p>Work is counted in logical units, not prices or nanoseconds. Faster hardware does not change what an operation costs.</p></div>
<div><strong>Capacity without upgrades</strong><p>Miners vote block capacity up or down, slowly, above floors fixed at genesis and with no ceiling. They cannot vote to change the rules.</p></div>
<div><strong>Room for better nodes</strong><p>Implementations may compile, cache or parallelize freely, as long as every result and every counter matches.</p></div>
</div>

## Agreement you can check

Freezing the rules only helps if independent implementations agree on every byte
and every counter. Ting is built so that agreement can be checked, not assumed.

- **The rules in one place.** The [protocol rules](../../ting-protocol-rules.md) state every
  format, instruction, price and failure order in one self-contained document, down
  to SHA-256; with a network's seed blocks they define its consensus completely.
  Nothing is left to an engine's defaults or a library's version.
- **A test suite inside the chain.** Every chain begins with its
  [seed](reference/conformance.md#the-seed): thousands of transactions that run
  every instruction and host function, every price step and every reachable
  outcome, each with the receipt the rules give it. Nodes hold the seed's block ids,
  so a node that syncs has replayed the whole suite. If its own execution ever
  disagrees, the reference node stops and names the first transaction it got wrong.
- **Vectors for the rest.** What a valid chain can never contain, such as malformed
  blocks, is covered by portable
  [consensus vectors](reference/conformance.md#consensus-vectors).
- **Counters, not estimates.** Every receipt records exactly how much compute, state
  access, data and growth its transaction used, so implementations agree on costs
  to the unit, not only on results.

## Four layers, four speeds of change

One idea makes the rest of this wiki easier: every rule belongs to exactly one layer.

| Layer | Who can change it | Examples |
|---|---|---|
| Consensus | Nobody, once frozen | Block and transaction formats, instructions, resource charges, the voting rule |
| Capacity | Miners, by work-weighted vote | Block limits for compute, state access, data and growth |
| Node software | Any implementer, at any time | Database, compiler, networking, mempool policy |
| Applications | Anyone, by deploying programs | Accounts, signatures, assets, fees, upgrades |

> [!IMPORTANT]
> **What defines the consensus.** The [protocol rules](../../ting-protocol-rules.md),
> together with a network's [seed blocks](reference/conformance.md#the-seed), define
> Ting's consensus, and nothing else does. This wiki explains it: what each part
> does, why it is built that way and how to use it. Where the wiki and the protocol
> rules differ, the rules decide and the wiki is corrected. The pages under Running
> a node describe the Rust reference node, which implements the rules and serves
> this wiki.

## Choose your path

<div class="cards">
<a class="card" href="tour.md"><strong>Understand it</strong><span>A short tour, common questions, the glossary and the design commitments.</span></a>
<a class="card" href="start/node.md"><strong>Try it</strong><span>Run a node, explore the chain and deploy a first program on a development chain.</span></a>
<a class="card" href="applications.md"><strong>Build on it</strong><span>What programs can do, and how to write one the network accepts.</span></a>
<a class="card" href="../../ting-protocol-rules.md"><strong>Implement it</strong><span>The protocol rules in one document, and the seed and vectors that check an implementation.</span></a>
</div>
