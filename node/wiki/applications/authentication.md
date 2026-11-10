# Authentication and replay

The protocol never claims that a transaction is authorized. Anyone can submit any
transaction, and a transaction says nothing about who sent it. Authorization belongs
to the program receiving the call.

## Signed intents

The usual pattern: a user signs a message describing exactly what they want, and the
transaction carries that message and its signature as input. The program verifies the
signature with whichever scheme it chooses, then acts.

A signed intent should bind:

- the **chain**: the program reads the ChainId from context, so the signature cannot
  be replayed on another network;
- the **application**: the executing ContractId, so it cannot be replayed against
  another contract;
- the **action** and its parameters;
- **replay protection**: a sequence number or a unique marker.

The signature does not need to cover the transaction itself. Who relays a transaction
is a separate question from who authorized the action inside it.

A program need not invent this format. [TCR-1](../standards/tcr-1.md) is a standard
form of the signed intent: the program publishes each operation as a sentence with
its fields, a wallet shows that sentence and signs a message that binds all of the
above, and the program checks it. A wallet that follows TCR-1 can then act in the
program without knowing it.

## Signing for a site

The same keys can sign what never reaches the chain: a sign-in, a permission, a
statement. Such a message must not be one any program accepts, and it must name the
site it is for, or one site could use a signature made for another.
[TOCS-1](../standards/tocs-1.md) is the standard for it, and says how the site checks
the answer against the account's keys on chain.

## Replay protection

The same transaction bytes can be included again, and every occurrence runs. If an
action must happen only once, the program records that it happened, for example as an
expected sequence number per signer or a stored marker per used intent.

## Contract callers

When one contract calls another, the callee can read the caller's ContractId from
context. The runtime sets that value, so it can be relied on, and it is how contracts
authorize one another. At top level there is no calling contract.

## Signature verification is code

Verifying a signature is ordinary metered WebAssembly. There are no built-in signature
or hashing functions. A verifier can be a library compiled into the program, or a
separate contract that many programs call. See
[Cryptography lives in programs](../decisions/crypto-in-programs.md).
