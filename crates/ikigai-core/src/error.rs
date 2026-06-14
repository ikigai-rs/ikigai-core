use std::fmt;

use crate::iri::Iri;

/// The crate result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors raised during resolution and endpoint invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// No endpoint resolved for the target.
    Unresolved(Iri),
    /// A required argument was absent.
    MissingArgument(String),
    /// An argument was present but unusable.
    InvalidArgument {
        /// The argument name.
        name: String,
        /// What was wrong with it.
        detail: String,
    },
    /// An endpoint failed while producing its representation.
    Endpoint(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unresolved(iri) => write!(f, "no endpoint resolved for {iri}"),
            Error::MissingArgument(name) => write!(f, "missing required argument `{name}`"),
            Error::InvalidArgument { name, detail } => {
                write!(f, "invalid argument `{name}`: {detail}")
            }
            Error::Endpoint(msg) => write!(f, "endpoint error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}
