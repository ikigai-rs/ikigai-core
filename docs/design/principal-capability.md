# Principals as capability scopes: `urn:cap:principal:<iri>`

**Status:** built in 0.1.93 (ledger [#1077](http://localhost:1060/l/default/item/1077), which
also answers [#76](http://localhost:1060/l/default/item/76)). Brian decided the shape on
2026-10-10: identity travels as a CAPABILITY scope minted by the authenticating door, not as
a field on `Invocation` (a public struct field is a flag day).

**Anchor types:** `ikigai_core::{Capability, PrincipalError, principal_scope,
PRINCIPAL_SCOPE_PREFIX, MAX_PRINCIPAL_LEN}`. The rules are stated once, in the "Principals"
section of `Capability`'s doc comment; this note records why they are those rules.

---

## The convention

A door that authenticated a party mints `urn:cap:principal:<iri>` into the capability the
request runs under:

```rust
let session = ceiling.with_principal("urn:iki:gonk:passkey:AbC")?;
```

An endpoint that needs to know who it is serving reads it back:

```rust
match inv.capability.principal() { Some(who) => …, None => … }   // attribution
if inv.capability.acts_as(&draft.author) { … }                     // an author-scoped gate
```

Because it is a scope, the rest follows without new machinery: the cache key already
fingerprints the scope set, so two principals are two partitions
(`kernel::tests::cache_is_keyed_by_principal`); the wire already carries the flat scope
set, so the name crosses mounts in the serialized form it always had; the trace already
records the capability on every event.

## Decisions

**`<iri>` is an absolute IRI, verbatim.** Every principal already in the ecosystem is one
(ikigai-log's `Principal`, ikigai-script's stamped principals, gonk's
`urn:iki:gonk:passkey:*` and `urn:iki:gonk:client:*`, ikigai-quic's examples). No escaping
and no normalization: an escape would give one identity two spellings, and since `allows`
and `acts_as` compare strings, two spellings are two principals that each fail the other's
check. So what cannot be carried as-is is REFUSED at minting (`PrincipalError`): not an
absolute IRI, longer than 512 bytes (ikigai-log's bound, so every principal a door mints is
one the log can write), containing `*`, or spelling a deny-shaped scope (`x:-y` would make
`urn:cap:principal:x:-y`, which `is_deny_scope` reads as an exclusion). A fragment is
allowed: WebIDs carry one.

**One principal per capability.** A request is made for one party, and the log's
`principal=` column holds one answer. A capability holding several well-formed principal
scopes (reachable through `Capability::scoped` or a deserialized peer capability) holds
none: `principal()` is `None`, `allows` grants none of them, and narrowing drops them all.
The last part matters: otherwise a capability naming alice and bob, which reads as nobody,
could be attenuated to bob.

**Narrowing never adds or changes a principal, and `attenuate` may shed it.** Attenuation
keeps the held principal only when the request names it, and never takes one from the
request. Shedding is ordinary narrowing ("attenuation narrows it like any scope"): an
endpoint that issues an identity-independent sub-request with `issue_attenuated` sends it
anonymous, and that sub-request shares a cache partition with every other anonymous one.
An endpoint that narrows and wants to keep the name names its scope in the list.

**`clamp` keeps the ceiling's principal.** This is the one place the convention departs
from "like any scope", and ledger #879 is why. gonk once tagged the session capability with
the client's identity; a client running `ikigai mcp --grant hermes` carried a narrower
capability of its own, the door's `session.clamp(&carried)` intersected the tag away, and
every Hermes write was logged `principal=-` and stored unattributed (verified live
2026-10-07). Under plain intersection the decided design would reproduce that bug at every
door. So a clamp's principal is exactly the ceiling's: a peer cannot shed the identity its
channel authenticated, and a principal the peer carries never enters. Under a ROOT ceiling
the carried capability passes as it always did, name included: root may act as anyone, and
a root-ceilinged channel (IPC to the peercred-verified owner) already trusts the peer
completely.

**Root holds every principal and names none.** `principal()` of root is `None` (root is the
host's own authority, not a party's), `acts_as(x)` of root is `true` for every `x` (the
rule ikigai-script already had: root sees every draft), root narrows to any one principal
by attenuation, and `with_principal` on root is refused rather than silently returning
root: a root capability cannot carry a name, and a door that wants attribution mints onto
the scoped session it hands out.

**A wildcard is never an identity.** A held `urn:cap:principal:*` or `urn:cap:*` is not a
principal (`*` is refused in an IRI, and `urn:cap:*` is not under the prefix), so
`principal()` ignores it and `acts_as` is false for every name. A declared requirement
`urn:cap:principal:*` is still the floor's presence test, which a held wildcard satisfies;
so the floor can say "identified callers only" to the manifold, but an endpoint that asks
WHO reads `principal()`.

**No vocabulary term.** The scope is a capability token, not an RDF term. `ik:principal`
already exists for routes (the credential's subject IRI) and means the same IRI.

## What it costs

**The cache partitions by principal at every depth, not only at the door.** `Invocation::issue`
passes the capability verbatim, so every sub-request under alice's request carries her
name and is cached apart from bob's, even where the answer does not depend on who asked.
ikigai-quic's stamped `principal` argument partitioned only the top-level request.
`issue_attenuated` without the principal scope is the way out for a sub-request that does
not depend on identity.

**A trusted carried capability loses its name at a clamp.** A host that clamps a capability
it recorded itself (ikigai-time's `ceiling.clamp(&job.capability)`, the engine's
`floor.clamp(&identity)` at login) rather than one a peer presented reads the principal off
the recorded capability and re-mints it after the clamp. The clamp cannot tell a recorded
capability from a presented one, and failing closed is the side to err on.

## What adopts it (after 0.1.93 publishes)

- **ikigai-cli.** ikigai-quic: the minter mints `Session::principal` into
  `Session::capability` with `with_principal`, so `clamp` carries it; `stamp_principal` and
  `PRINCIPAL_ARG` can go once nothing reads the argument (that removes the per-client cache
  partition at the door, replaced by the one above). ikigai-web: `PrincipalFn`'s answer is
  minted into the request capability rather than stamped as an inline `principal` argument
  on writes; reads then carry it too. The engine's `login` and ikigai-time's job clamp
  re-mint after clamping.
- **ikigai-gonk.** The doors (HTTP passkey session, QUIC client certificate) mint the IRIs
  they already compute; `admit`/`access` and the author fill read `capability.principal()`
  instead of the stamped argument, where it is the same fact.
- **ikigai-script.** `PrincipalStamper` becomes `inv.capability.principal()`; the draft gate
  is `acts_as(author)`. Draft answers for an author can become cacheable, since the
  capability now differs per principal, which is exactly why they were not.
- **ikigai-log.** `Principal` is built from `capability.principal()` per request, so a
  per-call tracer no longer needs the host to supply the name out of band.
