//! Ledger #948: an action match has its own identity, and contracts are content-addressed.
//!
//! The manifold (`urn:kernel:actions`) used to name each match after its endpoint's
//! DESCRIPTION ID (`urn:ikigai:endpoint:{id}:action:{verb}`), and an id is not unique in
//! a kernel: a mount surfaces a peer's endpoint under the peer's id, and one endpoint can
//! sit at two doors. Every copy then wrote its triples onto ONE subject, so a parser that
//! keyed by subject kept one door and lost the rest, and a copy with a different contract
//! merged into the local one's node.
//!
//! These are the three cases of the reproduction (ikigai-devtools
//! `claude/research/repro-948-manifold-collisions.diff`, written against ikigai-cli),
//! ported to core. A mount is modeled as a second space whose door carries the same
//! description id under another name, which is what a mounted peer's entry is to the
//! manifold walk: an entry whose `describe()` answers the peer's description. The
//! catalog half runs through this crate's real `TurtleRenderer`, because the merge it
//! shows is in the rendered graph.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, ArgSpec, Capability, Description, EndpointSpace, Exact, Fallback, FnEndpoint, Iri,
    Kernel, ReprType, Representation, Request, Space, Verb,
};
use ikigai_vocab::TurtleRenderer;
use oxrdf::{NamedOrBlankNode, Term, Triple};

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const IK: &str = "https://ikigai-rs.dev/ns#";

fn upper(description: Description) -> FnEndpoint {
    FnEndpoint::new("toUpper", |inv| {
        let s = inv.inline_str("in").unwrap_or("").to_uppercase();
        Ok(Representation::new(
            ReprType::new("text/plain"),
            s.into_bytes(),
        ))
    })
    .with_description(description)
}

/// The local `toUpper`'s contract.
fn local_contract() -> Description {
    Description::new("toUpper")
        .verb(Verb::Source)
        .input(ArgSpec::new("in").class(XSD_STRING))
}

/// The peer's `toUpper`: same id, a DIFFERENT contract (one more capability, one more
/// optional input).
fn peer_contract() -> Description {
    Description::new("toUpper")
        .verb(Verb::Source)
        .requires("urn:cap:peer:only")
        .input(ArgSpec::new("in").class(XSD_STRING))
        .input(ArgSpec::new("locale").optional().class(XSD_STRING))
}

/// The local space plus a "mounted" one: the peer's copy surfaces at `urn:narrow:…`.
fn mounted(peer: Description) -> Kernel {
    let local: Arc<dyn Space> = Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:iki:fn:toUpper"), upper(local_contract())),
    );
    let remote: Arc<dyn Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:narrow:iki:fn:toUpper"), upper(peer)));
    Kernel::with_meta_renderer(
        Arc::new(Fallback::new(vec![local, remote])),
        Arc::new(TurtleRenderer),
    )
}

fn source(kernel: &Kernel, target: &str, args: &[(&str, &str)]) -> String {
    let mut request = Request::new(Verb::Source, Iri::parse(target).unwrap());
    for (name, value) in args {
        request = request.with_arg(*name, ArgRef::Inline(value.as_bytes().to_vec()));
    }
    String::from_utf8(
        block_on(kernel.issue(request, &Capability::root()))
            .unwrap()
            .bytes,
    )
    .unwrap()
}

fn triples(turtle: &str) -> Vec<Triple> {
    oxttl::TurtleParser::new()
        .for_slice(turtle.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|e| panic!("{e}\n{turtle}"))
}

fn subject(t: &Triple) -> String {
    match &t.subject {
        NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
        other => panic!("a blank node in a kernel face: {other}"),
    }
}

fn object(t: &Triple) -> String {
    match &t.object {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        other => panic!("unexpected object {other}"),
    }
}

/// Every `ik:ActionMatch` node in the Turtle manifold, as subject → predicate → objects.
fn matches(turtle: &str) -> BTreeMap<String, BTreeMap<String, BTreeSet<String>>> {
    let all = triples(turtle);
    let typed: BTreeSet<String> = all
        .iter()
        .filter(|t| t.predicate.as_str() == "http://www.w3.org/1999/02/22-rdf-syntax-ns#type")
        .filter(|t| object(t) == format!("{IK}ActionMatch"))
        .map(subject)
        .collect();
    let mut out: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    for t in all.iter().filter(|t| typed.contains(&subject(t))) {
        let predicate = t
            .predicate
            .as_str()
            .strip_prefix(IK)
            .unwrap_or(t.predicate.as_str())
            .to_string();
        out.entry(subject(t))
            .or_default()
            .entry(predicate)
            .or_default()
            .insert(object(t));
    }
    out
}

