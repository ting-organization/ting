# Reference programs

> [!IMPORTANT]
> **Not part of the protocol.** The reference programs are ordinary deployments.
> Consensus does not know them, no node treats them differently from any other
> program, and a chain is complete without them. Anyone may deploy other programs
> for the same purposes. These are published so that applications, wallets and
> miners can share one implementation of the most common needs.

Ting has [no native coin or accounts](/wiki/decisions/no-native-coin), and
[cryptography lives in programs](/wiki/decisions/crypto-in-programs). The reference
programs fill those roles with ordinary WebAssembly: signature checks, an identity
registry, a coin anyone can issue, a hash function, and a batch of calls that lets
one account pay for another's operation. They share a small set of
[conventions](/wiki/programs/conventions), so one account works with every coin, one
wallet can sign for every application, and a miner can rank fees from any of them.

## The programs

| Program | What it does | Bytes | CodeHash |
|---|---|---|---|
| [`policy-secp256k1`](/wiki/programs/policies#policy-secp256k1) | Authorizes with one secp256k1 key | 68,890 | `492e5470e3384a0bfa7cdfcf496c2e374a314325ce24327c75670f17e9201dcf` |
| [`policy-multisig-secp256k1`](/wiki/programs/policies#policy-multisig-secp256k1) | Authorizes with k of n secp256k1 keys | 69,291 | `b3ccde2a6007ee8b71c1f9c642c6713ce789918a6ae42197b6b40ad72e33cee0` |
| [`policy-p256`](/wiki/programs/policies#policy-p256) | Authorizes with one P-256 key, such as a phone's secure element | 63,879 | `f065bf6d04a34ecbbeb94fb91571ccb2c08c6070cffd2ea4df948b46cda5ef06` |
| [`id-registry`](/wiki/programs/identity) | Identities with several keys, guardians, delays and recovery | 19,849 | `edea54d81ee9c28319bb5f0aea7aacbe891654e358f6a5cbe7225078ba7396d3` |
| [`coin`](/wiki/programs/coin) | One program for every coin: supply, transfers, fees, scheduling, mining rewards | 21,460 | `c675a4b753b61b50ae4b63cde122171f8f2bfb9a05a92df895633ca8c351e4d6` |
| [`sha256`](/wiki/programs/sha256) | SHA-256 of any input, for programs that call rather than link it | 2,474 | `1c3f95c74b7f77b462e892d29482dbc7e08a690999a4bd45f59fc846c7bc2a85` |
| [`batch`](/wiki/programs/batch) | Several calls in one transaction, all or none, so one account can pay for another's operation | 4,669 | `68118bab6c4f9c9a88147a25cbc2327a89b50ea997633e26bfe84e9563438a6e` |

The CodeHash is `H("code", bytes)`, the hash every node records for an installed
program ([Contracts and records](/wiki/data/records#identifiers)). It identifies the
program exactly: a deployment whose CodeHash matches one above runs exactly the
behaviour these pages describe.

## The programs are frozen

The reference programs never change. Their bytes, and so their CodeHashes, are fixed,
and on a frozen protocol their behaviour, their errors and their costs are fixed
with them. Installed code is immutable on Ting in any case
([upgrades through programs](/wiki/applications/upgrades)): a program with different
rules is a different program, with a different CodeHash, and never a new version of
one of these. The pages of this section describe the programs as they are, and an
application can rely on every rule they state.

## Getting the bytes

Every node that serves this wiki serves the programs next to it:

| Path | Content |
|---|---|
| [`/wiki/programs/policy-secp256k1.wasm`](/wiki/programs/policy-secp256k1.wasm) | `policy-secp256k1` |
| [`/wiki/programs/policy-multisig-secp256k1.wasm`](/wiki/programs/policy-multisig-secp256k1.wasm) | `policy-multisig-secp256k1` |
| [`/wiki/programs/policy-p256.wasm`](/wiki/programs/policy-p256.wasm) | `policy-p256` |
| [`/wiki/programs/id-registry.wasm`](/wiki/programs/id-registry.wasm) | `id-registry` |
| [`/wiki/programs/coin.wasm`](/wiki/programs/coin.wasm) | `coin` |
| [`/wiki/programs/sha256.wasm`](/wiki/programs/sha256.wasm) | `sha256` |
| [`/wiki/programs/batch.wasm`](/wiki/programs/batch.wasm) | `batch` |
| [`/wiki/programs/manifest.json`](/wiki/programs/manifest.json) | Each program's `name`, size in `bytes` and `code_hash`, in lower-case hex |

The same files are in the `programs/` folder of the Ting repository. A copy obtained
anywhere else is genuine exactly when its CodeHash matches.

## Canonical deployment

Every reference program except the coin is deployed once per chain, by a top-level
deployment transaction with empty input and this budget:

```text
budget = [50,000,000 C, 64 S, 16,777,216 D, 16,777,216 G]
```

On a given chain that transaction is the same for everyone, and so is its
ContractId, `H("contract", ChainId ‖ TransactionId ‖ u64(0))`. A wallet derives the
identifiers of the signature policies and the identity registry this way, with no
list of addresses to trust, and an account defined through a policy (see
[accounts](/wiki/programs/conventions#accounts)) means the same on every node. Another
deployment of the same bytes is valid too, but has another ContractId, and so other
accounts. A program that must accept any deployment of a known program compares its
CodeHash instead, with the [`code_hash`](/wiki/vm/host#code-hashes) host import.

The coin is the exception because every coin is its own deployment of the coin
program, with its own name, supply and options in the deployment input. Its
ContractId identifies the coin; its CodeHash shows that it follows the
[coin](/wiki/programs/coin) rules.
