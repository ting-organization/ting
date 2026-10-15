# Glossary

| Term | Meaning |
|---|---|
| Accumulated work | The sum of work along a branch. Nodes follow the valid branch with the most. [More](/wiki/consensus/proof-of-work) |
| BlockId | Domain-separated SHA-256 hash of a block header. It must not exceed the header's target. |
| Budget | The maximum C, S, D and G a transaction, or a nested call, may consume. [More](/wiki/resources/budgets) |
| C / S / D / G | The four counters: compute, state access, data and growth. [More](/wiki/resources/counters) |
| Call | A transaction kind, and a host function, that runs an installed program with input. |
| Call error 128 | What `call` and `deploy` return when the requested sub-budget does not fit. No child is created. |
| Capacity | The active per-block limit for each counter. It moves only by capacity voting. [More](/wiki/resources/capacity) |
| ChainId | Hash of the canonical genesis configuration. It identifies the network. [More](/wiki/consensus/genesis) |
| CodeHash | Hash of a program's original code bytes. |
| Context | Read-only values a program can query: ChainId, TransactionId, contract IDs, height, timestamp, parent BlockId, producer data and its remaining budget. [More](/wiki/vm/host) |
| ContractId | A program's identifier, derived from the ChainId, the TransactionId and the deployment ordinal. [More](/wiki/data/records) |
| Deploy | A transaction kind, and a host function, that installs code and runs its first invocation. [More](/wiki/vm/lifecycle) |
| Deployment ordinal | A per-transaction counter of deployment attempts. It never goes back, even when a deployment fails. |
| Epoch | A span of blocks over which capacity votes are counted. |
| Event | Opaque bytes emitted by a program, tagged with its ContractId and kept in the receipt if the invocation and every caller around it succeed. [More](/wiki/data/blocks#events) |
| Genesis | The fixed configuration a network starts from, and the height-0 anchor derived from it. |
| Growth | Gross new persistent allocation, measured by G. [More](/wiki/resources/growth) |
| Host interface | The twelve functions a program imports from module `ting`. [More](/wiki/vm/host) |
| Invocation | One run of a program, with a fresh instance, its own checkpoint and its own result. |
| Manifest | The record holding a program's CodeHash and code length. |
| Mempool | A node's set of pending transactions. Its rules are node policy. |
| Namespace | The kind of a record: 0 manifest, 1 code, 2 application storage. |
| Occurrence | One inclusion of a transaction in a block. An ID can have several. |
| Producer data | Bytes of any length in each block body, chosen by the miner, committed by the header and readable by programs. |
| Profile | The exact subset of WebAssembly that nodes accept. [More](/wiki/vm/profile) |
| Reference program | A program published with Ting for a common need, such as a coin, an identity or a signature check. It is an ordinary deployment, not part of the protocol. [More](/wiki/programs) |
| Read-only | An invocation mode that forbids storage writes, deployments and events, inherited by every nested call. |
| Receipt | A transaction's status, resources used, result bytes and retained events. |
| Record | One entry in the state: a 32-byte key and a value of any length. Empty and absent differ. |
| Price list | The measured C price of every instruction, byte, page, hash, program start and code preparation; one C is about one dependent integer addition. [More](/wiki/resources/prices) |
| Memory tier | The band of an instance's memory size that sets the price of its loads, stores and bytes. [More](/wiki/resources/prices#memory-tiers) |
| Reorganization | A switch of the canonical branch to a competing branch with more work. |
| State root | The root of the sparse Merkle tree over all records. [More](/wiki/data/state-tree) |
| Status | The outcome code in a receipt, from 0 (success) to 11. [More](/wiki/data/blocks) |
| Target | The largest allowed BlockId value. A smaller target means harder mining. |
| TransactionId | Hash of the ChainId and the whole encoded transaction. |
| Work | The contribution of one block, derived from its target. |
