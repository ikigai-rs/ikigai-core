//! **The limiter, end to end** — the paper's Definition 7 and §9.6's *difference*,
//! realized as a hit on a kernel-known endpoint (`docs/formalism/README.md` §1,
//! "Limiter (difference)"). One test per claim: a limited name is `Unresolved`,
//! byte-identical to an unbound one, and offered by no manifold; an alias INTO
//! the family is limited under the canonical and one OUT of it resolves; a
//! limiter is a corridor, so it limits at a position; inside a confinement the
//! trace says *limited*, not *scope-unresolved*; the floor never sees ⊥ and
//! nothing is stored; and a governor over a limited family still limits it.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    AliasTable, AsyncFnEndpoint, Capability, Confine, Description, EndpointSpace, Error, Exact,
    Fallback, FnEndpoint, Iri, Kernel, Limit, MetaRenderer, ReprType, Representation, Request,
    Resolution, Rewrite, Scope, Space, TraceEvent, Tracer, Verb, DENIED_NOTE, LIMITED_NOTE,
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

fn meta(target: &str) -> Request {
    Request::new(Verb::Meta, iri(target))
}

/// A cacheable constant with a description id of its own, counting invocations.
fn door(id: &'static str, hits: &Arc<AtomicU32>) -> FnEndpoint {
    let hits = Arc::clone(hits);
    FnEndpoint::new(id, move |_inv| {
        hits.fetch_add(1, Ordering::SeqCst);
        Ok(text(id.as_bytes()).cacheable())
    })
    .with_description(Description::new(id).verb(Verb::Source))
}

/// Renders a description as its id, so a catalog body is a list of ids.
struct IdRenderer;

impl MetaRenderer for IdRenderer {
    fn render(
        &self,
        description: &Description,
        _target: &ReprType,
    ) -> ikigai_core::Result<Representation> {
        Ok(text(description.id.as_bytes()))
    }
}

/// S: a public door and a personal one. The formalism row's fixture.
fn s(hits: &Arc<AtomicU32>) -> Arc<dyn Space> {
    Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:public:y"), door("public-y", hits))
            .bind(Exact::new("urn:personal:x"), door("personal-x", hits)),
    )
}

/// `Fallback([Limit("urn:personal:"), S])` — S minus the personal family.
fn limited(hits: &Arc<AtomicU32>) -> Arc<dyn Space> {
    Arc::new(Fallback::new(vec![
        Arc::new(Limit::new("urn:personal:")),
        s(hits),
    ]))
}

fn kernel(root: Arc<dyn Space>) -> Kernel {
    Kernel::with_meta_renderer(root, Arc::new(IdRenderer))
}

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

// ---- 1. The formalism row ---------------------------------------------------

