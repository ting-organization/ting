# Application standards

> [!IMPORTANT]
> **Not part of the protocol.** A standard is an agreement between programs, wallets,
> pages and sites. Consensus does not know it, no node checks it, and a program that
> follows one is run like any other. Anyone may agree on other formats for the same
> purposes. These are published so that one wallet works with every program and
> every site that follows them, without knowing any of them.

The [reference programs](programs.md) give applications shared accounts, signature
checks, identities and coins. What they leave open is how a person is asked: how a
page puts an operation of some program in front of a user, how the user's wallet
shows it in words it can stand behind, and what exactly the user's key then signs.
Left to each application, every program would need its own wallet, and every wallet
would sign bytes it cannot explain.

The standards settle that once:

| Standard | Where it acts | What it is for |
|---|---|---|
| [TCR-1: Ting Call Requests](standards/tcr-1.md) | On chain, in a program | A page asks a user to do an operation in a program, such as buying an item or giving one away. The wallet shows the operation in the program's own words and what leaves the account, signs, and submits the call; the program checks that the account approved exactly that |
| [TOCS-1: Ting Off-Chain Signing](standards/tocs-1.md) | Off chain, for a site | A site asks a user to sign in, to grant a permission or to sign a statement. The wallet shows the request with the site's origin and signs; the site checks the answer against the account's keys on chain |

Each page is the whole standard: a program, a wallet or a site can be written from
it and its test vectors alone.

## What they build on

Both standards use the reference programs'
[conventions](programs/conventions.md), and add nothing to the chain:

- **Accounts.** The one who acts or signs is an
  [account](programs/conventions.md#accounts): a key under a
  [signature policy](programs/policies.md), or a slot of an
  [identity](programs/identity.md). TCR-1 takes any policy account, and a
  program's own account; TOCS-1 covers accounts of a single key and of an identity.
- **Signed messages.** Every message starts with its own application tag and is
  signed over the same [signing digest](programs/conventions.md#signed-messages).
  TCR-1's messages start with `app/call-v1` and TOCS-1's with `app/offchain-v1`, so a
  signature made under one can never be presented under the other, to a coin or to
  the identity registry. A TOCS-1 signature is valid for nothing on chain.
- **Payments.** A TCR-1 operation that takes coins carries a payment in the
  [coin](programs/coin.md), a [bound action](programs/conventions.md#bound-actions)
  tied to that one operation. The wallet checks and shows the payment itself, and
  the program can rely on it.
- **Time locks.** An account with a
  [delay](programs/conventions.md#delayed-authorization) keeps it in every program
  that follows TCR-1: the program schedules its operations, visible to its owner's
  wallet and cancellable until they run, or refuses them. It never carries them out
  at once.
- **Fees.** An operation that pays nothing itself is paid for as any such call is
  ([paying for a transaction](programs/conventions.md#paying-for-a-transaction)).

## Ting links

A request reaches the wallet as a Ting link. The same text works as a QR code to
scan, as a link to tap on the phone that holds the wallet, and as text to copy and
paste:

```text
ting:<request>?<field>=<value>&…
```

| Request | Standard | Asks |
|---|---|---|
| `call` | [TCR-1](standards/tcr-1.md#the-link) | Do an operation of a program, in the program's own words |
| `sign` | [TOCS-1](standards/tocs-1.md#sign-codes) | Fetch a site's signing request from the address in `url`, and sign it |

A wallet reads every Ting link the same way:

- The scheme, the request and the field names are read without regard to case, and
  a `//` after the scheme is tolerated.
- Fields may come in any order. A field given twice makes the link invalid, and
  unknown fields are ignored.
- A request the wallet does not know is shown as one it does not know, never
  guessed at.
- Nothing happens until the user confirms on the screen the link opens.

A link carries no secret and proves nothing about who made it: anyone can show any
link anywhere. What protects the user is what the wallet checks against the chain
and shows before it signs, which each standard states.

## Test vectors

Each standard has a file of test vectors: inputs with the exact bytes, hashes and
answers they give, and inputs every implementation must refuse. Every node that
serves this wiki serves them next to it:

| Path | Content |
|---|---|
| [`/wiki/standards/tcr-1-vectors.json`](../../standards/tcr-1-vectors.json) | Templates, values, operations, calls, links, scheduled operations and cancels of TCR-1 |
| [`/wiki/standards/tocs-1-vectors.json`](../../standards/tocs-1-vectors.json) | Documents, origins, sign codes, requests and answers of TOCS-1 |

The same files are in the `standards/` folder of the Ting repository.

## A standard and its versions

A standard is named by its letters and its number, and its tags carry that number:
`app/call-v1` and the tags under it are TCR-1, `app/offchain-v1` is TOCS-1. The
formats of a standard do not change. Formats that differ are another standard, with
another number and other tags, so that no wallet, program or site ever reads a
message of one as a message of another. A wallet refuses what it cannot read; it
never guesses.
