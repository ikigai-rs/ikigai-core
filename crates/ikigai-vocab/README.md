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

## Plans: a pipeline as a graph

The vocabulary also carries the **process terms** — `ik:Process`, `ik:Step`,
`ik:Argument`, `ik:Fork` and their edges (`ik:pipeFrom`, `ik:mapOver`, `ik:forkOf` +
`ik:order`, `ik:binds` / `ik:ref`) — so a plan, a DAG of requests with
single-assignment names, is a resource in the same graph as the endpoints it calls:
stored, diffed, signed, transrepted, and **validated before any step runs**. A plan
declares its parameters as the same ArgSpec nodes an endpoint does, so a stored plan
describes itself, the manifold offers it, and a plan can call a plan. It is
deliberately not Turing complete — no conditional, no loop, no "now"; a conditional in
a plan is a missing resource, and the environment comes from the kernel.

The text face is the REPL grammar plus names; the graph is that text, skolemized under
`urn:plan:{id}` (`ikigai_vocab::plan` is the scheme, coded once):

```text
urls   = source urn:cms:bookmarks root=@root as=text/uri-list
checks = @urls .. urn:http:reachable ttl=@ttl
count  = @checks | urn:text:wc count=lines
stored = sink urn:cms:linkstatus content=@checks checked=@count
@stored | ( source urn:cms:linkstatus as=text/html ; source urn:cms:linkstatus as=text/turtle )
```

`SHAPES` (`src/shapes.ttl`) is the SHACL contract: one verb from the five per step, one
target, one result, an argument by value or by reference but not both, every edge and
`@name` inside the plan, each name bound once. The shapes are data here — this crate has
no SHACL engine — and `ikigai-shacl` runs them. `tests/fixtures/plan-linkcheck*.ttl` is
the CMS link-check pass written as a plan, with three variants that are three kinds of
edit (a stricter policy, repair instead of removal, a narrower scope) and no new Rust.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or
[Apache-2.0](../../LICENSE-APACHE) at your option.
