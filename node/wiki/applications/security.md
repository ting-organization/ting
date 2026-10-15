# Security notes

Programs on Ting run in a strict, deterministic environment. Most security
problems come from assuming the protocol does something it deliberately leaves to programs.

## Assume nothing about the sender

- A transaction carries no authenticated sender. Verify signed intents, or check the
  calling ContractId.
- Bind the chain, the contract, the action and a nonce into anything users sign. See
  [Authentication and replay](/wiki/applications/authentication).
- The same transaction can run more than once. Make one-time actions record that they happened.

## Assume nothing about the block

- Timestamps and producer data are chosen by the miner within consensus bounds. Don't
  rely on them for precise timing, and never use them as randomness.
- There is no source of randomness. Anything a miner can see before mining is not
  secret, and a miner can decline to publish a block it dislikes.
- Recent blocks can be replaced by a heavier branch.

## Calls and state

- **Reentrancy is allowed.** A contract you call can call back into you before your
  invocation finishes, and it sees your writes so far. Order your checks and writes
  with that in mind, or guard explicitly.
- **Check child status.** A failed call returns a status; it does not fail the caller.
  Ignoring the status means carrying on as if the child had succeeded.
- **Treat results as input.** Bytes from `call_result` are whatever the callee chose to return.
- **Use read-only calls for queries.** They guarantee that the callee changes no state
  and emits nothing.
- **Storage is isolated but not private.** On chain, only your contract can read or
  write its storage. Off chain, anyone can read every record.

## Resources

- Failure refunds nothing. Work done before a failure is paid by the transaction, so
  check cheap conditions before expensive ones.
- Nothing caps a value's size or a call's depth except budgets. Large values and
  deep composition are fine; they cost what they cost.
- Pass sub-budgets deliberately. A child given everything you have left can spend all of it.

## Code is permanent

Installed code cannot be patched. Plan for mistakes before deploying, for example
with a way to pause, a way to migrate, or a deliberately small surface. See
[Upgrades through programs](/wiki/applications/upgrades).
