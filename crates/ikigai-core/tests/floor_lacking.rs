//! **A declared-floor refusal carries the missing scopes** (ledger #3840).
//!
//! When an endpoint declares `requires` and the caller's capability does not satisfy it,
//! the kernel refuses before dispatch with [`Error::Denied`]. A host's door that logs
//! requests wants to record WHICH capability was missing (an access log's `needs=`
//! column) without parsing the sentence, and none of a host's transports issues a traced
//! request per call, so the trace's [`DENIED_NOTE`] does not reach it. The refusal itself
//! carries the scopes: [`Error::lacking`].
//!
//! These pin that the floor's refusal lacks exactly the unmet scopes (all of them), that
//! an endpoint's own run-time refusal lacks none, that the sentence and the class a 403
//! is chosen by are unchanged, and that a nested sub-request's floor refusal carries its
//! own scope.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Capability, Description, EndpointSpace, Error, Exact, FailureClass,
    FnEndpoint, Iri, Kernel, ReprType, Representation, Request, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(body: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec())
}

fn source(kernel: &Kernel, target: &str, capability: &Capability) -> Result<Representation, Error> {
    block_on(kernel.issue(Request::new(Verb::Source, iri(target)), capability))
}

fn holding(scopes: &[&str]) -> Capability {
    Capability::scoped(scopes.iter().map(|s| s.to_string()).collect::<Vec<_>>())
}

fn kernel() -> Kernel {
    // One scope declared.
    let vault = FnEndpoint::new("vault", |_| Ok(text("secret")))
        .with_description(Description::new("vault").requires("urn:cap:t:vault:read"));
    // Three declared: an exact scope, a family, and another exact scope.
    let ledger = FnEndpoint::new("ledger", |_| Ok(text("rows"))).with_description(
        Description::new("ledger")
            .requires("urn:cap:t:ledger:read")
            .requires("urn:cap:t:audit:*")
            .requires("urn:cap:t:ledger:export"),
    );
    // Declares nothing and refuses at run time, on its own grounds.
    let acl = FnEndpoint::new("acl", |_| {
        Err(Error::Denied(
            "path /etc/secret is outside the granted roots".into(),
        ))
    });
    // A composite that declares its own scope and reads the vault, propagating
    // whatever the vault's floor answers.
    let report = AsyncFnEndpoint::new("report", |inv| {
        Box::pin(async move {
            let secret = inv.source(&Iri::parse("urn:t:vault").unwrap()).await?;
            Ok(secret)
        })
    })
    .with_description(Description::new("report").requires("urn:cap:t:report"));
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:t:vault"), vault)
            .bind(Exact::new("urn:t:ledger"), ledger)
            .bind(Exact::new("urn:t:acl"), acl)
            .bind(Exact::new("urn:t:report"), report),
    ))
}

#[test]
fn a_floor_refusal_lacks_exactly_the_one_missing_scope() {
    let err = source(&kernel(), "urn:t:vault", &holding(&["urn:cap:t:other"])).unwrap_err();
    assert_eq!(err.lacking(), ["urn:cap:t:vault:read"]);
}

#[test]
fn a_floor_refusal_lacks_every_missing_scope_and_only_those() {
    let k = kernel();
    // Holding none of the three: all three, in declaration order, the family as declared.
    let err = source(&k, "urn:t:ledger", &holding(&[])).unwrap_err();
    assert_eq!(
        err.lacking(),
        [
            "urn:cap:t:ledger:read",
            "urn:cap:t:audit:*",
            "urn:cap:t:ledger:export",
        ]
    );
    // Holding one exact scope and a grant under the family: only the third is missing.
    let err = source(
        &k,
        "urn:t:ledger",
        &holding(&["urn:cap:t:ledger:read", "urn:cap:t:audit:trail"]),
    )
    .unwrap_err();
    assert_eq!(err.lacking(), ["urn:cap:t:ledger:export"]);
    // And it agrees with the description's own reading of the same floor.
    let description = Description::new("ledger")
        .requires("urn:cap:t:ledger:read")
        .requires("urn:cap:t:audit:*")
        .requires("urn:cap:t:ledger:export");
    let caller = holding(&["urn:cap:t:audit:trail"]);
    let err = source(&k, "urn:t:ledger", &caller).unwrap_err();
    assert_eq!(
        err.lacking(),
        description.unsatisfied_scopes(Verb::Source, &caller)
    );
}

#[test]
fn the_sentence_and_the_class_are_unchanged() {
    let k = kernel();
    let err = source(&k, "urn:t:vault", &holding(&[])).unwrap_err();
    assert_eq!(
        err.to_string(),
        "denied: capability does not grant `urn:cap:t:vault:read` (declared by `urn:t:vault`)"
    );
    // Several missing: the sentence still names the first alone, byte for byte.
    let err = source(&k, "urn:t:ledger", &holding(&[])).unwrap_err();
    assert_eq!(
        err.to_string(),
        "denied: capability does not grant `urn:cap:t:ledger:read` (declared by `urn:t:ledger`)"
    );
    // Still a `Denied`: the same variant, kind, class and 403 an edge chooses by them.
    assert!(matches!(&err, Error::Denied(m) if m.contains("urn:cap:t:ledger:read")));
    assert_eq!(err.kind(), "denied");
    assert_eq!(err.class(), FailureClass::Denied);
    assert!(!err.is_transient());
}

#[test]
fn an_endpoints_own_refusal_lacks_nothing() {
    let err = source(&kernel(), "urn:t:acl", &holding(&[])).unwrap_err();
    assert!(matches!(err, Error::Denied(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "denied: path /etc/secret is outside the granted roots"
    );
    assert!(err.lacking().is_empty(), "{err:?}");
}

#[test]
fn a_nested_floor_refusal_carries_its_own_scope() {
    // The caller passes the composite's floor and fails the vault's: the refusal that
    // comes back is the vault's, lacking the vault's scope, not the composite's.
    let err = source(&kernel(), "urn:t:report", &holding(&["urn:cap:t:report"])).unwrap_err();
    assert_eq!(err.lacking(), ["urn:cap:t:vault:read"]);
    assert_eq!(
        err.to_string(),
        "denied: capability does not grant `urn:cap:t:vault:read` (declared by `urn:t:vault`)"
    );
    // Failing the composite's own floor stops there, before the sub-request is issued.
    let err = source(&kernel(), "urn:t:report", &holding(&[])).unwrap_err();
    assert_eq!(err.lacking(), ["urn:cap:t:report"]);
}

#[test]
fn the_json_face_carries_the_scopes_and_reads_them_back() {
    let err = source(
        &kernel(),
        "urn:t:ledger",
        &holding(&["urn:cap:t:ledger:read"]),
    )
    .unwrap_err();
    let json = err.to_json();
    assert_eq!(
        json["lacking"],
        serde_json::json!(["urn:cap:t:audit:*", "urn:cap:t:ledger:export"])
    );
    assert_eq!(Error::from_json(&json).unwrap(), err);
    // A runtime refusal's face is unchanged: no `lacking` field at all.
    let own = source(&kernel(), "urn:t:acl", &holding(&[])).unwrap_err();
    assert_eq!(
        own.to_json(),
        serde_json::json!({
            "kind": "denied",
            "message": "path /etc/secret is outside the granted roots",
        })
    );
}
