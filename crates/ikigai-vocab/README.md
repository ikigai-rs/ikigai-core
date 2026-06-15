# ikigai-vocab

The self-description vocabulary for **ikigai**, and its RDF projection.

Endpoints describe themselves with a neutral `Description` (from
[`ikigai-core`](https://crates.io/crates/ikigai-core)); this crate renders that
description to RDF. `TurtleRenderer` implements `ikigai_core::MetaRenderer`,
projecting a description to `text/turtle` or `text/plain` — so a `Meta` request
returns a machine-readable (or human-readable) account of an endpoint.

```rust
use std::sync::Arc;
use ikigai_core::Kernel;
use ikigai_vocab::TurtleRenderer;

# fn demo(space: Arc<dyn ikigai_core::Space>) {
// Inject the RDF renderer; `Meta` requests now resolve to Turtle.
let kernel = Kernel::with_meta_renderer(space, Arc::new(TurtleRenderer));
# let _ = kernel; }
```

Turtle rendering is dependency-free, keeping the crate lean and WebAssembly-friendly.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or
[Apache-2.0](../../LICENSE-APACHE) at your option.
