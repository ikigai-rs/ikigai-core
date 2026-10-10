//! **A failed invocation is a trace node** (ledger #559, #20).
//!
//! Since core 0.1.73 a failed sub-request is a CACHE dependency: a composite that
//! catches a child's `NotFound` and returns a fallback hangs from the child's thread,
//! and one that swallows a child's `Denied` is never cached. But the kernel recorded a
//! trace event only after an invocation returned `Ok`, so the same failed child left
//! NO trace node: the trace and the cache disagreed about what a resolution touched.
//! And the refusal that matters most for SSRF and jail escapes, a module's own
//! parameterized ACL returning `Denied` from inside `invoke`, was invisible to every
//! tracer; only the kernel floor's denial ([`DENIED_NOTE`]) showed.
//!
//! These pin both: the failed child is a node under its parent, tagged
//! [`FAILED_NOTE`] with the error's kind and nothing else from the error, timed, and
//! named by the same IRI the cache hangs the parent from.

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, AsyncFnEndpoint, Capability, Description, EndpointSpace, Error, Exact, FixedClock,
    FnEndpoint, Iri, Kernel, ReprType, Representation, Request, TraceEvent, Tracer, Verb,
    DENIED_NOTE, FAILED_NOTE, UNCACHED_NOTE,
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
struct Recorder(Mutex<Vec<TraceEvent>>);

impl Tracer for Recorder {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Recorder {
    fn events(&self) -> Vec<TraceEvent> {
        self.0.lock().unwrap().clone()
    }

    fn only(&self, target: &str) -> TraceEvent {
        let matching: Vec<TraceEvent> = self
            .events()
            .into_iter()
            .filter(|e| e.target == target)
            .collect();
        assert_eq!(matching.len(), 1, "one event for {target}: {matching:?}");
        matching.into_iter().next().unwrap()
    }
}

fn note<'a>(event: &'a TraceEvent, key: &str) -> Option<&'a str> {
    event
        .notes
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn traced(kernel: &Kernel, target: &str) -> (Result<Representation, Error>, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let result =
        block_on(kernel.issue_traced(source(target), &Capability::root(), recorder.clone()));
    (result, recorder)
}

/// The cached reads hanging from `thread`, per `urn:kernel:dependents`.
fn dependents(kernel: &Kernel, thread: &str) -> Vec<String> {
    let request = source("urn:kernel:dependents")
        .with_arg("thread", ArgRef::Inline(thread.as_bytes().to_vec()));
    let out = block_on(kernel.issue(request, &Capability::root())).unwrap();
    String::from_utf8(out.bytes)
        .unwrap()
        .lines()
        .skip(2)
        .filter_map(|l| l.split_whitespace().next())
        .filter(|t| t.starts_with("urn:"))
        .map(str::to_string)
        .collect()
}

/// The tic-tac-toe shape from the tutorial (tutorial PR #38): the platonic cell
/// sources the STORED cell and maps its `NotFound` to `"-"`. The stored cell is
/// bound, so its `NotFound` is the endpoint's own answer, not a kernel miss.
fn tic_tac_toe() -> Kernel {
    let stored = FnEndpoint::new("stored", |_| Err(Error::NotFound("unplayed".into())));
    let platonic = AsyncFnEndpoint::new("platonic", |inv| {
        Box::pin(async move {
            match inv.source(&Iri::parse("urn:t:stored:2:2").unwrap()).await {
                Ok(mark) => Ok(mark.cacheable()),
                Err(Error::NotFound(_)) => Ok(text("-").cacheable()),
                Err(other) => Err(other),
            }
        })
    });
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:t:stored:2:2"), stored)
            .bind(Exact::new("urn:t:cell:2:2"), platonic),
    ))
    .with_clock(Arc::new(FixedClock::at(1_000)))
}

#[test]
fn a_child_that_failed_not_found_is_a_node_and_the_cache_names_the_same_child() {
    let kernel = tic_tac_toe();
    let (result, recorder) = traced(&kernel, "urn:t:cell:2:2");
    assert_eq!(result.unwrap().bytes, b"-");

    // The trace: two nodes, the failed child under its parent.
    let parent = recorder.only("urn:t:cell:2:2");
    let child = recorder.only("urn:t:stored:2:2");
    assert_eq!(recorder.events().len(), 2, "{:?}", recorder.events());
    assert_eq!(child.parent, Some(parent.span));
    assert_eq!(note(&child, FAILED_NOTE), Some("not-found"));
    assert_eq!(note(&parent, FAILED_NOTE), None, "the parent succeeded");
    // It RAN: timed, unlike a pre-dispatch refusal, and never a cache hit.
    assert!(
        child.started.is_some() && child.ended.is_some(),
        "{child:?}"
    );
    assert!(!child.cache_hit);
    // The kind only: the error's message never reaches an observer.
    assert!(
        child.notes.iter().all(|(_, v)| !v.contains("unplayed")),
        "{child:?}"
    );

    // The cache: the parent was stored, hanging from the failed child's thread, so a
    // write that plays the square cuts it. Same name on both sides.
    assert!(
        dependents(&kernel, &child.target).contains(&parent.target),
        "{:?}",
        dependents(&kernel, &child.target)
    );
}

