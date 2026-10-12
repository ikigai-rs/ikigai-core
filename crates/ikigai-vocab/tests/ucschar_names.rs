//! **A name no IRI can carry** (ledger #1351, #1361). DEL, the C1 controls, private use and
//! the noncharacters are legal in a Turtle `IRIREF` but are not RFC 3987 `ucschar`s, so an
//! IRI parser refuses them. Before 0.1.100 neither name escaper encoded them, an endpoint id
//! or input name holding one was spelled into the endpoint node, the input nodes and the
//! contract IRI as it was, and oxttl refused the whole document. Now `encode_name`
//! percent-encodes them in every name, so the documents parse whether or not a host calls
//! `Description::validate`, and `validate`, the strict opt-in, still refuses such a name as
//! an authoring mistake.

use ikigai_core::{ActionSpec, ArgSpec, Description, Error, Verb};

/// DEL, a C1 control (NEL), private use, and two noncharacters.
const REFUSED: [char; 5] = ['\u{7F}', '\u{85}', '\u{E000}', '\u{FDD0}', '\u{FFFF}'];

fn turtle_parses(ttl: &str) -> bool {
    oxttl::TurtleParser::new()
        .for_reader(ttl.as_bytes())
        .all(|triple| triple.is_ok())
}

fn iri_parses(iri: &str) -> bool {
    oxrdf::NamedNode::new(iri).is_ok()
}

/// The same bad character in each NAME position validate covers: the id, a flat input
/// name, and an input name on an explicit `ActionSpec`.
fn in_every_name_position(c: char) -> [(&'static str, Description); 3] {
    [
        (
            "id",
            Description::new(format!("tag{c}suggest")).verb(Verb::Source),
        ),
        (
            "input name",
            Description::new("tag-suggest")
                .verb(Verb::Source)
                .input(ArgSpec::new(format!("bo{c}ok"))),
        ),
        (
            "input name",
            Description::new("tag-suggest")
                .action(ActionSpec::new(Verb::Sink).input(ArgSpec::new(format!("bo{c}ok")))),
        ),
    ]
}

#[test]
fn without_validate_the_name_is_encoded_and_every_document_parses() {
    // What validate used to be the only guard against (ledger #1351): the Turtle and the
    // contract IRI were both unparseable. Encoded now (ledger #1361), with no validate.
    for c in REFUSED {
        let d = Description::new(format!("tag{c}suggest"))
            .verb(Verb::Source)
            .input(ArgSpec::new(format!("bo{c}ok")));
        let cp = u32::from(c);
        let ttl = ikigai_vocab::to_turtle(&d);
        assert!(turtle_parses(&ttl), "U+{cp:04X}: Turtle refused\n{ttl}");
        // Encoded in the IRIs (the literals still carry the name as written).
        let node = format!("<urn:ikigai:endpoint:{}>", ikigai_core::encode_name(&d.id));
        assert!(!node.contains(c), "U+{cp:04X} written raw in {node}");
        assert!(ttl.contains(&node), "U+{cp:04X}: no {node}\n{ttl}");
        for spec in d.action_specs() {
            let iri = spec.contract_iri(&d.id);
            assert!(iri_parses(&iri), "U+{cp:04X}: contract IRI {iri:?}");
            // The id still round-trips out of the contract IRI.
            let (id, _, _) = ikigai_core::parse_contract_iri(&iri).unwrap();
            assert_eq!(id, d.id);
        }
    }
}

#[test]
fn validate_refuses_each_code_point_in_every_name_position() {
    for c in REFUSED {
        let point = format!("U+{:04X}", u32::from(c));
        for (field, d) in in_every_name_position(c) {
            let err = d.validate().expect_err("must be refused");
            assert!(
                matches!(&err, Error::InvalidArgument { name, .. } if name == field),
                "{point} in {field}: {err:?}"
            );
            let text = err.to_string();
            assert!(text.contains(&point), "{point} not named: {text}");
            // The message names the field and shows the name, escaped, never raw.
            assert!(text.contains(field), "{text}");
            assert!(!text.contains(c), "{point} written raw: {text:?}");
        }
    }
}

#[test]
fn ordinary_unicode_names_are_still_admitted() {
    for (id, name) in [("naïve", "café"), ("書籍", "書名"), ("über-tag", "größe")] {
        let d = Description::new(id)
            .verb(Verb::Source)
            .input(ArgSpec::new(name))
            .action(ActionSpec::new(Verb::Sink).input(ArgSpec::new(name)));
        d.validate().expect("a ucschar is a name");
        let ttl = ikigai_vocab::to_turtle(&d);
        assert!(turtle_parses(&ttl), "{ttl}");
        assert!(!ttl.contains('%'), "nothing is encoded: {ttl}");
        for spec in d.action_specs() {
            assert!(iri_parses(&spec.contract_iri(&d.id)));
        }
    }
}

#[test]
fn an_authored_iri_is_not_a_name_and_degrades_instead() {
    // A `class` or `requires` value is an authored IRI: the emitter parses it and writes
    // a literal when it is not one, so the document still parses and validate leaves it
    // alone (it is not what ledger #1351 decided).
    for c in REFUSED {
        let d = Description::new("tag-suggest")
            .verb(Verb::Source)
            .input(ArgSpec::new("book").class(format!("urn:cls:a{c}b")))
            .requires(format!("urn:cap:a{c}b"));
        d.validate().expect("not a name");
        assert!(turtle_parses(&ikigai_vocab::to_turtle(&d)));
    }
}
