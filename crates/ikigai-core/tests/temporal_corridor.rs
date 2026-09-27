//! **The first temporal corridor, end to end.** The chain reaches every face the
//! kernel answers a question about resolution on — selection, the cache probe,
//! the pipe — and carries a clock derived from the corridor that pins time
//! (ledger #516, #517; `docs/design/resolution-scope.md`; `docs/formalism/README.md`
//! §7). The first test is the arc's deliverable: an endpoint that reads time by
//! resolution AND by `inv.now()` sees one instant under a named temporal corridor
//! and the live one under the root; the pinned read is cacheable and the live one
//! is not; two corridors are two entries; and a confinement's manifold offers only
//! what its chain resolves. The rest pin each face on its own.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, AsyncFnEndpoint, Capability, Clock, Confine, Description, EndpointSpace, Error, Exact,
    Expiry, FixedClock, FnEndpoint, Invocation, Iri, Issuer, Kernel, MetaRenderer, Provenance,
    ReprType, Representation, Request, Scope, Space, Thread, Time, TraceEvent, Tracer, Verb,
    SCOPE_CLOCK_NOTE, SCOPE_NOTE,
};

/// The kernel's own clock: "live" for these tests.
const LIVE: u64 = 2_000_000;
/// The instant the first temporal corridor pins.
const PINNED: u64 = 1_000_000;
/// The instant a second corridor pins.
const LATER: u64 = 3_000_000;

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(bytes: &[u8]) -> Representation {
    Representation::new(ReprType::new("text/plain"), bytes.to_vec())
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn millis(time: Option<Time>) -> String {
    time.map(|t| t.as_millis().to_string())
        .unwrap_or_else(|| "none".to_string())
}

/// `urn:time:now` as the ROOT binds it: the live clock, read through the
/// invocation. Uncacheable (the default `Always`): a live door.
fn live_now() -> FnEndpoint {
    FnEndpoint::new("now-live", |inv: &Invocation<'_>| {
        Ok(text(millis(inv.now()).as_bytes()))
    })
}

/// `urn:time:now` as a temporal corridor binds it: one instant, a pure function
/// of the corridor's context — cacheable `Never`.
fn pinned_now(at: u64) -> FnEndpoint {
    FnEndpoint::new("now-pinned", move |_: &Invocation<'_>| {
        Ok(text(at.to_string().as_bytes()).cacheable())
    })
}

/// An action selectable when a `urn:class:Doc` is present.
fn doc_action(id: &'static str) -> FnEndpoint {
    FnEndpoint::new(id, |_: &Invocation<'_>| Ok(text(b"summary"))).with_description(
        Description::new(id)
            .verb(Verb::Source)
            .input(ikigai_core::ArgSpec::new("doc").class("urn:class:Doc")),
    )
}

/// The doors a temporal corridor holds: the pinned `urn:time:now`, and one
/// as-of action so a confinement's manifold has something to offer.
fn corridor(at: u64) -> Arc<dyn Space> {
    Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:time:now"), pinned_now(at))
            .bind(
                Exact::new("urn:as-of:summarize"),
                doc_action("as-of-summarize"),
            ),
    )
}

/// A named temporal corridor: the binding AND the clock derived from it, in one
/// call — the pairing #517 requires.
fn temporal(name: &str, at: u64) -> Scope {
    Scope::empty().with_named_at(iri(name), corridor(at), Arc::new(FixedClock::at(at)))
}

/// The endpoint under test: reads time by RESOLUTION and by the CLOCK, and lists
/// the actions its chain offers for a document. Cacheable on its own account, so
/// its effective expiry is whatever its time door's is.
fn probe(computed: &Arc<AtomicU32>) -> AsyncFnEndpoint {
    let computed = Arc::clone(computed);
    AsyncFnEndpoint::new("probe", move |inv| {
        let computed = Arc::clone(&computed);
        Box::pin(async move {
            computed.fetch_add(1, Ordering::SeqCst);
            let resolved = inv.source(&iri("urn:time:now")).await?;
            let resolved = String::from_utf8_lossy(&resolved.bytes).into_owned();
            let clock = millis(inv.now());
            let mut offers: Vec<String> = inv
                .select_action(&["urn:class:Doc"])
                .into_iter()
                .map(|m| m.endpoint)
                .collect();
            offers.sort();
            Ok(text(
                format!(
                    "resolved={resolved} clock={clock} offers={}",
                    offers.join(",")
                )
                .as_bytes(),
            )
            .cacheable())
        })
    })
}

