//! **An entry that expired by time is revalidated through its endpoint** (ledger #2534,
//! step A of HTTP revalidation, #2495).
//!
//! Before this, `ReprCache::serve` evicted an entry the moment a lookup found it
//! expired, so its body and anything that could validate it were gone before the next
//! read could ask the origin "has it changed?". Now an answer can carry its origin's
//! validator (`Representation::validated_by`); when its deadline passes and every
//! golden thread it hangs from still stands, the kernel keeps it and invokes the
//! endpoint with the stale answer offered (`Invocation::stale`). An endpoint that
//! answers `Representation::revalidated(T)` has the stored body served under the new
//! deadline, without producing it again, and the step is traced as revalidated.
//!
//! These pin: the offer and the refresh; a cut entry never offered; a new answer
//! replacing the entry; the lost-cut rule declining a refresh; an entry without a
//! validator evicted as before; a composite over a revalidated read; and a
//! `revalidated` answer nobody asked for refused.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Capability, Clock, EndpointSpace, Error, Exact, Expiry, FnEndpoint,
    Invocation, Iri, Kernel, OriginValidator, ReprType, Representation, Request, Time, TraceEvent,
    TraceState, Tracer, Verb, REVALIDATED_NOTE, UNCACHED_NOTE,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn etag(tag: &str) -> OriginValidator {
    OriginValidator {
        etag: Some(tag.to_string()),
        last_modified: None,
    }
}

/// A clock the test moves.
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

#[derive(Default)]
struct Recorder(Mutex<Vec<TraceEvent>>);

impl Tracer for Recorder {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Recorder {
    fn only(&self, target: &str) -> TraceEvent {
        let events = self.0.lock().unwrap();
        let matching: Vec<&TraceEvent> = events.iter().filter(|e| e.target == target).collect();
        assert_eq!(matching.len(), 1, "one event for {target}: {events:?}");
        matching[0].clone()
    }
}

fn note<'e>(event: &'e TraceEvent, key: &str) -> Option<&'e str> {
    event
        .notes
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// An origin the test controls: its current version, and how many times each thing
/// happened to it.
#[derive(Default)]
struct Origin {
    /// The version the origin holds now; its body is `body-{version}`.
    version: Mutex<String>,
    /// How many times an endpoint produced a body from it.
    produced: AtomicUsize,
    /// What each invocation was offered, in order (`None` for no offer).
    offers: Mutex<Vec<Option<(OriginValidator, ReprType)>>>,
}

impl Origin {
    fn at(version: &str) -> Arc<Self> {
        let origin = Origin::default();
        *origin.version.lock().unwrap() = version.to_string();
        Arc::new(origin)
    }
    fn set(&self, version: &str) {
        *self.version.lock().unwrap() = version.to_string();
    }
    fn produced(&self) -> usize {
        self.produced.load(Ordering::SeqCst)
    }
    fn offers(&self) -> Vec<Option<(OriginValidator, ReprType)>> {
        self.offers.lock().unwrap().clone()
    }
}

/// A conditional GET in miniature: offered a stale answer whose entity tag is the
/// origin's current version, it answers "still valid for another `ttl`"; otherwise it
/// produces the body, valid for `ttl`, tagged with the version. `validated` is whether
/// it attaches a validator at all.
fn origin_endpoint(origin: Arc<Origin>, ttl: u64, validated: bool) -> FnEndpoint {
    FnEndpoint::new("origin", move |inv: &Invocation<'_>| {
        origin_answer(&origin, inv, ttl, validated)
    })
}

fn origin_answer(
    origin: &Origin,
    inv: &Invocation<'_>,
    ttl: u64,
    validated: bool,
) -> ikigai_core::Result<Representation> {
    let now = inv.now().expect("a clock");
    let version = origin.version.lock().unwrap().clone();
    origin.offers.lock().unwrap().push(
        inv.stale()
            .map(|stale| (stale.validator.clone(), stale.repr_type.clone())),
    );
    if let Some(stale) = inv.stale() {
        if stale.validator.etag.as_deref() == Some(version.as_str()) {
            return Ok(Representation::revalidated(now.plus_millis(ttl)));
        }
    }
    origin.produced.fetch_add(1, Ordering::SeqCst);
    let answer = Representation::new(
        ReprType::new("text/plain"),
        format!("body-{version}").into_bytes(),
    )
    .cacheable_until(now.plus_millis(ttl))
    .depends_on("urn:origin:state");
    Ok(if validated {
        answer.validated_by(etag(&version))
    } else {
        answer
    })
}

