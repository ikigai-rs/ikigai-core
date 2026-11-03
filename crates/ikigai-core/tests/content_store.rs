//! **The content store** (ledger #2240, stage 4 of the streams design): `ArgRef::Content`
//! resolves inside the invocation that carries it, through the kernel's content store,
//! and a request carrying one is a pure value keyed by the digest.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    principal_scope, ArgRef, AsyncFnEndpoint, Body, Capability, ContentId, ContentStore,
    EndpointSpace, Error, Exact, FnEndpoint, Iri, Kernel, MemoryContentStore, ReprType,
    Representation, Request, Verb,
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

// ---------------------------------------------------------- whose content it is (ledger #2965)

fn session() -> Capability {
    Capability::scoped(["urn:cap:read"])
}

fn alice() -> Capability {
    session()
        .with_principal("urn:example:person:alice")
        .unwrap()
}

fn bob() -> Capability {
    session().with_principal("urn:example:person:bob").unwrap()
}

/// `urn:stream:collect` answers the content's own bytes, with nothing to bind or grant: what a
/// wire peer can name on any kernel with a store.
fn collect(id: ContentId) -> Request {
    Request::new(Verb::Source, iri("urn:stream:collect")).with_arg("content", ArgRef::Content(id))
}

/// The bytes `capability` reads under `id`, or the refusal's text.
fn read(kernel: &Kernel, id: ContentId, capability: &Capability) -> Result<Vec<u8>, String> {
    block_on(kernel.issue(collect(id), capability))
        .map(|answer| answer.bytes)
        .map_err(|e| e.to_string())
}

fn absent(id: ContentId) -> Result<Vec<u8>, String> {
    Err(format!("not found: no content is held under {id}"))
}

#[test]
fn a_foreign_content_id_answers_exactly_as_an_absent_one() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, _) = kernel(Some(Arc::clone(&store)));
    let held = store.put_for(&alice(), b"alice's".to_vec()).unwrap();

    assert_eq!(read(&kernel, held.id(), &alice()), Ok(b"alice's".to_vec()));
    // Bob cannot read it, and cannot learn it is here: the words are those for no content.
    assert_eq!(read(&kernel, held.id(), &bob()), absent(held.id()));
    let nothing = ContentId::of(b"nothing");
    assert_eq!(read(&kernel, nothing, &bob()), absent(nothing));
    // Through an endpoint, by the seam every reader uses, the same.
    assert!(matches!(
        block_on(kernel.issue(length_of(held.id()), &bob())),
        Err(Error::NotFound(_))
    ));
    // Root is the owner of the kernel, and reads it.
    assert_eq!(
        read(&kernel, held.id(), &Capability::root()),
        Ok(b"alice's".to_vec())
    );
}

#[test]
fn a_principal_reads_its_own_content_whatever_else_it_holds() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, _) = kernel(Some(Arc::clone(&store)));
    let held = store.put_for(&alice(), b"alice's".to_vec()).unwrap();
    let wider = Capability::scoped(["urn:cap:read", "urn:cap:write"])
        .with_principal("urn:example:person:alice")
        .unwrap();
    assert_eq!(read(&kernel, held.id(), &wider), Ok(b"alice's".to_vec()));
}

#[test]
fn a_cached_answer_on_foreign_content_does_not_reach_another_principal() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, runs) = kernel(Some(Arc::clone(&store)));
    let held = store.put_for(&alice(), vec![1u8; 64]).unwrap();
    assert_eq!(
        block_on(kernel.issue(length_of(held.id()), &alice()))
            .unwrap()
            .bytes,
        b"64"
    );
    assert!(kernel.is_cached(&length_of(held.id()), &alice()));
    assert!(!kernel.is_cached(&length_of(held.id()), &bob()));
    assert!(matches!(
        block_on(kernel.issue(length_of(held.id()), &bob())),
        Err(Error::NotFound(_))
    ));
    assert_eq!(
        runs.load(Ordering::SeqCst),
        2,
        "bob's was no hit on alice's"
    );
}