#[test]
fn a_limiter_carves_a_family_out_of_the_chain_and_out_of_every_manifold() {
    let hits = Arc::new(AtomicU32::new(0));
    let kernel = kernel(limited(&hits));
    let cap = Capability::root();

    // Unresolved for the family; the endpoint behind the limiter is never entered.
    let err = block_on(kernel.issue(source("urn:personal:x"), &cap)).unwrap_err();
    assert!(
        matches!(err, Error::Unresolved(ref t) if t.as_str() == "urn:personal:x"),
        "got {err:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "⊥ must stop resolution");
    // A name outside the family resolves through S unchanged.
    assert_eq!(
        block_on(kernel.issue(source("urn:public:y"), &cap))
            .unwrap()
            .bytes,
        b"public-y"
    );
    assert_eq!(kernel.cache_len(), 1, "only the public read is stored");

    // Absent from the catalog and from the action manifold; the public door is in both.
    for face in ["urn:kernel:catalog", "urn:kernel:actions"] {
        let body = block_on(kernel.issue(source(face), &cap)).unwrap();
        let body = String::from_utf8(body.bytes).unwrap();
        assert!(
            !body.contains("personal"),
            "`{face}` offered a limited name:\n{body}"
        );
        assert!(
            body.contains("public"),
            "`{face}` lost the public door:\n{body}"
        );
    }
    // Describe-by-IRI agrees with the faces.
    assert!(kernel.describe(&iri("urn:personal:x")).is_none());
    assert!(kernel.describe(&iri("urn:public:y")).is_some());

    // The probe: never cached — nothing was stored, and nothing ever will be.
    assert!(!kernel.is_cached(&source("urn:personal:x"), &cap));

    // Meta on a limited name is unresolved too: describing a hole would reveal it.
    let err = block_on(kernel.issue(meta("urn:personal:x"), &cap)).unwrap_err();
    assert!(matches!(err, Error::Unresolved(_)), "got {err:?}");
    assert_eq!(
        block_on(kernel.issue(meta("urn:public:y"), &cap))
            .unwrap()
            .bytes,
        b"public-y"
    );
}

// ---- 2. Indistinguishable from no door anywhere -------------------------------

#[test]
fn a_limited_name_answers_exactly_as_an_unbound_name_does() {
    // §9.6's remark, pinned as text: the two errors differ only in the name asked
    // for. And the floor never runs — a bound-but-refused door would be `Denied`,
    // which reveals a binding; the personal door here demands a scope the caller
    // lacks, and the caller still learns nothing.
    let gated = FnEndpoint::new("personal-x", |_| Ok(text(b"x"))).with_description(
        Description::new("personal-x")
            .verb(Verb::Source)
            .requires("urn:cap:personal:read"),
    );
    let kernel = kernel(Arc::new(Fallback::new(vec![
        Arc::new(Limit::new("urn:personal:")),
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:personal:x"), gated)),
    ])));
    let narrow = Capability::root().attenuate(["urn:cap:other"]);

    let limited = block_on(kernel.issue(source("urn:personal:x"), &narrow)).unwrap_err();
    let unbound = block_on(kernel.issue(source("urn:nowhere:x"), &narrow)).unwrap_err();
    assert!(matches!(limited, Error::Unresolved(_)), "got {limited:?}");
    assert!(matches!(unbound, Error::Unresolved(_)), "got {unbound:?}");
    assert_eq!(
        limited
            .to_string()
            .replace("urn:personal:x", "urn:nowhere:x"),
        unbound.to_string(),
        "a limited name must not be distinguishable by the error's text"
    );
    assert_eq!(kernel.cache_len(), 0, "nothing stored for either");

    // The ONE place the difference shows: the trace, and only when one is installed.
    let events = Arc::new(Collect::default());
    let _ = block_on(kernel.issue_traced(source("urn:personal:x"), &narrow, events.clone()));
    let _ = block_on(kernel.issue_traced(source("urn:nowhere:x"), &narrow, events.clone()));
    let events = events.0.lock().unwrap();
    assert_eq!(
        events.len(),
        1,
        "a plain miss in the empty chain records nothing"
    );
    let event = &events[0];
    assert_eq!(event.target, "urn:personal:x");
    assert_eq!(note(event, LIMITED_NOTE), Some("urn:personal:x"));
    assert_eq!(note(event, DENIED_NOTE), None, "not a denial");
    assert_eq!(
        note(event, SCOPE_NOTE),
        None,
        "the empty chain carries no scope"
    );
    assert!(event.started.is_none() && event.ended.is_none() && !event.cache_hit);
}

// ---- 3. Rewrites into and out of the family -----------------------------------

#[test]
fn an_alias_into_a_limited_family_is_limited_under_the_canonical_and_one_out_of_it_resolves() {
    let hits = Arc::new(AtomicU32::new(0));
    let cap = Capability::root();

    // The kernel's own table: `urn:alias:in` → personal (limited, named by the
    // canonical); `urn:personal:out` → public (rewritten before the limiter sees it).
    let aliased = kernel(limited(&hits)).with_aliases(Arc::new(
        AliasTable::new()
            .exact("urn:alias:in", "urn:personal:x")
            .exact("urn:personal:out", "urn:public:y"),
    ));
    let err = block_on(aliased.issue(source("urn:alias:in"), &cap)).unwrap_err();
    assert!(
        matches!(err, Error::Unresolved(ref t) if t.as_str() == "urn:personal:x"),
        "limited under the canonical name: {err:?}"
    );
    assert_eq!(
        block_on(aliased.issue(source("urn:personal:out"), &cap))
            .unwrap()
            .bytes,
        b"public-y",
        "a rewrite OUT of the family runs before the limiter and resolves"
    );

    // A `Rewrite` composed by hand under the limiter reports its canonical, and the
    // kernel adopts it before asking `is_limiter`: limited under the canonical.
    let rewritten = kernel(Arc::new(Rewrite::new(limited(&hits), |target: &Iri| {
        target
            .as_str()
            .strip_prefix("urn:logical:")
            .map(|rest| iri(&format!("urn:personal:{rest}")))
    })));
    let err = block_on(rewritten.issue(source("urn:logical:x"), &cap)).unwrap_err();
    assert!(
        matches!(err, Error::Unresolved(ref t) if t.as_str() == "urn:personal:x"),
        "got {err:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1, "only the public read ran");
}

// ---- 4. A limiter at a position ------------------------------------------------

#[test]
fn a_limiter_injected_as_a_corridor_limits_under_that_scope_only() {
    let hits = Arc::new(AtomicU32::new(0));
    // The ROOT binds the personal door plainly; the corridor is the only limiter.
    let reader = AsyncFnEndpoint::new("reader", |inv| {
        Box::pin(async move { inv.source(&iri("urn:personal:x")).await })
    });
    let kernel = kernel(Arc::new(Fallback::new(vec![
        Arc::new(EndpointSpace::new().bind_arc(Exact::new("urn:read"), Arc::new(reader))),
        s(&hits),
    ])));
    let cap = Capability::root();
    let no_personal = || {
        Scope::empty().with_named(
            iri("urn:ctx:no-personal"),
            Arc::new(Limit::new("urn:personal:")) as Arc<dyn Space>,
        )
    };

    // Under the root: resolves. Under the corridor: unresolved — directly, and
    // through a sub-request from an endpoint running in that chain.
    assert!(block_on(kernel.issue(source("urn:personal:x"), &cap)).is_ok());
    let err = block_on(kernel.issue_in(source("urn:personal:x"), &cap, no_personal())).unwrap_err();
    assert!(matches!(err, Error::Unresolved(_)), "got {err:?}");
    let err = block_on(kernel.issue_in(source("urn:read"), &cap, no_personal())).unwrap_err();
    assert!(matches!(err, Error::Unresolved(_)), "got {err:?}");
    // …and the root is untouched by what a corridor limited.
    assert!(block_on(kernel.issue(source("urn:read"), &cap)).is_ok());
    assert!(block_on(kernel.issue(source("urn:public:y"), &cap)).is_ok());
    assert!(!kernel.is_cached_in(&source("urn:personal:x"), &cap, &no_personal()));
    assert!(kernel.is_cached(&source("urn:personal:x"), &cap));
}

#[test]
fn inside_a_confinement_a_limited_name_is_traced_as_limited_not_scope_unresolved() {
    let hits = Arc::new(AtomicU32::new(0));
    // The confinement's space is `Fallback([Limit(personal), doc])`; the root
    // binds the personal door. Two different structural facts for two names:
    // `urn:personal:x` has a door in the chain and it is ⊥; `urn:data:other`
    // has no door in the chain at all.
    let confined_space: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        Arc::new(Limit::new("urn:personal:")),
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:doc:1"), door("doc", &hits))),
    ]));
    let extractor = AsyncFnEndpoint::new("extract", |inv| {
        Box::pin(async move {
            let limited = inv.source(&iri("urn:personal:x")).await.unwrap_err();
            let outside = inv.source(&iri("urn:data:other")).await.unwrap_err();
            assert!(matches!(limited, Error::Unresolved(_)));
            assert!(matches!(outside, Error::Unresolved(_)));
            inv.source(&iri("urn:doc:1")).await
        })
    });
    let kernel = kernel(Arc::new(Fallback::new(vec![
        Arc::new(EndpointSpace::new().bind_arc(
            Exact::new("urn:extract"),
            Arc::new(Confine::new(
                iri("urn:ctx:doc:1"),
                confined_space,
                Arc::new(extractor),
            )),
        )),
        s(&hits),
    ])));
    let cap = Capability::root();

    let events = Arc::new(Collect::default());
    block_on(kernel.issue_traced(source("urn:extract"), &cap, events.clone())).unwrap();
    let events = events.0.lock().unwrap();
    let limited = events
        .iter()
        .find(|e| e.target == "urn:personal:x")
        .expect("the limited sub-request is traced");
    assert_eq!(note(limited, LIMITED_NOTE), Some("urn:personal:x"));
    assert_eq!(note(limited, SCOPE_NOTE), Some("urn:ctx:doc:1 severed"));
    assert_eq!(
        note(limited, SCOPE_MISS_NOTE),
        None,
        "the chain HAS a door for it — the door is ⊥ — which is not a scope miss"
    );
    let outside = events
        .iter()
        .find(|e| e.target == "urn:data:other")
        .expect("the confined miss is traced, as before");
    assert_eq!(note(outside, SCOPE_MISS_NOTE), Some("urn:data:other"));
    assert_eq!(note(outside, LIMITED_NOTE), None);
}

