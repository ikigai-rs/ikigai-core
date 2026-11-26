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

## Opt-in contract `exists-bool-v1`

This section specifies a future profile, not a feature available on main.
A repeat of the probe on core main at
[`41d76f68241d48b2994b2830d8628bd1c003d855`](https://github.com/ikigai-rs/ikigai-core/tree/41d76f68241d48b2994b2830d8628bd1c003d855)
returned `s3cr3t` for both Source and undeclared Exists on a verb-insensitive
`FnEndpoint`. Requiring `true` instead failed. The assertions used the kernel
issue path with a caller holding no scopes and an endpoint requiring none.

### Meaning and encoding

An authorized, well-formed Exists request under this profile asks about the
resource selected by the canonical target **and effective arguments**, including
bindings and defaults, in the request's scope and time view. A resolved route
does not prove the resource exists. `true` means the endpoint found the selected
resource. `false` means the endpoint affirmatively determined that it is absent.
Neither promises the result of a later Source or Sink. An atomic decision needs
its own operation.

There is one successful representation. At the kernel boundary its media type
string is exactly `text/plain` (lowercase) with an empty parameter map. Check the
raw type before normalization. Its complete body is
one of these UTF-8 byte sequences:

| Meaning | Text | Bytes in hexadecimal |
|---|---|---|
| Present | `true` | `74 72 75 65` |
| Absent | `false` | `66 61 6c 73 65` |

Do not trim, fold case, strip a byte-order mark, transcode, or coerce from a
different format. In particular, neither LF nor CRLF termination is valid.
An endpoint that emits a newline today can keep that encoding on the legacy
path and remove the newline in its profile-specific writer. A host may add a
newline when *displaying* the answer, not when forwarding its representation.
An argument such as `as` cannot negotiate a different successful encoding.
Apply the endpoint's argument rules normally, refusing unsupported arguments
rather than using them to select a different output format.

Denial, concealment, an unresolved target, argument errors, and unavailability
retain their existing error behavior and classes. In particular, concealment
is indistinguishable from the ordinary unresolved answer. Even a typed
`NotFound` from an endpoint is an error to the kernel, not permission to synthesize
`false`. The endpoint can emit `false` after deciding that its own backend result
means absence of the selected resource, not an inaccessible or failed lookup.

### Endpoint support and authority

Support requires an explicit, profile-aware handler registered by the endpoint
or its host on the resolved path. A declaration of the Exists verb is not proof
of profile support, and a profile registration does not add a verb declaration.
Never call a verb-insensitive legacy handler to see if its bytes look boolean.
For a profile request on an authorized route without a handler, return
`Unsupported` with `verb: Exists`, not `false`. Keep existing resolution,
admission, concealment and applicable argument checks ahead of dispatch. In
particular, do not reveal endpoint support to a caller whose request is concealed.

The capability floor is unchanged. Declared Exists uses its own requirements.
Undeclared Exists uses the declared Source requirements, or the union of the
endpoint's declared requirements if Source is not declared. With no requirements
the floor is open. Explicit Exists requirements can differ from Source only
insofar as the handler does not read or disclose anything outside its authority.
A narrow grant to check existence does not grant a body read. Neither selecting
the profile nor passing the floor bypasses other authorization checks.

The endpoint makes the decision, preferably through metadata, an index or a
filesystem check without materializing Source. It does not mutate the resource
or other application state to answer. When a cheap check is unavailable, an
explicitly registered compatibility adapter may make a nonmutating Source read,
with the full cost documented. It must enforce Source authority in addition to
Exists authority and preserve the same selection, scope, time view, lifetime
and dependencies. It may map a successful authorized read to `true` only if
Source success establishes presence for that resource. The body is not the
decision and can itself be `false`. It maps only a documented, authoritative
absence for that resource to `false`, never a denial, concealed error, unresolved
route or unavailable backend. Without those guarantees, refuse support.

### Validation, streaming and revalidation

Validate the media type and bytes before making any success visible or inserting
it in the cache. A malformed successful reply (including a redirect or a body
of the wrong media type) becomes `Error::Endpoint`, never a boolean. For a
framed or streamed body, buffer at most five payload bytes, reject as soon as
the bytes cannot be a prefix of either token (including on any sixth byte), and
require clean end of body even after a complete token. Cancel on rejection and
honor the request's timeout and cancellation while waiting for end of body.
Preserve typed stream failures rather than treating failure as clean end.
Return a collected representation after validation. Preserve freshness and
dependency metadata, including metadata learned at completion. Invalid,
interrupted or incomplete answers are not published or cached as successes.

A revalidation marker is not a boolean representation. Allow it only for an
offered, time-expired, previously validated answer under the same profile and
effective cache key, whose origin validator covers that existence decision.
Honor the kernel's current thread and metadata rules, including cuts during
revalidation. Return the stored boolean and apply the allowed freshness update,
not the bytes or media type of the marker. A marker with no eligible offered
entry is an `Error::Endpoint`. A fresh replacement has to pass full validation.
Never offer a legacy entry, a Source entry, a cut entry or an unvalidated body
for profile revalidation.

### Selection and rollout

The compatibility default remains **legacy**. An in-process caller expressly
selects the profile on the Exists invocation through typed call options, not
through a resource argument or a media type. It is a requirement, not a
preference. If the receiving host cannot honor it, it returns `Unsupported`
(with no verb for an unsupported profile facility). It never retries as legacy
on the caller's behalf. A caller can separately choose to issue a legacy request,
but must treat its answer as an opaque representation, not a presence signal.

On the wire, use a transport revision or extension that explicitly supports
profile negotiation. The serving peer advertises supported profile tokens before
profile calls are sent. An old peer, absent advertisement, or advertisement
without the exact case-sensitive token does not support this profile. On a
profile-capable connection the caller sends an `exists_profile` metadata field
with the exact token on each opted-in Exists request. No field means legacy.
A receiver that sees an unknown token refuses with `Unsupported` (facility)
without dispatching. A success reply must echo the exact selected token in
`exists_profile` response metadata. The caller refuses a successful reply with
a missing or different token as an endpoint protocol error, even if its bytes
look boolean. Typed error replies do not require an echo. The transport's future
binding must define advertisement and field framing before use. No current wire
revision is assigned these fields by this document.

A forwarding peer must carry the selection across each hop and verify the
upstream success before echoing it downstream, or fail closed. It cannot treat
a legacy hop as a profile handler. The profile token is independent of the wire
revision and of the contract IRI digest forms. Separate profile and legacy cache
entries and in-flight work even when the verb, target and arguments are identical.
Do not reuse a legacy result whose bytes happen to pass the new validator.

Copy the [shared vectors](../../crates/ikigai-vocab/tests/vectors/exists_vectors.json)
verbatim into the language faces. Before rollout, conformance must exercise
both polarities, exact bytes and type, error preservation, argument selection
and authorization, stream completion and failure, and revalidation, including
cross-profile and cut entries. Test mixed peers, legacy bodies that look like
booleans, cache separation and invalidation. Migrate endpoint writers and callers,
including wire and host adapters, before any separately reviewed default change.
This design adds no runtime support and changes no default.

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
