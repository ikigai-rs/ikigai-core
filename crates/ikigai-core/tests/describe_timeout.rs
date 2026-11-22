//! Minimal core-side model of an enumerated peer row whose metadata timed out.
//! The peer adapter is outside this repository; the fixture supplies its placeholder.

use futures::executor::block_on;
use ikigai_core::{
    Capability, Description, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, MetaRenderer, ReprType,
    Representation, Request, Verb,
};
use std::sync::Arc;

struct IdRenderer;
impl MetaRenderer for IdRenderer {
    fn render(&self, d: &Description, _: &ReprType) -> ikigai_core::Result<Representation> {
        Ok(Representation::new(
            ReprType::new("text/turtle"),
            d.id.as_bytes().to_vec(),
        ))
    }
}

fn endpoint(description: Description) -> FnEndpoint {
    FnEndpoint::new("peer", |_| {
        Err(ikigai_core::Error::Timeout("metadata".into()))
    })
    .with_description(description)
}

#[test]
fn enumerated_placeholder_is_in_catalog_but_not_manifold() {
    // Model the adapter after Entries succeeded and the first row's Meta timed out.
    // A second row is described successfully, proving enumeration reaches the walk.
    let space = EndpointSpace::new()
        .bind(
            Exact::new("urn:peer:slow"),
            endpoint(Description::new("urn:peer:slow").contract_volatile()),
        )
        .bind(
            Exact::new("urn:peer:ready"),
            endpoint(Description::new("urn:peer:ready").verb(Verb::Source)),
        );
    let kernel = Kernel::with_meta_renderer(Arc::new(space), Arc::new(IdRenderer));
    let read = |target| {
        String::from_utf8(
            block_on(kernel.issue(
                Request::new(Verb::Source, Iri::parse(target).unwrap()),
                &Capability::root(),
            ))
            .unwrap()
            .bytes,
        )
        .unwrap()
    };
    let catalog = read("urn:kernel:catalog");
    let actions = read("urn:kernel:actions");
    assert!(catalog.contains("urn:peer:slow"));
    assert!(catalog.contains("urn:peer:ready"));
    assert!(actions.contains("urn:peer:ready"));
    assert!(!actions.contains("urn:peer:slow"));
    assert!(!actions.contains("mount-unavailable"));
    assert!(!catalog.contains("mount-unavailable"));
}
