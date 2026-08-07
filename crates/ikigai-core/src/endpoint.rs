use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::arg::ArgRef;
use crate::capability::Capability;
use crate::describe::Description;
use crate::error::{Error, Result};
use crate::grammar::Bindings;
use crate::iri::Iri;
use crate::repr::{Expiry, Representation, Thread, Time};
use crate::request::Request;
use crate::select::{ActionMatch, TransreptionStep};
use crate::verb::Verb;

/// Lets an endpoint issue sub-requests back through the kernel. Implemented by
/// the [`Kernel`](crate::Kernel); a detached [`Invocation`] has no issuer, so
/// `source`/`issue` are unavailable when testing an endpoint in isolation.
#[async_trait]
pub trait Issuer: Send + Sync {
    /// Resolve and evaluate a sub-request.
    async fn issue(&self, request: Request, capability: &Capability) -> Result<Representation>;

    /// Like [`issue`](Issuer::issue), but carrying the issuing invocation's trace
    /// `parent` span — so a recorded execution links each sub-request to the node
    /// that issued it (the tree the `trace` command renders). The default ignores it
    /// and delegates to `issue`; the kernel overrides it to thread the span. `parent`
    /// is `None` outside tracing, so this is free off the trace path.
    async fn issue_with_parent(
        &self,
        request: Request,
        capability: &Capability,
        parent: Option<u64>,
    ) -> Result<Representation> {
        let _ = parent;
        self.issue(request, capability).await
    }

    /// Like [`issue_with_parent`](Issuer::issue_with_parent), additionally carrying
    /// the [`TraceScope`](crate::TraceScope) of the resolution this sub-request
    /// belongs to — so concurrent traced resolutions on one shared kernel each
    /// record into their *own* collector, never a neighbor's. The default drops the
    /// scope and delegates (a detached or remote issuer has no trace to record
    /// into); the kernel overrides it. `trace` is `None` off the trace path.
    async fn issue_scoped(
        &self,
        request: Request,
        capability: &Capability,
        parent: Option<u64>,
        trace: Option<crate::TraceScope>,
    ) -> Result<Representation> {
        let _ = trace;
        self.issue_with_parent(request, capability, parent).await
    }

    /// Merge a subtree of [`TraceEvent`](crate::TraceEvent)s produced by *another*
    /// kernel — a remote one reached through a mounted `RemoteSpace` — into this
    /// issuer's trace, re-based under `parent` (the span of the invocation that
    /// forwarded the request). The default ignores them (a detached or remote issuer
    /// has no trace to merge into); the kernel overrides it to re-map the span ids
    /// and record. Reached by an endpoint through [`Invocation::record_subtree`].
    fn record_subtree(&self, parent: Option<u64>, spans: Vec<crate::TraceEvent>) {
        let _ = (parent, spans);
    }

    /// The current time per the issuer's injected [`Clock`](crate::Clock), or
    /// `None` if it has none. An endpoint computing a time-based deadline (e.g.
    /// `now + max-age`) reads it through [`Invocation::now`]. Default `None`.
    fn now(&self) -> Option<Time> {
        None
    }

    /// Plan a chain of transreptors converting media type `from` → `to` over the
    /// issuer's mounted spaces (see [`select_transreptor`](crate::select_transreptor)).
    /// The default offers none — a detached or remote issuer can't enumerate spaces; the
    /// kernel overrides it to select over its root. An endpoint reads it through
    /// [`Invocation::select_transreptor`].
    fn select_transreptor(&self, from: &str, to: &str) -> Option<Vec<TransreptionStep>> {
        let _ = (from, to);
        None
    }

    /// Find endpoints whose required inputs are satisfiable by the RDF classes in `present`
    /// (see [`select_action`](crate::select_action)) — "given these typed entities, what can
    /// I do with them?" The default offers none (a detached or remote issuer can't enumerate
    /// spaces); the kernel overrides it. An endpoint reads it through
    /// [`Invocation::select_action`].
    fn select_action(&self, present: &[&str]) -> Vec<ActionMatch> {
        let _ = present;
        Vec::new()
    }
}

