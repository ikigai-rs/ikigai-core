//! **Why was this read NOT cached?** (NetKernel News 2.47.) Effective expiry
//! propagates from dependencies, so an expensive cached composite that gains one
//! uncacheable source becomes uncacheable itself — the cms-web books graph went from
//! ~20 µs to ~1 s a read that way, and nothing said so. The kernel now names the
//! cause on the computed event (`UNCACHED_NOTE`) and remembers it, with no tracer
//! installed, in `urn:kernel:uncached`. What these pin: the cms-web shape names its
//! source, every cause in the vocabulary is reported as documented, a cached read
//! says nothing, and the readout is bounded, ordered and never records itself.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, CacheBound, CachePolicy, Capability, Description, EndpointSpace, EntryFacts,
    Error, Exact, Expiry, FixedClock, FnEndpoint, Iri, Kernel, Provenance, ReprType,
    Representation, Request, Scope, Space, Time, TraceEvent, Tracer, UriTemplate, Verb,
    UNCACHED_NOTE,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(body: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec())
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

#[derive(Default)]
struct Collect(Mutex<Vec<TraceEvent>>);
impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// Issue `target` traced under `capability`, and return the `uncached` note on its
/// own event — `None` when the kernel stored it (or served it from the cache).
fn why_as(kernel: &Kernel, target: &str, capability: &Capability) -> Option<String> {
    let collect = Arc::new(Collect::default());
    block_on(kernel.issue_traced(source(target), capability, collect.clone())).unwrap();
    let events = collect.0.lock().unwrap();
    let event = events.iter().rev().find(|e| e.target == target).unwrap();
    event
        .notes
        .iter()
        .find(|(k, _)| k == UNCACHED_NOTE)
        .map(|(_, v)| v.clone())
}

fn why(kernel: &Kernel, target: &str) -> Option<String> {
    why_as(kernel, target, &Capability::root())
}

fn readout(kernel: &Kernel) -> String {
    let body = block_on(kernel.issue(source("urn:kernel:uncached"), &Capability::root()))
        .unwrap()
        .bytes;
    String::from_utf8(body).unwrap()
}

/// A composite that sources each of `targets`, tolerating failures (a fallback on
/// a refusal is exactly how a volatile dependency hides), and declares itself
/// cacheable.
fn composite(targets: &'static [&'static str]) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("composite", move |inv| {
        Box::pin(async move {
            let mut out = Vec::new();
            for target in targets {
                match inv.source(&iri(target)).await {
                    Ok(r) => out.push(String::from_utf8_lossy(&r.bytes).into_owned()),
                    Err(_) => out.push("fallback".to_string()),
                }
            }
            Ok(text(&out.join(" + ")).cacheable())
        })
    })
}

fn live(name: &'static str) -> FnEndpoint {
    FnEndpoint::new(name, move |_| Ok(text(name)))
}

fn constant(name: &'static str) -> FnEndpoint {
    FnEndpoint::new(name, move |_| Ok(text(name).cacheable()))
}

/// The cms-web shape: the books graph declares itself cacheable and joins one live
/// overlay.
fn cms_web() -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:tags:overlay"), live("overlay"))
            .bind(Exact::new("urn:books:catalog"), constant("catalog"))
            .bind(
                Exact::new("urn:books:graph"),
                composite(&["urn:books:catalog", "urn:tags:overlay"]),
            ),
    ))
}

#[test]
fn a_cached_composite_joined_to_a_volatile_source_names_the_source() {
    let kernel = cms_web();
    assert_eq!(
        why(&kernel, "urn:books:graph").as_deref(),
        Some("dependency urn:tags:overlay")
    );
    // Every read recomputes — the regression — and every read says why.
    assert_eq!(
        why(&kernel, "urn:books:graph").as_deref(),
        Some("dependency urn:tags:overlay")
    );
    assert!(!kernel.is_cached(&source("urn:books:graph"), &Capability::root()));
    // The source it names says why IT is volatile; the cacheable part says nothing.
    assert_eq!(
        why(&kernel, "urn:tags:overlay").as_deref(),
        Some("declared")
    );
    assert_eq!(why(&kernel, "urn:books:catalog"), None);

    // And with no tracer: the readout names both, the composite with its count.
    let body = readout(&kernel);
    let graph = body
        .lines()
        .find(|l| l.trim_start().starts_with("urn:books:graph"))
        .unwrap_or_else(|| panic!("no books row in:\n{body}"));
    assert!(graph.contains("×2"), "{graph}");
    assert!(graph.contains("dependency urn:tags:overlay"), "{graph}");
    assert!(graph.ends_with("[root]"), "{graph}");
    let overlay = body
        .lines()
        .find(|l| l.trim_start().starts_with("urn:tags:overlay"))
        .unwrap();
    assert!(overlay.contains("declared"), "{overlay}");
    assert!(!body.contains("urn:books:catalog"), "{body}");
}

