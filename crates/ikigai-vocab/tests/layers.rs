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

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{axiom, block, declared, fixture, ik, named, parse, Schema, PROV, RDFS, RDF_TYPE};
use ikigai_vocab::{CONTEXT, VOCABULARY};
use oxrdf::{NamedOrBlankNode, Term, Triple};

/// The classes arc 1 of the layers design names, and those the layers emit since: a feed
/// layer and its announcements (ledger #3284, #3287) and the talk layers' briefs (#3300).
const CLASSES: [&str; 12] = [
    "Layer",
    "Collaboration",
    "Machine",
    "Message",
    "Question",
    "Offer",
    "Progress",
    "Release",
    "Handback",
    "Feed",
    "Announcement",
    "Brief",
];

/// The properties arc 1 names, and a layer's kind since (ledger #3881). Only `layerModel`
/// declares a domain.
const PROPERTIES: [&str; 13] = [
    "layerModel",
    "layerKind",
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
    // Not arc 1's, and listed so the example is checked against it: see
    // `the_trap_check_sees_a_mistyping`.
    "layer",
];

fn example() -> Vec<Triple> {
    fixture("layers-example.ttl")
}

const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";

/// The kinds of layer a UI groups layers by (ledger #3881): each concept of
/// `ik:LayerKinds` by its IRI's local name, with the notation a layer's configuration
/// names it by and the tab name it gives.
const KINDS: [(&str, &str, &str); 5] = [
    ("TalkKind", "talk", "Talk"),
    ("TaskKind", "task", "Tasks"),
    ("FeedKind", "feed", "Feed"),
    ("InquiryKind", "inquiry", "Inquiries"),
    ("JoinKind", "join", "Join"),
];

