//! **Spaces that change while the kernel runs** (ledger #2006): a space that gains
//! and loses doors without the kernel's root being replaced, and a space that lives
//! for one request or one session.
//!
//! Both rest on one combinator, [`Threaded`]: a space that resolves through an inner
//! space and hangs **every answer it serves** from one golden thread. Cutting that
//! thread invalidates everything the space ever served, with no cache-walking code,
//! because the cache already drops whatever hangs from a cut thread.
//!
//! - **A transient or session space** is a `Threaded` over a fixed inner space, with a
//!   thread named for the session ([`Threaded::session`],
//!   `urn:iki:space:session:{id}`). The host injects it as a corridor for the
//!   session's requests; ending the session is cutting its thread. Nothing else needs
//!   freeing: the corridor is dropped with the last request that carried it.
//! - **A dynamic space** ([`DynamicSpace`]) is a `Threaded` whose inner space it can
//!   swap, rebuilt from a **declaration resource** (the Turtle `urn:kernel:topology`
//!   emits, in the `ik:` arrangement vocabulary). The mechanism is NetKernel's
//!   DynamicSpaceAccessor: the dynamic part is a space INSIDE a fixed host space, so the
//!   kernel's root never changes.
//!
//! # Why the rebuild happens outside resolution
//!
//! [`Space::resolve`] is synchronous and holds no issuer, so unlike NetKernel's
//! accessor this space cannot re-read its declaration when a request arrives. The host
//! reads it instead, when the declaration's golden thread is cut: the kernel already
//! reports that through a [`CutListener`](crate::CutListener), so the host's loop is
//! "wait for a cut, re-read, validate, swap". Core has no runtime, so core supplies the
//! pieces and a future the host awaits ([`DynamicSpace::drive`]); the host spawns it.
//!
//! # The coarse cut
//!
//! Every answer the dynamic space serves hangs from a thread the SPACE owns
//! ([`DynamicSpace::thread`], named like the space), not from the declaration's. A
//! successful swap cuts it, so no cached answer computed under the old doors survives
//! the change; a refused edit cuts nothing, because the old doors stay in force. That
//! is version one: any change cuts every answer the space served (the finer cut, per
//! door, is ledger #2034).
//!
//! # Authority
//!
//! There is no capability for changing a dynamic space. The authority to change it is
//! the authority to WRITE its declaration, which the declaration's own store already
//! gates; the host reads the declaration under whatever capability it hands
//! [`DynamicSpace::refresh`].

use std::sync::{Arc, RwLock};

use async_trait::async_trait;

#[cfg(feature = "declare")]
use crate::capability::Capability;
use crate::content::ContentId;
use crate::declare::{build, DeclarationError, Registry};
use crate::describe::{ArgSpec, Description};
use crate::endpoint::{BoxFuture, Endpoint, Invocation};
use crate::error::{Error, Result};
use crate::grammar::Bindings;
use crate::iri::{escape_iri_fragment, Iri, IriError};
use crate::kernel::{Clock, Kernel};
use crate::repr::{ReprType, Representation, Thread, Time};
use crate::request::Request;
use crate::space::{
    answered, space_iri, EndpointSpace, Resolution, Resolved, Scope, Space, SpaceEntry,
};
use crate::topology::{escape_literal, SpaceKind, Topology};

/// Where a session space is named, and its thread with it: `urn:iki:space:session:`.
/// See [`Threaded::session`].
pub const SESSION_PREFIX: &str = "urn:iki:space:session:";

/// Where a dynamic space is named, and its thread with it: `urn:iki:space:dynamic:`.
/// See [`DynamicSpace::new`].
pub const DYNAMIC_PREFIX: &str = "urn:iki:space:dynamic:";

