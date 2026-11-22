//! **A closed integrity and confidentiality lattice, carried as open label IRIs** (ledger
//! #3640, design note `docs/design/label-lattice.md`, extending
//! `docs/design/provenance-labels.md`).
//!
//! Nothing here is kernel API. The lattice, its encoding and its policy check live in this
//! file as a model, so that the note's claims are pinned before anything is built:
//!
//! 1. the join is fixed: integrity takes the minimum, confidentiality the maximum, and the
//!    join is commutative, associative and idempotent, with (trusted, public) as its identity;
//! 2. a lattice point encodes as a set of threshold IRIs such that **set union is the
//!    join**, so the provenance-label design, which only ever unions label sets, carries the
//!    lattice with no second join operator;
//! 3. the empty label set is the bottom point (trusted, public), so an unlabeled
//!    representation means what it means today: default behavior is unchanged;
//! 4. a gatekeeper names the highest point it admits, and that is two refused IRIs: the
//!    per-label refusal of the provenance-label design, applied to threshold IRIs, admits
//!    exactly the points at or below it;
//! 5. an endpoint that adds a stray threshold IRI can only lower integrity or raise
//!    confidentiality, never the reverse;
//! 6. the kernel's existing join already computes the lattice join through a real
//!    resolution, using dependency threads as a stand-in carrier for labels.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use futures::executor::block_on;
use ikigai_core::{
    Capability, Endpoint, EndpointSpace, Exact, FnEndpoint, Invocation, Iri, Kernel, ReprType,
    Representation, Request, Result, Verb,
};

// ── The model ────────────────────────────────────────────────────────────────────

/// Integrity, lowest first. Fixed by the consumer, not open.
const INTEGRITY: [&str; 3] = ["untrusted", "endorsed", "trusted"];
/// Confidentiality, lowest first.
const CONFIDENTIALITY: [&str; 3] = ["public", "internal", "secret"];

/// A lattice point: indexes into `INTEGRITY` and `CONFIDENTIALITY`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Point {
    integrity: usize,
    confidentiality: usize,
}

const fn p(integrity: usize, confidentiality: usize) -> Point {
    Point {
        integrity,
        confidentiality,
    }
}

const UNTRUSTED: usize = 0;
const ENDORSED: usize = 1;
const TRUSTED: usize = 2;
const PUBLIC: usize = 0;
const INTERNAL: usize = 1;
const SECRET: usize = 2;

/// The bottom of the flow order: what a constant, or an unlabeled answer, carries.
const BOTTOM: Point = p(TRUSTED, PUBLIC);

fn all_points() -> impl Iterator<Item = Point> {
    (0..INTEGRITY.len()).flat_map(|i| (0..CONFIDENTIALITY.len()).map(move |c| p(i, c)))
}

/// The fixed join: integrity by minimum, confidentiality by maximum.
fn join(a: Point, b: Point) -> Point {
    p(
        a.integrity.min(b.integrity),
        a.confidentiality.max(b.confidentiality),
    )
}

/// The flow order: `a` may flow where `b` is admitted.
fn flows_to(a: Point, b: Point) -> bool {
    a.integrity >= b.integrity && a.confidentiality <= b.confidentiality
}

/// "Integrity is below `level`": one IRI per level above the lowest.
fn below(level: usize) -> String {
    format!("urn:label:integrity:below:{}", INTEGRITY[level])
}

/// "Confidentiality is above `level`": one IRI per level below the highest.
fn above(level: usize) -> String {
    format!("urn:label:confidentiality:above:{}", CONFIDENTIALITY[level])
}

/// A point as the set of threshold IRIs it satisfies. The top integrity and the bottom
/// confidentiality satisfy none, so `BOTTOM` is the empty set.
fn encode(point: Point) -> BTreeSet<String> {
    let integrity = (point.integrity + 1..INTEGRITY.len()).map(below);
    let confidentiality = (0..point.confidentiality).map(above);
    integrity.chain(confidentiality).collect()
}

