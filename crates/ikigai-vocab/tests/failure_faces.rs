//! An error's faces carry its numbers (ledger #3386): a retry overlay that gave up says how
//! many attempts and how long as fields, and a caller reads them from the display, the JSON
//! face (`Error::to_json`, read back by `Error::from_json`) and the Turtle face
//! (`Error::to_turtle`) without parsing a sentence. The shared vectors pin the same rows for
//! the faces in other languages (`failure_class_vectors.rs`).

use std::collections::BTreeSet;
use std::time::Duration;

use ikigai_core::{Error, FailureClass, Iri, Verb};
use oxrdf::{Term, Triple};

const IK: &str = "https://ikigai-rs.dev/ns#";
const XSD_NON_NEGATIVE: &str = "http://www.w3.org/2001/XMLSchema#nonNegativeInteger";

fn parse(ttl: &str) -> Vec<Triple> {
    oxttl::TurtleParser::new()
        .for_slice(ttl.as_bytes())
        .collect::<Result<_, _>>()
        .unwrap_or_else(|e| panic!("{e}\n{ttl}"))
}

/// The one object of `ik:{predicate}` in a face (which has one subject).
fn object(graph: &[Triple], predicate: &str) -> Option<Term> {
    let mut found = graph
        .iter()
        .filter(|t| t.predicate.as_str() == format!("{IK}{predicate}"))
        .map(|t| t.object.clone());
    let first = found.next();
    assert!(found.next().is_none(), "ik:{predicate} is stated twice");
    first
}

/// A typed count's value, checking its datatype.
fn count(graph: &[Triple], predicate: &str) -> Option<u64> {
    object(graph, predicate).map(|term| match term {
        Term::Literal(literal) => {
            assert_eq!(
                literal.datatype().as_str(),
                XSD_NON_NEGATIVE,
                "ik:{predicate}"
            );
            literal.value().parse().expect("a whole number")
        }
        other => panic!("ik:{predicate} is not a literal: {other}"),
    })
}

fn text(graph: &[Triple], predicate: &str) -> String {
    match object(graph, predicate) {
        Some(Term::Literal(literal)) => literal.value().to_string(),
        other => panic!("ik:{predicate}: {other:?}"),
    }
}

fn gave_up() -> Error {
    Error::GaveUp {
        attempts: 9,
        elapsed: Duration::from_secs(120),
        detail: "unavailable: the hub refused the connection".into(),
    }
}

#[test]
fn the_display_sentence_names_both_numbers() {
    let sentence = gave_up().to_string();
    assert_eq!(
        sentence,
        "gave up after 9 attempts over 120000 ms: unavailable: the hub refused the connection"
    );
    assert!(
        sentence.starts_with("gave up"),
        "the kind comes first, in words"
    );
    let once = Error::GaveUp {
        attempts: 1,
        elapsed: Duration::from_millis(250),
        detail: "timeout: slow".into(),
    };
    assert_eq!(
        once.to_string(),
        "gave up after 1 attempt over 250 ms: timeout: slow"
    );
}

/// A gave-up is told apart from a refusal and from one timeout by its fields, not its text.
#[test]
fn a_gave_up_is_typed_apart_from_a_refusal_and_a_timeout() {
    let error = gave_up();
    assert_eq!(error.kind(), "gave-up");
    assert_eq!(error.class(), FailureClass::Transient);
    assert!(error.is_transient());
    assert_eq!(error.attempts(), Some(9));
    assert_eq!(error.elapsed_millis(), Some(120_000));
    for other in [
        Error::Refused("401".into()),
        Error::Timeout("slow".into()),
        Error::Unavailable("down".into()),
    ] {
        assert_eq!(other.attempts(), None, "{other}");
        assert_eq!(other.elapsed_millis(), None, "{other}");
        assert_ne!(other.kind(), "gave-up");
    }
}

#[test]
fn a_gave_up_round_trips_through_its_json_face_with_its_numbers() {
    let error = gave_up();
    let json = error.to_json();
    assert_eq!(json["attempts"], 9);
    assert_eq!(json["elapsed_ms"], 120_000);
    // Through text, as a face in another language receives it.
    let wire = serde_json::to_string(&json).unwrap();
    let back = Error::from_json(&serde_json::from_str(&wire).unwrap()).unwrap();
    assert_eq!(back, error);
    assert_eq!(back.to_string(), error.to_string());
    // Durations cross in whole milliseconds: one pass rounds, a second changes nothing.
    let fine = Error::GaveUp {
        attempts: 3,
        elapsed: Duration::from_micros(1_900),
        detail: String::new(),
    };
    let once = Error::from_json(&fine.to_json()).unwrap();
    assert_eq!(once.elapsed_millis(), Some(1));
    assert_eq!(Error::from_json(&once.to_json()).unwrap(), once);
}

