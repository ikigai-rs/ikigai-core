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
    /// The request is well-formed, authorized and names something that exists — and
    /// the **current state** of the resource refuses it: a move to a square already
    /// taken, a move after the game is over, a transition the resource is not in a
    /// position to make, a write whose precondition does not hold. The HTTP analogue is
    /// **409 Conflict** ("conflicts with the current state of the target resource").
    /// **Permanent** (see [`is_transient`](Error::is_transient)): re-issuing the same
    /// request against the same state gets the same answer; what changes the answer is
    /// a change of state, not a retry. Carries a message only, like
    /// [`NotFound`](Error::NotFound) — a precondition has no single argument to name,
    /// which is exactly why [`InvalidArgument`](Error::InvalidArgument) was the wrong
    /// home for it. Never cached, like every error, and a composite that swallows one
    /// and returns a cacheable fallback is forced uncacheable (see
    /// `Invocation::issue` on failed sub-requests).
    ///
    /// **Which refusal is which.** Four variants refuse a request an author might
    /// confuse, and each answers a different question:
    ///
    /// - [`InvalidArgument`](Error::InvalidArgument) — *is the argument itself usable?*
    ///   Wrong spelling, wrong type, out of range, not one of the allowed values. The
    ///   fault is in the request, and naming the argument is the whole message.
    /// - [`NotFound`](Error::NotFound) — *does the fronted thing exist?* It does not.
    /// - [`Denied`](Error::Denied) — *may this capability do it?* It may not; a
    ///   different grant would succeed against the same state.
    /// - `Conflict` — *does the state permit it?* The argument is fine, the thing
    ///   exists, the grant covers it, and the state says no. A different state would
    ///   succeed with the same request and the same grant.
    ///
    /// **Why `Conflict` and not `Precondition` / `FailedPrecondition`.** gRPC's
    /// `FAILED_PRECONDITION` names the same idea, but in HTTP — the edge ikigai actually
    /// meets — a *precondition* is something the CALLER stated (`If-Match`,
    /// `If-None-Match`) and its failure is 412, a narrower case: every failed stated
    /// precondition is a conflict with the current state, but most conflicts (a taken
    /// square) involve no condition the caller wrote. Naming the variant for the
    /// narrower case would invite authors to reserve it for conditional writes and
    /// keep faking the common case. An edge that evaluated a caller's precondition
    /// knows it did, and can still answer 412; everything else is 409.
    ///
    /// **Across the wire, for now.** Until the wire taxonomy gains a tag for it (a
    /// wire-version event, not this crate's), a remote caller receives this as
    /// [`Endpoint`](Error::Endpoint) carrying the displayed message — correct if
    /// untyped, the same way [`DepthExceeded`](Error::DepthExceeded) crosses today.
    ///
    /// ```
    /// use ikigai_core::Error;
    ///
    /// let err = Error::Conflict("1,1 is taken — X played there".into());
    /// assert_eq!(err.to_string(), "conflict: 1,1 is taken — X played there");
    /// assert!(!err.is_transient());
    /// ```
    Conflict(String),
    /// The operation exceeded its time budget. **Transient** — re-issuing an
    /// idempotent verb may succeed (see [`is_transient`](Error::is_transient)).
    Timeout(String),
    /// A dependency or transport is unavailable (down, connection refused,
    /// unreachable). **Transient**, like [`Timeout`](Error::Timeout).
    Unavailable(String),
    /// A sub-request was refused because it would nest deeper than the kernel's
    /// budget (`Kernel::with_max_depth`, default 64) — the typed answer to an
    /// endpoint that issues its own IRI or a transclusion cycle, which used to
    /// recurse until the stack died. **Permanent**: re-issuing the same request
    /// takes the same path. `depth` is the nesting depth the refused request would
    /// have run at (a host's own request is 0; each sub-request is one deeper);
    /// `target` is the canonical name it asked for. Never cached: errors never
    /// reach the representation store, and a composite that swallows this error
    /// and returns a cacheable fallback is forced uncacheable (see
    /// `Invocation::issue` on failed sub-requests).
    DepthExceeded {
        /// The depth the refused sub-request would have run at.
        depth: u32,
        /// The canonical target it asked for.
        target: Iri,
    },
}

impl Error {
    /// Whether re-issuing might succeed: `true` for **transient** failures (timeout,
    /// unavailable), `false` for **permanent** ones (unresolved, bad argument,
    /// denied, or a domain endpoint error). The retry / circuit-breaker / failover
    /// overlays gate on this. Note the request *verb* separately governs whether a
    /// re-issue is *safe*: a non-idempotent `Sink` needs an idempotency key even
    /// when the error is transient. `NotFound` and `Denied` are permanent — re-issuing
    /// won't conjure the resource or the grant — and so is `Conflict`: re-issuing
    /// against the same state gets the same refusal, so a retry overlay that re-sent it
    /// would only spend its budget. What clears a conflict is a change of state.
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
            Error::Conflict(msg) => write!(f, "conflict: {msg}"),
            Error::Timeout(msg) => write!(f, "timeout: {msg}"),
            Error::Unavailable(msg) => write!(f, "unavailable: {msg}"),
            Error::DepthExceeded { depth, target } => write!(
                f,
                "nesting budget exceeded: sub-request for {target} would run at depth {depth}"
            ),
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
        // A conflict is permanent against the same state: only a change of state
        // clears it, never a retry.
        assert!(!Error::Conflict("taken".into()).is_transient());
        assert!(!Error::Endpoint("boom".into()).is_transient());
        assert!(!Error::MissingArgument("in".into()).is_transient());
        assert!(!Error::Unresolved(Iri::parse("urn:x").unwrap()).is_transient());
        // A depth refusal is permanent too: the same request takes the same path.
        assert!(!Error::DepthExceeded {
            depth: 65,
            target: Iri::parse("urn:x").unwrap()
        }
        .is_transient());
    }

    #[test]
    fn conflict_displays_with_its_own_word() {
        // Pinned apart from the doctest because the wire degrades this variant to
        // `Endpoint(to_string())` until it has a tag of its own: the displayed text
        // is what a remote caller reads, so its shape is part of the contract.
        assert_eq!(
            Error::Conflict("the game is over — a draw".into()).to_string(),
            "conflict: the game is over — a draw"
        );
    }
}
