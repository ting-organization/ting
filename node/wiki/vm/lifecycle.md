# Deploy and invoke

A deployment installs immutable code and runs it once as its initializer; an
invocation runs installed code. Both happen the same way whether a transaction
or another program asks for them, and both follow a fixed order of steps, each
with its debit, so that every node fails at the same step with the same
counters. Every debit below is atomic ([budgets](/wiki/resources/budgets#atomic-debits)):
if it does not fit, nothing of it is charged and the step fails with the status
of the exhausted counter.

## Deployment

A deployment from a program has first passed the `deploy` import's own checks:
read-only mode, the spans, the sub-budget, and receiving its code and input within
that sub-budget ([host interface](/wiki/vm/host#calls-and-deployments)). Then, and
for a top-level deployment directly:

1. **Ordinal.** Take the transaction's current deployment ordinal, which starts at
   zero, and advance the counter. Rollback never restores it, so a failed
   deployment still uses its ordinal. An overflowing counter fails the deployment
   with status 7.
2. **Identifier.** Debit 1,900 C for deriving the ContractId from the chain, the
   transaction and the ordinal ([identifiers](/wiki/data/records#identifiers)),
   then read its manifest record, `[1,900, 1, 0, 0]`. If it exists, the deployment
   fails with status 10.
3. **Validation.** Debit the code's [code price](/wiki/resources/prices#code) plus
   the price of hashing it into its CodeHash, in one debit before validation and
   before any cache lookup, then [validate](/wiki/vm/validation). Invalid code fails
   with status 9; the debit stays consumed. Valid code counts as prepared for the
   rest of the transaction.
4. **Installation.** Write the manifest, which records the code price debited in
   step 3, and the code record. Each write follows
   the storage-write rule ([host interface](/wiki/vm/host#storage-writes)): 1,900 C
   and one S for the key and the read of the old record, then one atomic debit of
   the price of hashing the value into its leaf, one S and the record's
   [growth](/wiki/resources/growth).
5. **Initializer.** Invoke the new contract as below, with the initial input, in
   ordinary mode, with the deploying contract as its caller, or none at the top
   level. It can write the new contract's storage, call other contracts and emit
   events.
6. **Identifier result.** When the initializer succeeds, debit 32 C and 32 D for
   producing the identifier. The deployment's result is the 32-byte ContractId and
   nothing else: the initializer's own result bytes are discarded, though they
   were charged as data when it returned them. A top-level deployment's receipt
   carries the ContractId as its result; a program reads it with `call_result`.

If any step fails, including the identifier debit, the installation and
everything the initializer and its descendants did are rolled back. The ordinal
stays used and every resource consumed stays charged. A failed initializer's
status and result bytes are the deployment's outcome, so a revert's bytes reach
the deployer; a failure before the initializer has an empty result.

## Invocation

Before its top-level call or deployment, a transaction pays its data base
([counters](/wiki/resources/counters#d-data)) and then one root dispatch C, the
same whatever the program does. Every run of a program, top-level or nested,
then works the same way:

1. **Code.** Read the manifest, `[1,900, 1, 0, 0]`; for a call to a contract that
   does not exist, the invocation ends here with status 8. Read the code record,
   `[1,900, 1, 0, 0]`. Then, unless the transaction has already prepared code with
   this CodeHash, debit its [code price](/wiki/resources/prices#code) for loading
   it, even when the node holds a compiled form; from then on the code counts as
   prepared for the rest of the transaction, whatever later fails. The price is the
   one the manifest records, and no byte of the code is read before it is paid: the
   code record's read needs only the length the manifest gives.
2. **Preparation.** Debit, in one atomic step before the instance exists,

   ```text
   C = 40,000 + 40,000 × initial_pages + 40 × global_count + 4 × table_size
     + 12 × active_data_segments + bytes(sum of the lengths of all active data segments)
   ```

   which pays for starting the instance, its initial memory, setting the globals
   and the table, and copying the data segments
   ([invocations](/wiki/resources/prices#invocations)). The initializer expressions
   themselves cost nothing more. Then build the fresh instance: globals set to their constants,
   the table filled from the element segments, memory zeroed and the data
   segments copied in source order ([the abstract machine](/wiki/vm/execution#instances)).
3. **Entry.** Debit the reservation of `invoke` ([function entry](/wiki/vm/metering#function-entry)).
4. **Run.** Execute `invoke`. The program reads its input and context through host
   functions and ends with `return`, with `revert`, with a trap, by exhausting a
   budget, or by simply finishing, which is a successful empty return.

Nothing survives an invocation except storage records, retained events and the
result. Globals, the table and memory are initialized from the immutable code
every time, so two calls to the same contract within one transaction each start
from scratch, and the only persistent program state is what the storage
functions write.

## Installed code is always valid

Invalid deployable code is an ordinary deployment failure with its charges.
Installed code, by contrast, was validated when it was deployed, and its manifest
and code record always agree. Finding them otherwise is a violation of the state
invariant, a fault of the node that holds that state, and never an alternative
outcome of the VM.

## Compiled code is a node's business

A node may compile and cache modules however it likes. A cache hit must produce
exactly the outcome and counters of a cold start, which is why validation, code
reads and code loading are charged even when the code is cached. Whether code is
prepared within a transaction is part of the transaction's own history, not of any
node's cache.
