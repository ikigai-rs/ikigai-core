//! Overlay argument views (ledger #4319): what an overlay endpoint can do TODAY to amend
//! the arguments of the request it was invoked with, pinned with the public API alone.
//! These are the reproductions `docs/design/overlay-argument-view.md` starts from; they
//! change nothing in the kernel and propose no type.
//!
//! The overlay here wraps an inner endpoint that reads a `budget` argument and sources a
//! dependency. An outer request never carries `budget`: the overlay supplies it from
//! state outside the request, the way a door's run budget reaches a downstream overlay.
//!
//! 1. A DETACHED inner invocation has no issuer: the inner endpoint cannot source its
//!    dependency, so the composed answer cannot hang from it.
//! 2. RE-ISSUING the amended request records the dependency (a cut drops the outer
//!    answer), but costs one depth and leaves a second cache entry keyed on the amended
//!    request.
//! 3. An outer CACHE HIT skips the overlay: a changed budget is never applied.
//! 4. The kernel's own request reborrow (reference settlement, ledger #2511) shares the
//!    record and costs no depth: the shape the note asks a public view to have.
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::executor::block_on;
use ikigai_core::{
    ArgRef, ArgSpec, Capability, Description, Endpoint, EndpointSpace, Error, Exact, FnEndpoint,
    Invocation, Iri, Kernel, ReprType, Representation, Request, Result, Verb,
};

const DATA: &str = "urn:test:data";
const THREAD: &str = "urn:test:data:thread";
const OVERLAY: &str = "urn:test:query";
const BUDGET: &str = "budget";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(s: impl Into<String>) -> Representation {
    Representation::new(ReprType::new("text/plain"), s.into().into_bytes()).cacheable()
}

/// `urn:test:data`: a cacheable answer hanging from `THREAD`.
fn data() -> FnEndpoint {
    FnEndpoint::new("data", |inv| {
        inv.depends_on(THREAD);
        Ok(text("rows"))
    })
}

/// The inner endpoint: reads `budget`, sources `urn:test:data`, answers both.
fn inner() -> Arc<dyn Endpoint> {
    Arc::new(ikigai_core::AsyncFnEndpoint::new("inner", |inv| {
        Box::pin(async move {
            let budget = inv.inline_str(BUDGET)?.to_string();
            let rows = inv.source(&iri(DATA)).await?;
            Ok(text(format!(
                "{} under {budget}",
                String::from_utf8_lossy(&rows.bytes)
            )))
        })
    }))
}

/// How an overlay hands the amended request to its inner endpoint.
#[derive(Clone, Copy)]
enum Route {
    /// `Invocation::detached` over the amended request.
    Detached,
    /// `inv.issue` of the amended request, guarded by the stamp it carries.
    Reissue,
}

/// An overlay that stamps `budget` (read from `budget`, outside the request) and
/// hands the amended request on by `route`, counting its own runs.
struct Overlay {
    inner: Arc<dyn Endpoint>,
    route: Route,
    budget: Arc<AtomicU32>,
    runs: Arc<AtomicUsize>,
}

impl Overlay {
    fn amended(&self, inv: &Invocation<'_>) -> Request {
        let stamp = self.budget.load(Ordering::SeqCst).to_string();
        let mut request = inv.request.clone();
        request
            .args
            .insert(BUDGET.to_string(), ArgRef::Inline(stamp.into_bytes()));
        request
    }
}

#[async_trait]
impl Endpoint for Overlay {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        if inv.request.args.contains_key(BUDGET) {
            // The guard that terminates the re-issue: already stamped, run the inner.
            return self.inner.invoke(inv).await;
        }
        self.runs.fetch_add(1, Ordering::SeqCst);
        let amended = self.amended(inv);
        match self.route {
            Route::Detached => {
                let detached = Invocation::detached(&amended, inv.bindings, inv.capability);
                self.inner.invoke(&detached).await
            }
            Route::Reissue => inv.issue(amended).await,
        }
    }

    fn name(&self) -> &str {
        "overlay"
    }
}

struct Fixture {
    kernel: Kernel,
    budget: Arc<AtomicU32>,
    runs: Arc<AtomicUsize>,
}

fn fixture(route: Route) -> Fixture {
    let budget = Arc::new(AtomicU32::new(10));
    let runs = Arc::new(AtomicUsize::new(0));
    let overlay = Overlay {
        inner: inner(),
        route,
        budget: budget.clone(),
        runs: runs.clone(),
    };
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new(DATA), data())
            .bind(Exact::new(OVERLAY), overlay),
    ));
    Fixture {
        kernel,
        budget,
        runs,
    }
}

fn query() -> Request {
    Request::new(Verb::Source, iri(OVERLAY))
}