/// A pinned, boxed, `Send` future — the unit of work a [`Spawner`] runs.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Runs futures concurrently on the host's executor. Injected into the kernel like
/// [`Clock`](crate::Clock) — via [`Kernel::into_scheduled`](crate::Kernel::into_scheduled)
/// — and object-safe so it can be a trait object; `ikigai-scheduler` implements it.
/// With no spawner, [`Invocation::fan_out`] falls back to sequential resolution, so
/// the kernel stays runtime-free and single-threaded by default.
pub trait Spawner: Send + Sync {
    /// Spawn `task` to run concurrently; the returned future resolves when it
    /// completes, so a caller can join several spawned tasks — **parking**, not
    /// blocking, until they finish (which is what keeps re-entrant fan-out from
    /// pinning a thread while its children run).
    fn spawn(&self, task: BoxFuture<()>) -> BoxFuture<()>;
}

/// The context handed to an endpoint when it is invoked.
///
/// All input arrives here — the request, the grammar-captured bindings, and the
/// authorizing capability — and, when the kernel is driving, the ability to
/// issue sub-requests via [`Invocation::source`] / [`Invocation::issue`].
/// Endpoints take no ambient authority.
pub struct Invocation<'a> {
    /// The request being served.
    pub request: &'a Request,
    /// Variables captured by the grammar that resolved this request.
    pub bindings: &'a Bindings,
    /// The capability authorizing invocation.
    pub capability: &'a Capability,
    issuer: Option<&'a dyn Issuer>,
    /// Concurrency context for [`fan_out`](Invocation::fan_out): the host's spawner
    /// and an *owned* issuer handle (so a spawned sub-request can re-enter the kernel
    /// without borrowing this invocation). Both present only on a
    /// [`scheduled`](crate::Kernel::into_scheduled) kernel; otherwise fan-out is
    /// sequential.
    spawner: Option<Arc<dyn Spawner>>,
    issuer_arc: Option<Arc<dyn Issuer>>,
    /// This invocation's trace span, when the kernel is recording — so a sub-request
    /// it issues is linked to this node as its parent. `None` off the trace path.
    span: Option<u64>,
    /// The trace scope this invocation records into, when the kernel is recording —
    /// threaded into every sub-request so concurrent traced resolutions on one
    /// shared kernel stay isolated. `None` off the trace path.
    trace: Option<crate::TraceScope>,
    /// Facts the endpoint attached to its own span via
    /// [`trace_note`](Self::trace_note); drained by the kernel into the
    /// [`TraceEvent`](crate::TraceEvent) once the invocation completes. Only
    /// collected while tracing (no cost, no growth off the trace path).
    trace_notes: Mutex<Vec<(String, String)>>,
    deps: Mutex<Vec<Expiry>>,
    /// Union of the golden threads of every sub-resource resolved during this
    /// invocation — so the kernel can propagate them onto the result.
    dep_threads: Mutex<BTreeSet<Thread>>,
}

impl<'a> Invocation<'a> {
    /// A context with no kernel attached: `source`/`issue` are unavailable.
    /// Useful for invoking an endpoint directly in tests.
    pub fn detached(
        request: &'a Request,
        bindings: &'a Bindings,
        capability: &'a Capability,
    ) -> Self {
        Invocation {
            request,
            bindings,
            capability,
            issuer: None,
            spawner: None,
            issuer_arc: None,
            span: None,
            trace: None,
            trace_notes: Mutex::new(Vec::new()),
            deps: Mutex::new(Vec::new()),
            dep_threads: Mutex::new(BTreeSet::new()),
        }
    }

    /// A context backed by an issuer, enabling sub-requests (`source`/`issue`).
    ///
    /// The kernel builds these with itself as the issuer. It's also the seam a
    /// dynamically-loaded **module** uses: a module shim runs its endpoint with a
    /// *host-backed* issuer, so the endpoint's `inv.source`/`inv.issue` resolve
    /// against the host kernel (its cache, its other spaces) across the module
    /// boundary — the endpoint code is unchanged, only the issuer is remote.
    pub fn with_issuer(
        request: &'a Request,
        bindings: &'a Bindings,
        capability: &'a Capability,
        issuer: &'a dyn Issuer,
    ) -> Self {
        Invocation {
            request,
            bindings,
            capability,
            issuer: Some(issuer),
            spawner: None,
            issuer_arc: None,
            span: None,
            trace: None,
            trace_notes: Mutex::new(Vec::new()),
            deps: Mutex::new(Vec::new()),
            dep_threads: Mutex::new(BTreeSet::new()),
        }
    }

