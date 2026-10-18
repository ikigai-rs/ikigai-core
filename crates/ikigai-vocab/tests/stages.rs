//! Ledger #32: a fork could not bind a name, be a fork's branch, or be mapped over,
//! because `ik:binds`, `ik:forkOf` and `ik:mapOver` had `rdfs:domain ik:Step` and a fork
//! is not a step. Asserting one of them on a fork VALIDATED (the shapes target asserted
//! types), and under RDFS entailment typed the fork an `ik:Step`: a graph that lints clean
//! and means something false. `ik:Stage` (a superclass of both) is the domain now.
//!
//! These tests pin the vocabulary half (the entailment), and run the three new plan
//! shapes through a hand mirror of `shapes.ttl` generalized to stages, as `plans.rs` does
//! for the step-only fixtures (this crate has no SHACL engine; ikigai-shacl runs the
//! shapes for real).

use std::collections::{BTreeMap, BTreeSet};

use ikigai_vocab::{NS, SHAPES, VOCABULARY};
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

#[test]
fn the_stage_fixtures_satisfy_the_shapes_generalized_to_stages() {
    // The shapes mention the stage property shapes from both the step and the fork shape.
    for shape in ["step", "fork"] {
        let block = SHAPES
            .split("\n\n")
            .find(|b| b.contains(&format!("<urn:ikigai:shape:{shape}> a sh:NodeShape")))
            .unwrap_or_else(|| panic!("the {shape} shape"));
        for stage in ["binds", "mapOver", "forkOf", "order", "order-with-fork"] {
            assert!(
                block.contains(&format!("<urn:ikigai:shape:stage:{stage}>")),
                "the {shape} shape lists stage:{stage}"
            );
        }
    }
    for name in STAGE_FIXTURES {
        let g = fixture(name);
        let objects = |s: &str, p: &str| -> Vec<Term> {
            let p = ik(p);
            g.iter()
                .filter(|t| subject(t) == Some(s) && t.predicate.as_str() == p)
                .map(|t| t.object.clone())
                .collect()
        };
        let iri = |s: &str, p: &str| -> Option<String> {
            let os = objects(s, p);
            assert!(os.len() <= 1, "{name}: {s} has {} {p}", os.len());
            os.first().map(|o| match o {
                Term::NamedNode(n) => n.as_str().to_string(),
                other => panic!("{name}: {s} {p} is not an IRI: {other}"),
            })
        };
        let lit = |s: &str, p: &str| -> Option<String> {
            let os = objects(s, p);
            assert!(os.len() <= 1, "{name}: {s} has {} {p}", os.len());
            os.first().map(|o| match o {
                Term::Literal(l) => l.value().to_string(),
                other => panic!("{name}: {s} {p} is not a literal: {other}"),
            })
        };
        let process = g
            .iter()
            .find(|t| t.predicate.as_str() == RDF_TYPE && object_iri(t) == Some(&ik("Process")))
            .and_then(subject)
            .expect("a process")
            .to_string();
        let steps: BTreeSet<String> = objects(&process, "step")
            .into_iter()
            .filter_map(|o| match o {
                Term::NamedNode(n) => Some(n.as_str().to_string()),
                _ => None,
            })
            .collect();
        // The plan's forks: reached from its steps through ik:forkOf+.
        let mut forks: BTreeSet<String> = BTreeSet::new();
        let mut frontier: Vec<String> = steps.iter().filter_map(|s| iri(s, "forkOf")).collect();
        while let Some(f) = frontier.pop() {
            if forks.insert(f.clone()) {
                frontier.extend(iri(&f, "forkOf"));
            }
        }
        assert!(
            !forks.is_empty(),
            "{name}: its forks are reachable from its steps"
        );
        let stages: BTreeSet<String> = steps.union(&forks).cloned().collect();
        let local = |x: &str| stages.contains(x);

        let mut names: BTreeMap<String, usize> = BTreeMap::new();
        for stage in &stages {
            // At most one name, an identifier.
            if let Some(b) = lit(stage, "binds") {
                assert!(
                    b.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                    "{stage} binds an identifier"
                );
                *names.entry(b).or_default() += 1;
            }
            // A branch has a position, and only a branch does.
            assert_eq!(
                iri(stage, "forkOf").is_some(),
                lit(stage, "order").is_some(),
                "{name}: {stage}: ik:order iff ik:forkOf"
            );
            if let Some(f) = iri(stage, "forkOf") {
                assert!(
                    forks.contains(&f),
                    "{stage} is a branch of a fork of this plan"
                );
            }
            // One feed: a step's pipe / map / fork; a fork's upstream / map / fork.
            let feed = if forks.contains(stage) {
                "upstream"
            } else {
                "pipeFrom"
            };
            let feeds = [
                iri(stage, feed),
                iri(stage, "mapOver"),
                iri(stage, "forkOf"),
            ];
            assert!(
                feeds.iter().flatten().count() <= 1,
                "{name}: {stage} has one feed"
            );
            for up in feeds[..2].iter().flatten() {
                assert!(local(up), "{name}: {stage}'s upstream {up} is local");
            }
        }
        let result = iri(&process, "result").expect("one result");
        assert!(local(&result), "{name}: the result is a stage of the plan");
        // Single assignment, across steps and forks.
        for (n, count) in &names {
            assert_eq!(*count, 1, "{name}: {n} bound {count} times");
        }
        // Every fork has a branch (a step or a fork), with distinct positions.
        for f in &forks {
            let orders: Vec<String> = stages
                .iter()
                .filter(|s| iri(s, "forkOf").as_deref() == Some(f.as_str()))
                .filter_map(|s| lit(s, "order"))
                .collect();
            assert!(!orders.is_empty(), "{name}: {f} has a branch");
            let distinct: BTreeSet<&String> = orders.iter().collect();
            assert_eq!(distinct.len(), orders.len(), "{name}: {f}: distinct orders");
        }
        // Every reference into the plan's names cites a name a stage binds.
        for t in g.iter().filter(|t| t.predicate.as_str() == ik("ref")) {
            let Some(var) = object_iri(t).and_then(|r| r.strip_prefix(&format!("{process}:var:")))
            else {
                continue;
            };
            assert!(
                names.contains_key(var),
                "{name}: @{var} is bound by a stage"
            );
        }
    }
}