#[test]
fn content_put_for_nobody_is_read_by_root_alone() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, _) = kernel(Some(Arc::clone(&store)));
    let held = store.put(b"the host's".to_vec()).unwrap();
    assert_eq!(
        read(&kernel, held.id(), &Capability::root()),
        Ok(b"the host's".to_vec())
    );
    for capability in [alice(), session(), Capability::scoped(Vec::<String>::new())] {
        assert_eq!(read(&kernel, held.id(), &capability), absent(held.id()));
    }
}

#[test]
fn a_hold_under_a_capability_naming_nobody_is_read_by_root_alone() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, runs) = kernel(Some(Arc::clone(&store)));
    let held = store.put_for(&session(), vec![1u8; 64]).unwrap();
    let wider = Capability::scoped(["urn:cap:read", "urn:cap:write"]);
    let narrower = wider.attenuate(["urn:cap:write"]);
    // The deliverer included: nothing tells it from any other holder of the same scopes.
    for capability in [session(), wider, narrower, alice()] {
        assert_eq!(read(&kernel, held.id(), &capability), absent(held.id()));
    }
    // Root's cached answer stays in root's partition.
    let root = Capability::root();
    assert_eq!(
        block_on(kernel.issue(length_of(held.id()), &root))
            .unwrap()
            .bytes,
        b"64"
    );
    assert!(kernel.is_cached(&length_of(held.id()), &root));
    assert!(!kernel.is_cached(&length_of(held.id()), &session()));
    assert!(matches!(
        block_on(kernel.issue(length_of(held.id()), &session())),
        Err(Error::NotFound(_))
    ));
    assert_eq!(runs.load(Ordering::SeqCst), 2, "the session's was no hit");

    // And a principal's content is not read by its grant without the name.
    let named = store.put_for(&alice(), b"alice's".to_vec()).unwrap();
    assert_eq!(read(&kernel, named.id(), &session()), absent(named.id()));
}

#[test]
fn a_peer_cannot_clamp_its_way_into_a_hold_that_names_nobody() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, _) = kernel(Some(Arc::clone(&store)));
    // A door admitting principal-less callers: each peer resolves under a clamp of its ceiling.
    let door = Capability::scoped(["urn:cap:read", "urn:cap:write"]);
    let deliverer = door.clamp(&session());
    let peer = door.clamp(&session());
    assert_eq!(deliverer, peer, "narrowed to the same scopes");

    let held = store.put_for(&deliverer, b"a peer's".to_vec()).unwrap();
    assert_eq!(read(&kernel, held.id(), &peer), absent(held.id()));
    assert_eq!(read(&kernel, held.id(), &door), absent(held.id()));
    assert_eq!(
        read(&kernel, held.id(), &Capability::root()),
        Ok(b"a peer's".to_vec())
    );
}

#[test]
fn anonymous_callers_share_no_holds() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, _) = kernel(Some(Arc::clone(&store)));
    // What a web door with no principal mints: every anonymous caller the same capability.
    let anonymous = || Capability::scoped(["urn:cap:upload"]);
    let first = store.put_for(&anonymous(), b"first".to_vec()).unwrap();
    let second = store.put_for(&anonymous(), b"second".to_vec()).unwrap();
    assert_eq!(read(&kernel, first.id(), &anonymous()), absent(first.id()));
    assert_eq!(
        read(&kernel, second.id(), &anonymous()),
        absent(second.id())
    );
    // The same bytes delivered by both widen nothing.
    let again = store.put_for(&anonymous(), b"first".to_vec()).unwrap();
    assert_eq!(read(&kernel, again.id(), &anonymous()), absent(again.id()));
}

