# Sub-request authority: attenuation, and delegation

**Status:** Part A (attenuation) **is built** — `Invocation::issue_attenuated` /
`source_attenuated`, this change. Part B (delegation — a module acting under authority
*it* holds) is **design only and deliberately not built**; the verdict and the
conditions that would change it are in the last two sections.

**Anchor types:** `ikigai_core::{Capability, Invocation, Kernel, TraceEvent}`.
**Companions:** `docs/design/cache-ejection.md` §2 (the capability fingerprint is not an
identity), `docs/design/trace-observability.md`.

---

## The gap, as found

`Invocation::issue` passed the invocation's own capability to every sub-request,
verbatim. There was no attenuating form and no elevating one. One consequence, two
shapes:

**A module built on a capability-gated module must make its callers hold the underlying
module's authority.** `ikigai-ledger` says so in its own source: *"`Invocation::issue`
has no attenuating or elevating form, so a ledger read succeeds only for a caller who
also holds the store's grant for this ledger's graph… That is why every action in this
crate declares the store scopes as well as its own."* Its read actions declare the broad
`urn:cap:store:read` alongside the per-graph family, because one enumeration path needs
the whole dataset; the crate guards that path with a `debug_assert!(inv.capability
.is_root())` and a doc comment explaining that under any other capability it would be an
over-offer. An authority-conditional code path with a debug assertion for a guard is
what a module writes when it cannot express delegation.

**And a module that dereferences a caller-named IRI does so with every scope its caller
happens to hold.** That is the same missing knob pointed the other way, and it is the
cheaper half to fix.

## Two problems, and they are not the same problem

|  | **A — attenuation** | **B — delegation** |
|---|---|---|
| Direction | strictly weaker | strictly different |
| Principal | unchanged | **changes** |
| Worst outcome | a refusal | confused deputy |
| Escalation possible | no, structurally | yes, if built wrong |
| Status | built | designed, not built |

Conflating them is the failure mode: "let an endpoint choose the capability of its
sub-request" reads like one feature and is two, one of which is safe by construction and
one of which is the whole of this document's caution.

---

## Part A — attenuation on a sub-request (built)

```rust
inv.issue_attenuated(request, ["urn:cap:fs:read"]).await?;
inv.source_attenuated(&caller_named_iri, ["urn:cap:fs:read"]).await?;
```

Both narrow this invocation's capability through the existing
`Capability::attenuate` — `Root → s`, `Scoped(t) → t ∩ s` — and issue under the result.
The intersection is the entire safety argument: asking for a scope the caller does not
hold yields nothing, so the worst outcome is a `Denied` further down. Dependency
recording (expiry, golden threads) is unchanged, because it is the same sub-request
under less authority; a test pins that, since losing it would show up only as a
composite that never invalidates.

**What it can honestly claim.** The narrowing is *voluntary*: the module still holds
`self.capability` and can still call `issue`. So this defends the module's downstream,
not the module itself. It is worth having anyway, because the motivating shape —
"resolve this IRI my caller named" — is the one place a module hands control of the
*target* to someone else while keeping control of the *authority*, and dropping the
scopes it does not need converts an exfiltration into a refusal.

**Not added, on purpose.** `fan_out` and the `scope_sync` bridge have no attenuating
form. Both carry the minting invocation's capability today (`fan_out` clones it into
each spawned task; `SyncIssuer`'s calls are served by the invocation that minted it, so
a sync scope cannot widen authority — there is a test for that). Adding attenuated twins
is mechanical but each one is public API in a lockstep-published workspace with ~31
consumers, and nothing needs them yet. When something does, they are three lines each
and they must route through the same private seam.

### ★ The load-bearing fact that was not written down anywhere

`Capability::root()` and `Capability::scoped()` are both **public**. Any code in the
process can *construct* a capability of any strength. The type's doc comment says the
handle "cannot be constructed from arbitrary data outside this crate", which is true and
is not the guarantee that matters here.

What actually makes in-process non-escalation structural is this: **an endpoint has no
way to hand a capability to an issuer.** `Invocation` owns the only reachable path back
into the kernel and has never offered to take one. A public `issue_under(request, cap)`
would give every endpoint root authority in one line — `inv.issue_under(r,
&Capability::root())` — and would do it without touching `attenuate`, `scoped`, or any
line of `capability.rs`.

So the seam this change added is deliberately private, with that reasoning on it. And it
fixes the shape of every future authority-carrying form:

