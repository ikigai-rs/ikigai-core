# ikigai-core

The resolution-kernel spine of **ikigai**: address information by identity and
resolve it through composable address spaces, with content-addressed caching so a
result is computed once and reused.

```rust
use ikigai_core::{builtins, ArgRef, Capability, EndpointSpace, Exact, Iri, Kernel, Request, Verb};

# async fn demo() -> ikigai_core::Result<()> {
let space = EndpointSpace::new()
    .bind(Exact::new("urn:example:to-upper"), builtins::to_upper());
let kernel = Kernel::new(std::sync::Arc::new(space));

let req = Request::new(Verb::Source, Iri::parse("urn:example:to-upper")?)
    .with_arg("in", ArgRef::Inline(b"ikigai".to_vec()));
let rep = kernel.issue(req, &Capability::root()).await?;
assert_eq!(rep.bytes, b"IKIGAI");
# Ok(()) }
```

<a id="whats-here"></a>

## Kernel features

- **Identity & representations** — validated IRIs, request verbs, content-addressed
  `RequestId`/`ContentId`, and typed `Representation`s with opt-in caching.
- **Resolution** — a `Grammar` (`Exact`, RFC 6570 `UriTemplate`) matches a request
  within a `Space` to an `Endpoint`; spaces compose via `Mount` / `Fallback` / `Rewrite` /
  `Limit`, may claim an identity (`.named(iri)`: a claim that any space so named holds
  the same doors, so binding another door drops it; a configuration-free module space
  names itself `urn:iki:space:<module>` through `space_iri`), and **the arrangement is itself a
  resource**: `urn:kernel:topology` renders the chain a request is resolved in as a graph
  (every node an IRI, order explicit), so "is this family reachable without passing the
  limiter?" is a query, and a resolution reports which space answered.
- **"Why did my endpoint not get called?"** — `urn:kernel:explain target=<iri>` is a dry
  run of resolving one name in the asker's chain, invoking nothing: the rewrites that
  would fire, what each member of the chain does with the name (answered, declined,
  limited, shadowed, or opaque where it reports no structure), the endpoint, door and
  grammar bindings that would answer, and every capability scope the asker (or a
  narrower `scopes=`) would be refused for. `Description::unsatisfied_scopes` asks the
  same floor question of one contract.
- **Logical rewrite** — an `AliasTable` maps stable logical URIs onto different
  backing resources (`urn:fn:` → `urn:iki:fn:`, `urn:log:config` →
  `file:/logConfig.yaml`), installed as the `Alias` overlay via
  `Kernel::with_aliases`. Both names are ONE resource: one cache entry, one golden
  thread, one catalog listing (the backing name), and the capability floor is
  enforced against the backing resource. A namespace migration becomes a transition
  window instead of a flag day. The live table — with per-rule hop and miss counters
  — reads out at `urn:kernel:aliases`.
- **Async kernel** — `Kernel::issue` resolves, invokes, and caches. Async but
  *executor-agnostic* (no runtime dependency), so the same core runs natively, in the
  browser via WebAssembly, or embedded.
- **Golden threads you can watch** — a write cuts the threads its dependents hang
  from; `urn:kernel:cache` marks which cached entries a cut has made stale and
  reports lifetime representation evictions (bounds or stale removals; excludes
  replacements, rejected admissions and cached failures) in text and Turtle,
  `urn:kernel:cached` asks whether the asker's own read would hit,
  `urn:kernel:dependents thread=<name>` lists what a cut of one thread would recompute
  (and `as=text/turtle` states the cache, its threads and a thread's dependents as one
  joinable graph), and
  `Kernel::listen` hands a host a bounded queue of cuts (the thread and what it
  invalidated), so it can recompute before the first reader or push a change to a
  page. The kernel reports; it never issues a request from inside a cut.
- **Cache policies a host chooses** — `Lru` (the default), `Fifo`, `CostAware`,
  `ScanResistant` (an entry earns its place by being wanted again, so a one-pass scan
  such as a river read cannot flush the working set) and `Partitioned` (a hard byte
  budget per IRI prefix), installed with `Kernel::with_cache_policy`. A host's
  `config.toml` names one through `CacheConfig` (its `[cache]` table), which fails
  loud on an unknown policy or a bad value; `urn:kernel:cache` names the policy in force.
- **Capabilities & self-description** — an unforgeable capability handle, and a `Meta`
  verb routed to a pluggable `MetaRenderer` (one resource, many representations).
- **Formal companion** — [`docs/formalism/README.md`](https://github.com/ikigai-rs/ikigai-core/blob/main/docs/formalism/README.md) maps the kernel onto Peter Rodgers' *Peter's Hotel* formalism, every theorem pinned to the test that holds its precondition.

<a id="held-content"></a>

## Held content belongs to who delivered it (0.3.0)

Before 0.3.0, any caller that named a content id read what the content store held under it, so a
wire peer could read content another party had delivered. Now an invocation reads a content id
under its own capability, and a foreign id answers exactly as an absent one does (`NotFound`).
Your code still compiles, and these calls now answer differently:

| Held with | Who reads it |
|---|---|
| `MemoryContentStore::put`, `put_replay` | Root only |
| `put_for`, `put_replay_for`, under a capability naming one principal | That principal, under any capability that acts as it, and root |
| `put_for`, `put_replay_for`, under a capability naming no principal, or several | Root only, so the deliverer cannot read it back |

To upgrade a door that holds a caller's content and dispatches under that caller's capability:

1. Name the caller in the capability (`Capability::with_principal`). Anonymous callers can't
   read back what they deliver, because equal scopes don't make two callers one party.
2. Hold the content with `put_for` or `put_replay_for`, under that capability.
3. Check any custom `ContentStore` or `Issuer`. Unless it overrides `get_for` or `content_for`,
   only root reads through it.

A sub-request made with `Invocation::issue_attenuated` reads its caller's content only when its
scopes name the caller's principal (`principal_scope`). Otherwise it gets `NotFound`.

`ContentStore::get` and `Issuer::content` stay public and don't check who is asking. Use them only
in host code, never on behalf of a caller or a peer.

## Lineage

ikigai is inspired by Resource-Oriented Computing and NetKernel, and builds on the
[Oxigraph](https://github.com/oxigraph/oxigraph) RDF/SPARQL crate family.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or
[Apache-2.0](../../LICENSE-APACHE) at your option.
