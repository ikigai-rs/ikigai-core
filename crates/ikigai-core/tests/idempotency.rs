//! The **idempotency-key toolkit** for `Sink` endpoints (ledger #4056, from the research note
//! `docs/research/sink-idempotency.md`, section 5.5): the token's parser, the fingerprint
//! without the token, the `Replay` record's stable encoding, and the published window. Core
//! stores nothing; the last test is an endpoint that adopts the token, so a lost answer's
//! retry writes one copy and answers the first answer's bytes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::executor::block_on;
use ikigai_core::idempotency::{
    IdempotencyKey, IdempotencyKeyError, Replay, ReplayOutcome, ReplayScope, IDEMPOTENCY_KEY_ARG,
    MAX_LEN, MIN_WINDOW,
};
use ikigai_core::{
    ActionSpec, ArgRef, ArgSpec, Capability, ContentId, Description, EndpointSpace, Error, Exact,
    FailureClass, FnEndpoint, Iri, Kernel, ReprType, Representation, Request, Time, Verb,
};

fn inline(s: &str) -> ArgRef {
    ArgRef::Inline(s.as_bytes().to_vec())
}

fn target() -> Iri {
    Iri::parse("urn:ledger:main").unwrap()
}

#[test]
fn the_key_takes_the_ledger_alphabet_and_at_most_256_characters() {
    for ok in ["a", "A-z.0_9~:", "urn:retry:7f3a", &"k".repeat(MAX_LEN)] {
        assert_eq!(IdempotencyKey::parse(ok).unwrap().as_str(), ok);
    }
    assert_eq!(IdempotencyKey::parse(""), Err(IdempotencyKeyError::Empty));
    assert_eq!(
        IdempotencyKey::parse(&"k".repeat(MAX_LEN + 1)),
        Err(IdempotencyKeyError::TooLong { length: 257 })
    );
    for bad in ["a b", "a/b", "a+b", "é", "a%20", "a\n"] {
        assert!(
            matches!(
                IdempotencyKey::parse(bad),
                Err(IdempotencyKeyError::Character { .. })
            ),
            "{bad:?}"
        );
    }
    let err = IdempotencyKey::parse("ab/c").unwrap_err();
    assert_eq!(
        err,
        IdempotencyKeyError::Character {
            found: '/',
            position: 3
        }
    );
    assert!(err.to_string().contains("character 3"), "{err}");
}

#[test]
fn a_request_carries_its_key_as_an_inline_argument() {
    let req = Request::new(Verb::Sink, target());
    assert_eq!(
        IdempotencyKey::from_request(&req, IDEMPOTENCY_KEY_ARG),
        Ok(None)
    );
    let keyed = req.clone().with_arg(IDEMPOTENCY_KEY_ARG, inline("k-1"));
    assert_eq!(
        IdempotencyKey::from_request(&keyed, IDEMPOTENCY_KEY_ARG)
            .unwrap()
            .unwrap()
            .as_str(),
        "k-1"
    );
    let bad = req
        .clone()
        .with_arg(IDEMPOTENCY_KEY_ARG, inline("no spaces"));
    match IdempotencyKey::from_request(&bad, IDEMPOTENCY_KEY_ARG) {
        Err(Error::InvalidArgument { name, .. }) => assert_eq!(name, IDEMPOTENCY_KEY_ARG),
        other => panic!("{other:?}"),
    }
    let by_ref = req.with_arg(IDEMPOTENCY_KEY_ARG, ArgRef::Reference(target()));
    assert!(IdempotencyKey::from_request(&by_ref, IDEMPOTENCY_KEY_ARG).is_err());
}

#[test]
fn the_fingerprint_does_not_depend_on_argument_order() {
    let one = Request::new(Verb::Sink, target())
        .with_arg("title", inline("a bug"))
        .with_arg("body", inline("it breaks"))
        .with_arg(IDEMPOTENCY_KEY_ARG, inline("k1"));
    let other = Request::new(Verb::Sink, target())
        .with_arg(IDEMPOTENCY_KEY_ARG, inline("k1"))
        .with_arg("body", inline("it breaks"))
        .with_arg("title", inline("a bug"));
    assert_eq!(
        one.fingerprint_without(IDEMPOTENCY_KEY_ARG),
        other.fingerprint_without(IDEMPOTENCY_KEY_ARG)
    );
}

