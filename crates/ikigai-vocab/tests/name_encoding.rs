//! **A name is encoded, whatever it holds** (ledger #1361). An endpoint id and an input
//! name are NAMES, spelled into `urn:ikigai:endpoint:{id}`, its `:input:{name}` nodes, the
//! contract IRI and the match IRI. Before 0.1.100 the name escapers encoded only what a
//! Turtle `IRIREF` cannot carry (and `%`), so two classes passed through raw and made every
//! one of those documents unparseable:
//!
//! - `[` and `]`, legal in an `IRIREF` but only inside an IRI's host (an IPv6 literal), so
//!   an id that is itself a full IRI such as `http://[::1]/x` broke the endpoint node;
//! - a code point outside RFC 3987's `ucschar` (DEL, a C1 control, private use, a
//!   noncharacter), which ledger #1351 made `Description::validate` refuse, but only for a
//!   host that calls it.
//!
//! Brian decided (2026-10-12) that the emitters never write an unparseable document,
//! whatever name an endpoint carries: both classes are percent-encoded in a NAME, and an
//! authored IRI (a `class`, a `requires` scope) keeps its brackets. A plan id was treated as
//! an authored IRI here; since 0.1.101 it is spelled as a name (ledger #1391,
//! `tests/hash_query_names.rs`).

use ikigai_core::{match_iri, ActionSpec, ArgSpec, Description, Verb};

/// The two classes a name escaper did not encode: brackets, and one of each kind of
/// non-`ucschar` code point (DEL, NEL, private use, two noncharacters).
const NAMES: [&str; 8] = [
    "http://[::1]/x",
    "a[0]",
    "]",
    "del\u{7F}",
    "nel\u{85}",
    "pua\u{E000}",
    "nonchar\u{FDD0}",
    "nonchar\u{FFFF}",
];

