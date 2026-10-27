//! **`urn:stream:spill`** (ledger #2372): a body written through the host's spill facility
//! and answered as a body that can be read again, under the caller's disk budget
//! (`urn:cap:stream:spill:<bytes>`); and the public [`ContentHasher`] a facility hashes with.
//! The facility here keeps a `Vec` behind a `Replay`: core's tests never touch a file.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::executor::block_on;
use futures::StreamExt;
use ikigai_core::{
    cap_stream_spill, ArgRef, Body, ByteStream, Bytes, Capability, ContentHasher, ContentId,
    Endpoint, EndpointSpace, Error, Exact, FailureClass, Invocation, Iri, Kernel, Replay, ReprType,
    Representation, Request, Result, Spill, TraceEvent, Tracer, Verb, CAP_STREAM_SPILL,
    FAILED_NOTE, UNDECLARED_FAILURE_NOTE,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn inline(bytes: &[u8]) -> ArgRef {
    ArgRef::Inline(bytes.to_vec())
}

fn spill() -> Request {
    Request::new(Verb::Source, iri(ikigai_core::stream::SPILL))
}

/// A spilled body held in memory: one chunk per `open`, its id computed while written.
struct Held {
    bytes: Bytes,
    id: ContentId,
}

impl Replay for Held {
    fn open(&self) -> ByteStream {
        Box::pin(futures::stream::iter([Ok(self.bytes.clone())]))
    }

    fn len_hint(&self) -> Option<u64> {
        Some(self.bytes.len() as u64)
    }

    fn validator(&self) -> Option<ContentId> {
        Some(self.id)
    }
}

/// The in-memory facility: hashes while it "writes", stops at the limit, and records the
/// limit it was handed and how many times it ran.
#[derive(Default)]
struct InMemory {
    limit: AtomicU64,
    runs: AtomicUsize,
}

#[async_trait]
impl Spill for InMemory {
    async fn spill(&self, mut body: ByteStream, max_bytes: u64) -> Result<Arc<dyn Replay>> {
        self.limit.store(max_bytes, Ordering::SeqCst);
        self.runs.fetch_add(1, Ordering::SeqCst);
        let (mut held, mut hasher) = (Vec::new(), ContentHasher::new());
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            if (held.len() + chunk.len()) as u64 > max_bytes {
                return Err(Error::OverBudget {
                    detail: format!("past {max_bytes} bytes"),
                });
            }
            hasher.update(&chunk);
            held.extend_from_slice(&chunk);
        }
        Ok(Arc::new(Held {
            bytes: held.into(),
            id: hasher.finish(),
        }))
    }
}

/// Answers a one-shot body of three chunks, typed `text/csv`, cacheable, counting reads.
struct Upload(Arc<AtomicUsize>);

const UPLOAD: &[u8] = b"a,b\n1,2\n3,4\n";

#[async_trait]
impl Endpoint for Upload {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let body = self.invoke_body(inv).await?;
        body.collect_within(inv.memory_budget().unwrap()).await
    }

    async fn invoke_body(&self, _inv: &Invocation<'_>) -> Result<Representation<Body>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let chunks = [&UPLOAD[..4], &UPLOAD[4..8], &UPLOAD[8..]].map(|c| Ok(Bytes::from(c)));
        Ok(Representation::from_body(
            ReprType::new("text/csv"),
            Body::once(futures::stream::iter(chunks)),
        )
        // Fresh until its thread is cut, though its body can be read only once.
        .cacheable())
    }
}

fn kernel() -> (Kernel, Arc<InMemory>, Arc<AtomicUsize>) {
    let reads = Arc::new(AtomicUsize::new(0));
    let facility = Arc::new(InMemory::default());
    let space = EndpointSpace::new().bind(Exact::new("urn:upload"), Upload(reads.clone()));
    let kernel = Kernel::new(Arc::new(space))
        .with_spill(facility.clone())
        .with_stream_spill(0, 100);
    (kernel, facility, reads)
}