/// 1. A detached invocation has no issuer: the inner endpoint's sub-request fails, so a
///    detached route cannot compose an inner endpoint that reads anything.
#[test]
fn a_detached_inner_invocation_cannot_issue_its_dependency() {
    let f = fixture(Route::Detached);
    let err = block_on(f.kernel.issue(query(), &Capability::root())).unwrap_err();
    assert!(
        matches!(err, Error::Endpoint(ref detail) if detail.contains("require a kernel context")),
        "the refusal is the missing issuer: {err:?}"
    );
    assert_eq!(f.runs.load(Ordering::SeqCst), 1, "the overlay ran");
}

/// 2. Re-issuing records the dependency on the outer invocation, and leaves a second cache
///    entry keyed on the amended request beside the outer one keyed on the original.
#[test]
fn reissue_records_dependencies_and_keys_the_amended_request_separately() {
    let f = fixture(Route::Reissue);
    let root = Capability::root();
    let answer = block_on(f.kernel.issue(query(), &root)).unwrap();
    assert_eq!(answer.bytes, b"rows under 10");

    let mut amended = query();
    amended
        .args
        .insert(BUDGET.to_string(), ArgRef::Inline(b"10".to_vec()));
    assert!(f.kernel.is_cached(&query(), &root), "outer, original key");
    assert!(f.kernel.is_cached(&amended, &root), "inner, amended key");

    // The dependency reached the outer answer: a cut of the data drops both entries.
    f.kernel.cut(THREAD);
    assert!(!f.kernel.is_cached(&query(), &root));
    assert!(!f.kernel.is_cached(&amended, &root));
}

/// 2b. Re-issuing costs one depth. Under a budget of 1 the inner endpoint, invoked
///     directly at depth 0, may source its data at depth 1; behind a re-issuing overlay it
///     runs at depth 1 and its read at depth 2 is refused.
#[test]
fn reissue_costs_one_depth() {
    let root = Capability::root();
    let direct = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new(DATA), data())
            .bind(Exact::new("urn:test:inner"), InnerAt(inner())),
    ))
    .with_max_depth(1);
    let stamped = Request::new(Verb::Source, iri("urn:test:inner"))
        .with_arg(BUDGET, ArgRef::Inline(b"10".to_vec()));
    assert_eq!(
        block_on(direct.issue(stamped, &root)).unwrap().bytes,
        b"rows under 10"
    );

    let f = fixture(Route::Reissue);
    let kernel = f.kernel.with_max_depth(1);
    let err = block_on(kernel.issue(query(), &root)).unwrap_err();
    assert!(
        matches!(err, Error::DepthExceeded { depth: 2, .. }),
        "{err:?}"
    );
}

/// The inner endpoint bound on its own, for the depth baseline.
struct InnerAt(Arc<dyn Endpoint>);

#[async_trait]
impl Endpoint for InnerAt {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        self.0.invoke(inv).await
    }

    fn name(&self) -> &str {
        "inner"
    }
}

/// 3. The outer lookup happens before the overlay runs: after the budget changes, a
///    second identical request is served the answer computed under the old budget and
///    the overlay is not entered. Neither route can enforce what is not in the key.
#[test]
fn an_outer_cache_hit_skips_the_overlay() {
    let f = fixture(Route::Reissue);
    let root = Capability::root();
    assert_eq!(
        block_on(f.kernel.issue(query(), &root)).unwrap().bytes,
        b"rows under 10"
    );
    f.budget.store(3, Ordering::SeqCst);
    assert_eq!(
        block_on(f.kernel.issue(query(), &root)).unwrap().bytes,
        b"rows under 10",
        "served from the outer entry, under the budget it was computed with"
    );
    assert_eq!(f.runs.load(Ordering::SeqCst), 1, "the overlay ran once");
}

/// 4. The kernel's private request reborrow, as reference settlement uses it: the settled
///    request reaches the endpoint sharing the outer record, so a cut of the referent drops
///    the answer, and the endpoint still runs at the outer depth (a budget of 1 admits the
///    settlement read at depth 1 and nothing deeper is spent on the swap).
#[test]
fn the_kernels_own_request_reborrow_shares_the_record_at_no_depth() {
    let root = Capability::root();
    let reader = FnEndpoint::new("reader", |inv| Ok(text(inv.inline_str("in")?.to_string())))
        .with_description(
            Description::new("reader")
                .verb(Verb::Source)
                .input(ArgSpec::new("in")),
        );
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new(DATA), data())
            .bind(Exact::new("urn:test:reader"), reader),
    ))
    .with_max_depth(1);
    let request = Request::new(Verb::Source, iri("urn:test:reader"))
        .with_arg("in", ArgRef::Reference(iri(DATA)));
    assert_eq!(
        block_on(kernel.issue(request.clone(), &root))
            .unwrap()
            .bytes,
        b"rows"
    );
    assert!(
        kernel.is_cached(&request, &root),
        "keyed on the original request"
    );
    kernel.cut(THREAD);
    assert!(
        !kernel.is_cached(&request, &root),
        "the settlement read is a dependency"
    );
}
