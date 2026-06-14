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
#![forbid(unsafe_code)]

mod arg;
mod capability;
mod content;
pub(crate) mod hashing;
mod iri;
mod repr;
mod request;
mod verb;

pub use arg::ArgRef;
pub use capability::Capability;
pub use content::{ContentId, ContentIdError};
pub use iri::{Iri, IriError};
pub use repr::{ReprType, Representation};
pub use request::{Request, RequestId};
pub use verb::Verb;
