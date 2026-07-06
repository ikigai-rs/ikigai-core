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
//! in — permanently ([`Expiry::Never`]) or until a deadline ([`Expiry::At`]).
//! Validity is then tracked two ways, both of which must hold: **golden threads**
//! ([`Thread`]) — a cached representation depends on the threads it declared plus
//! those of every sub-resource it resolved (they propagate up through
//! composition), and [`Kernel::cut`] invalidates everything carrying a thread, so
//! results which read mutable state can be cached and invalidated on change (a
//! `Sink`, or an external watcher, cuts the thread named after the state) — and a
//! **time deadline**, evaluated against an injected [`Clock`], after which an
//! `At` entry is recomputed. A kernel with no clock simply never caches `At`
//! results, staying fully time-independent (and so deterministic for replay).
//!
//! The kernel also reserves the **`urn:kernel:*`** namespace for its own
//! operations as capability-gated resources, resolved intrinsically before the
//! root space: `sink urn:kernel:cut <thread>` cuts a thread (so an endpoint or a
//! remote peer can invalidate by *resolving*, not via a special method), and
//! `source urn:kernel:cache` / `urn:kernel:threads` introspect cache and threads.
//! `source urn:kernel:catalog` is the self-describing endpoint graph, and
//! `source urn:kernel:actions types=<classes>` lists the endpoints those typed entities
//! could drive (selection as a resource).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;

use crate::arg::ArgRef;
use crate::capability::Capability;
use crate::describe::Description;
use crate::endpoint::{Invocation, Issuer, Spawner};
use crate::error::{Error, Result};
use crate::iri::Iri;
use crate::meta::MetaRenderer;
use crate::repr::{Expiry, Provenance, ReprType, Representation, Thread, Time};
use crate::request::{Request, RequestId};
use crate::select::{ActionMatch, TransreptionStep};
use crate::space::{Resolution, Scope, Space, SpaceEntry};
use crate::verb::Verb;

/// The kernel's source of "now". Injected (rather than read from the system
/// directly) so pure resolution stays deterministic and replayable: a test or a
/// replay harness supplies a fixed or recorded clock. Only entries with a
/// time-based [`Expiry::At`] deadline consult it, so a kernel with no clock is
/// fully time-independent.
pub trait Clock: Send + Sync {
    /// The current time.
    fn now(&self) -> Time;
}

/// A [`Clock`] reading the system wall clock — the default real clock. Replay and
/// test harnesses inject a fixed clock instead. (A browser host injects a
/// `Date.now()`-backed clock; `ikigai-core` itself stays runtime-agnostic.)
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Time {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Time::from_millis(millis)
    }
}

/// One resolved invocation, as the kernel reports it to an installed [`Tracer`].
/// The `trace` command turns a stream of these (from one real resolution) into the
/// execution tree — which **worker thread** each node ran on, how long it took, and
/// whether the cache served it.
///
/// Serializable so a traced resolution can cross the wire: a remote kernel records
/// its own events and ships them back to be stitched into the caller's tree.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TraceEvent {
    /// The resolved request's target IRI.
    pub target: String,
    /// The worker thread the invocation ran on (the scheduler's thread name, or the
    /// thread id) — so a fan-out's branches show up on different workers.
    pub thread: String,
    /// When the invocation started / finished, per the injected [`Clock`] (`None`
    /// if the kernel has no clock).
    pub started: Option<Time>,
    pub ended: Option<Time>,
    /// Whether the representation cache served it (vs computed now).
    pub cache_hit: bool,
    /// This invocation's span id (unique within one traced resolution).
    pub span: u64,
    /// The span of the invocation that issued this one — `None` for the root. The
    /// `(span, parent)` edges reconstruct the real execution tree, including the
    /// concurrent branches a `fan_out` spawns onto different workers.
    pub parent: Option<u64>,
    /// The authority this invocation ran under: `None` for root (full authority),
    /// else the sorted scope set. So an attenuation down the call chain — or across
    /// a grant boundary — is visible in the tree, node by node: "under which
    /// capability" alongside "against which cache state".
    pub capability: Option<Vec<String>>,
}

/// Receives a [`TraceEvent`] per invocation while installed. The kernel records
/// only when one is set ([`Kernel::set_tracer`]) — off the hot path otherwise — so
/// the `trace` command can capture one real resolution and render it. The host
/// supplies the collector (e.g. a `Mutex<Vec<TraceEvent>>`).
pub trait Tracer: Send + Sync {
    /// Record one resolved invocation.
    fn record(&self, event: TraceEvent);
}

/// A read-only view of the host scheduler's live state, injected into the kernel
/// like a [`Clock`] so `urn:kernel:scheduler` can report it **intrinsically** — over
/// the wire and without the host binding an endpoint, uniform with `urn:kernel:cache`
/// and `urn:kernel:threads`. The kernel never *drives* the scheduler (which lives
/// above it, runtime-free); it only reads these rows. With none injected,
/// `urn:kernel:scheduler` reports the single-threaded default.
pub trait SchedulerReporter: Send + Sync {
    /// Label/value rows describing the scheduler (backend, thread count, task
    /// counters, …). The kernel renders them verbatim, so a host can add rows
    /// (queue depth, per-resource executors) without a core change.
    fn rows(&self) -> Vec<(String, String)>;
}

/// The current worker thread's label: its name (the scheduler names workers
/// `ikigai-sched-N`) or, unnamed, its id.
fn thread_label() -> String {
    let current = std::thread::current();
    current
        .name()
        .map(str::to_string)
        .unwrap_or_else(|| format!("{:?}", current.id()))
}

/// Resolves requests against a root space, invokes the resolved endpoint, and
/// caches cacheable representations by their content-addressed request id.
pub struct Kernel {
    root: Arc<dyn Space>,
    /// Cacheable representations, keyed by `(request id, capability fingerprint)`. The
    /// capability is part of the key so a cached entry is only ever served back to the
    /// same authority that computed it — a different (e.g. narrower) capability misses
    /// and must pass the endpoint's own check. Without this, a cache hit is served
    /// *before* the endpoint runs (see [`issue_inner`](Self::issue_inner)), which would
    /// skip the capability check and let one authority read another's cached result.
    cache: Mutex<HashMap<(RequestId, u64), CacheEntry>>,
    /// Current generation of each golden thread (absent ⇒ generation 0).
    /// [`Kernel::cut`] bumps a thread's generation, invalidating every cache entry
    /// pinned to an earlier one.
    generations: Mutex<HashMap<Thread, u64>>,
    meta: Option<Arc<dyn MetaRenderer>>,
    /// Source of "now" for time-based [`Expiry::At`] deadlines. Absent ⇒ the
    /// kernel cannot evaluate a deadline, so it declines to cache `At` results
    /// (golden-thread and `Never` caching are unaffected).
    clock: Option<Arc<dyn Clock>>,
    /// Host executor for concurrent fan-out, and a weak self-handle the kernel hands
    /// to invocations as an owned [`Issuer`] so a spawned sub-request can re-enter the
    /// kernel without borrowing the invocation. Both set by [`into_scheduled`](Self::into_scheduled);
    /// absent ⇒ [`Invocation::fan_out`] resolves sequentially (the default).
    spawner: Option<Arc<dyn Spawner>>,
    self_ref: Option<Weak<Kernel>>,
    /// Execution tracer, installed only while the `trace` command captures one
    /// resolution. `tracing` is a fast-path gate so an untraced issue pays just an
    /// atomic load, never the lock.
    tracer: Mutex<Option<Arc<dyn Tracer>>>,
    tracing: AtomicBool,
    /// Monotonic span-id source, advanced once per traced invocation so each node
    /// gets a unique id and its children can name it as their parent.
    span_counter: AtomicU64,
    /// Read-only handle to the host scheduler, for `urn:kernel:scheduler`. Injected
    /// by a scheduled host (the [`Clock`] pattern); absent ⇒ single-threaded default.
    scheduler: Option<Arc<dyn SchedulerReporter>>,
    /// Rolling window of the most recent resolutions (target, time, cache outcome),
    /// for `urn:kernel:constraint` — the kernel's throughput readout. Always-on like
    /// the cache; bounded to [`CONSTRAINT_WINDOW`]. `urn:kernel:*` introspection
    /// requests are not recorded (they short-circuit before the resolution body).
    constraint: Mutex<VecDeque<ResolutionSample>>,
    /// `rdfs:subClassOf*` closure for type-aware [`select_action`](Self::select_action):
    /// each class mapped to itself plus all its (transitive) superclasses. Set from a
    /// vocabulary/alignment graph via [`with_subclass_axioms`](Self::with_subclass_axioms);
    /// empty ⇒ exact-class matching (the default, no inference).
    subclass_closure: BTreeMap<String, BTreeSet<String>>,
}

/// How many recent resolutions `urn:kernel:constraint` aggregates over.
const CONSTRAINT_WINDOW: usize = 512;

/// One sampled resolution behind `urn:kernel:constraint`: which resource, how long
/// it took to compute (per the injected [`Clock`]; `None` if the kernel has none),
/// and whether the cache served it (a hit consumes no constraint capacity).
struct ResolutionSample {
    target: String,
    elapsed_ms: Option<u64>,
    cache_hit: bool,
}

