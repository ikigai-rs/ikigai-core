//! **A body handed to a stage** (ledger #2402): an argument declared `reads_body`, and a
//! host that issues a request with a live body attached ([`BodyIssue::attach`]), carrying
//! the resolution chain and the upstream's provenance.
//!
//! The reproduction comes first: a counting producer piped into a body-reading stage that
//! is not a transreptor. By value (all a host could do before) the producer is polled to
//! its end before the stage starts; attached, the stage reads its first chunk while the
//! producer is still producing.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::executor::block_on;
use futures::StreamExt;
use ikigai_core::{
    ArgRef, ArgSpec, AsyncFnEndpoint, Body, BodyIssue, ByteStream, Bytes, Capability, ContentId,
    Description, Endpoint, EndpointSpace, Error, Exact, Expiry, FnEndpoint, Invocation, Iri,
    IssueContext, Kernel, MemoryContentStore, Provenance, Replay, ReprType, Representation,
    Request, Result, Scope, Thread, TraceEvent, Tracer, Verb, COLLECTED_NOTE, UNCACHED_NOTE,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

type Log = Arc<Mutex<Vec<String>>>;

const CHUNKS: usize = 5;

/// A one-shot body of `CHUNKS` chunks that logs `p{i}` as each chunk is PRODUCED, which is
/// when it is polled: a stream does nothing until someone reads it.
fn counting(log: &Log) -> Body {
    let log = Arc::clone(log);
    Body::once(futures::stream::iter(0..CHUNKS).map(move |i| {
        log.lock().unwrap().push(format!("p{i}"));
        Ok(Bytes::from(format!("{i}")))
    }))
}

/// The producer as a resource: `urn:count` answers the counting body, cacheable (fresh
/// until cut), though its body can be read only once.
struct Producer(Log);

#[async_trait]
impl Endpoint for Producer {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let body = self.invoke_body(inv).await?;
        body.collect_within(inv.memory_budget().unwrap()).await
    }

    async fn invoke_body(&self, _inv: &Invocation<'_>) -> Result<Representation<Body>> {
        Ok(Representation::from_body(ReprType::new("text/plain"), counting(&self.0)).cacheable())
    }
}

fn producer(log: &Log) -> Producer {
    Producer(Arc::clone(log))
}

/// A stage that reads `content` chunk by chunk, logging `c{i}` as each one ARRIVES, and
/// answers the bytes joined: NOT a transreptor. `declared` says whether its contract
/// declares `content` as read as a body.
fn stage(log: &Log, declared: bool) -> AsyncFnEndpoint {
    let log = Arc::clone(log);
    let content = ArgSpec::new("content").summary("what to join");
    let content = if declared {
        content.reads_body()
    } else {
        content
    };
    AsyncFnEndpoint::new("join", move |inv| {
        let log = Arc::clone(&log);
        Box::pin(async move {
            let mut stream: ByteStream = inv.content_body("content").await?.into_stream();
            let mut out = Vec::new();
            let mut n = 0;
            while let Some(chunk) = stream.next().await {
                log.lock().unwrap().push(format!("c{n}"));
                n += 1;
                out.extend_from_slice(&chunk?);
            }
            Ok(Representation::new(ReprType::new("text/plain"), out).cacheable())
        })
    })
    .with_description(
        Description::new("join")
            .verb(Verb::Source)
            .input(content)
            .output("text/plain"),
    )
}

/// A by-value stage: reads `content` with `inline_arg`, so it could never be handed a body.
fn by_value() -> FnEndpoint {
    FnEndpoint::new("upper", |inv| {
        let text = inv.inline_str("content")?.to_uppercase();
        Ok(Representation::new(ReprType::new("text/plain"), text.into_bytes()).cacheable())
    })
    .with_description(
        Description::new("upper")
            .verb(Verb::Source)
            .input(ArgSpec::new("content")),
    )
}

