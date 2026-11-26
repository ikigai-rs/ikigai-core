//! Ledger #4163: a module's fixed delegation grant (`Kernel::with_grants`, ledger #3769)
//! is stated where the module is described, in its Meta faces and in `urn:kernel:catalog`,
//! so authority a caller does not hold is never silent. Only the kernel states one: a
//! kernel with no grants describes exactly what it did before, an endpoint cannot claim a
//! grant in its own `describe()`, and a caller who cannot read the module's Meta learns
//! nothing about its grant.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Delegation, Description, Endpoint, EndpointSpace, Error, Exact, FnEndpoint,
    Iri, Kernel, ReprType, Representation, Request, UriTemplate, Verb,
};
use ikigai_vocab::TurtleRenderer;
use oxrdf::{Literal, NamedNode, Term, Triple};

const IK: &str = "https://ikigai-rs.dev/ns#";
const DASH_READ: &str = "urn:cap:dash:read";
const PRICES_READ: &str = "urn:cap:fs:read:/srv/prices.json";
const INSPECT: &str = "urn:cap:kernel:inspect";
const NODE: &str = "urn:ikigai:endpoint:dashboard:delegation:urn:dash:prices";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn dashboard(description: Description) -> FnEndpoint {
    FnEndpoint::new("dashboard", |_| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            b"ok".to_vec(),
        ))
    })
    .with_description(description)
}

fn dashboard_description() -> Description {
    Description::new("dashboard")
        .title("Dashboard")
        .verb(Verb::Source)
        .requires(DASH_READ)
        .output("text/plain")
}

fn root() -> EndpointSpace {
    EndpointSpace::new()
        .bind(
            Exact::new("urn:dash:prices"),
            dashboard(dashboard_description()),
        )
        .bind(
            Exact::new("urn:other:plain"),
            dashboard(Description::new("plain").verb(Verb::Source)),
        )
}

fn ungranted(space: EndpointSpace) -> Kernel {
    Kernel::with_meta_renderer(Arc::new(space), Arc::new(TurtleRenderer))
}

fn granted(space: EndpointSpace) -> Kernel {
    ungranted(space).with_grants(&Capability::root(), [("urn:dash:", [PRICES_READ])])
}

fn meta(kernel: &Kernel, target: &str, face: &str, cap: &Capability) -> Result<Vec<u8>, Error> {
    let request = Request::new(Verb::Meta, iri(target))
        .with_arg("as", ArgRef::Inline(face.as_bytes().to_vec()));
    block_on(kernel.issue(request, cap)).map(|rep| rep.bytes)
}

fn catalog(kernel: &Kernel, cap: &Capability) -> Result<Vec<u8>, Error> {
    block_on(kernel.issue(Request::new(Verb::Source, iri("urn:kernel:catalog")), cap))
        .map(|rep| rep.bytes)
}

fn triples(bytes: &[u8]) -> Vec<Triple> {
    oxttl::TurtleParser::new()
        .for_slice(bytes)
        .collect::<Result<_, _>>()
        .unwrap()
}

fn objects(graph: &[Triple], subject: &str, predicate: &str) -> Vec<Term> {
    let subject = NamedNode::new(subject).unwrap();
    let predicate = NamedNode::new(format!("{IK}{predicate}")).unwrap();
    graph
        .iter()
        .filter(|t| t.subject == subject.clone().into() && t.predicate == predicate)
        .map(|t| t.object.clone())
        .collect()
}

fn named(s: &str) -> Term {
    NamedNode::new(s).unwrap().into()
}

fn mentions_delegation(graph: &[Triple]) -> bool {
    graph.iter().any(|t| {
        t.predicate.as_str().starts_with(&format!("{IK}delegat"))
            || t.predicate.as_str() == format!("{IK}actsUnder")
            || t.object == named(&format!("{IK}Delegation"))
    })
}

