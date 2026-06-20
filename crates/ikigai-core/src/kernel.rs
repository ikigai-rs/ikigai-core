//! The resolution kernel: it ties spaces, endpoints, and the representation
//! cache together.
//!
//! [`Kernel::issue`] is `async` so the kernel can be driven by any executor — a
//! multi-threaded thread pool on native (NetKernel-style scheduling), a
//! single-threaded executor in the browser — without `ikigai-core` depending on
//! a runtime. Resolution itself is synchronous (pure routing); the asynchronous
//! work lives in endpoint invocation.
//!
//! A result is cached only when the verb is idempotent *and* the endpoint opted
//! in via [`Expiry::Never`]. Validity is then tracked by **golden threads**
//! ([`Thread`]): a cached representation depends on the threads it declared plus
//! those of every sub-resource it resolved (they propagate up through
//! composition), and [`Kernel::cut`] invalidates everything carrying a thread.
//! That lets results which read mutable state be cached and invalidated on change
//! — a `Sink` (or an external watcher) cuts the thread named after the state.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::arg::ArgRef;
use crate::capability::Capability;
use crate::endpoint::{Invocation, Issuer};
use crate::error::{Error, Result};
use crate::meta::MetaRenderer;
use crate::repr::{Expiry, ReprType, Representation, Thread};
use crate::request::{Request, RequestId};
use crate::space::{Resolution, Scope, Space, SpaceEntry};
use crate::verb::Verb;

/// Resolves requests against a root space, invokes the resolved endpoint, and
/// caches cacheable representations by their content-addressed request id.
pub struct Kernel {
    root: Arc<dyn Space>,
    cache: Mutex<HashMap<RequestId, CacheEntry>>,
    /// Current generation of each golden thread (absent ⇒ generation 0).
    /// [`Kernel::cut`] bumps a thread's generation, invalidating every cache entry
    /// pinned to an earlier one.
    generations: Mutex<HashMap<Thread, u64>>,
    meta: Option<Arc<dyn MetaRenderer>>,
}

/// A cached representation plus the golden-thread edges that keep it valid: each
/// `(thread, generation)` records the generation that thread held when the entry
/// was stored. The entry is valid only while every thread is still at that
/// generation — cut any of them and it's stale.
struct CacheEntry {
    representation: Representation,
    edges: Vec<(Thread, u64)>,
}

impl Kernel {
    /// A kernel over the given root space.
    pub fn new(root: Arc<dyn Space>) -> Self {
        Kernel {
            root,
            cache: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            meta: None,
        }
    }

    /// A kernel that answers `Meta` requests by rendering through `renderer`.
    pub fn with_meta_renderer(root: Arc<dyn Space>, renderer: Arc<dyn MetaRenderer>) -> Self {
        Kernel {
            root,
            cache: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            meta: Some(renderer),
        }
    }

