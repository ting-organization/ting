# Upgrades through programs

Installed code never changes. There is no replace operation, no privileged upgrade key
and no self-destruct. A new version of a program is a new deployment with a new
ContractId, and users can always see exactly which code they are calling. Upgrades
are made of ordinary calls, new deployments and the application's own state: the
protocol's rules stay fixed while applications evolve through program code.

There is also no way to run one contract's code against another contract's storage.
Each version keeps its own storage, so moving state between versions is always explicit.

## Patterns

- **Routing.** A small contract stores the ContractId of the current version and
  forwards calls to it. Who may change that record is a rule the router's own code
  enforces, and whoever holds that right controls the application.
- **Migration.** A new version reads what it needs from the old one through calls, or
  users move their own data in explicit transactions.
- **Versioned interfaces.** Inputs carry a version tag, so a contract can accept old
  and new formats side by side.
- **Replaceable components.** A signature verifier, hash library or proof system is a
  contract like any other. An application that calls it by ContractId can move to a
  newer one through its own rules, such as a router record, when it chooses; nothing
  in the protocol has to change for that.

Each pattern has trade-offs, and the protocol takes no side. A router adds a call to
every use, migration costs transactions, and permanently fixed code is safe only if
it is right.
