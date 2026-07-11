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
    /// The capability did not authorize the operation — a **permanent** denial.
    /// Typed (rather than a generic `Endpoint` string) so the trace, the manifold,
    /// and a future structured wire error recognize a 403-equivalent without
    /// sniffing the message text.
    Denied(String),
    /// The named resource is absent — a **permanent** not-found. Typed (rather than a
    /// generic `Endpoint` string) so a caller, the trace, and a structured wire error
    /// can recognize a 404-equivalent — an upstream "it isn't here" — without sniffing
    /// the message text. Distinct from [`Unresolved`](Error::Unresolved), which is the
    /// *kernel* finding no binding for the target; `NotFound` is a bound endpoint
    /// reporting that the thing it fronts does not exist.
    NotFound(String),
    /// The operation exceeded its time budget. **Transient** — re-issuing an
    /// idempotent verb may succeed (see [`is_transient`](Error::is_transient)).
    Timeout(String),
    /// A dependency or transport is unavailable (down, connection refused,
    /// unreachable). **Transient**, like [`Timeout`](Error::Timeout).
    Unavailable(String),
}

impl Error {
    /// Whether re-issuing might succeed: `true` for **transient** failures (timeout,
    /// unavailable), `false` for **permanent** ones (unresolved, bad argument,
    /// denied, or a domain endpoint error). The retry / circuit-breaker / failover
    /// overlays gate on this. Note the request *verb* separately governs whether a
    /// re-issue is *safe*: a non-idempotent `Sink` needs an idempotency key even
    /// when the error is transient. `NotFound` and `Denied` are permanent — re-issuing
    /// won't conjure the resource or the grant.
    pub fn is_transient(&self) -> bool {
        matches!(self, Error::Timeout(_) | Error::Unavailable(_))
    }
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
            Error::Denied(msg) => write!(f, "denied: {msg}"),
            Error::NotFound(msg) => write!(f, "not found: {msg}"),
            Error::Timeout(msg) => write!(f, "timeout: {msg}"),
            Error::Unavailable(msg) => write!(f, "unavailable: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_is_only_timeout_and_unavailable() {
        assert!(Error::Timeout("slow".into()).is_transient());
        assert!(Error::Unavailable("down".into()).is_transient());
        // Permanent — re-issuing the same request won't change the answer.
        assert!(!Error::Denied("no grant".into()).is_transient());
        assert!(!Error::NotFound("gone".into()).is_transient());
        assert!(!Error::Endpoint("boom".into()).is_transient());
        assert!(!Error::MissingArgument("in".into()).is_transient());
        assert!(!Error::Unresolved(Iri::parse("urn:x").unwrap()).is_transient());
    }
}
