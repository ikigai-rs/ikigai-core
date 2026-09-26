# Resolution scope: the chain, its two faces, and what the cache owes it

**Status:** built, core 0.1.72 (ledger [#509](http://localhost:1060/l/default/item/509)).
**Anchor types:** `ikigai_core::{Scope, Confine, Invocation::confine, Kernel::issue_in,
Issuer::issue_in_scope, CacheKey::scope, SCOPE_NOTE, SCOPE_MISS_NOTE}`.
**Companions:** `docs/design/sub-request-authority.md` (the authority argument this reuses
verbatim), `docs/design/cache-ejection.md` §2 (a fingerprint is not an identity),
`docs/design/spaces-as-named-graphs.md` (where corridor identity goes next, ledger
[#515](http://localhost:1060/l/default/item/515)).
**Normative reference:** Peter Rodgers, *Peter's Hotel: A Set-Theoretic Formalism* v5.18 —
§3 (context chains, value corridors), §9.1 (the trapdoor, Def. 10), §9.3 (confinement,
Prop. 4), §9.4 and §5.1 (cache sharing across a boundary).

---

## The gap, as found

`Scope { injected: Vec<Arc<dyn Space>> }` existed and was never populated. Every resolve
passed `Scope::empty()`, `EndpointSpace::resolve` ignored it, and every sub-request
re-entered the same root through `issue_under → issue_scoped → issue_inner`. Resolution was
a pure function of one static host-built tree, so the only confinement the kernel could
offer was a capability refusal. The paper's claim, which this arc makes true here: *a
denial is a decision that can be misconfigured; an unresolvable identifier has nowhere to
go.* Inside a severed chain a resource outside it must be `Unresolved`, not `Denied`.

## The primitive: one chain, two faces

A request is resolved in a **chain** `⟨injected corridors (innermost first), root⟩`.
`Scope` is that chain minus the root the kernel holds: the corridors, their identities, and
whether the root is still on the end. `Scope::empty()` — nothing injected, root present —
is what every plain `Kernel::issue` resolves in, and it is the status quo by construction
(fingerprint `0`, no trace note, key unchanged).

The kernel walks it in `Scope::resolve_in`: each injected corridor innermost first, then
the root unless severed. Both faces are that one walk:

| | **Injection** | **Severing** |
|---|---|---|
| Chain | `⟨C…, root⟩` | `⟨host corridors…, S⟩`, no root |
| Effect | `C` **shadows** a root door, for the request and every sub-request | anything not in the chain is `Unresolved` |
| Paper | value corridor (§3) | trapdoor (§9.1) |
| API | `Kernel::issue_in(request, cap, Scope::empty().with_named(name, C))` | `Confine::new(name, S, inner)` / `inv.confine(name, S)` |
| Who | the host, holding the kernel | any endpoint, on itself |
| Worst outcome | a spoofed door for everything below | an `Unresolved` |

The chain is a property of the **request** and travels with it: the kernel stamps the
`Invocation` with the chain its request resolved in (`Invocation::with_scope`, crate-private),
and `issue_under` / `fan_out` hand it to `Issuer::issue_in_scope`, so a confinement holds
under everything below the endpoint that entered it — including across a `fan_out` spawn
(test: `fan_out_from_a_confined_endpoint_stays_confined`).

## The nine decisions

### 1. The cache key gains a scope dimension

`CacheKey` is now `(request id, capability fingerprint, scope fingerprint)`. Paper §9.4: a
cached representation may be shared across a boundary only if the corridors actually
consulted are the same on both sides. The same request id under the same capability can
resolve to a **different endpoint** in a different chain (`urn:shared` bound in the root and
in a corridor), so a key without the chain would serve one chain's answer to the other —
silently, types identical, tests green: the ~2000× cms-web shape with the sign flipped.

The fingerprint is over the **whole chain**, not the corridors consulted — sound and
over-partitioned. Two chains differing only in a corridor neither request touched do not
share. Consulted-corridors-only is a later refinement and needs the resolver to report
which corridor answered (it can: `Resolved` is the place, the way `canonical` is reported).

The empty chain fingerprints to `0`, `CacheKey::new(id, cap)` sets `scope: 0`, so every
key built before this change is byte-for-byte the empty chain's key. Tests:
`the_empty_scope_is_the_status_quo`, and the §9.4 pair
`a_result_computed_{outside,inside}_confinement_is_not_served_{inside,outside}_it` — both
directions, with invoke counters.

`urn:kernel:*` is keyed with scope `0` whatever chain the caller ran in: it is resolved
ahead of the chain (decision 4), so its answer does not depend on it.

### 2. What is fingerprinted — a name the injector supplies

Spaces have no identity (#515). `Arc` pointer identity would partition a process-local
cache soundly and would be useless for the case injection exists for: a temporal corridor
rebuilt per request is a new pointer every time, so nothing ever shares. (And an address
is reused after the `Arc` drops while the cache entry lives on — unsound as well as
useless.) So the identity is a **name the injector supplies**: `Scope::with_named(name: Iri,
space)`, the value corridor's identifier in the paper's terms
(`urn:ctx:time:2026-09-25T18:00Z`).

**The name is a claim**, exactly like `Resolved::canonical`: same name ⇒ same doors ⇒ same
resource. Name two different corridors alike and one request is served the other's cached
answer. Said on the type, with a doctest that shows two freshly built corridors under one
name producing one cache entry and a different name producing a second.

`Scope::with(space)` (unnamed) is kept — it was public — and mints a process-unique
identity from a counter, so an anonymous corridor shares with its own clones and nothing
else. Its doc says so and points at `with_named`.

**What #515 still needs**, stated for the hub rather than done here:

- A space's identity should be **on the space**, not supplied at injection: a `Space` that
  knows its own IRI could be injected without naming it, be listed in a topology, and be
  named by a trace as *the corridor that answered*. Today identity is a property of the
  injection, so the same space injected under two names is two cache partitions.
- Chain order is part of the fingerprint (`⟨a, b⟩ ≠ ⟨b, a⟩`), correctly — but that means a
  topology resource has to represent order, not just membership. `ik:layers` in
  `spaces-as-named-graphs.md` is an ordered list; keep it one.
- The trace names the chain (decision 9), not the corridor that answered. Naming the
  answering corridor needs the identity to come back on `Resolved`, which is the same
  seam consulted-corridors-only caching needs. One change serves both.

### 3. Injection is authority — only the host may inject with root

Whoever pushes a corridor innermost can stand in for `urn:personal:contacts` for every
sub-request of that resolution. So the widening face is `Kernel::issue_in`, reachable only
by whoever holds the `Kernel` — the trust line of `Capability::root()`. Holding a `&dyn
Issuer` for the kernel is the same trust line (`issue_scoped` already lets an issuer-holder
pass any capability), and `Invocation`'s issuer field is private, so an endpoint cannot get
one from its invocation.

From inside an endpoint the only chain-changing operation is `Invocation::confine(name,
space)`, and its shape is the argument: it takes the chain the invocation runs in, puts
`space` **behind** every corridor already there — where the root used to be — and cuts the
root off. Relative to the chain it started in, nothing resolves differently except what the
root would have answered. It cannot get ahead of a corridor the host injected, so it can
never shadow one; the root's doors are exactly what confinement exists to remove. The
worst outcome is an `Unresolved`. This is `issue_attenuated`'s shape applied to the chain,
and `sub-request-authority.md`'s load-bearing fact applies verbatim: *an
authority-carrying sub-request takes its authority from the kernel, never from its
caller.* `Invocation::with_scope` is `pub(crate)` for the same reason `issue_under` is
private.

Nested confinement follows: an endpoint confined to `S1` that confines again with `S2`
runs in `⟨host corridors…, S1, S2⟩`, severed — `S2` adds doors `S1` lacks and shadows
nothing. Test: `an_endpoint_can_only_narrow_its_chain`, which also pins that an endpoint
reached *through* the corridor is confined too (the chain is closed under the subtree).

An open question recorded rather than decided: whether `confine` should be allowed to
**drop** the host's corridors (strictly narrower still, but it would make a pinned
`urn:time:now` unavailable inside rather than pinned). Kept them, on the view that the
host's per-request context is a claim about the request, not a privilege of the endpoint.

### 4. `urn:kernel:*` stays outside the chain

The kernel intercepts its own namespace before the root and before the chain; `Alias`
refuses to alias it; an injected corridor cannot shadow it either, and a severed chain
still reaches it (capability-gated as ever). Consequence worth naming: a confined endpoint
can read `urn:kernel:catalog` and see a list of names it cannot resolve. Names, not
content — the same leak `sub-request-authority.md` accepts for golden threads. Test:
`an_injected_corridor_cannot_shadow_urn_kernel_and_a_severed_chain_still_reaches_it`.

### 5. The capability floor is unchanged and runs against the endpoint that answered

Whichever corridor answered, `unsatisfied_scope` is evaluated on *that* endpoint's
declaration, after resolve and before the cache lookup — so a corridor shadowing an open
root door with a gated one is gated, and its cached entry is fenced from a caller the
declaration refuses. Test:
`the_capability_floor_is_evaluated_on_the_corridor_endpoint_that_answered`.

### 6. Values are not corridors here

The paper carries a request's values as transient corridors so they can be resolved by
identifier, and notes that value corridors accumulate down the chain so a trapdoor exposes
all of them unless filtered. ikigai's values already travel **on the `Request`** as
`ArgRef`s, and a confined sub-request carries its own request's args and nothing of its
parent's — there is no accumulation to filter. Modelling value corridors would add a second
way to say what the request already says, and a second thing the trapdoor has to strip.
Not modelled; if a by-reference argument (`ArgRef::Reference`) names something outside the
chain, dereferencing it inside is `Unresolved`, which is the trapdoor working.

### 7. The wire drops the scope — a named hole, in two places

`Issuer` implementors outside core (`ikigai-module`'s `SessionHostIssuer`,
`SocketHostIssuer`, `HostBridge`) implement only `issue`, and a mounted remote cannot carry
a chain at all: the wire has no field for it. So a `Confine`d endpoint that reaches a mount
inside `S` escapes the confinement **at the wire** — the remote resolves in its own root.
In the paper's terms a mount is a *named exception endpoint* whose external access is part
of the boundary (Prop. 4): confining to a corridor that contains a mount confines to
whatever the mount reaches. That is the host's decision to make when it builds `S`, and it
is not closable from core without a protocol version bump (a chain on `Call::Issue*`),
which is a different arc.

The second place is closable and is closed: `Issuer::issue_in_scope`'s **default refuses a
non-empty chain** (`Error::Endpoint`, naming the chain and the limitation) instead of
dropping it. An empty chain delegates, so every existing issuer behaves exactly as before;
a module endpoint inside a confinement — or under a host-injected corridor — fails loudly
rather than resolving in the plain root on the branch that looks like success. Test:
`an_issuer_that_cannot_carry_the_chain_refuses_a_non_empty_scope_rather_than_escaping_it`.
The cost: a host that starts using `issue_in` with module endpoints in the path gets a
refusal until `ikigai-module` implements the seam (three sites, each a delegation to
`issue_in` on its kernel once the wire carries the chain; until then they cannot).

### 8. Public surface: additive

- `Scope`: `with_named`, `sever`, `confined`, `is_severed`, `is_empty`, `fingerprint`,
  `Display`. `with`, `spaces`, `empty` unchanged. Representation changed (a thin
  `Option<Arc<Chain>>` handle) — fields were private.
- `Issuer::issue_in_scope` — new **defaulted** method; `issue_scoped`'s signature untouched.
  Grepped: no implementor of `issue_scoped` or `issue_with_parent` exists outside core;
  the three module issuers implement `issue` only.
- `Invocation::confine`, `Invocation::scope` — new. The scope field is private, so
  additive.
- `Kernel::issue_in` — new.
- `Confine` — new type, new module.
- `CacheKey::scope` — new **public field**, `#[serde(default)]`, plus `in_scope`.
  Grepped: nothing outside core constructs or destructures `CacheKey` (the two-argument
  `new` is unchanged). The serialized form gains a field; nothing consumes it yet
  (`cache-ejection.md` is design only).
- `SCOPE_NOTE`, `SCOPE_MISS_NOTE` — new constants.

Nothing forced a breaking change. Bumped 0.1.71 → 0.1.72: additive API is a *minor* in
semver terms, but the ecosystem is lockstep on `^0.1.x` — a `0.2.0` would ceiling out
every one of the ~31 consumers (rule 5's second half) for a change none of them has to
adopt.

### 9. Traced without a struct change

Two reserved note keys beside `DENIED_NOTE` / `ALIAS_NOTE`, so `TraceEvent`'s postcard
layout is untouched:

- `SCOPE_NOTE` (`"scope"`) — on every event of a non-empty-chain resolution (computed,
  cache hit, denial, miss), paired with the chain as `Scope` renders it: innermost first,
  ending in `root` or `severed` (`"urn:ctx:doc:1 severed"`). The two terminal tokens carry
  no colon, so they cannot be confused with a corridor IRI; an anonymous corridor renders
  as `_:<n>`.
- `SCOPE_MISS_NOTE` (`"scope-unresolved"`) — a miss **inside** a non-empty chain is now
  recorded (a plain miss in the empty chain still records nothing), paired with the target.
  The error is `Unresolved`, indistinguishable from "no such resource" by design; the trace
  is the only place "this name IS bound, just not in here" can show.

Empty-chain events are byte-identical to before. `ikigai-log` maps note keys straight
through its vocabulary table, so these need one term each there. Test:
`the_chain_is_disclosed_on_every_traced_event_and_a_confined_miss_is_traced`.

## The clock seam — an open question, not decided here

`Invocation::now()` reads the injected `Clock` through the issuer. A temporal corridor
pinning `urn:time:now` to a value is a **resolution-seam** claim about time; `now()` is a
**clock-seam** claim; and an endpoint stamping `derived_at` from `now()` inside such a
corridor would report the wall clock beside data resolved as-of the pinned instant. Either
the corridor also carries a `Clock` the invocation prefers (a `Scope`-level clock, the
`with_clock` precedent), or `now()` becomes sugar over resolving `urn:time:now` in the
chain, or the two are declared different questions ("when is it" vs "as of when am I
looking"). Settle it before the first as-of demo, or the demo lies about one of the two.

## Not built, on purpose

Temporal / geographic / personal corridors, a context resource, the limiter (#511), the
depth budget (#513 — and #25's re-entrancy budget is adjacent: a confined chain can still
recurse through a corridor endpoint that calls itself), consulted-corridors caching, the
chain on the wire, space identity beyond decision 2, the topology resource. `Kernel::probe`
/ `is_cached` and `urn:kernel:cache` answer for the empty chain only. `select_transreptor`
/ `select_action` select over the **root** even from inside a confinement, so the manifold
can offer a transreptor the chain cannot then resolve — selection has not learned the
chain; a `select_*_in` over the corridors is the obvious next step and not this one.

## The read measurement

Cache-hit read on the empty-scope path (`Kernel::issue`, one cacheable `FnEndpoint`,
warmed once, 200 000 re-issues per round, three rounds, release, `futures::block_on`, M-series
laptop). The first round of each run is cold and reported for honesty; the warm rounds are the
number.

| tree | run | round 0 (cold) | round 1 | round 2 |
|---|---|---|---|---|
| main (0.1.71), machine otherwise idle | 1 | 540 ns | 410 ns | 410 ns |
| main | 2 | 546 ns | 411 ns | 410 ns |
| main | 3 | 438 ns | 408 ns | 408 ns |
| first cut (`Scope` = two `Vec`s by value), idle | 1 | 553 ns | 434 ns | 433 ns |
| first cut | 2 | 472 ns | 426 ns | 426 ns |
| **interleaved, another arc building on the box (load ~2.5)** | | | | |
| main | i1 | 549 ns | 412 ns | 411 ns |
| shipped (`Scope` = `Option<Arc<Chain>>`) | i1 | 552 ns | 420 ns | 421 ns |
| main | i2 | 526 ns | 413 ns | 410 ns |
| shipped | i2 | 541 ns | 411 ns | 413 ns |

The first cut cost a real +16–24 ns (~5 %): a `Scope` of two `Vec`s moved through the
async state machine and cloned into every invocation. Making the empty chain a null handle
took most of it back: interleaved under identical load the shipped tree sits 0–10 ns
(≤ 2.5 %) above main, inside the run-to-run band. What remains is plausibly the key itself
— `CacheKey` grew from 40 to 48 bytes and a hit hashes it twice (`get`, then `get_mut`) —
and was not chased further. Not the kind of number the ~2000× scar is about — that one came
from an added *source*, which this arc adds none of — but it is the number the brief asked
for, and it was worth the second cut.
The bench is not committed; it is twenty lines against the
public API and the table above is what it printed.
