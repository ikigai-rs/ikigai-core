# Per-entry describe outcomes and enumeration completeness

**Status: design only (ledger #4353). Nothing in this note is implemented.** It follows
[`describe-timeouts.md`](describe-timeouts.md) (ledger #409), which recommended a classified
unresolved catalog entry and a separate incomplete-manifold diagnostic. This note specifies
the two signals that recommendation needs, and how they move through every space combinator
into both consumers and their caches. Who may see an unresolved row, and how the faces render
it, is the follow-up named "Specify diagnostic faces and disclosure" in that note. This note
references that item and does not design it. The code it describes is core main at
`41d76f6`.

## Evidence: what the walk does today

`crates/ikigai-core/tests/describe_outcomes.rs` pins four gaps, each of them today's
behavior. Every assertion is meant to flip when the matching follow-up lands:

- `an_opaque_member_vanishes_beside_an_enumerable_one`. `Fallback::entries` answers `None`
  only when no member enumerates. A member that cannot enumerate, beside one that can, is
  dropped from the listing. Its door still resolves and answers. It is missing from
  `urn:kernel:catalog` and from `urn:kernel:actions`, and neither face says the walk was
  partial.
- `an_opaque_root_leaves_a_manifold_of_kernel_operations_only`. The kernel walks
  `Fallback([KernelOps, root])` (`Kernel::described`), and the kernel's own operations
  always enumerate. So a root that cannot enumerate at all still yields a manifold that
  looks complete. It lists only `urn:kernel:*`.
- `describe_entry_collapses_distinct_reasons_into_none`. `describe_entry` answers `None` in
  four cases: a pattern that is neither an IRI nor a template, a probe that misses, a probe
  that hits a limiter's ⊥, and a template probe that lands on a different binding. A caller
  cannot tell them apart. A grammar that refuses the probe placeholder, which the pin does
  not cover, is a fifth `None`.
- `a_placeholder_not_marked_volatile_makes_the_manifold_cacheable_until_rebind`. The faces
  fold only each contract's own `ContractCaching`. An adapter whose stand-in for a failed
  fetch keeps the default `UntilRebind` makes both faces `Expiry::Never`. The transient
  failure is then cached as an answer until the bindings change. The same stand-in marked
  `contract_volatile` gives `Expiry::Always`. Safety now depends on every adapter
  remembering to mark it.

Reading the code adds three facts:

- **The two consumers do not run the same walk.** The catalog arm
  (`kernel.rs`, `("catalog", Verb::Source)`) probes exact patterns with
  `select::probe` and calls `describe()` directly. It sends only templates through
  `describe_entry`. `select_actions_bounded` sends every entry through `describe_entry`. A
  rule added to one walk is therefore not automatically in the other.
- **The walk calls the infallible `Endpoint::describe`.** A mounted forwarder's `describe()`
  is a `Meta` round trip to the peer, so the peer's failure has nowhere to go except into a
  substituted `Description`.
- **`ChainView` and `Fallback` lose the same information.** `ChainView::entries`
  deduplicates by pattern and answers `Some` if any member enumerates. A severed root is
  left out on purpose. A corridor that cannot enumerate is left out silently.

## Two signals, kept apart

A walk can be partial in two different ways. The design keeps them separate, because each
has a different owner and a different meaning:

1. **Per-entry outcome.** A pattern was listed, and its contract was or was not obtained.
   The answer comes from the endpoint the probe resolved to, which is where the peer's error
   is known.
2. **Enumeration completeness.** Whether the list of patterns is the whole list. The answer
   comes from the space, which is where "I cannot list" or "my listing failed" is known.

A missing row means absence only when the enumeration is complete and no entry is
unresolved. Everything below exists to keep that sentence true.

### The per-entry outcome

Proposed shape. The names are placeholders, to be fixed in the first implementation item.

```rust
/// What one enumerated entry's walk obtained.
#[non_exhaustive]
pub enum EntryOutcome {
    /// The contract was obtained: today's `Some(EntryDescription)`.
    Described(EntryDescription),
    /// The entry names a door, and its contract was not obtained in this walk.
    Unresolved(Unresolved),
    /// The walk established that this pattern offers nothing a caller can reach.
    Absent(Absence),
}

#[non_exhaustive]
pub struct Unresolved {
    pub reason: UnresolvedReason,
    /// How long this outcome may stand. It is derived from the reason, never supplied by
    /// a stand-in `Description`.
    pub persistence: Persistence,
}

#[non_exhaustive]
pub enum UnresolvedReason {
    /// The contract fetch passed its deadline.
    Timeout,
    /// Some other failure, classed by the existing `FailureClass`. Peer error text is
    /// not carried here.
    Failed(FailureClass),
    /// The template's probe could not be formed or resolved: the grammar refused the
    /// placeholder, or the probe landed on a different binding (`describe_entry`'s
    /// identity guard).
    Unprobeable,
}

#[non_exhaustive]
pub enum Absence {
    /// The probe missed. Locally, nothing is bound. From a peer, the peer said "not found".
    Unbound,
    /// The probe hit a limiter's ⊥.
    Limited,
    /// The pattern is neither an IRI nor a template with variables.
    Malformed,
}

pub enum Persistence {
    /// Holds until the bindings change. `Unprobeable`, `Malformed`, `Limited`, and a
    /// local `Unbound` all depend only on the binding set.
    UntilRebind,
    /// Bounded by the authority that answered (a peer's "not found").
    For(ContractCaching),
    /// Holds for this walk only: `Timeout`, and a `Failed` class that is not
    /// `Permanent`.
    ThisWalk,
}
```

Classification of today's `None`s, so no case is dropped silently:

| Today in `describe_entry` | Proposed outcome |
|---|---|
| not an IRI, not a template; a template with no variables | `Absent(Malformed)` |
| probe misses | `Absent(Unbound)` |
| probe hits ⊥ | `Absent(Limited)` |
| template expansion fails, or the grammar refuses the placeholder | `Unresolved(Unprobeable)` |
| template probe lands on another binding (identity guard) | `Unresolved(Unprobeable)` |
| peer's `Meta` times out (today a stand-in `Description`) | `Unresolved(Timeout)` |
| peer's `Meta` fails otherwise | `Unresolved(Failed(class))` |
| peer's `Meta` answers not found | `Absent(Unbound)`, `Persistence::For(..)` |

An unprobeable template is classed *unresolved*, not absent. Today's rule, "better invisible
than misdescribed", still holds: the row is never misdescribed. But the door is bound and
reachable at IRIs other than the probe, so leaving it out silently is the same silent
partial answer this note removes. It is unresolved in a deterministic way, so caching is
unaffected (see [Caches](#caches)). Whether it marks the manifold incomplete for an
unprivileged reader is the disclosure item's decision. This note's recommendation is yes.

**Where the outcome comes from.** A new provided method on `Endpoint`:

```rust
fn describe_outcome(&self) -> DescribeOutcome {
    DescribeOutcome::Described(self.describe())
}
```

`DescribeOutcome` is the endpoint-level half (`Described(Description)`,
`Unresolved(Unresolved)`, `Absent(Absence)`). The walk adds the probe-level classification
from the table and the template variables. A mount adapter's forwarder overrides
`describe_outcome`: when the peer times out it returns `Unresolved(Timeout)` instead of
inventing a zero-verb `Description`. `describe_entry` keeps its signature as
`describe_entry_outcome(..).described()`, so existing callers see exactly today's answers.

### What an `Endpoint::describe` that cannot fail keeps

`describe()` stays, unchanged and infallible. It remains the contract for every path that
already has one request in flight for one target and can fail *that request*: the
capability floor, `Meta` on a single IRI, forwarding projection (`select::probe` describing
a forwarder's target), and `urn:kernel:validate`'s id lookup. It also keeps compatibility:
every endpoint written so far compiles and behaves as today, because the default
`describe_outcome` wraps it. What `describe()` loses is the walk. No enumerating face reads
it directly once the shared walk (follow-up 3) lands.

Two paths keep the infallible call but must stop trusting a stand-in. They are follow-ups,
not designed here:

- **The capability floor** memoizes `describe()` until the bindings change
  (`the_floor_describes_an_endpoint_once_until_the_bindings_change`). A stand-in with no
  `requires` would be memoized as "requires nothing". The floor must consult
  `describe_outcome` and never memoize an `Unresolved`. What it enforces for an unresolved
  door (refuse, or defer to the peer's own check) is a separate decision.
- **`Meta` on an unresolved door** should answer the classified error (`Error::Timeout`, or
  the failure's class) rather than render a stand-in. As an error with a transient class, it
  is not negatively cached.

### Enumeration completeness

Proposed shape, again with placeholder names:

```rust
pub struct Enumeration {
    pub entries: Vec<SpaceEntry>,
    pub gaps: Vec<Gap>,          // empty ⇔ complete
}

#[non_exhaustive]
pub struct Gap {
    pub reason: GapReason,
    pub persistence: Persistence,
    /// The space that could not list, when it names itself (`Space::id`) or carries an
    /// origin. For the inspector only; never rendered to an unprivileged reader.
    pub at: Option<String>,
}

#[non_exhaustive]
pub enum GapReason {
    /// The space does not enumerate (`entries()` is `None`).
    Opaque,
    /// The space enumerates, and this listing failed.
    Failed(UnresolvedReason),
}
```

The new provided method is `fn enumerate(&self) -> Enumeration`. Its default derives from
`entries()`: `Some(v)` gives `v` with no gaps, and `None` gives no entries and one
`Opaque` gap with `Persistence::UntilRebind`. `entries()` keeps its meaning and its answers,
so every reader of it (topology, declarations, hosts) is unaffected. An adapter whose
listing can fail overrides `enumerate` to report `Failed(Timeout)`. Today such an adapter
can only answer `None`, which is indistinguishable from "does not enumerate", or
`Some(vec![])`, which falsely claims the listing is complete and empty.

## Propagation through every combinator

| Space | `enumerate` | Per-entry outcome |
|---|---|---|
| `EndpointSpace` | its bindings, complete | `Described`, or `Unprobeable` per template |
| `Mount` | the inner's, gaps included | the inner endpoint's, via the probe |
| `Level`, `Threaded`, `Framed`, `impl Space for Arc<S>` | forwarded | forwarded |
| `Rewrite` | default: one `Opaque` gap (today `None`) | n/a |
| `Limit` | empty, complete (today `Some(vec![])`) | a probe on ⊥ is `Absent(Limited)`, not a gap |
| `Fallback` | concatenation; gaps are the **union** of every member's | first hit wins, as in resolution |
| `ChainView` | first-binding dedup as today; gaps are the union of consulted members' | first hit wins |
| `Alias` | the inner's plus root rules; the inner's gaps kept | a root rule is described by its target, so the target's outcome |
| `DynamicSpace` | the current version's, gaps included | the current version's |
| an overlay outside core | default from its `entries()`; **should forward `enumerate`** as it forwards `entries` and `topology` | **should forward `describe_outcome`** |

Rules behind the table:

- **Templates.** The probe stays the way a template is described. Each way a probe can fail
  becomes a classified outcome (see the table above), never a silent `None`.
- **Shadowing.** An entry's outcome is the outcome of the door that resolution reaches, so
  shadowing is settled by the probe, as today. An unresolved earlier member **shadows** a
  confirmed later one at the same pattern. The walk must not fall through to the later
  member's contract: that would attribute a contract to a door that resolution does not
  reach. The catalog must not merge an unresolved row into a confirmed contract that shares
  its id, and `shared_id_marks` counts only described rows.
- **Fallback.** A member with a gap is a gap *even when another member enumerates*. This
  flips the first pin. Its door may still be reached through the probe of an exact pattern
  some other member lists. In that case the probe resolves through the whole fallback, and
  the row is described by whatever resolution reaches, as today.
- **Chain views.** The kernel's operations, placed ahead, are complete. A severed root is
  not a gap: severing is the chain's declared shape, not a failure to list. A consulted
  corridor that cannot enumerate is a gap.
- **Failed enumeration.** This is a gap of the failing space, with the listing failure's
  persistence. It never establishes that any particular door timed out.
- **Opaque spaces.** These are a gap with `UntilRebind` persistence. They are honest and
  stable, so they do not make a face uncacheable, but the face is incomplete. This flips the
  second pin: a kernel over an opaque root has an incomplete manifold.
- **Root prefix alias rules** are not gaps. The doors they rename are listed under their
  backing names, once each (alias Decision 4).

## Both consumers, one walk

The catalog and `urn:kernel:actions` should run **one** shared walk instead of two
divergent loops. A crate-private `walk(space) -> Walk` returns, in entry order:

- every `(SpaceEntry, EntryOutcome)`;
- the union of gaps;
- the folded `ContractCaching` of described contracts, and the folded `Persistence` of
  unresolved entries and gaps.

Exact patterns go through the same `describe_entry_outcome` as templates. Then the
catalog's exact path is covered by the same limiter and outcome rules, rather than by its
own `probe` + `describe()`. `Absent` entries contribute nothing to either face, because
absence after a complete walk is the honest answer. A walk is **complete** when it has no
gap and no `Unresolved` entry.

What each consumer does with the walk:

- **Catalog.** It renders described rows as today. It carries unresolved entries and gaps
  to the renderer as their own kind, never as an `ik:Endpoint` with an invented contract.
  How they render, and to whom, belongs to the disclosure item. The catalog already
  requires inspect authority.
- **Action manifold.** Only `Described` rows produce `ActionMatch`es. This is the rule
  today, and it stays: an unresolved door offers no action. An incomplete walk must be
  *said* on every face. The Turtle face gains one incomplete-manifold statement that is not
  an `ik:ActionMatch`. In its unprivileged form, it names no door, peer, count or error
  text. The plain list face cannot add an in-band line without it being read as an IRI and
  piped into a `..` map. So this note recommends carrying "incomplete" as a typed answer
  fact on the response-metadata channel ([`response-metadata.md`](response-metadata.md),
  accepted, not yet implemented). A host that cannot carry the fact refuses the plain face
  with a classified error rather than serve a silent partial list. The exact form is the
  disclosure item's decision. The requirement fixed here is that a partial answer says it
  is partial.
- **Public selection functions.** `select_actions` and `select_action` keep their
  `Vec<ActionMatch>` signatures and today's answers. A new `select_actions_walk` (name open)
  returns the matches with completeness, so a module walking a space outside the kernel can
  learn the same thing.

## Caches

The face's expiry becomes the meet of three bounds: the described contracts' caching, as
today; the persistence of every unresolved entry; and the persistence of every gap.

- `ThisWalk` (a timeout, or a non-permanent failure) gives `ContractCaching::Volatile`,
  that is, `Expiry::Always`. **A transient failure is never cached as an answer**, whatever
  the adapter put in a stand-in. This flips the fourth pin, and the adapter no longer has
  to remember `contract_volatile`.
- `UntilRebind` (opaque, unprobeable, malformed, limited, local miss) adds no bound. It
  changes only when the bindings do, and both faces already hang from `BINDINGS_THREAD`.
- `For(c)` (a peer's "not found") adds `c`. A peer's miss is an answer from the authority,
  so it may be kept, but no longer than the adapter says the peer's listing holds.
- **A refresh never revives stale success.** An entry that was described and is now
  unresolved is unresolved in this walk. Before a refresh policy exists, a face never
  substitutes the last confirmed contract for it.
- **Capability-keyed separation is unchanged.** The manifold is still cached per capability
  fingerprint. Completeness does not depend on the capability, but the rendered diagnostic
  may (disclosure item), so it stays inside the capability-keyed entry.
- **Other caches derived from `describe()`**: the floor memo (above), `Meta` answers, and
  `Kernel::describe` / `describe_pattern`. These must apply the same rule before an adapter
  stops sending stand-ins. Until they do, an adapter should keep marking its stand-in
  `contract_volatile`, as `describe_timeout.rs` does.

The two faces are requested independently, so they may still observe different
snapshots. One shared walk makes each answer internally consistent. It does not make the
two answers simultaneous, and that stays out of scope, as in `describe-timeouts.md`.

## What breaks, for whom

Nothing breaks at compile time. Both new trait methods are provided, the new types are
`#[non_exhaustive]`, and `entries`, `describe`, `describe_entry` and `select_actions` keep
their signatures and answers.

What changes in behavior:

- **Hosts with a space that cannot enumerate** (a `Rewrite`, a remote, a custom space with
  the default `entries`) see their manifold and catalog flagged incomplete where they were
  silently partial. This is the intended change, and a CHANGELOG behavior change. A host that
  wants a complete manifold makes the space enumerate, or limits that family out of the
  walked space.
- **Space implementors** that wrap one space should forward `enumerate` beside `entries`
  and `topology`. If they do not, the default rebuilds completeness from their `entries()`
  and drops the inner gaps, so a failed inner listing would read as complete. Follow-up 1
  adds a conformance check for this.
- **Endpoint wrappers** (governors through `Resolution::map_endpoint` and
  `Resolved::with_endpoint`, `forward::Projected`, alias endpoints, every overlay that
  forwards `describe`) must forward `describe_outcome`. If they do not, the default wraps
  the stand-in `describe()` and the outcome is lost. Core's own wrappers are fixed in
  follow-up 2. Outside core, it is the same convention as `forwards` and `is_limiter`.
- **Mount adapters** should override `describe_outcome` and `enumerate` instead of
  substituting a zero-verb description or an empty listing. They keep a `describe()` for
  the in-flight paths.

## Tests

- `crates/ikigai-core/tests/describe_outcomes.rs`, added with this note, pins the four gaps
  above. Each test's doc comment names the gap it pins. The follow-up that closes a gap
  flips the matching assertion in the same pull request.
- `crates/ikigai-core/tests/describe_timeout.rs` (from #409) stays as it is. Its stand-in
  is marked volatile, which is why its faces are not cached today.

## Proposed follow-up items

Each item is one reviewable pull request. They are in order, and the order matters. Items 1
to 3 change no face's text. Item 3 changes only cache expiry and gives completeness to
callers. Items 4 to 6 change what readers see.

1. **core: `Space::enumerate` and enumeration completeness.** Add `Enumeration` and `Gap`,
   the provided default from `entries()`, and forwarding in every core combinator per the
   propagation table (`Fallback` and `ChainView` take the union of gaps). Add a conformance
   helper that fails an overlay which forwards `entries` but not `enumerate`. No face reads
   it yet. The first two pins flip at the `enumerate` level.
2. **core: `Endpoint::describe_outcome` and `describe_entry_outcome`.** Add the outcome
   types and the classification table. `describe_entry` becomes a projection of the new
   function, with today's answers. Every core endpoint wrapper forwards the method. The
   third pin flips.
3. **core: one shared walk for the catalog and `urn:kernel:actions`.** Route the catalog's
   exact patterns through the same outcome as templates. Fold persistence into each face's
   expiry, so a transient outcome is never cached. Add `select_actions_walk`. Text is
   unchanged; expiry changes. The fourth pin flips.
4. **core and vocab: diagnostic faces and disclosure.** This is the existing
   `describe-timeouts.md` follow-up. It covers the catalog's unresolved-entry node, the
   manifold's incomplete statement, the plain face's fact on the response-metadata channel
   or its refusal, and who may see a named row. It must review existence-sensitive spaces
   before rendering anything. Its message text follows [ACCESSIBILITY.md](../../ACCESSIBILITY.md).
5. **core: the floor, `Meta` and `Kernel::describe` on an unresolved door.** The floor never
   memoizes an `Unresolved`. Decide what the floor enforces for one. `Meta` answers the
   classified error.
6. **mount adapter: report outcomes, not stand-ins.** Override `describe_outcome` and
   `enumerate` in the peer adapter (outside this repository; the text for that project's item
   is drafted in this note's pull request). Add an integration test with the real Turtle
   renderer: timeout, peer not-found, other error, a healthy control, capability filtering, an
   id collision, recovery on the next walk, and cache expiry.
7. **core: release note.** Record the incomplete-manifold behavior change for opaque spaces
   in the CHANGELOG, in the release that ships item 4.
