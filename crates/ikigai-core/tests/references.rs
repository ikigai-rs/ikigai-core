//! **The kernel settles a by-reference argument by the target's contract** (ledger #2511,
//! K1 of the named-requests design, decision 5).
//!
//! Before this a reference reached only an endpoint whose code matched
//! `ArgRef::Reference`, and the ecosystem matched it two incompatible ways (as a name, as a
//! value), while everything that read `inline_str` refused it (`expected an inline value`).
//! Now one kernel rule, keyed on the contract, the rule an attached body follows:
//!
//! 1. a by-value input reading `inline_str("in")` answers over the referent's bytes;
//! 2. cutting the referent recomputes the answer, and a cache hit reads nothing;
//! 3. a referent the caller may not read is `Denied`, naming the argument and reference;
//! 4. an `xsd:anyURI` input receives the IRI's text;
//! 5. a `reads_body` input receives the reference untouched;
//! 6. an over-budget referent is `OverBudget`, naming the argument;
//! 7. an argument a closed set does not declare is the endpoint's own, as it always was,
//!    and any argument of an open set is a by-value one;
//! 8. a named request's fixed reference reaches its target, which settles it.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::executor::block_on;
use ikigai_core::{
    Alias, AliasTable, ArgRef, ArgSpec, Body, Bytes, Capability, Description, Endpoint,
    EndpointSpace, Error, Exact, Fallback, FixedArgument, FnEndpoint, Invocation, Iri, Kernel,
    ReprType, Representation, Request, Result, Space, TopologyRule, Verb,
};

const READ: &str = "urn:cap:t:data:read";
const ANY_URI: &str = "http://www.w3.org/2001/XMLSchema#anyURI";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(bytes: impl Into<Vec<u8>>) -> Representation {
    Representation::new(ReprType::new("text/plain"), bytes.into())
}

/// `urn:t:data`: a cell a Sink writes and a Source reads, counting its reads.
fn cell(value: Arc<Mutex<Vec<u8>>>, reads: Arc<AtomicU32>) -> FnEndpoint {
    FnEndpoint::new("data", move |inv| match inv.request.verb {
        Verb::Sink => {
            *value.lock().unwrap() = inv.inline_arg("content")?.to_vec();
            Ok(text(b"ok".to_vec()))
        }
        _ => {
            reads.fetch_add(1, Ordering::SeqCst);
            Ok(text(value.lock().unwrap().clone()).cacheable())
        }
    })
    .with_description(
        Description::new("data")
            .verb(Verb::Source)
            .verb(Verb::Sink)
            .input(ArgSpec::new("content").optional())
            .requires(READ),
    )
}

/// An upper-caser reading `in` by value, cacheable, counting its runs.
fn upper(runs: Arc<AtomicU32>) -> FnEndpoint {
    FnEndpoint::new("upper", move |inv| {
        runs.fetch_add(1, Ordering::SeqCst);
        Ok(text(inv.inline_str("in")?.to_uppercase()).cacheable())
    })
    .with_description(
        Description::new("upper")
            .verb(Verb::Source)
            .input(ArgSpec::new("in")),
    )
}

/// What the endpoint was handed for `arg`, as text: `ref <iri>` for a reference.
fn handed(inv: &Invocation<'_>, arg: &str) -> String {
    match inv.request.args.get(arg) {
        Some(ArgRef::Reference(iri)) => format!("ref <{}>", iri.as_str()),
        Some(ArgRef::Inline(bytes)) => format!("inline {}", String::from_utf8_lossy(bytes)),
        Some(ArgRef::Content(id)) => format!("content {id}"),
        None => "absent".into(),
    }
}

/// An endpoint that says what it was handed for `arg`, under a contract `describe` builds.
fn reporter(arg: &'static str, description: Description) -> FnEndpoint {
    FnEndpoint::new(description.id.clone(), move |inv| {
        Ok(text(handed(inv, arg)))
    })
    .with_description(description)
}

/// A one-shot streamed referent of six bytes.
struct Streamed;

#[async_trait]
impl Endpoint for Streamed {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let body = self.invoke_body(inv).await?;
        body.collect_within(inv.memory_budget().unwrap()).await
    }

    async fn invoke_body(&self, _inv: &Invocation<'_>) -> Result<Representation<Body>> {
        let chunks = [
            Ok(Bytes::from_static(b"abc")),
            Ok(Bytes::from_static(b"def")),
        ];
        Ok(Representation::from_body(
            ReprType::new("text/plain"),
            Body::once(futures::stream::iter(chunks)),
        ))
    }
}

struct Host {
    kernel: Kernel,
    reads: Arc<AtomicU32>,
    runs: Arc<AtomicU32>,
}