#[test]
fn the_layer_family_is_defined() {
    // Over PARSED subjects, as every family test in src/lib.rs asks it.
    let subjects = declared();
    for term in CLASSES.iter().chain(&PROPERTIES[..12]) {
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
    for property in &PROPERTIES[..12] {
        let b = block(property, "rdf:Property");
        let domain = axiom(b, "domain");
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
        ("Feed", ik("Layer")),
        ("Announcement", format!("{PROV}Entity")),
        ("Brief", format!("{PROV}Entity")),
    ] {
        assert!(schema.is_a(&ik(class), &of), "ik:{class} ⊑ {of}");
    }
    // Neither a layer nor a manifest is an entry, and a machine is not a message. A feed
    // is a layer, never an entry of one; an announcement and a brief sit beside messages
    // under shapes of their own, so reading a layer's messages does not include them.
    for (class, not) in [
        ("Layer", format!("{PROV}Entity")),
        ("Collaboration", format!("{PROV}Entity")),
        ("Machine", format!("{PROV}Entity")),
        ("Message", ik("Question")),
        ("Feed", format!("{PROV}Entity")),
        ("Announcement", ik("Message")),
        ("Brief", ik("Message")),
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
    for property in &PROPERTIES[..12] {
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

#[test]
fn the_layer_kinds_are_a_scheme_of_concepts_each_named_once() {
    let triples = parse(VOCABULARY);
    let values = |subject: &str, predicate: &str| -> Vec<Term> {
        triples
            .iter()
            .filter(|t| named(&t.subject) == Some(subject) && t.predicate.as_str() == predicate)
            .map(|t| t.object.clone())
            .collect()
    };
    let iri = |s: &str| Term::from(oxrdf::NamedNode::new(s).unwrap());
    let scheme = ik("LayerKinds");
    assert_eq!(
        values(&scheme, RDF_TYPE),
        [iri(&format!("{SKOS}ConceptScheme"))],
        "ik:LayerKinds is a skos:ConceptScheme"
    );
    assert_eq!(
        block("layerKind", "rdf:Property")
            .lines()
            .find_map(|l| l.trim_start().strip_prefix("rdfs:range ")),
        Some("skos:Concept ;"),
        "ik:layerKind ranges over concepts"
    );

    // Every concept in the scheme, found from the graph rather than from KINDS, so a
    // sixth concept added without the four properties fails here too.
    let in_scheme: BTreeSet<String> = triples
        .iter()
        .filter(|t| t.predicate.as_str() == format!("{SKOS}inScheme") && t.object == iri(&scheme))
        .filter_map(|t| named(&t.subject).map(String::from))
        .collect();
    let expected: BTreeSet<String> = KINDS.iter().map(|(local, ..)| ik(local)).collect();
    assert_eq!(
        in_scheme, expected,
        "the scheme's concepts are the five kinds"
    );

    let mut notations: BTreeMap<String, String> = BTreeMap::new();
    for concept in &in_scheme {
        assert_eq!(
            values(concept, RDF_TYPE),
            [iri(&format!("{SKOS}Concept"))],
            "{concept} is a skos:Concept"
        );
        for property in ["prefLabel", "definition", "notation", "inScheme"] {
            let found = values(concept, &format!("{SKOS}{property}"));
            assert_eq!(
                found.len(),
                1,
                "{concept} has exactly one skos:{property}: {found:?}"
            );
            if let Term::Literal(l) = &found[0] {
                assert!(
                    !l.value().trim().is_empty(),
                    "{concept}'s skos:{property} is empty"
                );
            }
        }
        let Term::Literal(notation) = &values(concept, &format!("{SKOS}notation"))[0] else {
            panic!("{concept}'s skos:notation is a literal")
        };
        if let Some(other) = notations.insert(notation.value().to_string(), concept.clone()) {
            panic!(
                "{concept} and {other} share the notation {}",
                notation.value()
            );
        }
    }
    for (local, notation, label) in KINDS {
        let concept = ik(local);
        assert_eq!(
            notations.get(notation),
            Some(&concept),
            "ik:{local} is named `{notation}`"
        );
        let Term::Literal(pref) = &values(&concept, &format!("{SKOS}prefLabel"))[0] else {
            panic!("ik:{local}'s skos:prefLabel is a literal")
        };
        assert_eq!(pref.value(), label, "ik:{local}'s tab is named {label}");
    }

    // Every layer of the example declares a kind of the scheme, and its range check sees
    // the concept typed as the vocabulary types it.
    let example = example();
    let layers: BTreeSet<&str> = example
        .iter()
        .filter(|t| t.predicate.as_str() == RDF_TYPE && t.object == iri(&ik("Layer")))
        .filter_map(|t| named(&t.subject))
        .collect();
    assert!(!layers.is_empty());
    for layer in &layers {
        let kinds: Vec<&Term> = example
            .iter()
            .filter(|t| named(&t.subject) == Some(layer) && t.predicate.as_str() == ik("layerKind"))
            .map(|t| &t.object)
            .collect();
        assert!(
            kinds.len() == 1
                && matches!(kinds[0], Term::NamedNode(n) if in_scheme.contains(n.as_str())),
            "{layer} declares one kind of ik:LayerKinds: {kinds:?}"
        );
    }
    let mut with_concepts = example.clone();
    with_concepts.extend(
        triples
            .iter()
            .filter(|t| named(&t.subject).is_some_and(|s| in_scheme.contains(s)))
            .cloned(),
    );
    let traps = Schema::load().traps(&with_concepts);
    assert!(traps.is_empty(), "family traps:\n{}", traps.join("\n"));
}

#[test]
fn the_layer_kind_order_names_every_kind_once_in_tab_order() {
    // A scheme is a set; `ik:LayerKindOrder` is the order a UI shows its tabs in, so a
    // reader never hard-codes kind names (ledger #3881, round 2).
    const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    let triples = parse(VOCABULARY);
    let iri = |s: &str| Term::from(oxrdf::NamedNode::new(s).unwrap());
    let one = |subject: &str, predicate: &str| -> Term {
        let found: Vec<&Term> = triples
            .iter()
            .filter(|t| named(&t.subject) == Some(subject) && t.predicate.as_str() == predicate)
            .map(|t| &t.object)
            .collect();
        assert_eq!(found.len(), 1, "{subject} has one <{predicate}>: {found:?}");
        found[0].clone()
    };
    let order = ik("LayerKindOrder");
    assert_eq!(
        one(&order, RDF_TYPE),
        iri(&format!("{SKOS}OrderedCollection")),
        "ik:LayerKindOrder is a skos:OrderedCollection"
    );
    for (prefix, property) in [(RDFS, "label"), (RDFS, "comment"), (SKOS, "prefLabel")] {
        let Term::Literal(l) = one(&order, &format!("{prefix}{property}")) else {
            panic!("ik:LayerKindOrder's {property} is a literal")
        };
        assert!(
            !l.value().trim().is_empty(),
            "ik:LayerKindOrder's {property} is empty"
        );
    }

    // Walk the list cell by cell. Each cell is named under the collection, one-based, as
    // ik:layers writes its cells: the house rule is no blank nodes.
    let mut members = Vec::new();
    let mut cell = one(&order, &format!("{SKOS}memberList"));
    while cell != iri(&format!("{RDF}nil")) {
        let Term::NamedNode(n) = &cell else {
            panic!("a cell of ik:LayerKindOrder is not named: {cell:?}")
        };
        assert_eq!(
            n.as_str(),
            format!("{order}:member:{}", members.len() + 1),
            "cell {} of ik:LayerKindOrder",
            members.len() + 1
        );
        assert!(members.len() < 64, "ik:LayerKindOrder's list does not end");
        let Term::NamedNode(member) = one(n.as_str(), &format!("{RDF}first")) else {
            panic!("{n}'s member is a named concept")
        };
        members.push(member.as_str().to_string());
        cell = one(n.as_str(), &format!("{RDF}rest"));
    }

    // In tab order, and every concept of the scheme exactly once.
    let expected: Vec<String> = KINDS.iter().map(|(local, ..)| ik(local)).collect();
    assert_eq!(members, expected, "Talk, Tasks, Feed, Inquiries, Join");
    let in_scheme: BTreeSet<String> = triples
        .iter()
        .filter(|t| {
            t.predicate.as_str() == format!("{SKOS}inScheme") && t.object == iri(&ik("LayerKinds"))
        })
        .filter_map(|t| named(&t.subject).map(String::from))
        .collect();
    let listed: BTreeSet<String> = members.iter().cloned().collect();
    assert_eq!(
        listed.len(),
        members.len(),
        "no kind is listed twice: {members:?}"
    );
    assert_eq!(
        listed, in_scheme,
        "the order lists every concept of ik:LayerKinds"
    );
}