struct Fixture {
    kernel: Kernel,
    computed: Arc<AtomicU32>,
}

fn fixture() -> Fixture {
    let computed = Arc::new(AtomicU32::new(0));
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:time:now"), live_now())
            .bind(Exact::new("urn:summarize"), doc_action("summarize"))
            .bind_arc(Exact::new("urn:probe"), Arc::new(probe(&computed)))
            // The same probe, confined to a corridor of its own: inside it the
            // root is gone, so its manifold is whatever the host's corridors and
            // its own box can resolve.
            .bind_arc(
                Exact::new("urn:probe:confined"),
                Arc::new(Confine::new(
                    iri("urn:ctx:box"),
                    Arc::new(EndpointSpace::new()),
                    Arc::new(probe(&computed)),
                )),
            ),
    ))
    .with_clock(Arc::new(FixedClock::at(LIVE)));
    Fixture { kernel, computed }
}

// ---- The deliverable ---------------------------------------------------------

#[test]
fn a_temporal_corridor_pins_both_faces_of_time_and_the_pinned_read_is_cacheable() {
    let Fixture { kernel, computed } = fixture();
    let cap = Capability::root();
    let six_pm = || temporal("urn:ctx:time:2026-09-25T18:00Z", PINNED);
    let nine_am = || temporal("urn:ctx:time:2026-09-26T09:00Z", LATER);

    // Under the root both faces are live, the manifold is the root's, and the
    // read is uncacheable: it depends on a live door.
    let live = block_on(kernel.issue(source("urn:probe"), &cap)).unwrap();
    assert_eq!(
        live.bytes,
        format!("resolved={LIVE} clock={LIVE} offers=urn:summarize").as_bytes()
    );
    assert_eq!(live.expiry, Expiry::Always);
    assert!(!kernel.is_cached(&source("urn:probe"), &cap));
    assert_eq!(computed.load(Ordering::SeqCst), 1);

    // Under the corridor both faces answer the pinned instant — the same instant,
    // whichever seam the endpoint used — the manifold is the chain's (its as-of
    // action AND the root's, since the root is still present), and the read is
    // cacheable `Never`: a pure function of its context.
    let pinned = block_on(kernel.issue_in(source("urn:probe"), &cap, six_pm())).unwrap();
    assert_eq!(
        pinned.bytes,
        format!("resolved={PINNED} clock={PINNED} offers=urn:as-of:summarize,urn:summarize")
            .as_bytes()
    );
    assert_eq!(pinned.expiry, Expiry::Never);
    assert_eq!(computed.load(Ordering::SeqCst), 2);

    // The probe sees it: cached in that chain, not in the empty one.
    assert!(kernel.is_cached_in(&source("urn:probe"), &cap, &six_pm()));
    assert!(!kernel.is_cached(&source("urn:probe"), &cap));

    // A second request under a corridor REBUILT under the same name is a hit.
    let again = block_on(kernel.issue_in(source("urn:probe"), &cap, six_pm())).unwrap();
    assert_eq!(again.bytes, pinned.bytes);
    assert_eq!(computed.load(Ordering::SeqCst), 2, "served from the cache");

    // A differently named corridor is a different context: computed again, and
    // two corridors are two entries (each holds its probe and its pinned door).
    let later = block_on(kernel.issue_in(source("urn:probe"), &cap, nine_am())).unwrap();
    assert!(later
        .bytes
        .starts_with(format!("resolved={LATER} clock={LATER}").as_bytes()));
    assert_eq!(computed.load(Ordering::SeqCst), 3);
    assert!(kernel.is_cached_in(&source("urn:probe"), &cap, &nine_am()));
    assert_eq!(kernel.cache_len(), 4);

    // Confined inside the corridor: the host's pinned time still reaches both
    // faces (the box keeps the host's corridors AND their clock), and the
    // manifold offers only what the severed chain can resolve — the root's
    // action is gone from it, not merely refused.
    let confined = block_on(kernel.issue_in(source("urn:probe:confined"), &cap, six_pm())).unwrap();
    assert_eq!(
        confined.bytes,
        format!("resolved={PINNED} clock={PINNED} offers=urn:as-of:summarize").as_bytes()
    );
}

// ---- Face 1: selection reaches the chain ---------------------------------------

