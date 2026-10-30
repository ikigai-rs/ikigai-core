//! The plan fixtures pin the emitted shape of the process vocabulary: the CMS link check
//! as a plan, plus three variants that are three kinds of edit. These tests keep the
//! vocabulary, the shapes and the fixtures agreeing in both directions — a term cannot be
//! declared without an example, an example cannot use an undeclared term, and the text
//! face in each fixture's header must match its graph step for step.
//!
//! `shapes.rs` validates these fixtures against the published shapes with the real
//! SHACL engine. The dependency walk here additionally checks cycles through names,
//! which the shapes cannot express.

use std::collections::BTreeSet;

use ikigai_vocab::{plan, NS, SHAPES, VOCABULARY};
use oxrdf::{NamedNode, NamedOrBlankNode, Term, Triple};

const FIXTURES: [&str; 4] = [
    "plan-linkcheck",
    "plan-linkcheck-strict",
    "plan-linkcheck-repair",
    "plan-linkcheck-scoped",
];

fn fixture_text(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}.ttl", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn parse(ttl: &str) -> Vec<Triple> {
    oxttl::TurtleParser::new()
        .for_reader(ttl.as_bytes())
        .map(|t| t.expect("fixture parses"))
        .collect()
}

fn ik(term: &str) -> NamedNode {
    NamedNode::new(format!("{NS}{term}")).unwrap()
}

/// A tiny read-only view over a fixture's triples.
struct Graph(Vec<Triple>);

impl Graph {
    fn load(name: &str) -> (String, Graph) {
        let text = fixture_text(name);
        let g = Graph(parse(&text));
        (text, g)
    }

    fn objects(&self, s: &str, p: &str) -> Vec<&Term> {
        let p = ik(p);
        self.0
            .iter()
            .filter(|t| matches!(&t.subject, NamedOrBlankNode::NamedNode(n) if n.as_str() == s))
            .filter(|t| t.predicate == p)
            .map(|t| &t.object)
            .collect()
    }

    fn iri(&self, s: &str, p: &str) -> Option<String> {
        let os = self.objects(s, p);
        assert!(os.len() <= 1, "{s} has {} {p} values", os.len());
        os.first().map(|o| match o {
            Term::NamedNode(n) => n.as_str().to_string(),
            other => panic!("{s} {p} is not an IRI: {other}"),
        })
    }

    fn str(&self, s: &str, p: &str) -> Option<String> {
        let os = self.objects(s, p);
        assert!(os.len() <= 1, "{s} has {} {p} values", os.len());
        os.first().map(|o| match o {
            Term::Literal(l) => l.value().to_string(),
            other => panic!("{s} {p} is not a literal: {other}"),
        })
    }

    fn iris(&self, s: &str, p: &str) -> Vec<String> {
        self.objects(s, p)
            .into_iter()
            .map(|o| match o {
                Term::NamedNode(n) => n.as_str().to_string(),
                other => panic!("{s} {p} is not an IRI: {other}"),
            })
            .collect()
    }

    fn of_type(&self, class: &str) -> BTreeSet<String> {
        let ty = NamedNode::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type").unwrap();
        let class = ik(class);
        self.0
            .iter()
            .filter(|t| t.predicate == ty && t.object == Term::NamedNode(class.clone()))
            .map(|t| match &t.subject {
                NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
                other => panic!("blank-node subject {other}"),
            })
            .collect()
    }

    fn process(&self) -> String {
        let ps = self.of_type("Process");
        assert_eq!(ps.len(), 1, "exactly one ik:Process per fixture: {ps:?}");
        ps.into_iter().next().unwrap()
    }

    /// The step of this plan binding `name`, if any (a parameter is not a step).
    fn binder(&self, process: &str, name: &str) -> Option<String> {
        self.iris(process, "step")
            .into_iter()
            .find(|s| self.str(s, "binds").as_deref() == Some(name))
    }
}