fn kernel_over(origin: &Arc<Origin>, validated: bool, clock: &TestClock) -> Kernel {
    Kernel::new(Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:origin:doc"),
        origin_endpoint(Arc::clone(origin), 100, validated),
    )))
    .with_clock(Arc::new(clock.clone()))
}

#[test]
fn a_time_expired_entry_with_a_validator_is_offered_and_still_valid_serves_the_stored_body() {
    let clock = TestClock::at(1_000);
    let origin = Origin::at("v1");
    let kernel = kernel_over(&origin, true, &clock);
    let root = Capability::root();

    let first = block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
    assert_eq!(first.bytes, b"body-v1");
    assert_eq!(first.origin_validator(), Some(&etag("v1")));
    assert_eq!(origin.produced(), 1);

    // Past the deadline: the entry is offered, not evicted, and "still valid" keeps it.
    clock.set(1_150);
    let recorder = Arc::new(Recorder::default());
    let second =
        block_on(kernel.issue_traced(source("urn:origin:doc"), &root, recorder.clone())).unwrap();
    assert_eq!(second.bytes, b"body-v1", "the stored body is served");
    assert_eq!(
        second.expiry,
        Expiry::At(Time::from_millis(1_250)),
        "under the new deadline"
    );
    assert_eq!(
        origin.produced(),
        1,
        "the endpoint's bytes were not produced again"
    );
    assert_eq!(
        origin.offers(),
        vec![None, Some((etag("v1"), ReprType::new("text/plain")))],
        "the second invocation was offered the stored validator and type"
    );
    let event = recorder.only("urn:origin:doc");
    assert_eq!(event.state(), TraceState::Revalidated, "{event:?}");
    assert!(!event.cache_hit);
    assert_eq!(note(&event, REVALIDATED_NOTE), Some("1250"));
    assert_eq!(note(&event, UNCACHED_NOTE), None, "the refresh was stored");

    // Within the new deadline: served from the cache, the endpoint not entered.
    clock.set(1_200);
    let recorder = Arc::new(Recorder::default());
    let third =
        block_on(kernel.issue_traced(source("urn:origin:doc"), &root, recorder.clone())).unwrap();
    assert_eq!(third.bytes, b"body-v1");
    assert_eq!(recorder.only("urn:origin:doc").state(), TraceState::Served);
    assert_eq!(origin.offers().len(), 2, "not invoked");
    assert_eq!(kernel.cache_len(), 1);
}

#[test]
fn a_cut_entry_is_never_offered() {
    let clock = TestClock::at(1_000);
    let origin = Origin::at("v1");
    let kernel = kernel_over(&origin, true, &clock);
    let root = Capability::root();

    block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
    // Expired by time AND cut: a cut says the content changed, so nothing is offered.
    clock.set(1_150);
    kernel.cut("urn:origin:state");
    let recorder = Arc::new(Recorder::default());
    let read =
        block_on(kernel.issue_traced(source("urn:origin:doc"), &root, recorder.clone())).unwrap();
    assert_eq!(read.bytes, b"body-v1");
    assert_eq!(
        origin.offers(),
        vec![None, None],
        "the cut entry was not offered"
    );
    assert_eq!(origin.produced(), 2, "recomputed");
    assert_eq!(
        recorder.only("urn:origin:doc").state(),
        TraceState::Computed
    );

    // Cut while still within its deadline: evicted and recomputed, as before.
    kernel.cut("urn:origin:state");
    block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
    assert_eq!(origin.offers(), vec![None, None, None]);
    assert_eq!(origin.produced(), 3);
}

