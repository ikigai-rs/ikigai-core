//! **Negative caching** (ledger #2657): a failure cached for a STATED time, so a
//! composite that catches one failing branch caches around it and retries on schedule,
//! instead of recomputing on every read for as long as that branch fails.
//!
//! Before it, a caller that caught any failure but a miss was uncacheable at once
//! (`Recorded::record` in `endpoint.rs`): a feed stream that returned one failed feed
//! as a value recomputed the whole stream on every read. What these pin: an expiring
//! failure keeps its catcher cacheable until the expiry and then recomputes; a failure
//! with no stated expiry is never cached, as before; a cut drops a cached failure; the
//! host's ceiling clamps a stated expiry; and the readouts name a cached failure.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Capability, Clock, EndpointSpace, Error, Exact, FnEndpoint, Iri, Kernel,
    ReprType, Representation, Request, Time, TraceEvent, Tracer, Verb, DEFAULT_FAILURE_CEILING,
    FAILED_NOTE, FAILURE_CACHED_NOTE, UNCACHED_NOTE,
};

const START: u64 = 1_000;

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn text(body: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec())
}

/// A clock a test moves forward.
#[derive(Default)]
struct Moving(AtomicU64);
impl Clock for Moving {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}
impl Moving {
    fn at(millis: u64) -> Arc<Moving> {
        Arc::new(Moving(AtomicU64::new(millis)))
    }
    fn set(&self, millis: u64) {
        self.0.store(millis, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Collect(Mutex<Vec<TraceEvent>>);
impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// How a failing feed states (or does not state) its failure's expiry.
#[derive(Clone, Copy)]
enum Stated {
    Nothing,
    In(Duration),
}

/// A feed that always fails with `error`, stating `stated`, counting its calls.
fn failing(calls: Arc<AtomicUsize>, stated: Stated, error: Error) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("failing feed", move |inv| {
        let calls = Arc::clone(&calls);
        let error = error.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            if let Stated::In(ttl) = stated {
                inv.failure_expires_in(ttl);
            }
            Err(error)
        })
    })
}

/// A stream over two feeds that returns a failed feed as a VALUE, counting its calls.
fn stream(calls: Arc<AtomicUsize>) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("stream", move |inv| {
        let calls = Arc::clone(&calls);
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            let mut out = Vec::new();
            for feed in ["urn:feed:ok", "urn:feed:down"] {
                match inv.source(&iri(feed)).await {
                    Ok(r) => out.push(String::from_utf8_lossy(&r.bytes).into_owned()),
                    Err(e) => out.push(format!("failed: {e}")),
                }
            }
            Ok(text(&out.join("\n")).cacheable())
        })
    })
}

struct Fixture {
    kernel: Kernel,
    clock: Arc<Moving>,
    stream: Arc<AtomicUsize>,
    down: Arc<AtomicUsize>,
}

fn fixture_with(stated: Stated, error: Error, clock: Option<Arc<Moving>>) -> Fixture {
    let stream_calls = Arc::new(AtomicUsize::new(0));
    let down_calls = Arc::new(AtomicUsize::new(0));
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:stream"), stream(Arc::clone(&stream_calls)))
        .bind(
            Exact::new("urn:feed:ok"),
            FnEndpoint::new("ok feed", |_| Ok(text("ok").cacheable())),
        )
        .bind(
            Exact::new("urn:feed:down"),
            failing(Arc::clone(&down_calls), stated, error),
        );
    let moving = clock.clone().unwrap_or_else(|| Moving::at(START));
    let mut kernel = Kernel::new(Arc::new(space));
    if clock.is_some() {
        kernel = kernel.with_clock(moving.clone());
    }
    Fixture {
        kernel,
        clock: moving,
        stream: stream_calls,
        down: down_calls,
    }
}

fn fixture(stated: Stated) -> Fixture {
    fixture_with(
        stated,
        Error::Unavailable("origin answered 500".into()),
        Some(Moving::at(START)),
    )
}

impl Fixture {
    fn read(&self) -> String {
        let r = block_on(self.kernel.issue(source("urn:stream"), &Capability::root())).unwrap();
        String::from_utf8(r.bytes).unwrap()
    }

    fn calls(&self) -> (usize, usize) {
        (
            self.stream.load(Ordering::SeqCst),
            self.down.load(Ordering::SeqCst),
        )
    }

    fn readout(&self, target: &str) -> String {
        let r = block_on(self.kernel.issue(source(target), &Capability::root())).unwrap();
        String::from_utf8(r.bytes).unwrap()
    }
}

