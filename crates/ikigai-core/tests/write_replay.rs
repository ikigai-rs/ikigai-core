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
fn replay_keeps_cached_read() {
    let reads = Arc::new(AtomicUsize::new(0));
    let writes = Arc::new(AtomicUsize::new(0));
    let endpoint = FnEndpoint::new("cell", {
        let reads = reads.clone();
        let writes = writes.clone();
        move |inv| match inv.request.verb {
            Verb::Source => {
                reads.fetch_add(1, Ordering::SeqCst);
                Ok(Representation::new(ReprType::new("text/plain"), b"value".to_vec()).cacheable())
            }
            Verb::Sink => {
                if writes.fetch_add(1, Ordering::SeqCst) > 0 {
                    inv.nothing_written();
                }
                Ok(Representation::new(
                    ReprType::new("text/plain"),
                    b"answer".to_vec(),
                ))
            }
            _ => unreachable!(),
        }
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:cell"), endpoint),
    ));
    let cap = Capability::root();
    let request = |verb| Request::new(verb, Iri::parse("urn:cell").unwrap());
    let issue = |verb| block_on(kernel.issue(request(verb), &cap)).unwrap();
    issue(Verb::Source);
    let first = issue(Verb::Sink);
    issue(Verb::Source);
    issue(Verb::Source);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    let replay = issue(Verb::Sink);
    assert_eq!(first.bytes, replay.bytes);
    issue(Verb::Source);
    assert_eq!(writes.load(Ordering::SeqCst), 2);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        2,
        "replay cut the cached read"
    );
}

#[test]
fn no_write_assertion_applies_to_failures_and_delete_but_default_still_cuts() {
    use ikigai_core::Error;
    let reads = Arc::new(AtomicUsize::new(0));
    let endpoint = FnEndpoint::new("cell", {
        let reads = reads.clone();
        move |inv| match inv.request.verb {
            Verb::Source => {
                reads.fetch_add(1, Ordering::SeqCst);
                Ok(Representation::new(ReprType::new("text/plain"), b"value".to_vec()).cacheable())
            }
            Verb::Delete => {
                inv.nothing_written();
                Err(Error::NotFound("already deleted".into()))
            }
            Verb::Sink => Err(Error::Endpoint("may have written".into())),
            _ => unreachable!(),
        }
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:cell"), endpoint),
    ));
    let cap = Capability::root();
    let issue =
        |verb| block_on(kernel.issue(Request::new(verb, Iri::parse("urn:cell").unwrap()), &cap));
    issue(Verb::Source).unwrap();
    assert!(issue(Verb::Delete).is_err());
    issue(Verb::Source).unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert!(issue(Verb::Sink).is_err());
    issue(Verb::Source).unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 2);
}
