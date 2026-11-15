//! **An expired composite runs again even when every part comes back unchanged**
//! (ledger #3772, found by #2753; design: `docs/design/composite-revalidation.md`).
//!
//! The baseline the design starts from, as core 0.3.5 behaves. Five transforms each read
//! one feed; each feed is cacheable for 100 ms and carries an origin validator. Past the
//! deadline, four feeds answer `Representation::revalidated` (an HTTP 304) and one
//! answers a changed body (a 200). A composite's expiry is the meet of its parts'
//! deadlines, and its entry carries no validator, so the lookup evicts it and invokes
//! the transform; the transform's own read is what revalidates the feed. All five
//! transforms run again, though only one part changed, and no thread is cut.
//!
//! The ikigai-http reproduction (`unchanged_revalidation_reuses_feed_transforms`, ignored)
//! is the same shape over real HTTP. When the design is built, the counts asserted here
//! flip from `[2, 2, 2, 2, 2]` to `[1, 1, 1, 1, 2]` and that ignored test is un-ignored.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Capability, Clock, EndpointSpace, Exact, Expiry, FnEndpoint, Invocation, Iri,
    Kernel, ListenSpec, OriginValidator, ReprType, Representation, Request, Time, Verb,
};

const FEEDS: usize = 5;
const TTL: u64 = 100;

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

/// A clock the test moves.
#[derive(Clone)]
struct TestClock(Arc<AtomicU64>);

impl Clock for TestClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

fn etag(version: &str) -> OriginValidator {
    OriginValidator {
        etag: Some(version.to_string()),
        last_modified: None,
    }
}

/// A feed in miniature: a conditional GET whose entity tag is the origin's version.
fn feed(version: Arc<Mutex<String>>, produced: Arc<AtomicUsize>) -> FnEndpoint {
    FnEndpoint::new("feed", move |inv: &Invocation<'_>| {
        let now = inv.now().expect("a clock");
        let current = version.lock().unwrap().clone();
        if inv
            .stale()
            .is_some_and(|stale| stale.validator.etag.as_deref() == Some(current.as_str()))
        {
            return Ok(Representation::revalidated(now.plus_millis(TTL)));
        }
        produced.fetch_add(1, Ordering::SeqCst);
        Ok(
            Representation::new(ReprType::new("text/plain"), format!("feed-{current}"))
                .cacheable_until(now.plus_millis(TTL))
                .validated_by(etag(&current)),
        )
    })
}

/// A transform over one feed (the XSLT lift of the HTTP case), counting its runs.
fn transform(feed: String, runs: Arc<AtomicUsize>) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("transform", move |inv: &Invocation<'_>| {
        let runs = Arc::clone(&runs);
        let feed = feed.clone();
        Box::pin(async move {
            runs.fetch_add(1, Ordering::SeqCst);
            let part = inv.source(&iri(&feed)).await?;
            let mut bytes = b"lifted:".to_vec();
            bytes.extend(part.bytes);
            Ok(Representation::new(ReprType::new("text/plain"), bytes).cacheable())
        })
    })
}

#[test]
fn unchanged_revalidation_still_reruns_every_feed_transform() {
    let clock = TestClock(Arc::new(AtomicU64::new(1_000)));
    let versions: Vec<Arc<Mutex<String>>> = (0..FEEDS)
        .map(|_| Arc::new(Mutex::new("v1".to_string())))
        .collect();
    let produced: Vec<Arc<AtomicUsize>> = (0..FEEDS).map(|_| Arc::default()).collect();
    let runs: Vec<Arc<AtomicUsize>> = (0..FEEDS).map(|_| Arc::default()).collect();
    let mut space = EndpointSpace::new();
    for i in 0..FEEDS {
        let feed_iri = format!("urn:feed:{i}");
        space = space
            .bind(
                Exact::new(feed_iri.as_str()),
                feed(Arc::clone(&versions[i]), Arc::clone(&produced[i])),
            )
            .bind(
                Exact::new(format!("urn:lift:{i}").as_str()),
                transform(feed_iri, Arc::clone(&runs[i])),
            );
    }
    let kernel = Kernel::new(Arc::new(space)).with_clock(Arc::new(clock.clone()));
    let root = Capability::root();
    let listener = kernel
        .listen(ListenSpec::new().prefix("urn:"), &root)
        .unwrap();
    let count = |cells: &[Arc<AtomicUsize>]| -> Vec<usize> {
        cells.iter().map(|c| c.load(Ordering::SeqCst)).collect()
    };
    let read_all = || {
        (0..FEEDS)
            .map(|i| block_on(kernel.issue(source(&format!("urn:lift:{i}")), &root)).unwrap())
            .collect::<Vec<_>>()
    };

    let first = read_all();
    assert_eq!(count(&runs), [1, 1, 1, 1, 1]);
    assert!(first
        .iter()
        .all(|r| r.expiry == Expiry::At(Time::from_millis(1_100))));
    // Within the deadline every transform is served from the cache.
    clock.0.store(1_050, Ordering::SeqCst);
    read_all();
    assert_eq!(count(&runs), [1, 1, 1, 1, 1]);

    // Past the deadline: four origins unchanged (a 304), one changed (a 200).
    *versions[4].lock().unwrap() = "v2".to_string();
    clock.0.store(1_150, Ordering::SeqCst);
    let second = read_all();
    assert_eq!(second[0].bytes, b"lifted:feed-v1");
    assert_eq!(second[4].bytes, b"lifted:feed-v2");
    assert_eq!(
        count(&produced),
        [1, 1, 1, 1, 2],
        "only the changed feed produced a new body"
    );
    // ★ THE BASELINE (ledger #3772): every transform ran again. The design's target is
    // `[1, 1, 1, 1, 2]`; this assertion flips when it is built.
    assert_eq!(count(&runs), [2, 2, 2, 2, 2]);
    assert!(second
        .iter()
        .all(|r| r.expiry == Expiry::At(Time::from_millis(1_250))));
    assert!(
        listener.drain().events.is_empty(),
        "no thread was cut: the reruns came from expiry alone"
    );
}
