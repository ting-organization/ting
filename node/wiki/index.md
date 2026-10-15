<div class="hero">
<p class="kicker">The Ting wiki</p>
<h1>A blockchain designed to be finished</h1>
<p class="hero-lead">Most programmable blockchains keep changing their own rules. Ting writes a small set of consensus rules once, pins them down with independent test vectors, and then freezes them. Everything else is an ordinary program that anyone can deploy.</p>
<div class="hero-actions"><a class="button" href="/wiki/tour">Take the tour</a><a href="/wiki/start/node">Start a node</a><a href="/wiki/decisions">Read the design commitments</a></div>
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
list. They are programs, deployed and used through ordinary transactions.

## What that gives you

<div class="benefits">
<div><strong>Rules that stay put</strong><p>Once the protocol is frozen, every instruction keeps its meaning and its resource charge for as long as the chain runs.</p></div>
<div><strong>Cryptography you can replace</strong><p>A new signature scheme or hash function is deployed as a program. Applications move to it when they choose, without a protocol change.</p></div>
<div><strong>No privileged application</strong><p>There is no native coin, account model or fee asset. The chain does not pick winners among the programs built on it.</p></div>
<div><strong>Costs that don't drift</strong><p>Work is counted in logical units, not prices or nanoseconds. Faster hardware does not change what an operation costs.</p></div>
<div><strong>Capacity without upgrades</strong><p>Miners vote block capacity up or down, slowly, above floors fixed at genesis and with no ceiling. They cannot vote to change the rules.</p></div>
<div><strong>Room for better nodes</strong><p>Implementations may compile, cache or parallelize freely, as long as every result and every counter matches.</p></div>
</div>

## Four layers, four speeds of change

One idea makes the rest of this wiki easier: every rule belongs to exactly one layer.

| Layer | Who can change it | Examples |
|---|---|---|
| Consensus | Nobody, once frozen | Block and transaction formats, instructions, resource charges, the voting rule |
| Capacity | Miners, by work-weighted vote | Block limits for compute, state access, data and growth |
| Node software | Any implementer, at any time | Database, compiler, networking, mempool policy |
| Applications | Anyone, by deploying programs | Accounts, signatures, assets, fees, upgrades |

> [!IMPORTANT]
> **This wiki is the specification.** Its concept pages state the consensus rules
> exactly; where a description elsewhere differs from an explicit rule on the
> [VM pages](/wiki/vm), the VM rule governs. *Must*, *must not* and *should* state
> requirements. The pages under Running a node describe the Rust reference node,
> which implements the rules and serves this wiki, not consensus.

## Choose your path

<div class="cards">
<a class="card" href="/wiki/tour"><strong>Understand it</strong><span>A short tour, common questions and the glossary.</span></a>
<a class="card" href="/wiki/start/node"><strong>Try it</strong><span>Run a node, explore the chain and deploy a first program on a development chain.</span></a>
<a class="card" href="/wiki/applications"><strong>Build on it</strong><span>What programs can do, and how to write one the network accepts.</span></a>
<a class="card" href="/wiki/decisions"><strong>Review it</strong><span>The design commitments and the conformance suite.</span></a>
</div>
