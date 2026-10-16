# Access and exposure

The reference node has two interfaces with very different trust levels.

## The control channel

The CLI administers the node over its own TCP listener, which must be on a loopback
address: a configuration naming any other address is refused. Both sides hold the
secret in `control.token`: 64 lower-case hex characters, a random 256-bit value
created at init, readable and writable by its owner alone: on Unix by its mode, on
Windows by an access list that grants the user who ran `ting init` and nobody else,
whatever the data directory's folder would pass down. The token
never crosses the connection, nor appears in status output, logs or peer messages;
keep the file private.

A control request takes one connection of its own and three frames, framed as in
the [peer protocol](/wiki/node/sync#framing). Proofs are HMAC-SHA-256 keyed with the
token's 64 characters, over the concatenation of the parts listed, nonces as their
32 bytes:

1. The client sends `{"nonce": …}`, 32 random bytes in hex.
2. The node answers `{"nonce": …, "proof": …}`: its own 32 random bytes, and a proof
   over `ting-control-node`, the client's nonce and its own. The client checks it
   before it sends anything more, so a listener that is not the node, such as one on
   a port a stale `runtime.json` names, learns nothing it could use.
3. The client sends `{"request": …, "proof": …}`: the request as a string of JSON,
   such as `{"method":"status","params":{}}`, and a proof over
   `ting-control-client`, the node's nonce, its own and the request string's bytes.
   This frame may be as large as twice the next block's data capacity and 64 KiB
   more, at least 16 MiB, so that `submit` carries a transaction of any size a
   block can hold.

The node checks the proof in constant time and answers `{"ok": true, "result": …}`,
or `{"ok": false, "error": …}`, with `unauthorized` for a wrong proof and
`unknown control method` for any other method than these:

| Method | Purpose | CLI |
|---|---|---|
| `status` | Chain status plus the listener addresses actually bound | `ting status`, `ting docs` |
| `peers` | Configured and discovered peers with their last success or error, and the size of the address book | `ting peers` |
| `add_peer`, `remove_peer` | Change the peer list | `ting add node`, `ting remove node` |
| `api` | Enable or disable REST, with or without CORS | `ting api enable`, `ting api disable` |
| `miner` | Start, stop or describe the built-in miner; set producer data and capacity votes | `ting miner …` |
| `votes` | Where capacity voting stands, with this node's votes | `ting votes` |
| `template`, `solution` | The node's half of [mining](/wiki/node/mining) | `ting template`, `ting solution` |
| `mine` | Template, search and solution in one bounded request | `ting mine` |
| `submit` | Submit a transaction, deferred with `not_before` | `ting submit` |
| `invalidate`, `reconsider` | Mark a block invalid, or clear invalid marks ([overriding validity](/wiki/node/storage#overriding-validity)) | `ting invalidate`, `ting reconsider` |
| `stop` | Graceful shutdown | `ting stop` |

The node serves at most 16 control connections at once, and the CLI waits up to 35
seconds for an answer. Only this channel can mine, manage peers, toggle REST,
override block validity and shut the node down. Mining software holding the token
uses `template` and `solution` exactly as the CLI does; mining has no REST surface.

`runtime.json` records the addresses actually bound, and the process ID. It is never
treated as authority to stop a process.

## The REST interface

REST is off by default. When on, it binds to loopback unless you choose another
address, and it can be switched on or off while the node runs. Enabling it binds the
listener before recording success, so a failed bind leaves REST off and reports the
error.

Everything it offers is public by design:

- reads of blocks, transactions, events, contracts, storage, status and metrics;
- transaction submission into the bounded mempool with `POST /v1/transactions`, with
  no token, because programs define their own authorization.

REST status leaves out process IDs, listener addresses and control-channel details.
REST's reads run on threads of their own, one per processor core, beside the thread
that validates and applies blocks, and its few requests to that thread have their
own share of its queue, so REST traffic cannot slow block processing or crowd out
peers ([how the node is put together](/wiki/node#how-it-is-put-together)). There is
no rate per client. Requests, bodies, queues and pages are bounded:

| Bound | Value | When exceeded |
|---|---|---|
| Requests in progress | 32 per processor core | `503`, `HTTP service busy` |
| REST's requests waiting for the chain thread (submissions, the mempool, pending status) | 128 | `503`, `node request queue unavailable` |
| Time per request | 35 seconds; a submission also ten seconds for every MiB its body may have | `504`, `HTTP deadline exceeded` |
| A submission's body | Twice the data capacity of the next block, and 4 KiB | `413` |
| Any other request body | 16 MiB | `413` |
| A JSON page | Just under 16 MiB | The page stops early; follow `next` |

Every response carries `X-Content-Type-Options: nosniff`,
`Referrer-Policy: no-referrer` and `Cache-Control: no-store`. The endpoints are
listed on [Explore the chain](/wiki/start/explore#the-rest-api).

## Exposing a node

- Remote REST access is an explicit choice of bind address. For a remote audience,
  put a TLS reverse proxy in front.
- By default REST grants no cross-origin permissions. `ting api enable --cors`
  answers browser applications from any origin, such as a wallet in a web page;
  leave it off unless such applications must reach this node.
- The explorer, the API page and the wiki load no external assets. The API page's
  Swagger UI (5.33.0) is vendored with its license, notice and an integrity manifest
  and served by the node; its online schema validator, remote example servers and
  external documentation assets are disabled, and its requests go to this node's own
  API.
- The explorer inserts all dynamic data as text, never as HTML.
- Wiki pages come from a fixed registry compiled into the node. No user-controlled
  Markdown and no filesystem path is ever rendered.
- Peer traffic is unencrypted and unauthenticated, but it carries only public chain
  data, and no peer message reaches administration.
- With discovery on, a node tells its peers the addresses of nodes it has recently
  synchronized with, and announces its own listening port. With
  `"discovery": false` it does neither. Its address can still spread: a node that
  configures it, synchronizes with it and has discovery on shares it like any other
  ([finding more peers](/wiki/node/sync#finding-more-peers)).
