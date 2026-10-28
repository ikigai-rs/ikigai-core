//! **A streamed body** (ledger #2237, the first stage of the binary streams design, ledger
//! #2228): what a [`Representation<Body>`](crate::Representation) carries in place of a
//! buffer.
//!
//! A body has three shapes, and a producer offers the weakest one it can honor:
//!
//! - [`Body::Bytes`]: today's buffer, behind a cheap shared handle ([`bytes::Bytes`]), so a
//!   cache hit hands it out without copying it.
//! - [`Body::Replay`]: something that opens a fresh reader as often as asked (a file, a
//!   spill, an object in a store), maybe seekable, maybe knowing its length.
//! - [`Body::Once`]: a one-shot stream (a generator, a live source, an upload in flight).
//!
//! Two axes are kept apart. [`Expiry`](crate::Expiry) says how long the bytes are TRUE; the
//! body's shape says whether they can be READ AGAIN. The kernel stores `Bytes` and `Replay`
//! answers as it stores bytes, and never stores a `Once` answer: the uncached note names
//! the one-shot body as the reason ([`UNCACHED_NOTE`](crate::UNCACHED_NOTE)).
//!
//! **`Body` does not implement `Serialize`, on purpose.** `Representation`'s derive bounds
//! its body by `Serialize`, so `Representation<Vec<u8>>` crosses a wire exactly as before
//! and `Representation<Body>` cannot be framed by accident: every transport handles a
//! stream explicitly or not at all.
//!
//! ```compile_fail
//! fn framed<T: serde::Serialize>(_: &T) {}
//! let body = ikigai_core::Representation::from_body(
//!     ikigai_core::ReprType::new("application/octet-stream"),
//!     ikigai_core::Body::from(b"x".to_vec()),
//! );
//! framed(&body); // `Body: Serialize` is not satisfied
//! ```
//!
//! …while the bytes form still is (the same check, passing):
//!
//! ```
//! fn framed<T: serde::Serialize>(_: &T) {}
//! let bytes = ikigai_core::Representation::new(
//!     ikigai_core::ReprType::new("application/octet-stream"),
//!     b"x".to_vec(),
//! );
//! framed(&bytes);
//! ```
//!
//! Runtime-free and wasm-clean: a stream is futures-core's [`Stream`], driven by whatever
//! executor the host chose. Core spawns nothing and owns no timer, so a deadline is checked
//! as each chunk arrives (see [`Kernel::issue_body`](crate::Kernel::issue_body)).

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use futures_core::Stream;
use futures_util::StreamExt;

use crate::content::ContentId;
use crate::error::{Error, Result};
use crate::run::MemoryBudget;

/// A one-shot stream of chunks: futures-core's [`Stream`] of `Result<Bytes>`, boxed and
/// `Send`. An `Err` item ends the body as a FAILURE: a reader must never take the chunks
/// before it as the whole (refuse, never truncate).
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>;

/// A body that can be **read again**: every [`open`](Replay::open) is a fresh reader from
/// the start. What a file, a spill or a stored object offers.
///
/// `Arc<dyn Replay>` also implements this trait, so a shared replay returned by
/// [`Spill::spill`](crate::Spill::spill) can be passed directly to
/// [`MemoryContentStore::put_replay`](crate::MemoryContentStore::put_replay).
///
/// A replay that cannot keep its bytes stable while a reader is open (a file written in
/// place) checks on open and at the end, and fails the stream with
/// [`Error::Conflict`] rather than hand out torn bytes: a cut invalidates lookups, not open
/// readers, so a reader that is already open is promised the version it opened.
pub trait Replay: Send + Sync {
    /// A fresh reader from the first byte.
    fn open(&self) -> ByteStream;

    /// A fresh reader from byte `offset`, or `None` when this body is not seekable (the
    /// default). A seekable body with a known length and a strong validator is what an
    /// HTTP door may answer `206` over.
    fn open_at(&self, offset: u64) -> Option<ByteStream> {
        let _ = offset;
        None
    }

