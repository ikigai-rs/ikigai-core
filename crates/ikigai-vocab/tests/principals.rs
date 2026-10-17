//! The principal family (ledger #1821): the `ik:` terms for who a door admits and for
//! what. A principal is an IRI, separate from the credentials that authenticate it
//! (`ik:Credential`, `ik:authenticatedBy`, `ik:authMethod`); a principal has a kind
//! (`foaf:Person`, `prov:SoftwareAgent`, `ik:Machine` or `ik:BlindedPrincipal`); and what
//! a door admits it for is one `ik:Grant` (`ik:grantee`, `ik:scope`, `ik:notAfter`).
//! Acting for someone stays `prov:actedOnBehalfOf` in the principals graph.
//!
//! Every domain and range the vocabulary declares is run over an example principals
//! graph (`tests/fixtures/principals-example.ttl`) whose nodes assert the types they
//! really have, as `layers.rs` does for the layer family, and each domain or range this
//! family declines is shown to be load-bearing: declare it, and the example traps.

mod common;

use std::collections::BTreeSet;

use common::{
    axiom, block, declared, fixture, ik, named, parse, Schema, PROV, RDFS, RDF_TYPE, XSD,
};
use ikigai_vocab::{CONTEXT, VOCABULARY};
use oxrdf::{NamedOrBlankNode, Term, Triple};

const FOAF: &str = "http://xmlns.com/foaf/0.1/";

const CLASSES: [&str; 3] = ["BlindedPrincipal", "Credential", "Grant"];

/// Each property with the domain it declares (only `grantee`) and its range.
const PROPERTIES: [(&str, Option<&str>, &str); 5] = [
    ("authenticatedBy", None, "ik:Credential"),
    ("authMethod", None, "rdfs:Resource"),
    ("grantee", Some("ik:Grant"), "rdfs:Resource"),
    ("scope", None, "rdfs:Resource"),
    ("notAfter", None, "xsd:dateTime"),
];

fn example() -> Vec<Triple> {
    fixture("principals-example.ttl")
}

#[test]
fn the_principal_family_is_defined() {
    let subjects = declared();
    for term in CLASSES
        .iter()
        .copied()
        .chain(PROPERTIES.iter().map(|(p, _, _)| *p))
    {
        assert!(
            subjects.contains(&ik(term)),
            "ik:{term} is a principal term (ledger #1821) and is not declared"
        );
    }
    for class in CLASSES {
        let b = block(class, "rdfs:Class");
        assert!(
            b.contains("rdfs:label") && b.contains("rdfs:comment"),
            "{b}"
        );
    }
    for (property, domain, range) in PROPERTIES {
        let b = block(property, "rdf:Property");
        assert_eq!(
            axiom(b, "domain"),
            domain.map(|d| format!("{d} ;")).as_deref(),
            "ik:{property}:\n{b}"
        );
        assert_eq!(
            axiom(b, "range"),
            Some(format!("{range} ;").as_str()),
            "ik:{property}:\n{b}"
        );
    }
    let schema = Schema::load();
    for (class, of) in [
        ("BlindedPrincipal", format!("{PROV}Agent")),
        ("Credential", format!("{PROV}Entity")),
        ("Grant", format!("{PROV}Entity")),
        // The machine principal is the layer family's, related to rather than redefined.
        ("Machine", format!("{PROV}SoftwareAgent")),
    ] {
        assert!(schema.is_a(&ik(class), &of), "ik:{class} ⊑ {of}");
    }
    // A blinded principal withholds who stands behind it, so it is not typed a person;
    // a credential is not a principal; and a grant is not a credential.
    for (class, not) in [
        ("BlindedPrincipal", format!("{FOAF}Person")),
        ("BlindedPrincipal", format!("{PROV}Entity")),
        ("Credential", format!("{PROV}Agent")),
        ("Grant", ik("Credential")),
        ("Credential", ik("Grant")),
    ] {
        assert!(
            !schema.is_a(&ik(class), &not),
            "ik:{class} must not be ⊑ {not}"
        );
    }
    assert!(
        block("BlindedPrincipal", "rdfs:Class").contains("NOT decided yet"),
        "the blinded principal's construction is an open decision, and its comment says so"
    );
}

#[test]
fn the_context_coerces_every_principal_term() {
    let core: serde_json::Value = serde_json::from_str(CONTEXT).unwrap();
    let core = core["@context"].as_object().unwrap();
    for class in CLASSES {
        assert_eq!(
            core.get(class).and_then(|v| v.as_str()),
            Some(format!("ik:{class}").as_str()),
            "the context names ik:{class}"
        );
    }
    for (property, coercion) in [
        ("authenticatedBy", "@id"),
        ("authMethod", "@id"),
        ("grantee", "@id"),
        ("scope", "@id"),
        ("notAfter", "xsd:dateTime"),
    ] {
        let v = core
            .get(property)
            .unwrap_or_else(|| panic!("the context has no `{property}`"));
        assert_eq!(v["@id"].as_str(), Some(format!("ik:{property}").as_str()));
        assert_eq!(v["@type"].as_str(), Some(coercion), "`{property}` coerces");
    }
}

