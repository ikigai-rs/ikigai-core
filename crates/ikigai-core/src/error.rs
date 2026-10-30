use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::iri::Iri;
use crate::verb::Verb;

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
    /// position to make, a write whose precondition does not hold. The HTTP analog is
    /// **409 Conflict** ("conflicts with the current state of the target resource").
    /// **Permanent** (see [`is_transient`](Error::is_transient)): re-issuing the same
    /// request against the same state gets the same answer; what changes the answer is
    /// a change of state, not a retry. Carries a message only, like
    /// [`NotFound`](Error::NotFound) — a precondition has no single argument to name,
    /// which is exactly why [`InvalidArgument`](Error::InvalidArgument) was the wrong
    /// home for it. Never cached, like every error its endpoint states no expiry for
    /// (`Invocation::failure_expires_at`), and a composite that swallows one and returns
    /// a cacheable fallback is forced uncacheable (see `Invocation::issue` on failed
    /// sub-requests).
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
    /// A wait on something else ran out: a network wait, an upstream that did not
    /// answer in time. **Transient** — re-issuing an idempotent verb may succeed (see
    /// [`is_transient`](Error::is_transient)). A COMPUTATION that ran past a bound the
    /// caller holds costs the same and fails the same on a repeat, and is
    /// [`OverBudget`](Error::OverBudget) instead (since 0.1.108; the budgeted modules move
    /// to it in their own releases).
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
    /// `target` is the canonical name it asked for. Never cached, not even by an
    /// endpoint that propagates it after stating a failure expiry
    /// (`Invocation::failure_expires_at`): the cache key does not carry depth, so a
    /// cached refusal could reach a shallower request. And a composite that swallows this error
    /// and returns a cacheable fallback is forced uncacheable (see
    /// `Invocation::issue` on failed sub-requests).
    DepthExceeded {
        /// The depth the refused sub-request would have run at.
        depth: u32,
        /// The canonical target it asked for.
        target: Iri,
    },
    /// The same request may succeed, **but not before a stated time** (ledger #1546,
    /// #200): a rate limit, a saturated store that knows when a slot frees, a 503 that
    /// carried `Retry-After`, a peer held off for a known while, an open circuit with its
    /// cooldown left. Class [`RetryAfter`](FailureClass::RetryAfter).
    ///
    /// `delay` is how long to wait, from when the error was produced; `None` says "not
    /// now" without a time. A retry overlay waits and re-issues only when the delay fits
    /// inside its run's deadline and its budget has a retry left, and otherwise returns
    /// this unchanged, so the caller owns the wait; an HTTP edge answers 429 with
    /// `Retry-After`. Named for what a receiver DOES, not for the cause: a rate limit says
    /// "rate-limited" in its `detail`.
    ///
    /// ★ **Not [`is_transient`](Error::is_transient)**, deliberately: an overlay built
    /// before this variant existed treats it as permanent, which is the safe reading of a
    /// rate limit (an immediate re-issue goes straight back into it). New code reads
    /// [`class`](Error::class).
    ///
    /// ```
    /// use std::time::Duration;
    /// use ikigai_core::{Error, FailureClass};
    ///
    /// let limited = Error::RetryAfter {
    ///     delay: Some(Duration::from_millis(1500)),
    ///     detail: "rate-limited: 5 requests per minute".into(),
    /// };
    /// assert_eq!(limited.to_string(), "retry after 1500 ms: rate-limited: 5 requests per minute");
    /// assert_eq!(limited.class(), FailureClass::RetryAfter);
    /// assert!(!limited.is_transient());
    ///
    /// let later = Error::RetryAfter { delay: None, detail: "the store is saturated".into() };
    /// assert_eq!(later.to_string(), "retry later: the store is saturated");
    /// ```
    RetryAfter {
        /// How long to wait before re-issuing, when the producer knows.
        delay: Option<Duration>,
        /// What was refused, and why.
        detail: String,
    },
    /// This input exceeds a bound someone holds, and under the same bound it costs the
    /// same and fails the same (ledger #323, #972): a query past its time budget, a
    /// decompression past its size bound, fuel run out. Class
    /// [`OverBudget`](FailureClass::OverBudget). **Never retried** by anything in the
    /// chain: only a holder of a LARGER bound can clear it, with a new request.
    ///
    /// Distinct from [`Timeout`](Error::Timeout), which keeps the meaning "a wait on
    /// something else ran out" and is worth repeating; and from
    /// [`InvalidArgument`](Error::InvalidArgument), which blames an argument for what is
    /// the input's expansion.
    ///
    /// ```
    /// use ikigai_core::{Error, FailureClass};
    ///
    /// let err = Error::OverBudget { detail: "the query ran past its 5000 ms budget".into() };
    /// assert_eq!(err.to_string(), "over budget: the query ran past its 5000 ms budget");
    /// assert_eq!(err.class(), FailureClass::OverBudget);
    /// assert!(!err.is_transient());
    /// ```
    OverBudget {
        /// Which bound, and by how much.
        detail: String,
    },
    /// A **remote** authority refused the credential this endpoint presented (ledger
    /// #297): an origin's 401 or 403, a platform refusal outside the capability system.
    /// Class [`Refused`](FailureClass::Refused). Distinct from [`Denied`](Error::Denied),
    /// which stays the capability floor's word wherever the floor is enforced: "you may
    /// not ask" is the caller's authority, "the origin said no" is the endpoint's
    /// credential, and only the credential's holder (normally the endpoint itself) may
    /// renew it and re-issue, once.
    ///
    /// ```
    /// use ikigai_core::{Error, FailureClass};
    ///
    /// let err = Error::Refused("api.example.com answered 401".into());
    /// assert_eq!(err.to_string(), "refused: api.example.com answered 401");
    /// assert_eq!(err.class(), FailureClass::Refused);
    /// ```
    Refused(String),
    /// The target does not do this at all (ledger #855, #610): it does not answer the
    /// verb, or it needs a host facility the host did not provide (the kernel's clock, a
    /// tracer, a store). Class [`Permanent`](FailureClass::Permanent): a configuration
    /// fact, not something a retry changes.
    ///
    /// `verb` says which of the two it is, because an HTTP edge answers them
    /// differently: `Some(verb)` is "this target does not answer `verb`" (405, with an
    /// `Allow` built from the description), `None` a missing facility (501). The
    /// message alone could not tell an edge which, and the request's own verb is no
    /// substitute: a `Source` refused for want of a clock is a `Source` the target does
    /// answer.
    ///
    /// ```
    /// use ikigai_core::{Error, FailureClass, Verb};
    ///
    /// let verb = Error::Unsupported { verb: Some(Verb::Sink), detail: "log is append-only by Source".into() };
    /// let facility = Error::Unsupported { verb: None, detail: "the host configured no clock".into() };
    /// assert_eq!(verb.to_string(), "unsupported: log is append-only by Source");
    /// assert_eq!(facility.to_string(), "unsupported: the host configured no clock");
    /// assert_eq!(facility.class(), FailureClass::Permanent);
    /// ```
    Unsupported {
        /// The verb the target does not answer, or `None` for a missing host facility.
        verb: Option<Verb>,
        /// What is unsupported.
        detail: String,
    },
}

