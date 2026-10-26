//! **The content store** (ledger #2240, stage 4 of the streams design): `ArgRef::Content`
//! resolves inside the invocation that carries it, through the kernel's content store,
//! and a request carrying one is a pure value keyed by the digest.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, AsyncFnEndpoint, Capability, ContentId, EndpointSpace, Error, Exact, FnEndpoint, Iri,
    Kernel, MemoryContentStore, ReprType, Representation, Request, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

/// A cacheable Source that answers the length of its `content` body, counting runs.
fn length(runs: Arc<AtomicUsize>) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("length", move |inv| {
        let runs = Arc::clone(&runs);
        Box::pin(async move {
            runs.fetch_add(1, Ordering::SeqCst);
            let body = inv.content_body("content").await?;
            let n = inv.collect(body).await?.len();
            Ok(
                Representation::new(ReprType::new("text/plain"), n.to_string().into_bytes())
                    .cacheable(),
            )
        })
    })
}

fn kernel(store: Option<Arc<MemoryContentStore>>) -> (Kernel, Arc<AtomicUsize>) {
    let runs = Arc::new(AtomicUsize::new(0));
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:length"), length(Arc::clone(&runs)))
        .bind(
            Exact::new("urn:inline-only"),
            FnEndpoint::new("inline-only", |inv| {
                let bytes = inv.inline_arg("content")?.to_vec();
                Ok(Representation::new(ReprType::new("text/plain"), bytes))
            }),
        );
    let kernel = Kernel::new(Arc::new(space));
    let kernel = match store {
        Some(store) => kernel.with_content_store(store),
        None => kernel,
    };
    (kernel, runs)
}

fn length_of(id: ContentId) -> Request {
    Request::new(Verb::Source, iri("urn:length")).with_arg("content", ArgRef::Content(id))
}

#[test]
fn a_content_id_resolves_through_the_store_and_caches_on_the_digest() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, runs) = kernel(Some(Arc::clone(&store)));
    let held = store.put(vec![1u8; 4096]).unwrap();
    let root = Capability::root();
    let answer = block_on(kernel.issue(length_of(held.id()), &root)).unwrap();
    assert_eq!(answer.bytes, b"4096");
    // The same digest is the same request: a hit, not a second read of the store.
    assert!(kernel.is_cached(&length_of(held.id()), &root));
    block_on(kernel.issue(length_of(held.id()), &root)).unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    // And the request's identity names the digest, not the bytes.
    assert_eq!(held.id(), ContentId::of(&[1u8; 4096]));
}

#[test]
fn a_retried_request_reads_its_content_again() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, runs) = kernel(Some(Arc::clone(&store)));
    let held = store.put(b"again".to_vec()).unwrap();
    let sink = || {
        Request::new(Verb::Sink, iri("urn:length")).with_arg("content", ArgRef::Content(held.id()))
    };
    for _ in 0..3 {
        assert_eq!(
            block_on(kernel.issue(sink(), &Capability::root()))
                .unwrap()
                .bytes,
            b"5"
        );
    }
    assert_eq!(
        runs.load(Ordering::SeqCst),
        3,
        "a Sink is never cached; each read the store"
    );
}

#[test]
fn no_store_is_unsupported_and_an_id_it_does_not_hold_is_not_found() {
    let (bare, _) = kernel(None);
    let id = ContentId::of(b"nothing");
    assert!(matches!(
        block_on(bare.issue(length_of(id), &Capability::root())),
        Err(Error::Unsupported { .. })
    ));
    let (stored, _) = kernel(Some(Arc::new(MemoryContentStore::new(16))));
    assert!(matches!(
        block_on(stored.issue(length_of(id), &Capability::root())),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn inline_arg_keeps_refusing_content_it_cannot_lend_as_bytes() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, _) = kernel(Some(Arc::clone(&store)));
    let held = store.put(b"stored".to_vec()).unwrap();
    let request = Request::new(Verb::Source, iri("urn:inline-only"))
        .with_arg("content", ArgRef::Content(held.id()));
    match block_on(kernel.issue(request, &Capability::root())) {
        Err(Error::InvalidArgument { name, detail }) => {
            assert_eq!(name, "content");
            assert!(detail.contains("content_body"), "{detail}");
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn the_stream_builtins_read_stored_content() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, _) = kernel(Some(Arc::clone(&store)));
    let held = store.put("caf\u{e9}".as_bytes().to_vec()).unwrap();
    let text = Request::new(Verb::Source, iri("urn:stream:text"))
        .with_arg("content", ArgRef::Content(held.id()));
    assert_eq!(
        block_on(kernel.issue(text, &Capability::root()))
            .unwrap()
            .bytes,
        "caf\u{e9}".as_bytes()
    );
}

#[test]
fn content_dropped_from_the_store_no_longer_resolves() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, _) = kernel(Some(Arc::clone(&store)));
    let held = store.put(b"brief".to_vec()).unwrap();
    let id = held.id();
    drop(held);
    assert!(store.is_empty());
    let sink = Request::new(Verb::Sink, iri("urn:length")).with_arg("content", ArgRef::Content(id));
    assert!(matches!(
        block_on(kernel.issue(sink, &Capability::root())),
        Err(Error::NotFound(_))
    ));
}
