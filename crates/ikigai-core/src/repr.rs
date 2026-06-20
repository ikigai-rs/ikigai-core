use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::content::ContentId;
use crate::hashing::{feed_bytes, feed_str};

/// A **golden thread**: a named validity token a cached representation can depend
/// on.
///
/// A thread has identity but no representation — it names a piece of state whose
/// change should invalidate caches. Cutting a thread ([`Kernel::cut`](crate::Kernel::cut))
/// invalidates every cached representation that depended on it, directly or
/// transitively through composition. By convention a thread's name is the IRI of
/// the state it tracks (e.g. `urn:file:notes.txt`, `urn:person:alice`), so a
/// `Sink` that mutates a resource — or an external watcher — cuts the thread named
/// after it.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Thread(String);

impl Thread {
    /// A thread with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Thread(name.into())
    }

    /// The thread's name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for Thread {
    fn from(s: String) -> Self {
        Thread(s)
    }
}

impl From<&str> for Thread {
    fn from(s: &str) -> Self {
        Thread(s.to_string())
    }
}

impl fmt::Display for Thread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

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
    //
    // No `skip_serializing_if`: it would omit an empty map, which a
    // non-self-describing binary codec (postcard, used by the IPC wire) can't
    // round-trip. `default` still fills a missing field in self-describing formats.
    #[serde(default)]
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
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Representation {
    /// The representation type.
    pub repr_type: ReprType,
    /// The representation's bytes.
    pub bytes: Vec<u8>,
    /// Cache validity; defaults to [`Expiry::Always`] (uncacheable).
    #[serde(default)]
    pub expiry: Expiry,
    /// Golden threads this representation depends on (its cache *provenance*).
    ///
    /// Set by [`depends_on`](Representation::depends_on) and grown by the kernel,
    /// which unions in the threads of every sub-resource resolved while producing
    /// it — so a composite inherits its parts' threads and cutting any of them
    /// invalidates it. Kernel-local: **not serialized** (cache validity is a
    /// per-kernel concern, not part of a representation crossing a wire) and **not
    /// part of representation identity** (see the manual `PartialEq`).
    #[serde(skip)]
    threads: BTreeSet<Thread>,
}

// Threads are cache provenance, not content: two representations with the same
// type and bytes are equal regardless of how their validity was tracked. (Also
// keeps wire round-trips equal, since `threads` is `serde(skip)`.)
impl PartialEq for Representation {
    fn eq(&self, other: &Self) -> bool {
        self.repr_type == other.repr_type
            && self.bytes == other.bytes
            && self.expiry == other.expiry
    }
}

impl Eq for Representation {}

impl Representation {
    /// Build a representation from a type and bytes (uncacheable by default).
    pub fn new(repr_type: ReprType, bytes: impl Into<Vec<u8>>) -> Self {
        Representation {
            repr_type,
            bytes: bytes.into(),
            expiry: Expiry::Always,
            threads: BTreeSet::new(),
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

    /// Declare that this representation depends on a golden [`Thread`] (builder).
    ///
    /// When it is cached, cutting that thread invalidates it. A handler reading
    /// mutable state names the thread for that state (conventionally its IRI) and
    /// cuts it on write; an external watcher cuts it on change. Threads also
    /// propagate up through composition automatically, so a composer need only
    /// resolve its parts.
    pub fn depends_on(mut self, thread: impl Into<Thread>) -> Self {
        self.threads.insert(thread.into());
        self
    }

    /// The golden threads this representation depends on.
    pub fn threads(&self) -> &BTreeSet<Thread> {
        &self.threads
    }

    /// Replace the thread set (kernel use: install the effective set after
    /// unioning in dependency threads).
    pub(crate) fn with_threads(mut self, threads: BTreeSet<Thread>) -> Self {
        self.threads = threads;
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
