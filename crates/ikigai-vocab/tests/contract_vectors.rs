//! The shared contract vectors (ledger #1096): what core computes for match IRIs,
//! contract IRIs, their parse, and a door's Turtle, written to
//! `tests/vectors/contract_vectors.json` for the other faces to pin themselves against.
//!
//! ikigai-python and ikigai-deno mirror `match_iri`, `ActionSpec::contract_iri` and
//! `to_turtle` by hand. Each used to generate its own vectors from a released core; this
//! file is the one copy they consume instead (see `tests/vectors/README.md`), and the
//! test below is why it cannot drift: it recomputes every output from the inputs in
//! `tests/vectors/contract_cases.json` and fails if the committed file differs by a byte.
//!
//! To add a case, edit `contract_cases.json`, then regenerate with
//!
//! ```text
//! cargo test -p ikigai-vocab --test contract_vectors -- --ignored
//! ```
//!
//! and commit both files.

use ikigai_core::{
    canonical_contract_iri, canonical_match_iri, match_iri, parse_contract_iri, parse_match_iri,
    ActionSpec, Description, Verb,
};
use oxrdf::{NamedOrBlankNode, Term, Triple};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

const CASES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/contract_cases.json"
);
const VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/vectors/contract_vectors.json"
);

const VERBS: [Verb; 5] = [
    Verb::Source,
    Verb::Sink,
    Verb::Exists,
    Verb::Delete,
    Verb::Meta,
];

const ABOUT: &str = "Contract vectors computed by ikigai-core and ikigai-vocab from \
     tests/vectors/contract_cases.json (crates/ikigai-vocab/tests/contract_vectors.rs, \
     which fails if this file differs from what it computes). Verbs are spelled as \
     ikigai_core::Verb serializes them; a parse that core refuses is null. A contract \
     using no construct of the ArgSpec release is digested under `form`; one using any \
     (groups, needs, lists, an empty enumeration, value notes, examples, an open or \
     envelope argument set, output_from) under `form_v2`.";

fn verb_name(verb: Verb) -> String {
    format!("{verb:?}")
}

fn str_field<'a>(case: &'a Value, field: &str) -> &'a str {
    case[field]
        .as_str()
        .unwrap_or_else(|| panic!("case without a string `{field}`: {case}"))
}

fn list<'a>(cases: &'a Value, field: &str) -> &'a Vec<Value> {
    cases[field]
        .as_array()
        .unwrap_or_else(|| panic!("contract_cases.json has no `{field}` array"))
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

/// What core computes for every case: the bytes `contract_vectors.json` must hold.
fn compute() -> String {
    let text = std::fs::read_to_string(CASES).expect("read contract_cases.json");
    let cases: Value = serde_json::from_str(&text).expect("contract_cases.json is JSON");

    let doors: Vec<Value> = list(&cases, "doors")
        .iter()
        .map(|case| {
            let pattern = str_field(case, "pattern");
            let d: Description = serde_json::from_value(case["description"].clone())
                .unwrap_or_else(|e| panic!("door {}: {e}", case["name"]));
            let actions: Vec<Value> = d
                .action_specs()
                .iter()
                .map(|a| {
                    json!({
                        "verb": verb_name(a.verb),
                        "match": match_iri(a.verb, pattern),
                        "contract": a.contract_iri(&d.id),
                    })
                })
                .collect();
            json!({
                "name": case["name"],
                "pattern": pattern,
                "description": case["description"],
                "actions": actions,
                "turtle": ikigai_vocab::to_turtle(&d),
            })
        })
        .collect();

    let contracts: Vec<Value> = list(&cases, "contracts")
        .iter()
        .map(|case| {
            let id = str_field(case, "id");
            let spec: ActionSpec = serde_json::from_value(case["action"].clone())
                .unwrap_or_else(|e| panic!("contract {}: {e}", case["name"]));
            json!({
                "name": case["name"],
                "id": id,
                "action": case["action"],
                "contract": spec.contract_iri(id),
            })
        })
        .collect();

    let matches: Vec<Value> = list(&cases, "patterns")
        .iter()
        .map(|pattern| {
            let pattern = pattern.as_str().expect("a pattern is a string");
            let iris: Vec<Value> = VERBS
                .iter()
                .map(|v| json!({"verb": verb_name(*v), "match": match_iri(*v, pattern)}))
                .collect();
            json!({"pattern": pattern, "iris": iris})
        })
        .collect();

    let parses: Vec<Value> = list(&cases, "parses")
        .iter()
        .map(|iri| {
            let iri = iri.as_str().expect("a parse case is a string");
            json!({
                "iri": iri,
                "match": parse_match_iri(iri).map(|(v, p)| json!([verb_name(v), p])),
                "contract": parse_contract_iri(iri)
                    .map(|(id, v, d)| json!([id, verb_name(v), d.to_string()])),
                "canonical": canonical_match_iri(iri).or_else(|| canonical_contract_iri(iri)),
            })
        })
        .collect();

    let doc = sorted(json!({
        "about": ABOUT,
        "form": "ikigai:contract:v1",
        "form_v2": "ikigai:contract:v2",
        "doors": doors,
        "contracts": contracts,
        "matches": matches,
        "parses": parses,
    }));
    let mut out = serde_json::to_string_pretty(&doc).expect("serialize the vectors");
    out.push('\n');
    out
}

