//! **Existence-sensitive resources** (ledger #2176, UX record D28). A resource whose
//! EXISTENCE is sensitive answers a caller its capability floor refuses exactly as a
//! name bound nowhere is answered, so the caller cannot tell "exists, and I may not"
//! from "does not exist". Before the flag a module could not reach that: the floor
//! refused with `Denied` before the endpoint ran, and a missing name was `Unresolved`
//! (ikigai-throttle's state face, ledger #198, hit the limit).
//!
//! One test per claim: the defect (an unmarked door is still told apart, as D28 wants
//! for a resource whose existence is not sensitive); the collapse, byte-identical in
//! the error and in the trace; every verb, `Meta` included; per verb, where the
//! authoring forms differ; the cache, keyed by the capability; `urn:kernel:validate`
//! and `urn:kernel:actions`; and the door in the topology.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    ActionSpec, ArgRef, AsyncFnEndpoint, Capability, Confine, Description, EndpointSpace, Error,
    Exact, Fallback, FnEndpoint, Iri, Kernel, MetaRenderer, ReprType, Representation, Request,
    Result, Space, TraceEvent, Tracer, UriTemplate, Verb, DENIED_NOTE,
};

/// The grant the governor state face requires.
const STATE: &str = "urn:cap:throttle:state";
/// A governor that exists, and one that does not.
const HIDDEN: &str = "urn:throttle:state:g1";
const MISSING: &str = "urn:throttle:state:g2";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn request(verb: Verb, target: &str) -> Request {
    let request = Request::new(verb, iri(target));
    if verb.is_mutating() {
        request.with_arg("content", ArgRef::Inline(b"x".to_vec()))
    } else {
        request
    }
}

fn source(target: &str) -> Request {
    request(Verb::Source, target)
}

fn nobody() -> Capability {
    Capability::scoped(Vec::<String>::new())
}

fn holder() -> Capability {
    Capability::scoped([STATE.to_string()])
}

/// Renders a description as its id.
struct IdRenderer;

impl MetaRenderer for IdRenderer {
    fn render(&self, description: &Description, _target: &ReprType) -> Result<Representation> {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            description.id.as_bytes().to_vec(),
        ))
    }
}

/// A cacheable governor state door under `description`, counting invocations.
fn governor(description: Description, calls: &Arc<AtomicU32>) -> FnEndpoint {
    let calls = Arc::clone(calls);
    FnEndpoint::new("governor-state", move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(Representation::new(ReprType::new("text/plain"), b"tokens=3".to_vec()).cacheable())
    })
    .with_description(description)
}

/// The state face: Source, gated on `STATE`, existence-sensitive.
fn sensitive() -> Description {
    Description::new("governor-state")
        .verb(Verb::Source)
        .requires(STATE)
        .existence_sensitive()
}

/// A kernel binding `HIDDEN` (and nothing at `MISSING`) under `description`.
fn kernel(description: Description) -> (Kernel, Arc<AtomicU32>) {
    let calls = Arc::new(AtomicU32::new(0));
    let space = EndpointSpace::new().bind(Exact::new(HIDDEN), governor(description, &calls));
    (
        Kernel::with_meta_renderer(Arc::new(space), Arc::new(IdRenderer)),
        calls,
    )
}

fn issue(kernel: &Kernel, request: Request, capability: &Capability) -> Result<Representation> {
    block_on(kernel.issue(request, capability))
}

/// The error a name bound nowhere gets, as its caller sees it: kind and display.
fn seen(error: &Error) -> (&'static str, String) {
    (error.kind(), error.to_string())
}

/// The same answer, with the other name in it: what "byte-identical but for the name
/// the caller typed" means.
#[track_caller]
fn assert_answers_as_missing(hidden: &Error, missing: &Error) {
    assert!(matches!(hidden, Error::Unresolved(_)), "got {hidden:?}");
    let (kind, text) = seen(missing);
    assert_eq!(
        seen(hidden),
        (kind, text.replace(MISSING, HIDDEN)),
        "a hidden name answers as a missing one"
    );
}

