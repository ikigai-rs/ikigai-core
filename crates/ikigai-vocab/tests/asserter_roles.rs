//! Claim asserters (ledger #4511, design: `docs/design/claim-asserters.md`, status proposed).
//!
//! The question: on a claim ikigai emits (a hand-back's evidence, a verdict, a credential, a
//! cited answer, a log entry), can a reader tell in what standing its asserter made it: the
//! acting agent, the principal it acts for, a person reviewing, a deterministic check, a model
//! or a third party?
//!
//! `today_the_standing_is_not_in_the_graph` pins the gap on the published vocabulary: the
//! only attribution a claim carries is `prov:wasAttributedTo`, which names WHO, and the same
//! person IRI is a principal on one claim and a reviewer on the next, so the standing cannot
//! be read from the agent's type either. The draft's terms are not declared yet, and nothing
//! in the vocabulary is a `prov:Role`. When the terms are published, flip the first test to
//! read them from `VOCABULARY` instead of the note.
//!
//! The other tests hold the draft in the note to the conventions `vocabulary.ttl` states, so
//! a publish copies terms that already pass them: it parses, every term is labeled and
//! commented, the property declares no domain and ranges over `skos:Concept`, no drafted
//! name collides with a published one, and the scheme's concepts are complete and distinct.

use std::collections::BTreeSet;

use ikigai_vocab::{NS, VOCABULARY};
use oxrdf::{NamedOrBlankNode, Term, Triple};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const PROV: &str = "http://www.w3.org/ns/prov#";
const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";

const PREFIXES: &str = "\
@prefix ik:   <https://ikigai-rs.dev/ns#> .
@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix prov: <http://www.w3.org/ns/prov#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
";

/// The draft's concepts, by local name and notation.
const ROLES: [(&str, &str); 6] = [
    ("AgentAsserter", "agent"),
    ("PrincipalAsserter", "principal"),
    ("ReviewerAsserter", "reviewer"),
    ("CheckAsserter", "check"),
    ("ModelAsserter", "model"),
    ("ThirdPartyAsserter", "third-party"),
];

fn ik(term: &str) -> String {
    format!("{NS}{term}")
}

fn parse(ttl: &str) -> Vec<Triple> {
    oxttl::TurtleParser::new()
        .for_reader(ttl.as_bytes())
        .map(|t| t.expect("valid turtle"))
        .collect()
}

fn named(subject: &NamedOrBlankNode) -> Option<&str> {
    match subject {
        NamedOrBlankNode::NamedNode(n) => Some(n.as_str()),
        _ => None,
    }
}

fn objects(graph: &[Triple], subject: &str, predicate: &str) -> Vec<String> {
    graph
        .iter()
        .filter(|t| named(&t.subject) == Some(subject) && t.predicate.as_str() == predicate)
        .map(|t| match &t.object {
            Term::NamedNode(n) => n.as_str().to_string(),
            Term::Literal(l) => l.value().to_string(),
            other => other.to_string(),
        })
        .collect()
}

fn subjects(graph: &[Triple]) -> BTreeSet<String> {
    graph
        .iter()
        .filter_map(|t| named(&t.subject).map(String::from))
        .collect()
}

/// The drafted terms: the one ```turtle block of the design note that declares
/// `ik:asserterRole`, parsed under the vocabulary's prefixes.
fn draft() -> Vec<Triple> {
    let path = format!(
        "{}/../../docs/design/claim-asserters.md",
        env!("CARGO_MANIFEST_DIR")
    );
    let note = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let blocks: Vec<&str> = note
        .split("```turtle\n")
        .skip(1)
        .filter_map(|b| b.split("\n```").next())
        .filter(|b| b.contains("ik:asserterRole a rdf:Property"))
        .collect();
    assert_eq!(
        blocks.len(),
        1,
        "exactly one block in the note drafts the terms"
    );
    parse(&format!("{PREFIXES}{}", blocks[0]))
}

