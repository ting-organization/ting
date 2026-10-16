# Following the chain

Wallets, indexers and dashboards read the chain from outside, usually through a node's
REST API. A few properties of Ting shape how they should do it.

## Receipts and events

Every transaction occurrence has a receipt with its status, the resources it used and
its result bytes. Successful invocations can emit events: the emitting ContractId plus
program-defined bytes, in order. Events suit an indexer that follows the whole chain,
and a client that wants one program's events, or those of one of its topics, asks a
node for just those ([the REST API](/wiki/start/explore#the-rest-api)), at a cost that
grows with the events found, not with the chain. Results suit a client that wants the
answer to one transaction.

## Expect reorganizations

The canonical chain can change when a heavier branch appears. An integration should:

- track block IDs and parent IDs, not only heights;
- notice when the tip moves to a branch that does not extend what it has seen, and
  rewind to the common ancestor;
- treat recent transactions as provisional and decide how many confirmations it needs.

## Trust what you verify

A REST response is what that node has validated. Storage reads include the state root
they were read from, and asked for, each record's
[Merkle proof](/wiki/data/state-tree#proofs): checked against the state root of a
header the client trusts, the value no longer rests on the node's word. Without a
proof, the client trusts the node it asks. Running your own node removes that trust.

## Data kept elsewhere

A program can store a commitment, such as a hash, to a large dataset kept off chain.
The commitment proves what the data was; it does not make the data available.
Applications that rely on external data define their own availability rules. Ting
itself publishes, in every block, all the data needed to validate that block.
