use serde::{Deserialize, Serialize};

use crate::content::ContentId;
use crate::iri::Iri;

/// How an argument value is supplied to a request.
///
/// By-value arguments are content-addressed so that two requests carrying the
/// same value share an identity — the key to cache reuse across callers, and
/// the reason ikigai avoids the "fresh id per literal" problem.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum ArgRef {
    /// A reference to another resolvable resource; identity is its IRI.
    ///
    /// **How it reaches the endpoint is settled by the endpoint's contract** (ledger #2511),
    /// by the kernel, before the endpoint runs: an input declared
    /// [`reads_body`](crate::ArgSpec::reads_body) receives the reference and reads it
    /// itself; an input whose class is `xsd:anyURI` receives the IRI's text inline; any
    /// other declared by-value input, and any argument of an open set, receives the
    /// referenced representation inline, read under the caller's capability, collected
    /// under the run's byte budget and recorded as a dependency of the answer. The request's
    /// identity stays the reference. An argument a closed contract does not declare is
    /// handed over as it is, for the endpoint to read.
    Reference(Iri),
    /// A small literal value carried inline; identity is the value's bytes.
    Inline(Vec<u8>),
    /// A larger value interned into a content store, named by its content id: identity
    /// is the digest. Resolved only inside the invocation that carries it, from a body a
    /// plan attached or the kernel's content store (ledger #2240,
    /// [`Kernel::with_content_store`](crate::Kernel::with_content_store)), as a body
    /// ([`Invocation::content_body`](crate::Invocation::content_body)), never as inline bytes.
    Content(ContentId),
}