fn kernel(log: &Log) -> Kernel {
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:count"), producer(log))
        .bind(Exact::new("urn:join"), stage(log, true))
        .bind(Exact::new("urn:join-undeclared"), stage(log, false))
        .bind(Exact::new("urn:upper"), by_value());
    Kernel::new(Arc::new(space))
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn position(log: &[String], entry: &str) -> usize {
    log.iter()
        .position(|e| e == entry)
        .unwrap_or_else(|| panic!("`{entry}` is not in {log:?}"))
}

fn provenance(answer: &Representation<Body>) -> Provenance {
    Provenance::new(answer.expiry, answer.threads().clone())
}

#[derive(Default)]
struct Collect(Mutex<Vec<TraceEvent>>);

impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Collect {
    fn note(&self, target: &str, key: &str) -> Option<String> {
        let events = self.0.lock().unwrap();
        let event = events.iter().rev().find(|e| e.target == target)?;
        event
            .notes
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }
}

/// The reproduction, as a host could pipe before 0.2.3: it holds the producer's body and
/// can hand a stage only bytes, so the producer is polled to its end before the stage
/// reads anything.
#[test]
fn by_value_the_producer_is_polled_to_the_end_before_the_stage_starts() {
    let log: Log = Log::default();
    let kernel = kernel(&log);
    let root = Capability::root();
    let upstream = block_on(kernel.issue_body(source("urn:count"), &root)).unwrap();
    let bytes = block_on(upstream.bytes.collect(1 << 20)).unwrap();
    let staged = source("urn:join").with_arg("content", ArgRef::Inline(bytes.to_vec()));
    let answer = block_on(kernel.issue(staged, &root)).unwrap();
    assert_eq!(answer.bytes, b"01234");
    let log = log.lock().unwrap();
    assert!(
        position(&log, &format!("p{}", CHUNKS - 1)) < position(&log, "c0"),
        "every chunk produced before the first is read: {log:?}"
    );
}

/// The fix: the producer's body attached to a stage that declares `content` read as a body.
/// The stage reads its first chunk before the producer has produced its last: the two
/// interleave, and nothing is collected between them.
#[test]
fn attached_to_a_declared_reader_the_stage_starts_before_the_producer_ends() {
    let log: Log = Log::default();
    let kernel = kernel(&log);
    let root = Capability::root();
    let upstream = block_on(kernel.issue_body(source("urn:count"), &root)).unwrap();
    let issue = BodyIssue::new()
        .with_incoming(provenance(&upstream))
        .attach("content", upstream.bytes);
    let answer = block_on(kernel.issue_with(source("urn:join"), &root, issue)).unwrap();
    assert_eq!(answer.bytes, b"01234");
    let log = log.lock().unwrap();
    assert!(
        position(&log, "c0") < position(&log, &format!("p{}", CHUNKS - 1)),
        "the first chunk is read while the producer is still producing: {log:?}"
    );
    // Strictly alternating: each chunk is produced, then read, then the next produced.
    let expected: Vec<String> = (0..CHUNKS)
        .flat_map(|i| [format!("p{i}"), format!("c{i}")])
        .collect();
    assert_eq!(*log, expected);
}

/// The same stage with `content` NOT declared: the kernel collects the attached body before
/// entering it, so the producer runs to its end first (the by-value order), the stage still
/// answers, and the trace names the argument it collected.
#[test]
fn an_undeclared_argument_is_handed_the_body_collected() {
    let log: Log = Log::default();
    let kernel = kernel(&log);
    let root = Capability::root();
    let tracer = Arc::new(Collect::default());
    let issue = BodyIssue::new()
        .traced(tracer.clone())
        .attach("content", counting(&log));
    let answer = block_on(kernel.issue_with(source("urn:join-undeclared"), &root, issue)).unwrap();
    assert_eq!(answer.bytes, b"01234");
    {
        let log = log.lock().unwrap();
        assert!(
            position(&log, &format!("p{}", CHUNKS - 1)) < position(&log, "c0"),
            "collected first: {log:?}"
        );
    }
    assert_eq!(
        tracer
            .note("urn:join-undeclared", COLLECTED_NOTE)
            .as_deref(),
        Some("content")
    );
    // A stage that reads `content` with `inline_arg` is handed the bytes, and the request
    // IS the by-value one: one identity, one cache entry.
    let issue = BodyIssue::new().attach("content", Body::from(b"abc".to_vec()));
    let answer = block_on(kernel.issue_with(source("urn:upper"), &root, issue)).unwrap();
    assert_eq!(answer.bytes, b"ABC");
    let by_value = source("urn:upper").with_arg("content", ArgRef::Inline(b"abc".to_vec()));
    assert!(kernel.is_cached(&by_value, &root));
}

/// Past the run's byte budget the collection is refused with OverBudget naming the argument,
/// never a shorter value, and the endpoint is never entered.
#[test]
fn an_undeclared_argument_past_the_budget_is_over_budget() {
    let log: Log = Log::default();
    let kernel = kernel(&log);
    let root = Capability::root();
    let issue = BodyIssue::new()
        .with_context(IssueContext::new().with_memory_limit(3))
        .attach("content", counting(&log));
    let refused = block_on(kernel.issue_with(source("urn:join-undeclared"), &root, issue))
        .expect_err("five bytes do not fit in three");
    match &refused {
        Error::OverBudget { detail } => {
            assert!(detail.contains("`content`"), "{detail}");
            assert!(detail.contains("reads_body"), "{detail}");
        }
        other => panic!("expected OverBudget, got {other:?}"),
    }
    assert!(
        !log.lock().unwrap().iter().any(|e| e.starts_with('c')),
        "the stage was never entered"
    );
    // A declared reader under the same budget is handed the body as it is: it decides
    // what to buffer (here, nothing beyond its own output).
    let issue = BodyIssue::new()
        .with_context(IssueContext::new().with_memory_limit(3))
        .attach("content", counting(&log));
    let answer = block_on(kernel.issue_with(source("urn:join"), &root, issue)).unwrap();
    assert_eq!(answer.bytes, b"01234");
}

/// A held body that can be read again, with the digest of its bytes as its validator: what
/// a spill answers.
struct Spilled(Bytes);

impl Replay for Spilled {
    fn open(&self) -> ByteStream {
        Box::pin(futures::stream::iter([Ok(self.0.clone())]))
    }

    fn len_hint(&self) -> Option<u64> {
        Some(self.0.len() as u64)
    }

    fn validator(&self) -> Option<ContentId> {
        Some(ContentId::of(&self.0))
    }
}

/// A declared reader answers the three ways a body can be given (inline, a `b3:` content id
/// from the store, a live body attached) and a spilled body attached, exactly alike.
#[test]
fn a_declared_reader_answers_every_form_of_its_argument_alike() {
    let log: Log = Log::default();
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let kernel = kernel(&log).with_content_store(store.clone());
    let root = Capability::root();
    let bytes = b"0123456789".to_vec();
    let join = || source("urn:join");

    let inline = block_on(kernel.issue(
        join().with_arg("content", ArgRef::Inline(bytes.clone())),
        &root,
    ))
    .unwrap();
    let held = store.put(bytes.clone()).unwrap();
    let by_id = block_on(kernel.issue(
        join().with_arg("content", ArgRef::Content(held.id())),
        &root,
    ))
    .unwrap();
    let chunks: Vec<_> = bytes
        .chunks(3)
        .map(|c| Ok(Bytes::copy_from_slice(c)))
        .collect();
    let live = block_on(kernel.issue_with(
        join(),
        &root,
        BodyIssue::new().attach("content", Body::once(futures::stream::iter(chunks))),
    ))
    .unwrap();
    let spilled = block_on(kernel.issue_with(
        join(),
        &root,
        BodyIssue::new().attach("content", Body::replay(Spilled(Bytes::from(bytes.clone())))),
    ))
    .unwrap();
    for answer in [&by_id, &live, &spilled] {
        assert_eq!(answer.bytes, inline.bytes);
        assert_eq!(answer.repr_type, inline.repr_type);
    }
    // A spilled body is attached under its digest, so its request is the content-id one.
    assert_eq!(ContentId::of(&bytes), held.id());
}

/// A live body is read once, so the answer built on it is never stored, and the uncached
/// note names the one-shot content. Spilled first, it is attached under its digest, and
/// the answer is cached on it like any value.
#[test]
fn an_attached_read_is_uncacheable_unless_spilled_and_says_why() {
    let log: Log = Log::default();
    let kernel = kernel(&log);
    let root = Capability::root();
    let tracer = Arc::new(Collect::default());
    let issue = BodyIssue::new()
        .traced(tracer.clone())
        .attach("content", counting(&log));
    block_on(kernel.issue_with(source("urn:join"), &root, issue)).unwrap();
    let why = tracer
        .note("urn:join", UNCACHED_NOTE)
        .expect("an attached one-shot read is not stored");
    assert!(why.contains("one-shot content"), "{why}");
    assert_eq!(kernel.cache_len(), 0);

    // Spilled: the same request twice is one computation.
    let spilled = || Body::replay(Spilled(Bytes::from_static(b"spilled")));
    let first = BodyIssue::new().attach("content", spilled());
    block_on(kernel.issue_with(source("urn:join"), &root, first)).unwrap();
    let tracer = Arc::new(Collect::default());
    let again = BodyIssue::new()
        .traced(tracer.clone())
        .attach("content", spilled());
    let answer = block_on(kernel.issue_with(source("urn:join"), &root, again)).unwrap();
    assert_eq!(answer.bytes, b"spilled");
    let events = tracer.0.lock().unwrap();
    let event = events.iter().find(|e| e.target == "urn:join").unwrap();
    assert!(event.cache_hit, "a spilled body is a value: {event:?}");
}

/// A read whose body is attached but never read is still not stored: the nonce in its
/// identity means it can never be asked for again.
#[test]
fn an_unread_one_shot_attachment_is_not_stored_either() {
    let space = EndpointSpace::new().bind(
        Exact::new("urn:ignore"),
        FnEndpoint::new("ignore", |_| {
            Ok(Representation::new(ReprType::new("text/plain"), b"ok".to_vec()).cacheable())
        })
        .with_description(
            Description::new("ignore")
                .verb(Verb::Source)
                .input(ArgSpec::new("content").optional().reads_body()),
        ),
    );
    let kernel = Kernel::new(Arc::new(space));
    let issue = BodyIssue::new().attach("content", Body::once(futures::stream::empty()));
    let answer =
        block_on(kernel.issue_with(source("urn:ignore"), &Capability::root(), issue)).unwrap();
    assert_eq!(answer.bytes, b"ok");
    assert_eq!(kernel.cache_len(), 0);
}

/// The scope and the upstream's provenance reach the stage through the body issue: it
/// resolves in the corridor the issue names (the root binds nothing there), and its answer
/// is no more cacheable than the upstream and hangs from the upstream's threads.
#[test]
fn the_body_issue_carries_scope_and_provenance() {
    let log: Log = Log::default();
    let corridor: Arc<dyn ikigai_core::Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:corridor:join"), stage(&log, true)));
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    let root = Capability::root();
    let read = || source("urn:corridor:join");
    let spilled = || Body::replay(Spilled(Bytes::from_static(b"doc")));
    let scope = || Scope::empty().with_named(iri("urn:ctx:corridor"), Arc::clone(&corridor));
    let upstream = Provenance::new(Expiry::Never, BTreeSet::from([Thread::from("urn:doc")]));

    // Outside the corridor the name is bound nowhere.
    let outside = BodyIssue::new().attach("content", spilled());
    assert!(matches!(
        block_on(kernel.issue_with(read(), &root, outside)),
        Err(Error::Unresolved(_))
    ));
    // Inside it, the stage answers, carrying the upstream's thread…
    let inside = BodyIssue::new()
        .in_scope(scope())
        .with_incoming(upstream.clone())
        .attach("content", spilled());
    let answer = block_on(kernel.issue_body_with(read(), &root, inside)).unwrap();
    assert!(answer.threads().contains(&Thread::from("urn:doc")));
    assert_eq!(
        kernel.cache_len(),
        1,
        "a spilled read with a cacheable upstream is stored"
    );
    // …so cutting the upstream invalidates it.
    kernel.cut("urn:doc");
    let tracer = Arc::new(Collect::default());
    let again = BodyIssue::new()
        .in_scope(scope())
        .with_incoming(upstream)
        .traced(tracer.clone())
        .attach("content", spilled());
    block_on(kernel.issue_with(read(), &root, again)).unwrap();
    assert!(!tracer.0.lock().unwrap()[0].cache_hit, "the cut reached it");

    // A volatile upstream makes the stage volatile, and the note says it was the upstream.
    // (Another body: the cache key is the request, and the incoming provenance is not part
    // of it, so the same body would be served from the entry stored above.)
    let tracer = Arc::new(Collect::default());
    let volatile = BodyIssue::new()
        .in_scope(scope())
        .with_incoming(Provenance::new(Expiry::Always, BTreeSet::new()))
        .traced(tracer.clone())
        .attach(
            "content",
            Body::replay(Spilled(Bytes::from_static(b"live"))),
        );
    block_on(kernel.issue_with(read(), &root, volatile)).unwrap();
    let why = tracer.note("urn:corridor:join", UNCACHED_NOTE).unwrap();
    assert!(why.contains("upstream"), "{why}");
}

/// An incremental transreptor's `content` is a body reader without declaring it: the
/// promise core made first (ledger #2239), so an attached body streams into it.
#[test]
fn an_incremental_transreptors_content_takes_the_body_as_it_is() {
    let log: Log = Log::default();
    let reader = Arc::clone(&log);
    let transreptor = AsyncFnEndpoint::new("bytes-to-text", move |inv| {
        let log = Arc::clone(&reader);
        Box::pin(async move {
            let mut stream = inv.content_body("content").await?.into_stream();
            let mut out = Vec::new();
            while let Some(chunk) = stream.next().await {
                log.lock().unwrap().push("c".to_string());
                out.extend_from_slice(&chunk?);
            }
            Ok(Representation::new(ReprType::new("text/plain"), out))
        })
    })
    .with_description(
        Description::new("bytes-to-text")
            .transreptor(["application/octet-stream"], ["text/plain"])
            .incremental()
            .verb(Verb::Source)
            .input(ArgSpec::new("content")),
    );
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:to-text"), transreptor),
    ));
    let tracer = Arc::new(Collect::default());
    let issue = BodyIssue::new()
        .traced(tracer.clone())
        .attach("content", counting(&log));
    let answer =
        block_on(kernel.issue_with(source("urn:to-text"), &Capability::root(), issue)).unwrap();
    assert_eq!(answer.bytes, b"01234");
    let log = log.lock().unwrap();
    assert!(
        position(&log, "c") < position(&log, &format!("p{}", CHUNKS - 1)),
        "{log:?}"
    );
    assert_eq!(tracer.note("urn:to-text", COLLECTED_NOTE), None);
}