/// Every ik: term declared in VOCABULARY, and the subset declared in the process section.
fn declared_terms() -> (BTreeSet<String>, BTreeSet<String>) {
    let mut all = BTreeSet::new();
    let mut process = BTreeSet::new();
    let mut in_process_section = false;
    for line in VOCABULARY.lines() {
        if line.starts_with("# ---- processes") {
            in_process_section = true;
        }
        let Some(rest) = line.trim_start().strip_prefix("ik:") else {
            continue;
        };
        let Some((name, tail)) = rest.split_once(' ') else {
            continue;
        };
        if tail.starts_with("a rdf:Property") || tail.starts_with("a rdfs:Class") {
            all.insert(name.to_string());
            if in_process_section {
                process.insert(name.to_string());
            }
        }
    }
    assert!(
        process.len() >= 15,
        "the process section declares its terms: {process:?}"
    );
    (all, process)
}

/// The ik: terms a graph uses: every predicate, and every class in an rdf:type object.
fn used_terms(triples: &[Triple]) -> BTreeSet<String> {
    let mut used = BTreeSet::new();
    for t in triples {
        if let Some(term) = t.predicate.as_str().strip_prefix(NS) {
            used.insert(term.to_string());
        }
        if let Term::NamedNode(n) = &t.object {
            if t.predicate.as_str().ends_with("#type") {
                if let Some(term) = n.as_str().strip_prefix(NS) {
                    used.insert(term.to_string());
                }
            }
        }
    }
    used
}

#[test]
fn fixtures_parse_and_are_skolemized() {
    for name in FIXTURES {
        let (_, g) = Graph::load(name);
        assert!(g.0.len() > 20, "{name} carries a real graph");
        for t in &g.0 {
            assert!(
                !matches!(t.subject, NamedOrBlankNode::BlankNode(_))
                    && !matches!(t.object, Term::BlankNode(_)),
                "{name}: blank node in {t}"
            );
        }
    }
}

#[test]
fn every_term_a_fixture_uses_is_declared() {
    let (declared, _) = declared_terms();
    for name in FIXTURES {
        let (_, g) = Graph::load(name);
        for term in used_terms(&g.0) {
            assert!(declared.contains(&term), "{name} uses undeclared ik:{term}");
        }
    }
    // the shapes too: every ik: IRI they mention is a declared term (the bare namespace
    // IRI is the ontology itself, the sh:prefixes target, not a term)
    for t in parse(SHAPES) {
        if let Term::NamedNode(n) = &t.object {
            if let Some(term) = n.as_str().strip_prefix(NS).filter(|t| !t.is_empty()) {
                assert!(
                    declared.contains(term),
                    "shapes.ttl mentions undeclared ik:{term}"
                );
            }
        }
    }
}

#[test]
fn every_process_term_appears_in_a_fixture() {
    // The other direction: a term cannot be added to the process section without an
    // example that uses it.
    let (_, process_terms) = declared_terms();
    let mut used = BTreeSet::new();
    for name in FIXTURES {
        let (_, g) = Graph::load(name);
        used.extend(used_terms(&g.0));
    }
    // `ik:Stage` is entailed, never asserted (every step and fork is one by its subclass
    // axiom), so no fixture names it; `tests/stages.rs` exercises it by entailment.
    used.insert("Stage".to_string());
    let unexercised: Vec<_> = process_terms.difference(&used).collect();
    assert!(
        unexercised.is_empty(),
        "process terms no fixture exercises: {unexercised:?}"
    );
}

// ---- the text face ↔ graph walk -------------------------------------------------