/// **A space whose every answer hangs from one golden thread.** It resolves through
/// `inner` and wraps each hit's endpoint so the answer depends on `thread`
/// ([`Invocation::depends_on`]); cutting the thread invalidates every answer the space
/// served, and nothing it did not serve.
///
/// Transparent otherwise: its [`entries`](Space::entries) and
/// [`topology`](Space::topology) are the inner space's, and a hit keeps everything the
/// inner resolution reported ([`Resolution::map_endpoint`]). A limiter's hit is not
/// wrapped, because a limiter is never invoked.
///
/// # Its name is its own, never the inner space's
///
/// A `Threaded` claims no identity unless it is given one ([`named`](Self::named),
/// [`session`](Self::session)). Forwarding the inner space's would be a false claim:
/// an answer cached through the bare inner space does not hang from the thread, and a
/// corridor sharing its name would be served that answer and keep it past the cut.
///
/// ```
/// use std::sync::Arc;
/// use futures::executor::block_on;
/// use ikigai_core::{
///     Capability, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, ReprType, Representation,
///     Request, Scope, Space, Threaded, Verb,
/// };
///
/// let doors = Arc::new(EndpointSpace::new().bind(
///     Exact::new("urn:example:greeting"),
///     FnEndpoint::new("greeting", |_| {
///         Ok(Representation::new(ReprType::new("text/plain"), b"hello".to_vec()).cacheable())
///     }),
/// ));
/// let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
/// let cap = Capability::root();
/// // One session: its own corridor, named (and threaded) for the session.
/// let session: Arc<dyn Space> = Arc::new(Threaded::session("s-42", doors).unwrap());
/// let scope = || Scope::empty().with(Arc::clone(&session));
/// let read = || Request::new(Verb::Source, Iri::parse("urn:example:greeting").unwrap());
///
/// block_on(kernel.issue_in(read(), &cap, scope())).unwrap();
/// assert!(kernel.is_cached_in(&read(), &cap, &scope()));
///
/// // The session ends: cut its thread, and what it served is gone.
/// kernel.cut("urn:iki:space:session:s-42");
/// assert!(!kernel.is_cached_in(&read(), &cap, &scope()));
/// ```
pub struct Threaded {
    inner: Arc<dyn Space>,
    thread: Thread,
    id: Option<Iri>,
}

impl Threaded {
    /// Thread every answer `inner` serves from `thread`. Anonymous: inject it with
    /// [`Scope::with`] for a one-request corridor, or name it.
    pub fn new(inner: Arc<dyn Space>, thread: impl Into<Thread>) -> Self {
        Threaded {
            inner,
            thread: thread.into(),
            id: None,
        }
    }

    /// A **session space**: `inner`, named and threaded `urn:iki:space:session:{id}`
    /// ([`SESSION_PREFIX`]), so each session is its own cache partition and its own
    /// thread, and nothing is shared across sessions. Ending the session is cutting
    /// that thread. `id` is escaped as an IRI fragment
    /// ([`escape_iri_fragment`](crate::escape_iri_fragment)); refused only when the
    /// result is still not an IRI.
    pub fn session(id: &str, inner: Arc<dyn Space>) -> std::result::Result<Self, IriError> {
        let name = Iri::parse(format!("{SESSION_PREFIX}{}", escape_iri_fragment(id)))?;
        Ok(Threaded {
            inner,
            thread: Thread::new(name.as_str()),
            id: Some(name),
        })
    }

    /// Claim an identity for this space (builder) — a claim, same name ⇒ same doors
    /// AND the same thread; see [`Space::id`].
    pub fn named(mut self, id: Iri) -> Self {
        self.id = Some(id);
        self
    }

    /// The thread every answer this space serves hangs from.
    pub fn thread(&self) -> &Thread {
        &self.thread
    }
}

impl Space for Threaded {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let resolution = thread_hit(self.inner.resolve(request, scope), &self.thread);
        answered(resolution, &self.id)
    }

    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        self.inner.entries()
    }

    fn id(&self) -> Option<Iri> {
        self.id.clone()
    }

    /// The inner space's structure: a `Threaded` adds no door and removes none. A
    /// named `Threaded` over an anonymous inner space names the node; over a named one,
    /// the inner space's own name stays (it is what the node's doors are).
    fn topology(&self) -> Topology {
        let node = self.inner.topology();
        match (&node.id, &self.id) {
            (None, Some(id)) => node.with_id(Some(id.clone())),
            _ => node,
        }
    }
}

