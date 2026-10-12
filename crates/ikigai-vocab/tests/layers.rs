//! The layer family (ledger #1182, design: ikigai-devtools
//! `claude/research/collab-layer-v1-2026-10-10.md`): the `ik:` terms a layer host emits
//! for layers, manifests, participants, message entries and task tuples.
//!
//! The risk these terms carry is the rdfs:domain family trap: a domain is an assertion
//! about EVERY subject that ever carries the property, so a domain that is merely
//! "usually" true types nodes into a family they are not in, and no code path ever shows
//! it (`ik:Endpoint` on the contract properties, `ik:Explanation` on the archive stamps).
//! So every domain and range the vocabulary declares is run here over an example graph
//! of the family (`tests/fixtures/layers-example.ttl`) whose nodes assert the types they
//! really have, and a declaration that would type one of them into another family fails.
//! The checker is proven to see a trap before its silence is trusted.

use std::collections::{BTreeMap, BTreeSet};

use ikigai_vocab::{CONTEXT, NS, VOCABULARY};
use oxrdf::{NamedOrBlankNode, Term, Triple};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const PROV: &str = "http://www.w3.org/ns/prov#";

/// The classes arc 1 of the layers design names.
const CLASSES: [&str; 9] = [
    "Layer",
    "Collaboration",
    "Machine",
    "Message",
    "Question",
    "Offer",
    "Progress",
    "Release",
    "Handback",
];

/// The properties arc 1 names, then the four ledger #1214 added (a layer's egress
/// context, entry bound and sharing decision, and a participant's grants). Only
/// `layerModel` declares a domain.
const PROPERTIES: [&str; 16] = [
    "layerModel",
    "body",
    "brief",
    "note",
    "repository",
    "offer",
    "addressedTo",
    "evidence",
    "seq",
    "supersedes",
    "inReplyTo",
    "egressContext",
    "maxEntryBytes",
    "sharedWith",
    "holds",
    // Not the family's, and listed so the example is checked against it: see
    // `the_trap_check_sees_a_mistyping`.
    "layer",
];

/// The family's own properties: every one of `PROPERTIES` but the last.
const FAMILY: usize = PROPERTIES.len() - 1;

fn ik(term: &str) -> String {
    format!("{NS}{term}")
}

fn parse(ttl: &str) -> Vec<Triple> {
    oxttl::TurtleParser::new()
        .for_reader(ttl.as_bytes())
        .map(|t| t.expect("valid turtle"))
        .collect()
}

fn example() -> Vec<Triple> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/layers-example.ttl"
    );
    parse(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}")))
}

fn named(subject: &NamedOrBlankNode) -> Option<&str> {
    match subject {
        NamedOrBlankNode::NamedNode(n) => Some(n.as_str()),
        _ => None,
    }
}

/// What the vocabulary declares that an RDFS reasoner would act on: each property's
/// domain and range when it is a CLASS (a datatype, `rdfs:Resource` and `rdfs:Literal`
/// type nothing), and the subclass edges.
struct Schema {
    domains: BTreeMap<String, String>,
    ranges: BTreeMap<String, String>,
    supers: BTreeMap<String, BTreeSet<String>>,
}