#[test]
fn the_example_types_every_node_the_family_describes() {
    let triples = example();
    assert!(
        triples
            .iter()
            .all(|t| matches!(t.subject, NamedOrBlankNode::NamedNode(_))
                && !matches!(t.object, Term::BlankNode(_))),
        "skolemized: no blank nodes"
    );
    let typed: BTreeSet<&str> = triples
        .iter()
        .filter(|t| t.predicate.as_str() == RDF_TYPE)
        .filter_map(|t| named(&t.subject))
        .collect();
    let used: BTreeSet<&str> = triples.iter().map(|t| t.predicate.as_str()).collect();
    for (property, _, _) in PROPERTIES {
        assert!(
            used.contains(ik(property).as_str()),
            "the example uses ik:{property}, so its domain and range are checked"
        );
    }
    let asserted: BTreeSet<String> = triples
        .iter()
        .filter(|t| t.predicate.as_str() == RDF_TYPE)
        .map(|t| t.object.to_string())
        .collect();
    for class in CLASSES.iter().chain(&["Machine"]) {
        assert!(
            asserted.contains(&format!("<{}>", ik(class))),
            "the example has an ik:{class}"
        );
    }
    for t in &triples {
        let p = t.predicate.as_str();
        if PROPERTIES.iter().any(|(q, _, _)| ik(q) == p) {
            let s = named(&t.subject).unwrap();
            assert!(typed.contains(s), "{s} carries {p} and asserts no type");
        }
        // A principal or a credential named by a family property asserts its type, so
        // the range is judged against something.
        if p == ik("authenticatedBy") || p == ik("grantee") {
            let Term::NamedNode(o) = &t.object else {
                panic!("{p} names a node by IRI")
            };
            assert!(
                typed.contains(o.as_str()),
                "{o} is named by {p} and asserts no type"
            );
        }
        if p == ik("notAfter") {
            let Term::Literal(l) = &t.object else {
                panic!("ik:notAfter is a literal")
            };
            assert_eq!(l.datatype().as_str(), format!("{XSD}dateTime"));
        }
    }
}

#[test]
fn no_declared_domain_or_range_types_an_example_node_into_a_family_it_is_not_in() {
    // Every domain and range in the vocabulary, not only this family's: the layer
    // family's ik:Machine and PROV's own terms are judged the same way.
    let traps = Schema::load().traps(&example());
    assert!(traps.is_empty(), "family traps:\n{}", traps.join("\n"));
}

#[test]
fn the_trap_check_sees_a_mistyping() {
    let schema = Schema::load();
    let prefix = "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
                  @prefix foaf: <http://xmlns.com/foaf/0.1/> .\n";
    for (data, names) in [
        // A credential is not a grant: giving one a grantee types it a grant.
        (
            "<urn:example:credential:x> a ik:Credential ; ik:grantee <urn:example:p> .",
            "Grant",
        ),
        // A principal is not its credential.
        (
            "<urn:example:p> a foaf:Person ; ik:authenticatedBy <urn:example:q> .\n\
             <urn:example:q> a foaf:Person .",
            "Credential",
        ),
        (
            "<urn:example:p> a foaf:Person ; ik:authenticatedBy \"a passkey\" .",
            "Credential",
        ),
    ] {
        let traps = schema.traps(&parse(&format!("{prefix}{data}")));
        assert!(
            traps.iter().any(|t| t.contains(&ik(names))),
            "{data} must trap into ik:{names}; got {traps:?}"
        );
    }
    // The domains this family declines are load-bearing: give a shared property the
    // domain of one of the classes carrying it, and the example traps.
    for (property, domain) in [
        ("notAfter", ik("Grant")),
        ("notAfter", ik("Credential")),
        ("authMethod", ik("Credential")),
        ("authenticatedBy", format!("{FOAF}Person")),
        ("authenticatedBy", ik("Machine")),
    ] {
        let mut narrowed = Schema::load();
        narrowed.domains.insert(ik(property), domain.clone());
        let traps = narrowed.traps(&example());
        assert!(
            traps.iter().any(|t| t.starts_with(&ik(property))),
            "rdfs:domain {domain} on ik:{property} would type another node one; got {traps:?}"
        );
    }
    // And so is the declined range: a shared default's grantee is a class, which an agent
    // range would type an agent.
    let mut narrowed = Schema::load();
    narrowed
        .ranges
        .insert(ik("grantee"), format!("{PROV}Agent"));
    let traps = narrowed.traps(&example());
    assert!(
        traps
            .iter()
            .any(|t| t.starts_with(&ik("grantee")) && t.contains(&format!("{RDFS}Class"))),
        "rdfs:range prov:Agent on ik:grantee would type the default grant's class an agent; \
         got {traps:?}"
    );
}

#[test]
fn the_principals_graph_generalizes_the_participants_graph() {
    // The comment block says so, and the vocabulary keeps acting-for as PROV's term: a
    // lookup in this graph, never a second capability scope.
    assert!(VOCABULARY.contains("generalizes the layer host's participants.ttl"));
    assert!(VOCABULARY.contains("Acting for someone is a LOOKUP in this graph"));
    let acting_for = format!("{PROV}actedOnBehalfOf");
    assert!(
        !declared()
            .iter()
            .any(|s| s.ends_with("actedOnBehalfOf") || *s == acting_for),
        "acting-for is prov:actedOnBehalfOf, not an ik: copy of it"
    );
    assert!(
        example().iter().any(|t| t.predicate.as_str() == acting_for),
        "the example's agent and machine act for their person"
    );
}