/// Wrap a hit so its answer hangs from `thread`.
fn thread_hit(resolution: Resolution, thread: &Thread) -> Resolution {
    resolution.map_endpoint(|endpoint| {
        Arc::new(ThreadedEndpoint {
            inner: endpoint,
            thread: thread.clone(),
        })
    })
}

/// An endpoint whose answer additionally hangs from one thread. Forwards everything
/// else, so the door it stands at describes, confines and names itself as before.
struct ThreadedEndpoint {
    inner: Arc<dyn Endpoint>,
    thread: Thread,
}

#[async_trait]
impl Endpoint for ThreadedEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        inv.depends_on(self.thread.clone());
        self.inner.invoke(inv).await
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn describe(&self) -> Description {
        self.inner.describe()
    }

    fn confinement(&self) -> Option<Topology> {
        self.inner.confinement()
    }
}

/// A host's own rule a candidate arrangement must pass before it binds — for
/// instance "every path to `urn:pay:*` passes the audit overlay", as a SPARQL ASK
/// over the candidate's [`to_turtle`](Topology::to_turtle) that the host's function
/// reads from a query resource. Core cannot run SPARQL, so the RULE is the host's
/// resource and only the call is code. `Err(why)` refuses the candidate.
pub type SpaceCheck =
    Arc<dyn Fn(Topology) -> BoxFuture<std::result::Result<(), String>> + Send + Sync>;

/// Why a candidate declaration did not bind. The old doors stay in force, and
/// [`DynamicSpace::status`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DynamicRefusal {
    /// The declaration could not be read (not found, denied, not UTF-8 text …).
    Unreadable {
        /// What went wrong, as the read reported it.
        reason: String,
    },
    /// The declaration is not one [`build`] accepts: it names the node and why.
    Declaration(DeclarationError),
    /// The declaration holds a level that seals names. A running kernel's seal table is
    /// fixed when it is built, so a level that arrived later could not have its seals
    /// enforced: every request through it would be refused as an unregistered sealing
    /// level. Refused here instead, where it is one clear refusal.
    SealingLevel {
        /// The level.
        node: String,
    },
    /// The host's [`SpaceCheck`] refused the candidate.
    Check {
        /// The host's reason.
        reason: String,
    },
}

impl DynamicRefusal {
    /// The node the refusal names, when it names one.
    pub fn node(&self) -> Option<&str> {
        match self {
            DynamicRefusal::Declaration(error) => declaration_node(error),
            DynamicRefusal::SealingLevel { node } => Some(node),
            _ => None,
        }
    }
}

/// The node a declaration error names, when it names one.
fn declaration_node(error: &DeclarationError) -> Option<&str> {
    match error {
        DeclarationError::Malformed { node, .. } => node.as_deref(),
        DeclarationError::UnknownEndpoint { door, .. } => Some(door),
        DeclarationError::Opaque { node, .. }
        | DeclarationError::Rewrite { node }
        | DeclarationError::Chain { node }
        | DeclarationError::Confine { node }
        | DeclarationError::Dynamic { node }
        | DeclarationError::Pattern { node, .. }
        | DeclarationError::TooLarge { node, .. } => Some(node),
        _ => None,
    }
}

impl std::fmt::Display for DynamicRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DynamicRefusal::Unreadable { reason } => {
                write!(f, "the declaration could not be read: {reason}")
            }
            DynamicRefusal::Declaration(error) => write!(f, "{error}"),
            DynamicRefusal::SealingLevel { node } => write!(
                f,
                "<{node}> is a level that seals names: a running kernel's seals are fixed \
                 when it is built, so a dynamic space cannot add one"
            ),
            DynamicRefusal::Check { reason } => write!(f, "the host's check refused it: {reason}"),
        }
    }
}

impl std::error::Error for DynamicRefusal {}

