//! **A streamed body** (ledger #2237): what stage 1 of the binary streams design promises,
//! pinned end to end through the kernel. A one-shot body is handed over and never stored,
//! and the uncached note says why; a body that can be read again is stored and served as
//! bytes are; the bytes API collects under a bound and refuses past it; a stream that
//! fails part-way stores nothing; a cut invalidates lookups, not open readers; an
//! open-ended body ends at its run's deadline; and every endpoint written before bodies
//! existed answers a body request through the default, unchanged.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::executor::block_on;
use futures::StreamExt;
use ikigai_core::{
    ArgRef, AsyncFnEndpoint, Body, ByteStream, Bytes, Capability, Clock, Endpoint, EndpointSpace,
    Error, Exact, FnEndpoint, Invocation, Iri, IssueContext, Kernel, Replay, ReprType,
    Representation, Request, Result, Time, TraceEvent, Tracer, Verb, COLLECT_LIMIT, UNCACHED_NOTE,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn text() -> ReprType {
    ReprType::new("text/plain")
}

/// An endpoint whose answer is whatever body `make` builds, declared cacheable when
/// `cacheable` says so. Its `invoke` collects, as the trait asks of a streaming endpoint.
struct Streamer<F> {
    make: F,
    cacheable: bool,
    runs: Arc<AtomicUsize>,
}

fn streamer<F>(cacheable: bool, make: F) -> (Streamer<F>, Arc<AtomicUsize>)
where
    F: Fn() -> Body + Send + Sync,
{
    let runs = Arc::new(AtomicUsize::new(0));
    let endpoint = Streamer {
        make,
        cacheable,
        runs: Arc::clone(&runs),
    };
    (endpoint, runs)
}

#[async_trait]
impl<F> Endpoint for Streamer<F>
where
    F: Fn() -> Body + Send + Sync,
{
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        self.invoke_body(inv).await?.collect(COLLECT_LIMIT).await
    }

    async fn invoke_body(&self, _inv: &Invocation<'_>) -> Result<Representation<Body>> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let answer = Representation::from_body(text(), (self.make)());
        Ok(if self.cacheable {
            answer.cacheable()
        } else {
            answer
        })
    }
}

/// A one-shot body of `chunks`, each an `Ok` chunk.
fn once(chunks: &'static [&'static [u8]]) -> Body {
    Body::once(futures::stream::iter(
        chunks.iter().map(|c| Ok(Bytes::from_static(c))),
    ))
}

/// A body that can be read again, counting how often it is opened.
struct Counted {
    bytes: &'static [u8],
    opened: Arc<AtomicUsize>,
}

impl Replay for Counted {
    fn open(&self) -> ByteStream {
        self.opened.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::iter([Ok(Bytes::from_static(self.bytes))]))
    }

    fn len_hint(&self) -> Option<u64> {
        Some(self.bytes.len() as u64)
    }
}

#[derive(Default)]
struct Collect(Mutex<Vec<TraceEvent>>);
impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn uncached_note(kernel: &Kernel, target: &str) -> Option<String> {
    let collect = Arc::new(Collect::default());
    block_on(kernel.issue_traced(source(target), &Capability::root(), collect.clone())).unwrap();
    let events = collect.0.lock().unwrap();
    let event = events.iter().rev().find(|e| e.target == target).unwrap();
    event
        .notes
        .iter()
        .find(|(k, _)| k == UNCACHED_NOTE)
        .map(|(_, v)| v.clone())
}

#[test]
fn a_one_shot_body_is_handed_over_and_never_stored_and_the_note_says_why() {
    // Declared cacheable: the expiry says the bytes are TRUE forever, and the body says
    // they cannot be READ AGAIN. The second axis wins for storage.
    let (live, runs) = streamer(true, || once(&[b"a", b"b"]));
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:live"), live),
    ));
    let root = Capability::root();

    let body = block_on(kernel.issue_body(source("urn:live"), &root)).unwrap();
    assert!(body.bytes.is_one_shot());
    assert_eq!(block_on(body.bytes.collect(16)).unwrap(), &b"ab"[..]);
    assert!(!kernel.is_cached(&source("urn:live"), &root));
    assert_eq!(kernel.cache_len(), 0);

    // The bytes API collects it, and still stores nothing: every read runs the producer.
    assert_eq!(
        block_on(kernel.issue(source("urn:live"), &root))
            .unwrap()
            .bytes,
        b"ab"
    );
    assert_eq!(
        block_on(kernel.issue(source("urn:live"), &root))
            .unwrap()
            .bytes,
        b"ab"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 3);

    // The note names the cause, on the trace and in the readout.
    assert_eq!(
        uncached_note(&kernel, "urn:live").as_deref(),
        Some("one-shot body")
    );
    let readout = block_on(kernel.issue(source("urn:kernel:uncached"), &root)).unwrap();
    let readout = String::from_utf8(readout.bytes).unwrap();
    assert!(
        readout
            .lines()
            .any(|l| l.contains("urn:live") && l.contains("one-shot body")),
        "{readout}"
    );
}

