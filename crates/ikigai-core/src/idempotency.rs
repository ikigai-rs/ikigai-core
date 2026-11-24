//! The **idempotency-key toolkit** for `Sink` endpoints (ledger #4056, from the research note
//! `docs/research/sink-idempotency.md`, section 5).
//!
//! A client that sent a write and does not know whether it landed (a timeout, a dropped
//! connection) sends it again under the same token, the argument `idempotency-key=`. An
//! endpoint that honors the token answers the repeat with the FIRST answer, byte for byte, and
//! writes nothing. The contract says so with the existing
//! [`Resend::Key`](crate::Resend::Key) (`resend_safe_with_key("idempotency-key")`), so a
//! retrying overlay resends it with no change, and publishes how long a token is remembered
//! with [`ActionSpec::resend_window`](crate::ActionSpec::resend_window) (`ik:resendWindow`).
//!
//! The pieces each endpoint should not have to rewrite:
//!
//! - [`IdempotencyKey`]: the token, parsed with the ledger key's alphabet (ASCII letters,
//!   digits, `-._~:`, at most [`MAX_LEN`] characters);
//! - [`Request::fingerprint_without`](crate::Request::fingerprint_without): the request's
//!   identity with the token left out, which a repeat must match;
//! - [`Replay`]: the record an endpoint stores **in the same store update as its write**
//!   (never a kernel overlay or the cache), with a stable byte encoding.
//!
//! Core stores nothing, and no default behavior changes: an endpoint opts in.
//!
//! The rules the record follows, which an endpoint keeps:
//!
//! - **Scope**: per (endpoint resource, principal, token) ([`ReplayScope`]). A call with no
//!   principal shares one unnamed scope.
//! - **A different request under the same token** is refused with
//!   [`IdempotencyKey::mismatch`], an `InvalidArgument` naming the token, never answered as a
//!   match.
//! - **What is recorded**: a success, and a permanent failure the endpoint produced after it
//!   decided on the write, a `Conflict`, `NotFound` or `Gone` ([`ReplayOutcome`]). Never a refusal made before it ran (`Denied`,
//!   `InvalidArgument`, a limiter) or a transient failure: those wrote nothing, so a retry runs
//!   again ([`ReplayOutcome::records`]).
//! - **In flight**: a repeat while the first holds the token is answered
//!   [`IdempotencyKey::in_flight`], a `RetryAfter`.
//! - **Window**: at least [`MIN_WINDOW`] (24 hours), configurable; a token presented after
//!   its window is a new request ([`Replay::is_live`]).
//!
//! ```
//! use ikigai_core::idempotency::{IdempotencyKey, Replay, ReplayOutcome, ReplayScope};
//! use ikigai_core::{ArgRef, Iri, ReprType, Representation, Request, Time, Verb};
//!
//! let first = Request::new(Verb::Sink, Iri::parse("urn:ledger:main").unwrap())
//!     .with_arg("title", ArgRef::Inline(b"a bug".to_vec()))
//!     .with_arg("idempotency-key", ArgRef::Inline(b"retry-7f3a".to_vec()));
//! let key = IdempotencyKey::from_request(&first, "idempotency-key").unwrap().unwrap();
//! let answer = Representation::new(ReprType::new("text/plain"), b"#1 filed".to_vec());
//!
//! // Stored beside the write, in the same store update.
//! let record = Replay::success(
//!     ReplayScope::new("urn:ledger:main", None, key),
//!     first.fingerprint_without("idempotency-key"),
//!     &answer,
//!     Time::from_millis(1_000),
//! );
//! let stored = record.to_bytes();
//!
//! // The retry: same request, same token. It answers the first answer, byte for byte.
//! let back = Replay::from_bytes(&stored).unwrap();
//! assert!(back.matches(first.fingerprint_without("idempotency-key")));
//! assert_eq!(back.representation().unwrap().bytes, answer.bytes);
//! assert_eq!(back.outcome, ReplayOutcome::Success);
//! ```

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{ArgRef, ContentId, Error, ReprType, Representation, Request, RequestId, Time};

/// The argument name the research note settles on, matching the HTTP `Idempotency-Key` header
/// so nobody has to learn two names. Declare it with
/// `ActionSpec::resend_safe_with_key(IDEMPOTENCY_KEY_ARG)`.
pub const IDEMPOTENCY_KEY_ARG: &str = "idempotency-key";

