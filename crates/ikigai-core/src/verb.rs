use serde::{Deserialize, Serialize};

/// The request verbs.
///
/// Only the idempotent verbs ([`Verb::Source`], [`Verb::Exists`], [`Verb::Meta`])
/// produce cacheable responses; the mutating verbs ([`Verb::Sink`],
/// [`Verb::Delete`]) do not.
///
/// Ordered by declaration (Source < Sink < Exists < Delete < Meta), which is also
/// the discriminant order, so `(id, verb)` can key a `BTreeMap`/`BTreeSet`:
///
/// ```
/// use std::collections::BTreeSet;
/// use ikigai_core::Verb;
///
/// let actions: BTreeSet<(String, Verb)> =
///     [("file".to_string(), Verb::Meta), ("file".to_string(), Verb::Source)].into();
/// assert_eq!(actions.first(), Some(&("file".to_string(), Verb::Source)));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
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

    /// `(id, verb)` is the natural key for anything walking a manifold (ledger #9:
    /// ikigai-conformance's coverage ledger fell back to a `Vec` with unique-on-insert
    /// because this did not compile).
    #[test]
    fn verb_keys_an_ordered_set_in_declaration_order() {
        use std::collections::BTreeSet;
        let mut seen: BTreeSet<(String, Verb)> = BTreeSet::new();
        assert!(seen.insert(("file".to_string(), Verb::Meta)));
        assert!(seen.insert(("file".to_string(), Verb::Source)));
        assert!(seen.insert(("file".to_string(), Verb::Delete)));
        assert!(!seen.insert(("file".to_string(), Verb::Source)));
        assert!(seen.insert(("alpha".to_string(), Verb::Sink)));
        let order: Vec<(&str, Verb)> = seen.iter().map(|(id, v)| (id.as_str(), *v)).collect();
        assert_eq!(
            order,
            vec![
                ("alpha", Verb::Sink),
                ("file", Verb::Source),
                ("file", Verb::Delete),
                ("file", Verb::Meta),
            ]
        );
        // Declaration order, which is also the discriminant order.
        let all = [
            Verb::Source,
            Verb::Sink,
            Verb::Exists,
            Verb::Delete,
            Verb::Meta,
        ];
        assert!(all
            .windows(2)
            .all(|w| w[0] < w[1] && (w[0] as u8) < (w[1] as u8)));
    }

    #[test]
    fn serde_uses_names() {
        assert_eq!(serde_json::to_string(&Verb::Source).unwrap(), "\"Source\"");
        let v: Verb = serde_json::from_str("\"Meta\"").unwrap();
        assert_eq!(v, Verb::Meta);
    }
}