#[test]
fn a_one_shot_body_that_declares_no_caching_names_both_causes() {
    let (live, _) = streamer(false, || once(&[b"x"]));
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:live"), live),
    ));
    assert_eq!(
        uncached_note(&kernel, "urn:live").as_deref(),
        Some("declared; one-shot body")
    );
}

#[test]
fn a_body_that_can_be_read_again_is_stored_and_served_as_bytes_are() {
    let opened = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&opened);
    let (file, runs) = streamer(true, move || {
        Body::replay(Counted {
            bytes: b"file bytes",
            opened: Arc::clone(&counter),
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:file"), file),
    ));
    let root = Capability::root();

    let first = block_on(kernel.issue_body(source("urn:file"), &root)).unwrap();
    assert!(matches!(first.bytes, Body::Replay(_)));
    assert!(kernel.is_cached(&source("urn:file"), &root));
    // A hit hands out the same replay, without running the endpoint again…
    let second = block_on(kernel.issue_body(source("urn:file"), &root)).unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    // …and every read of it opens a fresh reader.
    assert_eq!(
        block_on(first.bytes.collect(64)).unwrap(),
        &b"file bytes"[..]
    );
    assert_eq!(
        block_on(second.bytes.collect(64)).unwrap(),
        &b"file bytes"[..]
    );
    assert_eq!(opened.load(Ordering::SeqCst), 2);
    // The bytes API is served from the same entry, collected.
    assert_eq!(
        block_on(kernel.issue(source("urn:file"), &root))
            .unwrap()
            .bytes,
        b"file bytes"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(opened.load(Ordering::SeqCst), 3);
}

#[test]
fn a_replay_that_does_not_know_its_length_is_not_admitted_by_the_byte_bound() {
    struct Unknown;
    impl Replay for Unknown {
        fn open(&self) -> ByteStream {
            Box::pin(futures::stream::iter([Ok(Bytes::from_static(b"?"))]))
        }
    }
    let (file, _) = streamer(true, || Body::replay(Unknown));
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:file"), file),
    ));
    assert_eq!(
        uncached_note(&kernel, "urn:file").as_deref(),
        Some("policy")
    );
    assert!(!kernel.is_cached(&source("urn:file"), &Capability::root()));
}

#[test]
fn an_endpoint_written_before_bodies_answers_a_body_request_unchanged() {
    let plain = FnEndpoint::new("plain", |_| {
        Ok(Representation::new(ReprType::new("text/plain"), b"plain".to_vec()).cacheable())
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:plain"), plain),
    ));
    let root = Capability::root();
    let body = block_on(kernel.issue_body(source("urn:plain"), &root)).unwrap();
    assert_eq!(body.bytes.as_bytes().unwrap(), &b"plain"[..]);
    assert!(kernel.is_cached(&source("urn:plain"), &root));
    // One entry serves both faces.
    assert_eq!(
        block_on(kernel.issue(source("urn:plain"), &root))
            .unwrap()
            .bytes,
        b"plain"
    );
    assert_eq!(kernel.cache_len(), 1);
}

#[test]
fn the_bytes_api_refuses_a_body_past_the_collect_bound_and_never_truncates() {
    // One chunk past the bound, in 1 MiB chunks that share one buffer.
    let mib = Bytes::from(vec![7u8; 1 << 20]);
    let chunks = COLLECT_LIMIT / (1 << 20) + 1;
    let (big, _) = streamer(false, move || {
        let mib = mib.clone();
        Body::once(futures::stream::iter(
            (0..chunks).map(move |_| Ok(mib.clone())),
        ))
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:big"), big),
    ));
    let root = Capability::root();
    let err = block_on(kernel.issue(source("urn:big"), &root)).unwrap_err();
    assert!(matches!(err, Error::OverBudget { .. }), "{err:?}");
    // The body API hands it over, uncollected; a caller with a larger bound may read it.
    let body = block_on(kernel.issue_body(source("urn:big"), &root)).unwrap();
    let read = block_on(body.bytes.collect(COLLECT_LIMIT + (1 << 20))).unwrap();
    assert_eq!(read.len(), COLLECT_LIMIT + (1 << 20));
}

