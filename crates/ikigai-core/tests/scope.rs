//! **The resolution chain, end to end.** One test per decision in
//! `docs/design/resolution-scope.md`: a confined sub-request is *unresolvable*,
//! not denied (root never entered); the cache never shares across a chain
//! boundary in either direction; an injected corridor shadows a root door for
//! the request and its sub-requests; nothing shadows `urn:kernel:*`; the cache
//! partitions by corridor NAME; an endpoint can only narrow its chain; the empty
//! chain is the status quo; the chain is legible in the trace; fan-out stays
//! confined; and an issuer that cannot carry a chain refuses rather than escapes.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, BoxFuture, CacheKey, Capability, Confine, Description, Endpoint,
    EndpointSpace, Error, Exact, FnEndpoint, Invocation, Iri, Issuer, Kernel, ReprType,
    Representation, Request, Scope, Space, Spawner, TraceEvent, Tracer, Verb, DENIED_NOTE,
    SCOPE_MISS_NOTE, SCOPE_NOTE,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(bytes: &[u8]) -> Representation {
    Representation::new(ReprType::new("text/plain"), bytes.to_vec())
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

/// A cacheable constant that counts how often it is actually computed.
fn counted(name: &'static str, body: &'static [u8], hits: &Arc<AtomicU32>) -> FnEndpoint {
    let hits = Arc::clone(hits);
    FnEndpoint::new(name, move |_inv| {
        hits.fetch_add(1, Ordering::SeqCst);
        Ok(text(body).cacheable())
    })
}

/// An endpoint that `source`s one fixed target and returns what it got.
fn sourcer(name: &'static str, target: &'static str) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new(name, move |inv| {
        Box::pin(async move {
            let inner = inv.source(&iri(target)).await?;
            Ok(text(&inner.bytes))
        })
    })
}

fn space_with(
    name: &'static str,
    target: &'static str,
    body: &'static [u8],
    hits: &Arc<AtomicU32>,
) -> Arc<dyn Space> {
    Arc::new(EndpointSpace::new().bind(Exact::new(target), counted(name, body, hits)))
}

// ---- 1. Confinement: unresolvable, never denied --------------------------

#[test]
fn a_confined_sub_request_for_a_root_bound_iri_is_unresolved_and_the_root_endpoint_is_never_entered(
) {
    let secret_hits = Arc::new(AtomicU32::new(0));
    let doc_hits = Arc::new(AtomicU32::new(0));
    let document = space_with("doc", "urn:doc:1", b"the document", &doc_hits);
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(
                Exact::new("urn:data:secret"),
                counted("secret", b"s3cr3t", &secret_hits),
            )
            .bind_arc(
                Exact::new("urn:extract:secret"),
                Arc::new(Confine::new(
                    iri("urn:ctx:doc:1"),
                    Arc::clone(&document),
                    Arc::new(sourcer("extract", "urn:data:secret")),
                )),
            )
            .bind_arc(
                Exact::new("urn:extract:doc"),
                Arc::new(Confine::new(
                    iri("urn:ctx:doc:1"),
                    document,
                    Arc::new(sourcer("extract", "urn:doc:1")),
                )),
            ),
    ));

    // Under ROOT authority — no capability could be lacking — the secret is
    // simply not there: `Unresolved`, naming the target, and the secret endpoint
    // was never entered. There is no decision here to have misconfigured.
    let err =
        block_on(kernel.issue(source("urn:extract:secret"), &Capability::root())).unwrap_err();
    assert!(
        matches!(err, Error::Unresolved(ref t) if t.as_str() == "urn:data:secret"),
        "got {err:?}"
    );
    assert!(!matches!(err, Error::Denied(_)));
    assert_eq!(
        secret_hits.load(Ordering::SeqCst),
        0,
        "the root endpoint must not run"
    );

    // What the corridor binds is reachable, exactly as it would be from the root.
    let out = block_on(kernel.issue(source("urn:extract:doc"), &Capability::root())).unwrap();
    assert_eq!(out.bytes, b"the document");
    assert_eq!(doc_hits.load(Ordering::SeqCst), 1);

    // And from outside, the secret resolves as it always did — confinement is a
    // property of the chain the extractor ran in, not of the secret.
    let out = block_on(kernel.issue(source("urn:data:secret"), &Capability::root())).unwrap();
    assert_eq!(out.bytes, b"s3cr3t");
}

// ---- 2. §9.4: no sharing across the boundary, either direction -----------