#[test]
fn a_new_answer_to_an_offer_replaces_the_entry() {
    let clock = TestClock::at(1_000);
    let origin = Origin::at("v1");
    let kernel = kernel_over(&origin, true, &clock);
    let root = Capability::root();

    block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
    // The origin moved on: the offer is answered with a new body (a 200, not a 304).
    origin.set("v2");
    clock.set(1_150);
    let recorder = Arc::new(Recorder::default());
    let read =
        block_on(kernel.issue_traced(source("urn:origin:doc"), &root, recorder.clone())).unwrap();
    assert_eq!(read.bytes, b"body-v2");
    assert_eq!(read.expiry, Expiry::At(Time::from_millis(1_250)));
    assert_eq!(origin.produced(), 2);
    assert_eq!(
        recorder.only("urn:origin:doc").state(),
        TraceState::Computed
    );
    assert_eq!(
        kernel.cache_len(),
        1,
        "one entry: the new answer replaced the old"
    );

    // Served within the new deadline; once that passes, offered under the NEW validator.
    clock.set(1_200);
    assert_eq!(
        block_on(kernel.issue(source("urn:origin:doc"), &root))
            .unwrap()
            .bytes,
        b"body-v2"
    );
    assert_eq!(origin.offers().len(), 2, "served, not invoked");
    clock.set(1_300);
    let again = block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
    assert_eq!(again.bytes, b"body-v2");
    assert_eq!(
        origin.offers().last().cloned().flatten(),
        Some((etag("v2"), ReprType::new("text/plain")))
    );
    assert_eq!(origin.produced(), 2, "revalidated under v2");
}

#[test]
fn a_cut_during_the_revalidating_call_declines_the_refresh() {
    let clock = TestClock::at(1_000);
    let origin = Origin::at("v1");
    // The endpoint cuts the thread its entry hangs from while it revalidates: the
    // write a concurrent Sink would make, landing between the offer and the refresh.
    let kernel_slot: Arc<OnceLock<Weak<Kernel>>> = Arc::new(OnceLock::new());
    let inner = Arc::new(origin_endpoint(Arc::clone(&origin), 100, true));
    let racing = {
        let kernel_slot = Arc::clone(&kernel_slot);
        AsyncFnEndpoint::new("racing", move |inv: &Invocation<'_>| {
            let kernel_slot = Arc::clone(&kernel_slot);
            let inner = Arc::clone(&inner);
            Box::pin(async move {
                if inv.stale().is_some() {
                    let kernel = kernel_slot.get().and_then(Weak::upgrade).unwrap();
                    kernel.cut("urn:origin:state");
                }
                ikigai_core::Endpoint::invoke(inner.as_ref(), inv).await
            })
        })
    };
    let kernel = Arc::new(
        Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:origin:doc"), racing),
        ))
        .with_clock(Arc::new(clock.clone())),
    );
    kernel_slot.set(Arc::downgrade(&kernel)).unwrap();
    let root = Capability::root();

    block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
    clock.set(1_150);
    let recorder = Arc::new(Recorder::default());
    let read =
        block_on(kernel.issue_traced(source("urn:origin:doc"), &root, recorder.clone())).unwrap();
    // The endpoint said "still valid", so this caller gets the stored body, as a
    // computed answer raced by a cut is still handed back; but the refresh is declined.
    assert_eq!(read.bytes, b"body-v1");
    assert_eq!(origin.produced(), 1);
    let event = recorder.only("urn:origin:doc");
    assert_eq!(event.state(), TraceState::Revalidated);
    assert_eq!(note(&event, UNCACHED_NOTE), Some("cut-in-flight"));
    assert_eq!(
        kernel.cache_len(),
        0,
        "nothing refreshed, the stale entry gone"
    );

    // So the next read, even inside the deadline the endpoint gave, recomputes with
    // nothing offered.
    clock.set(1_160);
    block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
    assert_eq!(origin.offers().last().cloned(), Some(None));
    assert_eq!(origin.produced(), 2);
}

#[test]
fn an_entry_without_a_validator_is_evicted_as_before() {
    let clock = TestClock::at(1_000);
    let origin = Origin::at("v1");
    let kernel = kernel_over(&origin, false, &clock);
    let root = Capability::root();

    let first = block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
    assert_eq!(first.origin_validator(), None);
    clock.set(1_150);
    // The public probe sees it expired; the read evicts and recomputes, offering nothing.
    let recorder = Arc::new(Recorder::default());
    block_on(kernel.issue_traced(source("urn:origin:doc"), &root, recorder.clone())).unwrap();
    assert_eq!(origin.offers(), vec![None, None]);
    assert_eq!(origin.produced(), 2);
    assert_eq!(
        recorder.only("urn:origin:doc").state(),
        TraceState::Computed
    );
}

