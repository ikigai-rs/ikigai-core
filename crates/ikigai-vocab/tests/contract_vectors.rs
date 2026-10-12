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
     ikigai_core::Verb serializes them; a parse that core refuses is null.";

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

/// Doors whose committed Turtle a strict parser refuses, each with the error it gives.
///
/// `deno/edge`'s id is `ev>il 100% naïve`. `escape_iri_fragment` leaves `%` alone by
/// design (an authored IRI may carry real escapes), and `to_turtle` uses it for the
/// endpoint subject and the flat input nodes, so the subject is
/// `<urn:ikigai:endpoint:ev%3Eil%20100%%20naïve>`: `%%2` is not a percent escape and oxttl
/// refuses the whole document. The contract IRIs on the same door are fine (`contract_iri`
/// writes `%` as `%25`). That is a core emission defect (an id or input name with a bare
/// `%`), reported for its own arc rather than fixed here; when it is fixed this door's
/// bytes change and this list empties, which the test below insists on.
const KNOWN_UNPARSEABLE: [(&str, &str); 1] = [("deno/edge", "Invalid IRI percent encoding")];

#[test]
fn every_door_s_turtle_parses() {
    let doc: Value = serde_json::from_str(&compute()).unwrap();
    for door in doc["doors"].as_array().unwrap() {
        let name = door["name"].as_str().unwrap();
        let known = KNOWN_UNPARSEABLE.iter().find(|(n, _)| *n == name);
        match (parse_door(&doc, name), known) {
            (Ok(triples), None) => assert!(!triples.is_empty(), "door {name}: no triples"),
            (Err(e), None) => panic!("{e}"),
            (Err(e), Some((_, why))) => assert!(e.contains(why), "{e}"),
            (Ok(_), Some(_)) => {
                panic!("door {name} parses now: take it out of KNOWN_UNPARSEABLE (and its note)")
            }
        }
    }
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

/// Regenerate `contract_vectors.json`. Ignored so a normal run only CHECKS; run it with
/// `-- --ignored` after editing the cases.
#[test]
#[ignore = "writes tests/vectors/contract_vectors.json; run with -- --ignored to regenerate"]
fn regenerate_the_shared_contract_vectors() {
    std::fs::write(VECTORS, compute()).expect("write contract_vectors.json");
}
