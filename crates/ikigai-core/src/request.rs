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
    //
    // No `skip_serializing_if`: it would omit an empty map, which a
    // non-self-describing binary codec (postcard, used by the IPC wire) can't
    // round-trip. `default` still fills a missing field in self-describing formats.
    //
    // Decoded by `unique_args`, never serde's map impl, which keeps the LAST of a repeated
    // key: a peer that hand-builds bytes with `graph` twice was narrowed to one graph with
    // no error (ledger #1731). No conforming encoder can emit a repeat (a map has one
    // value per key), so this changes no valid encoding: a malformed one is refused.
    #[serde(default, deserialize_with = "unique_args")]
    pub args: BTreeMap<String, ArgRef>,
}

/// Decode a request's argument map, refusing a repeated name rather than keeping the last
/// value of it (ledger #1731). The refusal reads as every face's: `invalid argument
/// `{name}`: given twice; …` ([`GIVEN_TWICE`](crate::GIVEN_TWICE)).
fn unique_args<'de, D>(deserializer: D) -> Result<BTreeMap<String, ArgRef>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Unique;
    impl<'de> serde::de::Visitor<'de> for Unique {
        type Value = BTreeMap<String, ArgRef>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a map of argument names to values, each name once")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut args = BTreeMap::new();
            while let Some((name, value)) = map.next_entry::<String, ArgRef>()? {
                if args.contains_key(&name) {
                    return Err(serde::de::Error::custom(format!(
                        "invalid argument `{name}`: {}",
                        crate::GIVEN_TWICE
                    )));
                }
                args.insert(name, value);
            }
            Ok(args)
        }
    }
    deserializer.deserialize_map(Unique)
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
    /// `(name, value-identity)` pairs.
    ///
    /// This is half of the kernel's cache key, not all of it. The kernel adds the
    /// fingerprint of the capability the request was issued under and the fingerprint
    /// of the resolution chain it was resolved in ([`CacheKey`](crate::CacheKey)), and
    /// nothing else. **The endpoint the request resolved to is not in the key**: a
    /// cached answer stays valid across a rebind until something cuts it. Within a
    /// process that is the host's job, since it knows which names it moved
    /// ([`Kernel::bindings_changed`](crate::Kernel::bindings_changed),
    /// [`Kernel::cut`](crate::Kernel::cut)); across processes it is why a cache cannot
    /// simply be imported (`docs/design/cache-ejection.md` §3).
    ///
    /// ```
    /// use ikigai_core::{ArgRef, CacheKey, Iri, Request, Verb};
    ///
    /// let target = Iri::parse("urn:demo:greet").unwrap();
    /// let a = Request::new(Verb::Source, target.clone())
    ///     .with_arg("name", ArgRef::Inline(b"ada".to_vec()))
    ///     .with_arg("lang", ArgRef::Inline(b"en".to_vec()));
    /// let b = Request::new(Verb::Source, target)
    ///     .with_arg("lang", ArgRef::Inline(b"en".to_vec()))
    ///     .with_arg("name", ArgRef::Inline(b"ada".to_vec()));
    /// assert_eq!(a.id(), b.id(), "argument order is not identity");
    ///
    /// // The kernel's key: the request, the capability's fingerprint, the chain's
    /// // fingerprint (0 for the empty chain). No endpoint anywhere in it.
    /// let key = CacheKey::new(a.id(), 7).in_scope(0);
    /// assert_eq!((key.request, key.capability, key.scope), (a.id(), 7, 0));
    /// ```
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

    /// Ledger #1731: serde's map impl keeps the LAST of a repeated key, so a hand-built
    /// request carrying `graph` twice was silently narrowed to one graph. Refused now, in a
    /// self-describing format and on the postcard wire alike.
    #[test]
    fn a_repeated_argument_name_is_refused_at_decode() {
        let json = r#"{"verb":"Source","target":"urn:r:1","args":{"graph":{"Inline":[65]},"graph":{"Inline":[66]}}}"#;
        let err = serde_json::from_str::<Request>(json).unwrap_err();
        assert!(
            err.to_string()
                .contains("invalid argument `graph`: given twice"),
            "{err}"
        );

        // The postcard wire: encode a one-argument request, then splice a second entry
        // with the same key into the map (count byte 1 → 2), as a hand-built peer could.
        let one = Request::new(Verb::Source, iri("urn:r:1"))
            .with_arg("graph", ArgRef::Inline(b"A".to_vec()));
        let bytes = postcard::to_stdvec(&one).unwrap();
        let entry =
            postcard::to_stdvec(&("graph".to_string(), ArgRef::Inline(b"B".to_vec()))).unwrap();
        let count_at = bytes.len() - entry.len() - 1;
        assert_eq!(bytes[count_at], 1, "the args map count: {bytes:?}");
        let mut doubled = bytes.clone();
        doubled[count_at] = 2;
        doubled.extend_from_slice(&entry);
        assert!(postcard::from_bytes::<Request>(&bytes).is_ok());
        let err = postcard::from_bytes::<Request>(&doubled).unwrap_err();
        // postcard keeps only its own error kind, so the message is not carried: what is
        // pinned is that the bytes are refused rather than narrowed.
        let _ = err;
        // Two distinct names still decode.
        let two = one.with_arg("query", ArgRef::Inline(b"q".to_vec()));
        let back: Request = postcard::from_bytes(&postcard::to_stdvec(&two).unwrap()).unwrap();
        assert_eq!(back, two);
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
