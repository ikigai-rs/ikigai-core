//! **Opacity** (ledger #3866): an endpoint uses its scope only by issuing requests into it.
//! What it requests, the value it returns and that value's validity depend on the scope only
//! through what those requests answer.
//!
//! The kernel's semantics require it. The cache keys an answer on the REQUEST chain's
//! fingerprint, so an endpoint that could read more of its scope than the key covers could
//! return a cacheable answer the kernel then serves to another scope sharing the key. Through
//! 0.3.x `Invocation::scope` was public and handed endpoint code the resolved scope: its level
//! stack, its live corridors and their count, whether it was severed, its fingerprint and its
//! printed form. That reader is gone (a `compile_fail` doctest on `Invocation::scope` pins it,
//! and another on `Scope::spaces`); these tests pin what is left:
//!
//! 1. two scopes that share a key are indistinguishable to an endpoint, even recomputed;
//! 2. the scope facts an endpoint may still read, `Invocation::clock_pinned` and
//!    `Invocation::pinned_now`, are covered by the key: a pinned-clock answer is not served to
//!    an unpinned scope;
//! 3. and that coverage holds under one name: the chain's identity is a temporal corridor's
//!    name AND its instant (ledger #3869), so the same name injected with and without a clock
//!    is two keys, as `Scope::with_named_at` documents.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Capability, EndpointSpace, Exact, FixedClock, FnEndpoint, Iri, Kernel,
    ReprType, Representation, Request, Scope, Space, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(bytes: &[u8]) -> Representation {
    Representation::new(ReprType::new("text/plain"), bytes.to_vec())
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

/// An endpoint that answers everything it can learn about where it runs: the scope facts
/// the endpoint API still offers, and what one request into the scope answers. Cacheable
/// whatever it saw, so the KEY is what separates scopes, never the endpoint's own care.
fn observer(runs: &Arc<AtomicU32>) -> AsyncFnEndpoint {
    let runs = Arc::clone(runs);
    AsyncFnEndpoint::new("observer", move |inv| {
        runs.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let asked = inv.source(&iri("urn:ctx:door")).await?;
            let seen = format!(
                "pinned={} at={:?} now={:?} asked={} depth={}",
                inv.clock_pinned(),
                inv.pinned_now().map(|t| t.as_millis()),
                inv.now().map(|t| t.as_millis()),
                String::from_utf8_lossy(&asked.bytes),
                inv.depth(),
            );
            Ok(text(seen.as_bytes()).cacheable())
        })
    })
}

fn constant(name: &'static str, bytes: &'static [u8]) -> FnEndpoint {
    FnEndpoint::new(name, move |_| Ok(text(bytes).cacheable()))
}

fn kernel(runs: &Arc<AtomicU32>) -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:observer"), observer(runs))
            .bind(Exact::new("urn:ctx:door"), constant("root door", b"root's")),
    ))
    .with_clock(Arc::new(FixedClock::at(2_000)))
}

#[test]
fn two_scopes_that_share_a_key_are_indistinguishable_to_an_endpoint() {
    // Two corridors built separately under one name: distinct live spaces, and the second
    // binds a door the first does not. Same name, so one key.
    let first: Arc<dyn Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:ctx:door"), constant("a", b"same")));
    let second: Arc<dyn Space> = Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:ctx:door"), constant("b", b"same"))
            .bind(Exact::new("urn:ctx:unasked"), constant("c", b"never asked")),
    );
    let name = || iri("urn:ctx:shared");
    let a = Scope::empty().with_named(name(), first);
    let b = Scope::empty().with_named(name(), second);
    assert_eq!(a.fingerprint(), b.fingerprint(), "one key");

    // Computed fresh in each, on two kernels with nothing cached: the same bytes. Through
    // 0.3.x the endpoint could have told them apart by its corridors' addresses or count.
    let runs = Arc::new(AtomicU32::new(0));
    let in_a =
        block_on(kernel(&runs).issue_in(source("urn:observer"), &Capability::root(), a)).unwrap();
    let in_b =
        block_on(kernel(&runs).issue_in(source("urn:observer"), &Capability::root(), b.clone()))
            .unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 2, "computed twice");
    assert_eq!(in_a.bytes, in_b.bytes);
    assert_eq!(
        String::from_utf8_lossy(&in_a.bytes),
        "pinned=false at=None now=Some(2000) asked=same depth=0"
    );
}

#[test]
fn a_pinned_clock_answer_is_not_served_to_an_unpinned_scope() {
    let runs = Arc::new(AtomicU32::new(0));
    let kernel = kernel(&runs);
    let cap = Capability::root();
    let doors = || -> Arc<dyn Space> {
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:ctx:door"), constant("d", b"then")))
    };
    let pinned = || {
        Scope::empty().with_named_at(
            iri("urn:ctx:time:then"),
            doors(),
            Arc::new(FixedClock::at(1_000)),
        )
    };
    let read = |scope: Scope| {
        let answer = block_on(kernel.issue_in(source("urn:observer"), &cap, scope)).unwrap();
        String::from_utf8_lossy(&answer.bytes).into_owned()
    };

    let then = "pinned=true at=Some(1000) now=Some(1000) asked=then depth=0";
    assert_eq!(read(pinned()), then);
    assert_eq!(read(pinned()), then);
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "the pinned answer is cached"
    );

    // The empty chain, and a chain whose corridor carries no clock under another name:
    // neither is served the pinned answer.
    assert_eq!(
        read(Scope::empty()),
        "pinned=false at=None now=Some(2000) asked=root's depth=0"
    );
    assert_eq!(
        read(Scope::empty().with_named(iri("urn:ctx:plain"), doors())),
        "pinned=false at=None now=Some(2000) asked=then depth=0"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 3);
}

/// Before ledger #3869 this was one key ("the name claims the clock"), and the unpinned
/// chain was served the pinned answer.
#[test]
fn one_name_with_and_without_a_clock_is_two_keys() {
    let runs = Arc::new(AtomicU32::new(0));
    let kernel = kernel(&runs);
    let cap = Capability::root();
    let doors = || -> Arc<dyn Space> {
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:ctx:door"), constant("d", b"then")))
    };
    let name = || iri("urn:ctx:time:then");
    let with_clock = Scope::empty().with_named_at(name(), doors(), Arc::new(FixedClock::at(1_000)));
    let without = Scope::empty().with_named(name(), doors());
    assert_ne!(with_clock.fingerprint(), without.fingerprint());

    let first = block_on(kernel.issue_in(source("urn:observer"), &cap, with_clock)).unwrap();
    let second = block_on(kernel.issue_in(source("urn:observer"), &cap, without)).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&first.bytes),
        "pinned=true at=Some(1000) now=Some(1000) asked=then depth=0"
    );
    assert_eq!(
        String::from_utf8_lossy(&second.bytes),
        "pinned=false at=None now=Some(2000) asked=then depth=0"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}
