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
//! **Held content belongs to whoever delivered it** (ledger #2965). A digest is a name, not a
//! secret, and a wire peer can put any id in an argument, so an invocation reads a content id
//! under its own capability ([`ContentStore::get_for`]): root reads everything, and any other
//! capability reads only what was held for it
//! ([`put_for`](MemoryContentStore::put_for)). A foreign id answers exactly as an absent one
//! does, [`Error::NotFound`], so naming one does not tell the caller the content is here.
//! [`put`](MemoryContentStore::put) names nobody, and only root reads what it holds.
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

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex, Weak};

use bytes::Bytes;

use crate::body::{Body, Replay};
use crate::capability::Capability;
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
    /// is held under it. This is the host's view and asks nothing of the caller: what an
    /// invocation reads goes through [`get_for`](Self::get_for).
    fn get(&self, id: &ContentId) -> Option<Body>;

    /// The body stored under `id`, when `capability` may read it, or `None`: a store must
    /// not tell content held for someone else from content that is not there.
    ///
    /// The default fails closed for a store that keeps no record of who delivered what: only
    /// root reads. A store that serves other capabilities overrides this.
    fn get_for(&self, id: &ContentId, capability: &Capability) -> Option<Body> {
        if capability.is_root() {
            self.get(id)
        } else {
            None
        }
    }
}

/// Who a hold on content was taken for, and so who may read the content through it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Holder {
    /// The host, naming nobody ([`MemoryContentStore::put`]): root alone reads.
    Host,
    /// One principal: any capability acting as it reads, whatever else it holds.
    Principal(String),
    /// A capability naming no principal: an equal one reads, since nothing tells the two
    /// apart. They already share a cache partition.
    Grant(BTreeSet<String>),
}

impl Holder {
    fn of(capability: &Capability) -> Holder {
        match (capability.scopes(), capability.principal()) {
            (None, _) => Holder::Host,
            (Some(_), Some(principal)) => Holder::Principal(principal.to_string()),
            (Some(scopes), None) => Holder::Grant(scopes.clone()),
        }
    }

    /// Whether `capability`, which is not root, may read what this hold keeps.
    fn admits(&self, capability: &Capability) -> bool {
        match self {
            Holder::Host => false,
            Holder::Principal(principal) => capability.principal() == Some(principal.as_str()),
            Holder::Grant(scopes) => {
                capability.principal().is_none() && capability.scopes() == Some(scopes)
            }
        }
    }
}