#[test]
fn a_cacheable_composite_over_a_revalidated_answer_recomputes_and_finds_it_revalidated() {
    let clock = TestClock::at(1_000);
    let origin = Origin::at("v1");
    let composed = Arc::new(AtomicUsize::new(0));
    let page = {
        let composed = Arc::clone(&composed);
        AsyncFnEndpoint::new("page", move |inv: &Invocation<'_>| {
            let composed = Arc::clone(&composed);
            Box::pin(async move {
                composed.fetch_add(1, Ordering::SeqCst);
                let inner = inv.source(&iri("urn:origin:doc")).await?;
                let mut bytes = b"page:".to_vec();
                bytes.extend(inner.bytes);
                // Permanently cacheable on its own: its deadline is its part's.
                Ok(Representation::new(ReprType::new("text/plain"), bytes).cacheable())
            })
        })
    };
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(
                Exact::new("urn:origin:doc"),
                origin_endpoint(Arc::clone(&origin), 100, true),
            )
            .bind(Exact::new("urn:page"), page),
    ))
    .with_clock(Arc::new(clock.clone()));
    let root = Capability::root();

    let first = block_on(kernel.issue(source("urn:page"), &root)).unwrap();
    assert_eq!(first.bytes, b"page:body-v1");
    assert_eq!(first.expiry, Expiry::At(Time::from_millis(1_100)));

    // Both expired. The page has no validator, so it is evicted and recomputed; its
    // inner read is offered and revalidated.
    clock.set(1_150);
    let recorder = Arc::new(Recorder::default());
    let second =
        block_on(kernel.issue_traced(source("urn:page"), &root, recorder.clone())).unwrap();
    assert_eq!(second.bytes, b"page:body-v1");
    assert_eq!(
        second.expiry,
        Expiry::At(Time::from_millis(1_250)),
        "the composite takes its part's new deadline"
    );
    assert_eq!(
        composed.load(Ordering::SeqCst),
        2,
        "the composite recomputed"
    );
    assert_eq!(origin.produced(), 1, "its part was not produced again");
    assert_eq!(recorder.only("urn:page").state(), TraceState::Computed);
    let inner = recorder.only("urn:origin:doc");
    assert_eq!(inner.state(), TraceState::Revalidated);
    assert_eq!(inner.parent, Some(recorder.only("urn:page").span));

    // And the composite is cached again under that deadline.
    clock.set(1_200);
    block_on(kernel.issue(source("urn:page"), &root)).unwrap();
    assert_eq!(composed.load(Ordering::SeqCst), 2);
}

#[test]
fn a_revalidated_answer_nobody_offered_anything_is_refused() {
    let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:liar"),
        FnEndpoint::new("liar", |_| {
            Ok(Representation::revalidated(Time::from_millis(5_000)))
        }),
    )))
    .with_clock(Arc::new(TestClock::at(1_000)));
    let err = block_on(kernel.issue(source("urn:liar"), &Capability::root())).unwrap_err();
    assert!(
        matches!(err, Error::Endpoint(ref message) if message.contains("revalidated")),
        "{err:?}"
    );
    assert_eq!(kernel.cache_len(), 0);
}

#[test]
fn the_public_cache_face_keeps_a_validated_entry_it_cannot_serve() {
    use ikigai_core::{CacheKey, EntryState, ReprCache};

    let cache = ReprCache::default();
    let key = CacheKey::new(source("urn:origin:doc").id(), 0);
    let stored = Representation::new(ReprType::new("text/plain"), b"x".to_vec())
        .cacheable_until(Time::from_millis(100))
        .validated_by(etag("v1"));
    assert!(cache.store(key, "urn:origin:doc".into(), stored, cache.snapshot(), None));
    let later = Some(Time::from_millis(200));
    assert!(cache.get(&key, later).is_none(), "never served as it is");
    assert_eq!(cache.len(), 1, "but kept for revalidation");
    assert_eq!(cache.rows_at(later)[0].state, EntryState::Expired);
    // A cut makes it an ordinary stale entry: the next lookup evicts it.
    cache.cut("urn:origin:doc".into());
    let threaded = Representation::new(ReprType::new("text/plain"), b"y".to_vec())
        .cacheable_until(Time::from_millis(100))
        .depends_on("urn:origin:doc")
        .validated_by(etag("v1"));
    assert!(cache.store(
        key,
        "urn:origin:doc".into(),
        threaded,
        cache.snapshot(),
        None
    ));
    cache.cut("urn:origin:doc".into());
    assert!(cache.get(&key, later).is_none());
    assert_eq!(cache.len(), 0, "a cut entry is evicted");
}