async fn read_all(replay: &Arc<dyn Replay>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut stream = replay.open();
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk.unwrap());
    }
    out
}

fn replay_of(answer: Representation<Body>) -> (ReprType, Arc<dyn Replay>) {
    match answer.bytes {
        Body::Replay(replay) => (answer.repr_type, replay),
        other => panic!("a spill answers a replay, got {other:?}"),
    }
}

#[test]
fn content_hasher_in_chunks_is_content_id_of_the_whole() {
    let mut hasher = ContentHasher::new();
    for chunk in UPLOAD.chunks(3) {
        hasher.update(chunk);
    }
    assert_eq!(hasher.finish(), ContentId::of(UPLOAD));
}

#[test]
fn a_one_shot_body_spills_to_a_replay_read_twice_with_its_content_id_and_type() {
    let (kernel, facility, _) = kernel();
    let request = spill().with_arg("content", ArgRef::Reference(iri("urn:upload")));
    let answer = block_on(kernel.issue_body(request, &Capability::root())).unwrap();
    let (repr_type, replay) = replay_of(answer);
    assert_eq!(
        repr_type,
        ReprType::new("text/csv"),
        "the source's type is kept"
    );
    assert_eq!(block_on(read_all(&replay)), UPLOAD);
    assert_eq!(
        block_on(read_all(&replay)),
        UPLOAD,
        "a second open, the same bytes"
    );
    assert_eq!(replay.validator(), Some(ContentId::of(UPLOAD)));
    assert_eq!(replay.len_hint(), Some(UPLOAD.len() as u64));
    assert_eq!(
        facility.limit.load(Ordering::SeqCst),
        100,
        "root gets the ceiling"
    );
}

#[test]
fn past_the_limit_is_over_budget_never_a_shorter_body() {
    let (kernel, facility, _) = kernel();
    let short = (UPLOAD.len() - 1) as u64;
    // Streamed past `max-bytes`: the facility refuses at the limit.
    let streamed = spill()
        .with_arg("content", ArgRef::Reference(iri("urn:upload")))
        .with_arg("max-bytes", inline(short.to_string().as_bytes()));
    assert!(matches!(
        block_on(kernel.issue_body(streamed, &Capability::root())),
        Err(Error::OverBudget { .. })
    ));
    // A body that states its length past the limit is refused before the facility runs.
    let runs = facility.runs.load(Ordering::SeqCst);
    let stated = spill()
        .with_arg("content", inline(UPLOAD))
        .with_arg("max-bytes", inline(short.to_string().as_bytes()));
    assert!(matches!(
        block_on(kernel.issue(stated, &Capability::root())),
        Err(Error::OverBudget { .. })
    ));
    assert_eq!(
        facility.runs.load(Ordering::SeqCst),
        runs,
        "nothing was written"
    );
    // Exactly the limit is in.
    let exact = spill()
        .with_arg("content", inline(UPLOAD))
        .with_arg("max-bytes", inline(UPLOAD.len().to_string().as_bytes()));
    assert_eq!(
        block_on(kernel.issue(exact, &Capability::root()))
            .unwrap()
            .bytes,
        UPLOAD
    );
}

