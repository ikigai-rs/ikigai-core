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
    Reference(Iri),
    /// A small literal value carried inline; identity is the value's bytes.
    Inline(Vec<u8>),
    /// A larger value interned into a content store, named by its content id.
    Content(ContentId),
}