#[test]
fn the_fingerprint_leaves_out_the_token_and_nothing_else() {
    let base = Request::new(Verb::Sink, target()).with_arg("title", inline("a bug"));
    let k1 = base.clone().with_arg(IDEMPOTENCY_KEY_ARG, inline("k1"));
    let k2 = base.clone().with_arg(IDEMPOTENCY_KEY_ARG, inline("k2"));
    // The token is not part of it: the same request under two tokens, or none, agrees.
    assert_eq!(k1.fingerprint_without(IDEMPOTENCY_KEY_ARG), base.id());
    assert_eq!(
        k1.fingerprint_without(IDEMPOTENCY_KEY_ARG),
        k2.fingerprint_without(IDEMPOTENCY_KEY_ARG)
    );
    // Leaving out an argument the request does not carry is its id.
    assert_eq!(base.fingerprint_without(IDEMPOTENCY_KEY_ARG), base.id());
    // Everything else is: a different argument value, verb or target differs.
    let retitled = Request::new(Verb::Sink, target())
        .with_arg("title", inline("another bug"))
        .with_arg(IDEMPOTENCY_KEY_ARG, inline("k1"));
    assert_ne!(
        retitled.fingerprint_without(IDEMPOTENCY_KEY_ARG),
        k1.fingerprint_without(IDEMPOTENCY_KEY_ARG)
    );
    let deleted = Request::new(Verb::Delete, target())
        .with_arg("title", inline("a bug"))
        .with_arg(IDEMPOTENCY_KEY_ARG, inline("k1"));
    assert_ne!(
        deleted.fingerprint_without(IDEMPOTENCY_KEY_ARG),
        k1.fingerprint_without(IDEMPOTENCY_KEY_ARG)
    );
    // A content argument contributes its content id, as `id` does.
    let content = base
        .clone()
        .with_arg("blob", ArgRef::Content(ContentId::of(b"x")))
        .with_arg(IDEMPOTENCY_KEY_ARG, inline("k1"));
    assert_eq!(
        content.fingerprint_without(IDEMPOTENCY_KEY_ARG),
        base.with_arg("blob", ArgRef::Content(ContentId::of(b"x")))
            .id()
    );
}

fn scope(principal: Option<&str>) -> ReplayScope {
    ReplayScope::new(
        "urn:ledger:main",
        principal,
        IdempotencyKey::parse("k1").unwrap(),
    )
}

#[test]
fn a_replay_record_round_trips_through_its_bytes() {
    let fingerprint = Request::new(Verb::Sink, target())
        .with_arg("title", inline("a bug"))
        .fingerprint_without(IDEMPOTENCY_KEY_ARG);
    let answer = Representation::new(
        ReprType::new("application/json").with_param("charset", "utf-8"),
        br#"{"id":1}"#.to_vec(),
    );
    let records = [
        Replay::success(scope(None), fingerprint, &answer, Time::from_millis(0)),
        Replay::success(
            scope(Some("urn:example:person:alice")),
            fingerprint,
            &Representation::new(ReprType::new("text/plain"), Vec::new()),
            Time::from_millis(u64::MAX),
        ),
        Replay::failure(
            scope(Some("urn:example:person:bob")),
            fingerprint,
            &Error::Conflict("#1 is closed".into()),
            Time::from_millis(42),
        )
        .expect("a post-decision conflict is recorded"),
    ];
    for record in records {
        let bytes = record.to_bytes();
        assert_eq!(Replay::from_bytes(&bytes).unwrap(), record);
        // Stable: encoding the decoded record gives the same bytes.
        assert_eq!(Replay::from_bytes(&bytes).unwrap().to_bytes(), bytes);
        // Anything else is refused: a truncation, a trailing byte, a different tag.
        assert!(Replay::from_bytes(&bytes[..bytes.len() - 1]).is_err());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(Replay::from_bytes(&longer).is_err());
        let mut retagged = bytes.clone();
        retagged[0] ^= 1;
        assert!(Replay::from_bytes(&retagged).is_err());
    }
}

#[test]
fn the_encoding_is_pinned() {
    // A record written by one release must read in the next: pin the bytes' digest.
    let record = Replay::success(
        scope(Some("urn:example:person:alice")),
        Request::new(Verb::Sink, target()).id(),
        &Representation::new(ReprType::new("text/plain"), b"#1 filed".to_vec()),
        Time::from_millis(1_700_000_000_000),
    );
    let bytes = record.to_bytes();
    assert!(bytes.starts_with(b"ikigai.replay.v1\0"));
    assert_eq!(
        ContentId::of(&bytes).to_string(),
        PINNED_REPLAY_DIGEST,
        "the Replay encoding changed: a stored record would no longer read"
    );
}

const PINNED_REPLAY_DIGEST: &str =
    "b3:11694dacf640f2d496f5b101a78b277e0ce2458e571b3c52c99c61b734fded06";

