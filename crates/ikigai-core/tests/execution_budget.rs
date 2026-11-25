//! Monotone execution budgets (ledger #4322): what the kernel does today, the claims
//! `docs/design/execution-budget.md` starts from. Nothing here is a proposed API: every test
//! uses only what is public now, and pins current behavior.
//!
//! 1. A door fact carried as a capability MARKER is dropped by attenuation, while the run
//!    context ([`IssueContext`]) the host issued under reaches the same attenuated
//!    sub-request. This is the downstream case the note replaces.
//! 2. An endpoint can narrow the run context for what it issues and cannot widen it.
//! 3. The run context is not part of the cache key: a hit is served under a tighter
//!    context without running the endpoint, and spends nothing from the run's pool.
//! 4. With no context, the budgets differ in what they do: no deadline, no retry pool
//!    unless the host configured one, and a byte pool from the host's default policy.
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Capability, EndpointSpace, Exact, FixedClock, FnEndpoint, Iri, IssueContext,
    Kernel, ReprType, Representation, Request, Time, Verb,
};

/// A door marker in the style of a downstream host's (a scope no grant names).
const MARKER: &str = "urn:test:door:anonymous-budget:1000";
const READ: &str = "urn:cap:read";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn text(body: String) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.into_bytes())
}

/// What a leaf saw when it ran.
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    marker: bool,
    not_after: Option<u64>,
    retries: Option<u32>,
    memory: bool,
}

fn leaf(log: Arc<Mutex<Vec<Seen>>>, cacheable: bool) -> FnEndpoint {
    FnEndpoint::new("leaf", move |inv| {
        log.lock().unwrap().push(Seen {
            marker: inv.capability.allows(MARKER),
            not_after: inv.not_after().map(Time::as_millis),
            retries: inv.retry_budget().map(|p| p.remaining()),
            memory: inv.memory_budget().is_some(),
        });
        if let Some(pool) = inv.retry_budget() {
            pool.try_spend();
        }
        let answer = text("leaf".into());
        Ok(if cacheable {
            answer.cacheable()
        } else {
            answer
        })
    })
}

fn seen(log: &Arc<Mutex<Vec<Seen>>>) -> Vec<Seen> {
    log.lock().unwrap().clone()
}

/// The downstream case: a script runner derives its sub-request's capability by attenuation
/// to what the script needs, so a door marker the caller held does not reach the store. The
/// deadline the host issued the run under does.
#[test]
fn attenuation_drops_a_door_marker_and_keeps_the_run_context() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let runner = AsyncFnEndpoint::new("runner", |inv| {
        Box::pin(async move {
            inv.source(&iri("urn:leaf")).await?;
            inv.issue_attenuated(source("urn:leaf"), [READ]).await
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:leaf"), leaf(log.clone(), false))
            .bind(Exact::new("urn:runner"), runner),
    ))
    .with_clock(Arc::new(FixedClock::at(1_000)));
    let door = Capability::scoped([READ, MARKER]);
    let run = IssueContext::new().with_not_after(Time::from_millis(2_000));
    block_on(kernel.issue_with_context(source("urn:runner"), &door, run)).unwrap();
    let seen = seen(&log);
    assert_eq!(
        seen.iter()
            .map(|s| (s.marker, s.not_after))
            .collect::<Vec<_>>(),
        [(true, Some(2_000)), (false, Some(2_000))],
        "the marker dies at attenuation; the run's deadline does not: {seen:?}"
    );
}

/// An endpoint narrows the run for what it issues, and asking for more changes nothing.
#[test]
fn an_endpoint_narrows_the_run_and_cannot_widen_it() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let narrowing = AsyncFnEndpoint::new("narrowing", |inv| {
        Box::pin(async move {
            inv.with_retries(1).source(&iri("urn:leaf")).await?;
            inv.with_retries(50)
                .with_not_after(Time::from_millis(99_000))
                .source(&iri("urn:leaf"))
                .await
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:leaf"), leaf(log.clone(), false))
            .bind(Exact::new("urn:narrowing"), narrowing),
    ))
    .with_retry_budget(0, 3)
    .with_clock(Arc::new(FixedClock::at(1_000)));
    let run = IssueContext::new().with_not_after(Time::from_millis(1_500));
    block_on(kernel.issue_with_context(source("urn:narrowing"), &Capability::root(), run)).unwrap();
    assert_eq!(
        seen(&log)
            .iter()
            .map(|s| (s.retries, s.not_after))
            .collect::<Vec<_>>(),
        // The first read saw a child pool of 1 and spent it from the run's 3 as well; the
        // second asked for 50 retries and a later deadline and got what was left: 2, 1500.
        [(Some(1), Some(1_500)), (Some(2), Some(1_500))]
    );
}

/// The cache key does not carry the run context: a second run under a TIGHTER deadline is
/// served the first run's answer, and the endpoint that would have seen the tighter deadline
/// does not run. Within one run, a hit spends nothing from the run's pool: it observes.
#[test]
fn a_cache_hit_is_served_whatever_the_run_context_and_spends_nothing() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let twice = AsyncFnEndpoint::new("twice", |inv| {
        Box::pin(async move {
            inv.source(&iri("urn:leaf")).await?;
            inv.source(&iri("urn:leaf")).await?;
            let left = inv.retry_budget().map(|p| p.remaining());
            Ok(text(format!("{left:?}")))
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:leaf"), leaf(log.clone(), true))
            .bind(Exact::new("urn:twice"), twice),
    ))
    .with_retry_budget(0, 3)
    .with_clock(Arc::new(FixedClock::at(1_000)));
    let root = Capability::root();

    // Within one run: the leaf runs once and spends one; the second read is a hit.
    let answer = block_on(kernel.issue(source("urn:twice"), &root)).unwrap();
    assert_eq!(
        answer.bytes, b"Some(2)",
        "3 minted, only the run that ran spent"
    );
    assert_eq!(seen(&log).len(), 1);

    // Across runs: a tighter deadline is not a different key.
    let wide = IssueContext::new().with_not_after(Time::from_millis(9_000));
    let tight = IssueContext::new().with_not_after(Time::from_millis(1_001));
    block_on(kernel.issue_with_context(source("urn:leaf"), &root, wide)).unwrap();
    block_on(kernel.issue_with_context(source("urn:leaf"), &root, tight)).unwrap();
    assert_eq!(seen(&log).len(), 1, "both served from the cache");
    assert!(
        seen(&log).iter().all(|s| s.not_after != Some(1_001)),
        "no endpoint ran under the tight deadline: {:?}",
        seen(&log)
    );
}

/// With no context, each budget has its own missing policy: no deadline and no retry pool
/// (open), while the byte pool is minted from the host's default policy (always bounded).
#[test]
fn a_missing_context_is_open_for_time_and_retries_and_bounded_for_bytes() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:leaf"), leaf(log.clone(), false)),
    ));
    block_on(kernel.issue(source("urn:leaf"), &Capability::root())).unwrap();
    assert_eq!(
        seen(&log),
        [Seen {
            marker: true,
            not_after: None,
            retries: None,
            memory: true,
        }],
        "root allows every scope, the marker included"
    );
}