    /// Attach this invocation's trace span (set by the kernel when recording), so
    /// sub-requests issued from it link to this node as their parent.
    pub(crate) fn with_span(mut self, span: Option<u64>) -> Self {
        self.span = span;
        self
    }

    /// Attach the trace scope this invocation belongs to (set by the kernel when
    /// recording), threaded into sub-requests so they record into the same trace.
    pub(crate) fn with_trace(mut self, trace: Option<crate::TraceScope>) -> Self {
        self.trace = trace;
        self
    }

    /// Attach the concurrency context — the injected [`Spawner`] and an owned
    /// [`Issuer`] handle — so [`fan_out`](Self::fan_out) can spawn sub-requests
    /// concurrently. Set by the kernel when it has been made schedulable via
    /// [`Kernel::into_scheduled`](crate::Kernel::into_scheduled).
    pub(crate) fn with_concurrency(
        mut self,
        spawner: Option<Arc<dyn Spawner>>,
        issuer_arc: Option<Arc<dyn Issuer>>,
    ) -> Self {
        self.spawner = spawner;
        self.issuer_arc = issuer_arc;
        self
    }

    /// This invocation's trace span, or `None` when the kernel isn't recording. An
    /// endpoint that forwards to another kernel checks this to decide whether to
    /// trace the forward, then passes it — via [`record_subtree`](Self::record_subtree)
    /// — as the parent to re-base the returned spans under.
    pub fn trace_span(&self) -> Option<u64> {
        self.span
    }

    /// Attach a `key = value` fact to this invocation's own trace span — e.g. the
    /// LLM facade noting `model` / `provider` it resolved to, or the HTTP client
    /// noting the redirect hops it followed. A no-op unless this resolution is
    /// being traced, so it is free on the hot path; notes land on the
    /// [`TraceEvent`](crate::TraceEvent) the kernel records for this node.
    pub fn trace_note(&self, key: impl Into<String>, value: impl Into<String>) {
        if self.trace.is_none() {
            return;
        }
        self.trace_notes
            .lock()
            .expect("trace notes lock")
            .push((key.into(), value.into()));
    }

    /// Drain the notes recorded during this invocation (kernel-side, at
    /// trace-record time).
    pub(crate) fn take_trace_notes(&self) -> Vec<(String, String)> {
        std::mem::take(&mut self.trace_notes.lock().expect("trace notes lock"))
    }

    /// Merge a subtree of [`TraceEvent`](crate::TraceEvent)s from another kernel into
    /// this invocation's trace, re-based under this node — so a resolution forwarded
    /// to a remote kernel (through a mounted `RemoteSpace`) shows the remote's
    /// execution stitched under the mount, not collapsed into one node. A no-op off
    /// the trace path or when detached.
    pub fn record_subtree(&self, spans: Vec<crate::TraceEvent>) {
        if let Some(issuer) = self.issuer {
            issuer.record_subtree(self.span, spans);
        }
    }

    /// The bytes of an inline argument, or an error if absent / not inline.
    pub fn inline_arg(&self, name: &str) -> Result<&[u8]> {
        match self.request.args.get(name) {
            Some(ArgRef::Inline(bytes)) => Ok(bytes),
            Some(_) => Err(Error::InvalidArgument {
                name: name.to_string(),
                detail: "expected an inline value".to_string(),
            }),
            None => Err(Error::MissingArgument(name.to_string())),
        }
    }

    /// An inline argument decoded as UTF-8.
    pub fn inline_str(&self, name: &str) -> Result<&str> {
        std::str::from_utf8(self.inline_arg(name)?).map_err(|_| Error::InvalidArgument {
            name: name.to_string(),
            detail: "not valid UTF-8".to_string(),
        })
    }

