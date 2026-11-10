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
    /// The named thing existed and was deliberately retired: the tombstone's sibling of
    /// [`NotFound`](Error::NotFound), and the HTTP analog is **410 Gone**. **Permanent**
    /// (see [`is_transient`](Error::is_transient)). Carries a message only. Never cached,
    /// like every error, unless its endpoint states a failure expiry
    /// (`Invocation::failure_expires_at`).
    ///
    /// **Across the wire, for now.** Until the wire taxonomy gains a tag for it (a
    /// wire-version event, not this crate's), a remote caller receives this as
    /// [`Endpoint`](Error::Endpoint) carrying the displayed message, exactly as
    /// [`Conflict`](Error::Conflict) documents for itself.
    ///
    /// ```
    /// use ikigai_core::{Error, FailureClass};
    ///
    /// let err = Error::Gone("urn:name:old was retired in 2024".into());
    /// assert_eq!(err.to_string(), "gone: urn:name:old was retired in 2024");
    /// assert_eq!(err.kind(), "gone");
    /// assert_eq!(err.class(), FailureClass::Permanent);
    /// assert!(!err.is_transient());
    /// ```
    Gone(String),
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
    /// **Which refusal is which.** Five variants refuse a request an author might
    /// confuse, and each answers a different question:
    ///
    /// - [`InvalidArgument`](Error::InvalidArgument) — *is the argument itself usable?*
    ///   Wrong spelling, wrong type, out of range, not one of the allowed values. The
    ///   fault is in the request, and naming the argument is the whole message.
    /// - [`NotFound`](Error::NotFound) — *does the fronted thing exist?* It is not here.
    /// - [`Gone`](Error::Gone) — *did it ever?* It was here and was retired.
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
    /// A retry overlay **gave up**: it re-issued a transiently failing request
    /// `attempts` times over `elapsed` and stopped, because its budget or its deadline ran
    /// out (ledger #3386, #3380). Class [`Transient`](FailureClass::Transient), like the
    /// last error it stands for: the dependency was away, not refusing, and a caller may
    /// come back later. An overlay outside it that holds its own budget may repeat it
    /// exactly as it would have repeated that last error, which is what an overlay
    /// returned before this variant existed; the run's shared retry pool, not the class,
    /// is what stops attempts multiplying.
    ///
    /// Typed, with both numbers as fields, so a caller can tell "the hub was away for two
    /// minutes after 9 attempts" from a refusal or a single timeout without reading the
    /// message. `detail` is the last attempt's error as it displayed.
    ///
    /// A new variant rather than fields on [`Unavailable`](Error::Unavailable) or
    /// [`Timeout`](Error::Timeout): those are one-field variants every downstream crate
    /// builds and matches as `Unavailable(message)`, and a field added to them would break
    /// each of those; a new variant breaks none, because `Error` is `#[non_exhaustive]`.
    /// And a single wait that ran out is not a give-up: the numbers would be absent more
    /// often than present.
    ///
    /// ```
    /// use std::time::Duration;
    /// use ikigai_core::{Error, FailureClass};
    ///
    /// let err = Error::GaveUp {
    ///     attempts: 9,
    ///     elapsed: Duration::from_secs(120),
    ///     detail: "unavailable: the hub refused the connection".into(),
    /// };
    /// assert_eq!(
    ///     err.to_string(),
    ///     "gave up after 9 attempts over 120000 ms: unavailable: the hub refused the connection"
    /// );
    /// assert_eq!(err.kind(), "gave-up");
    /// assert_eq!(err.class(), FailureClass::Transient);
    /// assert_eq!(err.attempts(), Some(9));
    /// assert_eq!(err.elapsed_millis(), Some(120_000));
    /// ```
    GaveUp {
        /// How many times the request was issued, the first one included.
        attempts: u32,
        /// How long the overlay spent, from the first attempt until it gave up, waits
        /// included.
        elapsed: Duration,
        /// The last attempt's error, as it displayed.
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
    /// unavailable, an overlay that gave up), `false` for **permanent** ones (unresolved, bad argument,
    /// denied, or a domain endpoint error). The retry / circuit-breaker / failover
    /// overlays gate on this. Note the request *verb* separately governs whether a
    /// re-issue is *safe* ([`Verb::is_idempotent`]): a non-idempotent `Sink` needs an
    /// idempotency key even when the error is transient. `NotFound`, `Gone` and `Denied` are permanent — re-issuing
    /// won't conjure the resource or the grant — and so is `Conflict`: re-issuing
    /// against the same state gets the same refusal, so a retry overlay that re-sent it
    /// would only spend its budget. What clears a conflict is a change of state.
    ///
    /// Exactly `class() == FailureClass::Transient`. [`RetryAfter`](Error::RetryAfter)
    /// is NOT transient by this definition, on purpose (see the variant).
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Error::Timeout(_) | Error::Unavailable(_) | Error::GaveUp { .. }
        )
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
    ///     (Error::NotFound("absent".into()), FailureClass::Permanent),
    ///     (Error::Gone("retired".into()), FailureClass::Permanent),
    ///     (Error::Conflict("taken".into()), FailureClass::Permanent),
    ///     (Error::Timeout("slow".into()), FailureClass::Transient),
    ///     (Error::Unavailable("down".into()), FailureClass::Transient),
    ///     (Error::DepthExceeded { depth: 65, target: Iri::parse("urn:x").unwrap() }, FailureClass::Permanent),
    ///     (Error::RetryAfter { delay: Some(Duration::from_secs(30)), detail: "limited".into() }, FailureClass::RetryAfter),
    ///     (Error::OverBudget { detail: "past 5000 ms".into() }, FailureClass::OverBudget),
    ///     (Error::Refused("401".into()), FailureClass::Refused),
    ///     (Error::Unsupported { verb: Some(Verb::Sink), detail: "read-only".into() }, FailureClass::Permanent),
    ///     (Error::GaveUp { attempts: 9, elapsed: Duration::from_secs(120), detail: "down".into() }, FailureClass::Transient),
    /// ];
    /// for (error, class) in classes {
    ///     assert_eq!(error.class(), class, "{error}");
    ///     assert_eq!(error.is_transient(), class == FailureClass::Transient, "{error}");
    /// }
    /// ```
    pub fn class(&self) -> FailureClass {
        match self {
            Error::Timeout(_) | Error::Unavailable(_) | Error::GaveUp { .. } => {
                FailureClass::Transient
            }
            Error::RetryAfter { .. } => FailureClass::RetryAfter,
            Error::OverBudget { .. } => FailureClass::OverBudget,
            Error::Denied(_) => FailureClass::Denied,
            Error::Refused(_) => FailureClass::Refused,
            Error::Endpoint(_) => FailureClass::Unknown,
            Error::Unresolved(_)
            | Error::MissingArgument(_)
            | Error::InvalidArgument { .. }
            | Error::NotFound(_)
            | Error::Gone(_)
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

    /// A [`GaveUp`](Error::GaveUp)'s attempt count: how many times the overlay issued the
    /// request before it gave up. `None` for every other variant.
    ///
    /// ```
    /// use std::time::Duration;
    /// use ikigai_core::Error;
    ///
    /// let gave_up = Error::GaveUp { attempts: 3, elapsed: Duration::from_secs(2), detail: String::new() };
    /// assert_eq!(gave_up.attempts(), Some(3));
    /// assert_eq!(Error::Unavailable("down".into()).attempts(), None);
    /// ```
    pub fn attempts(&self) -> Option<u32> {
        match self {
            Error::GaveUp { attempts, .. } => Some(*attempts),
            _ => None,
        }
    }

    /// A [`GaveUp`](Error::GaveUp)'s elapsed time in **whole milliseconds, rounded
    /// down**: the form its display prints and its faces carry. Rounded down because it
    /// reports a time spent, and the overlay spent at least that long. Saturates at
    /// `u64::MAX`. `None` for every other variant.
    ///
    /// ```
    /// use std::time::Duration;
    /// use ikigai_core::Error;
    ///
    /// let spent = |elapsed| Error::GaveUp { attempts: 2, elapsed, detail: String::new() };
    /// assert_eq!(spent(Duration::from_secs(120)).elapsed_millis(), Some(120_000));
    /// assert_eq!(spent(Duration::from_micros(1_900)).elapsed_millis(), Some(1));
    /// assert_eq!(Error::Timeout("slow".into()).elapsed_millis(), None);
    /// ```
    pub fn elapsed_millis(&self) -> Option<u64> {
        let Error::GaveUp { elapsed, .. } = self else {
            return None;
        };
        Some(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
    }

    /// The error's **JSON face**: an object with its [`kind`](Self::kind) word and its
    /// payload as fields, the shape the shared failure-class vectors carry in each row's
    /// `json` (`crates/ikigai-vocab/tests/vectors`), so a face in another language can
    /// read every number without parsing a message. [`from_json`](Self::from_json) reads
    /// it back.
    ///
    /// | kind | fields beside `kind` |
    /// | --- | --- |
    /// | `unresolved` | `iri` |
    /// | `missing-argument` | `name` |
    /// | `invalid-argument` | `name`, `detail` |
    /// | `endpoint`, `denied`, `not-found`, `gone`, `conflict`, `timeout`, `unavailable`, `refused` | `message` |
    /// | `depth-exceeded` | `depth`, `target` |
    /// | `retry-after` | `retry_after_ms` (absent without a delay), `detail` |
    /// | `over-budget` | `detail` |
    /// | `unsupported` | `verb` (absent for a missing facility), `detail` |
    /// | `gave-up` | `attempts`, `elapsed_ms`, `detail` |
    ///
    /// Durations cross in whole milliseconds, as [`retry_after_millis`](Self::retry_after_millis)
    /// and [`elapsed_millis`](Self::elapsed_millis) round them, so a sub-millisecond
    /// remainder does not survive the trip; everything else does.
    ///
    /// ```
    /// use std::time::Duration;
    /// use ikigai_core::Error;
    ///
    /// let err = Error::GaveUp {
    ///     attempts: 9,
    ///     elapsed: Duration::from_secs(120),
    ///     detail: "unavailable: the hub refused the connection".into(),
    /// };
    /// let json = err.to_json();
    /// assert_eq!(
    ///     json,
    ///     serde_json::json!({
    ///         "kind": "gave-up",
    ///         "attempts": 9,
    ///         "elapsed_ms": 120000,
    ///         "detail": "unavailable: the hub refused the connection",
    ///     })
    /// );
    /// assert_eq!(Error::from_json(&json).unwrap(), err);
    /// ```
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::{json, Value};
        let mut face = match self {
            Error::Unresolved(iri) => json!({ "iri": iri.as_str() }),
            Error::MissingArgument(name) => json!({ "name": name }),
            Error::InvalidArgument { name, detail } => json!({ "name": name, "detail": detail }),
            Error::Endpoint(message)
            | Error::Denied(message)
            | Error::NotFound(message)
            | Error::Gone(message)
            | Error::Conflict(message)
            | Error::Timeout(message)
            | Error::Unavailable(message)
            | Error::Refused(message) => json!({ "message": message }),
            Error::DepthExceeded { depth, target } => {
                json!({ "depth": depth, "target": target.as_str() })
            }
            Error::RetryAfter { detail, .. } => match self.retry_after_millis() {
                Some(ms) => json!({ "retry_after_ms": ms, "detail": detail }),
                None => json!({ "detail": detail }),
            },
            Error::OverBudget { detail } => json!({ "detail": detail }),
            Error::Unsupported { verb, detail } => match verb {
                Some(verb) => json!({ "verb": verb, "detail": detail }),
                None => json!({ "detail": detail }),
            },
            Error::GaveUp {
                attempts, detail, ..
            } => json!({
                "attempts": attempts,
                "elapsed_ms": self.elapsed_millis(),
                "detail": detail,
            }),
        };
        if let Value::Object(fields) = &mut face {
            fields.insert("kind".into(), Value::from(self.kind()));
        }
        face
    }

    /// The error's **Turtle face**: one `ik:Failure` node (a blank node: an error has no
    /// name of its own) stating its `ik:failureKind` word, its `ik:failureClass`
    /// individual, its displayed sentence as `ik:failureMessage`, and its numbers as typed
    /// counts: `ik:attempts` and `ik:elapsedMillis` for [`GaveUp`](Error::GaveUp),
    /// `ik:retryAfterMillis` for a [`RetryAfter`](Error::RetryAfter) with a delay. A graph
    /// reader gets the numbers without parsing the sentence. It describes the error rather
    /// than encoding it, so read an error back from its [JSON face](Self::to_json).
    ///
    /// ```
    /// use std::time::Duration;
    /// use ikigai_core::Error;
    ///
    /// let err = Error::GaveUp { attempts: 9, elapsed: Duration::from_secs(120), detail: "unavailable: down".into() };
    /// assert_eq!(
    ///     err.to_turtle(),
    ///     "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
    ///      @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\n\
    ///      [] a ik:Failure ;\n    \
    ///      ik:failureKind \"gave-up\" ;\n    \
    ///      ik:failureClass ik:TransientFailure ;\n    \
    ///      ik:failureMessage \"gave up after 9 attempts over 120000 ms: unavailable: down\" ;\n    \
    ///      ik:attempts \"9\"^^xsd:nonNegativeInteger ;\n    \
    ///      ik:elapsedMillis \"120000\"^^xsd:nonNegativeInteger .\n"
    /// );
    /// ```
    pub fn to_turtle(&self) -> String {
        use std::fmt::Write as _;

        let class: String = self
            .class()
            .word()
            .split('-')
            .flat_map(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().chain(chars).collect::<String>())
            })
            .collect();
        let mut ttl = format!(
            "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
             @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\n\
             [] a ik:Failure ;\n    \
             ik:failureKind \"{}\" ;\n    \
             ik:failureClass ik:{class}Failure ;\n    \
             ik:failureMessage \"{}\"",
            self.kind(),
            crate::topology::escape_literal(&self.to_string()),
        );
        let counts = [
            ("attempts", self.attempts().map(u64::from)),
            ("elapsedMillis", self.elapsed_millis()),
            ("retryAfterMillis", self.retry_after_millis()),
        ];
        for (property, value) in counts {
            if let Some(value) = value {
                let _ = write!(
                    ttl,
                    " ;\n    ik:{property} \"{value}\"^^xsd:nonNegativeInteger"
                );
            }
        }
        ttl.push_str(" .\n");
        ttl
    }

    /// Read an error back from its [JSON face](Self::to_json). Refuses, as
    /// [`InvalidArgument`](Error::InvalidArgument) naming the field, a value that is not an
    /// object, a `kind` no variant has, and a field that is absent or of the wrong type.
    /// Fields the kind does not use are ignored.
    ///
    /// ```
    /// use ikigai_core::Error;
    ///
    /// let json = serde_json::json!({"kind": "gave-up", "attempts": "nine", "elapsed_ms": 1, "detail": ""});
    /// assert_eq!(
    ///     Error::from_json(&json).unwrap_err().to_string(),
    ///     "invalid argument `attempts`: expected a whole number from 0 to 4294967295"
    /// );
    /// ```
    pub fn from_json(value: &serde_json::Value) -> Result<Error> {
        let invalid = |name: &str, detail: &str| Error::InvalidArgument {
            name: name.into(),
            detail: detail.into(),
        };
        if !value.is_object() {
            return Err(invalid("error", "expected a JSON object"));
        }
        let text = |field: &str| -> Result<String> {
            value[field]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid(field, "expected a string"))
        };
        let whole = |field: &str| -> Result<u64> {
            value[field]
                .as_u64()
                .ok_or_else(|| invalid(field, "expected a whole number"))
        };
        let millis = |field: &str| -> Result<Option<Duration>> {
            match value.get(field) {
                None | Some(serde_json::Value::Null) => Ok(None),
                Some(_) => whole(field).map(|ms| Some(Duration::from_millis(ms))),
            }
        };
        let iri = |field: &str| -> Result<Iri> {
            Iri::parse(text(field)?).map_err(|e| invalid(field, &e.to_string()))
        };
        let kind = text("kind")?;
        Ok(match kind.as_str() {
            "unresolved" => Error::Unresolved(iri("iri")?),
            "missing-argument" => Error::MissingArgument(text("name")?),
            "invalid-argument" => Error::InvalidArgument {
                name: text("name")?,
                detail: text("detail")?,
            },
            "endpoint" => Error::Endpoint(text("message")?),
            "denied" => Error::Denied(text("message")?),
            "not-found" => Error::NotFound(text("message")?),
            "gone" => Error::Gone(text("message")?),
            "conflict" => Error::Conflict(text("message")?),
            "timeout" => Error::Timeout(text("message")?),
            "unavailable" => Error::Unavailable(text("message")?),
            "refused" => Error::Refused(text("message")?),
            "depth-exceeded" => Error::DepthExceeded {
                depth: u32::try_from(whole("depth")?).map_err(|_| {
                    invalid("depth", "expected a whole number from 0 to 4294967295")
                })?,
                target: iri("target")?,
            },
            "retry-after" => Error::RetryAfter {
                delay: millis("retry_after_ms")?,
                detail: text("detail")?,
            },
            "over-budget" => Error::OverBudget {
                detail: text("detail")?,
            },
            "unsupported" => Error::Unsupported {
                verb: match value.get("verb") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(verb) => Some(
                        serde_json::from_value::<Verb>(verb.clone())
                            .map_err(|e| invalid("verb", &e.to_string()))?,
                    ),
                },
                detail: text("detail")?,
            },
            "gave-up" => Error::GaveUp {
                attempts: value["attempts"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| {
                        invalid("attempts", "expected a whole number from 0 to 4294967295")
                    })?,
                elapsed: Duration::from_millis(whole("elapsed_ms")?),
                detail: text("detail")?,
            },
            other => {
                return Err(invalid(
                    "kind",
                    &format!("`{other}` is not the kind of any error"),
                ))
            }
        })
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
    /// use std::time::Duration;
    /// use ikigai_core::{Error, Iri};
    ///
    /// let kinds = [
    ///     (Error::Unresolved(Iri::parse("urn:x").unwrap()), "unresolved"),
    ///     (Error::MissingArgument("in".into()), "missing-argument"),
    ///     (Error::InvalidArgument { name: "in".into(), detail: "bad".into() }, "invalid-argument"),
    ///     (Error::Endpoint("boom".into()), "endpoint"),
    ///     (Error::Denied("/etc/secret".into()), "denied"),
    ///     (Error::NotFound("absent".into()), "not-found"),
    ///     (Error::Gone("retired".into()), "gone"),
    ///     (Error::Conflict("taken".into()), "conflict"),
    ///     (Error::Timeout("slow".into()), "timeout"),
    ///     (Error::Unavailable("down".into()), "unavailable"),
    ///     (Error::DepthExceeded { depth: 65, target: Iri::parse("urn:x").unwrap() }, "depth-exceeded"),
    ///     (Error::RetryAfter { delay: None, detail: "limited".into() }, "retry-after"),
    ///     (Error::OverBudget { detail: "past 5000 ms".into() }, "over-budget"),
    ///     (Error::Refused("401".into()), "refused"),
    ///     (Error::Unsupported { verb: None, detail: "no clock".into() }, "unsupported"),
    ///     (Error::GaveUp { attempts: 9, elapsed: Duration::from_secs(120), detail: "down".into() }, "gave-up"),
    /// ];
    /// for (error, word) in kinds {
    ///     assert_eq!(error.kind(), word);
    /// }
    /// ```
    ///
    /// A kind is not a [`class`](Self::class): several kinds share one (`not-found`,
    /// `gone`, `conflict` and `unsupported` are all permanent), and a word here names the
    /// variant, while a class names a retry rule.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Unresolved(_) => "unresolved",
            Error::MissingArgument(_) => "missing-argument",
            Error::InvalidArgument { .. } => "invalid-argument",
            Error::Endpoint(_) => "endpoint",
            Error::Denied(_) => "denied",
            Error::NotFound(_) => "not-found",
            Error::Gone(_) => "gone",
            Error::Conflict(_) => "conflict",
            Error::Timeout(_) => "timeout",
            Error::Unavailable(_) => "unavailable",
            Error::DepthExceeded { .. } => "depth-exceeded",
            Error::RetryAfter { .. } => "retry-after",
            Error::OverBudget { .. } => "over-budget",
            Error::Refused(_) => "refused",
            Error::Unsupported { .. } => "unsupported",
            Error::GaveUp { .. } => "gave-up",
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
            Error::Gone(msg) => write!(f, "gone: {msg}"),
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
            Error::GaveUp {
                attempts, detail, ..
            } => {
                let noun = if *attempts == 1 {
                    "attempt"
                } else {
                    "attempts"
                };
                let millis = self.elapsed_millis().unwrap_or_default();
                write!(
                    f,
                    "gave up after {attempts} {noun} over {millis} ms: {detail}"
                )
            }
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