#[test]
fn scopes_differ_by_principal_endpoint_and_token() {
    let a = scope(None);
    let b = scope(Some("urn:example:person:alice"));
    let c = ReplayScope::new(
        "urn:ledger:other",
        None,
        IdempotencyKey::parse("k1").unwrap(),
    );
    let d = ReplayScope::new(
        "urn:ledger:main",
        None,
        IdempotencyKey::parse("k2").unwrap(),
    );
    let ids = [
        a.content_id(),
        b.content_id(),
        c.content_id(),
        d.content_id(),
    ];
    for (i, x) in ids.iter().enumerate() {
        for y in &ids[i + 1..] {
            assert_ne!(x, y);
        }
    }
    assert_eq!(scope(None).content_id(), a.content_id());
}

#[test]
fn only_successes_and_permanent_post_decision_failures_are_recorded() {
    let fp = Request::new(Verb::Sink, target()).id();
    let at = Time::from_millis(0);
    assert!(Replay::failure(scope(None), fp, &Error::Conflict("raced".into()), at).is_some());
    for not_recorded in [
        Error::Denied("no".into()),
        Error::InvalidArgument {
            name: "title".into(),
            detail: "empty".into(),
        },
        Error::MissingArgument("title".into()),
        Error::RetryAfter {
            delay: None,
            detail: "limited".into(),
        },
        Error::Unavailable("the store is down".into()),
        Error::Timeout("slow".into()),
        Error::Endpoint("unknown".into()),
    ] {
        assert!(!ReplayOutcome::records(&not_recorded), "{not_recorded:?}");
        assert!(Replay::failure(scope(None), fp, &not_recorded, at).is_none());
    }
    let failed = Replay::failure(scope(None), fp, &Error::Conflict("raced".into()), at).unwrap();
    assert!(failed.representation().is_none());
    assert_eq!(failed.outcome, ReplayOutcome::Conflict("raced".into()));
    // A repeat answers the first error exactly, and its class with it.
    let again = Replay::from_bytes(&failed.to_bytes())
        .unwrap()
        .error()
        .unwrap();
    assert_eq!(
        again.to_string(),
        Error::Conflict("raced".into()).to_string()
    );
    assert_eq!(again.class(), FailureClass::Permanent);
    for recorded in [Error::NotFound("#9".into()), Error::Gone("#3".into())] {
        let r = Replay::failure(scope(None), fp, &recorded, at).unwrap();
        let back = Replay::from_bytes(&r.to_bytes()).unwrap();
        assert_eq!(back.error().unwrap().to_string(), recorded.to_string());
    }
}

#[test]
fn a_mismatch_is_an_invalid_argument_and_in_flight_is_retry_after() {
    let key = IdempotencyKey::parse("k1").unwrap();
    let mismatch = key.mismatch(IDEMPOTENCY_KEY_ARG);
    assert!(
        matches!(&mismatch, Error::InvalidArgument { name, .. } if name == IDEMPOTENCY_KEY_ARG)
    );
    assert!(
        mismatch
            .to_string()
            .contains("was used for a different request"),
        "{mismatch}"
    );
    let busy = key.in_flight(Some(Duration::from_millis(250)));
    assert_eq!(busy.class(), FailureClass::RetryAfter);
}

#[test]
fn a_record_answers_within_its_window_and_never_under_24_hours() {
    let record = Replay::success(
        scope(None),
        Request::new(Verb::Sink, target()).id(),
        &Representation::new(ReprType::new("text/plain"), b"ok".to_vec()),
        Time::from_millis(1_000),
    );
    let day = MIN_WINDOW.as_millis() as u64;
    assert!(record.is_live(Time::from_millis(1_000 + day - 1), MIN_WINDOW));
    assert!(!record.is_live(Time::from_millis(1_000 + day), MIN_WINDOW));
    // A shorter window is read as 24 hours.
    assert!(record.is_live(Time::from_millis(1_000 + day - 1), Duration::from_secs(1)));
    let week = Duration::from_secs(7 * 86_400);
    assert!(record.is_live(Time::from_millis(1_000 + 2 * day), week));
}

fn keyed_append() -> ActionSpec {
    ActionSpec::new(Verb::Sink)
        .input(ArgSpec::new("title"))
        .input(ArgSpec::new(IDEMPOTENCY_KEY_ARG).optional())
        .resend_safe_with_key(IDEMPOTENCY_KEY_ARG)
}

#[test]
fn the_window_is_part_of_the_contract_only_when_declared() {
    let plain = keyed_append();
    let windowed = keyed_append().resend_window(MIN_WINDOW);
    assert_eq!(plain.resend_window_secs, None);
    assert_eq!(windowed.resend_window_secs, Some(86_400));
    // Undeclared: the contract and its JSON are unchanged.
    let json = serde_json::to_value(&plain).unwrap();
    assert!(json.get("resend_window").is_none(), "{json}");
    assert_ne!(
        plain.contract_iri("ledger"),
        windowed.contract_iri("ledger")
    );
    assert_ne!(
        windowed.contract_iri("ledger"),
        keyed_append()
            .resend_window(Duration::from_secs(2 * 86_400))
            .contract_iri("ledger")
    );
    // The JSON round-trips.
    let json = serde_json::to_value(&windowed).unwrap();
    assert_eq!(json["resend_window"], 86_400);
    let back: ActionSpec = serde_json::from_value(json).unwrap();
    assert_eq!(back, windowed);
}

