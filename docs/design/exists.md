# An answer for Exists

Research note, 2026-11-21. This proposes a contract and changes no behavior. The
survey covers the public revisions linked below, not every application or deployment.

## Current behavior

At core main [`1d111516aa7e390110a8ad1007e96576966eb90a`](https://github.com/ikigai-rs/ikigai-core/tree/1d111516aa7e390110a8ad1007e96576966eb90a),
`Exists` can return a resource body. A temporary integration test bound a
`FnEndpoint` returning `text/plain` bytes `resource body`, issued `Exists` with
an open capability, and asserted those exact bytes; `cargo test -p ikigai-core
--test exists_spike_755` passed. The test was removed after the experiment. This
shows one endpoint path, not that every endpoint behaves this way. The
[`FnEndpoint` implementation](../../crates/ikigai-core/src/endpoint.rs#L2659)
passes the invocation to its closure without dispatching by verb; the
[kernel invokes the endpoint and accepts its representation](../../crates/ikigai-core/src/kernel.rs#L5018).
The [existing floor test](../../crates/ikigai-core/tests/floor_every_verb.rs#L87)
checks admission, not the answer's shape. The
[floor for an undeclared Exists](../../crates/ikigai-core/src/kernel.rs#L1242)
uses Source's requirements when declared. Builtins using verb-insensitive closures
can exhibit the same shape, while explicit Exists implementations may already
return booleans.

## Proposed contract

For an authorized, well-formed request to a resolved endpoint, `Exists` answers
`text/plain` containing exactly `true` or `false` in UTF-8. The boolean concerns
the resource selected by the canonical target **and request arguments**, in the
same scope and time view as the request. `false` is an endpoint's affirmative
answer of absence, not a synonym for failure. Denied, concealed, unresolved,
invalid arguments, unavailable, and other errors retain their existing classes.
A resolvable route alone does not prove that a resource exists. An endpoint must
not mutate just to answer. This is a point-in-time answer, not a promise that a
subsequent Source or Sink will succeed; atomic decisions need their own operation.

Make the endpoint responsible for deciding existence and producing the boolean:
it can use a metadata lookup, index or filesystem check without materializing the
representation. Kernel post-resolution projection alone cannot distinguish a
successful `false` from a successful resource body, and still pays for a full
read. The kernel should enforce the answer's type and exact bytes before returning
or caching it, while preserving expiry, dependency threads and authority. Handle
streaming and revalidation explicitly: validate a bounded boolean body and only
revalidate against a previously validated Exists answer. Do not infer `true`
from arbitrary success, including the bytes `false`. Where no cheap check exists,
a separately declared compatibility adapter may source the resource and map only
a documented absence to false; it must preserve Source authority and dependencies,
and its full-read cost must be visible. Otherwise an unsupported Exists should
refuse. Explicit Exists grants may be narrower only if the endpoint's check does
not read or disclose data beyond that grant. Keep the current undeclared-verb
floor until a separately reviewed authority contract replaces it.

This is a breaking change for endpoints and callers. First add a versioned or
opt-in contract and conformance checks; migrate endpoints and callers, including
wire and host adapters, before changing the default. Do not silently project legacy
bodies: a body may itself be the literal `false`. Settle media type, whitespace
and protocol negotiation in that versioned contract, with cross-language vectors.
Existing newline-terminated booleans need a migration path.

## Caching and invalidation

Cache both true and false only when the endpoint can establish freshness. Keep
Exists separate from Source: [request identity includes the verb](../../crates/ikigai-core/src/request.rs#L119),
and the [cache key](../../crates/ikigai-core/src/cache.rs#L118) also separates
capability and scope. Preserve arguments, canonicalization, time scope, defaults,
and authority-sensitive dimensions; never reuse a Source body as an Exists answer
or share a privileged result with a narrower caller.

The existing [dependency propagation](../../crates/ikigai-core/src/kernel.rs#L5108)
attaches the canonical target thread to cacheable Source and Exists answers, and
inherits endpoint and subrequest threads and the most restrictive expiry. Both
polarities need those edges. Sink or Delete through the canonical name cuts its
thread; aliases must converge on the same canonical name. External creates,
deletes, renames, parent-directory changes, binding changes, and changes to query
inputs must also cut the relevant target or additional declared threads. If a
backend cannot observe a change, choose bounded expiry or `Always` (uncached),
including for false. A missing route or an error is not a negative answer; do not
cache it as false. A composite that branches on false must inherit its dependencies
so creation invalidates it too. Test races between lookup, storage and invalidation,
and both answers through aliases and external watchers.

## Public consumer inventory

Line links pin the revisions inspected. The search covered source and tests in the
five named public repositories for `Exists` and the conditional and browse paths;
it cannot establish the behavior of applications outside those repositories.

| Repository and location | Observed behavior and migration |
|---|---|
| [ikigai-fn `src/lib.rs:695`](https://github.com/ikigai-rs/ikigai-fn/blob/970ed151446569b6a62355edd1585ee258c4126f/src/lib.rs#L695), [733](https://github.com/ikigai-rs/ikigai-fn/blob/970ed151446569b6a62355edd1585ee258c4126f/src/lib.rs#L733), [1607](https://github.com/ikigai-rs/ikigai-fn/blob/970ed151446569b6a62355edd1585ee258c4126f/src/lib.rs#L1607) | `conditional` explicitly sources its condition and reads its text, including `equals`; it does not itself issue Exists. Template markers issue constructed requests and splice returned representations. Preserve Source for value comparisons; introduce an explicit existence condition only where the application means existence. The cited tic-tac-toe open/taken application was not found in this repository, so its exact request and migration remain unverified. |
| [ikigai-lisp `src/lib.rs:1671`](https://github.com/ikigai-rs/ikigai-lisp/blob/837374b156c1b588b2d0415baab87bbe4474d118/src/lib.rs#L1671), [1761](https://github.com/ikigai-rs/ikigai-lisp/blob/837374b156c1b588b2d0415baab87bbe4474d118/src/lib.rs#L1761) | `exists` issues the verb through the common call and returns its bytes as a UTF-8 **string**, not a Scheme boolean. Migrate deliberately or provide a new boolean primitive; `"false"` is truthy in Scheme. Preserve typed errors. |
| [ikigai-engine `engine.rs:2041`](https://github.com/ikigai-rs/ikigai-engine/blob/08f9dd2a809653a3b4c5a96270f7df9e37995aed/crates/ikigai-engine/src/engine.rs#L2041), [5953](https://github.com/ikigai-rs/ikigai-engine/blob/08f9dd2a809653a3b4c5a96270f7df9e37995aed/crates/ikigai-engine/src/engine.rs#L5953) | REPL `exists` displays the endpoint answer; its test explicitly expects arbitrary `exists:a` text. Update display and tests; audit plans and external adapters which forward the verb. |
| [ikigai-conformance `suite.rs:1941`](https://github.com/ikigai-rs/ikigai-conformance/blob/f00b1f038df44cb63bf6e4332187c467d7037d98/src/suite.rs#L1941), [4557](https://github.com/ikigai-rs/ikigai-conformance/blob/f00b1f038df44cb63bf6e4332187c467d7037d98/src/suite.rs#L4557), [4821](https://github.com/ikigai-rs/ikigai-conformance/blob/f00b1f038df44cb63bf6e4332187c467d7037d98/src/suite.rs#L4821) | The suite probes declared verbs and inspects representation bytes and faces, including repeated cacheable answers. Add Exists-specific boolean and polarity checks; avoid assuming a Source face for Exists. Required-argument probes still need their error assertions. |
| [ikigai-gonk `src/browse.rs:1063`](https://github.com/ikigai-rs/ikigai-gonk/blob/cee4580e424c151d7bf616538829621db5825985/src/browse.rs#L1063), [1132](https://github.com/ikigai-rs/ikigai-gonk/blob/cee4580e424c151d7bf616538829621db5825985/src/browse.rs#L1132) | The browse overlay's cache-thread selection is Source-only, and classifies Exists as nonmutating; this code does not itself read an Exists body. Do not enable its cache for Exists until external-change invalidation covers both polarities. |
| [ikigai-gonk `src/backfill.rs:368`](https://github.com/ikigai-rs/ikigai-gonk/blob/cee4580e424c151d7bf616538829621db5825985/src/backfill.rs#L368), [backup.rs:1685](https://github.com/ikigai-rs/ikigai-gonk/blob/cee4580e424c151d7bf616538829621db5825985/src/backup.rs#L1685) | Backfill reads the answer bytes, accepts trimmed `true`, and treats any other successful bytes as false; it should reject malformed booleans. Backup explicitly implements Exists using a file check and emits a newline-terminated boolean; it needs an encoding migration. |

## Proposed follow-up items

- **Specify and version the Exists answer** — Define boolean encoding, absence,
  errors, endpoint support and authority, including compatibility and streaming
  behavior; publish shared vectors before changing defaults.
- **Enforce boolean Exists in the kernel** — Add opt-in validation before cache and
  return, retain error and metadata semantics, and test legacy bodies, streaming,
  revalidation and capability floors.
- **Prove existence cache invalidation** — Test both polarities, canonical aliases,
  composite dependencies, concurrent cuts and external create/delete/rename;
  document when a backend must use bounded expiry or no cache.
- **Migrate endpoint existence checks** — Inventory endpoints and builtins, add
  cheap explicit checks where possible, and document the cost and authority of
  any full-read compatibility adapter.
- **Migrate public consumers and conformance** — Update Lisp, REPL, templates,
  browse, backfill and conformance using the versioned contract; locate and test
  the tic-tac-toe open/taken application before changing its condition.