/// A Turtle-only meta renderer: emits `text/turtle` for the canonical request and
/// errors for anything else, which is what sends the kernel to selection.
struct TurtleOnly;
impl MetaRenderer for TurtleOnly {
    fn render(
        &self,
        description: &Description,
        target: &ReprType,
    ) -> Result<Representation, Error> {
        match target.media_type.as_str() {
            "text/turtle" | "*/*" | "" => Ok(Representation::new(
                ReprType::new("text/turtle"),
                format!("<urn:x> ik:id \"{}\" .", description.id).into_bytes(),
            )
            .cacheable()),
            other => Err(Error::Endpoint(format!(
                "unsupported meta target `{other}`"
            ))),
        }
    }
}

/// An auto-invocable `text/turtle → application/rdf+xml` transreptor that tags
/// its output so a test can see WHICH one ran.
fn transreptor(id: &'static str, tag: &'static str) -> FnEndpoint {
    FnEndpoint::new(id, move |inv: &Invocation<'_>| {
        let content = inv.inline_str("content").unwrap_or("");
        Ok(Representation::new(
            ReprType::new("application/rdf+xml"),
            format!("{tag}({content})").into_bytes(),
        )
        .cacheable())
    })
    .with_description(
        Description::new(id)
            .verb(Verb::Source)
            .input(ikigai_core::ArgSpec::new("content"))
            .input(ikigai_core::ArgSpec::new("as"))
            .transreptor(["text/turtle"], ["application/rdf+xml"]),
    )
}

#[test]
fn selection_in_the_empty_scope_is_byte_identical_to_selection_over_the_root() {
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:rdf:transrept"), transreptor("rdf", "ROOT"))
            .bind(Exact::new("urn:summarize"), doc_action("summarize")),
    ));
    let empty = Scope::empty();
    assert_eq!(
        kernel.select_transreptor_in("text/turtle", "application/rdf+xml", &empty),
        kernel.select_transreptor("text/turtle", "application/rdf+xml")
    );
    assert_eq!(
        kernel.select_action_in(&["urn:class:Doc"], &empty),
        kernel.select_action(&["urn:class:Doc"])
    );
    let query = ikigai_core::ActionQuery {
        verb: Some(Verb::Source),
        ..Default::default()
    };
    assert_eq!(
        kernel.select_actions_in(&query, &empty),
        kernel.select_actions(&query)
    );
    assert!(!kernel.select_actions_in(&query, &empty).is_empty());
}

#[test]
fn a_corridor_that_shadows_a_transreptor_is_the_one_the_plan_uses_and_meta_transrepts_through_it() {
    let kernel = Kernel::with_meta_renderer(
        Arc::new(
            EndpointSpace::new()
                .bind(Exact::new("urn:rdf:transrept"), transreptor("rdf", "ROOT"))
                .bind(Exact::new("urn:thing"), doc_action("thing")),
        ),
        Arc::new(TurtleOnly),
    );
    let cap = Capability::root();
    let shadow: Arc<dyn Space> = Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:rdf:transrept"),
        transreptor("rdf-ctx", "CORRIDOR"),
    ));
    let scope = || Scope::empty().with_named(iri("urn:ctx:shadow"), Arc::clone(&shadow));

    // The plan: over the root, the root's; in the chain, the corridor's — the same
    // IRI, consulted innermost first, exactly as resolution would consult it.
    let root_plan = kernel
        .select_transreptor("text/turtle", "application/rdf+xml")
        .unwrap();
    let chain_plan = kernel
        .select_transreptor_in("text/turtle", "application/rdf+xml", &scope())
        .unwrap();
    assert_eq!(root_plan[0].endpoint, "urn:rdf:transrept");
    assert_eq!(chain_plan[0].endpoint, "urn:rdf:transrept");

    // Meta as=application/rdf+xml runs the plan THROUGH the chain: the corridor's
    // transreptor answers inside it, the root's outside.
    let meta = |scope: Scope| {
        let request = Request::new(Verb::Meta, iri("urn:thing"))
            .with_arg("as", ArgRef::Inline(b"application/rdf+xml".to_vec()));
        block_on(kernel.issue_in(request, &cap, scope)).unwrap()
    };
    let outside = meta(Scope::empty());
    let inside = meta(scope());
    assert!(
        String::from_utf8_lossy(&outside.bytes).starts_with("ROOT("),
        "{outside:?}"
    );
    assert!(
        String::from_utf8_lossy(&inside.bytes).starts_with("CORRIDOR("),
        "{inside:?}"
    );

    // A severed chain with no transreptor in it plans NOTHING, though the root
    // has one: the manifold offers only what the chain can resolve, and Meta
    // falls back to the canonical Turtle instead of failing on a plan it could
    // not run.
    let empty_box: Arc<dyn Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:thing"), doc_action("thing")));
    let severed = Scope::empty()
        .with_named(iri("urn:ctx:box"), empty_box)
        .sever();
    assert_eq!(
        kernel.select_transreptor_in("text/turtle", "application/rdf+xml", &severed),
        None
    );
    let fallback = meta(severed);
    assert_eq!(fallback.repr_type.media_type, "text/turtle");
}

