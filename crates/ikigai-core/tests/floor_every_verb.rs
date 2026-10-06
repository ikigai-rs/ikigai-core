//! **Declared = enforced, on every verb** (ledger #750: A2, A3). The capability floor
//! used to check only the verbs a description declared, so an endpoint declaring a
//! gated `Source` answered an undeclared `Exists` — and `FnEndpoint`, like the
//! builtins, does not dispatch on verb, so `Exists` returned the gated bytes to a
//! caller holding nothing. And a `requires` declared with no verb was enforced on
//! none. The rule now, one test per row of the table on the kernel's `Floor`:
//!
//! - a declared verb → exactly its own `requires`;
//! - an undeclared `Exists` → `Source`'s `requires` when `Source` is declared, else
//!   every `requires` the endpoint declares;
//! - an undeclared `Source`, `Sink` or `Delete` → every `requires` it declares;
//! - `Meta` → none (the kernel answers it; the endpoint is never entered);
//! - no `requires` anywhere → open on every verb, as before.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ActionSpec, ArgRef, Capability, Description, Endpoint, EndpointSpace, Error, Exact, FnEndpoint,
    Iri, Kernel, ReprType, Representation, Request, Result, Verb,
};

const TARGET: &str = "urn:data:secret";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

/// A kernel binding one `FnEndpoint` (which, like the builtins, answers every verb
/// with the same function) under `description`, and a count of its invocations.
fn kernel(description: Description) -> (Kernel, Arc<AtomicU32>) {
    let calls = Arc::new(AtomicU32::new(0));
    let seen = Arc::clone(&calls);
    let endpoint = FnEndpoint::new("secret", move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(Representation::new(
            ReprType::new("text/plain"),
            b"s3cr3t".to_vec(),
        ))
    })
    .with_description(description);
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new(TARGET), endpoint),
    ));
    (kernel, calls)
}

fn request(verb: Verb) -> Request {
    let request = Request::new(verb, iri(TARGET));
    if verb.is_mutating() {
        request.with_arg("content", ArgRef::Inline(b"x".to_vec()))
    } else {
        request
    }
}

fn issue(kernel: &Kernel, verb: Verb, capability: &Capability) -> Result<Representation> {
    block_on(kernel.issue(request(verb), capability))
}

fn nobody() -> Capability {
    Capability::scoped(Vec::<String>::new())
}

fn holding(scopes: &[&str]) -> Capability {
    Capability::scoped(scopes.iter().map(|s| s.to_string()))
}

#[track_caller]
fn assert_denied(outcome: Result<Representation>, what: &str) {
    assert!(
        matches!(outcome, Err(Error::Denied(_))),
        "{what}: expected a denial, got {:?}",
        outcome.map(|r| String::from_utf8_lossy(&r.bytes).into_owned())
    );
}

#[track_caller]
fn assert_admitted(outcome: Result<Representation>, what: &str) {
    assert!(
        outcome.is_ok(),
        "{what}: expected admission, got {outcome:?}"
    );
}

/// The case the audit reproduced: an undeclared `Exists` on a `Source`-gated
/// endpoint, by a caller holding nothing, is refused — and the endpoint is never
/// entered, so the gated bytes never leave it.
#[test]
fn an_undeclared_exists_needs_what_source_needs() {
    let (kernel, calls) = kernel(
        Description::new("secret")
            .verb(Verb::Source)
            .requires("urn:cap:secret"),
    );
    assert_denied(issue(&kernel, Verb::Source, &nobody()), "Source");
    assert_denied(issue(&kernel, Verb::Exists, &nobody()), "Exists");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a denied verb entered the endpoint"
    );
}

/// The implicit-Exists users (fn conditional templates, the REPL's and Lisp's
/// `exists`, conformance probes, a browse overlay) hold the READ grant: that is
/// enough, and nothing more is asked of them.
#[test]
fn a_reader_holding_only_the_source_grant_can_still_exists() {
    let (kernel, _) = kernel(
        Description::new("secret")
            .verb(Verb::Source)
            .requires("urn:cap:secret"),
    );
    let reader = holding(&["urn:cap:secret"]);
    assert_admitted(issue(&kernel, Verb::Source, &reader), "Source");
    assert_admitted(issue(&kernel, Verb::Exists, &reader), "Exists");
}