/// The longest token accepted, in characters (the ledger key's bound).
pub const MAX_LEN: usize = 256;

/// The shortest window a token may be remembered for: 24 hours. A host may configure a longer
/// one, and publishes it as `ik:resendWindow`.
pub const MIN_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether `c` is in the ledger key's alphabet: ASCII letters, digits and `-._~:`.
fn allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~' | ':')
}

/// A request's idempotency token: one to [`MAX_LEN`] characters of the ledger key's alphabet
/// (ASCII letters, digits, `-._~:`), so it survives a path mapping unchanged.
///
/// ```
/// use ikigai_core::idempotency::IdempotencyKey;
///
/// assert_eq!(IdempotencyKey::parse("a-1.b_2~c:3").unwrap().as_str(), "a-1.b_2~c:3");
/// assert!(IdempotencyKey::parse("").is_err());
/// assert!(IdempotencyKey::parse("has space").is_err());
/// assert!(IdempotencyKey::parse(&"k".repeat(257)).is_err());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct IdempotencyKey(String);

/// Why a token was refused: it says what was wrong, and the [`Display`](fmt::Display) reads as
/// the detail of an `invalid argument` error.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdempotencyKeyError {
    /// The token was empty.
    Empty,
    /// The token was longer than [`MAX_LEN`] characters.
    TooLong {
        /// Its length, in characters.
        length: usize,
    },
    /// The token held a character outside the alphabet.
    Character {
        /// The first such character.
        found: char,
        /// Its position, counted in characters from 1.
        position: usize,
    },
    /// The token was given as content or bytes that are not UTF-8 text.
    NotText,
}

impl fmt::Display for IdempotencyKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdempotencyKeyError::Empty => f.write_str("an idempotency key cannot be empty"),
            IdempotencyKeyError::TooLong { length } => write!(
                f,
                "an idempotency key is at most {MAX_LEN} characters, and this one is {length}"
            ),
            IdempotencyKeyError::Character { found, position } => write!(
                f,
                "character {position} ({found:?}) is not allowed in an idempotency key, which \
                 takes ASCII letters, digits and - . _ ~ :"
            ),
            IdempotencyKeyError::NotText => f.write_str(
                "an idempotency key is given inline as text, not as content or a reference",
            ),
        }
    }
}

impl std::error::Error for IdempotencyKeyError {}

impl IdempotencyKey {
    /// Parse a token, refusing an empty one, one longer than [`MAX_LEN`] characters, and one
    /// with a character outside the alphabet.
    pub fn parse(s: &str) -> Result<Self, IdempotencyKeyError> {
        if s.is_empty() {
            return Err(IdempotencyKeyError::Empty);
        }
        if let Some((i, found)) = s.chars().enumerate().find(|(_, c)| !allowed(*c)) {
            return Err(IdempotencyKeyError::Character {
                found,
                position: i + 1,
            });
        }
        // Every allowed character is one ASCII byte, so the byte length is the count.
        if s.len() > MAX_LEN {
            return Err(IdempotencyKeyError::TooLong { length: s.len() });
        }
        Ok(IdempotencyKey(s.to_string()))
    }

    /// The token a request carries as its argument `name`: `None` when it carries none, and an
    /// `InvalidArgument` naming `name` when the value is not a valid token (or not given
    /// inline as text).
    pub fn from_request(request: &Request, name: &str) -> Result<Option<Self>, Error> {
        let Some(arg) = request.args.get(name) else {
            return Ok(None);
        };
        let parsed = match arg {
            ArgRef::Inline(bytes) => std::str::from_utf8(bytes)
                .map_err(|_| IdempotencyKeyError::NotText)
                .and_then(IdempotencyKey::parse),
            ArgRef::Reference(_) | ArgRef::Content(_) => Err(IdempotencyKeyError::NotText),
        };
        parsed.map(Some).map_err(|e| Error::InvalidArgument {
            name: name.to_string(),
            detail: e.to_string(),
        })
    }