/// What the kernel refuses rather than guesses: a body attached to a `Meta` request or to
/// a kernel operation, and one attached under a name the request already carries.
#[test]
fn a_body_on_meta_or_on_a_given_argument_is_refused() {
    let log: Log = Log::default();
    let kernel = kernel(&log);
    let root = Capability::root();
    let meta = Request::new(Verb::Meta, iri("urn:join"));
    let refused = block_on(kernel.issue_with(
        meta,
        &root,
        BodyIssue::new().attach("content", Body::from(b"x".to_vec())),
    ))
    .expect_err("Meta reads no body");
    assert!(matches!(refused, Error::InvalidArgument { ref name, .. } if name == "content"));
    let cut = Request::new(Verb::Sink, iri("urn:kernel:cut"));
    let refused = block_on(kernel.issue_with(
        cut,
        &root,
        BodyIssue::new().attach("thread", Body::from(b"urn:doc".to_vec())),
    ))
    .expect_err("a kernel operation reads no body");
    match refused {
        Error::InvalidArgument { name, detail } => {
            assert_eq!(name, "thread");
            assert!(detail.contains("kernel operation"), "{detail}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
    let given = source("urn:join").with_arg("content", ArgRef::Inline(b"y".to_vec()));
    let refused = block_on(kernel.issue_with(
        given,
        &root,
        BodyIssue::new().attach("content", Body::from(b"x".to_vec())),
    ))
    .expect_err("one value per argument");
    match refused {
        Error::InvalidArgument { name, detail } => {
            assert_eq!(name, "content");
            assert!(detail.contains(ikigai_core::GIVEN_TWICE), "{detail}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

/// The contract half: `reads_body` is part of what an action IS (its contract IRI moves),
/// and a binding cannot declare it.
#[test]
fn the_declaration_is_part_of_the_contract_and_only_on_a_by_value_input() {
    let plain = Description::new("join")
        .verb(Verb::Source)
        .input(ArgSpec::new("content"));
    let reader = Description::new("join")
        .verb(Verb::Source)
        .input(ArgSpec::new("content").reads_body());
    assert_ne!(
        plain.action_specs()[0].contract_iri("join"),
        reader.action_specs()[0].contract_iri("join")
    );
    assert!(reader.validate().is_ok());
    let binding = Description::new("file")
        .verb(Verb::Source)
        .input(ArgSpec::new("path").binding().reads_body());
    let refused = binding
        .validate()
        .expect_err("a binding's value is in the name");
    assert!(refused.to_string().contains("reads_body"), "{refused}");
}

/// The spill facility of `tests/stream_spill.rs`, minimal: holds what it is handed, under
/// its limit, as a body that can be read again with the digest as its validator.
struct InMemory;

#[async_trait]
impl ikigai_core::Spill for InMemory {
    async fn spill(&self, mut body: ByteStream, max_bytes: u64) -> Result<Arc<dyn Replay>> {
        let mut held = Vec::new();
        while let Some(chunk) = body.next().await {
            held.extend_from_slice(&chunk?);
            if held.len() as u64 > max_bytes {
                return Err(Error::OverBudget {
                    detail: format!("past {max_bytes} bytes"),
                });
            }
        }
        Ok(Arc::new(Spilled(Bytes::from(held))))
    }
}

/// Core's own body readers say so, and "unless spilled" end to end: a live body attached
/// to `urn:stream:spill` (which declares `content` read as a body, so it streams in, never
/// collected by the kernel) comes back as a body that can be read again, and attached to
/// the next stage under its digest, that stage's answer is cached on it.
#[test]
fn a_live_body_spilled_through_the_kernel_makes_the_next_stage_cacheable() {
    let log: Log = Log::default();
    let kernel = kernel(&log)
        .with_spill(Arc::new(InMemory))
        .with_stream_spill(0, 1 << 20);
    let root = Capability::root();
    let tracer = Arc::new(Collect::default());
    let spill = BodyIssue::new()
        .traced(tracer.clone())
        .attach("content", counting(&log));
    let spilled =
        block_on(kernel.issue_body_with(source(ikigai_core::stream::SPILL), &root, spill)).unwrap();
    assert_eq!(
        tracer.note(ikigai_core::stream::SPILL, COLLECTED_NOTE),
        None,
        "urn:stream:spill reads `content` as a body"
    );
    assert!(!spilled.bytes.is_one_shot());
    let shared = spilled.bytes;
    let join = |body: Body| BodyIssue::new().attach("content", body);
    let again = || match &shared {
        Body::Replay(replay) => Body::Replay(Arc::clone(replay)),
        other => panic!("a spill answers a replayable body, got {other:?}"),
    };
    let first = block_on(kernel.issue_with(source("urn:join"), &root, join(again()))).unwrap();
    assert_eq!(first.bytes, b"01234");
    let tracer = Arc::new(Collect::default());
    let second = BodyIssue::new()
        .traced(tracer.clone())
        .attach("content", again());
    block_on(kernel.issue_with(source("urn:join"), &root, second)).unwrap();
    let events = tracer.0.lock().unwrap();
    assert!(events.iter().any(|e| e.target == "urn:join" && e.cache_hit));
}

/// Retry/failover guards must inspect attachments without taking or opening them.
#[test]
fn attachment_reopenability_survives_reads() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct WithoutValidator(Arc<AtomicUsize>);
    impl Replay for WithoutValidator {
        fn open(&self) -> ByteStream {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(futures::stream::iter([Ok(Bytes::from_static(b"input"))]))
        }
    }

    let opens = Arc::new(AtomicUsize::new(0));

    let reader = AsyncFnEndpoint::new("inspect", |inv| {
        Box::pin(async move {
            let expected = inv.inline_arg("expected")? == b"yes";
            assert_eq!(inv.attachments_reopenable(), expected);
            assert_eq!(inv.attachments_reopenable(), expected);
            let body = inv.content_body("content").await?;
            // Once has already been taken, even before its stream is polled.
            assert_eq!(inv.attachments_reopenable(), expected);
            assert_eq!(body.collect(32).await?, b"input"[..]);
            assert_eq!(inv.attachments_reopenable(), expected);
            if expected {
                assert_eq!(
                    inv.content_body("content").await?.collect(32).await?,
                    b"input"[..]
                );
            } else {
                assert!(matches!(
                    inv.content_body("content").await,
                    Err(Error::Conflict(_))
                ));
            }
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"ok".to_vec(),
            ))
        })
    })
    .with_description(
        Description::new("inspect")
            .verb(Verb::Source)
            .input(ArgSpec::new("content").reads_body())
            .input(ArgSpec::new("expected")),
    );
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:inspect"), reader),
    ));
    for (body, expected) in [
        (
            Body::once(futures::stream::iter([Ok(Bytes::from_static(b"input"))])),
            false,
        ),
        (Body::replay(Spilled(Bytes::from_static(b"input"))), true),
        (Body::replay(WithoutValidator(Arc::clone(&opens))), true),
        (Body::from(b"input".to_vec()), true),
    ] {
        let request = source("urn:inspect").with_arg(
            "expected",
            ArgRef::Inline(if expected {
                b"yes".to_vec()
            } else {
                b"no".to_vec()
            }),
        );
        block_on(kernel.issue_with(
            request,
            &Capability::root(),
            BodyIssue::new().attach("content", body),
        ))
        .unwrap();
    }
    assert_eq!(
        opens.load(Ordering::SeqCst),
        2,
        "queries must not open a replay body"
    );

    let request = source("urn:inspect")
        .with_arg("content", ArgRef::Inline(b"input".to_vec()))
        .with_arg("expected", ArgRef::Inline(b"yes".to_vec()));
    block_on(kernel.issue(request, &Capability::root())).unwrap();
}
