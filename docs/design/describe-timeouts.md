# Reporting a row that could not be described

## Scope and evidence

A space can enumerate a door while its description is unavailable. A peer adapter can,
for example, enumerate a row and then receive a timeout while fetching its metadata. This
note concerns the core walk's representation of that result. It does not choose a wire
protocol, deadline, or budget.

Today `Space::entries` supplies patterns, resolution supplies an endpoint, and
`Endpoint::describe` returns a `Description`, not a result. The catalog renders distinct
descriptions; action selection derives matches from declared verbs and filters them by
capability and query. Thus an adapter that substitutes a description with no verbs yields a
catalog entry but no action match or explanation. The catalog describes endpoint IDs and
contracts, whereas the manifold describes doors and verbs visible to a capability: their
contents are not generally identical, even without a failure.

`crates/ikigai-core/tests/describe_timeout.rs` pins this core-side behavior with an
enumerable two-row stand-in: a volatile, zero-verb placeholder represents a timed-out
metadata lookup, and a second row declares `Source`. The placeholder appears in the
catalog and is absent from the plain action list; the control appears in both, and neither
output has a `mount-unavailable` marker. An initial assertion that the slow row appeared
in actions failed; the committed assertion records the observed divergence. The fixture
supplies the placeholder directly: it neither calls a real peer nor proves how a particular
adapter handles a timeout. The catalog uses an ID-only test renderer, not the production
Turtle renderer. This isolates the core behavior without changing it.

## Recommendation

Keep the enumerated row **marked unresolved in the catalog**, and report the incomplete
walk **in the manifold without presenting the row as an action**. Enumeration is evidence
of a door, not evidence of any verb, contract, or permission. Dropping it from both would
make a failure indistinguishable from absence; inventing verbs would offer unconfirmed
operations. Keeping it only in the catalog without an explanation leaves an agent with a
silently incomplete tool list. A typed diagnostic is not an `ActionMatch` and must not
be returned as an invocable tool.

Introduce a fallible, per-entry describe outcome in the core walk, distinguishing a
confirmed description from an unresolved entry with a classified reason such as timeout.
The space or adapter supplies the outcome for the particular enumerated door; core
combinators preserve it through mounting, rewriting, fallback, and reachability checks.
This requires an explicit API design: simply changing `Endpoint::describe` to return an
error may lose the enumerated door and resolution context, and adding a synthetic endpoint
with declared verbs would misstate its contract. Existing opaque spaces and enumeration
failures also need distinct handling; they are not evidence that a particular row timed out.

For the catalog, an authorized inspector should see the enumerated door or template, a
machine-readable unresolved status, and the reason class. Do not present the placeholder
as a confirmed endpoint contract or merge it into a confirmed contract sharing its ID.
For the manifold, include a machine-readable incomplete status and diagnostic for each
row whose identity the caller is allowed to know, separately from action matches. The
plain list face must either acquire an unambiguous, documented way to signal incompleteness
without looking like a tool, or refuse the request with a classified error; it must not
silently return a partial list. If a representation cannot carry the diagnostic safely,
fail the whole response explicitly. Clients must treat an incomplete manifold as a partial
answer, not proof that its listed actions are exhaustive.

A diagnostic naming a door can disclose existence. Catalog access already requires inspect
authority; the manifold does not, and a failed lookup cannot establish the row's required
capabilities or whether its existence is sensitive. Therefore the safe default for an
unprivileged manifold is a generic incomplete-walk diagnostic without the door, peer,
count, or detailed error. A named diagnostic requires an independently established right
to reveal that identity; do not infer permission from enumeration or a missing contract.
The exact authorization rule and whether even a generic signal is acceptable for hidden
spaces require review. Error text from a peer must not be exposed without an explicit
policy. Existing capability filtering and existence-sensitive behavior must remain intact.

## Distinguishing states and freshness

For an authorized client, an explicit `timeout` status on the enumerated door means its
contract was not obtained in this walk; it does not mean the endpoint is absent or that
an invocation timed out. A missing row is meaningful only after a **complete** walk of
the relevant space and scope. An opaque space, failed enumeration, filtered authority,
or an incomplete walk cannot establish absence. An unprivileged client may learn only that
the answer is incomplete, not which door was slow. Other errors need their own classified
outcomes; do not label every failure a timeout or collapse a confirmed miss into a timeout.

A transient failure must not become a durable negative contract. Until a refresh and
invalidation policy is specified, responses containing unresolved outcomes should be
non-cacheable, including both aggregate faces and any individual metadata answer. A
later successful walk replaces the diagnostic with a confirmed contract and eligible
actions; a later timeout must not reuse stale success as a current offer without an
explicit freshness policy. Preserve binding-change invalidation and capability-scoped
cache separation. Repeated reads can observe different snapshots, so the two independently
requested faces need not reflect the same instant; any stronger snapshot guarantee is a
separate design decision.

## Follow-up items

- **Define fallible per-entry description outcomes.** Specify the core API and propagation
  through space combinators, including templates, shadowing, failed enumeration, and opaque
  spaces, without manufacturing contracts.
- **Specify diagnostic faces and disclosure.** Define structured catalog and manifold
  diagnostics, plain-face behavior, completeness, and authorization for identity and reason;
  review existence-sensitive cases before implementation.
- **Implement and test unresolved-row handling.** Cover timeout, confirmed miss, other errors,
  healthy controls, capability filtering, collisions, recovery, and cache expiry using the
  real renderers and a peer-adapter integration test.
