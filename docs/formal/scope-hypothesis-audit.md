# Audit: does ikigai-core enforce that scope changes only at declared boundaries?

**Status:** audit of ikigai-core 0.3.0 at `fd35136`, 2026-11-05, ledger
[#3149](http://localhost:1060/l/default/item/3149), under the continuous formal assurance epic
[#2383](http://localhost:1060/l/default/item/2383). It changes no behavior: it adds this document,
tests that hold today's behavior, and compile-only doctests.
**Cites into:** the gatekeeper certificate ([#2385](http://localhost:1060/l/default/item/2385)),
the formalism's Theorem 4(b) (`docs/formalism/README.md` §1.1, R7.3, R7.5). The ambient gate
([#2384](http://localhost:1060/l/default/item/2384)) owns the certificate's other hypothesis, H.
**Gate:** every `pin:` and `doctest:` below is resolved by `crates/ikigai-core/tests/formalism_pins.rs`.
A test whose name starts `finding_` holds a defect as it stands: fixing the finding turns it red,
and that pull request rewrites the row here.

## The answer

**No: core does not enforce the hypothesis completely.** It enforces most of it, and the gap has
a precise shape.

The hypothesis has two parts, and they come apart:

1. **Reach.** An endpoint holding only its `Invocation` cannot resolve anything its chain does not
   already resolve, plus doors of a space value it already holds. There is no public way to hand
   a sub-request a chain, to obtain the issuer, or to push a level. This part holds, and it is
   pinned below.
2. **Declaredness.** The scope a sub-request resolves in is not always visible in the arrangement.
   `Invocation::confine` picks a corridor in the endpoint's body, and nothing in
   `urn:kernel:topology` shows it. Any endpoint holding a `Kernel` picks any chain at all. Neither
   appears to a checker reading the graph; the second makes a door's own `ik:confinedTo` false.

**What the certificate can cite today**, for Theorem 4(b): every path from an entry point to a
guarded corridor passes the gatekeeper, **provided** that (i) no endpoint in the arrangement holds a
`Kernel` or an `Issuer` of it, (ii) no endpoint holds a space value containing a guarded door, unless
its door declares it, and (iii) the walk's answer is "unknown", not "no", wherever the graph is
opaque. Conditions (i) and (ii) are capability facts about closures: core cannot see them, and
nothing checks them today. They belong with the ambient gate, or with the changes suggested below.
**Condition (i) is false of `ikigai-gonk` today** (B4), so a certificate for that host needs those
doors named as exceptions, or those edges modeled.

**Run-time naming is absent, and that is a good result.** Nothing in core resolves a space, corridor,
level or kernel by name or IRI. A space's name is a claim the cache and the topology read, never an
address, so cross-space reach exists only along edges someone composed.

    pin: `scope_hypothesis::no_space_is_resolved_by_its_name` (crates/ikigai-core/tests/scope_hypothesis.rs)
    pin: `scope_hypothesis::a_scope_an_endpoint_builds_is_inert` (crates/ikigai-core/tests/scope_hypothesis.rs)
    pin: `scope_hypothesis::a_sync_issuer_stays_in_the_chain_of_the_invocation_that_minted_it` (crates/ikigai-core/tests/scope_hypothesis.rs)
    pin: `scope::an_endpoint_can_only_narrow_its_chain` (crates/ikigai-core/tests/scope.rs)
    pin: `scope::fan_out_from_a_confined_endpoint_stays_confined` (crates/ikigai-core/tests/scope.rs)
    doctest: `ScopeHypothesisPins`
    doctest: `Invocation::with_scope`

## How to read the kinds

- **(a)** a declared boundary a static checker can read, or an API that cannot change scope at all.
- **(b)** arbitrary: endpoint code can choose the scope at run time.
- **(c)** unclear: it depends on what a host composed, or the graph cannot say.

"Host" below means code holding the `Kernel` at build or at a door. Core has no type that separates
host code from endpoint code, and that is the root of finding B4.

## Inventory

### From an `Invocation`

| API | Kind | Declared where | Reasoning |
|---|---|---|---|
| `Invocation::{issue, source, issue_body, source_body, issue_attenuated, source_attenuated, fan_out}(…Request or &Iri…)`, `transrept`, `content_body` | (a) | The chain is the kernel's: root, corridors and the found path | None takes a scope. All route through private `issue_under`, or carry `self.scope` across the spawn. The target IRI is free, so a checker must assume any name is asked, which the walk already does |
| `Invocation::scope_sync`, `SyncIssuer::{issue, source}` | (a) | As above | Each call is served by the minting invocation's `issue`. No chain parameter exists |
| `Invocation::confine(&self, name: Iri, space: Arc<dyn Space>) -> Invocation` | **(b)** | Nowhere | B1, B2 |
| `Invocation::scope(&self) -> &Scope` | (b) through `spaces` | Read-only otherwise | B3. The rest of what it returns only reads |
| `Scope::spaces(&self) -> &[Arc<dyn Space>]` | **(b)** | Nowhere | B3 |
| `Scope::{levels, is_severed, is_empty, fingerprint, clock, now}`, `LevelPath::{names, len, is_empty}` | (a) | Read-only | An endpoint may branch on them and can change nothing. A level path gives names, never spaces |
| `Scope::{empty, with, with_named, with_named_at, sever, confined, stack}`, `Confine::chain_from` | (a) | Inert values | A `Scope` takes effect only when issued in, and that takes a kernel (B4) |
| `Invocation::{with_bindings, with_retries, with_memory_limit, with_not_after, with_stale}` | (a) | No scope effect | Each reborrow keeps the chain |
| `Invocation::with_clock(self, Arc<dyn Clock>) -> Self` | (a) | No scope effect | Reachable on a reborrow, contrary to `resolution-scope.md`, but the clock stays on that handle |
| `Invocation::with_scope`, `with_depth`, `with_run`, `Scope::descend` | (a) | `pub(crate)` | Crate-private |
| `Invocation::detached(request, bindings, capability)` | (a) | No issuer | Cannot issue |
| `Invocation::with_issuer(request, bindings, capability, issuer: &dyn Issuer)` | **(c)** | Nowhere | C1 |
| `ArgRef::Reference(Iri)`, settled by the kernel from the contract | (a) | Callee's resolved scope, decision 6 of `resolution-scope.md` | C7: designed behavior |

    pin: `scope_hypothesis::a_clock_attached_on_a_reborrow_stays_on_the_handle` (crates/ikigai-core/tests/scope_hypothesis.rs)

### The kernel's handle

| API | Kind | Declared where | Reasoning |
|---|---|---|---|
| `Kernel::{issue_in, issue_with_incoming_in, issue_with, issue_body_with}`, `BodyIssue::in_scope(self, Scope)` | **(b)** when an endpoint holds the kernel; (a) from a host | Nowhere | B4 |
| `Issuer::{issue_in_scope, issue_at_depth, issue_recording, issue_in_context, issue_body_in_context}(…, scope: Scope, …)` | **(b)**, same | Nowhere | B4. The default refuses a non-empty chain, and an empty one is accepted, so it cannot hold a confinement against a holder |
| `Kernel::{is_cached_in, topology_in, select_transreptor_in, select_transreptor_in_with, select_action_in, select_actions_in}`, the free `select_*_in`, `Issuer::select_*_in` | (a) | Read-only | They answer about a chain and change nothing |
| `Kernel::new`, `try_new`, `with_meta_renderer` called inside an endpoint | **(c)** | Nowhere | C2 |
| `urn:kernel:topology`, `explain`, `validate` | (a) | `kernel_ops::depends_on_chain` | They answer in the asker's chain. No argument names a chain or a space: `explain`'s `scopes` are capability scopes |

### Spaces and the arrangement

| API | Kind | Declared where | Reasoning |
|---|---|---|---|
| `Confine::new(name, space, inner)` | (a) | The door's `ik:confinedTo`; a declaration's `ik:confinedTo` builds one | The boundary as the formalism wants it, subject to C9 and B2 |
| `Endpoint::confinement(&self) -> Option<Topology>`, `Endpoint::forwards(&self) -> Option<Forward>` | **(c)** | Self-reported | C9 |
| `Level::{new, sealing, in_namespace}` | (a) | `ik:Level`, `ik:seals` | The walk follows the pushes |
| `EndpointSpace`, `Mount`, `Fallback`, `Limit`, `Threaded::{new, session, named}` | (a) | `ik:EndpointSpace`, `ik:Mount`, `ik:Fallback`, `ik:Limit` | They consult members and change no scope. A `Threaded` shows its inner structure |
| `Kernel::with_sealed`, `check_sealing` | (a) | Kernel configuration; `Kernel::sealed` | They narrow. The walk is told a host's seals, which are not in the graph |
| `Alias::new`, `AliasTable::{exact, prefix}`, `Kernel::with_aliases` | (a) | `ik:Alias`, `ik:RewriteRule`, `urn:kernel:aliases` | Rules are data, and resolution stays in the child |
| `AliasTable::{exact_at_root, prefix_at_root}`, `ResolveAt::Root`, `TopologyRule::{named_request, named_template}` | **(c)** | `ik:resolveAt "root"` | C6: declared, but it does not mean the root inside a level |
| `Rewrite::new(inner, closure)` | **(c)** | `ik:Rewrite`, rule opaque | C5 |
| `Space::resolve(&self, &Request, &Scope) -> Resolution`, `Resolved::endpoint`, `Endpoint::invoke`, `invoke_body` | **(c)** | `ik:OpaqueSpace` by default | C3 |
| `Resolved::within(self, &Level) -> Self` | **(c)** | Not in the tree | C4 |
| `DynamicSpace::{new, with_check, install, install_topology, refresh, drive}` | **(c)** | `ik:DynamicSpace`, its current children | C8 |
| `declare::build`, `Registry::{register, get}` | (a) | The declaration itself | Endpoints are looked up by name at build, never spaces. Opaque spaces, closures and nested dynamic spaces are refused |
| `Space::id`, `space_iri`, `SPACE_PREFIX`, `SESSION_PREFIX`, `DYNAMIC_PREFIX` | (a) | `ik:` node identity | Names, never lookups. The one door a name yields is a dynamic space's own `{space}:status` |

## Findings: (b)

### B1. `Invocation::confine` is a boundary the arrangement never shows

An endpoint's body can sever its own chain into any corridor it can build, chosen from its input.
`Confine` does the same at a door and reports `ik:confinedTo`; this reports nothing.

```rust
let corridor = if inv.inline_str("pick")? == "a" { space_a } else { space_b };
inv.confine(iri("urn:ctx:picked"), corridor).source(&iri("urn:x")).await
```

It is strictly narrowing in reach (R7.5 holds), so it cannot by itself open a guarded door the
endpoint does not already hold. What it breaks is the hypothesis's form: this is endpoint code
constructing the scope its sub-requests resolve in. Everything found in the corridor then runs
behind a chain no reader of the graph can see.

    pin: `scope_hypothesis::finding_invocation_confine_chooses_a_corridor_the_arrangement_never_shows` (crates/ikigai-core/tests/scope_hypothesis.rs)

**To make it declared:** make it `pub(crate)`, leaving `Confine` as the one public path. **Cost:**
no caller outside core in the repositories searched. Inside core: `Confine::invoke`, which stays,
one doctest, and three lines of tests (`tests/scope.rs`, `tests/temporal_corridor.rs`).

### B2. A corridor's name is unchecked, so an endpoint can answer inside another door's confinement

The cache keys a chain on corridor names (R7.2). `check_claim` compares a name only to the identity
a space claims for itself, and an anonymous space claims none. So an endpoint can name its corridor
after a door's declared confinement, under the same capability, and its answer is what that door's
endpoint then reads:

```rust
inv.confine(iri("urn:ctx:shared"), door("urn:trust-set", b"forged"))
    .source(&iri("urn:trust-set")).await
// then urn:verifier, a Confine over the genuine corridor named urn:ctx:shared, reads "forged".
```

The type's documentation already says a name is a claim, and that naming two corridors alike
serves one the other's answer. It treats that as the injector's error. `Invocation::confine` makes
the injector any endpoint. A sealed name is safe: inside a confinement a host-sealed name is
`Unresolved` (R7.12).

    pin: `scope_hypothesis::finding_a_confinement_name_is_unchecked_so_one_endpoint_can_answer_for_anothers_corridor` (crates/ikigai-core/tests/scope_hypothesis.rs)

**To make it declared:** B1 removes the run-time path. For `Confine` values, refuse at build a
confinement name bound twice over different structure, by the walk `Kernel::check_sealing` already
does. **Cost:** none for callers in the repositories searched: `Confine::new` appears outside core
only in `ikigai-sexpr`'s tests. A corridor that is opaque cannot be compared, so the check would
state that limit.

### B3. `Scope::spaces` hands an endpoint live corridors, and dispatching through one skips the floor

`inv.scope().spaces()` returns the chain's corridors, the host's and the confined ones, as live
spaces. The endpoint can resolve in one and invoke what it finds, outside the kernel. So a door whose
declared floor refuses this caller answers it anyway, with no cache entry or trace event of its own.

```rust
let Resolution::Hit(hit) = inv.scope().spaces()[0].resolve(&request, inv.scope()) else { .. };
hit.endpoint.invoke(inv).await // `urn:cap:gated` is required of a sub-request, and never asked here
```

It adds no reach by name, since the chain already held these doors. It does contradict R7.4 (the
floor runs against whichever corridor answered) for any corridor on the chain. The same space value
fed to `Kernel::new` gives C2.

    pin: `scope_hypothesis::finding_scope_spaces_lets_an_endpoint_dispatch_past_a_corridors_capability_floor` (crates/ikigai-core/tests/scope_hypothesis.rs)

**To make it declared:** make `spaces` `pub(crate)`, and offer the corridor names if anyone needs
them. **Cost:** no caller outside core in the repositories searched (`.spaces()` in `ikigai-gonk` is
a different method on its own types). Inside core: `tests/stack.rs` twice, `tests/body_issue.rs` once.

### B4. An endpoint holding a `Kernel` chooses any chain, and its door can report the opposite

Nothing keeps a `Kernel` out of a closure, and `issue_in` takes the chain as an argument. R7.5 draws
the trust line at whoever holds the kernel, and no type draws it. A door reporting `ik:confinedTo`
can read the root:

```rust
let kernel = handle.get().and_then(Weak::upgrade).unwrap();
kernel.issue_in(source("urn:secret"), inv.capability, Scope::empty()).await
```

Plain `kernel.issue` has the same effect: it resolves in the empty chain, so it drops the caller's.

    pin: `scope_hypothesis::finding_an_endpoint_holding_the_kernel_leaves_the_confinement_its_door_reports` (crates/ikigai-core/tests/scope_hypothesis.rs)

**In use today.** Holding a kernel is a common pattern, not an oddity: non-test code in 15 of the
repositories searched has a field or parameter of that type, most of it in host code. Four sites
were read and confirmed to issue through a held kernel from inside an endpoint:

- `ikigai-gonk` `src/doors.rs`, `Forward`: every door's kernel into the hub, under the caller's
  capability.
- `ikigai-gonk` `src/queue.rs`, `read_through`: page reads go to the hub directly, under the caller's
  capability, which its own documentation explains.
- `ikigai-gonk` `src/system.rs`, `read_as`: reads the hub under `Capability::scoped(…)` that the code
  itself mints, so this one bears on H as well.
- `ikigai-tutorial` `crates/ttt-host/src/lib.rs`, `Forwarded`: `issue_in` in a chain the host chose.

These look like deliberate host-built bridges, not escapes, but a checker cannot tell them from one.
**This list is a lower bound.** Sites that hold a kernel and were seen only to cut threads or refresh
a space (`ikigai-cli` `names.rs` and `ui.rs`, `ikigai-throttle` `state.rs`, `ikigai-cms-web`
`presentations.rs`, `ikigai-gonk` `script.rs`) were not traced through their whole call graphs, and
files where an endpoint and a held kernel merely sit together, with no call traced between them
(`ikigai-cli` `ikigai-embedded/src/lib.rs`, `ikigai-tutorial` `shop` and `live-space`,
`ikigai-name` `server.rs`, among others), were not classified. Measure this before choosing a fix.

**To make it declared**, in increasing order of cost:

1. Add a rule to the ambient gate ([#2384](http://localhost:1060/l/default/item/2384)): a type that
   implements `Endpoint`, or a closure bound as one, holds no `Kernel`, `Weak<Kernel>` or
   `dyn Issuer`, except at an allow-listed door. **Cost:** at least the four sites above, as
   allow-list entries or rewrites to `inv.issue`; the total needs the measurement above. Whether a
   lint can see a closure's captures also needs checking; this audit did not read the gate.
2. Declare the crossing: `Endpoint::crosses_kernel() -> Option<Iri>`, defaulted to `None`, rendered
   as a new door property a walk treats as "unknown" or follows. **Cost:** a `vocabulary.ttl` edit
   and a redeploy, plus each door above; a door that does not say so stays unseen, so it needs (1).
3. Move the chain-taking `Issuer` methods to a sealed trait only `Kernel` implements. **Cost:** none
   for the four `ikigai-module` issuers, which implement `issue` only, and no non-test caller outside
   core was found. Two core tests implement `Issuer` (`PlainIssuer`). This shuts `&dyn Issuer` but not
   an owned `Kernel`, so it complements (1) and does not replace it.

## Findings: (c)

Each is triaged (declared, an exception the certificate names, or a finding of its own) in
[scope-hypothesis-triage.md](scope-hypothesis-triage.md).

**C1. `Invocation::with_issuer`.** It starts an invocation in the empty chain, at depth 0, with a
fresh run. It exists for `ikigai-module`'s shim, whose issuer crosses a wire that carries no chain.
With a kernel in hand, a decorator can run the endpoint it wraps outside the chain it was given.
Without one it reaches nothing, since an invocation hands out no issuer. Example:
`inner.invoke(&Invocation::with_issuer(req, b, inv.capability, &kernel))`. It resolves with B4.
Elsewhere it appears in test code of `ikigai-conformance`, `ikigai-encrypt`, `ikigai-json` and
`ikigai-linkeddata`.

    pin: `scope_hypothesis::finding_with_issuer_starts_an_invocation_in_the_empty_chain` (crates/ikigai-core/tests/scope_hypothesis.rs)

**C2. `Kernel::new` inside an endpoint.** Any endpoint can build a kernel over a space value it holds
and issue in it under `Capability::root()`. That is reach only into what it already holds, but it
dispatches past every floor, and it uses a cache and a chain no one else sees. Example:
`Kernel::new(Arc::clone(&inv.scope().spaces()[0])).issue(req, &Capability::root())`. The fix for B3
removes the source inside core; a space held by capture is H's question. No test pins this one: it
follows from B3 and the public constructors.

**C3. Direct dispatch.** An overlay holding a space or endpoint can call `resolve` and `invoke`
itself. The target then runs in the invoker's chain, not where it was found, and the kernel's floor,
cache and limiter check are skipped. `ikigai-throttle` (`src/body.rs`), `ikigai-gonk` (`src/budget.rs`,
`src/sparql.rs`) and core's own overlays use the shape, so it is not removable. The decorator form is
sound when the door's own floor already ran. Whether a given overlay's use is sound needs its own
reading; this audit did not do that. There are 39 `impl Space for` sites outside core, with test
directories excluded.

**C4. `Resolved::within`.** A space can report any `Level` it holds, including one nowhere in the
tree, and its endpoint then runs in that level. The graph shows `ik:OpaqueSpace`, and the walk says
"unknown", which is the correct answer. No caller outside core was found, so `pub(crate)` costs nothing there, though the
method's own documentation offers it for spaces that hold a level; that use would need a replacement.

    pin: `scope_hypothesis::finding_an_opaque_space_can_place_its_endpoint_in_a_level_the_tree_does_not_show` (crates/ikigai-core/tests/scope_hypothesis.rs)
    pin: `topology::an_opaque_space_behind_the_wall_is_not_answered_and_the_second_question_says_so` (crates/ikigai-core/tests/topology.rs)

**C5. `Rewrite`.** The node says a rewrite happens here and nothing about the rule. This is an honest
stop, and the walk treats it as one. `build` refuses it in a declaration. A closure could rewrite by
anything it captured, but it changes a name, not a scope.

    pin: `topology::a_rewrite_behind_the_wall_is_not_answered` (crates/ikigai-core/tests/topology.rs)

**C6. A view declared at the root does not resolve from the root inside a level.** `ViewEndpoint`
issues through its invocation, whose chain is its resolved scope. So inside `Level` M a rule with
`ik:resolveAt "root"` asks M first and never reaches the root's door of that name. This may be the
intended reading of "the root of the caller's resolution chain", and the pushdown walk does cover
it, but `ik:resolveAt "root"` reads as the opposite, and a checker taking the keyword at its word is
wrong. **Suggested:** decide, and either document it on `ResolveAt::Root` and in the vocabulary, or
resolve past the level stack. The second changes answers only for a view inside a level.

    pin: `scope_hypothesis::finding_a_view_declared_at_the_root_resolves_from_its_level_when_it_is_inside_one` (crates/ikigai-core/tests/scope_hypothesis.rs)

**C7. By-reference arguments are read in the callee's scope (designed).** The kernel settles a
reference through the callee's invocation, in the callee's resolved scope. A `urn:m:takes` request
carrying `ArgRef::Reference(urn:name)`, issued from the root, therefore reads the module's `urn:name`
when the module binds one. This is designed behavior under
[decision 6 of the resolution-scope design](../design/resolution-scope.md#6-values-are-not-corridors-here):
a referent outside a confinement is `Unresolved`, which is the trapdoor working. `SettleReferences`
reads in the chain the endpoint runs in, just as the endpoint would if it sourced the name itself.
The caller's capability still governs the read. `ArgRef::Reference` documents this scope; C7 is not
a defect.

    pin: `scope_hypothesis::designed_a_by_reference_argument_is_read_in_the_callees_scope` (crates/ikigai-core/tests/scope_hypothesis.rs)

**C8. `DynamicSpace`.** The arrangement is a value that changes: whoever may write the declaration can
add doors for what the registry holds, and a certificate checked on version N says nothing about
N+1 unless the host's `SpaceCheck` re-runs it. The graph is truthful at each moment, and build refuses
opaque spaces and closures, so it is a declared boundary with a moving declaration. `with_check` is
optional. **Used:** `ikigai-fn` and `ikigai-gonk` (outside its test-only `Views::unchecked`) pass a
check; `ikigai-cli` (`names.rs`) and `ikigai-tutorial` (`book-wasm`, `live-space`) do not. **Suggested:** the certificate should state that
a dynamic space's doors are covered only under a check that repeats it; whether a kernel should
require one under a limiter is a separate decision.

    pin: `dynamic::the_hosts_check_runs_before_bind_and_can_refuse` (crates/ikigai-core/tests/dynamic.rs)

**C9. Self-reported structure.** `confinement`, `forwards`, `Space::topology` and `Space::id` are what
a checker reads, and each is the implementor's word. An overlay that does not forward `confinement`
reports its door as unconfined; B4 reports a confinement that does not hold. Core's overlays and
`ikigai-gonk`'s forward it. A static checker therefore proves a property of the reports. **Suggested:**
state that in the certificate, and pin overlays in each repository the way `ikigai-gonk` does.

    pin: `topology::a_door_bound_to_a_confinement_reports_the_corridor_as_its_confined_to` (crates/ikigai-core/tests/topology.rs)

## Durable context carried into sub-requests

| Context | Travels? | Declared? | Finding |
|---|---|---|---|
| A host's injected corridors (`Kernel::issue_in`) | Yes, at every depth and across `fan_out` | **No**: not in the root's topology, only in `topology_in(scope)` and the trace | **C10** |
| The chain's clock (`Scope::with_named_at`) | Yes, read by `Invocation::now` | **No**; the trace's `SCOPE_CLOCK_NOTE` shows it. It is not fingerprinted, on purpose (`resolution-scope.md`) | **C10** |
| Confined corridors | Yes, below the confining endpoint | At the door for `Confine`; nowhere for `Invocation::confine` | B1 |
| The level stack | **No**: each resolution replaces it with the found path | `ik:Level` | None |
| Capability, depth, trace scope | Yes | Not scope: they refuse or observe | None |
| Run context: retry pool, byte pool, deadline, attached body | Yes | Not scope: they name no space and change no resolution | None |
| Process state: `SEALING_LEVELS`, `PREVIEWING`, the anonymous corridor counter | Not per request | They decide a build-time walk, an alias counter and an identity; no answer | None |

**C10. A host's corridor is the request-scoped level the hypothesis worries about.** It can shadow any
door but `urn:kernel:*` for a whole subtree, and the arrangement a checker reads does not show it. The
design calls this host authority (R7.5, decision 3 in `resolution-scope.md`), and it is the point of
as-of corridors. So this is not a defect in the code; it is a condition the certificate must state: a
limiter in the root does not wall a corridor's doors, and only `topology_in` of the chain actually
used shows them. Host corridors in the repositories searched: `ikigai-engine`'s `as-of=`
(`Engine::with_as_of_doors`, a fixed door set a host supplies; the line names the instant, never a
space), `ikigai-fn`'s prefix corridor, and `ikigai-tutorial`'s game, shop and spreadsheet corridors.
**Suggested:** a kernel-level list of the corridor shapes a host may inject, rendered into the
topology, with `issue_in` refusing others. **Cost:** every call site above gains a declaration at
build, and anonymous per-request corridors need a shape to match. An as-of door set is one shape,
which fits; a corridor rebuilt per game fits only if its doors are the same. Not small, so decide it
with the certificate's wording rather than ahead of it.

    pin: `scope_hypothesis::finding_a_host_corridor_and_its_clock_travel_below_the_request_undeclared` (crates/ikigai-core/tests/scope_hypothesis.rs)
    pin: `scope_hypothesis::the_level_stack_does_not_travel_into_an_endpoint_found_outside_it` (crates/ikigai-core/tests/scope_hypothesis.rs)

## Why these findings are in the open

`AGENTS.md` sends security reports to private vulnerability reporting, so this was weighed. B2 and B3
are the two that could read as vulnerabilities. Both need Rust code already linked into the host's
process and running as an endpoint, and such code can also call `Capability::root()` and read the
filesystem, which is what the ambient gate polices. A dynamically loaded module reaches its host
through an issuer that has no `Scope` to read and refuses a non-empty chain. No caller of
`Invocation::confine` or `Scope::spaces` outside core was found, so no script, plan or wire peer was
seen to reach either. On that reading they are gaps in an assurance claim, not a path across a trust
boundary. If a host treats in-process endpoints as mutually untrusted, that reading changes.

## What was searched

Bare, depth-one clones of every public repository the `ikigai-rs` organization listed on 2026-11-05,
read with `git grep` and never checked out or run: 54 repositories, `ikigai-a11y` through `ikigai-xslt`,
including `ikigai-engine`, `ikigai-cli`, `ikigai-gonk`, `ikigai-module`, `ikigai-tutorial`,
`ikigai-sexpr`, `ikigai-throttle` and `ikigai-fn`. `ikigai-browse/tests/corpus`, frozen old copies of
core, is excluded. Not searched: `.github`, which holds no crates; private repositories; anything outside
the organization. A search finds a call by its spelling, so "no caller" means none spelled that way,
and an aliased or macro-generated call would be missed.
