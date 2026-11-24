//! **A `Meta` can answer an error instead of its description** (ledger #4079, the core half
//! of #4050): `Endpoint::meta_failure`.
//!
//! Before this, the kernel answered every `Meta` from `Endpoint::describe()`, which cannot
//! fail, so a mount whose peer was unreachable could only describe itself as a placeholder
//! that "declares no arguments", and nothing could retry it. The reproduction (the first
//! test, as it stood on main at d57fde6) got that placeholder back as a success. What these
//! pin:
//!
//! 1. a `Meta` of an endpoint answering `meta_failure() = Some(Unavailable)` answers
//!    `Err(Unavailable)`, in the JSON face and the text face, and traces it as a failure;
//! 2. the floor still answers first: a caller it refuses on a verb it gates is `Denied`
//!    there, and a caller an existence-sensitive endpoint hides from gets the miss on its
//!    `Meta`, never the failure (the floor never gates a `Meta` itself, so no `Meta` is
//!    `Denied`);
//! 3. `Kernel::describe`, the catalog and `urn:kernel:actions` still read `describe()`;
//! 4. the failure is not cached, and no description cached earlier is served while it is
//!    answered: the next `Meta` asks again, and succeeds once the endpoint answers `None`;
//! 5. core's public wrappers of one endpoint (`Confine`, a `Threaded` space's hit) pass it
//!    through (the crate-private ones are a unit test in `kernel.rs`);
//! 6. `Source`, `Exists`, `Sink` and `Delete` are unaffected.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::executor::block_on;
use ikigai_core::{
    ActionQuery, ArgRef, Capability, Confine, Description, Endpoint, EndpointSpace, Error, Exact,
    Invocation, Iri, Kernel, MetaRenderer, ReprType, Representation, Request, Result, Space,
    Threaded, TraceEvent, Tracer, Verb, FAILED_NOTE,
};

const REMOTE: &str = "urn:test:remote";
const PEER: &str = "urn:cap:test:peer";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn meta_as(target: &str, media: &str) -> Request {
    Request::new(Verb::Meta, iri(target)).with_arg("as", ArgRef::Inline(media.as_bytes().to_vec()))
}

fn json_meta(target: &str) -> Request {
    meta_as(target, "application/json")
}

/// A mounted name whose peer may be down. While `down`, its Meta fails; its `describe()`
/// is always the placeholder a mount can give without its peer. Counts how often the
/// kernel asks and how often it is invoked.
struct Remote {
    down: AtomicBool,
    asked: AtomicU32,
    invoked: AtomicU32,
    description: Description,
}

impl Remote {
    fn new(description: Description) -> Arc<Self> {
        Arc::new(Remote {
            down: AtomicBool::new(true),
            asked: AtomicU32::new(0),
            invoked: AtomicU32::new(0),
            description,
        })
    }

    fn placeholder() -> Arc<Self> {
        Remote::new(
            Description::new("pending")
                .verb(Verb::Source)
                .verb(Verb::Exists)
                .verb(Verb::Sink)
                .verb(Verb::Delete),
        )
    }
}

#[async_trait]
impl Endpoint for Remote {
    async fn invoke(&self, _inv: &Invocation<'_>) -> Result<Representation> {
        self.invoked.fetch_add(1, Ordering::SeqCst);
        Ok(Representation::new(
            ReprType::new("text/plain"),
            b"answered".to_vec(),
        ))
    }

    fn name(&self) -> &str {
        "pending"
    }

    fn describe(&self) -> Description {
        self.description.clone()
    }

    fn meta_failure(&self) -> Option<Error> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.down
            .load(Ordering::SeqCst)
            .then(|| Error::Unavailable("the peer is down, so its contract is unknown".into()))
    }
}

/// An `Arc<Remote>` as a bindable endpoint, so a test keeps its handle on the counters.
struct Shared(Arc<Remote>);

#[async_trait]
impl Endpoint for Shared {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        self.0.invoke(inv).await
    }
    fn name(&self) -> &str {
        self.0.name()
    }
    fn describe(&self) -> Description {
        self.0.describe()
    }
    fn meta_failure(&self) -> Option<Error> {
        self.0.meta_failure()
    }
}

/// JSON for `application/json`, one plain line naming the endpoint for every other face.
struct Renderer;

impl MetaRenderer for Renderer {
    fn render(&self, description: &Description, target: &ReprType) -> Result<Representation> {
        if target.media_type == "application/json" {
            return Ok(Representation::new(
                ReprType::new("application/json"),
                serde_json::to_vec(description).unwrap(),
            ));
        }
        Ok(Representation::new(
            ReprType::new("text/plain"),
            format!("endpoint {}\n", description.id).into_bytes(),
        ))
    }
}

