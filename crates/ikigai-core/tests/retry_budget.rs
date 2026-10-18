//! **The retry budget and the run deadline ride authority** (ledger #1546, #200; the
//! retry-authority note, section 4, decisions 5, 7 and 8).
//!
//! One pool per resolution tree, minted from the capability at the top, inherited by every
//! sub-request and narrowed at each to what its capability is entitled to; a deadline a door
//! sets, inherited and only shortened. And with nothing configured, nothing changes.

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    cap_retry, AsyncFnEndpoint, BoxFuture, Capability, EndpointSpace, Exact, FixedClock,
    FnEndpoint, Iri, IssueContext, Kernel, ReprType, Representation, Request, Spawner, Time, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn text(body: String) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.into_bytes())
}

/// What a leaf saw: the pool's count before and after it spent one, and its deadline.
#[derive(Clone, Debug, Default, PartialEq)]
struct Seen {
    before: Option<u32>,
    spent: Option<bool>,
    not_after: Option<u64>,
}

/// A leaf that records what it saw and spends one retry, like an overlay that re-issued.
fn spending_leaf(log: Arc<Mutex<Vec<Seen>>>) -> FnEndpoint {
    FnEndpoint::new("leaf", move |inv| {
        let pool = inv.retry_budget();
        let seen = Seen {
            before: pool.map(|p| p.remaining()),
            spent: pool.map(|p| p.try_spend()),
            not_after: inv.not_after().map(Time::as_millis),
        };
        log.lock().unwrap().push(seen);
        Ok(text("leaf".into()))
    })
}

/// A composite that reads the leaf twice, the second time under `second`: `None` for its
/// own capability, `Some(scopes)` for an attenuated one.
fn twice(second: Option<Vec<String>>) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("twice", move |inv| {
        let second = second.clone();
        Box::pin(async move {
            inv.source(&iri("urn:leaf")).await?;
            match second {
                None => inv.source(&iri("urn:leaf")).await?,
                Some(scopes) => inv.issue_attenuated(source("urn:leaf"), scopes).await?,
            };
            let left = inv.retry_budget().map(|p| p.remaining());
            Ok(text(format!("{left:?}")))
        })
    })
}

fn kernel_with(second: Option<Vec<String>>, log: Arc<Mutex<Vec<Seen>>>) -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:leaf"), spending_leaf(log))
            .bind(Exact::new("urn:twice"), twice(second)),
    ))
}

fn seen(log: &Arc<Mutex<Vec<Seen>>>) -> Vec<Seen> {
    log.lock().unwrap().clone()
}

/// Decision 5: with no retry budget configured, no invocation sees a pool, at the top or
/// below it, and a door's limit is dropped.
#[test]
fn nothing_configured_is_todays_behavior() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let kernel = kernel_with(None, log.clone());
    let answer = block_on(kernel.issue_with_context(
        source("urn:twice"),
        &Capability::root(),
        IssueContext::new().with_retry_limit(5),
    ))
    .unwrap();
    assert_eq!(answer.bytes, b"None");
    assert!(
        seen(&log).iter().all(|s| s.before.is_none()),
        "{:?}",
        seen(&log)
    );
}

/// One pool for the whole tree: the leaf's spends are the composite's, so two nested
/// layers of "retry once" cost one budget, not two.
#[test]
fn the_tree_shares_one_pool() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let kernel = kernel_with(None, log.clone()).with_retry_budget(0, 3);
    let answer = block_on(kernel.issue(source("urn:twice"), &Capability::root())).unwrap();
    assert_eq!(answer.bytes, b"Some(1)", "3 minted, two spent below");
    assert_eq!(
        seen(&log)
            .iter()
            .map(|s| (s.before, s.spent))
            .collect::<Vec<_>>(),
        [(Some(3), Some(true)), (Some(2), Some(true))]
    );
}

/// Decision 8: the ceiling rides the capability. A caller holding `urn:cap:retry:1` gets one
/// retry for its whole tree; the second spend finds the pool empty and takes nothing.
#[test]
fn the_capability_sets_the_ceiling() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let kernel = kernel_with(None, log.clone()).with_retry_budget(0, 3);
    let one = Capability::scoped([cap_retry(1)]);
    let answer = block_on(kernel.issue(source("urn:twice"), &one)).unwrap();
    assert_eq!(answer.bytes, b"Some(0)");
    assert_eq!(
        seen(&log)
            .iter()
            .map(|s| (s.before, s.spent))
            .collect::<Vec<_>>(),
        [(Some(1), Some(true)), (Some(0), Some(false))]
    );
}

