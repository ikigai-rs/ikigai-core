//! `Representation<B = Vec<u8>>` (ledger #2237): the default body parameter changes
//! nothing for the bytes case. These are the spike's checks, kept as regression tests:
//! today's constructors infer at real call sites with no annotation, and a
//! `Representation<Vec<u8>>` serializes byte-for-byte as it did before the parameter
//! existed (the fixtures below were captured from 0.1.112, before the change).

use ikigai_core::{Expiry, ReprType, Representation, Time};

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
