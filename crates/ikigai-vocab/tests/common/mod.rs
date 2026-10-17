//! The rdfs:domain family-trap checker shared by the family tests (`layers.rs`,
//! `principals.rs`).
//!
//! The risk a family of terms carries is the domain trap: a domain is an assertion about
//! EVERY subject that ever carries the property, so a domain that is merely "usually"
//! true types nodes into a family they are not in, and no code path ever shows it
//! (`ik:Endpoint` on the contract properties, `ik:Explanation` on the archive stamps).
//! So each family runs every domain and range the vocabulary declares over an example
//! graph whose nodes assert the types they really have, and a declaration that would
//! type one of them into another family fails.

use std::collections::{BTreeMap, BTreeSet};

use ikigai_vocab::{NS, VOCABULARY};
use oxrdf::{NamedOrBlankNode, Term, Triple};

pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
pub const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
pub const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
pub const PROV: &str = "http://www.w3.org/ns/prov#";

pub fn ik(term: &str) -> String {
    format!("{NS}{term}")
}

pub fn parse(ttl: &str) -> Vec<Triple> {
    oxttl::TurtleParser::new()
        .for_reader(ttl.as_bytes())
        .map(|t| t.expect("valid turtle"))
        .collect()
}

/// The example graph `tests/fixtures/{name}`.
pub fn fixture(name: &str) -> Vec<Triple> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    parse(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
}

pub fn named(subject: &NamedOrBlankNode) -> Option<&str> {
    match subject {
        NamedOrBlankNode::NamedNode(n) => Some(n.as_str()),
        _ => None,
    }
}

/// The subjects the vocabulary declares, PARSED, as every family test asks it: a term in
/// syntax the parser does not accept reads as present to `contains` and as missing to
/// every consumer.
pub fn declared() -> BTreeSet<String> {
    parse(VOCABULARY)
        .iter()
        .filter_map(|t| named(&t.subject).map(String::from))
        .collect()
}

/// What the vocabulary declares that an RDFS reasoner would act on: each property's
/// domain and range when it is a CLASS (a datatype, `rdfs:Resource` and `rdfs:Literal`
/// type nothing), and the subclass edges.
pub struct Schema {
    pub domains: BTreeMap<String, String>,
    pub ranges: BTreeMap<String, String>,
    pub supers: BTreeMap<String, BTreeSet<String>>,
}

impl Schema {
    pub fn load() -> Self {
        let mut schema = Schema {
            domains: BTreeMap::new(),
            ranges: BTreeMap::new(),
            supers: BTreeMap::new(),
        };
        let is_class = |o: &str| {
            !o.starts_with(XSD) && o != format!("{RDFS}Resource") && o != format!("{RDFS}Literal")
        };
        for t in parse(VOCABULARY) {
            let (Some(s), Term::NamedNode(o)) = (named(&t.subject), &t.object) else {
                continue;
            };
            let p = t.predicate.as_str();
            if p == format!("{RDFS}domain") && is_class(o.as_str()) {
                schema.domains.insert(s.into(), o.as_str().into());
            } else if p == format!("{RDFS}range") && is_class(o.as_str()) {
                schema.ranges.insert(s.into(), o.as_str().into());
            } else if p == format!("{RDFS}subClassOf") {
                schema
                    .supers
                    .entry(s.into())
                    .or_default()
                    .insert(o.as_str().into());
            }
        }
        schema
    }

    /// `class` is `of` or a subclass of it, over the vocabulary's own subclass edges.
    pub fn is_a(&self, class: &str, of: &str) -> bool {
        let mut seen = BTreeSet::new();
        let mut todo = vec![class.to_string()];
        while let Some(c) = todo.pop() {
            if c == of {
                return true;
            }
            if seen.insert(c.clone()) {
                todo.extend(self.supers.get(&c).into_iter().flatten().cloned());
            }
        }
        false
    }

    /// What an RDFS reasoner would infer from `triples` that is FALSE: a subject typed
    /// into a property's domain, or an object into its range, when the node's ASSERTED
    /// types are outside that class's family; or a literal where the range is a class.
    /// A node asserting no type cannot be contradicted, only described, which is why each
    /// family test makes sure its example's types are asserted.
    pub fn traps(&self, triples: &[Triple]) -> Vec<String> {
        let types = |node: &str| -> Vec<String> {
            triples
                .iter()
                .filter(|t| t.predicate.as_str() == RDF_TYPE && named(&t.subject) == Some(node))
                .filter_map(|t| match &t.object {
                    Term::NamedNode(n) => Some(n.as_str().to_string()),
                    _ => None,
                })
                .collect()
        };
        let mut traps = Vec::new();
        for t in triples {
            let p = t.predicate.as_str();
            if let (Some(domain), Some(subject)) = (self.domains.get(p), named(&t.subject)) {
                let held = types(subject);
                if !held.is_empty() && !held.iter().any(|c| self.is_a(c, domain)) {
                    traps.push(format!("{p} domain ⇒ {subject} {held:?} is a {domain}"));
                }
            }
            match (self.ranges.get(p), &t.object) {
                (Some(range), Term::NamedNode(object)) => {
                    let held = types(object.as_str());
                    if !held.is_empty() && !held.iter().any(|c| self.is_a(c, range)) {
                        traps.push(format!("{p} range ⇒ {object} {held:?} is a {range}"));
                    }
                }
                (Some(range), Term::Literal(literal)) => {
                    traps.push(format!("{p} range ⇒ the literal {literal} is a {range}"));
                }
                _ => {}
            }
        }
        traps.sort();
        traps.dedup();
        traps
    }
}

/// The block that declares `ik:{term}`, for reading its axioms line by line.
pub fn block(term: &str, kind: &str) -> &'static str {
    VOCABULARY
        .split("\n\n")
        .find(|b| b.trim_start().starts_with(&format!("ik:{term} a {kind}")))
        .unwrap_or_else(|| panic!("ik:{term} is declared as {kind}"))
}

/// The value of the declaration line `rdfs:{axiom} …` in a term's block, if any: read
/// from a DECLARATION line, since the comments argue about domains in prose.
pub fn axiom<'a>(block: &'a str, axiom: &str) -> Option<&'a str> {
    block
        .lines()
        .find_map(|l| l.trim_start().strip_prefix(&format!("rdfs:{axiom} ")))
}