    /// Issue a sub-request through the kernel, recording it as a dependency of
    /// this invocation's result so expiry propagates. Errors if detached.
    pub async fn issue(&self, request: Request) -> Result<Representation> {
        let issuer = self
            .issuer
            .ok_or_else(|| Error::Endpoint("sub-requests require a kernel context".to_string()))?;
        let representation = issuer
            .issue_scoped(request, self.capability, self.span, self.trace.clone())
            .await?;
        self.deps
            .lock()
            .expect("deps lock")
            .push(representation.expiry);
        // Inherit the sub-resource's golden threads so cutting any of them
        // invalidates this (composite) result too.
        self.dep_threads
            .lock()
            .expect("dep threads lock")
            .extend(representation.threads().iter().cloned());
        Ok(representation)
    }

    /// `SOURCE` another resource — dereference a by-reference argument — recording
    /// it as a dependency.
    pub async fn source(&self, target: &Iri) -> Result<Representation> {
        self.issue(Request::new(Verb::Source, target.clone())).await
    }

    /// Plan a transreptor chain converting media type `from` → `to` over the kernel's
    /// mounted spaces, or `None` if there's no kernel context (detached) or no chain
    /// exists. The endpoint then issues each [`TransreptionStep`] — piping the bytes in
    /// as `content` and setting `as` to the step's target — to run the conversion. This
    /// is the seam content-negotiation and octet-stream sniff-and-dispatch build on:
    /// "find me a way from type A to type B," then drive it through the kernel like any
    /// sub-request.
    pub fn select_transreptor(&self, from: &str, to: &str) -> Option<Vec<TransreptionStep>> {
        self.issuer?.select_transreptor(from, to)
    }

    /// Find endpoints whose required inputs are satisfiable by the RDF classes in `present`
    /// — the actions available given a set of typed entities (see
    /// [`select_action`](crate::select_action)). Empty if there's no kernel context
    /// (detached). The seed of layer action-inference: a layer endpoint can surface "what you
    /// can do with what's on the canvas," then issue the chosen one.
    pub fn select_action(&self, present: &[&str]) -> Vec<ActionMatch> {
        match self.issuer {
            Some(issuer) => issuer.select_action(present),
            None => Vec::new(),
        }
    }

    /// Resolve `requests` **concurrently**, returning their results in request order.
    ///
    /// On a [`scheduled`](crate::Kernel::into_scheduled) kernel each sub-request is
    /// spawned as its own task and the join *parks* — so a re-entrant fan-out (e.g.
    /// `compose` expanding several `$a{}` markers) never holds a thread while its
    /// children run, and a child can run on the thread the parent released. Without a
    /// spawner it falls back to sequential [`issue`](Self::issue) — the kernel's
    /// default single-threaded behaviour. Either way each result's expiry and golden
    /// threads are recorded as dependencies of this invocation, exactly like `issue`.
    pub async fn fan_out(&self, requests: Vec<Request>) -> Vec<Result<Representation>> {
        let (Some(spawner), Some(issuer)) = (&self.spawner, &self.issuer_arc) else {
            // Sequential fallback: same order, same dependency recording as `issue`.
            let mut results = Vec::with_capacity(requests.len());
            for request in requests {
                results.push(self.issue(request).await);
            }
            return results;
        };

        // Spawn each sub-request into its own slot, then join (parking) on all.
        let slots: Vec<Arc<Mutex<Option<Result<Representation>>>>> = requests
            .iter()
            .map(|_| Arc::new(Mutex::new(None)))
            .collect();
        let joins: Vec<BoxFuture<()>> = requests
            .into_iter()
            .zip(&slots)
            .map(|(request, slot)| {
                let issuer = Arc::clone(issuer);
                let capability = self.capability.clone();
                let slot = Arc::clone(slot);
                // Carry this invocation's span AND trace scope across the spawn, so
                // each spawned sub-request links to this node as its parent and
                // records into this resolution's own trace — that's what lets the
                // recorded events reconstruct the real (concurrent) execution tree
                // without bleeding into a concurrently-traced neighbor.
                let parent = self.span;
                let trace = self.trace.clone();
                spawner.spawn(Box::pin(async move {
                    let result = issuer
                        .issue_scoped(request, &capability, parent, trace)
                        .await;
                    *slot.lock().expect("fan-out slot") = Some(result);
                }))
            })
            .collect();
        futures_util::future::join_all(joins).await;

        // Collect in order; record each result's dependency expiry and threads.
        let mut results = Vec::with_capacity(slots.len());
        for slot in slots {
            let result = slot
                .lock()
                .expect("fan-out slot")
                .take()
                .expect("spawned fan-out task completed");
            if let Ok(representation) = &result {
                self.deps
                    .lock()
                    .expect("deps lock")
                    .push(representation.expiry);
                self.dep_threads
                    .lock()
                    .expect("dep threads lock")
                    .extend(representation.threads().iter().cloned());
            }
            results.push(result);
        }
        results
    }