#[test]
fn a_still_valid_answer_with_a_new_validator_replaces_the_stored_one() {
    let clock = TestClock::at(1_000);
    let offered = Arc::new(Mutex::new(Vec::new()));
    let doc = {
        let offered = Arc::clone(&offered);
        FnEndpoint::new("doc", move |inv: &Invocation<'_>| {
            let now = inv.now().expect("a clock");
            offered
                .lock()
                .unwrap()
                .push(inv.stale().map(|stale| stale.validator.clone()));
            Ok(match inv.stale() {
                // A 304 that names the version afresh (a weak tag, say).
                Some(_) => {
                    Representation::revalidated(now.plus_millis(100)).validated_by(etag("W/\"v1\""))
                }
                None => Representation::new(ReprType::new("text/plain"), b"body".to_vec())
                    .cacheable_until(now.plus_millis(100))
                    .validated_by(etag("\"v1\"")),
            })
        })
    };
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:origin:doc"), doc),
    ))
    .with_clock(Arc::new(clock.clone()));
    let root = Capability::root();
    block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
    clock.set(1_150);
    let read = block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
    assert_eq!(read.bytes, b"body");
    assert_eq!(read.origin_validator(), Some(&etag("W/\"v1\"")));
    clock.set(1_300);
    block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
    assert_eq!(
        offered.lock().unwrap().clone(),
        vec![None, Some(etag("\"v1\"")), Some(etag("W/\"v1\""))],
        "the next offer carries the validator the refresh gave"
    );
}

// Ledger #2553: zero freshness still needs a stored body for conditional reads.
#[test]
fn an_immediately_expired_validated_answer_is_stored_and_revalidated_on_every_read() {
    for deadline in [999, 1_000] {
        let clock = TestClock::at(1_000);
        let origin = Origin::at("v1");
        let endpoint_origin = Arc::clone(&origin);
        let doc = FnEndpoint::new("past", move |inv: &Invocation<'_>| {
            // Pin both the original answer and every refresh at or before now.
            let answer = origin_answer(&endpoint_origin, inv, 0, true)?;
            Ok(answer.with_expiry(Expiry::At(Time::from_millis(deadline))))
        });
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:origin:doc"), doc),
        ))
        .with_clock(Arc::new(clock));
        let root = Capability::root();
        for read in 0..3 {
            let recorder = Arc::new(Recorder::default());
            let answer =
                block_on(kernel.issue_traced(source("urn:origin:doc"), &root, recorder.clone()))
                    .unwrap();
            assert_eq!(answer.bytes, b"body-v1");
            assert_eq!(answer.origin_validator(), Some(&etag("v1")));
            assert_eq!(answer.expiry, Expiry::At(Time::from_millis(deadline)));
            assert_eq!(kernel.cache_len(), 1, "stored even with no freshness");
            assert_eq!(origin.produced(), 1, "refresh produces no body bytes");
            assert_eq!(origin.offers().len(), read + 1, "endpoint asked every time");
            let event = recorder.only("urn:origin:doc");
            assert!(!event.cache_hit, "never served as fresh");
            assert_eq!(note(&event, UNCACHED_NOTE), None, "stored for revalidation");
            if read > 0 {
                assert_eq!(event.state(), TraceState::Revalidated);
                assert_eq!(
                    origin.offers()[read],
                    Some((etag("v1"), ReprType::new("text/plain")))
                );
            }
        }
        let uncached = block_on(kernel.issue(source("urn:kernel:uncached"), &root)).unwrap();
        assert!(!String::from_utf8(uncached.bytes)
            .unwrap()
            .contains("urn:origin:doc"));
    }
}