impl Schema {
    fn load() -> Self {
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
    fn is_a(&self, class: &str, of: &str) -> bool {
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
    /// A node asserting no type cannot be contradicted, only described, which is why
    /// `the_example_types_every_node_the_family_describes` makes sure the example's are
    /// asserted.
    fn traps(&self, triples: &[Triple]) -> Vec<String> {
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
fn block(term: &str, kind: &str) -> &'static str {
    VOCABULARY
        .split("\n\n")
        .find(|b| b.trim_start().starts_with(&format!("ik:{term} a {kind}")))
        .unwrap_or_else(|| panic!("ik:{term} is declared as {kind}"))
}

#[test]
fn the_layer_family_is_defined() {
    // Over PARSED subjects, as every family test in src/lib.rs asks it: a term in
    // syntax the parser does not accept reads as present to `contains` and as missing
    // to every consumer.
    let subjects: BTreeSet<String> = parse(VOCABULARY)
        .iter()
        .filter_map(|t| named(&t.subject).map(String::from))
        .collect();
    for term in CLASSES.iter().chain(&PROPERTIES[..FAMILY]) {
        assert!(
            subjects.contains(&ik(term)),
            "ik:{term} is a layer term (ledger #1182) and is not declared"
        );
    }
    for class in CLASSES {
        let b = block(class, "rdfs:Class");
        assert!(
            b.contains("rdfs:label") && b.contains("rdfs:comment"),
            "{b}"
        );
    }
    // Only an ik:Layer has an access model. Every other property is carried by more
    // than one kind of entry now or by design, so it declares no domain: checked on a
    // DECLARATION line, since the comments argue about domains in prose.
    for property in &PROPERTIES[..FAMILY] {
        let b = block(property, "rdf:Property");
        let domain = b
            .lines()
            .find_map(|l| l.trim_start().strip_prefix("rdfs:domain "));
        let expected = (*property == "layerModel").then_some("ik:Layer ;");
        assert_eq!(domain, expected, "ik:{property}:\n{b}");
        assert!(
            b.contains("rdfs:range"),
            "ik:{property} states its range:\n{b}"
        );
    }
    let schema = Schema::load();
    for (class, of) in [
        ("Machine", format!("{PROV}SoftwareAgent")),
        ("Question", ik("Message")),
        ("Message", format!("{PROV}Entity")),
        ("Question", format!("{PROV}Entity")),
        ("Offer", format!("{PROV}Entity")),
        ("Progress", format!("{PROV}Entity")),
        ("Release", format!("{PROV}Entity")),
        ("Handback", format!("{PROV}Entity")),
    ] {
        assert!(schema.is_a(&ik(class), &of), "ik:{class} ⊑ {of}");
    }
    // Neither a layer nor a manifest is an entry, and a machine is not a message.
    for (class, not) in [
        ("Layer", format!("{PROV}Entity")),
        ("Collaboration", format!("{PROV}Entity")),
        ("Machine", format!("{PROV}Entity")),
        ("Message", ik("Question")),
    ] {
        assert!(
            !schema.is_a(&ik(class), &not),
            "ik:{class} must not be ⊑ {not}"
        );
    }
    assert!(
        parse(VOCABULARY).iter().any(|t| {
            named(&t.subject) == Some(ik("supersedes").as_str())
                && t.predicate.as_str() == format!("{RDFS}subPropertyOf")
                && t.object.to_string() == format!("<{PROV}wasRevisionOf>")
        }),
        "ik:supersedes states the same fact as prov:wasRevisionOf for a PROV reader"
    );
}

#[test]
fn the_example_types_every_node_the_family_describes() {
    // The trap check judges against ASSERTED types and is silent about an untyped
    // node, so it is only worth something if the example types what it describes.
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
    for property in &PROPERTIES[..FAMILY] {
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
    for class in CLASSES {
        assert!(
            asserted.contains(&format!("<{}>", ik(class))),
            "the example has an ik:{class}"
        );
    }
    for t in &triples {
        let p = t.predicate.as_str();
        if PROPERTIES.iter().any(|q| ik(q) == p) {
            let s = named(&t.subject).unwrap();
            assert!(typed.contains(s), "{s} carries {p} and asserts no type");
        }
        if p == ik("offer") || p == ik("supersedes") || p == ik("inReplyTo") {
            let Term::NamedNode(o) = &t.object else {
                panic!("{p} names an entry by IRI")
            };
            assert!(
                typed.contains(o.as_str()),
                "{o} is named by {p} and asserts no type"
            );
        }
    }
    assert!(
        !used.contains(ik("layer").as_str()),
        "ik:layer is the explain family's chain cell, never 'the layer an entry is in'"
    );
}

#[test]
fn no_declared_domain_or_range_types_an_example_node_into_a_family_it_is_not_in() {
    // Every domain and range in the vocabulary, not only the layer family's: an
    // existing term reused by a layer host (ik:about, ik:order) is judged the same way.
    let traps = Schema::load().traps(&example());
    assert!(traps.is_empty(), "family traps:\n{}", traps.join("\n"));
}

#[test]
fn the_trap_check_sees_a_mistyping() {
    let schema = Schema::load();
    let prefix = "@prefix ik: <https://ikigai-rs.dev/ns#> .\n";
    for (data, names) in [
        // The near-miss this family invites: ik:layer to say which layer a message
        // is in types the message a dry run's consultation.
        (
            "<urn:layer:talk:entry:x> a ik:Message ; ik:layer <urn:layer:talk> .",
            "Consultation",
        ),
        (
            "<urn:layer:t:offer:x> a ik:Offer ; ik:layerModel \"channel\" .",
            "Layer",
        ),
        (
            "<urn:layer:t:report:x> a ik:Handback ; ik:offer <urn:layer:talk:entry:x> .\n\
             <urn:layer:talk:entry:x> a ik:Message .",
            "Offer",
        ),
        (
            "<urn:layer:t:report:x> a ik:Handback ; ik:offer \"urn:layer:t:offer:x\" .",
            "Offer",
        ),
    ] {
        let traps = schema.traps(&parse(&format!("{prefix}{data}")));
        assert!(
            traps.iter().any(|t| t.contains(&ik(names))),
            "{data} must trap into ik:{names}; got {traps:?}"
        );
    }
    // And the domains this family declines are load-bearing: give a shared property
    // the domain of one of the classes carrying it, and the example traps.
    for (property, domain) in [
        ("note", "Release"),
        ("offer", "Handback"),
        ("addressedTo", "Progress"),
        ("evidence", "Handback"),
        ("body", "Question"),
        ("seq", "Question"),
        // A person and a machine both hold grants.
        ("holds", "Machine"),
    ] {
        let mut narrowed = Schema::load();
        narrowed.domains.insert(ik(property), ik(domain));
        let traps = narrowed.traps(&example());
        assert!(
            traps.iter().any(|t| t.starts_with(&ik(property))),
            "rdfs:domain ik:{domain} on ik:{property} would type another report or \
             entry an ik:{domain}; got {traps:?}"
        );
    }
}

#[test]
fn the_layer_contexts_agree_with_the_vocabulary() {
    // The design's two layer contexts (section 9): each is its OWN allowlist and
    // never core's CONTEXT, which defines every ik: term. What core owes them is that
    // every ik: term they name is defined here with the same coercion, so a document
    // compacted with a layer's context expands to the vocabulary's own triples.
    let tasks = r#"{ "@context": {
        "ik": "https://ikigai-rs.dev/ns#",
        "dcterms": "http://purl.org/dc/terms/",
        "prov": "http://www.w3.org/ns/prov#",
        "xsd": "http://www.w3.org/2001/XMLSchema#",
        "Offer": "ik:Offer", "Progress": "ik:Progress", "Release": "ik:Release", "Handback": "ik:Handback",
        "title":        "dcterms:title",
        "brief":        "ik:brief",
        "note":         "ik:note",
        "repository":   { "@id": "ik:repository",        "@type": "@id" },
        "offer":        { "@id": "ik:offer",             "@type": "@id" },
        "addressedTo":  { "@id": "ik:addressedTo",       "@type": "@id" },
        "evidence":     { "@id": "ik:evidence",          "@type": "@id" },
        "attributedTo": { "@id": "prov:wasAttributedTo", "@type": "@id" },
        "created":      { "@id": "dcterms:created",      "@type": "xsd:dateTime" }
    } }"#;
    let talk = r#"{ "@context": {
        "ik": "https://ikigai-rs.dev/ns#",
        "dcterms": "http://purl.org/dc/terms/",
        "prov": "http://www.w3.org/ns/prov#",
        "xsd": "http://www.w3.org/2001/XMLSchema#",
        "Message": "ik:Message", "Question": "ik:Question",
        "body":         "ik:body",
        "inReplyTo":    { "@id": "ik:inReplyTo",         "@type": "@id" },
        "supersedes":   { "@id": "ik:supersedes",        "@type": "@id" },
        "seq":          { "@id": "ik:seq",               "@type": "xsd:integer" },
        "attributedTo": { "@id": "prov:wasAttributedTo", "@type": "@id" },
        "created":      { "@id": "dcterms:created",      "@type": "xsd:dateTime" }
    } }"#;
    let core: serde_json::Value = serde_json::from_str(CONTEXT).unwrap();
    let core = core["@context"].as_object().unwrap();
    let id_and_type = |v: &serde_json::Value| -> (String, Option<String>) {
        match v {
            serde_json::Value::String(s) => (s.clone(), None),
            v => (
                v["@id"].as_str().unwrap().to_string(),
                v["@type"].as_str().map(String::from),
            ),
        }
    };
    for (name, text) in [("tasks", tasks), ("talk", talk)] {
        let ctx: serde_json::Value = serde_json::from_str(text).unwrap();
        let ctx = ctx["@context"].as_object().unwrap();
        assert!(
            !ctx.contains_key("@vocab"),
            "{name}: no @vocab in an allowlist"
        );
        let mut named = 0;
        for (term, value) in ctx {
            let (id, ty) = id_and_type(value);
            let Some(local) = id.strip_prefix("ik:") else {
                continue;
            };
            named += 1;
            let defined = core
                .get(local)
                .unwrap_or_else(|| panic!("{name}: ik:{local} is not in the vocabulary"));
            assert_eq!(
                id_and_type(defined),
                (id.clone(), ty.clone()),
                "{name}: `{term}` coerces as the vocabulary does"
            );
        }
        assert!(
            named < core.len() / 10,
            "{name} is an allowlist, not core's CONTEXT"
        );
    }
}
