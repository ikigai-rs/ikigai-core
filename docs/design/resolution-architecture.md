# ikigai — Module System & Resolution Architecture (design draft)

**Status:** forward-looking design sketch. None of the *new* machinery below
(named spaces, modules, the loading boundary, transreption as a first-class
primitive, typed-argument pull, interception, supply-chain security) is built
yet. The *foundations* it stands on are real and shipping: `compose` +
re-entrant `Invocation`, the `Resolver` seam, `ikigai-wire`, the four runtimes
(embedded / IPC / QUIC / WebTransport), the content-addressed cache, and the
`space.rs` composition primitives (`EndpointSpace` / `Mount` / `Fallback` /
`Rewrite`).

This document consolidates the design worked out in conversation on
2026-06-17/18 so we have one artifact to react to and iterate against, rather
than a thread. Open questions are called out inline and gathered at the end.

---

## 0. The thesis

ikigai is a **content-addressed, capability-secured resolution fabric** —
*ZeroTrust · Flexible · Dynamic · Cacheable* — that runs identically in a
terminal, across processes, over the network, and in a browser tab.

The four pillars are *outcomes*, not features, and they collapse to **two
primitives**:

- **Content-addressing** (BLAKE3 request/content ids + the cache) pays for
  *Cacheable* and half of *ZeroTrust* (verifiability).
- **Object-capability-scoped uniform resolution** pays for *ZeroTrust*,
  *Flexible*, and *Dynamic*.

Everything in this document is one of **four primitives wearing a different
hat**:

1. **Addressable, re-entrant resolution** — every resource is an IRI; resolving
   one may resolve others (`Invocation::issue` / `source`).
2. **Content-addressing** — results are identified and cached by the hash of
   what they are.
3. **Transreption** — lossless-ideal transformation between *representations* of
   the same logical resource, kernel-mediated and cached.
4. **Object-capabilities** — no ambient authority; the right to resolve arrives
   *with* the request and can only be *attenuated*, never escalated.

The recurring test we apply: **a new requirement should be expressible with the
existing four primitives.** When it is, the design is sound. So far, it always
has been.

---

## 1. Spaces, names, and the resolution stack

A **space** maps IRIs to endpoints. The core already ships the composition
algebra:

- `EndpointSpace` — binds IRI patterns to endpoints.
- `Mount` — a prefix delegates to a sub-space.
- `Fallback` — an ordered list of spaces; first to resolve wins (stacking).
- `Rewrite` — rewrites IRIs before resolution.

