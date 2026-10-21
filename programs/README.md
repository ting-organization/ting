# Reference programs

The deployable modules of Ting's reference programs, byte for byte. They are not
part of the protocol: they are ordinary programs, published so that applications,
wallets and miners share one implementation of coins, identities and signature
checks. They are frozen; these bytes do not change.

| File | Program |
|---|---|
| `policy-secp256k1.wasm` | Authorizes with one secp256k1 key |
| `policy-multisig-secp256k1.wasm` | Authorizes with k of n secp256k1 keys |
| `policy-p256.wasm` | Authorizes with one P-256 key |
| `id-registry.wasm` | Identities with several keys, guardians, delays and recovery |
| `coin.wasm` | One program for every coin |
| `sha256.wasm` | SHA-256 of any input |
| `batch.wasm` | Several calls in one transaction, all or none |
| `manifest.json` | Each program's `name`, size in `bytes` and `code_hash` |

What each program does, its inputs, results, records and events, and how it is
deployed are described in the wiki's [Reference programs](../node/wiki/programs.md)
section, which every node also serves at `/wiki/programs`, together with these files.