/// The delegation the dashboard was given, as every face must state it: the door (its
/// identity), the prefix, the scopes, and that it replaces the caller's capability.
fn assert_states_the_grant(graph: &[Triple]) {
    assert_eq!(
        objects(graph, "urn:ikigai:endpoint:dashboard", "actsUnder"),
        vec![named(NODE)]
    );
    assert_eq!(
        objects(graph, NODE, "delegatedAt"),
        vec![named("urn:dash:prices")]
    );
    assert_eq!(
        objects(graph, NODE, "delegationPrefix"),
        vec![Term::from(Literal::new_simple_literal("urn:dash:"))]
    );
    assert_eq!(objects(graph, NODE, "scope"), vec![named(PRICES_READ)]);
    assert_eq!(
        objects(graph, NODE, "replacesCallerCapability"),
        vec![Term::from(Literal::from(true))]
    );
    let typed = Triple::new(
        NamedNode::new(NODE).unwrap(),
        NamedNode::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type").unwrap(),
        NamedNode::new(format!("{IK}Delegation")).unwrap(),
    );
    assert!(graph.contains(&typed), "the node is an ik:Delegation");
}

fn the_grant() -> Delegation {
    Delegation {
        door: "urn:dash:prices".into(),
        prefix: "urn:dash:".into(),
        scopes: vec![PRICES_READ.into()],
    }
}

#[test]
fn a_kernel_with_one_grant_states_it_in_every_meta_face() {
    let kernel = granted(root());
    let caller = Capability::scoped([DASH_READ]);

    let turtle = meta(&kernel, "urn:dash:prices", "text/turtle", &caller).unwrap();
    assert_states_the_grant(&triples(&turtle));

    let json = meta(&kernel, "urn:dash:prices", "application/json", &caller).unwrap();
    let back: Description = serde_json::from_slice(&json).unwrap();
    assert_eq!(back.delegations, vec![the_grant()]);

    let text = String::from_utf8(meta(&kernel, "urn:dash:prices", "text/plain", &caller).unwrap())
        .unwrap();
    assert!(
        text.contains(
            "acts under a host grant at urn:dash:prices: urn:cap:fs:read:/srv/prices.json; \
             each sub-request it issues under the grant runs with these scopes instead of the \
             caller's capability"
        ),
        "{text}"
    );

    // An endpoint given no grant says nothing new.
    let plain = triples(&meta(&kernel, "urn:other:plain", "text/turtle", &caller).unwrap());
    assert!(!mentions_delegation(&plain));
}

#[test]
fn a_kernel_with_one_grant_states_it_in_the_catalog() {
    let kernel = granted(root());
    let graph = triples(&catalog(&kernel, &Capability::scoped([INSPECT])).unwrap());
    assert_states_the_grant(&graph);
    // Exactly one delegation node in the whole catalog: the plain endpoint and the
    // kernel's own operations carry none.
    let delegations: Vec<_> = graph
        .iter()
        .filter(|t| t.predicate.as_str() == format!("{IK}actsUnder"))
        .collect();
    assert_eq!(delegations.len(), 1, "{delegations:?}");
}

#[test]
fn an_endpoint_granted_at_two_doors_is_one_contract_listing_both() {
    let space = EndpointSpace::new();
    let shared: Arc<dyn Endpoint> = Arc::new(dashboard(dashboard_description()));
    let space = space
        .bind_arc(Exact::new("urn:dash:prices"), Arc::clone(&shared))
        .bind_arc(Exact::new("urn:dash:volumes"), shared);
    let kernel = granted(space);
    let graph = triples(&catalog(&kernel, &Capability::root()).unwrap());
    let mut under = objects(&graph, "urn:ikigai:endpoint:dashboard", "actsUnder");
    under.sort_by_key(|t| t.to_string());
    assert_eq!(
        under,
        vec![
            named(NODE),
            named("urn:ikigai:endpoint:dashboard:delegation:urn:dash:volumes"),
        ]
    );
    // One contract, so the catalog does not mark the id as shared by different ones.
    let comments = graph
        .iter()
        .filter(|t| t.predicate.as_str() == "http://www.w3.org/2000/01/rdf-schema#comment")
        .count();
    assert_eq!(comments, 0, "no shared-id mark");
}