/// **Who may repeat a failure, and when** (ledger #200, #1546; the retry-authority note,
/// section 2). Seven classes, each a retry rule; an [`Error`]'s class is a pure function
/// of its variant ([`Error::class`]), so nothing can disagree with itself.
///
/// | class | meaning | who may repeat it |
/// | --- | --- | --- |
/// | [`Transient`](Self::Transient) | the same request may succeed now | an overlay holding retry budget, for an idempotent verb ([`Verb::is_idempotent`]) |
/// | [`RetryAfter`](Self::RetryAfter) | the same request may succeed, not before a stated time | an overlay holding budget AND time (the delay fits its deadline); otherwise the caller |
/// | [`OverBudget`](Self::OverBudget) | the input exceeds a bound; under the same bound it fails the same | nobody in the chain: only a holder of a larger bound, with a new request |
/// | [`Denied`](Self::Denied) | the caller's capability does not cover it | nobody: a different authority is a different request |
/// | [`Refused`](Self::Refused) | a remote authority refused the credential this endpoint presented | the credential's holder, once, after renewing it |
/// | [`Permanent`](Self::Permanent) | the request or the current state is wrong | nobody: a change of request or state clears it |
/// | [`Unknown`](Self::Unknown) | the endpoint could not say | nobody automatically |
///
/// `#[non_exhaustive]`: match with a wildcard arm, and treat a class you do not know as
/// [`Unknown`](Self::Unknown) (repeat nothing).
///
/// Serialized as its [`word`](Self::word):
///
/// ```
/// use ikigai_core::FailureClass;
///
/// let words: Vec<&str> = FailureClass::ALL.iter().map(|c| c.word()).collect();
/// assert_eq!(
///     words,
///     ["transient", "retry-after", "over-budget", "denied", "refused", "permanent", "unknown"]
/// );
/// assert_eq!(serde_json::to_string(&FailureClass::RetryAfter).unwrap(), "\"retry-after\"");
/// for class in FailureClass::ALL {
///     assert_eq!(FailureClass::from_word(class.word()), Some(class));
/// }
/// assert_eq!(FailureClass::from_word("Transient"), None);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum FailureClass {
    /// The same request may succeed now.
    Transient,
    /// The same request may succeed, not before a stated time.
    RetryAfter,
    /// This input exceeds a bound; under the same bound it costs the same and fails the
    /// same.
    OverBudget,
    /// The caller's capability does not cover it.
    Denied,
    /// A remote authority refused the credential the endpoint presented.
    Refused,
    /// The request or the current state is wrong.
    Permanent,
    /// The endpoint could not say.
    Unknown,
}

