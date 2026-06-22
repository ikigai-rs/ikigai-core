//! `ikigai-core` — the resolution kernel spine.
//!
//! M0 establishes the identity model that everything else resolves through:
//!
//! - [`Iri`] — validated absolute resource identifiers
//! - [`Verb`] — the request verbs and their cacheability
//! - [`ArgRef`] — by-reference / inline / content-addressed arguments
//! - [`Request`] / [`RequestId`] — a request and its content-addressed identity
//! - [`Representation`] / [`ReprType`] / [`ContentId`] — a typed value and its content address
//! - [`Capability`] — the unforgeable authority handle (shape only in M0)
//!
//! The identity model is content-addressed: equal inputs collapse to equal
//! identities, so caching and de-duplication fall out by construction.
//!
//! M1 adds resolution: a [`Request`] is matched by a [`Grammar`] within a
//! [`Space`] to an [`Endpoint`] that produces a [`Representation`]. Spaces
//! compose via [`Mount`], [`Fallback`], and [`Rewrite`].
//!
//! # Naming conventions
//!
//! Two namespaces, two conventions:
//!
//! - **Resource identifiers** — IRIs and endpoint names, e.g. `toUpper` in
//!   `urn:fn:toUpper` — use RDF-idiomatic casing: `lowerCamelCase` for
//!   properties and operations, `PascalCase` for classes.
//! - **Rust identifiers** use `snake_case` and `PascalCase` per Rust convention.
//!
//! A `snake_case` constructor therefore maps to a `lowerCamelCase` identifier —
//! e.g. [`builtins::to_upper`] builds the `toUpper` endpoint.
#![forbid(unsafe_code)]

mod arg;
pub mod builtins;
mod capability;
mod content;
mod describe;
mod endpoint;
mod error;
mod grammar;
pub(crate) mod hashing;
mod iri;
mod kernel;
mod meta;
mod repr;
mod request;
mod space;
mod verb;

pub use arg::ArgRef;
pub use capability::Capability;
pub use content::{ContentId, ContentIdError};
pub use describe::{ArgSpec, Description, InputSource};
pub use endpoint::{BoxFuture, Endpoint, FnEndpoint, Invocation, Issuer, Spawner};
pub use error::{Error, Result};
pub use grammar::{Bindings, Exact, Grammar, TemplateError, UriTemplate};
pub use iri::{Iri, IriError};
pub use kernel::{Clock, Kernel, SchedulerReporter, SystemClock, TraceEvent, Tracer};
pub use meta::MetaRenderer;
pub use repr::{Expiry, ReprType, Representation, Thread, Time};
pub use request::{Request, RequestId};
pub use space::{
    EndpointSpace, Fallback, Mount, Resolution, Resolved, Rewrite, Scope, Space, SpaceEntry,
};
pub use verb::Verb;
