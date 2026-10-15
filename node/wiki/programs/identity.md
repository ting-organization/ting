# Identity registry

> [!NOTE]
> A reference program, not part of the protocol: the chain runs it like any other
> deployment. See [Reference programs](/wiki/programs).

The `id-registry` program keeps identities. One deployment holds every identity on
a chain. An identity, or ID, is a stable name for a person or an organization that:

- signs with one or more **keys**, typically one per device, each a policy program
  and its parameters, so any signature scheme with a policy, post-quantum ones
  included, can serve as a key;
- owns **accounts**, called slots, each with its own time lock, such as an instant
  spending account and a time-locked savings account;
- is protected by **guardians**, other IDs that can replace lost keys;
- changes its own configuration only after a delay, during which it can cancel the
  change.

The registry is itself a policy
([policy interface](/wiki/programs/conventions#policy-interface)). Every application
that uses the shared account model therefore accepts ID accounts with no changes, and
an ID account's address stays the same when its keys rotate or are recovered. The
registry's application tag is `app/id-v1`.

The registry sets no limit on the number or size of keys, guardians, slots, changes
or proposals, nor on how deeply identities nest. Counters and identifiers are `u64`,
and costs grow with what an ID chooses to hold. Single bytes such as `01` and `28`
are written in hex; `H`, `Bytes(x)`, `‖` and the other notation are those of the
[conventions](/wiki/programs/conventions#encoding).

## Trust model

- **Guardians.** More than half of an ID's guardians do not collude against its
  owner. Under that assumption a majority of guardians prevails: the owner's keys
  cannot block a recovery. A dishonest guardian can still delay one: an approval it
  adds and later withdraws restarts the recovery delay of a proposal that had
  reached its threshold, once per proposal ([recovery](#recovery)). Honest guardians
  therefore all approve, rather than stopping at the threshold, and the windows
  below count only their approvals.
- **Keys.** Any one key can authorize. A stolen key that can still be used signs like
  its owner; time locks limit the damage to the owner's instant accounts, as long as
  someone cancels what the thief schedules: the owner with another key, or a new key
  after a recovery. Without guardians, an owner who keeps a key and a thief cancel
  each other's withdrawals and changes, and the time-locked funds stay where they
  are for as long as both keep doing so.
- **Time.** Delays are measured in block timestamps
  ([timestamps](/wiki/consensus/difficulty#timestamps)). A block may be dated a few
  minutes before the time of the blocks before it and, by the nodes' clock policy,
  at most two hours ahead. A miner can therefore make a due time arrive up to about
  two hours early, or a threshold count from about two hours late. Delays well above
  two hours tolerate that, and a wallet cancels at least two hours before an action
  is due. Time arithmetic saturates at `2^64 − 1`.
- **Windows.** An owner who still holds a key cancels a thief's withdrawals and
  changes directly. Otherwise, counted from the thief's first action:
  1. The guardians reach their threshold before any change the thief proposes takes
     effect, `change_delay` after the proposal: reaching the threshold voids every
     pending change ([recovery](#recovery)).
  2. The recovery is executed when due, `recovery_delay` after the threshold, and
     before `change_delay` after it: a change the thief proposes after the threshold
     falls due then, and could be applied first.
  3. A time-locked slot keeps its funds only if the recovery falls due before the
     thief's withdrawals do: the threshold must be reached within the slot's delay
     minus `recovery_delay`, within 5 days with the defaults. The new key then
     cancels the withdrawals in the same transaction as the recovery, through a
     [batch](/wiki/programs/batch).

  Each bound is strict, since a run or an apply in the same block may come first, and
  timestamps shift it by up to about two hours. The registry enforces only
  `recovery_delay < change_delay`; wallets warn about short margins and hand the
  recovery to a node as a deferred transaction for its due time.
- **A threshold below a majority** trusts fewer guardians than the default. Any group
  that reaches it can recover the ID, and by reaching it on any proposal also voids
  the owner's pending changes; a single guardian with a threshold of 1 can do so
  again and again, and so voids every change that would remove it.
- **Approvals stay.** An approval counts until its guardian withdraws it or the
  recovery epoch advances, which every key, guardian, threshold and delay change
  does. A guardian approves one proposal at a time and withdraws its approval before
  approving another ([recovery](#recovery)). An abandoned proposal still counts the
  approvals of guardians who have approved nothing since, so a few more approvals
  complete it, even from a minority, and executing it voids every other proposal; one
  that reached its threshold can be executed by anyone. Every guardian therefore
  withdraws its approval of an abandoned recovery, which takes effect at once. The
  owner also retires it: any of these changes retires all open proposals at once,
  even one that sets the same value. That change waits `change_delay`, though, and a
  proposal that reaches its threshold meanwhile voids it, so the withdrawals are what
  protect the owner.

## Identifiers and accounts

```text
id      = H("app/id-v1/id", creation input)
params  = id ‖ u64 slot                                         40 bytes
account = H("app/account-v1/policy", registry ‖ Bytes(params))
```

The creation input is the create operation's input after its op byte
([creation input](#creation-input)). `registry` is the registry's ContractId
([identifiers](/wiki/data/records#identifiers)), which the canonical deployment makes
the same for everyone on a chain ([Reference programs](/wiki/programs)). An ID account
is an ordinary policy account: its policy is the registry, and its parameters name
the ID and a slot.

The ID is known before it is created, so it can be shared and can receive funds
first. Anyone may submit the creation, a sponsor for example, but only with exactly
that configuration, so nobody else can claim the ID.

Each slot is an account with its own delay. With the defaults, slot 0 is the spending
account, with no delay, and slot 1 the savings account, which is time-locked. Moving
funds into savings is instant. Everything out of savings is scheduled, including
transfers to the owner's own spending account, so a thief cannot empty savings by
moving the funds to spending first ([coin](/wiki/programs/coin)). The account of a
slot the ID does not have authorizes nothing until a change adds that slot.

## Configuration and defaults

Every setting is chosen at creation. The defaults are recommendations, which wallets
apply unless the user chooses otherwise. The registry enforces only the logical rules
in the last column.

| Setting | Default | Rule |
|---|---|---|
| Keys | One per device | At least one |
| Guardians | Chosen by the user | Distinct IDs, never the ID itself; at creation listed in increasing byte order |
| `threshold` | 0, meaning more than half of the guardians | 0, or at most the number of guardians; wallets warn below a majority |
| `change_delay` | 604,800 s (7 days) | Greater than `recovery_delay` |
| `recovery_delay` | 172,800 s (2 days) | Less than `change_delay` |
| Slot delays | `[0, 604,800]`: spending, savings | At least one slot |

The rule `recovery_delay < change_delay` lets guardians restore the owner before a
thief's configuration change takes effect.

## Records

The registry keeps everything in its own storage
([namespaces](/wiki/data/records#namespaces)), under logical keys derived from the ID:

```text
record(kind, id, index) = H("app/id-v1/record", u8 kind ‖ id ‖ index)
```

| Kind | Record | Index | Value |
|---:|---|---|---|
| 0 | Core | empty | The core record, below |
| 1 | Key | `u64 key_epoch ‖ u64 key_id` | `policy(32) ‖ params` |
| 2 | Guardian | guardian id (32) | `01` while the guardian is linked |
| 3 | Slot | `u64 slot` | `u64 delay` |
| 4 | Change | `u64 change_no` | `u64 change_epoch ‖ u64 proposer ‖ u64 proposed_at ‖ u64 effective_at ‖ change` |
| 5 | Proposal | proposal hash (32) | `u64 approvals ‖ u64 reached_at` |
| 6 | Approval | proposal hash (32) ‖ guardian id (32) | `01` approved, `02` withdrawn |
| 7 | Current approval | `u64 recovery_epoch ‖` guardian id (32) | The proposal hash the guardian approves in that recovery epoch |

A key's value is its policy's ContractId followed by the parameters as they are, with
no length prefix. A change's `proposer` is the `key_id` of the key that proposed it,
and `proposed_at` the block timestamp of the proposal. A guardian's current approval
exists while it approves a proposal of that recovery epoch, and is deleted when it
withdraws that approval in the same epoch. Records of earlier epochs stay and are
never read again.

### The core record

The core record is 97 bytes:

```text
u8  version = 1
u64 op_nonce         proposals, sequential
u64 change_epoch     advances on recovery and when a recovery reaches its threshold;
                     voids pending changes of older epochs
u64 recovery_epoch   advances on recovery and on key, guardian, threshold and
                     delay changes; voids older proposals
u64 key_epoch        advances on recovery; voids keys of older epochs
u64 next_key_id      identifier of the next added key
u64 key_count
u64 guardian_count
u64 slot_count
u64 threshold
u64 change_delay
u64 recovery_delay
u64 next_change      number of the next proposed change
```

An ID exists when its core record exists. Keys, guardians and slots each live in
their own records, so an ID can hold any number of them, and a policy check reads
only the key and the slot it uses. A record holds a value of any size
([values of any size](/wiki/data/records#values-of-any-size)), so a large
post-quantum key is one record. A key's identifier never changes while the key
exists, so a pending change can never target a different key than intended. Epochs
void old records without deleting them.

## Operations

The first input byte selects what the registry does:

- empty input does nothing and succeeds, which is how the registry is
  [deployed](/wiki/programs#canonical-deployment);
- `28` starts a policy check ([the registry as a policy](#the-registry-as-a-policy)):
  it is the first byte of `Bytes(params)` for 40-byte parameters;
- `01` to `08` select an operation below;
- any other first byte reverts `bad input`.

An operation's input is decoded exactly: a wrong length, an unknown version or
operation byte, or a trailing byte reverts `bad input`.

```text
key_auth = u64 key_id ‖ Bytes(witness)
key_list = u64 count ‖ count × (policy(32) ‖ Bytes(params))
```

| Op | Input after the op byte | Authorization | Effect | Result |
|---|---|---|---|---|
| `01` create | creation input | None | Creates the ID | The ID, 32 bytes |
| `02` propose | `id ‖ u64 op_nonce ‖ key_auth ‖ change` | A key of the ID | Schedules a [change](#changes) | `u64 change_no` |
| `03` cancel | `id ‖ key_auth ‖ u64 count ‖ count × u64 change_no` | A key of the ID, never delayed | Voids pending changes | Empty |
| `04` apply | `id ‖ u64 change_no` | Anyone, once due | Applies a pending change | Empty |
| `05` approve | `id ‖ guardian_id(32) ‖ key_list ‖ key_auth` | A key of that guardian | Approves a [recovery](#recovery) to `key_list` | Empty |
| `06` withdraw | `id ‖ proposal(32) ‖ guardian_id(32) ‖ key_auth` | A key of that guardian | Withdraws an approval | Empty |
| `07` recover | `id ‖ key_list` | Anyone, once approved and due | Replaces all keys | Empty |
| `08` info | `id` | None | Reads the ID | The core record |

Once their input decodes, operations `02` to `08` revert `unknown id` before any other
check when the ID does not exist. Info writes nothing, so other programs can call it
read-only; every other operation writes records or events and needs an ordinary call
([read-only mode](/wiki/vm/calls#read-only-mode)).

### Creation input

```text
u8  version = 1
key_list
u64 guardian_count    guardian_count × id(32)
u64 threshold
u64 change_delay
u64 recovery_delay
u64 slot_count        slot_count × u64 delay
salt(32)
```

The salt is any 32 bytes; it gives otherwise equal configurations different IDs.

The guardians are listed in strictly increasing byte order, so none is listed twice
and one set of guardians has one encoding. Creation reverts `bad input` when a rule
of [configuration and defaults](#configuration-and-defaults) fails, including
guardians out of order or repeated and a guardian equal to the ID, and `exists` when
the ID exists already.
Guardians need not exist yet; they must exist when they approve. The keys receive the
identifiers `0` to `count − 1` in key epoch 0, and `next_key_id` is `count`;
`op_nonce`, the three epochs and `next_change` start at 0. The registry writes the
records, emits Created and returns the ID.

### Key-signed operations

A key of the ID signs propose and cancel; a key of a guardian signs approve and
withdraw. The signer is key `key_id` of the signing ID in that ID's current key epoch.
The registry builds the message

```text
message = Bytes("app/id-v1") ‖ ChainId ‖ registry ContractId ‖ id ‖ u64 key_id ‖ body
body    = 02 ‖ u64 op_nonce ‖ change                    propose
        | 03 ‖ u64 count ‖ count × u64 change_no        cancel
        | 05 ‖ proposal(32) ‖ guardian_id               approve
        | 06 ‖ proposal(32) ‖ guardian_id               withdraw
```

taking the ChainId and its own ContractId from the [context](/wiki/vm/host#context).
`key_id` is the signing key's own identifier, so a signature names the key that made
it: where two keys of an ID verify the same signature, such as one device key listed
twice, nobody can present it as the other key's, which matters because a change
lives only as long as its proposing key. For approve and withdraw, `id` is the ID
being recovered, `guardian_id` the approving guardian and `key_id` that guardian's
key; for approve the registry computes `proposal` from the key list in the input
([recovery](#recovery)). The registry then calls the
key's policy read-only with

```text
Bytes(key params) ‖ Bytes(message) ‖ Bytes(witness)
```

and the child budget of the
[conventions](/wiki/programs/conventions#the-budget-reserve): its remaining budget
minus the standard reserve R ([budgets](/wiki/resources/budgets)). A signature policy
verifies the witness over the signing digest of the message
([signature policies](/wiki/programs/policies)). The check reverts, in this order:

- `no key` when the signing ID has no key `key_id` in its current key epoch;
- `insufficient budget` when the remaining budget is below R in any counter;
- `policy failed` when the call does not succeed, including host error 128;
- `not authorized` when the result is anything but `01` or `01 ‖ u64 d` with `d > 0`.

For propose, `op_nonce` must equal the stored value, checked before the key
(`bad nonce`), and it increments when the proposal succeeds. Cancels, approvals and
withdrawals carry no nonce. Each is single-use by itself: a change can be cancelled
only once and change numbers are never reused, and the approval record admits one
approval and one withdrawal per guardian and proposal. A thief who spends `op_nonce`
with proposals therefore cannot make an owner's signed cancel stale. A signed approval
stays valid until it succeeds or the recovery epoch advances, even one refused with
`approving another`, which anyone can submit once the guardian has withdrawn its
other approval. A wallet therefore never signs an approval while its guardian
supports another proposal. A key whose own
policy answers with a delay `d` pushes a proposed change to at least `now + d`;
cancels, approvals and withdrawals ignore key delays.

### Revert reasons

Every failure reverts with one of these ASCII strings as its result:

| Reason | Operations | When |
|---|---|---|
| `bad input` | All | The input does not decode exactly, the first byte is unknown, a creation breaks a configuration rule, a cancel's change numbers are not strictly increasing, an approval or a recovery lists no keys, or a policy check is not framed as three `Bytes` fields |
| `exists` | Create | The ID exists already |
| `unknown id` | `02` to `08` | The ID does not exist |
| `bad nonce` | Propose | `op_nonce` differs from the stored value |
| `no key` | `02`, `03`, `05`, `06` | The signing ID has no such key in its current key epoch |
| `insufficient budget` | `02`, `03`, `05`, `06` | The remaining budget is below R in some counter |
| `policy failed` | `02`, `03`, `05`, `06` | The call of the key's policy failed |
| `not authorized` | `02`, `03`, `05`, `06` | The key's policy did not authorize |
| `not pending` | Cancel, apply | Apply: the change is not [pending](#changes). Cancel: a listed number was never assigned, or none of the listed changes is pending |
| `not due` | Apply, recover | The waiting time has not passed |
| `not applicable` | Apply | An operation of the change cannot apply, or the result breaks a configuration rule |
| `not guardian` | Approve, withdraw | `guardian_id` is not linked to the ID |
| `unknown guardian` | Approve, withdraw | The guardian's ID does not exist |
| `already approved` | Approve | The guardian already approves this proposal |
| `withdrawn` | Approve | The guardian withdrew from this proposal |
| `approving another` | Approve | The guardian approves another proposal of the current recovery epoch |
| `not approving` | Withdraw | The guardian has no standing approval of this proposal, or the recovery has run |
| `not approved` | Recover | The proposal has not reached the required approvals |
| `counter exhausted` | Propose, approve, recover | A `u64` counter would overflow; in apply an overflow reverts `not applicable` instead |

A stored record the registry cannot decode reverts `corrupt`; the registry's own
writes never produce one. A policy check reverts only for input not framed as
`Bytes(params) ‖ Bytes(message) ‖ Bytes(witness)` (`bad input`); every other failure
of a check answers `00`, except for some parameters of another length than 40 bytes
([the registry as a policy](#the-registry-as-a-policy)).

## The registry as a policy

For an ID account the policy parameters are `id ‖ u64 slot` and the witness is a
`key_auth`. The registry answers by the
[policy interface](/wiki/programs/conventions#policy-interface):

1. It answers `00` unless the parameters are exactly 40 bytes, the ID exists and
   `slot < slot_count`.
2. It answers `00` unless the witness is exactly one `key_auth` naming a key of the
   current key epoch.
3. It answers `00` if its remaining budget is below R in any counter. Otherwise it
   calls the key's policy read-only with the child budget and
   `Bytes(key params) ‖ Bytes(message) ‖ Bytes(witness)`, passing the application's
   message on unchanged.
4. It answers `00` if that call fails, including host error 128, or if its result is
   anything but `01`, a key delay of 0, or `01 ‖ u64 d` with `d > 0`, a key delay
   of `d`.
5. It answers `01` if `max(slot delay, key delay)` is 0, and `01 ‖ u64` of that
   maximum otherwise.

Keys never see the slot or the ID: a signature is bound to one account only if the
application's message names the account, as the coin's does. The message is the
application's own, so a signature for one application cannot authorize another. The
registry's `op_nonce` plays no part here: replay protection for an application's
actions is the application's own.

The registry recognizes a policy check by its first byte alone, `28`, the low byte of
the length of 40-byte parameters. With parameters whose length has another low byte
the input is read as an operation instead, and the call reverts or returns a result
that is not a policy answer, where the
[policy interface](/wiki/programs/conventions#policy-interface) asks for `00`. The
caller is not authorized either way, since a failed call, like any result other than
`01` or `01 ‖ u64 d`, counts as not authorized.

### Nested identities

A key can itself be an ID account: its policy is the registry, and its parameters are
another ID's `id ‖ u64 slot`. IDs compose this way. An organization's key can be an
employee's ID, and an identity kept by another registry program can be a key of this
one, as long as it answers as a policy. The witness nests with the keys:

```text
witness = u64 key_id ‖ Bytes(u64 inner_key_id ‖ Bytes(inner witness))
```

The inner slot's delay becomes the key's delay, so a key that is another ID's savings
slot delays what is signed through it. Nesting has no depth limit. Each further level
adds one call, the registry checking the inner ID, and holds back one more R, so the
transaction budget bounds the depth.

The delay is that of the path the submitter presents. An application's message names
the account, not the key, so one signature passes through every key of the ID that
reaches its signer: if one of them has no delay, for example the inner ID's spending
slot next to its savings slot, a signature meant for the delayed key also passes
instantly. A key's delay protects only when no faster path reaches the same signer.
Cancels, approvals and withdrawals ignore key delays in any case. A nested key also
brings the inner ID into the outer one's trust: whoever holds or recovers the inner ID
holds the key.

## Changes

A change is a list of operations applied together:

```text
change = u64 count ‖ count × operation
```

| Operation | Encoding | Effect when applied |
|---|---|---|
| Add key | `01 ‖ policy(32) ‖ Bytes(params)` | Adds a key with identifier `next_key_id` in the current key epoch |
| Remove key | `02 ‖ u64 key_id` | Removes a key of the current key epoch |
| Add guardian | `03 ‖ id(32)` | Links a guardian |
| Remove guardian | `04 ‖ id(32)` | Unlinks a guardian |
| Set threshold | `05 ‖ u64 threshold` | 0 means more than half |
| Set delays | `06 ‖ u64 change_delay ‖ u64 recovery_delay` | Replaces both delays |
| Set slot | `07 ‖ u64 slot ‖ u64 delay` | Sets a slot's delay; `slot == slot_count` adds a slot |

No operation removes a slot.

**Proposing.** A change waits `change_delay`. Lowering the delay of a slot that exists
when the change is proposed also waits out that slot's current delay, and a key whose
policy answered with a delay `d` makes the change wait at least `d`. A change with
several operations waits for the longest:

```text
effective_at = now + max(change_delay, key delay,
                         current delay of every existing slot the change lowers)
```

The sum saturates. After `unknown id`, `bad nonce` and the key check, the registry
stores the change under the number `next_change` with the current `change_epoch`, the
proposing key's `key_id` as `proposer` and the block timestamp as `proposed_at`,
advances `next_change` and `op_nonce`, emits Change proposed and returns the change
number. Operations are decoded when proposed but checked only when applied.

**Pending.** A change is pending while its record exists, its `change_epoch` is the
current one and its proposer is still a key of the current key epoch. A change
therefore dies with the key that proposed it: removing a stolen key voids every
change the key proposed, including one that cannot apply today but could later, and
a recovery or a recovery reaching its threshold voids all of them
([recovery](#recovery)). The proposer is the ID's own key entry: a change proposed
through a [nested](#nested-identities) key lives as long as that entry, and removing a
device from the inner ID, or recovering the inner ID, leaves it pending.

**Cancelling.** Any key of the ID cancels pending changes at once, any number of them
with one signature, listed by strictly increasing number. After `unknown id`, cancel
reverts `not pending` when a listed number was never assigned, at least
`next_change`, or when none of the listed changes is pending, and then checks the
key. It deletes the record of each listed change that is pending, skipping the
others, and emits Change cancelled for each. A cancel carries no nonce and leaves
`op_nonce` unchanged.

**Applying.** Once due, anyone applies a change. Apply reverts `not pending` unless
the change is pending, then `not due` while `now < effective_at`, and also `not due`
while, for any slot that exists now and that the change lowers below its delay now,
`now < proposed_at + that delay`. The wait for lowering a slot is so measured against
the delay in force when the change applies: a slot the owner added, or raised, after
the change was proposed is waited out too, counted from the proposal. It runs the
operations in order, then
checks the configuration rules again: at least one key, a threshold of 0 or at most
the guardian count, and `recovery_delay < change_delay`. It reverts
`not applicable`, and the change stays pending for the owner to cancel, when:

- a remove key names no key of the current key epoch;
- an add guardian names the ID itself or a guardian already linked;
- a remove guardian names a guardian not linked;
- a set slot names a slot above `slot_count`;
- a counter would overflow;
- the result breaks a configuration rule.

Otherwise it deletes the change record, stores the core record and emits Change
applied. Each key, guardian, threshold and delay operation advances
`recovery_epoch`: a proposal names a whole key set and is approved under one recovery
configuration, and it never counts under another, so no old proposal can bring back
a key removed since. Apply needs no signature, so the apply transaction can be handed
to a node in advance as a [deferred transaction](/wiki/node/mining#deferred-transactions)
timed for `effective_at`.

Anyone chooses when, and in which order, due changes are applied. Since a change dies
with its proposing key, applying a change that removes a key first voids the changes
that key proposed and has not had applied. A wallet retires a key in the same change
as the changes it proposed, or after they apply, or proposes them again with a key
that stays.

A change stays pending, applicable or not, until it applies, is cancelled or is
voided. One that cannot apply when it falls due, such as a lowering of a slot that
does not exist yet, or a change that also removes a key not added yet, applies the
moment the owner's own change makes it applicable, and if it is older than the
slot's delay, with no further wait. Lowering a slot's delay so waits out the delay in
force from the proposal, and a thief cannot switch off savings without the owner
having had that long to see the change: wallets cancel every pending change they did
not propose, applicable or not.

Raising a delay applies after `change_delay`, and lowering it again waits out the
raised delay. A slot raised to `2^64 − 1` therefore never pays out again, even after
a recovery, and delays raised as far leave the ID unable to change or recover. Wallets
treat every raise, of a slot or of the ID's delays, as a change to confirm with care.

## Recovery

```text
proposal = H("app/id-v1/recovery", id ‖ u64 recovery_epoch ‖ key_list)
required = threshold if threshold > 0, else floor(guardian_count / 2) + 1
```

A proposal is the hash of a new key set for the ID in its current recovery epoch.
A guardian approves by submitting the key list itself: the registry computes the
proposal, the guardian's key signs it, and the Recovery approved event carries the
list, so the guardians' wallets and the owner's see which keys a recovery would
install. A withdrawal names the proposal by its hash.

1. **Initiate and approve.** Any linked guardian approves a proposal; the first
   approval initiates it. Each proposal has its own record, so proposals run in
   parallel and no guardian can reset another's. A guardian approves one proposal at
   a time: its approval counts for the proposal it currently supports and for no
   other, so a minority can never complete one proposal with approvals that the
   other guardians have since given to another. To change its mind, a guardian
   withdraws and then approves the other proposal, both in one
   [batch](/wiki/programs/batch) if it likes. After `unknown id`, approving reverts
   `bad input` for an empty key list, then `not guardian`, `unknown guardian` and the
   guardian's key check follow, and approving reverts `already approved` if the
   guardian approves already, `withdrawn` if it withdrew, and `approving another`
   while its current approval names another proposal of the current recovery epoch.
   Approving records the proposal as the guardian's current approval. Withdrawing
   reverts `not approving` without a standing approval, and deletes the current
   approval when it names the withdrawn proposal.
2. **Threshold.** An approval adds one to `approvals`. When that brings `approvals` to
   at least `required` while `reached_at` is 0, `reached_at` becomes the block
   timestamp and `change_epoch` advances, voiding every pending change. From then on
   no change, a thief's or any other, can take effect before the recovery is due, nor
   be applied to void it, since every change waits `change_delay`, longer than
   `recovery_delay`. A withdrawal marks the approval withdrawn and subtracts one;
   when that leaves fewer than `required`, `reached_at` returns to 0, and the next
   approval that reaches the threshold starts the recovery delay again. So guardians
   can stop a recovery they regret, and a dishonest guardian whose approval counted
   toward the threshold can delay it, once per proposal; the honest guardians'
   approvals beyond the threshold prevent that. A guardian that withdrew cannot
   approve the same proposal again. An approval emits Recovery approved, a withdrawal
   Recovery withdrawn.
3. **Execute.** Anyone submits the key list. The registry computes the proposal from
   it and the current `recovery_epoch`. After `unknown id`, it reverts `bad input`
   for an empty key list, `not approved` unless `reached_at` is set and `approvals`
   is at least `required`, and `not due` while `now < reached_at + recovery_delay`
   (saturating). It then advances `key_epoch`, stores the new keys with identifiers
   `0` to `count − 1`, sets `next_key_id` and `key_count` to `count`, advances
   `change_epoch` and `recovery_epoch`, voiding every pending change and every other
   proposal, deletes the proposal record and emits Recovered.

`required` follows the ID's settings at the moment of each approval, withdrawal and
execution; a change of keys, guardians, threshold or delays voids open proposals by
advancing `recovery_epoch`, so approvals are never counted under settings they were
not given under. A recovery leaves guardians and settings unchanged; the new keys can
change them with the usual delay. Execution needs no signature either, so it too can be
handed to a node as a deferred transaction.

The owner's keys cannot block a recovery ([trust model](#trust-model)). Guardians can
only replace keys; they never move funds themselves. An ID without guardians can never
be recovered.

## Events

Every event payload is a topic followed by the event's data. The emitting
ContractId identifies the registry.

| Event | Topic name | Data after the topic |
|---|---|---|
| Created | `created` | `id` |
| Change proposed | `change-proposed` | `id ‖ u64 change_no ‖ u64 effective_at ‖ change` |
| Change cancelled | `change-cancelled` | `id ‖ u64 change_no` |
| Change applied | `change-applied` | `id ‖ u64 change_no` |
| Recovery approved | `recovery-approved` | `id ‖ proposal(32) ‖ guardian_id ‖ u64 approvals ‖ u64 reached_at ‖ key_list` |
| Recovery withdrawn | `recovery-withdrawn` | `id ‖ proposal(32) ‖ guardian_id ‖ u64 approvals ‖ u64 reached_at` |
| Recovered | `recovered` | `id ‖ u64 key_epoch` |

The topic is `H("app/id-v1/event/<name>", "")`:

```text
created            96abd985fbe766b579404b96e9547834030ea1cfd2c60d4f2ccfbbbd7f707872
change-proposed    7ff11cafa49724d4278569c491f89105d0be81ad7cbf7e1d5b609390fd443a65
change-cancelled   b6d11f187b2c36bc35768a60bf6cb709d9c67341859129aac1a13ce7fddc74e7
change-applied     015a8ed72d902602eade12811f2e27bcf43bc0e10bdefb86a3d0f072c44d9136
recovery-approved  8d56fb8441fd29b7f2f23affbf37a0882198cf926d367ede840601b270e3b436
recovery-withdrawn a603fefdd0345f40399bab0e66d2c1b34890d9f13860533071ae4586996c2cce
recovered          c3295f0ba1c5ad2d10b876a48fafd0ff917e7e2599895ad5bcefe9c0607bce03
```

In Recovery approved and Recovery withdrawn, `approvals` and `reached_at` are the
proposal's values afterwards; a Recovery approved whose `reached_at` is the block's
timestamp marks the threshold being reached and every pending change voided.
Recovered carries the new key epoch.

Wallets alert owners on every change proposal and recovery approval for their IDs,
showing the keys a recovery would install, and alert guardians on proposals for the
IDs they agreed to protect. Anyone can name any ID as a guardian without its consent,
so a wallet alerts only for IDs its user accepted. The delays only protect owners who
are told.

## Signing anything off chain

The same keys sign logins, documents and statements off chain. Such a message starts
with `Bytes(app_tag)` like every policy message, and the signature is the key's
witness over it. Its signing digest is the same as an on-chain message's, so:

- the tag is never one of an application that runs on chain, such as `app/coin-v1`
  or `app/id-v1`: a signature over such a message would be valid there. Wallets
  refuse those tags off chain;
- after the tag, the message names the ChainId, the registry, the ID and the
  signing key's `key_id`, as the registry's own messages do. Anyone can add any
  public key to an ID of their own, so a statement that names no ID would verify for
  that ID too.

A verifier:

1. checks that the message names the ID and the key it is verified for;
2. reads the ID's core record and the key's record, `record(0, id, "")` and
   `record(1, id, u64 key_epoch ‖ u64 key_id)`, through a node's storage endpoint
   ([the REST API](/wiki/start/explore#the-rest-api)) at a recent canonical block;
3. checks that the key exists in the current key epoch;
4. verifies the witness, or simulates the key's policy;
5. records the block it checked, since keys can change later.

Wallets display what they sign and warn on unknown application tags.

## Security walkthrough

| Event | Outcome |
|---|---|
| Device lost | Guardians approve new keys; after 2 days the owner has them back. |
| Device stolen, key usable | The thief can spend the instant account. Everything out of savings is scheduled for 7 days and pays nothing at once, not even a fee. An owner with another key cancels the withdrawals. Otherwise the guardians reach their threshold within 5 days of the first withdrawal, the recovery falls due 2 days later, and the new key cancels the withdrawals in the same transaction. |
| The thief splits savings into many small withdrawals | One cancel, with one signature, voids any number of them: about 330 in one transaction at 1,024 S per block, and more with further cancels, one signature each. |
| The thief spends nonces to make the owner's cancels stale | Cancels carry no nonce, in the coin and in the registry. |
| A thief proposes to remove guardians or lower the savings delay | It takes at least 7 days; the owner with another key cancels it, and the guardians reaching their threshold voids it. |
| The owner removes a stolen key | Every change that key proposed dies with it, and so does every open recovery proposal, which could otherwise bring the key back. The removal itself waits the change delay, and a thief who still holds the key can cancel it, so against an active thief the guardians decide. |
| A minority of guardians acts maliciously | It cannot reach the threshold alone. Each guardian approves one proposal at a time, so the minority cannot combine its approvals with honest guardians' approvals that now support another proposal, and cannot void or reset the honest guardians' proposal, as long as guardians withdraw their approvals of abandoned recoveries. An approval it adds and later withdraws can delay a recovery once per proposal; when all honest guardians approve, it delays nothing. |
| The ID has no guardians | Nobody can recover it. An owner who keeps a key and a thief cancel each other's withdrawals and changes, so the time-locked funds stay locked for both. |
| A key scheme becomes vulnerable to quantum computers | The owner adds a post-quantum key and removes the old one; the ID and its accounts are unchanged. |

## Costs

Typical costs per transaction, rounded. Signature verification dominates every signed
operation ([signature policies](/wiki/programs/policies)). Each program a transaction
runs also pays its code price once, about 1.2 million C for the registry
([the cost of an invocation](/wiki/programs/conventions#the-cost-of-an-invocation)).
The transfers are made with the [coin](/wiki/programs/coin).

| Operation | C | S |
|---|---:|---:|
| Coin transfer from an ID account, secp256k1 key | about 15.6 million: the transfer from a secp256k1 account plus about 1.4 million for the registry (its code, its call, record hashing, and the core, key and slot records) | 17 |
| Coin transfer from an ID account, P-256 key | about 20.5 million | 17 |
| Coin transfer through a nested ID, one whose key is another ID's slot | about 15.8 million | 22 |
| Create, with 2 keys, 3 guardians and 2 slots | about 1.85 million, plus 598 G for the records | 19 |
| Propose a change | about 14.2 million | 10 |
| Cancel a change | about 14.1 million | 10 |
| Recovery approval | about 14.4 million | 17 |
| Recover with one new key | about 1.56 million | 10 |

A large post-quantum key adds C at the [byte price](/wiki/resources/prices#memory-tiers),
not S.

## Not included

The registry deliberately has no on-chain trust links between IDs (a public trust
graph), no human-readable names, no hidden guardians and no emergency freeze.
Statements between people can be signed off chain
([signing anything off chain](#signing-anything-off-chain)). Per-key delays need no
feature of their own: a key whose policy answers with a delay, such as another ID's
savings slot, delays what is signed through it, as long as no faster key reaches the
same signer ([nested identities](#nested-identities)).