#[test]
fn an_immediately_expired_answer_without_a_validator_is_declined() {
    let origin = Origin::at("v1");
    let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:origin:doc"),
        origin_endpoint(Arc::clone(&origin), 0, false),
    )))
    .with_clock(Arc::new(TestClock::at(1_000)));
    for _ in 0..2 {
        let recorder = Arc::new(Recorder::default());
        block_on(kernel.issue_traced(
            source("urn:origin:doc"),
            &Capability::root(),
            recorder.clone(),
        ))
        .unwrap();
        assert_eq!(kernel.cache_len(), 0);
        assert_eq!(
            note(&recorder.only("urn:origin:doc"), UNCACHED_NOTE),
            Some("expired 1000")
        );
    }
    assert_eq!(origin.offers(), vec![None, None]);
    assert_eq!(origin.produced(), 2);
}

#[test]
fn a_zero_freshness_refresh_of_a_previously_fresh_entry_keeps_the_body() {
    let clock = TestClock::at(1_000);
    let origin = Origin::at("v1");
    let endpoint_origin = Arc::clone(&origin);
    let doc = FnEndpoint::new("zero-refresh", move |inv: &Invocation<'_>| {
        let ttl = if inv.stale().is_some() { 0 } else { 100 };
        origin_answer(&endpoint_origin, inv, ttl, true)
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:origin:doc"), doc),
    ))
    .with_clock(Arc::new(clock.clone()));
    let root = Capability::root();
    block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
    clock.set(1_100);
    for _ in 0..2 {
        let answer = block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
        assert_eq!(answer.bytes, b"body-v1");
        assert_eq!(answer.origin_validator(), Some(&etag("v1")));
        assert_eq!(answer.expiry, Expiry::At(Time::from_millis(1_100)));
        assert_eq!(kernel.cache_len(), 1, "zero freshness refresh retained");
        assert_eq!(origin.produced(), 1);
    }
    assert_eq!(origin.offers().len(), 3, "each refresh asked the endpoint");
}

#[test]
fn immediately_stale_entries_obey_admission_and_eviction() {
    use ikigai_core::{CacheBound, Lru};

    // Too small to admit the body, then enough bytes but only one entry.
    for (byte_bound, admitted) in [(1, false), (1_024, true)] {
        let origin = Origin::at("v1");
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new()
                .bind(
                    Exact::new("urn:origin:doc"),
                    origin_endpoint(Arc::clone(&origin), 0, true),
                )
                .bind(
                    Exact::new("urn:other"),
                    FnEndpoint::new("other", |_| {
                        Ok(Representation::new(ReprType::new("text/plain"), vec![0]).cacheable())
                    }),
                ),
        ))
        .with_clock(Arc::new(TestClock::at(1_000)))
        .with_cache_policy(Arc::new(Lru::with_bound(CacheBound::new(1, byte_bound))));
        let root = Capability::root();
        block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
        assert_eq!(kernel.cache_len(), usize::from(admitted));
        // Eviction removes the body entirely; the next read must fetch in full.
        block_on(kernel.issue(source("urn:other"), &root)).unwrap();
        assert_eq!(kernel.cache_len(), 1);
        block_on(kernel.issue(source("urn:origin:doc"), &root)).unwrap();
        assert_eq!(origin.offers(), vec![None, None]);
        assert_eq!(origin.produced(), 2);
        assert_eq!(kernel.cache_len(), 1);
    }
}

#[test]
fn a_validator_does_not_make_a_deadline_storable_without_a_clock() {
    let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:origin:doc"),
        FnEndpoint::new("clockless", |_| {
            Ok(
                Representation::new(ReprType::new("text/plain"), b"body".to_vec())
                    .cacheable_until(Time::from_millis(0))
                    .validated_by(etag("v1")),
            )
        }),
    )));
    let recorder = Arc::new(Recorder::default());
    block_on(kernel.issue_traced(
        source("urn:origin:doc"),
        &Capability::root(),
        recorder.clone(),
    ))
    .unwrap();
    assert_eq!(kernel.cache_len(), 0);
    assert_eq!(
        note(&recorder.only("urn:origin:doc"), UNCACHED_NOTE),
        Some("no-clock")
    );
}