/// Reads a point back from any label set. Open labels that are not threshold IRIs are
/// ignored. A set no `encode` produced (a stray IRI an endpoint added) still decodes, to
/// the lowest integrity and highest confidentiality any of its IRIs claims.
fn decode<S: AsRef<str>>(labels: impl IntoIterator<Item = S>) -> Point {
    let mut point = BOTTOM;
    for label in labels {
        let label = label.as_ref();
        if let Some(name) = label.strip_prefix("urn:label:integrity:below:") {
            let level = INTEGRITY.iter().position(|l| *l == name).unwrap();
            point.integrity = point.integrity.min(level - 1);
        } else if let Some(name) = label.strip_prefix("urn:label:confidentiality:above:") {
            let level = CONFIDENTIALITY.iter().position(|l| *l == name).unwrap();
            point.confidentiality = point.confidentiality.max(level + 1);
        }
    }
    point
}

/// A gatekeeper policy naming the highest point it admits, spelled as the provenance-label
/// design's `refuses`: the IRIs whose presence refuses the request.
fn refusals_for(ceiling: Point) -> BTreeSet<String> {
    let mut refused = BTreeSet::new();
    if ceiling.integrity > UNTRUSTED {
        refused.insert(below(ceiling.integrity));
    }
    if ceiling.confidentiality + 1 < CONFIDENTIALITY.len() {
        refused.insert(above(ceiling.confidentiality));
    }
    refused
}

/// The floor check of the provenance-label design: refused if any refused IRI is present.
fn admitted(labels: &BTreeSet<String>, refused: &BTreeSet<String>) -> bool {
    labels.is_disjoint(refused)
}

fn union(a: &BTreeSet<String>, b: &BTreeSet<String>) -> BTreeSet<String> {
    a.union(b).cloned().collect()
}

// 1 ───────────────────────────────────────────────────────────────────────────────

#[test]
fn integrity_joins_by_minimum_and_confidentiality_by_maximum() {
    let cases = [
        // A trusted public constant joined with an untrusted page: the page's integrity.
        (
            p(TRUSTED, PUBLIC),
            p(UNTRUSTED, PUBLIC),
            p(UNTRUSTED, PUBLIC),
        ),
        // A trusted secret joined with an untrusted public page: the worst of each side.
        (
            p(TRUSTED, SECRET),
            p(UNTRUSTED, PUBLIC),
            p(UNTRUSTED, SECRET),
        ),
        // Neither dominates: each dimension joins on its own.
        (
            p(ENDORSED, PUBLIC),
            p(TRUSTED, INTERNAL),
            p(ENDORSED, INTERNAL),
        ),
        // Joining a point with itself changes nothing.
        (
            p(ENDORSED, INTERNAL),
            p(ENDORSED, INTERNAL),
            p(ENDORSED, INTERNAL),
        ),
        // The bottom is the identity.
        (BOTTOM, p(UNTRUSTED, SECRET), p(UNTRUSTED, SECRET)),
    ];
    for (a, b, expected) in cases {
        assert_eq!(join(a, b), expected, "{a:?} ⊔ {b:?}");
        assert_eq!(join(b, a), expected, "{b:?} ⊔ {a:?}");
    }
}

#[test]
fn the_join_is_a_semilattice_with_bottom_as_identity() {
    for a in all_points() {
        assert_eq!(join(a, a), a);
        assert_eq!(join(BOTTOM, a), a);
        for b in all_points() {
            assert_eq!(join(a, b), join(b, a));
            // The join is an upper bound in the flow order, and only accumulates.
            assert!(flows_to(a, join(a, b)) && flows_to(b, join(a, b)));
            for c in all_points() {
                assert_eq!(join(join(a, b), c), join(a, join(b, c)));
            }
        }
    }
}

// 2 ───────────────────────────────────────────────────────────────────────────────

#[test]
fn set_union_of_encodings_is_the_lattice_join() {
    for a in all_points() {
        assert_eq!(decode(encode(a)), a, "round trip of {a:?}");
        for b in all_points() {
            let unioned = union(&encode(a), &encode(b));
            assert_eq!(unioned, encode(join(a, b)), "{a:?} ∪ {b:?}");
        }
    }
}

#[test]
fn threshold_iris_sit_beside_open_labels_in_one_set() {
    let mut labels = encode(p(ENDORSED, INTERNAL));
    labels.insert("urn:label:origin:web".into());
    labels.insert("urn:label:mailbox".into());
    // Open labels are carried and ignored by the lattice reading.
    assert_eq!(decode(&labels), p(ENDORSED, INTERNAL));
    // And joined with another point, both survive: the open ones and the lattice's.
    let joined = union(&labels, &encode(p(TRUSTED, SECRET)));
    assert!(joined.contains("urn:label:origin:web"));
    assert_eq!(decode(&joined), p(ENDORSED, SECRET));
}