    /// The length in bytes, when it is known before reading. A hint for framing
    /// (`Content-Length`) and for the cache's byte bound; a reader still counts what it
    /// reads.
    fn len_hint(&self) -> Option<u64> {
        None
    }

    /// A strong validator: the content id of exactly these bytes, when the producer
    /// knows it (a spill hashes while it writes). `None` by default.
    fn validator(&self) -> Option<ContentId> {
        None
    }
}

// Let shared replays (including the trait object returned by Spill) pass through
// APIs that accept Replay by value without a host-defined forwarding wrapper.
impl Replay for Arc<dyn Replay> {
    fn open(&self) -> ByteStream {
        self.as_ref().open()
    }

    fn open_at(&self, offset: u64) -> Option<ByteStream> {
        self.as_ref().open_at(offset)
    }

    fn len_hint(&self) -> Option<u64> {
        self.as_ref().len_hint()
    }

    fn validator(&self) -> Option<ContentId> {
        self.as_ref().validator()
    }
}

/// The body of a [`Representation<Body>`](crate::Representation): a buffer, a body that
/// can be read again, or a one-shot stream. See the module documentation.
///
/// ```
/// use futures::executor::block_on;
/// use ikigai_core::Body;
///
/// // Today's buffer, as a body: one chunk, known length, read as often as asked.
/// let body = Body::from(b"hello".to_vec());
/// assert_eq!(body.len_hint(), Some(5));
/// assert!(!body.is_one_shot());
/// assert_eq!(block_on(body.collect(1024)).unwrap(), &b"hello"[..]);
///
/// // A one-shot stream: collected under a bound, refused past it, never truncated.
/// let chunks = || Body::once(futures::stream::iter(
///     [b"ab".to_vec(), b"cd".to_vec()].map(|c| Ok(c.into())),
/// ));
/// assert_eq!(block_on(chunks().collect(4)).unwrap(), &b"abcd"[..]);
/// assert!(matches!(
///     block_on(chunks().collect(3)),
///     Err(ikigai_core::Error::OverBudget { .. })
/// ));
/// ```
pub enum Body {
    /// A buffer in memory, shared without copying.
    Bytes(Bytes),
    /// A body that opens a fresh reader on every read.
    Replay(Arc<dyn Replay>),
    /// A one-shot stream: it can be read once, and the kernel never stores it.
    Once(ByteStream),
}

impl Body {
    /// A one-shot body over `stream`.
    pub fn once<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<Bytes>> + Send + 'static,
    {
        Body::Once(Box::pin(stream))
    }

