//! Door context (ledger #4321): what the run context does today, the evidence for the design
//! in `docs/design/door-context.md`. Nothing here changes the kernel; each test pins one
//! claim the note builds on, with the public API alone.
//!
//! 1. A door fact carried in the run context (its deadline, the one door fact core carries
//!    today) survives an attenuated sub-request, a confinement and a scheduled fan-out, while
//!    a door fact carried as a capability scope is dropped by the same attenuation. This is
//!    why the note extends the run context rather than the capability.
//! 2. An issuer that does not override `issue_in_context` (a module bridge, a wire without a
//!    field for it) drops the run context: the far side sees no door fact at all. Authority
//!    is untouched, so this is fail-closed for authority and fail-open for the fact.
//! 3. A cacheable answer that read a run-context fact is served, unchanged, to a second run
//!    whose context differs: the cache key does not include the run context, and reading it
//!    marks nothing. Observational context silently influences a cached result today.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Bindings, BoxFuture, Capability, Endpoint, EndpointSpace, Exact, FnEndpoint,
    Invocation, Iri, IssueContext, Issuer, Kernel, ReprType, Representation, Request, Result,
    Spawner, Time, Verb,
};

/// A scope a door might mint to mark where a request entered, the shape the note's
/// downstream workaround started from.
const DOOR_MARKER: &str = "urn:cap:door:page";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn text(body: String) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.into_bytes())
}

/// What a leaf saw of the door: the run's deadline and whether its capability still holds
/// the door marker.
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    not_after: Option<u64>,
    marker: bool,
}

fn observing_leaf(log: Arc<Mutex<Vec<Seen>>>) -> FnEndpoint {
    FnEndpoint::new("leaf", move |inv| {
        log.lock().unwrap().push(Seen {
            not_after: inv.not_after().map(Time::as_millis),
            marker: inv.capability.allows(DOOR_MARKER),
        });
        Ok(text("leaf".into()))
    })
}

struct InlineSpawner;
impl Spawner for InlineSpawner {
    fn spawn(&self, task: BoxFuture<()>) -> BoxFuture<()> {
        task
    }
}

#[test]
fn a_run_context_fact_survives_attenuation_confinement_and_fan_out_and_a_scope_marker_does_not() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let confined_leaf: Arc<dyn Endpoint> = Arc::new(observing_leaf(log.clone()));
    let composite = AsyncFnEndpoint::new("composite", move |inv| {
        let corridor =
            Arc::new(EndpointSpace::new().bind_arc(Exact::new("urn:leaf"), confined_leaf.clone()));
        Box::pin(async move {
            // The plain sub-request: both the deadline and the marker reach the leaf.
            inv.source(&iri("urn:leaf")).await?;
            // Attenuated to what the leaf needs: the marker was not asked for, so it goes.
            inv.issue_attenuated(source("urn:leaf"), ["urn:cap:read"])
                .await?;
            // Confined to a corridor: the chain narrows, the run is carried.
            inv.confine(iri("urn:corridor"), corridor)
                .source(&iri("urn:leaf"))
                .await?;
            // Scheduled fan-out: each spawned branch carries the run.
            for result in inv
                .fan_out(vec![source("urn:leaf"), source("urn:leaf")])
                .await
            {
                result?;
            }
            Ok(text("ok".into()))
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:leaf"), observing_leaf(log.clone()))
            .bind(Exact::new("urn:composite"), composite),
    ))
    .into_scheduled(Arc::new(InlineSpawner));
    let door = Capability::root().attenuate(["urn:cap:read", DOOR_MARKER]);
    let run = IssueContext::new().with_not_after(Time::from_millis(42));
    block_on(kernel.issue_with_context(source("urn:composite"), &door, run)).unwrap();

    let seen = log.lock().unwrap().clone();
    assert_eq!(
        seen.len(),
        5,
        "plain, attenuated, confined and two branches: {seen:?}"
    );
    assert!(
        seen.iter().all(|s| s.not_after == Some(42)),
        "the run context reaches every leaf: {seen:?}"
    );
    assert_eq!(
        seen.iter().map(|s| s.marker).collect::<Vec<_>>(),
        [true, false, true, true, true],
        "only the attenuated sub-request loses the scope marker: {seen:?}"
    );
}

/// An issuer written before `issue_in_context` existed: it forwards the request and the
/// capability and nothing else, as a module host bridge or an older wire does.
struct Bridge(Arc<Kernel>);

#[async_trait]
impl Issuer for Bridge {
    async fn issue(&self, request: Request, capability: &Capability) -> Result<Representation> {
        self.0.issue(request, capability).await
    }
}

#[test]
fn an_issuer_without_the_context_seam_drops_the_door_fact_and_keeps_the_authority() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let kernel = Arc::new(Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:leaf"), observing_leaf(log.clone())),
    )));
    let request = source("urn:caller");
    let bindings = Bindings::new();
    let door = Capability::root().attenuate(["urn:cap:read", DOOR_MARKER]);

    // In process, through the kernel's own seam: the deadline crosses.
    let direct = Invocation::with_issuer(&request, &bindings, &door, kernel.as_ref());
    block_on(
        direct
            .with_not_after(Time::from_millis(42))
            .source(&iri("urn:leaf")),
    )
    .unwrap();
    // Through a bridge that only forwards request and capability: it does not.
    let bridge = Bridge(kernel.clone());
    let bridged = Invocation::with_issuer(&request, &bindings, &door, &bridge);
    block_on(
        bridged
            .with_not_after(Time::from_millis(42))
            .source(&iri("urn:leaf")),
    )
    .unwrap();

    let seen = log.lock().unwrap().clone();
    assert_eq!(
        seen,
        [
            Seen {
                not_after: Some(42),
                marker: true
            },
            Seen {
                not_after: None,
                marker: true
            },
        ],
        "the bridge drops the run context silently and carries the capability unchanged"
    );
}

#[test]
fn a_cached_answer_that_read_the_run_context_is_served_to_a_run_with_another_context() {
    let runs = Arc::new(AtomicUsize::new(0));
    let counted = runs.clone();
    // A cacheable answer that reports the door fact it saw: an observation written into
    // the result, the case the note says must not happen silently.
    let reporter = FnEndpoint::new("reporter", move |inv| {
        counted.fetch_add(1, Ordering::SeqCst);
        let seen = inv
            .not_after()
            .map(|t| t.as_millis().to_string())
            .unwrap_or_default();
        Ok(text(seen).cacheable())
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:reporter"), reporter),
    ));
    let cap = Capability::root();
    let first = IssueContext::new().with_not_after(Time::from_millis(100));
    let second = IssueContext::new().with_not_after(Time::from_millis(200));

    let a = block_on(kernel.issue_with_context(source("urn:reporter"), &cap, first)).unwrap();
    let b = block_on(kernel.issue_with_context(source("urn:reporter"), &cap, second)).unwrap();

    assert_eq!(a.bytes, b"100");
    assert_eq!(
        b.bytes, b"100",
        "the second run is served the first run's observation: the key has no run context"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the endpoint ran once");
}
