# What applications build

Ting supplies ordering, deterministic execution, isolated storage, calls between
contracts, events and commitments. Programs supply the meaning.

Balances, owners, permissions, signatures, fees and versions do not exist in the
protocol. A program defines them in its own storage and enforces them in its own code:

- A payment application defines balances, and how transfers are authorized.
- A registry defines ownership, and how entries are updated.
- A shared service defines an input format and lets other contracts call it.

None of these needs a change to the transaction format, and none gets special
treatment from the chain. The protocol has no built-in signature verification,
balance, transfer, mint, gas price or fee hook. New signature verifiers, hash
libraries, proof systems, assets and application rules arrive as deployments, through
ordinary transactions, rather than as protocol changes.

Miner compensation follows the same pattern. Each block's body carries producer data
chosen by its miner, committed by the header and committed in turn by the proof of
work. Programs can read it as context and give it meaning, for example as the
destination of their own miner compensation. Ordinary application calls implement
such compensation and any reward claims; the protocol never calls a privileged reward
program on its own. Producer data does not prove that a particular person or public
key mined the block.

This section covers the patterns every application needs and the assumptions it must
not make. The most common applications (coins, identities and signature checks) exist
as frozen [reference programs](programs.md) that any application can build on.