fn turtle_parses(ttl: &str) -> Result<(), String> {
    oxttl::TurtleParser::new()
        .for_reader(ttl.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn iri_parses(iri: &str) -> Result<(), String> {
    oxrdf::NamedNode::new(iri)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[test]
fn an_id_holding_either_class_emits_a_parseable_document_and_contract_iri() {
    let mut refused = Vec::new();
    for id in NAMES {
        let d = Description::new(id)
            .verb(Verb::Source)
            .input(ArgSpec::new("in"))
            .action(ActionSpec::new(Verb::Sink).input(ArgSpec::new("content")));
        let ttl = ikigai_vocab::to_turtle(&d);
        if let Err(e) = turtle_parses(&ttl) {
            refused.push(format!("id {id:?}: Turtle: {e}"));
        }
        for spec in d.action_specs() {
            let iri = spec.contract_iri(&d.id);
            if let Err(e) = iri_parses(&iri) {
                refused.push(format!("id {id:?}: contract IRI {iri:?}: {e}"));
            }
        }
    }
    assert!(refused.is_empty(), "{}", refused.join("\n"));
}

#[test]
fn an_input_name_holding_either_class_emits_a_parseable_document() {
    let mut refused = Vec::new();
    for name in NAMES {
        // The flat form and an explicit action: two different input-node spellings.
        let d = Description::new("ep")
            .verb(Verb::Source)
            .input(ArgSpec::new(name))
            .action(ActionSpec::new(Verb::Sink).input(ArgSpec::new(name)));
        if let Err(e) = turtle_parses(&ikigai_vocab::to_turtle(&d)) {
            refused.push(format!("input {name:?}: Turtle: {e}"));
        }
    }
    assert!(refused.is_empty(), "{}", refused.join("\n"));
}

#[test]
fn a_door_pattern_holding_either_class_has_a_parseable_match_iri() {
    let mut refused = Vec::new();
    for pattern in NAMES {
        let iri = match_iri(Verb::Source, pattern);
        if let Err(e) = iri_parses(&iri) {
            refused.push(format!("pattern {pattern:?}: {iri:?}: {e}"));
        }
    }
    assert!(refused.is_empty(), "{}", refused.join("\n"));
}

/// The catalog's endpoint and input nodes are spelled by `ikigai_core::encode_name`, the one
/// function core spells the explain graph's `ik:answer`, a validation report's
/// `sh:resultPath`, a thread and a binding with (ledger #1345, #1352, #1361). Pinned over the
/// emitted Turtle, as triples, for a corpus holding every class a name escaper encodes.
#[test]
fn the_catalog_spells_every_name_as_core_does() {
    let corpus = [
        "plain",
        "naïve 書名",
        "50% off",
        "a%41",
        "ev>il \"q\" {x} |^` \\ \t\n",
        "http://[::1]/x",
        "a[0]",
        "del\u{7F} nel\u{85} pua\u{E000} nonchar\u{FDD0}\u{FFFF}",
    ];
    for name in corpus {
        let d = Description::new(name)
            .verb(Verb::Source)
            .input(ArgSpec::new(name));
        let ttl = ikigai_vocab::to_turtle(&d);
        let triples: Vec<oxrdf::Triple> = oxttl::TurtleParser::new()
            .for_reader(ttl.as_bytes())
            .collect::<Result<_, _>>()
            .unwrap_or_else(|e| panic!("{name:?}: {e}\n{ttl}"));
        let endpoint = format!("urn:ikigai:endpoint:{}", ikigai_core::encode_name(name));
        let input = format!("{endpoint}:input:{}", ikigai_core::encode_name(name));
        let subjects: std::collections::BTreeSet<String> = triples
            .iter()
            .map(|t| match &t.subject {
                oxrdf::NamedOrBlankNode::NamedNode(n) => n.as_str().to_owned(),
                other => panic!("{name:?}: a blank node {other}"),
            })
            .collect();
        assert!(
            subjects.contains(&endpoint),
            "{name:?}: no {endpoint}\n{ttl}"
        );
        assert!(subjects.contains(&input), "{name:?}: no {input}\n{ttl}");
        // The name survives as a literal, and out of the contract and match IRIs exactly.
        assert!(triples.iter().any(|t| matches!(&t.object,
            oxrdf::Term::Literal(l) if l.value() == name)));
        let spec = &d.action_specs()[0];
        let (id, _, _) = ikigai_core::parse_contract_iri(&spec.contract_iri(name)).unwrap();
        assert_eq!(id, name);
        let (_, pattern) = ikigai_core::parse_match_iri(&match_iri(Verb::Source, name)).unwrap();
        assert_eq!(pattern, name);
    }
}

/// An AUTHORED IRI is not a name: a `class` or `requires` scope with an IPv6 host keeps its
/// brackets, and is still written as a resource.
#[test]
fn an_authored_iri_keeps_its_brackets() {
    let d = Description::new("http://[::1]/x")
        .verb(Verb::Source)
        .input(ArgSpec::new("a[0]").class("http://[::1]/cls"))
        .requires("http://[::1]/cap");
    let ttl = ikigai_vocab::to_turtle(&d);
    turtle_parses(&ttl).unwrap_or_else(|e| panic!("{e}\n{ttl}"));
    assert!(ttl.contains("<http://[::1]/cls>"), "{ttl}");
    assert!(ttl.contains("<http://[::1]/cap>"), "{ttl}");
    assert!(
        ttl.contains("<urn:ikigai:endpoint:http://%5B::1%5D/x>"),
        "{ttl}"
    );
    d.validate().expect("brackets are admitted in a name");
}

/// A plan's bound, parameter and argument names are NAMES too (`ikigai_vocab::plan`).
#[test]
fn a_plan_name_holding_either_class_is_a_parseable_iri() {
    use ikigai_vocab::plan;
    let mut refused = Vec::new();
    for name in NAMES {
        for iri in [
            plan::var_iri("p", name),
            plan::input_iri("p", name),
            plan::argument_iri("p", 1, name),
        ] {
            if let Err(e) = iri_parses(&iri) {
                refused.push(format!("{name:?}: {iri:?}: {e}"));
            }
        }
    }
    assert!(refused.is_empty(), "{}", refused.join("\n"));
}
