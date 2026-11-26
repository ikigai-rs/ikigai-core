# Monotone execution budgets: set by a host, only narrowed

**Status:** design only (ledger #4322). Nothing in the kernel changes. It answers the last
follow-up of [overlay-request.md](overlay-request.md) (ledger #1212). The claims about today's
kernel are pinned by `crates/ikigai-core/tests/execution_budget.rs` and by the existing
`retry_budget.rs`, `byte_budget.rs` and `negative_caching.rs`. Source reviewed on 2026-11-24
at core `41d76f6`.

## The question

An **enforcing execution budget** bounds how much work a resolution tree may do: time per
evaluation, re-issues, bytes buffered, or later fuel. It is separate from a **descriptive
label** ([provenance-labels.md](provenance-labels.md), [label-lattice.md](label-lattice.md)).
A label says what something is and travels by join. A budget says how much may still be done
and travels only by narrowing. This note answers these questions:

- Who may set a budget?
- How does it narrow down a call chain?
- Is it shared or per branch under fan-out?
- How long does it live?
- How does it cross a remote call?
- What happens on a cache hit?
- What happens when no budget is present?

It compares the answers with the run context the kernel already has.

## The downstream case

A downstream host bounds anonymous SPARQL evaluations to 1 second, below the store's 5-second
base ([ikigai-gonk `budget.rs`](https://github.com/ikigai-rs/ikigai-gonk/blob/cee4580e424c151d7bf616538829621db5825985/src/budget.rs)).
The store's own budget comes from a capability grant, `urn:cap:store:budget:<ms>`. A grant is
monotone: it can only raise the budget, so "anonymous gets less" cannot be written as a grant.
The host uses three workarounds:

1. **A door marker** in the capability, `urn:iki:gonk:door:anonymous-budget:<ms>`, read by a
   stamp at the hub's front door. The stamp **overwrites** the request's `budget=` argument.
2. **A task-local**, `RUN_BUDGET`, set around a script run. A script's query is a sub-request
   under an **attenuated** capability, and attenuation drops the marker
   ([capability.rs:387](../../crates/ikigai-core/src/capability.rs#L387)). The host's comment
   says the task-local works only because "the hub is not a scheduled kernel, so nothing here
   fans out onto other tasks".
3. **An overlay re-issue** that stamps `budget=` and issues the request again. This costs one
   depth and reruns resolution
   ([sparql.rs:771](https://github.com/ikigai-rs/ikigai-gonk/blob/cee4580e424c151d7bf616538829621db5825985/src/sparql.rs#L771)).

`attenuation_drops_a_door_marker_and_keeps_the_run_context` reproduces the cause with the
public API. A caller holds `urn:cap:read` and a marker, and issues under a run deadline. An
endpoint reads a leaf twice: once under its own capability, then through
`issue_attenuated(…, ["urn:cap:read"])`. The leaf sees the marker only the first time. It sees
the deadline both times. The carrier this case needs already exists. It is the run context,
not the capability.

## What the run context does today

[`IssueContext`](../../crates/ikigai-core/src/run.rs#L663) is what a run carries from the door
that issued it down every sub-request, beyond the request and its capability. It holds three
budgets:

| | Retries (#1546) | Deadline (#1546) | Bytes buffered (#2238) |
|---|---|---|---|
| Shape | shared pool | instant, kept by `min` | shared pool |
| Set by | host: `IssueContext::with_retry_limit`, issued with `Kernel::issue_with_context` | host: `with_not_after` | host: `with_memory_limit` |
| Ceiling | capability grant `urn:cap:retry:<n>`, clamped to the host's `with_retry_budget(base, ceiling)` | none | grant `urn:cap:stream:memory:<n>`, clamped to `MemoryLimits` |
| Endpoint can | narrow: `Invocation::with_retries` | narrow: `Invocation::with_not_after` | narrow: `Invocation::with_memory_limit` |
| Fan-out | one pool for every branch | same deadline | one pool |
| Cache hit | not spent | not consulted | spent only for the bytes the hit itself buffers |
| Missing | no pool; overlays keep their own `attempts` | no deadline | minted at the host's default (64 MiB) |
| Remote | wire v9 `IssueWith`: what is left, as the server's pool limit | wire v9 `IssueWith`: time left in ms | not carried |

This table describes the code:

- Every sub-request form inherits the context, including attenuated sub-requests, confinement
  and a spawned fan-out branch. Each branch clones the same pools
  ([endpoint.rs:2296](../../crates/ikigai-core/src/endpoint.rs#L2296)).
- At admission the kernel narrows each pool to what the request's capability is entitled to
  ([`admitted`](../../crates/ikigai-core/src/run.rs#L787)). Admitting twice changes nothing.
- A narrowed pool is a **child** ([`Pool`](../../crates/ikigai-core/src/run.rs#L195)). It has
  its own limit and also draws from every pool above it, so a spend anywhere is a spend in every
  pool above it.
- The context is **not in the cache key**, and it decides only whether an answer is produced,
  never what the answer is (ledger #3869,
  [cache.rs:105](../../crates/ikigai-core/src/cache.rs#L105)). The kernel never stores a refusal
  the context decided (`OverBudget`, `Timeout`, `GaveUp`), even when the endpoint states a
  failure expiry ([kernel.rs:3348](../../crates/ikigai-core/src/kernel.rs#L3348)).

The new tests pin the remaining claims:

- `an_endpoint_narrows_the_run_and_cannot_widen_it`: an endpoint that asks for 50 retries
  and a later deadline gets what the run has left.
- `a_cache_hit_is_served_whatever_the_run_context_and_spends_nothing`: a hit inside one run
  leaves the pool untouched. A second run under a tighter deadline is served the first run's
  answer, and no endpoint runs under the tighter deadline.
- `a_missing_context_is_open_for_time_and_retries_and_bounded_for_bytes`: a request issued
  with no context has no deadline and no retry pool, but it does have a byte pool.

The `IssueContext` doc already reserves room for more budgets: "later halves (a fuel budget,
ledger #1889) are added here without a new seam".

## Recommendation: extend the run context; add no new carrier

An enforcing budget is a new **kind of entry in `IssueContext`**. It is not a capability
marker, a request argument, a task-local, or a generic context map:

- **Not a capability marker.** Attenuation drops a marker that is not asked for, and a grant
  can only raise a budget. Both rules are right for authority and wrong for a budget.
- **Not a request argument.** An argument is half the cache key and crosses a wire as data a
  peer can forge. An overlay can rewrite it, which is why the downstream stamp must overwrite
  rather than add. Arguments are also not inherited down a chain
  ([resolution-scope.md §6](resolution-scope.md#6-values-are-not-corridors-here)).
- **Not a task-local.** It does not cross a scheduled kernel's spawn. It does not cross a
  remote call. It is not part of core's contract.
- **Not an open map.** A writable map lets endpoint code erase a budget or enlarge it. A
  descriptive label may live in an open vocabulary; an enforcing budget may not. Each budget
  needs its own narrowing operation.

### Shape

A budget is named by a **dimension** IRI that its consumer owns, for example
`urn:budget:store:evaluation-ms`. A dimension has one of two parts, chosen by its consumer:

- **A cap**: a bound on each unit of work, kept by `min` like `not_after`. The downstream case
  is a cap: *each* evaluation may run at most 1000 ms.
- **A pool**: a shared count with the existing child-pool semantics, like retries and bytes.
  A pool fits a total, such as "at most 10 evaluations per run" or fuel.

The consumer computes the bound it enforces as the least of three values:

- its own policy for the capability (for the store, the `urn:cap:store:budget:<ms>` grant
  under its ceiling);
- any narrowing argument the request carries (for the store, `budget=`);
- the run's cap for the dimension.

A grant still raises the policy, and the run context still only lowers it.

The following API sketch states needs, not a committed API:

```rust
// host (a door): set once per run
let run = IssueContext::new().with_budget_cap(STORE_EVALUATION_MS, 1_000);
kernel.issue_with_context(request, &capability, run).await?;

// consumer (the store's endpoint): read, never write
let cap = inv.budget_cap(STORE_EVALUATION_MS); // Option<u64>
let pool = inv.budget_pool(STORE_EVALUATIONS);  // Option<&BudgetPool>, same Pool type

// any endpoint: narrow for what it issues, as with_retries does
inv.with_budget_cap(STORE_EVALUATION_MS, 200).issue(sub).await?;
```

### Who may set it

**Only the host.** A host mints a run's budgets on the `IssueContext` it passes to
`Kernel::issue_with_context`, the call with which retries, deadlines and bytes are set today.
An endpoint cannot mint one:

- `Invocation` has no way to issue under a fresh context.
- Every sub-request form threads `self.run` through, as it threads the capability through the
  private `issue_under`.
- The only budget operations an endpoint has are narrowing reborrows.

A host can also register a **ceiling policy** for a dimension, `BudgetLimits { base,
ceiling }`, like `MemoryLimits`. The policy optionally names a grant family that raises the
base up to the ceiling. Admission then narrows the run's budget to what each request's
capability is entitled to, as it does for bytes. The policy is optional. The downstream case
needs only the cap the door sets, because the store's own grant already provides the ceiling.

A budget **grants nothing**. It never satisfies a capability floor, and it never widens a
scope. When a budget is missing or very large, an endpoint the capability does not reach is
still out of reach.

### Monotone narrowing down the chain

These rules hold for every operation:

- `with_budget_cap` keeps the smaller value.
- `with_budget_pool` makes a child pool when the new limit is below what is left, and
  otherwise returns the same pool.
- Admission is idempotent.

So, along any path from the root, a cap never increases, and a pool's remaining count never
exceeds that of any pool above it. This is the invariant `run.rs` already states for its
budgets: "a peer that carries a context across a wire can shorten its own budget, never
lengthen it".

Re-issue and confinement do not reset a budget. A detached invocation has no issuer and no run
context. Its consumer applies its own base, as `Invocation::collect` falls back to
`DEFAULT_STREAM_MEMORY`. Because the base is the consumer's own policy, a detached call
cannot get more than the base.

### Fan-out: shared by default, per branch by narrowing

- **A cap** is per unit of work. Fan-out does not change it.
- **A pool** is **shared** across branches by default, as the retry pool is today
  (`fan_out_branches_share_the_pool_and_the_deadline` in `retry_budget.rs`). k branches
  therefore cannot multiply a total.
- **Per-branch** accounting is a narrowing the parent applies to each branch. It issues each
  branch through `with_budget_pool(dim, share)`. Each branch then has its own limit and still
  draws from the shared pool, so the sum of what the branches spend never exceeds the parent's
  pool, whatever the shares add up to.

Core gives no "split evenly" primitive. How the parent divides the pool is its own policy.

### Lifetime

A budget lives for **one root issue**. It is minted when the host calls
`issue_with_context`, and it lives as long as any invocation of that tree holds a clone. If a
spawned branch outlives the root's answer, it still spends from the same pool. This is
harmless, because nobody can refill that pool.

The context is not persisted and not cached. Each root issue starts from what the host
passes. A host that wants one budget across several roots can pass clones of one
`IssueContext`: clones share their pools, as today (for example, a per-session budget). This
should be documented, because it is the only way two roots can share a budget.

A pool's dimension declares whether a spend is **consumed** (fuel, evaluations, retries) or
**reserved** and handed back (bytes, through `Reservation`). `Pool::give_back` already
supports the second kind.

### Across a remote call

Core defines no wire form for `IssueContext`, but the wire crate already carries part of it.
A mounted remote receives the request and the capability, and the server clamps the capability
to the session's
([sub-request-authority.md](sub-request-authority.md#can-a-modules-authority-exceed-the-hosts-own-and-across-the-wire)).
Since wire v9, a client whose run has a retry pool or a deadline sends `Call::IssueWith`
instead of `Call::IssueAs`
([ikigai-wire](https://github.com/ikigai-rs/ikigai-cli/blob/main/crates/ikigai-wire/src/lib.rs),
sent by ikigai-ipc and ikigai-quic). Its `IssueWith` carries two budgets:

- `retries`: what is left of the caller's retry pool. The server mints a pool with that limit,
  and admission narrows it to the clamped capability's entitlement.
- `time_left_ms`: the deadline as time left in whole milliseconds, which the server adds to its
  own `now`.

A remote call therefore keeps the retry pool's remaining count and the deadline. It drops the
byte pool, and it would drop any new budget dimension. The proposal extends the same rule as
the capability clamp:

- **Caps cross as values**, and the server admits each one as `min(carried, session policy)`.
  A carried cap can only lower what the server would give this peer, so an untrusted carried
  value is safe to honor.
- **Pools do not cross as live counters.** Before the call, the caller **reserves** an amount
  from its pool (at most what is left) and sends that amount as a fresh pool limit. The server
  mints a local pool of `min(sent, session policy)`. A reply may report what was unused, and
  the caller gives that back. Without such a report, the reservation is spent. This keeps the
  caller's total sound without trusting the peer's accounting.
- **The deadline crosses as time left**, not as an instant, because the two kernels' clocks
  differ. The server applies it to its own clock. `IssueWith` already does this.
- **A missing or malformed carried budget** is treated as missing, and the server applies its
  session policy for that peer. A missing budget never means unlimited.

This belongs to the wire crate, not core. Core's part is to make these values readable and
mintable at a server's admission.

### On a cache hit: observe, and spend only what the hit itself costs

An execution budget bounds **work**. A hit does no evaluation, so it does not spend a cap or an
evaluation pool, and a cap does not forbid a hit. This is today's behavior for every budget
(`a_cache_hit_is_served_whatever_the_run_context_and_spends_nothing`). The byte pool shows
the one exception, which proves the rule: a hit that collects a stored `Replay` into memory
buffers real bytes, so it draws from the byte pool
([kernel.rs:3267](../../crates/ikigai-core/src/kernel.rs#L3267)). The rule is: **a hit spends
what serving it costs, never what producing it cost.**

Charging a hit the original cost was rejected for two reasons:

- It would make the cache useless for exactly the callers a budget protects.
- It would turn the stored cost into an observable signal.

To limit how often a caller may ask, the host applies a rate limit at its door before it
issues, as the downstream host already counts anonymous runs. That is not an execution
budget, and core has no rate limiter.

This is sound only under the invariant the kernel already holds for the run context: **a
budget may refuse, never change an answer.** A consumer that runs out answers
`OverBudget` (or `Timeout`). It never returns a truncated or partial success. The kernel
never stores those refusals (ledger #3869). So a success produced under a large budget is the
same success a tight budget would produce, and serving it to a tight run is correct. A
dimension whose consumer would change its answer, for example by returning the first n rows,
is not an execution budget. It is an argument and belongs in the key.

This also settles the cache question in [overlay-request.md](overlay-request.md#request-amendment)
for the downstream case. Today the stamped `budget=` is an argument, so two budgets give two
keys, and a warm entry under the outer request skips the overlay. With the budget in the run
context, the store reads its cap without amending the request. The key stays one key, and a
hit is served under either budget, as it should be: the work was already done.

### When the budget is missing

Each dimension declares a missing policy. The proposal allows two, and neither means
unlimited:

- **Consumer base** (the default): no cap and no pool in the context means the consumer's own
  policy for the capability. This matches today's store: no door, no stamp, 5 s.
- **Host base**: when the host registered `BudgetLimits`, admission mints the dimension at the
  capability's entitlement, as the byte pool is minted, so every request the kernel runs has
  one.

The retry pool keeps its present opt-in behavior (decision 5 of the retry-authority design:
with nothing configured, nothing changes). It is the only budget whose absence means "each
overlay decides", and it stays that way because an overlay's own `attempts` is itself a bound.

## What it replaces downstream

With a `with_budget_cap` on the run context, the downstream host can drop the task-local,
the marker-driven stamp and the re-issue:

- **Before**: the door adds a marker. The hub's front door stamps `budget=` from it. A script
  run puts `RUN_BUDGET` in a task-local, because the attenuated sub-request loses the marker.
  The overlay stamps and re-issues, costing one depth.
- **After**: the door issues an anonymous run with
  `IssueContext::new().with_budget_cap(STORE_EVALUATION_MS, door_ms)`. The store reads
  `min(grant budget, budget=, cap)`. The cap reaches every evaluation in the tree, whether it
  is attenuated, confined, re-issued, or spawned on another task of a scheduled kernel. There
  is no stamp to bypass with a by-reference `budget=`, and no extra depth.

There is one behavior to keep on purpose. Today, gonk's own ledger queries under an anonymous
**page** run at the store's base, because only runs are scoped. A cap set on the page's whole
issue would bound those queries too, because nothing below can widen it. So the door sets the
cap only on the requests whose whole tree is the caller's work: script runs, `urn:plan:eval`,
the SPARQL check, and the SPARQL protocol face. It sets no cap on page renders. This is the
same partition the task-local draws today, now made at the door.

## Proposed follow-up items

1. **core: budget dimensions in the run context.** Add caps and pools keyed by dimension IRI to
   `IssueContext`, built with narrowing-only builders. Add `Invocation::budget_cap`,
   `budget_pool` and a narrowing reborrow. Add an optional `BudgetLimits` per dimension at the
   kernel, admitted like `MemoryLimits`. Keep the cache-key and stored-refusal rules of #3869
   for every dimension. Turn the tests in `execution_budget.rs` into tests of the new API.
2. **core: document shared pools across roots.** Say on `IssueContext` that a clone shares its
   pools, and that this is how a host spans several roots with one budget.
3. **wire: carry the rest of the run context in `IssueWith`.** It already carries the retry
   pool's remaining count and the deadline as time left. Add the byte pool and the new budget
   dimensions: caps as values, pools as reserved amounts. Clamp each to the session policy at
   the server. Treat a missing or malformed budget as missing.
4. **store and sparql: read the evaluation-time cap from the run context.** Bound each
   evaluation by `min(grant budget, budget=, cap)`, and refuse with `OverBudget` or `Timeout`,
   never with a truncated answer.
5. **gonk: replace `RUN_BUDGET` and the marker stamp.** Issue anonymous runs under a door-set
   cap, delete the task-local and the re-issuing overlay, and keep page renders uncapped.
