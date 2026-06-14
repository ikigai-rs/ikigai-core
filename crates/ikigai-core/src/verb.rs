use serde::{Deserialize, Serialize};

/// The request verbs.
///
/// Only the idempotent verbs ([`Verb::Source`], [`Verb::Exists`], [`Verb::Meta`])
/// produce cacheable responses; the mutating verbs ([`Verb::Sink`],
/// [`Verb::Delete`]) do not.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[repr(u8)]
pub enum Verb {
    /// Read a resource's representation.
    Source = 1,
    /// Write or replace a resource's representation.
    Sink = 2,
    /// Test for a resource's existence.
    Exists = 3,
    /// Remove a resource.
    Delete = 4,
    /// Read a resource's self-description.
    Meta = 5,
}

impl Verb {
    /// Whether responses to this verb are eligible for caching
    /// (idempotent and non-mutating).
    pub fn is_cacheable(self) -> bool {
        matches!(self, Verb::Source | Verb::Exists | Verb::Meta)
    }

    /// Whether this verb may mutate underlying state.
    pub fn is_mutating(self) -> bool {
        matches!(self, Verb::Sink | Verb::Delete)
    }

    /// Stable byte code used in identity hashing.
    pub(crate) fn code(self) -> u8 {
        self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cacheability_matches_idempotency() {
        for v in [Verb::Source, Verb::Exists, Verb::Meta] {
            assert!(v.is_cacheable() && !v.is_mutating());
        }
        for v in [Verb::Sink, Verb::Delete] {
            assert!(!v.is_cacheable() && v.is_mutating());
        }
    }

    #[test]
    fn serde_uses_names() {
        assert_eq!(serde_json::to_string(&Verb::Source).unwrap(), "\"Source\"");
        let v: Verb = serde_json::from_str("\"Meta\"").unwrap();
        assert_eq!(v, Verb::Meta);
    }
}