/// The `toUpper` matches, keyed by the door each one names.
fn by_door(
    manifold: &BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
) -> BTreeMap<String, (String, BTreeMap<String, BTreeSet<String>>)> {
    let mut out = BTreeMap::new();
    for (subject, predicates) in manifold {
        let doors = predicates.get("endpoint").cloned().unwrap_or_default();
        assert_eq!(
            doors.len(),
            1,
            "{subject} names {} doors, so a consumer keyed by subject keeps one: {predicates:?}",
            doors.len()
        );
        let door = doors.into_iter().next().unwrap();
        if door.contains("toUpper") || door == "urn:shout" {
            out.insert(door, (subject.clone(), predicates.clone()));
        }
    }
    out
}

fn contract_of(predicates: &BTreeMap<String, BTreeSet<String>>) -> String {
    let contracts = predicates.get("contract").cloned().unwrap_or_default();
    assert_eq!(
        contracts.len(),
        1,
        "one ik:contract per match: {predicates:?}"
    );
    contracts.into_iter().next().unwrap()
}

/// Case 1: a mount (a second door) carrying the SAME contract. Before #948 the local
/// copy disappeared behind the peer's: one subject, two `ik:endpoint` triples.
#[test]
fn a_mounted_copy_with_the_same_contract_does_not_hide_the_local_one() {
    let kernel = mounted(local_contract());
    let turtle = source(&kernel, "urn:kernel:actions", &[("as", "text/turtle")]);
    let doors = by_door(&matches(&turtle));
    assert_eq!(
        doors.keys().collect::<Vec<_>>(),
        ["urn:iki:fn:toUpper", "urn:narrow:iki:fn:toUpper"],
        "{turtle}"
    );
    let (local_subject, local) = &doors["urn:iki:fn:toUpper"];
    let (peer_subject, peer) = &doors["urn:narrow:iki:fn:toUpper"];
    assert_ne!(local_subject, peer_subject, "{turtle}");
    // Identical contracts are ONE node: content addressing merges exactly these.
    assert_eq!(contract_of(local), contract_of(peer), "{turtle}");

    // The plain face lists both doors, and the Rust API reports both.
    let plain = source(&kernel, "urn:kernel:actions", &[]);
    assert!(plain.lines().any(|l| l == "urn:iki:fn:toUpper"), "{plain}");
    assert!(
        plain.lines().any(|l| l == "urn:narrow:iki:fn:toUpper"),
        "{plain}"
    );
}

/// Case 2: one endpoint bound at two doors collapsed the same way.
#[test]
fn one_endpoint_at_two_doors_is_two_matches_over_one_contract() {
    let kernel = Kernel::with_meta_renderer(
        Arc::new(
            EndpointSpace::new()
                .bind(Exact::new("urn:iki:fn:toUpper"), upper(local_contract()))
                .bind(Exact::new("urn:shout"), upper(local_contract())),
        ),
        Arc::new(TurtleRenderer),
    );
    let turtle = source(&kernel, "urn:kernel:actions", &[("as", "text/turtle")]);
    let doors = by_door(&matches(&turtle));
    assert_eq!(
        doors.keys().collect::<Vec<_>>(),
        ["urn:iki:fn:toUpper", "urn:shout"],
        "{turtle}"
    );
    assert_eq!(
        contract_of(&doors["urn:iki:fn:toUpper"].1),
        contract_of(&doors["urn:shout"].1),
        "{turtle}"
    );
    // The match subjects themselves are distinct, and each is a match IRI.
    assert!(doors
        .values()
        .all(|(s, _)| s.starts_with("urn:ikigai:match:")));
    assert_ne!(doors["urn:iki:fn:toUpper"].0, doors["urn:shout"].0);
}

