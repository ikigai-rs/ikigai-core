//! Today's outer cache key does not include context observed by an overlay.
use futures::executor::block_on;
use ikigai_core::{
    Capability, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, ReprType, Representation, Request,
    Verb,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[test]
fn warm_outer_answer_skips_overlay_when_unkeyed_budget_changes() {
    // This stands in for a host-provided budget outside the request and cache key.
    let budget = Arc::new(AtomicUsize::new(10));
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_budget = Arc::clone(&budget);
    let overlay_calls = Arc::clone(&calls);
    let overlay = FnEndpoint::new("budget overlay", move |_| {
        overlay_calls.fetch_add(1, Ordering::SeqCst);
        let effective_limit = observed_budget.load(Ordering::SeqCst);
        Ok(
            Representation::new(ReprType::new("text/plain"), effective_limit.to_string())
                .cacheable(),
        )
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:test:overlay"), overlay),
    ));
    let capability = Capability::root();
    let request = || Request::new(Verb::Source, Iri::parse("urn:test:overlay").unwrap());

    assert_eq!(
        block_on(kernel.issue(request(), &capability))
            .unwrap()
            .bytes,
        b"10"
    );
    budget.store(2, Ordering::SeqCst);
    assert_eq!(
        block_on(kernel.issue(request(), &capability))
            .unwrap()
            .bytes,
        b"10"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // A cut demonstrates what the overlay would have returned in the new context.
    kernel.cut("urn:test:overlay");
    assert_eq!(
        block_on(kernel.issue(request(), &capability))
            .unwrap()
            .bytes,
        b"2"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
