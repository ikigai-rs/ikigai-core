//! **The stream builtins** (ledger #2239, stage 3 of the streams design): `urn:stream:collect`,
//! `urn:stream:text` and `urn:stream:range`, answered by the kernel itself and sealed; and a
//! plan that hands an incremental step its body without collecting it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::executor::block_on;
use ikigai_core::{
    ArgRef, AsyncFnEndpoint, Body, ByteStream, Bytes, Capability, ContentId, Description, Endpoint,
    EndpointSpace, Error, Exact, FnEndpoint, Invocation, Iri, Kernel, Replay, ReprType,
    Representation, Request, Result, TraceEvent, Tracer, Verb, UNCACHED_NOTE,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn inline(text: &str) -> ArgRef {
    ArgRef::Inline(text.as_bytes().to_vec())
}

fn chunks(parts: &'static [&'static [u8]]) -> Body {
    Body::once(futures::stream::iter(
        parts.iter().map(|p| Ok(Bytes::from_static(p))),
    ))
}

/// Answers whatever body `make` builds, as `repr_type`, cacheable.
struct Answers<F> {
    repr_type: ReprType,
    make: F,
}

#[async_trait]
impl<F: Fn() -> Body + Send + Sync> Endpoint for Answers<F> {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let body = self.invoke_body(inv).await?;
        body.collect_within(inv.memory_budget().unwrap()).await
    }

    async fn invoke_body(&self, _inv: &Invocation<'_>) -> Result<Representation<Body>> {
        Ok(Representation::from_body(self.repr_type.clone(), (self.make)()).cacheable())
    }
}

fn answers<F: Fn() -> Body + Send + Sync>(media: &str, make: F) -> Answers<F> {
    Answers {
        repr_type: ReprType::new(media),
        make,
    }
}

/// A seekable replay over fixed bytes, counting opens; `seekable: false` refuses `open_at`.
struct Fixed {
    bytes: &'static [u8],
    seekable: bool,
    validator: bool,
    opened: Arc<AtomicUsize>,
}

impl Replay for Fixed {
    fn open(&self) -> ByteStream {
        self.opened.fetch_add(1, Ordering::SeqCst);
        // Two chunks, so a reader crosses a boundary.
        let (a, b) = self.bytes.split_at(self.bytes.len() / 2);
        Box::pin(futures::stream::iter([
            Ok(Bytes::from_static(a)),
            Ok(Bytes::from_static(b)),
        ]))
    }

    fn open_at(&self, offset: u64) -> Option<ByteStream> {
        if !self.seekable {
            return None;
        }
        self.opened.fetch_add(1, Ordering::SeqCst);
        let rest = &self.bytes[(offset as usize).min(self.bytes.len())..];
        Some(Box::pin(futures::stream::iter([Ok(Bytes::from_static(
            rest,
        ))])))
    }

    fn len_hint(&self) -> Option<u64> {
        Some(self.bytes.len() as u64)
    }

    fn validator(&self) -> Option<ContentId> {
        self.validator.then(|| ContentId::of(self.bytes))
    }
}

fn fixed(bytes: &'static [u8], seekable: bool) -> (impl Fn() -> Body, Arc<AtomicUsize>) {
    let opened = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&opened);
    (
        move || {
            Body::replay(Fixed {
                bytes,
                seekable,
                validator: true,
                opened: Arc::clone(&counter),
            })
        },
        opened,
    )
}

fn kernel() -> (Kernel, Arc<AtomicUsize>) {
    let (seekable, opened) = fixed(b"0123456789", true);
    let (unseekable, _) = fixed(b"0123456789", false);
    let space = EndpointSpace::new()
        .bind(
            Exact::new("urn:doc"),
            FnEndpoint::new("doc", |_| {
                Ok(
                    Representation::new(ReprType::new("text/plain"), b"hello, world".to_vec())
                        .cacheable(),
                )
            }),
        )
        .bind(Exact::new("urn:file"), answers("text/plain", seekable))
        .bind(Exact::new("urn:pipe"), answers("text/plain", unseekable))
        .bind(
            Exact::new("urn:live"),
            answers("text/plain", || chunks(&[b"ab", b"cd"])),
        )
        .bind(
            Exact::new("urn:cafe"),
            answers("text/plain", || chunks(&[b"caf\xc3", b"\xa9!"])),
        )
        .bind(
            Exact::new("urn:broken"),
            answers("text/plain", || chunks(&[b"ok", b"\xff"])),
        )
        .bind(
            Exact::new("urn:cut-short"),
            answers("text/plain", || chunks(&[b"caf\xc3"])),
        )
        .bind(
            Exact::new("urn:latin"),
            Answers {
                repr_type: ReprType::new("text/plain").with_param("charset", "iso-8859-1"),
                make: || chunks(&[b"caf\xe9"]),
            },
        );
    (Kernel::new(Arc::new(space)), opened)
}

