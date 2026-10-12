//! **`urn:kernel:actions`' Turtle face writes each authored scope as a term a parser
//! accepts** (ledger #1356). A `requires` value that starts like an IRI was written as
//! `<escape_iri_fragment(scope)>` with no parse step, so a malformed escape (`100%`) or an
//! escaped string that is still not an IRI (`https://ok> . <urn:z`, whose authority ends
//! in a non-numeric port) made the WHOLE manifold unparseable for every consumer. The
//! catalog's projection (`ikigai-vocab`'s `iri_term`) already parsed the escaped form and
//! fell back to a literal; this pins the manifold to the same rule, so the two faces
//! spell one scope the same way.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Description, EndpointSpace, Exact, FnEndpoint, Iri, Kernel, ReprType,
    Representation, Request, Verb,
};
use oxrdf::{Term, Triple};

const IK_REQUIRES: &str = "https://ikigai-rs.dev/ns#requires";

/// The manifold's Turtle for one door requiring `scope`, asked by the root capability.
fn manifold(scope: &str) -> String {
    let endpoint = FnEndpoint::new("door", |_| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            b"x".to_vec(),
        ))
    })
    .with_description(Description::new("door").verb(Verb::Source).requires(scope));
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:x:door"), endpoint),
    ));
    let request = Request::new(Verb::Source, Iri::parse("urn:kernel:actions").unwrap())
        .with_arg("as", ArgRef::Inline(b"text/turtle".to_vec()));
    let answer = block_on(kernel.issue(request, &Capability::root())).unwrap();
    String::from_utf8(answer.bytes).unwrap()
}

fn parse(text: &str) -> Result<Vec<Triple>, String> {
    oxttl::TurtleParser::new()
        .for_reader(text.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// The door's match row: the kernel's own actions are in the manifold too.
const DOOR_MATCH: &str = "<urn:ikigai:match:source:urn:x:door>";

/// Every `ik:requires` object on the door's row of the parsed manifold.
fn requires(triples: &[Triple]) -> Vec<Term> {
    triples
        .iter()
        .filter(|t| t.subject.to_string() == DOOR_MATCH && t.predicate.as_str() == IK_REQUIRES)
        .map(|t| t.object.clone())
        .collect()
}

fn literal(s: &str) -> Term {
    Term::Literal(oxrdf::Literal::new_simple_literal(s))
}

fn named(s: &str) -> Term {
    Term::NamedNode(oxrdf::NamedNode::new(s).unwrap())
}

#[test]
fn a_scope_that_is_not_an_iri_after_escaping_is_a_literal_and_the_manifold_parses() {
    // Each of these starts like an IRI and is not one: a bare `%` that opens no escape,
    // and an injection attempt whose escaped form has a non-numeric port.
    for scope in ["urn:cap:100%", "https://ok> . <urn:z", "urn:cap:%zz"] {
        let text = manifold(scope);
        let triples = parse(&text)
            .unwrap_or_else(|e| panic!("unparseable manifold for {scope:?}: {e}\n{text}"));
        assert_eq!(requires(&triples), vec![literal(scope)], "{text}");
    }
}

#[test]
fn a_scope_that_escapes_to_an_iri_stays_an_iri_and_injects_nothing() {
    let text = manifold("urn:cap:x> . <urn:everything");
    let triples = parse(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    assert_eq!(
        requires(&triples),
        vec![named("urn:cap:x%3E%20.%20%3Curn:everything")],
        "{text}"
    );
    // No triple names the would-be injected node in any position.
    assert!(
        !triples
            .iter()
            .any(|t| t.subject.to_string() == "<urn:everything>"
                || t.object.to_string() == "<urn:everything>"),
        "{text}"
    );
}

#[test]
fn well_formed_scopes_are_unchanged() {
    for (scope, term) in [
        ("urn:cap:cal:write", named("urn:cap:cal:write")),
        // A real percent-escape is part of an authored IRI's identity: kept as written.
        ("urn:cap:a%20b", named("urn:cap:a%20b")),
        (
            "https://example.org/cap#read",
            named("https://example.org/cap#read"),
        ),
        // A descriptive label stays a literal, even one that would parse as an IRI: the
        // catalog's rule, which `ik:requires`' definition states.
        ("read the calendar", literal("read the calendar")),
        ("cap:net", literal("cap:net")),
    ] {
        let text = manifold(scope);
        let triples = parse(&text).unwrap_or_else(|e| panic!("{scope:?}: {e}\n{text}"));
        assert_eq!(requires(&triples), vec![term], "{text}");
    }
}
