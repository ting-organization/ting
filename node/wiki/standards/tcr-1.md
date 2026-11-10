# TCR-1: Ting Call Requests

> [!NOTE]
> An application standard, not part of the protocol: an agreement between programs,
> wallets and pages, which the chain knows nothing of. See
> [Application standards](../standards.md).

TCR-1 is how a page asks a Ting user to do something in a program on chain,
such as giving away an item, buying one or listing one for sale, and how the
program makes sure the user agreed to exactly that. The whole request travels
in a link, as a QR code or a tap, so a program's page can be a static site:
no server stores requests or receives answers. The wallet signs and submits
the call itself, and the page reads the outcome from the chain.

[TOCS-1](tocs-1.md) signs things off chain for a site; TCR-1 acts on chain in
a program. Their messages can never be taken for each other's.

**Must**, **must not** and **should** are requirements: an implementation that
breaks a *must* is not TCR-1. This page is the whole standard: a program, a
wallet or a page can be written from it and its [test vectors](#test-vectors).

## Design

TCR-1 is built in four layers, each with one job:

1. **Operations** say what is done, in the program's own words: a
   [template](#templates), which declares an operation's fields and gives the
   sentence that shows them, and the fields' values.
2. **Authorization** says who does it: an account that approves the
   operation itself through its policy, or a program acting as its own
   account by calling. The account's own delay decides whether an operation
   happens now or is scheduled.
3. **Payments** move coins, when an operation takes some: a coin payment tied
   to the one approved operation it pays for. A payment from the acting
   account is also its approval, so a call that pays checks one signature. A
   call that moves nothing can carry a fee payment, a payment of nothing that
   pays the miner, and so also check one signature.
4. **The link and the wallet** carry an operation to the user, and show it.

It holds to these properties:

- What the user signs is tied to the operation itself, the chain, the program
  and their account, never to an opaque hash a page supplies.
- A page cannot reword an operation: a program publishes its templates and
  accepts no others.
- Money that leaves the user's account is what the wallet checks and shows
  itself, never a program's claim.
- An account with a delay keeps it in every program: its operations wait, can
  be cancelled, and are visible to the wallet, as its coin payments are.
- One link works for every user and needs no server.
- Programs implement one fixed set of inputs, records and events, and wallets
  need no knowledge of any particular program.
- Using a program does not hold up the user's other activity: an operation's
  nonce is the program's own, and only an operation that pays, or whose fee
  the acting account pays itself, uses the coin's.

## Roles and flow

- The **program** is a Ting program that follows this document.
- The **page** shows the link: a static site, an app, a printed poster. Anyone
  can show any link, so the page plays no part in security.
- The **wallet** holds the user's keys. It shows the operation,
  signs only once the user approves, and submits the call.

1. The page shows a **call link**, `ting:call?…`, as a QR code to scan or a
   link to tap.
2. The wallet reads the link, checks it against the chain, and shows the
   program, the operation in the program's words, and what will leave the
   user's account.
3. Once the user approves, the wallet signs the operation, or the payment
   that stands for it, and submits the call.
4. The program checks that the acting account approved the operation. It
   does the operation now, or schedules it when the account has a delay, and
   runs the payment through the coin.
5. The page learns the outcome from the chain, from the program's events or
   records, read from a node. A link may name a page to go back to, which
   the wallet offers to open with the transaction's ID.

## Notation

- `‖` joins byte strings. Integers are unsigned and little-endian: `u8`,
  `u16`, `u32`, `u64`, `u128`.
- `Bytes(x) = u32(len(x)) ‖ x`.
- Text is UTF-8. `"abc"` in a byte layout means its UTF-8 bytes.
- Hex is lowercase, two digits a byte, without `0x`.
- **H**, Ting's domain-separated hash ([domain-separated hashing](../data/encoding.md#domain-separated-hashing)):

  ```text
  H(tag, x) = SHA-256("Ting/" ‖ u16(len(tag)) ‖ tag ‖ x)
  ```

- **Accounts** ([accounts](../programs/conventions.md#accounts)):

  ```text
  policy account   = H("app/account-v1/policy",   policy ‖ Bytes(params))
  contract account = H("app/account-v1/contract", program)
  ```

  `policy` is the ContractId of the program that authorizes a policy account,
  and `params` the bytes it is given, such as a public key or an
  [identity](../programs/identity.md) and its slot. A contract account belongs to the program whose ContractId it hashes.
- **Signing.** A policy checks a witness over a message's signing digest,
  `H("app/sign-v1", message)` ([signed messages](../programs/conventions.md#signed-messages)).
- **R** is the standard budget reserve, and a **child budget** the caller's
  remaining budget minus R ([the budget reserve](../programs/conventions.md#the-budget-reserve)).
- The **coin** is the coin reference program ([coin](../programs/coin.md)).
- **Times** are block timestamps, in Unix seconds.

## Operations

### Templates

A template declares an operation, its name and its fields, and then gives the
sentence, in the program's own words, that shows them:

```text
give(item:u64,to:account): Give item {item} to {to}
```

The program's author writes its templates, and the program accepts an
operation only under one of them. It publishes them in its
[template list](#records). A link carries the template in full, so the wallet
can show it; the call carries only its hash.

The declaration fixes the values' bytes: the fields in the order declared.
The sentence may name the fields in any order, and name one more than once.
So the same operation can have templates in several languages that share
one declaration, and the program one way to read its values.

### Grammar

```text
template = operation "(" [ field { "," field } ] ")" ": " sentence
field    = name ":" type
type     = scalar | list
scalar   = "u8" | "u16" | "u32" | "u64" | "u128"
         | "amount" | "coin" | "account" | "bytes32"
         | "time" | "duration" | "text" | "bytes" | enum
enum     = "enum(" label { "|" label } ")"
list     = "list(" name ":" scalar { "," name ":" scalar } ")"
operation, name = letter { letter | digit | "_" }      at most 32 characters
letter   = "a" … "z"
digit    = "0" … "9"
label    = one or more characters other than | ( ) { } : ,
sentence = { words | slot }                            at least one character
slot     = "{" ( name | "payment" ) "}"
words    = one or more characters other than { }
```

The declaration ends at the `)` that closes the one after the operation's
name: a label holds no parentheses, so every `(` in it is matched. The
sentence starts after the `": "` that follows. A template must also:

- be a **line** under [TOCS-1's text rules](tocs-1.md#text): valid UTF-8 in
  its shortest form, in Normalization Form C, free of the listed code points
  that hide or reorder text, not empty, without U+000A, and neither starting
  nor ending with white space;
- be at most 512 bytes, declaration and sentence together. A program with a
  large enum, or many fields, may not fit: that is a limit of this version;
- name each field once, and the fields of a list once within it. `payment` is
  not a field name: it names the [payment slot](#the-payment-slot);
- show every field in the sentence at least once, so nothing that is signed
  goes unshown, and name no field it does not declare;
- have labels that are not empty, neither start nor end with white space, and
  are not repeated within an enum.

Templates that share an operation name share their declaration: within one
program's list, two templates with the same name have the same text up to and
including `"): "`. A program may thus accept an operation in several
languages, and still read its values one way.

### Field types

| Type | Bytes in the values | The wallet shows |
|---|---|---|
| `u8`, `u16`, `u32`, `u64`, `u128` | The integer, little-endian | The number, in decimal |
| `amount` | `coin(32) ‖ u128` | The amount in that coin's units, with its symbol |
| `coin` | 32 bytes, a ContractId | The coin by its name |
| `account` | 32 bytes | One of the user's own accounts by name, a contact by name, or else the 64 hex digits, marked as not a contact |
| `bytes32` | 32 bytes | The 64 hex digits |
| `time` | `u64`, Unix seconds | The date and time, in the user's time zone |
| `duration` | `u64`, seconds | A length of time, such as 7 days |
| `text` | `Bytes(UTF-8)` | The text, quoted. It follows the rules of a line, except that it may be empty |
| `bytes` | `Bytes(x)` | Its length; the hex when opened |
| `enum(l₀\|…\|lₖ)` | `u8`, the label's index from 0 | The label. An index past the last label is refused |
| `list(n₁:t₁,…,nₖ:tₖ)` | `u64 count ‖ count × (t₁ ‖ … ‖ tₖ)` | The count; when opened, the items, each field under its name |

- An `amount` carries its coin, so that the coin it is shown in is part of
  what the user signs. The program must check that the coin is one it means.
- A coin, in an `amount` or a `coin` field, is shown by name only when the
  wallet knows it: a coin it lists or the user added, running the coin
  reference program. Otherwise the wallet shows the ContractId, marked as a
  coin it does not know, and an amount in the coin's smallest units.
- A list holds any of the scalar types, an enum included, but no list.

### The payment slot

`{payment}` in a sentence shows the call's [payment](#payments): the amount
and the coin the wallet checked itself, such as "25 DLR". It is not declared
and takes no bytes in the values. A program reads the amount from the
payment, so the sentence and what leaves the account can never disagree.

A template that shows `{payment}` requires the call to carry a payment from
the acting account: that payment is then the account's approval, so the
amount in the sentence is one the account's own signature covers. A program
must refuse such a call without a payment, or with a payer other than the
acting account, and a wallet must refuse such a link without a payment.

The other way round, a payment that moves coins goes only with a template
that shows `{payment}`: what leaves the account is always in the sentence too.
A template without `{payment}` takes no payment, or a
[fee payment](#fee-payments), which moves nothing.

### Values

The **values** of an operation are the fields' bytes, in the order the fields
are declared, back to back. Decoding is exact: the wallet must refuse the
whole link, never skip a part, when the values have fewer bytes than the
fields call for or any byte after the last field, when a text breaks its
rules, when an enum index is past its last label, or when a list's count
could not fit in the bytes left.

The wallet shows the sentence with each slot replaced by its field's value,
shown as the table says, and `{payment}` by the payment; the rest of the
sentence as written.

### Hashes

```text
T         = H("app/call-v1/template", template)
operation = H("app/call-v1/operation", T ‖ values)
```

`template` is its UTF-8 bytes, declaration and sentence, exactly as the link
carries it: any change, a single space included, gives another `T`.
`operation` names what is done, whoever does it.

## Authorization

### The call

```text
call = T ‖ values ‖ auth ‖ payments
auth = 00 ‖ policy(32) ‖ Bytes(params) ‖ u64 nonce ‖ u64 valid_until ‖ Bytes(witness)   a policy account
     | 01                                                                          the calling program's account
```

`payments` is described under [payments](#payments); a call without one has
the single byte `00`.

**A policy account** is any account a policy program authorizes: a key, an
identity's slot. It is `H("app/account-v1/policy", policy ‖ Bytes(params))`,
and it approves the message

```text
message = Bytes("app/call-v1") ‖ ChainId ‖ program ‖ account ‖ 01 ‖ u64 nonce ‖ u64 valid_until ‖ operation
digest  = H("app/sign-v1", message)
```

in one of two ways: with its policy's witness over `digest`, or, in a call
that pays, with a payment from the account itself bound to `digest`
([payments](#payments)) and an empty witness. `ChainId` and `program`, the
program's own ContractId, come from the program's execution context. `nonce`
is the account's nonce in the program ([records](#records)), apart from its
nonce in any coin.

**A program's account** is the contract account of the program that calls
this one, `H("app/account-v1/contract", caller)`, with `caller` from the
execution context. Only that program can act for it, by calling. It has no
nonce, no deadline, no signature and no delay: the calling program keeps its
own rules, replay protection included
([contract accounts](../programs/conventions.md#contract-accounts)). It lets a program hold what
other programs keep, such as an escrow or a marketplace holding items for
sale. A wallet never uses it.

A program's account carries no payment, and an operation that takes one is
not open to it. A payment is a coin action of a policy account, bound to the
program that calls the coin with it, and a program has no such action to
make from inside another program's call: a program that pays does so on its
own, before or after its call.

### What the program does

For a call, the program must:

1. Decode the input exactly: `T` must be the hash of one of its own
   templates, the values must decode as that template's declaration says, and
   `payments` may hold one payment at most; otherwise revert `bad input`.
2. For a policy account, revert `expired` when `valid_until` is not 0 and the
   block timestamp exceeds it, and `bad nonce` unless `nonce` equals the
   account's stored nonce.
3. Establish that the account approved the operation, and when it may take
   effect:
   - **A policy account that pays itself.** When the call's payment, or its
     [fee payment](#fee-payments), comes from the acting account, `witness`
     must be empty (`bad input` otherwise) and the program does not call the
     policy. The coin checks the payment, which
     is bound to `digest`, when the program runs it in step 7, and refuses it
     from an account with a delay, so the operation takes effect now.
   - **Any other policy account.** Revert `insufficient budget` when the
     program's remaining budget is below R in any counter. Otherwise call the
     policy read-only with the child budget and
     `Bytes(params) ‖ Bytes(message) ‖ Bytes(witness)`. A failed call, or any
     result other than `01` or `01 ‖ u64 d` with `d > 0`, reverts
     `not authorized`. `01` means now, and `01 ‖ u64 d` after `d` seconds.
   - **A program's account.** Revert `no calling contract` when no program
     called, and `bad input` when the call carries a payment. It takes effect
     now.
4. With a delay: revert `delayed payment` when the call has a payment, and
   `not authorized` when the program does not schedule
   ([now or later](#now-or-later)).
5. For a policy account, increment the nonce.
6. With a delay, schedule the operation, and stop there.
7. With a payment, check it and run it through the coin
   ([payments](#payments)).
8. Do the operation for the account, and nothing but what its template says,
   and emit *executed*.

**The order is the program's.** The steps say what must hold, not in what
order to check it. A program should check the operation's own conditions,
such as that the account holds the item or that the price is right, before it
calls the policy, as the coin arranges its own checks
([order of evaluation](../programs/coin.md#order-of-evaluation)). A call that would fail anyway then
fails before its most expensive part, and a dry run of the call with an empty
witness tells whether it can succeed: it fails with the program's own reason
when it cannot, and on `not authorized`, or the coin's refusal of the
payment's signature, only when nothing else stands in the way. A wallet may
dry-run a call so before the user approves, when its nodes offer it.
Whatever the order, a call whose account did not approve it, or whose payment
fails, reverts as a whole.

A program may accept the same operation under several templates, one per
language for instance; each is checked the same way.

### Now or later

The policy's answer says when an operation may take effect. `01` means now.
`01 ‖ u64 d` means only after `d` seconds, cancellable meanwhile: the answer
for an account with a delay, such as an identity's savings account, whose
operations wait so that a thief who holds one of its keys cannot act at once
([delayed authorization](../programs/conventions.md#delayed-authorization)).

**A program that holds assets for accounts should schedule.** Otherwise
whatever an account with a delay holds in it can never move, and its users
are pushed to keep their assets in an account without the lock. A program
that holds nothing for anyone may refuse delayed operations; it must then
refuse them outright, never carry them out at once.

To schedule, the program writes the operation's
[scheduled record](#records), under the account and its **sequence**, the
nonce the operation was signed with, with

```text
execute_at = now + d          (saturating at 2^64 − 1)
```

and emits *scheduled*. A program that schedules keeps these rules, and one
that breaks them breaks the time lock:

- **Nothing leaves at once.** A scheduled operation moves nothing and pays no
  one until it runs. It carries no payment.
- **The delay belongs to the signer.** It is the policy's answer, never
  shortened for a small or harmless-looking operation.
- **Cancelling is never delayed, never raced, and costs the account nothing**
  ([cancelling](#cancelling)).

A link says that its program schedules the operation with `delayed=1`, and
the wallet then offers the user's accounts with a delay for it too.

### A scheduled operation, step by step

An operation from an account with a delay `d` (7 days for an identity's
savings account, by the registry's defaults) goes through three transactions, each paid for like any call
that pays nothing itself: by a fee payment from another account, bound to a
batch after the call, or by no one while blocks have room.

| When | Transaction | Signed by | Paid by | What happens |
|---|---|---|---|---|
| At once | The [call](#the-call) | The account with the delay | An account without a delay, or no one | The program checks the signature through the account's policy, which answers with the delay. It writes the scheduled record and emits *scheduled*. The wallet hands the run to its nodes, deferred until `execute_at` |
| Until `execute_at` | None | | | Nothing changes. The owner's wallet shows the operation as pending, with a way to cancel it |
| Any time before it runs | A [cancel](#cancelling) | Any key of the account | An account a thief cannot sign for, or no one | The program deletes the record and emits *cancelled* |
| From `execute_at` | The [run](#running) | No one | No one while blocks have room; otherwise whoever wants it in, from then on | The program does the operation with the checks of that moment, deletes the record and emits *executed* |

Nothing is signed ahead for a later transaction: the run needs no signature,
and a fee payment for it is signed only once it is due.

### Running

```text
run = H("app/call-v1/run", "") ‖ account ‖ u64 sequence
```

Anyone may run a scheduled operation once the block timestamp reaches
`execute_at`. The program reverts `not scheduled` for an operation that is
not, and `not due` before its time. Running does the operation for the
account with the checks of that moment (the account must still hold what it
gives away, for instance), deletes the scheduled record and emits *executed*.
A run that fails reverts, and the operation stays scheduled until it is
cancelled.

Whoever schedules an operation should hand its run to a node at the same
time, as a transaction deferred until `execute_at`
([deferred transactions](../node/mining.md#deferred-transactions)), so that nobody has to come back
for it while blocks have room. A run pays no miner itself, like any call
without a payment, so when blocks are full it waits until someone pays for
it: whoever wants it in, the owner or the account an operation gives
something to, puts it in a batch with a fee payment bound to the batch after
it ([paying for a transaction](../programs/conventions.md#paying-for-a-transaction)). That payment is
signed when the run is due, never when the operation is scheduled: a payment
signed ahead would hold its account's next coin nonce, and with it every other
payment from that account, for the whole delay.

**The same run is the same transaction.** A run carries no signature and no
nonce, so every run of an operation with the same budget is byte for byte the
same transaction, with the same TransactionId ([blocks, transactions and receipts](../data/blocks.md)). That merges
the runs different people hand to their nodes into one. But a run that went
into a block and failed, sent too early for instance, is the same transaction
as a later try: peers refuse to relay it while one of the last 10 blocks
includes it, and a lookup of its id lists every occurrence, the failed one
included ([the mempool](../node/mining.md#the-mempool)). A wallet that sends a run again after
one failed should change its budget, which gives it a new TransactionId, and
must read every occurrence of a transaction, never only the first. A cancel
signed again with the same sequences and deadline is the same transaction
too.

### Cancelling

```text
cancel  = H("app/call-v1/cancel", "") ‖ policy(32) ‖ Bytes(params) ‖ u64 valid_until ‖ Bytes(witness)
        ‖ u64 count ‖ count × u64 sequence
message = Bytes("app/call-v1") ‖ ChainId ‖ program ‖ account ‖ 02 ‖ u64 valid_until ‖ u64 count ‖ count × u64 sequence
```

A cancel follows the coin's rules for cancelling ([cancelling](../programs/coin.md#cancelling)):

- It carries no nonce and leaves the account's nonce alone. Each operation can
  be cancelled once, so a cancel cannot be replayed, and a thief who spends the
  account's nonces cannot make the owner's signed cancel stale.
- The sequences are strictly increasing, and each must be below the account's
  nonce. Those that already ran or were cancelled are skipped; at least one
  must still be scheduled, or the cancel reverts `not scheduled`.
- `valid_until` is checked as for a call, and the policy is called as for a
  call's approval. `01` and `01 ‖ u64 d` are both accepted: a cancel is never
  delayed.
- One authorization cancels any number of operations, so a thief who schedules
  many cannot outpace the owner.
- Each cancelled operation's scheduled record is deleted and *cancelled*
  emitted.

### Events

```text
scheduled = H("app/call-v1/event/scheduled", "") ‖ account ‖ u64 sequence ‖ u64 execute_at ‖ T ‖ values
executed  = H("app/call-v1/event/executed", "")  ‖ account ‖ u64 sequence ‖ operation
cancelled = H("app/call-v1/event/cancelled", "") ‖ account ‖ u64 sequence
```

- *scheduled* tells a wallet that one of its user's accounts has an operation
  waiting, even one it never saw the link of: the case that matters most is
  the owner's wallet when a thief holding one of the keys scheduled it. The
  wallet finds the template for `T` in the program's
  [template list](#records). `values` runs to the end of the event.
- *executed* is emitted whenever an operation takes effect, at once or when
  it runs. A page that made a link knows its `operation`, and finds it done in
  the program's events. For a program's account, `sequence` is 0.

### Other inputs

Besides calls, runs and cancels, a program answers the one-byte query `00`
while it runs a payment ([payments](#payments)). It may take inputs of its
own as well, such as reads, as long as it tells them apart from these: a call
starts with one of its template hashes, and a run and a cancel with their own
fixed hashes, which no template hash equals.

## Payments

When an operation takes the user's coins, the call carries a payment tied to
that one approved operation:

```text
payments   = u8 count ‖ count × payment                count 0 or 1 in this version
payment    = coin(32) ‖ Bytes(coin input)
coin input = 10 ‖ 00 ‖ policy(32) ‖ Bytes(params) ‖ u64 nonce ‖ Bytes(witness) ‖ action
action     = effect ‖ fee u128 ‖ tip u128 ‖ u64 not_before ‖ u64 valid_until ‖ 01 ‖ program(32) ‖ digest
effect     = 01 ‖ to(32) ‖ amount u128     a transfer
           | 02 ‖ amount u128              a burn
```

A count above 1 is `bad input` in this version, to programs and wallets
alike; the count is there so that a later version can carry several payments,
in several coins, without another format.

`coin input` is the coin's authorized action, operation `10`, of a policy
account, as the coin reads it. Its last 65 bytes bind it
([bound actions](../programs/conventions.md#bound-actions)): the coin runs it only when
`program` calls the coin with it and, asked with the input `00`, answers with
exactly `digest`, the signing digest of the operation it pays for. A payment
is therefore good for one operation only: this account's, with this nonce and
deadline, in this program, with this template and these values.

**A payment as the approval.** The coin's message for the payment names the
paying account and holds `digest`, which names the acting account, its nonce,
the deadline and the operation. When the payer is the acting account, the
coin's check of the payment proves that the account approved exactly this
operation, and a check by the program would prove the same thing again. The
call's witness is then empty, and the call checks one signature, not two. An
account with a delay can never approve this way: the coin refuses its tied
payments.

For a call with a payment, the program must:

1. Refuse a `coin` it does not accept. It must never call a contract it does
   not know as a coin: a contract that only claims to be one could report
   success without paying anything.
2. Decode the coin input exactly as the coin will, and require that it is
   bound to the program itself and to `digest`. The payer is the payment's own
   policy account, `H("app/account-v1/policy", policy ‖ Bytes(params))` of the
   coin input.
3. Require the effect the operation needs. A template that shows
   `{payment}` takes a burn or a transfer, its amount from the payment, and
   for a transfer its recipient, and requires the payer to be the acting
   account ([the payment slot](#the-payment-slot)). A template that does not
   takes a [fee payment](#fee-payments) and nothing else.
4. Store `digest` as its answer to the query `00`, call the coin with the coin
   input unchanged, then remove that answer. If the coin call fails, revert.
   The program must answer `00` only while it runs a payment, and revert
   otherwise.

A payment always comes from an account without a delay: the coin refuses a
bound action from an account with one (`delayed binding`), and the program
refuses a payment for a delayed operation (step 4 of
[what the program does](#what-the-program-does)).

### Fee payments

An operation that moves no coins can still be approved by a payment, one that
moves nothing and carries only the fee:

```text
fee payment effect = 01 ‖ H("app/account-v1/contract", program) ‖ amount 0
```

a transfer of 0 from the acting account to the program's own account, with
the fee in the action's `fee` and the action bound to `digest` as any payment
is. Like a payment from the acting account, it is the account's approval, so
the call's witness is empty and the call checks one signature, and it pays the
miner without a batch.

- Every operation whose template does not show `{payment}` must accept either
  no payment or a fee payment from the acting account, so that a wallet can
  always use one. The effect is fixed exactly, so a program compares its
  bytes; any other payment for such an operation is refused.
- A program accepts a fee payment only in a coin it takes, as for any payment
  ([payments](#payments), step 1). A contract that only claims to be a coin
  would otherwise approve anyone. A link names that coin in `coin`
  ([the link](#the-link)).
- An account with a delay never makes one: the coin refuses its bound actions
  and its fees. Its operations are approved by its witness, and paid for from
  another account in a batch, or not at all. A call with a fee payment is
  therefore never scheduled.
- A wallet shows a fee payment as it shows a call without a payment: nothing
  leaves the account but the fee.

**Prices that follow the chain.** A payment set from the program's state,
such as a price that rises as pixels are painted over, is exact only for the
state the page read. A link carrying one is short-lived: someone else's call
can change the price before it goes in. A program may take more than the
price; its sentence then says where the excess goes, for instance "burning
{payment}".

The payment's fee pays the miner for the whole call, and a fee payment's
likewise. A call without either pays no miner itself, and neither does a run
or a cancel. Anyone may pay for
such a call by putting it in a batch, followed by a fee payment bound to the
batch after exactly the calls before it
([paying for a transaction](../programs/conventions.md#paying-for-a-transaction)): the payment is good
only with that call in front of it, and if the call fails, the batch reverts
and nothing is paid. The call should also be submitted alone, unpaid, so that
it does not depend on the payment.

## Records

A program keeps three records under fixed storage keys, so that a wallet can
read them with a proof ([proofs](../data/state-tree.md#proofs)):

```text
H("app/call-v1/templates", "")                       = u64 count ‖ count × Bytes(template)
H("app/call-v1/nonce", account)                      = u64 nonce
H("app/call-v1/scheduled", account ‖ u64 sequence)   = u64 execute_at ‖ T ‖ values
```

- **The template list** holds every template the program accepts, each once,
  and nothing else. The program writes it when it is deployed, and may build
  templates from its deployment's options, such as the names of its colors;
  it never changes the list afterwards. A wallet refuses a link whose template
  the list does not hold, before the user signs, and finds in it the template
  of a hash it meets on chain.
- **The nonce** of each policy account. An absent record means 0.
- **A scheduled operation**, from when it is scheduled until it runs or is
  cancelled. An absent record means none. `values` runs to the end of the
  record.

`H("app/call-v1/events", "")` is reserved for a later version, which may
describe a program's own events. A program must not use these keys for
anything else.

## The link

```text
ting:call?net=<8 hex>&program=<64 hex>&template=<text>[&values=<base64url> | &zvalues=<base64url>]
         [&coin=<64 hex>&burn=<amount> | &coin=<64 hex>&to=<64 hex>&amount=<amount>]
         [&from=<64 hex>] [&delayed=1] [&return=<address>]
```

| Field | Value | Meaning |
|---|---|---|
| `net` | 8 hex digits | The first 8 hex digits of the ChainId; a link for another network is refused |
| `program` | 64 hex digits | The program's ContractId |
| `template` | text, percent-encoded | The [template](#templates), declaration and sentence |
| `values` | base64url, optional | The [values](#values) |
| `zvalues` | base64url, optional | The values compressed, in place of `values` |
| `coin` | 64 hex digits, optional | The coin of the payment; without one, the coin the program takes [fee payments](#fee-payments) in |
| `burn` | amount, optional | The payment burns this amount |
| `to`, `amount` | 64 hex digits, amount; optional | The payment pays this amount to the account `to` |
| `from` | 64 hex digits, optional | The account that must act, for an operation only one account can do |
| `delayed` | `1`, optional | The program schedules this operation for accounts with a delay |
| `return` | address, percent-encoded, optional | A page the wallet offers to open once the call is sent |

- A link has `values` or `zvalues`, not both, or neither when there are no
  values.
- A link with a payment has `coin` and either `burn`, or `to` with `amount`.
  A link without one may have `coin` alone, naming the coin the program takes
  fee payments in, and none of the others. Amounts are greater than 0. A link
  whose template shows `{payment}` must have a payment, and a link with a
  payment must have such a template.
- An **amount** is decimal, in the coin's units: one or more digits,
  optionally followed by a dot and one or more digits, such as `12.5`, with no
  more decimals than the coin has. `12.`, `.5` and `12,5` are not amounts.
- **`values`** is base64url without padding (RFC 4648 §5), with no bits left
  over.
- **`zvalues`** is the values compressed with DEFLATE (RFC 1951), without
  zlib or gzip framing, then written as `values` is. The wallet must refuse it
  when it does not decompress exactly, with nothing after the compressed
  data, or when it unpacks to more than 65,536 bytes; it stops unpacking
  there. Compression only shortens the link: the hashes, the signature and
  the call use the values unpacked. A page should use it whenever it makes the
  link shorter.
- **`template`** and **`return`** are percent-encoded as JavaScript's
  `encodeURIComponent` does, and decoded as `decodeURIComponent` does: a link
  where that fails is refused.
- **`return`** must have an origin as [TOCS-1](tocs-1.md#origins) defines one,
  which allows https addresses, and http ones only on the user's own computer,
  and must hold no `#`. The wallet never opens it by itself: once the call is
  sent, it shows a button naming the address's host, and opens the address
  with `#ting-tx=` and the call's TransactionId in hex appended. The fragment
  never reaches a server; the page reads the transaction and its events from
  the chain. A link has no origin, so a `return` address is the page's claim
  like the rest of the link: the user chooses whether to follow it.
- `delayed=1` comes only on a link without a payment; `coin` alone is not
  one. A link for an owner's operation may carry `coin`, `from` and
  `delayed=1` together: the wallet then uses a fee payment for an account
  without a delay, and a witness for one with a delay.
- A link carries no account of the user's (except `from`), no nonce, fee or
  deadline, and no names or descriptions: the wallet fills in or looks up all
  of those. One link therefore works for every user, and can stay on a page
  or a poster.

Links are read as every [Ting link](../standards.md#ting-links) is: the scheme, the
request and field names without regard to case, a `//` after the scheme
tolerated, fields in any order, a field given twice making the link invalid,
unknown fields ignored. A link should stay under about 1,000 characters, so
that its QR code scans well from a screen; a link tapped on the phone itself
can be longer. An operation too large for one link is not split into several:
each would be approved and paid on its own, and part of it could go in
without the rest.

A wallet may open a call link tapped in another app: its screen does nothing
until the user confirms.

## The wallet

### Checking

The wallet must refuse a link when:

- a field is missing, malformed or given twice, or the values, payment or
  `return` fields break the rules above;
- `net` is not the chain it uses;
- the template breaks its [rules](#grammar), or the values do not decode
  exactly;
- the template shows `{payment}` and the link has no payment, or the link has
  a payment and the template does not show `{payment}`;
- `program` is not a contract on the chain, or runs the code of a reference
  program (the coin, the identity registry, the batch or a signature policy);
- the program's [template list](#records) is missing or malformed, does not
  hold the link's template, or holds another template with the same operation
  name and another declaration;
- `coin` is present, with a payment or alone, and does not run the coin
  reference program's code;
- `from` is present and is not one of the user's accounts.

### Showing

How the wallet lays out a call is up to it, but it must show three things
and keep them apart:

- **The program.** By name only when the wallet has a name for it: from a
  list its makers reviewed, by the program's CodeHash, or the name the user
  gave it on an earlier call. Otherwise as a program the wallet does not
  know, with its ContractId, and with a warning the first time. The link
  carries no name, and the wallet must not take one from the page.
- **The operation, in the program's words.** The sentence with the values
  filled in, marked as what the program says it does.
- **What the wallet checked itself.** The account that acts; what leaves it
  (an amount burned, or paid to whom); the fee, and the account that pays it;
  until when the signature can be used. For an account with a delay: when the
  operation will take effect, and that it can be cancelled until then.

Money is always the wallet's line. A sentence shows money as `{payment}`, the
payment itself, or as an `amount` field, which is the program's value: a
price for listing an item, a limit. If a sentence says "for 25 DLR" and the
payment is 250, the user reads 250 on the wallet's own line. The wallet may
point out when no `amount` in the sentence matches the payment, but this is
never a reason to refuse.

### Signing and submitting

The user chooses the acting account: any of theirs without a delay, or with
a delay when the link has `delayed=1`. For an identity's account, the policy is
the identity registry and the params `id ‖ u64 slot`: by the registry's
defaults, slot 0 is the spending account and slot 1 the time-locked savings
account ([identifiers and accounts](../programs/identity.md#identifiers-and-accounts)).

- `nonce` is the account's nonce in the program, read with a proof, after
  the account's own calls to the program that are still pending.
- `valid_until` should be at most one hour ahead, so that a call that did not
  get in soon does not happen later.
- **With a payment**, the payment comes from the acting account and the
  call's witness is empty: the payment is the approval. Its `nonce` is the
  account's nonce in the coin, after its own pending actions; `fee` comes from
  the wallet's fee settings; `tip` and `not_before` are 0; `valid_until` is
  the call's; the coin input's witness is the policy's, over the coin
  message's signing digest.
- **Without a payment**, when the link names a `coin`, the acting account has
  no delay, and the fee is paid by the acting account in that coin, the call
  carries a [fee payment](#fee-payments) and its witness is empty, as with a
  payment. Otherwise the call's witness is the policy's, over `digest`; for an
  identity's account it is `u64 key_id ‖ Bytes(signature)`. The wallet then pays the fee
  from one of the user's accounts without a delay, with a fee payment bound to
  a batch that runs the call ([payments](#payments)), or submits the call
  unpaid.
- A call that carries its payment or fee payment is submitted once. One paid
  through a batch is also submitted alone, unpaid.

The wallet submits the call to `program` with a budget of its choosing (a
budget is an upper bound, so a generous one costs nothing), and follows it as
it follows a payment, until a block includes it or `valid_until` passes. Until
then it must not sign another call to the program with the same nonce, nor
another payment with the same coin nonce (one payment per nonce,
[paying for a transaction](../programs/conventions.md#paying-for-a-transaction)). Once
the call is sent, it offers the link's `return` address, if any.

When it schedules an operation, it should hand the run to its nodes as a
deferred transaction. When one of the user's scheduled operations is due and
has not run, it should offer to pay for the run, and for any of the user's
calls that waits unpaid, signing the fee payment then. A run it sends again
after one failed gets another budget ([running](#running)).

When the acting account cannot pay, the wallet may offer to move coins to it
from one of the user's accounts with a delay, and ask about the call again
once they arrive. It must not sign the call ahead of time: a signed payment
would hold the account's next coin nonce, and with it every other payment
from that account, for the whole delay.

### Watching

The wallet watches every program's events for *scheduled* naming one of the
user's accounts, and shows each such operation, in the program's words, with
a way to cancel it, as it shows the coin's scheduled payments. An operation
the user did not make is the sign of a stolen key. The scheduled records let
it check, with a proof, which of them are still pending. It pays for a cancel
from an account a thief cannot sign for, not another account of the same
identity ([paying for a transaction](../programs/conventions.md#paying-for-a-transaction)), or submits it
unpaid.

**An operation that cannot run.** When a due operation's run reverts, the
wallet shows it as one that could not run, with the program's reason and a way
to cancel it, not as pending: what it needed may be gone for good, such as an
item its account no longer holds. A cancel clears it.

**Text read from the chain.** Nothing on chain checks the text rules: a
program's template list, the values of a scheduled operation, and any text a
program stores are whatever was written. When a template or a text that a
wallet or a page reads from the chain breaks the rules, or the values do not
decode as the template says, it must not show it as written. A wallet shows
such a scheduled operation as one it cannot display (the program, the template
hash, when it takes effect and the raw values) and still offers to cancel it;
it never hides a scheduled operation. A page shows such a text as one that
cannot be shown.

## Security

- **What an approval binds.** The chain, the program, the account, its nonce
  in the program, the deadline, the template and the values, whether the
  account approves with its witness over `digest` or with a payment bound to
  it. Changing any of them gives another digest.
- **Fee payments.** A fee payment moves nothing, and binds what any payment
  does. It approves only through a coin the program takes, so a contract that
  claims to be a coin can approve no one.
- **What a payment binds.** Everything the coin's message holds (the chain,
  the coin, the paying account, its nonce, what it pays, the fee and its
  deadline), and the digest of the one operation it pays for. It cannot pay
  for another operation, another instance of the same operation, or a call to
  another program.
- **The payment in the sentence.** `{payment}` is filled in from the payment
  the wallet checked, and a template that shows it is approved by that
  payment, so the amount a user reads in the sentence is the one their own
  signature covers.
- **Rewording.** `operation` includes the template's hash, a program accepts
  only its own templates, and a wallet refuses a link whose template is not in
  the program's list. A sentence the program does not have is never shown for
  signing.
- **Fake programs.** An approval names the program it is for, and so does a
  payment's binding. A fake program cannot act as the user in a real one. It
  can keep what was paid to it, the amount the wallet showed, and act
  elsewhere only as itself.
- **Program accounts.** Only the program whose account it is can act for it,
  by calling, and a program can act for no account but its own. No key and no
  link can act for a program's account.
- **What is at stake.** The wallet's own line, which no template can change:
  the amount and the fee, whatever the program is. What the user holds in the
  program is at stake as the program's own code decides.
- **No origin.** Nothing tells the wallet who made a link, and the wallet
  shows no site. Anyone can put any program's operation on any page. The user
  reads the program, by a name the wallet or the user gave it, and the
  sentence. A `return` address is the page's claim too: the wallet only offers
  it.
- **What the wallet cannot check.** The sentence is the program's claim: a
  malicious program can describe anything in honest words. That is what the
  reviewed list and the first-use warning are for. Nor can the wallet check
  what the user receives.
- **Mismatched values.** A program whose code reads values otherwise than its
  templates declare shows the user one thing and does another. Its author
  should test each template against the program.
- **Replay.** The program's nonce makes each approval good once, the coin's
  each payment, and their deadlines stop them from being used late.
- **Time locks.** A delayed answer means a scheduled operation that can be
  cancelled until it runs, or, in a program that does not schedule, no
  operation at all. A payment never comes from an account with a delay, so an
  operation approved by its payment always takes effect at once.
- **Activity kept apart.** A call without a payment touches no coin nonce, so
  a call that waits to get in holds up nothing else.
- **Hidden text.** Templates and text values follow TOCS-1's text rules,
  which refuse what could make a sentence read as something else. Text read
  from the chain is shown only when it keeps them.
- **Compressed values.** The wallet unpacks at most 65,536 bytes, so a short
  link cannot make it unpack without end.

## Not in this version

- **Several payments in one call**, in several coins, for a swap or a pool:
  the format has room for them ([payments](#payments)), and a later version
  may allow them.
- **Program accounts that pay** ([the call](#the-call)): the coin offers no
  action a program could make for its own account from inside another
  program's call.
- **Operations of two parties at once**, such as a trade both sign. Each
  party makes its own call; a program that settles between them binds each
  party's payment to the trade it is settling, as the coin's bound actions
  allow ([bound actions](../programs/conventions.md#bound-actions)).
- **Links for calls that need no approval**, such as a run, for a wallet to
  pay their fee: a page can submit such a call itself.
- **A program's own events** in its own words: the key is
  [reserved](#records).

## Example

A program, with the example ContractId
`765a9fa0c02d1ff7bbb1bf47ece78424c3d52627811999c874afa8895b4f7fd5`, keeps
items for accounts on the production chain and sells them for the coin
`305378d1…13c4`, whose symbol is DLR. Its
template list holds the two templates below. The acting account is the
example account `22…22`. Values and messages are spaced by field for reading.

**Give** item 7 to the account `11…11`, with nonce 0, until 1793000000:

```text
template   give(item:u64,to:account): Give item {item} to {to}
T          136b3686867293c0763efe081477ede6d5b6f4d4c92cbbf3744b9a1932b9b3e8
values     0700000000000000 1111111111111111111111111111111111111111111111111111111111111111
operation  b0ea3fb7ceacdd8246d27aba4b87381b1273b90fe58bb4d917d235ae9d805084
message    0b000000 6170702f63616c6c2d7631
           15d6549cfacd816f835c632b4c67308d1c63e642f4cbf4049368430670705107
           765a9fa0c02d1ff7bbb1bf47ece78424c3d52627811999c874afa8895b4f7fd5
           2222222222222222222222222222222222222222222222222222222222222222
           01 0000000000000000 4002df6a00000000
           b0ea3fb7ceacdd8246d27aba4b87381b1273b90fe58bb4d917d235ae9d805084
digest     2af209f9f7928bac0254bcf9e40c60d0f74fdf6f77de71df567c1a0db6450ee3
```

```text
ting:call?net=15d6549c&program=765a9fa0c02d1ff7bbb1bf47ece78424c3d52627811999c874afa8895b4f7fd5&template=give(item%3Au64%2Cto%3Aaccount)%3A%20Give%20item%20%7Bitem%7D%20to%20%7Bto%7D&values=BwAAAAAAAAAREREREREREREREREREREREREREREREREREREREREREQ
```

The same values compressed are `zvalues=Y2eAAEECAAA`, 11 characters in
place of 54. The wallet shows "Give item 7 to" the account, as a contact or
in full, and that nothing leaves the user's account but the fee. The call
carries no payment (`00`), so its witness is the account's signature over the
digest. Were the program to schedule it for accounts with a delay, the link
would end in `&delayed=1`. Were it to name its coin, `&coin=305378d1…13c4`, a
wallet paying its fee in that coin from `22…22` would instead carry a fee payment,
a transfer of 0 to the program's own account with the fee, and leave the
witness empty.

**Buy** item 7 for 25 DLR, paid to the seller `33…33`, with nonce 1, until
1793000000:

```text
template   buy(item:u64): Buy item {item} for {payment}
T          f7b7c40915e60615bd017682423d2b9b680faf3dc52d1748f8969cff8c22b2fe
values     0700000000000000
operation  806780c812e173d3c30d947e04c8a454a569d6998e1b5a19e92a3f36e95cead3
digest     65c295a6077b636553d933b06a3c21042cf9b366db00ac6680a924d388bca8e5
```

```text
ting:call?net=15d6549c&program=765a9fa0c02d1ff7bbb1bf47ece78424c3d52627811999c874afa8895b4f7fd5&template=buy(item%3Au64)%3A%20Buy%20item%20%7Bitem%7D%20for%20%7Bpayment%7D&values=BwAAAAAAAAA&coin=305378d17f6af84a563d0f8a8455084948aff7330b62b4f2f98a44b82ba113c4&to=3333333333333333333333333333333333333333333333333333333333333333&amount=25
```

The price is not among the values: the sentence shows the payment. The wallet
shows "Buy item 7 for 25 DLR", and on its own line that 25 DLR goes from the
user's account to `33…33`. The payment comes from `22…22` itself, so the
call's witness is empty: the payment, whose `within` is
`01 ‖ 765a9fa0…7fd5 ‖ 65c295a6…a8e5`, is the approval, and the coin's check of
it is the call's only signature check. The program requires the payment to
equal the item's price, and its payer to be `22…22`.

The fixed values:

| Hash | Value |
|---|---|
| `H("app/call-v1/templates", "")` | `5e5d1a485b7c790dcadf931959467bda642e5f8c3eeee945a96118174c033979` |
| `H("app/call-v1/events", "")`, reserved | `50207d6c72473695fffef8ac61aa353828d0926461c21a66996ea9ee79779fd1` |
| `H("app/call-v1/nonce", 22…22)` | `74a2fd2e9de861c7950a320ae9c7c93ad113aac470fce95024686743a3a2e77f` |
| `H("app/call-v1/scheduled", 22…22 ‖ u64 0)` | `5d42b569802462ea7155e42210db5ac79ef8f3f8d5963f986ac4b8f206b266a2` |
| `H("app/call-v1/run", "")` | `8381accdf4b5f2fda7f3211ce6165cf75c5d2c1c83d9363a6eff54cba06f9ebf` |
| `H("app/call-v1/cancel", "")` | `3f4e28d529bca5fb7098e7de868051f09d317a85db389bdef290706058b1df07` |
| `H("app/call-v1/event/scheduled", "")` | `87b120eab58d69a8ae3ace1fbac076394efc796c00464fcd19347c06291d2ec4` |
| `H("app/call-v1/event/executed", "")` | `5bc6659722a5f453391ecbdcc33d6226929eab184053f32e7f87863c6d07bcc2` |
| `H("app/call-v1/event/cancelled", "")` | `a87b8224d37e671d5a6793b35c1c8c6737e4a1dd7e2feeb683ba197652bc547e` |

## Test vectors

[`tcr-1-vectors.json`](../../../standards/tcr-1-vectors.json), served by every
node next to this page and kept in the `standards/` folder of the Ting
repository, was generated by a page's implementation of this standard, in
JavaScript, and is checked by a wallet's, in Kotlin, and a program's, in Rust:
three implementations of the same bytes. Integers are written in decimal, bytes in hex, and witnesses are made
up; a `refused` text is a reason for reading, not one an implementation must
give.

| Entry | What to check |
|---|---|
| `templates` | Each template's bytes, `T`, operation, declaration, fields and sentence |
| `refused_templates` | Templates every program and wallet must refuse |
| `values`, `refused_values` | Values with their decoding and operation, and values that must be refused |
| `operations` | The message and digest of an operation, for an account, nonce and deadline |
| `calls` | Call bytes for each kind of approval: a witness, a payment from the acting account (with its coin action, message, digest and input), a fee payment, and a program's account |
| `zvalues` | Compressed values that unpack, and ones to refuse: bytes after the end, cut short, zlib framing, past 65,536 bytes |
| `template_lists` | Lists that decode, and ones a wallet refuses: malformed, a template twice, an operation with two declarations |
| `canvas` | The deployment of an example program, a shared canvas, and the template list it stores, its paint template built from its colors |
| `scheduled`, `cancel` | A scheduled record and event, its run and *executed*, a cancel's message, digest and input, and *cancelled* |
| `links`, `refused_links` | Links and what they hold, and links every wallet must refuse |

## Versions

The tag `app/call-v1` and the tags under it are TCR-1. A later version changes
them together, so that no wallet or program reads one version's message, hash
or event as another's. A wallet refuses links it cannot read; it must never
guess.