fn root() -> Capability {
    Capability::root()
}

fn stream(name: &str) -> Request {
    source(&format!("urn:stream:{name}"))
}

fn read(kernel: &Kernel, request: Request) -> Result<Vec<u8>> {
    block_on(kernel.issue(request, &root())).map(|r| r.bytes)
}

#[test]
fn collect_reads_a_streamed_body_into_one_buffer_keeping_its_type() {
    let (kernel, _) = kernel();
    let request = stream("collect").with_arg("content", ArgRef::Reference(iri("urn:live")));
    let answer = block_on(kernel.issue_body(request, &root())).unwrap();
    assert_eq!(answer.repr_type.media_type, "text/plain");
    assert_eq!(answer.bytes.as_bytes().unwrap(), &b"abcd"[..]);
    // Inline content is already a buffer.
    let inline = stream("collect").with_arg("content", inline("xyz"));
    assert_eq!(read(&kernel, inline).unwrap(), b"xyz");
}

#[test]
fn collect_refuses_past_the_runs_byte_budget() {
    let (kernel, _) = kernel();
    let kernel = kernel.with_stream_memory(3, 3);
    let request = stream("collect").with_arg("content", ArgRef::Reference(iri("urn:live")));
    assert!(matches!(
        read(&kernel, request),
        Err(Error::OverBudget { .. })
    ));
}

