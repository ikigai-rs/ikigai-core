//! **The spill facility** (ledger #2372, part of the streams arc, ledger #2228): what
//! turns a one-shot body into one that can be read again, by writing it somewhere and
//! hashing it while it writes. Core holds only the seam: the trait, the kernel's slot for
//! it ([`Kernel::with_spill`](crate::Kernel::with_spill)) and the sealed builtin
//! `urn:stream:spill` ([`stream::SPILL`](crate::stream::SPILL)) that calls it under the
//! caller's disk budget (`urn:cap:stream:spill:<bytes>`,
//! [`cap_stream_spill`](crate::cap_stream_spill)). The file-backed facility is a host
//! library's (ikigai-fs); a host with no file system (the browser) installs none, and
//! `urn:stream:spill` is then `Unsupported`.
//!
//! ```
//! use std::sync::Arc;
//! use futures::executor::block_on;
//! use futures::StreamExt;
//! use ikigai_core::{
//!     cap_stream_spill, ArgRef, Body, ByteStream, Bytes, Capability, ContentHasher, ContentId,
//!     EndpointSpace, Error, Iri, Kernel, Replay, Request, Result, Spill, Verb,
//! };
//!
//! // A facility that "spills" to memory: a real one writes a file.
//! struct Held(Bytes, ContentId);
//! impl Replay for Held {
//!     fn open(&self) -> ByteStream {
//!         Box::pin(futures::stream::iter([Ok(self.0.clone())]))
//!     }
//!     fn len_hint(&self) -> Option<u64> {
//!         Some(self.0.len() as u64)
//!     }
//!     fn validator(&self) -> Option<ContentId> {
//!         Some(self.1)
//!     }
//! }
//! struct InMemory;
//! #[async_trait::async_trait]
//! impl Spill for InMemory {
//!     async fn spill(&self, mut body: ByteStream, max_bytes: u64) -> Result<Arc<dyn Replay>> {
//!         let (mut held, mut hasher) = (Vec::new(), ContentHasher::new());
//!         while let Some(chunk) = body.next().await {
//!             let chunk = chunk?;
//!             if (held.len() + chunk.len()) as u64 > max_bytes {
//!                 return Err(Error::OverBudget { detail: "past the spill limit".into() });
//!             }
//!             hasher.update(&chunk);
//!             held.extend_from_slice(&chunk);
//!         }
//!         Ok(Arc::new(Held(held.into(), hasher.finish())))
//!     }
//! }
//!
//! let kernel = Kernel::new(Arc::new(EndpointSpace::new())).with_spill(Arc::new(InMemory));
//! let spill = Request::new(Verb::Source, Iri::parse("urn:stream:spill").unwrap())
//!     .with_arg("content", ArgRef::Inline(b"hello".to_vec()));
//! let granted = Capability::scoped([cap_stream_spill(5)]);
//! let answer = block_on(kernel.issue_body(spill.clone(), &granted)).unwrap();
//! let Body::Replay(replay) = &answer.bytes else { panic!("a spill can be read again") };
//! assert_eq!(replay.validator(), Some(ContentId::of(b"hello")));
//! // One byte less of grant: refused, never a shorter body.
//! let short = Capability::scoped([cap_stream_spill(4)]);
//! assert!(matches!(block_on(kernel.issue(spill, &short)), Err(Error::OverBudget { .. })));
//! ```

use std::sync::Arc;

use async_trait::async_trait;

use crate::body::{ByteStream, Replay};
use crate::error::Result;

/// A **spill facility**: writes a one-shot body somewhere it can be read back from, and
/// answers it as a [`Replay`]. Installed on a kernel with
/// [`Kernel::with_spill`](crate::Kernel::with_spill), in the shape of
/// [`ContentStore`](crate::ContentStore); called by `urn:stream:spill`.
///
/// The contract, which `urn:stream:spill` checks on every answer:
///
/// - the replay's [`validator`](Replay::validator) is the content id of the bytes it
///   reads back ([`ContentHasher`](crate::ContentHasher) computes it while they are
///   written, with core's identity rule), and its [`len_hint`](Replay::len_hint) is their
///   exact length;
/// - every [`open`](Replay::open) reads the same bytes, the whole body;
/// - more than `max_bytes` is refused with [`Error::OverBudget`](crate::Error::OverBudget)
///   (and what was written is dropped), never answered as a shorter body. A facility
///   stops reading at the limit: a body past it is not written whole first.
#[async_trait]
pub trait Spill: Send + Sync {
    /// Write `body` out, at most `max_bytes` of it, and answer it as a body that can be
    /// read again; an error of the body's own ends the spill with that error.
    async fn spill(&self, body: ByteStream, max_bytes: u64) -> Result<Arc<dyn Replay>>;
}
