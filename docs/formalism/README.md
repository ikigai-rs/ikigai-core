# The ikigai realisation

**An appendix-shaped companion to** Peter J. Rodgers, *Peter's Hotel: A Set-Theoretic
Formalism*, version 5.18 (25 September 2026). It has the shape of that paper's Appendix B,
"The NetKernel Realisation", and is offered beside it, not against the paper: the paper's
constructs, realised in `ikigai-core`, with the places where ikigai's mathematics is genuinely
an extension and the deviations that were deliberate, each argued.

**Status:** written 2026-09-25 against ikigai-core **0.1.72** (`2504ad5`, published). Outline:
ledger [#522](http://localhost:1060/l/default/item/522). The explainer in a different register
is [#523](http://localhost:1060/l/default/item/523); the book review is
[#521](http://localhost:1060/l/default/item/521).
**Companions in this repository:** `docs/design/resolution-scope.md` (the chain),
`docs/design/sub-request-authority.md` (attenuation, and why delegation is not built),
`docs/design/cache-ejection.md` (what the cache key does not identify),
`docs/design/spaces-as-named-graphs.md` (where corridor identity goes next).
**The gate:** `crates/ikigai-core/tests/formalism_pins.rs`. Every pin in this document is
resolved against the source tree on every `cargo test`. A renamed or deleted test turns this
document red. A document with no pins cannot pass.

---

## How to read this document

A paper has no gate; a design document rots for the same reason. The rule that makes this
document different is that **every definition, theorem and claimed deviation names the test
that holds its precondition**, in a line the test suite can find, and a claim the code cannot
yet vouch for is marked so. The three forms, shown here inside a fence the gate does not read:

```text
pin: `module::tests::test_name` (crates/ikigai-core/src/module.rs)
doctest: `Type::item`
UNPINNED — what a test would assert, in one sentence
```

- `pin:` names a `#[test]` function (a unit test by its module path, an integration test by its
  file), and the file it lives in. The gate fails if the function is missing or is not a test,
  or if the file does not contain it.
- `doctest:` names a public item whose `///` block carries a fenced example, which `cargo test`
  compiles and runs. The only comment the compiler reads.
- `UNPINNED` is **a finding, not a formatting choice**: the sentence after the dash says what a
  test would assert. The register in §9 collects every one of them; that list is the set of
  things this crate cannot yet vouch for.

Several pins under one claim are several lines. In a table, the pin column holds them separated
by `;`. A pin is a name the test can find, never prose.

**Numbering.** The paper's definitions, propositions, theorems and sections are cited as v5.18
numbers them (Def. 10, Prop. 4, Thm. 4(a), §9.4, B.2). Claims made here are numbered
R⟨section⟩.⟨n⟩, R for realisation. **Every number cited carries its date and instrument**; they
are collected in §10.

**Vocabulary.** The paper writes *corridor* for the formal object and records that NetKernel
calls it a *space*. ikigai's trait is `Space`. Below, *corridor* is the paper's object and
`Space` the Rust trait; the *chain* is the paper's Γ, the *root* is the space a `Kernel` is
built over, and the *static tree* is that space's composition of `Mount`, `Fallback`, `Rewrite`
and `Alias` combinators. The paper's Σ* is written I.

**Conditional where the code is conditional.** Two of the theorems below have a precondition
the kernel does not yet discharge for every endpoint (R4.4, R2.2's structural half). They are
stated with the precondition explicit and the precondition pinned to the test that supplies it
by hand. That is the whole value of the document: it says exactly where ikigai is weaker than
its own story.

---

## 1. The construct map

Read with the paper's Appendix A (glossary) and Appendix B open. *Deviation* marks a place
where the realisation differs from the definition on purpose; the argument is in §1.1–§1.5.
*Absent* marks a construct the paper has and ikigai does not, with the ledger item that
tracks it.

| Paper construct | Where | ikigai-core | Status | pin |
|---|---|---|---|---|
| Identifier, I = Σ* | §1.2 | `Iri` — a validated **absolute IRI**. The identifier space is the RFC 3987 subset of Σ*, not all of it; the empty string ε is not an identifier. | deviation, §1.5 | pin: `iri::tests::accepts_absolute_iris` (crates/ikigai-core/src/iri.rs); pin: `iri::tests::rejects_relative_or_malformed` (crates/ikigai-core/src/iri.rs) |
| Door (φ, F, Ω) | Def. 1 | a `(Grammar, Endpoint)` binding in an `EndpointSpace`: φ is `Grammar::match_iri`, which also returns the capture (the paper's "division into captured parts"); F is the set it accepts; Ω is `Endpoint::invoke`. Shipped grammars: `Exact` (a singleton family) and `UriTemplate` (RFC 6570 level 1, non-empty captures, adjacent variables refused at construction — so the capture function is a function of the identifier, as Thm. 4(a)(ii) and B.2 require). Regularity is a property of the shipped grammars, not of the trait: any decidable `Grammar` may be bound. | realised | pin: `grammar::tests::exact_matches_only_itself` (crates/ikigai-core/src/grammar.rs); pin: `grammar::tests::template_captures_trailing_var` (crates/ikigai-core/src/grammar.rs); pin: `grammar::tests::expand_is_inverse_of_match` (crates/ikigai-core/src/grammar.rs); pin: `grammar::tests::rejects_ambiguous_and_malformed` (crates/ikigai-core/src/grammar.rs) |
| Corridor, first-match; effective families (Def. 3, Prop. 1) | §2.2 | `EndpointSpace::resolve`: bindings in declaration order, the first grammar that matches wins. The partition of Prop. 1 is what `entries()` enumerates, in the same order. A shadowed binding is invisible to selection rather than misattributed. | realised | pin: `space::tests::endpoint_space_enumerates_its_bindings` (crates/ikigai-core/src/space.rs); pin: `resolution::grammar_bindings_flow_to_endpoint` (crates/ikigai-core/tests/resolution.rs); pin: `resolution::unresolved_target_misses` (crates/ikigai-core/tests/resolution.rs); pin: `select::tests::a_shadowed_probe_is_discarded_not_misattributed` (crates/ikigai-core/src/select.rs) |
| Context chain Γ, innermost first | §3 | Two things, and the difference is §1.1. **Per request:** `Scope` — ⟨injected corridors innermost first, root⟩, carried on the `Invocation` into every sub-request. **Static:** the root's tree, of which `Fallback` is the ordered first-hit member list. | realised, deviation §1.1 | pin: `scope::an_injected_corridor_shadows_a_root_door_for_the_request_and_for_its_sub_requests` (crates/ikigai-core/tests/scope.rs); pin: `resolution::fallback_tries_in_order` (crates/ikigai-core/tests/resolution.rs); pin: `space::tests::fallback_concatenates_enumerable_members_in_order` (crates/ikigai-core/src/space.rs) |
| Import (union) | Def. 4 | `Mount` — an import guarded by a prefix: the inner space's families, restricted to identifiers under the prefix, added to what resolution reaches at the mount's position. No rewrite; the inner patterns are already full identifiers. | realised | pin: `resolution::mount_gates_by_prefix` (crates/ikigai-core/tests/resolution.rs) |
| Mapper (preimage) | Def. 6 | `Rewrite` (a closure τ) and `Alias` (a table of exact and prefix rules, with counters). Both resolve τ(i) **once** in the enclosed space and **report** the rewrite on `Resolved::canonical`, which the kernel adopts before the cache key, the capability floor and the write-cut. **No outward fallback with τ(i)** — §1.2 and §5. | realised, deviation §1.2 | pin: `resolution::rewrite_remaps_target_before_resolution` (crates/ikigai-core/tests/resolution.rs); doctest: `Resolved::canonical`; pin: `alias::an_alias_can_never_launder_authority` (crates/ikigai-core/tests/alias.rs); pin: `alias::every_core_overlay_forwards_a_reported_canonical` (crates/ikigai-core/tests/alias.rs) |
| Transparent overlay | Def. 5 | the interception family: `Resolution::map_endpoint` decorates the endpoint of an inner resolution and forwards `entries()`, so the overlay's family is exactly the union of what it encloses — the paper's transparent family, by enumeration. Every governor in `ikigai-throttle` is this shape; core's own overlays forward a reported canonical through it. | realised | doctest: `Resolution::map_endpoint`; pin: `space::tests::a_governor_stacks_on_an_already_erased_space` (crates/ikigai-core/src/space.rs) |
| Opaque overlay | Def. 5 | `MountedRemote` in `ikigai-cli`: declares the remote's published patterns as its own family and hides the rest. Outside this crate. | realised elsewhere | UNPINNED — lives in ikigai-cli (`MountedRemote`), which a test in this crate cannot reach |
| Limiter (difference) | Def. 7 | **Absent.** `Resolution` is `Hit \| Miss`; a miss always falls through, so no space can carve a family out of a chain. Subtraction exists only by construction site (a smaller root per process). Ledger [#511](http://localhost:1060/l/default/item/511). | absent | UNPINNED — a test would bind `Fallback([Limit("urn:personal:"), S])` with S binding `urn:personal:x`, and assert `Unresolved` for it and its absence from `urn:kernel:actions` |
| Trapdoor (severs the chain) | Def. 10 | `Confine` (endpoint-side) and `Invocation::confine`: the inner endpoint's sub-requests resolve in ⟨host corridors…, S⟩ with no root; a root-bound name is `Unresolved`, never `Denied`, and the root endpoint is never entered. Two deviations from Def. 10, argued in §1.3. | realised, deviation §1.3 | doctest: `Confine`; pin: `scope::a_confined_sub_request_for_a_root_bound_iri_is_unresolved_and_the_root_endpoint_is_never_entered` (crates/ikigai-core/tests/scope.rs); pin: `scope::an_endpoint_can_only_narrow_its_chain` (crates/ikigai-core/tests/scope.rs); pin: `scope::fan_out_from_a_confined_endpoint_stays_confined` (crates/ikigai-core/tests/scope.rs); pin: `scope::confine_describes_and_names_as_its_inner_endpoint` (crates/ikigai-core/tests/scope.rs) |
| Gatekeeper | §9.5 | Not a construct. The kernel's **capability floor** is a gatekeeper on every door at once: the scopes a description `requires` for the verb are checked before dispatch and before the cache, and a refusal is recorded on the trace before it is returned. Who is asking is established at the transport (per-identity grants, `ikigai-cli`), outside this crate. This is the paper's *interception*: a decision, not structure (§9.6). | realised as the floor | pin: `kernel::tests::declared_requires_is_kernel_enforced_before_dispatch` (crates/ikigai-core/src/kernel.rs); pin: `kernel::tests::a_denied_bound_endpoint_reports_the_refusal` (crates/ikigai-core/src/kernel.rs) |
| Projection across a boundary (restricted union) | §9.6 | Two halves. The **authority** half is in core: a carried capability is clamped to the ceiling the channel authenticated (`Capability::clamp`). The **surface** half — only declared doors are visible to the far side — is the served kernel's choice of root per process (`ikigai-embedded`), outside this crate. | realised, half here | pin: `capability::tests::clamp_bounds_a_carried_capability_to_the_ceiling` (crates/ikigai-core/src/capability.rs); UNPINNED — the served surface is chosen in ikigai-embedded, which a test in this crate cannot reach |
| Value corridors | §3 | **Not modelled.** Values travel on the `Request` as `ArgRef`s (inline bytes, a content address, or a by-reference IRI) and are part of the request's identity; they are never doors and can shadow nothing. A by-reference argument naming something outside the chain is `Unresolved` inside it — the trapdoor working. §1.4. | deviation §1.4 | pin: `request::tests::distinct_inputs_have_distinct_identity` (crates/ikigai-core/src/request.rs); UNPINNED — a test would confine an endpoint, hand it `ArgRef::Reference` to a root-bound IRI, and assert `Unresolved` on dereference |
| Sticky header (who is asking) | §3 | the **capability**: carried into every sub-request, and only ever narrowed (§2). There is no principal on the request; attribution is a tracer concern (`ikigai-log`'s `Principal`). | realised as the capability | pin: `kernel::tests::each_event_records_the_capability_it_ran_under` (crates/ikigai-core/src/kernel.rs) |
| Continuation vs sub-request | §3 | A mapper's continuation is a nested `resolve` call inside one synchronous resolution, never a new request; every request an endpoint issues is a sub-request through `Invocation`. The two cannot be confused because only one of them exists as a request. | realised | pin: `with_bindings::a_sub_request_through_the_reborrow_reaches_the_kernel` (crates/ikigai-core/tests/with_bindings.rs) |
| Path cache γ_path | §5.1 | **Absent, measured.** `resolve` runs on every request, before the representation-cache lookup, so that the declared-capability floor fences cached answers too. A cached read is ~0.6 µs end to end of which resolution is 7–330 ns (§10, [#510](http://localhost:1060/l/default/item/510)); a path cache would need a binding-change thread as its validity predicate, which does not exist either. | absent | pin: `kernel::tests::declared_requires_is_kernel_enforced_before_dispatch` (crates/ikigai-core/src/kernel.rs); UNPINNED — the measurement is a scratch bench, not a test (§10) |
| Representation cache γ_rep, keyed by identifier and context | §5.1 | `CacheKey { request: RequestId, capability: u64, scope: u64 }` — the content-addressed request (verb, canonical target, arguments including `as=`), the authority fingerprint, the chain fingerprint. §3 and §7. | realised, extended | pin: `kernel::tests::cache_is_keyed_by_capability` (crates/ikigai-core/src/kernel.rs); pin: `scope::the_empty_scope_is_the_status_quo` (crates/ikigai-core/tests/scope.rs); pin: `request::tests::identity_is_deterministic` (crates/ikigai-core/src/request.rs) |
| Validity predicate ν | §5.1 | `Expiry { Always, At(t), Never }` on the representation, plus golden-thread edges pinned to generations, plus the cut sequence that closes the lost-cut race. §4. | realised, extended | pin: `cache::tests::a_cut_after_the_snapshot_declines_the_store` (crates/ikigai-core/src/cache.rs); pin: `cache::tests::a_deadline_is_honoured_and_a_clockless_kernel_assumes_the_worst` (crates/ikigai-core/src/cache.rs) |
| Dependency graph δ(r), golden thread | §5.3, B.6 | `Representation::depends_on(thread)` declares; sub-request results are inherited; a `Sink`/`Delete` cuts the thread named after its canonical target; an external watcher cuts the same thread. §4. | realised, conditional R4.4 | pin: `kernel::tests::a_thread_propagates_up_through_composition` (crates/ikigai-core/src/kernel.rs); pin: `kernel::tests::a_sink_invalidates_the_cached_source_of_its_target` (crates/ikigai-core/src/kernel.rs) |
| Clocks may be read; results are then not cacheable | Hyp. H, §5.1 | `Expiry::Always` is the default (an endpoint opts in to caching); a deadline is `At`, judged against the kernel's injected `Clock`; a clockless kernel declines to cache a deadline at all. | realised | pin: `kernel::tests::a_clockless_kernel_declines_to_cache_a_deadline` (crates/ikigai-core/src/kernel.rs); pin: `kernel::tests::the_shipped_fixed_clock_drives_a_deadline_and_does_not_move` (crates/ikigai-core/src/kernel.rs); pin: `verb::tests::cacheability_matches_idempotency` (crates/ikigai-core/src/verb.rs) |
| Metadata verb, self-description (§8.2) | §3, §8.2 | `Verb::Meta` routed to a `MetaRenderer`; `urn:kernel:catalog` lists every binding; every kernel operation describes itself. **The arrangement is not a resource**: `Fallback`, `Mount`, `Rewrite` and the chain are invisible in any representation ([#515](http://localhost:1060/l/default/item/515)). | half realised | pin: `kernel::tests::meta_is_routed_through_the_renderer` (crates/ikigai-core/src/kernel.rs); pin: `kernel::tests::catalog_enumerates_every_bound_endpoint_through_the_renderer` (crates/ikigai-core/src/kernel.rs); pin: `kernel::tests::every_kernel_operation_describes_itself` (crates/ikigai-core/src/kernel.rs); UNPINNED — a test would source `urn:kernel:topology` and find the tree as a graph with ordered members |
| Verbs folded into identifiers | §3 | **Deviation, deliberate:** `Verb` is first-class and part of the request identity. §6. | deviation §6 | pin: `request::tests::distinct_inputs_have_distinct_identity` (crates/ikigai-core/src/request.rs) |
| `new` verb | §3 | Absent: `Sink` to an unbound-but-resolvable name makes it reifiable. `exists` is `Verb::Exists`, answered by the endpoint (the paper's decidable approximation of reifiability). | deviation §6 | pin: `verb::tests::cacheability_matches_idempotency` (crates/ikigai-core/src/verb.rs) |
| Hop bound (Cor. 3), nesting budget (B.7) | §10 | Neither. The hop counter is not needed (§5: resolution terminates by construction); the nesting budget is needed and absent ([#513](http://localhost:1060/l/default/item/513)). | absent | UNPINNED — a test would bind an endpoint that sources its own IRI and assert a typed refusal at the configured depth, not a stack overflow |
| Hypothesis H | §3 | Holds **structurally** for wasm modules (the shim's only import is `issue`, in `ikigai-module`); for in-process Rust endpoints it is **review**, exactly as the paper says of NetKernel. | as the paper says | UNPINNED — the wasm half lives in ikigai-module; the in-process half is not a property a test can observe (`std::fs` is one line away) |

### 1.1 One corridor with internal structure: the static tree is not a chain

The paper's chain has levels; an endpoint runs *in the scope at the level where it was found*,
inner levels dropped, and its sub-requests resolve from there (§3, B.2). ikigai has that
structure only in the per-request `Scope`: the chain is exactly ⟨injected corridors…, root⟩, and
the **root is one corridor**, however it is composed. `Mount`, `Fallback`, `Rewrite` and `Alias`
are combinators over one `resolve` function, not levels of the chain. Consequences:

- An endpoint found anywhere in the tree runs in the request's whole chain. Its sub-requests
  resolve from the **top** of the chain — the innermost injected corridor, then the root from
  its top — never from the mount that bound it. The paper's per-level shadowing inside the
  arrangement does not exist; the chain's shadowing (§7) does.
- The paper's fallback edge — out of an owned corridor back into the corridor hosting the
  construct — does not exist, because there is no owned corridor to fall out of: a `Mount`
  or `Rewrite` that misses returns `Miss` to its parent combinator, which continues with the
  **original** request. §1.2 and §5 rest on this.
- For Thm. 4(b) the pushdown stack has depth at most |injected| + 1, and the static tree
  contributes no pushes. Gatekeeper completeness over the static tree is therefore a
  question about the tree's *structure*, which today is Rust values a host built and not a
  resource — the gap [#515](http://localhost:1060/l/default/item/515) names.

Pinned by the sub-request half of the shadowing test and by the reborrow test:

    pin: `scope::an_injected_corridor_shadows_a_root_door_for_the_request_and_for_its_sub_requests` (crates/ikigai-core/tests/scope.rs)
    pin: `with_bindings::a_sub_request_through_the_reborrow_reaches_the_kernel` (crates/ikigai-core/tests/with_bindings.rs)

### 1.2 The mapper does not fall back with τ(i)

Def. 6: if τ(i) is not resolved in the enclosed corridor, resolution continues outward *with
τ(i)*, from the mapper's own corridor, so the mapper can admit it again. ikigai's `Rewrite`
resolves τ(i) once in its inner space; a miss is a `Miss`, and the enclosing `Fallback`
continues with **i**. At the kernel, a rewrite that lands on nothing is `Unresolved` naming the
canonical target, recorded as a rewrite on the trace and counted per rule at
`urn:kernel:aliases`. Once mapped, the rewritten identifier is the identifier from then on
(cache key, floor, cut — R3.2), which is the half of Def. 6 ikigai keeps.

    pin: `alias::a_miss_after_a_rewrite_is_reported_as_a_rewrite` (crates/ikigai-core/tests/alias.rs)
    pin: `alias::the_two_names_key_the_same_cache_entry` (crates/ikigai-core/tests/alias.rs)

Why: the fallback edge is what makes one mapper Turing-complete (Prop. 5 and its corollary).
Without it, resolution over a static tree terminates by construction (R5.1), and the price is
the pattern the paper's §12.7 builds — a mapper that expects the *outer* corridor to serve some
of its rewritten names. ikigai says: put the door in the enclosed space, or do not rewrite.

### 1.3 The trapdoor is bound at one door, and keeps the host's corridors ahead of S

Def. 10 constructs Γ′ = ⟨S, V(Γ)⟩: the enclosed corridor innermost, the value corridors after it,
nothing else. `Confine` constructs ⟨host-injected corridors…, S⟩: the corridors the host injected
for the request stay **ahead**, S goes where the root was, and the root is cut off. Two
deviations, one argument each:

- **S is outermost, not innermost.** A confining endpoint is an endpoint; the only chain
  operation it may perform is one that cannot shadow a corridor the host put there. Placing S
  behind the host's corridors makes `confine` strictly narrowing — relative to the chain it
  started in, nothing resolves differently except what the root would have answered
  (`docs/design/resolution-scope.md`, decision 3). The paper puts injection and severing in
  different hands too (the arrangement injects, the trapdoor severs); ikigai makes the
  ordering enforce it. Recorded as open, not decided: whether `confine` should be allowed to
  drop the host's corridors as well.
- **Bound at one door, not transparent.** The paper's trapdoor is a transparent overlay: from
  outside it admits everything S serves, which is why §12.3 finds the model client reachable
  from the application corridor and prescribes wrapping the trapdoor in a mapper. `Confine`
  is an endpoint decorator bound at one door with the inner endpoint's own description and
  name; S is never exposed outward. It is the paper's trapdoor already wrapped in the mapper
  that exposes one identifier. Metadata passes through, as Prop. 4 allows: `Meta` never
  invokes, and `Confine::describe` is the inner endpoint's description.

Pinned by the chain the decorator reports and by nested confinement:

    pin: `scope::confine_describes_and_names_as_its_inner_endpoint` (crates/ikigai-core/tests/scope.rs)
    pin: `scope::an_endpoint_can_only_narrow_its_chain` (crates/ikigai-core/tests/scope.rs)

The paper's depth-preserving empty corridors (§9.4) are unnecessary: the cache keys on a
fingerprint of the chain, not on a depth (§7).

### 1.4 Values are not corridors

The paper carries a request's values as transient corridors so that an endpoint can obtain a
value by resolving its identifier, notes that they accumulate down the chain, that a trapdoor
exposes all of them unless filtered, and that their identifiers must be unique across the chain.
In ikigai a value is an argument: `ArgRef::Inline` bytes, `ArgRef::Content` (a content address),
or `ArgRef::Reference` (an IRI, resolved in the chain like any other). Arguments are part of the
request's identity (R3.1) and travel with the request, not the chain; a sub-request carries its
own arguments and nothing of its parent's. So: no accumulation, nothing for a trapdoor to
filter, no uniqueness hypothesis, and a value can shadow nothing because it is not a door. What
is lost is the paper's uniform "everything is obtained by resolving an identifier"; what is
gained is that the trapdoor's boundary is exactly S plus the host's corridors, with no
value-corridor tail. The behavioural half — a by-reference argument outside the chain is
`Unresolved` inside a confinement — follows from R7.1 but has no test of its own.

    pin: `request::tests::distinct_inputs_have_distinct_identity` (crates/ikigai-core/src/request.rs)
    UNPINNED — a test would confine an endpoint, pass it `ArgRef::Reference` to a root-bound IRI, and assert `Unresolved` on `inv.source` of it

### 1.5 What is absent, and the empty identifier

- **Limiter** ([#511](http://localhost:1060/l/default/item/511)), **path cache**
  ([#510](http://localhost:1060/l/default/item/510)), **nesting budget**
  ([#513](http://localhost:1060/l/default/item/513)), **topology as a resource**
  ([#515](http://localhost:1060/l/default/item/515)), **lossless flag**
  ([#514](http://localhost:1060/l/default/item/514)): each is a row above, each UNPINNED.
  None is a decision against the paper; core grows capabilities incrementally, and a gap
  is not a design until it is written down as one.
- **ε.** The paper's null identifier resolves entirely from context (§2.3, Cor. 1). An `Iri`
  is absolute, so ε is not an identifier here. The nearest thing is a short identifier bound
  by an injected corridor — `urn:time:now` under a temporal corridor — which is Cor. 2 (richer
  context, shorter identifier) rather than Cor. 1.

---

## 2. Authority is a lattice inside the algebra

The paper keeps refusal outside the algebra as a *decision* (§9.6: interception "is the one
point in the algebra where a decision, rather than structure, bounds what can be reached"). In
ikigai authority is a value that travels with the request and composes by a meet, so the
question "what can this request reach, and under what authority" has one answer computed by
one predicate. That is an extension; the constructs it rests on are the paper's.

**Definition R2.1 (the capability lattice).** Let S be the set of scope strings. A capability
is `Root` or `Scoped(t)` for t ⊆ S. Order: `Scoped(t) ≤ Scoped(u)` iff t ⊆ u, and
`Scoped(t) ≤ Root` for every t. Then `attenuate(c, s)` is the meet c ⊓ `Scoped(s)`:
`Root ⊓ s = Scoped(s)` and `Scoped(t) ⊓ s = Scoped(t ∩ s)`. `clamp(ceiling, carried)` is
`ceiling ⊓ carried` with `Root` as top: `ceiling.clamp(Root) = ceiling`, `Root.clamp(c) = c`,
otherwise the intersection. There is no join, by construction: no operation widens.

    pin: `capability::tests::attenuation_only_narrows_never_widens` (crates/ikigai-core/src/capability.rs)
    pin: `capability::tests::attenuating_root_yields_exactly_the_requested_scopes` (crates/ikigai-core/src/capability.rs)
    pin: `capability::tests::clamp_bounds_a_carried_capability_to_the_ceiling` (crates/ikigai-core/src/capability.rs)

**Theorem R2.2 (authority is a descending chain).** Let a host issue a request under c₀. Every
invocation in the tree of sub-requests it gives rise to runs under some cₖ with cₖ ≤ cₖ₋₁, and
cₖ ∈ {cₖ₋₁, cₖ₋₁ ⊓ s} for a set s the issuing endpoint named.

*Proof shape.* The ways an endpoint can issue a sub-request are `Invocation::issue` and
`source` (the invocation's capability, verbatim), `issue_attenuated` and `source_attenuated`
(the meet), `fan_out` (a clone of the invocation's capability per spawned request), and a
`SyncIssuer` bridge (served by the invocation that minted it). All route through the private
`issue_under(request, capability)`, and **no public path takes a capability**: `Capability::root`
and `Capability::scoped` are public, so any code can *construct* a strong capability, but an
endpoint has no way to hand one to an issuer. Across a wire the receiver clamps a carried
capability to the session's ceiling (R2.1), so the chain descends across peers too.

    pin: `kernel::tests::an_attenuated_sub_request_drops_authority_the_caller_still_holds` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::attenuating_a_sub_request_cannot_widen_past_the_caller` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::a_sync_scope_cannot_widen_authority` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::an_attenuated_sub_request_is_still_recorded_as_a_dependency` (crates/ikigai-core/src/kernel.rs)
    doctest: `Invocation::issue_attenuated`
    UNPINNED — the structural half ("no public path takes a capability") is a visibility fact; a `compile_fail` doctest on `Invocation` calling `issue_under` would hold it, and none exists

The wire half (a QUIC server resolving under `session.clamp(&carried)`) is in `ikigai-cli`:
the clamp is pinned above, the site that applies it is outside this crate. IPC deliberately
does not clamp (the peer is the peercred-verified owner), which holds while IPC has no
non-root principal (`docs/design/sub-request-authority.md`).

**Theorem R2.3 (the manifold is Reach(Γ) ∩ Offered(c), and Offered = Admitted).** Let
`requires(d, v)` be the scopes door d declares for verb v. `Offered(c)` is the set of (d, v)
with every declared scope satisfied by c under `cap_satisfies` (exact membership, or the
trailing-`*` family form: `urn:cap:net:*` is satisfied by any held grant under the prefix).
`urn:kernel:actions` under c lists Reach(Γ) ∩ Offered(c), where Reach is computed by
**enumeration** of `entries()`. The kernel admits (d, v) for c iff the same predicate holds,
evaluated after resolution and before the cache lookup. Hence what the manifold offers is
exactly what the kernel admits, relative to the declarations — in both directions for the
kernel's own operations, which the test sweeps verb by verb, withholding each declared scope
in turn.

    pin: `kernel::tests::the_action_manifold_is_capability_scoped` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::declared_requires_is_kernel_enforced_before_dispatch` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::kernel_ops_declare_exactly_what_they_enforce` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::the_manifold_offers_exactly_the_kernel_ops_a_capability_admits` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::wildcard_requires_passes_any_grant_under_the_family` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::per_verb_action_specs_enforce_independently` (crates/ikigai-core/src/kernel.rs)

Three qualifications, each the difference between the theorem and the code:

1. **Reach by enumeration under-approximates.** A space that cannot enumerate (`Rewrite`, a
   remote) contributes nothing; `Alias` lists the backing names once, not the logical
   preimage. The manifold may therefore omit a reachable door; it never offers an unreachable
   one on this account. Safe direction.
2. **Selection is over the root, not the chain** ([#516](http://localhost:1060/l/default/item/516)).
   Inside a confinement the manifold can offer an action the chain cannot resolve — an
   over-offer, the unsafe direction, one layer up from the rule that prevents it at the door.
3. **Declared ⊆ enforced holds by construction; the converse is discipline.** An endpoint's own
   finer runtime gate (a path ACL) is its ceiling on top of the floor. An endpoint that gates
   at runtime on a scope it never declared makes the manifold lie; the conformance suite, not
   the kernel, is where that is checked.

    pin: `space::tests::rewrite_is_not_enumerable` (crates/ikigai-core/src/space.rs)
    pin: `kernel::tests::a_non_enumerable_root_still_reports_that_it_cannot_say` (crates/ikigai-core/src/kernel.rs)
    pin: `alias::the_catalog_lists_the_backing_name_once` (crates/ikigai-core/tests/alias.rs)
    UNPINNED — a test would confine an endpoint and assert `inv.select_action` omits root-bound actions; today it offers them

**The confused deputy, located.** In the paper's terms the deputy is confused where *effect*
authority departs from *caller* authority. By R2.2 they never depart: at every hop exactly one
authority is in effect and it is the caller's, or a meet of it. The cost is real and paid
elsewhere — a module built on a gated module makes its callers hold the underlying grant
(`ikigai-ledger` declares the store's scopes beside its own). Delegation — a module acting
under authority *it* holds, bound by the host — is designed and deliberately not built
(`docs/design/sub-request-authority.md`, Part B): the grant would have to enter the cache key,
the trace would record two authorities per hop, and the motivating case needs a
request-derived grant nobody has decided how to express safely.

    UNPINNED — by design: a test would hand a granted endpoint a hostile argument and assert the far-side door refuses it; there is no granted endpoint to test

**What the boundary reveals.** The paper's limiter leaves a requester unable to distinguish
"limited" from "matched by no door anywhere" (§9.6, remark). ikigai's capability floor is an
interception and reveals what a limiter would not: a bound-but-refused name is `Denied`, an
unbound one `Unresolved`, so any caller can learn whether a name is bound. Inside a
confinement the distinction collapses the way the paper wants — a root-bound name is
`Unresolved`, structurally — which is the reason `Confine` exists.

    pin: `kernel::tests::unresolved_target_errors` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::declared_requires_is_kernel_enforced_before_dispatch` (crates/ikigai-core/src/kernel.rs)
    pin: `scope::a_confined_sub_request_for_a_root_bound_iri_is_unresolved_and_the_root_endpoint_is_never_entered` (crates/ikigai-core/tests/scope.rs)

---

## 3. Content addressing makes the soundness condition checkable

§5.1's soundness condition: a cached representation may be served only if a fresh reification
would yield the same one, which holds when the representation is determined by the identifier,
the context, and the state of what it depended on. ikigai turns "determined by" into a **key
completeness** claim about a hash, which is what makes it checkable: list what is in the key,
list what is not, and pin each.

**Definition R3.1 (request identity).** `RequestId` = BLAKE3 over a version tag
(`ikigai.request.v0`), the verb's code, the target IRI, the argument count, and the arguments
in sorted name order, each value tagged by kind (reference IRI / inline bytes / content
address). Identity is deterministic, order-independent over arguments, equal for equal values,
and distinct for distinct verb, target, value or kind.

    pin: `request::tests::identity_is_deterministic` (crates/ikigai-core/src/request.rs)
    pin: `request::tests::argument_order_does_not_matter` (crates/ikigai-core/src/request.rs)
    pin: `request::tests::equal_values_share_identity` (crates/ikigai-core/src/request.rs)
    pin: `request::tests::distinct_inputs_have_distinct_identity` (crates/ikigai-core/src/request.rs)

**Theorem R3.2 (key completeness — conditional).** Let q be a request, c a capability, Γ a
chain, and q* the request with its target replaced by the canonical name every rewrite along
the resolution reported. The cache key is K = (id(q*), fp(c), fp(Γ)). Two issues with equal K
are served one representation. This is **sound** — a fresh reification would agree — provided
the representation is a function of exactly: (verb, canonical target, arguments including
`as=`) [in id(q*)], the authority [fp(c)], the corridors of the chain [fp(Γ)], and the state of
the resources it depended on [ν, §4]. Facts the key carries:

| Fact | Where in K | pin |
|---|---|---|
| verb, target, arguments, `as=` | `RequestId` | pin: `request::tests::distinct_inputs_have_distinct_identity` (crates/ikigai-core/src/request.rs) |
| the target **after** every rewrite — one entry for a logical name and its backing name, through a nested overlay too | `RequestId` of q* | pin: `alias::the_two_names_key_the_same_cache_entry` (crates/ikigai-core/tests/alias.rs); pin: `alias::a_nested_alias_gives_the_two_names_one_cache_entry` (crates/ikigai-core/tests/alias.rs) |
| the authority (BLAKE3 over the sorted scope set; root in its own namespace) | `capability` | pin: `kernel::tests::cache_is_keyed_by_capability` (crates/ikigai-core/src/kernel.rs) |
| the chain: corridor **names**, their order, and whether the root is present | `scope` | pin: `scope::a_result_computed_outside_confinement_is_not_served_inside_it` (crates/ikigai-core/tests/scope.rs); pin: `scope::a_result_computed_inside_confinement_is_not_served_outside_it` (crates/ikigai-core/tests/scope.rs); pin: `scope::two_requests_with_the_same_named_scope_share_one_cache_entry_and_different_names_do_not` (crates/ikigai-core/tests/scope.rs) |

Facts the key does **not** carry, and what covers each:

| Fact | Covered by | Status |
|---|---|---|
| the state of dependencies (files, store graphs, other resources) | ν: golden-thread edges, §4 | realised, conditional R4.4 |
| time | ν: `Expiry::At` against the kernel clock | realised |
| **the binding behind the canonical name — which endpoint, which code version** | nothing. Within one process the binding is assumed not to move; a root is immutable at construction, so today this bites only a space with interior mutability (dynamic mounts, module reload) — and there a catalog or Meta cached `Never` is stale forever after a rebind. Across processes it is the normal case (`docs/design/cache-ejection.md` §3). The fix is a thread the kernel cuts on binding change ([#510](http://localhost:1060/l/default/item/510)). | UNPINNED — a test would build a `Space` with a swappable binding, cache a `Never` read, swap, and assert the entry is not served; today it is |
| a grant under delegation | not applicable: delegation is not built (§2) | n/a |

Two of the facts the key *does* carry are **claims**, not observations, and the theorem is
conditional on them in the same way it is conditional on ν:

- **A corridor's name is a claim**: same name ⇒ same doors. Name two different corridors alike
  and one request is served the other's answer. This is exactly the contract a reported
  canonical makes, stated on the type with an example that produces one entry for two freshly
  built corridors under one name and a second entry under another.
- **A reported canonical is a name in this kernel's namespace.** A rewrite that crosses out of
  the kernel (a mount stripping its prefix into a wire address) must report nothing, or two
  unrelated resources fuse into one entry and one thread. Core's own overlays forward a
  reported canonical; the cross-kernel rule is stated on the field and honoured in
  `ikigai-cli` by discipline.

    doctest: `Scope`
    doctest: `Resolved::canonical`
    pin: `alias::every_core_overlay_forwards_a_reported_canonical` (crates/ikigai-core/tests/alias.rs)

**Remark (why a hash and not a tuple).** The key holds a `u64` per fingerprint, never the
capability or the chain; the fingerprints are BLAKE3 prefixes with length-prefixed fields, so
scope boundaries cannot be forged by concatenation. Equal fingerprints do not mean equal
authority across hosts (scope strings are host-relative) — a bundle of another host's entries
must re-derive the capability half locally, which is why no import exists
(`docs/design/cache-ejection.md` §2).

---

## 4. Golden threads give ν a shape

The paper's ν is a predicate on (key, time) with the soundness condition as its specification.
ikigai gives it a concrete form with three parts, and the third — the cut sequence — is a small
linearizability result the paper does not need to state because it does not model concurrent
invalidation.

**Definition R4.1 (validity).** An entry e stores its representation, its expiry x, and edges
{(T, g_T)}: for each golden thread T the representation depends on, the generation T held when e
was stored. ν(e, t) holds iff every edge is current — gen(T) = g_T — and, if x = `At(d)`, the
kernel's clock reads t < d. A kernel without a clock treats a deadline as passed and declines to
store one. Threads are the representation's declared threads plus those inherited from every
sub-request it resolved and from a piped input's provenance.

    pin: `cache::tests::a_cut_invalidates_and_the_lookup_evicts` (crates/ikigai-core/src/cache.rs)
    pin: `kernel::tests::cutting_a_thread_invalidates_the_entry_that_declared_it` (crates/ikigai-core/src/kernel.rs)
    pin: `cache::tests::a_deadline_is_honoured_and_a_clockless_kernel_assumes_the_worst` (crates/ikigai-core/src/cache.rs)
    pin: `kernel::tests::time_based_entry_serves_until_its_deadline_then_recomputes` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::incoming_threads_are_inherited_so_cutting_the_source_invalidates` (crates/ikigai-core/src/kernel.rs)

**Theorem R4.2 (cuts are a total order, and no cut is lost).** Every cut takes the next number
from one monotonic counter and is appended to a bounded log. A request takes a snapshot s of the
counter **before it resolves anything**; the store of its result declines if any thread the
result depends on was cut at a number n > s, or if s predates the oldest retained cut (the log
can no longer answer, so it answers "yes"). Consequently, if e is stored, then for every T in
its edges no cut of T occurred between the snapshot and the store, and every observation the
invocation made of T-dependent state was made after s; so the pinned g_T is the generation
current for the whole of that window, and any later cut of T bumps gen(T) past g_T and ν fails
at the next lookup. The check is per thread — a cut to an unrelated thread does not decline —
and a thread-free entry cannot be raced at all.

    pin: `cache::tests::a_cut_after_the_snapshot_declines_the_store` (crates/ikigai-core/src/cache.rs)
    pin: `cache::tests::a_cut_to_an_unrelated_thread_still_stores` (crates/ikigai-core/src/cache.rs)
    pin: `cache::tests::a_snapshot_older_than_the_cut_log_declines` (crates/ikigai-core/src/cache.rs)
    pin: `cache::tests::a_thread_free_entry_is_never_raced` (crates/ikigai-core/src/cache.rs)
    pin: `kernel::tests::a_cut_during_an_in_flight_invocation_is_not_consumed_by_the_entry_it_invalidates` (crates/ikigai-core/src/kernel.rs)

The last pin is the race itself, end to end: an endpoint held open inside its invocation, the
thread its result depends on cut mid-flight, the pre-cut representation returned — and not
stored, and the next read recomputes. It is the test PR #108 (core 0.1.70) added when the
race was closed. The result is conservative in one direction only: a cut in (s, observation)
declines a result that may in fact be fresh. Not caching is never wrong.

**Proposition R4.3 (propagation is a meet; serving cost is the paper's).** A composite's
effective expiry is `most_restrictive` — the meet in the order Always < At(earlier) < At(later)
< Never — of its own expiry and every dependency's, and its thread set is the union. Hence a
composite is no fresher than its most volatile part, and cutting any member invalidates the
team but not its siblings. Readers never invalidate (a lookup bumps policy metadata only);
invalidation is lazy at the next lookup; an eviction policy bounds what is *kept* and can never
make a cut entry serve. Prop. 3 (N_reify ≤ min(N_chg, N_obs)) therefore applies verbatim.

    pin: `kernel::tests::most_restrictive_is_the_expiry_meet` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::a_thread_propagates_up_through_composition` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::a_deadline_propagates_through_composition` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::volatile_dependency_forbids_caching_the_composer` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::cutting_one_member_invalidates_the_team_but_not_its_siblings` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::cacheable_endpoint_runs_only_once` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::no_policy_can_make_a_cut_entry_serve` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::an_installed_cache_policy_bounds_what_the_kernel_keeps` (crates/ikigai-core/src/kernel.rs)

*Corollary (the least cacheable dependency).* Adding an `Always` source to a `Never` composite
makes the composite `Always` — a correctness no-op and a performance change with no type
signal. Measured 2026-08-13 in `ikigai-cms-web` (PR #71): joining an uncacheable overlay into a
cached books graph took a read from ~20 µs to ~1.0 s, every read, with 68 tests green (§10).

**Theorem R4.4 (the write-cut — CONDITIONAL).** After a successful `Sink` or `Delete` on
canonical target t, the kernel cuts thread t. Every entry with an edge on t is invalid
thereafter. **Precondition P:** a cacheable `Source` of t carries an edge on thread t — because
its endpoint declared `.depends_on(t)`, or inherited it. Under P, a read after a write through
the kernel is never stale, under either name of an aliased resource, and through a nested
alias.

    pin: `kernel::tests::a_sink_invalidates_the_cached_source_of_its_target` (crates/ikigai-core/src/kernel.rs)
    pin: `kernel::tests::cutting_a_thread_invalidates_the_entry_that_declared_it` (crates/ikigai-core/src/kernel.rs)
    pin: `alias::a_sink_through_one_name_invalidates_a_source_through_the_other` (crates/ikigai-core/tests/alias.rs)
    pin: `alias::a_sink_through_one_name_cuts_the_other_when_the_alias_is_nested` (crates/ikigai-core/tests/alias.rs)

**P is supplied by hand.** In every pinned test the endpoint declares the thread named after
its own target explicitly (`Cell` in the first, `.depends_on("urn:file:notes.txt")` in the
second); `ikigai-fs` does the same. The kernel stores only the threads an endpoint declared plus
those of its sub-requests; it does not pair a cacheable read with a thread named after its own
canonical target. So every other cacheable endpoint fronting mutable state is one forgotten
line from serving stale bytes after a write through the same name — hole A of
[#512](http://localhost:1060/l/default/item/512), the declared-versus-enforced shape again.
The unconditional theorem is what #512's fix A would discharge: the kernel adds the edge, and
endpoints declare nothing.

    UNPINNED — a test would bind a cacheable `Source` that does NOT call `depends_on`, `Sink` the same name, and assert the next `Source` recomputes; today it serves the stale entry

**Hole B, in the same place.** A failed sub-request records no dependency: an endpoint that
catches `Unresolved` and returns a cacheable fallback is cached `Never` with no thread on the
missing name, so a `Sink` that later creates it cuts nothing the entry depends on.

    UNPINNED — a test would cache a composite built on an `Unresolved` sub-request, `Sink` that name into existence, and assert the composite recomputes; today it does not

**Across processes.** Generations are per-process counters; another instance's 6 is not this
one's 6, so ν does not transfer and no cache import exists — the honest first tranche would
admit only thread-free `Never` entries (`docs/design/cache-ejection.md` §1). Not built.

---

## 5. Resolution terminates by construction; computation does not (yet)

The paper's §10: with a mapper's fallback edge, termination of resolution is undecidable in
general (Prop. 5), a single mapper is universal (corollary), and a hop bound restores both
termination and Thm. 4(a) (Cor. 3). Its Appendix B.7 records that NetKernel has no hop counter
and relies on a nesting budget instead, which makes Reach depend on nesting depth.

**Theorem R5.1 (resolution over a static tree terminates).** Let the root be a finite tree of
core's combinators. `resolve` is structural recursion over it: `EndpointSpace` loops its
bindings once; `Mount` tests the prefix and calls its inner space at most once; `Fallback`
calls each member at most once, in order; `Rewrite` calls its inner space exactly once, with
τ(i) or with i; `Alias` walks its table — refusing a cycle by a visited set and an over-long
chain by a hop budget (default 8) — then calls its inner space exactly once; a `map_endpoint`
overlay calls its inner space exactly once. No combinator calls an enclosing space, so a
resolution makes at most one call per node plus a bounded table walk, and Prop. 5's
construction — which needs the fallback edge out of an owned corridor — cannot be built.

*Hypotheses:* (i) every `Space` in the tree is one of core's combinators, or a foreign
implementation that calls only spaces it encloses (review); (ii) the tree is acyclic as a
graph of `Arc`s, which holds for a tree built by value and can be defeated only with interior
mutability (review). The paper's "dynamic chains need a separate argument" (Q3) is exactly
where these hypotheses stop.

    pin: `alias::tests::a_cycle_is_refused_not_truncated` (crates/ikigai-core/src/alias.rs)
    pin: `alias::tests::an_over_long_chain_is_refused` (crates/ikigai-core/src/alias.rs)
    pin: `alias::a_cycle_is_refused_rather_than_recursed_or_truncated` (crates/ikigai-core/tests/alias.rs)
    pin: `alias::a_cycle_under_a_nested_alias_is_refused_never_half_applied` (crates/ikigai-core/tests/alias.rs)
    pin: `alias::a_miss_after_a_rewrite_is_reported_as_a_rewrite` (crates/ikigai-core/tests/alias.rs)

A refusal (cycle, over-long chain, a substitution that is not an IRI) is not a miss and not a
truncation: a `Space` cannot return an error, so a hand-composed `Alias` resolves to an
endpoint that fails on invoke with the trail, and the kernel — which holds the table — refuses
before dispatch. Decision 5 of `alias.rs`: never a half-applied name.

**Proposition R5.2 (the regular fragment, by enumeration).** Thm. 4(a) constructs Reach(Γ)
from the arrangement when doors are regular, rewritings rational, and paths bounded. ikigai
computes Reach by enumeration: `entries()` lists every binding's pattern — a singleton for
`Exact`, a level-1 template for `UriTemplate`, both regular — and composes through `Mount` and
`Fallback` by concatenation. The one construct that cannot enumerate is the closure `Rewrite`:
an arbitrary `Fn(&Iri) -> Option<Iri>` is the paper's "arbitrary rewriting → Rice's theorem",
and its `entries()` is `None` for that reason; the kernel reports that it cannot say rather
than reporting an empty catalog. `Alias` is the paper's template mapper — exact and prefix
rules, whose preimages are regular — and *is* enumerable, listing the backing names once.

    pin: `space::tests::rewrite_is_not_enumerable` (crates/ikigai-core/src/space.rs)
    pin: `kernel::tests::a_non_enumerable_root_still_reports_that_it_cannot_say` (crates/ikigai-core/src/kernel.rs)
    pin: `alias::tests::the_overlay_is_transparent_to_enumeration` (crates/ikigai-core/src/alias.rs)
    pin: `grammar::tests::pattern_reflects_the_grammar` (crates/ikigai-core/src/grammar.rs)

**Computation is unbounded.** The paper separates the resolution hop counter (which ikigai
does not need, by R5.1) from the nesting budget on sub-requests (which ikigai lacks). A
`Request` has no depth, `issue_inner` no counter; an endpoint that sources its own IRI, or a
transclusion cycle, recurses until the stack dies. B.7's caching obligation — a refusal at the
bound must not be served to a shallower request — is met trivially when the budget lands,
because errors never reach the store. The depth will not cross the wire without a protocol
bump, so a cycle between two peers that mount each other stays unbounded even then
([#513](http://localhost:1060/l/default/item/513)).

    UNPINNED — a test would set a depth bound, bind an endpoint that sources itself, and assert a typed refusal at the bound rather than a stack overflow

---

## 6. Verbs are first-class, deliberately

The paper folds verbs into identifiers: a request with verb v for i is a request for v:i over an
extended alphabet, "every result about resolution holds with verbs folded into identifiers",
and state-changing requests enter the model only as the changes of §7. ikigai keeps `Verb`
first-class — `Source`, `Sink`, `Exists`, `Delete`, `Meta` — and this is a deviation with an
argument, not an oversight.

**R6.1 (the identity space is I × V).** The verb is hashed into `RequestId`, so `source:x` and
`sink:x` are distinct identities exactly as the paper's encoding would make them. Nothing about
resolution changes: a door's family may differ per verb (a per-verb `ActionSpec`, the paper's
"door that serves an identifier only for some verbs"), and each verb's declaration is enforced
independently.

    pin: `request::tests::distinct_inputs_have_distinct_identity` (crates/ikigai-core/src/request.rs)
    pin: `kernel::tests::per_verb_action_specs_enforce_independently` (crates/ikigai-core/src/kernel.rs)
    pin: `describe::tests::action_specs_normalize_both_authoring_forms` (crates/ikigai-core/src/describe.rs)

**R6.2 (why the kernel must see the verb: who cuts what).** §5.3 says a dependency on an
external condition is represented by a resource "invalidated explicitly when that condition
changes", and B.6 records that NetKernel's golden thread is cut by the endpoint that knows.
ikigai answers the question the paper leaves to the endpoint — *who cuts what* — in the kernel:
after a successful mutating verb the kernel cuts the thread named after the canonical target
(R4.4). Under the verb-as-prefix encoding `sink:x` and `source:x` are unrelated identifiers, and
the kernel could connect them only by a naming convention it would have to parse out of the
identifier — which is the verb, spelled differently. Keeping the verb visible is what lets the
kernel own the internal half of invalidation, so an endpoint never has to remember to cut its
own thread on a write. It is also what gives `Verb::is_cacheable` and `is_mutating` a single
definition the cache and the cut both read. The cost is that the paper's "verb-free model" is
recovered only up to R6.1's encoding; the theorems still apply, per verb.

    pin: `kernel::tests::a_sink_invalidates_the_cached_source_of_its_target` (crates/ikigai-core/src/kernel.rs)
    pin: `alias::a_sink_through_one_name_invalidates_a_source_through_the_other` (crates/ikigai-core/tests/alias.rs)
    pin: `verb::tests::cacheability_matches_idempotency` (crates/ikigai-core/src/verb.rs)

**Smaller differences.** There is no `new`; a `Sink` to a resolvable name makes it reifiable.
`Exists` is answered by the endpoint that resolution finds, as the paper says — resolvability
is necessary, the endpoint's own approximation decides. `Meta` never invokes the endpoint: the
kernel renders its description, so Prop. 4's "metadata requests are outside the proposition"
is how `Confine` behaves without a special case (§1.3).

---

## 7. Scope and §9.4

§9.4: a cached representation may be shared across a trapdoor boundary only if the corridors
actually used in resolving it are the same on both sides; an implementation must ensure the
scope recorded with an entry distinguishes a result computed in confinement from one computed
outside. `docs/design/resolution-scope.md` is the full account; this section states what is
pinned and where the realisation is coarser than the paper's rule.

**R7.1 (the chain is a property of the request, and holds below it).** A request resolves in
⟨injected corridors innermost first, root unless severed⟩. The kernel stamps the invocation
with the chain its request resolved in, and every sub-request — through `issue`, `source`,
`issue_attenuated`, `fan_out` across a spawn — resolves in the same chain. An injected corridor
shadows a root door for the request and for everything below it; a severed chain leaves a
root-bound name `Unresolved` for everything below it.

    pin: `scope::an_injected_corridor_shadows_a_root_door_for_the_request_and_for_its_sub_requests` (crates/ikigai-core/tests/scope.rs)
    pin: `scope::a_confined_sub_request_for_a_root_bound_iri_is_unresolved_and_the_root_endpoint_is_never_entered` (crates/ikigai-core/tests/scope.rs)
    pin: `scope::fan_out_from_a_confined_endpoint_stays_confined` (crates/ikigai-core/tests/scope.rs)

**R7.2 (the fingerprint: names, order, severed-ness; whole chain).** `Scope::fingerprint` is
BLAKE3 over whether the root is present and each corridor's identity in chain order — a name
the injector supplied, or a process-unique number for an anonymous corridor. Same name ⇒ one
entry across rebuilt corridors; a different name, a different order, or severing ⇒ a different
entry; an anonymous corridor shares only with its own clones. The empty chain fingerprints to
0, so every key built before scopes existed is the empty chain's key, byte for byte. The
fingerprint covers the **whole chain, not the corridors consulted**: sound and over-partitioned
(the paper's §5.1 remedy, keying on consulted corridors, needs the resolver to report which
corridor answered — the same seam a trace that names the answering corridor needs, and not
built).

    pin: `scope::two_requests_with_the_same_named_scope_share_one_cache_entry_and_different_names_do_not` (crates/ikigai-core/tests/scope.rs)
    pin: `scope::the_empty_scope_is_the_status_quo` (crates/ikigai-core/tests/scope.rs)
    doctest: `Scope`
    UNPINNED — consulted-corridors keying: a test would inject two corridors, source a name only the outer binds, and assert one entry is shared with a chain lacking the inner corridor; today it is not

**R7.3 (`urn:kernel:*` is ahead of the chain).** The kernel intercepts its own namespace before
the chain and before the root: an injected corridor cannot shadow a kernel operation, a severed
chain still reaches one (capability-gated as ever), and `Alias` refuses to alias one away. The
answer therefore does not depend on the chain and is keyed with scope 0. Consequence, stated:
a confined endpoint can read `urn:kernel:catalog` and see names it cannot resolve — names, not
content, the same leak golden-thread names already carry.

    pin: `scope::an_injected_corridor_cannot_shadow_urn_kernel_and_a_severed_chain_still_reaches_it` (crates/ikigai-core/tests/scope.rs)
    pin: `alias::the_kernel_namespace_cannot_be_aliased_away` (crates/ikigai-core/tests/alias.rs)
    pin: `alias::tests::the_kernel_namespace_is_not_aliasable` (crates/ikigai-core/src/alias.rs)

**R7.4 (the floor runs against whichever corridor answered).** A corridor shadowing an open
root door with a gated one is gated on its own declaration, and its cached entry is fenced from
a caller the declaration refuses.

    pin: `scope::the_capability_floor_is_evaluated_on_the_corridor_endpoint_that_answered` (crates/ikigai-core/tests/scope.rs)

**R7.5 (injection is authority; severing is not).** Whoever pushes a corridor innermost can
stand in for any door for every sub-request of that resolution, so injection with the root
present is reachable only by whoever holds the `Kernel` (`Kernel::issue_in`) — the trust line
of `Capability::root()`. From inside an endpoint the only chain-changing operation is
`Invocation::confine`, whose placement (§1.3) makes it strictly narrowing; `Invocation::with_scope`
is crate-private for the reason `issue_under` is (R2.2). Nested confinement adds doors in the
root's old position and shadows nothing.

    pin: `scope::an_endpoint_can_only_narrow_its_chain` (crates/ikigai-core/tests/scope.rs)
    UNPINNED — the visibility half: a `compile_fail` doctest calling `Invocation::with_scope` from outside the crate would hold it, and none exists

**R7.6 (the chain is legible).** Every traced event of a non-empty-chain resolution carries the
chain, innermost first, ending in `root` or `severed`; a miss inside a non-empty chain is
traced with the name that had no binding there, since the error is indistinguishable from
"unbound" by design. Empty-chain events are byte-identical to before scopes existed.

    pin: `scope::the_chain_is_disclosed_on_every_traced_event_and_a_confined_miss_is_traced` (crates/ikigai-core/tests/scope.rs)

**R7.7 (the wire drops the chain — the paper's named exception).** A mounted remote cannot
carry a chain: the wire has no field for it. A `Confine`d endpoint that reaches a mount inside
S escapes the confinement at the wire, and the remote resolves in its own root — in Prop. 4's
terms a mount is a *named exception endpoint* whose external access is part of the boundary,
so confining to a corridor that contains a mount confines to whatever the mount reaches. That
is the host's decision when it builds S. The half core can close, it closes: the default
`Issuer::issue_in_scope` **refuses** a non-empty chain instead of dropping it, so an issuer
written before scopes existed fails loudly rather than resolving in the plain root on the
branch that looks like success.

    pin: `scope::an_issuer_that_cannot_carry_the_chain_refuses_a_non_empty_scope_rather_than_escaping_it` (crates/ikigai-core/tests/scope.rs)
    UNPINNED — the escape at the wire is in ikigai-cli's `MountedRemote`; a test there would confine an endpoint to a corridor holding a mount and record that the remote resolved in its own root

**The clock seam.** `Invocation::now()` reads the injected `Clock`; a temporal corridor pinning
`urn:time:now` is a resolution-seam claim about time. Decided 2026-09-25
([#517](http://localhost:1060/l/default/item/517)): the chain carries a `Clock` *derived* from
the corridor that binds `urn:time:now` at injection, `now()` prefers it, and the kernel keeps
its own clock for validity so a pinned past never un-expires a live entry. Not built.

    UNPINNED — a test would inject a corridor binding `urn:time:now` to T and assert `inv.now()` inside reads T while a cached `At` entry is still judged against the kernel clock

**The read measurement** (§10): injecting the chain into every issue cost 0–10 ns on a ~410 ns
cache-hit read once the empty chain became a null handle.

---

## 8. Transreption

Def. 8: a transreption is an **injective** function on representations that changes form
without changing which resource is represented; Def. 9: a projection is any non-injective map,
and a projection of r is a different resource with its own identifier; Thm. 1: H(f(X)) ≤ H(X)
with equality iff f is injective on the support.

**R8.1 (the declaration).** A transreptor is an endpoint of kind `Transreptor(Transreption
{ from, to })`: the media types it reads and the media types it produces. Selection finds a
direct edge from → to if one exists, else a two-hop pivot through the canonical hub
`text/turtle`; only auto-invocable transreptors (every required input is `content` or `as`)
are selected; identity and unreachable pairs select nothing. Meta rendering rides the same
selection to reach a non-canonical type.

    pin: `describe::tests::transreptor_builder_records_its_conversions` (crates/ikigai-core/src/describe.rs)
    pin: `select::tests::finds_a_direct_hop` (crates/ikigai-core/src/select.rs)
    pin: `select::tests::pivots_via_turtle_when_no_direct_hop` (crates/ikigai-core/src/select.rs)
    pin: `select::tests::none_when_unreachable_or_identity` (crates/ikigai-core/src/select.rs)
    pin: `select::tests::parameterized_transreptors_are_not_auto_invocable` (crates/ikigai-core/src/select.rs)
    pin: `kernel::tests::meta_transrepts_to_a_non_canonical_type_via_selection` (crates/ikigai-core/src/kernel.rs)

**Proposition R8.2 (the star, and two hops).** With one hub, the representation spaces
reachable by selection form a star of radius one around `text/turtle`, and every selected
plan has length ≤ 2. If t₁ : s → hub and t₂ : hub → s′ are transreptions (Def. 8) then t₂∘t₁ is
one: a composition of injective functions is injective, and the defining equation composes.
Conversely, if the two-hop plan is a transreption then t₁ is injective on its domain and t₂ is
injective on t₁'s image — so a lossy first edge is never rescued, and a lossy second edge is
rescued only where the hub representations actually reached happen to be distinguished by it.
"Two-hop lossless iff both edges are" is the safe reading; the pivot test pins the plan shape,
not injectivity.

    pin: `select::tests::pivots_via_turtle_when_no_direct_hop` (crates/ikigai-core/src/select.rs)

**Injectivity is a declaration, and today an undeclared one** ([#514](http://localhost:1060/l/default/item/514)).
`Transreption` carries no lossless flag; nothing in core distinguishes a transreption from a
projection, so a structural lift (markdown → RDF), an arbitrary XSLT, or a model's output
registered as a transreptor becomes a silent hop in an `as=` request, and the caller receives a
projection believing it received the same resource in another form. The pivot doubles the
exposure. Core cannot enforce injectivity — like `requires` before 0.1.49 it can only carry the
declaration and let conformance round-trip where a reverse edge exists.

    UNPINNED — a test would register a lossy transreptor and assert `select_transreptor` will not route through it without an explicit opt-in; today it routes

**A deviation in strictness.** When no transreptor reaches a requested Meta type, the kernel
serves the canonical Turtle rather than failing. The paper's "choice of representation space"
would call that a substitution; ikigai's Meta prefers a description in a space the caller did
not ask for to no description. Stated so a reader is not surprised.

    pin: `kernel::tests::meta_falls_back_to_turtle_when_no_transreptor_reaches_the_type` (crates/ikigai-core/src/kernel.rs)

---

## 9. The register: UNPINNED and conditional

This list is the finding. Each item is a claim the code cannot yet vouch for, with the ledger
item that would discharge it where one exists. The gate counts these; it does not resolve them.

**Absent constructs (the paper has them, ikigai does not):**

- §1 — limiter: no third resolution outcome (#511).
- §1, §5 — nesting budget on sub-requests; the depth would not cross the wire (#513).
- §1 — path cache; its validity predicate, a binding-change thread (#510).
- §1 — the arrangement as a resource (`urn:kernel:topology`) (#515).
- §8 — the lossless flag on `Transreption`; selection through lossy edges (#514).

**Conditional theorems (stated with the precondition explicit):**

- R4.4 — the write-cut invalidates a cached read only if the read carries an edge on its own
  canonical target; supplied by hand in every pinned test. Hole A of #512 discharges it.
- R4 hole B — a failed sub-request records no dependency. Fix B of #512.
- R3.2 — the key does not identify the binding behind the canonical name (#510, #26).
- R3.2 — two facts in the key are claims: a corridor's name (same name ⇒ same doors) and a
  reported canonical (a name in this kernel's namespace). Both pinned as declarations
  (doctests), neither observable by the kernel.
- R2.2 and R7.5 — the structural halves ("no public path takes a capability", "no endpoint can
  set its own chain") are visibility facts with no `compile_fail` doctest.
- R7.2 — the fingerprint covers the whole chain, not the corridors consulted.
- R5.1 — hypotheses (i) foreign `Space`s call only what they enclose and (ii) the `Arc` graph
  is acyclic are review, not tests.

**Outside this crate (realised, not reachable by a test here):**

- §1 — opaque overlay (`MountedRemote`), the served surface (`ikigai-embedded`), Hypothesis H
  for wasm modules (`ikigai-module`), the QUIC clamp site (`ikigai-cli`).
- R7.7 — the escape at the wire.

**Not built, by decision:**

- §2 — delegation (Part B of `sub-request-authority.md`).
- §7 — the scope-level clock (#517, decided 2026-09-25).
- §1.4 — the behavioural half of "values are not corridors" has no test.

**Pins that did not exist under the name the outline guessed** (recorded for the hub): the
race test from PR #108 is `a_cut_during_an_in_flight_invocation_is_not_consumed_by_the_entry_it_invalidates`;
the "§9.4 test" is the pair `a_result_computed_{outside,inside}_confinement_is_not_served_{inside,outside}_it`;
the `urn:kernel:actions` test is `the_action_manifold_is_capability_scoped`; the kernel.rs
"~3911" test is `cutting_a_thread_invalidates_the_entry_that_declared_it`; the `select_transreptor`
pivot test is `pivots_via_turtle_when_no_direct_hop`; the "cycle/over-long chain refusal tests
in `alias.rs`" are `a_cycle_is_refused_not_truncated` and `an_over_long_chain_is_refused`
(unit) plus `a_cycle_is_refused_rather_than_recursed_or_truncated` (integration); `tests/scope.rs`
holds thirteen tests, all pinned above.

---

## 10. Numbers cited, with date and instrument

| Number | What | Date | Instrument |
|---|---|---|---|
| ~0.6 µs | a cache-hit `Kernel::issue`, end to end | 2026-09-25 | scratch crate over core 0.1.71, release build, `Fallback` of `Mount`s over `EndpointSpace`s, 200 000 iterations; 12 / 300 / 3000 bindings gave 601–628 / 589–662 / 577–902 ns (first / last binding). Ledger #510. Not committed. |
| 7–330 ns | `root.resolve` alone, same bench | 2026-09-25 | 7–21 / 7–96 / 7–327 ns at 12 / 300 / 3000 bindings, first / last binding. `describe()` 124–138 ns; `request.id()` 144–155 ns. Ledger #510. |
| 408–411 ns → 410–421 ns | cache-hit read before / after the chain landed | 2026-09-25 | twenty-line bench against the public API, one cacheable `FnEndpoint`, 200 000 re-issues × 3 rounds, release, `futures::block_on`, M-series laptop; interleaved runs under load ~2.5. First cut (two `Vec`s by value) was +16–24 ns; the shipped `Option<Arc<Chain>>` is 0–10 ns above main. Table in `docs/design/resolution-scope.md`, PR #120. Not committed. |
| ~20 µs → ~1.0 s | a cached graph read after one uncacheable source was joined in | 2026-08-13 | `ikigai-cms-web` PR #71, measured on the live reading room; 68 tests passed on the slow version. |
| 8 | `DEFAULT_MAX_HOPS`, the alias chain cap | — | `alias.rs`, a constant. |
| 4096 | `CUT_LOG`, cuts the race check remembers | — | `cache.rs`, a constant. |
| 4096 entries / 64 MiB | the default LRU bound | — | `cache.rs`, `CachePolicy` default. |

Brian's instruction on the first two rows (2026-09-25): revisit once scope, limiter, depth
bound, lossless flag and topology are in; re-measure, do not inherit.

---

## 11. Version note

This document and its gate change nothing the crate publishes: no source under `src/`, no
public item, no behaviour, no vocabulary. The gate is a dev-only integration test that reads
this file from the repository. A patch bump would move ~31 lockstep consumers for nothing; the
crate README's one-line link rides whatever bump comes next. **No bump.** One consequence,
named: the test file is packaged with the crate (Cargo includes `tests/`), and run from an
unpacked `.crate` it fails, because the document is not there. That is the same class as a test
that reads the repository, and it is preferable to a gate that silently passes on an absent
document.