/// `urn:shared` is bound in the root AND in the corridor, to different
/// endpoints. A probe confined to the corridor sources it.
struct Shared {
    kernel: Kernel,
    outside: Arc<AtomicU32>,
    inside: Arc<AtomicU32>,
}

fn shared() -> Shared {
    let outside = Arc::new(AtomicU32::new(0));
    let inside = Arc::new(AtomicU32::new(0));
    let corridor = space_with("inside", "urn:shared", b"inside", &inside);
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(
                Exact::new("urn:shared"),
                counted("outside", b"outside", &outside),
            )
            .bind_arc(
                Exact::new("urn:probe"),
                Arc::new(Confine::new(
                    iri("urn:ctx:corridor"),
                    corridor,
                    Arc::new(sourcer("probe", "urn:shared")),
                )),
            ),
    ));
    Shared {
        kernel,
        outside,
        inside,
    }
}

#[test]
fn a_result_computed_outside_confinement_is_not_served_inside_it() {
    let s = shared();
    let cap = Capability::root();
    // Outside first: computed and cached in the empty chain.
    let out = block_on(s.kernel.issue(source("urn:shared"), &cap)).unwrap();
    assert_eq!(out.bytes, b"outside");
    assert_eq!(s.kernel.cache_len(), 1);

    // Inside: the SAME request id, the SAME capability — a different chain, so a
    // different endpoint answers and the cached "outside" is never served.
    let out = block_on(s.kernel.issue(source("urn:probe"), &cap)).unwrap();
    assert_eq!(out.bytes, b"inside");
    assert_eq!(
        s.inside.load(Ordering::SeqCst),
        1,
        "computed inside, not served from outside"
    );
    assert_eq!(s.outside.load(Ordering::SeqCst), 1);
    // Two entries for one request id: one per chain. (The probe itself is
    // `Always`, so it adds none.)
    assert_eq!(s.kernel.cache_len(), 2);

    // A second confined read is a hit on the CONFINED entry.
    block_on(s.kernel.issue(source("urn:probe"), &cap)).unwrap();
    assert_eq!(s.inside.load(Ordering::SeqCst), 1);
    assert_eq!(s.kernel.cache_len(), 2);
}

#[test]
fn a_result_computed_inside_confinement_is_not_served_outside_it() {
    let s = shared();
    let cap = Capability::root();
    // Inside first.
    let out = block_on(s.kernel.issue(source("urn:probe"), &cap)).unwrap();
    assert_eq!(out.bytes, b"inside");
    assert_eq!(s.kernel.cache_len(), 1);

    // Outside: must compute the root's answer, not serve the corridor's.
    let out = block_on(s.kernel.issue(source("urn:shared"), &cap)).unwrap();
    assert_eq!(out.bytes, b"outside");
    assert_eq!(
        s.outside.load(Ordering::SeqCst),
        1,
        "computed outside, not served from inside"
    );
    assert_eq!(s.inside.load(Ordering::SeqCst), 1);
    assert_eq!(s.kernel.cache_len(), 2);
}

// ---- 3. Injection shadows the root, all the way down ---------------------

#[test]
fn an_injected_corridor_shadows_a_root_door_for_the_request_and_for_its_sub_requests() {
    let root_hits = Arc::new(AtomicU32::new(0));
    let corridor_hits = Arc::new(AtomicU32::new(0));
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(
                Exact::new("urn:door"),
                counted("root-door", b"root", &root_hits),
            )
            // A root-bound composite that sources the door.
            .bind(
                Exact::new("urn:composite"),
                sourcer("composite", "urn:door"),
            ),
    ));
    let corridor = space_with("corridor-door", "urn:door", b"corridor", &corridor_hits);
    let cap = Capability::root();
    let injected = || Scope::empty().with_named(iri("urn:ctx:test"), Arc::clone(&corridor));

    // The request itself.
    let out = block_on(kernel.issue_in(source("urn:door"), &cap, injected())).unwrap();
    assert_eq!(out.bytes, b"corridor");
    // …and a sub-request of a ROOT-bound endpoint, reached through the chain.
    let out = block_on(kernel.issue_in(source("urn:composite"), &cap, injected())).unwrap();
    assert_eq!(out.bytes, b"corridor");
    assert_eq!(
        root_hits.load(Ordering::SeqCst),
        0,
        "the shadowed root door never ran"
    );

    // Without the corridor the same requests reach the root — the corridor was a
    // property of those requests, not of the kernel.
    let out = block_on(kernel.issue(source("urn:composite"), &cap)).unwrap();
    assert_eq!(out.bytes, b"root");
    assert_eq!(root_hits.load(Ordering::SeqCst), 1);
}