#[test]
fn a_composite_catching_an_expiring_failure_is_cached_until_the_expiry_then_recomputes() {
    let f = fixture(Stated::In(Duration::from_secs(60)));
    let first = f.read();
    assert!(first.contains("failed: unavailable"), "{first}");
    assert_eq!(f.calls(), (1, 1));

    // Within the minute: the stream is served from the cache, and nothing recomputes.
    f.clock.set(START + 59_999);
    assert_eq!(f.read(), first);
    assert_eq!(
        f.calls(),
        (1, 1),
        "the stream cached around the failing feed"
    );

    // Past it: the stream recomputes and the feed is asked again, on schedule.
    f.clock.set(START + 60_000);
    f.read();
    assert_eq!(f.calls(), (2, 2), "the expiry passed: retried");
}

#[test]
fn a_failure_with_no_stated_expiry_is_never_cached() {
    let f = fixture(Stated::Nothing);
    f.read();
    f.read();
    assert_eq!(f.calls(), (2, 2), "every read recomputes, as before");

    let collect = Arc::new(Collect::default());
    block_on(
        f.kernel
            .issue_traced(source("urn:stream"), &Capability::root(), collect.clone()),
    )
    .unwrap();
    let events = collect.0.lock().unwrap();
    let event = events.iter().find(|e| e.target == "urn:stream").unwrap();
    let why = event.notes.iter().find(|(k, _)| k == UNCACHED_NOTE);
    assert_eq!(
        why.map(|(_, v)| v.as_str()),
        Some("failed urn:feed:down"),
        "{:?}",
        event.notes
    );
    assert!(!f.readout("urn:kernel:cache").contains("failures"));
}

#[test]
fn a_cut_on_the_failing_resource_drops_the_cached_failure() {
    let f = fixture(Stated::In(Duration::from_secs(60)));
    f.read();
    f.read();
    assert_eq!(f.calls(), (1, 1));

    f.kernel.cut("urn:feed:down");
    f.read();
    assert_eq!(
        f.calls(),
        (2, 2),
        "the cut dropped the failure and the stream"
    );

    // And the failure the recompute cached is served again.
    f.read();
    assert_eq!(f.calls(), (2, 2));
}

#[test]
fn the_ceiling_clamps_a_stated_expiry() {
    let day = Stated::In(Duration::from_secs(24 * 60 * 60));

    // The default: five minutes.
    let f = fixture(day);
    f.read();
    let ceiling = DEFAULT_FAILURE_CEILING.as_millis() as u64;
    assert_eq!(ceiling, 300_000);
    f.clock.set(START + ceiling - 1);
    f.read();
    assert_eq!(f.calls(), (1, 1));
    f.clock.set(START + ceiling);
    f.read();
    assert_eq!(
        f.calls(),
        (2, 2),
        "a day was clamped to the default ceiling"
    );

    // A host's own ceiling.
    let mut f = fixture(day);
    f.kernel = f.kernel.with_failure_ceiling(Duration::from_secs(10));
    f.read();
    f.clock.set(START + 9_999);
    f.read();
    assert_eq!(f.calls(), (1, 1));
    f.clock.set(START + 10_000);
    f.read();
    assert_eq!(
        f.calls(),
        (2, 2),
        "a day was clamped to the host's ten seconds"
    );

    // A ceiling of zero caches no failure at all.
    let mut f = fixture(day);
    f.kernel = f.kernel.with_failure_ceiling(Duration::ZERO);
    f.read();
    f.read();
    assert_eq!(f.calls(), (2, 2));
}

#[test]
fn the_failing_resource_itself_is_served_from_the_cache_and_says_so() {
    let f = fixture(Stated::In(Duration::from_secs(60)));
    let feed = || block_on(f.kernel.issue(source("urn:feed:down"), &Capability::root()));
    assert!(matches!(feed(), Err(Error::Unavailable(_))));
    let collect = Arc::new(Collect::default());
    let again = block_on(f.kernel.issue_traced(
        source("urn:feed:down"),
        &Capability::root(),
        collect.clone(),
    ));
    assert_eq!(again, Err(Error::Unavailable("origin answered 500".into())));
    assert_eq!(f.down.load(Ordering::SeqCst), 1);
    let events = collect.0.lock().unwrap();
    let event = &events[0];
    assert!(event.cache_hit);
    assert!(event
        .notes
        .contains(&(FAILED_NOTE.to_string(), "unavailable".to_string())));
    assert!(event.notes.contains(&(
        FAILURE_CACHED_NOTE.to_string(),
        format!("until {}", START + 60_000)
    )));
}

