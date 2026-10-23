//! **The Turtle faces of `urn:kernel:scheduler` and `urn:kernel:constraint`**
//! (ledger #1800): each parses as Turtle, holds no blank node, states the values the
//! text face shows, and is refused to a caller without `urn:cap:kernel:inspect`, as
//! the text face is.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, Clock, EndpointSpace, Error, Exact, Expiry, FnEndpoint, Iri, Kernel,
    ReprType, Representation, Request, SchedulerReporter, Time, Verb,
};
use oxrdf::{NamedOrBlankNode, Term, Triple};

const IK: &str = "https://ikigai-rs.dev/ns#";
const XSD_NNI: &str = "http://www.w3.org/2001/XMLSchema#nonNegativeInteger";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn request(op: &str, turtle: bool) -> Request {
    let request = Request::new(Verb::Source, iri(&format!("urn:kernel:{op}")));
    if turtle {
        request.with_arg("as", ArgRef::Inline(b"text/turtle".to_vec()))
    } else {
        request
    }
}

fn ask(kernel: &Kernel, op: &str, turtle: bool, cap: &Capability) -> Result<String, Error> {
    block_on(kernel.issue(request(op, turtle), cap)).map(|r| {
        let media = r.repr_type.media_type.clone();
        assert_eq!(
            media,
            if turtle { "text/turtle" } else { "text/plain" },
            "{op}"
        );
        String::from_utf8(r.bytes).unwrap()
    })
}

/// Parse a face, refusing any blank node in any position.
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
    assert!(!triples.is_empty(), "{turtle}");
    triples
}

/// The objects of `subject ik:{property}`.
fn objects(triples: &[Triple], subject: &str, property: &str) -> Vec<Term> {
    let predicate = format!("{IK}{property}");
    triples
        .iter()
        .filter(|t| {
            matches!(&t.subject, NamedOrBlankNode::NamedNode(n) if n.as_str() == subject)
                && t.predicate.as_str() == predicate
        })
        .map(|t| t.object.clone())
        .collect()
}

/// The one literal value of `subject ik:{property}`, with its datatype.
fn literal(triples: &[Triple], subject: &str, property: &str) -> (String, String) {
    let found = objects(triples, subject, property);
    assert_eq!(found.len(), 1, "{subject} ik:{property}: {found:?}");
    match &found[0] {
        Term::Literal(l) => (l.value().to_string(), l.datatype().as_str().to_string()),
        other => panic!("{subject} ik:{property} is not a literal: {other}"),
    }
}

fn count(triples: &[Triple], subject: &str, property: &str) -> String {
    let (value, datatype) = literal(triples, subject, property);
    assert_eq!(datatype, XSD_NNI, "{subject} ik:{property}");
    value
}

fn node(term: &Term) -> String {
    match term {
        Term::NamedNode(n) => n.as_str().to_string(),
        other => panic!("not an IRI: {other}"),
    }
}

fn has_type(triples: &[Triple], subject: &str, class: &str) -> bool {
    let class = format!("{IK}{class}");
    triples.iter().any(|t| {
        matches!(&t.subject, NamedOrBlankNode::NamedNode(n) if n.as_str() == subject)
            && t.predicate.as_str() == "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
            && matches!(&t.object, Term::NamedNode(n) if n.as_str() == class)
    })
}

// --- urn:kernel:scheduler --------------------------------------------------

#[test]
fn the_default_scheduler_face_states_the_single_threaded_default() {
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    let root = Capability::root();
    let text = ask(&kernel, "scheduler", false, &root).unwrap();
    assert_eq!(text, "scheduler\n  backend    single\n  threads    1\n");

    let turtle = ask(&kernel, "scheduler", true, &root).unwrap();
    let triples = parse(&turtle);
    let s = "urn:kernel:scheduler";
    assert!(has_type(&triples, s, "Scheduler"), "{turtle}");
    assert_eq!(literal(&triples, s, "schedulerBackend").0, "single");
    assert_eq!(count(&triples, s, "workerThreads"), "1");
    assert!(objects(&triples, s, "reading").is_empty(), "{turtle}");
}

struct HostReporter;

impl SchedulerReporter for HostReporter {
    fn rows(&self) -> Vec<(String, String)> {
        [
            ("backend", "pool"),
            ("threads", "4"),
            ("active", "2"),
            ("spawned", "17"),
            ("completed", "15"),
            ("queue depth", "3 \"waiting\""),
        ]
        .iter()
        .map(|(l, v)| (l.to_string(), v.to_string()))
        .collect()
    }
}

