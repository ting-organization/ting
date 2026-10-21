# Signature policies

> [!NOTE]
> Reference programs, not part of the protocol: the chain runs them like any other
> deployment. See [Reference programs](../programs.md).

Three reference programs answer the
[policy interface](conventions.md#policy-interface) with ECDSA signatures:
`policy-secp256k1` checks one secp256k1 key, `policy-multisig-secp256k1` a threshold
of several, and `policy-p256` one P-256 key. Each is the rule behind
[policy accounts](conventions.md#policy-accounts): the account names the
policy and its params, here public keys, and a witness holding the right signatures
authorizes a message for it. The policies keep no state, emit no events and never ask
for a delay. A delay comes from a policy that consults them, such as an
[identity](identity.md).

## How they verify

All three policies:

- take the interface's input, and succeed with an empty result on empty input, which
  is how they are [deployed](../programs.md#canonical-deployment). Input that is not
  exactly three `Bytes` fields reverts with `bad input`;
- verify ECDSA over the
  [signing digest](conventions.md#signed-messages),
  `H("app/sign-v1", message)`, which serves directly as the 32-byte message hash: it
  is not hashed again;
- read public keys in SEC1 form: `02` or `03` followed by the 32-byte x coordinate
  (compressed, 33 bytes), or `04` followed by x and y (uncompressed, 65 bytes). The
  multisignature policy takes the uncompressed form only;
- read a signature as `r ‖ s`, 64 bytes: two 32-byte big-endian integers, each from
  1 to n − 1, where n is the order of the curve;
- accept only a low S, `s <= (n − 1) / 2`. Whenever `(r, s)` is a valid signature,
  so is `(r, n − s)`; requiring the low one leaves every signature a single form;
- return `01` when the signatures are valid and `00` otherwise. An invalid key, a
  wrong length, a zero or out-of-range scalar and a high S all return `00`; nothing
  in the params or the witness makes a policy revert.

The single-key policies also accept a third, compact form: `05` followed by x,
33 bytes, which names the point with that x and an even y on secp256k1, and the one
with the smaller of the two possible y on P-256. The point at infinity is never a
key. The forms of one key are different params, so each defines a different account.

## `policy-secp256k1`

| Part | Content |
|---|---|
| Params | A secp256k1 public key, 33 bytes compressed or compact, or 65 bytes uncompressed |
| Witness | One signature, `r` then `s`, 64 bytes, over the signing digest |

The result is `01` exactly when the witness is a valid low-S signature by the key.
High-S signatures, invalid keys, zero or out-of-range scalars and witnesses of any
other length return `00`.

## `policy-multisig-secp256k1`

A threshold of signatures by a list of keys:

```text
params  = u64(threshold) ‖ u64(count) ‖ key[count]
witness = entry[threshold]
entry   = u64(key_index) ‖ r ‖ s
```

| Field | Content |
|---|---|
| `threshold` | How many signatures are required, `1 <= threshold <= count` |
| `count`, `key` | The keys, each a 65-byte uncompressed SEC1 key, starting with `04`, in strictly increasing byte order |
| `key_index` | The position in the list of the key that made this signature, counting from zero |
| `r`, `s` | The signature, as for a single key |

The result is `01` exactly when:

- `1 <= threshold <= count`, the params hold exactly `count` keys with nothing after
  them, and every key is greater than the one before it, compared byte by byte;
- the witness holds exactly `threshold` entries with nothing after them;
- the indices are strictly increasing and below `count`, so no key signs twice;
- every key the witness names is an uncompressed key, and every listed signature is a
  valid low-S signature by its key over the signing digest. Keys the witness does not
  name are compared for their order but not checked as points.

Anything else returns `00`.

The keys are sorted, so no key is listed twice: a 2-of-3 account always needs two
different keys, and one set of keys with one threshold defines exactly one account.
Different keys need not have different holders, though: a key and its negation, the
point with the same x and the other y, have the secret keys `d` and `n − d`, so whoever
holds one holds both. A wallet that builds a joint account from other people's keys
checks that no key is another's negation.

## `policy-p256`

| Part | Content |
|---|---|
| Params | A P-256 (secp256r1) public key, 33 bytes compressed or compact, or 65 bytes uncompressed |
| Witness | One low-S signature, `r` then `s`, 64 bytes, over the signing digest |

The result is `01` exactly when the witness is a valid low-S signature by the key, and
`00` otherwise, by the same rules as `policy-secp256k1`.

P-256 is the curve of phone secure elements, such as the Secure Enclave and Android
StrongBox, and of WebAuthn authenticators. A phone becomes a key of an
[identity](identity.md) through this policy when its secure element signs
the signing digest itself, as the 32-byte hash of an ECDSA signature: the policy
verifies exactly that and nothing else. A WebAuthn assertion is a different signature.
In it the authenticator signs its own authenticator data together with a hash of the
client data, which merely contains the challenge, so an assertion is not a valid
witness even when its challenge is the signing digest. Hardware signers may also
return a high S; the wallet replaces it by `n − s`, which is just as valid, and submits
the signature as `r ‖ s`. The policy itself accepts only the low form.

## Costs

| Policy | Compute of one check |
|---|---:|
| `policy-secp256k1`, uncompressed key | about 12.6 million C |
| `policy-secp256k1`, compressed key | about 12.9 million C |
| `policy-multisig-secp256k1`, 2 of 3 | about 21.0 million C |
| `policy-p256` | about 17.5 million C |

About 4 million C of each figure is the policy's code price, which a transaction pays
only the first time it runs the policy
([the cost of an invocation](conventions.md#the-cost-of-an-invocation)).
A compressed key costs more because recovering its y coordinate takes a square root.
The multisignature policy costs roughly one single-key verification per required
signature on top of its code. P-256's field and point arithmetic is generic, where
the secp256k1 policies' is specialized for their curve. Its program, though, is built
with [wide arithmetic](../vm/instructions.md#wide-arithmetic), which turns its 64-bit
multiplications into single instructions, while the secp256k1 code works in 32-bit
pieces. At the production network's initial 450,000,000 C per block
([production profile](../consensus/genesis.md#production-profile)), 25 P-256
verifications fit in one block. Like every invocation, each check also reads 2 S.