#[test]
fn text_decodes_utf8_split_across_chunks_and_refuses_what_is_not() {
    let (kernel, _) = kernel();
    let text = |of: &str| stream("text").with_arg("content", ArgRef::Reference(iri(of)));
    let answer = block_on(kernel.issue(text("urn:cafe"), &root())).unwrap();
    assert_eq!(answer.bytes, "café!".as_bytes());
    assert_eq!(answer.repr_type.params.get("charset").unwrap(), "utf-8");
    for bad in ["urn:broken", "urn:cut-short"] {
        match read(&kernel, text(bad)) {
            Err(Error::InvalidArgument { name, .. }) => assert_eq!(name, "content"),
            other => panic!("{bad}: expected InvalidArgument, got {other:?}"),
        }
    }
    // Another charset is refused by name, never guessed at.
    match read(&kernel, text("urn:latin")) {
        Err(Error::Unsupported { detail, .. }) => {
            assert!(detail.contains("iso-8859-1"), "{detail}")
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn text_over_a_replay_stays_a_replay_and_is_never_collected() {
    let (kernel, opened) = kernel();
    let request = stream("text").with_arg("content", ArgRef::Reference(iri("urn:file")));
    let answer = block_on(kernel.issue_body(request, &root())).unwrap();
    assert!(matches!(answer.bytes, Body::Replay(_)));
    assert_eq!(
        opened.load(Ordering::SeqCst),
        0,
        "nothing was read to answer"
    );
    let read = block_on(answer.bytes.collect(64)).unwrap();
    assert_eq!(read, &b"0123456789"[..]);
}

#[test]
fn range_reads_an_offset_window_of_a_seekable_body() {
    let (kernel, opened) = kernel();
    let range = |of: &str, start: &str, end: Option<&str>| {
        let request = stream("range")
            .with_arg("of", ArgRef::Reference(iri(of)))
            .with_arg("start", inline(start));
        match end {
            Some(end) => request.with_arg("end", inline(end)),
            None => request,
        }
    };
    // A buffer is sliced; a seekable replay is opened at the offset.
    assert_eq!(
        read(&kernel, range("urn:doc", "7", Some("12"))).unwrap(),
        b"world"
    );
    assert_eq!(
        read(&kernel, range("urn:file", "3", Some("6"))).unwrap(),
        b"345"
    );
    assert_eq!(opened.load(Ordering::SeqCst), 1);
    // No end reads to the end; an end past it is clamped, as HTTP clamps.
    assert_eq!(read(&kernel, range("urn:file", "8", None)).unwrap(), b"89");
    assert_eq!(
        read(&kernel, range("urn:doc", "7", Some("999"))).unwrap(),
        b"world"
    );
    // A start past the end is the 416 case.
    assert!(matches!(
        read(&kernel, range("urn:file", "10", None)),
        Err(Error::InvalidArgument { .. })
    ));
    // An end before the start is refused.
    assert!(matches!(
        read(&kernel, range("urn:doc", "5", Some("2"))),
        Err(Error::InvalidArgument { .. })
    ));
    // A body that cannot be read from an offset is refused, never collected to slice.
    for unseekable in ["urn:pipe", "urn:live"] {
        assert!(
            matches!(
                read(&kernel, range(unseekable, "1", Some("2"))),
                Err(Error::Unsupported { .. })
            ),
            "{unseekable}"
        );
    }
}

#[test]
fn a_cached_range_hangs_from_its_sources_threads() {
    let (kernel, opened) = kernel();
    let range = || {
        stream("range")
            .with_arg("of", ArgRef::Reference(iri("urn:file")))
            .with_arg("start", inline("2"))
            .with_arg("end", inline("4"))
    };
    assert_eq!(read(&kernel, range()).unwrap(), b"23");
    assert!(kernel.is_cached(&range(), &root()));
    assert_eq!(read(&kernel, range()).unwrap(), b"23");
    assert_eq!(
        opened.load(Ordering::SeqCst),
        1,
        "the second read was a hit"
    );
    kernel.cut("urn:file");
    assert!(
        !kernel.is_cached(&range(), &root()),
        "a cut of `of` cuts its ranges"
    );
}

#[test]
fn no_module_answers_for_a_stream_builtin() {
    // A root that binds `urn:stream:collect` itself is never consulted for it.
    let space = EndpointSpace::new().bind(
        Exact::new("urn:stream:collect"),
        FnEndpoint::new("impostor", |_| {
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"impostor".to_vec(),
            ))
        }),
    );
    let kernel = Kernel::new(Arc::new(space));
    let request = stream("collect").with_arg("content", inline("real"));
    assert_eq!(read(&kernel, request).unwrap(), b"real");
    // Not listed in the catalog yet: every module's conformance suite walks what is
    // listed outside `urn:kernel:`, and these are core's (ledger #2239).
    let listed: Vec<String> = kernel
        .entries()
        .unwrap()
        .into_iter()
        .map(|entry| entry.pattern)
        .collect();
    for name in ["urn:stream:text", "urn:stream:range"] {
        assert!(!listed.iter().any(|p| p == name), "{name} in {listed:?}");
    }
}

/// A transreptor from `text/a` to `text/b` that records the shape of the body it was
/// handed, and upper-cases it.
struct Shout {
    incremental: bool,
    seen: Arc<Mutex<Vec<&'static str>>>,
}

#[async_trait]
impl Endpoint for Shout {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let body = self.invoke_body(inv).await?;
        body.collect_within(inv.memory_budget().unwrap()).await
    }

    async fn invoke_body(&self, inv: &Invocation<'_>) -> Result<Representation<Body>> {
        let body = inv.content_body("content").await?;
        self.seen.lock().unwrap().push(match &body {
            Body::Bytes(_) => "bytes",
            Body::Replay(_) => "replay",
            Body::Once(_) => "once",
        });
        let bytes = inv.collect(body).await?;
        Ok(
            Representation::new(ReprType::new("text/b"), bytes.to_ascii_uppercase())
                .cacheable()
                .into_body(),
        )
    }

    fn describe(&self) -> Description {
        let description = Description::new("shout")
            .transreptor(["text/a"], ["text/b"])
            .verb(Verb::Source)
            .input(ikigai_core::ArgSpec::new("content"))
            .input(ikigai_core::ArgSpec::new("as").optional());
        if self.incremental {
            description.incremental()
        } else {
            description
        }
    }
}

