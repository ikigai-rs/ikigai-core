# ikigai-shacl

A SHACL **validation endpoint** for the
[ikigai-core](https://crates.io/crates/ikigai-core) resolution kernel.

The intended role, in ROC terms, is to expose
[SHACL](https://www.w3.org/TR/shacl/) shape validation as an addressable
resource: resolve a request against a data graph and a shapes graph, and get
back a validation report as a typed `Representation` — so conformance checking
composes with the rest of an ikigai address space (capabilities, caching,
self-description) like any other endpoint.

## Status

**Early stub.** This crate is a placeholder reserving the name and the design
slot. There is no public API or endpoint implementation yet — the crate body is
a documented placeholder pending the design notes. Track the
[ikigai-core](https://crates.io/crates/ikigai-core) repository for progress; do
not depend on it for validation today.

## License

Licensed under either of [MIT](../../LICENSE-MIT) or
[Apache-2.0](../../LICENSE-APACHE) at your option.