    /// Issue a request: return a valid cached representation if one exists,
    /// otherwise resolve, invoke the endpoint, and cache the result if cacheable.
    pub async fn issue(&self, request: Request, capability: &Capability) -> Result<Representation> {
        let id = request.id();
        let cacheable_verb = request.verb.is_cacheable();

        // Representation-cache lookup (idempotent verbs only): serve a cached entry
        // whose golden-thread edges are all still current. A cut entry is evicted
        // here and recomputed below. The guard is dropped before any await.
        if cacheable_verb {
            if let Some(cached) = self.valid_cached(&id) {
                return Ok(cached);
            }
        }

        // Resolution is synchronous, pure routing.
        let resolved = match self.root.resolve(&request, &Scope::empty()) {
            Resolution::Hit(resolved) => resolved,
            Resolution::Miss => return Err(Error::Unresolved(request.target.clone())),
        };

        let representation = if request.verb == Verb::Meta {
            // Uniform Meta routing: the kernel renders the endpoint's canonical
            // self-description to the requested type via the transform layer,
            // rather than each endpoint hand-rolling it.
            let renderer = self
                .meta
                .as_ref()
                .ok_or_else(|| Error::Endpoint("no Meta renderer configured".to_string()))?;
            renderer
                .render(&resolved.endpoint.describe(), &meta_target(&request))?
                .cacheable()
        } else {
            // Invocation is asynchronous.
            let invocation =
                Invocation::with_issuer(&request, &resolved.bindings, capability, self);
            let representation = resolved.endpoint.invoke(&invocation).await?;
            // Effective expiry propagates from the dependencies: a result is
            // cacheable only if it opted in AND every dependency it read is cacheable.
            let effective = if representation.expiry == Expiry::Never
                && invocation.dependency_expiry() == Expiry::Never
            {
                Expiry::Never
            } else {
                Expiry::Always
            };
            // Golden threads propagate too: the result depends on its own declared
            // threads plus those of every sub-resource it resolved, so cutting any
            // of them invalidates this composite.
            let mut threads = representation.threads().clone();
            threads.extend(invocation.dependency_threads());
            representation.with_expiry(effective).with_threads(threads)
        };

        // A successful mutating verb invalidates its target: cut the thread named
        // after it, so cached `Source`s of that resource — and composites over
        // them — recompute. This is the internal half of the golden thread (the
        // kernel owns invalidation on writes); an external watcher cuts the same
        // thread on an out-of-band change.
        if request.verb.is_mutating() {
            self.cut(request.target.as_str());
        }

        if cacheable_verb && representation.expiry == Expiry::Never {
            let edges = self.edges_for(representation.threads());
            self.cache.lock().expect("cache lock").insert(
                id,
                CacheEntry {
                    representation: representation.clone(),
                    edges,
                },
            );
        }
        Ok(representation)
    }

    /// A cached representation for `id` whose golden-thread edges are all current.
    /// A stale entry (some thread has been cut since) is evicted and `None`
    /// returned, so the caller recomputes.
    fn valid_cached(&self, id: &RequestId) -> Option<Representation> {
        let mut cache = self.cache.lock().expect("cache lock");
        let outcome = cache.get(id).map(|entry| {
            let gens = self.generations.lock().expect("generations lock");
            let valid = entry
                .edges
                .iter()
                .all(|(thread, gen)| generation_of(&gens, thread) == *gen);
            (valid, entry.representation.clone())
        });
        match outcome {
            None => None,
            Some((true, representation)) => Some(representation),
            Some((false, _)) => {
                cache.remove(id);
                None
            }
        }
    }

    /// Pin each thread to its current generation, forming an entry's validity edges.
    fn edges_for(&self, threads: &BTreeSet<Thread>) -> Vec<(Thread, u64)> {
        let gens = self.generations.lock().expect("generations lock");
        threads
            .iter()
            .map(|thread| (thread.clone(), generation_of(&gens, thread)))
            .collect()
    }

    /// Cut a golden thread: invalidate every cached representation that depends on
    /// it, directly or transitively through composition. Cheap — it bumps the
    /// thread's generation; dependent entries are evicted lazily on next lookup.
    /// A `Sink` that mutates a resource cuts the thread named after it; an external
    /// watcher cuts it on change.
    pub fn cut(&self, thread: impl Into<Thread>) {
        let mut gens = self.generations.lock().expect("generations lock");
        *gens.entry(thread.into()).or_insert(0) += 1;
    }

    /// The number of representations currently cached (diagnostics/tests).
    pub fn cache_len(&self) -> usize {
        self.cache.lock().expect("cache lock").len()
    }

    /// Whether issuing `request` right now would be served from the cache — a
    /// read-only probe that neither resolves nor mutates. `false` for verbs that
    /// aren't cacheable, and for any request whose result isn't already cached
    /// (including one that would be a miss). Lets a caller report cache state
    /// without the observer effect of actually issuing the request.
    pub fn is_cached(&self, request: &Request) -> bool {
        if !request.verb.is_cacheable() {
            return false;
        }
        let cache = self.cache.lock().expect("cache lock");
        match cache.get(&request.id()) {
            // Read-only probe: a cut (but not-yet-evicted) entry is not "cached".
            Some(entry) => {
                let gens = self.generations.lock().expect("generations lock");
                entry
                    .edges
                    .iter()
                    .all(|(thread, gen)| generation_of(&gens, thread) == *gen)
            }
            None => false,
        }
    }

