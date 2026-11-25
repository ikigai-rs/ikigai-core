# Cache-safe context-dependent overlays

**Status:** design spike, not an API or behavior change. This decision applies when
an overlay's amendment, answer, refusal, or observable effects can depend on context
that is absent from the cache key. It builds on [overlay request views](overlay-request.md).

## Reproduction

Run `cargo test -p ikigai-core --test overlay_context_cache`. The test models an
overlay with a host-owned atomic budget outside the request. The overlay returns the
budget as its cacheable answer, standing in for an amendment that changes the answer;
it does not implement a request re-issue or model actual budget accounting. With the
same request, root capability, and default resolution scope, budget 10 yields `10`.
Changing the budget to 2 still yields `10`, and the call count remains one. Cutting
the target and issuing again yields `2` and a second call. Thus the warm outer hit
skips the overlay; the cut is a control showing that the changed context matters.
This is a deterministic sequential reproduction, not a concurrency or failure-cache test.

The current key is request identity, capability fingerprint, and resolution-chain
fingerprint ([cache.rs](../../crates/ikigai-core/src/cache.rs)); the lookup precedes
endpoint invocation ([kernel.rs](../../crates/ikigai-core/src/kernel.rs)). Recording
a dependency or marking a response volatile after a miss cannot prevent an earlier
hit. Re-issuing an amended request does not fix the outer hit either.

## Decision: bypass until an identity is proven

The safe default is to bypass **both lookup and insertion**, for successes and
failures, before any affected request can hit the cache. The host must identify the
context-sensitive region before lookup, including every cacheable ancestor whose
result or effects can depend on it. Propagate the bypass through the entire affected
subtree, including re-issues, scheduled work, and nested calls. A cached ancestor can
otherwise skip the subtree altogether. If the affected region cannot be determined
before lookup, bypass at the root. The policy must apply to in-flight coalescing or
any other reuse of computed answers as well. Failure to propagate the policy, or an
unknown boundary, must take the conservative bypass path. This is a proposed policy;
today's response volatility alone does not provide it.

This deliberately trades reuse for correctness. A later design may allow either:

- **A stable partition:** before lookup, derive an immutable, trusted partition from
  all context that can change the result, failure, or effects. Include its identity
  alongside the existing key at every affected ancestor and child. Equality must mean
  the composed computation is interchangeable, including enforcement and effects.
  A budget *class* or immutable limit may qualify only if that equivalence is proved;
  the remaining balance never qualifies. Different roots and concurrent calls must
  not share unless the proof covers them. Missing, untrusted, or unrepresentable
  context takes the bypass path.
- **A pre-lookup transformation:** transform before lookup and key on a reviewed,
  canonical effective identity, including transformation version and all relevant
  context and configuration. The review must establish which original and effective
  request is authorized, validated, resolved, traced, and exposed; avoid collisions
  and preserve dependencies. A transformation cannot quietly alter target, verb, or
  security-sensitive arguments after their checks. If this contract cannot be met,
  bypass. Merely re-issuing or borrowing a request inside the endpoint is too late.

Neither option is approved as a general-purpose API by this note. A context partition
is not authority, and does not replace capability attenuation or the capability
floor. Context propagation across tasks and remote boundaries needs an explicit trust
and missing-context policy; a task-local alone is not that contract.

## Budget and cache hits

For the proposed execution budget, a hit **observes** the current budget and spends
no execution units: serving a cached answer does not perform the covered computation.
Admission and any minimum-budget refusal must run before reuse, even on a hit. If the
budget also limits delivery bytes, charge those bytes separately on every delivery,
including a hit, using the existing accounting contract; do not infer a delivery charge
from the cost of the original computation. An overlay that must spend a unit per
invocation, or whose side effects must occur every time, cannot use this hit policy
without a separate pre-lookup charge or effect contract. Until that is specified it
must bypass. The mutable remaining counter is never part of a key: it changes during
execution and cannot provide a stable identity or safe concurrent accounting.

A low budget that changes the amended request or its result is not merely an admission
check. It requires the bypass above or a proven stable partition/transformation. A
budget refusal must not be reused across budgets unless the same equivalence proof
covers it. Budget state is shared and narrowed according to a separate future budget
contract; this note does not claim that today's run context enforces these rules.

## Failures and configuration changes

Apply the same eligibility and identity rules to the failure cache as to successful
answers, including failures with a stated expiry and cached ancestors that catch a
failure. Never allow a prior cached failure to suppress an overlay in a different
context, or a cached success to suppress a context-dependent refusal. Bypass must
suppress failure lookup and insertion as well as success lookup and insertion. Existing
expiry, ceiling, dependency, and cut rules remain necessary but do not distinguish
unkeyed contexts.

Each overlay configuration revision must invalidate both success and failure entries
that can depend on it, including ancestors. A future implementation can record a
configuration thread on every affected computation and cut it on a change, with the
existing cut snapshot/race check preventing an old in-flight computation from being
inserted as fresh. Alternatively, include an immutable configuration generation in a
pre-lookup identity. Publication of configuration and its cut or generation must be
ordered atomically with respect to lookup and insertion: after the change is visible,
no old entry or old in-flight result may be served as the new configuration. A cut
alone is insufficient if an affected entry never recorded the configuration dependency,
or if an old computation starts under a new generation. Configuration storage,
publication, and race behavior need an implementation-level proof; on uncertainty,
bypass. The reproduction's target cut is a control, not proof of this protocol.

## Proposed follow-up items

- Specify and implement pre-lookup bypass propagation across ancestors, re-issues,
  failures, coalescing, fan-out, and remote boundaries; test warm success and failure
  entries, including concurrent roots and a cached ancestor.
- Specify configuration publication and invalidation, then test cuts racing lookup
  and insertion for both success and failure entries and dependent ancestors.
- Specify host-owned monotone budget accounting, admission on hits, delivery charges,
  concurrency and boundary behavior; test exhaustion and hit/miss parity.
- Evaluate a stable partition or pre-lookup transformation with a concrete overlay,
  identity and authorization review, collision handling, and equivalence tests before
  enabling reuse. Specify the argument-view contract separately.
