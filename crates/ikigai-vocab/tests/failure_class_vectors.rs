//! The shared failure-class vectors (ledger #1546, #200; the retry-authority note, section
//! 2.2): for every `ikigai_core::Error` variant, its kind word, its failure class, whether it
//! is transient, its display form and its delay, which core computes, beside the decided
//! wire v9 tag and HTTP edge status, written to `tests/vectors/failure_class_vectors.json`
//! for the faces that cannot call core.
//!
//! The Rust wire suite and the edge's `error_status` (ikigai-cli), ikigai-python and
//! ikigai-deno each map errors a second, third and fourth way; this file is the one table they
//! are held to. The test below recomputes every row from
//! `tests/vectors/failure_class_cases.json` and fails if the committed file differs by a byte.
//!
//! To add a case, edit `failure_class_cases.json`, then regenerate with
//!
//! ```text
//! cargo test -p ikigai-vocab --test failure_class_vectors -- --ignored
//! ```
//!
//! and commit both files.

use std::collections::BTreeSet;
use std::time::Duration;

use ikigai_core::{Error, FailureClass, Iri, Verb};
use serde_json::{json, Map, Value};

const CASES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/failure_class_cases.json"
);
const VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/failure_class_vectors.json"
);

const ABOUT: &str = "Failure-class vectors (ledger #1546) computed from \
     tests/vectors/failure_class_cases.json by crates/ikigai-vocab/tests/failure_class_vectors.rs, \
     which fails if this file differs from what it computes. Per case, ikigai-core computes \
     `kind` (Error::kind), `class` (Error::class, a FailureClass word), `transient` \
     (Error::is_transient), `display` (the Display form a v8 peer receives inside Endpoint) and \
     `retry_after_ms` (Error::retry_after_millis: the delay in whole milliseconds rounded UP, \
     the wire form). The rest is the DECIDED table this file pins for the faces (decision 4 of \
     ledger #1546): `wire_tag` is the wire v9 discriminant (0-8 as today, 9-13 appended; \
     ikigai-wire owns the encoding), and `http` is the edge's answer: `status`, and \
     `retry_after`, the Retry-After header in delay-seconds rounded up and never 0, null when \
     no header is sent. `classes` lists every class with its vocabulary individual.";

