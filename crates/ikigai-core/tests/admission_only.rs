//! **Depth and the run context are admission inputs only; the clock reaches an answer's
//! content only through an expiry or a named instant** (ledger #3869).
//!
//! None of depth, the run context (the memory and retry pools, the deadline, the time
//! left) or a temporal corridor's clock is in the cache key, which is the request after
//! every rewrite, the capability fingerprint and the chain fingerprint. So the kernel
//! guarantees each one can change only WHETHER an answer is produced, or how long it is
//! served, and never what a later request is served:
//!
//! 1. A refusal the run context decided (`OverBudget`, `Timeout`, `GaveUp`) is never
//!    cached, even when its endpoint stated an expiry, and neither is an endpoint's own
//!    `Unresolved` (that one is pinned in `negative_caching.rs`).
//! 2. A deadline an endpoint states after a pinned FUTURE instant is served at most its
//!    window on the kernel's clock, a success's and a failure's alike.
//! 3. A temporal corridor's identity is its name and its instant: two instants under one
//!    name never share a cache entry.
//!
//! And two properties of cuts across scopes, pinned (section 4): a cut is global, and a
//! scope-dependent canonical name is cut only where it is the canonical.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::executor::block_on;
use ikigai_core::{
    Alias, AliasTable, ArgRef, AsyncFnEndpoint, Capability, Clock, EndpointSpace, Error, Exact,
    Expiry, FixedClock, FnEndpoint, Iri, IssueContext, Kernel, ReprType, Representation, Request,
    Scope, Space, Time, Verb,
};

const START: u64 = 1_000;
/// The instant a pinned-future corridor pins: far ahead of the kernel's clock.
const FUTURE: u64 = 1_000_000;
const MINUTE: u64 = 60_000;

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(bytes: &[u8]) -> Representation {
    Representation::new(ReprType::new("text/plain"), bytes.to_vec())
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn sink(target: &str, content: &[u8]) -> Request {
    Request::new(Verb::Sink, iri(target)).with_arg("content", ArgRef::Inline(content.to_vec()))
}

/// The kernel's clock, moved by the test.
#[derive(Clone)]
struct Moving(Arc<AtomicU64>);
impl Moving {
    fn at(millis: u64) -> Self {
        Moving(Arc::new(AtomicU64::new(millis)))
    }
    fn set(&self, millis: u64) {
        self.0.store(millis, Ordering::SeqCst);
    }
}
impl Clock for Moving {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

fn runs(counter: &Arc<AtomicU32>) -> u32 {
    counter.load(Ordering::SeqCst)
}

// 1 ─────────────────────────────────────────────────────────────────────────────

/// A refusal the run context decided, stated with an expiry, is computed again on the
/// next read. Before ledger #3869 each of these was cached for its stated minute.
#[test]
fn a_run_context_refusal_is_never_cached_even_with_a_stated_expiry() {
    for error in [
        Error::OverBudget {
            detail: "past the run's 4096-byte pool".into(),
        },
        Error::Timeout("the run's deadline passed".into()),
        Error::GaveUp {
            attempts: 3,
            elapsed: Duration::from_secs(2),
            detail: "unavailable: down".into(),
        },
    ] {
        let calls = Arc::new(AtomicU32::new(0));
        let counted = Arc::clone(&calls);
        let refused = error.clone();
        let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(
            Exact::new("urn:job"),
            AsyncFnEndpoint::new("job", move |inv| {
                let counted = Arc::clone(&counted);
                let refused = refused.clone();
                Box::pin(async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    inv.failure_expires_in(Duration::from_secs(60));
                    Err(refused)
                })
            }),
        )))
        .with_clock(Arc::new(Moving::at(START)));
        for _ in 0..2 {
            assert!(block_on(kernel.issue(source("urn:job"), &Capability::root())).is_err());
        }
        assert_eq!(runs(&calls), 2, "{} was cached", error.kind());
    }
}

