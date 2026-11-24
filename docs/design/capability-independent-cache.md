# Capability-independent answers after admission

Status: design proposal, not implemented. The default remains a cache partition per
capability. This note proposes an opt-in for an endpoint whose answer is independent of
the caller's capability, while preserving a separate authorization decision for every read.

## Observation

`shared_cache_baseline` uses a counted, cacheable stand-in for an origin. Two different
capabilities each hold `urn:cap:net:read`; two sequential reads under each cause two endpoint
calls in total, not one. A third capability without that scope is denied without another
call. This reproduces the kernel partitioning and declared-floor behavior, not an actual
HTTP origin or concurrent fetches. `cargo test -p ikigai-core --test shared_cache_baseline`
passes with those assertions. The request identity is the same; the differing capability
fingerprints select different entries. This can multiply origin traffic across doors and
grants even when the bytes are public.

## Proposed contract

A trusted endpoint author may declare **per verb** that a successful answer, including its
representation metadata, is identical for all admitted capabilities for a given settled
request and resolution scope. Absence of the declaration means today's per-capability key.
The declaration is a security assertion, not an inference from a public URL or from a
`requires` floor. A URL can vary by credentials, headers, cookies, origin policy, principal,
sub-request authority, dynamic state, or a response's `Vary` and cache directives. The
endpoint must account for every such input in request identity or decline sharing. A response
marked private, uncacheable, or dependent on credentials must not enter the shared cache.
The existing expiry and cacheability rules still apply. Start with successful `Source` answers;
leave failures, other verbs, streams that cannot be retained, and revalidation out until their
contracts are specified. No behavior changes merely by adding this note.

Admission must happen before **every** shared lookup, stale offer, or delivery, including a
hit. The current declared floor is checked after resolution and before lookup, but a floor can
be only a coarse requirement: endpoint runtime host/path ACLs can be finer. A declaration is
eligible only if all per-caller checks can be performed by a trusted admission hook before
lookup, using the actual resolved endpoint, settled target and caller capability. The hook must
run even for a hit and deny without exposing bytes or an offer. If a check cannot be moved out
of invocation, sharing is ineligible. Admission cannot grant authority for sub-requests or
stand in for an origin's own authentication. Changes to an ACL must take effect on the next
read, independently of cache expiry. Define failure, side effects, async behavior, tracing,
and the order of defaulting and body settlement before introducing such a hook.

After admission, use a distinct tagged shared partition in place of the capability component,
retaining the canonical request identity, verb, arguments and resolution-scope fingerprint.
Do not use a magic `u64` fingerprint: it could collide with a real capability partition. An
explicit key variant separates existing entries and shared entries, and avoids treating root as
public. Scope remains because different resolution chains can select different endpoints. A
binding or declaration change must invalidate the shared entry before it can be used under a
new contract; retain the binding thread and the cut-in-flight protection, and consider a
binding generation in the key if the existing invalidation cannot ensure that ordering. The
caller-specific cache remains the default, including for composites that depend on a shared
leaf; sharing a leaf does not establish that the composite is capability-independent.

A cache hit must retain the reader's own capability in tracing and enforce its budgets. Do
not cache caller-specific errors or metadata in the shared entry. Revalidation must eventually
perform admission for each reader, preserve validators and origin cache rules, and prevent a
caller-specific revalidation result from replacing a shared one. Sequential sharing does not
promise one origin request for simultaneous misses: request coalescing requires a separate
design that admits each waiter and handles cancellation, errors and cuts safely.

## Golden threads and Meta

The shared entry keeps the same origin and dependency threads, expiry, and generation
snapshots as any other entry. A cut invalidates it and its dependent composites across all
capability partitions; a cut during computation must prevent storing the stale result. A
shared result is only valid if its dependency results and provenance are also independent of
capability, including dependencies resolved with attenuation or grants. Otherwise do not opt
in. Thread names and cache diagnostics can reveal information; the declaration must not make
private dependency names public. In particular, cut listeners currently filter by the
capability fingerprint of entries read. A shared key cannot simply be treated as a read by
every listener. Design per-reader accounting or a conservative listener policy before
shipping, preserving own-reads isolation and invalidation notifications.

`Meta` is answered by the kernel from the description and is exempt from the endpoint floor;
it is not a `Source` invocation and must keep its existing capability-aware key and rendering
semantics. Expose the new declaration in self-description only after defining its vocabulary
and what a reader may learn; do not apply the declaration to `Meta` itself. A binding change
must refresh both the declaration and cached `Meta` as it does for descriptions today.

## Validation needed for implementation

Use a counted origin substitute: two distinct admitted grants read the same request
sequentially and cause one fetch; a caller missing the net grant is denied before any cached
bytes are served, whether it arrives before or after the hit. Add a finer ACL denial on a hit,
revocation after population, differing request arguments and scopes, ordinary capability-
dependent endpoints, expiry and origin cache directives, and binding replacement. Cut an origin
thread and a transitive dependency, including a cut during an in-flight fetch; verify both
shared leaves and per-capability composites recompute. Exercise listeners for two capabilities,
`Meta`, tracing, and concurrent misses. Test credential-dependent and private responses to
ensure they never enter the shared partition. This note's baseline test does not prove the
proposed sharing or those security properties.

## Proposed follow-up items

- **Define per-read admission for shared answers.** Specify and implement a trusted endpoint
  hook that covers finer ACLs and runs before hits and offers, with revocation and denial tests.
- **Add an opt-in shared cache key.** Define the per-verb declaration and tagged partition;
  preserve scope, expiry, binding invalidation, dependency provenance, and safe default behavior.
- **Specify shared-cache observability and listeners.** Preserve per-caller traces, own-reads
  isolation and cut notification semantics without exposing dependency names to other callers.
- **Describe the declaration in Meta.** Define vocabulary and rendering for the assertion,
  its visibility, and binding-change invalidation while leaving Meta capability-aware.
- **Evaluate origin and concurrent fetch behavior.** Map HTTP cache and credential rules to
  eligibility, then design optional miss coalescing and revalidation with per-waiter admission.
