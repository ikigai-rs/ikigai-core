//! **A space's name survives only what keeps its doors** (ledger #987).
//!
//! A name is a cache claim: any space named `n` holds the same doors. A module
//! that self-names its configuration-free `space()` (`urn:iki:space:fn`, say) is
//! routinely EXTENDED by a host — `ikigai_fn::space().bind(extra, ..)` — and that
//! extended space holds different doors. If `bind` kept the name, the extended
//! space would claim it falsely, and nothing would panic: the cache would share
//! answers between the two corridors, and a tree holding both would be refused as
//! one name claimed twice. So adding a door drops the claim, and an explicit
//! `.named(..)` after extending names it again.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    Capability, EndpointSpace, Exact, Fallback, FnEndpoint, Iri, Kernel, ReprType, Representation,
    Request, Scope, Space, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn constant(name: &'static str, body: &'static [u8]) -> FnEndpoint {
    FnEndpoint::new(name, move |_inv| {
        Ok(Representation::new(ReprType::new("text/plain"), body.to_vec()).cacheable())
    })
}

/// The module's configuration-free space, named as the convention says.
fn module_space() -> EndpointSpace {
    EndpointSpace::new()
        .bind(Exact::new("urn:mod:a"), constant("a", b"a"))
        .named(iri("urn:iki:space:mod"))
}

#[test]
fn binding_a_door_drops_the_claim() {
    assert_eq!(module_space().id(), Some(iri("urn:iki:space:mod")));
    let extended = module_space().bind(Exact::new("urn:mod:b"), constant("b", b"b"));
    assert_eq!(
        extended.id(),
        None,
        "more doors, so the old name is no longer true"
    );
}

#[test]
fn binding_a_shared_endpoint_drops_the_claim() {
    let shared: Arc<dyn ikigai_core::Endpoint> = Arc::new(constant("b", b"b"));
    let extended = module_space().bind_arc(Exact::new("urn:mod:b"), shared);
    assert_eq!(extended.id(), None);
}

#[test]
fn naming_after_extending_names_the_extended_space() {
    let renamed = module_space()
        .bind(Exact::new("urn:mod:b"), constant("b", b"b"))
        .named(iri("urn:iki:space:mod:extended"));
    assert_eq!(renamed.id(), Some(iri("urn:iki:space:mod:extended")));
}

#[test]
fn an_unnamed_space_stays_unnamed_when_extended() {
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:mod:a"), constant("a", b"a"))
        .bind(Exact::new("urn:mod:b"), constant("b", b"b"));
    assert_eq!(space.id(), None);
}

/// The consequence the claim had: a corridor's name partitions the cache, so the
/// extended space, carrying the plain one's name, served its own answer to a
/// request made under the plain one. Here `urn:mod:b` is the extended space's extra
/// door and also a root door; under the plain corridor it must reach the root.
#[test]
fn the_extended_space_does_not_share_the_plain_ones_cache_entries() {
    let root =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:mod:b"), constant("root", b"root")));
    let kernel = Kernel::new(root);
    let cap = Capability::root();
    let read = |scope: Scope| {
        block_on(kernel.issue_in(Request::new(Verb::Source, iri("urn:mod:b")), &cap, scope))
            .unwrap()
            .bytes
    };

    let extended = module_space().bind(Exact::new("urn:mod:b"), constant("b", b"extended"));
    assert_eq!(read(Scope::empty().with(Arc::new(extended))), b"extended");
    assert_eq!(
        read(Scope::empty().with(Arc::new(module_space()))),
        b"root",
        "the plain corridor has no `urn:mod:b` door, so the root answers it"
    );
}

/// The other consequence: one tree holding the plain space and the extended one
/// would carry one name over two door sets, which the topology refuses.
#[test]
fn a_tree_can_hold_the_plain_and_the_extended_space() {
    let extended = module_space().bind(Exact::new("urn:mod:b"), constant("b", b"b"));
    let tree = Fallback::new(vec![Arc::new(module_space()), Arc::new(extended)]);
    tree.topology()
        .try_to_turtle()
        .expect("one name, one door set: the extended space is anonymous");
}