/// The hazard rule 1 closes, end to end: a run with a short deadline is refused with a
/// stated expiry; a later run with a long deadline must be computed, not served the
/// first run's refusal (the deadline is not in the key).
#[test]
fn a_later_run_with_a_longer_deadline_is_not_served_an_earlier_runs_timeout() {
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:slow"),
        AsyncFnEndpoint::new("needs a second", move |inv| {
            let counted = Arc::clone(&counted);
            Box::pin(async move {
                counted.fetch_add(1, Ordering::SeqCst);
                match inv.time_left() {
                    Some(left) if left < Duration::from_secs(1) => {
                        inv.failure_expires_in(Duration::from_secs(60));
                        Err(Error::Timeout("less than a second left".into()))
                    }
                    _ => Ok(text(b"done").cacheable()),
                }
            })
        }),
    )))
    .with_clock(Arc::new(Moving::at(START)));
    let run = |deadline: u64| {
        block_on(kernel.issue_with_context(
            source("urn:slow"),
            &Capability::root(),
            IssueContext::new().with_not_after(Time::from_millis(deadline)),
        ))
    };
    assert!(matches!(run(START + 100), Err(Error::Timeout(_))));
    assert_eq!(run(START + 10_000).unwrap().bytes, b"done");
    assert_eq!(runs(&calls), 2);
}

// 2 ─────────────────────────────────────────────────────────────────────────────

/// A corridor named by its instant that pins `at`, over no doors of its own.
fn pinned(at: u64) -> Scope {
    Scope::empty().with_named_at(
        iri(&format!("urn:ctx:time:{at}")),
        Arc::new(EndpointSpace::new()),
        Arc::new(FixedClock::at(at)),
    )
}

/// An endpoint fresh for one minute from the time IT sees.
fn fresh_for_a_minute(calls: &Arc<AtomicU32>) -> FnEndpoint {
    let calls = Arc::clone(calls);
    FnEndpoint::new("a minute", move |inv| {
        calls.fetch_add(1, Ordering::SeqCst);
        let now = inv.now().expect("a clock");
        Ok(text(b"v").cacheable_until(now.plus_millis(MINUTE)))
    })
}

/// Under a pinned future, `now + 60 s` is the pinned instant plus a minute, which the
/// kernel's clock reaches only after `FUTURE`: before ledger #3869 the entry was served
/// for ~1000 s instead of 60. Now it lives its minute on the kernel's clock.
#[test]
fn a_success_stated_under_a_pinned_future_is_served_only_its_window() {
    let calls = Arc::new(AtomicU32::new(0));
    let clock = Moving::at(START);
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:minute"), fresh_for_a_minute(&calls)),
    ))
    .with_clock(Arc::new(clock.clone()));
    let cap = Capability::root();
    let read = || block_on(kernel.issue_in(source("urn:minute"), &cap, pinned(FUTURE))).unwrap();

    assert_eq!(
        read().expiry,
        Expiry::At(Time::from_millis(START + MINUTE)),
        "the window, on the kernel's clock"
    );
    clock.set(START + MINUTE - 1);
    read();
    assert_eq!(runs(&calls), 1, "within the minute: served");
    clock.set(START + MINUTE);
    read();
    assert_eq!(runs(&calls), 2, "after the minute: computed again");
}

/// The clamp only ever shortens, and only under a pinned future: with no corridor the
/// stated deadline stands, and a fixed deadline before the pinned instant is read as
/// stated (`temporal_corridor::validity_is_judged_on_the_kernels_clock_never_the_chains`
/// pins that one end to end).
#[test]
fn the_clamp_leaves_a_live_read_and_a_pinned_past_as_stated() {
    let calls = Arc::new(AtomicU32::new(0));
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:minute"), fresh_for_a_minute(&calls)),
    ))
    .with_clock(Arc::new(Moving::at(START + 500)));
    let cap = Capability::root();
    let live = block_on(kernel.issue(source("urn:minute"), &cap)).unwrap();
    assert_eq!(
        live.expiry,
        Expiry::At(Time::from_millis(START + 500 + MINUTE))
    );
    // A pinned past states a deadline the kernel's clock may already have passed; it is
    // not extended to a minute from the kernel's now.
    let past = block_on(kernel.issue_in(source("urn:minute"), &cap, pinned(START))).unwrap();
    assert_eq!(past.expiry, Expiry::At(Time::from_millis(START + MINUTE)));
}