#[test]
fn today_the_standing_is_not_in_the_graph() {
    let vocabulary = parse(VOCABULARY);
    let declared = subjects(&vocabulary);

    // The draft's terms are not published.
    for term in ["asserterRole", "AsserterRoles"]
        .into_iter()
        .chain(ROLES.iter().map(|(name, _)| *name))
    {
        assert!(
            !declared.contains(&ik(term)),
            "ik:{term} is already declared"
        );
    }
    // Nothing in the vocabulary is a PROV role a qualified attribution could carry.
    let roles: Vec<_> = vocabulary
        .iter()
        .filter(|t| {
            t.predicate.as_str() == RDF_TYPE
                && t.object
                    == Term::NamedNode(oxrdf::NamedNode::new_unchecked(format!("{PROV}Role")))
        })
        .collect();
    assert!(
        roles.is_empty(),
        "the vocabulary types a prov:Role: {roles:?}"
    );

    // What a reader holds today: two claims attributed to the same person, who is the
    // principal on the brief and the reviewer on the verdict. The only attribution term is
    // prov:wasAttributedTo, and the agent's type is the same on both, so nothing in the
    // graph tells the two standings apart.
    let graph = parse(&format!(
        "{PREFIXES}
        <urn:person:brian> a foaf:Person .
        <urn:layer:work:entry:01> a ik:Brief ; prov:wasAttributedTo <urn:person:brian> .
        <urn:judge:verdict:01> a prov:Entity ; prov:wasAttributedTo <urn:person:brian> .
        "
    ));
    let attribution = |claim: &str| {
        let agents = objects(&graph, claim, &format!("{PROV}wasAttributedTo"));
        let types: Vec<_> = agents
            .iter()
            .flat_map(|a| objects(&graph, a, RDF_TYPE))
            .collect();
        (agents, types)
    };
    assert_eq!(
        attribution("urn:layer:work:entry:01"),
        attribution("urn:judge:verdict:01"),
        "the graph already tells a principal's claim from a reviewer's"
    );
}

#[test]
fn the_drafted_property_follows_the_vocabulary_conventions() {
    let draft = draft();
    let property = ik("asserterRole");
    assert_eq!(
        objects(&draft, &property, RDF_TYPE),
        ["http://www.w3.org/1999/02/22-rdf-syntax-ns#Property".to_string()]
    );
    // No domain: a hand-back, a verdict, a credential, a citation and a log entry all carry it.
    assert!(objects(&draft, &property, &format!("{RDFS}domain")).is_empty());
    // A concept range, so the JSON-LD generator coerces it to @id.
    assert_eq!(
        objects(&draft, &property, &format!("{RDFS}range")),
        [format!("{SKOS}Concept")]
    );
    for term in [property.clone(), ik("AsserterRoles")] {
        assert_eq!(
            objects(&draft, &term, &format!("{RDFS}label")).len(),
            1,
            "{term} label"
        );
        assert_eq!(
            objects(&draft, &term, &format!("{RDFS}comment")).len(),
            1,
            "{term} comment"
        );
    }
}

#[test]
fn the_drafted_roles_are_one_complete_scheme_of_distinct_prov_roles() {
    let draft = draft();
    let scheme = ik("AsserterRoles");
    assert_eq!(
        objects(&draft, &scheme, RDF_TYPE),
        [format!("{SKOS}ConceptScheme")]
    );

    let top: BTreeSet<String> = objects(&draft, &scheme, &format!("{SKOS}hasTopConcept"))
        .into_iter()
        .collect();
    let expected: BTreeSet<String> = ROLES.iter().map(|(name, _)| ik(name)).collect();
    assert_eq!(top, expected, "the scheme lists exactly the six roles");

    let mut notations = BTreeSet::new();
    for (name, notation) in ROLES {
        let concept = ik(name);
        let types: BTreeSet<String> = objects(&draft, &concept, RDF_TYPE).into_iter().collect();
        assert_eq!(
            types,
            [format!("{SKOS}Concept"), format!("{PROV}Role")]
                .into_iter()
                .collect(),
            "{concept} is a skos:Concept and a prov:Role"
        );
        assert_eq!(
            objects(&draft, &concept, &format!("{SKOS}inScheme")),
            std::slice::from_ref(&scheme)
        );
        assert_eq!(
            objects(&draft, &concept, &format!("{SKOS}topConceptOf")),
            std::slice::from_ref(&scheme)
        );
        assert_eq!(
            objects(&draft, &concept, &format!("{SKOS}notation")),
            [notation]
        );
        for property in ["prefLabel", "definition"] {
            let found = objects(&draft, &concept, &format!("{SKOS}{property}"));
            assert_eq!(found.len(), 1, "{concept} has one skos:{property}");
            assert!(!found[0].trim().is_empty());
        }
        assert!(
            notations.insert(notation),
            "notation {notation} is used twice"
        );
    }
}

#[test]
fn no_drafted_term_collides_with_a_published_one() {
    let declared = subjects(&parse(VOCABULARY));
    let drafted: Vec<String> = subjects(&draft())
        .into_iter()
        .filter(|s| s.starts_with(NS))
        .collect();
    assert_eq!(
        drafted.len(),
        2 + ROLES.len(),
        "the draft declares {drafted:?}"
    );
    // Case-insensitively too: ik:layer and ik:Layer are already two terms nobody should add to.
    let published: BTreeSet<String> = declared.iter().map(|s| s.to_lowercase()).collect();
    for term in drafted {
        assert!(
            !published.contains(&term.to_lowercase()),
            "{term} is already a published term (in some case)"
        );
    }
}
