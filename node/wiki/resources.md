# Four resources

Every transaction is measured with four independent counters.

| Counter | Measures exactly | For example |
|---|---|---|
| **C**, compute | Instructions, function entries, memory, byte work, hashing, starting programs and preparing code, each at a measured price | Executing instructions, copying bytes, writing a record's value |
| **S**, state access | Logical record reads and writes, including the runtime's reads of manifests and code | Loading a program, reading and writing storage |
| **D**, data | The encoded transaction with its framing and fixed receipt fields, and every output and event byte programs generate | The transaction itself, results, events |
| **G**, growth | Gross positive persistent allocation | Creating records, enlarging values |

They are quantities, not fees. They are never converted into one another or
combined into a single number, and they are defined by logical events with fixed
charges, never by the CPU time, disk operations or cache behavior of a run. The
compute charges were set once from measurements, so that one C stands for about the
same work whatever it is spent on; after that, two nodes on very different hardware
count exactly the same amounts.

Each transaction declares a budget for all four, and each block has a capacity for
all four. The capacities are independent as well: more compute per block does not
bring more state access or more growth with it. Why they are kept separate is
covered in [Four counters, not one price](/wiki/decisions/four-counters).

The pages of this section give the exact rules: [every event and its
charge](/wiki/resources/counters), [the price list](/wiki/resources/prices) for
compute, how [budgets nest and run
out](/wiki/resources/budgets), how [persistent allocation](/wiki/resources/growth)
is counted, and the [block totals and the few fixed
bounds](/wiki/resources/capacity).
