# Design commitments

Every rule in Ting follows from ten commitments. The pages in this section take
the most consequential ones in turn: the decision, why it was made, and what it costs.

1. Consensus specifies ordering, execution, commitments and resource bounds, and
   nothing else.
2. SHA-256 is the fixed consensus hash and proof-of-work function.
3. Every accepted program has deterministic execution semantics.
4. The protocol has no native currency, external user accounts, signature scheme,
   fee asset, account nonce or application address format.
5. Authentication, replay protection, assets and application cryptography are
   ordinary program behavior.
6. Code is immutable after deployment. Applications upgrade through calls, new
   deployments and their own state.
7. Resource counters describe logical work and quantities, independent of CPU speed,
   time, caches and prices.
8. Miners can adjust resource capacities at predefined rates above fixed floors. They
   cannot vote to change instructions, accounting, hashes or application rules.
9. All block data needed to validate a state transition must be available.
10. Optimized execution must preserve results, failure behavior and accounting.

The execution model and the host interface are consensus rules. A compiler, a
database or a relay policy is an implementation choice.

These commitments are settled.

## Declined

- **Merged mining** with another chain's proof of work, which would put foreign
  formats into frozen consensus.
- **State rent or expiry**, which would have nothing to charge in and would change
  storage semantics under deployed programs.

## Outside sources

These references inform the design:

- [NIST hash-function guidance](https://csrc.nist.gov/projects/hash-functions), for
  SHA-256;
- [WebAssembly Core 1.0, W3C Recommendation dated 5 December 2019](https://www.w3.org/TR/2019/REC-wasm-core-1-20191205/),
  the base of the execution profile;
- [WebAssembly Core Specification, Release 2.0](https://www.w3.org/TR/wasm-core-2/),
  for the three accepted additions;
- the [wide-arithmetic proposal](https://github.com/WebAssembly/wide-arithmetic),
  for its four instructions;
- the [WebAssembly execution specification](https://webassembly.github.io/spec/core/exec/instructions.html),
  for reviewing where Ting fixes a behavior deterministically;
- [ASERT difficulty adjustment (aserti3-2d), Bitcoin Cash, November 2020](https://upgradespecs.bitcoincashnode.org/2020-11-15-asert/),
  the source of the retarget arithmetic;
- [EIP-2929](https://eips.ethereum.org/EIPS/eip-2929), as evidence for separating
  storage access from ordinary computation.

Later changes to these documents are not incorporated into Ting's consensus. The
wire formats, resource definitions, voting rule and state model are Ting's own, and
Ting claims no adoption or endorsement by the projects referenced.
