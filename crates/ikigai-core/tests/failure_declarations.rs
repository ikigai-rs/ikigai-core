//! **Declared failure classes** (ledger #1546, #200; the retry-authority note, section 3).
//!
//! An action may state the failure classes it can produce. The declaration is for the
//! manifold and for conformance, never for a runtime decision: the kernel compares what
//! an endpoint produced with what it declared and, on a mismatch, adds an
//! `undeclared-failure` trace note. It never rewrites the error (decision 6). These pin
//! that, and that an action stating nothing keeps its contract IRI.

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    ActionSpec, ArgSpec, Capability, Description, EndpointSpace, Error, Exact, FailureClass,
    Failures, FnEndpoint, Iri, Kernel, Request, TraceEvent, Tracer, Verb, FAILED_NOTE,
    UNDECLARED_FAILURE_NOTE,
};

#[derive(Default)]
struct Recorder(Mutex<Vec<TraceEvent>>);

impl Tracer for Recorder {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// Fail with `error` behind `description`, traced, and return the error and the note.
fn fail_under(description: Description, error: Error) -> (Error, Option<String>) {
    let produced = error.clone();
    let endpoint =
        FnEndpoint::new("subject", move |_| Err(produced.clone())).with_description(description);
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:subject"), endpoint),
    ));
    let recorder = Arc::new(Recorder::default());
    let request = Request::new(Verb::Source, Iri::parse("urn:subject").unwrap());
    let err =
        block_on(kernel.issue_traced(request, &Capability::root(), recorder.clone())).unwrap_err();
    let events = recorder.0.lock().unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
    let notes = &events[0].notes;
    assert!(
        notes.contains(&(FAILED_NOTE.to_string(), err.kind().to_string())),
        "{notes:?}"
    );
    let undeclared = notes
        .iter()
        .find(|(key, _)| key == UNDECLARED_FAILURE_NOTE)
        .map(|(_, value)| value.clone());
    (err, undeclared)
}

fn flat() -> Description {
    Description::new("subject").verb(Verb::Source)
}

#[test]
fn a_declared_class_is_not_noted() {
    let limited = Error::RetryAfter {
        delay: None,
        detail: "rate-limited".into(),
    };
    let (err, note) = fail_under(flat().fails(FailureClass::RetryAfter), limited.clone());
    assert_eq!(err, limited);
    assert_eq!(note, None);
}

#[test]
fn an_undeclared_class_is_noted_and_the_error_is_untouched() {
    let slow = Error::Timeout("upstream did not answer".into());
    let (err, note) = fail_under(flat().fails(FailureClass::RetryAfter), slow.clone());
    assert_eq!(err, slow, "the kernel never rewrites the error");
    assert_eq!(note.as_deref(), Some("transient"));
}

#[test]
fn an_action_that_states_nothing_is_never_noted() {
    for error in [
        Error::Endpoint("boom".into()),
        Error::Timeout("slow".into()),
        Error::OverBudget {
            detail: "past 5000 ms".into(),
        },
    ] {
        let (_, note) = fail_under(flat(), error.clone());
        assert_eq!(note, None, "{error}");
    }
}

#[test]
fn permanent_is_implied_for_every_action() {
    let (_, note) = fail_under(
        flat().fails(FailureClass::Transient),
        Error::NotFound("gone".into()),
    );
    assert_eq!(note, None);
    // "Only the implied ones" is a declaration too.
    let mut only_implied = flat();
    only_implied.failures = Failures::Only(vec![]);
    let (_, note) = fail_under(only_implied, Error::Conflict("taken".into()));
    assert_eq!(note, None);
}

#[test]
fn denied_is_implied_only_beside_a_requires() {
    let refusing = || Error::Denied("the module's ACL refused /etc".into());
    let (_, note) = fail_under(flat().fails(FailureClass::Transient), refusing());
    assert_eq!(note.as_deref(), Some("denied"));
    // Root passes the floor, so the endpoint's own denial is the one produced.
    let gated = flat()
        .requires("urn:cap:fs:read")
        .fails(FailureClass::Transient);
    let (_, note) = fail_under(gated, refusing());
    assert_eq!(note, None);
}

#[test]
fn an_explicit_action_replaces_the_flat_declaration_for_its_verb() {
    // The flat form says retry-after; the Source action restates nothing about failures,
    // so for Source the declaration is UNSTATED and nothing is noted.
    let description = Description::new("subject")
        .fails(FailureClass::RetryAfter)
        .action(ActionSpec::new(Verb::Source));
    let (_, note) = fail_under(description, Error::Endpoint("boom".into()));
    assert_eq!(note, None);
    // And an explicit declaration is read for its own verb.
    let description = Description::new("subject")
        .action(ActionSpec::new(Verb::Source).fails(FailureClass::Transient));
    let (_, note) = fail_under(description, Error::Endpoint("boom".into()));
    assert_eq!(note.as_deref(), Some("unknown"));
}

/// The digest promise: an action that states nothing keeps its contract IRI, in both
/// digest forms, and one that states classes gets a new one.
#[test]
fn stating_nothing_keeps_the_contract_iri() {
    // Pinned before 0.1.108 (ikigai-vocab's contract vectors carry the same rows): a v1
    // and a v2 contract that predate the field.
    let v1 = ActionSpec::new(Verb::Source)
        .input(ArgSpec::new("in"))
        .requires("urn:cap:a");
    let v2 = v1.clone().open_arguments("anything else");
    let unstated = [v1.contract_iri("to-upper"), v2.contract_iri("to-upper")];
    assert_eq!(v1.failures, Failures::Unstated);
    let stated = [
        v1.clone()
            .fails(FailureClass::Transient)
            .contract_iri("to-upper"),
        v2.clone()
            .fails(FailureClass::Transient)
            .contract_iri("to-upper"),
    ];
    for (before, after) in unstated.iter().zip(&stated) {
        assert_ne!(before, after);
    }
    // A flat declaration moves the synthesized action's IRI the same way.
    let flat_stated = Description::new("to-upper")
        .verb(Verb::Source)
        .input(ArgSpec::new("in"))
        .requires("urn:cap:a")
        .fails(FailureClass::Transient);
    assert_eq!(
        flat_stated.action_specs()[0].contract_iri("to-upper"),
        stated[0]
    );
}
