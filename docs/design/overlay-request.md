# Overlay request views and door context

**Status:** research proposal; no API or behavior change. Source review on 2026-11-21
against core [`1d111516`](https://github.com/ikigai-rs/ikigai-core/tree/1d111516aa7e390110a8ad1007e96576966eb90a)
and the downstream example at [`cee4580e`](https://github.com/ikigai-rs/ikigai-gonk/tree/cee4580e424c151d7bf616538829621db5825985).
The examples below describe needs, not an agreement to implement an API.

## Findings

The reported constraints hold in the inspected sources. An invocation exposes a shared
request reference, and its request reborrow is crate-private ([endpoint.rs:469](../../crates/ikigai-core/src/endpoint.rs#L469),
[endpoint.rs:2404](../../crates/ikigai-core/src/endpoint.rs#L2404)). A detached invocation has
no issuer and starts a fresh recording set ([endpoint.rs:815](../../crates/ikigai-core/src/endpoint.rs#L815));
it cannot issue a dependency. Constructing one with an issuer also starts a fresh record and
does not preserve the kernel's full invocation context. A normal re-issue records success and
failure dependencies and consumes one depth ([endpoint.rs:1661](../../crates/ikigai-core/src/endpoint.rs#L1661),
[endpoint.rs:1685](../../crates/ikigai-core/src/endpoint.rs#L1685)).

A concrete overlay checks query text, then, when a task-local run budget requires it, stamps
the argument and re-issues the request; the second pass invokes the inner endpoint
([sparql.rs:771](https://github.com/ikigai-rs/ikigai-gonk/blob/cee4580e424c151d7bf616538829621db5825985/src/sparql.rs#L771)).
This is source-level reproduction of the route and its extra depth, not a measurement of its
cost. The task-local exists because the derived sub-request capability drops the door marker;
it relies on execution in the same task ([budget.rs:281](https://github.com/ikigai-rs/ikigai-gonk/blob/cee4580e424c151d7bf616538829621db5825985/src/budget.rs#L281)).
Core attenuation intersects the requested scopes with held scopes ([capability.rs:387](../../crates/ikigai-core/src/capability.rs#L387));
a marker omitted from that set is dropped. This is expected narrowing, not an attenuation bug.
A page-origin classification is another possible door context, but the inspected implementation
uses it at admission to narrow authority; it is not evidence of a second page-label task-local
([doors.rs:823](https://github.com/ikigai-rs/ikigai-gonk/blob/cee4580e424c151d7bf616538829621db5825985/src/doors.rs#L823),
[doors.rs:1124](https://github.com/ikigai-rs/ikigai-gonk/blob/cee4580e424c151d7bf616538829621db5825985/src/doors.rs#L1124)).

Core itself already uses the private request reborrow to settle reference arguments before
calling an inner endpoint, sharing the record ([kernel.rs:1016](../../crates/ikigai-core/src/kernel.rs#L1016)).
The existing reference and binding-reborrow tests exercise dependency propagation; they do
not prove a public overlay API or the downstream task-local works across scheduled tasks.

## Request amendment

| Approach | Dependencies and depth | Cache identity and dispatch |
|---|---|---|
| Detached invocation | No kernel issuer; a fresh record loses the outer invocation's recorded work. No new kernel depth for the direct call. | The outer request's key still applies; the detached call has no kernel lookup. |
| Re-issue amended request | A normal sub-request records its result, including failures, on the outer invocation; it costs one depth and can resolve back through the overlay. | The outer entry uses the original request; the inner lookup uses the amended request, capability and scope. It reruns resolution and the capability floor. Termination requires an idempotent stamp or other guard. |
| Borrowed amended request view | A view analogous to `with_request` can share issuer, record, scope, trace, run and depth. Inner dependencies remain outer dependencies; no new resolution depth. | The kernel already keyed the outer lookup on the original request. There is no second key or floor check for the modified view. |

The key is request identity, capability fingerprint and resolution-chain fingerprint
([request.rs:93](../../crates/ikigai-core/src/request.rs#L93),
[cache.rs:118](../../crates/ikigai-core/src/cache.rs#L118)); lookup occurs before the endpoint
runs ([kernel.rs:4902](../../crates/ikigai-core/src/kernel.rs#L4902)). Thus even re-issue
cannot enforce a changed budget on an **outer cache hit**: the overlay is skipped. An amended
view does not automatically rekey a response, and marking the returned response volatile only
prevents a future insertion; it cannot fix an already served hit. For example, identical
request, capability and scope under two task-local budgets can share a cached outer result.
Whether that is acceptable depends on whether the budget affects only execution or also the
answer and whether a cached answer is allowed to bypass enforcement.

Prefer investigating a **borrowed argument-only view**, not arbitrary mutation of the original
invocation. It would preserve the outer request for tracing and identity, borrow a replacement
for the inner call, and share the entire recording state as the private seam does. Restrict
verb and target changes: resolution, bindings, endpoint declaration, capability floor, defaults,
and reference settlement were chosen for the original request. Even an argument-only change
may invalidate validation or change a security-sensitive parameter; callers need a clear
contract for the affected inner endpoint and must not treat this as a second authorized dispatch.
Consider whether the API must validate or restrict replacements, and test both buffered and
streaming paths, success and failure dependencies, tracing, and depth. Do not expose the
private helper unchanged until these semantics and cache treatment are decided.

If amendment depends only on stable inputs already in the key, the original key can represent
the composed overlay's result, provided changes to overlay configuration invalidate it. If it
depends on unkeyed context, choose a stable context partition available **before lookup**, a
pre-lookup transformation with a reviewed identity, or bypass lookup and insertion for the
whole affected subtree. A dependency recorded after a miss cannot repair an earlier hit.
Avoid treating a mutable budget counter as a stable cache key; define separately whether a hit
spends or observes the budget. The re-issue path also needs this decision.

## Context without authority

A separate host-initialized request context could carry door facts through ordinary and
attenuated sub-requests, confinement, and scheduled fan-out, with explicit propagation across
issuer and module boundaries. It must not grant capabilities, widen a scope, or replace the
floor: a page label is an observation, not proof of permission. An enforcing budget is more
than a label: its setter, monotonic narrowing, shared versus per-branch accounting, lifetime,
and handling of remote calls must be specified so endpoint code cannot erase or enlarge it.
A generic freely writable map is insufficient for that invariant. Missing or untrusted remote
context needs an explicit fail-closed or boundary policy, not silent authority creation.

This is distinct from the existing [resolution context](resolution-context.md): that design
chooses where names resolve, including host corridors and levels, and fingerprints the chain.
[Resolution scope](resolution-scope.md#6-values-are-not-corridors-here) explains why request
arguments are not accumulating value corridors. Door facts are neither a new resolution chain
nor inherited parent arguments. The existing run context already carries bounded retries,
deadlines and byte accounting; assess extending its explicit propagation before inventing
another carrier ([endpoint.rs:505](../../crates/ikigai-core/src/endpoint.rs#L505)).
Context that changes results, refusals, or side effects needs an explicit cache and failure-cache
policy at lookup, including isolation between concurrent roots. Context that is only observational
must not silently influence a cached result. Wire propagation, task spawning and trust at remote
boundaries remain open design questions; task-local inheritance is not a portable contract.

## Proposed follow-up items

- **Specify an overlay argument view.** Define allowed replacements, validation and authorization
  invariants, original versus effective request observability, and tests for shared dependencies,
  streaming and depth before exposing any API.
- **Design cache-safe context-dependent overlays.** Reproduce warm-cache behavior across two
  budgets and decide pre-lookup partitioning or bypass, including failures and configuration cuts.
- **Specify door context propagation.** Define host-only initialization, attenuation and
  confinement behavior, scheduled fan-out, isolation, and issuer and remote boundary semantics.
- **Specify monotone execution budgets.** Decide shared accounting, narrowing, cache-hit behavior,
  and missing-context policy separately from descriptive page labels.
