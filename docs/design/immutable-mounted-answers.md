# Immutable mounted answers — design

**Status:** design only, 2026-10-30. Nothing here is built. Ledger
[#2686](http://localhost:1060/l/default/item/2686), from the very-large-graphs direction
(register entry 12, section 4). Follows [#2660](http://localhost:1060/l/default/item/2660)
(the `mounted` uncached reason, core 0.2.9). The hub briefs the build from this; the code is
the spec where they differ once it exists.

---

## The problem, reproduced

A mount's forwarding endpoint (engine) holds every peer answer uncached with
[`Representation::with_uncached_reason(Mounted)`](../../crates/ikigai-core/src/repr.rs),
which sets `Expiry::Always` whatever the peer said. That is right for a peer's mutable
state: the peer's cuts cannot reach this kernel, so nothing here could invalidate a cached
copy. And it is the only vocabulary core gives a mount: there is no way to say "this peer
answer cannot change, keep it".

So an answer that **cannot** change is recomputed on every read too. Reproduced on core
0.2.10 (`8c3b104`) with a mount written the only way core allows: a name
`urn:peer:blob:b3:<digest>` whose peer answer is `Expiry::Never` and whose bytes hash to the
digest in the name, forwarded with `with_uncached_reason(Mounted)`:

```
peer asked 5 times for 5 reads
cached after the reads: false
uncached (why a computed read was not cached; the last 64, most recent first)
  urn:peer:blob:b3:36f66cda…4e6b      ×5  mounted  [root]
```

A federated read of sealed data costs a peer round trip on every read, so a federated
river is slower than a local one by exactly the part that never changes.

## What "cannot change" can mean

Two different claims, and they are worth very different amounts:

1. **The name is a digest.** The request names `b3:<hex>` as the content it wants. The root
   held the expectation **before the peer spoke**, so checking the bytes against it proves
   the answer is the one the name means, now and forever. Nothing to invalidate, no trust
   in the peer needed. A new version of the content is a new name, so the 2026-08-15
   staleness (a cacheable mounted resource served old bytes after the peer upgraded)
   cannot happen to it.
2. **The peer says the answer is immutable.** The name is an ordinary name; the peer
   promises it will never answer it differently. A digest sent **with** the bytes proves
   only that the reply arrived intact: a lying peer computes the digest of whatever it
   sends. (Same point as [cache-ejection §5](cache-ejection.md): content addressing proves
   internal consistency, not that the answer is right.) The root can hold the peer to the
   promise only when it reads the name again, and a peer that upgrades and changes the
   bytes under the same name is the 2026-08-15 incident exactly.

So the design treats (1) as permanent and (2) as permanent **for one peer epoch** (see
below), and builds (1) first.

★ **Naming.** The item says "marked sealed". "Sealed" already means something in core:
[sealed names](../../crates/ikigai-core/src/seal.rs), prefixes only their owner may answer
(`SealOwner`, `SEALED_NOTE`). This design says **immutable** for the peer's mark and **pin**
for the digest the root verifies, so the two concepts never share a word in an API, a trace
note or a readout.

## Where it lives

Core owns the concept, the verification and the readout; the engine owns the mount and the
wire field; the peer's serving edge owns the mark it sends.

### Core: a pin on the representation, verified by the kernel

- **`Representation::pinned(digest: ContentId, by: PinSource) -> Self`** (builder), with
  `PinSource::Name` (the expected digest came from the request) or `PinSource::Peer` (it
  came from the peer's reply). Kernel-local, like the uncached reason: a field of the
  private `Facts`, **never serialized**, not part of representation identity. The wire
  encoding of `Representation` stays byte-for-byte what `tests/repr_body_param.rs` pins.
- **The kernel verifies, not the endpoint.** On the issue path, where the endpoint's answer
  is turned into `why` before `store_outcome` (`kernel.rs`, the two store sites around the
  `Expiry::Always => why.reason = representation.uncached_reason()` match), a pinned answer
  is checked first: `ContentId::of(bytes)` against the pin. Putting it in the kernel means
  every forwarder (an engine mount, a module host bridge, a future one) gets the same rule,
  and no endpoint can obtain a pin's cacheability without the bytes being hashed by the
  root.
  - **Match:** the answer is stored with `Expiry::Never`. A `Name` pin keeps the threads it
    has (normally none: nothing on this kernel can change it). A `Peer` pin must also carry
    the mount's **epoch thread** (below); a `Peer` pin without one is held `mounted`, so a
    forwarder cannot skip the epoch by omission.
  - **Mismatch:** the answer is not served and not stored. The read fails with
    `Error::Endpoint` naming the target, the expected digest and the digest of the bytes
    received (class `Unknown`: not something to retry blindly; adding a variant to the
    exhaustive `Error` is a substantial change and this does not need one). The failure is
    not cached (no failure expiry), so a composite over it reports `failed <name>` in the
    readout as any failure does.
- **The digest covers the bytes**, as every content name in ikigai does (`ContentId::of`:
  `ArgRef` content ids, `ContentStore`, `Spill`), not `Representation::content_id()` (type
  and bytes). The type is the peer's word and is cached with the bytes. A mount that needs
  the type pinned too puts it in the name (see open questions).
- **A streamed body** (`Representation<Body>`, one-shot or replayable) cannot be checked
  before it is read. First tranche: a pinned answer whose body is not a buffer is held
  `mounted` as today, never trusted unread. Later: hash while it is read (the `Spill`
  already hashes as it writes, ledger #2372) and store only if the digest matched at the
  end; a mismatch found at the end fails the read that was streaming it.

### Engine: the mount honors the pin

- **Digest names are declared, never guessed.** A name that merely *contains* `b3:<hex>` is
  not a digest name: `urn:peer:turtle-of:b3:…` answers something derived from that content,
  and verifying its bytes against the digest would refuse every honest answer. The mount
  declares which of its names are content names, e.g. a template
  `urn:peer:blob:{digest}` with `digest` named as the pin. For a request that matches, the
  forwarding endpoint parses the variable with `ContentId::parse` and answers
  `peer_reply.pinned(digest, PinSource::Name)` **instead of** `with_uncached_reason(Mounted)`
  (in place of the `as_held_here` step today). A name that does not match is forwarded
  exactly as now.
- **The peer's mark on the wire.** The reply envelope (ikigai-wire's reply, not
  `Representation`'s own encoding, which every peer decodes and is pinned) gains one
  optional field, `immutable: Option<ContentId>`. The peer sends it only with
  `Expiry::Never`; the mount answers `pinned(digest, PinSource::Peer)` plus its epoch
  thread when it is present and the expiry is `Never`, and `with_uncached_reason(Mounted)`
  otherwise. An old peer sends no field and an old mount ignores it: both directions
  degrade to today's behavior.
- **The epoch thread.** Each mount owns one thread, named after the mount (e.g.
  `urn:kernel:mount:<prefix>`, kernel-namespaced so no module can cut it), and cuts it when
  the peer it talks to changes: a reconnect that reports a different peer identity or
  version. That is a **local** cut, fired by the mounting kernel from what it observes, so
  it needs no cross-kernel cut. A peer-pinned entry dies with its epoch; a name-pinned
  entry does not hang on it and survives the upgrade, because it cannot be stale.

### The peer: when it may say immutable

The peer's serving edge sends `immutable` only for an answer whose endpoint marked it so
(`Representation::immutable()`, a kernel-local fact on the peer) **and** whose effective
expiry is still `Never` after composition **and** that picked up no thread but its own
target (any other thread is state that can change). It computes the digest itself from the
bytes it is about to send. The mark is the endpoint's promise that the name never answers
differently; the conditions only stop a composite from inheriting a promise its parts did
not make.

### Containing a lying peer

- **A name-pin mismatch** is refused and, being a failure, never cached. The mount counts
  it against the peer.
- **A peer-pin broken across reads**: the mount remembers, for a bounded number of
  peer-pinned names (the same order as `UNCACHED_LOG`), the digest each was pinned to in the
  current epoch. A re-read in the same epoch (after an eviction) that comes back pinned to a
  different digest is a broken promise: refused like a mismatch and counted. Bounded memory
  makes this detection best-effort, and the design says so; the epoch thread is what bounds
  the damage either way.
- **Flagged.** A peer with any mismatch in the current epoch is flagged: until the epoch
  changes (or an operator clears it), the mount stops honoring its `immutable` marks
  (every peer-pinned answer is held `mounted` again), while name pins keep being verified
  and cached, since they never trusted the peer. The flag is visible (next section).

## What `urn:kernel:uncached` says

| read | outcome | readout |
| --- | --- | --- |
| mutable mounted read | held uncached | `mounted`, as today |
| digest-named read, bytes match | stored `Never` | no row: a stored read records nothing, the existing rule |
| peer-marked read, bytes match, peer not flagged | stored `Never` on the epoch thread | no row |
| pin on a streamed body (first tranche) | held uncached | `mounted streamed` |
| peer flagged for a mismatch | held uncached | `mounted distrusted` |
| bytes do not match the pin | refused | no row for the read itself (it failed); the error names both digests; a composite over it reports `failed <name>` |

`mounted streamed` and `mounted distrusted` are new `UncachedReason` variants
(`#[non_exhaustive]`, so adding them is not a break), rendered as those two words in the
reason clause the `UNCACHED_NOTE` grammar already has. The `urn:kernel:uncached` summary
gains the two clauses. A mismatch also goes on the trace as the failed event it is, with
the two digests in its detail.

## Tests the build must add

In `crates/ikigai-core/tests/uncached.rs` (or a new `tests/pinned.rs`), with the mount
modeled the way the existing `a_peer_answer_held_here_uncached_reports_mounted…` test
models it: an endpoint that counts its invocations and builds the answer the mount would.

1. **A digest-named mounted read is cached after the first read**: five reads, one
   invocation, `is_cached` true, no `urn:kernel:uncached` row. (Today: five invocations,
   `mounted ×5`; the reproduction above.)
2. **A mismatched digest is refused**: an answer pinned to `ContentId::of(b"a")` with bytes
   `b"b"` fails with `Error::Endpoint` naming both digests, is not cached, and the next read
   invokes the endpoint again.
3. **A mutable mounted read still reports `mounted`**: the existing test keeps passing
   unchanged, and an unpinned answer next to a pinned one in the same kernel still reads
   `mounted`.
4. **A peer pin lives for one epoch**: a `PinSource::Peer` answer on the epoch thread is
   cached; cutting the thread makes the next read invoke the endpoint; a `Peer` pin without
   an epoch thread is held `mounted`.
5. **A name pin survives the epoch cut**: cutting the epoch thread leaves a name-pinned
   entry cached.
6. **A pinned streamed body is not trusted unread**: it is served, not stored, and reads
   `mounted streamed`.
7. **A composite inherits the pin's cacheability, not the pin**: a cacheable composite over
   a name-pinned mounted read is cached (it was `dependency <name>` before); over a
   mismatched one it fails or reports `failed <name>` as any failure does.
8. Wire round trip: `Representation`'s postcard and JSON encodings are unchanged by a pin
   (the pin never crosses), alongside `tests/repr_body_param.rs`.

The engine's mount tests (name template parsing, the reply field, the epoch cut on a peer
version change, the flag) belong to the engine's build.

## Open questions for the hub

- **Type pinning.** The digest covers bytes only. A content name that should also fix the
  type would put it in the name (`{digest}.ttl`) for the mount to check against the reply's
  type; or the pin could take an optional `ReprType`. Neither is needed for the first
  tranche.
- **Sharing across capabilities.** The cache key carries the capability fingerprint, so the
  same digest-named answer read under two capabilities is fetched twice. A pinned answer is
  the same bytes for everyone allowed to read it, but the capability floor fences cached
  answers too, so sharing them needs its own design; out of scope.
- **Which mismatch policy for name pins.** Refuse and count, as above, or also stop asking
  that peer for digest names until the epoch changes. Refusing is enough for correctness;
  the second is only about wasted round trips.

## Build order

1. Core: `PinSource`, `Representation::pinned`, verification on the issue path for buffers,
   the two `UncachedReason` variants and their rendering, tests 1–3 and 5–8.
2. Engine: declared digest-name templates in the mount; replace `as_held_here` for those
   names. This alone delivers the item's point: a federated read of digest-named data is
   computed once.
3. Wire and peer edge: the `immutable` reply field, `Representation::immutable()` on the
   peer, the epoch thread, the pin memory and the flag; test 4 and the engine's tests.
4. Streamed pinned bodies hashed while read.
