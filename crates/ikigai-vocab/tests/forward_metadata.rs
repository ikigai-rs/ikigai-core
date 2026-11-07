//! Ledger #2662: forwarding metadata is data in the real Meta faces and catalog.
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    Alias, AliasTable, ArgRef, Capability, Description, EndpointSpace, Exact, Fallback, FnEndpoint,
    Iri, Kernel, ReprType, Representation, Request, TopologyRule, Verb,
};
use ikigai_vocab::TurtleRenderer;
use oxrdf::{Literal, NamedNode, Term, Triple};

const IK: &str = "https://ikigai-rs.dev/ns#";

fn kernel() -> Kernel {
    let target = FnEndpoint::new("eval", |_| {
        Ok(Representation::new(ReprType::new("text/plain"), vec![]))
    })
    .with_description(Description::new("eval").verb(Verb::Source));
    let names = AliasTable::new()
        .exact_at_root("urn:example:view", "urn:plan:eval")
        .with_rule(
            TopologyRule::named_request("urn:example:named", "urn:plan:eval").verb(Verb::Source),
        )
        .unwrap()
        .with_rule(
            TopologyRule::named_request("urn:example:missing", "urn:plan:missing").verb(Verb::Sink),
        )
        .unwrap();
    // Separate tables produce two forwarding endpoints. Rules within one table
    // are composed into a single Forward by the alias resolver.
    let outer = Alias::new(
        Arc::new(AliasTable::new().exact_at_root("urn:example:chain", "urn:example:named")),
        Arc::new(EndpointSpace::new()),
    );
    let root = Fallback::new(vec![
        Arc::new(outer),
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:plan:eval"), target)),
        Arc::new(Alias::new(Arc::new(names), Arc::new(EndpointSpace::new()))),
    ]);
    Kernel::with_meta_renderer(Arc::new(root), Arc::new(TurtleRenderer))
}

fn face(kernel: &Kernel, verb: Verb, target: &str, media_type: &str) -> Vec<u8> {
    let request = Request::new(verb, Iri::parse(target).unwrap())
        .with_arg("as", ArgRef::Inline(media_type.as_bytes().to_vec()));
    block_on(kernel.issue(request, &Capability::root()))
        .unwrap()
        .bytes
}

fn triples(bytes: &[u8]) -> Vec<Triple> {
    oxttl::TurtleParser::new()
        .for_slice(bytes)
        .collect::<Result<_, _>>()
        .unwrap()
}

fn objects(graph: &[Triple], subject: &str, predicate: &str) -> Vec<Term> {
    graph
        .iter()
        .filter(|t| t.subject.to_string() == format!("<{subject}>"))
        .filter(|t| t.predicate.as_str() == format!("{IK}{predicate}"))
        .map(|t| t.object.clone())
        .collect()
}

#[test]
fn meta_json_and_turtle_agree_on_named_unbound_chained_missing_and_plain_endpoints() {
    let kernel = kernel();
    for (name, target, verb) in [
        ("urn:example:named", Some("urn:plan:eval"), Some("Source")),
        ("urn:example:view", Some("urn:plan:eval"), None),
        ("urn:example:chain", Some("urn:example:named"), None),
        (
            "urn:example:missing",
            Some("urn:plan:missing"),
            Some("Sink"),
        ),
        ("urn:plan:eval", None, None),
    ] {
        let json: serde_json::Value =
            serde_json::from_slice(&face(&kernel, Verb::Meta, name, "application/json")).unwrap();
        assert_eq!(
            json.get("forward_target").and_then(|v| v.as_str()),
            target,
            "{name}"
        );
        assert_eq!(
            json.get("forward_verb").and_then(|v| v.as_str()),
            verb,
            "{name}"
        );
        if target.is_none() {
            assert!(json.get("forward_target").is_none());
        }
        if verb.is_none() {
            assert!(json.get("forward_verb").is_none());
        }
        let subject = format!(
            "urn:ikigai:endpoint:{}",
            ikigai_core::encode_name(json["id"].as_str().unwrap())
        );
        let graph = triples(&face(&kernel, Verb::Meta, name, "text/turtle"));
        assert_eq!(
            objects(&graph, &subject, "forwardTarget"),
            target
                .map(|s| Term::NamedNode(NamedNode::new(s).unwrap()))
                .into_iter()
                .collect::<Vec<_>>(),
            "{name}"
        );
        assert_eq!(
            objects(&graph, &subject, "forwardVerb"),
            verb.map(|s| Term::Literal(Literal::new_simple_literal(s)))
                .into_iter()
                .collect::<Vec<_>>(),
            "{name}"
        );
    }
}

#[test]
fn the_turtle_catalog_carries_the_named_requests_structured_forward() {
    let graph = triples(&face(
        &kernel(),
        Verb::Source,
        "urn:kernel:catalog",
        "text/turtle",
    ));
    let subject = "urn:ikigai:endpoint:urn:example:named";
    assert_eq!(
        objects(&graph, subject, "forwardTarget"),
        vec![Term::NamedNode(NamedNode::new("urn:plan:eval").unwrap())]
    );
    assert_eq!(
        objects(&graph, subject, "forwardVerb"),
        vec![Term::Literal(Literal::new_simple_literal("Source"))]
    );
    assert!(objects(&graph, "urn:ikigai:endpoint:eval", "forwardTarget").is_empty());
}
