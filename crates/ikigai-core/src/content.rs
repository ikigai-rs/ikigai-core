use std::fmt;

use serde::{Deserialize, Serialize};

/// A content address: a BLAKE3 digest of canonical bytes, rendered as `b3:<hex>`.
///
/// Equal content yields an equal `ContentId`, which is what makes the cache
/// de-duplicating and the whole identity model content-addressed. The `b3:`
/// prefix names the algorithm, leaving room for agility later.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct ContentId([u8; 32]);

impl ContentId {
    /// Content address of a byte slice.
    pub fn of(bytes: &[u8]) -> Self {
        ContentId(*blake3::hash(bytes).as_bytes())
    }

    /// Finalize a hasher into a content address.
    pub(crate) fn from_hasher(hasher: blake3::Hasher) -> Self {
        ContentId(*hasher.finalize().as_bytes())
    }

    /// The raw 32-byte digest.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Parse a `b3:<hex>` content address.
    pub fn parse(s: &str) -> Result<Self, ContentIdError> {
        let hex = s
            .strip_prefix("b3:")
            .ok_or_else(|| ContentIdError("expected `b3:` prefix".into()))?;
        let bytes = decode_hex(hex).ok_or_else(|| ContentIdError("invalid hex digest".into()))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| ContentIdError("digest must be 32 bytes".into()))?;
        Ok(ContentId(arr))
    }
}

impl fmt::Display for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("b3:")?;
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl From<ContentId> for String {
    fn from(id: ContentId) -> String {
        id.to_string()
    }
}

impl TryFrom<String> for ContentId {
    type Error = ContentIdError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        ContentId::parse(&value)
    }
}

/// Error parsing a [`ContentId`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ContentIdError(String);

impl fmt::Display for ContentIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid content id: {}", self.0)
    }
}

impl std::error::Error for ContentIdError {}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_distinct() {
        assert_eq!(ContentId::of(b"hello"), ContentId::of(b"hello"));
        assert_ne!(ContentId::of(b"hello"), ContentId::of(b"world"));
    }

    #[test]
    fn display_is_b3_hex() {
        let id = ContentId::of(b"x");
        let s = id.to_string();
        assert!(s.starts_with("b3:"));
        assert_eq!(s.len(), 3 + 64);
    }

    #[test]
    fn parse_round_trip() {
        let id = ContentId::of(b"round trip");
        assert_eq!(ContentId::parse(&id.to_string()).unwrap(), id);
        assert!(ContentId::parse("deadbeef").is_err()); // no prefix
        assert!(ContentId::parse("b3:zz").is_err()); // bad hex
    }

    #[test]
    fn serde_round_trip() {
        let id = ContentId::of(b"serde");
        let json = serde_json::to_string(&id).unwrap();
        let back: ContentId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