    /// A body that can be read again, over `replay`.
    pub fn replay<R: Replay + 'static>(replay: R) -> Self {
        Body::Replay(Arc::new(replay))
    }

    /// The length in bytes when it is known before reading: always for
    /// [`Bytes`](Body::Bytes), when the producer says so for
    /// [`Replay`](Body::Replay), never for [`Once`](Body::Once).
    pub fn len_hint(&self) -> Option<u64> {
        match self {
            Body::Bytes(bytes) => Some(bytes.len() as u64),
            Body::Replay(replay) => replay.len_hint(),
            Body::Once(_) => None,
        }
    }

    /// Whether this body can be read only once ([`Once`](Body::Once)): the kernel never
    /// stores such an answer.
    pub fn is_one_shot(&self) -> bool {
        matches!(self, Body::Once(_))
    }

    /// The buffer, when this body is one ([`Bytes`](Body::Bytes)); a cheap shared handle.
    pub fn as_bytes(&self) -> Option<&Bytes> {
        match self {
            Body::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// Read the body as a stream: the buffer as one chunk (none when empty), a fresh
    /// reader from a [`Replay`], or the one-shot stream itself.
    pub fn into_stream(self) -> ByteStream {
        match self {
            Body::Bytes(bytes) if bytes.is_empty() => Box::pin(futures_util::stream::empty()),
            Body::Bytes(bytes) => Box::pin(futures_util::stream::once(async move { Ok(bytes) })),
            Body::Replay(replay) => replay.open(),
            Body::Once(stream) => stream,
        }
    }

    /// Collect the whole body into one buffer, refusing with [`Error::OverBudget`] once it
    /// would exceed `max_bytes`. **It never truncates**: past the bound the answer is the
    /// refusal, not a shorter body, and at most one chunk is read beyond it. A stream that
    /// fails part-way returns its error, and nothing it produced before is handed out.
    ///
    /// A [`Bytes`](Body::Bytes) body is returned as it is (no copy) when it fits.
    pub async fn collect(self, max_bytes: usize) -> Result<Bytes> {
        let over = |seen: usize| Error::OverBudget {
            detail: format!(
                "the body is larger than the {max_bytes}-byte collect bound (at least {seen} bytes)"
            ),
        };
        match self {
            Body::Bytes(bytes) if bytes.len() > max_bytes => Err(over(bytes.len())),
            Body::Bytes(bytes) => Ok(bytes),
            other => {
                if let Some(len) = other.len_hint() {
                    if len > max_bytes as u64 {
                        return Err(over(usize::try_from(len).unwrap_or(usize::MAX)));
                    }
                }
                drain(other.into_stream(), |chunk_len, held| {
                    let seen = held.saturating_add(chunk_len);
                    if seen > max_bytes {
                        Err(over(seen))
                    } else {
                        Ok(())
                    }
                })
                .await
            }
        }
    }

    /// Collect the whole body into one buffer **under a run's byte pool** (ledger #2238):
    /// every chunk read out of a stream is drawn from `budget` before it is held, and the
    /// collect refuses with [`Error::OverBudget`] at the first chunk the pool cannot cover,
    /// in the middle of the stream too. **It never truncates.** A collect that fails
    /// (past the pool, or a stream error) drops what it read and gives its draw back.
    ///
    /// A [`Bytes`](Body::Bytes) body is returned as it is and draws nothing: it is already
    /// in memory, and refusing it after the fact would free nothing. A
    /// [`Replay`](Body::Replay) whose length hint exceeds what is left is refused before it
    /// is opened.
    ///
    /// ```
    /// use futures::executor::block_on;
    /// use ikigai_core::{Body, IssueContext};
    ///
    /// let run = IssueContext::new().with_memory_limit(5);
    /// let pool = run.memory_budget().unwrap();
    /// let stream = |parts: &[&'static [u8]]| {
    ///     let parts: Vec<_> = parts.iter().map(|p| Ok(ikigai_core::Bytes::from_static(p))).collect();
    ///     Body::once(futures::stream::iter(parts))
    /// };
    /// assert_eq!(block_on(stream(&[b"ab", b"c"]).collect_within(pool)).unwrap(), &b"abc"[..]);
    /// assert_eq!(pool.remaining(), 2, "three bytes drawn for the whole run");
    /// // Past the pool: refused mid-stream, and the failed collect gives back its draw.
    /// assert!(matches!(
    ///     block_on(stream(&[b"x", b"yz"]).collect_within(pool)),
    ///     Err(ikigai_core::Error::OverBudget { .. })
    /// ));
    /// assert_eq!(pool.remaining(), 2);
    /// // A buffer is already held: handed over as it is, drawing nothing.
    /// assert_eq!(block_on(Body::from(b"0123456789".to_vec()).collect_within(pool)).unwrap().len(), 10);
    /// assert_eq!(pool.remaining(), 2);
    /// ```
    pub async fn collect_within(self, budget: &MemoryBudget) -> Result<Bytes> {
        let over = |seen: u64| {
            Error::OverBudget {
            detail: format!(
                "the body is larger than the run's byte budget: {} byte(s) left, at least {seen}                  needed (a `urn:cap:stream:memory:<bytes>` grant raises it, up to the host's                  ceiling)",
                budget.remaining()
            ),
        }
        };
        let stream = match self {
            Body::Bytes(bytes) => return Ok(bytes),
            other => {
                if let Some(len) = other.len_hint() {
                    if len > budget.remaining() {
                        return Err(over(len));
                    }
                }
                other.into_stream()
            }
        };
        let mut drawn: u64 = 0;
        let result = drain(stream, |chunk_len, held| {
            let wanted = chunk_len as u64;
            if budget.try_draw(wanted) {
                drawn += wanted;
                Ok(())
            } else {
                Err(over(held as u64 + wanted))
            }
        })
        .await;
        if result.is_err() {
            budget.give_back(drawn);
        }
        result
    }

    /// A second handle on a body that can be read again (`Bytes`, `Replay`): what the
    /// cache keeps and hands out. `None` for a one-shot body, which cannot be shared.
    pub(crate) fn share(&self) -> Option<Body> {
        match self {
            Body::Bytes(bytes) => Some(Body::Bytes(bytes.clone())),
            Body::Replay(replay) => Some(Body::Replay(Arc::clone(replay))),
            Body::Once(_) => None,
        }
    }

    /// End a one-shot body at `deadline` on `clock`: each chunk that arrives once the clock
    /// reads at or past it is replaced by [`Error::OverBudget`], and the stream ends there.
    /// A body that can be read again is returned unchanged (its reader is bounded by
    /// whoever opens it). Core owns no timer, so a producer that stalls without yielding
    /// is not ended by this: a door adds an idle limit between chunks.
    pub(crate) fn ending_at(self, deadline: crate::Time, clock: Arc<dyn crate::Clock>) -> Body {
        match self {
            Body::Once(stream) => Body::Once(Box::pin(Deadlined {
                inner: Some(stream),
                deadline,
                clock,
            })),
            other => other,
        }
    }
}

/// Read `stream` to its end into one buffer, asking `admit(chunk_len, held)` before each
/// chunk is held: the one loop behind [`Body::collect`] and [`Body::collect_within`], so a
/// bound and a pool refuse at the same point (the first chunk that does not fit), and a
/// stream error ends the read as a failure.
async fn drain(
    mut stream: ByteStream,
    mut admit: impl FnMut(usize, usize) -> Result<()>,
) -> Result<Bytes> {
    let mut buffer = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        admit(chunk.len(), buffer.len())?;
        buffer.extend_from_slice(&chunk);
    }
    Ok(buffer.freeze())
}