#[test]
fn a_read_the_kernel_stores_carries_no_note_and_leaves_the_readout_empty() {
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:a"), constant("a"))
            .bind(Exact::new("urn:ab"), composite(&["urn:a"])),
    ));
    assert_eq!(why(&kernel, "urn:ab"), None);
    // Served from the cache: still nothing to say.
    assert_eq!(why(&kernel, "urn:ab"), None);
    assert_eq!(
        readout(&kernel),
        "uncached (why a computed read was not cached; the last 64, most recent first)\n  \
         (nothing computed uncached)\n"
    );
}

#[test]
fn an_endpoint_that_declared_none_and_joined_a_volatile_source_is_told_both() {
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:tags:overlay"), live("overlay"))
            .bind(
                Exact::new("urn:page"),
                AsyncFnEndpoint::new("page", |inv| {
                    Box::pin(async move {
                        inv.source(&iri("urn:tags:overlay")).await?;
                        inv.source(&iri("urn:tags:overlay")).await?;
                        Ok(text("page"))
                    })
                }),
            ),
    ));
    // Named once, however many times it was read.
    assert_eq!(
        why(&kernel, "urn:page").as_deref(),
        Some("declared; dependency urn:tags:overlay")
    );
}

#[test]
fn a_refused_or_failed_dependency_is_named_and_a_miss_is_not() {
    let gated = FnEndpoint::new("gated", |_| Ok(text("secret").cacheable())).with_description(
        Description::new("gated")
            .verb(Verb::Source)
            .requires("urn:cap:demo:read"),
    );
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:gated"), gated)
            .bind(
                Exact::new("urn:boom"),
                FnEndpoint::new("boom", |_| Err(Error::Endpoint("boom".into()))),
            )
            .bind(
                Exact::new("urn:mixed"),
                composite(&["urn:gated", "urn:boom", "urn:nowhere"]),
            )
            .bind(Exact::new("urn:missing"), composite(&["urn:nowhere"])),
    ));
    let narrow = Capability::root().attenuate(["urn:cap:other"]);
    assert_eq!(
        why_as(&kernel, "urn:mixed", &narrow).as_deref(),
        Some("denied urn:gated; failed urn:boom")
    );
    // A miss hangs a thread (a later Sink that creates it cuts the composite) and
    // does not make the result volatile: stored, nothing to say.
    assert_eq!(why(&kernel, "urn:missing"), None);
    assert!(kernel.is_cached(&source("urn:missing"), &Capability::root()));
}

#[test]
fn past_eight_names_the_rest_are_counted_not_dropped() {
    let mut space =
        EndpointSpace::new().bind(UriTemplate::parse("urn:v:{n}").unwrap(), live("volatile"));
    space = space.bind(
        Exact::new("urn:fan"),
        AsyncFnEndpoint::new("fan", |inv| {
            Box::pin(async move {
                for n in 0..10 {
                    inv.source(&iri(&format!("urn:v:{n}"))).await?;
                }
                Ok(text("fan").cacheable())
            })
        }),
    );
    let kernel = Kernel::new(Arc::new(space));
    assert_eq!(
        why(&kernel, "urn:fan").as_deref(),
        Some(
            "dependency urn:v:0 urn:v:1 urn:v:2 urn:v:3 urn:v:4 urn:v:5 urn:v:6 urn:v:7; \
             +2 more"
        )
    );
}

#[test]
fn a_volatile_piped_input_is_named_upstream() {
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:upper"), constant("upper")),
    ));
    let collect = Arc::new(Collect::default());
    kernel.set_tracer(collect.clone());
    block_on(kernel.issue_with_incoming(
        source("urn:upper"),
        &Capability::root(),
        Provenance::new(Expiry::Always, BTreeSet::new()),
    ))
    .unwrap();
    kernel.clear_tracer();
    let events = collect.0.lock().unwrap();
    let note = events[0]
        .notes
        .iter()
        .find(|(k, _)| k == UNCACHED_NOTE)
        .map(|(_, v)| v.as_str());
    assert_eq!(note, Some("upstream"));
}