#[test]
fn no_facility_is_unsupported_naming_it() {
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    let request = spill().with_arg("content", inline(b"hello"));
    match block_on(kernel.issue(request, &Capability::root())) {
        Err(Error::Unsupported { verb: None, detail }) => {
            assert!(detail.contains("no spill facility"), "{detail}")
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn the_grant_narrows_and_never_widens() {
    let (kernel, facility, _) = kernel();
    let request = || spill().with_arg("content", inline(b"hello"));
    let limit_for = |capability: &Capability, request: Request| {
        block_on(kernel.issue(request, capability)).map(|answer| {
            assert_eq!(answer.bytes, b"hello");
            facility.limit.load(Ordering::SeqCst)
        })
    };
    // No grant in the family at all: denied by the declared floor.
    let plain = Capability::scoped(["urn:cap:read"]);
    assert!(matches!(
        block_on(kernel.issue(request(), &plain)),
        Err(Error::Denied(_))
    ));
    // A grant is the caller's limit.
    let five = Capability::scoped([cap_stream_spill(5)]);
    assert_eq!(limit_for(&five, request()).unwrap(), 5);
    let four = Capability::scoped([cap_stream_spill(4)]);
    assert!(matches!(
        block_on(kernel.issue(request(), &four)),
        Err(Error::OverBudget { .. })
    ));
    // Never above the host's ceiling, however large the grant.
    let huge = Capability::scoped([cap_stream_spill(u64::MAX)]);
    assert_eq!(limit_for(&huge, request()).unwrap(), 100);
    // `max-bytes` lowers the grant and never raises it.
    let lower = request().with_arg("max-bytes", inline(b"7"));
    assert_eq!(limit_for(&huge, lower).unwrap(), 7);
    let higher = request().with_arg("max-bytes", inline(b"1000"));
    assert_eq!(limit_for(&five, higher).unwrap(), 5);
    // An attenuated capability keeps only what it was left: the smaller grant.
    let both = Capability::scoped([cap_stream_spill(5), cap_stream_spill(50)]);
    let narrowed = both.attenuate([cap_stream_spill(5)]);
    assert_eq!(limit_for(&narrowed, request()).unwrap(), 5);
}

#[test]
fn a_cached_spill_hangs_from_its_contents_threads() {
    let (kernel, facility, reads) = kernel();
    let request = || spill().with_arg("content", ArgRef::Reference(iri("urn:upload")));
    assert_eq!(
        block_on(kernel.issue(request(), &Capability::root()))
            .unwrap()
            .bytes,
        UPLOAD
    );
    assert!(kernel.is_cached(&request(), &Capability::root()));
    assert_eq!(
        block_on(kernel.issue(request(), &Capability::root()))
            .unwrap()
            .bytes,
        UPLOAD
    );
    assert_eq!(reads.load(Ordering::SeqCst), 1, "the second read was a hit");
    assert_eq!(facility.runs.load(Ordering::SeqCst), 1);
    kernel.cut("urn:upload");
    assert!(
        !kernel.is_cached(&request(), &Capability::root()),
        "a cut of the content cuts its spill"
    );
}

/// Every trace event's notes, in order.
#[derive(Default)]
struct Notes(Mutex<Vec<(String, String)>>);

impl Tracer for Notes {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().extend(event.notes);
    }
}

#[test]
fn declared_equals_enforced() {
    // The floor is the declared `requires`: a grant in the family admits, none denies.
    let (kernel, _, _) = kernel();
    let notes = Arc::new(Notes::default());
    kernel.set_tracer(notes.clone());
    let any_grant = Capability::scoped([cap_stream_spill(0)]);
    let request = spill().with_arg("content", inline(b"hello"));
    // OverBudget is declared: the kernel reports no undeclared failure.
    assert!(matches!(
        block_on(kernel.issue(request.clone(), &any_grant)),
        Err(Error::OverBudget { .. })
    ));
    // So is Unsupported (classed Permanent), with no facility installed.
    let bare = Kernel::new(Arc::new(EndpointSpace::new()));
    bare.set_tracer(notes.clone());
    assert!(matches!(
        block_on(bare.issue(request.clone(), &any_grant)),
        Err(Error::Unsupported { .. })
    ));
    let notes = notes.0.lock().unwrap();
    assert!(
        notes.iter().any(|(key, _)| key == FAILED_NOTE),
        "both failures were traced: {notes:?}"
    );
    assert!(
        !notes.iter().any(|(key, _)| key == UNDECLARED_FAILURE_NOTE),
        "a failure the builtin does not declare: {notes:?}"
    );
    // The wildcard is the family the grants are minted in.
    assert!(cap_stream_spill(0).starts_with(CAP_STREAM_SPILL.trim_end_matches('*')));
    assert_eq!(
        Error::Unsupported {
            verb: None,
            detail: String::new()
        }
        .class(),
        FailureClass::Permanent
    );
}