#[test]
fn a_kernel_with_no_grants_describes_exactly_what_it_did() {
    let caller = Capability::root();
    let kernel = ungranted(root());
    for face in ["text/turtle", "application/json", "text/plain"] {
        let bytes = meta(&kernel, "urn:dash:prices", face, &caller).unwrap();
        // Byte for byte what the endpoint's own description renders to, with no kernel.
        let expected = rendered_without_a_kernel(&dashboard_description(), face);
        assert_eq!(bytes, expected, "{face}");
    }
    let json = meta(&kernel, "urn:dash:prices", "application/json", &caller).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert!(value.get("delegations").is_none(), "{value}");
    assert!(!mentions_delegation(&triples(
        &catalog(&kernel, &caller).unwrap()
    )));
}

fn rendered_without_a_kernel(description: &Description, face: &str) -> Vec<u8> {
    use ikigai_core::MetaRenderer;
    TurtleRenderer
        .render(description, &ReprType::new(face))
        .unwrap()
        .bytes
}

#[test]
fn an_endpoint_cannot_claim_a_grant_in_its_own_description() {
    let mut claimed = Description::new("liar").verb(Verb::Source);
    claimed.delegations = vec![Delegation {
        door: "urn:liar".into(),
        prefix: "urn:liar".into(),
        scopes: vec!["urn:cap:everything".into()],
    }];
    let space = || root().bind(Exact::new("urn:liar"), dashboard(claimed.clone()));
    for kernel in [ungranted(space()), granted(space())] {
        let face = meta(&kernel, "urn:liar", "text/turtle", &Capability::root()).unwrap();
        assert!(!mentions_delegation(&triples(&face)));
        let json = meta(&kernel, "urn:liar", "application/json", &Capability::root()).unwrap();
        let back: Description = serde_json::from_slice(&json).unwrap();
        assert!(back.delegations.is_empty());
        let graph = triples(&catalog(&kernel, &Capability::root()).unwrap());
        assert!(objects(&graph, "urn:ikigai:endpoint:liar", "actsUnder").is_empty());
    }
}

#[test]
fn a_name_under_the_prefix_that_was_not_enumerated_states_no_grant() {
    // A template binding under the granted prefix answers `urn:dash:anything`, but the
    // grant was given only to the enumerated door, so its description states none.
    let space = root().bind(
        UriTemplate::parse("urn:dash:{name}").unwrap(),
        dashboard(Description::new("generic").verb(Verb::Source)),
    );
    let kernel = granted(space);
    let face = meta(
        &kernel,
        "urn:dash:anything",
        "text/turtle",
        &Capability::root(),
    )
    .unwrap();
    assert!(!mentions_delegation(&triples(&face)));
    let graph = triples(&catalog(&kernel, &Capability::root()).unwrap());
    assert!(objects(&graph, "urn:ikigai:endpoint:generic", "actsUnder").is_empty());
    assert_states_the_grant(&graph);
}

#[test]
fn a_caller_who_cannot_read_the_modules_meta_never_sees_its_grant() {
    // An existence-sensitive module: a caller the floor refuses gets the answer a name
    // bound nowhere gets, for Meta too, so the grant cannot be read from it.
    let hidden = dashboard_description().existence_sensitive();
    let space = EndpointSpace::new().bind(Exact::new("urn:dash:prices"), dashboard(hidden));
    let kernel = granted(space);
    let stranger = Capability::scoped(["urn:cap:unrelated"]);
    for face in ["text/turtle", "application/json", "text/plain"] {
        match meta(&kernel, "urn:dash:prices", face, &stranger) {
            Err(Error::Unresolved(_)) => {}
            other => panic!("{face}: expected a miss, got {other:?}"),
        }
    }
    // The catalog needs inspect authority; without it, nothing is described.
    match catalog(&kernel, &stranger) {
        Err(Error::Denied(_)) => {}
        other => panic!("expected the catalog to be denied, got {other:?}"),
    }
    // The module's own callers do read it, grant included.
    let caller = Capability::scoped([DASH_READ]);
    let turtle = meta(&kernel, "urn:dash:prices", "text/turtle", &caller).unwrap();
    assert_states_the_grant(&triples(&turtle));
}
