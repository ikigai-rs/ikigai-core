# Response metadata at the transport boundary

**Status: recommendation accepted by Brian, 2026-11-24 (an answer wrapper with typed stable facts and separately typed exchange effects, no general header map in core); not yet implemented.** Previously: proposal, not an implemented contract. This note studies core at
[`1d11151`](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/repr.rs#L251-L307)
and the public CLI repository at
[`ca3a50b`](https://github.com/ikigai-rs/ikigai-cli/blob/ca3a50b00943fc45990b131a10ca24f57ebaa963/crates/ikigai-web/src/entity.rs#L1-L32).
It recommends a typed, per-answer metadata channel, carried separately from the
body and mediated by the transport, with an explicit distinction between
cacheable answer facts and per-exchange effects. It does not propose a general
HTTP header map in core.

## Evidence and scope

Inspection reproduces the gap: `Representation` exposes type, body and expiry;
its private, nonserialized facts hold cache provenance and origin information,
not response headers ([core `repr.rs` lines 274–317](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/repr.rs#L274-L317)). The web workaround wraps the
body and header lines in `application/vnd.ikigai.http-entity`. Its own example
shows that the returned media type and bytes are the envelope, while other faces
see those bytes raw ([`entity.rs` lines 169–184](https://github.com/ikigai-rs/ikigai-cli/blob/ca3a50b00943fc45990b131a10ca24f57ebaa963/crates/ikigai-web/src/entity.rs#L169-L184),
[lines 26–28](https://github.com/ikigai-rs/ikigai-cli/blob/ca3a50b00943fc45990b131a10ca24f57ebaa963/crates/ikigai-web/src/entity.rs#L26-L28)). This is a source-level reproduction, not a runtime test of every face.
The web door can already set a restricted set of headers through that envelope;
the problem is the cross-transport contract, not an absolute inability to set a
header. `HX-Trigger` is not in its current closed list
([lines 46–105](https://github.com/ikigai-rs/ikigai-cli/blob/ca3a50b00943fc45990b131a10ca24f57ebaa963/crates/ikigai-web/src/entity.rs#L46-L105)).

## What the metadata means

A header's name alone does not establish its lifetime or authority. The host
must decide the mapping in context:

| Example | Candidate meaning and owner |
| --- | --- |
| `Content-Type`, `Content-Language`, `Content-Disposition` | Facts about the selected representation. Type already lives in `ReprType`; language and disposition can be answer facts when stable for the selected answer. |
| `ETag`, `Last-Modified` | Validators for a selected representation, but the door owns their HTTP projection and conditional response rules. Core's origin validator is an opaque revalidation input, not automatically an outgoing validator ([core `repr.rs` lines 221–248](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/repr.rs#L221-L248)). |
| `Link` | May describe a stable relationship of an answer, or be specific to the exchange; define each use before caching it. |
| `Cache-Control`, `Vary` | HTTP policy and selection information, reconciled with kernel expiry and request credentials by the door. They are not a substitute for kernel cache keys or expiry. |
| `Location`, `Retry-After`, `HX-Trigger`, `Set-Cookie` | Exchange instructions or effects: redirect, retry advice, notification, or client state. Their validity, status, timing, and recipient need explicit rules; never assume they are replayable with cached body bytes. `Retry-After` may accompany a failure, so an answer-only API is insufficient. |
| `Content-Length`, `Transfer-Encoding`, `Connection`, security and CORS policy | Framing, hop, or host policy: transport-owned, never delegated blindly to an endpoint. |

The current web allowlist already reserves door-owned fields, limits values,
merges `Vary`, restricts redirects, and permits cookies only with explicit door
opt-in and no cacheable read ([`entity.rs` lines 34–80](https://github.com/ikigai-rs/ikigai-cli/blob/ca3a50b00943fc45990b131a10ca24f57ebaa963/crates/ikigai-web/src/entity.rs#L34-L80),
[lines 299–320](https://github.com/ikigai-rs/ikigai-cli/blob/ca3a50b00943fc45990b131a10ca24f57ebaa963/crates/ikigai-web/src/entity.rs#L299-L320)). A replacement must preserve those checks; a server-set cookie can be HttpOnly, but this channel alone does not implement session security.

## Options

1. **Attach typed metadata to the answer.** Stable facts can travel with the
   selected representation and be cached and invalidated with it. A separate
   exchange-effects component must not be silently stored or replayed: initially
   mark any answer carrying effects uncacheable, including composites, until a
   deliberate replay policy exists. Adding a public field breaks struct users;
   changing serialized `Representation` changes its wire shape. A wrapper result
   or private extension has migration costs too. Core must define a small,
   extensible vocabulary without enshrining arbitrary HTTP fields. Composition
   must not implicitly forward a child's instructions to its parent: the parent
   explicitly selects facts and effects for its own answer.
2. **Provide a per-request response-scope resource.** A transport injects a
   resource through which an endpoint requests effects. The body remains clean
   and non-HTTP hosts can supply an explicit unsupported implementation. But a
   cache hit does not run the endpoint, so a write to that resource cannot be
   assumed to recur; a cached composite can also skip nested writes. Disable
   caching for operations that use it, or specify an effect journal and replay
   rules. Concurrent subrequests need isolation, deterministic conflict/merge
   rules, explicit propagation to the root, rollback on failure, and a lifetime
   tied to one exchange. A scope name must not accidentally share cached results
   across requests; unique scopes can also destroy cache reuse. Remote and wasm
   endpoints need a bridge to the resource. This model fits ephemeral effects but
   makes stable facts and observability more indirect.
3. **Keep the envelope and teach every face to unwrap it.** It works today and
   retains existing cache provenance, but every face and intermediary must know
   an HTTP-specific format, validation, and which information to expose or drop.
   Existing raw consumers change behavior. The envelope changes body identity and
   media type, and is awkward for streams. A new face can leak envelope bytes;
   unwrapping only at the last door also leaves compositions seeing the wrapper.
   Refusing or reporting unsupported effects is necessary; silent dropping is
   unsafe. Keeping it as a compatibility adapter has lower immediate migration
   cost than making it the permanent universal answer.
4. **Keep transports header-free.** No kernel or wire change, but each host must
   invent an alternative for redirects, notification and cookies. This leaves
   HTTP semantics unavailable to endpoints and does not resolve the existing
   cross-face envelope leak. It is a coherent policy only if that limitation is
   accepted explicitly.

## Recommended contract and consequences

Prototype option 1 as an **answer wrapper**, not a committed `Representation`
field: body plus typed stable facts and a separately typed list of requested
exchange effects. Define an equivalent outcome path for errors, including retry
advice. An HTTP door validates and authorizes effects, applies its own status,
framing and security policy, and refuses unsupported or invalid requests with an
observable failure. Non-HTTP faces receive the ordinary body and must expose,
explicitly reject, or document unsupported effects; they must not silently claim
success. This requires an explicit caller contract, not merely dropping a list.
Multiple cookies require ordered, repeated values; redirects and conflicting
singletons need deterministic refusal. Validate length, syntax and authority at
the trusted door even if an endpoint helper validated earlier.

Stable facts are part of the **cached answer**, with the same effective expiry,
capability and scope partition and golden threads as its body. A fact depending on
another resource must contribute that dependency; a thread cut invalidates the
whole answer. Facts must survive a cache hit and time revalidation, or be
recomputed under an explicit rule. Effects initially make the entire resolution
uncacheable, including a parent that consumes an effectful child, so a cache hit
cannot omit or replay a cookie or notification. Define error and partial-failure
semantics before enabling effects in nested or parallel calls. Avoid storing
secrets in shared cache entries or diagnostics. The current kernel propagates
threads through composition and excludes them from serialization and identity
([core `repr.rs` lines 281–305](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/repr.rs#L281-L305));
its content ID covers type and bytes ([lines 365–370](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/repr.rs#L365-L370)). Decide separately whether stable facts enter an answer ID: preserving the body content ID allows byte deduplication, but a distinct answer identity must include any facts relevant to equality or signatures.

`Meta` is a resource self-description, not the metadata of one invocation
([core `verb.rs` lines 24–33](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/verb.rs#L24-L33)). The catalog describes endpoint contracts via `describe()`
([core `endpoint.rs` lines 2560–2573](https://github.com/ikigai-rs/ikigai-core/blob/1d111516aa7e390110a8ad1007e96576966eb90a/crates/ikigai-core/src/endpoint.rs#L2560-L2573));
it may eventually advertise possible metadata and effects, not their per-request
values or secrets. Do not overload `Meta` as a second request for the current
response: it can resolve and cache independently of the body.

The CLI wire serializes `Representation` inside replies with postcard
([`ikigai-wire` lines 1–7](https://github.com/ikigai-rs/ikigai-cli/blob/ca3a50b00943fc45990b131a10ca24f57ebaa963/crates/ikigai-wire/src/lib.rs#L1-L7),
[lines 462–469](https://github.com/ikigai-rs/ikigai-cli/blob/ca3a50b00943fc45990b131a10ca24f57ebaa963/crates/ikigai-wire/src/lib.rs#L462-L469)). A new outcome requires version negotiation, bounded decoding, and explicit old-peer behavior: reject required effects rather than lose them; downgrade only when absence is semantically safe. Golden threads remain kernel-local, so remote validity needs its own contract. The same typed outcome and capability boundary must work in wasm without relying on native threads, sockets or shared memory; browser hosts may be unable to set forbidden response headers and must report that limitation. Streaming must deliver metadata before headers are committed, and define failures after streaming begins.

Migrate incrementally: retain envelope support at the HTTP edge, translate it into the validated new outcome at a defined boundary, and test mixed versions and double wrapping. Remove it only after producers and faces migrate. No behavior change is proposed by this note.

## Proposed follow-up items

- **Prototype typed response outcomes.** Specify stable facts, exchange effects, error outcomes, composition and unsupported-effect behavior; demonstrate ordinary body access across faces.
- **Define cache and invalidation semantics for response metadata.** Test hits, thread cuts, revalidation, nested effects, capability partitions and concurrent failure paths.
- **Version response outcomes on the wire and wasm bridge.** Add negotiated compatibility, size limits, downgrade refusals and streaming tests across native and browser hosts.
- **Migrate HTTP entity producers and doors.** Preserve allowlists and door policy, support repeatable fields, and test redirects, retry advice, notifications and opted-in cookies before retiring the adapter.
- **Describe metadata capabilities in the catalog.** Specify discoverable endpoint contracts without exposing per-request values, credentials or effects as `Meta` output.