/// A failure's stated expiry takes the same clamp: before ledger #3869 only the ceiling
/// (five minutes) bounded it under a pinned future.
#[test]
fn a_failure_stated_under_a_pinned_future_is_served_only_its_window() {
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let clock = Moving::at(START);
    let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:down"),
        AsyncFnEndpoint::new("down", move |inv| {
            let counted = Arc::clone(&counted);
            Box::pin(async move {
                counted.fetch_add(1, Ordering::SeqCst);
                inv.failure_expires_in(Duration::from_millis(MINUTE));
                Err(Error::Unavailable("down for a minute".into()))
            })
        }),
    )))
    .with_clock(Arc::new(clock.clone()));
    let cap = Capability::root();
    let read = || block_on(kernel.issue_in(source("urn:down"), &cap, pinned(FUTURE)));
    assert!(read().is_err());
    clock.set(START + MINUTE - 1);
    assert!(read().is_err());
    assert_eq!(runs(&calls), 1, "within the minute: the cached failure");
    clock.set(START + MINUTE);
    assert!(read().is_err());
    assert_eq!(runs(&calls), 2, "after the minute: computed again");
}

// 3 ─────────────────────────────────────────────────────────────────────────────

/// One corridor NAME injected at two instants: the second must not be served the
/// first's answer. Before ledger #3869 the fingerprint hashed the name alone, and the
/// read at `LATER` answered `EARLIER`.
#[test]
fn two_instants_under_one_name_never_share_an_entry() {
    const EARLIER: u64 = 5_000;
    const LATER: u64 = 9_000;
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:stamp"),
        FnEndpoint::new("stamp", move |inv| {
            counted.fetch_add(1, Ordering::SeqCst);
            let now = inv.now().expect("a clock").as_millis();
            Ok(text(now.to_string().as_bytes()).cacheable())
        }),
    )))
    .with_clock(Arc::new(Moving::at(START)));
    let cap = Capability::root();
    let misnamed = |at: u64| {
        Scope::empty().with_named_at(
            iri("urn:ctx:time:misnamed"),
            Arc::new(EndpointSpace::new()),
            Arc::new(FixedClock::at(at)),
        )
    };
    let read = |at: u64| {
        block_on(kernel.issue_in(source("urn:stamp"), &cap, misnamed(at)))
            .unwrap()
            .bytes
    };
    assert_eq!(read(EARLIER), b"5000");
    assert_eq!(read(LATER), b"9000");
    assert_eq!(read(EARLIER), b"5000");
    assert_eq!(
        runs(&calls),
        2,
        "each instant cached once, under its own identity"
    );
}

// 4 ─────────────────────────────────────────────────────────────────────────────

/// A cell: `Source` answers its value (cacheable, counting runs), `Sink` sets it.
fn cell(state: Arc<Mutex<Vec<u8>>>, calls: Arc<AtomicU32>) -> Arc<FnEndpoint> {
    Arc::new(FnEndpoint::new("cell", move |inv| match inv.request.verb {
        Verb::Sink => {
            *state.lock().unwrap() = inv.inline_arg("content")?.to_vec();
            Ok(text(b"written"))
        }
        _ => {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(text(&state.lock().unwrap()).cacheable())
        }
    }))
}

/// A named corridor with no doors: a scope of its own, and a fingerprint of its own.
fn named(name: &str) -> Scope {
    Scope::empty().with_named(iri(name), Arc::new(EndpointSpace::new()))
}