// ---- 4. `urn:kernel:*` is ahead of the chain ------------------------------

#[test]
fn an_injected_corridor_cannot_shadow_urn_kernel_and_a_severed_chain_still_reaches_it() {
    let fake_hits = Arc::new(AtomicU32::new(0));
    // `urn:kernel:cache` is a live kernel operation that needs no Meta renderer.
    let fake = space_with("fake-cache", "urn:kernel:cache", b"forged", &fake_hits);
    let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:x"),
        counted("x", b"x", &Arc::new(AtomicU32::new(0))),
    )));
    let cap = Capability::root();

    // Injected ahead of the root: still not ahead of the kernel.
    let out = block_on(kernel.issue_in(
        source("urn:kernel:cache"),
        &cap,
        Scope::empty().with_named(iri("urn:ctx:forger"), Arc::clone(&fake)),
    ))
    .unwrap();
    assert_ne!(out.bytes, b"forged");
    assert_eq!(fake_hits.load(Ordering::SeqCst), 0);

    // Severed: the root is gone, the kernel's own namespace is not (it was never
    // in the chain to begin with). Capability-gated as ever.
    let severed = Scope::empty().confined(iri("urn:ctx:forger"), fake);
    assert!(severed.is_severed());
    let out = block_on(kernel.issue_in(source("urn:kernel:cache"), &cap, severed.clone())).unwrap();
    assert_ne!(out.bytes, b"forged");
    assert_eq!(fake_hits.load(Ordering::SeqCst), 0);
    // …while a root name is not reachable from the same chain.
    let err = block_on(kernel.issue_in(source("urn:x"), &cap, severed)).unwrap_err();
    assert!(matches!(err, Error::Unresolved(_)));
}

// ---- 5. The cache partitions by corridor NAME -----------------------------

#[test]
fn two_requests_with_the_same_named_scope_share_one_cache_entry_and_different_names_do_not() {
    let hits = Arc::new(AtomicU32::new(0));
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    let cap = Capability::root();
    // A corridor REBUILT per request — a new `Arc` every time, as a temporal
    // corridor would be — under one name.
    let pinned = || space_with("pinned", "urn:time:now", b"18:00Z", &hits);
    let named = |name: &str| Scope::empty().with_named(iri(name), pinned());

    block_on(kernel.issue_in(source("urn:time:now"), &cap, named("urn:ctx:t1"))).unwrap();
    block_on(kernel.issue_in(source("urn:time:now"), &cap, named("urn:ctx:t1"))).unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "same name ⇒ one computation"
    );
    assert_eq!(kernel.cache_len(), 1);

    block_on(kernel.issue_in(source("urn:time:now"), &cap, named("urn:ctx:t2"))).unwrap();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "a different name is a different chain"
    );
    assert_eq!(kernel.cache_len(), 2);

    // Anonymous corridors never share across requests — only with their own clones.
    let anon = Scope::empty().with(pinned());
    block_on(kernel.issue_in(source("urn:time:now"), &cap, anon.clone())).unwrap();
    block_on(kernel.issue_in(source("urn:time:now"), &cap, anon)).unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 3);
    block_on(kernel.issue_in(source("urn:time:now"), &cap, Scope::empty().with(pinned()))).unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 4);
    assert_eq!(kernel.cache_len(), 4);

    // Chain ORDER is part of the identity: ⟨a, b⟩ and ⟨b, a⟩ resolve differently.
    let a = || pinned();
    let b = || pinned();
    let ab = Scope::empty()
        .with_named(iri("urn:ctx:a"), a())
        .with_named(iri("urn:ctx:b"), b());
    let ba = Scope::empty()
        .with_named(iri("urn:ctx:b"), b())
        .with_named(iri("urn:ctx:a"), a());
    assert_ne!(ab.fingerprint(), ba.fingerprint());
    // And severing is too.
    assert_ne!(ab.fingerprint(), ab.clone().sever().fingerprint());
}

// ---- 6. An endpoint can only narrow its chain -----------------------------