    /// Run a **synchronous** closure on its own thread, giving it a cloneable,
    /// `'static`, blocking [`SyncIssuer`] whose calls are served by THIS
    /// invocation — the bridge every embedded evaluator needs.
    ///
    /// The problem this solves: [`issue`](Self::issue) is async and borrows the
    /// invocation, but a sync embedded runtime (a Steel `register_fn` builtin,
    /// a Python callable under the GIL, a JS threadsafe function) needs a
    /// `Send + Sync + 'static` handle it can call BLOCKING, and a naive
    /// `block_on` inside would nest executors and deadlock. ikigai-lisp proved
    /// the working shape — a dedicated thread plus a channel the async side
    /// drains — and this is that bridge, in core, for every consumer.
    ///
    /// Mechanics: `f` runs on a fresh thread holding a [`SyncIssuer`]; each
    /// `issuer.issue(req)` crosses a channel and is served here via
    /// [`issue`](Self::issue) — so capability attenuation and enforcement,
    /// cache dependency/golden-thread recording, and trace parentage are all
    /// EXACTLY as if the endpoint had issued the sub-request itself. The scope
    /// returns when `f` does (all issuer clones dropped ⇒ the drain ends).
    /// Sub-requests are served one at a time, in arrival order.
    ///
    /// A panic in `f` surfaces as an error, not a poisoned kernel. Requires a
    /// kernel context (errors when detached) and real threads (unavailable
    /// under wasm — module endpoints there use the host-call seam instead).
    #[cfg(not(target_family = "wasm"))]
    pub async fn scope_sync<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(SyncIssuer) -> R + Send + 'static,
    {
        use futures_util::StreamExt;
        if self.issuer.is_none() {
            return Err(Error::Endpoint(
                "sub-requests require a kernel context".to_string(),
            ));
        }
        let (tx, mut rx) = futures_channel::mpsc::unbounded::<SyncCall>();
        let handle = std::thread::Builder::new()
            .name("ikigai-sync-scope".to_string())
            .spawn(move || f(SyncIssuer { tx }))
            .map_err(|e| Error::Endpoint(format!("sync scope thread failed to start: {e}")))?;
        // Serve the closure's sub-requests until every issuer clone is gone —
        // which is when `f` has returned (or unwound). Each one goes through
        // `self.issue`, so this invocation records it as a dependency.
        while let Some(call) = rx.next().await {
            let result = self.issue(call.request).await;
            // A dropped receiver just means the closure gave up waiting; the
            // dependency accounting above already happened, so nothing to undo.
            let _ = call.reply.send(result);
        }
        // The channel closed, so `f` is done; this join is immediate.
        handle
            .join()
            .map_err(|_| Error::Endpoint("sync scope panicked".to_string()))
    }

    /// The current time per the kernel's injected [`Clock`](crate::Clock), or
    /// `None` if the kernel has no clock (or the invocation is detached). An
    /// endpoint turns a relative freshness window into an absolute deadline with
    /// it — e.g. `inv.now().map(|t| repr.cacheable_until(t.plus_millis(max_age)))`.
    pub fn now(&self) -> Option<Time> {
        self.issuer.and_then(|issuer| issuer.now())
    }

    /// Combined expiry of the dependencies issued during this invocation: the
    /// [meet](Expiry::most_restrictive) of them all, so the result is no fresher
    /// than its most volatile dependency. `Always` if any is volatile, the earliest
    /// `At` deadline among any time-bounded ones, else `Never` (no deps ⇒ `Never`,
    /// imposing no limit).
    pub(crate) fn dependency_expiry(&self) -> Expiry {
        let deps = self.deps.lock().expect("deps lock");
        deps.iter()
            .copied()
            .fold(Expiry::Never, Expiry::most_restrictive)
    }