impl FailureClass {
    /// Every class, in the order of the table above.
    pub const ALL: [FailureClass; 7] = [
        FailureClass::Transient,
        FailureClass::RetryAfter,
        FailureClass::OverBudget,
        FailureClass::Denied,
        FailureClass::Refused,
        FailureClass::Permanent,
        FailureClass::Unknown,
    ];

    /// The class as one stable lowercase word: its serialized form, and what a trace
    /// note, a vector file and a face say for it.
    pub fn word(self) -> &'static str {
        match self {
            FailureClass::Transient => "transient",
            FailureClass::RetryAfter => "retry-after",
            FailureClass::OverBudget => "over-budget",
            FailureClass::Denied => "denied",
            FailureClass::Refused => "refused",
            FailureClass::Permanent => "permanent",
            FailureClass::Unknown => "unknown",
        }
    }

    /// The class a [`word`](Self::word) names, exactly as written; `None` for any other
    /// string.
    pub fn from_word(word: &str) -> Option<FailureClass> {
        FailureClass::ALL
            .into_iter()
            .find(|class| class.word() == word)
    }
}

impl fmt::Display for FailureClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

impl Error {
    /// Whether re-issuing might succeed: `true` for **transient** failures (timeout,
    /// unavailable), `false` for **permanent** ones (unresolved, bad argument,
    /// denied, or a domain endpoint error). The retry / circuit-breaker / failover
    /// overlays gate on this. Note the request *verb* separately governs whether a
    /// re-issue is *safe* ([`Verb::is_idempotent`]): a non-idempotent `Sink` needs an
    /// idempotency key even when the error is transient. `NotFound` and `Denied` are permanent — re-issuing
    /// won't conjure the resource or the grant — and so is `Conflict`: re-issuing
    /// against the same state gets the same refusal, so a retry overlay that re-sent it
    /// would only spend its budget. What clears a conflict is a change of state.
    ///
    /// Exactly `class() == FailureClass::Transient`. [`RetryAfter`](Error::RetryAfter)
    /// is NOT transient by this definition, on purpose (see the variant).
    pub fn is_transient(&self) -> bool {
        matches!(self, Error::Timeout(_) | Error::Unavailable(_))
    }