/// What a [`DynamicSpace`] reports about itself: what is in force, and the last
/// refusal. A snapshot; [`DynamicSpace::status`] and the space's status resource
/// (`{space}:status`) read it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DynamicStatus {
    /// The space's fixed name.
    pub space: Iri,
    /// The declaration resource it is built from.
    pub declaration: Iri,
    /// The digest of the declaration text now in force, or `None` while the space is
    /// empty because nothing has bound yet (fail closed).
    pub in_force: Option<ContentId>,
    /// How many declarations have bound since the space was made: `0` while it is
    /// empty, and the version number of the arrangement in force otherwise.
    pub installs: u64,
    /// When the arrangement in force bound, by the space's clock
    /// ([`DynamicSpace::with_clock`]); `None` without a clock or before anything bound.
    pub installed_at: Option<Time>,
    /// The last refusal since the arrangement in force bound, and the digest of what
    /// was refused when there was text to digest. Cleared by the next bind.
    pub refusal: Option<(DynamicRefusal, Option<ContentId>)>,
}

/// What a dynamic space holds: its inner space, and its status.
struct State {
    inner: Arc<dyn Space>,
    status: DynamicStatus,
}

/// **A space whose doors are rebuilt from a declaration resource while the kernel
/// runs** (ledger #2006). It delegates resolution, [`entries`](Space::entries) and
/// [`topology`](Space::topology) to its CURRENT inner space and threads every answer
/// it serves from [`thread`](Self::thread), so a successful swap cuts everything the
/// old doors served: the COARSE cut of version one, where any change invalidates every
/// answer the space served and a refused change invalidates nothing (the finer cut, per
/// door, is ledger #2034). The mechanism is NetKernel's DynamicSpaceAccessor: the
/// dynamic part is a space inside a fixed host space, so the kernel's root never
/// changes.
///
/// # The host reads the declaration, outside resolution
///
/// [`Space::resolve`] is synchronous and holds no issuer, so this space cannot re-read
/// its declaration when a request arrives. The host does, when the declaration's thread
/// is cut (`drive`, behind the `declare` feature, is that loop as a future, over a
/// [`CutListener`](crate::CutListener); core has no runtime, so the host spawns it).
/// There is no capability for changing the space: the authority is the grant to WRITE
/// its declaration, which the declaration's own store enforces.
///
/// # Its name is FIXED, and the coarse cut keeps that honest
///
/// A space's name is a cache claim — same name, same doors ([`Space::id`]) — and a
/// dynamic space's doors change. It keeps one name, `urn:iki:space:dynamic:{name}`,
/// across every version anyway, because every answer served under the old doors hangs
/// from its thread and the swap cuts that thread: no entry from before a change
/// survives it, so no request is ever served an answer from doors that are gone.
/// (Naming each version instead would change every scope fingerprint on each edit and
/// leave the old version's entries in the cache until evicted.)
///
/// # Validate before bind; fail closed
///
/// A candidate binds only when [`build`] accepts it (unknown endpoint names, opaque
/// spaces, closures, the host's seals — [`Registry::sealing`] — each refused naming the
/// node), it holds no level that seals names ([`DynamicRefusal::SealingLevel`]), and
/// the host's [`SpaceCheck`] passes it. Anything refused keeps the current inner space
/// and is reported in the [status](Self::status). The space starts EMPTY: until a
/// declaration binds, every name answers [`Unresolved`](Error::Unresolved), never the
/// wrong doors. A later read that fails is a refusal like any other and keeps the
/// doors in force; to empty the space, bind a declaration that binds nothing.
///
/// One rebuilder per space: binds are not serialized against each other, so two
/// concurrent installs (two loops, say) end with whichever committed last.
///
/// # Status
///
/// The space answers one name of its own, `{space}:status` ([`status_iri`](Self::status_iri)):
/// what is in force, how many declarations have bound, and the last refusal, as
/// `text/plain` or (`as=text/turtle`) Turtle. Uncacheable: it is live state. It is
/// answered ahead of the inner space, so no declaration can shadow it.
///
/// ```
/// use std::sync::Arc;
/// use futures::executor::block_on;
/// use ikigai_core::{
///     Capability, DynamicSpace, EndpointSpace, Error, Exact, FnEndpoint, Iri, Kernel, Registry,
///     ReprType, Representation, Request, Space, Verb,
/// };
///
/// let hello = Arc::new(FnEndpoint::new("hello", |_| {
///     Ok(Representation::new(ReprType::new("text/plain"), b"hi".to_vec()).cacheable())
/// }));
/// let mut registry = Registry::new();
/// registry.register(hello.clone()).unwrap();
/// let dynamic = Arc::new(DynamicSpace::new(
///     "demo",
///     Iri::parse("urn:example:decl:demo").unwrap(),
///     registry,
/// ));
/// let kernel = Kernel::new(dynamic.clone());
/// let cap = Capability::root();
/// let read = || Request::new(Verb::Source, Iri::parse("urn:example:hello").unwrap());
///
/// // Empty until something binds: fail closed.
/// assert!(matches!(block_on(kernel.issue(read(), &cap)), Err(Error::Unresolved(_))));
///
/// // Bind an arrangement (here built in code; a host reads it from the declaration).
/// let doors = EndpointSpace::new().bind_arc(Exact::new("urn:example:hello"), hello);
/// block_on(dynamic.install_topology(&kernel, &doors.topology())).unwrap();
/// assert_eq!(block_on(kernel.issue(read(), &cap)).unwrap().bytes, b"hi");
/// assert!(kernel.is_cached(&read(), &cap));
/// assert_eq!(dynamic.status().installs, 1);
///
/// // Bind one without the door: it is gone, and so is what it served.
/// block_on(dynamic.install_topology(&kernel, &EndpointSpace::new().topology())).unwrap();
/// assert!(!kernel.is_cached(&read(), &cap));
/// assert!(matches!(block_on(kernel.issue(read(), &cap)), Err(Error::Unresolved(_))));
/// ```
pub struct DynamicSpace {
    id: Iri,
    thread: Thread,
    status_iri: Iri,
    declaration: Iri,
    registry: Registry,
    check: Option<SpaceCheck>,
    clock: Option<Arc<dyn Clock>>,
    state: Arc<RwLock<State>>,
    status_endpoint: Arc<dyn Endpoint>,
}