#[test]
fn the_shared_contract_vectors_are_what_core_computes() {
    let committed = std::fs::read_to_string(VECTORS).unwrap_or_default();
    assert!(
        committed == compute(),
        "tests/vectors/contract_vectors.json is not what core computes from \
         contract_cases.json; regenerate it with \
         `cargo test -p ikigai-vocab --test contract_vectors -- --ignored` and commit it"
    );
}

/// The ledger #1096 refusal is pinned in the shared file, not only in core's unit
/// tests: a face that mirrors the old `u8::from_str_radix` leniency fails against it.
#[test]
fn the_shared_vectors_pin_the_sign_refusal() {
    let doc: Value = serde_json::from_str(&compute()).unwrap();
    let parses = doc["parses"].as_array().unwrap();
    let find = |iri: &str| {
        parses
            .iter()
            .find(|p| p["iri"] == iri)
            .unwrap_or_else(|| panic!("no parse case for {iri}"))
    };
    assert_eq!(find("urn:ikigai:match:source:%+1")["match"], Value::Null);
    assert_eq!(
        find("urn:ikigai:match:source:%01")["match"],
        json!(["Source", "\u{1}"])
    );
}

/// Lenient in, canonical out (ledger #1107), pinned in the shared file: every IRI that
/// parses carries a `canonical` spelling that parses to the same value and is its own
/// canonical spelling, and the lenient spellings each map to the one core writes.
#[test]
fn the_shared_vectors_pin_lenient_in_canonical_out() {
    let doc: Value = serde_json::from_str(&compute()).unwrap();
    for p in doc["parses"].as_array().unwrap() {
        let iri = p["iri"].as_str().unwrap();
        if p["match"].is_null() && p["contract"].is_null() {
            assert_eq!(p["canonical"], Value::Null, "{iri}");
            continue;
        }
        let canonical = p["canonical"]
            .as_str()
            .unwrap_or_else(|| panic!("{iri} parses but has no canonical spelling"));
        assert_eq!(
            canonical_match_iri(canonical).or_else(|| canonical_contract_iri(canonical)),
            Some(canonical.to_string()),
            "{iri}: the canonical spelling is a fixed point"
        );
        let reparsed = (
            parse_match_iri(canonical).map(|(v, p)| json!([verb_name(v), p])),
            parse_contract_iri(canonical)
                .map(|(id, v, d)| json!([id, verb_name(v), d.to_string()])),
        );
        assert_eq!(
            (
                reparsed.0.unwrap_or(Value::Null),
                reparsed.1.unwrap_or(Value::Null)
            ),
            (p["match"].clone(), p["contract"].clone()),
            "{iri}: the canonical spelling names the same door or contract"
        );
    }
    let canonical_of = |iri: &str| {
        doc["parses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["iri"] == iri)
            .unwrap_or_else(|| panic!("no parse case for {iri}"))["canonical"]
            .clone()
    };
    for lenient in [
        "urn:ikigai:match:source:urn:file:{path}",
        "urn:ikigai:match:source:urn:%66ile:%7bpath%7d",
    ] {
        assert_eq!(
            canonical_of(lenient),
            json!("urn:ikigai:match:source:urn:file:%7Bpath%7D"),
            "{lenient}"
        );
    }
    assert_eq!(
        canonical_of(
            "urn:ikigai:contract:{x}:source:b3:\
             ABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABAB"
        ),
        json!(
            "urn:ikigai:contract:%7Bx%7D:source:b3:\
             abababababababababababababababababababababababababababababababab"
        )
    );
}

/// A door's committed Turtle, parsed.
fn parse_door(doc: &Value, name: &str) -> Result<Vec<Triple>, String> {
    let door = doc["doors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == name)
        .unwrap_or_else(|| panic!("no door named {name}"));
    let ttl = door["turtle"]
        .as_str()
        .expect("a door's turtle is a string");
    oxttl::TurtleParser::new()
        .for_reader(ttl.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("door {name}: its Turtle does not parse: {e}\n{ttl}"))
}

fn door_triples(doc: &Value, name: &str) -> Vec<Triple> {
    parse_door(doc, name).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn every_door_s_turtle_parses() {
    let doc: Value = serde_json::from_str(&compute()).unwrap();
    for door in doc["doors"].as_array().unwrap() {
        let name = door["name"].as_str().unwrap();
        assert!(
            !door_triples(&doc, name).is_empty(),
            "door {name}: no triples"
        );
    }
}

/// Ledger #1345: an endpoint id or input name is a NAME, so a `%` in it is encoded as
/// `%25`, the way core spells the id in a contract IRI. Left alone, `deno/edge`'s id
/// `ev>il 100% naïve` became `<urn:ikigai:endpoint:ev%3Eil%20100%%20naïve>` and oxttl
/// refused the whole document. An authored IRI (a `class`, a scope) keeps its escapes:
/// a real one stays a resource, a malformed one degrades to a literal.
#[test]
fn a_percent_in_a_name_is_encoded_and_in_an_authored_iri_is_kept() {
    let doc: Value = serde_json::from_str(&compute()).unwrap();
    let subjects = |name: &str| -> BTreeSet<String> {
        door_triples(&doc, name)
            .iter()
            .filter_map(|t| match &t.subject {
                NamedOrBlankNode::NamedNode(n) => Some(n.as_str().to_string()),
                NamedOrBlankNode::BlankNode(_) => None,
            })
            .collect()
    };
    assert!(
        subjects("deno/edge").contains("urn:ikigai:endpoint:ev%3Eil%20100%25%20na\u{ef}ve"),
        "{:?}",
        subjects("deno/edge")
    );
    let percent = subjects("core/percent-names");
    for node in [
        "urn:ikigai:endpoint:50%25%20off",
        "urn:ikigai:endpoint:50%25%20off:input:100%25",
        "urn:ikigai:endpoint:50%25%20off:input:rate%2525",
    ] {
        assert!(percent.contains(node), "{node} not in {percent:?}");
    }
    // The action view's input nodes hang off the contract IRI, which encodes the id the
    // same way, and spell each input name as the endpoint view does.
    assert!(
        percent.iter().any(
            |s| s.starts_with("urn:ikigai:contract:50%25%20off:source:b3:")
                && s.ends_with(":input:rate%2525")
        ),
        "{percent:?}"
    );
    let triples = door_triples(&doc, "core/percent-names");
    assert_eq!(
        objects(&triples, "class"),
        BTreeSet::from([
            Kind::Iri("urn:cls:a%20b".into()),
            Kind::Literal("urn:cls:100%".into()),
        ])
    );
    assert_eq!(
        objects(&triples, "requires"),
        BTreeSet::from([
            Kind::Iri("urn:cap:a%20b".into()),
            Kind::Literal("urn:cap:100%".into()),
        ])
    );
}

/// What a term is, for comparing objects across the endpoint and action views.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Iri(String),
    Literal(String),
}

fn kind(term: &Term) -> Kind {
    match term {
        Term::NamedNode(n) => Kind::Iri(n.as_str().to_string()),
        Term::Literal(l) => Kind::Literal(l.value().to_string()),
        other => panic!("a blank node in a door's Turtle: {other}"),
    }
}

fn objects(triples: &[Triple], local: &str) -> BTreeSet<Kind> {
    let predicate = format!("{}{local}", ikigai_vocab::NS);
    triples
        .iter()
        .filter(|t| t.predicate.as_str() == predicate)
        .map(|t| kind(&t.object))
        .collect()
}

/// The hostile door (ledger #1113): a declared `class` and a `requires` scope are meant to
/// be IRIs but nothing validates them, and literals carry newline, CR and tab. Core's
/// Turtle percent-encodes the value when the result is a real IRI and falls back to a
/// LITERAL when it is not (whatever its prefix says), so the document always parses and
/// nothing the author wrote becomes a triple of its own. A face that emits `<{class}>` raw,
/// or picks `<…>` for a scope by its prefix alone, fails this door's bytes.
#[test]
fn the_hostile_door_encodes_or_falls_back_and_never_injects() {
    let doc: Value = serde_json::from_str(&compute()).unwrap();
    let triples = door_triples(&doc, "core/hostile-terms");

    let iri = |s: &str| Kind::Iri(s.to_string());
    let literal = |s: &str| Kind::Literal(s.to_string());
    assert_eq!(
        objects(&triples, "class"),
        BTreeSet::from([
            // `>` and a space, percent-encoded: still a urn, so still a resource …
            iri("urn:cls:a%3E%20b"),
            // … but encoding this one gives its authority a non-numeric "port", and a
            // class that is no IRI at all has no scheme: both degrade to literals.
            literal("http://ok> . <urn:z> a <urn:w"),
            literal("not a class"),
        ])
    );
    assert_eq!(
        objects(&triples, "requires"),
        BTreeSet::from([
            iri("urn:cap:core:read"),
            literal("not an iri"),
            iri("urn:cap:x%3E%20.%20%3Curn:everything"),
            // An `https://` prefix is not enough: it still falls back.
            literal("https://cap> . <urn:z"),
        ])
    );
    // The literals carry the author's text exactly: the escapes round-trip.
    assert_eq!(
        objects(&triples, "title"),
        BTreeSet::from([literal("T\tab\r\nend")])
    );
    assert!(
        objects(&triples, "summary").contains(&literal("one\ntwo\rthree\tfour \"q\" \\")),
        "the endpoint summary"
    );
    assert!(
        objects(&triples, "summary").contains(&literal("line one\r\nline\ttwo")),
        "the input summary"
    );
    assert_eq!(
        objects(&triples, "default"),
        BTreeSet::from([literal("x\ry")])
    );
    assert_eq!(
        objects(&triples, "oneOf"),
        BTreeSet::from([literal("a\nb"), literal("c\td")])
    );
    // Nothing the author wrote became a node: every subject is this door's endpoint or one
    // of its contracts, and no object is one of the names smuggled into a value.
    for t in &triples {
        let s = match &t.subject {
            NamedOrBlankNode::NamedNode(n) => n.as_str(),
            other => panic!("a blank subject: {other}"),
        };
        assert!(
            s.starts_with("urn:ikigai:endpoint:hostile-terms")
                || s.starts_with("urn:ikigai:contract:hostile-terms:"),
            "injected subject {s}"
        );
        assert!(
            !matches!(
                kind(&t.object),
                Kind::Iri(o) if ["urn:z", "urn:w", "urn:everything"].contains(&o.as_str())
            ),
            "injected object {}",
            t.object
        );
    }
}

/// Ledger #1547: the rows that predate the ArgSpec release keep their v1 digests (the
/// file's v1 rows are the proof), the v2 constructs move a digest, and set semantics hold
/// under v2 too.
#[test]
fn the_dual_form_keeps_v1_digests_and_digests_the_new_constructs_under_v2() {
    let doc: Value = serde_json::from_str(&compute()).unwrap();
    let contract = |name: &str| -> String {
        doc["contracts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no contract case {name}"))["contract"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        contract("v2/group"),
        contract("v2/group-members-are-a-set (same digest as v2/group)")
    );
    // The same contract with its group taken away is a v1 contract with another IRI.
    let mut bare: Value = doc["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "v2/group")
        .unwrap()["action"]
        .clone();
    bare.as_object_mut().unwrap().remove("groups");
    let bare: ActionSpec = serde_json::from_value(bare).unwrap();
    assert_ne!(bare.contract_iri("kernel-cut"), contract("v2/group"));
    let door = doc["doors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "core/argspec-shape")
        .expect("the argspec-shape door");
    let ttl = door["turtle"].as_str().unwrap();
    for term in [
        "ik:group",
        "ik:pipe",
        "ik:needs",
        "ik:listSeparator",
        "ik:enumerated true",
        "ik:valueNote",
        "ik:example",
        "ik:envelope",
        "ik:openArguments",
        "ik:outputFrom ik:Origin",
    ] {
        assert!(ttl.contains(term), "{term} in {ttl}");
    }
}

/// Regenerate `contract_vectors.json`./// Regenerate `contract_vectors.json`. Ignored so a normal run only CHECKS; run it with
/// `-- --ignored` after editing the cases.
#[test]
#[ignore = "writes tests/vectors/contract_vectors.json; run with -- --ignored to regenerate"]
fn regenerate_the_shared_contract_vectors() {
    std::fs::write(VECTORS, compute()).expect("write contract_vectors.json");
}