#[test]
fn the_readouts_name_a_cached_failure_and_its_expiry() {
    let f = fixture(Stated::In(Duration::from_secs(60)));
    f.read();
    let line = format!("until {}  failed unavailable", START + 60_000);
    for readout in ["urn:kernel:cache", "urn:kernel:uncached"] {
        let body = f.readout(readout);
        assert!(
            body.contains("failures (cached until a stated expiry)"),
            "{readout}:\n{body}"
        );
        let row = body
            .lines()
            .find(|l| l.contains("urn:feed:down"))
            .unwrap_or_else(|| panic!("{readout}:\n{body}"));
        assert!(row.contains("live"), "{row}");
        assert!(row.contains(&line), "{row}");
    }
    f.kernel.cut("urn:feed:down");
    let body = f.readout("urn:kernel:cache");
    let row = body.lines().find(|l| l.contains("urn:feed:down")).unwrap();
    assert!(
        row.contains("cut"),
        "a cut failure is marked, not hidden: {row}"
    );
}

#[test]
fn a_denial_a_miss_a_depth_refusal_or_a_clock_free_kernel_caches_no_failure() {
    for error in [
        Error::Denied("no".into()),
        Error::NotFound("gone".into()),
        Error::DepthExceeded {
            depth: 3,
            target: iri("urn:deep"),
        },
    ] {
        let f = fixture_with(
            Stated::In(Duration::from_secs(60)),
            error.clone(),
            Some(Moving::at(START)),
        );
        let feed = || block_on(f.kernel.issue(source("urn:feed:down"), &Capability::root()));
        assert!(feed().is_err());
        assert!(feed().is_err());
        assert_eq!(f.down.load(Ordering::SeqCst), 2, "{error:?} was cached");
    }

    let f = fixture_with(
        Stated::In(Duration::from_secs(60)),
        Error::Unavailable("down".into()),
        None,
    );
    f.read();
    f.read();
    assert_eq!(f.calls(), (2, 2), "no clock: nothing could tell it expired");
}

#[test]
fn a_failure_that_read_something_volatile_is_not_cached() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let space = EndpointSpace::new()
        .bind(
            Exact::new("urn:volatile"),
            FnEndpoint::new("volatile", |_| Ok(text("now"))),
        )
        .bind(
            Exact::new("urn:feed:down"),
            AsyncFnEndpoint::new("reads then fails", move |inv| {
                let counted = Arc::clone(&counted);
                Box::pin(async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    inv.source(&iri("urn:volatile")).await?;
                    inv.failure_expires_in(Duration::from_secs(60));
                    Err(Error::Unavailable("down".into()))
                })
            }),
        );
    let kernel = Kernel::new(Arc::new(space)).with_clock(Moving::at(START));
    for _ in 0..2 {
        assert!(block_on(kernel.issue(source("urn:feed:down"), &Capability::root())).is_err());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn an_answer_supersedes_a_cached_failure_under_the_same_key() {
    // The failure is cached; a cut drops it; the resource recovers and its answer is
    // cached in its place — the readout lists no failure for it any more.
    let recovered = Arc::new(AtomicUsize::new(0));
    let flag = Arc::clone(&recovered);
    let space = EndpointSpace::new().bind(
        Exact::new("urn:feed:flaky"),
        AsyncFnEndpoint::new("flaky", move |inv| {
            let flag = Arc::clone(&flag);
            Box::pin(async move {
                if flag.load(Ordering::SeqCst) == 0 {
                    inv.failure_expires_in(Duration::from_secs(60));
                    Err(Error::Timeout("slow".into()))
                } else {
                    Ok(text("back").cacheable())
                }
            })
        }),
    );
    let kernel = Kernel::new(Arc::new(space)).with_clock(Moving::at(START));
    let read = || block_on(kernel.issue(source("urn:feed:flaky"), &Capability::root()));
    assert!(read().is_err());
    recovered.store(1, Ordering::SeqCst);
    assert!(read().is_err(), "still the cached failure");
    kernel.cut("urn:feed:flaky");
    assert_eq!(read().unwrap().bytes, b"back");
    let cache = String::from_utf8(
        block_on(kernel.issue(source("urn:kernel:cache"), &Capability::root()))
            .unwrap()
            .bytes,
    )
    .unwrap();
    assert!(!cache.contains("failures"), "{cache}");
}