/// A module's own parameterized ACL (the real gate for fs/net): the endpoint is
/// entered, decides the path is outside its grant, and returns `Denied` itself.
fn acl_kernel() -> Kernel {
    let file = FnEndpoint::new("file", |_| {
        Err(Error::Denied(
            "path /etc/secret is outside the granted roots".into(),
        ))
    });
    let floored = FnEndpoint::new("floored", |_| Ok(text("gated")))
        .with_description(Description::new("floored").requires("urn:cap:t:floor"));
    let reader = AsyncFnEndpoint::new("reader", |inv| {
        Box::pin(async move {
            let file = inv.source(&Iri::parse("urn:t:file").unwrap()).await;
            let gated = inv.source(&Iri::parse("urn:t:floored").unwrap()).await;
            Ok(text(&format!("{} {}", file.is_ok(), gated.is_ok())).cacheable())
        })
    });
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:t:file"), file)
            .bind(Exact::new("urn:t:floored"), floored)
            .bind(Exact::new("urn:t:reader"), reader),
    ))
    .with_clock(Arc::new(FixedClock::at(1_000)))
}

#[test]
fn an_endpoints_own_denial_is_a_node_distinct_from_the_floors() {
    let kernel = acl_kernel();
    let recorder = Arc::new(Recorder::default());
    let caller = Capability::scoped(Vec::<String>::new());
    let out =
        block_on(kernel.issue_traced(source("urn:t:reader"), &caller, recorder.clone())).unwrap();
    assert_eq!(out.bytes, b"false false");

    let reader = recorder.only("urn:t:reader");
    // The endpoint's OWN refusal: it ran (timed), tagged failed=denied, and carries
    // neither the floor's note nor any of the refusal's text (the path).
    let own = recorder.only("urn:t:file");
    assert_eq!(own.parent, Some(reader.span));
    assert_eq!(note(&own, FAILED_NOTE), Some("denied"));
    assert_eq!(note(&own, DENIED_NOTE), None);
    assert!(own.started.is_some() && own.ended.is_some(), "{own:?}");
    assert!(
        own.notes.iter().all(|(_, v)| !v.contains("/etc/secret")),
        "{own:?}"
    );
    // The kernel FLOOR's refusal, unchanged: never ran, names the scope it lacked,
    // and is not also a failed invocation (nothing was invoked).
    let floor = recorder.only("urn:t:floored");
    assert_eq!(floor.parent, Some(reader.span));
    assert_eq!(note(&floor, DENIED_NOTE), Some("urn:cap:t:floor"));
    assert_eq!(note(&floor, FAILED_NOTE), None);
    assert!(floor.started.is_none() && floor.ended.is_none());

    // The cache names the same two children: a result built on a refusal is never
    // stored, and the reason lists exactly the nodes the trace shows as refused.
    assert_eq!(
        note(&reader, UNCACHED_NOTE),
        Some("denied urn:t:file urn:t:floored")
    );
}

#[test]
fn a_failed_root_is_its_own_node_with_the_kind_of_its_error() {
    let kernel = tic_tac_toe();
    let (result, recorder) = traced(&kernel, "urn:t:stored:2:2");
    assert!(matches!(result, Err(Error::NotFound(_))), "{result:?}");
    let root = recorder.only("urn:t:stored:2:2");
    assert_eq!(root.parent, None);
    assert_eq!(note(&root, FAILED_NOTE), Some("not-found"));
}

#[test]
fn a_composite_that_propagates_fails_too_and_keeps_its_own_notes() {
    let failing = AsyncFnEndpoint::new("strict", |inv| {
        Box::pin(async move {
            inv.trace_note("attempt", "1");
            let mark = inv.source(&Iri::parse("urn:t:stored").unwrap()).await?;
            Ok(mark)
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(
                Exact::new("urn:t:stored"),
                FnEndpoint::new("stored", |_| Err(Error::Timeout("slow".into()))),
            )
            .bind(Exact::new("urn:t:strict"), failing),
    ));
    let (result, recorder) = traced(&kernel, "urn:t:strict");
    assert!(matches!(result, Err(Error::Timeout(_))), "{result:?}");
    let strict = recorder.only("urn:t:strict");
    let stored = recorder.only("urn:t:stored");
    assert_eq!(stored.parent, Some(strict.span));
    assert_eq!(note(&stored, FAILED_NOTE), Some("timeout"));
    assert_eq!(note(&strict, FAILED_NOTE), Some("timeout"));
    // What the endpoint noted before it failed is kept, after the failure.
    assert_eq!(note(&strict, "attempt"), Some("1"));
    // The no-clock kernel has no times, failed or not.
    assert!(stored.started.is_none() && stored.ended.is_none());
}