/// Case 3, the deciding one: a mounted copy with a DIFFERENT contract merged into the
/// local node, so the local `toUpper` appeared to need the peer's capability. The
/// catalog merged the same way.
#[test]
fn a_mounted_copy_with_a_different_contract_does_not_merge() {
    let kernel = mounted(peer_contract());
    let turtle = source(&kernel, "urn:kernel:actions", &[("as", "text/turtle")]);
    let doors = by_door(&matches(&turtle));
    let (_, local) = &doors["urn:iki:fn:toUpper"];
    let (_, peer) = &doors["urn:narrow:iki:fn:toUpper"];
    assert!(
        !local
            .get("requires")
            .is_some_and(|r| r.contains("urn:cap:peer:only")),
        "the local toUpper does not need the peer's capability: {turtle}"
    );
    assert!(
        peer.get("requires")
            .is_some_and(|r| r.contains("urn:cap:peer:only")),
        "{turtle}"
    );
    let local_contract = contract_of(local);
    let peer_contract = contract_of(peer);
    assert_ne!(local_contract, peer_contract, "{turtle}");

    // The catalog: each contract node carries exactly its own copy's triples.
    let catalog = source(&kernel, "urn:kernel:catalog", &[]);
    let all = triples(&catalog);
    let on = |node: &str, predicate: &str| -> BTreeSet<String> {
        all.iter()
            .filter(|t| subject(t) == node && t.predicate.as_str() == format!("{IK}{predicate}"))
            .map(object)
            .collect()
    };
    assert!(on(&local_contract, "requires").is_empty(), "{catalog}");
    assert_eq!(
        on(&peer_contract, "requires"),
        BTreeSet::from(["urn:cap:peer:only".to_string()]),
        "{catalog}"
    );
    let input_names = |node: &str| -> BTreeSet<String> {
        on(node, "input")
            .iter()
            .flat_map(|input| on(input, "inputName"))
            .collect()
    };
    assert_eq!(
        input_names(&local_contract),
        BTreeSet::from(["in".to_string()]),
        "{catalog}"
    );
    assert_eq!(
        input_names(&peer_contract),
        BTreeSet::from(["in".to_string(), "locale".to_string()]),
        "{catalog}"
    );
    // The endpoint node (the id's node, shared by every copy) points at both contracts.
    assert_eq!(
        on("urn:ikigai:endpoint:toUpper", "action"),
        BTreeSet::from([local_contract, peer_contract]),
        "{catalog}"
    );
}

/// Validate accepts what the manifold hands out: a match IRI pre-flights against THAT
/// door's contract, so the peer's extra input is unknown at the local door and allowed
/// at the peer's.
#[test]
fn validate_accepts_the_match_iri_and_checks_that_doors_contract() {
    let kernel = mounted(peer_contract());
    let turtle = source(&kernel, "urn:kernel:actions", &[("as", "text/turtle")]);
    let doors = by_door(&matches(&turtle));
    let report = |action: &str| {
        source(
            &kernel,
            "urn:kernel:validate",
            &[("action", action), ("args", "in=x\nlocale=tr")],
        )
    };
    let local = report(&doors["urn:iki:fn:toUpper"].0);
    assert!(local.contains("sh:conforms false"), "{local}");
    assert!(local.contains("locale"), "{local}");
    let peer = report(&doors["urn:narrow:iki:fn:toUpper"].0);
    assert!(peer.contains("sh:conforms true"), "{peer}");

    // The contract IRI is accepted too, and means that contract.
    let by_contract = report(&contract_of(&doors["urn:narrow:iki:fn:toUpper"].1));
    assert!(by_contract.contains("sh:conforms true"), "{by_contract}");
}

/// The pre-0.1.91 spelling still pre-flights while every copy of the id agrees on the
/// contract, and is refused as ambiguous, naming the match IRIs, when they do not.
#[test]
fn validate_keeps_the_old_action_iri_until_it_is_ambiguous() {
    let legacy = "urn:ikigai:endpoint:toUpper:action:source";
    let same = mounted(local_contract());
    let report = source(
        &same,
        "urn:kernel:validate",
        &[("action", legacy), ("args", "in=x")],
    );
    assert!(report.contains("sh:conforms true"), "{report}");

    let different = mounted(peer_contract());
    let request = Request::new(Verb::Source, Iri::parse("urn:kernel:validate").unwrap())
        .with_arg("action", ArgRef::Inline(legacy.as_bytes().to_vec()))
        .with_arg("args", ArgRef::Inline(b"in=x".to_vec()));
    let refused = block_on(different.issue(request, &Capability::root()))
        .expect_err("two contracts behind one id")
        .to_string();
    assert!(refused.contains("ambiguous"), "{refused}");
    assert!(
        refused.contains("urn:ikigai:match:source:urn:iki:fn:toUpper")
            && refused.contains("urn:ikigai:match:source:urn:narrow:iki:fn:toUpper"),
        "{refused}"
    );
}
