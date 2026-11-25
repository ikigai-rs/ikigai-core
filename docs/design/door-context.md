# Door context: facts that survive attenuation without granting authority

**Status:** design spike (ledger #4321); no API or behavior change. Source review on 2026-11-24
against core [`41d76f6`](https://github.com/ikigai-rs/ikigai-core/tree/41d76f68241d48b2994b2830d8628bd1c003d855).
Follows [Overlay request views and door context](overlay-request.md) (ledger #1212), whose
"Context without authority" section names the question. Evidence:
[`tests/door_context.rs`](../../crates/ikigai-core/tests/door_context.rs), three tests that pin
what the kernel does today with the public API alone. Every type and method named below as
*proposed* is a sketch to decide on, not an agreement to build it.

## The question

A door (an HTTP route, a REPL, a scheduler) knows things about a request that no endpoint can
know: where it entered, which page it was made for, which run budget the door chose. Those
**door facts** must reach the endpoints that answer, through ordinary and attenuated
sub-requests, confinement and scheduled fan-out, with four invariants:

1. A door fact never grants a capability, widens a scope or replaces the capability floor. A
   page label is an observation, not permission.
2. Only the host sets door facts.
3. Endpoint code can neither erase nor enlarge them.
4. A door fact that changes an answer is part of the answer's cache identity at lookup; one
   that does not change it cannot silently end up in a cached answer.

## Findings

Each claim is pinned by a test in `tests/door_context.rs` or by the cited source.

**The run context already crosses every in-process seam the question lists.** `IssueContext`
([run.rs:663](../../crates/ikigai-core/src/run.rs#L663)) carries the retry pool, the deadline
and the byte pool. Every reborrow copies it: `with_bindings`
([endpoint.rs:939](../../crates/ikigai-core/src/endpoint.rs#L939)), `confine`
([endpoint.rs:1050](../../crates/ikigai-core/src/endpoint.rs#L1050)) and the crate-private
`with_request` ([endpoint.rs:2584](../../crates/ikigai-core/src/endpoint.rs#L2584)). Every
sub-request passes it to the issuer, whatever capability it runs under, attenuated or not
([endpoint.rs:1854](../../crates/ikigai-core/src/endpoint.rs#L1854)). Scheduled fan-out
clones it into each spawned branch rather than relying on a task-local
([endpoint.rs:2296](../../crates/ikigai-core/src/endpoint.rs#L2296)). Test
`a_run_context_fact_survives_attenuation_confinement_and_fan_out_and_a_scope_marker_does_not`
puts a deadline on a run whose door capability also holds a marker scope. All five leaves (a
plain sub-request, an attenuated one, a confined one, two fan-out branches) see the deadline.
Only the attenuated one loses the marker: attenuation intersects scopes
([capability.rs:387](../../crates/ikigai-core/src/capability.rs#L387)), so the capability is
the wrong carrier for door facts, which is what pushed the downstream host to a task-local.

**Endpoints cannot replace the run context, only narrow it.** The field is private. The only
changes an endpoint can make are `with_retries`, `with_memory_limit` and `with_not_after`,
and each of them narrows. `issue_context()` hands out a shared reference, for forwarding. An
endpoint can construct an `IssueContext` but has no way to issue under one: only
`Kernel::issue_with_context` accepts one, and that is a host entry point. This is the same
structure that keeps `issue_under` private.

**An issuer that does not override `issue_in_context` drops the context silently.** The
default drops the context and delegates
([endpoint.rs:179](../../crates/ikigai-core/src/endpoint.rs#L179)). That covers module host
bridges and a wire without a field for the context. Test
`an_issuer_without_the_context_seam_drops_the_door_fact_and_keeps_the_authority` issues the
same sub-request through the kernel and through such a bridge. The kernel path delivers the
deadline. The bridge path delivers no deadline and an unchanged capability. For authority this
fails closed, because nothing is gained. For a door fact it fails open: the far side cannot
tell "no door said anything" from "a door said something and it was lost". The scope seam
already made the opposite choice for the resolution chain:
`issue_in_scope`'s default refuses a non-empty chain
([resolution-scope.md, decision 7](resolution-scope.md#7-the-wire-drops-the-scope--a-named-hole-in-two-places)).

**The cache key has no run-context dimension, and reading the context marks nothing.** The
key is request identity, capability fingerprint and scope fingerprint
([cache.rs:118](../../crates/ikigai-core/src/cache.rs#L118)). Test
`a_cached_answer_that_read_the_run_context_is_served_to_a_run_with_another_context` caches an
answer that reports its run's deadline. A second run with another deadline is served the first
run's answer, and the endpoint runs once. For a deadline that is acceptable because the deadline
only bounds the work. A door fact read the same way would leak one root's observation into
another root's answer, and nothing would show it.

## Decision 1: extend the run context; do not invent a carrier

Add one half to `IssueContext`: an immutable, host-built **door context**, held behind an
`Arc` so a sub-request copies a pointer. The run context is the right home for four reasons:

- It already crosses every seam the question lists (the first finding), including fan-out,
  without a task-local. A second carrier would have to repeat each of those copies, and the
  first one it missed would be a silent drop.
- It is already opaque and `#[non_exhaustive]`, and its documentation reserves room for "later
  halves" without a new seam ([run.rs:649](../../crates/ikigai-core/src/run.rs#L649)).
- It is already independent of the capability, so attenuation cannot touch it.
- It is already where a wire carries run facts (wire v9's `IssueWith`), so remote propagation
  is a field on an existing message, not a new protocol concept.

The door half differs from the existing halves in one respect. A budget only **narrows** on the
way down. Door facts are **fixed** for the whole tree: neither narrowed nor widened. Narrowing
a page label (dropping it) is erasure, which invariant 3 forbids. So the door half gets no
builder on `Invocation` at all, not even a narrowing one.

Proposed shape, for discussion:

```rust
// Built by the host, sealed once; no mutator after `build`.
let door = DoorContext::builder(Iri::parse("urn:door:http:public")?) // where it entered
    .observe("urn:label:page:checkout")        // observational: never keyed, never read into an answer
    .partition("urn:label:audience:public")    // result-affecting: keyed at lookup
    .build();
kernel.issue_with_context(request, &capability, IssueContext::new().with_door(door)).await
```

Facts are IRIs: an entry door and two sets of labels, observational and partitioning, in the
open-IRI style of the [label lattice](label-lattice.md). They are not a free-form map. A
writable `String → String` map cannot keep invariant 3, and the overlay note already rejected
one. A run budget is **not** a door fact. It stays an enforcing half of its own (retries,
deadline, bytes, and later fuel, ledger #1889), governed by the budget follow-up below.

## Decision 2: authority never reads it

- No `Capability` constructor, attenuation or floor check takes a `DoorContext`, and the
  kernel's admission and floor code never read it. Their inputs stay request, capability and
  scope.
- A door that wants a label to **narrow** authority does that at admission, by attenuating the
  capability it issues under, as the downstream page-origin door already does
  ([overlay-request.md, findings](overlay-request.md#findings)). The label and the narrowing
  are then two facts, and only the capability is authority.
- An endpoint that branches on a door fact can only **refuse more or answer less** than its
  capability allows. Nothing it reads from the door can raise what a sub-request may do,
  because sub-request authority still comes only from `issue_under`.

## Decision 3: only the host sets it; endpoints cannot erase or enlarge it

- The door half is set only by the builder on `IssueContext`, and the kernel accepts an
  `IssueContext` only at its root entry points: host code.
- `Invocation` gets one reader, `door() -> Option<&DoorContext>`, and no setter. Every reborrow
  (`with_bindings`, `confine`, `with_retries`, `with_memory_limit`, `with_not_after`,
  `with_request`) copies the half unchanged. The narrowing builders on `IssueContext` keep it.
- `Invocation::detached` has no door half. That is not erasure: a detached invocation has no
  issuer, so it can issue nothing that would run without the door.
- An endpoint that holds its own `Arc<Kernel>` and issues a fresh root through it is host code
  by construction, as it is today for capabilities.

## Decision 4: propagation

| Boundary | Door context | Why |
|---|---|---|
| Ordinary sub-request (`issue`, `source`, bodies) | carried unchanged | the run is carried |
| Attenuated sub-request | carried unchanged | independent of scopes (test 1) |
| Confinement | carried unchanged | the chain narrows, the run does not (test 1) |
| Scheduled fan-out | carried in each branch's cloned context | no task-local (test 1) |
| Reborrows, including the private `with_request` | carried unchanged | shared run |
| Kernel-op and admission-only paths | carried, never consulted | authority never reads it |
| Concurrent roots | one `Arc` per root, never shared | no global or task-local state |
| Issuer without the seam (module bridge, older wire) | refused when the context holds partitioning facts; dropped, with a trace note, when it holds only observational ones | decision 5 |
| Remote wire, outbound | carried as a field of `IssueWith` once the protocol has it | the existing run-fact channel |
| Remote wire, inbound | never adopted; the receiving door mints its own | decision 5 |

The only cross-root channel the kernel has is its shared cache. Isolation between concurrent
roots is therefore a cache property, settled in decision 6.

## Decision 5: fail closed at boundaries that cannot carry it

**Missing context is "unknown", never "trusted".** `door() == None` means no door spoke or a
boundary lost what it said. An endpoint must not infer an internal, privileged or default
origin from absence.

**An issuer without the seam.** Follow the scope seam's precedent (decision 7 of resolution
scope), split by kind:

- If the context holds **partitioning** facts, the default `issue_in_context` refuses with
  `Error::Denied`, worded to [ACCESSIBILITY.md](../../ACCESSIBILITY.md): it starts with
  `denied:`, then names the fact and the issuer that cannot carry it. Dropping such a fact would
  serve an answer computed for no audience as if it were computed for this one.
- If the context holds **only observational** facts, it delegates as today and records a trace
  note that the door context was dropped at that issuer. That loses telemetry, never
  correctness.

Budgets keep their documented behavior: beyond such an issuer, a fresh pool is minted from the
capability. That fails closed for authority, and the budget follow-up decides whether it should
also refuse.

**Remote, inbound.** A peer's claimed door context is data, not a door. The receiving kernel's
wire door mints its own `DoorContext` with itself as the entry. It may copy the peer's claims
only as observational facts namespaced by the peer's identity, and only when the host
configured that peer as trusted for door facts. It never copies them as partitioning facts:
a remote peer choosing the cache partition of a local answer is a peer choosing what other
local roots are served. A partitioning fact the receiving door requires and the peer did not
send is a refusal, not a default.

**Remote, outbound.** Observational facts may be sent. A peer that does not declare support for
door context gets the same treatment as an issuer without the seam.

## Decision 6: cache policy, at lookup

The key is computed before the endpoint runs, so anything that must partition answers has to
be known at admission. The door context is: the host built it before the call.

- **Partitioning facts are a fourth key dimension.** `CacheKey` gains a door fingerprint over
  the sorted partitioning labels. It is `0` when there are none, as the scope fingerprint is
  `0` for the empty chain, so every existing key stays byte-for-byte the same. The fingerprint
  is inherited down the tree, so an inner hit is partitioned exactly as the outer one is. The
  failure cache uses the same key. This makes an outer hit safe (the problem the overlay note
  found): the hit is already in the right partition, so skipping the overlay serves nothing
  computed for another audience.
- **Observational facts are never in the key, and reading one makes the answer unstorable.**
  `door()` records a read of an observational fact the way a one-shot body is recorded today:
  the kernel declines to store the answer and the uncached face says why
  (`door context read`). Test 3 shows what this prevents. Most endpoints never need to read
  them: the kernel stamps the door's facts onto the root's trace events itself, and the trace
  scope is already per root.
- **The entry door is observational** unless the host lists it as a partition. Two doors that
  serve the same request, capability and chain share answers. That is the point of a shared
  cache.
- **Budgets are never in the key** (a mutable counter is not a stable identity). Whether a hit
  spends or only observes a budget is the budget follow-up's question, as the overlay note said.

The deadline today is the counterexample this policy avoids repeating: it is readable, unkeyed
and does not mark the answer (test 3). It is acceptable only because a deadline is meant to
bound work, not to be written into answers. The follow-up list records whether a read of
`not_after` that reaches an answer should be detectable.

## Not decided here

- Whether observational facts should also reach the trace face of `urn:kernel:explain`.
- Whether an endpoint can *declare* that it depends on a partitioning fact, so the floor can
  refuse a root that lacks it up front, instead of the endpoint refusing at run time.
- Fingerprinting consulted partitions only, rather than the whole set, mirroring the
  refinement resolution scope left open for corridors.

## Proposed follow-up items

- **Add the door half to `IssueContext`** (decisions 1 to 3): `DoorContext` and its builder,
  `IssueContext::with_door`, `Invocation::door`, carried by every reborrow. Tests: each row of
  the propagation table, and a floor refusal that a door label does not lift.
- **Partition the cache key by door fingerprint** (decision 6): a fourth `CacheKey` dimension
  that is `0` when empty. Tests: the two-audience pair in both directions with invoke counters,
  the same pair on the failure cache, and an outer hit that skips an overlay inside the right
  partition.
- **Make observational reads unstorable** (decision 6): record the read, decline insertion,
  name the reason on the uncached face, and stamp door facts on root trace events. Turns test 3's
  door analogue from "served" into "not stored".
- **Refuse partitioning door context at issuers without the seam** (decision 5): the default
  `issue_in_context` refuses with a `denied:` message, and drops observational context with a
  trace note. Then implement the seam in `ikigai-module`'s bridges.
- **Carry door context on the wire** (decision 5): an `IssueWith` field, the peer-trust
  configuration for inbound claims, and the inbound re-minting rule. This needs a protocol
  version bump.
- **Specify monotone execution budgets** (carried over from the overlay note): shared versus
  per-branch accounting, hit behavior, refusal at seamless issuers, and whether a budget read
  that reaches an answer should be detectable, deciding the deadline case of test 3.
- **Replace the downstream task-local** once the door half exists: the run-budget overlay reads
  the run context, and its scheduled fan-out holds without same-task execution.
