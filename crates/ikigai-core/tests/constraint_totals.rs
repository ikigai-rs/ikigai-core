//! **`urn:kernel:constraint over=start`** (ledger #4096): the totals since the kernel
//! started, per endpoint and target shape (`docs/design/constraint-history.md`, sections
//! 3b and 4). Every count only grows; past 256 rows new endpoints fold into `(other)`;
//! a cache hit credits the row that computed the entry; a template endpoint's shape
//! never holds a captured value; a restart shows as a new start time. `over=window`, the
//! default, is the window as it was.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Clock, EndpointSpace, Error, Exact, FnEndpoint, Iri, Kernel, ReprType,
    Representation, Request, Threaded, Time, UriTemplate, Verb,
};
use oxrdf::{NamedOrBlankNode, Term, Triple};

const IK: &str = "https://ikigai-rs.dev/ns#";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

/// A clock that advances 5 ms every time it is read, from `start`.
struct SteppingClock(AtomicU64);

impl Clock for SteppingClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.fetch_add(5, Ordering::SeqCst))
    }
}

fn clock(start: u64) -> Arc<SteppingClock> {
    Arc::new(SteppingClock(AtomicU64::new(start)))
}

fn body(text: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), text.as_bytes().to_vec())
}

fn issue(kernel: &Kernel, target: &str) -> Result<Representation, Error> {
    block_on(kernel.issue(Request::new(Verb::Source, iri(target)), &Capability::root()))
}

fn totals(kernel: &Kernel, turtle: bool) -> String {
    let mut request = Request::new(Verb::Source, iri("urn:kernel:constraint"))
        .with_arg("over", ArgRef::Inline(b"start".to_vec()));
    if turtle {
        request = request.with_arg("as", ArgRef::Inline(b"text/turtle".to_vec()));
    }
    let answer = block_on(kernel.issue(request, &Capability::root())).unwrap();
    String::from_utf8(answer.bytes).unwrap()
}

fn parse(turtle: &str) -> Vec<Triple> {
    let triples: Vec<Triple> = oxttl::TurtleParser::new()
        .for_reader(turtle.as_bytes())
        .map(|t| t.unwrap_or_else(|e| panic!("{e}\n{turtle}")))
        .collect();
    for t in &triples {
        assert!(
            !matches!(t.subject, NamedOrBlankNode::BlankNode(_))
                && !matches!(t.object, Term::BlankNode(_)),
            "blank node in {t}\n{turtle}"
        );
    }
    triples
}

fn subject(t: &Triple) -> String {
    match &t.subject {
        NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
        other => other.to_string(),
    }
}

fn object(t: &Triple) -> String {
    match &t.object {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        other => other.to_string(),
    }
}

/// One totals row of the Turtle face, its properties by local name.
type Row = BTreeMap<String, Vec<String>>;

/// The Turtle face's kernel properties and its rows, keyed by `endpointId targetShape`.
fn rows(kernel: &Kernel) -> (Row, BTreeMap<String, Row>) {
    let turtle = totals(kernel, true);
    let triples = parse(&turtle);
    let mut by_subject: BTreeMap<String, Row> = BTreeMap::new();
    for t in &triples {
        let p = t.predicate.as_str();
        let local = p.strip_prefix(IK).unwrap_or(p).to_string();
        by_subject
            .entry(subject(t))
            .or_default()
            .entry(local)
            .or_default()
            .push(object(t));
    }
    let head = by_subject.remove("urn:kernel:constraint").expect(&turtle);
    let mut named = BTreeMap::new();
    for row in head.get("totalsRow").cloned().unwrap_or_default() {
        let props = by_subject[&row].clone();
        let id = props["endpointId"][0].clone();
        let shape = props
            .get("targetShape")
            .map(|s| s[0].clone())
            .unwrap_or_default();
        named.insert(format!("{id} {shape}").trim().to_string(), props);
    }
    (head, named)
}

fn count(row: &Row, property: &str) -> u64 {
    row.get(property)
        .map(|v| v[0].parse().unwrap())
        .unwrap_or(0)
}

fn space() -> EndpointSpace {
    EndpointSpace::new()
        .bind(
            Exact::new("urn:test:volatile"),
            FnEndpoint::new("volatile", |_| Ok(body("v"))),
        )
        .bind(
            Exact::new("urn:test:kept"),
            FnEndpoint::new("kept", |_| Ok(body("k").cacheable())),
        )
        .bind(
            Exact::new("urn:test:failing"),
            FnEndpoint::new("failing", |_| Err(Error::Endpoint("down".to_string()))),
        )
        .bind(
            UriTemplate::parse("urn:file:{path}").unwrap(),
            FnEndpoint::new("file", |_| Ok(body("f").cacheable())),
        )
}