fn deadline(at: u64) -> FnEndpoint {
    FnEndpoint::new("deadline", move |_| {
        Ok(text("until").with_expiry(Expiry::At(Time(at))))
    })
}

#[test]
fn a_deadline_with_no_clock_or_already_past_is_named_and_a_future_one_is_stored() {
    let space = || {
        Arc::new(
            EndpointSpace::new()
                .bind(Exact::new("urn:past"), deadline(5_000))
                .bind(Exact::new("urn:future"), deadline(20_000)),
        )
    };
    // No clock: nothing could ever tell when it expired.
    let clockless = Kernel::new(space());
    assert_eq!(why(&clockless, "urn:future").as_deref(), Some("no-clock"));
    // A clock at 10 s: 5 s is already past — not filed only to be evicted unread —
    // and 20 s is stored with nothing to say.
    let kernel = Kernel::new(space()).with_clock(Arc::new(FixedClock::at(10_000)));
    assert_eq!(why(&kernel, "urn:past").as_deref(), Some("expired 5000"));
    assert!(!kernel.is_cached(&source("urn:past"), &Capability::root()));
    assert_eq!(why(&kernel, "urn:future"), None);
    assert!(kernel.is_cached(&source("urn:future"), &Capability::root()));
    assert_eq!(kernel.cache_len(), 1);
}

#[test]
fn a_cut_while_computing_and_a_policy_refusal_are_named() {
    // The endpoint cuts the thread its own answer hangs from while it runs: the
    // answer is stale on arrival, and the store declines it.
    let handle: Arc<OnceLock<Weak<Kernel>>> = Arc::new(OnceLock::new());
    let racy = {
        let handle = Arc::clone(&handle);
        FnEndpoint::new("racy", move |_| {
            let kernel = handle.get().unwrap().upgrade().unwrap();
            kernel.cut("urn:racy");
            Ok(text("racy").cacheable())
        })
    };
    let kernel = Arc::new(Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:racy"), racy),
    )));
    handle.set(Arc::downgrade(&kernel)).unwrap();
    assert_eq!(why(&kernel, "urn:racy").as_deref(), Some("cut-in-flight"));

    struct AdmitNothing;
    impl CachePolicy for AdmitNothing {
        fn capacity(&self) -> CacheBound {
            CacheBound::default()
        }
        fn admit(&self, _: &EntryFacts<'_>) -> bool {
            false
        }
        fn victim(&self, _: &[EntryFacts<'_>]) -> Option<usize> {
            None
        }
    }
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:a"), constant("a")),
    ))
    .with_cache_policy(Arc::new(AdmitNothing));
    assert_eq!(why(&kernel, "urn:a").as_deref(), Some("policy"));
}

#[test]
fn the_readout_names_the_chain_is_bounded_and_never_records_a_kernel_operation() {
    let space: Arc<dyn Space> = Arc::new(
        EndpointSpace::new().bind(UriTemplate::parse("urn:live:{n}").unwrap(), live("live")),
    );
    let kernel = Kernel::new(space);
    // A row computed inside a corridor names its chain.
    let corridor = Scope::empty().with_named(
        iri("urn:ctx:game:7"),
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:live:game"), live("game"))),
    );
    block_on(kernel.issue_in(source("urn:live:game"), &Capability::root(), corridor)).unwrap();
    assert!(
        readout(&kernel).contains("declared  [urn:ctx:game:7 root]"),
        "{}",
        readout(&kernel)
    );
    // Seventy distinct resources: the log keeps the last sixty-four, newest first.
    for n in 0..70 {
        block_on(kernel.issue(source(&format!("urn:live:{n}")), &Capability::root())).unwrap();
    }
    let body = readout(&kernel);
    let rows: Vec<&str> = body.lines().skip(1).collect();
    assert_eq!(rows.len(), 64, "{body}");
    assert!(rows[0].trim_start().starts_with("urn:live:69 "), "{body}");
    assert!(rows[63].trim_start().starts_with("urn:live:6 "), "{body}");
    assert!(!body.contains("urn:live:game"), "the oldest row went first");
    // Reading the readout (and the cache readout) is not itself remembered.
    assert!(!body.contains("urn:kernel:"), "{body}");
}