#[test]
fn a_confined_endpoints_manifold_never_lists_a_root_only_action_or_transreptor() {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let observer = |seen: &Arc<Mutex<Vec<String>>>| {
        let seen = Arc::clone(seen);
        FnEndpoint::new("observer", move |inv: &Invocation<'_>| {
            let actions: Vec<String> = inv
                .select_action(&["urn:class:Doc"])
                .into_iter()
                .map(|m| m.endpoint)
                .collect();
            let plan = inv
                .select_transreptor("text/turtle", "application/rdf+xml")
                .map(|steps| steps[0].endpoint.clone())
                .unwrap_or_else(|| "none".to_string());
            seen.lock()
                .unwrap()
                .push(format!("{}|{plan}", actions.join(",")));
            Ok(text(b"ok"))
        })
    };
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:rdf:transrept"), transreptor("rdf", "ROOT"))
            .bind(Exact::new("urn:summarize"), doc_action("summarize"))
            .bind(Exact::new("urn:observe"), observer(&seen))
            .bind_arc(
                Exact::new("urn:observe:confined"),
                Arc::new(Confine::new(
                    iri("urn:ctx:box"),
                    Arc::new(EndpointSpace::new().bind(
                        Exact::new("urn:as-of:summarize"),
                        doc_action("as-of-summarize"),
                    )),
                    Arc::new(observer(&seen)),
                )),
            ),
    ));
    let cap = Capability::root();
    block_on(kernel.issue(source("urn:observe"), &cap)).unwrap();
    block_on(kernel.issue(source("urn:observe:confined"), &cap)).unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen[0], "urn:summarize|urn:rdf:transrept",
        "the root's manifold, outside"
    );
    assert_eq!(
        seen[1], "urn:as-of:summarize|none",
        "only the chain's, inside"
    );
}

/// An issuer written before scopes existed: `issue` and nothing else.
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
    fn select_action(&self, present: &[&str]) -> Vec<ikigai_core::ActionMatch> {
        self.0.select_action(present)
    }
    fn select_transreptor(
        &self,
        from: &str,
        to: &str,
    ) -> Option<Vec<ikigai_core::TransreptionStep>> {
        self.0.select_transreptor(from, to)
    }
}

#[test]
fn an_issuer_that_cannot_select_in_a_chain_offers_nothing_rather_than_the_root() {
    let issuer = PlainIssuer(Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:rdf:transrept"), transreptor("rdf", "ROOT"))
            .bind(Exact::new("urn:summarize"), doc_action("summarize")),
    )));
    let request = source("urn:whoever");
    let bindings = Default::default();
    let cap = Capability::root();
    let inv = Invocation::with_issuer(&request, &bindings, &cap, &issuer);

    // The empty chain delegates: the old issuer's manifold, unchanged.
    assert_eq!(inv.select_action(&["urn:class:Doc"]).len(), 1);
    assert!(inv
        .select_transreptor("text/turtle", "application/rdf+xml")
        .is_some());

    // A chain it cannot select in: nothing, never the root's over-offer.
    let corridor: Arc<dyn Space> = Arc::new(EndpointSpace::new());
    let confined = inv.confine(iri("urn:ctx:box"), corridor);
    assert!(confined.select_action(&["urn:class:Doc"]).is_empty());
    assert!(confined
        .select_transreptor("text/turtle", "application/rdf+xml")
        .is_none());
}

// ---- Face 2: the cache probe and the readout -----------------------------------

