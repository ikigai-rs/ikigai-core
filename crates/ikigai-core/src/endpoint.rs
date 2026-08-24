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

    /// How many spawned tasks can make progress **simultaneously** — this executor's
    /// achievable concurrency, and only that. It is **not** a queue depth, not a count
    /// of tasks outstanding or completed, and not how many branches a caller intends to
    /// dispatch: it is how many of them would actually be running at one instant if the
    /// caller handed over more work than the executor can carry at once.
    ///
    /// A caller reads it to *size* a fan-out — to know how much concurrency a shared
    /// downstream (an inference backend, a rate-limited API) will really see — so an
    /// honest small answer is worth more than a flattering large one.
    ///
    /// **A single-threaded executor answers `Some(1)`, never `None`.** An executor that
    /// runs one task to completion before the next has width 1, and saying so is the
    /// entire point of this accessor. That covers the inline case as well as the
    /// threaded one: a spawner that returns the task's own future to be polled
    /// cooperatively on the calling thread interleaves nothing when the work inside
    /// blocks, so its width is 1, not the number of tasks handed to it.
    ///
    /// `None` means **unknown** — reserved for a spawner that genuinely cannot answer
    /// (an elastic or remote pool whose size is not observable). It is not a shorthand
    /// for "small". Returning it forces the caller to guess, and the damaging guess is
    /// the likely one: read as "wide", a serialized workload gets routed to a batching
    /// backend and runs slower than sequencing it would have.
    ///
    /// Defaulted to `None` so every existing implementor keeps compiling untouched;
    /// override it wherever the number is known. This is a **read**, deliberately: the
    /// kernel never drives the scheduler, which lives above it and stays runtime-free
    /// (see [`SchedulerReporter`](crate::SchedulerReporter)), so there is no setter and
    /// no resize — a host that changes its executor's width reports the new number here.
    fn width(&self) -> Option<usize> {
        None
    }
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
    /// An explicitly attached source of "now", overriding the issuer's. Set only by
    /// [`with_clock`](Invocation::with_clock) — the kernel never sets it, because a
    /// kernel-driven invocation already reads the kernel's clock through its issuer.
    clock: Option<Arc<dyn crate::Clock>>,
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
    ///
    /// ## ★ Nothing fills in the bindings for you
    ///
    /// `bindings` is whatever the caller passes, and the caller is a test, so the
    /// usual value is `Bindings::default()` — **empty**. Template capture happens
    /// during resolution: [`Kernel::issue`](crate::Kernel::issue) matches the target
    /// against the grammars in scope and hands the endpoint whatever the matching
    /// [`Grammar`](crate::Grammar) captured. A detached invocation skips that step
    /// entirely, so an endpoint bound to `urn:thing:{app}` invoked detached sees no
    /// `app`, and behaves exactly as it would for the bare `urn:thing` — silently,
    /// on the branch that reads like success.
    ///
    /// That has already cost a session a failing test that looked like a golden-thread
    /// bug: the endpoint was reading the right layers, the test was resolving an IRI
    /// with an `{app}` segment in it, and the binding was never there to be read.
    /// If the behaviour under test depends on a captured variable, state it:
    ///
    /// ```
    /// # use ikigai_core::{Bindings, Capability, Invocation, Iri, Request, Verb};
    /// # let request = Request::new(Verb::Source, Iri::parse("urn:thing:cms-web").unwrap());
    /// # let capability = Capability::root();
    /// let mut bindings = Bindings::new();
    /// bindings.insert("app", "cms-web"); // the grammar would have captured this
    /// let inv = Invocation::detached(&request, &bindings, &capability);
    /// assert_eq!(inv.bindings.get("app"), Some("cms-web"));
    /// ```
    ///
    /// The IRI is not the source of truth here and writing it out in full does not
    /// help: the endpoint reads `inv.bindings`, not the target's text.
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
            clock: None,
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
            clock: None,
            span: None,
            trace: None,
            trace_notes: Mutex::new(Vec::new()),
            deps: Mutex::new(Vec::new()),
            dep_threads: Mutex::new(BTreeSet::new()),
        }
    }

    /// Attach an explicit source of "now", so [`now`](Self::now) answers without an
    /// issuer to read it from.
    ///
    /// **This exists for the detached case.** A kernel-driven invocation already has a
    /// clock — the kernel's, reached through its issuer — and the kernel does not call
    /// this. What had no answer at all was
    /// [`detached`](Self::detached): `now()` is the issuer's clock, a detached
    /// invocation has no issuer, so an endpoint that stamps its output from the kernel
    /// clock returned `None` in every detached test. Silently, and on the branch that
    /// reads like success — which is the part that cost something. By 2026-08-23
    /// `ikigai-browse` had five endpoints stamping `derived_at` from `now()` and not one
    /// test that had ever seen them produce a timestamp.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use ikigai_core::{Bindings, Capability, FixedClock, Invocation, Iri, Request, Verb};
    /// # let request = Request::new(Verb::Source, Iri::parse("urn:example").unwrap());
    /// # let bindings = Bindings::default();
    /// # let capability = Capability::root();
    /// let inv = Invocation::detached(&request, &bindings, &capability)
    ///     .with_clock(Arc::new(FixedClock::at(1_700_000_000_000)));
    /// assert_eq!(inv.now().map(|t| t.as_millis()), Some(1_700_000_000_000));
    /// ```
    ///
    /// An explicitly attached clock **wins over the issuer's**, on the general rule that
    /// what a caller stated beats what it inherited. Nothing in the kernel path reaches
    /// this, so the two cannot disagree in production.
    ///
    /// It does not make the detached test the *better* one. A detached invocation skips
    /// grammar-driven argument routing and kernel-side capability enforcement, so an
    /// endpoint exercised only that way is exercised only in part; the fuller test is a
    /// real [`Kernel`](crate::Kernel) with [`with_clock`](crate::Kernel::with_clock),
    /// and [`FixedClock`](crate::FixedClock) is there to make that one cheap too. This
    /// is here so that reaching for the kernel clock never costs an endpoint author its
    /// unit tests — the outcome that pushes an author toward `SystemTime::now()`, which
    /// is both unmockable and not wasm-clean.
    pub fn with_clock(mut self, clock: Arc<dyn crate::Clock>) -> Self {
        self.clock = Some(clock);
        self
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

    /// The current time per the kernel's injected [`Clock`](crate::Clock), or `None`
    /// if the kernel has no clock. An endpoint turns a relative freshness window into
    /// an absolute deadline with it — e.g.
    /// `inv.now().map(|t| repr.cacheable_until(t.plus_millis(max_age)))`.
    ///
    /// A clock attached with [`with_clock`](Self::with_clock) answers first; otherwise
    /// this is the issuer's clock. A [`detached`](Self::detached) invocation with
    /// neither has no time — which is the honest answer, not a fallback to the system
    /// clock: reading the wall clock behind the caller's back is what makes resolution
    /// non-replayable, and core does it in exactly one place, inside
    /// [`SystemClock`](crate::SystemClock), where a host opts into it by name.
    pub fn now(&self) -> Option<Time> {
        self.clock
            .as_ref()
            .map(|clock| clock.now())
            .or_else(|| self.issuer.and_then(|issuer| issuer.now()))
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

/// A pinned, boxed, `Send` future that may borrow the invocation it serves —
/// what an [`AsyncFnEndpoint`] closure returns. Unlike [`BoxFuture`] (which is
/// `'static`, for [`Spawner`] tasks) this is bounded by the invocation borrow,
/// which is what lets the future call [`Invocation::issue`] /
/// [`Invocation::source`] on its way to a result.
pub type InvokeFuture<'a> = Pin<Box<dyn Future<Output = Result<Representation>> + Send + 'a>>;

/// The boxed invocation function behind an [`AsyncFnEndpoint`].
type AsyncInvokeFn = Box<dyn for<'a, 'b> Fn(&'a Invocation<'b>) -> InvokeFuture<'a> + Send + Sync>;

/// An endpoint backed by an **async** Rust closure — [`FnEndpoint`]'s twin for
/// the composite case.
///
/// [`FnEndpoint`] takes a sync closure, so an endpoint that issues
/// sub-requests ([`Invocation::issue`] / [`Invocation::source`] are async) has
/// had to hand-implement [`Endpoint`] with `#[async_trait]` plus the
/// name/describe plumbing. This type is that boilerplate, once: the same flat
/// single-verb authoring as `FnEndpoint`, with an async body. Pure async
/// plumbing — no threads, no spawning — so it is wasm-clean.
///
/// The closure returns a boxed future over the invocation borrow (the shape
/// `#[async_trait]` expands to); author it as `|inv| Box::pin(async move
/// { … })`:
///
/// ```
/// use ikigai_core::{ArgRef, AsyncFnEndpoint, Error, Representation, ReprType};
///
/// let upcase_of = AsyncFnEndpoint::new("upcaseOf", |inv| {
///     Box::pin(async move {
///         let src = match inv.request.args.get("src") {
///             Some(ArgRef::Reference(iri)) => iri.clone(),
///             _ => return Err(Error::MissingArgument("src".to_string())),
///         };
///         let body = inv.source(&src).await?; // async sub-request through the kernel
///         Ok(Representation::new(
///             ReprType::new("text/plain"),
///             body.bytes.to_ascii_uppercase(),
///         ))
///     })
/// });
/// ```
pub struct AsyncFnEndpoint {
    name: String,
    invoke: AsyncInvokeFn,
    description: Option<Description>,
}

impl AsyncFnEndpoint {
    /// Build an endpoint from a name and an async invocation function (a
    /// closure returning a boxed [`InvokeFuture`], typically
    /// `|inv| Box::pin(async move { … })`).
    pub fn new<F>(name: impl Into<String>, invoke: F) -> Self
    where
        F: for<'a, 'b> Fn(&'a Invocation<'b>) -> InvokeFuture<'a> + Send + Sync + 'static,
    {
        AsyncFnEndpoint {
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
impl Endpoint for AsyncFnEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        (self.invoke)(inv).await
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::describe::ArgSpec;
    use crate::grammar::{Bindings, Exact};
    use crate::kernel::Kernel;
    use crate::repr::ReprType;
    use crate::space::EndpointSpace;
    use futures::executor::block_on;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn iri(s: &str) -> Iri {
        Iri::parse(s).unwrap()
    }

    #[test]
    fn an_async_closure_endpoint_invokes_detached() {
        // The async twin of FnEndpoint's basic contract: name + closure, invoked
        // directly against a detached invocation (no kernel).
        let greet = AsyncFnEndpoint::new("greet", |inv| {
            Box::pin(async move {
                let who = inv.inline_str("who")?;
                Ok(Representation::new(
                    ReprType::new("text/plain"),
                    format!("hi {who}").into_bytes(),
                ))
            })
        });
        assert_eq!(greet.name(), "greet");

        let request = Request::new(Verb::Source, iri("urn:demo:greet"))
            .with_arg("who", ArgRef::Inline(b"ada".to_vec()));
        let bindings = Bindings::default();
        let cap = Capability::root();
        let inv = Invocation::detached(&request, &bindings, &cap);
        let rep = block_on(greet.invoke(&inv)).unwrap();
        assert_eq!(rep.bytes, b"hi ada");
    }

    #[test]
    fn describe_defaults_to_the_name_and_honors_with_description() {
        // Catalog/manifold parity with FnEndpoint: no description reports just
        // the name; with_description reports exactly what was declared.
        let bare = AsyncFnEndpoint::new("bare", |_inv| {
            Box::pin(async { Ok(Representation::new(ReprType::new("text/plain"), Vec::new())) })
        });
        assert_eq!(bare.describe().id, "bare");

        let described = AsyncFnEndpoint::new("described", |_inv| {
            Box::pin(async { Ok(Representation::new(ReprType::new("text/plain"), Vec::new())) })
        })
        .with_description(
            Description::new("described")
                .verb(Verb::Source)
                .requires("urn:cap:demo")
                .input(ArgSpec::new("who")),
        );
        let description = described.describe();
        assert_eq!(description.id, "described");
        assert_eq!(description.verbs, vec![Verb::Source]);
        assert_eq!(description.requires, vec!["urn:cap:demo".to_string()]);
        let inputs: Vec<&str> = description.inputs.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(inputs, ["who"]);
    }

    #[test]
    fn an_async_endpoint_issues_sub_requests_through_the_kernel() {
        // The whole reason the type exists: the closure's future issues a
        // sub-request (async, borrowing the invocation) and the result flows
        // through — no hand-rolled Endpoint impl in sight.
        static LEAF: AtomicU32 = AtomicU32::new(0);
        let leaf = FnEndpoint::new("leaf", |_inv: &Invocation<'_>| {
            LEAF.fetch_add(1, Ordering::SeqCst);
            Ok(
                Representation::new(ReprType::new("text/plain"), b"hello".to_vec())
                    .cacheable()
                    .depends_on("urn:leaf"),
            )
        });
        let upcase_of = AsyncFnEndpoint::new("upcaseOf", |inv| {
            Box::pin(async move {
                let src = match inv.request.args.get("src") {
                    Some(ArgRef::Reference(iri)) => iri.clone(),
                    _ => return Err(Error::MissingArgument("src".to_string())),
                };
                let upstream = inv.source(&src).await?;
                let upper = String::from_utf8_lossy(&upstream.bytes).to_uppercase();
                Ok(
                    Representation::new(ReprType::new("text/plain"), upper.into_bytes())
                        .cacheable(),
                )
            })
        });
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:data:leaf"), leaf)
            .bind(Exact::new("urn:fn:upcaseOf"), upcase_of);
        let kernel = Kernel::new(Arc::new(space));
        let cap = Capability::root();
        let req = || {
            Request::new(Verb::Source, iri("urn:fn:upcaseOf"))
                .with_arg("src", ArgRef::Reference(iri("urn:data:leaf")))
        };

        let rep = block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(rep.bytes, b"HELLO");
        assert_eq!(LEAF.load(Ordering::SeqCst), 1);

        // Dependency plumbing flows exactly as through a hand-rolled endpoint:
        // the composite is cached, and cutting the LEAF's golden thread (which
        // the composite never declared itself) invalidates the composite too.
        block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(LEAF.load(Ordering::SeqCst), 1, "composite + leaf cached");
        kernel.cut("urn:leaf");
        let rep = block_on(kernel.issue(req(), &cap)).unwrap();
        assert_eq!(rep.bytes, b"HELLO");
        assert_eq!(
            LEAF.load(Ordering::SeqCst),
            2,
            "cutting the inherited thread recomputed the composite"
        );
    }

    #[test]
    fn a_detached_async_endpoint_cannot_issue() {
        // Mirror of the detached FnEndpoint behaviour: no kernel context means
        // sub-requests fail cleanly, not silently.
        let needs_kernel = AsyncFnEndpoint::new("needsKernel", |inv| {
            Box::pin(async move { inv.source(&iri("urn:data:leaf")).await })
        });
        let request = Request::new(Verb::Source, iri("urn:demo:needsKernel"));
        let bindings = Bindings::default();
        let cap = Capability::root();
        let inv = Invocation::detached(&request, &bindings, &cap);
        let err = block_on(needs_kernel.invoke(&inv)).unwrap_err();
        assert!(
            format!("{err:?}").contains("kernel context"),
            "detached issue fails cleanly: {err:?}"
        );
    }

    // --- Spawner::width (achievable concurrency, read-only) -------------------

    /// The single-threaded shape: the task's own future, polled cooperatively on the
    /// calling thread. Nothing interleaves, so the honest answer is `Some(1)` — never
    /// `None`, which would make a caller guess "wide" about a serialized executor.
    struct SingleThreaded;
    impl Spawner for SingleThreaded {
        fn spawn(&self, task: BoxFuture<()>) -> BoxFuture<()> {
            task
        }
        fn width(&self) -> Option<usize> {
            Some(1)
        }
    }

    /// A pool-shaped spawner reporting the number of tasks it can carry at once.
    struct Pool(usize);
    impl Spawner for Pool {
        fn spawn(&self, task: BoxFuture<()>) -> BoxFuture<()> {
            task
        }
        fn width(&self) -> Option<usize> {
            Some(self.0)
        }
    }

    /// An implementor written before `width` existed, left exactly as it was: it
    /// compiles untouched and answers `None` (unknown).
    struct Unhinted;
    impl Spawner for Unhinted {
        fn spawn(&self, task: BoxFuture<()>) -> BoxFuture<()> {
            task
        }
    }

    #[test]
    fn a_spawner_reports_its_width_through_the_trait_object() {
        // The kernel and the host both hold `Arc<dyn Spawner>`, so the number has to
        // survive dynamic dispatch — that is the whole path a caller reads it over.
        let single: Arc<dyn Spawner> = Arc::new(SingleThreaded);
        let pool: Arc<dyn Spawner> = Arc::new(Pool(8));
        let unhinted: Arc<dyn Spawner> = Arc::new(Unhinted);

        assert_eq!(
            single.width(),
            Some(1),
            "a single-threaded executor says 1, not unknown"
        );
        assert_eq!(pool.width(), Some(8), "a pool reports its achievable width");
        assert_eq!(
            unhinted.width(),
            None,
            "no override means unknown, and the default supplies it"
        );
    }

    #[test]
    fn the_width_default_changes_no_spawn_behaviour() {
        // Additive by construction: `width` is a read. Whatever it answers — 1, 8, or
        // unknown — the task still runs exactly as it did before the accessor existed.
        static RAN: AtomicU32 = AtomicU32::new(0);
        let spawners: Vec<Arc<dyn Spawner>> = vec![
            Arc::new(SingleThreaded),
            Arc::new(Pool(8)),
            Arc::new(Unhinted),
        ];
        for spawner in &spawners {
            block_on(spawner.spawn(Box::pin(async {
                RAN.fetch_add(1, Ordering::SeqCst);
            })));
        }
        assert_eq!(
            RAN.load(Ordering::SeqCst),
            3,
            "every spawner ran its task regardless of the width it reports"
        );
    }

    /// The gap this closes. A detached invocation has no issuer, `now()` is the
    /// issuer's clock, so an endpoint that stamps its output from the kernel clock
    /// produced `None` in every detached test — and `None` is a plausible-looking
    /// answer, so the test that asserted around it passed forever.
    #[test]
    fn a_detached_invocation_has_no_time_until_it_is_given_one() {
        let request = Request::new(Verb::Source, iri("urn:demo:stamp"));
        let bindings = Bindings::default();
        let cap = Capability::root();

        let bare = Invocation::detached(&request, &bindings, &cap);
        assert_eq!(bare.now(), None, "no issuer and no clock is no time");

        let stamped = Invocation::detached(&request, &bindings, &cap)
            .with_clock(Arc::new(crate::FixedClock::at(1_700_000_000_000)));
        assert_eq!(stamped.now(), Some(Time::from_millis(1_700_000_000_000)));
    }

    /// An endpoint reading the kernel clock is now testable detached — which is the
    /// whole point, since `detached` is the idiom eight repos test their endpoints
    /// with. Asserted through a real endpoint rather than on `now()` directly,
    /// because what regressed silently was an endpoint's OUTPUT.
    #[test]
    fn an_endpoint_that_stamps_from_the_clock_is_testable_detached() {
        let stamp = FnEndpoint::new("stamp", |inv: &Invocation| {
            let at = inv
                .now()
                .map(|t| t.as_millis().to_string())
                .unwrap_or_else(|| "no clock".to_string());
            Ok(Representation::new(
                ReprType::new("text/plain"),
                at.into_bytes(),
            ))
        });
        let request = Request::new(Verb::Source, iri("urn:demo:stamp"));
        let bindings = Bindings::default();
        let cap = Capability::root();

        let unclocked = Invocation::detached(&request, &bindings, &cap);
        assert_eq!(
            block_on(stamp.invoke(&unclocked)).unwrap().bytes,
            b"no clock",
            "the branch every detached test used to take"
        );

        let clocked = Invocation::detached(&request, &bindings, &cap)
            .with_clock(Arc::new(crate::FixedClock::at(42)));
        assert_eq!(block_on(stamp.invoke(&clocked)).unwrap().bytes, b"42");
    }

    /// An explicitly attached clock beats the issuer's, on the rule that what a
    /// caller stated beats what it inherited. Nothing in the kernel path attaches
    /// one, so the two never disagree in production — but the precedence is a
    /// promise, so it is pinned here rather than left to whichever branch of `now()`
    /// happens to be written first.
    #[test]
    fn an_attached_clock_outranks_the_issuers() {
        let space = EndpointSpace::new().bind(
            Exact::new("urn:demo:stamp"),
            FnEndpoint::new("stamp", |_: &Invocation| {
                Ok(Representation::new(ReprType::new("text/plain"), Vec::new()))
            }),
        );
        let kernel = Kernel::new(Arc::new(space)).with_clock(Arc::new(crate::FixedClock::at(1)));

        let request = Request::new(Verb::Source, iri("urn:demo:stamp"));
        let bindings = Bindings::default();
        let cap = Capability::root();

        let inherited = Invocation::with_issuer(&request, &bindings, &cap, &kernel);
        assert_eq!(inherited.now(), Some(Time::from_millis(1)));

        let stated = Invocation::with_issuer(&request, &bindings, &cap, &kernel)
            .with_clock(Arc::new(crate::FixedClock::at(2)));
        assert_eq!(stated.now(), Some(Time::from_millis(2)));
    }
}
