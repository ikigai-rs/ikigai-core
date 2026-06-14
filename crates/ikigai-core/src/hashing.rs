//! Canonical, prefix-free hashing helpers for content-addressed identity.
//!
//! Every variable-length field is length-prefixed so distinct field sequences
//! can never collide. Identity correctness depends on this being deterministic,
//! so all hashed encodings flow through these helpers.

use blake3::Hasher;

/// Feed a length-prefixed byte string.
#[inline]
pub(crate) fn feed_bytes(h: &mut Hasher, b: &[u8]) {
    h.update(&(b.len() as u64).to_le_bytes());
    h.update(b);
}

/// Feed a length-prefixed UTF-8 string.
#[inline]
pub(crate) fn feed_str(h: &mut Hasher, s: &str) {
    feed_bytes(h, s.as_bytes());
}

/// Feed a single discriminant byte.
#[inline]
pub(crate) fn feed_u8(h: &mut Hasher, v: u8) {
    h.update(&[v]);
}