**Named spaces (#16).** Give spaces identity so a request can express a
*space/resource* relationship. A space's name is an **IRI prefix**:
`urn:personal:*` *is* the personal space, `urn:demo:*` the demo space,
`urn:dash:*` the dashboard. Routing falls out of the IRI namespace — no separate
"scope" parameter. The kernel's root is a `Fallback`/`Mount` tree of named
spaces.

**`compose` already spans the stack.** A transclusion marker
`$a{urn:personal:contacts}` does `inv.issue(...)`, which routes through the root
stack to whichever space owns the prefix. `compose` never learns which space (or
remote kernel) answered. The stack *is* the module set.

**Mixed local/remote is just a remote-backed space in the stack (#25).** The
stacking already exists; the missing piece is a `Space` whose `resolve` forwards
over a transport and returns a remote endpoint (the WebTransport demo already
does this client-side). A single `compose` can mix a local bootstrap shell with
live remote data, the boundary invisible to `inv.issue()`.

---

## 2. Modules

**A module = a named space + a manifest.** The manifest declares: name/version,
the patterns it **binds** (exports), which of those it **exposes** per transport
(opt-in, default *not* exposed — #19), and what it **imports** (the resources it
needs to resolve — dependencies). The host mounts the module's space into the
stack under its name.

### 2.1 The weight gradient — low floor, high ceiling; *weight ≈ isolation*

A module is a space-provider; weight is orthogonal, and what you're really
choosing is **how much isolation you pay for**.

1. **Inline closure / function endpoints (lightest).** Bind a function to an
   IRI; manifest and self-description are *inferred* from the function (name →
   IRI, signature → `ArgSpec`s). No artifact, no WIT, no hand-written contract.
   We already do this for Rust: `FnEndpoint::new("toUpper", |inv| …)`.
2. **Lightweight scripted module.** A file of functions hosted by an embedded
   runtime (RustPython, a JS engine). Exposed by convention
   (`greet` in module `personal` → `urn:personal:greet`) or a one-line map.
   In-process; **shares the runtime's capability envelope** — these functions
   trust each other.
3. **Full component module (heaviest).** A WIT/WASM component: explicit
   manifest, independently loadable, *mutually* sandboxed, distributable.

**You isolate at the component boundary, not per function.** Lightweight
functions inside one runtime cooperate freely (cheap); a hard wall — untrusted
code, another language, a shippable artifact — costs a component. The developer
picks the granularity of isolation; the ceremony tracks it.

The **adoption story lives at the floor**: a decorated function

```python
@ikigai.endpoint            # → urn:py:greet, ArgSpec "name" inferred
def greet(name):
    return f"Hello, {name}"
```

becomes a resolvable, self-describing (`describe` works from the signature),
cacheable, capability-gated resource — *the four pillars for the price of a
decorator*. The runtime is the one amortized cost (loaded once); exposing a
function is a binding entry plus a thin signature-derived wrapper that marshals
request args → native args and the return value → a `Representation`. Zero glue.

---

## 3. The loading boundary — built-in vs WASM components

Static Rust gives **built-in** modules: compiled in, fast, trusted (core
builtins, the demo space). It does *not* give independent loading, sandboxing,
polyglot, or per-module distribution. For those, the answer is the **WebAssembly
Component Model + WASI**.

The key realisation: **a WASM module is just another transport across the
`Resolver` seam.** Its WIT interface mirrors what we already have:

```wit
world ikigai-module {
  export manifest: func() -> list<u8>                 // name, binds, exposes, imports
  export resolve:  func(call: list<u8>) -> list<u8>   // ikigai-wire Call → Reply
  import  issue:   func(call: list<u8>) -> list<u8>   // resolve a sub-request via the kernel
}
```

- It **exports** `resolve` (its endpoints). The host wraps the component as a
  `WasmModuleSpace` implementing the core `Space` trait and mounts it — the
  kernel cannot tell a WASM module from a static `EndpointSpace`. Built-in and
  loadable modules coexist behind one interface.
- It **imports** `issue` — exactly the re-entrant `Invocation::issue`, crossing
  the component boundary. The host satisfies that import with a
  **capability-scoped** resolver, so **object-capability is enforced at the WASM
  boundary**: a module can only resolve what it was granted. Sandboxing, ocap,
  and polyglot all fall out of the same boundary. This is the security story for
  untrusted modules.
- The **payload is `ikigai-wire` bytes** (the same `Call`/`Reply` that IPC,
  QUIC, and WebTransport speak). The component boundary is "in-process WASM
  transport," consistent with everything else.

**Higher-order / structured data uses WIT interface types.** Scalar values
(string, bytes) transrept trivially over the wire. *Structured* data crossing a
language boundary (a Rust record → a Python object) needs interface-typed
marshaling, and WIT (records / variants / lists, lifted and lowered by
wit-bindgen) is purpose-built for it. So WIT slots into the type graph (§4) as
the **structured-data transreptor at the component boundary**, complementing
`ikigai-wire` at the value level.

**Cost:** a boundary crossing per resolution. The content-addressed cache
absorbs most of it (hot results never re-cross); `compose`'s fork/join
parallelises module calls.

---

## 4. Transreption — the universal adapter

**Transreption = transform a resource from representation type A to type B,
transparently, ideally losslessly, kernel-mediated and cached.** It is already
pervasive but unnamed: the `MetaRenderer` (Description → Turtle/JSON), `compose`
(shape → assembled HTML), the `ikigai-wire` codec (Call ↔ bytes), and
`describe`'s `as` argument (already a transreption request).

Formalised, it has four parts:

1. **A transreptor is an endpoint** `(repr:A) → (repr:B)` — bindable,
   capability-gated, itself a module.
2. **The kernel keeps a type graph and finds a *path*.** Nodes are
   representation types; edges are transreptors. Asking for a resource "as" type
   B when you have A resolves the chain automatically
   (`toml → json → json-ld → rdf`) — shortest / lowest-cost path. A new
   transreptor (a new edge) lights up new conversions for free.
3. **It is content-negotiation, and it is cached.** Keyed by
   `content-id(source) + target-type`, every conversion is memoised — the
   Cacheable pillar.
4. **Lossless is the ideal, not a guarantee.** Some directions are lossy; for
   manifests (§6) authoring is one-way (dev format is the source of truth, RDF
   is derived), and round-tripping for display is best-effort.

### 4.1 Compilation *is* transreption

The type graph spans **code**, not just data: its nodes include source,
bytecode, LLVM IR, WASM, and machine code; its edges include parsers,
serialisers, **and compilers / codegen**. `python-source → bytecode` is a
transreption; so is `c/rust/swift/zig/… → llvm-ir → wasm`.

Three payoffs:

- **Polyglot for the price of one edge.** LLVM IR is the lingua franca — C, C++,
  Rust, Swift, Zig, Julia, Fortran, Crystal all have LLVM front-ends. Build the
  `llvm-ir → wasm` transreptor *once* and the entire LLVM-compilable universe
  becomes ingestible as modules. You own one edge; the front-ends do the rest.
- **Content-addressed compilation = a free, shared, verifiable build cache.**
  Because transreptions cache by `content-id(source) + target-type +
  transreptor`, compiling source → WASM is memoised. Compile once, ever; a
  federated peer that already did it serves the WASM by content-id instead of
  recompiling (#26) — distributed `sccache` / Nix-substituter behaviour as a
  *property of the fabric*. (Nuance: keying on the *input* content-id makes the
  **local** cache sound regardless of compiler determinism; making it
  **federated/verifiable** — peers agreeing on output bytes — needs
  *reproducible* builds.)
- **The loadable unit is always capability-scoped WASM**, so safety is
  source-language-agnostic — a module compiled from C is sandboxed at the same
  boundary as one written in Rust.

**Discipline:** WASM is the *stable* interchange node and the unit of isolation.
LLVM IR is a version-coupled *on-ramp* (IR is not stable across LLVM releases),
and ABI/runtime isn't free (`c → ir → wasm` needs wasi-libc / WASI shims).
Anchor on WASM; treat the compile edges as pluggable, version-pinned
transreptors.

---

## 5. Typed argument pull + transreption ("witgen-like")

A pure function should be able to operate on the entire resolvable universe
without knowing it exists. `toUpper` stays `fn(in: String) -> String` forever;
what changes is that *"give me `in` as a String"* becomes a **resolution**.

- **Arguments are references-or-values.** `ArgRef` carries `Inline(bytes)` |
  `Reference(iri)` | `Content(content-id)`. (`Invocation::source` already
  dereferences a by-reference argument; this generalises it.)
- **The endpoint declares the type it wants** each argument in — its contract /
  self-description (already used for routing) gains `in: text/plain`.
- **Reading an argument is a typed pull**, e.g. `inv.pull_as::<String>("in")`:
  1. inspect the `ArgRef`;
  2. if it's a reference, **resolve it** via re-entrant `issue` (recording the
     dependency, like `source`);
  3. **transrept** the representation to the declared type (the §4 path-find);
  4. **cache** the result by content-id.

So `toUpper "hello"`, `toUpper urn:data:page`, `toUpper file://foo.txt`, and
`toUpper https://example.com` *all work* — `toUpper` stays pure; the substrate
pulls and transrepts whatever arrived into the string it asked for. `toUpper`
never learns any of it happened.

**The witgen analogy.** The input contract *is* the interface. A proc-macro
reads the signature and declared types and emits the pull glue
(`#[ikigai::endpoint] fn toUpper(in: String) -> String`), exactly as wit-bindgen
generates marshaling from a WIT interface. One contract drives **routing,
transreption, and binding generation**.

**Pull, not push — and it's safer.** Eager auto-coercion (resolve every
reference arg before `invoke`) is convenient but wrong as the *primitive*:

- **Lazy** — an endpoint that doesn't read an arg never pays to resolve it.
- **Capability-gated** — a pull is an ocap-checked resolution. `toUpper https://…`
  only works if the invocation's capability permits the network; a sandboxed
  `toUpper` with no net capability simply *can't* pull a URL. This is ocap
  applied to **inputs**, and it only works because the pull is explicit. Eager
  coercion would be the kernel fetching arbitrary URLs on the function's behalf
  — the ambient authority we're avoiding.
- **CAS + the wire** — over a wire you pass the *reference* (URI / content-id),
  not the content; the receiver pulls from the nearest CAS. Ship
  `Reference(urn:data:page)`, not the page; pull, transrept, and cache locally.
  "ROC using itself to run itself."

So **pull is the primitive; auto-coerce-before-invoke is opt-in sugar on top**
(see interception, §7). Caveats: the type graph needs the edge (else a clear
"can't get X as text/plain"), and external pulls (`http`, `file`) are
resolvers + transreptors that must exist *and* be capability-reachable.

---

## 6. Manifests — never mandate RDF syntax

NetKernel's XML module/space trees were a genuine adoption tax. **Mandating
Turtle/RDF would be that tax squared.** The fix is not to abandon RDF but to make
**RDF the internal canonical form, never the authoring surface.**

- Developers write **JSON / TOML / YAML**.
- ikigai supplies an implied **`@context`** → JSON-LD → RDF. (JSON-LD means JSON
  *already is* RDF; TOML/YAML map to the same JSON shape:
  `TOML/YAML → JSON tree → +@context → JSON-LD → RDF`.)
- **Design that one `@context` well — it is the hinge** the whole system turns
  on; getting it wrong taxes every developer forever.
- The dashboard runs it backwards: project the canonical RDF *back* to
  JSON/TOML/Turtle so a developer reads a module in whatever they're comfortable
  with.

This is just **transreption** (§4) on the manifest. A lightweight module's
manifest is *inferred*; the full `@context`-driven manifest appears only when you
need explicit exposure rules, imports, or capability scoping. Progressive
disclosure.

---

## 7. Interception — cross-cutting concerns, as space composition

The opt-in pre-transrept (§5) is one instance of a general pattern: **an
interceptor wraps a resolution and pre/post-processes it.** Crucially, in ROC
this is *not* a new middleware framework — **an interceptor is a `Space` that
wraps another space**, exactly like `Mount` / `Fallback` / `Rewrite`. It
composes in the same stack.

Reusable uses:

- **Gate-keeping** (authorisation checks before resolution)
- **Throttling / rate-limiting**
- **Audit / logging**
- **Caching policy, retries**
- **Cross-language marshaling** (e.g. a Rust string → a Python value at a module
  boundary; WIT for higher-order types — §3)
- **Opt-in pre-transrept** (auto-pull/coerce declared args before invoke — the
  sugar layer over the §5 primitive)

**Tensions, to design deliberately:**

- **Ordering / composition** — the classic middleware-order problem.
- **Hidden control flow** — interception adds indirection that is invisible at
  the call site. This collides head-on with debuggability (§8); the two must be
  designed *together*, so the resolution trace surfaces the interceptor chain.
  Otherwise we've rebuilt an inscrutable maze.

---

## 8. Debuggability — a first-class constraint, not a footnote

ROC's indirection — resolution chains, transreption chains, interceptors, async,
distribution — was *the* historical adoption barrier. NetKernel was powerful and
inscrutable. We treat debuggability as a primary design goal, and we exploit an
asymmetry nobody before exploited:

> **ROC's own properties can make it *easier* to debug than ordinary async /
> distributed code, not harder.**

Four levers:

1. **Module simulation harness.** Test and debug a module with its kernel /
   imports **mocked** — canned representations for the module's `issue` / `source`
   calls — so the developer reasons only about what's *inside the module*. The
   lightweight-function floor (§2.1) makes pure functions trivially testable; the
   simulated environment extends that to modules with dependencies.
2. **Deterministic replay.** Content-addressing + dependency tracking (the
   "golden thread", #23) mean the same request yields the same DAG. A resolution
   can be replayed offline from the cache.
3. **The resolution DAG is itself a resolvable resource.** The
   resolution / transreption / dependency graph is content-addressed and
   *reflective* — "ROC uses itself." A debugger isn't bolted on; it is built *on*
   the fabric: resolve the trace, inspect the chain, render it live.
4. **Surface interception.** Because interceptors (§7) add hidden control flow,
   the trace must show the interceptor chain — *why* a resolution was throttled,
   transformed, or denied.

Done well, "ROC is hard to debug" *flips* from a liability into a differentiator.

---

## 9. Language-runtime support

Supporting a language is two things:

1. **The runtime as a (heavy) module** — RustPython, a JS engine, etc., loaded
   once and amortised, hosting lightweight scripted endpoints (§2.1).
2. **An idiom-native SDK** — the surface developers actually touch. For Python:
   an `ikigai` module providing the `@ikigai.endpoint` decorator,
   `ikigai.source(iri)` / `ikigai.compose(...)` (wired to the host's
   capability-scoped `issue` import), and the registration glue. JS gets the
   analogous helpers. This is the **lightweight floor delivered per language**,
   and it is where the boundary-marshaling interceptors (§7) hook in.

**Resource-oriented import (the subtle part).** Inside a Python module, calling
other Python is tricky because the runtime differs from a standard interpreter
and we want cross-resource calls to remain resource-oriented (cached, gated,
provenance-tracked). The recommended line is **by scope, not mechanism**:

- **Native imports stay native** for pure, in-module libraries (stdlib, a math
  helper) — no ROC tax where it buys nothing.
- **Cross-resource / cross-module access goes through resolution** via the
  injected `ikigai.source(iri)` — backed by the capability-scoped `issue` import,
  so it is cached, gated, and sandboxed.

The rule of thumb: **ROC at the boundaries, native idioms inside.** A *pure*-ROC
mode (every `import` routes through resolution via a custom meta-path finder that
resolves `urn:py:foo`, execs it, and caches the module object by content-id) is
possible as an opt-in, but it fights Python hard and the purity rarely pays.

---

## 10. Module discovery, loading & supply-chain security

**Discovery:**

- **Directories** — scan one or more directories; load found modules into the
  runtime spaces (drop-in).
- **URLs** — fetch a module from a URL; if the URL is *just* JavaScript or Python
  source, **transrept source → module** in the process (the §4.1
  compile-as-transreption path again).

**Loading pipeline:**

```
source (dir / URL / artifact)
  → [transrept if it's code → WASM/module]
  → RDF manifest view                         (transreption from JSON/TOML/YAML, §6)
  → SHACL-validate against the ikigai module shape   (well-formed? — the shape is itself a resource)
  → [verify canonicalized-RDF signature vs trusted certs]   (IF enforced)
  → instantiate (wasmtime, capability-scoped sandbox)
  → mount as a named space
```

**Security:**

- **SHACL validation** of the RDF view ensures the manifest is well-formed and
  conforms to the module shape — a machine-checkable contract, and the shape is
  itself a resolvable resource.
- **Canonicalized-RDF signatures** (RDFC-1.0 / Data-Integrity proofs) from
  **trusted certs** sign the module's RDF manifest. A **policy** can *enforce*
  signatures for sensitive deployments — reject unsigned modules.
- **wasmtime** provides the execution sandbox; the capability-scoped `issue`
  import (§3) bounds what a loaded module can reach.

This binds the module layer to the broader **trust stack** (the convergence
keystone: verifiable credentials + mTLS + capabilities + signatures). And it is
**progressive**: dev mode is "drop a file in a directory"; secure mode is
"signed + SHACL-valid + capability-scoped." Costs (RDF canonicalization edge
cases, trust-root / cert management) are paid only on the locked-down path.

---

## 11. Where the existing roadmap items land

| Item | Section |
|------|---------|
| #15 module structure | §2, §3 |
| #16 named spaces | §1 |
| #17 capability-gated access + attenuation | §3, §5, §10 |
| #18 transport-http | §6 (content-negotiation = transreption) |
| #19 per-resource opt-in exposure | §2 |
| #20 dashboard / control-plane | §6 (RDF → display), §8 |
| #23 golden thread | §8 (replay, the DAG) |
| #24 concurrent execution | §3 (parallel module calls) |
| #25 stacked local+remote spaces | §1 |
| #26 dynamic offloading / federation | §4.1 (federated build cache) |
| #27 personal contexts (`urn:personal:*`) | §1, §5 |
| #28 transreption | §4 |
| #29 interception | §7 |
| #30 debuggability | §8 |
| #31 discovery / loading / supply-chain | §10 |

---

## 12. Open questions

- **The `@context`** — the single most important artifact to get right (§6).
  Next concrete step: draft the `personal` module in JSON / TOML / Turtle
  side-by-side, all yielding one graph, plus the decorated-Python lightweight
  version, to prove the ergonomics at both weights.
- **Wire-bytes vs typed-WIT** at the component boundary — consistency
  (reuse `ikigai-wire`) vs. structured-type ergonomics (§3).
- **Capability-on-the-wire is now blocking.** The module boundary forces
  `Capability` to serialise (§3, §5); it is currently only `Clone + Debug`. This
  is the foundation the whole loadable-module story stands on (#17).
- **Interceptor ordering** and how the chain is made legible in traces (§7, §8).
- **RDF canonicalization edge cases** for signing (§10).
- **Pure-ROC Python import** as an opt-in mode — worth it, or a curiosity? (§9)
- **Reproducible compilation** — required for the *federated* build cache, not
  the local one (§4.1).

---

## 13. The unifying observation

Every feature in this document reduces to the same four primitives:
interception is space composition; WIT-marshaling and multi-format manifests and
code ingestion are transreption; signing + SHACL is the trust stack over RDF;
debuggability rides content-addressing and reflectivity; the typed-argument pull
is resolution + transreption + capability-check + CAS on the input path;
`compose` is the same on the assembly path. A C library and a TOML manifest reach
the kernel by the *same* mechanism — find the path, run the chain, cache by
content-id.

That a system keeps explaining its new requirements with its existing parts is
the signature of an architecture that was **found, not assembled** — and it is
exactly why it reads as inevitable to the people who get it and alien to the
people reasoning one level down.