// 3 ───────────────────────────────────────────────────────────────────────────────

#[test]
fn an_unlabeled_answer_is_the_bottom_point() {
    assert!(encode(BOTTOM).is_empty());
    assert_eq!(decode(BTreeSet::<String>::new()), BOTTOM);
    // Every ceiling admits the bottom: a policy refuses nothing that carries no label.
    for ceiling in all_points() {
        assert!(admitted(&BTreeSet::new(), &refusals_for(ceiling)));
    }
}

// 4 ───────────────────────────────────────────────────────────────────────────────

#[test]
fn a_ceiling_spelled_as_refused_iris_admits_exactly_the_points_below_it() {
    for ceiling in all_points() {
        let refused = refusals_for(ceiling);
        assert!(refused.len() <= 2, "a ceiling is at most two IRIs");
        for point in all_points() {
            assert_eq!(
                admitted(&encode(point), &refused),
                flows_to(point, ceiling),
                "{point:?} against ceiling {ceiling:?}"
            );
        }
    }
    // The mail Sink of `provenance_labels.rs`: `to` requires integrity of at least
    // `endorsed`, at any confidentiality. That is one refused IRI.
    assert_eq!(
        refusals_for(p(ENDORSED, SECRET)),
        BTreeSet::from([below(ENDORSED)])
    );
}

// 5 ───────────────────────────────────────────────────────────────────────────────

#[test]
fn a_stray_threshold_iri_only_narrows() {
    for point in all_points() {
        for stray in (1..INTEGRITY.len())
            .map(below)
            .chain((0..CONFIDENTIALITY.len() - 1).map(above))
        {
            let mut labels = encode(point);
            labels.insert(stray.clone());
            let read = decode(&labels);
            // Never above where it was in integrity, never below in confidentiality.
            assert!(
                flows_to(point, read),
                "{point:?} + {stray} read as {read:?}"
            );
        }
    }
    // A set with a gap reads as its worst IRI: `below:trusted` absent, `below:endorsed`
    // present still reads as untrusted.
    assert_eq!(decode([below(ENDORSED)]), p(UNTRUSTED, PUBLIC));
}

// 6 ───────────────────────────────────────────────────────────────────────────────

fn text(body: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec())
}

/// An endpoint answering with the given lattice point, its IRIs riding as threads.
fn labeled_source(point: Point) -> FnEndpoint {
    FnEndpoint::new("labeled", move |_inv| {
        Ok(encode(point)
            .into_iter()
            .fold(text("value"), |repr, iri| repr.depends_on(iri.as_str())))
    })
}

/// Reads two sources and answers with neither's threads named: the kernel joins them.
struct Composite;

#[async_trait]
impl Endpoint for Composite {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        inv.source(&Iri::parse("urn:t:a").unwrap()).await?;
        inv.source(&Iri::parse("urn:t:b").unwrap()).await?;
        Ok(text("derived"))
    }
}

#[test]
fn todays_kernel_join_already_computes_the_lattice_join() {
    let cases = [
        (p(TRUSTED, SECRET), p(UNTRUSTED, PUBLIC)),
        (p(ENDORSED, PUBLIC), p(TRUSTED, INTERNAL)),
        (BOTTOM, BOTTOM),
    ];
    for (a, b) in cases {
        let space = EndpointSpace::new()
            .bind(Exact::new("urn:t:a"), labeled_source(a))
            .bind(Exact::new("urn:t:b"), labeled_source(b))
            .bind(Exact::new("urn:t:composite"), Composite);
        let kernel = Kernel::new(Arc::new(space));
        let answer = block_on(kernel.issue(
            Request::new(Verb::Source, Iri::parse("urn:t:composite").unwrap()),
            &Capability::root(),
        ))
        .unwrap();
        let read = decode(answer.threads().iter().map(|t| t.as_str()));
        assert_eq!(read, join(a, b), "{a:?} ⊔ {b:?} through the kernel");
    }
}