#[test]
fn totals_count_computed_stored_failed_and_hits_per_endpoint_and_grow_monotonically() {
    let kernel = Kernel::new(Arc::new(space())).with_clock(clock(1_000));
    let mut previous: Option<BTreeMap<String, Row>> = None;
    for round in 0..4u64 {
        issue(&kernel, "urn:test:volatile").unwrap();
        issue(&kernel, "urn:test:kept").unwrap();
        issue(&kernel, "urn:test:failing").unwrap_err();
        let (head, named) = rows(&kernel);
        assert_eq!(count(&head, "totalResolutions"), 3 * (round + 1));
        let volatile = &named["volatile urn:test:volatile"];
        assert_eq!(count(volatile, "uncachedCallCount"), round + 1);
        assert_eq!(count(volatile, "storedCallCount"), 0);
        let kept = &named["kept urn:test:kept"];
        assert_eq!(count(kept, "uncachedCallCount"), 1, "computed once");
        assert_eq!(count(kept, "storedCallCount"), 1, "and stored");
        assert_eq!(count(kept, "cachedCallCount"), round, "then served");
        let failing = &named["failing urn:test:failing"];
        assert_eq!(count(failing, "uncachedCallCount"), round + 1);
        assert_eq!(count(failing, "failedCallCount"), round + 1);
        assert_eq!(
            count(failing, "failedMillis"),
            count(failing, "uncachedMillis"),
            "a failure's time is uncached time"
        );
        assert!(count(failing, "failedMillis") > 0);
        // Every counter of every row only grows.
        if let Some(previous) = &previous {
            for (name, before) in previous {
                let now = &named[name];
                for property in [
                    "uncachedCallCount",
                    "storedCallCount",
                    "cachedCallCount",
                    "failedCallCount",
                    "uncachedMillis",
                    "failedMillis",
                ] {
                    assert!(
                        count(now, property) >= count(before, property),
                        "{name} {property} fell"
                    );
                }
            }
        }
        previous = Some(named);
    }
    // The volatile endpoint's answers were not stored because it declared them so:
    // counted by kind, the kind's words those of `urn:kernel:uncached`.
    let turtle = totals(&kernel, true);
    assert!(
        turtle.contains("ik:reasonKind \"declared\" ;\n    ik:count \"4\""),
        "{turtle}"
    );
    let text = totals(&kernel, false);
    assert!(text.contains("not stored: declared 4"), "{text}");
}

#[test]
fn a_cache_hit_credits_the_row_that_computed_the_entry() {
    let kernel = Kernel::new(Arc::new(space())).with_clock(clock(1));
    for _ in 0..5 {
        issue(&kernel, "urn:test:kept").unwrap();
    }
    let (_, named) = rows(&kernel);
    let kept = &named["kept urn:test:kept"];
    assert_eq!(
        (
            count(kept, "uncachedCallCount"),
            count(kept, "storedCallCount"),
            count(kept, "cachedCallCount"),
        ),
        (1, 1, 4)
    );
    // A hit costs no capacity: only the computation's milliseconds are counted.
    let computed_ms = count(kept, "uncachedMillis");
    issue(&kernel, "urn:test:kept").unwrap();
    let (_, named) = rows(&kernel);
    assert_eq!(
        count(&named["kept urn:test:kept"], "uncachedMillis"),
        computed_ms
    );
    assert_eq!(count(&named["kept urn:test:kept"], "cachedCallCount"), 5);
}

#[test]
fn a_template_endpoints_shape_never_holds_a_captured_value() {
    let kernel = Kernel::new(Arc::new(space())).with_clock(clock(1));
    for path in ["/home/alice/notes.md", "/home/bob/secret.txt", "/srv/x"] {
        issue(&kernel, &format!("urn:file:{path}")).unwrap();
        issue(&kernel, &format!("urn:file:{path}")).unwrap();
    }
    let (_, named) = rows(&kernel);
    let file = &named["file urn:file:{path}"];
    assert_eq!(named.len(), 1, "one row for every name: {named:?}");
    assert_eq!(count(file, "uncachedCallCount"), 3);
    assert_eq!(count(file, "cachedCallCount"), 3);
    for face in [totals(&kernel, false), totals(&kernel, true)] {
        for value in ["alice", "bob", "notes", "secret", "/srv"] {
            assert!(!face.contains(value), "`{value}` leaked:\n{face}");
        }
        assert!(face.contains("urn:file:{path}"), "{face}");
    }
}

#[test]
fn a_per_resolution_wrapper_is_one_row_not_one_per_request() {
    // A `Threaded` space wraps every endpoint it serves in a fresh `Arc` per
    // resolution: a new address each time, one endpoint by its name.
    let threaded = Threaded::new(Arc::new(space()), "urn:test:thread");
    let kernel = Kernel::new(Arc::new(threaded)).with_clock(clock(1));
    for _ in 0..10 {
        issue(&kernel, "urn:test:volatile").unwrap();
    }
    let (_, named) = rows(&kernel);
    assert_eq!(named.len(), 1, "{named:?}");
    assert_eq!(
        count(&named["volatile urn:test:volatile"], "uncachedCallCount"),
        10
    );
}