/// A cached representation plus the golden-thread edges that keep it valid: each
/// `(thread, generation)` records the generation that thread held when the entry
/// was stored. The entry is valid only while every thread is still at that
/// generation — cut any of them and it's stale.
struct CacheEntry {
    /// The IRI of the request that produced this entry — kept so `urn:kernel:cache`
    /// can name what's cached (the cache is otherwise keyed by a content hash).
    target: String,
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
            clock: None,
            spawner: None,
            self_ref: None,
            tracer: Mutex::new(None),
            tracing: AtomicBool::new(false),
            span_counter: AtomicU64::new(0),
            scheduler: None,
            constraint: Mutex::new(VecDeque::new()),
            subclass_closure: BTreeMap::new(),
        }
    }

    /// A kernel that answers `Meta` requests by rendering through `renderer`.
    pub fn with_meta_renderer(root: Arc<dyn Space>, renderer: Arc<dyn MetaRenderer>) -> Self {
        Kernel {
            root,
            cache: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            meta: Some(renderer),
            clock: None,
            spawner: None,
            self_ref: None,
            tracer: Mutex::new(None),
            tracing: AtomicBool::new(false),
            span_counter: AtomicU64::new(0),
            scheduler: None,
            constraint: Mutex::new(VecDeque::new()),
            subclass_closure: BTreeMap::new(),
        }
    }

    /// Find a transreptor chain converting `from` → `to` among this kernel's mounted
    /// endpoints — a direct single hop, else a two-hop pivot via `text/turtle`. `None` if
    /// no auto-invocable chain exists. The basis for selection-driven metadata rendering,
    /// content negotiation, and sniff-and-dispatch. See [`crate::select_transreptor`].
    pub fn select_transreptor(&self, from: &str, to: &str) -> Option<Vec<TransreptionStep>> {
        crate::select::select_transreptor(self.root.as_ref(), from, to)
    }

    /// Install an `rdfs:subClassOf` axiom set — `(subclass, superclass)` IRI pairs, e.g.
    /// from a vocabulary or alignment graph — so [`select_action`](Self::select_action)
    /// reasons over the type hierarchy: an entity typed `foaf:Person` then satisfies an
    /// action requiring `schema:Person` when `foaf:Person rdfs:subClassOf schema:Person`.
    /// Builder; the transitive closure (each class → itself + all superclasses) is computed
    /// once here. Without it, matching is exact-class (no inference).
    pub fn with_subclass_axioms<I, A, B>(mut self, axioms: I) -> Self
    where
        I: IntoIterator<Item = (A, B)>,
        A: Into<String>,
        B: Into<String>,
    {
        // Direct sub → {super} adjacency, and the set of every class named.
        let mut direct: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut classes: BTreeSet<String> = BTreeSet::new();
        for (sub, sup) in axioms {
            let (sub, sup) = (sub.into(), sup.into());
            classes.insert(sub.clone());
            classes.insert(sup.clone());
            direct.entry(sub).or_default().insert(sup);
        }
        // Transitive closure incl. self: DFS up the hierarchy from each class (the visited
        // set also guards against cycles).
        let mut closure: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for class in &classes {
            let mut supers = BTreeSet::new();
            let mut stack = vec![class.clone()];
            while let Some(c) = stack.pop() {
                if supers.insert(c.clone()) {
                    if let Some(parents) = direct.get(&c) {
                        stack.extend(parents.iter().cloned());
                    }
                }
            }
            closure.insert(class.clone(), supers);
        }
        self.subclass_closure = closure;
        self
    }

    /// Find endpoints among this kernel's mounted endpoints whose required inputs are
    /// satisfiable by the RDF classes in `present` — the actions available given a set of
    /// typed entities. With an `rdfs:subClassOf` closure installed (see
    /// [`with_subclass_axioms`](Self::with_subclass_axioms)) the present types are first
    /// expanded through it, so a subclass satisfies an action requiring any of its
    /// superclasses. See [`crate::select_action`].
    pub fn select_action(&self, present: &[&str]) -> Vec<ActionMatch> {
        let expanded = self.expand_present(present);
        let refs: Vec<&str> = expanded.iter().map(String::as_str).collect();
        crate::select::select_action(self.root.as_ref(), &refs)
    }

    /// The action-level selection funnel over this kernel's bindings (see
    /// [`crate::select::select_actions`]): capability-scoped, verb- and output-aware,
    /// with the present types expanded through the `rdfs:subClassOf` closure first.
    pub fn select_actions(&self, query: &crate::select::ActionQuery<'_>) -> Vec<ActionMatch> {
        let expanded = self.expand_present(query.present);
        let refs: Vec<&str> = expanded.iter().map(String::as_str).collect();
        let expanded_query = crate::select::ActionQuery {
            present: &refs,
            ..*query
        };
        crate::select::select_actions(self.root.as_ref(), &expanded_query)
    }

    /// The typed self-description of the endpoint bound at `iri`, or `None` if the
    /// IRI resolves to nothing. Returns the [`Description`] struct — not the
    /// rendered Meta representation — so a caller projecting the catalog or the
    /// action manifold (e.g. an MCP tool list) gets typed [`ArgSpec`](crate::ArgSpec)
    /// contracts without re-parsing Turtle. The companion to [`select_actions`](Self::select_actions):
    /// list the allowed actions, then `describe` each endpoint for its schema.
    pub fn describe(&self, iri: &Iri) -> Option<Description> {
        match self
            .root
            .resolve(&Request::new(Verb::Meta, iri.clone()), &Scope::empty())
        {
            Resolution::Hit(resolved) => Some(resolved.endpoint.describe()),
            _ => None,
        }
    }

    /// Find a bound endpoint's description by its `Description::id` (the catalog
    /// subject identity) — the reverse of the entries → describe walk.
    fn description_for_id(&self, id: &str) -> Result<Option<Description>> {
        for entry in self.root.entries().unwrap_or_default() {
            let Ok(iri) = Iri::parse(&entry.pattern) else {
                continue;
            };
            if let Resolution::Hit(resolved) = self
                .root
                .resolve(&Request::new(Verb::Meta, iri), &Scope::empty())
            {
                let description = resolved.endpoint.describe();
                if description.id == id {
                    return Ok(Some(description));
                }
            }
        }
        Ok(None)
    }

    /// Expand each present type to itself + its superclasses (`rdfs:subClassOf*`); an
    /// unknown type maps to just itself. No closure installed ⇒ identity.
    fn expand_present(&self, present: &[&str]) -> Vec<String> {
        if self.subclass_closure.is_empty() {
            return present.iter().map(|p| p.to_string()).collect();
        }
        let mut expanded: BTreeSet<String> = BTreeSet::new();
        for &p in present {
            match self.subclass_closure.get(p) {
                Some(supers) => expanded.extend(supers.iter().cloned()),
                None => {
                    expanded.insert(p.to_string());
                }
            }
        }
        expanded.into_iter().collect()
    }

    /// Render `description` to `target` by transrepting its canonical Turtle: render the
    /// Turtle, [`select`](Self::select_transreptor) a transreptor chain `text/turtle →
    /// target`, and run it (piping `content`, setting `as`) through the kernel. Falls back
    /// to the canonical Turtle if no transreptor reaches `target`. Used by the `Meta` path
    /// for any type the renderer doesn't emit directly.
    async fn transrept_meta(
        &self,
        description: &Description,
        target: &ReprType,
        capability: &Capability,
    ) -> Result<Representation> {
        let renderer = self
            .meta
            .as_ref()
            .ok_or_else(|| Error::Endpoint("no Meta renderer configured".to_string()))?;
        let canonical = renderer.render(description, &ReprType::new(crate::select::CANONICAL))?;
        let Some(plan) = self.select_transreptor(crate::select::CANONICAL, &target.media_type)
        else {
            // Nothing converts Turtle to the requested type — hand back the canonical Turtle.
            return Ok(canonical.cacheable());
        };
        let mut current = canonical;
        for step in plan {
            let iri = Iri::parse(&step.endpoint).map_err(|e| {
                Error::Endpoint(format!("bad transreptor IRI `{}`: {e}", step.endpoint))
            })?;
            let request = Request::new(Verb::Source, iri)
                .with_arg("content", ArgRef::Inline(current.bytes))
                .with_arg("as", ArgRef::Inline(step.to.into_bytes()));
            // Box the re-entrant issue: the async call graph (issue → transrept_meta →
            // issue) is a cycle the compiler must size via indirection, even though at
            // runtime these are plain `Source` transreptions, never `Meta`.
            current = Box::pin(self.issue(request, capability)).await?;
        }
        Ok(current.cacheable())
    }

    /// Make this kernel **schedulable**: wrap it in an `Arc` and inject `spawner`, so
    /// re-entrant fan-out ([`Invocation::fan_out`]) runs concurrently on the host's
    /// executor instead of sequentially. Uses [`Arc::new_cyclic`] to store a weak
    /// self-handle the kernel hands to invocations as an owned [`Issuer`], so a
    /// spawned sub-request can re-enter the kernel without borrowing the invocation.
    /// (Chain after the other builders, e.g.
    /// `Kernel::with_meta_renderer(..).with_clock(..).into_scheduled(spawner)`.)
    pub fn into_scheduled(self, spawner: Arc<dyn Spawner>) -> Arc<Kernel> {
        Arc::new_cyclic(|weak: &Weak<Kernel>| {
            let mut kernel = self;
            kernel.spawner = Some(spawner);
            kernel.self_ref = Some(weak.clone());
            kernel
        })
    }

    /// Inject the [`Clock`] the kernel reads for time-based [`Expiry::At`]
    /// deadlines (builder). Without one, `At` results are not cached — golden-thread
    /// and `Never` caching are unaffected — so a kernel only consults a clock when a
    /// host opts into time-bounded freshness (e.g. mounting the HTTP client module).
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Inject a [`SchedulerReporter`] so `urn:kernel:scheduler` reports the host
    /// scheduler's live state (builder). Without one it reports the single-threaded
    /// default. A scheduled host injects this alongside
    /// [`into_scheduled`](Self::into_scheduled).
    pub fn with_scheduler_reporter(mut self, reporter: Arc<dyn SchedulerReporter>) -> Self {
        self.scheduler = Some(reporter);
        self
    }

    /// Install a [`Tracer`] to record each invocation of the *next* resolution. The
    /// `trace` command sets one around a single real `source`, then calls
    /// [`clear_tracer`](Self::clear_tracer). Takes `&self` (interior-mutable), so it
    /// works on the shared `Arc<Kernel>` the engine drives. Off the hot path when no
    /// tracer is set — an untraced issue pays only an atomic load.
    pub fn set_tracer(&self, tracer: Arc<dyn Tracer>) {
        *self.tracer.lock().expect("tracer lock") = Some(tracer);
        self.tracing.store(true, Ordering::SeqCst);
    }

    /// Remove the installed tracer.
    pub fn clear_tracer(&self) {
        self.tracing.store(false, Ordering::SeqCst);
        *self.tracer.lock().expect("tracer lock") = None;
    }

    /// The clock's "now", or `None` if the kernel has no clock — the start stamp
    /// shared by a [`TraceEvent`] and the always-on `urn:kernel:constraint` window.
    fn now_stamp(&self) -> Option<Time> {
        self.clock.as_ref().map(|clock| clock.now())
    }

    /// Record one resolution into the rolling constraint window (always-on, bounded
    /// to [`CONSTRAINT_WINDOW`]). `started` is the pre-resolution stamp; the elapsed
    /// compute time is `now − started` (only when the kernel has a clock). A cache
    /// hit is logged but its elapsed is left out of the constraint total.
    fn record_resolution(&self, request: &Request, started: Option<Time>, cache_hit: bool) {
        let elapsed_ms = match (started, self.clock.as_ref()) {
            (Some(start), Some(clock)) => {
                Some(clock.now().as_millis().saturating_sub(start.as_millis()))
            }
            _ => None,
        };
        let mut window = self.constraint.lock().expect("constraint lock");
        if window.len() >= CONSTRAINT_WINDOW {
            window.pop_front();
        }
        window.push_back(ResolutionSample {
            target: request.target.as_str().to_string(),
            elapsed_ms,
            cache_hit,
        });
    }

    /// A fresh span id for an invocation while tracing; `None` off the trace path,
    /// so an untraced issue never touches the counter.
    fn next_span(&self) -> Option<u64> {
        if self.tracing.load(Ordering::Relaxed) {
            Some(self.span_counter.fetch_add(1, Ordering::Relaxed))
        } else {
            None
        }
    }

    /// Report one resolved invocation to the installed tracer, if any — tagged with
    /// its own `span` and its issuer's `parent` span, so the events form a tree.
    fn trace_record(
        &self,
        request: &Request,
        capability: &Capability,
        span: Option<u64>,
        parent: Option<u64>,
        started: Option<Time>,
        cache_hit: bool,
    ) {
        if !self.tracing.load(Ordering::Relaxed) {
            return;
        }
        if let Some(tracer) = self.tracer.lock().expect("tracer lock").clone() {
            tracer.record(TraceEvent {
                target: request.target.as_str().to_string(),
                thread: thread_label(),
                started,
                ended: self.clock.as_ref().map(|clock| clock.now()),
                cache_hit,
                span: span.unwrap_or(0),
                parent,
                // The authority this hop ran under, mirrored from `Capability::scopes`
                // (None = root). Recorded only while tracing, so the clone is off the
                // hot path.
                capability: capability.scopes().map(|s| s.iter().cloned().collect()),
            });
        }
    }

    /// Issue a request: return a valid cached representation if one exists,
    /// otherwise resolve, invoke the endpoint, and cache the result if cacheable.
    pub async fn issue(&self, request: Request, capability: &Capability) -> Result<Representation> {
        // Top-level entry: no parent span (this is a trace root if one is recording).
        self.issue_inner(request, capability, None, None).await
    }

    /// Issue a request whose input was produced by an upstream pipe stage, folding
    /// that upstream's [`Provenance`] (expiry + golden threads) into the result's
    /// effective cacheability. So `source <X> | transform` is no more cacheable than
    /// `X` — cacheability flows down the pipe — and cutting `X`'s thread invalidates
    /// the transformed result too. The engine passes this for each pipe stage; plain
    /// [`issue`](Self::issue) is the no-upstream case.
    pub async fn issue_with_incoming(
        &self,
        request: Request,
        capability: &Capability,
        incoming: Provenance,
    ) -> Result<Representation> {
        self.issue_inner(request, capability, None, Some(incoming))
            .await
    }

    /// The resolution path, carrying the `parent` span of the invocation that issued
    /// this request (`None` at the top level) and any upstream pipe [`Provenance`].
    /// [`Issuer::issue_with_parent`] threads the span through re-entrant sub-requests
    /// so a recorded run links each node to its parent; the public
    /// [`issue`](Self::issue) is the parentless, no-upstream entry point.
    async fn issue_inner(
        &self,
        request: Request,
        capability: &Capability,
        parent: Option<u64>,
        incoming: Option<Provenance>,
    ) -> Result<Representation> {
        // The kernel-behavior namespace (`urn:kernel:*`) is resolved by the kernel
        // itself — before the root space, which cannot shadow it — exposing the
        // kernel's own operations (cut a thread, inspect cache and threads) as
        // capability-gated resources. A *cacheable* builtin (the catalog) still
        // participates in the cache, so a re-resolution serves `[cached]`; the live
        // introspection builtins return `Always` and are simply never stored.
        if let Some(op) = request.target.as_str().strip_prefix(KERNEL_NS) {
            let id = request.id();
            let cap_key = capability_key(capability);
            let cacheable_verb = request.verb.is_cacheable();
            if cacheable_verb {
                if let Some(cached) = self.valid_cached(&id, cap_key) {
                    return Ok(cached);
                }
            }
            let representation = self.issue_kernel(op, &request, capability)?;
            let storable = cacheable_verb
                && match representation.expiry {
                    Expiry::Always => false,
                    Expiry::Never => true,
                    Expiry::At(_) => self.clock.is_some(),
                };
            if storable {
                let edges = self.edges_for(representation.threads());
                self.cache.lock().expect("cache lock").insert(
                    (id, cap_key),
                    CacheEntry {
                        target: request.target.as_str().to_string(),
                        representation: representation.clone(),
                        edges,
                    },
                );
            }
            return Ok(representation);
        }

        let id = request.id();
        let cacheable_verb = request.verb.is_cacheable();
        // Start stamp, shared by the trace event and the always-on constraint window
        // (one clock read); the span only advances while tracing.
        let started = self.now_stamp();
        let span = self.next_span();

        // Representation-cache lookup (idempotent verbs only): serve a cached entry
        // whose golden-thread edges are all still current. A cut entry is evicted
        // here and recomputed below. The guard is dropped before any await.
        let cap_key = capability_key(capability);
        if cacheable_verb {
            if let Some(cached) = self.valid_cached(&id, cap_key) {
                self.trace_record(&request, capability, span, parent, started, true);
                self.record_resolution(&request, started, true);
                return Ok(cached);
            }
        }

        // Resolution is synchronous, pure routing.
        let resolved = match self.root.resolve(&request, &Scope::empty()) {
            Resolution::Hit(resolved) => resolved,
            Resolution::Miss => return Err(Error::Unresolved(request.target.clone())),
        };

        let representation = if request.verb == Verb::Meta {
            // Selection-driven Meta: the renderer emits the endpoint's description in its
            // *canonical* serializations (Turtle, and JSON/text where the renderer supports
            // them). Any other requested type is produced by transrepting the canonical
            // Turtle through a *selected* transreptor chain — so metadata rendering rides
            // the same transreptor model as everything else, with no per-format logic and no
            // hardcoded transreptor IRIs here.
            let renderer = self
                .meta
                .as_ref()
                .ok_or_else(|| Error::Endpoint("no Meta renderer configured".to_string()))?;
            let description = resolved.endpoint.describe();
            let target = meta_target(&request);
            match renderer.render(&description, &target) {
                Ok(repr) => repr.cacheable(),
                // The renderer doesn't emit this type directly — transrept the canonical
                // Turtle to it.
                Err(_) => {
                    self.transrept_meta(&description, &target, capability)
                        .await?
                }
            }
        } else {
            // Invocation is asynchronous. On a scheduled kernel, hand it the spawner
            // and an owned self-handle so re-entrant fan-out runs concurrently.
            let issuer_arc: Option<Arc<dyn Issuer>> = self
                .self_ref
                .as_ref()
                .and_then(Weak::upgrade)
                .map(|kernel| {
                    let issuer: Arc<dyn Issuer> = kernel;
                    issuer
                });
            let invocation =
                Invocation::with_issuer(&request, &resolved.bindings, capability, self)
                    .with_concurrency(self.spawner.clone(), issuer_arc)
                    .with_span(span);
            let representation = resolved.endpoint.invoke(&invocation).await?;
            // Effective expiry propagates from the dependencies: the result is no
            // fresher than its most volatile part. The endpoint's own expiry is met
            // with the combined dependency expiry — `Always` if either is volatile,
            // the earlier deadline if both are time-bounded, `Never` only if all are.
            let mut effective = representation
                .expiry
                .most_restrictive(invocation.dependency_expiry());
            // Golden threads propagate too: the result depends on its own declared
            // threads plus those of every sub-resource it resolved, so cutting any
            // of them invalidates this composite.
            let mut threads = representation.threads().clone();
            threads.extend(invocation.dependency_threads());
            // …and from the pipe: an upstream stage's provenance folds in the same
            // way, so a transform over a piped input is no more cacheable than that
            // input, and inherits its threads.
            if let Some(incoming) = incoming {
                effective = effective.most_restrictive(incoming.expiry);
                threads.extend(incoming.threads);
            }
            representation.with_expiry(effective).with_threads(threads)
        };
        // Computed (not served from cache) — record after the invocation completes.
        self.trace_record(&request, capability, span, parent, started, false);
        self.record_resolution(&request, started, false);

        // A successful mutating verb invalidates its target: cut the thread named
        // after it, so cached `Source`s of that resource — and composites over
        // them — recompute. This is the internal half of the golden thread (the
        // kernel owns invalidation on writes); an external watcher cuts the same
        // thread on an out-of-band change.
        if request.verb.is_mutating() {
            self.cut(request.target.as_str());
        }

        // Store an idempotent result unless it's volatile (`Always`). A time-based
        // `At` deadline is only storable when a clock is present to later evaluate
        // it — otherwise the kernel could never tell when it expired, so it declines.
        let storable = cacheable_verb
            && match representation.expiry {
                Expiry::Always => false,
                Expiry::Never => true,
                Expiry::At(_) => self.clock.is_some(),
            };
        if storable {
            let edges = self.edges_for(representation.threads());
            self.cache.lock().expect("cache lock").insert(
                (id, cap_key),
                CacheEntry {
                    target: request.target.as_str().to_string(),
                    representation: representation.clone(),
                    edges,
                },
            );
        }
        Ok(representation)
    }

    /// Whether a cache entry is valid *right now*: its golden-thread edges are all
    /// still at their pinned generation (nothing it depends on has been cut) AND,
    /// if it carries a time deadline, that deadline is still in the future per the
    /// injected clock. (`At` is only ever stored when a clock is present, so a
    /// missing clock here conservatively treats a deadline as expired.) The single
    /// source of truth shared by the serving path ([`valid_cached`](Self::valid_cached),
    /// which evicts on staleness) and the read-only probe ([`is_cached`](Self::is_cached)),
    /// so the two can never disagree.
    fn entry_is_valid(&self, entry: &CacheEntry) -> bool {
        let gens = self.generations.lock().expect("generations lock");
        let edges_current = entry
            .edges
            .iter()
            .all(|(thread, gen)| generation_of(&gens, thread) == *gen);
        let unexpired = match entry.representation.expiry {
            Expiry::At(deadline) => self.clock.as_ref().is_some_and(|c| c.now() < deadline),
            _ => true,
        };
        edges_current && unexpired
    }

    /// A cached representation for `id` that is still [valid](Self::entry_is_valid).
    /// A stale entry (a thread cut, or its deadline passed) is evicted and `None`
    /// returned, so the caller recomputes.
    fn valid_cached(&self, id: &RequestId, cap_key: u64) -> Option<Representation> {
        let mut cache = self.cache.lock().expect("cache lock");
        let key = (*id, cap_key);
        let outcome = cache
            .get(&key)
            .map(|entry| (self.entry_is_valid(entry), entry.representation.clone()));
        match outcome {
            None => None,
            Some((true, representation)) => Some(representation),
            Some((false, _)) => {
                cache.remove(&key);
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

    /// Resolve a `urn:kernel:*` request — a kernel operation exposed as a
    /// capability-gated resource. `op` is the suffix after `urn:kernel:`.
    ///
    /// These are deliberately resources, not just Rust methods: an endpoint can
    /// invalidate another resource by *resolving* `urn:kernel:cut` (no special
    /// `Issuer` method); a remote peer can do the same over the wire, gated by its
    /// capability; and `describe`/the dashboard can see them. Results are live
    /// kernel state, so they are uncacheable.
    fn issue_kernel(
        &self,
        op: &str,
        request: &Request,
        capability: &Capability,
    ) -> Result<Representation> {
        match (op, request.verb) {
            // Cut a golden thread. The thread is the sunk content — so
            // `sink urn:kernel:cut <thread>` works — or an explicit `thread` arg.
            ("cut", Verb::Sink) => {
                require_cap(capability, "urn:cap:kernel:cut")?;
                let thread = kernel_arg(request, "thread")
                    .or_else(|| kernel_arg(request, "content"))
                    .ok_or_else(|| Error::MissingArgument("thread".to_string()))?;
                self.cut(thread);
                Ok(kernel_text(format!("cut {thread}\n")))
            }
            // Inspect the cache: a count, then one line per entry — the IRI it was
            // resolved from, its representation type and size, and how many golden
            // threads it depends on (cut any of them and this entry recomputes).
            ("cache", Verb::Source) => {
                require_cap(capability, "urn:cap:kernel:inspect")?;
                let mut rows: Vec<(String, String, usize, usize)> = {
                    let cache = self.cache.lock().expect("cache lock");
                    cache
                        .values()
                        .map(|e| {
                            (
                                e.target.clone(),
                                e.representation.repr_type.media_type.clone(),
                                e.representation.bytes.len(),
                                e.edges.len(),
                            )
                        })
                        .collect()
                };
                rows.sort();
                let mut body = format!("cache\n  entries  {}\n", rows.len());
                let width = rows
                    .iter()
                    .map(|r| r.0.chars().count())
                    .max()
                    .unwrap_or(0)
                    .min(48);
                for (target, media, size, deps) in rows {
                    let deps = if deps == 1 {
                        "1 thread".to_string()
                    } else {
                        format!("{deps} threads")
                    };
                    let size = human_size(size);
                    body.push_str(&format!(
                        "  {target:<width$}  {media:<24}  {size:>9}  {deps}\n"
                    ));
                }
                Ok(kernel_text(body))
            }
            // Inspect the golden threads that have been cut, and how many times.
            ("threads", Verb::Source) => {
                require_cap(capability, "urn:cap:kernel:inspect")?;
                let gens = self.generations.lock().expect("generations lock");
                let mut body = String::from("threads (cut generations)\n");
                if gens.is_empty() {
                    body.push_str("  (none cut)\n");
                } else {
                    let mut rows: Vec<_> = gens.iter().collect();
                    rows.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
                    for (thread, generation) in rows {
                        body.push_str(&format!("  {}  gen {generation}\n", thread.as_str()));
                    }
                }
                Ok(kernel_text(body))
            }
            // Inspect the host scheduler (backend, threads, live task counts).
            ("scheduler", Verb::Source) => {
                require_cap(capability, "urn:cap:kernel:inspect")?;
                let mut body = String::from("scheduler\n");
                match &self.scheduler {
                    Some(reporter) => {
                        for (label, value) in reporter.rows() {
                            body.push_str(&format!("  {label:<10} {value}\n"));
                        }
                    }
                    // No scheduler injected ⇒ the runtime-free single-threaded default.
                    None => {
                        body.push_str("  backend    single\n");
                        body.push_str("  threads    1\n");
                    }
                }
                Ok(kernel_text(body))
            }
            // The throughput readout: where the constraint is right now — the
            // resources that consumed the most *uncached* compute over the recent
            // window. Cache hits are excluded from the total (they cost nothing); the
            // heaviest target is the bottleneck (Goldratt step 1).
            ("constraint", Verb::Source) => {
                require_cap(capability, "urn:cap:kernel:inspect")?;
                Ok(kernel_text(self.render_constraint()))
            }
            // The kernel's own catalog: every bound endpoint's `describe()` as one RDF
            // (Turtle) graph — so the kernel is queryable *about itself* with SPARQL and
            // renderable to HTML via transreption. Cacheable: the binding set is stable
            // within a session. Each Exact-bound endpoint is resolved and rendered via the
            // meta renderer; template patterns (not concrete IRIs) are skipped.
            ("catalog", Verb::Source) => {
                require_cap(capability, "urn:cap:kernel:inspect")?;
                let renderer = self
                    .meta
                    .as_ref()
                    .ok_or_else(|| Error::Endpoint("no Meta renderer configured".to_string()))?;
                let turtle = ReprType::new("text/turtle");
                let mut body = String::new();
                for entry in self.root.entries().unwrap_or_default() {
                    let Ok(iri) = Iri::parse(&entry.pattern) else {
                        continue;
                    };
                    if let Resolution::Hit(resolved) = self
                        .root
                        .resolve(&Request::new(Verb::Meta, iri), &Scope::empty())
                    {
                        if let Ok(repr) = renderer.render(&resolved.endpoint.describe(), &turtle) {
                            if let Ok(text) = String::from_utf8(repr.bytes) {
                                body.push_str(text.trim_end());
                                body.push('\n');
                            }
                        }
                    }
                }
                Ok(Representation::new(
                    ReprType::new("text/turtle").with_param("charset", "utf-8"),
                    body.into_bytes(),
                )
                .cacheable())
            }
            // Selection as a resource: the endpoints whose required inputs are satisfiable by
            // the RDF classes in `types` (comma/space-separated IRIs) — "given these typed
            // entities, what can I do with them?" (see [`crate::select_action`]). One endpoint
            // IRI per line, so it pipes into a `..` map. Cacheable like the catalog (a pure
            // function of the binding set + `types`).
            // `describe urn:kernel:actions` — the selector's self-description, so the
            // engine routes `types=` (it only names *declared* inputs) and the resource is
            // introspectable like any bound endpoint. Rendered on this sync path in its
            // canonical forms (JSON for the engine, Turtle for `describe`); the async
            // transrept-to-other-types route the normal Meta path uses isn't available here,
            // so any other requested type falls back to canonical Turtle.
            ("actions", Verb::Meta) => {
                let renderer = self
                    .meta
                    .as_ref()
                    .ok_or_else(|| Error::Endpoint("no Meta renderer configured".to_string()))?;
                let description = actions_description();
                let target = meta_target(request);
                let repr = renderer
                    .render(&description, &target)
                    .or_else(|_| renderer.render(&description, &ReprType::new("text/turtle")))?;
                Ok(repr.cacheable())
            }
            ("actions", Verb::Source) => {
                // NO inspect gate, deliberately: unlike the catalog (which discloses the
                // whole kernel), the manifold is SELF-LIMITING — it only ever contains
                // actions the asking capability can invoke. "What can I do?" must be
                // answerable by any capability about itself; denying it just forces an
                // attenuated agent into trial-and-error probing.
                // Every axis optional: bare `urn:kernel:actions` is the caller's whole
                // capability-scoped action manifold — "what can I do at all?".
                let types_arg = kernel_arg(request, "types").unwrap_or_default();
                let types: Vec<&str> = types_arg
                    .split([',', ' ', '\n', '\t'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                let verb = match kernel_arg(request, "verb") {
                    None => None,
                    Some(v) => Some(parse_verb(v)?),
                };
                let want = kernel_arg(request, "want");
                let query = crate::select::ActionQuery {
                    present: &types,
                    verb,
                    want,
                    // The AMBIENT capability: selection is cap-scoped by construction,
                    // with the same `allows` check enforcement runs at invoke time. The
                    // representation cache keys on the capability fingerprint, so a
                    // cached manifold is only ever served back to the authority it was
                    // computed for.
                    capability: Some(capability),
                };
                let matches = self.select_actions(&query);
                let turtle = kernel_arg(request, "as") == Some("text/turtle");
                if turtle {
                    // The auditable face: each match an ik:ActionMatch node joining the
                    // selection to the full contract in the catalog graph.
                    let mut body = String::from("@prefix ik: <https://ikigai-rs.dev/ns#> .\n");
                    for m in &matches {
                        body.push_str(&format!(
                            "\n<{}> a ik:ActionMatch ;\n    ik:endpoint <{}> ;\n    ik:verb \"{:?}\"",
                            m.action, m.endpoint, m.verb
                        ));
                        for scope in &m.requires {
                            let term = if scope.starts_with("urn:")
                                || scope.starts_with("http://")
                                || scope.starts_with("https://")
                            {
                                format!("<{scope}>")
                            } else {
                                format!("\"{}\"", scope.replace('\\', "\\\\").replace('"', "\\\""))
                            };
                            body.push_str(&format!(" ;\n    ik:requires {term}"));
                        }
                        if m.missing_optional > 0 {
                            body.push_str(&format!(
                                " ;\n    ik:missingOptional {}",
                                m.missing_optional
                            ));
                        }
                        body.push_str(" .\n");
                    }
                    return Ok(Representation::new(
                        ReprType::new("text/turtle").with_param("charset", "utf-8"),
                        body.into_bytes(),
                    )
                    .cacheable());
                }
                // The plain face stays one RESOLVABLE endpoint IRI per line (deduped
                // across its matched actions), so it pipes into a `..` map unchanged.
                let mut body = String::new();
                let mut seen = BTreeSet::new();
                for m in &matches {
                    if seen.insert(&m.endpoint) {
                        body.push_str(&m.endpoint);
                        body.push('\n');
                    }
                }
                Ok(Representation::new(
                    ReprType::new("text/plain").with_param("charset", "utf-8"),
                    body.into_bytes(),
                )
                .cacheable())
            }
            ("validate", Verb::Meta) => {
                let renderer = self
                    .meta
                    .as_ref()
                    .ok_or_else(|| Error::Endpoint("no Meta renderer configured".to_string()))?;
                let description = validate_description();
                let target = meta_target(request);
                let repr = renderer
                    .render(&description, &target)
                    .or_else(|_| renderer.render(&description, &ReprType::new("text/turtle")))?;
                Ok(repr.cacheable())
            }
            // Validated invoke, stage four of the selection funnel: check a PROPOSED
            // invocation against the action's declared contract BEFORE firing it —
            // required inputs present, one_of respected, XSD scalars plausible, no
            // unknown argument names, and the ambient capability satisfies the
            // action's requires (the same checks selection and enforcement run).
            // The report is genuine SHACL vocabulary: violations are data, and
            // sh:resultPath joins each one back to the catalog's input node. Pure
            // function of (action, args, capability) — cacheable; the cache key
            // carries the capability fingerprint.
            ("validate", Verb::Source) => {
                let description = match (
                    kernel_arg(request, "action"),
                    kernel_arg(request, "endpoint"),
                ) {
                    (Some(action), _) => {
                        let rest = action.strip_prefix("urn:ikigai:endpoint:").ok_or_else(|| {
                            Error::Endpoint(format!(
                                "validate: action must be a catalog action IRI (urn:ikigai:endpoint:<id>:action:<verb>), not `{action}`"
                            ))
                        })?;
                        let (id, verb_name) = rest.split_once(":action:").ok_or_else(|| {
                            Error::Endpoint(format!(
                                "validate: `{action}` names no :action: segment"
                            ))
                        })?;
                        self.description_for_id(id)?
                            .map(|d| (d, parse_verb(verb_name)))
                            .ok_or_else(|| {
                                Error::Endpoint(format!("validate: no endpoint with id `{id}`"))
                            })?
                    }
                    (None, Some(endpoint)) => {
                        let iri = Iri::parse(endpoint).map_err(|e| {
                            Error::Endpoint(format!("validate: bad endpoint IRI: {e}"))
                        })?;
                        let Resolution::Hit(resolved) = self
                            .root
                            .resolve(&Request::new(Verb::Meta, iri), &Scope::empty())
                        else {
                            return Err(Error::Unresolved(Iri::parse(endpoint).expect("parsed")));
                        };
                        let verb_name = kernel_arg(request, "verb")
                            .ok_or_else(|| Error::MissingArgument("verb".to_string()))?;
                        (resolved.endpoint.describe(), parse_verb(verb_name))
                    }
                    (None, None) => {
                        return Err(Error::MissingArgument(
                            "action (or endpoint + verb)".to_string(),
                        ))
                    }
                };
                let (description, verb) = description;
                let verb = verb?;
                let Some(spec) = description
                    .action_specs()
                    .into_iter()
                    .find(|a| a.verb == verb)
                else {
                    return Err(Error::Endpoint(format!(
                        "validate: `{}` declares no {verb:?} action",
                        description.id
                    )));
                };
                let proposed = kernel_arg(request, "args").unwrap_or_default();
                let verb_lower = format!("{verb:?}").to_lowercase();
                let action_iri =
                    format!("urn:ikigai:endpoint:{}:action:{verb_lower}", description.id);
                // resultPath must join to the catalog: explicitly authored actions own
                // action-scoped input nodes; synthesized ones reference endpoint-level.
                let explicit = description.actions.iter().any(|a| a.verb == verb);
                let input_ns = if explicit {
                    format!("{action_iri}:input:")
                } else {
                    format!("urn:ikigai:endpoint:{}:input:", description.id)
                };
                let report =
                    validate_against_spec(&action_iri, &input_ns, &spec, proposed, capability);
                Ok(Representation::new(
                    ReprType::new("text/turtle").with_param("charset", "utf-8"),
                    report.into_bytes(),
                )
                .cacheable())
            }
            _ => Err(Error::Unresolved(request.target.clone())),
        }
    }

    /// Aggregate the constraint window by target and render the heaviest first.
    /// Ranks by uncached compute time when the kernel has a clock, else by uncached
    /// call count — so the readout is meaningful even on a clockless kernel.
    fn render_constraint(&self) -> String {
        let window = self.constraint.lock().expect("constraint lock");
        let total = window.len();
        // Per target: (uncached_ms, calls, cached, any_timed).
        let mut by_target: HashMap<&str, (u64, usize, usize, bool)> = HashMap::new();
        for sample in window.iter() {
            let row = by_target.entry(&sample.target).or_default();
            row.1 += 1;
            if sample.cache_hit {
                row.2 += 1;
            }
            if let Some(ms) = sample.elapsed_ms {
                row.3 = true;
                if !sample.cache_hit {
                    row.0 += ms;
                }
            }
        }
        let mut rows: Vec<_> = by_target.into_iter().collect();
        // Heaviest uncached time first; break ties by uncached call count.
        rows.sort_by(|a, b| {
            let a_uncached = a.1 .1 - a.1 .2;
            let b_uncached = b.1 .1 - b.1 .2;
            b.1 .0.cmp(&a.1 .0).then(b_uncached.cmp(&a_uncached))
        });

        let mut body = format!("constraint  (last {total} resolutions)\n");
        if rows.is_empty() {
            body.push_str("  (no resolutions yet)\n");
            return body;
        }
        for (rank, (target, (uncached_ms, calls, cached, timed))) in rows.iter().take(5).enumerate()
        {
            let uncached = calls - cached;
            let cached_pct = if *calls > 0 { cached * 100 / calls } else { 0 };
            // The leader is the constraint only if it actually consumed capacity.
            let marker = if rank == 0 && (*uncached_ms > 0 || uncached > 0) {
                "   ← constraint"
            } else {
                ""
            };
            let cost = if *timed {
                format!("{uncached_ms}ms uncached")
            } else {
                format!("{uncached} uncached")
            };
            body.push_str(&format!(
                "  {target}{marker}\n    {cost} · {calls} calls · {cached_pct}% cached\n"
            ));
        }
        body
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
    pub fn is_cached(&self, request: &Request, capability: &Capability) -> bool {
        if !request.verb.is_cacheable() {
            return false;
        }
        // Read-only probe: an entry that is cut (a thread bumped) or expired (its
        // deadline passed) is not "cached", matching what `valid_cached` would
        // serve. Keyed by capability too, so the probe answers "cached *for this
        // authority*" — consistent with what issuing under it would actually serve.
        // Does not evict — eviction happens on the serving path.
        let cache = self.cache.lock().expect("cache lock");
        cache
            .get(&(request.id(), capability_key(capability)))
            .is_some_and(|entry| self.entry_is_valid(entry))
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

/// A stable fingerprint of a capability's authority, used to namespace cache entries.
/// Root (full authority) gets its own namespace; a scoped capability's namespace is
/// derived from its sorted scope set (so two equal capabilities share a namespace, and
/// a narrower one cannot collide with a broader one). Cheap, allocation-free, and
/// recomputed per request — the cache holds the `u64`, never the capability.
fn capability_key(capability: &Capability) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match capability.scopes() {
        // Root authority — a fixed namespace distinct from any scoped set.
        None => 0u8.hash(&mut hasher),
        Some(scopes) => {
            1u8.hash(&mut hasher);
            // `scopes` is a `BTreeSet`, so iteration is sorted ⇒ a stable fingerprint.
            for scope in scopes {
                scope.hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

/// The reserved kernel-behavior namespace prefix.
const KERNEL_NS: &str = "urn:kernel:";

/// Authorize a kernel operation, or report a capability denial.
/// Parse a `verb=` argument (case-insensitive verb name).
fn parse_verb(name: &str) -> Result<Verb> {
    match name.to_ascii_lowercase().as_str() {
        "source" => Ok(Verb::Source),
        "sink" => Ok(Verb::Sink),
        "exists" => Ok(Verb::Exists),
        "delete" => Ok(Verb::Delete),
        "meta" => Ok(Verb::Meta),
        other => Err(Error::Endpoint(format!(
            "unknown verb \"{other}\" (source, sink, exists, delete, meta)"
        ))),
    }
}

fn require_cap(capability: &Capability, scope: &str) -> Result<()> {
    if capability.allows(scope) {
        Ok(())
    } else {
        Err(Error::Endpoint(format!(
            "kernel: capability does not grant `{scope}`"
        )))
    }
}

/// An inline argument of a kernel request, decoded as UTF-8.
/// Self-description of the `urn:kernel:actions` selector. Declares the `types` input so the
/// engine routes `types=` (it only names *declared* inputs) and `describe urn:kernel:actions`
/// works — surfacing typed action-selection like any bound endpoint.
fn actions_description() -> Description {
    use crate::describe::ArgSpec;
    Description::new("kernel-actions")
        .title("Action selection")
        .summary(
            "Given the RDF classes of the entities you have, list the endpoints whose required \
             typed inputs are all satisfied — \"what can I do with these?\". One endpoint IRI \
             per line; pipe into a `..` map to act on each.",
        )
        .verb(Verb::Source)
        .verb(Verb::Meta)
        .input(
            ArgSpec::new("types")
                .summary("present RDF class IRIs, comma- or space-separated")
                .optional(),
        )
        .input(
            ArgSpec::new("verb")
                .summary("only actions answering this verb")
                .one_of(["source", "sink", "exists", "delete"])
                .optional(),
        )
        .input(
            ArgSpec::new("want")
                .summary("only actions that can produce this media type")
                .optional(),
        )
        .input(
            ArgSpec::new("as")
                .summary("response face")
                .one_of(["text/plain", "text/turtle"])
                .default_value("text/plain"),
        )
        .output("text/plain;charset=utf-8")
}

/// The self-description of `urn:kernel:validate`.
fn validate_description() -> Description {
    use crate::describe::ArgSpec;
    Description::new("validate")
        .title("Validate a proposed invocation")
        .summary(
            "Stage four of the selection funnel: check a proposed invocation against the \
             action's declared contract BEFORE firing it — required inputs present, one_of \
             respected, XSD scalars plausible, no unknown arguments, and the ambient \
             capability satisfying the action's requires. The answer is a SHACL validation \
             report: violations are data, and sh:resultPath joins each one back to the \
             catalog's input node.",
        )
        .verb(Verb::Source)
        .verb(Verb::Meta)
        .input(
            ArgSpec::new("action")
                .summary("the catalog action IRI (urn:ikigai:endpoint:<id>:action:<verb>)")
                .optional(),
        )
        .input(
            ArgSpec::new("endpoint")
                .summary("alternative: the bound endpoint IRI, with verb=")
                .optional(),
        )
        .input(
            ArgSpec::new("verb")
                .summary("the verb, when addressing by endpoint=")
                .one_of(["source", "sink", "exists", "delete"])
                .optional(),
        )
        .input(
            ArgSpec::new("args")
                .summary("the proposed arguments: key=value pairs joined with & (or newlines)")
                .optional(),
        )
        .output("text/turtle")
}

/// Check `proposed` (key=value pairs joined by `&`/newlines) against `spec`,
/// including the capability pre-flight, and render the SHACL report.
/// `input_ns` is the IRI prefix of the spec's input nodes in the catalog
/// (action-scoped for explicitly authored actions, endpoint-scoped for
/// synthesized ones), so `sh:resultPath` joins violations to the contract.
fn validate_against_spec(
    action_iri: &str,
    input_ns: &str,
    spec: &crate::describe::ActionSpec,
    proposed: &str,
    capability: &Capability,
) -> String {
    use crate::describe::InputSource;
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut provided: Vec<(&str, &str)> = Vec::new();
    for pair in proposed.split(['&', '\n']) {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        match pair.split_once('=') {
            Some((k, v)) => provided.push((k.trim(), v.trim())),
            None => provided.push((pair, "")),
        }
    }

    // (message, Some(input node IRI)) per violation.
    let mut violations: Vec<(String, Option<String>)> = Vec::new();

    for scope in &spec.requires {
        if !crate::select::cap_satisfies(capability, scope) {
            violations.push((format!("the capability does not grant `{scope}`"), None));
        }
    }
    for input in &spec.inputs {
        let node = format!("{input_ns}{}", input.name);
        let value = provided
            .iter()
            .find(|(k, _)| *k == input.name)
            .map(|(_, v)| *v);
        if input.required && input.source != InputSource::Binding && value.is_none() {
            violations.push((
                format!("required input `{}` is missing", input.name),
                Some(node),
            ));
            continue;
        }
        let Some(value) = value else { continue };
        if !input.one_of.is_empty() && !input.one_of.iter().any(|v| v == value) {
            violations.push((
                format!(
                    "`{value}` is not an accepted value of `{}` (one of: {})",
                    input.name,
                    input.one_of.join(", ")
                ),
                Some(node),
            ));
            continue;
        }
        if let Some(class) = &input.class {
            let plausible = match class.as_str() {
                "http://www.w3.org/2001/XMLSchema#boolean" => value == "true" || value == "false",
                "http://www.w3.org/2001/XMLSchema#integer" => value.parse::<i64>().is_ok(),
                "http://www.w3.org/2001/XMLSchema#dateTime" => {
                    let b = value.as_bytes();
                    b.len() >= 10
                        && b[..4].iter().all(u8::is_ascii_digit)
                        && b[4] == b'-'
                        && b[5..7].iter().all(u8::is_ascii_digit)
                        && b[7] == b'-'
                        && b[8..10].iter().all(u8::is_ascii_digit)
                }
                _ => true, // entity classes are not statically checkable
            };
            if !plausible {
                violations.push((
                    format!(
                        "`{value}` does not look like a {} for `{}`",
                        class, input.name
                    ),
                    Some(node),
                ));
            }
        }
    }
    for (k, _) in &provided {
        if !spec.inputs.iter().any(|i| i.name == *k) {
            violations.push((
                format!("unknown argument `{k}` — not in this action's contract"),
                None,
            ));
        }
    }

    let mut ttl = String::from("@prefix sh: <http://www.w3.org/ns/shacl#> .\n\n");
    if violations.is_empty() {
        ttl.push_str(
            "<urn:ikigai:validation:report> a sh:ValidationReport ;\n    sh:conforms true .\n",
        );
        return ttl;
    }
    ttl.push_str("<urn:ikigai:validation:report> a sh:ValidationReport ;\n    sh:conforms false");
    for (message, path) in &violations {
        ttl.push_str(" ;\n    sh:result [ a sh:ValidationResult ;\n        sh:resultSeverity sh:Violation ;\n        sh:focusNode <");
        ttl.push_str(action_iri);
        ttl.push('>');
        if let Some(path) = path {
            ttl.push_str(&format!(" ;\n        sh:resultPath <{path}>"));
        }
        ttl.push_str(&format!(
            " ;\n        sh:resultMessage \"{}\" ]",
            escape(message)
        ));
    }
    ttl.push_str(" .\n");
    ttl
}

fn kernel_arg<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    match request.args.get(name) {
        Some(ArgRef::Inline(bytes)) => std::str::from_utf8(bytes).ok(),
        _ => None,
    }
}

/// A `text/plain` representation of live kernel state (uncacheable by default).
fn kernel_text(body: String) -> Representation {
    Representation::new(
        ReprType::new("text/plain").with_param("charset", "utf-8"),
        body.into_bytes(),
    )
}

/// A short human-readable byte size (`512 B`, `1.5 KB`, `2.0 MB`) for the cache readout.
fn human_size(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
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

    async fn issue_with_parent(
        &self,
        request: Request,
        capability: &Capability,
        parent: Option<u64>,
    ) -> Result<Representation> {
        // Re-entrant sub-request: thread the issuing node's span through so the
        // recorded events link parent → child (across the fan-out spawn). A
        // sub-resource resolves on its own merits — no pipe upstream here.
        self.issue_inner(request, capability, parent, None).await
    }

    fn now(&self) -> Option<Time> {
        self.clock.as_ref().map(|clock| clock.now())
    }

    fn select_transreptor(&self, from: &str, to: &str) -> Option<Vec<TransreptionStep>> {
        // Delegate to the inherent method (selection over the kernel's root space).
        Kernel::select_transreptor(self, from, to)
    }

    fn select_action(&self, present: &[&str]) -> Vec<ActionMatch> {
        Kernel::select_action(self, present)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arg::ArgRef;
    use crate::builtins;
    use crate::describe::Description;
    use crate::endpoint::{BoxFuture, Endpoint, FnEndpoint, Invocation, Spawner};
    use crate::grammar::Exact;
    use crate::iri::Iri;
    use crate::repr::ReprType;
    use crate::space::EndpointSpace;
    use crate::verb::Verb;
    use futures::executor::block_on;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    /// A [`Clock`] whose "now" the test can set, to drive deadline expiry
    /// deterministically (no real time involved).
    #[derive(Clone)]
    struct TestClock(Arc<AtomicU64>);
    impl TestClock {
        fn at(millis: u64) -> Self {
            TestClock(Arc::new(AtomicU64::new(millis)))
        }
        fn set(&self, millis: u64) {
            self.0.store(millis, Ordering::SeqCst);
        }
    }
    impl Clock for TestClock {
        fn now(&self) -> Time {
            Time::from_millis(self.0.load(Ordering::SeqCst))
        }
    }

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

    /// A Turtle-only meta renderer (like `ikigai-vocab`'s): emits `text/turtle` for the
    /// canonical request, errors for anything else — which is what makes the kernel fall
    /// through to selection-driven transreption.
    struct TurtleishRenderer;
    impl MetaRenderer for TurtleishRenderer {
        fn render(&self, description: &Description, target: &ReprType) -> Result<Representation> {
            match target.media_type.as_str() {
                "text/turtle" | "*/*" | "" => Ok(Representation::new(
                    ReprType::new("text/turtle"),
                    format!("<urn:x> a ik:Endpoint ; ik:id \"{}\" .", description.id).into_bytes(),
                )
                .cacheable()),
                other => Err(Error::Endpoint(format!(
                    "unsupported meta target `{other}`"
                ))),
            }
        }
    }

    /// A stub auto-invocable transreptor (`text/turtle → application/rdf+xml`): wraps the
    /// piped `content` so a test can see it ran.
    fn stub_rdf_transrept() -> FnEndpoint {
        FnEndpoint::new("rdf-transrept", |inv: &Invocation<'_>| {
            let content = inv.inline_str("content").unwrap_or("");
            Ok(Representation::new(
                ReprType::new("application/rdf+xml"),
                format!("RDFXML({content})").into_bytes(),
            )
            .cacheable())
        })
        .with_description(
            Description::new("rdf-transrept")
                .verb(Verb::Source)
                .input(crate::describe::ArgSpec::new("content"))
                .input(crate::describe::ArgSpec::new("as"))
                .transreptor(["text/turtle"], ["application/rdf+xml"]),
        )
    }

    fn meta_kernel() -> Kernel {
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:fn:toUpper"), builtins::to_upper())
            .bind(Exact::new("urn:rdf:transrept"), stub_rdf_transrept());
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(TurtleishRenderer))
    }

    fn meta_as(kernel: &Kernel, target: &str, as_type: &str) -> Representation {
        let request = Request::new(Verb::Meta, iri(target))
            .with_arg("as", ArgRef::Inline(as_type.as_bytes().to_vec()));
        block_on(kernel.issue(request, &Capability::root())).unwrap()
    }

    #[test]
    fn an_endpoint_can_select_a_transreptor_through_its_invocation() {
        // An endpoint reaches the kernel's transreptor selection via inv.select_transreptor
        // (delegated through the Issuer) — the seam urn:transrept:auto / content-negotiation
        // build on.
        let probe = FnEndpoint::new("probe", |inv: &Invocation<'_>| {
            let body = match inv.select_transreptor("text/turtle", "application/rdf+xml") {
                Some(steps) => steps
                    .iter()
                    .map(|s| s.endpoint.clone())
                    .collect::<Vec<_>>()
                    .join(","),
                None => "none".to_string(),
            };
            Ok(Representation::new(
                ReprType::new("text/plain"),
                body.into_bytes(),
            ))
        });
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:probe"), probe)
            .bind(Exact::new("urn:rdf:transrept"), stub_rdf_transrept());
        let kernel = Kernel::new(Arc::new(space));
        let rep = block_on(kernel.issue(
            Request::new(Verb::Source, iri("urn:probe")),
            &Capability::root(),
        ))
        .unwrap();
        assert_eq!(String::from_utf8(rep.bytes).unwrap(), "urn:rdf:transrept");
    }

    #[test]
    fn a_detached_invocation_selects_nothing() {
        let request = Request::new(Verb::Source, iri("urn:x"));
        let bindings = crate::grammar::Bindings::default();
        let cap = Capability::root();
        let inv = Invocation::detached(&request, &bindings, &cap);
        assert!(inv
            .select_transreptor("text/turtle", "application/rdf+xml")
            .is_none());
        assert!(inv.select_action(&["https://schema.org/Person"]).is_empty());
    }

    #[test]
    fn an_endpoint_can_select_actions_through_its_invocation() {
        // An endpoint surfaces the actions available for a set of present types via
        // inv.select_action (delegated through the Issuer) — the seed of layer action-inference.
        let probe = FnEndpoint::new("probe", |inv: &Invocation<'_>| {
            let body = inv
                .select_action(&["https://schema.org/Person"])
                .iter()
                .map(|m| m.endpoint.clone())
                .collect::<Vec<_>>()
                .join(",");
            Ok(Representation::new(
                ReprType::new("text/plain"),
                body.into_bytes(),
            ))
        });
        let greet = FnEndpoint::new("greet", |_inv| {
            Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
        })
        .with_description(
            Description::new("greet")
                .verb(Verb::Source)
                .input(crate::describe::ArgSpec::new("who").class("https://schema.org/Person")),
        );
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:probe"), probe)
            .bind(Exact::new("urn:demo:greet"), greet);
        let kernel = Kernel::new(Arc::new(space));
        let rep = block_on(kernel.issue(
            Request::new(Verb::Source, iri("urn:probe")),
            &Capability::root(),
        ))
        .unwrap();
        assert_eq!(String::from_utf8(rep.bytes).unwrap(), "urn:demo:greet");
    }

    #[test]
    fn meta_serves_canonical_turtle_directly() {
        let rep = meta_as(&meta_kernel(), "urn:fn:toUpper", "text/turtle");
        assert_eq!(rep.repr_type.media_type, "text/turtle");
        assert!(String::from_utf8(rep.bytes)
            .unwrap()
            .contains("ik:id \"toUpper\""));
    }

    #[test]
    fn meta_transrepts_to_a_non_canonical_type_via_selection() {
        // as=application/rdf+xml: the renderer can't emit it, so the kernel renders canonical
        // Turtle and runs the selected turtle→rdf+xml transreptor over it.
        let rep = meta_as(&meta_kernel(), "urn:fn:toUpper", "application/rdf+xml");
        assert_eq!(rep.repr_type.media_type, "application/rdf+xml");
        let body = String::from_utf8(rep.bytes).unwrap();
        assert!(body.starts_with("RDFXML("), "transreptor ran: {body}");
        assert!(
            body.contains("ik:id \"toUpper\""),
            "over the canonical turtle: {body}"
        );
    }

    #[test]
    fn meta_falls_back_to_turtle_when_no_transreptor_reaches_the_type() {
        // as=application/pdf: nothing converts turtle→pdf, so fall back to canonical Turtle.
        let rep = meta_as(&meta_kernel(), "urn:fn:toUpper", "application/pdf");
        assert_eq!(rep.repr_type.media_type, "text/turtle");
        assert!(String::from_utf8(rep.bytes)
            .unwrap()
            .contains("ik:id \"toUpper\""));
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
    fn catalog_enumerates_every_bound_endpoint_through_the_renderer() {
        // The catalog is the kernel's self-describing graph: it walks the root
        // space and renders each entry's `describe()` via the Meta renderer, so a
        // SPARQL query / transreption can run over "the endpoints" as a resource.
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:fn:toUpper"), builtins::to_upper())
            .bind(Exact::new("urn:fn:echo"), builtins::echo());
        let kernel = Kernel::with_meta_renderer(Arc::new(space), Arc::new(EchoIdRenderer));
        let cap = Capability::root();
        let rep =
            block_on(kernel.issue(Request::new(Verb::Source, iri("urn:kernel:catalog")), &cap))
                .unwrap();
        let body = String::from_utf8(rep.bytes).unwrap();
        assert!(
            body.contains("toUpper"),
            "catalog should describe toUpper: {body}"
        );
        assert!(
            body.contains("echo"),
            "catalog should describe echo: {body}"
        );
        assert_eq!(rep.repr_type.media_type, "text/turtle");
        // Cacheable: a downstream SPARQL query inherits this so it can hit cache.
        assert_eq!(rep.expiry, Expiry::Never);
        // And it genuinely participates in the cache despite living in the
        // `urn:kernel:*` namespace — re-resolution is served from cache.
        let catalog = || Request::new(Verb::Source, iri("urn:kernel:catalog"));
        assert!(
            kernel.is_cached(&catalog(), &cap),
            "catalog should be cached after first issue"
        );
        assert_eq!(kernel.cache_len(), 1, "exactly the catalog is cached");
        block_on(kernel.issue(catalog(), &cap)).unwrap();
        assert_eq!(
            kernel.cache_len(),
            1,
            "re-issue is a cache hit, not a second entry"
        );
    }

    #[test]
    fn actions_resource_lists_type_satisfiable_endpoints() {
        // urn:kernel:actions exposes select_action as a resource: given the RDF classes in
        // `types`, list the endpoints those entities could drive — one IRI per line.
        let greet = FnEndpoint::new("greet", |_inv| {
            Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
        })
        .with_description(
            Description::new("greet")
                .verb(Verb::Source)
                .input(crate::describe::ArgSpec::new("who").class("https://schema.org/Person")),
        );
        let space = EndpointSpace::new()
            // untyped required inputs → never an inferred action
            .bind(Exact::new("urn:fn:toUpper"), builtins::to_upper())
            .bind(Exact::new("urn:demo:greet"), greet);
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();
        let req = Request::new(Verb::Source, iri("urn:kernel:actions")).with_arg(
            "types",
            ArgRef::Inline(b"https://schema.org/Person".to_vec()),
        );
        let rep = block_on(kernel.issue(req, &cap)).unwrap();
        let body = String::from_utf8(rep.bytes).unwrap();
        assert!(body.contains("urn:demo:greet"), "{body}");
        assert!(
            !body.contains("toUpper"),
            "untyped endpoint isn't an action: {body}"
        );
        assert_eq!(rep.repr_type.media_type, "text/plain");

        // Bare `urn:kernel:actions` is the whole (capability-scoped) action manifold —
        // every axis of the query is optional.
        let rep =
            block_on(kernel.issue(Request::new(Verb::Source, iri("urn:kernel:actions")), &cap))
                .unwrap();
        let body = String::from_utf8(rep.bytes).unwrap();
        assert!(body.contains("urn:demo:greet"), "{body}");
        assert!(
            body.contains("urn:fn:toUpper"),
            "no type filter applied: {body}"
        );
    }

    #[test]
    fn the_action_manifold_is_capability_scoped() {
        // An endpoint whose Source and Sink are different actions under different
        // capabilities — the calendar shape. An attenuated agent's manifold simply
        // lacks the write action; root sees both. Selection uses the same `allows`
        // check enforcement runs, so the two can never drift.
        use crate::describe::ActionSpec;
        let cal = FnEndpoint::new("cal", |_inv| {
            Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
        })
        .with_description(
            Description::new("cal")
                .action(
                    ActionSpec::new(Verb::Source)
                        .requires("urn:cap:cal:read")
                        .output("text/turtle"),
                )
                .action(ActionSpec::new(Verb::Sink).requires("urn:cap:cal:write")),
        );
        let space = EndpointSpace::new().bind(Exact::new("urn:demo:cal"), cal);
        let kernel = Kernel::new(Arc::new(space));

        let turtle = |cap: &Capability| {
            let req = Request::new(Verb::Source, iri("urn:kernel:actions"))
                .with_arg("as", ArgRef::Inline(b"text/turtle".to_vec()));
            let rep = block_on(kernel.issue(req, cap)).unwrap();
            String::from_utf8(rep.bytes).unwrap()
        };

        let all = turtle(&Capability::root());
        assert!(all.contains("cal:action:source> a ik:ActionMatch"), "{all}");
        assert!(all.contains("cal:action:sink> a ik:ActionMatch"), "{all}");
        assert!(all.contains("ik:requires <urn:cap:cal:write>"), "{all}");

        // No kernel:inspect in the scope — an agent may always read its OWN manifold.
        let reader = Capability::scoped(["urn:cap:cal:read"]);
        let scoped = turtle(&reader);
        assert!(scoped.contains("cal:action:source>"), "{scoped}");
        assert!(
            !scoped.contains("action:sink"),
            "the write action must not be OFFERED to a read capability: {scoped}"
        );

        // A wildcard requires ("urn:cap:net:*") offers to any capability holding
        // SOME grant under the prefix — the parameterized-ACL annotation form.
        let net = FnEndpoint::new("net", |_inv| {
            Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
        })
        .with_description(
            Description::new("net")
                .verb(Verb::Source)
                .requires("urn:cap:net:*"),
        );
        let space2 = EndpointSpace::new().bind(Exact::new("urn:demo:net"), net);
        let kernel2 = Kernel::new(Arc::new(space2));
        let query = |cap: &Capability| {
            let req = Request::new(Verb::Source, iri("urn:kernel:actions"));
            let rep = block_on(kernel2.issue(req, cap)).unwrap();
            String::from_utf8(rep.bytes).unwrap()
        };
        let host_scoped = Capability::scoped(["urn:cap:net:api.example.com"]);
        assert!(query(&host_scoped).contains("urn:demo:net"));
        let no_net = Capability::scoped(["urn:cap:cal:read"]);
        assert!(!query(&no_net).contains("urn:demo:net"));

        // verb= and want= narrow the funnel deterministically.
        let req = Request::new(Verb::Source, iri("urn:kernel:actions"))
            .with_arg("verb", ArgRef::Inline(b"sink".to_vec()));
        let rep = block_on(kernel.issue(req, &Capability::root())).unwrap();
        let body = String::from_utf8(rep.bytes).unwrap();
        assert!(body.contains("urn:demo:cal"), "{body}");
        let req = Request::new(Verb::Source, iri("urn:kernel:actions"))
            .with_arg("want", ArgRef::Inline(b"text/html".to_vec()));
        let rep = block_on(kernel.issue(req, &Capability::root())).unwrap();
        assert!(rep.bytes.is_empty(), "nothing produces text/html");
    }

    #[test]
    fn describe_returns_the_typed_struct() {
        let greet = FnEndpoint::new("greet", |_inv| {
            Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
        })
        .with_description(
            Description::new("greet")
                .verb(Verb::Source)
                .input(crate::describe::ArgSpec::new("who").class("https://schema.org/Person")),
        );
        let space = EndpointSpace::new().bind(Exact::new("urn:demo:greet"), greet);
        let kernel = Kernel::new(Arc::new(space));

        let d = kernel.describe(&iri("urn:demo:greet")).expect("resolves");
        assert_eq!(d.id, "greet");
        assert_eq!(d.inputs[0].name, "who");
        assert_eq!(
            d.inputs[0].class.as_deref(),
            Some("https://schema.org/Person")
        );
        assert!(kernel.describe(&iri("urn:demo:absent")).is_none());
    }

    #[test]
    fn validate_reports_violations_as_shacl() {
        use crate::describe::ActionSpec;
        let cal = FnEndpoint::new("cal", |_inv| {
            Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
        })
        .with_description(
            Description::new("cal").action(
                ActionSpec::new(Verb::Sink)
                    .requires("urn:cap:cal:write")
                    .input(
                        crate::describe::ArgSpec::new("start")
                            .class("http://www.w3.org/2001/XMLSchema#dateTime"),
                    )
                    .input(
                        crate::describe::ArgSpec::new("mode")
                            .one_of(["add", "move"])
                            .optional(),
                    ),
            ),
        );
        let space = EndpointSpace::new().bind(Exact::new("urn:demo:cal"), cal);
        let kernel = Kernel::new(Arc::new(space));
        let validate = |args: &str, cap: &Capability| {
            let req = Request::new(Verb::Source, iri("urn:kernel:validate"))
                .with_arg(
                    "action",
                    ArgRef::Inline(b"urn:ikigai:endpoint:cal:action:sink".to_vec()),
                )
                .with_arg("args", ArgRef::Inline(args.as_bytes().to_vec()));
            let rep = block_on(kernel.issue(req, cap)).unwrap();
            String::from_utf8(rep.bytes).unwrap()
        };

        // Clean: everything declared, capability satisfied.
        let ok = validate("start=2026-07-10T10:00:00&mode=add", &Capability::root());
        assert!(ok.contains("sh:conforms true"), "{ok}");

        // The violation matrix: missing required, bad enum value, implausible
        // datetime, unknown argument — each a sh:ValidationResult with a message,
        // and the input-node paths join back to the catalog contract.
        let bad = validate("mode=destroy&when=now", &Capability::root());
        assert!(bad.contains("sh:conforms false"), "{bad}");
        assert!(bad.contains("required input `start` is missing"), "{bad}");
        assert!(bad.contains("not an accepted value of `mode`"), "{bad}");
        assert!(bad.contains("unknown argument `when`"), "{bad}");
        assert!(
            bad.contains("sh:resultPath <urn:ikigai:endpoint:cal:action:sink:input:start>"),
            "explicit action inputs are action-scoped: {bad}"
        );
        let odd = validate("start=whenever", &Capability::root());
        assert!(odd.contains("does not look like a"), "{odd}");

        // Capability pre-flight: a read-only capability fails validation BEFORE
        // any invocation is attempted — the same allows check enforcement runs.
        let scoped = Capability::scoped(["urn:cap:cal:read"]);
        let denied = validate("start=2026-07-10T10:00:00", &scoped);
        assert!(denied.contains("sh:conforms false"), "{denied}");
        assert!(
            denied.contains("does not grant `urn:cap:cal:write`"),
            "{denied}"
        );
    }

    #[test]
    fn select_action_reasons_over_subclass_axioms() {
        // greet requires a schema:Person. With foaf:Person rdfs:subClassOf schema:Person
        // installed, an entity typed foaf:Person satisfies it — the RDFS inference bridge.
        const FOAF_PERSON: &str = "http://xmlns.com/foaf/0.1/Person";
        const SCHEMA_PERSON: &str = "https://schema.org/Person";
        // EndpointSpace isn't Clone, so build a fresh one per kernel.
        let space = || {
            let greet = FnEndpoint::new("greet", |_inv| {
                Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
            })
            .with_description(
                Description::new("greet")
                    .verb(Verb::Source)
                    .input(crate::describe::ArgSpec::new("who").class(SCHEMA_PERSON)),
            );
            Arc::new(EndpointSpace::new().bind(Exact::new("urn:demo:greet"), greet))
                as Arc<dyn Space>
        };

        // Without the axiom: foaf:Person matches nothing (exact-class only).
        assert!(
            Kernel::new(space())
                .select_action(&[FOAF_PERSON])
                .is_empty(),
            "no axioms ⇒ exact-class only"
        );

        // With the axiom: foaf:Person ⊑ schema:Person ⇒ greet is satisfiable.
        let reasoning = Kernel::new(space()).with_subclass_axioms([(FOAF_PERSON, SCHEMA_PERSON)]);
        assert!(
            reasoning
                .select_action(&[FOAF_PERSON])
                .iter()
                .any(|m| m.endpoint == "urn:demo:greet"),
            "foaf:Person should satisfy a schema:Person action via subClassOf"
        );
        // The direct type still matches too.
        assert!(reasoning
            .select_action(&[SCHEMA_PERSON])
            .iter()
            .any(|m| m.endpoint == "urn:demo:greet"));
    }

    #[test]
    fn subclass_closure_is_transitive_over_many_axioms() {
        // Multiple axioms incl. a chain foaf:Person ⊑ schema:Person ⊑ schema:Thing: an
        // action requiring schema:Thing is reachable from a foaf:Person entity two hops up.
        const FOAF_PERSON: &str = "http://xmlns.com/foaf/0.1/Person";
        const SCHEMA_PERSON: &str = "https://schema.org/Person";
        const SCHEMA_THING: &str = "https://schema.org/Thing";
        let space = || {
            let anything = FnEndpoint::new("anything", |_inv| {
                Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
            })
            .with_description(
                Description::new("anything")
                    .verb(Verb::Source)
                    .input(crate::describe::ArgSpec::new("it").class(SCHEMA_THING)),
            );
            Arc::new(EndpointSpace::new().bind(Exact::new("urn:demo:anything"), anything))
                as Arc<dyn Space>
        };
        let kernel = Kernel::new(space())
            .with_subclass_axioms([(FOAF_PERSON, SCHEMA_PERSON), (SCHEMA_PERSON, SCHEMA_THING)]);
        // Two hops: foaf:Person ⊑ schema:Person ⊑ schema:Thing.
        assert!(
            kernel
                .select_action(&[FOAF_PERSON])
                .iter()
                .any(|m| m.endpoint == "urn:demo:anything"),
            "transitive closure should reach schema:Thing from foaf:Person"
        );
    }

    #[test]
    fn actions_resource_describes_itself() {
        // `describe urn:kernel:actions` resolves to the selector's self-description (id
        // "kernel-actions") instead of erroring unresolved — so it's introspectable and the
        // engine can route `types=` (it only names declared inputs).
        let kernel = meta_kernel();
        let turtle =
            String::from_utf8(meta_as(&kernel, "urn:kernel:actions", "text/turtle").bytes).unwrap();
        assert!(turtle.contains("kernel-actions"), "described: {turtle}");
        // A type the renderer can't emit directly falls back to canonical Turtle — the async
        // transrept-to-other-types route isn't available on the sync intrinsic path.
        let fallback =
            String::from_utf8(meta_as(&kernel, "urn:kernel:actions", "application/json").bytes)
                .unwrap();
        assert!(fallback.contains("kernel-actions"), "fell back: {fallback}");
    }

    #[test]
    fn incoming_provenance_flows_cacheability_down_the_pipe() {
        // A cacheable endpoint (toUpper opts into caching via builtins) issued with
        // an *uncacheable* upstream provenance must itself become uncacheable —
        // cacheability is no greater than the piped input's.
        let space = EndpointSpace::new().bind(Exact::new("urn:fn:toUpper"), builtins::to_upper());
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();
        let req = || {
            Request::new(Verb::Source, iri("urn:fn:toUpper"))
                .with_arg("in", ArgRef::Inline(b"hi".to_vec()))
        };

        // Cacheable upstream (Never) → result stays cacheable → it caches.
        let cacheable_up = Provenance::new(Expiry::Never, BTreeSet::new());
        block_on(kernel.issue_with_incoming(req(), &cap, cacheable_up)).unwrap();
        assert_eq!(
            kernel.cache_len(),
            1,
            "cacheable upstream keeps the result cacheable"
        );

        // Uncacheable upstream (Always) → result becomes uncacheable → not cached.
        let other = Request::new(Verb::Source, iri("urn:fn:toUpper"))
            .with_arg("in", ArgRef::Inline(b"yo".to_vec()));
        let volatile_up = Provenance::new(Expiry::Always, BTreeSet::new());
        let rep = block_on(kernel.issue_with_incoming(other, &cap, volatile_up)).unwrap();
        assert_eq!(
            rep.expiry,
            Expiry::Always,
            "volatile upstream makes the result volatile"
        );
        assert_eq!(
            kernel.cache_len(),
            1,
            "the volatile-upstream result was not cached"
        );
    }

    #[test]
    fn incoming_threads_are_inherited_so_cutting_the_source_invalidates() {
        // The upstream's golden threads propagate, so cutting one invalidates the
        // transformed result — the pipe is a dependency edge.
        let space = EndpointSpace::new().bind(Exact::new("urn:fn:toUpper"), builtins::to_upper());
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();
        let req = || {
            Request::new(Verb::Source, iri("urn:fn:toUpper"))
                .with_arg("in", ArgRef::Inline(b"hi".to_vec()))
        };
        let mut threads = BTreeSet::new();
        threads.insert(Thread::new("urn:data:source"));
        let up = Provenance::new(Expiry::Never, threads);
        block_on(kernel.issue_with_incoming(req(), &cap, up)).unwrap();
        assert!(kernel.is_cached(&req(), &cap), "cached after first issue");
        kernel.cut("urn:data:source");
        assert!(
            !kernel.is_cached(&req(), &cap),
            "cutting the inherited thread invalidates it"
        );
    }

    #[test]
    fn live_kernel_introspection_is_never_cached() {
        // The introspection builtins return `Always` (live) — they must not be
        // cached, even though the catalog (also `urn:kernel:*`) is.
        let space = EndpointSpace::new().bind(Exact::new("urn:fn:toUpper"), builtins::to_upper());
        let kernel = Kernel::with_meta_renderer(Arc::new(space), Arc::new(EchoIdRenderer));
        let cap = Capability::root();
        for op in [
            "urn:kernel:cache",
            "urn:kernel:threads",
            "urn:kernel:constraint",
        ] {
            block_on(kernel.issue(Request::new(Verb::Source, iri(op)), &cap)).unwrap();
        }
        assert_eq!(
            kernel.cache_len(),
            0,
            "live introspection must never be cached"
        );
    }

    #[test]
    fn catalog_requires_the_inspect_capability() {
        let space = EndpointSpace::new().bind(Exact::new("urn:fn:toUpper"), builtins::to_upper());
        let kernel = Kernel::with_meta_renderer(Arc::new(space), Arc::new(EchoIdRenderer));
        let unprivileged = Capability::scoped(["urn:cap:nothing".to_string()]);
        assert!(block_on(kernel.issue(
            Request::new(Verb::Source, iri("urn:kernel:catalog")),
            &unprivileged
        ))
        .is_err());
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
        assert!(!kernel.is_cached(&req(), &Capability::root()));
        assert!(!kernel.is_cached(&req(), &Capability::root()));
        assert_eq!(CALLS.load(Ordering::SeqCst), 0, "is_cached must not invoke");
        assert_eq!(kernel.cache_len(), 0, "is_cached must not cache");

        // Cached after issuing once.
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert!(kernel.is_cached(&req(), &Capability::root()));

        // A different request (different argument identity) is still a miss.
        let other = Request::new(Verb::Source, iri("urn:fn:count"))
            .with_arg("in", ArgRef::Inline(b"z".to_vec()));
        assert!(!kernel.is_cached(&other, &Capability::root()));
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
        assert!(kernel.is_cached(&req(), &Capability::root()));

        // An external change (or a Sink) cuts the thread.
        kernel.cut("urn:file:notes.txt");
        assert!(
            !kernel.is_cached(&req(), &Capability::root()),
            "cut entry is no longer cached"
        );
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
        assert!(
            kernel.is_cached(&source(), &Capability::root()),
            "source is cached"
        );

        // Write v2 through the kernel: the mutating verb auto-cuts `urn:data:cell`.
        let sink = Request::new(Verb::Sink, iri("urn:data:cell"))
            .with_arg("in", ArgRef::Inline(b"v2".to_vec()));
        block_on(kernel.issue(sink, &cap)).unwrap();
        assert!(
            !kernel.is_cached(&source(), &Capability::root()),
            "the write invalidated the cached read"
        );

        // Read again: the cache recomputes and sees v2.
        assert_eq!(block_on(kernel.issue(source(), &cap)).unwrap().bytes, b"v2");
    }

    #[test]
    fn cache_is_keyed_by_capability() {
        // A cacheable endpoint whose output depends on the capability. If the cache
        // ignored the capability, the first (privileged) read would be served back to a
        // restricted capability — a capability bypass, since the hit skips the endpoint's
        // own check. The cache must instead be namespaced by authority.
        let who = FnEndpoint::new("who", |inv: &Invocation<'_>| {
            let body = if inv.capability.allows("urn:cap:secret") {
                "SECRET"
            } else {
                "public"
            };
            Ok(
                Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec())
                    .cacheable(),
            )
        });
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:demo:who"), who),
        ));
        let req = || Request::new(Verb::Source, iri("urn:demo:who"));
        let privileged = Capability::root(); // allows everything, including urn:cap:secret
        let restricted = Capability::root().attenuate(["urn:cap:other".to_string()]); // no secret

        // The privileged read caches SECRET under the root namespace.
        assert_eq!(
            block_on(kernel.issue(req(), &privileged)).unwrap().bytes,
            b"SECRET"
        );
        assert!(kernel.is_cached(&req(), &privileged));
        // The restricted capability is a different namespace — it must NOT probe as
        // cached, and issuing under it recomputes rather than serving the SECRET entry.
        assert!(
            !kernel.is_cached(&req(), &restricted),
            "the cache is namespaced by capability"
        );
        assert_eq!(
            block_on(kernel.issue(req(), &restricted)).unwrap().bytes,
            b"public",
            "the restricted capability recomputes, not served the cached SECRET"
        );
    }

    // --- the kernel-behavior namespace (urn:kernel:*) --------------------------

    fn cut_request(thread: &str) -> Request {
        Request::new(Verb::Sink, iri("urn:kernel:cut"))
            .with_arg("content", ArgRef::Inline(thread.as_bytes().to_vec()))
    }

    #[test]
    fn cut_as_a_resource_invalidates_a_cached_read() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let ep = FnEndpoint::new("x", |_cx: &Invocation<'_>| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(
                Representation::new(ReprType::new("text/plain"), b"v".to_vec())
                    .cacheable()
                    .depends_on("urn:data:x"),
            )
        });
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:data:x"), ep),
        ));
        let cap = Capability::root();
        let read = || Request::new(Verb::Source, iri("urn:data:x"));

        block_on(kernel.issue(read(), &cap)).unwrap();
        block_on(kernel.issue(read(), &cap)).unwrap();
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "cached");

        // Cut the thread by RESOLVING urn:kernel:cut — not calling Kernel::cut.
        let ack = block_on(kernel.issue(cut_request("urn:data:x"), &cap)).unwrap();
        assert!(String::from_utf8_lossy(&ack.bytes).contains("cut urn:data:x"));
        assert!(
            !kernel.is_cached(&read(), &Capability::root()),
            "the resource cut invalidated it"
        );

        block_on(kernel.issue(read(), &cap)).unwrap();
        assert_eq!(CALLS.load(Ordering::SeqCst), 2, "recomputed after the cut");
    }

    #[test]
    fn cache_readout_lists_each_entry_with_iri_type_and_threads() {
        let ep = FnEndpoint::new("x", |_cx: &Invocation<'_>| {
            Ok(
                Representation::new(ReprType::new("text/plain"), b"hello".to_vec())
                    .cacheable()
                    .depends_on("urn:data:x"),
            )
        });
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:data:x"), ep),
        ));
        let cap = Capability::root();
        block_on(kernel.issue(Request::new(Verb::Source, iri("urn:data:x")), &cap)).unwrap();
        let out = block_on(kernel.issue(Request::new(Verb::Source, iri("urn:kernel:cache")), &cap))
            .unwrap();
        let text = String::from_utf8_lossy(&out.bytes);
        assert!(text.contains("entries  1"), "count: {text}");
        assert!(text.contains("urn:data:x"), "names the IRI: {text}");
        assert!(text.contains("text/plain"), "names the type: {text}");
        assert!(text.contains("5 B"), "names the size: {text}");
        assert!(
            text.contains("1 thread"),
            "names the dependency count: {text}"
        );
    }

    #[test]
    fn the_kernel_namespace_is_capability_gated() {
        let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
        // A scoped capability lacking `urn:cap:kernel:cut` is refused.
        let scoped = Capability::scoped(["urn:cap:something:else"]);
        assert!(block_on(kernel.issue(cut_request("urn:data:x"), &scoped)).is_err());
        // Root holds it.
        assert!(block_on(kernel.issue(cut_request("urn:data:x"), &Capability::root())).is_ok());
    }

    #[test]
    fn the_kernel_namespace_reports_cut_threads_and_cache() {
        let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
        let cap = Capability::root();
        kernel.cut("urn:data:a");
        kernel.cut("urn:data:a");
        kernel.cut("urn:data:b");

        let threads =
            block_on(kernel.issue(Request::new(Verb::Source, iri("urn:kernel:threads")), &cap))
                .unwrap();
        let text = String::from_utf8_lossy(&threads.bytes);
        assert!(text.contains("urn:data:a  gen 2"), "{text}");
        assert!(text.contains("urn:data:b  gen 1"), "{text}");

        let cache =
            block_on(kernel.issue(Request::new(Verb::Source, iri("urn:kernel:cache")), &cap))
                .unwrap();
        assert!(String::from_utf8_lossy(&cache.bytes).contains("entries"));
    }

    #[test]
    fn kernel_scheduler_resource_reports_the_injected_reporter_or_the_single_default() {
        struct FakeReporter;
        impl SchedulerReporter for FakeReporter {
            fn rows(&self) -> Vec<(String, String)> {
                vec![
                    ("backend".to_string(), "pool:4".to_string()),
                    ("threads".to_string(), "4".to_string()),
                ]
            }
        }
        let scheduler_request = || Request::new(Verb::Source, iri("urn:kernel:scheduler"));

        // With a reporter injected, the resource renders its rows.
        let kernel = Kernel::new(Arc::new(EndpointSpace::new()))
            .with_scheduler_reporter(Arc::new(FakeReporter));
        let out = block_on(kernel.issue(scheduler_request(), &Capability::root())).unwrap();
        let text = String::from_utf8_lossy(&out.bytes);
        assert!(
            text.contains("backend") && text.contains("pool:4") && text.contains("threads  "),
            "{text}"
        );

        // Without one, it reports the runtime-free single-threaded default.
        let plain = Kernel::new(Arc::new(EndpointSpace::new()));
        let out = block_on(plain.issue(scheduler_request(), &Capability::root())).unwrap();
        assert!(String::from_utf8_lossy(&out.bytes).contains("backend    single"));

        // Capability-gated like the other kernel resources: a scope lacking
        // `urn:cap:kernel:inspect` is refused.
        let scoped = Capability::scoped(["urn:cap:something:else"]);
        assert!(block_on(plain.issue(scheduler_request(), &scoped)).is_err());
    }

    #[test]
    fn kernel_constraint_resource_ranks_the_heaviest_uncached_target_first() {
        // Two uncacheable endpoints (every resolution recomputes — pure uncached work).
        let heavy = FnEndpoint::new("heavy", |_inv: &Invocation<'_>| {
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"H".to_vec(),
            ))
        });
        let light = FnEndpoint::new("light", |_inv: &Invocation<'_>| {
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"L".to_vec(),
            ))
        });
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:test:heavy"), heavy)
            .bind(Exact::new("urn:test:light"), light);
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();

        for _ in 0..3 {
            block_on(kernel.issue(Request::new(Verb::Source, iri("urn:test:heavy")), &cap))
                .unwrap();
        }
        block_on(kernel.issue(Request::new(Verb::Source, iri("urn:test:light")), &cap)).unwrap();

        // The introspection request itself short-circuits before recording, so the
        // window holds exactly the four resolutions above.
        let out = block_on(kernel.issue(
            Request::new(Verb::Source, iri("urn:kernel:constraint")),
            &cap,
        ))
        .unwrap();
        let text = String::from_utf8_lossy(&out.bytes);
        assert!(text.contains("last 4 resolutions"), "{text}");

        // No clock ⇒ ranked by uncached call count: heavy (3) before light (1).
        let heavy_pos = text.find("urn:test:heavy").expect("heavy listed");
        let light_pos = text.find("urn:test:light").expect("light listed");
        assert!(
            heavy_pos < light_pos,
            "heaviest target ranks first:\n{text}"
        );

        // The leader is flagged as the constraint.
        let heavy_line_end = text[heavy_pos..]
            .find('\n')
            .map_or(text.len(), |i| heavy_pos + i);
        assert!(
            text[heavy_pos..heavy_line_end].contains("← constraint"),
            "leader flagged:\n{text}"
        );
    }

    // --- Time-based expiry (Expiry::At + an injected Clock) ------------------

    #[test]
    fn most_restrictive_is_the_expiry_meet() {
        use Expiry::*;
        let t1 = Time::from_millis(100);
        let t2 = Time::from_millis(200);
        // Always dominates everything.
        assert_eq!(Always.most_restrictive(Never), Always);
        assert_eq!(At(t1).most_restrictive(Always), Always);
        // An At deadline beats Never (the composite inherits the deadline).
        assert_eq!(Never.most_restrictive(At(t1)), At(t1));
        // Two deadlines take the earlier; Never is the identity.
        assert_eq!(At(t1).most_restrictive(At(t2)), At(t1));
        assert_eq!(Never.most_restrictive(Never), Never);
    }

    /// An endpoint that stamps a deadline `window` ms ahead of the kernel's clock.
    fn timed_endpoint(name: &'static str, window: u64, calls: &'static AtomicU32) -> FnEndpoint {
        FnEndpoint::new(name, move |inv: &Invocation<'_>| {
            calls.fetch_add(1, Ordering::SeqCst);
            let repr = Representation::new(ReprType::new("text/plain"), b"v".to_vec());
            // Turn the relative freshness window into an absolute deadline via the
            // injected clock — exactly how the HTTP module will map `max-age`.
            Ok(match inv.now() {
                Some(now) => repr.cacheable_until(now.plus_millis(window)),
                None => repr.cacheable_until(Time::from_millis(window)),
            })
        })
    }

    #[test]
    fn time_based_entry_serves_until_its_deadline_then_recomputes() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let clock = TestClock::at(1_000);
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:t:x"), timed_endpoint("x", 500, &CALLS)),
        ))
        .with_clock(Arc::new(clock.clone()));
        let cap = Capability::root();
        let req = || Request::new(Verb::Source, iri("urn:t:x"));

        // t=1000: computed, deadline = 1500.
        block_on(kernel.issue(req(), &cap)).unwrap();
        // t=1400: still fresh -> cache hit, not recomputed.
        clock.set(1_400);
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            1,
            "served from cache before deadline"
        );
        // t=1600: past the deadline -> stale, recomputes (new deadline = 2100).
        clock.set(1_600);
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(CALLS.load(Ordering::SeqCst), 2, "recomputed after deadline");
    }

    #[test]
    fn a_clockless_kernel_declines_to_cache_a_deadline() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        // No clock injected: the kernel can't evaluate a deadline, so an `At`
        // result is never stored and recomputes every time (rather than risk
        // serving it forever).
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:t:x"), timed_endpoint("x", 500, &CALLS)),
        ));
        let cap = Capability::root();
        let req = || Request::new(Verb::Source, iri("urn:t:x"));
        block_on(kernel.issue(req(), &cap)).unwrap();
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            2,
            "no clock -> At result not cached"
        );
        assert_eq!(kernel.cache_len(), 0);
    }

    #[test]
    fn a_deadline_propagates_through_composition() {
        static LEAF_CALLS: AtomicU32 = AtomicU32::new(0);
        let clock = TestClock::at(0);
        // `UpcaseOf` is a *permanently*-cacheable composer; sourcing a time-bounded
        // leaf must still cap the composite at the leaf's deadline (no fresher than
        // its most volatile part). LEAF_CALLS recomputing tracks the composite too,
        // since the composite only re-sources the leaf when it itself recomputes.
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new()
                .bind(
                    Exact::new("urn:t:leaf"),
                    timed_endpoint("leaf", 100, &LEAF_CALLS),
                )
                .bind(Exact::new("urn:fn:upcaseOf"), UpcaseOf),
        ))
        .with_clock(Arc::new(clock.clone()));
        let cap = Capability::root();
        let req = || {
            Request::new(Verb::Source, iri("urn:fn:upcaseOf"))
                .with_arg("src", ArgRef::Reference(iri("urn:t:leaf")))
        };

        // t=0: computed, inherited deadline = 100.
        let a = block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(a.bytes, b"V");
        // t=50: fresh -> composite served from cache, leaf not re-sourced.
        clock.set(50);
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(
            LEAF_CALLS.load(Ordering::SeqCst),
            1,
            "composite cached before the leaf's deadline"
        );
        // t=150: leaf deadline passed -> composite expired -> recomputes, re-sources leaf.
        clock.set(150);
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(
            LEAF_CALLS.load(Ordering::SeqCst),
            2,
            "composite expired with its leaf"
        );
    }

    #[test]
    fn is_cached_honours_the_deadline() {
        // The read-only probe must agree with the serving path: an expired entry
        // is not "cached", so a cache-status label (and the `cache` REPL command)
        // doesn't claim a Hit that the next issue would actually recompute.
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let clock = TestClock::at(0);
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:t:x"), timed_endpoint("x", 100, &CALLS)),
        ))
        .with_clock(Arc::new(clock.clone()));
        let cap = Capability::root();
        let req = || Request::new(Verb::Source, iri("urn:t:x"));

        block_on(kernel.issue(req(), &cap)).unwrap(); // cached, deadline = 100
        clock.set(50);
        assert!(
            kernel.is_cached(&req(), &Capability::root()),
            "fresh entry probes as cached"
        );
        clock.set(150);
        assert!(
            !kernel.is_cached(&req(), &Capability::root()),
            "expired entry probes as not cached"
        );
    }

    // --- Concurrent fan-out (Invocation::fan_out + an injected Spawner) -------

    /// A cooperative spawner for tests: returns each task as its own completion
    /// future so `join_all` drives them on the current task. Proves the fan-out
    /// plumbing without real threads — the threadpool deadlock-freedom test lives in
    /// `ikigai-scheduler`, which owns the executor.
    struct InlineSpawner;
    impl Spawner for InlineSpawner {
        fn spawn(&self, task: BoxFuture<()>) -> BoxFuture<()> {
            task
        }
    }

    fn leaf(byte: &'static str) -> FnEndpoint {
        FnEndpoint::new("leaf", move |_inv: &Invocation<'_>| {
            Ok(
                Representation::new(ReprType::new("text/plain"), byte.as_bytes().to_vec())
                    .cacheable(),
            )
        })
    }

    /// An endpoint that fans out to a fixed list of leaves and concatenates them.
    struct FanParent {
        leaves: Vec<Iri>,
    }
    #[async_trait::async_trait]
    impl Endpoint for FanParent {
        async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
            let requests = self
                .leaves
                .iter()
                .map(|iri| Request::new(Verb::Source, iri.clone()))
                .collect();
            let mut body = Vec::new();
            for result in inv.fan_out(requests).await {
                body.extend_from_slice(&result?.bytes);
            }
            Ok(Representation::new(ReprType::new("text/plain"), body).cacheable())
        }
    }

    fn fan_space() -> EndpointSpace {
        EndpointSpace::new()
            .bind(Exact::new("urn:leaf:1"), leaf("1"))
            .bind(Exact::new("urn:leaf:2"), leaf("2"))
            .bind(Exact::new("urn:leaf:3"), leaf("3"))
            .bind(
                Exact::new("urn:fan:parent"),
                FanParent {
                    leaves: vec![iri("urn:leaf:1"), iri("urn:leaf:2"), iri("urn:leaf:3")],
                },
            )
    }

    #[test]
    fn fan_out_resolves_all_in_order_on_a_scheduled_kernel() {
        let kernel = Kernel::new(Arc::new(fan_space())).into_scheduled(Arc::new(InlineSpawner));
        let out = block_on(kernel.issue(
            Request::new(Verb::Source, iri("urn:fan:parent")),
            &Capability::root(),
        ))
        .unwrap();
        assert_eq!(out.bytes, b"123", "fan-out preserves request order");
    }

    #[test]
    fn fan_out_falls_back_to_sequential_without_a_spawner() {
        // A plain (non-scheduled) kernel has no spawner — fan_out resolves
        // sequentially, with an identical result.
        let kernel = Kernel::new(Arc::new(fan_space()));
        let out = block_on(kernel.issue(
            Request::new(Verb::Source, iri("urn:fan:parent")),
            &Capability::root(),
        ))
        .unwrap();
        assert_eq!(out.bytes, b"123");
    }

    #[test]
    fn an_installed_tracer_records_each_invocation_then_stops_when_cleared() {
        struct Recorder(std::sync::Mutex<Vec<TraceEvent>>);
        impl Tracer for Recorder {
            fn record(&self, event: TraceEvent) {
                self.0.lock().expect("recorder").push(event);
            }
        }
        let recorder = Arc::new(Recorder(std::sync::Mutex::new(Vec::new())));
        let kernel = Kernel::new(Arc::new(fan_space()));
        let cap = Capability::root();
        let parent = || Request::new(Verb::Source, iri("urn:fan:parent"));

        kernel.set_tracer(recorder.clone());
        block_on(kernel.issue(parent(), &cap)).unwrap();
        kernel.clear_tracer();

        let events = recorder.0.lock().expect("recorder").clone();
        let targets: Vec<&str> = events.iter().map(|e| e.target.as_str()).collect();
        assert_eq!(events.len(), 4, "parent + 3 leaves, one event each");
        assert!(targets.contains(&"urn:fan:parent"));
        assert!(["urn:leaf:1", "urn:leaf:2", "urn:leaf:3"]
            .iter()
            .all(|leaf| targets.contains(leaf)));
        assert!(
            events.iter().all(|e| !e.cache_hit),
            "first resolution computes everything"
        );

        // Span linkage: the parent is a root (no parent span); every leaf names the
        // parent's span as its own parent — the edges that rebuild the execution tree.
        let root = events
            .iter()
            .find(|e| e.target == "urn:fan:parent")
            .expect("parent recorded");
        assert_eq!(root.parent, None, "the traced root has no parent span");
        assert!(
            events
                .iter()
                .filter(|e| e.target.starts_with("urn:leaf:"))
                .all(|leaf| leaf.parent == Some(root.span)),
            "each fanned-out leaf links to the parent's span"
        );
        assert!(
            events.iter().all(|e| e.capability.is_none()),
            "under root authority every hop records capability = None"
        );

        // A cleared tracer records nothing for subsequent resolutions.
        block_on(kernel.issue(parent(), &cap)).unwrap();
        assert_eq!(recorder.0.lock().expect("recorder").len(), 4);
    }

    #[test]
    fn each_event_records_the_capability_it_ran_under() {
        struct Recorder(std::sync::Mutex<Vec<TraceEvent>>);
        impl Tracer for Recorder {
            fn record(&self, event: TraceEvent) {
                self.0.lock().expect("recorder").push(event);
            }
        }
        let recorder = Arc::new(Recorder(std::sync::Mutex::new(Vec::new())));
        let kernel = Kernel::new(Arc::new(fan_space()));
        // A scoped (non-root) authority — passed out of input order to prove the
        // recorded set is sorted (the fingerprint's stability property).
        let cap = Capability::scoped(["urn:cap:b", "urn:cap:a"]);

        kernel.set_tracer(recorder.clone());
        block_on(kernel.issue(Request::new(Verb::Source, iri("urn:fan:parent")), &cap)).unwrap();
        kernel.clear_tracer();

        let events = recorder.0.lock().expect("recorder").clone();
        assert!(!events.is_empty(), "the scoped resolution recorded events");
        assert!(
            events
                .iter()
                .all(|e| e.capability
                    == Some(vec!["urn:cap:a".to_string(), "urn:cap:b".to_string()])),
            "each hop names the sorted scope set it ran under — attenuation is visible"
        );
    }
}