impl From<Vec<u8>> for Body {
    fn from(bytes: Vec<u8>) -> Self {
        Body::Bytes(Bytes::from(bytes))
    }
}

impl From<Bytes> for Body {
    fn from(bytes: Bytes) -> Self {
        Body::Bytes(bytes)
    }
}

impl From<&'static [u8]> for Body {
    fn from(bytes: &'static [u8]) -> Self {
        Body::Bytes(Bytes::from_static(bytes))
    }
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Body::Bytes(bytes) => f.debug_tuple("Bytes").field(bytes).finish(),
            Body::Replay(replay) => f
                .debug_struct("Replay")
                .field("len_hint", &replay.len_hint())
                .field("validator", &replay.validator())
                .finish(),
            Body::Once(_) => f.write_str("Once(..)"),
        }
    }
}

/// A one-shot stream ended at a deadline: [`Body::ending_at`].
struct Deadlined {
    inner: Option<ByteStream>,
    deadline: crate::Time,
    clock: Arc<dyn crate::Clock>,
}

impl Stream for Deadlined {
    type Item = Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Some(inner) = self.inner.as_mut() else {
            return Poll::Ready(None);
        };
        match inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(_))) if self.clock.now() >= self.deadline => {
                // Drop the producer now: its Drop releases whatever it holds.
                self.inner = None;
                Poll::Ready(Some(Err(Error::OverBudget {
                    detail: format!(
                        "the stream ran past its run's deadline ({} ms)",
                        self.deadline.as_millis()
                    ),
                })))
            }
            Poll::Ready(None) => {
                self.inner = None;
                Poll::Ready(None)
            }
            other => other,
        }
    }
}
