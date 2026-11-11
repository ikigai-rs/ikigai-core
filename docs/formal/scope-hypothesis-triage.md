# Triage: the scope-hypothesis audit's ten unclear findings (C1 to C10)

**Status:** proposed, for the maintainers' design decision. Written 2026-11-10 against
ikigai-core 0.3.5 (`dc6d0ac`), ledger [#3156](http://localhost:1060/l/default/item/3156). It
changes no behavior and no code. **Sources:** the
[audit](scope-hypothesis-audit.md#findings-c) (ledger
[#3149](http://localhost:1060/l/default/item/3149)), which has the inventory, the reasoning and the
cost to callers, and which this note does not repeat; the certificate it cites into (ledger
[#2385](http://localhost:1060/l/default/item/2385)).

## Each claim still holds

Every `finding_*` pin the audit wrote for a (c) item passes at `dc6d0ac`, so each still describes
the code as it is. The audit's own pins in `docs/formal/scope-hypothesis-audit.md` are the evidence,
and the gate resolves the ones repeated below. C2, C3, C5, C8 and C9 were checked against the
source, since their pins hold neighboring behavior. No source change between the audit's `fd35136` and `dc6d0ac`
touches these APIs.

## The three kinds

- **Declared**: the scope changes at a boundary the arrangement already shows, and the walk in
  `tests/topology.rs` accounts for it. No change to the certificate.
- **Exception**: the certificate holds only under a condition it must state. Most of these are the
  audit's conditions (i) to (iii) under [The answer](scope-hypothesis-audit.md#the-answer); three new
  conditions, (iv) to (vi), are proposed below.
- **Finding**: a defect of its own, to be filed as its own item.

## The decisions

| | API | Kind | Why |
|---|---|---|---|
| C1 | `Invocation::with_issuer` | Exception (i), and decision 7 | In process it needs a `Kernel` or an `Issuer`, which is condition (i) and resolves with B4. Across a wire, the empty chain is the hole `resolution-scope.md` decision 7 already names: a mount is a named exception endpoint (Prop. 4). The reset depth and fresh run add nothing a held kernel's `issue` does not already allow |
| C2 | `Kernel::new` inside an endpoint | Exception (i), (ii) | It reaches only a space value the endpoint already holds, so condition (ii) covers it, including a door whose floor is its gatekeeper. Condition (i) should say "holds **or builds** a `Kernel`". Inside core, its source is B3 |
| C3 | Direct dispatch (`Space::resolve`, `Endpoint::invoke`) | Declared as a decorator; exception (ii) otherwise | A decorator wrapping one endpoint is that endpoint's door: the kernel ran the door's floor before the decorator was called, and the inner endpoint runs in the chain the door was found in. An overlay that resolves in a space it holds is condition (ii) |
| C4 | `Resolved::within` | Exception (iii) | An opaque space renders `ik:OpaqueSpace` and the walk answers "unknown", which is correct. A space that renders a structure and still reports a level outside it is a false report, which is C9. The one call site in core is `Level` itself |
| C5 | `Rewrite` closure | Exception (iii) | An honest stop: the walk treats it as one, `build` refuses it, and it changes a name, not a scope |
| C6 | `ik:resolveAt "root"` inside a `Level` | Declared, and a **finding** | For reach it is declared: the view issues in its resolved scope (R7.11), and the pushdown walk already walks each entered level's stack before the root, with any name asked. The finding is that the keyword's own text, on `ResolveAt::Root`, on `ik:resolveAt` in `vocabulary.ttl` and on `Alias`, says "from the root of the caller's resolution chain", and inside a level the answer comes from the level |
| C7 | `ArgRef::Reference` read in the callee's scope | Declared | It is a design decision already made, twice: `SettleReferences` reads a referent "in the chain the endpoint runs in", exactly what the endpoint would get by sourcing it itself (ledger #2511, decision 5), and `resolution-scope.md` decision 6 calls a referent outside a confinement being `Unresolved` "the trapdoor working". The callee can source any name anyway, so the walk, which assumes any name is asked, already covers it. What is missing is a sentence on `ArgRef::Reference` saying which scope |
| C8 | `DynamicSpace` without `with_check` | Exception (v), new | The graph is true at each version, and a certificate is about one version. Whether a kernel should require a check is a policy question for whoever writes the certificate, not a defect |
| C9 | Self-reported `confinement`, `forwards`, `topology`, `id` | Exception (iv), new | A static checker proves a property of the reports. Each core wrapper (`Confine`, `ThreadedEndpoint`, `Projected`, `SettleReferences`, `Revalidate`) forwards `confinement`. Pinning wrappers outside core is each repository's job |
| C10 | Host corridors and the chain's clock | Exception (vi), new | Host authority by design (R7.5, `resolution-scope.md` decision 3). The audit's suggested list of corridor shapes is a design change. Decide it with the certificate's wording, not before |

    pin: `scope_hypothesis::finding_with_issuer_starts_an_invocation_in_the_empty_chain` (crates/ikigai-core/tests/scope_hypothesis.rs)
    pin: `scope_hypothesis::finding_an_opaque_space_can_place_its_endpoint_in_a_level_the_tree_does_not_show` (crates/ikigai-core/tests/scope_hypothesis.rs)
    pin: `topology::a_rewrite_behind_the_wall_is_not_answered` (crates/ikigai-core/tests/topology.rs)
    pin: `scope_hypothesis::finding_a_view_declared_at_the_root_resolves_from_its_level_when_it_is_inside_one` (crates/ikigai-core/tests/scope_hypothesis.rs)
    pin: `scope_hypothesis::finding_a_by_reference_argument_is_read_in_the_callees_scope_not_the_callers` (crates/ikigai-core/tests/scope_hypothesis.rs)
    pin: `dynamic::the_hosts_check_runs_before_bind_and_can_refuse` (crates/ikigai-core/tests/dynamic.rs)
    pin: `topology::a_door_bound_to_a_confinement_reports_the_corridor_as_its_confined_to` (crates/ikigai-core/tests/topology.rs)
    pin: `scope_hypothesis::finding_a_host_corridor_and_its_clock_travel_below_the_request_undeclared` (crates/ikigai-core/tests/scope_hypothesis.rs)

## What the certificate should add

Beside the audit's (i) to (iii), with (i) reading "holds or builds":

- **(iv)** It proves a property of what each space and endpoint reports about itself (C9, C4).
- **(v)** A dynamic space's doors are covered at the version checked, and at later versions only
  under a `SpaceCheck` that repeats the check (C8).
- **(vi)** It covers requests issued in the empty chain. A request issued in a host's chain is
  covered only by the same walk over `topology_in` of that chain, and a limiter in the root walls
  no corridor's doors (C10).

Wire mounts stay the named exception of `resolution-scope.md` decision 7 (C1).

## Items to file

The worker that wrote this note may not file items, so these are left to the dispatcher.

1. **Finding (C6).** `ik:resolveAt "root"` resolves from the view's own level when the view is
   inside a `Level`, and its text says the root. Decide the meaning, then either change the text
   (`ResolveAt::Root`, `Alias`, `ik:resolveAt` and `ik:RewriteRule` in `vocabulary.ttl`) or resolve
   past the level stack. The second changes answers only for a view inside a level. Either way the
   `finding_` pin changes: renamed if the text changes, red if the behavior does.
2. **Not a finding, small (C7).** Add the scope a referent is read in to `ArgRef::Reference`'s
   documentation, citing decision 6. Then rename the pin, since it holds designed behavior, not a
   defect, and update the audit's C7 row to match.

The certificate's wording (C1, C2, C4, C8, C9, C10) belongs to
[#2385](http://localhost:1060/l/default/item/2385) and needs no item of its own.