fn host() -> Host {
    let reads = Arc::new(AtomicU32::new(0));
    let runs = Arc::new(AtomicU32::new(0));
    let value = Arc::new(Mutex::new(b"hello".to_vec()));
    let doors = EndpointSpace::new()
        .bind(Exact::new("urn:t:data"), cell(value, Arc::clone(&reads)))
        .bind(Exact::new("urn:t:streamed"), Streamed)
        .bind(Exact::new("urn:t:upper"), upper(Arc::clone(&runs)))
        .bind(
            Exact::new("urn:t:name"),
            reporter(
                "src",
                Description::new("name")
                    .verb(Verb::Source)
                    .input(ArgSpec::new("src").class(ANY_URI)),
            ),
        )
        .bind(
            Exact::new("urn:t:body"),
            reporter(
                "content",
                Description::new("body")
                    .verb(Verb::Source)
                    .input(ArgSpec::new("content").reads_body()),
            ),
        )
        .bind(
            Exact::new("urn:t:closed"),
            reporter("x", Description::new("closed").verb(Verb::Source)),
        )
        .bind(
            Exact::new("urn:t:open"),
            reporter(
                "x",
                Description::new("open")
                    .verb(Verb::Source)
                    .open_arguments("anything"),
            ),
        );
    let names = AliasTable::new()
        .with_rule(
            TopologyRule::named_request("urn:t:shout", "urn:t:upper")
                .verb(Verb::Source)
                .fixing(FixedArgument::reference("in", iri("urn:t:data"))),
        )
        .unwrap();
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        Arc::new(doors),
        Arc::new(Alias::new(Arc::new(names), Arc::new(EndpointSpace::new()))),
    ]));
    Host {
        kernel: Kernel::new(root).with_stream_memory(4, 4),
        reads,
        runs,
    }
}

fn referring(target: &str, arg: &str, to: &str) -> Request {
    Request::new(Verb::Source, iri(target)).with_arg(arg, ArgRef::Reference(iri(to)))
}

impl Host {
    fn read(&self, request: Request) -> std::result::Result<String, Error> {
        self.read_as(request, &Capability::root())
    }

    fn read_as(&self, request: Request, cap: &Capability) -> std::result::Result<String, Error> {
        block_on(self.kernel.issue(request, cap)).map(|r| String::from_utf8(r.bytes).unwrap())
    }
}

// 1, 2 ──────────────────────────────────────────────────────────────────────────

#[test]
fn a_by_value_input_answers_over_the_referents_bytes_and_hangs_from_its_thread() {
    let host = host();
    let ask = || referring("urn:t:upper", "in", "urn:t:data");
    assert_eq!(host.read(ask()).unwrap(), "HELLO");
    assert_eq!(host.reads.load(Ordering::SeqCst), 1);
    // A hit reads nothing: the answer is keyed on the reference.
    assert_eq!(host.read(ask()).unwrap(), "HELLO");
    assert_eq!(host.runs.load(Ordering::SeqCst), 1);
    assert_eq!(host.reads.load(Ordering::SeqCst), 1);
    // A write to the referent cuts its thread, and the answer recomputes over it.
    let write = Request::new(Verb::Sink, iri("urn:t:data"))
        .with_arg("content", ArgRef::Inline(b"bye".to_vec()));
    host.read(write).unwrap();
    assert_eq!(host.read(ask()).unwrap(), "BYE");
    assert_eq!(host.runs.load(Ordering::SeqCst), 2);
}

// 3 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_referent_the_caller_may_not_read_is_denied_naming_it() {
    let host = host();
    let err = host
        .read_as(
            referring("urn:t:upper", "in", "urn:t:data"),
            &Capability::scoped(["urn:cap:t:other"]),
        )
        .unwrap_err();
    assert!(matches!(err, Error::Denied(_)), "{err}");
    let message = err.to_string();
    assert!(
        message.contains("argument `in` refers to <urn:t:data>, which this caller may not read"),
        "{message}"
    );
    assert_eq!(
        host.runs.load(Ordering::SeqCst),
        0,
        "the endpoint was never entered"
    );
}

// 4, 5 ──────────────────────────────────────────────────────────────────────────

#[test]
fn a_name_input_receives_the_iri_and_a_body_input_the_reference_untouched() {
    let host = host();
    assert_eq!(
        host.read(referring("urn:t:name", "src", "urn:t:data"))
            .unwrap(),
        "inline urn:t:data"
    );
    assert_eq!(
        host.read(referring("urn:t:body", "content", "urn:t:data"))
            .unwrap(),
        "ref <urn:t:data>"
    );
    assert_eq!(
        host.reads.load(Ordering::SeqCst),
        0,
        "neither was read by the kernel"
    );
}

// 6 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn an_over_budget_referent_is_refused_naming_the_argument() {
    let host = host();
    let err = host
        .read(referring("urn:t:upper", "in", "urn:t:streamed"))
        .unwrap_err();
    match err {
        Error::OverBudget { detail } => assert!(
            detail.starts_with("argument `in` refers to <urn:t:streamed>"),
            "{detail}"
        ),
        other => panic!("expected OverBudget, got {other:?}"),
    }
}

// 7 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn an_undeclared_argument_of_a_closed_set_is_left_and_an_open_sets_is_settled() {
    let host = host();
    assert_eq!(
        host.read(referring("urn:t:closed", "x", "urn:t:data"))
            .unwrap(),
        "ref <urn:t:data>"
    );
    assert_eq!(
        host.read(referring("urn:t:open", "x", "urn:t:data"))
            .unwrap(),
        "inline hello"
    );
}

// 8 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_named_requests_fixed_reference_is_settled_by_its_target() {
    let host = host();
    let shout = Request::new(Verb::Source, iri("urn:t:shout"));
    assert_eq!(host.read(shout.clone()).unwrap(), "HELLO");
    let write = Request::new(Verb::Sink, iri("urn:t:data"))
        .with_arg("content", ArgRef::Inline(b"again".to_vec()));
    host.read(write).unwrap();
    assert_eq!(
        host.read(shout).unwrap(),
        "AGAIN",
        "editing the referent recomputes the name"
    );
}