#[test]
fn a_reporters_rows_become_typed_properties_and_an_unknown_row_a_reading() {
    let kernel =
        Kernel::new(Arc::new(EndpointSpace::new())).with_scheduler_reporter(Arc::new(HostReporter));
    let root = Capability::root();
    let text = ask(&kernel, "scheduler", false, &root).unwrap();
    assert_eq!(
        text,
        "scheduler\n  backend    pool\n  threads    4\n  active     2\n  spawned    17\n  \
         completed  15\n  queue depth 3 \"waiting\"\n"
    );

    let turtle = ask(&kernel, "scheduler", true, &root).unwrap();
    let triples = parse(&turtle);
    let s = "urn:kernel:scheduler";
    assert_eq!(literal(&triples, s, "schedulerBackend").0, "pool");
    assert_eq!(count(&triples, s, "workerThreads"), "4");
    assert_eq!(count(&triples, s, "activeTasks"), "2");
    assert_eq!(count(&triples, s, "spawnedTasks"), "17");
    assert_eq!(count(&triples, s, "completedTasks"), "15");

    let readings = objects(&triples, s, "reading");
    assert_eq!(readings.len(), 1, "{turtle}");
    let reading = node(&readings[0]);
    assert!(has_type(&triples, &reading, "Reading"), "{turtle}");
    assert_eq!(literal(&triples, &reading, "readingLabel").0, "queue depth");
    assert_eq!(
        literal(&triples, &reading, "readingValue").0,
        "3 \"waiting\""
    );
    assert_eq!(literal(&triples, &reading, "order").0, "5");
}

// --- urn:kernel:constraint -------------------------------------------------

/// A clock that advances 5 ms every time it is read, so every resolution takes time.
struct SteppingClock(AtomicU64);

impl Clock for SteppingClock {
    fn now(&self) -> Time {
        Time::from_millis(self.0.fetch_add(5, Ordering::SeqCst))
    }
}

fn text_body(body: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec())
}

fn constraint_kernel(clock: bool) -> Kernel {
    let space = EndpointSpace::new()
        .bind(
            Exact::new("urn:test:heavy"),
            FnEndpoint::new("heavy", |_| Ok(text_body("H"))),
        )
        .bind(
            Exact::new("urn:test:light"),
            FnEndpoint::new("light", |_| Ok(text_body("L"))),
        )
        .bind(
            Exact::new("urn:test:kept"),
            FnEndpoint::new("kept", |_| Ok(text_body("K").cacheable())),
        );
    let kernel = Kernel::new(Arc::new(space));
    let kernel = if clock {
        kernel.with_clock(Arc::new(SteppingClock(AtomicU64::new(1_000))))
    } else {
        kernel
    };
    let root = Capability::root();
    for target in [
        "urn:test:heavy",
        "urn:test:heavy",
        "urn:test:heavy",
        "urn:test:light",
        "urn:test:kept",
        "urn:test:kept",
    ] {
        block_on(kernel.issue(Request::new(Verb::Source, iri(target)), &root)).unwrap();
    }
    kernel
}

/// The text face's rows: (target, cost, calls, cached %), in its order, and whether
/// the first is marked as the constraint.
fn text_rows(text: &str) -> (Vec<(String, String, String, String)>, bool) {
    let lines: Vec<&str> = text.lines().skip(1).collect();
    let marked = lines.first().is_some_and(|l| l.contains("← constraint"));
    let rows = lines
        .chunks(2)
        .map(|pair| {
            let target = pair[0]
                .trim()
                .trim_end_matches("← constraint")
                .trim()
                .to_string();
            let parts: Vec<&str> = pair[1].trim().split(" · ").collect();
            let cost = parts[0].split(' ').next().unwrap().to_string();
            let calls = parts[1].trim_end_matches(" calls").to_string();
            let pct = parts[2].trim_end_matches("% cached").to_string();
            (target, cost, calls, pct)
        })
        .collect();
    (rows, marked)
}

