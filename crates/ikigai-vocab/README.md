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

## JSON-LD context

`CONTEXT` is a JSON-LD `@context` for the whole vocabulary — every `ns#` term
mapped to its short name, with datatype and `@id` coercions (integers, booleans,
and IRI-valued properties like `ik:cors`/`ik:shape` typed correctly). It is
**generated from `VOCABULARY`**, so it never drifts from the terms. Serve it at the
external `ns#` URL under content negotiation (`application/ld+json`) beside the
Turtle, and a document's `"@context": "https://ikigai-rs.dev/ns"` resolves — so
config surfaces (e.g. the `urn:web:routes` route table) can be authored in plain
JSON/YAML that lifts to the same RDF.

When you change `vocabulary.ttl`, regenerate it with
`python3 crates/ikigai-vocab/context.gen.py`. A test
(`context_covers_every_vocabulary_term`) fails if the context drifts from the terms.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or
[Apache-2.0](../../LICENSE-APACHE) at your option.