> **An authority-carrying sub-request must take its authority FROM THE KERNEL, never
> from its caller.**

That is not a preference. It is the only arrangement under which the guarantee survives,
and it is — independently — the argument for the hub's belief that a module's authority
comes from its **binding** rather than from anything the module can say at call time.

---

## Part B — where a module's authority would come from

### The claim, argued

A host binds a module's endpoints. It is the party that decided this module exists in
this space, at these names, over this data — so it is the party entitled to say what the
module may do. Three alternatives, and why each fails:

- **The module declares its own grant.** Rejected by the fact above: authority the
  module can state is authority the module can mint. It also fails concretely for
  `ikigai-module`'s loadable-wasm shim, where the "module" is code the host did not
  write; a module-declared grant crossing that shim is the whole security model
  inverted. ⚠ **If delegation is ever built, the shim must not forward a
  module-declared grant.**
- **The caller elevates** (`IssueAs`-style, in-process). That is a widening primitive in
  the hands of the caller; it makes every endpoint a potential escalation path and
  turns "who may do this" into a runtime negotiation.
- **A per-request ambient grant.** ikigai has no ambient authority and should not grow
  one.

So: the binding. Which leaves *where in the binding*.

### Not the space algebra. A kernel-construction table.

The obvious shape is a combinator beside `Mount` / `Fallback` / `Rewrite`:
`Grant::new(scopes, inner)`, stamping every resolution from `inner` with a capability —
carried on `Resolved`, which today holds `{ endpoint, bindings, canonical }`.

It is the wrong shape, for one reason that outweighs its elegance: **a grant in the
space algebra can be declared by whoever constructs a space** — including a library, a
module crate's own `space()` helper, or a dynamically loaded module. A grant in a
*kernel-construction* table can only be declared by whoever constructs the kernel, and
whoever constructs the kernel is the host. Delegation authority belongs with the host,
so it belongs where only the host can write it.

```rust
let kernel = Kernel::new(space).with_grants([
    ("urn:iki:ledger:", ["urn:cap:store:write:graph:urn:iki:ledger:graph:acme",
                         "urn:cap:store:read:graph:urn:iki:ledger:graph:acme"]),
]);
```

Consequences of that choice, each settled rather than left to fall out:

- **Keyed on the CANONICAL target**, after alias/rewrite resolution — the kernel already
  computes it before the capability floor. Keying on the requested name would let a
  rewrite move a target into or out of a granted region.
- **No grant prefix may be a prefix of another**, refused at construction. Longest-match
  is a resolution rule; for authority it is a silent-precedence rule, and silent
  precedence over authority is how a grant ends up broader than anyone read it as.
- **Per-binding granularity comes free**: a host writes several entries, so
  `urn:iki:ledger:append` and `urn:iki:ledger:list` can hold different scopes. That
  answers "per-binding or per-endpoint" — neither; **per resolved target, declared by
  the host, at whatever granularity the host chooses.**
- **The table is a resource.** `urn:kernel:grants` alongside `urn:kernel:catalog` makes
  the delegation graph machine-legible and auditable, which matters more here than
  anywhere else in the kernel, and gives the not-yet-existing host doctor something to
  check.

### What the endpoint calls, and what happens when there is no grant

```rust
inv.issue_granted(request).await?   // takes NO capability; uses the bound one
```

It takes no capability argument — see the load-bearing fact. If the binding carries no
grant it **errors**; it must not fall back to the caller's capability. A silent fallback
turns "the host forgot to wire the grant" into "the call succeeded under a different
authority than the code intended", which is precisely the branch that looks like
success. `inv.grant()` returning `Option<&Capability>` lets a module choose deliberately.

### Composition: replacement, never union

A granted endpoint A issues a granted sub-request that resolves to granted endpoint B.

> **At every hop, exactly one authority is in effect, and switching authority is a
> REPLACEMENT.** `g_A ∪ caller` does not exist. `g_A ∪ g_B` does not exist.

A privileged sub-request from A runs under exactly `g_A`; when it reaches B, B's own
privileged sub-requests run under exactly `g_B`. Nothing accumulates down a chain, so
there is no depth at which a composition holds more than any single participant was
granted. A module whose grant is missing a scope gets a refusal, not a fallback to its
caller's.

### The capability floor stays the CALLER's

