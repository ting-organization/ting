# Coin

> [!NOTE]
> A reference program, not part of the protocol: the chain runs it like any other
> deployment. See [Reference programs](/wiki/programs).

One program, the coin program, implements every coin. Anyone issues a coin by
deploying it with their own options, and each coin is its own deployment, known by
its [ContractId](/wiki/data/records#identifiers). Coins differ only in their
deployment input: their rules are the same, can be audited once and are recognized
by the program's CodeHash ([Reference programs](/wiki/programs)). A coin has no
administrator and no upgrade. Its options are fixed at deployment, except the
minting authority, which can only be handed on or given up.

The coin builds on the [shared conventions](/wiki/programs/conventions): their
accounts, the policy interface and delayed authorization, producer data and events.
Its application tag is `app/coin-v1`. Time-locked savings accounts come from
[identities](/wiki/programs/identity), which declare a delay in their policy result;
the coin schedules their actions accordingly.

In the formats below, integers are little-endian and fixed width, and `Bytes(x)`, `‖`
and `H` are the encoding, concatenation and hash of the
[conventions](/wiki/programs/conventions#encoding); `account(32)` is a 32-byte value.
The errors named on this page are the exact ASCII result bytes the coin reverts with.

## Options

A coin's options are chosen in its [deployment input](#deployment-input):

| Option | Meaning |
|---|---|
| `name` | Non-empty UTF-8, for display |
| `symbol` | Non-empty UTF-8, for display |
| `decimals` | Where the decimal point is displayed (`u8`). Amounts on chain are integers in base units. |
| `max_supply` | Hard cap on all coins ever issued; `2^128 − 1` means no cap. Burned coins are never issued again. |
| `initial_reward` | Block reward in the first era; 0 disables mining rewards |
| `halving_interval` | Blocks per era; the reward halves each era. 0 means a constant reward. |
| `tail_reward` | Least reward after the halvings, a perpetual tail emission; 0 for none. Must not exceed `initial_reward`. |
| `minter` | The account allowed to mint, or 32 zero bytes for none |
| `allocations` | Any number of `(account, amount)` pairs credited at deployment |

Amounts are `u128` base units, up to about 3.4 × 10^38: enough for 10^20 whole coins
at 18 decimals. Like every quantity the coin keeps, an amount is fixed width so that
encodings stay exact. The coin imposes no other limit on sizes or counts.

Every issuance, whether an allocation, a reward or a mint, counts toward
`max_supply`.

### Supply models

The options combine into the usual supply models, with `r` a reward, `n` a number of
blocks and `t` a tail reward:

| Model | Options |
|---|---|
| Fixed supply | Allocations only, `initial_reward = 0`, no minter |
| Bitcoin-like | `initial_reward = r`, `halving_interval = n`, `max_supply = 2·r·n`, no allocations |
| Mined with a tail | `initial_reward = r`, `halving_interval = n`, `tail_reward = t`, no cap |
| Issuer-managed | A minter, an optional cap; the minter may renounce later |
| Mixed | Allocations plus mining, under one cap |

## Records

A coin keeps these records in its own storage:

| Key | Value |
|---|---|
| `00…00` | The configuration ([initialization](#initialization)), never changed |
| `00…01` | The state: `issued u128 ‖ burned u128 ‖ last_claimed u64 ‖ minter(32)`, 72 bytes |
| An account | `balance u128 ‖ locked u128 ‖ nonce u64`, 40 bytes; an absent record means all zero |
| `H("app/coin-v1/scheduled", account ‖ u64 sequence)` | A [scheduled action](#scheduling-an-action) |

`00…00` is 32 zero bytes and `00…01` is 31 zero bytes followed by `01`. An account's
record is kept under the 32-byte account itself.

`locked` is the part of `balance` held for the account's scheduled actions, and
`available = balance − locked`. The circulating supply is `issued − burned`.

The two reserved keys `00…00` and `00…01` can never be the target of a transfer, a
mint or an allocation, nor a payout account. The zero key also means "none" for the
minter, so only `00…01` is forbidden there.

Account records and scheduled-action records share the coin's storage. A
scheduled-action key is a hash that no account can have, so a payment to it reaches
no one: while the record exists, a transfer, mint or payout to that key reverts
`corrupt`, and a payment made before it exists is lost when the record is written,
like any payment to an account nobody controls. An account record that a payment
leaves under the key of an action that has run or been cancelled is never taken for
a scheduled action: running, querying and cancelling treat it as not scheduled.

The records are public. Any client reads them directly through any node's storage
endpoint, with the coin's ContractId and the record's key
([Explore the chain](/wiki/start/explore#the-rest-api)).

## Operations

The first input byte selects the operation.

| Op | Input after the op byte | Effect |
|---|---|---|
| `01` balance | `account(32)` | Returns the account's 40-byte record |
| `02` info | none | Returns the configuration record followed by the state record |
| `03` claim | none | Mints the block reward to the producer's payout account and returns the reward as a `u128` ([mining rewards](#mining-rewards)) |
| `04` run | `account(32) ‖ u64 sequence` | Executes a due scheduled action; anyone may call ([running](#running-a-scheduled-action)) |
| `05` scheduled | `account(32) ‖ u64 sequence` | Returns the scheduled-action record, or reverts `not scheduled` |
| `10` execute | `auth ‖ action` | An [authorized action](#authorized-actions) |
| `11` cancel | `cancel_auth ‖ cancel` | Cancels scheduled actions of the account ([cancelling](#cancelling)) |

The queries `01`, `02` and `05` are safe in [read-only](/wiki/vm/calls#read-only-mode)
calls. Any other first byte, or input that does not decode exactly, reverts
`bad input`. A stored record the coin cannot decode reverts `corrupt`. The coin's own
writes never cause one, but a key whose record is not an account record reverts
`corrupt` when it is read as an account: a reserved key in a balance query, and a
pending scheduled action's key in a balance query or as the target of a payment. A
transfer or mint to a reserved key reverts `reserved account` before that, and a
reserved payout account counts as none ([fees and tips](#fees-and-tips)).

## Deployment

A coin is created by deploying the coin program with the input below, whether by a
deploy transaction or from another program
([deploy and invoke](/wiki/vm/lifecycle#deployment)). Unlike the other reference
programs it has no [canonical deployment](/wiki/programs#canonical-deployment): every
coin is a deployment of its own, and empty input reverts `bad deploy input`.

### Deployment input

```text
u8 version = 1
Bytes(name)  Bytes(symbol)  u8 decimals
u128 max_supply  u128 initial_reward  u64 halving_interval  u128 tail_reward
minter(32)
u64 allocation_count  allocation_count × (account(32) ‖ amount u128)
```

The deployment fails, reverting `bad deploy input`, when:

- the encoding is not exact, or the version is not 1;
- the name or the symbol is empty or not UTF-8;
- `tail_reward > initial_reward`;
- an allocation names a reserved key;
- the minter is the state key `00…01`;
- the allocations together exceed `max_supply`.

A zero minter means none. The deployment's cost grows with the number of
allocations, which is the deployer's choice.

### Initialization

The deployment's first invocation stores the configuration record

```text
u8 version ‖ Bytes(name) ‖ Bytes(symbol) ‖ u8 decimals ‖ u128 max_supply ‖
u128 initial_reward ‖ u64 halving_interval ‖ u128 tail_reward ‖ u64 deploy_height
```

where `deploy_height` is the height of the block that deploys the coin. It credits
each allocation, an account named more than once receiving every amount, and emits
a Transfer event from the zero account for each. It then stores the state with
`issued` equal to the allocated total, `burned = 0`, `last_claimed = deploy_height`
and the minter.

## Authorized actions

Everything an account does with its coins is an authorized action, operation `10`:
an authorization followed by an action. Cancelling scheduled actions is operation
`11`, with an authorization of its own ([cancelling](#cancelling)).

### Authorization

```text
auth        = 00 ‖ policy(32) ‖ Bytes(params) ‖ u64 nonce ‖ Bytes(witness)   policy account
            | 01                                                             contract account
cancel_auth = 00 ‖ policy(32) ‖ Bytes(params) ‖ Bytes(witness)               policy account
            | 01                                                             contract account
```

The two kinds are the policy and contract accounts of the
[conventions](/wiki/programs/conventions#accounts). A cancel's authorization is the
same without the nonce.

- **Policy account.** The acting account is
  `H("app/account-v1/policy", policy ‖ Bytes(params))`, where `policy` is the
  ContractId of a policy program. For an action, `nonce` must equal the account's
  stored nonce. The coin calls the policy read-only with the input
  `Bytes(params) ‖ Bytes(message) ‖ Bytes(witness)` and the child budget of the
  [conventions](/wiki/programs/conventions#the-budget-reserve): its own remaining
  budget minus the standard reserve R. The result `01` means a required delay of 0,
  and `01 ‖ u64 d` with `d > 0` a required delay of `d` seconds, as the
  [policy interface](/wiki/programs/conventions#policy-interface) defines these
  results; anything else is `not authorized`. Whoever submits the transaction bounds
  the policy's cost through the transaction's [budget](/wiki/resources/budgets).
- **Contract account.** The acting account is
  `H("app/account-v1/contract", caller)`, where `caller` is the calling ContractId
  from the [execution context](/wiki/vm/host#context). A top-level call has no caller
  and reverts `no calling contract`. The required delay is 0 and the nonce is not
  checked: the owning program enforces its own rules.

The message a policy account authorizes is

```text
message = Bytes("app/coin-v1") ‖ ChainId ‖ coin ContractId ‖ account ‖ body
body    = 10 ‖ u64 nonce ‖ action        an action
        | 11 ‖ cancel                    a cancel
```

with the ChainId and the coin's own ContractId taken from the execution context, and
`action` or `cancel` the complete encoding after the authorization. The message names
the acting account, so a signature for one account of a key is never valid for
another ([signed messages](/wiki/programs/conventions#signed-messages)). A signature
policy checks a signature over the message's signing digest
([Signature policies](/wiki/programs/policies)).

### Actions

```text
timing = fee u128 ‖ tip u128 ‖ u64 not_before ‖ u64 valid_until ‖ within
within = 00                                   any context
       | 01 ‖ program(32) ‖ commitment(32)    only as a call by that program
```

| Action | Encoding | Rules |
|---|---|---|
| Transfer | `01 ‖ to(32) ‖ amount u128 ‖ timing` | Moves `amount` to `to` |
| Burn | `02 ‖ amount u128 ‖ timing` | Destroys `amount`; `burned` grows |
| Mint | `03 ‖ to(32) ‖ amount u128 ‖ timing` | The account must be the minter, and `issued + amount <= max_supply` |
| Set minter | `04 ‖ minter(32) ‖ timing` | The account must be the minter; the zero account renounces minting forever; `00…01` is rejected |

Every field is part of the signed message.

- `fee` is paid now, to the producer of the block that includes the action. It must
  be 0 when the account's policy requires a delay ([fees and tips](#fees-and-tips)).
- `tip` is paid when a scheduled action executes, to the producer of the block that
  runs it, so miners run due actions without anyone's help.
- `not_before`, a block timestamp or 0 for none, asks for a later execution: a
  scheduled payment, such as rent on the first of the month.
- `valid_until`, a block timestamp or 0 for never, makes an action that has not
  been included expire ([expiry](#expiry)).
- `within` binds the action to the program that calls the coin with it
  ([bound actions](/wiki/programs/conventions#bound-actions)). With `01`, the action
  is valid only when `program` is the calling contract and answers the query `00`
  with exactly `commitment`; otherwise it reverts `not in batch`. A payment that
  pays for other calls of a [batch](/wiki/programs/batch) names the batch and the
  prefix of those calls, so no one can take it out of the batch or attach it to
  other calls. A bound action executes at once or not at all: one that would be
  scheduled, because its account's policy requires a delay or because of its
  `not_before`, reverts `delayed binding`. The program that called the coin with it
  therefore finds its effect complete when the call returns.

### Fees and tips

Fees and tips are paid in the coin itself, to the payout account in the producer
data of the block concerned ([conventions](/wiki/programs/conventions#producer-data);
producer data travels in the [block body](/wiki/data/blocks#the-body)). If that block
has no payout account, nothing is paid and the amount stays with the sender. A payout
account equal to a reserved key counts as none, for fees, tips and rewards alike;
otherwise a miner could overwrite the coin's own records.

An action that may be scheduled should carry a non-zero tip, since the tip is what
pays a miner to run it.

**A delayed account pays no fee.** When the account's policy answers with a delay,
the action's `fee` must be 0, and so must a cancel's; otherwise it reverts
`delayed fee`. A fee is paid at once, so a fee from a time-locked account would let a
thief with one of its keys hand the whole balance to a miner in a single block, or
take it by mining that block. A delayed account pays miners only through tips, which
are locked with the action and paid when it runs, after the delay, unless it is
cancelled first ([nothing leaves at once](/wiki/programs/conventions#delayed-authorization)).
The transaction that schedules or cancels for a delayed account is paid for by
another account in the same transaction: a [batch](/wiki/programs/batch) runs the
operation and then a zero transfer that carries the fee, bound to the batch with
`within` so that the fee is paid only together with that operation
([paying for a transaction](/wiki/programs/conventions#paying-for-a-transaction)).
The paying account must be one a thief cannot sign for: not a slot of the same
identity, whose keys are the thief's too.

### Order of evaluation

1. Decode the input exactly; otherwise revert `bad input`.
2. Revert `expired` if `valid_until` is not 0 and the block timestamp exceeds it.
3. Determine the acting account and load its record. A contract account without a
   calling program reverts `no calling contract`.
4. For a policy account, require `nonce` to equal the stored nonce (`bad nonce`).
5. Check the action: minter rights (`not minter`), reserved targets
   (`reserved account`) and the cap (`supply cap`).
6. Require `available` to cover the fee charged (none when the block has no payout
   account) plus what the action debits, the amount of a transfer or a burn
   (`insufficient funds`); an overflow reverts `amount overflow`.
7. If `within` names a program, revert `not in batch` unless that program is the
   calling contract and, called read-only with the input `00` and the child budget
   of the [conventions](/wiki/programs/conventions#the-budget-reserve)
   (`insufficient budget` below the reserve), succeeds with exactly the 32-byte
   `commitment`.
8. For a policy account, revert `insufficient budget` if the remaining budget is
   below the standard reserve in any counter, then call the policy. A failed call,
   including host error 128 ([calls](/wiki/vm/host#calls-and-deployments)), reverts
   `policy failed`; a result other than `01` or `01 ‖ u64 d` with `d > 0` reverts
   `not authorized`. A required delay with a `fee` other than 0 reverts
   `delayed fee`.
9. Increment the nonce: for a policy account always, for a contract account only
   when the action is scheduled ([scheduling an action](#scheduling-an-action)). A
   nonce that would overflow reverts `nonce exhausted`.
10. Execute now or schedule ([scheduling](#scheduling)). A bound action is never
    scheduled: one whose `execute_at` lies after `now` reverts `delayed binding`.
    An action that is scheduled must also have `available` cover its `reserved`
    amount, tip included, plus the fee (`insufficient funds`, or `amount overflow`).
    The acting account is stored
    first; then the recipients and the payout account are credited, each from a
    fresh read of its record, so the result is exact even when accounts coincide.
11. Update the state record and emit the events.

The cheap checks come before the policy call, so a stale action, or one that cannot
pay its amount and fee, never pays for a signature verification. The tip is the
exception: it is reserved only if the action is scheduled, which the policy's answer
decides, so a balance that covers the amount and the fee but not the tip fails after
the verification.

## Scheduling

### When an action executes

```text
execute_at = max(now + required_delay, not_before)
```

`now` is the block timestamp, `required_delay` comes from the authorization, and the
sum saturates at `2^64 − 1`. If `execute_at <= now`, the action executes immediately,
exactly like an unscheduled action, and its `tip` is ignored. Otherwise it is
scheduled, unless it is a [bound action](#actions), which reverts
`delayed binding` instead. A cancel is never scheduled: it ignores any required delay, as
[delayed authorization](/wiki/programs/conventions#delayed-authorization) requires.

Spending from a time-locked savings account is therefore always scheduled,
including a transfer to the owner's own instant account, and nothing leaves the
account before `execute_at`: it pays no fee, and its tip is locked with the action. A
scheduled payment from an instant account is scheduled only because of its
`not_before`, and pays its fee at once.

### Scheduling an action

The action's sequence number is the acting account's nonce before step 9 increments
it; a contract account increments its nonce for this purpose only. The coin stores

```text
H("app/coin-v1/scheduled", account ‖ u64 sequence) =
    u64 execute_at ‖ u128 reserved ‖ u128 tip ‖ action without its timing
```

adds `reserved` to the account's `locked` and emits a Scheduled event. `reserved` is
`amount + tip` for transfers and burns and `tip` for mints and minter changes. The
action without its timing is its op byte and the fields before `timing`, such as
`01 ‖ to(32) ‖ amount u128` for a transfer. The fee, which only an account without a
required delay can pay, is paid at once. The action's checks run again when it
executes.

### Running a scheduled action

Anyone may run a scheduled action with operation `04` once the block timestamp
reaches `execute_at`. A run of an action that is not scheduled reverts
`not scheduled`, and an earlier call `not due`. Running:

1. checks the action again, since minter rights and the supply cap may have
   changed. A failing check reverts `not executable`, and the record stays until the
   owner cancels it;
2. releases `reserved` from `locked` and applies the action's effects exactly as an
   immediate execution would;
3. pays the tip to the payout account of the block that runs it or, if that block
   has none, releases it to the account's available balance;
4. deletes the record, emits the action's ordinary events, the tip's payment and
   finally an Executed event naming the account and the sequence.

A run needs no signature, so it is cheap ([costs](#costs)). Because it pays a
producer payment, miners that rank transactions by what they pay include due runs on
their own.

Whoever schedules an action can hand its run to a node at the same time, as a
transaction deferred until `execute_at`
([deferred transactions](/wiki/node/mining#deferred-transactions)). The node holds
it and moves it into its pool once it is due and executes successfully, so nobody
has to come back for it. A transaction names no sender
([transactions](/wiki/data/blocks#transactions)), so identical run transactions from
different parties are one transaction with one TransactionId.

### Cancelling

Operation `11` cancels any number of the account's scheduled actions with one
authorization:

```text
cancel = u64 count ‖ count × u64 sequence ‖ fee u128 ‖ u64 valid_until
```

The sequences are strictly increasing. A cancel carries no nonce and leaves the
account's nonce unchanged: each scheduled action can be cancelled only once and no
sequence is ever used twice, so a cancel cannot be replayed, and a thief who spends
the account's nonces cannot make an owner's signed cancel stale. It is accepted
whether or not the account's policy requires a delay, so a recovered identity
cancels a thief's pending withdrawals at once. One signature cancels any number of
them, so a thief who splits savings into many small withdrawals, paying one signature
verification for each, cannot outpace the owner.

1. Decode the input exactly; otherwise revert `bad input`, also for sequences that
   are not strictly increasing.
2. Revert `expired` if `valid_until` is not 0 and the block timestamp exceeds it.
3. Determine the acting account as for an action (`no calling contract`) and load its
   record.
4. Revert `not scheduled` if a listed sequence is not below the account's nonce, so
   was never assigned, or if none of the listed actions is scheduled. Listed actions
   that have already run or been cancelled are skipped, whatever their key holds now.
5. Require `available` to cover the fee charged (`insufficient funds`). The fee is
   checked before any lock is released, so an account whose whole balance is locked
   cancels with a zero fee, or after a deposit.
6. For a policy account, check the policy as in step 8 of an action
   (`insufficient budget`, `policy failed`, `not authorized`). Any required delay is
   accepted, but with a required delay the `fee` must be 0 (`delayed fee`).
7. For each scheduled action listed, release its `reserved` from `locked`, delete its
   record and emit a Cancelled event; then pay the fee.

A cancel is single-use through the actions it lists. Two cancels that list different
actions can both succeed and both pay their fees, even when they share an action. A
wallet that raises a cancel's fee therefore signs the same list again, so that only
one of the two can succeed, or gives the first a `valid_until`.

### Expiry

`valid_until` bounds when a signed action can be included: in a block whose
timestamp is later, it reverts `expired`, so an action that was never included
cannot be used afterwards. It bounds inclusion only. An action that was already
scheduled still executes when due, and stays until it is run or cancelled.

## Mining rewards

`claim` takes no authorization. For the current height `h`:

```text
era    = (h − deploy_height − 1) / halving_interval    (0 when halving_interval = 0)
halved = initial_reward >> era                         (0 when era >= 128)
reward = max(halved, tail_reward)
reward = min(reward, max_supply − issued)
```

The division rounds down. A claim reverts `already claimed` unless
`h > last_claimed`, `no payout account` if the block's producer data has none, and
`no reward` if the reward is zero. Otherwise it credits the payout account, adds the
reward to `issued`, sets `last_claimed = h`, emits a Transfer from the zero account
and a producer payment, and returns the reward.

Whoever submits a claim, the reward goes to the account in that block's producer
data, which only the block's miner can choose
([producer data](/wiki/node/mining#producer-data)). A block without a claim forfeits
its reward. A reorganization rolls claims back with their block.

## Events

Each event's payload starts with its 32-byte topic, `H(tag, "")`; the emitting
ContractId identifies the coin ([events](/wiki/data/blocks#events)).

| Event | Payload |
|---|---|
| Transfer | `H("app/coin-v1/event/transfer", "") ‖ from(32) ‖ to(32) ‖ amount u128` |
| Minter changed | `H("app/coin-v1/event/minter", "") ‖ minter(32)` |
| Scheduled | `H("app/coin-v1/event/scheduled", "") ‖ account(32) ‖ u64 sequence ‖ u64 execute_at ‖ tip u128 ‖ action without its timing` |
| Executed | `H("app/coin-v1/event/executed", "") ‖ account(32) ‖ u64 sequence` |
| Cancelled | `H("app/coin-v1/event/cancelled", "") ‖ account(32) ‖ u64 sequence` |
| Producer payment | The producer payment event of the [conventions](/wiki/programs/conventions#producer-payment), for fees, tips and rewards |

```text
transfer topic  = 59a929440b7f57a03cad176d225f75a76507f929ccd0248732ce032573ec6dd9
minter topic    = 49fa6d3e0a59b224360272ae992b933554a2fe546183c5fbbc0c981c11629a37
scheduled topic = 0c5552a092c4f9d07b46a23290da71a795daf1dbb79cfac0a74cfb4f2d2b060d
executed topic  = 46da6558e6256be0733d2f43b35df79073604ed5a485f3d9907f0d5df0e32116
cancelled topic = a1718d179c8935287b762407f8f51ee28cb9b2be64ced5af46e00bb0e6df0d27
```

The zero account as `from` marks issuance, and as `to` a burn. A fee or tip emits a
Transfer to the payout account plus a producer payment. Replaying the events
reproduces every balance and every lock: Transfer events carry every movement of
coins, a Scheduled event the action and its tip, from which its `reserved` follows,
and an Executed or Cancelled event releases that action's lock. A Scheduled event
also clears the balance under its record's key,
`H("app/coin-v1/scheduled", account ‖ u64 sequence)`: coins someone paid to that key
before the action was scheduled are lost there, with no event of their own, and the
circulating supply `issued − burned` then exceeds the sum of all balances by that
much. Scheduled events show
recipients their pending incoming transfers, and wallets alert owners on every
Scheduled event of their accounts, so that an unwanted action is seen while it can
still be cancelled.

## Costs

Typical costs per transaction, rounded. A signed action costs mostly its signature
policy. Every transaction that runs the coin also pays its code price once, about 1.3
million C, which is most of what an unsigned operation costs
([the cost of an invocation](/wiki/programs/conventions#the-cost-of-an-invocation)).

| Operation | C, about | S |
|---|---:|---:|
| Transfer signed with the secp256k1 policy | 14.2 million: the policy's 12.6 million and the coin's 1.5 million | 15 |
| Transfer signed with the P-256 policy | 19.1 million | 15 |
| Transfer signed 2 of 3 with the multisignature policy | 22.6 million | 12 |
| Transfer from an identity account, secp256k1 key | 15.6 million | 17 |
| Transfer from an identity account, P-256 key | 20.5 million | 17 |
| Scheduling a transfer with `not_before` | 14.2 million | 14 |
| Running a scheduled transfer | 1.48 million | 16 |
| Cancelling one scheduled action, secp256k1 key | 14.2 million | 15 |
| Each further action in the same cancel | 48,000 | 3 |
| Contract-account transfer | 1.58 million | 12 |
| Reward claim | 1.42 million | 9 |
| Rejected action (stale nonce) | 1.47 million | 5 |
| Balance query | 1.41 million | 5 |
| Deployment with two allocations | 2.06 million | 18 |
| A sponsored savings cancel through a batch: the cancel and a bound fee payment | 25.0 million | 44 |

Signed actions are bound by C, not S. At the production network's initial
150,000,000 C and 1,024 S per block
([production profile](/wiki/consensus/genesis#production-profile)), ten
secp256k1-signed transfers or seven P-256-signed ones fit in a block. A thief who
schedules 20 small withdrawals from a stolen key's savings pays about 312 million C,
just over two blocks' worth; the owner cancels all 20 with one signature for about
16.5 million. The verifications themselves are on
[Signature policies](/wiki/programs/policies).

## Not included

Allowances (`approve` and `transferFrom`), transfer hooks, freezing, blacklists and
adjustable parameters are deliberately absent. Contract accounts cover delegated
spending: a program that holds coins enforces its own rules for spending them. A
coin with other rules is a deployment of another program, with another CodeHash; an
existing coin never changes.