/// An undeclared mutating verb forces a cut, so a caller with no grant reaches
/// neither `Sink` nor `Delete` on a `Source`-gated endpoint.
#[test]
fn a_caller_with_no_grant_cannot_exists_sink_or_delete_a_source_gated_endpoint() {
    let (kernel, calls) = kernel(
        Description::new("secret")
            .verb(Verb::Source)
            .requires("urn:cap:secret"),
    );
    for verb in [Verb::Exists, Verb::Sink, Verb::Delete] {
        assert_denied(issue(&kernel, verb, &nobody()), &format!("{verb:?}"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// An undeclared mutating verb needs EVERY scope the endpoint declares — the union
/// over its verbs — not just one verb's.
#[test]
fn an_undeclared_mutating_verb_needs_every_declared_scope() {
    let (kernel, _) = kernel(
        Description::new("cal")
            .action(ActionSpec::new(Verb::Source).requires("urn:cap:cal:read"))
            .action(ActionSpec::new(Verb::Sink).requires("urn:cap:cal:write")),
    );
    assert_denied(
        issue(&kernel, Verb::Delete, &holding(&["urn:cap:cal:write"])),
        "Delete with the write grant only",
    );
    assert_denied(
        issue(&kernel, Verb::Delete, &holding(&["urn:cap:cal:read"])),
        "Delete with the read grant only",
    );
    assert_admitted(
        issue(
            &kernel,
            Verb::Delete,
            &holding(&["urn:cap:cal:read", "urn:cap:cal:write"]),
        ),
        "Delete with both",
    );
}

/// A declared verb is held to exactly its own declaration — unchanged: the read
/// grant reads and does not write, the write grant writes and does not read.
#[test]
fn a_declared_verb_needs_exactly_its_own_requires() {
    let (kernel, _) = kernel(
        Description::new("cal")
            .action(ActionSpec::new(Verb::Source).requires("urn:cap:cal:read"))
            .action(ActionSpec::new(Verb::Sink).requires("urn:cap:cal:write")),
    );
    let read = holding(&["urn:cap:cal:read"]);
    let write = holding(&["urn:cap:cal:write"]);
    assert_admitted(issue(&kernel, Verb::Source, &read), "Source with read");
    assert_denied(issue(&kernel, Verb::Sink, &read), "Sink with read");
    assert_admitted(issue(&kernel, Verb::Sink, &write), "Sink with write");
    assert_denied(issue(&kernel, Verb::Source, &write), "Source with write");
    // Exists follows Source, whichever grant the caller writes with.
    assert_admitted(issue(&kernel, Verb::Exists, &read), "Exists with read");
    assert_denied(issue(&kernel, Verb::Exists, &write), "Exists with write");
}

/// An endpoint that declares only a mutating verb has no `Source` for `Exists` to
/// follow; `Exists` — and an undeclared `Source` — need everything it declares.
#[test]
fn without_a_declared_source_an_undeclared_read_needs_every_declared_scope() {
    let (kernel, calls) = kernel(
        Description::new("drop")
            .verb(Verb::Sink)
            .requires("urn:cap:drop:write"),
    );
    assert_denied(issue(&kernel, Verb::Exists, &nobody()), "Exists");
    assert_denied(issue(&kernel, Verb::Source, &nobody()), "Source");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let writer = holding(&["urn:cap:drop:write"]);
    assert_admitted(issue(&kernel, Verb::Exists, &writer), "Exists with write");
}

/// A3: a `requires` declared with no verb at all — what the catalog renders as
/// `ik:requires` — applies to every verb, instead of to none.
#[test]
fn a_requires_declared_with_no_verb_applies_to_every_verb() {
    let (kernel, calls) = kernel(Description::new("secret").requires("urn:cap:secret"));
    for verb in [Verb::Source, Verb::Exists, Verb::Sink, Verb::Delete] {
        assert_denied(issue(&kernel, verb, &nobody()), &format!("{verb:?}"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let holder = holding(&["urn:cap:secret"]);
    for verb in [Verb::Source, Verb::Exists, Verb::Sink, Verb::Delete] {
        assert_admitted(issue(&kernel, verb, &holder), &format!("{verb:?}"));
    }
}

/// An endpoint that declares no `requires` anywhere is open on every verb, as it
/// always was.
#[test]
fn an_endpoint_declaring_no_requires_stays_open_on_every_verb() {
    let (kernel, _) = kernel(Description::new("open").verb(Verb::Source));
    for verb in [Verb::Source, Verb::Exists, Verb::Sink, Verb::Delete] {
        assert_admitted(issue(&kernel, verb, &nobody()), &format!("{verb:?}"));
    }
}

/// `Meta` is answered by the kernel from the description and never enters the
/// endpoint, so the floor asks nothing of it: self-description stays readable.
/// (The kernel here has no Meta renderer, so the answer is a renderer error — what
/// matters is that it is not a denial.)
#[test]
fn meta_is_not_floored() {
    let (kernel, calls) = kernel(
        Description::new("secret")
            .verb(Verb::Source)
            .requires("urn:cap:secret"),
    );
    let meta = issue(&kernel, Verb::Meta, &nobody());
    assert!(
        !matches!(meta, Err(Error::Denied(_))),
        "Meta was floored: {meta:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// The declaration the floor reads is the one `describe()` returns — a sanity check
/// that the fixture states what the tests above assume.
#[test]
fn the_fixture_declares_what_the_tests_assume() {
    let endpoint = FnEndpoint::new("x", |_| unreachable!())
        .with_description(Description::new("x").requires("urn:cap:secret"));
    let described = endpoint.describe();
    assert!(described.verbs.is_empty());
    assert_eq!(described.requires, vec!["urn:cap:secret".to_string()]);
}
