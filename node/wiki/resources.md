# Four resources

Everything a transaction does costs something: running instructions, reading and
writing records, carrying bytes across the network, and leaving state behind that
every node must keep. Ting measures these with four independent counters, so that a
transaction pays for each kind of work in its own unit and a block can hold only as
much of each as the network has agreed to. The exact rules are in
[sections 11 to 15 of the protocol rules](../../ting-protocol-rules.md#11-budgets-and-exhaustion).

| Counter | Measures exactly | For example |
|---|---|---|
| **C**, compute | Instructions, function entries, memory, byte work, hashing, starting programs and preparing code, each at a measured price | Executing instructions, copying bytes, writing a record's value |
| **S**, state access | Logical record reads and writes, including the runtime's reads of manifests and code | Loading a program, reading and writing storage |
| **D**, data | The encoded transaction with its framing and fixed receipt fields, and every output and event byte programs generate | The transaction itself, results, events |
| **G**, growth | Gross positive persistent allocation | Creating records, enlarging values |

They are quantities, not fees. They are never converted into one another or
combined into a single number. They are defined by logical events with fixed
charges, never by the CPU time, disk operations or cache behavior of a run. The
compute charges were set once from measurements, so that one C stands for about the
same work whatever it is spent on: about one integer addition that has to wait for
the one before it. They are fixed numbers, so two nodes on very different hardware
count exactly the same amounts.

Each transaction declares a budget for all four, and each block has a capacity for
all four. The capacities are independent as well: more compute per block does not
bring more state access or more growth with it. Why they are kept separate is
covered in [Four counters, not one price](decisions/four-counters.md).

The pages of this section explain each part: [every event and its
charge](resources/counters.md), [the price list](resources/prices.md) for
compute, how [budgets nest and run
out](resources/budgets.md), how [persistent allocation](resources/growth.md)
is counted, and the [block totals and the few fixed
bounds](resources/capacity.md).