#[test]
fn is_cached_in_answers_for_the_chain_and_the_readout_names_it() {
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:doc"), pinned_now(7)),
    ));
    let cap = Capability::root();
    let corridor: Arc<dyn Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:doc"), pinned_now(8)));
    let scope = || Scope::empty().with_named(iri("urn:ctx:doc:8"), Arc::clone(&corridor));

    block_on(kernel.issue_in(source("urn:doc"), &cap, scope())).unwrap();
    assert!(kernel.is_cached_in(&source("urn:doc"), &cap, &scope()));
    assert!(
        !kernel.is_cached(&source("urn:doc"), &cap),
        "the empty chain's entry does not exist yet"
    );
    assert!(
        !kernel.is_cached_in(&source("urn:doc"), &cap, &scope().sever()),
        "a different chain is a different entry"
    );
    block_on(kernel.issue(source("urn:doc"), &cap)).unwrap();
    assert!(kernel.is_cached(&source("urn:doc"), &cap));
    assert!(kernel.is_cached_in(&source("urn:doc"), &cap, &Scope::empty()));

    // The readout shows the chain each entry was computed in: the empty one as
    // `root`, a named one by its rendered chain.
    let out = block_on(kernel.issue(source("urn:kernel:cache"), &cap)).unwrap();
    let text = String::from_utf8_lossy(&out.bytes);
    let rows: Vec<&str> = text.lines().filter(|l| l.contains("urn:doc")).collect();
    assert_eq!(rows.len(), 2, "{text}");
    assert!(
        rows.iter().any(|r| r.trim_end().ends_with("  root")),
        "{text}"
    );
    assert!(
        rows.iter()
            .any(|r| r.trim_end().ends_with("urn:ctx:doc:8 root")),
        "{text}"
    );
}

// ---- Face 3: the pipe --------------------------------------------------------------

#[test]
fn a_pipe_stage_resolves_in_the_chain_and_folds_its_upstream() {
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    let cap = Capability::root();
    let corridor: Arc<dyn Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:stage"), pinned_now(1)));
    let scope = Scope::empty().with_named(iri("urn:ctx:pipe"), corridor);
    let upstream = Provenance::new(
        Expiry::Always,
        [Thread::from("urn:upstream")].into_iter().collect(),
    );

    // Outside the chain the stage does not exist; inside it resolves, and the
    // upstream's provenance folds in exactly as it does in the empty chain.
    let err = block_on(kernel.issue_with_incoming(source("urn:stage"), &cap, upstream.clone()))
        .unwrap_err();
    assert!(matches!(err, Error::Unresolved(_)));
    let out = block_on(kernel.issue_with_incoming_in(source("urn:stage"), &cap, upstream, scope))
        .unwrap();
    assert_eq!(out.bytes, b"1");
    assert_eq!(
        out.expiry,
        Expiry::Always,
        "no more cacheable than the pipe's input"
    );
    assert!(out.threads().contains(&Thread::from("urn:upstream")));
}

// ---- The scope-level clock ---------------------------------------------------------

#[test]
fn the_innermost_clock_wins_and_confinement_keeps_it() {
    let doors: Arc<dyn Space> = Arc::new(EndpointSpace::new());
    let outer = Scope::empty().with_named_at(
        iri("urn:ctx:a"),
        Arc::clone(&doors),
        Arc::new(FixedClock::at(1)),
    );
    assert_eq!(outer.now(), Some(Time::from_millis(1)));
    let inner = outer.clone().with_named_at(
        iri("urn:ctx:b"),
        Arc::clone(&doors),
        Arc::new(FixedClock::at(2)),
    );
    assert_eq!(inner.now(), Some(Time::from_millis(2)), "innermost wins");
    // A corridor injected WITHOUT a clock does not unset the chain's.
    let plain = inner
        .clone()
        .with_named(iri("urn:ctx:c"), Arc::clone(&doors));
    assert_eq!(plain.now(), Some(Time::from_millis(2)));
    // Confinement keeps the host's clock, as it keeps the host's corridors.
    assert_eq!(
        inner
            .clone()
            .confined(iri("urn:ctx:box"), Arc::clone(&doors))
            .now(),
        Some(Time::from_millis(2))
    );
    assert_eq!(inner.sever().now(), Some(Time::from_millis(2)));
    assert_eq!(Scope::empty().now(), None);
}

#[test]
fn the_fingerprint_is_the_corridors_name_not_its_clock() {
    let doors: Arc<dyn Space> = Arc::new(EndpointSpace::new());
    let name = || iri("urn:ctx:time:2026-09-25T18:00Z");
    let with_clock =
        Scope::empty().with_named_at(name(), Arc::clone(&doors), Arc::new(FixedClock::at(PINNED)));
    let other_clock =
        Scope::empty().with_named_at(name(), Arc::clone(&doors), Arc::new(FixedClock::at(LATER)));
    let no_clock = Scope::empty().with_named(name(), Arc::clone(&doors));
    assert_eq!(with_clock.fingerprint(), no_clock.fingerprint());
    assert_eq!(with_clock.fingerprint(), other_clock.fingerprint());
    assert_eq!(with_clock.to_string(), no_clock.to_string());
}