#[derive(Default)]
struct Collect(Mutex<Vec<TraceEvent>>);

impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

// ---- 1. The defect, and what stays as it was ----------------------------------

/// Without the flag a caller with no grant tells "exists" from "missing": `Denied`
/// versus `Unresolved`. That is right for a resource whose existence is NOT
/// sensitive (D28: a refusal is a 403), so it stays.
#[test]
fn an_unmarked_door_is_still_denied_and_so_told_from_a_missing_name() {
    let unmarked = Description::new("governor-state")
        .verb(Verb::Source)
        .requires(STATE);
    let (kernel, calls) = kernel(unmarked);
    let denied = issue(&kernel, source(HIDDEN), &nobody()).unwrap_err();
    let missing = issue(&kernel, source(MISSING), &nobody()).unwrap_err();
    assert!(matches!(denied, Error::Denied(_)), "got {denied:?}");
    assert!(matches!(missing, Error::Unresolved(_)), "got {missing:?}");
    assert_ne!(denied.kind(), missing.kind());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

// ---- 2. The collapse ------------------------------------------------------------

#[test]
fn a_caller_with_no_grant_cannot_tell_a_sensitive_door_from_a_missing_name() {
    let (kernel, calls) = kernel(sensitive());
    let hidden = issue(&kernel, source(HIDDEN), &nobody()).unwrap_err();
    let missing = issue(&kernel, source(MISSING), &nobody()).unwrap_err();
    assert_answers_as_missing(&hidden, &missing);
    assert_eq!(
        hidden.to_string(),
        format!("no endpoint resolved for {HIDDEN}")
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the endpoint is never entered"
    );

    // A caller holding the grant reads it, as ever.
    let read = issue(&kernel, source(HIDDEN), &holder()).unwrap();
    assert_eq!(read.bytes, b"tokens=3");
    // So does a caller holding some OTHER grant only: no.
    let other = Capability::scoped(["urn:cap:fs:read".to_string()]);
    let refused = issue(&kernel, source(HIDDEN), &other).unwrap_err();
    assert!(matches!(refused, Error::Unresolved(_)), "got {refused:?}");
}

/// In the empty chain a plain miss records no trace event, so a hidden refusal
/// records none either, where an unmarked one records its `denied` event.
#[test]
fn a_hidden_refusal_traces_as_a_plain_miss_does_nothing() {
    let (kernel, _) = kernel(sensitive());
    for target in [HIDDEN, MISSING] {
        let events = Arc::new(Collect::default());
        let err = block_on(kernel.issue_traced(source(target), &nobody(), events.clone()));
        assert!(matches!(err, Err(Error::Unresolved(_))));
        assert!(
            events.0.lock().unwrap().is_empty(),
            "{target}: {:?}",
            events.0.lock().unwrap()
        );
    }
    let (unmarked, _) = kernel_unmarked();
    let events = Arc::new(Collect::default());
    let _ = block_on(unmarked.issue_traced(source(HIDDEN), &nobody(), events.clone()));
    let events = events.0.lock().unwrap();
    assert!(
        events[0].notes.iter().any(|(k, _)| k == DENIED_NOTE),
        "{events:?}"
    );
}

/// Inside a chain a miss IS traced (`scope-unresolved`): a hidden refusal records
/// that same event, note for note, and no `denied`.
#[test]
fn inside_a_chain_a_hidden_refusal_traces_note_for_note_as_a_miss() {
    let calls = Arc::new(AtomicU32::new(0));
    let corridor: Arc<dyn Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new(HIDDEN), governor(sensitive(), &calls)));
    let probe = AsyncFnEndpoint::new("probe", |inv| {
        Box::pin(async move {
            let hidden = inv.source(&iri(HIDDEN)).await.unwrap_err();
            let missing = inv.source(&iri(MISSING)).await.unwrap_err();
            assert_answers_as_missing(&hidden, &missing);
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"ok".to_vec(),
            ))
        })
    });
    let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind_arc(
        Exact::new("urn:probe"),
        Arc::new(Confine::new(
            iri("urn:ctx:probe"),
            corridor,
            Arc::new(probe),
        )),
    )));
    let events = Arc::new(Collect::default());
    block_on(kernel.issue_traced(source("urn:probe"), &nobody(), events.clone())).unwrap();
    let events = events.0.lock().unwrap();
    let of = |target: &str| {
        events
            .iter()
            .find(|e| e.target == target)
            .unwrap_or_else(|| panic!("{target} is traced: {events:?}"))
            .clone()
    };
    let (hidden, missing) = (of(HIDDEN), of(MISSING));
    let renamed: Vec<(String, String)> = missing
        .notes
        .iter()
        .map(|(k, v)| (k.clone(), v.replace(MISSING, HIDDEN)))
        .collect();
    assert_eq!(hidden.notes, renamed);
    assert!(!hidden.notes.iter().any(|(k, _)| k == DENIED_NOTE));
    assert_eq!(
        (hidden.started, hidden.ended, hidden.cache_hit),
        (missing.started, missing.ended, missing.cache_hit)
    );
    assert_eq!(hidden.capability, missing.capability);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

