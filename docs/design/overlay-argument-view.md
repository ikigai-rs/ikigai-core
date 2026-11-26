# Overlay argument views

**Status:** design, 2026-11-24. Ledger #4319, following the research note
[overlay-request.md](overlay-request.md) (#1212). Nothing in the kernel changes with this
note, and it adds no public API. Reproductions of today's behavior live in a test,
`crates/ikigai-core/tests/overlay_view.rs`, built on the public API only. The API named
below is a proposal for a follow-up item, not something that exists.

## The question

An **overlay** is an endpoint that wraps an inner endpoint and changes the arguments the
inner endpoint sees, for example by stamping a run budget a door set. What should the
overlay hand the inner endpoint, so that:

- the inner endpoint's sub-requests are dependencies of the outer invocation;
- the call costs no depth and does not resolve, authorize or look up the cache again;
- the request the kernel dispatched stays the one it traced and keyed?

## Today, reproduced

Each claim in the research note holds. The test pins them:

| Test | What it shows |
|---|---|
| `a_detached_inner_invocation_cannot_issue_its_dependency` | `Invocation::detached` over the amended request has no issuer. The inner endpoint's `source` fails with `sub-requests require a kernel context`, so a detached route cannot compose an inner endpoint that reads anything. |
| `reissue_records_dependencies_and_keys_the_amended_request_separately` | Re-issuing the stamped request through `inv.issue` records the dependency: a cut of the data thread drops the outer answer. It also leaves a second cache entry, keyed on the amended request. |
| `reissue_costs_one_depth` | Under `with_max_depth(1)` the inner endpoint called directly may read its data at depth 1. Behind a re-issuing overlay, it runs at depth 1 and its read at depth 2 is refused with `DepthExceeded { depth: 2 }`. |
| `an_outer_cache_hit_skips_the_overlay` | After the overlay's budget changes, the same request is served the answer computed under the old budget, and the overlay does not run. |
| `the_kernels_own_request_reborrow_shares_the_record_at_no_depth` | The kernel's private `with_request`, used to settle a by-reference argument, hands the endpoint a changed request that shares the outer record. A cut of the referent drops the answer, and the endpoint runs at the outer depth. |

The last row is the shape this note specifies: the private seam already behaves as an
overlay view should. It cannot be exposed as it is, for the reasons the decisions below give:
it accepts any `Request`, so verb and target could change, it validates nothing, it shares
the stale-revalidation offer, and the trace never shows the change.

## Decisions

### 1. A view, not a copy and not a dispatch

The proposal is a borrowed `Invocation` over an **effective request**. It is built the way
`with_bindings` and `with_request` are built: the same issuer, spawner, clocks, span, trace
scope, resolution scope, depth, run context (retries, deadline, byte pool) and the same
`Arc<Recorded>`. It borrows the outer invocation, so it cannot outlive it.

A sketch, for the follow-up to refine:

```rust,ignore
// Build the amendment from the invocation; verb and target come from it and have no setter.
let amended = inv
    .amend_arguments()
    .set("budget", ArgRef::Inline(b"10".to_vec()))
    .remove("hint")
    .check_against(&*self.inner)?; // Result<AmendedArguments>

// A view over it, sharing everything the kernel set.
let view = inv.with_arguments(&amended)?; // Result<Invocation<'_>>
self.inner.invoke(&view).await            // or invoke_body, the same view
```

`AmendedArguments` owns the effective `Request` and records the `RequestId` of the
request it was built from, plus the names it changed. `with_arguments` refuses an
amendment built from another invocation: the recorded id must equal `inv.request.id()`.
This keeps the verb and target fixed by construction rather than by convention.

The private `with_request` stays private and unchanged. The public view is a separate
constructor with the checks below. Reference settlement can move onto the same internal
path later, without changing what it does.

### 2. Which replacements are allowed: by-value arguments only

| Change | Allowed | Why |
|---|---|---|
| Verb | No | The endpoint, its declaration, the capability floor (per verb) and the declared defaults were all chosen for the original verb. |
| Target | No | Resolution, the grammar's bindings and the resolution-chain fingerprint were all chosen for the original target. |
| Set or replace an argument with an inline or content-addressed value | Yes | This is the case overlays need. |
| Remove an argument | Yes | Removing an argument is a replacement with absence. A declared default is not injected again: defaults were injected before the original request's identity was taken. A required input that is removed fails the contract check (decision 3). |
| Set an argument to a reference (`ArgRef::Reference`) | No | Reference settlement already ran for the original request (ledger #2511). A new reference would reach an inner endpoint that expects settled bytes, unread and unrecorded. An overlay that needs a referent reads it itself with `inv.source` (a recorded dependency) and passes the bytes inline. |
| Change `idempotency-key` (`IDEMPOTENCY_KEY_ARG`) | No | An unsafe resend is matched by `fingerprint_without` the key. Changing the key under the same arguments, or the arguments under the same key, breaks that match: a resend either looks like a conflict or replays an answer for different arguments. |
| Change an input the inner contract marks as pinned | No | See decision 4. |

### 3. Replacements are checked against the inner endpoint's contract

The kernel admitted the original request against the **overlay's** description: the
overlay is the endpoint that resolved. The inner endpoint's contract was never consulted.
So `check_against(inner)` runs the inner endpoint's action specification for the fixed verb
over the effective arguments. It uses the same `ActionSpec::check` that `urn:kernel:validate`
runs, plus the inner endpoint's `requires` for that verb against the invocation's
capability.

- A violation is `Error::InvalidArgument` naming the argument and saying the overlay
  supplied it. For example: ``invalid argument `budget`: the overlay `query` supplied
  `ten`, and `inner` declares an integer``. Following [ACCESSIBILITY.md](../../ACCESSIBILITY.md),
  the message names the argument, says the kind of failure in words, and needs no color or
  markup. A violation is the overlay's defect, not the caller's, so it is never cached as a
  failure. It also states no failure expiry.
- An unmet `requires` is `Error::Denied`, naming the scope. The check can only refuse. It
  cannot grant, and it is not a second floor (decision 6).
- An inner endpoint with no description checks nothing, as an undescribed endpoint
  checks nothing today. The view still applies the structural rules of decision 2.

### 4. A security-sensitive argument is pinned, and values never reach the trace

The view cannot change authority: it carries the outer capability by construction, and
every sub-request it issues passes the floor under that capability. An argument can still be
security-sensitive without being authority. Examples are a key that identifies a replay, a
limit a door imposed, or a name the inner endpoint will read under the caller's capability.

- **Pinned inputs.** A proposed `ArgSpec::pinned()` marks an input that no overlay view may
  set, replace or remove. `check_against` refuses a change with `Error::Denied`, naming the
  argument, even when the new value is valid. `idempotency-key` behaves as pinned on every
  contract.
- **Narrowing only.** A limit an overlay may tighten but never loosen, such as a budget,
  is not an argument rule. It belongs to the run context (`with_retries` and `with_not_after`
  already narrow), and the follow-up on monotone execution budgets covers it. The view does
  not try to compare argument values.
- **No values in traces.** The trace records argument names, never values (decision 5).

### 5. Tracing shows the original and the effective request on one span

A view is not a dispatch, so it opens **no span**. The outer span's `target` stays the
original request's target. When the trace is recording, `with_arguments` adds notes to the
outer span, under public key constants like the existing denial key:

| Key | Value |
|---|---|
| `overlay` | the overlay endpoint's `name()` |
| `overlay.changed` | the names of the changed arguments, sorted and comma-separated, for example `budget, hint` |
| `overlay.effective` | the effective request's `RequestId`, so an observer can tell two effective requests apart without seeing their values |

The inner endpoint's sub-requests appear as children of the outer span. With nested views
(an overlay over an overlay), each view adds its own `overlay` and `overlay.changed` notes,
in order, and `overlay.effective` names the innermost request. Off the trace path, the view
records nothing and allocates nothing for tracing.

### 6. Why this is not a second authorized dispatch

Everything that makes a dispatch already happened once, for the original request, and the
view repeats none of it:

- **Resolution and bindings**: the inner endpoint is the overlay's own field, not a name
  resolved again.
- **Authorization**: the floor ran for the overlay's declaration under the caller's
  capability. The inner `requires` check in decision 3 can only refuse. It never admits
  anything the outer floor refused.
- **Cache**: no lookup and no insertion. Only the outer entry exists, keyed on the
  original request. The re-issue route's second entry, keyed on the amended request, does
  not exist.
- **Depth**: the view keeps the outer depth, so `DepthExceeded` counts what the inner
  endpoint issues, not the amendment.
- **Failure policy**: declared failure classes and failure caching apply to the outer
  endpoint's description and the outer key.
- **Trace**: no span, only notes (decision 5).

If the view counted as a dispatch, an overlay would show up as two invocations with two
keys and two floor decisions, while only one was keyed and only one can be invalidated.
The outer answer is the overlay's answer: whatever the inner endpoint read is a dependency
of that answer, through the shared record.

### 7. The stale-revalidation offer is hidden from a view

`Recorded::stale` holds the expired answer the kernel offers for revalidation. Every
reborrow shares it, because, as its comment says, "they serve the same request". A view
does not serve the same request. The offer's validator came from an inner run over effective
arguments that may differ from this one's. `inv.stale()` therefore answers `None` through
an argument view. The overlay itself, holding the outer invocation, can still revalidate
the composed answer.

### 8. What the view does not fix: an outer cache hit

`an_outer_cache_hit_skips_the_overlay` shows the limit. The outer lookup runs before any
endpoint, so no change to arguments made inside an overlay can apply to an answer already
cached. The view is cache-correct only when the amendment is a function of what the outer key
already holds: the original request, the capability fingerprint, and the resolution chain,
plus overlay configuration whose change cuts the entry. An amendment drawn from unkeyed
context, such as a task-local budget, needs one of three things: a partition decided before
lookup, a reviewed pre-lookup transformation, or a bypass of lookup and insertion. The
view's documentation must state this. It does not belong to the view itself: it is the
follow-up on cache-safe context-dependent overlays (overlay-request.md).

## Test plan for the implementation

The tests below go with the follow-up that adds the view. Each pairs the view with the
re-issue route where the difference is the point. The five tests in `overlay_view.rs`
remain as the baseline.

1. **Shared success dependencies.** An inner endpoint, behind a view, sources a resource
   hanging from a thread. The outer answer is cached under the original request only
   (`is_cached` true for the original, false for the effective). A cut of the thread drops it.
2. **Shared failure dependencies.** An inner endpoint's sub-request answers `NotFound`, and
   the inner endpoint falls back. The outer answer hangs from the failed name (ledger #611):
   creating it cuts the fallback. A failure with a stated expiry
   (`failure_expires_at`) is cached under the outer key until then.
3. **Buffered and streaming.** The same overlay through `invoke` and `invoke_body`
   (`kernel.issue` and `kernel.issue_body`) gives the same bytes and dependencies. A
   streamed inner body collected by the overlay counts against the outer run's byte pool:
   an inner body over the pool is `OverBudget` and is not cached.
4. **Depth.** Under `with_max_depth(1)`, a view's inner endpoint reads at depth 1 and
   succeeds, where the re-issue route fails at depth 2. The outer entry's recorded depth
   consumed equals the inner endpoint's reach.
5. **Tracing.** `issue_traced` shows one span for the overlay, with its `target` the
   original target and the three `overlay` notes. The `overlay.changed` note lists names
   only: a test with a distinctive value asserts it appears in no note. Inner
   sub-requests have the overlay span as their parent.
6. **Fixed verb and target.** An amendment built over one invocation and offered to
   another (a different request id) is refused. The builder has no setter for verb or
   target (a `compile_fail` doc test).
7. **Contract and pins.** A value the inner contract rejects is `InvalidArgument` naming the
   argument and the overlay, and is not cached. Removing a required input is refused the
   same way. A pinned input and `idempotency-key` are refused even with valid values. A
   `Reference` value is refused. An inner `requires` the capability lacks is `Denied`, and the
   inner endpoint never runs.
8. **No second dispatch.** The view leaves `cache_len` unchanged beyond the outer entry. It
   adds no trace span and no floor denial event.
9. **Stale offer.** When the outer entry expired with a validator, `stale()` is `Some` for
   the overlay and `None` through the view.
10. **Fan-out through a view.** On a scheduled kernel, `fan_out` from the view's inner endpoint
    records each branch's dependencies on the outer invocation. This covers work issued on
    another task, which the task-local workaround cannot reach.

## Proposed follow-up items

- **Add the overlay argument view.** `Invocation::amend_arguments`, `AmendedArguments`,
  `check_against` and `with_arguments` as specified in decisions 1 to 7, with the tests above.
  The new public API and trace-note keys are additive, so it ships in a patch release.
- **Add `ArgSpec::pinned()`** to the contract and the vocabulary (`ikigai:pinned` in
  `vocabulary.ttl` and `shapes.ttl`), with `idempotency-key` pinned implicitly.
  `urn:kernel:validate` reports a pinned change the same way.
- **Route reference settlement through the view's internal path**, so the kernel's own
  amendment is traced with the same notes. Its behavior otherwise stays as it is. While
  there, move the doc comment that sits above `with_request` in `endpoint.rs`, which
  describes `failure_dependencies`, back onto that function.
- The research note's remaining items stay open and unchanged: cache-safe
  context-dependent overlays, door context propagation, and monotone execution budgets.
  The view relies on the first for any amendment drawn from unkeyed context.
