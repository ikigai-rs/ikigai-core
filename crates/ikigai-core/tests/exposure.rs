//! **Doors a module serves without meaning to** (ledger #3796). A module's privacy is
//! admission: `Mount(prefix, Level(name, inner))` admits the names under `prefix`, and
//! a door inside the level whose pattern matches one SERVES it, whatever its author
//! meant. First the hazard, as the kernel serves it; then the static check that
//! reports it before anyone calls it.

use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    check_exposure, Bindings, Capability, EndpointSpace, Exact, Exposes, FnEndpoint, Grammar, Iri,
    Kernel, Level, MatchKind, Mount, ReprType, Representation, Request, Space, UriTemplate, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn says(id: &'static str) -> FnEndpoint {
    FnEndpoint::new(id, move |_inv| {
        Ok(Representation::new(ReprType::new("text/plain"), id.as_bytes().to_vec()).cacheable())
    })
}

const LEVEL: &str = "urn:example:level:mod";

/// One intended public door, one internal helper under the exposed prefix, one
/// internal helper outside it.
fn module() -> Arc<dyn Space> {
    Arc::new(Mount::new(
        "urn:mod:",
        Arc::new(Level::new(
            iri(LEVEL),
            Arc::new(
                EndpointSpace::new()
                    .bind(Exact::new("urn:mod:get"), says("get"))
                    .bind(
                        UriTemplate::parse("urn:mod:cache:{key}").unwrap(),
                        says("cache"),
                    )
                    .bind(Exact::new("urn:helper:normalize"), says("normalize")),
            ),
        )),
    ))
}

fn answer(kernel: &Kernel, target: &str) -> String {
    match block_on(kernel.issue(Request::new(Verb::Source, iri(target)), &Capability::root())) {
        Ok(r) => String::from_utf8_lossy(&r.bytes).into_owned(),
        Err(e) => format!("error: {e}"),
    }
}

#[test]
fn an_internal_helper_under_the_exposed_prefix_is_served_from_outside() {
    // The hazard, reproduced: nothing in the arrangement says the cache helper is
    // internal, and the guard admits its name, so an outside request reaches it.
    let kernel = Kernel::new(module());
    assert_eq!(answer(&kernel, "urn:mod:get"), "get");
    assert_eq!(answer(&kernel, "urn:mod:cache:abc"), "cache");
    // The helper outside the prefix stays private: the guard never admits it.
    assert!(answer(&kernel, "urn:helper:normalize").starts_with("error:"));
}

#[test]
fn the_check_flags_the_internal_door_under_the_prefix_and_not_the_one_outside_it() {
    let kernel = Kernel::new(module());
    let exposes = Exposes::new().level(LEVEL, ["urn:mod:get"]);
    let report = check_exposure(&kernel.topology(), &exposes);
    let flagged: Vec<_> = report
        .flagged
        .iter()
        .map(|d| d.door.pattern.as_str())
        .collect();
    assert_eq!(flagged, ["urn:mod:cache:{key}"]);
    assert_eq!(report.flagged[0].level, LEVEL);
    assert_eq!(report.flagged[0].prefix, "urn:mod:");
    assert_eq!(report.flagged[0].door.endpoint, "cache");
    assert!(report.undecided.is_empty(), "{report:?}");

    // Declared public, the helper is no longer reported.
    let both = exposes.level(LEVEL, ["urn:mod:cache:{key}"]);
    assert!(check_exposure(&kernel.topology(), &both).is_clean());
    // With nothing declared, every door an admitted name reaches is flagged.
    let none = check_exposure(&kernel.topology(), &Exposes::new());
    assert_eq!(none.flagged.len(), 2);
}

/// A grammar written outside core: the check cannot read it.
struct Tagged;
impl Grammar for Tagged {
    fn match_iri(&self, iri: &Iri) -> Option<Bindings> {
        iri.as_str().starts_with("urn:mod:tag:").then(Bindings::new)
    }
    fn pattern(&self) -> String {
        "urn:mod:tag:*".into()
    }
}

#[test]
fn an_opaque_grammar_is_reported_undecided_by_name_never_clean() {
    let space: Arc<dyn Space> = Arc::new(Mount::new(
        "urn:mod:",
        Arc::new(Level::new(
            iri(LEVEL),
            Arc::new(EndpointSpace::new().bind(Tagged, says("tagged"))),
        )),
    ));
    let report = check_exposure(&space.topology(), &Exposes::new());
    assert!(report.flagged.is_empty(), "{report:?}");
    assert_eq!(report.undecided.len(), 1);
    assert_eq!(report.undecided[0].door.kind, MatchKind::Custom);
    assert_eq!(report.undecided[0].door.pattern, "urn:mod:tag:*");
    assert_eq!(report.undecided[0].door.endpoint, "tagged");
    assert!(!report.is_clean());
}

#[test]
fn a_door_outside_any_level_and_one_under_an_unrelated_inner_mount_are_not_flagged() {
    // Outside a level there is no module, so nothing to compare; under an inner mount
    // whose prefix diverges from the guard's, no admitted name reaches the door.
    let space: Arc<dyn Space> = Arc::new(ikigai_core::Fallback::new(vec![
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:mod:root"), says("root"))),
        Arc::new(Mount::new(
            "urn:mod:",
            Arc::new(Level::new(
                iri(LEVEL),
                Arc::new(Mount::new(
                    "urn:other:",
                    Arc::new(EndpointSpace::new().bind(Exact::new("urn:mod:x"), says("x"))),
                )),
            )),
        )),
    ]));
    assert!(check_exposure(&space.topology(), &Exposes::new()).is_clean());
}