/// A page that runs the `text/a → text/b` plan over the body `make` builds.
fn page<F>(make: F) -> AsyncFnEndpoint
where
    F: Fn() -> Body + Send + Sync + 'static,
{
    let make = Arc::new(make);
    AsyncFnEndpoint::new("page", move |inv| {
        let make = Arc::clone(&make);
        Box::pin(async move {
            let plan = inv.select_transreptor("text/a", "text/b").expect("a plan");
            let input = Representation::from_body(ReprType::new("text/a"), make());
            let out = inv.transrept(input, &plan).await?;
            Ok(Representation::new(
                out.repr_type.clone(),
                inv.collect(out.bytes).await?.to_vec(),
            )
            .cacheable())
        })
    })
}

#[derive(Default)]
struct Collect(Mutex<Vec<TraceEvent>>);
impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn shout_kernel<F>(incremental: bool, make: F) -> (Kernel, Arc<Mutex<Vec<&'static str>>>)
where
    F: Fn() -> Body + Send + Sync + 'static,
{
    let seen = Arc::new(Mutex::new(Vec::new()));
    let shout = Shout {
        incremental,
        seen: Arc::clone(&seen),
    };
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:shout"), shout)
        .bind(Exact::new("urn:page"), page(make));
    (Kernel::new(Arc::new(space)), seen)
}

#[test]
fn a_plan_hands_an_incremental_step_the_stream_and_collects_for_the_rest() {
    let (incremental, seen) = shout_kernel(true, || chunks(&[b"ab", b"c"]));
    assert_eq!(read(&incremental, source("urn:page")).unwrap(), b"ABC");
    assert_eq!(*seen.lock().unwrap(), ["once"], "handed over uncollected");

    let (collecting, seen) = shout_kernel(false, || chunks(&[b"ab", b"c"]));
    assert_eq!(read(&collecting, source("urn:page")).unwrap(), b"ABC");
    assert_eq!(
        *seen.lock().unwrap(),
        ["bytes"],
        "collected first, as before bodies"
    );
}

#[test]
fn a_step_over_a_one_shot_body_is_never_stored_and_says_why() {
    let (kernel, _) = shout_kernel(true, || chunks(&[b"x"]));
    let collect = Arc::new(Collect::default());
    block_on(kernel.issue_traced(source("urn:page"), &root(), collect.clone())).unwrap();
    let events = collect.0.lock().unwrap();
    let step = events
        .iter()
        .find(|e| e.target == "urn:shout")
        .expect("the step ran");
    let note = step
        .notes
        .iter()
        .find(|(k, _)| k == UNCACHED_NOTE)
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| panic!("no uncached note: {:?}", step.notes));
    assert!(note.contains("one-shot content b3:"), "{note}");
    // The page built on it is not stored either.
    assert!(!kernel.is_cached(&source("urn:page"), &root()));
}

#[test]
fn a_step_over_a_replay_with_a_digest_is_cached_on_the_digest() {
    let (body, _) = fixed(b"hello", true);
    let (kernel, seen) = shout_kernel(true, body);
    assert_eq!(read(&kernel, source("urn:page")).unwrap(), b"HELLO");
    assert_eq!(*seen.lock().unwrap(), ["replay"]);
    let step = Request::new(Verb::Source, iri("urn:shout"))
        .with_arg("as", inline("text/b"))
        .with_arg("content", ArgRef::Content(ContentId::of(b"hello")));
    assert!(
        kernel.is_cached(&step, &root()),
        "keyed by the bytes' digest"
    );
}

#[test]
fn attached_content_resolves_only_inside_the_request_that_carries_it() {
    // A request naming content by id, with nothing attached and no content store
    // installed, is refused as Unsupported (tests/content_store.rs covers a store).
    let (kernel, _) = kernel();
    let request = stream("collect").with_arg("content", ArgRef::Content(ContentId::of(b"x")));
    assert!(matches!(
        read(&kernel, request),
        Err(Error::Unsupported { .. })
    ));
}

#[test]
fn incremental_is_absent_from_a_description_that_does_not_declare_it() {
    let plain = Description::new("plain").transreptor(["text/a"], ["text/b"]);
    let json = serde_json::to_string(&plain).unwrap();
    assert!(!json.contains("incremental"), "{json}");
    let declared = plain.clone().incremental();
    let json = serde_json::to_string(&declared).unwrap();
    assert!(json.contains("\"incremental\":true"), "{json}");
    let back: Description = serde_json::from_str(&json).unwrap();
    assert!(back.transreption().unwrap().is_incremental());
}