fn check_constraint_faces(clock: bool) {
    let kernel = constraint_kernel(clock);
    let root = Capability::root();
    let text = ask(&kernel, "constraint", false, &root).unwrap();
    assert!(
        text.starts_with("constraint  (last 6 resolutions)\n"),
        "{text}"
    );
    let (rows, marked) = text_rows(&text);
    assert_eq!(rows.len(), 3, "{text}");
    assert!(marked, "{text}");

    let turtle = ask(&kernel, "constraint", true, &root).unwrap();
    let triples = parse(&turtle);
    let w = "urn:kernel:constraint";
    assert!(has_type(&triples, w, "ConstraintWindow"), "{turtle}");
    assert_eq!(count(&triples, w, "windowResolutions"), "6");

    let mut ranked: Vec<(i64, String)> = objects(&triples, w, "rankedTarget")
        .iter()
        .map(|t| {
            let n = node(t);
            (literal(&triples, &n, "order").0.parse().unwrap(), n)
        })
        .collect();
    ranked.sort();
    assert_eq!(ranked.len(), rows.len(), "{turtle}");

    for ((rank, n), (target, cost, calls, pct)) in ranked.iter().zip(&rows) {
        assert!(has_type(&triples, n, "ConstraintTarget"), "{turtle}");
        let resource = objects(&triples, n, "constraintResource");
        assert_eq!(resource.len(), 1);
        assert_eq!(&node(&resource[0]), target, "rank {rank}");
        assert_eq!(&count(&triples, n, "callCount"), calls);
        assert_eq!(&count(&triples, n, "cachedPercent"), pct);
        let total: usize = calls.parse().unwrap();
        let cached: usize = count(&triples, n, "cachedCallCount").parse().unwrap();
        let uncached: usize = count(&triples, n, "uncachedCallCount").parse().unwrap();
        assert_eq!(cached + uncached, total);
        if clock {
            assert_eq!(
                Some(&count(&triples, n, "uncachedMillis")),
                cost.strip_suffix("ms").map(str::to_string).as_ref()
            );
        } else {
            assert!(
                objects(&triples, n, "uncachedMillis").is_empty(),
                "{turtle}"
            );
            assert_eq!(&uncached.to_string(), cost);
        }
    }
    // The leader the text marks is the window's current constraint.
    let current = objects(&triples, w, "currentConstraint");
    assert_eq!(current.len(), 1, "{turtle}");
    assert_eq!(node(&current[0]), ranked[0].1);
    assert_eq!(
        node(&objects(&triples, &ranked[0].1, "constraintResource")[0]),
        "urn:test:heavy"
    );
    // The cacheable target was served once from the cache.
    let kept = ranked
        .iter()
        .find(|(_, n)| node(&objects(&triples, n, "constraintResource")[0]) == "urn:test:kept")
        .expect("kept listed");
    assert_eq!(count(&triples, &kept.1, "cachedCallCount"), "1");
    assert_eq!(count(&triples, &kept.1, "cachedPercent"), "50");
}

#[test]
fn the_constraint_face_holds_what_the_text_face_shows_on_a_clockless_kernel() {
    check_constraint_faces(false);
}

#[test]
fn the_constraint_face_holds_what_the_text_face_shows_with_a_clock() {
    check_constraint_faces(true);
}

#[test]
fn an_empty_window_has_no_targets_and_no_constraint() {
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    let root = Capability::root();
    assert_eq!(
        ask(&kernel, "constraint", false, &root).unwrap(),
        "constraint  (last 0 resolutions)\n  (no resolutions yet)\n"
    );
    let turtle = ask(&kernel, "constraint", true, &root).unwrap();
    let triples = parse(&turtle);
    let w = "urn:kernel:constraint";
    assert_eq!(count(&triples, w, "windowResolutions"), "0");
    assert!(objects(&triples, w, "rankedTarget").is_empty(), "{turtle}");
    assert!(
        objects(&triples, w, "currentConstraint").is_empty(),
        "{turtle}"
    );
}

// --- both ---------------------------------------------------------------

#[test]
fn both_faces_of_both_are_refused_without_the_inspect_capability() {
    let kernel = constraint_kernel(false).with_scheduler_reporter(Arc::new(HostReporter));
    let scoped = Capability::scoped(["urn:cap:something:else"]);
    for op in ["scheduler", "constraint"] {
        for turtle in [false, true] {
            match ask(&kernel, op, turtle, &scoped) {
                Err(Error::Denied(detail)) => {
                    assert!(detail.contains("urn:cap:kernel:inspect"), "{detail}")
                }
                other => panic!("{op} (turtle {turtle}): {other:?}"),
            }
        }
    }
    let inspect = Capability::scoped(["urn:cap:kernel:inspect"]);
    for op in ["scheduler", "constraint"] {
        for turtle in [false, true] {
            ask(&kernel, op, turtle, &inspect).unwrap();
        }
    }
}

#[test]
fn another_face_is_refused_and_neither_face_is_cacheable() {
    let kernel = constraint_kernel(false);
    let root = Capability::root();
    for op in ["scheduler", "constraint"] {
        let bad = Request::new(Verb::Source, iri(&format!("urn:kernel:{op}")))
            .with_arg("as", ArgRef::Inline(b"application/json".to_vec()));
        assert!(
            matches!(
                block_on(kernel.issue(bad, &root)),
                Err(Error::InvalidArgument { .. })
            ),
            "{op}"
        );
        for turtle in [false, true] {
            let repr = block_on(kernel.issue(request(op, turtle), &root)).unwrap();
            assert_eq!(repr.expiry, Expiry::Always, "{op} (turtle {turtle})");
        }
    }
}