/// "Parent-owned operations are never retried in the leaf": a composite that hands a
/// subtree a capability WITHOUT the grant hands it the base (0 here), and the parent's pool
/// is untouched by that subtree.
#[test]
fn attenuating_away_the_grant_hands_the_subtree_only_the_base() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let kernel =
        kernel_with(Some(vec!["urn:cap:read".into()]), log.clone()).with_retry_budget(0, 3);
    let caller = Capability::scoped([cap_retry(3), "urn:cap:read".to_string()]);
    let answer = block_on(kernel.issue(source("urn:twice"), &caller)).unwrap();
    assert_eq!(
        answer.bytes, b"Some(2)",
        "only the first, granted, read spent"
    );
    let seen = seen(&log);
    assert_eq!(seen[0].before, Some(3));
    assert_eq!((seen[1].before, seen[1].spent), (Some(0), Some(false)));

    // Naming the grant hands it down: the subtree shares the parent's pool.
    let log = Arc::new(Mutex::new(Vec::new()));
    let kernel = kernel_with(Some(vec!["urn:cap:read".into(), cap_retry(3)]), log.clone())
        .with_retry_budget(0, 3);
    let answer = block_on(kernel.issue(source("urn:twice"), &caller)).unwrap();
    assert_eq!(answer.bytes, b"Some(1)");
}

/// A door's limit narrows what the capability is entitled to; it never raises it.
#[test]
fn a_door_limit_only_narrows() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let kernel = kernel_with(None, log.clone()).with_retry_budget(0, 3);
    let narrowed = IssueContext::new().with_retry_limit(1);
    block_on(kernel.issue_with_context(source("urn:leaf"), &Capability::root(), narrowed)).unwrap();
    let raised = IssueContext::new().with_retry_limit(10);
    let one = Capability::scoped([cap_retry(1)]);
    block_on(kernel.issue_with_context(source("urn:leaf"), &one, raised)).unwrap();
    assert_eq!(
        seen(&log).iter().map(|s| s.before).collect::<Vec<_>>(),
        [Some(1), Some(1)]
    );
}

/// Decision 7: the door's deadline reaches every sub-request, and an endpoint can shorten
/// it for what it issues, never extend it.
#[test]
fn the_deadline_is_inherited_and_only_shortened() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let shortening = AsyncFnEndpoint::new("shortening", |inv| {
        Box::pin(async move {
            inv.source(&iri("urn:leaf")).await?;
            inv.with_not_after(Time::from_millis(1_200))
                .source(&iri("urn:leaf"))
                .await?;
            inv.with_not_after(Time::from_millis(99_000))
                .source(&iri("urn:leaf"))
                .await
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:leaf"), spending_leaf(log.clone()))
            .bind(Exact::new("urn:shortening"), shortening),
    ))
    .with_clock(Arc::new(FixedClock::at(1_000)));
    let run = IssueContext::new().with_not_after(Time::from_millis(1_500));
    block_on(kernel.issue_with_context(source("urn:shortening"), &Capability::root(), run))
        .unwrap();
    assert_eq!(
        seen(&log).iter().map(|s| s.not_after).collect::<Vec<_>>(),
        [Some(1_500), Some(1_200), Some(1_500)]
    );
    // A plain issue has no deadline at all.
    log.lock().unwrap().clear();
    block_on(kernel.issue(source("urn:leaf"), &Capability::root())).unwrap();
    assert_eq!(seen(&log)[0].not_after, None);
}

struct InlineSpawner;
impl Spawner for InlineSpawner {
    fn spawn(&self, task: BoxFuture<()>) -> BoxFuture<()> {
        task
    }
}

/// Across a fan-out spawn the branches draw from the same pool and see the same deadline.
#[test]
fn fan_out_branches_share_the_pool_and_the_deadline() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let fanner = AsyncFnEndpoint::new("fanner", |inv| {
        Box::pin(async move {
            for result in inv
                .fan_out(vec![
                    source("urn:leaf"),
                    source("urn:leaf"),
                    source("urn:leaf"),
                ])
                .await
            {
                result?;
            }
            Ok(text("ok".into()))
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:leaf"), spending_leaf(log.clone()))
            .bind(Exact::new("urn:fan"), fanner),
    ))
    .with_retry_budget(0, 2)
    .into_scheduled(Arc::new(InlineSpawner));
    let run = IssueContext::new().with_not_after(Time::from_millis(7));
    block_on(kernel.issue_with_context(source("urn:fan"), &Capability::root(), run)).unwrap();
    let seen = seen(&log);
    assert_eq!(
        seen.iter().map(|s| s.spent).collect::<Vec<_>>(),
        [Some(true), Some(true), Some(false)],
        "two retries for three branches: {seen:?}"
    );
    assert!(seen.iter().all(|s| s.not_after == Some(7)));
}