/// A clock that can be moved — the kernel's, for the validity tests.
#[derive(Clone)]
struct TestClock(Arc<AtomicU64>);
impl TestClock {
    fn at(millis: u64) -> Self {
        TestClock(Arc::new(AtomicU64::new(millis)))
    }
    fn set(&self, millis: u64) {
        self.0.store(millis, Ordering::SeqCst);
    }
}
impl Clock for TestClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

/// An `At` entry with a FIXED deadline, counting computations.
fn deadline(at: u64, computed: &Arc<AtomicU32>) -> FnEndpoint {
    let computed = Arc::clone(computed);
    FnEndpoint::new("deadline", move |_: &Invocation<'_>| {
        computed.fetch_add(1, Ordering::SeqCst);
        Ok(text(b"v").cacheable_until(Time::from_millis(at)))
    })
}

#[test]
fn validity_is_judged_on_the_kernels_clock_never_the_chains() {
    let computed = Arc::new(AtomicU32::new(0));
    let kernel_clock = TestClock::at(1_000);
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:t"), deadline(1_500, &computed)),
    ))
    .with_clock(Arc::new(kernel_clock.clone()));
    let cap = Capability::root();
    let doors: Arc<dyn Space> = Arc::new(EndpointSpace::new());

    // A pinned PAST cannot un-expire a live entry: the chain says 100, the
    // deadline is 1500, but the kernel's clock has moved to 2000 — recomputed.
    let past = || {
        Scope::empty().with_named_at(
            iri("urn:ctx:past"),
            Arc::clone(&doors),
            Arc::new(FixedClock::at(100)),
        )
    };
    block_on(kernel.issue_in(source("urn:t"), &cap, past())).unwrap();
    assert!(kernel.is_cached_in(&source("urn:t"), &cap, &past()));
    kernel_clock.set(2_000);
    assert!(!kernel.is_cached_in(&source("urn:t"), &cap, &past()));
    block_on(kernel.issue_in(source("urn:t"), &cap, past())).unwrap();
    assert_eq!(
        computed.load(Ordering::SeqCst),
        2,
        "expired by the kernel's clock"
    );

    // A pinned FUTURE cannot expire a fresh one: the chain says 9999, past the
    // deadline, but the kernel's clock is back at 1000 — served.
    kernel_clock.set(1_000);
    let future = || {
        Scope::empty().with_named_at(
            iri("urn:ctx:future"),
            Arc::clone(&doors),
            Arc::new(FixedClock::at(9_999)),
        )
    };
    block_on(kernel.issue_in(source("urn:t"), &cap, future())).unwrap();
    assert_eq!(computed.load(Ordering::SeqCst), 3);
    block_on(kernel.issue_in(source("urn:t"), &cap, future())).unwrap();
    assert_eq!(
        computed.load(Ordering::SeqCst),
        3,
        "fresh by the kernel's clock"
    );
    assert!(kernel.is_cached_in(&source("urn:t"), &cap, &future()));
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

#[test]
fn the_trace_discloses_the_chains_clock_beside_the_chain() {
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    let cap = Capability::root();
    let corridor: Arc<dyn Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:doc"), pinned_now(8)));
    let tracer = Arc::new(Collect::default());
    kernel.set_tracer(tracer.clone());

    // With a clock: the instant, beside the chain. Without: the chain alone.
    let timed = Scope::empty().with_named_at(
        iri("urn:ctx:t"),
        Arc::clone(&corridor),
        Arc::new(FixedClock::at(PINNED)),
    );
    block_on(kernel.issue_in(source("urn:doc"), &cap, timed)).unwrap();
    let untimed = Scope::empty().with_named(iri("urn:ctx:u"), corridor);
    block_on(kernel.issue_in(source("urn:doc"), &cap, untimed)).unwrap();
    let events = tracer.0.lock().unwrap();
    assert_eq!(note(&events[0], SCOPE_NOTE), Some("urn:ctx:t root"));
    assert_eq!(
        note(&events[0], SCOPE_CLOCK_NOTE),
        Some(PINNED.to_string().as_str())
    );
    assert_eq!(note(&events[1], SCOPE_NOTE), Some("urn:ctx:u root"));
    assert_eq!(note(&events[1], SCOPE_CLOCK_NOTE), None);
}