impl DynamicSpace {
    /// An EMPTY dynamic space named `urn:iki:space:dynamic:{name}`
    /// ([`DYNAMIC_PREFIX`]), built from `declaration` against `registry`'s endpoints
    /// and seals. Nothing is read here: bind with
    /// [`install_topology`](Self::install_topology), or (behind the `declare` feature,
    /// which brings the Turtle parser) `install`, `refresh` or the `drive` loop.
    ///
    /// # Panics
    ///
    /// When `name` is not one or more `:`-separated segments of `a-z`, `0-9` and `-`
    /// (the rule [`space_iri`](crate::space_iri) holds module names to): it is a name in
    /// the host's own code or configuration, refused where it is written.
    pub fn new(name: &str, declaration: Iri, registry: Registry) -> Self {
        let id = space_iri(&format!("dynamic:{name}"));
        let status_iri =
            Iri::parse(format!("{}:status", id.as_str())).expect("a space name plus `:status`");
        let status = DynamicStatus {
            space: id.clone(),
            declaration: declaration.clone(),
            in_force: None,
            installs: 0,
            installed_at: None,
            refusal: None,
        };
        let state = Arc::new(RwLock::new(State {
            inner: Arc::new(EndpointSpace::new()),
            status,
        }));
        DynamicSpace {
            thread: Thread::new(id.as_str()),
            status_endpoint: Arc::new(StatusEndpoint {
                state: Arc::clone(&state),
            }),
            id,
            status_iri,
            declaration,
            registry,
            check: None,
            clock: None,
            state,
        }
    }

    /// Run `check` on every candidate after core's own refusals and before it binds
    /// (builder); see [`SpaceCheck`].
    pub fn with_check(mut self, check: SpaceCheck) -> Self {
        self.check = Some(check);
        self
    }

