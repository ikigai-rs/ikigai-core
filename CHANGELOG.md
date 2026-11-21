# Changelog

## 0.4.1 (2026-11-21)

### Changed (behavior)

- **A dynamic space's swap drops the answers that fell through it** (ledger #3916). Through
  0.4.0 a successful swap cut only the answers the space SERVED. A read that missed the space
  and was answered by a space consulted after it was cached without the space's thread, so an
  edit that added a door for that name left the outer answer in the cache: the next read was
  served the outer answer although an uncached resolution reaches the new door. Now a
  resolution that reaches a dynamic space and misses it hangs its answer from the space's
  thread too, so a swap drops every cached entry whose resolution reached the space, hit or
  miss. The answer stays cacheable: a read through an unchanged dynamic space is still served
  from the cache, and only a successful swap drops it. No API change.
- **…and the answers resolved inside a frame it built, and a fallback over a total miss**
  (ledger #3916, round 2). Three more cases of the same gap:
  - A level that a declaration mounts keeps its name across a swap that changes its doors.
    Resolution inside a level consults the level's own space directly, never the dynamic
    space around it, and a sub-request resolved there is keyed by the level's name. So the
    old door's answer was served after the swap. A confinement's corridor had the same gap.
    Now every level and corridor a dynamic space builds carries the space's thread, so what
    is resolved inside it, hit or miss, is dropped by the next swap. A level or corridor
    built anywhere else is unchanged.
  - A composite that caught a sub-request's `Unresolved` and cached a fallback did not hang
    from the dynamic spaces the miss had reached. A swap that bound the name left the
    fallback in the cache. Now the recorded threads ride the `Unresolved` to the composite
    that catches it. The composite stays cacheable.

  No API change.

## Unreleased: needs a minor bump (0.4.0)

This minor release carries the nested confinement below (ledger #3825), the missing scopes on a
capability-floor refusal (ledger #3840), two corrections pinned with three other kernel behaviors
(ledger #3867), and scope opacity, which removes `Invocation::scope` from the endpoint API
(ledger #3866).

### Added

- **A layer declares its kind** (ledger #3881). `ikigai-vocab` adds the property
  `ik:layerKind` (range `skos:Concept`, no domain) and the concept scheme `ik:LayerKinds`
  with five concepts: `ik:TalkKind`, `ik:TaskKind`, `ik:FeedKind`, `ik:InquiryKind` and
  `ik:JoinKind`. Each carries `skos:prefLabel` (the tab a UI groups the layer under: Talk,
  Tasks, Feed, Inquiries, Join), `skos:definition`, `skos:notation` (the key a layer's
  configuration names it by: `talk`, `task`, `feed`, `inquiry`, `join`) and `skos:inScheme`.
  The JSON-LD context coerces `layerKind` to `@id`: `context.gen.py` now reads a
  `skos:Concept` range as IRI-valued.
  The collection `ik:LayerKindOrder` (a `skos:OrderedCollection`) lists the five kinds in the
  order a UI shows its tabs, so a reader orders kinds by its `skos:memberList` instead of
  hard-coding kind names. Its list is written as named cells (`ik:LayerKindOrder:member:{n}`,
  one-based), as `ik:layers` writes its cells, never as blank nodes.
- **A declared-floor refusal carries the scopes the caller lacked** (ledger #3840).
  `Error::lacking()` returns every scope an endpoint declares for the verb that the
  capability does not satisfy, in declaration order and spelled as declared (a family keeps
  its trailing `*`), when the kernel's floor refused the request before dispatch. It is
  empty for every other error, including a `Denied` an endpoint raises itself at run time. A
  host's door reads it to log which capability a refused request needed (an access log's
  `needs=` column) without parsing the sentence; a nested sub-request's floor refusal
  carries its own scope, and an endpoint that propagates it with `?` passes it on. The
  sentence is unchanged byte for byte and still names the first missing scope, as does the
  trace's `denied` note. The kind (`denied`), class (`Denied`), wire tag (4) and HTTP status
  (403) are unchanged. The JSON face (`Error::to_json`, `Error::from_json`) carries the
  list as `lacking` when it is not empty. The shared failure-class vectors gain a
  `denied-by-the-floor` row that pins it, and the plain `denied` row is unchanged.

### Changed (breaking)

- **An endpoint uses its scope only by issuing requests into it: `Invocation::scope` is gone
  from the endpoint API** (ledger #3866). The kernel's semantics require it: the cache keys an
  answer on the request chain's fingerprint, so an endpoint that could read more of its scope
  than the key covers could return a cacheable answer the kernel serves to another scope that
  shares the key. Removed from public API:
  - `Invocation::scope()`, now crate-private. An endpoint never receives a `Scope`.
  - `Scope::spaces()` and `Scope::levels()`, now crate-private. `spaces` handed out live corridor
    spaces, which also let an endpoint invoke a corridor door past its capability floor (the
    scope-hypothesis audit's B3, now closed).

  Kept: `Invocation::now()`, unchanged. Added: `Invocation::clock_pinned()`, whether that time is
  a temporal corridor's, covered by the cache key through the corridor's name. A host's own
  readers on a chain it built stay (`Scope::fingerprint`, `is_severed`, `is_empty`, `clock`,
  `now`, `Display`). Migrating:
  - `inv.scope().clock().is_some()` becomes `inv.clock_pinned()`.
  - `inv.scope().now()` becomes `inv.now()`, which already prefers the chain's clock.
  - Printing `inv.scope()` in a message has no replacement: name the resource or the condition
    instead.
  - A test that probes the chain an endpoint ran in uses `ikigai_core::testkit::{scope, spaces,
    levels}` behind the new `testkit` feature. It is NOT API and carries no semver promise.

- **`Error::Denied` holds a `Denial`, not a `String`** (ledger #3840). A `Denial` is the
  message and the missing scopes. It dereferences to `str` and displays as its message, so
  matching `Error::Denied(m)` and reading `m` (`m.contains(…)`, `format!("{m}")`) still
  compiles, and `Error::Denied("…".into())` still builds one. Building one from a `String`
  needs `.into()`: `Error::Denied(format!(…).into())`. Where a `String` is needed, use
  `denial.into_message()` or `denial.to_string()`. To reword a refusal and keep its scopes,
  use `Denial::reworded`.

### Changed (behavior)

- **A confinement inside a confinement drops the outer confined corridor** (ledger #3825).
  `Scope::confined`, `Invocation::confine` and `Confine` keep only the host's corridors:
  confining removes every earlier confinement, then puts the new corridor behind the host's
  corridors and severs the root. Through 0.3.5 a nested confinement ran in
  `⟨host corridors…, T₁, T₂⟩`: the inner endpoint could still reach the outer confined
  corridor `T₁`, and `T₁`, consulted first, shadowed `T₂`'s own door for a shared name. Now it
  runs in `⟨host corridors…, T₂⟩`, exactly the chain a fresh confinement builds. `Scope::stack`
  follows the same rule: an inner chain's confinement replaces the outer one's. This only
  narrows: dropping `T₁` removes reach and grants nothing. The chain fingerprint follows the
  chain, so a result cached in the old nested chain is not reused.
- **`Meta` of a view inside a `Level` describes the target a `Source` through it reaches**
  (ledger #3867). A view's invocation resolves its target in the view's resolved scope, its own
  level first. `Meta` resolved the target in the request's chain instead, so where a level
  shadowed the target's name, `Meta` described the root's binding while `Source` reached the
  level's. `Meta` now resolves it where the invocation does, and so does each further forward
  it follows. Only a view inside a level whose target name the level also binds sees a
  different description.
- **A cut of an alias's logical name cuts its canonical thread too** (ledger #3867).
  `Kernel::cut` and `urn:kernel:cut` canonicalize the name through the kernel's alias table, as
  a request's target is, and cut the canonical thread after the name as given. Before, a cut of
  the logical name invalidated no read, since every read hangs from the canonical name. The
  name as given is still cut, so a dependency registered under it still invalidates; a name no
  rule matches is cut once, as before. `urn:kernel:cut`'s answer still names the thread as given.

### Pinned (no change)

- **Five kernel behaviors gain tests** (ledger #3867, `tests/view_cut_alias_pins.rs`): a `Sink`
  or `Delete` through a plain view writes the target and cuts both the target's thread and the
  view's name; the two corrections above; the kernel's alias table rewrites a name before
  resolution inside a severed or confined chain too, so the rewritten name is what resolves
  there; and a cycle through a view in one table and an alias in the kernel's table is stopped
  only by the nesting budget (`DepthExceeded`), which is load-bearing for it.
