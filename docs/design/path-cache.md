# The path cache — parked, with its shape written down

**Status:** design only, 2026-09-26. Nothing here is built, by decision — ledger
[#510](http://localhost:1060/l/default/item/510). Brian, 2026-09-25: *revisit once scope,
limiter, depth bound, lossless transreption and topology are in; re-measure then, do not
inherit the numbers below.* Companion to `docs/formalism/README.md` §1 (the γ_path row) and
§10 (the numbers); the code is the spec where they differ.

---

## What it is

Peter's Hotel §5.1 has two caches under one validity predicate: a **path cache** from
(identifier, context) to (endpoint, resolved scope), and a **representation cache** from the
same key to a representation. ikigai has the second and not the first. `root.resolve` runs on
every request, *before* the representation-cache lookup, on purpose: the declared-capability
floor fences cached answers as well as computed ones, and it needs the resolved endpoint's
declaration to do that. So every request pays resolution, and until 0.1.74 it also paid a
fresh `describe()`.

## Why not now — measured, twice

Verified against 0.1.71 on 2026-09-25 (release build, `Fallback` of `Mount`s over
`EndpointSpace`s, 200 000 iterations, first / last binding):

| bindings | cache-hit `issue` | `resolve` alone | `describe()` | `request.id()` |
|---|---|---|---|---|
| 12 | 601–628 ns | 7–21 ns | 129–138 ns | 144–150 ns |
| 300 | 589–662 ns | 7–96 ns | 124–130 ns | 144–155 ns |
| 3000 | 577–902 ns | 7–327 ns | 124–126 ns | 124–145 ns |

Resolution is 1–35 % of a ~0.6 µs read on any root we run today. A path cache would save at
most a few hundred nanoseconds per request — and, without a validity predicate, would serve a
stale route after a rebind, which is a correctness regression bought for a speedup nobody
has asked for.

0.1.74 took the *contract* half of the cost instead: the capability floor memoizes
`describe()` per endpoint identity (`FloorMemo` in `kernel.rs`, keyed by the address of
`Resolved::endpoint`, held by a `Weak` that reserves the address), invalidated by the thread
below. That was ~135 ns (≈ 31 %) off a cache-hit read of an endpoint with an explicit
`ActionSpec` — the module shape — and ~10 ns off a bare `FnEndpoint`. What remains on the
table is resolution itself: 7–330 ns, linear in the bindings a `Fallback` scans.

## The validity predicate exists now

`urn:kernel:bindings` (`ikigai_core::BINDINGS_THREAD`, 0.1.74) means "the set of bindings
this kernel resolves against changed". The catalog, the action manifold, validation reports,
every `Meta` answer and the floor memo hang from it; the party that changed the root cuts it
(`Kernel::bindings_changed`, or `sink urn:kernel:cut urn:kernel:bindings`). A path cache would
be one more thing hanging from it. That it is cut by the host and not observed by the kernel
is the same condition every dependent of the thread already accepts.

## The shape, when it is worth it

- **Key**: `(canonical target, fp(Γ))` — the target *after* the kernel's own alias table and
  after any `Resolved::canonical` a space reported, and the scope fingerprint the
  representation cache already keys on (a corridor can shadow a root door, so a route is
  per chain). **Not the capability**: resolution is capability-blind by construction —
  authority is the floor's job, evaluated per request on the memoized contract — and a rule
  worth stating here so a future `Space` does not quietly make routing depend on who asks.
- **Value**: the endpoint `Arc`, the grammar's `Bindings`, the reported canonical name, and
  the floor (or the whole `Description`). Everything `issue_inner` derives from `resolve`
  before it touches the cache or the endpoint.
- **Validity**: `urn:kernel:bindings`, cut as a whole. There is no per-name invalidation and
  cannot be: the kernel does not know which routes a rebind moved — the same reason a stored
  read does not hang from the thread (`Kernel::bindings_changed`).
- **Position**: after the kernel's own canonicalization and the depth check, in place of
  `scope.resolve_in`. A hit yields exactly what a resolve would have; the floor still runs on
  it, the representation cache is still looked up after, the auto-cut still fires. Nothing
  downstream can tell.
- **Bound**: an entry ceiling with a sweep, like the floor memo — but the endpoint `Arc` must
  be held *strongly* (a route keeps its endpoint alive; the floor memo only reserves an
  address), so an overlay that wraps the endpoint per resolution (every ikigai-throttle
  governor today) would fill it with one wrapper per request. The sweep is therefore by
  recency, not liveness, and such overlays defeat the cache for their prefix until they reuse
  their wrappers.
- **What it must never do**: serve a route computed in one chain to a request in another
  (fp(Γ) in the key); serve a route across a rebind (the thread); let a hit skip the floor
  (the floor runs on the value). And what it cannot cache: a `Space` whose `resolve` is not a
  pure function of `(target, chain)` — one that consults the clock, a counter, or a resource
  — has no place under a golden thread it does not declare, exactly as an endpoint does not.

## The triggers — any one brings this back

- **Per-request scope with repeated contexts** (`docs/design/resolution-scope.md`): as-of
  resolution or a corridor per tenant means the same target resolves in many chains, each a
  fresh walk; the key above is what makes those walks shareable.
- **Expensive grammars**: regex, SHACL-validated captures (ledger
  [#70](http://localhost:1060/l/default/item/70)), anything past a prefix compare.
- **A `Space` whose `resolve` consults a resource**: a routing table in the store, a wasm
  module that resolves in its own code. Then a resolve is a sub-request, and its cost is the
  cost of that resource, not a scan.
- **Thousands of bindings from loaded modules**: 3000 gave 327 ns worst case today on a linear
  `Fallback`; the paper's remedy for large corridors (§14.5) is either a memo — this — or
  compiling the corridor into one automaton, and both need the thread as their predicate.

## Re-measure, do not inherit

The instrument is the twenty-line bench in formalism §10: one cacheable `FnEndpoint` (and one
with an explicit `ActionSpec`), warmed, 200 000 re-issues × 3 rounds, release,
`futures::block_on`, main and branch interleaved. The number to beat is the `resolve alone`
column above, re-taken on the root the trigger produced — not the 2026-09-25 table.