#[test]
fn an_endpoint_can_only_narrow_its_chain() {
    // The API-level half is in the design note: `Invocation` never takes a
    // `Scope` from an endpoint (`with_scope` is crate-private), and the one
    // chain-changing operation it offers, `confine`, severs. This pins what an
    // endpoint can observe of that: confining from inside a confinement cannot
    // reach the root, and neither can an endpoint reached THROUGH the corridor.
    let secret_hits = Arc::new(AtomicU32::new(0));
    // The corridor holds an endpoint that itself reaches for the root.
    let leaky = Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:corridor:leaky"),
        sourcer("leaky", "urn:data:secret"),
    )) as Arc<dyn Space>;
    let nested = AsyncFnEndpoint::new("nested", |inv| {
        Box::pin(async move {
            assert!(inv.scope().is_severed(), "runs inside a confinement");
            // Confining again — with a corridor that binds the secret's NAME —
            // does not bring the root back and does not put this corridor ahead
            // of the one it runs in. The root name stays unresolvable…
            let again = inv.confine(
                iri("urn:ctx:again"),
                Arc::new(EndpointSpace::new().bind(
                    Exact::new("urn:data:secret"),
                    FnEndpoint::new("mine", |_| Ok(text(b"mine"))),
                )),
            );
            assert!(again.scope().is_severed());
            // …except where the NEW corridor binds it: that is the one thing a
            // confinement may add, in the root's old position.
            let mine = again.source(&iri("urn:data:secret")).await?;
            assert_eq!(mine.bytes, b"mine");
            // A name neither corridor binds: nowhere to go.
            let err = again.source(&iri("urn:elsewhere")).await.unwrap_err();
            assert!(matches!(err, Error::Unresolved(_)));
            // Through the outer corridor, an endpoint reaching for the root is
            // confined too: the chain holds under everything below it.
            let err = inv.source(&iri("urn:corridor:leaky")).await.unwrap_err();
            assert!(matches!(err, Error::Unresolved(ref t) if t.as_str() == "urn:data:secret"));
            Ok(text(b"ok"))
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(
                Exact::new("urn:data:secret"),
                counted("secret", b"s3cr3t", &secret_hits),
            )
            .bind_arc(
                Exact::new("urn:run"),
                Arc::new(Confine::new(iri("urn:ctx:outer"), leaky, Arc::new(nested))),
            ),
    ));
    let out = block_on(kernel.issue(source("urn:run"), &Capability::root())).unwrap();
    assert_eq!(out.bytes, b"ok");
    assert_eq!(secret_hits.load(Ordering::SeqCst), 0);

    // At the top level, the chain is the empty one: nothing injected, root present.
    let top = AsyncFnEndpoint::new("top", |inv| {
        Box::pin(async move {
            assert!(inv.scope().is_empty());
            assert_eq!(inv.scope().to_string(), "root");
            Ok(text(b"top"))
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:top"), top),
    ));
    block_on(kernel.issue(source("urn:top"), &Capability::root())).unwrap();
}

// ---- 7. The empty chain is the status quo ---------------------------------

#[test]
fn the_empty_scope_is_the_status_quo() {
    let empty = Scope::empty();
    assert!(empty.is_empty());
    assert!(!empty.is_severed());
    assert_eq!(empty.fingerprint(), 0);
    assert_eq!(empty.to_string(), "root");
    // A key built the old way IS the empty chain's key.
    let id = source("urn:x").id();
    assert_eq!(
        CacheKey::new(id, 7),
        CacheKey::new(id, 7).in_scope(empty.fingerprint())
    );
    assert_eq!(CacheKey::new(id, 7).scope, 0);
    // And a non-empty chain is not it.
    let space: Arc<dyn Space> = Arc::new(EndpointSpace::new());
    assert_ne!(
        Scope::empty()
            .with_named(iri("urn:ctx:a"), space)
            .fingerprint(),
        0
    );
    assert_ne!(Scope::empty().sever().fingerprint(), 0);
    assert!(!Scope::empty().sever().is_empty());
}

// ---- 8. The capability floor runs against whichever endpoint answered -----

#[test]
fn the_capability_floor_is_evaluated_on_the_corridor_endpoint_that_answered() {
    // Root binds an open door; the corridor shadows it with a gated one.
    let gated = FnEndpoint::new("gated", |_| Ok(text(b"gated").cacheable())).with_description(
        Description::new("gated")
            .verb(Verb::Source)
            .requires("urn:cap:demo:read"),
    );
    let corridor: Arc<dyn Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:door"), gated));
    let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:door"),
        FnEndpoint::new("open", |_| Ok(text(b"open"))),
    )));
    let narrow = Capability::root().attenuate(["urn:cap:other"]);
    let injected = || Scope::empty().with_named(iri("urn:ctx:gate"), Arc::clone(&corridor));

    // Plain chain: open door, no floor.
    assert_eq!(
        block_on(kernel.issue(source("urn:door"), &narrow))
            .unwrap()
            .bytes,
        b"open"
    );
    // Injected: the corridor answered, and ITS declaration is the floor.
    let err = block_on(kernel.issue_in(source("urn:door"), &narrow, injected())).unwrap_err();
    assert!(matches!(err, Error::Denied(_)), "got {err:?}");
    // Root authority computes and caches it in the chain…
    block_on(kernel.issue_in(source("urn:door"), &Capability::root(), injected())).unwrap();
    // …and the floor still fences that cached entry from the narrow caller.
    let err = block_on(kernel.issue_in(source("urn:door"), &narrow, injected())).unwrap_err();
    assert!(matches!(err, Error::Denied(_)));
}