    /// The token as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The refusal for a repeat whose fingerprint differs from the recorded one: an
    /// `InvalidArgument` naming the token's argument, never an answer. `name` is the argument
    /// (usually [`IDEMPOTENCY_KEY_ARG`]).
    pub fn mismatch(&self, name: &str) -> Error {
        Error::InvalidArgument {
            name: name.to_string(),
            detail: format!("`{}` was used for a different request", self.0),
        }
    }

    /// The answer for a repeat that arrives while the first attempt still holds the token: a
    /// `RetryAfter`, which is transient, so a retrying overlay waits and tries again.
    pub fn in_flight(&self, delay: Option<Duration>) -> Error {
        Error::RetryAfter {
            delay,
            detail: format!(
                "a request under the idempotency key `{}` is still in progress",
                self.0
            ),
        }
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<IdempotencyKey> for String {
    fn from(key: IdempotencyKey) -> String {
        key.0
    }
}

impl TryFrom<String> for IdempotencyKey {
    type Error = IdempotencyKeyError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        IdempotencyKey::parse(&s)
    }
}

impl std::str::FromStr for IdempotencyKey {
    type Err = IdempotencyKeyError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        IdempotencyKey::parse(s)
    }
}

/// Where a token is remembered: per endpoint resource, per principal and per token. Two
/// principals may use the same token without seeing each other's answer; a call with no
/// principal (`None`, as `Capability::principal` says for root) shares one unnamed scope.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ReplayScope {
    /// The endpoint resource the write was made to (the request's target IRI, or the
    /// endpoint's own name for it).
    pub endpoint: String,
    /// The principal that made it: `Capability::principal()`.
    pub principal: Option<String>,
    /// The token.
    pub key: IdempotencyKey,
}

impl ReplayScope {
    /// A scope for `key` at `endpoint` under `principal`.
    pub fn new(endpoint: impl Into<String>, principal: Option<&str>, key: IdempotencyKey) -> Self {
        ReplayScope {
            endpoint: endpoint.into(),
            principal: principal.map(str::to_string),
            key,
        }
    }

    /// The scope's stable digest, `b3:…`, which an endpoint may use as the record's key in
    /// its store. Built from the same length-prefixed fields [`Replay::to_bytes`] writes, so
    /// two scopes share a digest exactly when they are equal.
    pub fn content_id(&self) -> ContentId {
        let mut out = Vec::new();
        out.extend_from_slice(SCOPE_TAG);
        self.encode(&mut out);
        ContentId::of(&out)
    }

    fn encode(&self, out: &mut Vec<u8>) {
        put_str(out, &self.endpoint);
        match &self.principal {
            None => out.push(0),
            Some(p) => {
                out.push(1);
                put_str(out, p);
            }
        }
        put_str(out, self.key.as_str());
    }
}

/// What the first attempt came to, as recorded: a success, or one of the permanent failures
/// an endpoint produces about the STATE it decided on, each with its detail exactly, so a
/// repeat answers the same error ([`Replay::error`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ReplayOutcome {
    /// The write succeeded; a repeat answers the recorded representation.
    Success,
    /// [`Error::Conflict`]: the state did not permit it (a compare-and-set that lost).
    Conflict(String),
    /// [`Error::NotFound`]: what the write was about does not exist.
    NotFound(String),
    /// [`Error::Gone`]: what the write was about existed and is gone.
    Gone(String),
}

impl ReplayOutcome {
    /// The outcome recorded for a failure, or `None` when `error` is not recorded: only a
    /// permanent failure about the state ([`Conflict`](Error::Conflict),
    /// [`NotFound`](Error::NotFound), [`Gone`](Error::Gone)). A refusal made before the
    /// endpoint ran (`Denied`, `InvalidArgument`, `MissingArgument`, a limiter's
    /// `RetryAfter`, an unbound target) and a transient failure wrote nothing, so a retry
    /// must run again.
    pub fn of_failure(error: &Error) -> Option<Self> {
        match error {
            Error::Conflict(d) => Some(ReplayOutcome::Conflict(d.clone())),
            Error::NotFound(d) => Some(ReplayOutcome::NotFound(d.clone())),
            Error::Gone(d) => Some(ReplayOutcome::Gone(d.clone())),
            _ => None,
        }
    }

    /// Whether an attempt that failed with `error` is recorded (see
    /// [`of_failure`](Self::of_failure)).
    pub fn records(error: &Error) -> bool {
        Self::of_failure(error).is_some()
    }

