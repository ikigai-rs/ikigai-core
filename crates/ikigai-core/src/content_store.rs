//! **The content store** (ledger #2240, stage 4 of the streams design): values by
//! [`ContentId`], so a large argument travels as `content=b3:…`
//! ([`ArgRef::Content`](crate::ArgRef::Content)) instead of inline bytes, and a request
//! carrying one stays a pure value: its identity is the digest, a retried Sink reads the
//! content again, and memory stays flat.
//!
//! A host installs one with [`Kernel::with_content_store`](crate::Kernel::with_content_store)
//! and keeps its own handle to put content in (a door spills an upload while hashing it,
//! then dispatches the Sink with the id). The content is resolved **inside the invocation
//! that carries its id** ([`Invocation::content_body`](crate::Invocation::content_body)),
//! and nowhere else: there is no public name to read a content id by.
//!
//! [`MemoryContentStore`] is core's in-memory implementation; a spill-backed store (files,
//! hashed while written) is a host library's, through the same trait.
//!
//! ```
//! use std::sync::Arc;
//! use futures::executor::block_on;
//! use ikigai_core::{
//!     ArgRef, AsyncFnEndpoint, Capability, EndpointSpace, Exact, Iri, Kernel,
//!     MemoryContentStore, ReprType, Representation, Request, Verb,
//! };
//!
//! // A Sink that reads its `content` as a body, however it arrived.
//! let size = AsyncFnEndpoint::new("size", |inv| {
//!     Box::pin(async move {
//!         let body = inv.content_body("content").await?;
//!         let n = inv.collect(body).await?.len();
//!         Ok(Representation::new(ReprType::new("text/plain"), n.to_string().into_bytes()))
//!     })
//! });
//! let store = Arc::new(MemoryContentStore::new(1 << 20));
//! let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(Exact::new("urn:size"), size)))
//!     .with_content_store(store.clone());
//!
//! // The host puts the content in and holds it while the request runs.
//! let held = store.put(vec![7u8; 1000]).unwrap();
//! let sink = Request::new(Verb::Sink, Iri::parse("urn:size").unwrap())
//!     .with_arg("content", ArgRef::Content(held.id()));
//! assert_eq!(block_on(kernel.issue(sink.clone(), &Capability::root())).unwrap().bytes, b"1000");
//! // Dropped, it is gone: the same request now names nothing.
//! drop(held);
//! assert!(block_on(kernel.issue(sink, &Capability::root())).is_err());
//! ```

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, Weak};

use bytes::Bytes;

use crate::body::{Body, Replay};
use crate::content::ContentId;
use crate::error::{Error, Result};

/// A store of bodies by content id: what resolves [`ArgRef::Content`](crate::ArgRef::Content)
/// inside the invocation that carries it. Installed on a kernel with
/// [`Kernel::with_content_store`](crate::Kernel::with_content_store).
///
/// A store holds only bodies that can be read again ([`Body::Bytes`], [`Body::Replay`]):
/// a retried request reads its content again, which a one-shot stream could not give it.
pub trait ContentStore: Send + Sync {
    /// The body stored under `id`, a fresh handle on every call, or `None` when nothing
    /// is held under it.
    fn get(&self, id: &ContentId) -> Option<Body>;
}

/// Core's in-memory [`ContentStore`] (ledger #2240): content held while a
/// [`ContentHandle`] for it lives, bounded in bytes.
///
/// Putting the same bytes twice gives two handles on one entry, which lives until the
/// last is dropped. A put that would hold more than the bound is refused with
/// [`Error::OverBudget`], never truncated.
pub struct MemoryContentStore {
    inner: Arc<Shared>,
}

struct Shared {
    max_bytes: u64,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    entries: HashMap<ContentId, Entry>,
    held: u64,
}

struct Entry {
    body: Body,
    size: u64,
    holders: usize,
}

impl MemoryContentStore {
    /// An empty store holding at most `max_bytes` in all.
    pub fn new(max_bytes: u64) -> Self {
        MemoryContentStore {
            inner: Arc::new(Shared {
                max_bytes,
                state: Mutex::new(State::default()),
            }),
        }
    }

    /// Put `bytes` in, under their digest ([`ContentId::of`]), and hold them while the
    /// returned handle lives.
    pub fn put(&self, bytes: impl Into<Bytes>) -> Result<ContentHandle> {
        let bytes = bytes.into();
        let id = ContentId::of(&bytes);
        let size = bytes.len() as u64;
        self.hold(id, size, || Body::Bytes(bytes))
    }