// ---- 9. The chain is legible in the trace ---------------------------------

#[derive(Default)]
struct Collect(Mutex<Vec<TraceEvent>>);
impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn note<'a>(event: &'a TraceEvent, key: &str) -> Option<&'a str> {
    event
        .notes
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

#[test]
fn the_chain_is_disclosed_on_every_traced_event_and_a_confined_miss_is_traced() {
    let hits = Arc::new(AtomicU32::new(0));
    let corridor = space_with("doc", "urn:doc:1", b"the document", &hits);
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(
                Exact::new("urn:data:secret"),
                counted("secret", b"s", &hits),
            )
            .bind_arc(
                Exact::new("urn:extract"),
                Arc::new(Confine::new(
                    iri("urn:ctx:doc:1"),
                    Arc::clone(&corridor),
                    Arc::new(sourcer("extract", "urn:data:secret")),
                )),
            ),
    ));
    let cap = Capability::root();

    // A confined miss: the extractor's event carries no scope (it ran in the
    // empty chain); its sub-request's MISS is recorded — it would otherwise be
    // invisible — with the chain and the name that had no binding there.
    let events = Arc::new(Collect::default());
    let _ = block_on(kernel.issue_traced(source("urn:extract"), &cap, events.clone()));
    let events = events.0.lock().unwrap();
    let miss = events
        .iter()
        .find(|e| e.target == "urn:data:secret")
        .expect("the miss inside the confinement is traced");
    assert_eq!(note(miss, SCOPE_NOTE), Some("urn:ctx:doc:1 severed"));
    assert_eq!(note(miss, SCOPE_MISS_NOTE), Some("urn:data:secret"));
    assert_eq!(note(miss, DENIED_NOTE), None, "not a denial");
    assert!(miss.started.is_none() && !miss.cache_hit, "nothing ran");
    // (The extractor's own event is absent: a failed invocation records no event,
    // as before.) A successful read in the empty chain carries no scope note.
    drop(events);
    let events = Arc::new(Collect::default());
    block_on(kernel.issue_traced(source("urn:data:secret"), &cap, events.clone())).unwrap();
    let events = events.0.lock().unwrap();
    let plain = events
        .iter()
        .find(|e| e.target == "urn:data:secret")
        .unwrap();
    assert_eq!(
        note(plain, SCOPE_NOTE),
        None,
        "the empty chain adds no note"
    );
    drop(events);

    // An injected chain: computed event and cache-hit event both carry it.
    let events = Arc::new(Collect::default());
    let injected = || Scope::empty().with_named(iri("urn:ctx:doc:1"), Arc::clone(&corridor));
    block_on(kernel.issue_traced(source("urn:doc:1"), &cap, events.clone())).unwrap_err();
    block_on(kernel.issue_in(source("urn:doc:1"), &cap, injected())).unwrap();
    let tracer = Arc::new(Collect::default());
    kernel.set_tracer(tracer.clone());
    block_on(kernel.issue_in(source("urn:doc:1"), &cap, injected())).unwrap();
    let events = tracer.0.lock().unwrap();
    let hit = events.iter().find(|e| e.target == "urn:doc:1").unwrap();
    assert!(hit.cache_hit);
    assert_eq!(note(hit, SCOPE_NOTE), Some("urn:ctx:doc:1 root"));
    drop(events);

    // A plain miss in the empty chain still records nothing.
    let tracer = Arc::new(Collect::default());
    kernel.set_tracer(tracer.clone());
    block_on(kernel.issue(source("urn:nothing"), &cap)).unwrap_err();
    assert!(tracer.0.lock().unwrap().is_empty());
}

// ---- 10. Fan-out stays confined --------------------------------------------

