//! **`#` and `?` in a name, and brackets in a plan id** (ledger #1390, #1391).
//!
//! A name is ONE path segment of the IRI minted from it. Before 0.1.101 `encode_name` left
//! `#` and `?` raw, so in `urn:ikigai:endpoint:{id}` a `#` moved the rest of the name into
//! a fragment and a `?` into a query:
//!
//! - id `a#b#c` and an input `x#y` under id `a#b` put a second `#` in the IRI, which no
//!   IRI parser accepts: the Turtle and the contract IRI were refused outright;
//! - id `a#b` parsed, but anything that drops fragments (an HTTP client, a document
//!   cache) saw `urn:ikigai:endpoint:a` for it and for `a#c` alike.
//!
//! Brian decided (2026-10-12) that `#` and `?` are percent-encoded in a name ALWAYS, which
//! respells a name holding a single `#` or a `?` that parsed before. And a plan id sits
//! inside a `urn:plan:` path, where brackets are never legal, so it is spelled as a name
//! too (keeping only its real `%XX` escapes, ledger #1352).

use ikigai_core::{encode_name, match_iri, ActionSpec, ArgSpec, Description, Verb};
use ikigai_vocab::plan;

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

/// What anything that drops fragments and queries sees of an IRI.
fn without_fragment_or_query(iri: &str) -> &str {
    let end = iri.find(['#', '?']).unwrap_or(iri.len());
    &iri[..end]
}

fn described(id: &str, input: &str) -> Description {
    Description::new(id)
        .verb(Verb::Source)
        .input(ArgSpec::new(input))
        .action(ActionSpec::new(Verb::Sink).input(ArgSpec::new("content")))
}

#[test]
fn an_id_with_two_hashes_emits_a_parseable_document_and_contract_iri() {
    for id in ["a#b#c", "a?b?c", "a?b#c#d", "#", "?"] {
        let d = described(id, "in");
        let ttl = ikigai_vocab::to_turtle(&d);
        turtle_parses(&ttl).unwrap_or_else(|e| panic!("id {id:?}: {e}\n{ttl}"));
        for spec in d.action_specs() {
            let iri = spec.contract_iri(&d.id);
            iri_parses(&iri).unwrap_or_else(|e| panic!("id {id:?}: {iri}: {e}"));
        }
        iri_parses(&match_iri(Verb::Source, id)).unwrap();
    }
}

#[test]
fn an_input_name_with_a_hash_under_an_id_with_one_emits_a_parseable_document() {
    for (id, input) in [("a#b", "x#y"), ("a?b", "x?y"), ("a#b", "x?y")] {
        let ttl = ikigai_vocab::to_turtle(&described(id, input));
        turtle_parses(&ttl).unwrap_or_else(|e| panic!("{id:?}/{input:?}: {e}\n{ttl}"));
    }
}

#[test]
fn two_names_differing_after_a_hash_or_query_mint_two_resources() {
    for (a, b) in [("a#b", "a#c"), ("a?b", "a?c"), ("a#b", "a")] {
        let ia = format!("urn:ikigai:endpoint:{}", encode_name(a));
        let ib = format!("urn:ikigai:endpoint:{}", encode_name(b));
        assert_ne!(
            without_fragment_or_query(&ia),
            without_fragment_or_query(&ib),
            "{a:?} and {b:?}"
        );
        // And the whole name stays in the path: nothing to strip.
        assert_eq!(without_fragment_or_query(&ia), ia);
    }
}

#[test]
fn hash_and_query_are_encoded_and_decode_back() {
    assert_eq!(encode_name("a#b"), "a%23b");
    assert_eq!(encode_name("a?b"), "a%3Fb");
    assert_eq!(encode_name("%23#"), "%2523%23");
    for name in ["a#b", "a?b", "#?#", "%23#", "?x=1&y=2#top"] {
        let (_, pattern) = ikigai_core::parse_match_iri(&match_iri(Verb::Source, name)).unwrap();
        assert_eq!(pattern, name);
        let spec = ActionSpec::new(Verb::Source);
        let (id, _, _) = ikigai_core::parse_contract_iri(&spec.contract_iri(name)).unwrap();
        assert_eq!(id, name);
    }
}

/// The catalog writes the encoded node and the name as a literal, so the name survives.
#[test]
fn the_catalog_spells_a_hash_name_as_core_does() {
    let d = Description::new("a#b")
        .verb(Verb::Source)
        .input(ArgSpec::new("x?y"));
    let ttl = ikigai_vocab::to_turtle(&d);
    assert!(ttl.contains("<urn:ikigai:endpoint:a%23b>"), "{ttl}");
    assert!(
        ttl.contains("<urn:ikigai:endpoint:a%23b:input:x%3Fy>"),
        "{ttl}"
    );
    turtle_parses(&ttl).unwrap();
}

/// An AUTHORED IRI keeps its fragment and query: a `class` or `requires` scope is an IRI,
/// not a name.
#[test]
fn an_authored_iri_keeps_its_fragment_and_query() {
    let d = Description::new("ep")
        .verb(Verb::Source)
        .input(ArgSpec::new("in").class("http://www.w3.org/2001/XMLSchema#string"))
        .requires("urn:cap:x?y");
    let ttl = ikigai_vocab::to_turtle(&d);
    turtle_parses(&ttl).unwrap_or_else(|e| panic!("{e}\n{ttl}"));
    assert!(ttl.contains("XMLSchema#string"), "{ttl}");
}

/// Ledger #1391: a plan id sits inside a `urn:plan:` path, where `[`, `]`, `#` and `?` are
/// never legal, so every plan IRI parses whatever the id holds.
#[test]
fn a_plan_id_holding_brackets_hash_or_query_is_a_parseable_iri() {
    let mut refused = Vec::new();
    for id in ["a[0]", "]", "a#b#c", "a?b", "pua\u{E000}", "del\u{7F}"] {
        for iri in [
            plan::process_iri(id),
            plan::step_iri(id, 1),
            plan::var_iri(id, "v"),
            plan::fork_iri(id, 2),
            plan::input_iri(id, "i"),
            plan::argument_iri(id, 3, "a"),
        ] {
            if let Err(e) = iri_parses(&iri) {
                refused.push(format!("{id:?}: {iri:?}: {e}"));
            }
            if without_fragment_or_query(&iri) != iri {
                refused.push(format!("{id:?}: {iri:?} has a fragment or query"));
            }
        }
    }
    assert!(refused.is_empty(), "{}", refused.join("\n"));
}

#[test]
fn a_plan_id_is_spelled_as_a_name_keeping_its_real_escapes() {
    assert_eq!(plan::process_iri("a[0]"), "urn:plan:a%5B0%5D");
    assert_eq!(plan::process_iri("a#b"), "urn:plan:a%23b");
    assert_eq!(plan::process_iri("a?b"), "urn:plan:a%3Fb");
    // The ledger #1352 rule is unchanged: a real escape is kept, a lone `%` is the character.
    assert_eq!(plan::process_iri("a%20b"), "urn:plan:a%20b");
    assert_eq!(plan::process_iri("100%"), "urn:plan:100%25");
    assert_eq!(plan::process_iri("sha256:9f2c"), "urn:plan:sha256:9f2c");
    // Read back by stripping `urn:plan:` and written again: the same IRI.
    for id in ["a[0]", "a#b#c", "a?b", "100% [x]", "a%5B", "pua\u{E000}"] {
        let once = plan::process_iri(id);
        let read_back = once.strip_prefix("urn:plan:").unwrap();
        assert_eq!(plan::process_iri(read_back), once, "{id:?}");
    }
}
