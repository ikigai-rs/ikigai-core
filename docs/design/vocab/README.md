# ikigai module vocabulary (design draft)

**Status:** design draft. Placeholder namespaces (`https://ikigai-rs.org/ns/…`);
not yet wired into the kernel. Companion to
[`../resolution-architecture.md`](../resolution-architecture.md) — §2 (modules),
§6 (manifests), §10 (loading & supply-chain).

## The idea in one line

Developers author modules in **JSON / TOML / YAML**; ikigai injects an implied
**`@context`** that turns the JSON tree into JSON-LD → RDF. RDF is the internal
canonical form, **never the authoring surface** — NetKernel's XML trees were an
adoption tax, and mandating Turtle would be worse.

## Files

| file | what it is |
|------|------------|
| `module.context.jsonld` | the implied context (`module/v1`) — **the hinge**. The developer never writes it. |
| `fn.shape.ttl` | `urn:shape:fn` — the SHACL contract a resource must satisfy to be importable as a function. |
| `examples/personal.module.{json,toml,yaml}` | the same module, three ways — what a developer writes. **Note: no `@context`** — ikigai supplies it. |
| `examples/personal.module.ttl` | the single RDF graph all three produce. |
| `examples/greeting.py` + `greeting.inferred.ttl` | the lightweight end: a decorated function whose manifest is *inferred*, landing in the same vocabulary. |

## Three formats → one graph

TOML and YAML parse to the *same JSON tree*; JSON + the implied context *is*
JSON-LD. Run any of the three example manifests through ikigai and you get
`personal.module.ttl`. None of the three contains a triple, an angle bracket, or
a prefix.

## Design decisions

1. **`mediaType`, not `type`.** `type` is the conventional JSON-LD alias for
   `@type`; reusing it for media types is a footgun. We spend a slightly
   less-obvious key rather than booby-trap the context.

2. **The module `@id` is *synthesized*, not `@base`-templated.**
   `"module": "personal"` maps to `ik:name "personal"`, and ikigai's manifest
   transreptor assigns the subject `urn:ikigai:module:personal`. We deliberately
   do **not** use JSON-LD `@base` for this: RFC-3986 relative resolution does not
   *append* against a non-hierarchical `urn:` base — `"personal"` resolved
   against `urn:ikigai:module:` yields `urn:personal`, not what we want.
   Synthesizing the id in the transreptor keeps module ids as URNs and keeps the
   conversion where it belongs. (An HTTP base with a trailing slash *would*
   template correctly, at the cost of HTTP module ids; we chose URN consistency.)

3. **Platform-as-predicate** (`ikp:macos`, `ikp:linux`, `ikp:windows`). The
   ergonomic OS-keyed `backend` map produces clean RDF, at the cost of closing
   the platform set to the vocabulary. The open alternative is
   `backend: [{ platform, target }]` (cleaner for arbitrary platforms, more
   verbose to author). Predicate for the common OSes + an escape hatch is the
   current call — **open question.**

4. **Imports are constrained references, never open prefixes** (see below).

## Imports: loose locally, tight across a trust boundary

An import declaration is what **mints the capability** the module's `issue` is
scoped to — so it must be least-privilege. A bare prefix wildcard (`urn:fn:*`) is
rejected: **unbounded** (importing things that don't exist yet), **mutable**
(what answers an IRI can change), **non-reproducible** (depends on who published
what, when), and **over-broad** (a wildcard import is a wildcard capability).

- **Loose** — a bare IRI (`"urn:fn:toUpper"`): resolve whatever currently
  answers. Fine *within your own trust boundary*.
- **Tight** — an object that pins and/or scopes:
  - `version` (range) or `content` (`blake3:…`, exact + tamper-evident) — *pinning*;
  - `signer` (a trusted cert/DID) — *trust*;
  - `prefix` + `shape` (+ `signer`) — an **open namespace is admissible only when
    bounded by a contract shape and a trusted signer**. This is the safe form of
    "import a family."

**Structure and trust are separate, composable gates.** `shape`
(e.g. `urn:shape:fn`) checks the candidate is a well-formed function; `signer`
checks who vouched for it; `content`/`version` pin *which* one. Secure-mode
policy (resolution-architecture §10) can require every import be tight *and*
signed; dev mode lets you write the bare IRI. Progressive disclosure: loose to
start, tight when it's load-bearing.

For the lightweight (Python) end, imports can be **discovered** from the
`source(...)` call sites and then *confirmed and pinned* explicitly — the easy
path proposes the dependency list; the secure path locks it down.

## Binding: who may publish into a space (the dual of imports)

A module does **not** get to publish into a prefix by fiat. There is **no global
publish**: within a kernel the **host composes the stack** (`Mount` / `Rewrite`
are operator acts), so a module *offers* bindings and the host decides whether
and where they graft into a public prefix. The manifest's `space:` (and the
bound prefixes) is a **request/default ratified by the host's mount**, not a
claim — `"space": "urn:personal:"` does not, by itself, authorize anything.

Binding authority is the **write side** of capability-gated space access (#17):
a capability over a prefix, delegated and attenuated from the space's **root
authority**. Safe-by-default mechanism: a module binds in **its own namespace**
(owned by construction), and the host `Rewrite`s chosen bindings into the public
prefix — so a module literally cannot *name* another's namespace without an
explicit host graft. A first-party module that *defines* a space holds (or is
granted) authority over the prefix; a third-party contribution needs an explicit
grant.

**Publish-authority and import-trust are duals that meet at the signature.** A
space's bindings are trustworthy iff signed by the space authority — the *same*
signer an importer gates on (`{ prefix, signer }`). So locking *who may bind* is
exactly what makes a namespace *safe to import from*; both reduce to "whose
signature vouches for this prefix's contents." (This is why the worked example's
`greeting.py` is a **component of the personal module** — same authority, same
signer — not a stray file that squats `urn:personal:greeting`.)

## `urn:shape:fn`

The *structural* contract for a function resource: exactly one output
`ik:mediaType`, and every declared `ik:arg` a well-formed `{ argName, mediaType }`.
It carries **no** signing requirement — that's the import's `signer` gate, kept
orthogonal so the shape is reusable for local validation too. A constrained
`prefix` import names this shape to admit a whole family safely.

## Open questions (carried from the design discussion)

- **Namespace IRIs** are placeholders (`https://ikigai-rs.org/ns/…`). w3id.org?
  A permanent ikigai domain? Painful to change later — worth deciding early.
- **Platform-as-predicate vs `{platform, target}` list** (decision #3).
- **Representation types as IRIs, on two axes.** Today `ik:mediaType` is a string
  token. The principled model makes representation types **IRIs in one type
  space** (media-type / WIT-interface / XSD-datatype / SHACL-shape IRIs all as the
  nodes of the transreption type graph), so transreptors become RDF triples
  `(fromType) ik:transrepts (toType)`. That also separates two axes `xsd:string`
  conflated: **serialization** (`ik:mediaType`, e.g. `text/plain`) vs **structure**
  (a future `ik:schema` — a SHACL shape, a **WIT interface**, or an XSD datatype).
  WIT maps onto the structure axis (record ≈ SHACL shape; primitives ≈ XSD), and
  that mapping is itself a transreptor. See task #28.
- **Controlled vocabularies** for `cache` / `transport` / `capability` (string
  literals today; enumerated IRIs once SHACL tightens).
- **`module/v1` context versioning** — how the implied context evolves without
  breaking existing manifests.
