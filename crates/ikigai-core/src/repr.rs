use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::content::ContentId;
use crate::hashing::{feed_bytes, feed_str};

/// A representation type: a media type plus canonicalized parameters.
///
/// Parameters (e.g. `charset`, parse format) are part of the type so the cache
/// never conflates, say, a UTF-8 decode with a Latin-1 decode of the same bytes.
/// Parameters are stored sorted, giving a single canonical form.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Debug)]
pub struct ReprType {
    /// The media type, e.g. `text/turtle`.
    pub media_type: String,
    /// Canonicalized parameters (sorted by key).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
}

impl ReprType {
    /// A representation type with no parameters.
    pub fn new(media_type: impl Into<String>) -> Self {
        ReprType {
            media_type: media_type.into(),
            params: BTreeMap::new(),
        }
    }

    /// Add or replace a parameter (builder style).
    pub fn with_param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(key.into(), value.into());
        self
    }

    /// The canonical string form: `media/type;k=v;...` with sorted params.
    pub fn canonical(&self) -> String {
        let mut s = self.media_type.clone();
        for (k, v) in &self.params {
            s.push(';');
            s.push_str(k);
            s.push('=');
            s.push_str(v);
        }
        s
    }
}

impl fmt::Display for ReprType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

/// How long a representation stays valid in the cache.
///
/// M3a uses the two ends of the spectrum; richer expiry (dependent on
/// sub-requests, time-based, golden-thread) arrives with dependency tracking.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Expiry {
    /// Always expired — never cached. The safe default: an endpoint must opt in
    /// to caching (mirroring NetKernel, where a response with no expiry is volatile).
    #[default]
    Always,
    /// Never expires — permanently cacheable. Correct for a pure function of
    /// content-addressed inputs, where the request identity fully determines the result.
    Never,
}

/// A typed value produced by an endpoint.
///
/// M0 carries the universal byte form; richer in-memory forms (RDF graphs,
/// solution sets) arrive with the store.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub struct Representation {
    /// The representation type.
    pub repr_type: ReprType,
    /// The representation's bytes.
    pub bytes: Vec<u8>,
    /// Cache validity; defaults to [`Expiry::Always`] (uncacheable).
    #[serde(default)]
    pub expiry: Expiry,
}

impl Representation {
    /// Build a representation from a type and bytes (uncacheable by default).
    pub fn new(repr_type: ReprType, bytes: impl Into<Vec<u8>>) -> Self {
        Representation {
            repr_type,
            bytes: bytes.into(),
            expiry: Expiry::Always,
        }
    }

    /// Mark this representation permanently cacheable ([`Expiry::Never`]).
    pub fn cacheable(mut self) -> Self {
        self.expiry = Expiry::Never;
        self
    }

    /// Set the expiry explicitly (builder).
    pub fn with_expiry(mut self, expiry: Expiry) -> Self {
        self.expiry = expiry;
        self
    }

    /// The content address of this representation (its type and bytes together).
    pub fn content_id(&self) -> ContentId {
        let mut h = blake3::Hasher::new();
        feed_str(&mut h, "ikigai.repr.v0");
        feed_str(&mut h, &self.repr_type.canonical());
        feed_bytes(&mut h, &self.bytes);
        ContentId::from_hasher(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_sorts_params() {
        let t = ReprType::new("text/plain")
            .with_param("charset", "utf-8")
            .with_param("boundary", "x");
        assert_eq!(t.canonical(), "text/plain;boundary=x;charset=utf-8");
    }

    #[test]
    fn type_is_part_of_identity() {
        let utf8 = Representation::new(
            ReprType::new("text/plain").with_param("charset", "utf-8"),
            b"hi".to_vec(),
        );
        let latin1 = Representation::new(
            ReprType::new("text/plain").with_param("charset", "latin-1"),
            b"hi".to_vec(),
        );
        assert_ne!(utf8.content_id(), latin1.content_id());
        assert_eq!(utf8.content_id(), utf8.clone().content_id());
    }
}