#[test]
fn a_capability_naming_several_principals_names_nobody() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, _) = kernel(Some(Arc::clone(&store)));
    let several = Capability::scoped([
        principal_scope("urn:example:person:alice").unwrap(),
        principal_scope("urn:example:person:bob").unwrap(),
    ]);
    assert_eq!(several.principal(), None);
    let held = store.put_for(&several, b"whose?".to_vec()).unwrap();
    let hers = store.put_for(&alice(), b"alice's".to_vec()).unwrap();
    assert_eq!(read(&kernel, hers.id(), &several), absent(hers.id()));
    for capability in [several, alice(), bob()] {
        assert_eq!(read(&kernel, held.id(), &capability), absent(held.id()));
    }
}

#[test]
fn the_same_principal_reads_under_a_narrower_capability() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, _) = kernel(Some(Arc::clone(&store)));
    let alice_scope = principal_scope("urn:example:person:alice").unwrap();
    let full = Capability::scoped(["urn:cap:read", "urn:cap:write"])
        .with_principal("urn:example:person:alice")
        .unwrap();
    let held = store.put_for(&full, b"alice's".to_vec()).unwrap();

    // Attenuated, asking to keep the principal.
    let narrower = full.attenuate(["urn:cap:read".to_string(), alice_scope.clone()]);
    assert_ne!(narrower, full);
    assert_eq!(read(&kernel, held.id(), &narrower), Ok(b"alice's".to_vec()));
    // Down to the name alone: the hold asks who, not what.
    let name_only = full.attenuate([alice_scope]);
    assert_eq!(
        read(&kernel, held.id(), &name_only),
        Ok(b"alice's".to_vec())
    );
    // Clamped by a door: the ceiling's principal survives what the peer carried.
    let clamped = full.clamp(&session());
    assert_eq!(clamped.principal(), Some("urn:example:person:alice"));
    assert_eq!(read(&kernel, held.id(), &clamped), Ok(b"alice's".to_vec()));
    // Delivered under the narrower, read under the wider: one party either way.
    let delivered = store.put_for(&narrower, b"narrow".to_vec()).unwrap();
    assert_eq!(read(&kernel, delivered.id(), &full), Ok(b"narrow".to_vec()));

    // A narrowing that does not ask for the principal drops it, and fails closed.
    let nameless = full.attenuate(["urn:cap:read"]);
    assert_eq!(nameless.principal(), None);
    assert_eq!(read(&kernel, held.id(), &nameless), absent(held.id()));
}

/// An endpoint that reads `id` in a sub-request: under `scopes` when given, else under its
/// caller's own capability.
fn relay(id: ContentId, scopes: Option<Vec<String>>) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("relay", move |inv| {
        let scopes = scopes.clone();
        Box::pin(async move {
            let answer = match scopes {
                Some(scopes) => inv.issue_attenuated(collect(id), scopes).await?,
                None => inv.issue(collect(id)).await?,
            };
            Ok(Representation::new(
                ReprType::new("text/plain"),
                answer.bytes,
            ))
        })
    })
}

#[test]
fn a_sub_request_reads_its_callers_content_only_when_it_keeps_the_principal() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let held = store.put_for(&alice(), b"alice's".to_vec()).unwrap();
    let id = held.id();
    let alice_scope = principal_scope("urn:example:person:alice").unwrap();
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:relay:as-caller"), relay(id, None))
        .bind(
            Exact::new("urn:relay:keeps-principal"),
            relay(id, Some(vec!["urn:cap:read".to_string(), alice_scope])),
        )
        .bind(
            Exact::new("urn:relay:drops-principal"),
            relay(id, Some(vec!["urn:cap:read".to_string()])),
        );
    let kernel = Kernel::new(Arc::new(space)).with_content_store(store.clone());
    let call = |name: &str, capability: &Capability| {
        block_on(kernel.issue(Request::new(Verb::Source, iri(name)), capability))
            .map(|answer| answer.bytes)
    };

    assert_eq!(call("urn:relay:as-caller", &alice()).unwrap(), b"alice's");
    assert_eq!(
        call("urn:relay:keeps-principal", &alice()).unwrap(),
        b"alice's"
    );
    assert!(matches!(
        call("urn:relay:drops-principal", &alice()),
        Err(Error::NotFound(_))
    ));
    // The scopes name alice, and bob still cannot read through them: a narrowing adds no name.
    for name in ["urn:relay:as-caller", "urn:relay:keeps-principal"] {
        assert!(matches!(call(name, &bob()), Err(Error::NotFound(_))));
    }
}