fn kernel_over(space: impl Space + 'static) -> Kernel {
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(Renderer))
}

fn kernel_with(remote: &Arc<Remote>) -> Kernel {
    kernel_over(EndpointSpace::new().bind(Exact::new(REMOTE), Shared(Arc::clone(remote))))
}

fn issue(kernel: &Kernel, request: Request, capability: &Capability) -> Result<Representation> {
    block_on(kernel.issue(request, capability))
}

fn is_unavailable(answer: &Result<Representation>) -> bool {
    matches!(answer, Err(Error::Unavailable(_)))
}

#[derive(Default)]
struct Collect(Mutex<Vec<TraceEvent>>);

impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

// ------------------------------------------------------------------------ 1. the answer

#[test]
fn a_meta_answers_the_failure_in_every_face() {
    let remote = Remote::placeholder();
    let kernel = kernel_with(&remote);
    let root = Capability::root();

    let json = issue(&kernel, json_meta(REMOTE), &root);
    assert!(is_unavailable(&json), "JSON Meta: {json:?}");
    let text = issue(&kernel, meta_as(REMOTE, "text/plain"), &root);
    assert!(is_unavailable(&text), "text Meta: {text:?}");
    // The sentence a person reads through a contract read: the error's own, unchanged.
    assert_eq!(
        text.unwrap_err().to_string(),
        "unavailable: the peer is down, so its contract is unknown"
    );
    // A Meta with no face named (Turtle) is answered the same way.
    let plain = issue(&kernel, Request::new(Verb::Meta, iri(REMOTE)), &root);
    assert!(is_unavailable(&plain), "default Meta: {plain:?}");
    assert_eq!(
        remote.invoked.load(Ordering::SeqCst),
        0,
        "a Meta never enters the endpoint"
    );
}

#[test]
fn the_failure_is_traced_as_a_failed_meta() {
    let remote = Remote::placeholder();
    let kernel = kernel_with(&remote);
    let events = Arc::new(Collect::default());
    kernel.set_tracer(Arc::clone(&events) as Arc<dyn Tracer>);

    assert!(is_unavailable(&issue(
        &kernel,
        json_meta(REMOTE),
        &Capability::root()
    )));
    let events = events.0.lock().unwrap();
    let event = events
        .iter()
        .find(|event| event.target == REMOTE)
        .expect("the failed Meta is an event");
    assert!(event
        .notes
        .contains(&(FAILED_NOTE.to_string(), "unavailable".to_string())));
    assert!(!event.cache_hit);
}

// ------------------------------------------------------------------- 2. the floor first

/// The floor never gates a `Meta` (`ActionSpec`s for `Meta` are dropped, so self-description
/// stays readable wherever the catalog offers it): a caller the floor refuses on `Source`
/// is `Denied` there, before the endpoint is entered, and its `Meta` answers what it would
/// have read anyway, which is now the failure. No `Meta` is ever `Denied` by the floor, so
/// the only floor refusal that comes first is the existence-sensitive miss (next test).
#[test]
fn the_floor_still_refuses_the_verbs_it_gates() {
    let remote = Remote::new(
        Description::new("pending")
            .verb(Verb::Source)
            .requires(PEER),
    );
    let kernel = kernel_with(&remote);
    let stranger = Capability::scoped(Vec::<String>::new());

    let read = issue(&kernel, Request::new(Verb::Source, iri(REMOTE)), &stranger);
    assert!(matches!(read, Err(Error::Denied(_))), "{read:?}");
    assert_eq!(remote.invoked.load(Ordering::SeqCst), 0);
    assert!(is_unavailable(&issue(
        &kernel,
        json_meta(REMOTE),
        &stranger
    )));
}

#[test]
fn an_existence_sensitive_endpoint_answers_a_refused_meta_as_a_miss() {
    let remote = Remote::new(
        Description::new("pending")
            .verb(Verb::Source)
            .requires(PEER)
            .existence_sensitive(),
    );
    let kernel = kernel_with(&remote);

    let stranger = Capability::scoped(Vec::<String>::new());
    let hidden = issue(&kernel, json_meta(REMOTE), &stranger);
    assert!(matches!(hidden, Err(Error::Unresolved(_))), "{hidden:?}");
    assert_eq!(remote.asked.load(Ordering::SeqCst), 0);
}

// ------------------------------------------------------------ 3. the walks keep describe()

