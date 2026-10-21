# Deploy a first program

This walk-through deploys the program from the [tour](../tour.md), mines it into a
block and finds the byte it stored. You need:

- a source checkout of Ting;
- a running development chain with REST enabled, set up as in
  [A development chain](node.md#a-development-chain), so that you can mine
  the block yourself.

## Create the transaction

The repository includes a small example that prints a canonical deployment
transaction as hex:

```sh
cargo run --package ting-node --example storage_transaction --locked
```

It wraps the program in a Deploy transaction with a budget of 1,000,000 C, 128 S,
16,384 D and 16,384 G, and prints one line of hex.

## Submit it

Send the hex to your node:

```sh
curl -X POST http://127.0.0.1:18441/v1/transactions -H 'content-type: application/json' -d '{"hex":"<paste the output here>"}'
```

You can also paste it into "Try it out" on the [API page](/docs). The response
contains the transaction ID. The transaction now waits in the mempool, where the
explorer's **Pending transactions** view shows it.

## Mine it

```sh
ting --data-dir dev mine
```

Search for the transaction ID in the explorer. Its receipt shows:

- status **Success**;
- the C, S, D and G actually used, each well below the budget;
- the result: 32 bytes, the new **ContractId**.

Nothing in the transaction depends on the block it lands in, so the same transaction
produces the same ContractId on any node with the same genesis.

## Read the stored value

Follow the ContractId to the contract page. It shows the code hash and the size of
the installed module. Under **Storage**, look up the all-zero key:

```text
0000000000000000000000000000000000000000000000000000000000000000
```

The record is present with the value `2a`. Any other key is absent. A present
record with an empty value and an absent record are different things in Ting.

## Try variations

- **Submit the same hex again and mine.** It runs as a second occurrence of the same
  transaction, but the ContractId it would create already exists. The receipt
  reports status 10 and the resources used up to that point.
- **Shrink the budget.** Edit the budget in `node/examples/storage_transaction.rs`,
  then create, submit and mine the transaction again. The receipt reports which
  allowance ran out first.

## Check a module before deploying

A module outside the accepted profile fails at deployment and still pays for the
attempt. To check a file locally first, run the core's `ting-core` tool from the
repository root:

```sh
cargo run --locked -- inspect path/to/program.wasm
```

Next, [Writing a program](../applications/writing-programs.md) covers producing
modules with a compiler.
