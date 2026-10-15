# Security

Report a vulnerability privately: on this repository's **Security** tab, choose
**Report a vulnerability**. Please do not open a public issue for it.

A useful report names the commit or release, what an attacker gains (coins, a stalled
or split network, a crashed or isolated node, a leaked secret), and the steps or a
test that show it.

Everything in this repository is in scope:

- the consensus rules and the VM in `src/`, and the specification in `node/wiki/`;
- the reference node in `node/`: peer protocol, storage, REST API, control channel and
  miner;
- the reference programs in `programs/`.

Consensus is frozen: a flaw in a consensus rule is not patched in place but answered
by a new chain, as [the freeze](node/wiki/decisions/freeze.md) describes. A deployed
program cannot change either, so a flaw in a reference program is answered by a new
program. Flaws in the node are fixed in new releases.
