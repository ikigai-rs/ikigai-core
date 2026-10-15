//! The shared capability vectors (ledger #1583): what core decides about capability
//! coverage and the declared-capability floor, written to
//! `tests/vectors/capability_vectors.json` for the other faces to pin themselves against.
//!
//! ikigai-python and ikigai-deno enforce a served door's declared `requires` themselves
//! (ledger #1580), so each mirrors `is_deny_scope`, `Capability::allows`,
//! `Capability::satisfies`, `Capability::principal`, `Description::required_scopes` and
//! the kernel's denial text by hand. They used to copy core's test tables; this file is the
//! one copy they consume instead (see `tests/vectors/README.md`), and the test below is why
//! it cannot drift: it recomputes every output from the inputs in
//! `tests/vectors/capability_cases.json` and fails if the committed file differs by a byte.
//! Each `denials` row is computed by ISSUING the request through a kernel, so the text is
//! the kernel's own, not a reconstruction of it.
//!
//! To add a case, edit `capability_cases.json`, then regenerate with
//!
//! ```text
//! cargo test -p ikigai-vocab --test capability_vectors -- --ignored
//! ```
//!
//! and commit both files.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    is_deny_scope, ArgRef, Capability, Description, EndpointSpace, Error, Exact, FnEndpoint, Iri,
    Kernel, ReprType, Representation, Request, Verb,
};
use serde_json::{json, Map, Value};

const CASES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/capability_cases.json"
);
const VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/capability_vectors.json"
);

const VERBS: [Verb; 5] = [
    Verb::Source,
    Verb::Sink,
    Verb::Exists,
    Verb::Delete,
    Verb::Meta,
];

const ABOUT: &str = "Capability vectors computed by ikigai-core from \
     tests/vectors/capability_cases.json (crates/ikigai-vocab/tests/capability_vectors.rs, \
     which fails if this file differs from what it computes). A capability is \"root\" or \
     the list of scopes it holds; verbs are spelled as ikigai_core::Verb serializes them; \
     a denial is the text of the kernel's Denied, null when the floor admits the request.";

fn verb_name(verb: Verb) -> String {
    format!("{verb:?}")
}

fn list<'a>(cases: &'a Value, field: &str) -> &'a Vec<Value> {
    cases[field]
        .as_array()
        .unwrap_or_else(|| panic!("capability_cases.json has no `{field}` array"))
}

/// `"root"`, or the list of scopes a scoped capability holds.
fn capability(value: &Value) -> Capability {
    match value {
        Value::String(s) if s == "root" => Capability::root(),
        Value::Array(scopes) => Capability::scoped(
            scopes
                .iter()
                .map(|s| s.as_str().expect("a held scope is a string").to_string()),
        ),
        other => panic!("a capability is \"root\" or a list of scopes, not {other}"),
    }
}

fn description(value: &Value) -> Description {
    serde_json::from_value(value.clone())
        .unwrap_or_else(|e| panic!("a description that core cannot read: {e}\n{value}"))
}

fn verb(value: &Value) -> Verb {
    serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("not a verb: {value}: {e}"))
}

/// Every object's keys in sorted order, whatever serde_json's `preserve_order` feature
/// is unified to in this build, so the file's bytes depend on the cases alone.
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

/// A kernel binding `target` to an endpoint that answers every verb and declares
/// `description`, the way `FnEndpoint` and the builtins do (they do not dispatch on verb).
fn kernel(target: &str, description: Description) -> Kernel {
    let endpoint = FnEndpoint::new("vector", |_| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            b"ok".to_vec(),
        ))
    })
    .with_description(description);
    Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new(target), endpoint),
    ))
}

/// What the kernel answers `verb` on `target` under `capability`: the text of its
/// `Denied`, or `None` when the floor admitted the request (whatever happened next: the
/// endpoint answers every verb, and this kernel has no Meta renderer).
fn denial(kernel: &Kernel, target: &str, verb: Verb, capability: &Capability) -> Option<String> {
    let target = Iri::parse(target).unwrap_or_else(|e| panic!("target {target}: {e}"));
    let mut request = Request::new(verb, target);
    if verb.is_mutating() {
        request = request.with_arg("content", ArgRef::Inline(b"x".to_vec()));
    }
    match block_on(kernel.issue(request, capability)) {
        Err(Error::Denied(message)) => Some(message),
        _ => None,
    }
}