#[test]
fn a_gave_up_round_trips_through_its_turtle_face_with_its_numbers() {
    let error = gave_up();
    let graph = parse(&error.to_turtle());
    let typed = graph
        .iter()
        .find(|t| t.predicate.as_str() == "http://www.w3.org/1999/02/22-rdf-syntax-ns#type")
        .expect("a type");
    assert_eq!(typed.object.to_string(), format!("<{IK}Failure>"));
    assert_eq!(text(&graph, "failureKind"), "gave-up");
    assert_eq!(
        object(&graph, "failureClass").unwrap().to_string(),
        format!(
            "<{}>",
            ikigai_vocab::failure_class_iri(FailureClass::Transient)
        )
    );
    assert_eq!(text(&graph, "failureMessage"), error.to_string());
    assert_eq!(count(&graph, "attempts"), error.attempts().map(u64::from));
    assert_eq!(count(&graph, "elapsedMillis"), error.elapsed_millis());
    assert_eq!(count(&graph, "retryAfterMillis"), None);
}

/// Every kind's Turtle face parses, states its class as the vocabulary individual, its
/// sentence as written (escapes included), and only the numbers it has.
#[test]
fn every_kind_has_a_turtle_face_with_only_its_own_numbers() {
    let errors = [
        Error::Unresolved(Iri::parse("urn:x:missing").unwrap()),
        Error::MissingArgument("in".into()),
        Error::InvalidArgument {
            name: "mode".into(),
            detail: "not \"add\"\nnor \\ remove".into(),
        },
        Error::Endpoint("boom".into()),
        Error::Denied("no grant".into()),
        Error::NotFound("absent".into()),
        Error::Gone("retired".into()),
        Error::Conflict("taken".into()),
        Error::Timeout("slow".into()),
        Error::Unavailable("down".into()),
        Error::DepthExceeded {
            depth: 65,
            target: Iri::parse("urn:x:self").unwrap(),
        },
        Error::RetryAfter {
            delay: Some(Duration::from_micros(1_200)),
            detail: "held off".into(),
        },
        Error::RetryAfter {
            delay: None,
            detail: "not now".into(),
        },
        Error::OverBudget {
            detail: "past 5000 ms".into(),
        },
        Error::Refused("401".into()),
        Error::Unsupported {
            verb: Some(Verb::Sink),
            detail: "read-only".into(),
        },
        gave_up(),
    ];
    for error in errors {
        let graph = parse(&error.to_turtle());
        assert_eq!(text(&graph, "failureKind"), error.kind(), "{error}");
        assert_eq!(text(&graph, "failureMessage"), error.to_string(), "{error}");
        assert_eq!(
            object(&graph, "failureClass").unwrap().to_string(),
            format!("<{}>", ikigai_vocab::failure_class_iri(error.class())),
            "{error}"
        );
        assert_eq!(count(&graph, "attempts"), error.attempts().map(u64::from));
        assert_eq!(count(&graph, "elapsedMillis"), error.elapsed_millis());
        assert_eq!(
            count(&graph, "retryAfterMillis"),
            error.retry_after_millis()
        );
        // And its JSON face reads back to the same error, at millisecond precision.
        let back = Error::from_json(&error.to_json()).unwrap();
        assert_eq!(back.to_string(), error.to_string(), "{error}");
    }
}

/// The Turtle face names no term the vocabulary lacks.
#[test]
fn the_turtle_face_uses_only_vocabulary_terms() {
    let defined: BTreeSet<String> = parse(ikigai_vocab::VOCABULARY)
        .into_iter()
        .map(|t| t.subject.to_string())
        .collect();
    let faces = [
        gave_up(),
        Error::RetryAfter {
            delay: Some(Duration::from_secs(1)),
            detail: String::new(),
        },
    ];
    for error in faces {
        for triple in parse(&error.to_turtle()) {
            let terms = [triple.predicate.to_string(), triple.object.to_string()];
            for term in terms
                .into_iter()
                .filter(|t| t.starts_with(&format!("<{IK}")))
            {
                assert!(defined.contains(&term), "{term} is not in the vocabulary");
            }
        }
    }
}

#[test]
fn the_json_face_refuses_what_it_cannot_read_and_names_the_field() {
    let refusal = |value: serde_json::Value| Error::from_json(&value).unwrap_err().to_string();
    assert_eq!(
        refusal(serde_json::json!("gave-up")),
        "invalid argument `error`: expected a JSON object"
    );
    assert_eq!(
        refusal(serde_json::json!({"kind": "shrug"})),
        "invalid argument `kind`: `shrug` is not the kind of any error"
    );
    assert_eq!(
        refusal(serde_json::json!({"kind": "gave-up", "attempts": 9, "detail": "x"})),
        "invalid argument `elapsed_ms`: expected a whole number"
    );
    assert_eq!(
        refusal(
            serde_json::json!({"kind": "gave-up", "attempts": 4294967296u64, "elapsed_ms": 1, "detail": "x"})
        ),
        "invalid argument `attempts`: expected a whole number from 0 to 4294967295"
    );
}
