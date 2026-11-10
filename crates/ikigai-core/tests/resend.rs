//! **Resend safety** (ledger #3381). A client that sent a request and does not know whether
//! it landed has to decide whether to send it again. A keyed ledger `append` answers
//! `existing` on a replay and writes nothing, so its resend is safe under its key; a plain
//! `post` is not. Before the declaration the contract could not say which is which, and a
//! client had to guess from the verb's name.
//!
//! One test per claim: a contract that says nothing reads as NOT safe (the default, and an
//! old JSON contract); the declaration round-trips through the JSON (Meta) face, both
//! forms; the contract digest moves only for a declared action; a key must be a declared
//! input; and `urn:kernel:actions` says it in its Turtle face.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ActionSpec, ArgRef, ArgSpec, Capability, Description, EndpointSpace, Exact, FnEndpoint, Iri,
    Kernel, ReprType, Representation, Request, Resend, Verb,
};

/// A keyed ledger append: safe to resend when the call carries `key`.
fn append() -> ActionSpec {
    ActionSpec::new(Verb::Sink)
        .input(ArgSpec::new("content"))
        .input(ArgSpec::new("key").optional())
        .resend_safe_with_key("key")
}

/// A ledger with an append (keyed), a delete (always safe) and a read that declares nothing.
fn ledger() -> Description {
    Description::new("ledger")
        .action(append())
        .action(
            ActionSpec::new(Verb::Delete)
                .input(ArgSpec::new("content").optional())
                .resend_safe(),
        )
        .action(ActionSpec::new(Verb::Source).input(ArgSpec::new("at").optional()))
}

fn spec(description: &Description, verb: Verb) -> ActionSpec {
    description
        .action_specs()
        .into_iter()
        .find(|a| a.verb == verb)
        .unwrap()
}

#[test]
fn a_contract_that_says_nothing_is_not_safe_to_resend() {
    let post = ActionSpec::new(Verb::Sink).input(ArgSpec::new("content"));
    assert_eq!(post.resend, None);
    assert!(!post.resend_is_safe(|_| true));
    // An old JSON contract, written before the field, reads the same way.
    let old: ActionSpec =
        serde_json::from_value(serde_json::json!({"verb": "Sink", "inputs": []})).unwrap();
    assert_eq!(old.resend, None);
    assert!(!old.resend_is_safe(|_| true));
    // And the flat form synthesizes an action that is not safe.
    let flat = Description::new("flat").verb(Verb::Sink);
    assert_eq!(spec(&flat, Verb::Sink).resend, None);
    // Nothing is written for it: existing JSON contracts are unchanged.
    let json = serde_json::to_value(&post).unwrap();
    assert!(json.get("resend").is_none(), "{json}");
}

#[test]
fn the_declaration_round_trips_through_the_json_meta_face() {
    let description = ledger();
    let json = serde_json::to_value(&description).unwrap();
    let actions = json["actions"].as_array().unwrap();
    let by_verb = |verb: &str| actions.iter().find(|a| a["verb"] == verb).unwrap();
    assert_eq!(by_verb("Sink")["resend"], serde_json::json!({"key": "key"}));
    assert_eq!(by_verb("Delete")["resend"], serde_json::json!("safe"));
    assert!(by_verb("Source").get("resend").is_none());

    let back: Description = serde_json::from_value(json).unwrap();
    assert_eq!(back, description);
    assert_eq!(
        spec(&back, Verb::Sink).resend,
        Some(Resend::Key("key".to_string()))
    );
    assert_eq!(spec(&back, Verb::Delete).resend, Some(Resend::Safe));
    assert_eq!(spec(&back, Verb::Source).resend, None);
}

#[test]
fn a_keyed_resend_is_safe_only_when_the_key_is_given() {
    let append = append();
    assert!(append.resend_is_safe(|name| name == "key"));
    assert!(!append.resend_is_safe(|name| name == "content"));
    assert_eq!(append.resend.as_ref().and_then(Resend::key), Some("key"));
    assert!(ActionSpec::new(Verb::Delete)
        .resend_safe()
        .resend_is_safe(|_| false));
}

#[test]
fn the_contract_digest_moves_only_for_a_declared_action() {
    let plain = ActionSpec::new(Verb::Sink)
        .input(ArgSpec::new("content"))
        .input(ArgSpec::new("key").optional());
    let keyed = plain.clone().resend_safe_with_key("key");
    let other_key = plain.clone().resend_safe_with_key("content");
    let safe = plain.clone().resend_safe();
    let ids: Vec<String> = [&plain, &keyed, &other_key, &safe]
        .iter()
        .map(|a| a.contract_iri("ledger"))
        .collect();
    for (i, a) in ids.iter().enumerate() {
        for b in &ids[i + 1..] {
            assert_ne!(a, b);
        }
    }
    // An undeclared action is digested exactly as it was before the field (v1, unchanged
    // by deserializing a contract without it).
    let old: ActionSpec = serde_json::from_value(serde_json::json!({
        "verb": "Sink",
        "inputs": [
            {"name": "content", "summary": "", "required": true},
            {"name": "key", "summary": "", "required": false}
        ]
    }))
    .unwrap();
    assert_eq!(old.contract_iri("ledger"), plain.contract_iri("ledger"));
}

#[test]
fn a_resend_key_must_be_a_declared_input() {
    assert!(ledger().validate().is_ok());
    let stray = Description::new("ledger").action(
        ActionSpec::new(Verb::Sink)
            .input(ArgSpec::new("content"))
            .resend_safe_with_key("idempotency"),
    );
    let err = stray.validate().unwrap_err().to_string();
    assert!(err.contains("idempotency"), "{err}");
}

#[test]
fn the_action_manifold_says_whether_a_resend_is_safe() {
    let endpoint = FnEndpoint::new("ledger", |_| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            b"ok".to_vec(),
        ))
    })
    .with_description(ledger());
    let space = EndpointSpace::new().bind(Exact::new("urn:ledger:main"), endpoint);
    let kernel = Kernel::new(Arc::new(space));
    let request = Request::new(Verb::Source, Iri::parse("urn:kernel:actions").unwrap())
        .with_arg("as", ArgRef::Inline(b"text/turtle".to_vec()));
    let turtle = String::from_utf8(
        block_on(kernel.issue(request, &Capability::root()))
            .unwrap()
            .bytes,
    )
    .unwrap();
    let row = |verb: &str| {
        let subject = format!("<urn:ikigai:match:{verb}:urn:ledger:main>");
        let start = turtle
            .find(&subject)
            .unwrap_or_else(|| panic!("{subject} in {turtle}"));
        let end = start + turtle[start..].find(" .\n").unwrap();
        turtle[start..end].to_string()
    };
    let keyed = spec(&ledger(), Verb::Sink).contract_iri("ledger");
    assert!(
        row("sink").contains(&format!("ik:resendKey <{keyed}:input:key>")),
        "{turtle}"
    );
    assert!(row("delete").contains("ik:resendSafe true"), "{turtle}");
    let read = row("source");
    assert!(
        !read.contains("resendSafe") && !read.contains("resendKey"),
        "{read}"
    );
}
