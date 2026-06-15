# ikigai-core

The resolution-kernel spine of **ikigai**: address information by identity and
resolve it through composable address spaces, with content-addressed caching so a
result is computed once and reused.

```rust
use ikigai_core::{builtins, ArgRef, Capability, EndpointSpace, Exact, Iri, Kernel, Request, Verb};

# async fn demo() -> ikigai_core::Result<()> {
let space = EndpointSpace::new()
    .bind(Exact::new("urn:fn:toUpper"), builtins::to_upper());
let kernel = Kernel::new(std::sync::Arc::new(space));

let req = Request::new(Verb::Source, Iri::parse("urn:fn:toUpper")?)
    .with_arg("in", ArgRef::Inline(b"ikigai".to_vec()));
let rep = kernel.issue(req, &Capability::root()).await?;
assert_eq!(rep.bytes, b"IKIGAI");
# Ok(()) }
```

## What's here

- **Identity & representations** — validated IRIs, request verbs, content-addressed
  `RequestId`/`ContentId`, and typed `Representation`s with opt-in caching.
- **Resolution** — a `Grammar` (`Exact`, RFC 6570 `UriTemplate`) matches a request
  within a `Space` to an `Endpoint`; spaces compose via `Mount` / `Fallback` / `Rewrite`.
- **Async kernel** — `Kernel::issue` resolves, invokes, and caches. Async but
  *executor-agnostic* (no runtime dependency), so the same core runs natively, in the
  browser via WebAssembly, or embedded.
- **Capabilities & self-description** — an unforgeable capability handle, and a `Meta`
  verb routed to a pluggable `MetaRenderer` (one resource, many representations).

## Lineage

ikigai is inspired by Resource-Oriented Computing and NetKernel, and builds on the
[Oxigraph](https://github.com/oxigraph/oxigraph) RDF/SPARQL crate family.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or
[Apache-2.0](../../LICENSE-APACHE) at your option.