    /// The union of golden threads of every dependency resolved during this
    /// invocation — the kernel unions these onto the result's own threads.
    pub(crate) fn dependency_threads(&self) -> BTreeSet<Thread> {
        self.dep_threads.lock().expect("dep threads lock").clone()
    }
}

/// One bridged sub-request: the request and the channel its answer returns on.
#[cfg(not(target_family = "wasm"))]
struct SyncCall {
    request: Request,
    reply: std::sync::mpsc::SyncSender<Result<Representation>>,
}

/// A cloneable, `Send + Sync + 'static`, **blocking** handle for issuing
/// sub-requests from synchronous code — minted by
/// [`Invocation::scope_sync`], served by the invocation that minted it.
///
/// This is what a sync embedded runtime's callbacks capture: a Steel builtin,
/// a Python callable, a JS function. Authority is NOT carried here — every
/// call is resolved under the minting invocation's capability, so a handle
/// cannot widen what its endpoint could reach, and everything it resolves is
/// recorded as a dependency (cache expiry, golden threads, trace parentage)
/// of that invocation's result.
///
/// Blocking [`issue`](Self::issue) parks the CALLING thread (the closure's own
/// dedicated thread), never the kernel's executor. Once the scope that minted
/// this handle has ended, calls fail with a clean error.
#[cfg(not(target_family = "wasm"))]
#[derive(Clone)]
pub struct SyncIssuer {
    tx: futures_channel::mpsc::UnboundedSender<SyncCall>,
}

#[cfg(not(target_family = "wasm"))]
impl SyncIssuer {
    /// Issue a sub-request and BLOCK until its representation (or error)
    /// comes back. Fails cleanly when the minting scope has ended.
    pub fn issue(&self, request: Request) -> Result<Representation> {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.tx
            .unbounded_send(SyncCall { request, reply })
            .map_err(|_| Error::Endpoint("the sync scope has ended".to_string()))?;
        rx.recv()
            .map_err(|_| Error::Endpoint("the sync scope ended mid-request".to_string()))?
    }

    /// `SOURCE` a resource by IRI — the common case, as sugar.
    pub fn source(&self, target: &Iri) -> Result<Representation> {
        self.issue(Request::new(Verb::Source, target.clone()))
    }
}

/// An endpoint produces a [`Representation`] in response to a request.
///
/// Endpoints are synchronous and free of ambient authority in M1: everything
/// they may use arrives through the [`Invocation`]. (Async execution and
/// sub-request issuing are introduced with the kernel.)
#[async_trait]
pub trait Endpoint: Send + Sync {
    /// Produce a representation for the invocation.
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation>;

    /// A short label for diagnostics.
    fn name(&self) -> &str {
        "endpoint"
    }

    /// A structured self-description, which `ikigai-vocab` can project to RDF.
    /// The default reports just the endpoint's name.
    fn describe(&self) -> Description {
        Description::new(self.name())
    }
}

/// The boxed invocation function behind a [`FnEndpoint`].
type InvokeFn = Box<dyn Fn(&Invocation<'_>) -> Result<Representation> + Send + Sync>;

/// An endpoint backed by a Rust closure — the simplest, idempotent kind.
pub struct FnEndpoint {
    name: String,
    invoke: InvokeFn,
    description: Option<Description>,
}

impl FnEndpoint {
    /// Build an endpoint from a name and an invocation function.
    pub fn new(
        name: impl Into<String>,
        invoke: impl Fn(&Invocation<'_>) -> Result<Representation> + Send + Sync + 'static,
    ) -> Self {
        FnEndpoint {
            name: name.into(),
            invoke: Box::new(invoke),
            description: None,
        }
    }

    /// Attach a self-description declaring this endpoint's parameter contract,
    /// verbs, and outputs (builder). Without one, [`Endpoint::describe`] reports
    /// just the name.
    pub fn with_description(mut self, description: Description) -> Self {
        self.description = Some(description);
        self
    }
}

#[async_trait]
impl Endpoint for FnEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        (self.invoke)(inv)
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn describe(&self) -> Description {
        self.description
            .clone()
            .unwrap_or_else(|| Description::new(&self.name))
    }
}