#[test]
fn two_deliverers_of_the_same_bytes_each_read_while_their_own_handle_lives() {
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, _) = kernel(Some(Arc::clone(&store)));
    let hers = store.put_for(&alice(), b"same".to_vec()).unwrap();
    let his = store.put_for(&bob(), b"same".to_vec()).unwrap();
    let id = hers.id();
    assert_eq!((store.len(), store.held_bytes()), (1, 4), "one entry");

    assert_eq!(read(&kernel, id, &alice()), Ok(b"same".to_vec()));
    assert_eq!(read(&kernel, id, &bob()), Ok(b"same".to_vec()));

    // A clone is one more hold for the same party.
    let hers_again = hers.clone();
    drop(hers);
    assert_eq!(read(&kernel, id, &alice()), Ok(b"same".to_vec()));
    drop(hers_again);
    // Asked of the store: her earlier answer stays cached in her own partition, as any
    // answer does, and it is the store's hold this is about.
    assert!(store.get_for(&id, &alice()).is_none(), "her hold is gone");
    assert!(store.get_for(&id, &bob()).is_some());

    drop(his);
    assert!(store.is_empty());
    assert!(store.get_for(&id, &bob()).is_none());
}

#[test]
fn put_replay_for_holds_a_spill_for_its_deliverer() {
    use ikigai_core::{ByteStream, Bytes, Replay};
    struct Spilled;
    impl Replay for Spilled {
        fn open(&self) -> ByteStream {
            Box::pin(futures::stream::iter([Ok(Bytes::from_static(b"spilled"))]))
        }
        fn len_hint(&self) -> Option<u64> {
            Some(7)
        }
        fn validator(&self) -> Option<ContentId> {
            Some(ContentId::of(b"spilled"))
        }
    }
    let store = Arc::new(MemoryContentStore::new(1 << 20));
    let (kernel, _) = kernel(Some(Arc::clone(&store)));
    let held = store.put_replay_for(&alice(), Spilled).unwrap();
    assert_eq!(read(&kernel, held.id(), &alice()), Ok(b"spilled".to_vec()));
    assert_eq!(read(&kernel, held.id(), &bob()), absent(held.id()));
    let host = store.put_replay(Spilled).unwrap();
    drop(held);
    assert!(
        store.get_for(&host.id(), &alice()).is_none(),
        "the host's hold is not hers"
    );
    assert!(store.get_for(&host.id(), &Capability::root()).is_some());
}

#[test]
fn a_store_that_keeps_no_record_of_holders_is_read_by_root_alone() {
    struct Plain(MemoryContentStore);
    impl ContentStore for Plain {
        fn get(&self, id: &ContentId) -> Option<Body> {
            self.0.get(id)
        }
    }
    let inner = MemoryContentStore::new(1 << 20);
    let held = inner.put(b"a host library's".to_vec()).unwrap();
    let kernel =
        Kernel::new(Arc::new(EndpointSpace::new())).with_content_store(Arc::new(Plain(inner)));
    assert_eq!(
        read(&kernel, held.id(), &Capability::root()),
        Ok(b"a host library's".to_vec())
    );
    assert_eq!(read(&kernel, held.id(), &alice()), absent(held.id()));
}

#[test]
fn no_store_stays_unsupported_for_every_capability() {
    let (bare, _) = kernel(None);
    assert!(matches!(
        block_on(bare.issue(collect(ContentId::of(b"x")), &alice())),
        Err(Error::Unsupported { .. })
    ));
}
