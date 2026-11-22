//! Baseline for the capability-independent cache design: distinct admitted
//! capabilities currently compute separate entries, and the floor still denies.
use futures::executor::block_on;
use ikigai_core::{
    Capability, Description, EndpointSpace, Error, Exact, FnEndpoint, Iri, Kernel, ReprType,
    Representation, Request, Verb,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[test]
fn distinct_admitted_capabilities_compute_separately() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let endpoint = FnEndpoint::new("public origin", move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(Representation::new(ReprType::new("text/plain"), "public").cacheable())
    })
    .with_description(
        Description::new("public origin")
            .verb(Verb::Source)
            .requires("urn:cap:net:read"),
    );
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:test:origin"), endpoint),
    ));
    let request = || Request::new(Verb::Source, Iri::parse("urn:test:origin").unwrap());
    let cap =
        |extra: &str| Capability::scoped(vec!["urn:cap:net:read".to_string(), extra.to_string()]);
    let a = cap("urn:cap:grant:a");
    let b = cap("urn:cap:grant:b");
    for capability in [&a, &a, &b, &b] {
        assert_eq!(
            block_on(kernel.issue(request(), capability)).unwrap().bytes,
            b"public"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let denied = Capability::scoped(vec!["urn:cap:grant:c".to_string()]);
    assert!(matches!(
        block_on(kernel.issue(request(), &denied)),
        Err(Error::Denied(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