The kernel's declared-requires floor is evaluated against the authority the request
arrived with, unchanged. A grant must never raise the floor: that would silently make an
endpoint *reachable* by callers the host never authorized, decoupling "who may call this"
from what the description declares and breaking `declared = enforced` in the direction
the manifold cannot see. The division the brief states is the right one, and it is
exactly this: **the caller's capability decides whether the action happens; the module's
decides what the effect may be.**

### Can a module's authority exceed the host's own? And across the wire?

In-process there is no "host's own capability" to exceed — the kernel holds no ambient
authority and root arrives per request from whoever calls `Kernel::issue`. The grant is
minted by the code that constructs the kernel, and nothing in the process is above that
code. The honest statement is not "it is clamped" but **"it is the ceiling, and the host
binary is the only thing that can write it."**

Across the wire the answer is better than expected, and it already exists. A privileged
sub-request whose target is a mounted remote crosses as `Call::IssueAs(request,
capability)`, and the QUIC server clamps it: `session.capability.clamp(&carried)`, where
the session capability is minted per connection from the peer's certificate fingerprint.
So **a grant cannot escape the machine that minted it** — a remote honors it only to the
extent the remote already trusts this peer. (IPC deliberately does not clamp, documented
as "the peer is the owner, peercred-verified, so resolving under the carried capability
*is* the clamp." That reasoning holds for delegation too, and stops holding the moment
IPC grows a non-root principal.)

⚠ One caveat carried over from `cache-ejection.md` §2: `clamp` intersects **scope
strings**, and a scope string is frequently host-relative. Two hosts can hold
identically-spelled tokens that mean different things, so the clamp is sound about
*names* and only as sound about *authority* as the ecosystem's token grammar is global.

---

## The confused deputy: what core can enforce, and what it can only document

If a module holds store-write authority and passes a caller-supplied value into a
privileged sub-request unexamined, the caller has obtained the module's authority
without holding it. Splitting that into two dimensions makes the answer honest:

**Where the delegated authority reaches — core CAN bound this.** A grant entry can name
the IRI space its privileged sub-requests may target, checked by the kernel before
dispatch and independent of how the module built the request. That is defense in depth
rather than a new guarantee (the scope tokens already bound the effect *if* the
downstream door enforces them), and its real value is that the delegation graph becomes
readable: "the ledger may write store graph X, and may address nothing outside
`urn:iki:store:`."

**What flows into it — core CANNOT check this, and must not pretend to.** Core cannot
see that a caller's string became a SPARQL fragment. Taint-tracking strings through
arbitrary Rust is not a thing this kernel will do. It is a documented contract:

> **A privileged sub-request carries typed arguments, never interpolated caller
> strings.**