// ---- 5. A governor over a limited family still limits it ---------------------

#[test]
fn a_governor_over_a_limited_family_still_limits_it() {
    // The interception-overlay shape (`ikigai-throttle`): resolve through, then
    // decorate the endpoint with a type of the governor's own. That type has
    // never heard of limiters — its `is_limiter()` is the default — so if the
    // decoration reached ⊥, every governor would un-limit whatever it fronted.
    struct Governor<S: Space>(S, Arc<AtomicU32>);
    impl<S: Space> Space for Governor<S> {
        fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
            let wrapped = Arc::clone(&self.1);
            self.0
                .resolve(request, scope)
                .map_endpoint(move |endpoint| {
                    wrapped.fetch_add(1, Ordering::SeqCst);
                    Arc::new(Confine::new(
                        iri("urn:ctx:governed"),
                        Arc::new(EndpointSpace::new()),
                        endpoint,
                    )) // any decorating endpoint type will do; `Confine` is one core ships
                })
        }
        fn entries(&self) -> Option<Vec<ikigai_core::SpaceEntry>> {
            self.0.entries()
        }
    }
    let hits = Arc::new(AtomicU32::new(0));
    let wrapped = Arc::new(AtomicU32::new(0));
    let kernel = kernel(Arc::new(Governor(limited(&hits), Arc::clone(&wrapped))));
    let cap = Capability::root();

    let err = block_on(kernel.issue(source("urn:personal:x"), &cap)).unwrap_err();
    assert!(
        matches!(err, Error::Unresolved(_)),
        "un-limited by decoration: {err:?}"
    );
    assert_eq!(
        wrapped.load(Ordering::SeqCst),
        0,
        "⊥ was handed to the wrapper"
    );
    // The public door IS governed.
    assert!(block_on(kernel.issue(source("urn:public:y"), &cap)).is_ok());
    assert_eq!(wrapped.load(Ordering::SeqCst), 1);
    // And the manifold under the governor still subtracts the family.
    let body = block_on(kernel.issue(source("urn:kernel:actions"), &cap)).unwrap();
    let body = String::from_utf8(body.bytes).unwrap();
    assert!(!body.contains("personal"), "{body}");
}