#[test]
fn past_256_rows_new_endpoints_fold_into_one_counted_other_row() {
    let mut space = EndpointSpace::new();
    for i in 0..300 {
        space = space.bind(
            Exact::new(format!("urn:test:e{i}")),
            FnEndpoint::new(format!("e{i}"), |_| Ok(body("e"))),
        );
    }
    let kernel = Kernel::new(Arc::new(space)).with_clock(clock(1));
    for round in 1..=2u64 {
        for i in 0..300 {
            issue(&kernel, &format!("urn:test:e{i}")).unwrap();
        }
        let (head, named) = rows(&kernel);
        assert_eq!(named.len(), 257, "256 rows and (other)");
        let other = &named["(other)"];
        assert_eq!(count(other, "foldedEndpointCount"), 44, "distinct names");
        assert_eq!(count(other, "uncachedCallCount"), 44 * round);
        // Nothing is evicted: the first 256 endpoints keep their own rows.
        for i in 0..256 {
            assert_eq!(
                count(&named[&format!("e{i} urn:test:e{i}")], "uncachedCallCount"),
                round
            );
        }
        let sum: u64 = named.values().map(|r| count(r, "uncachedCallCount")).sum();
        assert_eq!(sum, count(&head, "totalResolutions"));
    }
    let text = totals(&kernel, false);
    assert!(text.contains("(other)  44 endpoints"), "{text}");
}

#[test]
fn a_restart_shows_as_a_new_start_time_and_counts_from_zero() {
    let first = Kernel::new(Arc::new(space())).with_clock(clock(1_000));
    issue(&first, "urn:test:volatile").unwrap();
    issue(&first, "urn:test:volatile").unwrap();
    let (head, _) = rows(&first);
    assert_eq!(count(&head, "kernelStartedMillis"), 1_000);
    assert_eq!(count(&head, "totalResolutions"), 2);
    // Later, the host restarts: a new kernel on the same clock.
    let restarted = Kernel::new(Arc::new(space())).with_clock(clock(90_000));
    issue(&restarted, "urn:test:volatile").unwrap();
    let (head, named) = rows(&restarted);
    assert_eq!(count(&head, "kernelStartedMillis"), 90_000);
    assert_eq!(count(&head, "totalResolutions"), 1);
    assert_eq!(
        count(&named["volatile urn:test:volatile"], "uncachedCallCount"),
        1
    );
    assert!(totals(&restarted, false).contains("started at 90000ms on the kernel's clock"));
}

#[test]
fn over_window_is_the_default_and_unchanged_and_other_spans_are_refused() {
    let kernel = Kernel::new(Arc::new(space())).with_clock(clock(1));
    issue(&kernel, "urn:test:volatile").unwrap();
    let plain = block_on(kernel.issue(
        Request::new(Verb::Source, iri("urn:kernel:constraint")),
        &Capability::root(),
    ))
    .unwrap();
    let window = block_on(
        kernel.issue(
            Request::new(Verb::Source, iri("urn:kernel:constraint"))
                .with_arg("over", ArgRef::Inline(b"window".to_vec())),
            &Capability::root(),
        ),
    )
    .unwrap();
    assert_eq!(plain.bytes, window.bytes);
    assert!(String::from_utf8_lossy(&plain.bytes).starts_with("constraint  (last 1 resolutions)"));
    let refused = block_on(
        kernel.issue(
            Request::new(Verb::Source, iri("urn:kernel:constraint"))
                .with_arg("over", ArgRef::Inline(b"week".to_vec())),
            &Capability::root(),
        ),
    )
    .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("`week` is not a span of `urn:kernel:constraint` (window, start)"),
        "{refused}"
    );
}

#[test]
fn the_totals_are_live_state_and_need_the_inspect_capability() {
    let kernel = Kernel::new(Arc::new(space()));
    let before = totals(&kernel, false);
    issue(&kernel, "urn:test:volatile").unwrap();
    let after = totals(&kernel, false);
    assert_ne!(
        before, after,
        "uncacheable: a second read sees the new resolution"
    );
    assert!(before.starts_with("constraint  (since start: 0 resolutions, 0 rows)\n"));
    // A clockless kernel counts, and states no time.
    assert!(after.contains("volatile  urn:test:volatile   ← constraint\n    1 computed"));
    assert!(!after.contains("started at"));
    let denied = block_on(
        kernel.issue(
            Request::new(Verb::Source, iri("urn:kernel:constraint"))
                .with_arg("over", ArgRef::Inline(b"start".to_vec())),
            &Capability::scoped(["urn:cap:other"]),
        ),
    )
    .unwrap_err();
    assert!(matches!(denied, Error::Denied(_)), "{denied}");
}