/// What core computes for every case: the bytes `capability_vectors.json` must hold.
fn compute() -> String {
    let text = std::fs::read_to_string(CASES).expect("read capability_cases.json");
    let cases: Value = serde_json::from_str(&text).expect("capability_cases.json is JSON");

    let deny_scopes: Vec<Value> = list(&cases, "deny_scopes")
        .iter()
        .map(|scope| {
            let scope = scope.as_str().expect("a deny_scopes case is a string");
            json!({"scope": scope, "is_deny_scope": is_deny_scope(scope)})
        })
        .collect();

    let coverage: Vec<Value> = list(&cases, "coverage")
        .iter()
        .flat_map(|case| {
            let cap = capability(&case["capability"]);
            case["scopes"]
                .as_array()
                .expect("a coverage case lists `scopes`")
                .iter()
                .map(|scope| {
                    let scope = scope.as_str().expect("a scope is a string");
                    json!({
                        "capability": case["capability"],
                        "scope": scope,
                        "allows": cap.allows(scope),
                        "satisfies": cap.satisfies(scope),
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect();

    let principals: Vec<Value> = list(&cases, "principals")
        .iter()
        .map(|held| json!({"capability": held, "principal": capability(held).principal()}))
        .collect();

    let floors: Vec<Value> = list(&cases, "floors")
        .iter()
        .map(|case| {
            let d = description(&case["description"]);
            let verbs: Vec<Value> = VERBS
                .iter()
                .map(|v| json!({"verb": verb_name(*v), "required_scopes": d.required_scopes(*v)}))
                .collect();
            json!({
                "name": case["name"],
                "description": case["description"],
                "verbs": verbs,
            })
        })
        .collect();

    let denials: Vec<Value> = list(&cases, "denials")
        .iter()
        .map(|case| {
            let target = case["target"].as_str().expect("a denial names a target");
            let k = kernel(target, description(&case["description"]));
            let mut row = case.clone();
            row["denied"] = json!(denial(
                &k,
                target,
                verb(&case["verb"]),
                &capability(&case["capability"])
            ));
            row
        })
        .collect();

    let doc = sorted(json!({
        "about": ABOUT,
        "form": "ikigai:capability:v1",
        "deny_scopes": deny_scopes,
        "coverage": coverage,
        "principals": principals,
        "floors": floors,
        "denials": denials,
    }));
    let mut out = serde_json::to_string_pretty(&doc).expect("serialize the vectors");
    out.push('\n');
    out
}

#[test]
fn the_shared_capability_vectors_are_what_core_computes() {
    let committed = std::fs::read_to_string(VECTORS).unwrap_or_default();
    assert!(
        committed == compute(),
        "tests/vectors/capability_vectors.json is not what core computes from \
         capability_cases.json; regenerate it with \
         `cargo test -p ikigai-vocab --test capability_vectors -- --ignored` and commit it"
    );
}

/// `floors` is `Description::required_scopes`, a helper; this pins it to the floor the
/// kernel ENFORCES, row by row. For every floor and every verb, a caller holding nothing
/// is refused naming the first required scope (or admitted when none is required), and
/// a caller holding exactly the required scopes is admitted. A trailing-`*` scope held
/// literally is a grant under its own prefix, so holding the list satisfies it.
#[test]
fn every_floor_row_is_what_the_kernel_enforces() {
    let doc: Value = serde_json::from_str(&compute()).unwrap();
    let nobody = Capability::scoped(Vec::<String>::new());
    for floor in doc["floors"].as_array().unwrap() {
        let name = floor["name"].as_str().unwrap();
        let target = "urn:vector:floor";
        let k = kernel(target, description(&floor["description"]));
        for row in floor["verbs"].as_array().unwrap() {
            let v = verb(&row["verb"]);
            let required: Vec<String> = serde_json::from_value(row["required_scopes"].clone())
                .expect("required_scopes is a list of strings");
            let refused = denial(&k, target, v, &nobody);
            match required.first() {
                None => assert_eq!(refused, None, "{name} {v:?}: nothing is required"),
                Some(first) => assert!(
                    refused
                        .as_deref()
                        .is_some_and(|m| m.contains(&format!("grant `{first}`"))),
                    "{name} {v:?}: expected a refusal naming {first}, got {refused:?}"
                ),
            }
            let holder = Capability::scoped(required.clone());
            assert_eq!(
                denial(&k, target, v, &holder),
                None,
                "{name} {v:?}: holding {required:?} is refused"
            );
        }
    }
}

/// The three shapes of the denial text, each pinned in the shared file (ledger #1292):
/// an exact scope, a family scope (`prefix*`), and the bare `*`.
#[test]
fn the_shared_vectors_pin_every_denial_shape() {
    let doc: Value = serde_json::from_str(&compute()).unwrap();
    let denied = |name: &str| {
        doc["denials"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == name)
            .unwrap_or_else(|| panic!("no denial named {name}"))["denied"]
            .clone()
    };
    assert_eq!(
        denied("exact"),
        json!("capability does not grant `urn:cap:cal:read` (declared by `urn:cal`)")
    );
    assert_eq!(
        denied("family"),
        json!(
            "capability does not grant `urn:cap:script:run:open-*` (declared by \
             `urn:script:open-door`; any grant under `urn:cap:script:run:open-` satisfies it)"
        )
    );
    assert_eq!(
        denied("star"),
        json!("capability does not grant `*` (declared by `urn:x`; any grant at all satisfies it)")
    );
    assert_eq!(denied("exact-held"), Value::Null);
}

/// Regenerate `capability_vectors.json`. Ignored so a normal run only CHECKS; run it with
/// `-- --ignored` after editing the cases.
#[test]
#[ignore = "writes tests/vectors/capability_vectors.json; run with -- --ignored to regenerate"]
fn regenerate_the_shared_capability_vectors() {
    std::fs::write(VECTORS, compute()).expect("write capability_vectors.json");
}
