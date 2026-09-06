use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Whether `s` can be written literally inside an RDF `IRIREF` (`<…>`).
///
/// The Turtle/SPARQL `IRIREF` production forbids exactly `#x00-#x20` (which
/// covers the space and every C0 control) plus `<`, `>`, `"`, `{`, `}`, `|`,
/// `^`, `` ` `` and `\`. A string containing any of them **cannot** be
/// interpolated between angle brackets: `>` closes the IRI early and whatever
/// follows is parsed as Turtle, so an identifier is an injection vector.
///
/// This is a syntactic check on one *fragment*, not IRI validation — use
/// [`Iri::parse`] for that. A fragment that passes here is safe to concatenate
/// into an IRI being emitted; it is not necessarily an IRI itself.
///
/// ```
/// use ikigai_core::is_iri_safe;
///
/// assert!(is_iri_safe("camel-case"));
/// assert!(is_iri_safe("urn:cms:graph"));
/// assert!(!is_iri_safe("evil> ; a <urn:x> . <urn:y"));
/// assert!(!is_iri_safe("has space"));
/// ```
pub fn is_iri_safe(s: &str) -> bool {
    !s.chars().any(is_iri_forbidden)
}

/// Percent-encode the characters an RDF `IRIREF` cannot carry literally, so
/// `s` is safe to interpolate between angle brackets.
///
/// Encodes exactly the set [`is_iri_safe`] rejects, as the UTF-8 bytes of each
/// offending character in upper-case `%XX` form. A string that is already safe
/// is returned **borrowed and byte-identical** — this is a guard rail on a
/// contract nothing else enforces, not a transformation of well-formed names.
///
/// ⚠ **Not injective.** `%` is deliberately left alone, because `class` and
/// `requires` values are authored IRIs that may already contain legitimate
/// percent-escapes and re-encoding them would change their identity. So
/// `a%3Eb` and `a>b` encode to the same fragment. That is an acceptable
/// collision between one real name and one that must never exist; it is not a
/// basis for round-tripping an encoded fragment back to its source.
///
/// ```
/// use ikigai_core::escape_iri_fragment;
///
/// // Ordinary identifiers pass through untouched.
/// assert_eq!(escape_iri_fragment("camel-case"), "camel-case");
/// assert_eq!(escape_iri_fragment("urn:meeting:zoom:schedule"), "urn:meeting:zoom:schedule");
///
/// // An id that would close the IRI early and inject triples cannot.
/// assert_eq!(
///     escape_iri_fragment("evil> ; a <urn:x> . <urn:y"),
///     "evil%3E%20;%20a%20%3Curn:x%3E%20.%20%3Curn:y"
/// );
/// ```
pub fn escape_iri_fragment(s: &str) -> Cow<'_, str> {
    if is_iri_safe(s) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    let mut buf = [0u8; 4];
    for c in s.chars() {
        if is_iri_forbidden(c) {
            for byte in c.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// The Turtle `IRIREF` exclusion set, stated once so the predicate and the
/// escaper can never disagree.
fn is_iri_forbidden(c: char) -> bool {
    c <= '\u{20}' || matches!(c, '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\')
}

/// A validated, absolute RDF [IRI](https://www.w3.org/TR/rdf11-concepts/#dfn-iri).
///
/// Construction validates against RFC 3987 (absolute form) via `oxiri`, so an
/// `Iri` is always a well-formed resource identifier — the basis of identity.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Iri(String);

impl Iri {
    /// Parse and validate an absolute IRI.
    pub fn parse(value: impl Into<String>) -> Result<Self, IriError> {
        let s = value.into();
        oxiri::Iri::parse(s.as_str()).map_err(|e| IriError(e.to_string()))?;
        Ok(Iri(s))
    }

    /// The IRI as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Iri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Iri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Iri({:?})", self.0)
    }
}

impl TryFrom<String> for Iri {
    type Error = IriError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Iri::parse(value)
    }
}

impl From<Iri> for String {
    fn from(iri: Iri) -> String {
        iri.0
    }
}

impl std::str::FromStr for Iri {
    type Err = IriError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Iri::parse(s)
    }
}

/// Error returned when a string is not a valid absolute IRI.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IriError(String);

impl fmt::Display for IriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid IRI: {}", self.0)
    }
}

impl std::error::Error for IriError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_absolute_iris() {
        let iri = Iri::parse("http://example.com/foo").unwrap();
        assert_eq!(iri.as_str(), "http://example.com/foo");
        Iri::parse("urn:ikigai:space:root").unwrap();
    }

    #[test]
    fn rejects_relative_or_malformed() {
        assert!(Iri::parse("").is_err());
        assert!(Iri::parse("foo/bar").is_err()); // no scheme
        assert!(Iri::parse("http://exa mple.com").is_err()); // space
    }

    #[test]
    fn serde_round_trip() {
        let iri = Iri::parse("https://ikigai.rs/r/1").unwrap();
        let json = serde_json::to_string(&iri).unwrap();
        assert_eq!(json, "\"https://ikigai.rs/r/1\"");
        let back: Iri = serde_json::from_str(&json).unwrap();
        assert_eq!(iri, back);
    }

    #[test]
    fn deserialize_validates() {
        let bad: Result<Iri, _> = serde_json::from_str("\"not a valid iri\"");
        assert!(bad.is_err());
    }

    #[test]
    fn iri_safety_covers_the_whole_turtle_exclusion_set() {
        for c in [
            '<', '>', '"', '{', '}', '|', '^', '`', '\\', ' ', '\n', '\t',
        ] {
            assert!(!is_iri_safe(&format!("a{c}b")), "{c:?} must be forbidden");
        }
        for c in '\u{0}'..='\u{20}' {
            assert!(!is_iri_safe(&c.to_string()), "{c:?} must be forbidden");
        }
        // Everything else — including `%`, `#` and non-ASCII — passes through.
        for s in [
            "a%3Eb",
            "urn:cms:graph#frag",
            "café",
            "toUpper",
            "a~b",
            "a'b",
        ] {
            assert!(is_iri_safe(s), "{s} must be allowed");
        }
    }

    #[test]
    fn escaping_a_safe_fragment_borrows_it_unchanged() {
        // The byte-identity guarantee, asserted rather than asserted-about: a safe
        // fragment is not merely equal after escaping, it is the SAME allocation.
        let s = "urn:meeting:zoom:schedule";
        match escape_iri_fragment(s) {
            Cow::Borrowed(b) => assert!(std::ptr::eq(b, s)),
            Cow::Owned(o) => panic!("safe fragment was rewritten to {o}"),
        }
    }

    #[test]
    fn the_escaped_set_stops_at_the_grammar_boundary() {
        // Deliberately the Turtle grammar's set and nothing wider: DEL and the C1
        // controls are legal in an IRIREF, so widening here would rewrite names the
        // grammar accepts.
        assert_eq!(escape_iri_fragment("a\u{7f}b"), "a\u{7f}b");
        assert_eq!(escape_iri_fragment("a\u{20}b"), "a%20b");
        assert_eq!(escape_iri_fragment("a\u{0}b"), "a%00b");
    }
}