#[test]
fn a_window_needs_a_key_and_at_least_24_hours() {
    let ok = Description::new("ledger").action(keyed_append().resend_window(MIN_WINDOW));
    assert!(ok.validate().is_ok());
    let short =
        Description::new("ledger").action(keyed_append().resend_window(Duration::from_secs(3_600)));
    let err = short.validate().unwrap_err().to_string();
    assert!(err.contains("at least 86400 seconds"), "{err}");
    let keyless = Description::new("ledger").action(
        ActionSpec::new(Verb::Sink)
            .resend_safe()
            .resend_window(MIN_WINDOW),
    );
    let err = keyless.validate().unwrap_err().to_string();
    assert!(err.contains("needs an idempotency key"), "{err}");
}

/// An endpoint that adopts the token: its "store" is one mutex holding the filed items AND
/// the replay records, so a record commits in the same update as its write. The first answer
/// is lost (the client never reads it); the retry files nothing new and answers the same bytes.
#[test]
fn an_endpoint_that_adopts_the_token_files_one_copy_and_replays_the_lost_answer() {
    #[derive(Default)]
    struct Store {
        items: Vec<String>,
        replays: HashMap<ContentId, Vec<u8>>,
    }
    let store = Arc::new(Mutex::new(Store::default()));
    let shared = store.clone();
    let endpoint = FnEndpoint::new("ledger", move |inv| {
        let req = inv.request;
        let title = match req.args.get("title") {
            Some(ArgRef::Inline(b)) => String::from_utf8_lossy(b).into_owned(),
            _ => return Err(Error::MissingArgument("title".into())),
        };
        let key = IdempotencyKey::from_request(req, IDEMPOTENCY_KEY_ARG)?;
        let mut s = shared.lock().unwrap();
        let fingerprint = req.fingerprint_without(IDEMPOTENCY_KEY_ARG);
        let scope = key
            .clone()
            .map(|k| ReplayScope::new(req.target.as_str(), inv.capability.principal(), k));
        if let Some(scope) = &scope {
            if let Some(stored) = s.replays.get(&scope.content_id()) {
                let record = Replay::from_bytes(stored).expect("a stored record");
                if !record.matches(fingerprint) {
                    return Err(scope.key.mismatch(IDEMPOTENCY_KEY_ARG));
                }
                return record
                    .representation()
                    .ok_or_else(|| record.error().unwrap());
            }
        }
        s.items.push(title);
        let answer = Representation::new(
            ReprType::new("text/plain"),
            format!("#{} filed", s.items.len()).into_bytes(),
        );
        if let Some(scope) = scope {
            let id = scope.content_id();
            let record = Replay::success(scope, fingerprint, &answer, Time::from_millis(0));
            s.replays.insert(id, record.to_bytes());
        }
        Ok(answer)
    })
    .with_description(Description::new("ledger").action(keyed_append().resend_window(MIN_WINDOW)));
    let space = EndpointSpace::new().bind(Exact::new("urn:ledger:main"), endpoint);
    let kernel = Kernel::new(Arc::new(space));
    let sink = |title: &str, key: &str| {
        Request::new(Verb::Sink, target())
            .with_arg("title", inline(title))
            .with_arg(IDEMPOTENCY_KEY_ARG, inline(key))
    };
    let issue = |req: Request| block_on(kernel.issue(req, &Capability::root()));

    let lost = issue(sink("a bug", "k1")).unwrap();
    let retry = issue(sink("a bug", "k1")).unwrap();
    assert_eq!(store.lock().unwrap().items.len(), 1, "one copy");
    assert_eq!(
        retry.bytes, lost.bytes,
        "the retry's bytes equal the lost answer's"
    );
    assert_eq!(retry.repr_type, lost.repr_type);

    // The same token for a different request is refused, and writes nothing.
    match issue(sink("another bug", "k1")) {
        Err(Error::InvalidArgument { name, detail }) => {
            assert_eq!(name, IDEMPOTENCY_KEY_ARG);
            assert!(
                detail.contains("was used for a different request"),
                "{detail}"
            );
        }
        other => panic!("{other:?}"),
    }
    // A new token is a new request.
    issue(sink("a bug", "k2")).unwrap();
    assert_eq!(store.lock().unwrap().items.len(), 2);
}