/// Core's in-memory [`ContentStore`] (ledger #2240): content held while a
/// [`ContentHandle`] for it lives, bounded in bytes.
///
/// Putting the same bytes twice gives two handles on one entry, which lives until the
/// last is dropped; two parties delivering the same bytes each read them while their own
/// handle lives, and no longer ([`put_for`](Self::put_for)). A put that would hold more than the bound is refused with
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
    /// How many live handles each holder has. Never empty: the last handle removes the entry.
    holders: HashMap<Holder, usize>,
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
    /// returned handle lives, for the host: through this handle only root reads them. Content
    /// delivered on behalf of a caller goes in with [`put_for`](Self::put_for).
    pub fn put(&self, bytes: impl Into<Bytes>) -> Result<ContentHandle> {
        self.put_held(Holder::Host, bytes.into())
    }

    /// [`put`](Self::put) on behalf of the caller holding `capability`, who reads the content
    /// while the handle lives (ledger #2965): any capability acting as the same principal,
    /// or, when `capability` names no principal, a capability equal to it. Root reads it too.
    /// Anyone else naming its id is answered as if nothing were held.
    ///
    /// ⚠ A capability naming no principal cannot be told from an equal one, so callers a door
    /// admits under one shared grant can read each other's content. A door serving parties
    /// that must not should name each in its capability
    /// ([`Capability::with_principal`](crate::Capability::with_principal)). A narrowed
    /// capability that kept no principal is not equal to the one that delivered.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use futures::executor::block_on;
    /// use ikigai_core::{
    ///     ArgRef, Capability, EndpointSpace, Error, Iri, Kernel, MemoryContentStore, Request,
    ///     Verb,
    /// };
    ///
    /// let store = Arc::new(MemoryContentStore::new(1 << 20));
    /// let kernel = Kernel::new(Arc::new(EndpointSpace::new())).with_content_store(store.clone());
    /// let session = Capability::scoped(["urn:cap:read"]);
    /// let alice = session.with_principal("urn:example:person:alice").unwrap();
    /// let bob = session.with_principal("urn:example:person:bob").unwrap();
    ///
    /// let held = store.put_for(&alice, b"alice's".to_vec()).unwrap();
    /// let read = Request::new(Verb::Source, Iri::parse("urn:stream:collect").unwrap())
    ///     .with_arg("content", ArgRef::Content(held.id()));
    /// assert_eq!(block_on(kernel.issue(read.clone(), &alice)).unwrap().bytes, b"alice's");
    /// assert!(matches!(block_on(kernel.issue(read, &bob)), Err(Error::NotFound(_))));
    /// ```
    pub fn put_for(
        &self,
        capability: &Capability,
        bytes: impl Into<Bytes>,
    ) -> Result<ContentHandle> {
        self.put_held(Holder::of(capability), bytes.into())
    }

    fn put_held(&self, holder: Holder, bytes: Bytes) -> Result<ContentHandle> {
        let id = ContentId::of(&bytes);
        let size = bytes.len() as u64;
        self.hold(holder, id, size, || Body::Bytes(bytes))
    }

    /// Put a body that can be read again under its strong validator
    /// ([`Replay::validator`]): a spill that hashed its bytes while it wrote them. Refused
    /// as [`Error::InvalidArgument`] when the replay states no validator (there is no id
    /// to hold it under without reading it), and counted by its length hint against the
    /// bound (`OverBudget` when it states none). Held for the host, as [`put`](Self::put)
    /// is: only root reads it through this handle.
    pub fn put_replay<R: Replay + 'static>(&self, replay: R) -> Result<ContentHandle> {
        self.put_replay_held(Holder::Host, replay)
    }

    /// [`put_replay`](Self::put_replay) on behalf of the caller holding `capability`, who
    /// reads it as [`put_for`](Self::put_for) says.
    pub fn put_replay_for<R: Replay + 'static>(
        &self,
        capability: &Capability,
        replay: R,
    ) -> Result<ContentHandle> {
        self.put_replay_held(Holder::of(capability), replay)
    }

    fn put_replay_held<R: Replay + 'static>(
        &self,
        holder: Holder,
        replay: R,
    ) -> Result<ContentHandle> {
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
        self.hold(holder, id, size, || Body::replay(replay))
    }

    fn hold(
        &self,
        holder: Holder,
        id: ContentId,
        size: u64,
        body: impl FnOnce() -> Body,
    ) -> Result<ContentHandle> {
        let mut state = self.inner.state.lock().expect("content store lock");
        if let Some(entry) = state.entries.get_mut(&id) {
            *entry.holders.entry(holder.clone()).or_default() += 1;
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
                    holders: HashMap::from([(holder.clone(), 1)]),
                },
            );
        }
        Ok(ContentHandle {
            id,
            holder,
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

    fn get_for(&self, id: &ContentId, capability: &Capability) -> Option<Body> {
        let state = self.inner.state.lock().expect("content store lock");
        let entry = state.entries.get(id)?;
        let readable =
            capability.is_root() || entry.holders.keys().any(|holder| holder.admits(capability));
        readable.then(|| entry.body.share()).flatten()
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
    holder: Holder,
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
                *entry.holders.entry(self.holder.clone()).or_default() += 1;
            }
        }
        ContentHandle {
            id: self.id,
            holder: self.holder.clone(),
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
                if let Some(handles) = entry.holders.get_mut(&self.holder) {
                    *handles -= 1;
                    if *handles == 0 {
                        entry.holders.remove(&self.holder);
                    }
                }
                entry.holders.is_empty()
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