/// One stage of the text face: `[name =] [@up (|..)] [source|sink] <iri> k=v…`, or a fork.
#[derive(Debug)]
enum Stage {
    Request {
        name: Option<String>,
        upstream: Option<(String, Edge)>,
        verb: &'static str,
        target: String,
        args: Vec<(String, String)>,
    },
    Fork {
        upstream: Option<String>,
        branches: Vec<Stage>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Edge {
    Pipe,
    Map,
}

fn text_face(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("# --- text face ---") {
            inside = true;
            continue;
        }
        if line.starts_with("# --- end text face ---") {
            break;
        }
        if inside {
            lines.push(line.trim_start_matches('#').trim().to_string());
        }
    }
    assert!(!lines.is_empty(), "fixture has a text face");
    lines
}

fn parse_request(words: &[&str]) -> Stage {
    let (verb, rest) = match words {
        ["source", rest @ ..] => ("Source", rest),
        ["sink", rest @ ..] => ("Sink", rest),
        rest => ("Source", rest), // a piped or mapped stage: bare IRI
    };
    let (target, args) = rest.split_first().expect("a stage names an IRI");
    Stage::Request {
        name: None,
        upstream: None,
        verb,
        target: target.to_string(),
        args: args
            .iter()
            .map(|a| {
                let (k, v) = a.split_once('=').expect("k=v");
                (k.to_string(), v.to_string())
            })
            .collect(),
    }
}

fn parse_line(line: &str) -> Stage {
    let (name, rest) = match line.split_once(" = ") {
        Some((n, r)) => (Some(n.trim().to_string()), r.trim()),
        None => (None, line),
    };
    let (upstream, rest) = match rest.strip_prefix('@') {
        Some(r) => {
            let (up, rest) = r.split_once(' ').unwrap();
            let (edge, rest) = match rest.split_once(' ').unwrap() {
                ("|", rest) => (Edge::Pipe, rest),
                ("..", rest) => (Edge::Map, rest),
                other => panic!("unknown connector {other:?}"),
            };
            (Some((up.to_string(), edge)), rest)
        }
        None => (None, rest),
    };
    if let Some(inner) = rest.strip_prefix("( ").and_then(|r| r.strip_suffix(" )")) {
        assert!(
            name.is_none(),
            "a fork binds no name (ik:binds is a step property)"
        );
        let (up, edge) = upstream.expect("the fixtures' forks have an upstream");
        assert_eq!(edge, Edge::Pipe);
        return Stage::Fork {
            upstream: Some(up),
            branches: inner
                .split(" ; ")
                .map(|b| parse_request(&b.split(' ').collect::<Vec<_>>()))
                .collect(),
        };
    }
    match parse_request(&rest.split(' ').collect::<Vec<_>>()) {
        Stage::Request {
            verb, target, args, ..
        } => Stage::Request {
            name,
            upstream,
            verb,
            target,
            args,
        },
        Stage::Fork { .. } => unreachable!(),
    }
}

fn check_request(g: &Graph, process: &str, plan_id: &str, step: &str, stage: &Stage) {
    let Stage::Request {
        name,
        upstream,
        verb,
        target,
        args,
    } = stage
    else {
        panic!("a request stage")
    };
    assert!(g.of_type("Step").contains(step), "{step} is typed ik:Step");
    assert_eq!(
        g.str(step, "binds").as_deref(),
        name.as_deref(),
        "{step} binds"
    );
    assert_eq!(g.str(step, "verb").as_deref(), Some(*verb), "{step} verb");
    assert_eq!(
        g.iri(step, "resolves").as_deref(),
        Some(target.as_str()),
        "{step} target"
    );
    let (pipe, map) = (g.iri(step, "pipeFrom"), g.iri(step, "mapOver"));
    match upstream {
        Some((up, Edge::Pipe)) => {
            assert_eq!(pipe, g.binder(process, up), "{step} pipes from @{up}");
            assert_eq!(map, None);
        }
        Some((up, Edge::Map)) => {
            assert_eq!(map, g.binder(process, up), "{step} maps over @{up}");
            assert_eq!(pipe, None);
        }
        None => assert!(pipe.is_none() && map.is_none(), "{step} has no upstream"),
    }
    let arg_nodes = g.iris(step, "argument");
    assert_eq!(arg_nodes.len(), args.len(), "{step} argument count");
    for (k, v) in args {
        let node = arg_nodes
            .iter()
            .find(|a| g.str(a, "inputName").as_deref() == Some(k))
            .unwrap_or_else(|| panic!("{step} has argument {k}"));
        assert!(
            g.of_type("Argument").contains(node),
            "{node} is typed ik:Argument"
        );
        match v.strip_prefix('@') {
            Some(var) => {
                assert_eq!(
                    g.iri(node, "ref"),
                    Some(plan::var_iri(plan_id, var)),
                    "{node} ref"
                );
                assert_eq!(g.str(node, "value"), None);
            }
            None => {
                assert_eq!(
                    g.str(node, "value").as_deref(),
                    Some(v.as_str()),
                    "{node} value"
                );
                assert_eq!(g.iri(node, "ref"), None);
            }
        }
    }
}

#[test]
fn text_face_matches_the_graph_step_for_step() {
    for name in FIXTURES {
        let (text, g) = Graph::load(name);
        let process = g.process();
        let plan_id = process.strip_prefix("urn:plan:").expect("plan IRI scheme");
        assert_eq!(process, plan::process_iri(plan_id));
        for parameter in g.iris(&process, "input") {
            let name = g.str(&parameter, "inputName").expect("parameter name");
            assert_eq!(parameter, plan::input_iri(plan_id, &name));
        }
        let mut next_step = 1;
        let mut next_fork = 1;
        for line in text_face(&text) {
            let stage = parse_line(&line);
            match &stage {
                Stage::Request { .. } => {
                    let step = plan::step_iri(plan_id, next_step);
                    check_request(&g, &process, plan_id, &step, &stage);
                    assert!(g.iri(&step, "forkOf").is_none(), "{step} is not a branch");
                    next_step += 1;
                }
                Stage::Fork { upstream, branches } => {
                    let fork = plan::fork_iri(plan_id, next_fork);
                    assert!(g.of_type("Fork").contains(&fork), "{fork} is typed ik:Fork");
                    assert_eq!(
                        g.iri(&fork, "upstream"),
                        upstream.as_ref().and_then(|u| g.binder(&process, u)),
                        "{fork} upstream"
                    );
                    for (i, branch) in branches.iter().enumerate() {
                        let step = plan::step_iri(plan_id, next_step);
                        check_request(&g, &process, plan_id, &step, branch);
                        assert_eq!(g.iri(&step, "forkOf").as_deref(), Some(fork.as_str()));
                        assert_eq!(
                            g.str(&step, "order").as_deref(),
                            Some((i + 1).to_string().as_str())
                        );
                        next_step += 1;
                    }
                    next_fork += 1;
                }
            }
        }
        // every step in the text face is listed on the process, and nothing else is
        let listed: BTreeSet<String> = g.iris(&process, "step").into_iter().collect();
        assert_eq!(
            listed,
            g.of_type("Step"),
            "{name}: ik:step lists every ik:Step"
        );
        assert_eq!(listed.len(), next_step - 1, "{name}: step count");
        assert_eq!(g.of_type("Fork").len(), next_fork - 1, "{name}: fork count");
    }
}

// ---- cycles through named references ----------------------------------------------

#[test]
fn fixtures_have_no_cycles_through_named_references() {
    for name in FIXTURES {
        let (_, g) = Graph::load(name);
        let process = g.process();
        let steps: BTreeSet<String> = g.iris(&process, "step").into_iter().collect();
        let forks: BTreeSet<String> = steps.iter().filter_map(|s| g.iri(s, "forkOf")).collect();

        // Acyclicity, INCLUDING the hop the shapes cannot take: an argument reference to a
        // bound name is an edge to the step that binds it. Every step's dependencies are
        // walked; a step reaching itself is a cycle.
        let deps = |s: &str| -> Vec<String> {
            let mut out: Vec<String> = [g.iri(s, "pipeFrom"), g.iri(s, "mapOver")]
                .into_iter()
                .flatten()
                .collect();
            if let Some(f) = g.iri(s, "forkOf") {
                out.extend(g.iri(&f, "upstream"));
            }
            for a in g.iris(s, "argument") {
                if let Some(r) = g.iri(&a, "ref") {
                    if let Some(var) = r.strip_prefix(&format!("{process}:var:")) {
                        out.extend(g.binder(&process, var));
                    }
                }
            }
            // a fork as a dependency stands for all its branches
            out.into_iter()
                .flat_map(|d| {
                    if forks.contains(&d) {
                        steps
                            .iter()
                            .filter(|s| g.iri(s, "forkOf").as_deref() == Some(d.as_str()))
                            .cloned()
                            .collect()
                    } else {
                        vec![d]
                    }
                })
                .collect()
        };
        for start in &steps {
            let mut stack = deps(start);
            let mut seen = BTreeSet::new();
            while let Some(s) = stack.pop() {
                assert_ne!(&s, start, "{name}: {start} depends on itself");
                if seen.insert(s.clone()) {
                    stack.extend(deps(&s));
                }
            }
        }
    }
}