#[test]
fn describe_the_catalog_and_the_manifold_still_read_describe() {
    let remote = Remote::placeholder();
    let kernel = kernel_with(&remote);
    let root = Capability::root();

    assert_eq!(kernel.describe(&iri(REMOTE)).unwrap().id, "pending");

    let catalog = issue(
        &kernel,
        Request::new(Verb::Source, iri("urn:kernel:catalog")),
        &root,
    )
    .expect("the catalog answers");
    assert!(
        String::from_utf8_lossy(&catalog.bytes).contains("endpoint pending"),
        "the catalog keeps the placeholder row"
    );

    // The default the item named: the manifold keeps the row too.
    let offered = kernel.select_actions(&ActionQuery {
        present: &[],
        verb: Some(Verb::Source),
        want: None,
        capability: Some(&root),
    });
    assert!(
        offered.iter().any(|m| m.id == "pending"),
        "urn:kernel:actions keeps the placeholder row"
    );
    assert!(
        remote.down.load(Ordering::SeqCst),
        "and the endpoint is still down"
    );
}

// ------------------------------------------------------------------- 4. never cached

#[test]
fn the_failure_is_not_cached_and_hides_a_description_cached_before_it() {
    let remote = Remote::placeholder();
    let kernel = kernel_with(&remote);
    let root = Capability::root();

    // Up: the Meta succeeds and is cached, as every Meta is.
    remote.down.store(false, Ordering::SeqCst);
    assert!(issue(&kernel, json_meta(REMOTE), &root).is_ok());
    assert!(kernel.is_cached(&json_meta(REMOTE), &root));

    // Down: the cached description is not served; each Meta asks again and fails.
    remote.down.store(true, Ordering::SeqCst);
    let before = remote.asked.load(Ordering::SeqCst);
    assert!(is_unavailable(&issue(&kernel, json_meta(REMOTE), &root)));
    assert!(is_unavailable(&issue(&kernel, json_meta(REMOTE), &root)));
    assert_eq!(
        remote.asked.load(Ordering::SeqCst),
        before + 2,
        "asked on every Meta"
    );

    // Up again: the Meta succeeds.
    remote.down.store(false, Ordering::SeqCst);
    let answer = issue(&kernel, json_meta(REMOTE), &root).expect("the peer is back");
    let described: serde_json::Value = serde_json::from_slice(&answer.bytes).unwrap();
    assert_eq!(described["id"], "pending");
}

#[test]
fn a_failure_from_the_start_leaves_nothing_in_the_cache() {
    let remote = Remote::placeholder();
    let kernel = kernel_with(&remote);
    let root = Capability::root();

    assert!(is_unavailable(&issue(&kernel, json_meta(REMOTE), &root)));
    assert!(!kernel.is_cached(&json_meta(REMOTE), &root));
    remote.down.store(false, Ordering::SeqCst);
    assert!(issue(&kernel, json_meta(REMOTE), &root).is_ok());
}

// --------------------------------------------------------------- 5. wrappers pass it on

#[test]
fn a_confinement_passes_the_failure_through() {
    let remote = Remote::placeholder();
    let confined = Confine::new(
        iri("urn:test:corridor"),
        Arc::new(EndpointSpace::new()),
        Arc::new(Shared(Arc::clone(&remote))),
    );
    assert!(matches!(
        confined.meta_failure(),
        Some(Error::Unavailable(_))
    ));
    let kernel = kernel_over(EndpointSpace::new().bind(Exact::new(REMOTE), confined));
    assert!(is_unavailable(&issue(
        &kernel,
        json_meta(REMOTE),
        &Capability::root()
    )));
}

#[test]
fn a_threaded_space_passes_the_failure_through() {
    let remote = Remote::placeholder();
    let inner = EndpointSpace::new().bind(Exact::new(REMOTE), Shared(Arc::clone(&remote)));
    let kernel = kernel_over(Threaded::new(Arc::new(inner), "urn:test:thread"));
    let root = Capability::root();
    assert!(is_unavailable(&issue(&kernel, json_meta(REMOTE), &root)));
    remote.down.store(false, Ordering::SeqCst);
    assert!(issue(&kernel, json_meta(REMOTE), &root).is_ok());
}

// ------------------------------------------------------------ 6. other verbs unaffected

#[test]
fn every_other_verb_reaches_the_endpoint() {
    let remote = Remote::placeholder();
    let kernel = kernel_with(&remote);
    let root = Capability::root();
    let content = || ArgRef::Inline(b"x".to_vec());

    for request in [
        Request::new(Verb::Source, iri(REMOTE)),
        Request::new(Verb::Exists, iri(REMOTE)),
        Request::new(Verb::Sink, iri(REMOTE)).with_arg("content", content()),
        Request::new(Verb::Delete, iri(REMOTE)),
    ] {
        let verb = request.verb;
        let answer = issue(&kernel, request, &root).unwrap_or_else(|e| panic!("{verb:?}: {e}"));
        assert_eq!(answer.bytes, b"answered", "{verb:?}");
    }
    assert_eq!(remote.invoked.load(Ordering::SeqCst), 4);
    assert_eq!(remote.asked.load(Ordering::SeqCst), 0, "only a Meta asks");
}
