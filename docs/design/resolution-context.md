# The resolution context: levels, the resolved scope, and sealed names

**Status:** phase 1 of ledger [#563](http://localhost:1060/l/default/item/563), built in
three PRs in this repository: `Level`, the found path and the resolved scope (PR #132); sealed
names (PR #133); the pushdown gatekeeper walk and the formal document (the third). Unreleased
at merge: the hub runs the release preflight.
Brian, 2026-09-27: *"I think the resolution context might be worth having."* The five design
questions were decided 2026-09-28 (all as recommended, below), with one requirement added: a
module must not be able to override core or security names — sealed names.
**Anchor types:** `ikigai_core::{Level, LevelPath, Resolved::levels, Resolved::within,
Scope::levels, LEVEL_NOTE, SpaceKind::Level}`.
**Companions:** `docs/design/resolution-scope.md` (the chain this extends — injection,
severing, the fingerprint), `docs/design/sub-request-authority.md` (attenuation, unchanged
here), `docs/formalism/README.md` §1.1 and §7 (the formal claims, rewritten for levels in the
third PR).
**Normative reference:** Peter Rodgers, *Peter's Hotel: A Set-Theoretic Formalism* v5.24 —
§3 (an endpoint runs in the scope at the level where it was found; Corollary 2, richer context
and shorter identifiers), B.2; and NetKernel, which keys a cached representation on the
RESOLVED scope (kernel 1.30.1).

---

## The gap

Until this arc a request had ONE scope. Its chain was the host-injected corridors, innermost
first, then the root — and the root was one corridor however it was composed. An endpoint
found anywhere in the root ran in the caller's whole chain, and its sub-requests started again
from the top (`Invocation::issue_under` → `Kernel::issue_inner` → `Scope::resolve_in` from the
first corridor). The paper and NetKernel keep a SECOND scope: the one the endpoint was FOUND
in. The endpoint runs there, and its sub-requests resolve from its own level outward. That
second scope is what makes names module-relative and internals private — a module exposes a
small surface, and its endpoints reach their own siblings first.

## The construct: an explicit, always-named `Level`

**Decided (Q1): opt-in.** A level is a new space, `Level::new(name, inner)`, and nothing in a
kernel changes until a host wraps a space in one. The alternative — every `.named()` space a
level — would have silently changed resolution in gonk (ten files name spaces), the cli (two)
and the tutorial (one). `Mount`, `Fallback`, `Rewrite`, `Alias` and `Limit` are not levels and
keep their meaning.

A level is always named because the name enters the cache key and the topology. It is the
claim every name makes in this crate — *same name ⇒ same doors* — and it is also the level's
`Space::id`, its `answered_by` when nothing inside it named itself, and its `ik:Level` node.

## The found path

`Resolved` carries the levels a resolution was found in, innermost first
(`Resolved::levels`). A `Level` appends itself on a hit, as the next-OUTER frame
(`Resolved::within`): the inner level has already pushed itself by the time the outer one sees
the hit. Anonymous combinators forward the path untouched; `map_endpoint` and `with_endpoint`
keep it through decoration, as they keep `canonical` and `answered_by`. (The brief said an
outer level "prepends"; with the path innermost first, the outer level is the one that goes
last.)

**Not a flag day.** The path is a PRIVATE field. `Resolved` had all-public fields and no
`#[non_exhaustive]`, and each of the last two fields added (`canonical` 0.1.64, `answered_by`
0.1.78) broke every struct literal outside core. A private field closes that door for good: a
literal outside this crate no longer compiles (E0451), for this field and every later one. The
ecosystem was grepped before the change and holds no literal — the last ones moved to
`Resolved::new` at 0.1.78 — so the closing cost no consumer a compile. `Resolved { endpoint,
.. }` still matches.

## The resolved scope

The request still resolves in its **resolution scope** (the `Scope` it was issued in). The
endpoint runs in its **resolved scope**, and its sub-requests resolve there:

1. **the host's injected corridors, unchanged and whole** — decided (Q2): injected first. They
   are host-chosen context (a temporal corridor, a game, a principal) — the paper's "value
   corridors that are kept" — and keeping them first keeps the host's authority to stand in for
   any name, the module's internal ones included. The tutorial's tic-tac-toe rules at the root
   still reach `stored:{x}:{y}` in the game's corridor. Home-first would be closer to
   NetKernel, where a module's own names win, but a host could then no longer override a
   module's internal name with a corridor.
2. **the found level, then each enclosing level outward** — each consulted as its enclosed
   space: the level's own doors, WITHOUT the guard it was entered through.
3. **any confined corridor** — where the root was (below);
4. **the root, unless severed.**

An endpoint found outside every level gets exactly the chain the request resolved in. That is
the backward-compatibility property, and it is pinned three ways at once by
`levels::a_kernel_without_a_level_is_byte_identical_in_answers_cache_keys_and_traces`: a fixed
workload's answers, every endpoint's `inv.scope()` and its fingerprint, every traced event, the
`urn:kernel:cache` readout and the fingerprints of six host-built scopes, compared against the
transcript main produced at `7774fc2` before any of this existed.

Mechanically the chain gains two things and changes nothing else: a **level stack**
(`Scope::levels`), and a count of how many of its corridors a confinement placed rather than a
host injected. `Scope::descend(path)` (crate-private) swaps the level stack for the found path
and keeps everything else; for a chain with no levels and an empty path it returns the very
chain, without an allocation. A hit from the level stack at position *i* reports the path the
level's own space found, then levels *i…* — so an endpoint found there runs from ITS level
outward, as one found from the root through the same levels would.

### A `Mount` guards the way in, not the module's own sub-requests

**Decided (Q3).** The module shape is `Mount(prefix, Level(name, inner))`. A request from
outside must pass the prefix guard. A sub-request from an endpoint inside the level resolves at
the level itself, so a short internal name reaches its siblings without matching the prefix —
module-relative names — and the same name from outside meets the guard: private internals. The
other reading (the mount also guards the module's own sub-requests) would leave a module unable
to name its siblings except under its public prefix. Capabilities are unchanged: every
sub-request carries the attenuated capability, and the declared floor applies inside a level as
anywhere (`levels::the_capability_floor_and_attenuation_still_apply_inside_a_level`).

### Confinement leaves the level stack behind

`Scope::confined` keeps its meaning: the confined chain is the host's corridors, then S, and
nothing else. The levels are part of the ARRANGEMENT — the side of the chain the root stands on
— so they are cut off with the root. A level INSIDE the confined corridor is still a level: an
endpoint found in it runs from its level, then the corridor holding it, and no root. That is
why a confined corridor is counted separately from an injected one: it is consulted after the
level stack, where the root was, while an injected one is consulted before it. With no levels
the two runs are adjacent and the walk is the walk it was.

**Decided (Q5): later.** "Confine to my level" — sub-requests see the injected corridors and
the endpoint's own level, nothing outward — would make a module sandbox one call. It is not in
phase 1.

## The cache key includes the level path — mandatory, not an optimization

**Decided (Q4): the whole resolution scope, sharing later.** A sub-request issued from level L
can resolve a short name differently from the same name issued from level M. So the key must
distinguish them, and it does: the chain's fingerprint covers the level stack by name. Only
when there is one — a chain without levels hashes exactly the bytes it hashed before, so every
key built before levels existed is still the key. With a level stack, the split between the
host's corridors and the confined ones is hashed too, because it decides whether a corridor is
consulted before the levels or after them.

The top-level request that FINDS an endpoint in a level is keyed in its own resolution scope
(the empty chain, typically): where it was found is a function of its name and the chain it
resolved in, and both are in the key. Pinned by
`levels::two_levels_binding_the_same_short_name_keep_separate_cache_entries` (two rows in
`urn:kernel:cache` for one request id, one per level) and
`levels::the_level_path_is_in_the_fingerprint_by_name_and_only_when_there_is_one`.

This is sound and over-partitioned: two scopes that consulted the same corridors do not share
an entry. Keying on the FOUND part of the scope — NetKernel's up-to-eight keys per entry,
ledger [#548](http://localhost:1060/l/default/item/548) — is phase 2, after measuring the
fragmentation.

## Space-scoped transreptors

Selection walks the resolved scope with no new code: `Invocation::select_transreptor` and
`select_action` pass the invocation's chain, which is now the resolved scope, and
`Scope::consulted` — the one walk selection and resolution share — walks the level stack. So a
transreptor a module's level binds is the one its endpoints plan through, and it is not offered
at the root. The `Meta` arm plans and runs its transreption in the resolved scope of the
endpoint described, so a module's Meta faces are converted by the module's own transreptors.
Pinned by `levels::an_endpoint_plans_through_the_transreptor_its_own_level_binds`.

## Legibility

- The trace: every event of an endpoint found in a level carries `LEVEL_NOTE`
  (`("level", "urn:…:inner urn:…:outer")`), beside `ANSWERED_NOTE`; a sub-request resolved in a
  resolved scope carries `SCOPE_NOTE` with the level stack rendered as `@name`
  (`@urn:example:level:mod root`) — `@` cannot begin an IRI, so a level is never read as a
  corridor. Neither appears without a level.
- The topology: `ik:Level` (named, `ik:space` its enclosed space) in the tree, and in a chain's
  `ik:layers` a level of the stack between the injected corridors and the root. An endpoint
  inside a level that reads `urn:kernel:topology` sees its own resolved scope, because the
  topology is keyed by, and rendered from, the chain it is issued in.
- `ik:Level` is a new class in `vocabulary.ttl`: **a semantic vocabulary change**, so the
  context is regenerated, and the `/ns` deploy after the vocab publish is a real change.

## Sealed names

**Brian, 2026-09-28:** a module must not be able to override core or security endpoints; and
*"arbitrary modules should be able to introduce new sealed names that don't conflict with
others."*

**Why it is needed.** A `Mount` guard limits what ENTERS a module, not what a module binds.
With levels a module could bind `urn:sign:trust-set` or `urn:secret:get`, and every sub-request
resolved at its level would get the module's answer. That cannot raise authority (the fake
runs under the same attenuated capability), outside callers never see it, and the level path
in the key keeps it out of everyone else's cache — but it is a confused-deputy hole wherever
TRUSTED code runs inside a module's level: an imported library, a runtime, a shared verifier
resolving its trust set.

**The table.** A kernel holds sealed prefixes, each with exactly ONE owner:

| owner | seals | how |
|---|---|---|
| core | `urn:kernel:` | as always: the kernel answers its namespace ahead of every chain |
| the host | its own list | `Kernel::with_sealed([...])`, at build |
| a level | names in its own namespace | `Level::sealing([...])`; ikigai-module reads them from a manifest in phase 3 |

`Kernel::sealed()` lists them; `Kernel::check_sealing(root, host)` is the fallible check.

**At resolution** (`Scope::resolve_in`): a sealed name takes the ordinary walk with the levels
that do not own it left out. A core- or host-sealed name skips every level: the host's injected
corridors, then the root. A name sealed by level M skips every level but M: from inside module
N it reaches M's real binding, never a copy in N; M's own sub-requests resolve it at M; from
the root it is answered only through M. The host's **injected** corridors may still stand in
for any sealed name — injection is already host authority (`Kernel::issue_in`). A **confined**
corridor may not: a confinement is an endpoint's own choice, and a trusted verifier confined to
a corridor holding a fake trust set is exactly the deputy the rule exists for. Inside a
confinement a sealed name the host did not inject is therefore `Unresolved` — narrower than
today's reach, in the safe direction, and only for a kernel that seals something.

**At build** (`Kernel::new`, `Kernel::with_sealed`): the root's topology is walked, and three
things are refused, naming what collided — a builder panics with the message,
`check_sealing` returns it as a `SealError`:

- **A door where the owner is not.** A door whose literal head is INSIDE a sealed family binds
  the sealed name by name; held by anyone but its owner (a core or host seal inside any level;
  a level's seal inside another level, or outside every level) it is refused outright, reachable
  or not — a binding the seal makes dead is exactly the silent skip the rule refuses. A template
  whose head the family EXTENDS (`urn:{ns}:{id}` against `urn:sign:`) binds nothing by name; it
  is refused only where a request for a sealed name can reach it — from the root through every
  mount above it, or through its owner's frame. Doors are bindings' patterns and alias rules'
  logical names (a table rewriting a sealed name answers it as surely as a binding). A
  limiter is a hole, not a door — it answers nothing, so it fakes nothing — and a host's
  limiter over a module's sealed name is a gatekeeper; limiters are never counted, at build
  or on resolution. Core's own seal needs no door check: no door can ever answer
  `urn:kernel:*`.
- **A seal outside the level's namespace.** Its namespace is the prefix it is mounted under
  (the mounts between it and its enclosing level), or one the host accepted with
  `Level::in_namespace`. A module that tries to seal `urn:sign:` is squatting — refused.
- **An overlap.** A claim equal to, inside, or enclosing another owner's is refused, naming
  both. Core and the host are checked first, so a module can never take one of theirs.

**What the topology cannot show** — an opaque space or a closure `Rewrite` inside a level, a
level hidden under an overlay that does not forward `Space::topology` (ledger
[#546](http://localhost:1060/l/default/item/546)) — is checked on every resolution: a sealed
name (the target, or a canonical the hit reported) found at a path whose innermost level is not
its owner, and a hit through a sealing level the kernel never registered, are refused with an
`Error::Endpoint` naming the name, the owner and the level, and traced under `SEALED_NOTE`.
Never a silent skip.

**What it costs.** Construction walks the topology only when there is something to check: a
host seal, or a sealing `Level` anywhere in the process (a monotone flag set by
`Level::sealing`), so a kernel built without either is built exactly as before. On resolution a
kernel with only core's seal pays one `bool` read; with eight host seals a cache-hit read
measured 306–314 ns against 307–321 ns without (noise), because the table is a short linear scan
of prefixes, and a level-stack frame is skipped with one comparison.

**Proposed default list for the cli** (Brian approves; a consumer change, not core's):
`urn:cap:`, `urn:secret:`, `urn:sign:`, `urn:encrypt:`, `urn:passkey:`, `urn:clock:`,
`urn:time:`, and `urn:iki:ledger:` for the ledger's write names — a seal is a prefix and says
nothing about verbs, and the write names are templates in the middle
(`urn:iki:ledger:{ledger}:append`), so the namespace is the one prefix that covers them; its
reads should come from the real ledger too. None of these is bound inside a level today, so
sealing them changes no answer; it closes the hole before the first module level exists.

## The gatekeeper check follows the pushes

Before levels the static tree contributed no pushes, and "is a door of the family reachable
from the entry without a limiter over it ahead of it?" was a path query over
`urn:kernel:topology` (formal document §1.1, R7.3). A `Level` is the one node that pushes: an
endpoint found under it resolves at the level's own space, without the guard it was entered
through. So the walk in `tests/topology.rs` now does two things:

1. **The entry walk**, as before — `ik:Level` is transparent from outside — noting every level
   ENTERED: one whose doors an outside request can reach, by any name.
2. **The pushes**: each entered level's resolved-scope stack (the level, then its enclosing
   levels) is walked again from the level itself, with no gate and only the walls the host's
   corridors put ahead of everything. Pushes discover further entered levels; each stack is
   pushed once. The stacks are the tree's own level paths, so the closure is bounded by the
   level nesting. The root is not re-walked from a push: it is consulted after the frames,
   behind at least the walls the entry walk met, so it can only reach less.

A sealed family has exactly the reach it had without levels: its doors count only where its
owner is (never inside a level for a host seal — the walk is told the host's seals, which are
kernel configuration and not in the graph; only inside the sealing level for a level's seal,
which `ik:seals` states). Doors met through a non-owner frame on the way to a nested owner are
counted: an over-approximation, in the direction a gatekeeper check can afford.

The SPARQL form stays a query for arrangements without levels, and says honestly where it
stops: its second question gains a branch that reports any `ik:Level` on a reachable path, so
"safe" is not available to the query for a tree with levels — the walk answers there. Run over
four rendered graphs in an in-memory oxigraph store: the §12.5 arrangement `false`/`false`
(safe), the open root `true`/`false`, a module whose level binds a personal door behind the
root's limiter `false`/`true` (the walk: reachable, and the kernel serves it), a module with no
door of the family `false`/`true` (the walk: unreachable). Without the new branch the third
graph read `false`/`false` — the query would have called a leak safe.

## What phase 2 should measure

- **Fragmentation from whole-scope keying.** Every sub-request from inside a level is keyed by
  its resolved scope, so two modules reading the same root resource share no entry. Count, on
  a real host (gonk, the cli), distinct `urn:kernel:cache` rows per request id once modules are
  levels, and the hit rate on root reads issued from inside levels versus from the root. That
  number decides whether found-scope keying
  ([#548](http://localhost:1060/l/default/item/548)) is worth its complexity.
- **The descent cost on a hot composite.** ~170 ns per invocation inside a level (the table
  below). If a profile shows it, memoize the resolved scope per (chain, found path) — the
  inputs are two `Arc`s, so the memo key is cheap.
- **Construction cost of the seal walk** on the largest real root (a module host with thousands
  of bindings), once a sealing level exists in the process: `Kernel::new` walks the topology
  then, and nothing measured it on a big tree.
- **Thread invalidation across levels** ([#581](http://localhost:1060/l/default/item/581)):
  a golden thread is a name, so a write through one level cuts a same-named resource another
  level answers. Sound (over-invalidation) but unmeasured.

## The read measurement

Same instrument as `resolution-scope.md` and the formal document's §10: a scratch crate against
the public API, release, `futures::block_on`, warmed, 200 000 issues × 3 rounds, main (an
export of `7774fc2`) and branch interleaved three times, M-series laptop, load 3.0–3.6. Not
committed.

| case | main | branch |
|---|---|---|
| cache-hit read, one cacheable `FnEndpoint` | 314–320 ns | 312–317 ns |
| composite (uncacheable) sourcing a cached sibling, no level | 962–1026 ns | 945–978 ns |
| cache-hit read through `Mount(Level(…))` | — | 341–362 ns |
| the same composite inside `Mount(Level(…))` | — | 1117–1144 ns |

No regression without a level: `resolve_in`'s empty-chain fast path is untouched, `descend` of
an empty path over a level-less chain is a clone of the handle it already cloned, and the only
new work on a hit is one empty `Vec` moved inside `Resolved`. A hit through a level costs
~30 ns (the level's name cloned into `answered_by` and one frame pushed). A composite that runs
inside one costs ~170 ns more per invocation: `descend` builds the resolved scope — one `Arc`,
one BLAKE3 over the level names — and the sub-request walks the level frame before the root.
Phase 2 can take most of that back by memoizing the resolved scope per (chain, path), if a
consumer's profile says it matters.