    /// Put a body that can be read again under its strong validator
    /// ([`Replay::validator`]): a spill that hashed its bytes while it wrote them. Refused
    /// as [`Error::InvalidArgument`] when the replay states no validator (there is no id
    /// to hold it under without reading it), and counted by its length hint against the
    /// bound (`OverBudget` when it states none).
    pub fn put_replay<R: Replay + 'static>(&self, replay: R) -> Result<ContentHandle> {
        let Some(id) = replay.validator() else {
            return Err(Error::InvalidArgument {
                name: "content".into(),
                detail: "a replay with no validator has no content id to be stored under".into(),
            });
        };
        let Some(size) = replay.len_hint() else {
            return Err(Error::OverBudget {
                detail: "a replay of unknown length cannot be counted against the store's bound"
                    .into(),
            });
        };
        self.hold(id, size, || Body::replay(replay))
    }

    fn hold(&self, id: ContentId, size: u64, body: impl FnOnce() -> Body) -> Result<ContentHandle> {
        let mut state = self.inner.state.lock().expect("content store lock");
        if let Some(entry) = state.entries.get_mut(&id) {
            entry.holders += 1;
        } else {
            let held = state.held.saturating_add(size);
            if held > self.inner.max_bytes {
                return Err(Error::OverBudget {
                    detail: format!(
                        "the content store holds {} of {} bytes, and {id} needs {size} more",
                        state.held, self.inner.max_bytes
                    ),
                });
            }
            state.held = held;
            state.entries.insert(
                id,
                Entry {
                    body: body(),
                    size,
                    holders: 1,
                },
            );
        }
        Ok(ContentHandle {
            id,
            store: Arc::downgrade(&self.inner),
        })
    }

    /// How many bytes the store holds.
    pub fn held_bytes(&self) -> u64 {
        self.inner.state.lock().expect("content store lock").held
    }

    /// How many distinct contents the store holds.
    pub fn len(&self) -> usize {
        self.inner
            .state
            .lock()
            .expect("content store lock")
            .entries
            .len()
    }

    /// Whether the store holds nothing.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ContentStore for MemoryContentStore {
    fn get(&self, id: &ContentId) -> Option<Body> {
        let state = self.inner.state.lock().expect("content store lock");
        state.entries.get(id).and_then(|entry| entry.body.share())
    }
}

impl fmt::Debug for MemoryContentStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.inner.state.lock().expect("content store lock");
        f.debug_struct("MemoryContentStore")
            .field("entries", &state.entries.len())
            .field("held", &state.held)
            .field("max_bytes", &self.inner.max_bytes)
            .finish()
    }
}

/// A hold on content in a [`MemoryContentStore`]: the content stays while a handle for it
/// lives, and the last handle dropped releases it.
#[must_use = "the content is released when the handle is dropped"]
pub struct ContentHandle {
    id: ContentId,
    store: Weak<Shared>,
}

impl ContentHandle {
    /// The content id to put in a request ([`ArgRef::Content`](crate::ArgRef::Content)).
    pub fn id(&self) -> ContentId {
        self.id
    }
}

impl Clone for ContentHandle {
    fn clone(&self) -> Self {
        if let Some(store) = self.store.upgrade() {
            let mut state = store.state.lock().expect("content store lock");
            if let Some(entry) = state.entries.get_mut(&self.id) {
                entry.holders += 1;
            }
        }
        ContentHandle {
            id: self.id,
            store: self.store.clone(),
        }
    }
}

impl Drop for ContentHandle {
    fn drop(&mut self) {
        let Some(store) = self.store.upgrade() else {
            return;
        };
        let mut state = store.state.lock().expect("content store lock");
        let release = match state.entries.get_mut(&self.id) {
            Some(entry) => {
                entry.holders -= 1;
                entry.holders == 0
            }
            None => false,
        };
        if release {
            if let Some(entry) = state.entries.remove(&self.id) {
                state.held = state.held.saturating_sub(entry.size);
            }
        }
    }
}

impl fmt::Debug for ContentHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ContentHandle").field(&self.id).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_lives_while_a_handle_does_and_counts_against_the_bound() {
        let store = MemoryContentStore::new(10);
        let a = store.put(b"hello".to_vec()).unwrap();
        let again = store.put(b"hello".to_vec()).unwrap();
        assert_eq!(a.id(), ContentId::of(b"hello"));
        assert_eq!(
            (store.len(), store.held_bytes()),
            (1, 5),
            "one entry, two holders"
        );
        assert!(matches!(
            store.put(b"too much".to_vec()),
            Err(Error::OverBudget { .. })
        ));
        drop(a);
        assert!(store.get(&ContentId::of(b"hello")).is_some(), "still held");
        let cloned = again.clone();
        drop(again);
        assert!(store.get(&ContentId::of(b"hello")).is_some());
        drop(cloned);
        assert!(store.get(&ContentId::of(b"hello")).is_none());
        assert_eq!(store.held_bytes(), 0);
        assert!(store.put(b"too much".to_vec()).is_ok(), "room again");
    }

    #[test]
    fn a_replay_is_held_under_its_validator_or_refused() {
        struct Spilled(bool);
        impl Replay for Spilled {
            fn open(&self) -> crate::ByteStream {
                Box::pin(futures_util::stream::iter([Ok(Bytes::from_static(
                    b"spilled",
                ))]))
            }
            fn len_hint(&self) -> Option<u64> {
                Some(7)
            }
            fn validator(&self) -> Option<ContentId> {
                self.0.then(|| ContentId::of(b"spilled"))
            }
        }
        let store = MemoryContentStore::new(100);
        let held = store.put_replay(Spilled(true)).unwrap();
        assert!(matches!(store.get(&held.id()), Some(Body::Replay(_))));
        assert!(matches!(
            store.put_replay(Spilled(false)),
            Err(Error::InvalidArgument { .. })
        ));
    }
}