    /// Enumerate the root space's bindings, if it supports enumeration. `None`
    /// when the root space is not enumerable.
    pub fn entries(&self) -> Option<Vec<SpaceEntry>> {
        self.root.entries()
    }
}

/// The current generation of `thread` (absent ⇒ 0).
fn generation_of(generations: &HashMap<Thread, u64>, thread: &Thread) -> u64 {
    generations.get(thread).copied().unwrap_or(0)
}

/// The representation type a `Meta` request asks for: the `as` inline argument
/// if present, else `text/turtle`.
fn meta_target(request: &Request) -> ReprType {
    if let Some(ArgRef::Inline(bytes)) = request.args.get("as") {
        if let Ok(media) = std::str::from_utf8(bytes) {
            return ReprType::new(media);
        }
    }
    ReprType::new("text/turtle")
}

#[async_trait]
impl Issuer for Kernel {
    async fn issue(&self, request: Request, capability: &Capability) -> Result<Representation> {
        // Delegate to the inherent method (which the context calls back into).
        Kernel::issue(self, request, capability).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arg::ArgRef;
    use crate::builtins;
    use crate::describe::Description;
    use crate::endpoint::{Endpoint, FnEndpoint, Invocation};
    use crate::grammar::Exact;
    use crate::iri::Iri;
    use crate::repr::ReprType;
    use crate::space::EndpointSpace;
    use crate::verb::Verb;
    use futures::executor::block_on;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn iri(s: &str) -> Iri {
        Iri::parse(s).unwrap()
    }

    struct EchoIdRenderer;
    impl MetaRenderer for EchoIdRenderer {
        fn render(&self, description: &Description, _target: &ReprType) -> Result<Representation> {
            Ok(Representation::new(
                ReprType::new("text/plain"),
                description.id.clone().into_bytes(),
            )
            .cacheable())
        }
    }

    #[test]
    fn meta_is_routed_through_the_renderer() {
        let space = EndpointSpace::new().bind(Exact::new("urn:fn:toUpper"), builtins::to_upper());
        let kernel = Kernel::with_meta_renderer(Arc::new(space), Arc::new(EchoIdRenderer));
        let cap = Capability::root();
        let rep =
            block_on(kernel.issue(Request::new(Verb::Meta, iri("urn:fn:toUpper")), &cap)).unwrap();
        assert_eq!(rep.bytes, b"toUpper");
    }

    #[test]
    fn meta_without_a_renderer_errors() {
        let space = EndpointSpace::new().bind(Exact::new("urn:fn:toUpper"), builtins::to_upper());
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();
        assert!(
            block_on(kernel.issue(Request::new(Verb::Meta, iri("urn:fn:toUpper")), &cap)).is_err()
        );
    }

    #[test]
    fn resolves_invokes_and_caches() {
        let space = EndpointSpace::new().bind(Exact::new("urn:fn:toUpper"), builtins::to_upper());
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();
        let req = || {
            Request::new(Verb::Source, iri("urn:fn:toUpper"))
                .with_arg("in", ArgRef::Inline(b"hi".to_vec()))
        };
        let a = block_on(kernel.issue(req(), &cap)).unwrap();
        let b = block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(a.bytes, b"HI");
        assert_eq!(a.bytes, b.bytes);
        assert_eq!(kernel.cache_len(), 1);
    }

    #[test]
    fn cacheable_endpoint_runs_only_once() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let counter = FnEndpoint::new("count", |_inv: &Invocation<'_>| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(Representation::new(ReprType::new("text/plain"), b"x".to_vec()).cacheable())
        });
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:fn:count"), counter),
        ));
        let cap = Capability::root();
        let req = || Request::new(Verb::Source, iri("urn:fn:count"));
        block_on(kernel.issue(req(), &cap)).unwrap();
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            1,
            "second issue should be a cache hit"
        );
    }

    #[test]
    fn is_cached_probes_without_resolving() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let counter = FnEndpoint::new("count", |_inv: &Invocation<'_>| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(Representation::new(ReprType::new("text/plain"), b"x".to_vec()).cacheable())
        });
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:fn:count"), counter),
        ));
        let cap = Capability::root();
        let req = || Request::new(Verb::Source, iri("urn:fn:count"));

        // Not cached before the first issue; probing does not resolve it.
        assert!(!kernel.is_cached(&req()));
        assert!(!kernel.is_cached(&req()));
        assert_eq!(CALLS.load(Ordering::SeqCst), 0, "is_cached must not invoke");
        assert_eq!(kernel.cache_len(), 0, "is_cached must not cache");

        // Cached after issuing once.
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert!(kernel.is_cached(&req()));

        // A different request (different argument identity) is still a miss.
        let other = Request::new(Verb::Source, iri("urn:fn:count"))
            .with_arg("in", ArgRef::Inline(b"z".to_vec()));
        assert!(!kernel.is_cached(&other));
    }

    #[test]
    fn uncacheable_endpoint_runs_every_time() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        // No `.cacheable()` -> default Expiry::Always -> never cached.
        let volatile = FnEndpoint::new("vol", |_inv: &Invocation<'_>| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"x".to_vec(),
            ))
        });
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:fn:vol"), volatile),
        ));
        let cap = Capability::root();
        let req = || Request::new(Verb::Source, iri("urn:fn:vol"));
        block_on(kernel.issue(req(), &cap)).unwrap();
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            2,
            "uncacheable result recomputes"
        );
    }

    #[test]
    fn unresolved_target_errors() {
        let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
        let cap = Capability::root();
        let err = block_on(kernel.issue(Request::new(Verb::Source, iri("urn:fn:nope")), &cap))
            .unwrap_err();
        assert!(matches!(err, Error::Unresolved(_)));
    }

    /// A composing endpoint: dereference the `src` by-reference argument and
    /// upper-case its content.
    struct UpcaseOf;

    #[async_trait::async_trait]
    impl Endpoint for UpcaseOf {
        async fn invoke(&self, cx: &Invocation<'_>) -> Result<Representation> {
            let src = match cx.request.args.get("src") {
                Some(ArgRef::Reference(iri)) => iri.clone(),
                _ => return Err(Error::MissingArgument("src".to_string())),
            };
            let upstream = cx.source(&src).await?;
            let upper = String::from_utf8_lossy(&upstream.bytes).to_uppercase();
            Ok(Representation::new(ReprType::new("text/plain"), upper.into_bytes()).cacheable())
        }
    }

    #[test]
    fn composes_over_a_referenced_resource_and_caches() {
        static GREETING_CALLS: AtomicU32 = AtomicU32::new(0);
        let greeting = FnEndpoint::new("greeting", |_cx: &Invocation<'_>| {
            GREETING_CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(Representation::new(ReprType::new("text/plain"), b"hello".to_vec()).cacheable())
        });
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:data:greeting"), greeting)
            .bind(Exact::new("urn:fn:upcaseOf"), UpcaseOf);
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();
        let req = || {
            Request::new(Verb::Source, iri("urn:fn:upcaseOf"))
                .with_arg("src", ArgRef::Reference(iri("urn:data:greeting")))
        };
        let a = block_on(kernel.issue(req(), &cap)).unwrap();
        let b = block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(a.bytes, b"HELLO");
        assert_eq!(a.bytes, b.bytes);
        // Both deps cacheable -> composed result cached -> greeting sourced once.
        assert_eq!(GREETING_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn volatile_dependency_forbids_caching_the_composer() {
        static CLOCK_CALLS: AtomicU32 = AtomicU32::new(0);
        // No `.cacheable()` -> volatile dependency.
        let clock = FnEndpoint::new("clock", |_cx: &Invocation<'_>| {
            CLOCK_CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"tick".to_vec(),
            ))
        });
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:data:clock"), clock)
            .bind(Exact::new("urn:fn:upcaseOf"), UpcaseOf);
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();
        let req = || {
            Request::new(Verb::Source, iri("urn:fn:upcaseOf"))
                .with_arg("src", ArgRef::Reference(iri("urn:data:clock")))
        };
        block_on(kernel.issue(req(), &cap)).unwrap();
        block_on(kernel.issue(req(), &cap)).unwrap();
        // Expiry propagates: volatile dep -> composer not cached -> clock sourced twice.
        assert_eq!(CLOCK_CALLS.load(Ordering::SeqCst), 2);
    }

    // --- golden threads --------------------------------------------------------

    #[test]
    fn cutting_a_thread_invalidates_the_entry_that_declared_it() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        // A cacheable read of mutable state: names the thread for that state.
        let file = FnEndpoint::new("file", |_cx: &Invocation<'_>| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(
                Representation::new(ReprType::new("text/plain"), b"v".to_vec())
                    .cacheable()
                    .depends_on("urn:file:notes.txt"),
            )
        });
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:file:notes.txt"), file),
        ));
        let cap = Capability::root();
        let req = || Request::new(Verb::Source, iri("urn:file:notes.txt"));

        block_on(kernel.issue(req(), &cap)).unwrap();
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            1,
            "cached on the second issue"
        );
        assert!(kernel.is_cached(&req()));

        // An external change (or a Sink) cuts the thread.
        kernel.cut("urn:file:notes.txt");
        assert!(!kernel.is_cached(&req()), "cut entry is no longer cached");
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(CALLS.load(Ordering::SeqCst), 2, "recomputed after the cut");
        // Re-cached at the new generation.
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            2,
            "cached again after recompute"
        );

        // Cutting an unrelated thread leaves it cached.
        kernel.cut("urn:file:other.txt");
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            2,
            "unrelated cut does not invalidate"
        );
    }

    #[test]
    fn a_thread_propagates_up_through_composition() {
        static LEAF: AtomicU32 = AtomicU32::new(0);
        let leaf = FnEndpoint::new("leaf", |_cx: &Invocation<'_>| {
            LEAF.fetch_add(1, Ordering::SeqCst);
            Ok(
                Representation::new(ReprType::new("text/plain"), b"hi".to_vec())
                    .cacheable()
                    .depends_on("urn:leaf"),
            )
        });
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:data:leaf"), leaf)
            .bind(Exact::new("urn:fn:upcaseOf"), UpcaseOf);
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();
        let req = || {
            Request::new(Verb::Source, iri("urn:fn:upcaseOf"))
                .with_arg("src", ArgRef::Reference(iri("urn:data:leaf")))
        };
        block_on(kernel.issue(req(), &cap)).unwrap();
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(LEAF.load(Ordering::SeqCst), 1, "composite + leaf cached");

        // The composite never declared `urn:leaf`; it inherited it by resolving the
        // leaf. Cutting it invalidates the composite anyway.
        kernel.cut("urn:leaf");
        let out = block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(out.bytes, b"HI");
        assert_eq!(
            LEAF.load(Ordering::SeqCst),
            2,
            "composite recomputed and re-sourced the leaf"
        );
    }

    /// A team resource: concatenates three member resources it resolves, and
    /// declares its own team-level thread.
    struct Team;

    #[async_trait::async_trait]
    impl Endpoint for Team {
        async fn invoke(&self, cx: &Invocation<'_>) -> Result<Representation> {
            let mut body = Vec::new();
            for member in ["urn:person:alice", "urn:person:bob", "urn:person:carol"] {
                body.extend_from_slice(&cx.source(&iri(member)).await?.bytes);
            }
            Ok(Representation::new(ReprType::new("text/plain"), body)
                .cacheable()
                .depends_on("urn:team:engineering"))
        }
    }

    #[test]
    fn cutting_one_member_invalidates_the_team_but_not_its_siblings() {
        static ALICE: AtomicU32 = AtomicU32::new(0);
        static BOB: AtomicU32 = AtomicU32::new(0);
        static CAROL: AtomicU32 = AtomicU32::new(0);
        fn person(thread: &'static str, body: &'static str, n: &'static AtomicU32) -> FnEndpoint {
            FnEndpoint::new("person", move |_cx: &Invocation<'_>| {
                n.fetch_add(1, Ordering::SeqCst);
                Ok(
                    Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec())
                        .cacheable()
                        .depends_on(thread),
                )
            })
        }
        let space = EndpointSpace::new()
            .bind(
                Exact::new("urn:person:alice"),
                person("urn:person:alice", "A", &ALICE),
            )
            .bind(
                Exact::new("urn:person:bob"),
                person("urn:person:bob", "B", &BOB),
            )
            .bind(
                Exact::new("urn:person:carol"),
                person("urn:person:carol", "C", &CAROL),
            )
            .bind(Exact::new("urn:team:engineering"), Team);
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();
        let team = || Request::new(Verb::Source, iri("urn:team:engineering"));

        let first = block_on(kernel.issue(team(), &cap)).unwrap();
        block_on(kernel.issue(team(), &cap)).unwrap();
        assert_eq!(first.bytes, b"ABC");
        let counts = || {
            (
                ALICE.load(Ordering::SeqCst),
                BOB.load(Ordering::SeqCst),
                CAROL.load(Ordering::SeqCst),
            )
        };
        assert_eq!(
            counts(),
            (1, 1, 1),
            "members resolved once; team then cached"
        );

        // Bob changes upstream. The team depends on every member individually.
        kernel.cut("urn:person:bob");
        let again = block_on(kernel.issue(team(), &cap)).unwrap();
        assert_eq!(again.bytes, b"ABC");
        // Team recomputes (it carried bob's thread) and re-resolves bob; alice and
        // carol are still valid in cache, so they are NOT recomputed.
        assert_eq!(counts(), (1, 2, 1), "only bob and the team recompute");

        // And a team-level cut hits only the team, not the members.
        kernel.cut("urn:team:engineering");
        block_on(kernel.issue(team(), &cap)).unwrap();
        assert_eq!(
            counts(),
            (1, 2, 1),
            "roster change re-runs the team, reuses members"
        );
    }

    /// A stateful resource: `Source` reads the current value (cacheable, declaring
    /// the thread named after itself); `Sink` replaces it.
    struct Cell {
        value: Mutex<Vec<u8>>,
    }

    #[async_trait::async_trait]
    impl Endpoint for Cell {
        async fn invoke(&self, cx: &Invocation<'_>) -> Result<Representation> {
            match cx.request.verb {
                Verb::Source => {
                    let value = self.value.lock().expect("cell").clone();
                    Ok(Representation::new(ReprType::new("text/plain"), value)
                        .cacheable()
                        .depends_on(cx.request.target.as_str()))
                }
                Verb::Sink => {
                    *self.value.lock().expect("cell") = cx.inline_arg("in")?.to_vec();
                    Ok(Representation::new(
                        ReprType::new("text/plain"),
                        b"ok".to_vec(),
                    ))
                }
                other => Err(Error::Endpoint(format!("cell: unsupported {other:?}"))),
            }
        }
    }

    #[test]
    fn a_sink_invalidates_the_cached_source_of_its_target() {
        let cell = Cell {
            value: Mutex::new(b"v1".to_vec()),
        };
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:data:cell"), cell),
        ));
        let cap = Capability::root();
        let source = || Request::new(Verb::Source, iri("urn:data:cell"));

        // Read v1; it caches, declaring the `urn:data:cell` thread.
        assert_eq!(block_on(kernel.issue(source(), &cap)).unwrap().bytes, b"v1");
        assert!(kernel.is_cached(&source()), "source is cached");

        // Write v2 through the kernel: the mutating verb auto-cuts `urn:data:cell`.
        let sink = Request::new(Verb::Sink, iri("urn:data:cell"))
            .with_arg("in", ArgRef::Inline(b"v2".to_vec()));
        block_on(kernel.issue(sink, &cap)).unwrap();
        assert!(
            !kernel.is_cached(&source()),
            "the write invalidated the cached read"
        );

        // Read again: the cache recomputes and sees v2.
        assert_eq!(block_on(kernel.issue(source(), &cap)).unwrap().bytes, b"v2");
    }
}
