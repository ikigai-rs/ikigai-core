use serde::{Deserialize, Serialize};

/// The request verbs.
///
/// Only the read-only verbs ([`Verb::Source`], [`Verb::Exists`], [`Verb::Meta`])
/// produce cacheable responses; the mutating verbs ([`Verb::Sink`],
/// [`Verb::Delete`]) do not. Idempotence is a different, wider set: `Delete` is
/// idempotent and mutating (see [`Verb::is_idempotent`]).
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
    /// Whether responses to this verb are eligible for caching (read-only:
    /// [`Source`](Verb::Source), [`Exists`](Verb::Exists), [`Meta`](Verb::Meta)).
    pub fn is_cacheable(self) -> bool {
        matches!(self, Verb::Source | Verb::Exists | Verb::Meta)
    }

    /// Whether this verb may mutate underlying state.
    pub fn is_mutating(self) -> bool {
        matches!(self, Verb::Sink | Verb::Delete)
    }

    /// Whether **repeating** a request with this verb leaves the resource as one request
    /// would: `Source`, `Exists`, `Meta` and `Delete`, as HTTP defines idempotent methods
    /// (RFC 9110 §9.2.2, where GET, HEAD, OPTIONS and DELETE are idempotent). The ONE
    /// definition of "safe to re-issue" (ledger #1546, decision 9): a retry or failover
    /// overlay re-issues a failed request only for a verb this answers `true`, and a
    /// `Sink` only with an idempotency key of its own.
    ///
    /// ⚠ **The transports' replay rule is STRICTER, and stays theirs.** A client whose
    /// connection failed after the request may have reached the server (a read-phase
    /// failure) replays only a verb that is not [mutating](Self::is_mutating): a replayed
    /// `Delete` that already ran answers `NotFound`, not what the first one answered, so
    /// a transport that cannot tell whether it ran must not repeat it silently. That is
    /// a second rule for a narrower situation (did it run?), not a second definition of
    /// idempotence; an overlay acting on a TYPED failure the endpoint returned knows the
    /// first attempt failed, and may repeat a `Delete`.
    ///
    /// ```
    /// use ikigai_core::Verb;
    ///
    /// let idempotent: Vec<Verb> = [Verb::Source, Verb::Sink, Verb::Exists, Verb::Delete, Verb::Meta]
    ///     .into_iter()
    ///     .filter(|v| v.is_idempotent())
    ///     .collect();
    /// assert_eq!(idempotent, [Verb::Source, Verb::Exists, Verb::Delete, Verb::Meta]);
    /// // The transports' stricter replay set: idempotent AND not mutating.
    /// assert!(Verb::Delete.is_idempotent() && Verb::Delete.is_mutating());
    /// ```
    pub fn is_idempotent(self) -> bool {
        !matches!(self, Verb::Sink)
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
    fn cacheability_matches_read_only() {
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

    /// Decision 9 of ledger #1546: Delete is idempotent (HTTP's DELETE is), Sink is the
    /// one verb that is not, and every non-mutating verb is.
    #[test]
    fn idempotence_is_every_verb_but_sink() {
        for v in [Verb::Source, Verb::Exists, Verb::Meta, Verb::Delete] {
            assert!(v.is_idempotent(), "{v:?}");
        }
        assert!(!Verb::Sink.is_idempotent());
        for v in [
            Verb::Source,
            Verb::Sink,
            Verb::Exists,
            Verb::Delete,
            Verb::Meta,
        ] {
            if !v.is_mutating() {
                assert!(v.is_idempotent(), "{v:?}: a read is always idempotent");
            }
        }
    }

    #[test]
    fn serde_uses_names() {
        assert_eq!(serde_json::to_string(&Verb::Source).unwrap(), "\"Source\"");
        let v: Verb = serde_json::from_str("\"Meta\"").unwrap();
        assert_eq!(v, Verb::Meta);
    }
}
