# TOCS-1: Ting Off-Chain Signing

> [!NOTE]
> An application standard, not part of the protocol: an agreement between wallets
> and sites, which the chain knows nothing of. See
> [Application standards](../standards.md).

TOCS-1 is how a site asks a Ting user to sign something off chain (to sign
in, to grant a permission, or to sign a statement) and how the site checks
the answer against the chain. Nothing in it touches the chain, and no
signature made under it is valid on chain.

This page is the whole standard: a wallet or a site can be written from it
and its [test vectors](#test-vectors), which every implementation should pass.
[TCR-1](tcr-1.md) acts on chain in a program; TOCS-1 signs things off chain
for a site. Their messages can never be taken for each other's.

**Must**, **must not** and **should** are requirements: an implementation that
breaks a *must* is not TOCS-1.

## Roles and flow

- The **site** wants a signature. It stores a request, shows a code, receives
  the answer and checks it.
- The **wallet** holds the user's keys. It fetches the request,
  shows it in its own words, and signs only once the user approves.

1. The site stores a request at an address of its own and shows its **sign
   code**, `ting:sign?url=<percent-encoded address>`, as a QR code to scan with
   a phone or as a link that opens the wallet on the same device.
2. The wallet fetches the address. The request's **origin** is the origin of
   the address it fetched, never anything the request says.
3. The wallet checks the request, shows it, and lets the user approve or
   decline.
4. The wallet POSTs its **answer**, as JSON, to the request's `callback`, on
   the same origin. The site answers `{ "message": "…" }`, which the wallet
   shows; a status other than 2xx is a failure, and its `message` says why.
5. On a phone, when the request has a `return` address, the wallet opens it
   after answering, which brings the user back to the site's page. The page
   that showed the code learns the outcome from its own server, which received
   the answer.

## Notation

- `‖` joins byte strings. Integers are unsigned and little-endian: `u8`,
  `u32`, `u64`.
- `Bytes(x) = u32(len(x)) ‖ x`.
- Text is UTF-8. `"abc"` in a byte layout means its UTF-8 bytes.
- Hex is lowercase, two digits a byte, without `0x`.
- **H**, Ting's domain-separated hash ([domain-separated hashing](../data/encoding.md#domain-separated-hashing)):

  ```text
  H(tag, x) = SHA-256("Ting/" ‖ u16(len(tag)) ‖ tag ‖ x)
  ```

  `len(tag)` counts the tag's bytes, as a `u16` little-endian.
  `H("app/sign-v1", "")` is
  `cc686e2d6d364e6d22ed5f6b6199e3531a0cee095560c55329c13a17bc00be25`.
- **Accounts** ([accounts](../programs/conventions.md#accounts)). A Ting account is the hash of the
  policy contract that authorizes it and that policy's parameters:

  ```text
  account = H("app/account-v1/policy", policy ‖ Bytes(params))
  ```

- **Signatures.** ECDSA over a 32-byte digest used as is (no further hashing),
  on secp256k1 or P-256, written `r ‖ s` (64 bytes, each big-endian), with
  `s ≤ n/2` (low S). A message is signed by signing its digest:

  ```text
  digest = H("app/sign-v1", message)
  ```

- **Times** are Unix seconds.

## Sign codes

```text
ting:sign?url=<percent-encoded request address>
```

The address is percent-encoded as JavaScript's `encodeURIComponent` does:
UTF-8, every byte except `A–Z a–z 0–9 - _ . ! ~ * ' ( )` as `%XX`. A wallet:

- reads `ting:` and `sign` without regard to case, and tolerates
  `ting://sign?url=`;
- decodes the rest as `decodeURIComponent` does, and refuses a code where that
  fails (a stray `%`, or bytes that are not UTF-8);
- refuses a code whose address has no [origin](#origins).

The sign code is one of the [Ting links](../standards.md#ting-links); a link
tapped in another app may open the wallet with it.

## Origins

The origin of an address is `scheme://host`, with `:port` when the port is
not the scheme's default. Only addresses of this exact shape have one:

```text
^([a-zA-Z]+)://([a-zA-Z0-9.-]+|\[[0-9a-fA-F:.]+\])(:[0-9]{1,5})?([/?#][\x21-\x7e]*)?$
```

and of those only:

- `https` addresses; and `http` addresses whose host is `localhost`,
  `127.0.0.1` or `[::1]`, for sites in development;
- with a port, if any, from 1 to 65535.

The origin is the scheme and host in lower case, then `:port` unless the port
is 443 for `https` or 80 for `http`.

So an address of anything but printable ASCII has no origin: a name in other
letters could pass for another site's (`bаnk.example` with a Cyrillic а), and
URL parsers disagree on how to turn it into ASCII. Nor does one with a user or
password (`https://user@host`), or with a percent-encoded host. A site with an
internationalized name uses its ASCII (`xn--`) form, which the wallet shows as
it is.

| Address | Origin |
|---|---|
| `https://forum.example/api/ting/requests/abc` | `https://forum.example` |
| `https://Forum.Example:8443/x?y` | `https://forum.example:8443` |
| `https://forum.example:443/x` | `https://forum.example` |
| `http://localhost:3000/a` | `http://localhost:3000` |
| `http://forum.example/a` | none: plain http elsewhere |
| `https://user:pw@forum.example/` | none |
| `https://%D0%B0pple.com/` | none |
| `https://bаnk.example/req` (Cyrillic а) | none |

## The request

The wallet fetches the address with `GET` and `Accept: application/json`. It
sends no cookies and keeps none, so that one site's answers cannot follow the
user to another, and it does not follow redirects: the site that answers is
the site at the address. The site serves:

```json
{
  "type": "ting-sign-request",
  "version": 1,
  "chain": "15d6549cfacd816f835c632b4c67308d1c63e642f4cbf4049368430670705107",
  "nonce": "0303030303030303030303030303030303030303030303030303030303030303",
  "issued_at": 1792000000,
  "expires_at": 1792000300,
  "signer": "7185932296cf9d0ef6e00291ba09f8a052c850d2ebe7f9275608e10d87d2b3cf",
  "document": "010100d2496b00000000…",
  "callback": "https://forum.example/api/ting/requests/abc/answer",
  "return": "https://forum.example/sign-in",
  "file": "https://forum.example/files/agreement.pdf"
}
```

| Field | Type | Meaning |
|---|---|---|
| `type` | text | `ting-sign-request` |
| `version` | number | `1` |
| `chain` | 64 hex digits | The ChainId the signature is for |
| `nonce` | 32 to 128 hex digits | 16 to 64 bytes the site chose at random, used once |
| `issued_at` | whole number | When the site made the request |
| `expires_at` | whole number | When the site stops accepting an answer |
| `signer` | 64 hex digits, optional | The ID that must sign, for a returning user |
| `document` | hex | [The document](#the-document): what the user reads |
| `callback` | address | Where the answer goes; same origin |
| `return` | address, optional | Where a phone goes back to after answering; same origin |
| `file` | address | Where the file a statement covers is; same origin. Present exactly when the statement covers a file |

Whole numbers are JSON numbers written as plain digits, without a sign,
fraction or exponent, from 0 to 2^53 − 1. Unknown fields are ignored. With `now` the wallet's clock, the wallet must
refuse a request when:

- `type` or `version` is not as above, or any field is missing or of another type;
- the address it fetched has no origin;
- `expires_at ≤ now` (expired);
- `issued_at > now + 300`, `expires_at ≤ issued_at`, or
  `expires_at − issued_at > 86400` (a request lives a day at most);
- `signer` is present and not 64 hex digits;
- `callback`, or `return` when present, has no origin or another origin than
  the request;
- the document does not decode;
- the statement covers a file and `file` is missing or on another origin, or
  `file` is present and the document is not a statement covering a file.

The wallet should also refuse a request for a chain it does not use, and one
whose `signer` names an ID the user does not have.

## The document

What the user reads, in bytes. It is the only content the wallet shows as the
site's: the request's other fields are checked, not shown.

```text
document = u8 version (1) ‖ u8 kind ‖ kind data
         ‖ Bytes(title) ‖ Bytes(body) ‖ u32 n ‖ n × field
field    = Bytes(label) ‖ Bytes(value)
key      = u8 0                          no key
         | u8 1 ‖ Bytes(public key)      a P-256 public key: 65 bytes, 04 ‖ x ‖ y
```

| Kind | Kind data | Asks |
|---|---|---|
| 1 sign in | `u64 session_until ‖ key` | Sign in to the site until `session_until`; with a key, in the browser that holds it |
| 2 grant | `u64 until ‖ u32 n ‖ n × Bytes(scope) ‖ key` | Allow the site the scopes until `until`; with a key, through the holder of that key |
| 3 statement | `u8 0`, or `u8 1 ‖ hash(32) ‖ Bytes(media type) ‖ Bytes(name)` | Sign the title, body and fields as a statement; with `1`, covering the file whose SHA-256 is `hash` |

The **key** of a sign-in or a grant is one the site asks the user to vouch
for, typically a key the browser made for the session, so that the browser can
prove later that the user signed in through it.

- A **scope** is 1 to 64 characters: `^[a-z0-9][a-z0-9:._/-]{0,63}$`.
- A **media type** is `type/subtype` in lower case:
  `^[a-z0-9!#$&^_.+-]{1,63}/[a-z0-9!#$&^_.+-]{1,63}$`.
- A **field** is a detail the site adds, as a label and a value.

### Text

Every text (title, body, scope, media type, attachment name, label, value)
must be:

- valid UTF-8 in its shortest form: no overlong encodings, no encoded
  surrogates;
- in Unicode Normalization Form C;
- free of these code points, which can hide or reorder what is shown:

  | Code points | What they are |
  |---|---|
  | U+0000–U+001F, except U+000A in the body | Controls |
  | U+007F–U+009F | Delete, C1 controls |
  | U+00AD | Soft hyphen |
  | U+034F | Combining grapheme joiner |
  | U+061C | Arabic letter mark |
  | U+115F, U+1160, U+3164, U+FFA0 | Hangul fillers |
  | U+180E | Mongolian vowel separator |
  | U+200B–U+200F | Zero-width space, joiners, direction marks |
  | U+2028–U+202E | Line and paragraph separators, direction embeddings and overrides |
  | U+2060–U+2069 | Word joiner, invisible operators, direction isolates |
  | U+D800–U+DFFF | Surrogates |
  | U+FDD0–U+FDEF, and every U+xxFFFE and U+xxFFFF | Noncharacters |
  | U+FEFF | Zero-width no-break space |
  | U+E0000–U+E007F | Tags |

The title, each attachment name, each label and each value must also be a
**line**: not empty, without U+000A, and not starting or ending with a
character of the Unicode `White_Space` property.

### Decoding

Decoding is exact. The wallet must refuse the whole document, never skip a
part, when it finds:

- a version other than 1, an unknown kind, an attachment marker other than 0
  or 1, a key marker other than 0 or 1, or a key that is not 65 bytes starting
  with `04`;
- a scope or media type that does not match its pattern, or text that breaks
  the rules above;
- fewer bytes than a length or count calls for, or any byte after the last field;
- more than 65,536 bytes in all.

`refused_documents` in the vectors holds one document for each of these.

## Showing a document

How the wallet presents a request is up to it, but it must state what the
request does in its own words, from the kind and its data, with the origin it
fetched from, and show the title, body and fields as the site's. For
example:

| Kind | The wallet says |
|---|---|
| Sign in | Sign in to *origin* (on this browser, when a key is given) until *session_until* |
| Grant | Allow *origin* to *scopes* as *user* until *until* |
| Statement | Sign this statement for *origin* (covering the attached file) |

Fields are shown exactly as written; the wallet gives neither a label nor a
value any meaning of its own. What a value says is the site's claim.

For a sign-in, the wallet should show the **match code**, two digits the site
shows too, so that the user can see the request is the one on their screen:

```text
h = H("app/offchain-v1/match", nonce)
match code = (h[0] · 256 + h[1]) mod 100, as two digits
```

A statement covering a file: the wallet fetches `file` when the user opens
it, under the same rules as the request (no cookies, no redirects), keeps it
only if its SHA-256 is `hash`, shows it, and lets the user sign only after
opening it.

### The body

The body is laid out from a few marks; nothing is loaded from the network.
Any other text is shown as written.

1. Split the body into chunks at each line feed, any number of spaces
   (U+0020), and another line feed.
2. In each chunk, drop the lines that are empty or only white space. A chunk
   without lines is left out.
3. If every line of a chunk starts with `- `, it is a bulleted list of the
   lines after `- `. Otherwise, if every line starts with one to four ASCII
   digits, `.` and a space, it is a numbered list, each item numbered as
   written. Otherwise the chunk is a paragraph, whose lines are kept apart.
4. Within each line, or list item, emphasis is found from left to right. At
   each `*` not yet taken, try `**X**`, then `*X*`, with the shortest `X`
   that is not empty and starts and ends with a character that is not white
   space; `X` is shown bold or italic, and the search goes on after the match.
   A `*` where neither matches is shown as written. Marks do not nest. A line
   with more than 200 `*` is shown as written, so that a line full of marks
   cannot make the search slow.

`bodies` in the vectors holds bodies and the blocks they give.

## The signed message

The wallet builds the message itself; a site never supplies the bytes that are
signed.

```text
message = Bytes("app/offchain-v1") ‖ chain ‖ account ‖ Bytes(origin) ‖ Bytes(nonce)
        ‖ u64 issued_at ‖ u64 expires_at ‖ H("app/offchain-v1/document", document)
```

- `chain`, `nonce`, `issued_at` and `expires_at` are the request's; `nonce`
  is its bytes, not its hex.
- `origin` is the origin of the address the wallet fetched, as text.
- `account` is the account that signs. For an identity it is the ID's
  spending account, slot 0 ([below](#identity-accounts)).

It is signed as every Ting application message is, over
`H("app/sign-v1", message)`. Its tag, `app/offchain-v1`, keeps it apart from
every on-chain message, each of which starts with its own application's tag,
so an answer can never authorize anything on chain. The origin in the message
makes a signature for one site worthless to another.

## The answer

The wallet POSTs JSON with `Content-Type: application/json` to `callback`:

```json
{
  "type": "ting-sign-answer",
  "version": 1,
  "nonce": "<the request's nonce, hex>",
  "account": "<32 bytes, hex>",
  "policy": "<32 bytes, hex>",
  "params": "<hex>",
  "witness": "<hex>"
}
```

`policy`, `params` and `witness` are what the account's policy checks on
chain. When the user declines, the wallet POSTs:

```json
{ "type": "ting-sign-answer", "version": 1, "nonce": "<hex>", "declined": true }
```

so that the site stops waiting. The site answers either with
`{ "message": "<one line for the user>" }`: a 2xx status for an answer it
accepted, any other for one it refused, with the reason. The wallet shows the
message marked as the site's, never as its own.

### Identity accounts

An identity is an ID in the [identity registry](../programs/identity.md),
with accounts in slots: slot 0 is its spending account. Its keys are numbered (`key_id`), and recoveries and key changes replace the
set of current keys by moving to a new key epoch.

```text
policy  = the identity registry's ContractId
params  = id ‖ u64 slot                                   40 bytes
witness = u64 key_id ‖ Bytes(signature)
```

The signature is made with the ID's key `key_id`.

### Plain accounts

An account of a single key uses a key policy:

```text
policy  = the secp256k1 or P-256 key policy's ContractId
params  = the 65-byte uncompressed public key, 04 ‖ x ‖ y
witness = the 64-byte signature
```

### Contract IDs

The registry and the key policies are Ting's reference programs. On any
chain their ContractIds follow from the canonical deployment of their code
([canonical deployment](../programs.md#canonical-deployment)). On the production chain,
`15d6549cfacd816f835c632b4c67308d1c63e642f4cbf4049368430670705107`:

| Program | ContractId |
|---|---|
| Identity registry | `1185ab4a07f12fd03e130acfd51211cd9360ec94b89e5f742bbd8f52767714bd` |
| secp256k1 key policy | `9558a75642eb797de5ec1be056f01c62d99c492c8f7ec6efed460767588987c9` |
| P-256 key policy | `35d2b107de4b2a738a8190be18ccfd35d9bac45da094cfa4281a3a4b178852b3` |

## Checking an answer

The site checks an answer against the request **as it stored it**, never the
request data an answer could bring, and against the chain's current records.
With `now` the site's clock:

1. `nonce` is the stored request's, `now ≤ expires_at`, and the nonce has not
   been answered before. The site then marks it used, so that an answer
   counts once.
2. `account = H("app/account-v1/policy", policy ‖ Bytes(params))`.
3. The message is rebuilt from the stored request, the site's own origin and
   `account`, and `digest = H("app/sign-v1", message)`.
4. When `policy` is the identity registry:
   1. `params` is 40 bytes: `id`, then `u64 slot`.
   2. Read the ID's **core record**. It must exist and `slot < slot_count`.
   3. Read `witness` as `u64 key_id ‖ Bytes(signature)`, with nothing after it.
   4. Read the **key record** of `key_id` in the core's `key_epoch`. It must
      exist, so a key a change removed or a recovery replaced fails. It is
      `policy(32) ‖ params`: the policy must be the secp256k1 or P-256 key
      policy, and `params` its public key.
   5. The signature must be valid for that key over `digest`.
5. When `policy` is the secp256k1 or P-256 key policy: `witness` must be a
   valid signature by the key in `params` over `digest`.
6. Anything else, such as a multisig policy or an ID nested in an ID, is
   refused: TOCS-1 does not cover it.

A valid signature here is a valid ECDSA signature with low S, as defined in
[Notation](#notation), by a public key that is a point on its curve.

### Reading the records

Records are storage of the registry contract, under these keys:

```text
core key = H("app/id-v1/record", u8 0 ‖ id)
key key  = H("app/id-v1/record", u8 1 ‖ id ‖ u64 key_epoch ‖ u64 key_id)
```

The core record is 97 bytes:

```text
u8 version (1) ‖ u64 op_nonce ‖ u64 change_epoch ‖ u64 recovery_epoch ‖ u64 key_epoch
‖ u64 next_key_id ‖ u64 key_count ‖ u64 guardian_count ‖ u64 slot_count ‖ u64 threshold
‖ u64 change_delay ‖ u64 recovery_delay ‖ u64 next_change
```

A Ting node serves a record at
`GET /v1/contracts/{contract hex}/storage/{key hex}`
([the REST API](../start/explore.md#the-rest-api)), as `"value_hex": "…"`, or
`"value_hex": null` when there is none. Its answer is only as
good as the node: a site should ask a node it runs or trusts over a secure
connection, or check the record's proof against a block header
(`POST /v1/storage` with `"proof": true`,
[proofs](../data/state-tree.md#proofs)).

## Security

- **What a signature binds.** The chain, the account, the origin the wallet
  fetched from, the nonce, the request's times and the document. Changing any
  of them gives another message.
- **What it cannot do.** It is not valid for anything on chain: no on-chain
  message starts with `app/offchain-v1`. It moves no coins and changes no
  identity.
- **Phishing.** The origin is where the wallet fetched the request, and the
  answer can only go back to that origin. A site cannot make the wallet sign
  for another site's origin. Its title and fields are its own claims; the
  wallet's own words say what the signature does.
- **Hidden text.** The text rules refuse what could make a document read as
  something else: direction overrides, invisible characters, unnormalized
  text, look-alike hosts.
- **Replay.** The nonce is used once and the request expires within a day.
  The site must store requests and accept each nonce once.
- **Clocks.** The wallet refuses requests from the future beyond five minutes
  and requests living longer than a day; the site enforces `expires_at` with
  its own clock.
- **Transport.** Only https, except for a site on the user's own computer.
  No cookies, no redirects.
- **Keys removed or recovered.** A site checks the key against the ID's
  current key epoch at the time of the check, so a stolen key that a
  recovery replaced stops working for new sign-ins at once.
- **Sessions.** A sign-in with a key binds the session to that key; the site
  should require proof of it, such as a signature by the browser, for the
  session's requests.

## Test vectors

[`tocs-1-vectors.json`](../../../standards/tocs-1-vectors.json), served by
every node next to this page and kept in the `standards/` folder of the Ting
repository, holds:

| Entry | What to check |
|---|---|
| `canonical` | The production chain's ContractIds |
| `documents` | Each document's fields, bytes and `H("app/offchain-v1/document", …)`: encoding gives the bytes, decoding gives the fields |
| `refused_documents` | Documents every wallet must refuse, with the reason; also: one of 65,537 bytes |
| `origins` | Addresses and their origins (`null` for none) |
| `sign_codes` | Codes and the address each holds (`null` for one to refuse) |
| `match_codes` | Nonces and their match codes |
| `request` | A request, the address it was fetched from and the time: it is accepted, with this origin |
| `identity_answer` | For the first document and that request: an ID's records, the account, message, digest, the RFC 6979 signature by `secret_key`, the witness and the answer JSON. A site that holds these records accepts the answer |
| `plain_answers` | The same for a secp256k1 and a P-256 key account |
| `decline` | A declining answer |
| `bodies` | Bodies and the blocks they lay out as |

Secret keys in the vectors are for tests only.

## Versions

`version` 1 in the request, the answer and the document, and the tag
`app/offchain-v1`, are TOCS-1. A later version changes the version numbers
and the tag together, so a wallet never signs a message one version would
read as another's. A wallet refuses versions it does not know; it must never
guess.
