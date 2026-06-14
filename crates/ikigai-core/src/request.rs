use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::arg::ArgRef;
use crate::content::ContentId;
use crate::hashing::{feed_bytes, feed_str, feed_u8};
use crate::iri::Iri;
use crate::verb::Verb;

/// A request: a verb applied to a target resource, with named arguments.
///
/// Arguments are held in a sorted map so a request's identity depends only on
/// the *set* of arguments, never on the order they were added.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub struct Request {
    /// The verb.
    pub verb: Verb,
    /// The target resource identifier.
    pub target: Iri,
    /// Named arguments, kept sorted for a canonical identity.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, ArgRef>,
}

impl Request {
    /// A request with no arguments.
    pub fn new(verb: Verb, target: Iri) -> Self {
        Request {
            verb,
            target,
            args: BTreeMap::new(),
        }
    }

    /// Add an argument (builder style).
    pub fn with_arg(mut self, name: impl Into<String>, arg: ArgRef) -> Self {
        self.args.insert(name.into(), arg);
        self
    }

    /// The content-addressed identity of this request (request scope).
    ///
    /// Argument order is irrelevant: identity depends only on the set of
    /// `(name, value-identity)` pairs. The resolved-endpoint / evaluation-scope
    /// dimension is layered on by the kernel at resolution time.
    pub fn id(&self) -> RequestId {
        let mut h = blake3::Hasher::new();
        feed_str(&mut h, "ikigai.request.v0");
        feed_u8(&mut h, self.verb.code());
        feed_str(&mut h, self.target.as_str());
        h.update(&(self.args.len() as u64).to_le_bytes());
        for (name, arg) in &self.args {
            feed_str(&mut h, name);
            match arg {
                ArgRef::Reference(iri) => {
                    feed_u8(&mut h, 1);
                    feed_str(&mut h, iri.as_str());
                }
                ArgRef::Inline(bytes) => {
                    feed_u8(&mut h, 2);
                    feed_bytes(&mut h, bytes);
                }
                ArgRef::Content(id) => {
                    feed_u8(&mut h, 3);
                    feed_bytes(&mut h, id.as_bytes());
                }
            }
        }
        RequestId(ContentId::from_hasher(h))
    }
}

/// The content-addressed identity of a [`Request`] — itself a content address of
/// the computation, used as the kernel's cache key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Debug)]
pub struct RequestId(ContentId);

impl RequestId {
    /// The underlying content address.
    pub fn content_id(&self) -> ContentId {
        self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iri(s: &str) -> Iri {
        Iri::parse(s).unwrap()
    }

    #[test]
    fn identity_is_deterministic() {
        let r = || {
            Request::new(Verb::Source, iri("urn:r:1"))
                .with_arg("in", ArgRef::Inline(b"hello".to_vec()))
        };
        assert_eq!(r().id(), r().id());
    }

    #[test]
    fn argument_order_does_not_matter() {
        let a = Request::new(Verb::Source, iri("urn:r:1"))
            .with_arg("a", ArgRef::Inline(b"1".to_vec()))
            .with_arg("b", ArgRef::Inline(b"2".to_vec()));
        let b = Request::new(Verb::Source, iri("urn:r:1"))
            .with_arg("b", ArgRef::Inline(b"2".to_vec()))
            .with_arg("a", ArgRef::Inline(b"1".to_vec()));
        assert_eq!(a.id(), b.id());
    }

    #[test]
    fn equal_values_share_identity() {
        // The cache-reuse thesis: same call, same value -> same id.
        let one = Request::new(Verb::Source, iri("urn:upper"))
            .with_arg("in", ArgRef::Inline(b"hello".to_vec()));
        let two = Request::new(Verb::Source, iri("urn:upper"))
            .with_arg("in", ArgRef::Inline(b"hello".to_vec()));
        assert_eq!(one.id(), two.id());
    }

    #[test]
    fn distinct_inputs_have_distinct_identity() {
        let base = Request::new(Verb::Source, iri("urn:r:1"))
            .with_arg("in", ArgRef::Inline(b"x".to_vec()));
        let diff_verb = Request::new(Verb::Exists, iri("urn:r:1"))
            .with_arg("in", ArgRef::Inline(b"x".to_vec()));
        let diff_target = Request::new(Verb::Source, iri("urn:r:2"))
            .with_arg("in", ArgRef::Inline(b"x".to_vec()));
        let diff_value = Request::new(Verb::Source, iri("urn:r:1"))
            .with_arg("in", ArgRef::Inline(b"y".to_vec()));
        let by_ref = Request::new(Verb::Source, iri("urn:r:1"))
            .with_arg("in", ArgRef::Reference(iri("urn:x")));
        let ids = [
            base.id(),
            diff_verb.id(),
            diff_target.id(),
            diff_value.id(),
            by_ref.id(),
        ];
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j], "ids {i} and {j} collided");
            }
        }
    }

    #[test]
    fn serde_round_trip() {
        let r = Request::new(Verb::Sink, iri("urn:r:1"))
            .with_arg("ref", ArgRef::Reference(iri("urn:x")))
            .with_arg("val", ArgRef::Inline(b"v".to_vec()))
            .with_arg("cid", ArgRef::Content(ContentId::of(b"big")));
        let json = serde_json::to_string(&r).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
        assert_eq!(r.id(), back.id());
    }
}
