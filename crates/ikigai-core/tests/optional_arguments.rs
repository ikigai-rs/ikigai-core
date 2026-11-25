//! **Optional inline readers** (ledger #4370, `docs/design/optional-arguments.md`):
//! `Invocation::optional_arg` and `optional_str` answer `None` for an absent argument alone.
//! A present value they cannot read (invalid UTF-8 for text, a reference, a stored or
//! attached body) is an `InvalidArgument` naming it, never absence, and the readers neither
//! resolve the reference nor fetch or consume the body.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, ArgSpec, AsyncFnEndpoint, Bindings, Body, BodyIssue, Bytes, Capability, ContentId,
    Description, EndpointSpace, Error, Exact, FnEndpoint, Invocation, Iri, Kernel, ReprType,
    Representation, Request, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn request() -> Request {
    Request::new(Verb::Source, iri("urn:optional"))
        .with_arg("empty", ArgRef::Inline(vec![]))
        .with_arg("text", ArgRef::Inline(b"hello".to_vec()))
        .with_arg("invalid", ArgRef::Inline(vec![0xff]))
        .with_arg("reference", ArgRef::Reference(iri("urn:value")))
        .with_arg("stored", ArgRef::Content(ContentId::of(b"value")))
}

fn names(error: Result<impl std::fmt::Debug, Error>, name: &str) -> bool {
    matches!(error, Err(Error::InvalidArgument { name: ref actual, .. }) if actual == name)
}

#[test]
fn an_absent_argument_is_none() {
    let (request, bindings, capability) = (request(), Bindings::new(), Capability::root());
    let inv = Invocation::detached(&request, &bindings, &capability);
    assert_eq!(inv.optional_arg("absent").unwrap(), None);
    assert_eq!(inv.optional_str("absent").unwrap(), None);
}

#[test]
fn a_present_inline_value_is_some_an_empty_one_included() {
    let (request, bindings, capability) = (request(), Bindings::new(), Capability::root());
    let inv = Invocation::detached(&request, &bindings, &capability);
    assert_eq!(inv.optional_arg("empty").unwrap(), Some(&b""[..]));
    assert_eq!(inv.optional_str("empty").unwrap(), Some(""));
    assert_eq!(inv.optional_arg("text").unwrap(), Some(&b"hello"[..]));
    assert_eq!(inv.optional_str("text").unwrap(), Some("hello"));
}

#[test]
fn invalid_utf8_is_refused_as_text_and_accepted_as_bytes() {
    let (request, bindings, capability) = (request(), Bindings::new(), Capability::root());
    let inv = Invocation::detached(&request, &bindings, &capability);
    assert_eq!(inv.optional_arg("invalid").unwrap(), Some(&b"\xff"[..]));
    assert!(names(inv.optional_str("invalid"), "invalid"));
}

#[test]
fn a_reference_or_stored_content_is_refused_by_both_readers() {
    let (request, bindings, capability) = (request(), Bindings::new(), Capability::root());
    let inv = Invocation::detached(&request, &bindings, &capability);
    for name in ["reference", "stored"] {
        assert!(names(inv.optional_arg(name), name), "{name} as bytes");
        assert!(names(inv.optional_str(name), name), "{name} as text");
    }
}

/// The defect the readers fix, kept as a contrast: the old idioms turn a present value
/// they cannot read into the default, where `optional_str(..)?` reports it.
#[test]
fn the_old_idioms_swallow_what_the_optional_reader_reports() {
    let (request, bindings, capability) = (request(), Bindings::new(), Capability::root());
    let inv = Invocation::detached(&request, &bindings, &capability);
    assert_eq!(inv.inline_str("invalid").unwrap_or("default"), "default");
    assert_eq!(inv.inline_str("stored").ok(), None);
    let read = |name| -> Result<&str, Error> { Ok(inv.optional_str(name)?.unwrap_or("default")) };
    assert_eq!(read("absent").unwrap(), "default");
    assert!(names(read("invalid"), "invalid"));
    assert!(names(read("stored"), "stored"));
}

/// Under a kernel: an attached one-shot body and a retained reference are refused by both
/// readers, the body is still whole for `content_body` afterwards, and the reference's
/// target is never resolved.
#[test]
fn under_a_kernel_the_readers_neither_consume_a_body_nor_resolve_a_reference() {
    let resolved = Arc::new(AtomicUsize::new(0));
    let target = {
        let resolved = Arc::clone(&resolved);
        FnEndpoint::new("value", move |_| {
            resolved.fetch_add(1, Ordering::SeqCst);
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"value".to_vec(),
            ))
        })
    };
    let stage = AsyncFnEndpoint::new("stage", |inv| {
        Box::pin(async move {
            for name in ["content", "reference"] {
                assert!(names(inv.optional_arg(name), name), "{name} as bytes");
                assert!(names(inv.optional_str(name), name), "{name} as text");
            }
            let body = inv.content_body("content").await?;
            let bytes = inv.collect(body).await?;
            Ok(Representation::new(
                ReprType::new("text/plain"),
                bytes.to_vec(),
            ))
        })
    })
    .with_description(
        Description::new("stage")
            .verb(Verb::Source)
            .input(ArgSpec::new("content").reads_body())
            .input(ArgSpec::new("reference").reads_body()),
    );
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:value"), target)
        .bind(Exact::new("urn:stage"), stage);
    let kernel = Kernel::new(Arc::new(space));
    let request = Request::new(Verb::Source, iri("urn:stage"))
        .with_arg("reference", ArgRef::Reference(iri("urn:value")));
    let body = Body::once(futures::stream::iter([Ok(Bytes::from_static(b"abc"))]));
    let issue = BodyIssue::new().attach("content", body);
    let answer = block_on(kernel.issue_with(request, &Capability::root(), issue)).unwrap();
    assert_eq!(answer.bytes, b"abc");
    assert_eq!(
        resolved.load(Ordering::SeqCst),
        0,
        "the reference was resolved"
    );
}