struct InlineSpawner;
impl Spawner for InlineSpawner {
    fn spawn(&self, task: BoxFuture<()>) -> BoxFuture<()> {
        task
    }
}

#[test]
fn fan_out_from_a_confined_endpoint_stays_confined() {
    let secret_hits = Arc::new(AtomicU32::new(0));
    let doc_hits = Arc::new(AtomicU32::new(0));
    let corridor = space_with("doc", "urn:doc:1", b"the document", &doc_hits);
    let fanner = AsyncFnEndpoint::new("fanner", |inv| {
        Box::pin(async move {
            let results = inv
                .fan_out(vec![source("urn:doc:1"), source("urn:data:secret")])
                .await;
            assert_eq!(results[0].as_ref().unwrap().bytes, b"the document");
            assert!(
                matches!(results[1], Err(Error::Unresolved(_))),
                "{:?}",
                results[1]
            );
            Ok(text(b"ok"))
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(
                Exact::new("urn:data:secret"),
                counted("secret", b"s", &secret_hits),
            )
            .bind_arc(
                Exact::new("urn:run"),
                Arc::new(Confine::new(
                    iri("urn:ctx:doc:1"),
                    corridor,
                    Arc::new(fanner),
                )),
            ),
    ))
    .into_scheduled(Arc::new(InlineSpawner));
    block_on(kernel.issue(source("urn:run"), &Capability::root())).unwrap();
    assert_eq!(secret_hits.load(Ordering::SeqCst), 0);
    assert_eq!(doc_hits.load(Ordering::SeqCst), 1);
}

// ---- 11. An issuer that cannot carry the chain refuses --------------------

/// An issuer written before scopes existed: it implements `issue` and nothing
/// else — the shape of every module host bridge in the ecosystem today.
struct PlainIssuer(Kernel);
#[async_trait::async_trait]
impl Issuer for PlainIssuer {
    async fn issue(
        &self,
        request: Request,
        capability: &Capability,
    ) -> Result<Representation, Error> {
        self.0.issue(request, capability).await
    }
}

#[test]
fn an_issuer_that_cannot_carry_the_chain_refuses_a_non_empty_scope_rather_than_escaping_it() {
    let secret_hits = Arc::new(AtomicU32::new(0));
    let issuer = PlainIssuer(Kernel::new(Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:data:secret"),
        counted("secret", b"s", &secret_hits),
    ))));
    let request = source("urn:whoever");
    let bindings = Default::default();
    let cap = Capability::root();
    let inv = Invocation::with_issuer(&request, &bindings, &cap, &issuer);

    // The empty chain delegates: nothing changes for an old issuer.
    assert_eq!(
        block_on(inv.source(&iri("urn:data:secret"))).unwrap().bytes,
        b"s"
    );

    // A confined chain through it would be resolved in the plain root — the
    // confinement silently not holding. Refused instead, by name.
    let empty_corridor: Arc<dyn Space> = Arc::new(EndpointSpace::new());
    let confined = inv.confine(iri("urn:ctx:box"), empty_corridor);
    let err = block_on(confined.source(&iri("urn:data:secret"))).unwrap_err();
    assert!(
        matches!(err, Error::Endpoint(ref m) if m.contains("urn:ctx:box severed") && m.contains("cannot honour")),
        "got {err:?}"
    );
    assert_eq!(
        secret_hits.load(Ordering::SeqCst),
        1,
        "only the empty-chain read ran"
    );
}

// ---- 12. Confine offers the inner contract -----------------------------------

#[test]
fn confine_describes_and_names_as_its_inner_endpoint() {
    let inner = FnEndpoint::new("inner", |_| Ok(text(b"x"))).with_description(
        Description::new("inner")
            .verb(Verb::Source)
            .requires("urn:cap:inner"),
    );
    let space: Arc<dyn Space> = Arc::new(EndpointSpace::new());
    let confined = Confine::new(iri("urn:ctx:c"), Arc::clone(&space), Arc::new(inner));
    assert_eq!(confined.name(), "inner");
    assert_eq!(confined.describe().id, "inner");
    let chain = confined.chain_from(&Scope::empty());
    assert!(chain.is_severed());
    assert_eq!(chain.to_string(), "urn:ctx:c severed");
    // Confining inside an injected chain keeps the host's corridor AHEAD.
    let chain = confined.chain_from(&Scope::empty().with_named(iri("urn:ctx:host"), space));
    assert_eq!(chain.to_string(), "urn:ctx:host urn:ctx:c severed");
}