    /// **Who may repeat this failure, and when**: the error's [`FailureClass`], a pure
    /// function of the variant (ledger #1546, #200). An overlay decides from the class of
    /// the error it RECEIVED, never from what an action declared.
    ///
    /// ```
    /// use std::time::Duration;
    /// use ikigai_core::{Error, FailureClass, Iri, Verb};
    ///
    /// let classes = [
    ///     (Error::Unresolved(Iri::parse("urn:x").unwrap()), FailureClass::Permanent),
    ///     (Error::MissingArgument("in".into()), FailureClass::Permanent),
    ///     (Error::InvalidArgument { name: "in".into(), detail: "bad".into() }, FailureClass::Permanent),
    ///     (Error::Endpoint("boom".into()), FailureClass::Unknown),
    ///     (Error::Denied("no grant".into()), FailureClass::Denied),
    ///     (Error::NotFound("gone".into()), FailureClass::Permanent),
    ///     (Error::Conflict("taken".into()), FailureClass::Permanent),
    ///     (Error::Timeout("slow".into()), FailureClass::Transient),
    ///     (Error::Unavailable("down".into()), FailureClass::Transient),
    ///     (Error::DepthExceeded { depth: 65, target: Iri::parse("urn:x").unwrap() }, FailureClass::Permanent),
    ///     (Error::RetryAfter { delay: Some(Duration::from_secs(30)), detail: "limited".into() }, FailureClass::RetryAfter),
    ///     (Error::OverBudget { detail: "past 5000 ms".into() }, FailureClass::OverBudget),
    ///     (Error::Refused("401".into()), FailureClass::Refused),
    ///     (Error::Unsupported { verb: Some(Verb::Sink), detail: "read-only".into() }, FailureClass::Permanent),
    /// ];
    /// for (error, class) in classes {
    ///     assert_eq!(error.class(), class, "{error}");
    ///     assert_eq!(error.is_transient(), class == FailureClass::Transient, "{error}");
    /// }
    /// ```
    pub fn class(&self) -> FailureClass {
        match self {
            Error::Timeout(_) | Error::Unavailable(_) => FailureClass::Transient,
            Error::RetryAfter { .. } => FailureClass::RetryAfter,
            Error::OverBudget { .. } => FailureClass::OverBudget,
            Error::Denied(_) => FailureClass::Denied,
            Error::Refused(_) => FailureClass::Refused,
            Error::Endpoint(_) => FailureClass::Unknown,
            Error::Unresolved(_)
            | Error::MissingArgument(_)
            | Error::InvalidArgument { .. }
            | Error::NotFound(_)
            | Error::Conflict(_)
            | Error::DepthExceeded { .. }
            | Error::Unsupported { .. } => FailureClass::Permanent,
        }
    }

    /// A [`RetryAfter`](Error::RetryAfter)'s delay in **whole milliseconds, rounded
    /// up**: the form it crosses a wire in (`Option<u64>`) and the one its display
    /// prints. Rounded up because a delay rounded down tells the caller to come back
    /// early. `None` for every other variant, and for a `RetryAfter` with no delay.
    ///
    /// ```
    /// use std::time::Duration;
    /// use ikigai_core::Error;
    ///
    /// let after = |delay| Error::RetryAfter { delay: Some(delay), detail: String::new() };
    /// assert_eq!(after(Duration::from_secs(30)).retry_after_millis(), Some(30_000));
    /// assert_eq!(after(Duration::from_micros(1_200)).retry_after_millis(), Some(2));
    /// assert_eq!(after(Duration::ZERO).retry_after_millis(), Some(0));
    /// assert_eq!(Error::RetryAfter { delay: None, detail: String::new() }.retry_after_millis(), None);
    /// assert_eq!(Error::Timeout("slow".into()).retry_after_millis(), None);
    /// ```
    pub fn retry_after_millis(&self) -> Option<u64> {
        let Error::RetryAfter {
            delay: Some(delay), ..
        } = self
        else {
            return None;
        };
        let nanos = delay.as_nanos();
        let millis = nanos.div_ceil(1_000_000);
        Some(u64::try_from(millis).unwrap_or(u64::MAX))
    }