#[test]
fn a_stream_that_fails_part_way_stores_nothing_built_on_it() {
    let (broken, broken_runs) = streamer(true, || {
        Body::once(futures::stream::iter([
            Ok(Bytes::from_static(b"half")),
            Err(Error::Unavailable("the producer went away".into())),
        ]))
    });
    // A composite that reads it as a body and propagates the failure…
    let reader = AsyncFnEndpoint::new("reader", |inv| {
        Box::pin(async move {
            let body = inv.source_body(&iri("urn:broken")).await?;
            let bytes = body.bytes.collect(1024).await?;
            Ok(Representation::new(text(), bytes.to_vec()).cacheable())
        })
    });
    // …and one that reads it through the bytes API and swallows the failure.
    let fallback = AsyncFnEndpoint::new("fallback", |inv| {
        Box::pin(async move {
            let said = match inv.source(&iri("urn:broken")).await {
                Ok(r) => r.bytes,
                Err(_) => b"fallback".to_vec(),
            };
            Ok(Representation::new(text(), said).cacheable())
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:broken"), broken)
            .bind(Exact::new("urn:reader"), reader)
            .bind(Exact::new("urn:fallback"), fallback),
    ));
    let root = Capability::root();

    // The bytes API never hands out the half it read.
    let err = block_on(kernel.issue(source("urn:broken"), &root)).unwrap_err();
    assert!(matches!(err, Error::Unavailable(_)), "{err:?}");
    // The reader fails, so nothing is stored for it.
    assert!(block_on(kernel.issue(source("urn:reader"), &root)).is_err());
    assert!(!kernel.is_cached(&source("urn:reader"), &root));
    // A fallback over the failure is uncacheable: the collect failed inside the sub-request,
    // and a failed sub-request makes its issuer volatile.
    assert_eq!(
        block_on(kernel.issue(source("urn:fallback"), &root))
            .unwrap()
            .bytes,
        b"fallback"
    );
    assert!(!kernel.is_cached(&source("urn:fallback"), &root));
    assert_eq!(kernel.cache_len(), 0);
    assert_eq!(broken_runs.load(Ordering::SeqCst), 3);
}

#[test]
fn a_composite_that_reads_a_true_forever_one_shot_body_is_itself_cacheable() {
    // Being replayable is a property of storage, not of truth: the composite holds bytes.
    let (live, live_runs) = streamer(true, || once(&[b"tok", b"ens"]));
    let summary = AsyncFnEndpoint::new("summary", |inv| {
        Box::pin(async move {
            let tokens = inv.source(&iri("urn:live")).await?.bytes;
            Ok(Representation::new(text(), [b"saw ".to_vec(), tokens].concat()).cacheable())
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:live"), live)
            .bind(Exact::new("urn:summary"), summary),
    ));
    let root = Capability::root();
    assert_eq!(
        block_on(kernel.issue(source("urn:summary"), &root))
            .unwrap()
            .bytes,
        b"saw tokens"
    );
    assert!(kernel.is_cached(&source("urn:summary"), &root));
    block_on(kernel.issue(source("urn:summary"), &root)).unwrap();
    assert_eq!(live_runs.load(Ordering::SeqCst), 1);
    // And a write to the source cuts it, through the thread the read hung it from.
    kernel.cut("urn:live");
    assert!(!kernel.is_cached(&source("urn:summary"), &root));
}

#[test]
fn a_cut_invalidates_lookups_not_open_readers() {
    let version = Arc::new(AtomicU64::new(1));
    let state = Arc::clone(&version);
    // A stream whose chunks are produced as it is read, from whatever version it opened.
    let (doc, _) = streamer(true, move || {
        let opened = state.load(Ordering::SeqCst);
        Body::replay(Snapshot(opened))
    });
    struct Snapshot(u64);
    impl Replay for Snapshot {
        fn open(&self) -> ByteStream {
            let v = self.0;
            Box::pin(futures::stream::iter(
                (0..3).map(move |i| Ok(Bytes::from(format!("v{v}.{i} ")))),
            ))
        }
        fn len_hint(&self) -> Option<u64> {
            Some(15)
        }
    }
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:doc"), doc),
    ));
    let root = Capability::root();

    let held = block_on(kernel.issue_body(source("urn:doc"), &root)).unwrap();
    let mut reader = held.bytes.into_stream();
    let first = block_on(reader.next()).unwrap().unwrap();
    // The state changes and its thread is cut while the reader is open.
    version.store(2, Ordering::SeqCst);
    kernel.cut("urn:doc");
    assert!(!kernel.is_cached(&source("urn:doc"), &root));
    // The open reader finishes the version it opened…
    let rest: Vec<Bytes> = block_on(reader.map(|c| c.unwrap()).collect::<Vec<_>>());
    let whole = [first.to_vec(), rest.concat()].concat();
    assert_eq!(whole, b"v1.0 v1.1 v1.2 ");
    // …and the next lookup recomputes.
    assert_eq!(
        block_on(kernel.issue(source("urn:doc"), &root))
            .unwrap()
            .bytes,
        b"v2.0 v2.1 v2.2 "
    );
}

/// A clock a test moves by hand.
struct Hand(AtomicU64);
impl Clock for Hand {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

#[test]
fn an_open_ended_body_ends_at_its_runs_deadline_and_never_reads_as_whole() {
    let clock = Arc::new(Hand(AtomicU64::new(1_000)));
    let tick = Arc::clone(&clock);
    // A live feed with no end of its own: each chunk advances the clock by 100 ms.
    let (feed, _) = streamer(false, move || {
        let tick = Arc::clone(&tick);
        Body::once(futures::stream::iter(0..).map(move |n: u64| {
            tick.0.fetch_add(100, Ordering::SeqCst);
            Ok(Bytes::from(format!("{n};")))
        }))
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:feed"), feed),
    ))
    .with_clock(clock.clone());
    let run = IssueContext::new().with_not_after(Time::from_millis(1_350));
    let answer =
        block_on(kernel.issue_body_with_context(source("urn:feed"), &Capability::root(), run))
            .unwrap();
    let items: Vec<Result<Bytes>> = block_on(answer.bytes.into_stream().collect::<Vec<_>>());
    // 1100, 1200, 1300 are inside the deadline; the chunk produced at 1400 is refused,
    // and the stream ends there.
    assert_eq!(items.len(), 4, "{items:?}");
    let chunks: Vec<&[u8]> = items[..3]
        .iter()
        .map(|c| c.as_ref().unwrap().as_ref())
        .collect();
    assert_eq!(chunks, [&b"0;"[..], b"1;", b"2;"]);
    assert!(
        matches!(items[3], Err(Error::OverBudget { .. })),
        "{:?}",
        items[3]
    );

    // With no deadline the feed is not ended by the kernel (a reader takes what it wants).
    let unbounded = block_on(kernel.issue_body(source("urn:feed"), &Capability::root())).unwrap();
    let taken: Vec<Result<Bytes>> = block_on(unbounded.bytes.into_stream().take(10).collect());
    assert!(taken.iter().all(|c| c.is_ok()));
}

#[test]
fn a_sink_reads_referenced_content_as_a_body() {
    let (live, _) = streamer(false, || once(&[b"up", b"load"]));
    let written = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let written = Arc::clone(&written);
        AsyncFnEndpoint::new("sink", move |inv| {
            let written = Arc::clone(&written);
            Box::pin(async move {
                let body = inv.content_body("content").await?;
                let mut stream = body.into_stream();
                while let Some(chunk) = stream.next().await {
                    written.lock().unwrap().push(chunk?);
                }
                Ok(Representation::new(text(), Vec::new()))
            })
        })
    };
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:live"), live)
            .bind(Exact::new("urn:store"), sink),
    ));
    let root = Capability::root();
    let write = Request::new(Verb::Sink, iri("urn:store"))
        .with_arg("content", ArgRef::Reference(iri("urn:live")));
    block_on(kernel.issue(write, &root)).unwrap();
    let chunks: Vec<Bytes> = written.lock().unwrap().clone();
    assert_eq!(
        chunks,
        [Bytes::from_static(b"up"), Bytes::from_static(b"load")]
    );

    // Content by id is refused until the content store exists (ledger #2240).
    let by_id = Request::new(Verb::Sink, iri("urn:store"))
        .with_arg("content", ArgRef::Content(ikigai_core::ContentId::of(b"x")));
    let err = block_on(kernel.issue(by_id, &root)).unwrap_err();
    assert!(
        matches!(err, Error::Unsupported { verb: None, .. }),
        "{err:?}"
    );
}