The ecosystem already has the reference implementation and it is worth naming as the
standard: `ikigai-store`'s `bindings=` *escapes nothing* — it parses JSON into
`oxigraph::model::Term`s and hands them to `substitute_variable`, so the query's syntax
tree is fixed before any caller value is in sight. Injection is unrepresentable rather
than handled. ⚠ And the asymmetry matters for exactly our case: **updates cannot bind**
(oxigraph's prepared update exposes no substitution), so the write side — the ledger's
side — has no typed-argument door at all and composes its SPARQL with helper
constructors instead.

**The third dimension, which is the one that actually saves the motivating case.**
`ikigai-store`'s scoped write door does not check syntax; it checks **effects**. It
copies graph G into a private store, runs the caller's update there, refuses if anything
landed outside G, and only then applies the diff. A ledger granted
`write:graph:X` therefore cannot write outside X *even if it is fully confused*, because
the refusal is on the far side of the grant. From that falls the adoption rule:

> **Delegate only to a door that enforces its own effect boundary.** The grant token is a
> declaration; the downstream door is the enforcement. A grant aimed at a door that
> merely *declares* a scope is a confused deputy waiting for its first caller.

And its corollary, which is the one-line rule a host needs: **never grant a
general-purpose evaluator.** A granted `urn:lisp:eval`, `urn:*:sparql`, `compose`, or
any endpoint whose contract is "run what I am given" hands its grant to every caller by
construction. Core cannot detect this; a host doctor reading `urn:kernel:grants` could
at least surface it for a human.

---

## What the trace must record

`TraceEvent::capability` today means "the authority this invocation ran under". Under
delegation there are two authorities per hop, and **a trace that records only one of
them is a lie** — it would report the ledger's store write as having been performed by a
principal that holds store authority, with nothing saying on whose request.

- `TraceEvent::capability` keeps its meaning: the **effective** authority — the grant, on
  a delegated hop. That is the authority that determined what the effect could be, and
  it is what an auditor asking "how was this write possible" needs to see.
- The **caller's** authority is recorded beside it, present only when it differs. Adding
  a typed field is the clean shape and is a **wire-format change** — `TraceEvent`'s
  `notes` field carries that warning already: "adding this field changes the postcard
  layout — a host shipping `TraceEvent`s over the wire must bump its protocol version."
  The cheaper shape is a reserved note key alongside `DENIED_NOTE` and `ALIAS_NOTE`,
  which are already kernel-attached notes in that same vector, one note per scope so it
  mirrors how a consumer already renders `capability`.
- A denial on a delegated hop is a **third** fact, distinct from both existing ones:
  the *grant* was insufficient. `DENIED_NOTE` carries the missing scope and
  `capability` carries what was held; a delegated denial needs the reader to be able to
  tell that the authority in question was not the caller's.

★ **It is not `log:onBehalfOf`, and the resemblance is a trap.** `ikigai-log` 0.1.1
solved the same *shape* one layer up — `log:onBehalfOf` plus a skolemized
`prov:Delegation` with `prov:agent`/`prov:hadActivity`, deliberately **not** a
`rdfs:subPropertyOf prov:wasAssociatedWith`, so a PROV reader cannot collapse the tenant
into the process. Borrow the discipline, not the term: `log:onBehalfOf`'s object is an
**agent IRI** (the tenant the host is serving, supplied by the host — the kernel does
not know it and the log's own docs say the kernel is never asked). The fact here is a
**capability scope set**, and the axis is authority, not agency. A third column,
rendered as its own term. Collapsing them would make "served for Alice" and "performed
under the ledger's grant" indistinguishable, which is the exact confusion that note
exists to prevent.

Consumer note: `ikigai-log`'s `LogTracer` maps `TraceEvent::capability` to one `cap=`
column per scope and maps note keys straight through its vocabulary table ("the kernel's
note key IS the log's column"). So a new kernel note key needs one vocabulary term in
`ikigai-log`, and nothing else.

⚠ The ecosystem vocabulary has no home for any of this yet. `ikigai-vocab` has exactly
two capability terms — `ik:requires` (what a resource demands) and `ik:cap` (what a route
grants) — and no term for *who holds authority*, *who delegated to whom*, or *under
which authority an invocation ran*. The runtime produces all three facts today and only
`ikigai-log`'s own namespace names any of them.

---

## What delegation does to the cache

The representation cache keys on `(request id, capability_key(capability))`, where
`capability_key` is BLAKE3 over the sorted scope set with root in its own namespace. Four
findings, in descending order of how quietly they would go wrong:

1. **★ The grant is not in the key, and it must be.** The outer result — the ledger's
   response — is cached under the *caller's* fingerprint, but its content was computed
   under the grant. Change the grant at the wiring and every entry computed under the old
   one still looks valid. This is `cache-ejection.md` §3 within one process: the key does
   not identify the code, and a grant is part of what produced the entry. It is also
   `ikigai-core-PENDING.md` §14 finding 7 (a rebind leaves the catalog stale forever),
   and one fix serves all three: **a golden thread the kernel cuts when a binding
   changes.** Failing that, the grant's fingerprint mixes into the key's capability half.
   Either way, do not ship delegation with the key untouched.
2. **The privileged sub-request's own entry is keyed under the grant**, which is constant
   across callers. That is correct — the entry is keyed to the authority that computed it,
   which is the rule — and it is the performance win: two callers with different
   capabilities share the module's internal reads. No leak follows, because the outer
   entry still separates them by the caller's fingerprint.
3. **Golden threads cross the authority boundary, and threads are visible.** A caller's
   composite inherits the threads of resources read under the grant, and
   `Representation::threads()` is public and travels the wire as validity tokens. So a
   narrow caller can learn that `urn:iki:store:graph:acme` exists as a dependency of
   something they may read. This is a leak of **names, not content**, and it cannot be
   fixed by substitution without breaking invalidation — a composite that does not carry
   its real dependencies is a composite that never recomputes. Accept it; state it.
4. **Expiry propagation is unaffected** and must stay that way: dependency recording is
   independent of which authority resolved the dependency.

---

## Verdict: not yet

Part A ships. **Part B should not be built until the conditions below are met**, and the
reason is not caution in general — it is three specific things, one of which is a defect
in the motivating case itself.

### 1. ★ The motivating case needs a *request-derived* grant

The ledger's graph is a function of the request: `urn:iki:ledger:acme:item:3` needs
`urn:cap:store:write:graph:urn:iki:ledger:graph:**acme**`. A static grant table cannot
express that. The three ways out:

- **A prefix/family grant** (`…:graph:urn:iki:ledger:graph:*` held rather than declared).
  `Capability::allows` is exact-set-membership; the trailing-`*` family form lives in
  `cap_satisfies` and is for **declarations**, not grants. Making a *held* scope match by
  prefix changes what every existing grant means, ecosystem-wide. This is the cousin of
  `ikigai-core-PENDING.md` §42 (no infix wildcard) and it is not a delegation change.
- **A templated grant** expanded against the resolution's bindings
  (`…:graph:{ledger}`). Elegant, uses machinery that exists — and puts **caller-controlled
  text into a capability token**. A capture containing a separator forges a different
  token. That is the "four name projections, four contracts" trap (escape ≠ valid; you
  must parse the output, not filter the input) sited in the one place the system cannot
  afford it. Buildable safely, but it is its own arc with its own adversarial tests, and
  it should not ride in as an implementation detail of this one.
- **Static enumeration plus Part A**: the host names each ledger's grants explicitly, the
  module narrows to the one it needs per request with `issue_attenuated`. Safe, honest,
  composes the two halves cleanly — and it means creating a ledger requires editing host
  wiring. That is a real operational cost and arguably a feature (a host that has not
  enumerated a resource has not authorized it), but it should be chosen on purpose and
  with the consumer in the room.

**Until one of those three is settled, delegation serves statically-enumerated resources
only** — and that is not yet the case that asked for it.

### 2. The premise has weakened, and the remaining harm is narrower than stated

The brief's escalation — "any local session permitted to file an item holds `DROP ALL`
over the dataset" — was true against the store's old broad token. `ikigai-store` 0.2.2
ships per-graph read and write scopes with effect-level confinement, and
`urn:iki:store:graph-update` **refuses** the broad `urn:cap:store:write` by design. On the
write side the escalation is already gone; what remains is abstraction leakage (the
ledger's callers must know it is built on the store, and grants are issued in pairs).

The read side is the part still worth fixing: as of its current main, `ikigai-ledger`
declares the broad `urn:cap:store:read` on its read actions because one enumeration path
reads the whole dataset. That is a genuine over-requirement and it is a live one — but
it is *one path*, and a cheaper local fix may exist (that arc owns the question; this one
should not design around work mid-flight). ⚠ Both claims are from reading the ledger's
and store's current mains, not from running them; whoever picks this up should confirm
against `ikigai-cli`'s actual grant wiring, which is where the deployment refusal
happened.

### 3. Nobody can adopt it this week, and the surface is not small

`ikigai-store`, `ikigai-ledger` and `ikigai-cli` are all held by other satellites and all
three are downstream. The change touches `Kernel` construction, `Invocation`, the cache
key, the trace format (and therefore a wire protocol version), the `ikigai-module` shim
contract, and `scope_sync`/`fan_out` — each of which has a silent-wrong-answer failure
mode. In a lockstep workspace at 0.1.x with ~31 dependents, shipping that with no
consumer able to prove it means the first real use is also the first test.

### What would change the answer

1. A decision on request-derived grants: prefix-grant semantics, validated template
   expansion, or static enumeration. **Any** of the three unblocks; the cost is choosing
   deliberately rather than discovering the choice inside an implementation.
2. A binding-change golden thread (or the grant folded into the cache key), so the cache
   cannot serve entries computed under a grant that is gone.
3. One consumer free to adopt it in the same cycle and prove it end to end — with the
   adversarial test, not the happy path: a granted endpoint handed a hostile argument,
   refused by the far-side door.
4. Evidence that Part A plus narrow per-graph tokens is *not* enough for a second,
   independent consumer. One case for which a local fix exists is a local fix; two are a
   kernel feature.

An honest "not yet" costs the ledger a doc paragraph explaining why its callers need two
grants. Getting delegation wrong costs 31 crates a mechanism they cannot remove.

---

## Release shape

Part A is additive public API on `Invocation` — a **minor** bump. It carries no format,
wire, or cache change, and it can and should ship ahead of any decision about Part B;
nothing in Part B's design would make `issue_attenuated` wrong, and Part B needs it (see
"static enumeration plus Part A" above).
