//! **An upstream that contributes nothing is no upstream** (ledger #2926).
//!
//! The cache keeps one upstream variant per key and serves an entry only under the
//! upstream it was stored under, because an entry's expiry and threads include that
//! upstream's. A host's first pipe stage has no upstream, but `ikigai-engine` hands it
//! the empty one, `Provenance::new(Expiry::Never, {})`, on every `source`, while its
//! `trace` issues with none. Those were two variants of one answer, and the probe, which
//! knows nothing of upstreams, called either one cached. So `source` then `trace` said
//! cached then computed (true: it ran the endpoint), a second `trace` said cached, and a
//! `source` after that said cached while running the endpoint (false).
//!
//! These pin: the two forms share one entry in both orders, as do a kernel operation and
//! a cached failure; and an upstream that does contribute still keeps its own variant.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    Capability, EndpointSpace, Error, Exact, Expiry, FixedClock, FnEndpoint, Iri, Kernel,
    Provenance, ReprType, Representation, Request, Thread, Time, TraceEvent, Tracer, Verb,
};

const NAME: &str = "urn:test:board";

fn source(target: &str) -> Request {
    Request::new(Verb::Source, Iri::parse(target).unwrap())
}

/// What `ikigai-engine` hands the first stage of a line.
fn empty() -> Provenance {
    Provenance::new(Expiry::Never, BTreeSet::new())
}

#[derive(Default)]
struct Collect(Mutex<Vec<TraceEvent>>);

impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// A kernel with one cacheable endpoint, and the count of its invocations.
fn counted() -> (Kernel, Arc<AtomicUsize>) {
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&runs);
    let board = FnEndpoint::new("board", move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(Representation::new(ReprType::new("text/plain"), b"closed".to_vec()).cacheable())
    });
    let space = EndpointSpace::new().bind(Exact::new(NAME), board);
    (Kernel::new(Arc::new(space)), runs)
}

/// What `source` does: the empty upstream.
fn read_as_source(kernel: &Kernel) {
    block_on(kernel.issue_with_incoming(source(NAME), &Capability::root(), empty())).unwrap();
}

/// What `trace` does: no upstream, and whether the kernel says it served the answer.
fn read_as_trace(kernel: &Kernel) -> bool {
    let collect = Arc::new(Collect::default());
    block_on(kernel.issue_traced(source(NAME), &Capability::root(), collect.clone())).unwrap();
    let events = collect.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    events[0].cache_hit
}

#[test]
fn a_trace_after_a_source_is_served_what_the_source_stored() {
    let (kernel, runs) = counted();
    read_as_source(&kernel);
    assert!(kernel.is_cached(&source(NAME), &Capability::root()));
    assert!(read_as_trace(&kernel), "the probe and the trace agree");
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn a_source_after_a_trace_is_served_what_the_trace_stored() {
    let (kernel, runs) = counted();
    assert!(!read_as_trace(&kernel), "nothing is stored yet");
    assert!(kernel.is_cached(&source(NAME), &Capability::root()));
    read_as_source(&kernel);
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "the probe said cached: it is"
    );
}

#[test]
fn a_kernel_operation_shares_its_entry_too() {
    let (kernel, _) = counted();
    let read = || source("urn:kernel:actions");
    let root = Capability::root();
    block_on(kernel.issue_with_incoming(read(), &root, empty())).unwrap();
    let collect = Arc::new(Collect::default());
    block_on(kernel.issue_traced(read(), &root, collect.clone())).unwrap();
    assert!(collect.0.lock().unwrap()[0].cache_hit);
}

#[test]
fn a_cached_failure_is_served_to_both_forms() {
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&runs);
    let gone = FnEndpoint::new("gone", move |inv| {
        counter.fetch_add(1, Ordering::SeqCst);
        inv.failure_expires_at(Time::from_millis(500));
        Err(Error::Unavailable("gone".to_string()))
    });
    let space = EndpointSpace::new().bind(Exact::new(NAME), gone);
    let kernel = Kernel::new(Arc::new(space)).with_clock(Arc::new(FixedClock::at(10)));
    let root = Capability::root();
    block_on(kernel.issue_with_incoming(source(NAME), &root, empty())).unwrap_err();
    block_on(kernel.issue(source(NAME), &root)).unwrap_err();
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn an_upstream_that_contributes_keeps_its_own_variant() {
    let root = Capability::root();
    let threaded = Provenance::new(Expiry::Never, BTreeSet::from([Thread::new("urn:up:a")]));
    let timed = Provenance::new(Expiry::At(Time::from_millis(100)), BTreeSet::new());
    for upstream in [threaded, timed] {
        let (kernel, runs) = counted();
        let kernel = kernel.with_clock(Arc::new(FixedClock::at(10)));
        block_on(kernel.issue_with_incoming(source(NAME), &root, upstream.clone())).unwrap();
        block_on(kernel.issue_with_incoming(source(NAME), &root, upstream.clone())).unwrap();
        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "the same upstream is served"
        );
        block_on(kernel.issue(source(NAME), &root)).unwrap();
        assert_eq!(
            runs.load(Ordering::SeqCst),
            2,
            "no upstream is not this one"
        );
        let plain = block_on(kernel.issue(source(NAME), &root)).unwrap();
        assert_eq!(plain.expiry, Expiry::Never);
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }
}
