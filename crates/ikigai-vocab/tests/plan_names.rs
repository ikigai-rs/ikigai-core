//! **A `%` in a plan IRI position** (ledger #1352, after #1345). Every `plan::*` IRI
//! must parse, whatever its segments carry, and each position is spelled by what the
//! segment IS:
//!
//! - a bound name, a parameter name and an argument name are NAMES, so a `%` in one is a
//!   character and is written `%25` (the rule `to_turtle` uses for an endpoint's input
//!   names, so `:input:{name}` is spelled one way under a plan and under an endpoint);
//! - the plan id is the IRI segment a stored plan is NAMED by (`urn:program:{name}` runs
//!   it, `urn:plan:{name}` describes it, and a plan read back from Turtle recovers its id by
//!   stripping the prefix), so a real escape in it is kept and only a `%` that does not
//!   start one is written `%25`: re-encoding `a%20b` would rename every stored plan
//!   carrying it and break the read-then-write round trip.

use ikigai_vocab::plan;

/// Parse `<iri> <urn:p> <urn:o> .` with a real Turtle parser; the error names the IRI.
fn parses(iri: &str) -> Result<(), String> {
    let doc = format!("<{iri}> <urn:p> <urn:o> .\n");
    oxttl::TurtleParser::new()
        .for_reader(doc.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map(|_| ())
        .map_err(|e| format!("<{iri}> does not parse: {e}"))
}

const HOSTILE: [&str; 5] = ["100%", "100% sure", "%", "a%4", "x%zz> . <urn:y"];

#[test]
fn every_plan_iri_parses_whatever_its_names_carry() {
    for name in HOSTILE {
        for iri in [
            plan::process_iri(name),
            plan::step_iri(name, 1),
            plan::fork_iri(name, 2),
            plan::var_iri("p", name),
            plan::input_iri("p", name),
            plan::argument_iri("p", 3, name),
        ] {
            parses(&iri).unwrap();
        }
    }
}

#[test]
fn a_name_spells_its_percent_as_a_character() {
    assert_eq!(plan::var_iri("p", "100%"), "urn:plan:p:var:100%25");
    assert_eq!(
        plan::input_iri("p", "100% sure"),
        "urn:plan:p:input:100%25%20sure"
    );
    // Injective: a name that LOOKS escaped stays distinct from the name it would decode to.
    assert_eq!(
        plan::argument_iri("p", 1, "a%41"),
        "urn:plan:p:step:1:arg:a%2541"
    );
    assert_ne!(plan::var_iri("p", "a%41"), plan::var_iri("p", "aA"));
    // A name with no `%` is spelled exactly as before.
    assert_eq!(plan::var_iri("p", "checks"), "urn:plan:p:var:checks");
    assert_eq!(
        plan::var_iri("p", "x> . <urn:y"),
        "urn:plan:p:var:x%3E%20.%20%3Curn:y"
    );
}

#[test]
fn a_plan_id_keeps_its_real_escapes_and_survives_a_round_trip() {
    // A lone `%` is not an escape, so it is the character.
    assert_eq!(plan::process_iri("100%"), "urn:plan:100%25");
    assert_eq!(plan::process_iri("a%4"), "urn:plan:a%254");
    assert_eq!(plan::process_iri("%zz"), "urn:plan:%25zz");
    // A real escape is part of the id's IRI spelling, kept byte for byte.
    assert_eq!(plan::process_iri("a%20b"), "urn:plan:a%20b");
    assert_eq!(plan::process_iri("sha256:9f2c"), "urn:plan:sha256:9f2c");
    // A reader recovers the id by stripping `urn:plan:`; writing that id again is the same
    // IRI, so a plan survives being read and rewritten.
    for id in ["100%", "100% sure", "a%20b", "%", "a%4", "x%zz> . <urn:y"] {
        let once = plan::process_iri(id);
        let read_back = once.strip_prefix("urn:plan:").unwrap();
        assert_eq!(plan::process_iri(read_back), once, "{id:?}");
    }
}
