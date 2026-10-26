//! The cache API as 0.1.112 published it (ledger #2261): `ReprCache::get` answers a
//! `Representation` (a buffer), `ReprCache::store` takes a `Representation`, and
//! `CacheEntry` is `Clone` with a `representation: Representation` field. Streamed bodies
//! (ledger #2237) added a body face BESIDE these (`get_body`, `store_body`), so that the
//! release carrying them can be a 0.1.x minor.
//!
//! Every use below is written as a 0.1.112 consumer would write it, with no annotation
//! that 0.1.112 did not need: if a signature here changes, this file stops compiling.

use std::sync::Arc;

use ikigai_core::{
    Body, CacheEntry, CacheKey, Expiry, Iri, Lru, ReprCache, ReprType, Representation, Request,
    Thread, Time, Verb,
};

fn key(n: u8) -> CacheKey {
    let target = Iri::parse(format!("urn:key:{n}")).unwrap();
    CacheKey::new(Request::new(Verb::Source, target).id(), 0)
}

fn answer(text: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), text).cacheable()
}

#[test]
fn get_answers_a_buffer_and_store_takes_one() {
    let cache = ReprCache::new(Arc::new(Lru::default()));
    let taken = cache.snapshot();
    // `store` takes a `Representation` by value, and the result of an inferred `.into()`
    // or of a function returning `Representation` both still fit.
    assert!(cache.store(key(1), "urn:x".to_string(), answer("hi"), taken, None));
    assert!(cache.store(key(2), "urn:y".into(), answer("yo").clone(), taken, Some(3)));

    // `get` answers `Option<Representation>`: a buffer, field `bytes: Vec<u8>`.
    let hit: Option<Representation> = cache.get(&key(1), None);
    let hit = hit.expect("stored");
    let bytes: Vec<u8> = hit.bytes;
    assert_eq!(bytes, b"hi");
    // A function path over `get` keeps its 0.1.112 type.
    let getter: fn(&ReprCache, &CacheKey, Option<Time>) -> Option<Representation> = ReprCache::get;
    assert_eq!(getter(&cache, &key(2), None).unwrap().bytes, b"yo");
    let storer: fn(
        &ReprCache,
        CacheKey,
        String,
        Representation,
        ikigai_core::CutSnapshot,
        Option<u64>,
    ) -> bool = ReprCache::store;
    assert!(storer(
        &cache,
        key(3),
        "urn:z".into(),
        answer("z"),
        cache.snapshot(),
        None
    ));

    // A cut still drops it.
    cache.cut(Thread::new("urn:never-depended-on"));
    assert!(cache.get(&key(1), None).is_some());
}

#[test]
fn cache_entry_is_cloneable_and_holds_a_buffer() {
    let entry = CacheEntry {
        target: "urn:x".into(),
        representation: Representation::new(ReprType::new("text/plain"), b"hi".to_vec())
            .with_expiry(Expiry::At(Time::from_millis(5))),
        edges: vec![(Thread::new("urn:x"), 3)],
        cost_millis: Some(7),
        hits: 2,
        stored_tick: 1,
        last_used_tick: 4,
    };
    let copy: CacheEntry = entry.clone();
    let buffer: &Vec<u8> = &copy.representation.bytes;
    assert_eq!(buffer, b"hi");
    assert_eq!(copy.bytes(), 2);
    let repr: Representation = entry.representation;
    assert_eq!(repr.content_id(), copy.representation.content_id());
}

#[test]
fn the_body_face_sits_beside_the_buffer_face() {
    let cache = ReprCache::new(Arc::new(Lru::default()));
    let taken = cache.snapshot();
    let streamed =
        Representation::from_body(ReprType::new("text/plain"), Body::from(&b"ab"[..])).cacheable();
    assert!(cache.store_body(key(1), "urn:x".into(), streamed, taken, None));
    // A buffer stored through the body face reads back through either face.
    assert_eq!(cache.get(&key(1), None).unwrap().bytes, b"ab");
    let body: Representation<Body> = cache.get_body(&key(1), None).unwrap();
    assert_eq!(body.bytes.as_bytes().unwrap(), &b"ab"[..]);
    // A one-shot body is never stored.
    let once = Representation::from_body(
        ReprType::new("text/plain"),
        Body::once(futures::stream::iter([Ok(
            ikigai_core::Bytes::from_static(b"x"),
        )])),
    )
    .cacheable();
    assert!(!cache.store_body(key(2), "urn:y".into(), once, cache.snapshot(), None));
}

#[test]
fn a_replay_entry_is_not_a_buffer_to_get() {
    struct Fixed;
    impl ikigai_core::Replay for Fixed {
        fn open(&self) -> ikigai_core::ByteStream {
            Box::pin(futures::stream::iter([Ok(
                ikigai_core::Bytes::from_static(b"r"),
            )]))
        }
        fn len_hint(&self) -> Option<u64> {
            Some(1)
        }
    }
    let cache = ReprCache::new(Arc::new(Lru::default()));
    let replay =
        Representation::from_body(ReprType::new("text/plain"), Body::replay(Fixed)).cacheable();
    assert!(cache.store_body(key(1), "urn:x".into(), replay, cache.snapshot(), None));
    // The buffer face does not read a replay behind the caller's back: it is a miss there,
    // and still a hit on the body face.
    assert!(cache.get(&key(1), None).is_none());
    assert!(cache.get_body(&key(1), None).is_some());
}
