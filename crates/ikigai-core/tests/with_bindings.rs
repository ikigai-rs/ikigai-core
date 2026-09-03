//! `Invocation::with_bindings` — a REBORROW, not a new context.
//!
//! The failure this closes is `ikigai-throttle`'s `Failover` handing candidate 2
//! candidate 1's grammar captures. The only way to fix it without this method was
//! `Invocation::detached`, which severs the endpoint from the kernel — so the
//! interesting assertions here are not "the bindings changed" (which would pass on
//! a broken implementation that quietly detached) but that the sub-context is still
//! wired to the kernel in both directions:
//!
//! - forward: a sub-request issued through it **reaches** the kernel;
//! - backward: the dependency that sub-request created is recorded against the
//!   invocation the kernel drains, so cacheability still propagates.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Bindings, Capability, EndpointSpace, Exact, Iri, Kernel, ReprType,
    Representation, Request, Space, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text() -> ReprType {
    ReprType::new("text/plain")
}

/// The outer endpoint: rebinds, then issues a sub-request through the REBORROWED
/// invocation — the shape a failover or any other re-dispatching combinator needs.
/// It reports its own answer as permanently cacheable, so whether the kernel caches
/// it is decided entirely by the dependency it picked up below.
fn dispatcher(inner_target: &'static str) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("dispatcher", move |inv| {
        Box::pin(async move {
            let mut rebound = Bindings::new();
            rebound.insert("name", "seven");
            let candidate = inv.with_bindings(&rebound);

            // The reborrow sees its own captures, not the outer invocation's.
            assert_eq!(candidate.bindings.get("name"), Some("seven"));
            assert_eq!(candidate.bindings.get("id"), None);

            let sub = candidate
                .issue(Request::new(Verb::Source, iri(inner_target)))
                .await?;
            Ok(Representation::new(text(), sub.bytes).cacheable())
        })
    })
}

fn counting(name: &'static str, cacheable: bool, hits: Arc<AtomicUsize>) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new(name, move |_inv| {
        let hits = Arc::clone(&hits);
        Box::pin(async move {
            hits.fetch_add(1, Ordering::SeqCst);
            let repr = Representation::new(text(), b"inner".to_vec());
            Ok(if cacheable { repr.cacheable() } else { repr })
        })
    })
}

fn kernel_with(inner: AsyncFnEndpoint) -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:demo:outer"), dispatcher("urn:demo:inner"))
            .bind(Exact::new("urn:demo:inner"), inner),
    ) as Arc<dyn Space>)
}

#[test]
fn a_sub_request_through_the_reborrow_reaches_the_kernel() {
    // ★ The assertion that a quietly-detached implementation fails: `detached`
    // has no issuer, so `issue` would error with "sub-requests require a kernel
    // context" instead of returning the inner endpoint's bytes.
    let hits = Arc::new(AtomicUsize::new(0));
    let kernel = kernel_with(counting("inner", true, Arc::clone(&hits)));

    let rep = block_on(kernel.issue(
        Request::new(Verb::Source, iri("urn:demo:outer")),
        &Capability::root(),
    ))
    .expect("the reborrowed invocation must still be able to issue");

    assert_eq!(rep.bytes, b"inner");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn a_dependency_picked_up_through_the_reborrow_still_limits_the_result() {
    // ★ The assertion that a COPYING implementation fails. The sub-request is
    // volatile; the outer result says `.cacheable()`. A result is only as cacheable
    // as its most volatile dependency, so the kernel must decline to cache it — and
    // it can only know that if the dependency the reborrow recorded landed on the
    // invocation the kernel drains. Copy the recording side instead of sharing it
    // and the dependency dies with the reborrow: the kernel sees a clean cacheable
    // result and serves a volatile resource from cache forever.
    let hits = Arc::new(AtomicUsize::new(0));
    let kernel = kernel_with(counting("inner", false, Arc::clone(&hits)));
    let cap = Capability::root();

    for _ in 0..2 {
        let rep = block_on(kernel.issue(Request::new(Verb::Source, iri("urn:demo:outer")), &cap))
            .unwrap();
        assert_eq!(rep.bytes, b"inner");
    }

    assert_eq!(
        kernel.cache_len(),
        0,
        "a volatile dependency issued through the reborrow must reach the kernel"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "the volatile inner resource must be re-sourced on every read"
    );
}

#[test]
fn a_cacheable_dependency_through_the_reborrow_still_lets_the_result_cache() {
    // The control for the test above: same shape, cacheable inner. If sharing the
    // recording side had broken dependency accounting in the other direction, this
    // is where it would show.
    let hits = Arc::new(AtomicUsize::new(0));
    let kernel = kernel_with(counting("inner", true, Arc::clone(&hits)));
    let cap = Capability::root();

    for _ in 0..2 {
        block_on(kernel.issue(Request::new(Verb::Source, iri("urn:demo:outer")), &cap)).unwrap();
    }

    assert_eq!(hits.load(Ordering::SeqCst), 1, "outer served from cache");
}
