//! `Representation<B = Vec<u8>>` (ledger #2237): the default body parameter changes
//! nothing for the bytes case. These are the spike's checks, kept as regression tests:
//! today's constructors infer at real call sites with no annotation, and a
//! `Representation<Vec<u8>>` serializes byte-for-byte as it did before the parameter
//! existed (the fixtures below were captured from 0.1.112, before the change). So does a
//! `CacheEntry`, which keeps its 0.1.112 shape and derive (ledger #2261).

use ikigai_core::{CacheEntry, Expiry, ReprType, Representation, Thread, Time};

/// The fixture value: a parameter, a non-default expiry, and a thread (which is
/// `serde(skip)`, so it must not appear in either encoding).
fn fixture() -> Representation {
    Representation::new(
        ReprType::new("text/plain").with_param("charset", "utf-8"),
        b"hi\x00\xff".to_vec(),
    )
    .with_expiry(Expiry::At(Time::from_millis(1_700_000_000_000)))
    .depends_on("urn:file:x")
}

/// Captured from ikigai-core 0.1.112 (origin/main e1d21a9) with the same `fixture()`.
const POSTCARD_0_1_112: &[u8] = &[
    10, 116, 101, 120, 116, 47, 112, 108, 97, 105, 110, 1, 7, 99, 104, 97, 114, 115, 101, 116, 5,
    117, 116, 102, 45, 56, 4, 104, 105, 0, 255, 1, 128, 208, 149, 255, 188, 49,
];

/// Captured from ikigai-core 0.1.112 (origin/main e1d21a9) with the same `fixture()`.
const JSON_0_1_112: &str = r#"{"repr_type":{"media_type":"text/plain","params":{"charset":"utf-8"}},"bytes":[104,105,0,255],"expiry":{"At":1700000000000}}"#;

#[test]
fn postcard_encoding_is_unchanged() {
    let bytes = postcard::to_stdvec(&fixture()).unwrap();
    assert_eq!(bytes, POSTCARD_0_1_112, "{bytes:?}");
    // And it reads back, with the default parameter driving the target type.
    let back: Representation = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(back, fixture());
}

#[test]
fn json_encoding_is_unchanged() {
    let json = serde_json::to_string(&fixture()).unwrap();
    assert_eq!(json, JSON_0_1_112);
    let back: Representation = serde_json::from_str(&json).unwrap();
    assert_eq!(back, fixture());
}

/// Captured from ikigai-core 0.1.112 (origin/main e1d21a9), where `CacheEntry` derived serde,
/// with the same `entry()`.
const ENTRY_POSTCARD_0_1_112: &[u8] = &[
    5, 117, 114, 110, 58, 120, 10, 116, 101, 120, 116, 47, 112, 108, 97, 105, 110, 0, 2, 104, 105,
    1, 5, 1, 5, 117, 114, 110, 58, 120, 3, 1, 7, 2, 1, 4,
];

/// Captured from ikigai-core 0.1.112 (origin/main e1d21a9), with the same `entry()`.
const ENTRY_JSON_0_1_112: &str = r#"{"target":"urn:x","representation":{"repr_type":{"media_type":"text/plain","params":{}},"bytes":[104,105],"expiry":{"At":5}},"edges":[["urn:x",3]],"cost_millis":7,"hits":2,"stored_tick":1,"last_used_tick":4}"#;

fn entry() -> CacheEntry {
    CacheEntry {
        target: "urn:x".into(),
        representation: Representation::new(ReprType::new("text/plain"), b"hi".to_vec())
            .with_expiry(Expiry::At(Time::from_millis(5))),
        edges: vec![(Thread::new("urn:x"), 3)],
        cost_millis: Some(7),
        hits: 2,
        stored_tick: 1,
        last_used_tick: 4,
    }
}

#[test]
fn a_cache_entry_encodes_as_it_did_in_0_1_112() {
    let entry = entry();
    assert_eq!(postcard::to_stdvec(&entry).unwrap(), ENTRY_POSTCARD_0_1_112);
    assert_eq!(serde_json::to_string(&entry).unwrap(), ENTRY_JSON_0_1_112);
    let back: CacheEntry = postcard::from_bytes(ENTRY_POSTCARD_0_1_112).unwrap();
    assert_eq!(back.representation, entry.representation);
    assert_eq!(back.edges, entry.edges);
}

/// The call shapes the spike checked (ledger #2237): none needs an annotation. The one
/// that does is a bare function path with nothing to infer from,
/// `let f = Representation::cacheable;` (E0282), documented on `Representation`.
#[test]
fn todays_call_shapes_infer_with_no_annotation() {
    let a = Representation::new(ReprType::new("text/plain"), "x");
    // A function path fed by a typed input.
    let cached: Vec<Representation> = vec![a.clone()]
        .into_iter()
        .map(Representation::cacheable)
        .collect();
    assert_eq!(cached[0].expiry, Expiry::Never);
    let ok: Result<Representation, ()> = Ok(a.clone());
    assert_eq!(
        ok.map(Representation::cacheable).map(|r| r.bytes.len()),
        Ok(1)
    );
    // The shared builders on a value.
    let threaded = a.clone().with_expiry(Expiry::Never).depends_on("urn:t");
    assert_eq!(threaded.threads().len(), 1);
    // A turbofish in type position takes the default.
    let bytes = postcard::to_stdvec(&a).unwrap();
    let back = postcard::from_bytes::<Representation>(&bytes).unwrap();
    assert_eq!(back.content_id(), a.content_id());
    // A closure over an annotated binding, and a constructor as a function value.
    let take = |r: Representation| r.bytes;
    assert_eq!(take(a.clone()), b"x");
    let make: fn(ReprType, Vec<u8>) -> Representation = Representation::new;
    assert_eq!(make(ReprType::new("text/plain"), b"x".to_vec()), a);
}