/// Every object's keys in sorted order, so the file's bytes depend on the cases alone.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k, sorted(v)))
                    .collect::<Map<_, _>>(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

/// Build the error a case names.
fn error_of(name: &Value, spec: &Value) -> Error {
    let s = |field: &str| -> String {
        spec[field]
            .as_str()
            .unwrap_or_else(|| panic!("case {name}: `{field}` is a string"))
            .to_string()
    };
    match spec["kind"].as_str().expect("a kind") {
        "unresolved" => Error::Unresolved(Iri::parse(s("iri")).expect("an IRI")),
        "missing-argument" => Error::MissingArgument(s("name")),
        "invalid-argument" => Error::InvalidArgument {
            name: s("name"),
            detail: s("detail"),
        },
        "endpoint" => Error::Endpoint(s("message")),
        "denied" => Error::Denied(s("message")),
        "not-found" => Error::NotFound(s("message")),
        "conflict" => Error::Conflict(s("message")),
        "timeout" => Error::Timeout(s("message")),
        "unavailable" => Error::Unavailable(s("message")),
        "depth-exceeded" => Error::DepthExceeded {
            depth: spec["depth"].as_u64().expect("a depth") as u32,
            target: Iri::parse(s("target")).expect("an IRI"),
        },
        "retry-after" => Error::RetryAfter {
            delay: spec["delay_us"].as_u64().map(Duration::from_micros),
            detail: s("detail"),
        },
        "over-budget" => Error::OverBudget {
            detail: s("detail"),
        },
        "refused" => Error::Refused(s("message")),
        "unsupported" => Error::Unsupported {
            verb: spec
                .get("verb")
                .map(|v| serde_json::from_value::<Verb>(v.clone()).expect("a verb")),
            detail: s("detail"),
        },
        other => panic!("case {name}: no variant has the kind `{other}`"),
    }
}

/// The wire v9 tag (the note's section 2.1): today's postcard discriminants for 0-8, the
/// new variants appended, never reordered.
fn wire_tag(error: &Error) -> u8 {
    match error {
        Error::Unresolved(_) => 0,
        Error::MissingArgument(_) => 1,
        Error::InvalidArgument { .. } => 2,
        Error::Endpoint(_) => 3,
        Error::Denied(_) => 4,
        Error::NotFound(_) => 5,
        Error::Timeout(_) => 6,
        Error::Unavailable(_) => 7,
        Error::Conflict(_) => 8,
        Error::DepthExceeded { .. } => 9,
        Error::RetryAfter { .. } => 10,
        Error::OverBudget { .. } => 11,
        Error::Refused(_) => 12,
        Error::Unsupported { .. } => 13,
        other => panic!("no decided wire tag for `{}`", other.kind()),
    }
}

/// The edge's status (decision 4): Timeout 504, OverBudget 422, Refused 502, DepthExceeded
/// and Endpoint 500, RetryAfter 429; Unsupported 405 for a verb and 501 for a facility.
fn http_status(error: &Error) -> u16 {
    match error {
        Error::Unresolved(_) | Error::NotFound(_) => 404,
        Error::MissingArgument(_) | Error::InvalidArgument { .. } => 400,
        Error::Denied(_) => 403,
        Error::Conflict(_) => 409,
        Error::Timeout(_) => 504,
        Error::Unavailable(_) => 503,
        Error::RetryAfter { .. } => 429,
        Error::OverBudget { .. } => 422,
        Error::Refused(_) => 502,
        Error::Unsupported { verb: Some(_), .. } => 405,
        Error::Unsupported { verb: None, .. } => 501,
        Error::Endpoint(_) | Error::DepthExceeded { .. } => 500,
        other => panic!("no decided status for `{}`", other.kind()),
    }
}

/// The Retry-After header in delay-seconds: rounded UP, never 0 (RFC 9110 §10.2.3 allows 0,
/// which tells a client to come back at once, the opposite of what was meant), and absent
/// when the error states no delay. The rule ikigai-web's `source_limit.rs` already applies.
fn retry_after_header(error: &Error) -> Option<u64> {
    error
        .retry_after_millis()
        .map(|ms| ms.div_ceil(1000).max(1))
}

fn compute() -> String {
    let text = std::fs::read_to_string(CASES).expect("read failure_class_cases.json");
    let cases: Value = serde_json::from_str(&text).expect("failure_class_cases.json is JSON");
    let rows: Vec<Value> = cases["cases"]
        .as_array()
        .expect("a `cases` array")
        .iter()
        .map(|case| {
            let name = &case["name"];
            let error = error_of(name, &case["error"]);
            json!({
                "name": name,
                "error": case["error"],
                "kind": error.kind(),
                "class": error.class().word(),
                "transient": error.is_transient(),
                "display": error.to_string(),
                "retry_after_ms": error.retry_after_millis(),
                "wire_tag": wire_tag(&error),
                "http": {
                    "status": http_status(&error),
                    "retry_after": retry_after_header(&error),
                },
            })
        })
        .collect();
    let classes: Vec<Value> = FailureClass::ALL
        .iter()
        .map(|class| {
            json!({
                "word": class.word(),
                "iri": ikigai_vocab::failure_class_iri(*class),
            })
        })
        .collect();
    let doc = sorted(json!({"about": ABOUT, "cases": rows, "classes": classes}));
    let mut out = serde_json::to_string_pretty(&doc).expect("serialize the vectors");
    out.push('\n');
    out
}

#[test]
fn the_shared_failure_class_vectors_are_what_core_computes() {
    let committed = std::fs::read_to_string(VECTORS).unwrap_or_default();
    assert!(
        committed == compute(),
        "tests/vectors/failure_class_vectors.json is not what core computes from \
         failure_class_cases.json; regenerate it with \
         `cargo test -p ikigai-vocab --test failure_class_vectors -- --ignored` and commit it"
    );
}

/// The file covers every kind and every class, so a face that pins it has seen them all.
#[test]
fn the_vectors_cover_every_kind_and_every_class() {
    let doc: Value = serde_json::from_str(&compute()).unwrap();
    let rows = doc["cases"].as_array().unwrap();
    let kinds: BTreeSet<&str> = rows.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    let expected: BTreeSet<&str> = [
        "unresolved",
        "missing-argument",
        "invalid-argument",
        "endpoint",
        "denied",
        "not-found",
        "conflict",
        "timeout",
        "unavailable",
        "depth-exceeded",
        "retry-after",
        "over-budget",
        "refused",
        "unsupported",
    ]
    .into();
    assert_eq!(kinds, expected);
    let classes: BTreeSet<&str> = rows.iter().map(|r| r["class"].as_str().unwrap()).collect();
    let all: BTreeSet<&str> = FailureClass::ALL.iter().map(|c| c.word()).collect();
    assert_eq!(classes, all);
    // One tag per kind, and no two kinds share one.
    let tags: BTreeSet<(u64, &str)> = rows
        .iter()
        .map(|r| (r["wire_tag"].as_u64().unwrap(), r["kind"].as_str().unwrap()))
        .collect();
    let tag_values: BTreeSet<u64> = tags.iter().map(|(t, _)| *t).collect();
    assert_eq!(tag_values.len(), expected.len(), "{tags:?}");
}

/// The rows a face is most likely to get wrong are pinned by name.
#[test]
fn the_vectors_pin_the_rules_that_are_easiest_to_get_wrong() {
    let doc: Value = serde_json::from_str(&compute()).unwrap();
    let row = |name: &str| -> Value {
        doc["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no case {name}"))
            .clone()
    };
    // Retry-after is NOT transient, and carries the 429 with a rounded-up header.
    let seconds = row("retry-after/seconds");
    assert_eq!(seconds["transient"], json!(false));
    assert_eq!(seconds["http"], json!({"status": 429, "retry_after": 30}));
    assert_eq!(
        row("retry-after/a-fraction-of-a-second-rounds-up")["http"]["retry_after"],
        json!(2)
    );
    let sub_ms = row("retry-after/a-fraction-of-a-millisecond-rounds-up");
    assert_eq!(sub_ms["retry_after_ms"], json!(2));
    assert_eq!(sub_ms["display"], json!("retry after 2 ms: held off"));
    assert_eq!(
        row("retry-after/zero-is-never-a-zero-header")["http"]["retry_after"],
        json!(1)
    );
    let later = row("retry-after/no-delay");
    assert_eq!(later["http"], json!({"status": 429, "retry_after": null}));
    assert_eq!(later["display"], json!("retry later: not now"));
    // The contested statuses (decision 4).
    assert_eq!(row("timeout")["http"]["status"], json!(504));
    assert_eq!(row("over-budget")["http"]["status"], json!(422));
    assert_eq!(row("refused")["http"]["status"], json!(502));
    assert_eq!(row("depth-exceeded")["http"]["status"], json!(500));
    assert_eq!(row("endpoint")["http"]["status"], json!(500));
    assert_eq!(row("endpoint")["class"], json!("unknown"));
    // Unsupported's two cases are two statuses, one class.
    assert_eq!(row("unsupported/a-verb")["http"]["status"], json!(405));
    assert_eq!(
        row("unsupported/a-host-facility")["http"]["status"],
        json!(501)
    );
    assert_eq!(row("unsupported/a-verb")["class"], json!("permanent"));
}

/// Regenerate `failure_class_vectors.json`. Ignored so a normal run only CHECKS.
#[test]
#[ignore = "writes tests/vectors/failure_class_vectors.json; run with -- --ignored to regenerate"]
fn regenerate_the_shared_failure_class_vectors() {
    std::fs::write(VECTORS, compute()).expect("write failure_class_vectors.json");
}