    /// Stamp each bind with `clock`'s time in the status (builder).
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// The thread every answer this space serves hangs from — named like the space,
    /// cut by every successful bind.
    pub fn thread(&self) -> &Thread {
        &self.thread
    }

    /// The declaration resource this space is built from.
    pub fn declaration(&self) -> &Iri {
        &self.declaration
    }

    /// The name of this space's status resource: `{space}:status`.
    pub fn status_iri(&self) -> &Iri {
        &self.status_iri
    }

    /// What is in force, and the last refusal.
    pub fn status(&self) -> DynamicStatus {
        self.read().status.clone()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        // A poisoned lock means a panic while swapping two `Arc`s and a status; the
        // state is still whole (each field is replaced, never left half-written).
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// **Bind a declaration's Turtle** — parse it (`Topology::from_turtle`), validate
    /// it, and swap it in; behind the `declare` feature. A refusal keeps the doors in
    /// force, is recorded in the status with the text's digest, and is returned.
    #[cfg(feature = "declare")]
    pub async fn install(
        &self,
        kernel: &Kernel,
        declaration: &str,
    ) -> std::result::Result<(), DynamicRefusal> {
        let digest = ContentId::of(declaration.as_bytes());
        match Topology::from_turtle(declaration) {
            Ok(topology) => self.bind(kernel, &topology, digest).await,
            Err(error) => Err(self.refuse(DynamicRefusal::Declaration(error), Some(digest))),
        }
    }

    /// **Bind an arrangement already in hand** — validate it and swap it in. Its
    /// digest in the status is over its rendered Turtle. A refusal keeps the doors in
    /// force, is recorded in the status, and is returned.
    pub async fn install_topology(
        &self,
        kernel: &Kernel,
        topology: &Topology,
    ) -> std::result::Result<(), DynamicRefusal> {
        let digest = ContentId::of(topology.to_turtle().as_bytes());
        self.bind(kernel, topology, digest).await
    }

    /// **Read the declaration through the kernel and bind it** — the step a host takes
    /// at start and on each cut of the declaration's thread; behind the `declare`
    /// feature. The read is a `Source` of [`declaration`](Self::declaration) under
    /// `capability`, which is the space's read authority, and it re-hangs the reader on
    /// the declaration's thread, so the next write to it is heard. A failed read is a
    /// refusal ([`DynamicRefusal::Unreadable`]): at start the space stays empty.
    #[cfg(feature = "declare")]
    pub async fn refresh(
        &self,
        kernel: &Kernel,
        capability: &Capability,
    ) -> std::result::Result<(), DynamicRefusal> {
        let read = Request::new(crate::verb::Verb::Source, self.declaration.clone());
        let bytes = match kernel.issue(read, capability).await {
            Ok(representation) => representation.bytes,
            Err(error) => {
                return Err(self.refuse(
                    DynamicRefusal::Unreadable {
                        reason: error.to_string(),
                    },
                    None,
                ))
            }
        };
        match String::from_utf8(bytes) {
            Ok(text) => self.install(kernel, &text).await,
            Err(error) => {
                let digest = ContentId::of(error.as_bytes());
                Err(self.refuse(
                    DynamicRefusal::Unreadable {
                        reason: format!("not UTF-8 text: {}", error.utf8_error()),
                    },
                    Some(digest),
                ))
            }
        }
    }

    /// **The rebuild loop, as a future the host spawns** (core has no runtime); behind
    /// the `declare` feature. It listens on the declaration's thread
    /// ([`Kernel::listen`], so `capability` must grant
    /// [`CAP_LISTEN`](crate::CAP_LISTEN)), [refreshes](Self::refresh) once, and then
    /// refreshes after every batch of cuts — a batch that reports dropped events
    /// refreshes too, since one read catches up whatever was missed. Listening starts
    /// BEFORE the first read, so a write between the two is not lost.
    ///
    /// It runs until it is dropped, and returns only the error that stopped it: the
    /// listen refused. A refused declaration does not stop it (it is in the status).
    ///
    /// ⚠ A listener under a capability other than root hears a cut only when an entry
    /// cached under that capability rests on the thread
    /// ([`listen`](crate::listen)'s authority rule). The first read caches one when the
    /// declaration's representation is cacheable; a declaration served uncacheable is
    /// never heard under a narrowed capability, and the host must refresh on its own
    /// signal instead.
    #[cfg(feature = "declare")]
    pub async fn drive(&self, kernel: &Kernel, capability: &Capability) -> Error {
        let spec = crate::listen::ListenSpec::new().exact(self.declaration.as_str());
        let listener = match kernel.listen(spec, capability) {
            Ok(listener) => listener,
            Err(error) => return error,
        };
        loop {
            // A refusal is recorded in the status; the loop goes on waiting.
            let _ = self.refresh(kernel, capability).await;
            let _ = listener.wait().await;
        }
    }

    /// Validate `topology` and, when it passes, swap it in and cut what the old doors
    /// served; otherwise record the refusal.
    async fn bind(
        &self,
        kernel: &Kernel,
        topology: &Topology,
        digest: ContentId,
    ) -> std::result::Result<(), DynamicRefusal> {
        let inner = match self.prepare(topology) {
            Ok(inner) => inner,
            Err(refusal) => return Err(self.refuse(refusal, Some(digest))),
        };
        if let Some(check) = &self.check {
            if let Err(reason) = check(topology.clone()).await {
                return Err(self.refuse(DynamicRefusal::Check { reason }, Some(digest)));
            }
        }
        {
            let mut state = self
                .state
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.inner = inner;
            state.status.in_force = Some(digest);
            state.status.installs += 1;
            state.status.installed_at = self.clock.as_ref().map(|clock| clock.now());
            state.status.refusal = None;
        }
        // The coarse cut: nothing the old doors served survives. Then the faces derived
        // from the bindings (catalog, manifold, Meta, the floor memo) recompute. A request
        // already running keeps the old inner space through its own `Arc`, and the cache
        // declines to store an answer computed across this cut.
        kernel.cut(self.thread.clone());
        kernel.bindings_changed();
        Ok(())
    }

    /// Core's refusals, in order: [`build`]'s (which runs the host's seals), then a
    /// level that seals names.
    fn prepare(&self, topology: &Topology) -> std::result::Result<Arc<dyn Space>, DynamicRefusal> {
        let inner = build(topology, &self.registry).map_err(DynamicRefusal::Declaration)?;
        if let Some(node) = sealing_level(topology) {
            return Err(DynamicRefusal::SealingLevel { node });
        }
        Ok(inner)
    }

    /// Record `refusal` in the status, keeping the doors in force; hand it back.
    fn refuse(&self, refusal: DynamicRefusal, digest: Option<ContentId>) -> DynamicRefusal {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.status.refusal = Some((refusal.clone(), digest));
        refusal
    }

    /// The current inner space.
    fn inner(&self) -> Arc<dyn Space> {
        Arc::clone(&self.read().inner)
    }
}

/// The first level in `topology` that seals a prefix, by name. Walks with a stack;
/// [`build`] has already bounded the tree.
fn sealing_level(topology: &Topology) -> Option<String> {
    let mut stack = vec![topology];
    while let Some(node) = stack.pop() {
        if let SpaceKind::Level { seals, .. } = &node.kind {
            if !seals.is_empty() {
                return Some(
                    node.id
                        .as_ref()
                        .map_or_else(String::new, |id| id.as_str().to_string()),
                );
            }
        }
        if let SpaceKind::EndpointSpace { doors } = &node.kind {
            stack.extend(doors.iter().filter_map(|d| d.confined.as_deref()));
        }
        stack.extend(node.children.iter());
    }
    None
}

impl Space for DynamicSpace {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        if request.target == self.status_iri {
            return Resolution::Hit(
                Resolved::new(Arc::clone(&self.status_endpoint), Bindings::new())
                    .with_answered_by(self.id.clone()),
            );
        }
        let resolution = thread_hit(self.inner().resolve(request, scope), &self.thread);
        answered(resolution, &Some(self.id.clone()))
    }

    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        let mut entries = vec![SpaceEntry::new(
            self.status_iri.as_str(),
            self.status_endpoint.name(),
        )];
        entries.extend(self.inner().entries().unwrap_or_default());
        Some(entries)
    }

    fn id(&self) -> Option<Iri> {
        Some(self.id.clone())
    }

    fn topology(&self) -> Topology {
        Topology::new(SpaceKind::Dynamic {
            declaration: self.declaration.clone(),
        })
        .with_id(Some(self.id.clone()))
        .child(self.inner().topology())
    }
}