    /// The error a repeat answers, for a recorded failure: the first one exactly. `None` for
    /// a success.
    pub fn error(&self) -> Option<Error> {
        match self {
            ReplayOutcome::Success => None,
            ReplayOutcome::Conflict(d) => Some(Error::Conflict(d.clone())),
            ReplayOutcome::NotFound(d) => Some(Error::NotFound(d.clone())),
            ReplayOutcome::Gone(d) => Some(Error::Gone(d.clone())),
        }
    }

    fn encode(&self, out: &mut Vec<u8>) {
        let (marker, detail) = match self {
            ReplayOutcome::Success => (0, None),
            ReplayOutcome::Conflict(d) => (1, Some(d)),
            ReplayOutcome::NotFound(d) => (2, Some(d)),
            ReplayOutcome::Gone(d) => (3, Some(d)),
        };
        out.push(marker);
        if let Some(d) = detail {
            put_str(out, d);
        }
    }
}

/// The record an endpoint stores beside a write made under an idempotency key, in the SAME
/// store update as the write (never a kernel overlay or the cache: neither commits with the
/// write, and both forget).
///
/// A repeat with the same [`scope`](Self::scope) answers it: the recorded
/// [`representation`](Self::representation) when it [`matches`](Self::matches) the repeat's
/// fingerprint, and [`IdempotencyKey::mismatch`] when it does not.
///
/// [`to_bytes`](Self::to_bytes) is a stable encoding: a versioned tag, then each field
/// length-prefixed in a fixed order, so a record written by one release reads in the next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replay {
    /// Where the token is remembered.
    pub scope: ReplayScope,
    /// The first request's [`Request::fingerprint_without`] the token.
    pub fingerprint: RequestId,
    /// The first answer's representation type (for a failure, `text/plain`).
    pub repr_type: ReprType,
    /// The first answer's bytes, exactly (for a failure, its message as displayed).
    pub bytes: Vec<u8>,
    /// What the first attempt came to.
    pub outcome: ReplayOutcome,
    /// When it was recorded, by the endpoint's clock.
    pub recorded_at: Time,
}

/// The encoding's tag: the format and its version. A new version gets a new tag.
const TAG: &[u8] = b"ikigai.replay.v1\0";
const SCOPE_TAG: &[u8] = b"ikigai.replay-scope.v1\0";

/// Why stored bytes did not decode as a [`Replay`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayDecodeError(String);

impl fmt::Display for ReplayDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a replay record: {}", self.0)
    }
}

impl std::error::Error for ReplayDecodeError {}

impl Replay {
    /// The record of a success: `answer`'s type and bytes, exactly.
    pub fn success(
        scope: ReplayScope,
        fingerprint: RequestId,
        answer: &Representation,
        recorded_at: Time,
    ) -> Self {
        Replay {
            scope,
            fingerprint,
            repr_type: answer.repr_type.clone(),
            bytes: answer.bytes.clone(),
            outcome: ReplayOutcome::Success,
            recorded_at,
        }
    }

    /// The record of a permanent failure the endpoint produced after it decided on the write,
    /// or `None` when `error` must not be recorded (see [`ReplayOutcome::records`]).
    pub fn failure(
        scope: ReplayScope,
        fingerprint: RequestId,
        error: &Error,
        recorded_at: Time,
    ) -> Option<Self> {
        let outcome = ReplayOutcome::of_failure(error)?;
        Some(Replay {
            scope,
            fingerprint,
            repr_type: ReprType::new("text/plain").with_param("charset", "utf-8"),
            bytes: error.to_string().into_bytes(),
            outcome,
            recorded_at,
        })
    }

    /// Whether a repeat with `fingerprint` is the same request the record was made for.
    pub fn matches(&self, fingerprint: RequestId) -> bool {
        self.fingerprint == fingerprint
    }

    /// Whether the record still answers at `now` under a window of `window`: a token
    /// presented after its window is a new request. A window shorter than [`MIN_WINDOW`] is
    /// read as [`MIN_WINDOW`].
    pub fn is_live(&self, now: Time, window: Duration) -> bool {
        let window = window.max(MIN_WINDOW);
        let millis = u64::try_from(window.as_millis()).unwrap_or(u64::MAX);
        now.as_millis() < self.recorded_at.as_millis().saturating_add(millis)
    }

