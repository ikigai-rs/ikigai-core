//! Review reproductions for PR 333 (ledger #3769): a grant follows a NAME prefix, not an
//! enumerated endpoint.
//!
//! Added unchanged first, where both passed (the defects); their final assertions are now
//! flipped: a grant is given to the endpoint the host enumerated, and a prefix covers a name
//! only on a segment boundary, so each delegated hop below is refused.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Capability, EndpointSpace, Error, Exact, FnEndpoint, Iri, Kernel, ReprType,
    Representation, Request, UriTemplate, Verb,
};

const PRICES_READ: &str = "urn:cap:fs:read:/srv/prices.json";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn file_door() -> FnEndpoint {
    FnEndpoint::new("file", |inv| {
        if !inv.capability.allows(PRICES_READ) {
            return Err(Error::Denied("no grant".into()));
        }
        Ok(Representation::new(
            ReprType::new("text/plain"),
            b"secret prices".to_vec(),
        ))
    })
}

/// A generic endpoint the host did NOT name in its grant: it reads whatever the caller asks
/// for (`urn:x:{target}`-style), under the grant if it has one.
fn generic() -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("generic", |inv| {
        Box::pin(async move {
            let corridor =
                Arc::new(EndpointSpace::new().bind(Exact::new("urn:file:prices"), file_door()));
            inv.confine(iri("urn:ctx:generic"), corridor)
                .source_granted(&iri("urn:file:prices"))
                .await
        })
    })
}

#[test]
fn a_template_binding_under_the_prefix_gets_the_dashboards_grant() {
    let dashboard = FnEndpoint::new("dashboard", |_| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            b"aggregate".to_vec(),
        ))
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:dash:prices"), dashboard)
            // A second module whose pattern happens to cover names under the prefix.
            .bind(
                UriTemplate::parse("urn:dash:{anything}").unwrap(),
                generic(),
            ),
    ))
    .with_grants(&Capability::root(), [("urn:dash:", [PRICES_READ])]);
    let nobody = Capability::root().attenuate(["urn:cap:unrelated"]);
    let err = block_on(kernel.issue(
        Request::new(Verb::Source, iri("urn:dash:whatever")),
        &nobody,
    ))
    .expect_err("the generic endpoint must not act under the dashboard's grant");
    assert!(matches!(err, Error::Denied(_)), "{err:?}");
    assert_eq!(kernel.grant_for(&iri("urn:dash:whatever")), None);
    assert_eq!(
        kernel.grant_for(&iri("urn:dash:prices")),
        Some(vec![PRICES_READ.to_string()]),
        "the door the host enumerated keeps its grant"
    );
}

#[test]
fn a_prefix_without_a_segment_boundary_covers_a_sibling_namespace() {
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:dashboard-other:x"), generic()),
    ))
    .with_grants(&Capability::root(), [("urn:dash", [PRICES_READ])]);
    let nobody = Capability::root().attenuate(["urn:cap:unrelated"]);
    let err = block_on(kernel.issue(
        Request::new(Verb::Source, iri("urn:dashboard-other:x")),
        &nobody,
    ))
    .expect_err("`urn:dash` must not grant `urn:dashboard-other:`");
    assert!(matches!(err, Error::Denied(_)), "{err:?}");
    assert_eq!(kernel.grant_for(&iri("urn:dashboard-other:x")), None);
}
