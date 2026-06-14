use std::fmt;

use serde::{Deserialize, Serialize};

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
}