    /// The first answer, for a recorded success: its type and bytes, exactly, with no marker
    /// saying it is a replay. `None` for a recorded failure (see [`error`](Self::error)).
    pub fn representation(&self) -> Option<Representation> {
        match self.outcome {
            ReplayOutcome::Success => Some(Representation::new(
                self.repr_type.clone(),
                self.bytes.clone(),
            )),
            _ => None,
        }
    }

    /// The first answer, for a recorded failure: the same error, exactly. `None` for a
    /// success.
    pub fn error(&self) -> Option<Error> {
        self.outcome.error()
    }

    /// The record's stable encoding: [`TAG`](Replay), the scope, the fingerprint's 32 bytes,
    /// the media type and its sorted parameters, the bytes, the outcome and the recorded time,
    /// each variable-length field prefixed by its length as a little-endian `u64`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(TAG.len() + self.bytes.len() + 128);
        out.extend_from_slice(TAG);
        self.scope.encode(&mut out);
        out.extend_from_slice(self.fingerprint.content_id().as_bytes());
        put_str(&mut out, &self.repr_type.media_type);
        out.extend_from_slice(&(self.repr_type.params.len() as u64).to_le_bytes());
        for (k, v) in &self.repr_type.params {
            put_str(&mut out, k);
            put_str(&mut out, v);
        }
        put_bytes(&mut out, &self.bytes);
        self.outcome.encode(&mut out);
        out.extend_from_slice(&self.recorded_at.as_millis().to_le_bytes());
        out
    }

    /// Decode [`to_bytes`](Self::to_bytes), refusing anything else: a wrong tag, a truncated
    /// field, an invalid token, or bytes left over.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ReplayDecodeError> {
        let mut r = Reader(bytes);
        if r.take(TAG.len())? != TAG {
            return Err(ReplayDecodeError("unknown tag or version".into()));
        }
        let endpoint = r.string()?;
        let principal = match r.byte()? {
            0 => None,
            1 => Some(r.string()?),
            b => return Err(ReplayDecodeError(format!("principal marker {b}"))),
        };
        let key =
            IdempotencyKey::parse(&r.string()?).map_err(|e| ReplayDecodeError(e.to_string()))?;
        let digest: [u8; 32] = r.take(32)?.try_into().expect("32 bytes");
        let fingerprint = RequestId::from_content_id(ContentId::from_digest(digest));
        let mut repr_type = ReprType::new(r.string()?);
        for _ in 0..r.u64()? {
            let k = r.string()?;
            let v = r.string()?;
            repr_type.params.insert(k, v);
        }
        let body = r.bytes()?.to_vec();
        let outcome = match r.byte()? {
            0 => ReplayOutcome::Success,
            1 => ReplayOutcome::Conflict(r.string()?),
            2 => ReplayOutcome::NotFound(r.string()?),
            3 => ReplayOutcome::Gone(r.string()?),
            b => return Err(ReplayDecodeError(format!("outcome marker {b}"))),
        };
        let recorded_at = Time::from_millis(r.u64()?);
        if !r.0.is_empty() {
            return Err(ReplayDecodeError(format!("{} bytes left over", r.0.len())));
        }
        Ok(Replay {
            scope: ReplayScope {
                endpoint,
                principal,
                key,
            },
            fingerprint,
            repr_type,
            bytes: body,
            outcome,
            recorded_at,
        })
    }
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u64).to_le_bytes());
    out.extend_from_slice(b);
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_bytes(out, s.as_bytes());
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ReplayDecodeError> {
        if self.0.len() < n {
            return Err(ReplayDecodeError("truncated".into()));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn byte(&mut self) -> Result<u8, ReplayDecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u64(&mut self) -> Result<u64, ReplayDecodeError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }
    fn bytes(&mut self) -> Result<&'a [u8], ReplayDecodeError> {
        let n = usize::try_from(self.u64()?)
            .map_err(|_| ReplayDecodeError("length overflows".into()))?;
        self.take(n)
    }
    fn string(&mut self) -> Result<String, ReplayDecodeError> {
        String::from_utf8(self.bytes()?.to_vec())
            .map_err(|_| ReplayDecodeError("a text field is not UTF-8".into()))
    }
}