/// Pin: a cut is GLOBAL across scopes. A write in scope Y to a name drops scope X's
/// cached read of the same canonical name, though X and Y are different cache keys.
#[test]
fn a_write_in_one_scope_drops_another_scopes_cached_read_of_the_same_name() {
    const CELL: &str = "urn:example:cell";
    let state = Arc::new(Mutex::new(b"old".to_vec()));
    let calls = Arc::new(AtomicU32::new(0));
    let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind_arc(
        Exact::new(CELL),
        cell(Arc::clone(&state), Arc::clone(&calls)),
    )));
    let cap = Capability::root();
    let in_x = || block_on(kernel.issue_in(source(CELL), &cap, named("urn:ctx:x"))).unwrap();

    assert_eq!(in_x().bytes, b"old");
    assert_eq!(in_x().bytes, b"old");
    assert_eq!(runs(&calls), 1, "cached in X");
    block_on(kernel.issue_in(sink(CELL, b"new"), &cap, named("urn:ctx:y"))).unwrap();
    assert_eq!(in_x().bytes, b"new", "the write in Y cut X's entry");
    assert_eq!(runs(&calls), 2);
}

/// Pin: a scope-DEPENDENT canonical. One logical name maps to `m1` in scope X and to `m2`
/// in scope Y. A write to `m2` (directly, or through the logical name in Y) leaves X's
/// cached read, which hangs from `m1`; a write to `m1` drops it.
#[test]
fn a_write_to_another_scopes_canonical_leaves_this_scopes_entry() {
    const LOGICAL: &str = "urn:example:logical";
    const M1: &str = "urn:example:m1";
    const M2: &str = "urn:example:m2";
    let (one, two) = (
        Arc::new(Mutex::new(b"one".to_vec())),
        Arc::new(Mutex::new(b"two".to_vec())),
    );
    let (ones, twos) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
    let m1 = cell(Arc::clone(&one), Arc::clone(&ones));
    let m2 = cell(Arc::clone(&two), Arc::clone(&twos));
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind_arc(Exact::new(M1), m1.clone())
            .bind_arc(Exact::new(M2), m2.clone()),
    ));
    // Each scope's corridor rewrites the logical name to its own backing name, served
    // in the corridor, and reports that canonical.
    let mapping = |to: &str, backing: Arc<FnEndpoint>| -> Arc<dyn Space> {
        Arc::new(Alias::new(
            Arc::new(AliasTable::new().exact(LOGICAL, to)),
            Arc::new(EndpointSpace::new().bind_arc(Exact::new(to), backing)),
        ))
    };
    let x = || Scope::empty().with_named(iri("urn:ctx:x"), mapping(M1, m1.clone()));
    let y = || Scope::empty().with_named(iri("urn:ctx:y"), mapping(M2, m2.clone()));
    let cap = Capability::root();
    let in_x = || block_on(kernel.issue_in(source(LOGICAL), &cap, x())).unwrap();

    assert_eq!(in_x().bytes, b"one");
    assert_eq!(
        block_on(kernel.issue_in(source(LOGICAL), &cap, y()))
            .unwrap()
            .bytes,
        b"two"
    );
    assert_eq!(runs(&ones), 1);

    // A write to m2, through the logical name in Y and then directly: X's entry stays.
    block_on(kernel.issue_in(sink(LOGICAL, b"two'"), &cap, y())).unwrap();
    block_on(kernel.issue(sink(M2, b"two''"), &cap)).unwrap();
    assert_eq!(in_x().bytes, b"one");
    assert_eq!(runs(&ones), 1, "a write to Y's canonical leaves X's entry");

    // A write to m1 drops it.
    block_on(kernel.issue(sink(M1, b"one'"), &cap)).unwrap();
    assert_eq!(in_x().bytes, b"one'");
    assert_eq!(runs(&ones), 2);
}
