# ikigai-core

**ikigai** is a resource-resolution kernel: you address information by identity and
resolve it through composable address spaces, with content-addressed caching so a
result is computed once and reused. Most requests are satisfied by deterministic
resolution; a language model is one optional, last-resort resolver — not the default.

This repository is the core, dependency-light layer — no network transports, and it
compiles to WebAssembly. The CLI and its transports live in
[`ikigai-cli`](https://github.com/ikigai-rs/ikigai-cli).

## Crates
| crate | role |
|-------|------|
| `ikigai-core`  | identity, representations, resolution, caching, capabilities |
| `ikigai-vocab` | self-description vocabulary |
| `ikigai-store` | RDF/SPARQL endpoint (Oxigraph-backed) |
| `ikigai-shacl` | SHACL validation endpoint |

Capability-gated file/store behaviour now lives in its own module crate,
[`ikigai-fs`](https://github.com/ikigai-rs/ikigai-fs) (published; native `std::fs`
+ browser `localStorage`), linked by hosts like the other module crates.

## Status
Pre-alpha scaffold. APIs are not yet defined.

## License
Licensed under either of MIT or Apache-2.0 at your option. See `LICENSE-MIT`,
`LICENSE-APACHE`, and `NOTICE`. See also `ACKNOWLEDGEMENTS.md`.
