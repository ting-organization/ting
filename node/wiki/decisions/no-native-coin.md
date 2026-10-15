# No native coin or accounts

## Decision

The protocol has no native currency, external user accounts, signature scheme, fee
asset, account nonce or application address format. Authentication, replay protection
and assets are ordinary program behavior.

## Why

- **No application policy in consensus.** An account format, a fee market or a
  currency encodes choices that applications will want to make differently, and
  revisit over time. Once frozen, a built-in version could never change.
- **No privileged program.** Every asset and every identity scheme is a deployment like
  any other. The chain does not favor one.
- **Generality.** A transaction is just a call or a deployment with a budget. What it
  means is up to the program that receives it.

## Consequences

- **No protocol fee.** Which transactions a miner includes is local policy. Fee
  arrangements, where wanted, are application-defined.
- **No issuance.** The protocol mints nothing and pays no mining reward. Producer data
  lets applications compensate miners on their own terms.
- **Applications define identity.** A wallet works with the applications it supports,
  not with a base-layer account type.

## Trade-offs

The protocol does not answer how miners are paid; programs do. A coin can pay block
rewards and fees to the account a block's producer data names, as the reference
[coin](/wiki/programs/coin) does with
[producer payments](/wiki/programs/conventions#producer-payment), and each miner
values those assets by its own policy. What mining earns therefore depends on
programs and on what their assets are worth, not on a protocol currency.