/// The endpoint a dynamic space answers `{space}:status` with.
struct StatusEndpoint {
    state: Arc<RwLock<State>>,
}

#[async_trait]
impl Endpoint for StatusEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let status = self
            .state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .status
            .clone();
        let face = match inv.request.args.get("as") {
            Some(crate::arg::ArgRef::Inline(bytes)) => std::str::from_utf8(bytes)
                .map(str::trim)
                .unwrap_or("")
                .to_string(),
            _ => "text/plain".to_string(),
        };
        match face.as_str() {
            "text/plain" => Ok(Representation::new(
                ReprType::new("text/plain").with_param("charset", "utf-8"),
                status_text(&status).into_bytes(),
            )),
            "text/turtle" => Ok(Representation::new(
                ReprType::new("text/turtle").with_param("charset", "utf-8"),
                status_turtle(&status).into_bytes(),
            )),
            other => Err(Error::InvalidArgument {
                name: "as".to_string(),
                detail: format!(
                    "`{other}` is not a face of `{}` (text/plain, text/turtle)",
                    inv.request.target.as_str()
                ),
            }),
        }
    }

    fn name(&self) -> &str {
        "dynamic-space-status"
    }

    fn describe(&self) -> Description {
        Description::new("dynamic-space-status")
            .title("A dynamic space's status")
            .summary(
                "What a dynamic space has in force: the digest of the declaration that bound, \
                 how many have bound, and the last refusal and why. Live state, uncacheable.",
            )
            .input(
                ArgSpec::new("as")
                    .summary("response face")
                    .one_of(["text/plain", "text/turtle"])
                    .default_value("text/plain"),
            )
            .output("text/plain;charset=utf-8")
            .output("text/turtle")
    }
}