// ---- 3. Every verb ---------------------------------------------------------------

#[test]
fn every_verb_and_meta_answer_a_refused_caller_as_a_missing_name() {
    let (kernel, calls) = kernel(sensitive());
    for verb in [
        Verb::Source,
        Verb::Exists,
        Verb::Sink,
        Verb::Delete,
        Verb::Meta,
    ] {
        let hidden = issue(&kernel, request(verb, HIDDEN), &nobody()).unwrap_err();
        let missing = issue(&kernel, request(verb, MISSING), &nobody()).unwrap_err();
        assert_answers_as_missing(&hidden, &missing);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // The holder still reads the description.
    let meta = issue(&kernel, request(Verb::Meta, HIDDEN), &holder()).unwrap();
    assert_eq!(meta.bytes, b"governor-state");
    // An unmarked door's Meta stays ungated, as the floor has always left it.
    let (open, _) = kernel_unmarked();
    assert!(issue(&open, request(Verb::Meta, HIDDEN), &nobody()).is_ok());
}

fn kernel_unmarked() -> (Kernel, Arc<AtomicU32>) {
    kernel(
        Description::new("governor-state")
            .verb(Verb::Source)
            .requires(STATE),
    )
}

/// The two authoring forms are exclusive per verb: an explicit action's flag replaces
/// the flat one. A verb that is not marked keeps `Denied`, which says the resource
/// exists: the flag hides what it marks and nothing else.
#[test]
fn the_flag_is_per_verb_and_an_explicit_action_replaces_the_flat_one() {
    let set = "urn:cap:throttle:set";
    let mixed = Description::new("governor-state")
        .verb(Verb::Source)
        .requires(STATE)
        .existence_sensitive()
        .action(ActionSpec::new(Verb::Sink).requires(set));
    let (k, _) = kernel(mixed);
    let read = issue(&k, source(HIDDEN), &nobody()).unwrap_err();
    assert!(matches!(read, Error::Unresolved(_)), "got {read:?}");
    let write = issue(&k, request(Verb::Sink, HIDDEN), &nobody()).unwrap_err();
    assert!(matches!(write, Error::Denied(_)), "got {write:?}");

    // And the other way round: a flat, unmarked form beside a marked action.
    let only_the_action = Description::new("governor-state")
        .verb(Verb::Sink)
        .requires(set)
        .action(
            ActionSpec::new(Verb::Source)
                .requires(STATE)
                .existence_sensitive(),
        );
    let (kernel, _) = kernel(only_the_action);
    let read = issue(&kernel, source(HIDDEN), &nobody()).unwrap_err();
    assert!(matches!(read, Error::Unresolved(_)), "got {read:?}");
    let write = issue(&kernel, request(Verb::Sink, HIDDEN), &nobody()).unwrap_err();
    assert!(matches!(write, Error::Denied(_)), "got {write:?}");
    // A caller who may read is told the resource exists, so its Meta is answered.
    assert!(issue(&kernel, request(Verb::Meta, HIDDEN), &holder()).is_ok());
}

/// The flag hides a REFUSAL; a verb that requires nothing is never refused.
#[test]
fn a_sensitive_door_that_requires_nothing_answers_everyone() {
    let (kernel, _) = kernel(
        Description::new("governor-state")
            .verb(Verb::Source)
            .existence_sensitive(),
    );
    assert!(issue(&kernel, source(HIDDEN), &nobody()).is_ok());
    assert!(issue(&kernel, request(Verb::Meta, HIDDEN), &nobody()).is_ok());
}

// ---- 4. The cache ---------------------------------------------------------------

/// The collapsed answer is never stored, and what the holder's read stored is never
/// served to another capability: the floor (and with it the collapse) runs BEFORE the
/// cache lookup, and the key carries the capability's fingerprint, so the holder's
/// entry is not even the key the refused caller would look up.
#[test]
fn the_holders_cached_answer_is_never_served_to_a_refused_caller() {
    let (kernel, calls) = kernel(sensitive());
    // Refused first: nothing is stored, nothing is probed as cached.
    let err = issue(&kernel, source(HIDDEN), &nobody()).unwrap_err();
    assert!(matches!(err, Error::Unresolved(_)));
    assert_eq!(kernel.cache_len(), 0);
    assert!(!kernel.is_cached(&source(HIDDEN), &nobody()));

    // The holder computes and stores, under its own capability.
    issue(&kernel, source(HIDDEN), &holder()).unwrap();
    assert_eq!(kernel.cache_len(), 1);
    assert!(kernel.is_cached(&source(HIDDEN), &holder()));
    assert!(
        !kernel.is_cached(&source(HIDDEN), &nobody()),
        "the key carries the capability fingerprint"
    );

    // The refused caller still gets the miss, and the holder its hit.
    let err = issue(&kernel, source(HIDDEN), &nobody()).unwrap_err();
    let missing = issue(&kernel, source(MISSING), &nobody()).unwrap_err();
    assert_answers_as_missing(&err, &missing);
    assert_eq!(
        issue(&kernel, source(HIDDEN), &holder()).unwrap().bytes,
        b"tokens=3"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the second read was a hit");

    // Meta likewise: the holder's description is cached for the holder only.
    issue(&kernel, request(Verb::Meta, HIDDEN), &holder()).unwrap();
    let err = issue(&kernel, request(Verb::Meta, HIDDEN), &nobody()).unwrap_err();
    assert!(matches!(err, Error::Unresolved(_)), "got {err:?}");
}

// ---- 5. The kernel's ungated faces ------------------------------------------------

fn body(kernel: &Kernel, request: Request, capability: &Capability) -> Result<String> {
    issue(kernel, request, capability).map(|r| String::from_utf8(r.bytes).unwrap())
}

fn validate(by: &str, value: &str) -> Request {
    let request = source("urn:kernel:validate").with_arg(by, ArgRef::Inline(value.into()));
    if by == "endpoint" {
        request.with_arg("verb", ArgRef::Inline(b"source".to_vec()))
    } else {
        request
    }
}

/// `urn:kernel:validate` pre-flights a hidden door, by every name an action can be
/// given, as it pre-flights a door bound nowhere; the holder gets its report.
#[test]
fn validate_answers_a_refused_caller_as_it_answers_a_missing_door() {
    let (kernel, _) = kernel(sensitive());
    let contract = sensitive().action_specs()[0].contract_iri("governor-state");
    let missing_contract = ActionSpec::new(Verb::Source)
        .requires(STATE)
        .existence_sensitive()
        .contract_iri("governor-other");
    let cases = [
        ("endpoint", HIDDEN.to_string(), MISSING.to_string()),
        (
            "action",
            format!("urn:ikigai:match:source:{HIDDEN}"),
            format!("urn:ikigai:match:source:{MISSING}"),
        ),
        (
            "action",
            "urn:ikigai:endpoint:governor-state:action:source".to_string(),
            "urn:ikigai:endpoint:governor-other:action:source".to_string(),
        ),
        ("action", contract.clone(), missing_contract.clone()),
    ];
    for (by, hidden, missing) in &cases {
        let refused = body(&kernel, validate(by, hidden), &nobody()).unwrap_err();
        let absent = body(&kernel, validate(by, missing), &nobody()).unwrap_err();
        assert_eq!(
            (refused.kind(), refused.to_string()),
            (
                absent.kind(),
                absent
                    .to_string()
                    .replace(missing.as_str(), hidden.as_str())
                    .replace(MISSING, HIDDEN)
                    .replace("governor-other", "governor-state")
            ),
            "{by}={hidden}"
        );
        let report = body(&kernel, validate(by, hidden), &holder()).unwrap();
        assert!(
            report.contains("sh:conforms true"),
            "{by}={hidden}: {report}"
        );
    }
}

/// `urn:kernel:actions` was already self-limiting (it offers only what the caller can
/// invoke): it never offers a refused caller the hidden action.
#[test]
fn the_action_manifold_never_offers_a_refused_caller_the_hidden_door() {
    let (kernel, _) = kernel(sensitive());
    let refused = body(&kernel, source("urn:kernel:actions"), &nobody()).unwrap();
    assert!(!refused.contains(HIDDEN), "{refused}");
    let offered = body(&kernel, source("urn:kernel:actions"), &holder()).unwrap();
    assert!(offered.contains(HIDDEN), "{offered}");
}

/// A template door hides each name it admits, and a refused caller cannot tell a name
/// the template matches from one bound nowhere.
#[test]
fn a_template_door_hides_every_name_it_admits() {
    let calls = Arc::new(AtomicU32::new(0));
    let space = EndpointSpace::new().bind(
        UriTemplate::parse("urn:throttle:state:{governor}").unwrap(),
        governor(sensitive(), &calls),
    );
    let kernel = Kernel::new(Arc::new(Fallback::new(vec![Arc::new(space)])));
    let hidden = issue(&kernel, source(HIDDEN), &nobody()).unwrap_err();
    let elsewhere = issue(&kernel, source("urn:throttle:other:g1"), &nobody()).unwrap_err();
    assert!(matches!(hidden, Error::Unresolved(_)), "got {hidden:?}");
    assert_eq!(hidden.kind(), elsewhere.kind());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

// ---- 6. The topology ---------------------------------------------------------------

/// The door says so, as `ik:existenceSensitive true`, for the certificate work
/// (ledger #2385) to read; an unmarked door says nothing new.
#[test]
fn the_topology_marks_the_sensitive_door() {
    let calls = Arc::new(AtomicU32::new(0));
    let unmarked = Description::new("governor-config").verb(Verb::Source);
    let space = EndpointSpace::new()
        .bind(Exact::new(HIDDEN), governor(sensitive(), &calls))
        .bind(
            Exact::new("urn:throttle:config"),
            governor(unmarked, &calls),
        );
    let kernel = Kernel::new(Arc::new(space));
    let topology = kernel.topology();
    let turtle = topology.to_turtle();
    assert_eq!(
        turtle.matches("ik:existenceSensitive true").count(),
        1,
        "{turtle}"
    );
    #[cfg(feature = "declare")]
    assert_eq!(
        ikigai_core::Topology::from_turtle(&turtle).unwrap(),
        topology,
        "the flag round-trips"
    );
}
