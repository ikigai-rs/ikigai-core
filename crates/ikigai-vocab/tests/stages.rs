//! Ledger #32: a fork could not bind a name, be a fork's branch, or be mapped over,
//! because `ik:binds`, `ik:forkOf` and `ik:mapOver` had `rdfs:domain ik:Step` and a fork
//! is not a step. Asserting one of them on a fork VALIDATED (the shapes target asserted
//! types), and under RDFS entailment typed the fork an `ik:Step`: a graph that lints clean
//! and means something false. `ik:Stage` (a superclass of both) is the domain now.
//!
//! These tests pin the vocabulary half (the entailment). `shapes.rs` runs all
//! plan fixtures against the published shapes with the real SHACL engine.

use std::collections::{BTreeMap, BTreeSet};

use ikigai_vocab::{NS, VOCABULARY};
use oxrdf::{NamedOrBlankNode, Term, Triple};

const STAGE_FIXTURES: [&str; 3] = ["plan-fork-binds", "plan-fork-branch", "plan-fork-mapped"];

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_SUBCLASS: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";

fn parse(ttl: &str) -> Vec<Triple> {
    oxttl::TurtleParser::new()
        .for_reader(ttl.as_bytes())
        .map(|t| t.expect("parses"))
        .collect()
}

fn fixture(name: &str) -> Vec<Triple> {
    let path = format!("{}/tests/fixtures/{name}.ttl", env!("CARGO_MANIFEST_DIR"));
    parse(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
}

fn subject(t: &Triple) -> Option<&str> {
    match &t.subject {
        NamedOrBlankNode::NamedNode(n) => Some(n.as_str()),
        NamedOrBlankNode::BlankNode(_) => None,
    }
}

fn object_iri(t: &Triple) -> Option<&str> {
    match &t.object {
        Term::NamedNode(n) => Some(n.as_str()),
        _ => None,
    }
}

fn ik(term: &str) -> String {
    format!("{NS}{term}")
}

/// The RDFS types of `node` in `data` under the vocabulary: its asserted types, the
/// domains of the properties it is the subject of, closed over `rdfs:subClassOf`.
fn entailed_types(vocabulary: &[Triple], data: &[Triple], node: &str) -> BTreeSet<String> {
    let domain: BTreeMap<&str, &str> = vocabulary
        .iter()
        .filter(|t| t.predicate.as_str() == RDFS_DOMAIN)
        .filter_map(|t| Some((subject(t)?, object_iri(t)?)))
        .collect();
    let mut types: BTreeSet<String> = BTreeSet::new();
    for t in data.iter().filter(|t| subject(t) == Some(node)) {
        if t.predicate.as_str() == RDF_TYPE {
            types.extend(object_iri(t).map(str::to_string));
        }
        if let Some(d) = domain.get(t.predicate.as_str()) {
            types.insert((*d).to_string());
        }
    }
    loop {
        let more: Vec<String> = vocabulary
            .iter()
            .filter(|t| t.predicate.as_str() == RDFS_SUBCLASS)
            .filter(|t| subject(t).is_some_and(|s| types.contains(s)))
            .filter_map(|t| object_iri(t).map(str::to_string))
            .filter(|c| !types.contains(c))
            .collect();
        if more.is_empty() {
            return types;
        }
        types.extend(more);
    }
}

#[test]
fn a_fork_that_binds_branches_or_maps_is_a_stage_and_never_a_step() {
    let vocabulary = parse(VOCABULARY);
    for name in STAGE_FIXTURES {
        let data = fixture(name);
        let forks: Vec<String> = data
            .iter()
            .filter(|t| t.predicate.as_str() == RDF_TYPE && object_iri(t) == Some(&ik("Fork")))
            .filter_map(|t| subject(t).map(str::to_string))
            .collect();
        assert!(!forks.is_empty(), "{name} has a fork");
        for fork in forks {
            let types = entailed_types(&vocabulary, &data, &fork);
            assert!(
                types.contains(&ik("Stage")),
                "{name}: {fork} is a stage: {types:?}"
            );
            assert!(
                !types.contains(&ik("Step")),
                "{name}: {fork} is entailed a step, the trap ledger #32 names: {types:?}"
            );
        }
    }
    // The axioms themselves.
    let triple = |s: &str, p: &str, o: &str| {
        vocabulary
            .iter()
            .any(|t| subject(t) == Some(s) && t.predicate.as_str() == p && object_iri(t) == Some(o))
    };
    for class in ["Step", "Fork"] {
        assert!(
            triple(&ik(class), RDFS_SUBCLASS, &ik("Stage")),
            "ik:{class} ⊑ ik:Stage"
        );
    }
    for property in ["binds", "mapOver", "forkOf"] {
        assert!(
            triple(&ik(property), RDFS_DOMAIN, &ik("Stage")),
            "ik:{property}"
        );
    }
    // ik:order stays domain-free: a domain would type every ik:Route as a plan stage.
    assert!(!vocabulary
        .iter()
        .any(|t| subject(t) == Some(&ik("order")) && t.predicate.as_str() == RDFS_DOMAIN));
}