/// The status's text face: one `key: value` line each (pinned in `tests/dynamic.rs`).
fn status_text(status: &DynamicStatus) -> String {
    let mut out = format!(
        "space: {}\ndeclaration: {}\n",
        status.space.as_str(),
        status.declaration.as_str()
    );
    match &status.in_force {
        Some(digest) => out += &format!("in force: {digest}\n"),
        None => out += "in force: nothing (the space is empty)\n",
    }
    out += &format!("installs: {}\n", status.installs);
    if let Some(at) = status.installed_at {
        out += &format!("installed at: {} ms since the Unix epoch\n", at.as_millis());
    }
    match &status.refusal {
        Some((refusal, digest)) => {
            out += &format!("refused: {refusal}\n");
            if let Some(node) = refusal.node() {
                out += &format!("refused at: {node}\n");
            }
            if let Some(digest) = digest {
                out += &format!("refused digest: {digest}\n");
            }
        }
        None => out += "refused: nothing since the last bind\n",
    }
    out
}

/// The status's Turtle face: statements about the space, in the `ik:` vocabulary.
fn status_turtle(status: &DynamicStatus) -> String {
    let mut out = format!(
        "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
         @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\n\
         <{}> a ik:DynamicSpace ;\n    ik:declaration <{}> ;\n    ik:installs \"{}\"^^xsd:nonNegativeInteger",
        status.space.as_str(),
        status.declaration.as_str(),
        status.installs
    );
    if let Some(digest) = &status.in_force {
        out += &format!(" ;\n    ik:inForce \"{digest}\"");
    }
    if let Some((refusal, _)) = &status.refusal {
        out += &format!(
            " ;\n    ik:refusal \"{}\"",
            escape_literal(&refusal.to_string())
        );
    }
    out += " .\n";
    out
}
