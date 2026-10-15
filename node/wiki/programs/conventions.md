# Shared conventions

> [!NOTE]
> Conventions of the reference programs, not part of the protocol: the chain runs
> programs that follow them like any other deployment. See
> [Reference programs](/wiki/programs).

The reference programs agree on how they encode and hash, how an account is named and
authorized, how a signature policy answers, how a block's producer is paid and how
events are tagged. Programs that follow the same conventions work together: one
account works with every coin, one wallet can sign for every application, and a miner
can rank transactions from any application without knowing it. On these pages an
**application** is a program that keeps records for accounts, such as a
[coin](/wiki/programs/coin) or the [identity registry](/wiki/programs/identity).

## Encoding

Integers have fixed widths and are little-endian, and a byte string carries its
length, as in the protocol's [canonical bytes](/wiki/data/encoding#types):

```text
Bytes(x) = u32(len(x)) ‖ x
```

`‖` is concatenation, the same operation the protocol pages write `||`; every page of
this section writes it `‖`. Every decoder rejects trailing bytes and out-of-range
lengths. Public keys and signatures keep the standard forms of their curves, which
are big-endian ([signature policies](/wiki/programs/policies#how-they-verify)).

## Hashing

```text
H(tag, x) = SHA256(ASCII("Ting/") ‖ u16(len(tag)) ‖ ASCII(tag) ‖ x)
```

This is the protocol's
[domain-separated hash](/wiki/data/encoding#domain-separated-hashing), which the
programs compute in their own code. Their tags all start with `app/`, which no
consensus tag does, so a value hashed by a program can never be taken for one hashed
by the protocol. These conventions use four tags; each application defines
further tags of its own.

| Tag | Hashes | Produces |
|---|---|---|
| `app/account-v1/policy` | A policy's ContractId and its params | A [policy account](#policy-accounts) |
| `app/account-v1/contract` | A program's ContractId | A [contract account](#contract-accounts) |
| `app/sign-v1` | A message | The [signing digest](#signed-messages) |
| `app/event/producer-payment-v1` | Empty data | The [producer payment](#producer-payment) topic |

## Common rules

- **Signing is off chain.** Program state is public, so programs hold no secrets:
  signatures are made off chain, and the programs only verify them.
- **The deployment invocation.** A deployment runs `invoke` with the deployment's
  input ([deploy and invoke](/wiki/vm/lifecycle#deployment)). A program that needs
  no setup succeeds on empty input without any effect, so it can be installed with
  empty input ([canonical deployment](/wiki/programs#canonical-deployment)): the
  signature policies, the identity registry and `sha256` work this way. A program
  that does need setup, such as the coin, recognizes its first run by a record that
  is still absent.
- **Revert reasons.** A program that refuses reverts with a short ASCII string as
  its result bytes, such as `insufficient budget`. The bytes reach its caller, or
  the receipt at the top level ([checkpoints](/wiki/vm/calls#checkpoints)).

## The cost of an invocation

Every invocation reads two records, the program's manifest and its code, for 2 S
and 3,800 C of key hashes; the first invocation of a program in a transaction also
pays its code price, about 60 C per byte of code
([invocation](/wiki/vm/lifecycle#invocation)). Every invocation then pays 40,000 C to
start an instance, and each reference program starts with one 64 KiB page of memory,
another 40,000 C every time it runs ([the price list](/wiki/resources/prices)). A call
to another program therefore costs about 84,000 C and 2 S before the callee does any
work of its own, plus its code price the first time in a transaction.

## No arbitrary limits

The reference programs impose no maximum on counts, lengths or input sizes. Instead
of fixed buffers, they grow memory at run time with `memory.grow`, which is charged
only when used ([memory growth](/wiki/vm/metering#memory-growth)). They store each
value in a single record of any size
([values of any size](/wiki/data/records#values-of-any-size)), and they count with
`u64`.

The only bounds left are the protocol's own: 4 GiB of memory per instance, the whole
32-bit address space, and module-shape bounds far above what compilers emit
([module shape](/wiki/vm/profile#module-shape)). Everything else is priced. Logical
safety rules, for example that one delay must exceed another, are not limits.

## Calling other programs

Calls are cheap enough to compose freely. A coin calling an identity that calls the
policy of one of its keys adds about 84,000 C and 2 S per hop, plus each program's
code price the first time a transaction runs it
([the cost of an invocation](#the-cost-of-an-invocation)), with no depth limit
([depth](/wiki/vm/calls#depth)). Linking code into a program is still cheaper for
small pure helpers, such as a hash function
([calling or linking](/wiki/programs/sha256#calling-or-linking)). Shared logic belongs
in its own program whenever reuse matters, or upgrading it independently by a new
deployment ([upgrades through programs](/wiki/applications/upgrades)).

### The budget reserve

A caller sizes a child's budget from its own remaining budget, which it reads with
context selector 8 ([context](/wiki/vm/host#context)). No budget therefore has to
travel in signed or relayed input:

```text
child budget = remaining − R                               in each counter
R            = [500,000 C, 64 S, 65,536 D, 65,536 G]       the standard reserve
```

`R` covers everything the caller spends after reading its remaining budget: copying
the call's target and budget, and its own work after the call. The callee pays for
receiving its input out of its own share
([calls and deployments](/wiki/vm/host#calls-and-deployments)), so an input of any
size fits. A caller whose remaining
budget is below `R` in any counter does not call: an application reverts with
`insufficient budget`, and a policy that consults another, such as the
[identity registry](/wiki/programs/identity#the-registry-as-a-policy), answers `00`.
The reserve is held back, not consumed, so a generous value costs nothing; the
transaction's budget is the real cap
([budgets within budgets](/wiki/resources/budgets#budgets-within-budgets)).

## Accounts

An account is a 32-byte identifier by which an application keys its records. It says
how the account authorizes actions, never who owns it. There are two kinds:

```text
policy account   = H("app/account-v1/policy",   policy ‖ Bytes(params))
contract account = H("app/account-v1/contract", owner)
```

`policy` is the ContractId of a policy program and `params` the bytes it is given,
such as a public key. `owner` is the ContractId of the program that owns the account.
Both are 32-byte identifiers ([identifiers](/wiki/data/records#identifiers)).

### Policy accounts

A policy account is authorized when its policy program approves a message with the
witness supplied for it ([policy interface](#policy-interface)). The account commits
to the policy and its params, and since installed code never changes, neither can
that rule. New signature schemes, post-quantum ones included, are new policy
programs. Which ContractId a reference policy has on a chain follows from its
[canonical deployment](/wiki/programs#canonical-deployment).

### Contract accounts

A contract account is an account owned by a program. It is authorized when the
application is called by that program, as the calling ContractId in the context shows
([context](/wiki/vm/host#context)). The runtime sets that value, so it cannot be
forged. Any logic with state of its own, such as a joint account with internal
shares, an escrow or a pool, is a program that owns a contract account.

### Nonces

An application keeps a sequential nonce for each policy account, so that every signed
message can be used only once. A cancel is the exception: it is single-use by itself
and carries no nonce ([cancellation cannot be raced](#delayed-authorization)). A
program that owns a contract account handles replay protection itself.

## Policy interface

A policy program decides whether a message is authorized, and whether the authorized
action must wait. It is called with three byte strings:

```text
input = Bytes(params) ‖ Bytes(message) ‖ Bytes(witness)
```

| Result | Meaning |
|---|---|
| `00` | Not authorized |
| `01` | Authorized now |
| `01` followed by `u64(delay)`, with `delay > 0` | Authorized only as an action scheduled at least `delay` seconds ahead ([delayed authorization](#delayed-authorization)) |

Any other result counts as not authorized, `01` followed by `u64(0)` included: a zero
delay is written as the plain `01`.

**A delay depends on who signs, never on what is signed.** A policy's answer may
depend on its params and its witness, which say who signs, but the delay it requires
never on the message: every message it authorizes for the same signer gets the same
delay, cancels included. An application decides from that answer alone what may
leave at once, such as a fee ([fees and tips](/wiki/programs/coin#fees-and-tips)). A
policy that answered a plain `01` to some messages, small amounts or cancels for
example, and a delay to others would let anyone holding its key take through a fee
what the delay should hold back. Spending limits therefore belong in applications,
not in policies.

- **Empty input** succeeds with an empty result. It is the deployment invocation.
- **Malformed framing**, input that is not exactly three `Bytes` fields with nothing
  after them, reverts.
- **Invalid params or witnesses** return `00`. They never revert.

### Calling a policy

An application calls a policy read-only, with flags 1
([read-only mode](/wiki/vm/calls#read-only-mode)), and with the child budget of
[the budget reserve](#the-budget-reserve), so a policy cannot change state. A call
that fails counts as not authorized, and so does a `call` that returns 128 because
the budget did not fit.

## Delayed authorization

A delay lets an account declare, once, that its value-moving actions must be announced
in advance and stay cancellable: a time-locked savings account. Because the policy
declares it, every application that supports scheduling honors it, for every asset the
account holds, from the first deposit.

- **Fail closed.** An application that cannot schedule must treat any result other
  than exactly `01` as not authorized. Such an application can refuse a delayed
  account, but never bypass its delay.
- **Scheduling.** An application that schedules executes the action no earlier than
  `now + delay`, measured in block timestamps, and lets the account cancel it until
  then.
- **Nothing leaves at once.** An application takes nothing from a delayed account
  before the delay has passed: no fee, no deposit, no payment to anyone. What a
  delayed action pays, a miner's tip included, is locked with the action and paid
  when it executes. Anything paid at once would let a thief holding one key empty a
  time-locked account: a miner collects at once whatever an action pays at once, and
  a thief can be that miner, or pay one.
- **Cancellation is never delayed.** Cancelling a scheduled action accepts `01` with
  or without a delay, and costs a delayed account nothing either, so a thief who
  cancels pending actions gains nothing.
- **Cancellation cannot be raced.** A cancel needs no nonce: each scheduled action
  can be cancelled only once, so a cancel cannot be replayed, and a thief who spends
  an account's nonces cannot make the owner's signed cancel stale. One authorization
  should cancel any number of actions, so that cancelling a thief's many small
  withdrawals costs the owner one signature while scheduling them cost the thief one
  each.
- **Composition.** A policy that consults another policy, for example an identity
  checking one of its keys, returns the larger of its own delay and the inner
  result's delay.

## Signed messages

A message starts with `Bytes(app_tag)`, the application's own domain string, for
example `app/coin-v1`. A signature made for one application therefore cannot be
presented to another, and a wallet can show what is being signed. Signature policies
check signatures over the signing digest of the message:

```text
signing digest = H("app/sign-v1", message)
```

After the tag, a message names everything the signature must not be moved to: the
ChainId, the application's own ContractId, the acting account and the operation,
together with what makes it single-use, such as the account's nonce. Naming the
account is essential. One key often authorizes several accounts: the slots of an
[identity](/wiki/programs/identity) share their keys, so a message that did not name
its account could be presented for the instant spending slot when it was signed for
the time-locked savings slot. The policy never sees which account it authorizes,
only the message.

## Producer data

Producer data is chosen by the miner, carried in the block body without a size limit
and committed by the header's `producer_hash`
([the body](/wiki/data/blocks#the-body)). A program reads it with context selector 7
([context](/wiki/vm/host#context)). An application that pays miners reads a payout
account from it:

```text
producer_data = "OCP1" ‖ entry*
entry         = u32(type) ‖ Bytes(value)
```

`"OCP1"` is those four ASCII bytes, `4f435031`, and `entry*` is any number of entries,
back to back, up to the end of the data.

- An entry of type 1 with a 32-byte value is the payout account. Entries are read in
  order and the first such entry counts.
- Entries of every other type, and type-1 entries whose value is not 32 bytes, are
  skipped.
- Data without the `OCP1` prefix, or whose entries overrun the end before a payout
  account is found, has no payout account.

Producer data holding only a payout account is 44 bytes:

```text
4f435031   01000000   20000000    <the 32-byte payout account>
"OCP1"     type 1     length 32
```

A miner running the reference node sets these bytes as its
[producer data](/wiki/node/mining#producer-data).

## Events

An application's event payload starts with a 32-byte topic, followed by data specific
to the event:

```text
topic = H(tag, "")
```

The topic is the hash of the event's tag over empty data. The event itself carries the
emitting program's ContractId ([events](/wiki/data/blocks#events)), and that
identifies the application.

### Producer payment

Every application that pays a block's producer emits:

```text
topic   = H("app/event/producer-payment-v1", "")
        = 29735265b80e981b9b7271e3de08bec1762823b25c977b2986b052879d0b6bcb
payload = topic ‖ payee ‖ u128(amount)
```

`payee` is the 32-byte account paid and `amount` the amount paid, so the payload is
80 bytes. The asset is the emitting program's own: a coin's payments are in that
coin. A miner can simulate pending transactions, add up the producer payments to its
own payout account, value each asset by its own policy
([local policy](/wiki/node/mining#local-policy)), and order transactions by payment
per share of block capacity, all without knowing any application.

## Paying for a transaction

A transaction names no sender, and the protocol charges no fee
([no native coin](/wiki/decisions/no-native-coin)): a miner is paid by the producer
payments the transaction emits. Some operations pay nothing themselves. The identity
registry's operations carry no payment, and a delayed account pays nothing at once
([nothing leaves at once](#delayed-authorization)), not even for scheduling or
cancelling its own actions. Such an operation is paid for by another account in the
same transaction.

A transaction is one call, so the [batch](/wiki/programs/batch) program combines them:
it runs several calls in order, all or none. The operation comes first and the
payment last, and the payment is a [bound action](#bound-actions): it names the batch
and the prefix of the calls before it. A payment that anyone could take out of the
batch would be worthless as a guarantee: a miner would submit it alone, collect the
fee and leave the operation out, which even pays more per share of block capacity.
Bound to the prefix, the payment is valid only right after exactly those calls, in
the batch that ran them, so the miner is paid only for including the operation.

- **Who pays.** The paying account must be one a thief cannot sign for. A slot of the
  same identity shares its keys, so a thief who holds one would empty it first; a
  guardian, a family member or any other account can pay instead.
- **One payment per nonce.** A wallet never signs two payments with the same nonce
  that differ in their binding, or one of them unbound: a miner would use the one
  that suits it.
- **The operation stays valid alone.** The operation itself is not bound, so it can
  be submitted on its own as well, unpaid. A miner whose policy includes unpaid
  transactions, as the reference node's does, mines it either way. A wallet
  submits it on its own too, so that it does not depend on the payment.
- **Only operations that fail closed.** The payment commits to the operation's
  input and its success, not to what it achieved. A sponsor therefore pays only for
  operations that are single-use and fail when they do not take effect, even when
  whoever submits them gives them too little budget. Every operation of the
  reference programs is such an operation.
- **One operation per payment.** Anyone with a key of the paying account, or of the
  account whose operation is paid for, can make one of several bundled operations
  fail first, a thief cancelling its own change for example, and so the whole batch.
  A wallet pays for each defensive operation in a batch of its own.
- **Expiry.** A payment carries a `valid_until`, so that one whose operation went in
  alone does not stay usable. The sponsor's next payment needs the next nonce in any
  case.

## Bound actions

An application may let an action name the program it must be called from, and a
commitment that program vouches for:

```text
within = 00                                   any context
       | 01 ‖ program(32) ‖ commitment(32)    only as a call by that program
```

The application accepts a bound action only when `program` is its calling contract
([context](/wiki/vm/host#context)) and, called read-only with the one-byte input `00`,
answers with exactly the 32-byte `commitment`. It executes a bound action at once or
refuses it: a bound action is never scheduled for later, whether its account requires
a [delay](#delayed-authorization) or the action names a later time of its own. When
the call returns successfully, the program it was bound to can therefore rely on the
action's effect.

The program defines what it vouches for: the [batch](/wiki/programs/batch) answers
with the prefix of the calls it has completed, so a payment bound to it is valid only
after exactly those calls. Any program can answer the same query. A program that
settles trades, for example, can vouch for the trade it is executing, so that each
party's transfer, bound to that program and that trade, is valid only while the whole
trade runs and takes effect within it. The [coin](/wiki/programs/coin#actions)
supports bound actions.

A binding to the batch commits only to the calls before the bound action, in the
same run of the batch:

- an action bound at the first position commits to nothing, since every batch
  starts from the same prefix, so it can always run alone;
- calls after the bound action, and calls a program makes before or after calling
  the batch, are not committed to;
- one party's action can therefore never depend on another party's later call. Two
  transfers bound to each other in a batch make no exchange, since the first runs
  alone; an exchange needs a program that settles it, as above.

A wallet binds an action only to a program whose CodeHash it knows: a program that
answers every query as expected vouches for nothing.