    /// The error's **kind** as one stable lowercase word, carrying nothing from its
    /// payload: no message, no argument name, no IRI, no depth. It is what the kernel
    /// reports under [`FAILED_NOTE`](crate::FAILED_NOTE) when an invocation fails,
    /// and it is safe to hand any observer because nothing a caller supplied, and
    /// nothing an endpoint wrote into its message (a path, a host), can ride along.
    ///
    /// The words, one per variant:
    ///
    /// ```
    /// use ikigai_core::{Error, Iri};
    ///
    /// let kinds = [
    ///     (Error::Unresolved(Iri::parse("urn:x").unwrap()), "unresolved"),
    ///     (Error::MissingArgument("in".into()), "missing-argument"),
    ///     (Error::InvalidArgument { name: "in".into(), detail: "bad".into() }, "invalid-argument"),
    ///     (Error::Endpoint("boom".into()), "endpoint"),
    ///     (Error::Denied("/etc/secret".into()), "denied"),
    ///     (Error::NotFound("gone".into()), "not-found"),
    ///     (Error::Conflict("taken".into()), "conflict"),
    ///     (Error::Timeout("slow".into()), "timeout"),
    ///     (Error::Unavailable("down".into()), "unavailable"),
    ///     (Error::DepthExceeded { depth: 65, target: Iri::parse("urn:x").unwrap() }, "depth-exceeded"),
    ///     (Error::RetryAfter { delay: None, detail: "limited".into() }, "retry-after"),
    ///     (Error::OverBudget { detail: "past 5000 ms".into() }, "over-budget"),
    ///     (Error::Refused("401".into()), "refused"),
    ///     (Error::Unsupported { verb: None, detail: "no clock".into() }, "unsupported"),
    /// ];
    /// for (error, word) in kinds {
    ///     assert_eq!(error.kind(), word);
    /// }
    /// ```
    ///
    /// A kind is not a [`class`](Self::class): several kinds share one (`not-found`,
    /// `conflict` and `unsupported` are all permanent), and a word here names the
    /// variant, while a class names a retry rule.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Unresolved(_) => "unresolved",
            Error::MissingArgument(_) => "missing-argument",
            Error::InvalidArgument { .. } => "invalid-argument",
            Error::Endpoint(_) => "endpoint",
            Error::Denied(_) => "denied",
            Error::NotFound(_) => "not-found",
            Error::Conflict(_) => "conflict",
            Error::Timeout(_) => "timeout",
            Error::Unavailable(_) => "unavailable",
            Error::DepthExceeded { .. } => "depth-exceeded",
            Error::RetryAfter { .. } => "retry-after",
            Error::OverBudget { .. } => "over-budget",
            Error::Refused(_) => "refused",
            Error::Unsupported { .. } => "unsupported",
        }
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
            Error::RetryAfter {
                delay: None,
                detail,
            } => write!(f, "retry later: {detail}"),
            Error::RetryAfter { detail, .. } => {
                let millis = self.retry_after_millis().unwrap_or_default();
                write!(f, "retry after {millis} ms: {detail}")
            }
            Error::OverBudget { detail } => write!(f, "over budget: {detail}"),
            Error::Refused(msg) => write!(f, "refused: {msg}"),
            Error::Unsupported { detail, .. } => write!(f, "unsupported: {detail}"),
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

    /// The four variants of ledger #1546 are each typed apart from the strings they
    /// replace, and none of them is transient: an overlay written against
    /// `is_transient` alone keeps treating them as permanent.
    #[test]
    fn the_new_variants_are_never_transient_and_each_has_its_class() {
        let cases = [
            (
                Error::RetryAfter {
                    delay: Some(Duration::from_secs(1)),
                    detail: "limited".into(),
                },
                FailureClass::RetryAfter,
                "retry-after",
            ),
            (
                Error::OverBudget {
                    detail: "past 5000 ms".into(),
                },
                FailureClass::OverBudget,
                "over-budget",
            ),
            (
                Error::Refused("403".into()),
                FailureClass::Refused,
                "refused",
            ),
            (
                Error::Unsupported {
                    verb: Some(Verb::Delete),
                    detail: "append-only".into(),
                },
                FailureClass::Permanent,
                "unsupported",
            ),
        ];
        for (error, class, kind) in cases {
            assert!(!error.is_transient(), "{error}");
            assert_eq!(error.class(), class, "{error}");
            assert_eq!(error.kind(), kind, "{error}");
        }
        // An endpoint error is the one class that says "it could not say".
        assert_eq!(
            Error::Endpoint("boom".into()).class(),
            FailureClass::Unknown
        );
        assert_eq!(
            Error::DepthExceeded {
                depth: 65,
                target: Iri::parse("urn:x").unwrap()
            }
            .class(),
            FailureClass::Permanent
        );
    }

    /// A sub-millisecond remainder rounds the displayed and wire delay UP, never down,
    /// and an absurd delay saturates rather than wrapping.
    #[test]
    fn a_retry_delay_rounds_up_and_saturates() {
        let after = |delay| Error::RetryAfter {
            delay: Some(delay),
            detail: "x".into(),
        };
        assert_eq!(
            after(Duration::from_nanos(1)).to_string(),
            "retry after 1 ms: x"
        );
        assert_eq!(
            after(Duration::from_millis(999)).retry_after_millis(),
            Some(999)
        );
        assert_eq!(after(Duration::MAX).retry_after_millis(), Some(u64::MAX));
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
