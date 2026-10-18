//! The shared validation vectors (ledger #1547, design note section 3.3): what core's ONE
//! checker, `ActionSpec::check`, answers for a contract and a proposed argument set,
//! written to `tests/vectors/validation_vectors.json` for the faces that cannot call it.
//!
//! ikigai-python's and ikigai-deno's `serve` enforce the same rules a third and fourth
//! way (and ikigai-mcp a second, until it calls core); this file is the one table they
//! are held to, so four implementations stay one. The test below recomputes every row
//! from `tests/vectors/validation_cases.json` and fails if the committed file differs by
//! a byte.
//!
//! To add a case, edit `validation_cases.json`, then regenerate with
//!
//! ```text
//! cargo test -p ikigai-vocab --test validation_vectors -- --ignored
//! ```
//!
//! and commit both files.

use ikigai_core::{ActionSpec, ProposedArgs};
use serde_json::{json, Map, Value};

const CASES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/validation_cases.json"
);
const VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/validation_vectors.json"
);

const ABOUT: &str = "Validation vectors computed by ikigai-core's ActionSpec::check from \
     tests/vectors/validation_cases.json (crates/ikigai-vocab/tests/validation_vectors.rs, \
     which fails if this file differs from what it computes). `action` is the serde shape \
     of ikigai_core::ActionSpec; `args` are the proposed (name, value) pairs in order, \
     repeats kept; `violations` are in the checker's order, each a `kind` word and a \
     `path` (`input:{name}`, `group:{kind}:{members}` or `input:{name}:value:{value}`, \
     null for an undeclared name).";

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

fn compute() -> String {
    let text = std::fs::read_to_string(CASES).expect("read validation_cases.json");
    let cases: Value = serde_json::from_str(&text).expect("validation_cases.json is JSON");
    let rows: Vec<Value> = cases["cases"]
        .as_array()
        .expect("a `cases` array")
        .iter()
        .map(|case| {
            let name = &case["name"];
            let spec: ActionSpec = serde_json::from_value(case["action"].clone())
                .unwrap_or_else(|e| panic!("case {name}: {e}"));
            let pairs = case["args"]
                .as_array()
                .unwrap_or_else(|| panic!("case {name}: `args` is an array of pairs"))
                .iter()
                .map(|pair| {
                    let pair = pair.as_array().expect("a pair");
                    (
                        pair[0].as_str().expect("a name").to_string(),
                        pair[1].as_str().expect("a value").to_string(),
                    )
                });
            let violations: Vec<Value> = spec
                .check(&ProposedArgs::from_pairs(pairs))
                .iter()
                .map(|v| {
                    json!({
                        "kind": v.kind.word(),
                        "path": v.path.as_ref().map(|p| p.word()),
                    })
                })
                .collect();
            json!({
                "name": name,
                "action": case["action"],
                "args": case["args"],
                "conforms": violations.is_empty(),
                "violations": violations,
            })
        })
        .collect();
    let doc = sorted(json!({"about": ABOUT, "cases": rows}));
    let mut out = serde_json::to_string_pretty(&doc).expect("serialize the vectors");
    out.push('\n');
    out
}

#[test]
fn the_shared_validation_vectors_are_what_core_computes() {
    let committed = std::fs::read_to_string(VECTORS).unwrap_or_default();
    assert!(
        committed == compute(),
        "tests/vectors/validation_vectors.json is not what core computes from \
         validation_cases.json; regenerate it with \
         `cargo test -p ikigai-vocab --test validation_vectors -- --ignored` and commit it"
    );
}

/// The rows a face is most likely to get wrong are pinned by name, so a change to the
/// checker that moves one is a test failure here and not only a byte diff.
#[test]
fn the_vectors_pin_the_rules_that_are_easiest_to_get_wrong() {
    let doc: Value = serde_json::from_str(&compute()).unwrap();
    let row = |name: &str| -> Vec<(String, Value)> {
        let case = doc["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no case {name}"));
        case["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| (v["kind"].as_str().unwrap().to_string(), v["path"].clone()))
            .collect()
    };
    let path = |p: &str| json!(p);
    assert!(row("closed/binding-is-never-missing").is_empty());
    assert!(row("exactly-one/a-blank-member-is-not-given").is_empty());
    assert_eq!(
        row("exactly-one/none"),
        [(
            "exactly-one".to_string(),
            path("group:exactly-one:content,thread")
        )]
    );
    assert!(row("list/repeats-are-joined").is_empty());
    assert_eq!(
        row("list/a-non-list-repeat-is-given-twice"),
        [("given-twice".to_string(), path("input:query"))]
    );
    assert_eq!(
        row("value-note/needs-unmet"),
        [("needs".to_string(), path("input:decision:value:retract"))]
    );
    assert_eq!(
        row("enumerated-empty/any-value-is-refused"),
        [("not-one-of".to_string(), path("input:provider"))]
    );
    assert!(row("open/admits-any-name").is_empty());
    assert_eq!(
        row("envelope/both")[0],
        ("envelope".to_string(), path("input:content"))
    );
    assert_eq!(
        row("closed/unknown-given-twice-is-reported-once"),
        [("unknown".to_string(), Value::Null)]
    );
}

/// Regenerate `validation_vectors.json`. Ignored so a normal run only CHECKS.
#[test]
#[ignore = "writes tests/vectors/validation_vectors.json; run with -- --ignored to regenerate"]
fn regenerate_the_shared_validation_vectors() {
    std::fs::write(VECTORS, compute()).expect("write validation_vectors.json");
}
