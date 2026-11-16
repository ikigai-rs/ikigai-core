//! **A cached result is reused only where its depth still fits the budget** (ledger
//! #3792). The nesting budget (`Kernel::with_max_depth`) and the representation cache
//! used not to know about each other: a result whose computation went `k` levels deep,
//! cached near the top, was served to the same request issued at depth `n` with
//! `n + k > max_depth`, where a fresh evaluation answers `DepthExceeded`. The cache must
//! be transparent: a hit answers exactly what a fresh evaluation at that point would.
//!
//! The kernel now records with each entry the depth its computation consumed (δ: the
//! deepest level any part of it reached, minus its own start depth, counting a
//! sub-request that was itself a hit by that entry's δ) and serves it at depth `n` only
//! when `n + δ ≤ max_depth`. What these pin: the reproduction (a deep reuse that does
//! not fit is not served, and the fresh refusal comes back); the boundary (exactly
//! `n + δ = max_depth` IS served); δ through a hit inside a computation; a refusal is
//! still never cached; and a seeded loop over random chains and start depths in which
//! the cached and the fresh answers always agree.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Capability, EndpointSpace, Error, Exact, Iri, Kernel, ReprType,
    Representation, Request, Verb,
};

const MAX: u32 = 6;

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

/// What a request answered, in a form two kernels' answers compare by.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Value(Vec<u8>),
    Refused { depth: u32, target: String },
    Other(String),
}

fn outcome(result: ikigai_core::Result<Representation>) -> Outcome {
    match result {
        Ok(r) => Outcome::Value(r.bytes),
        Err(Error::DepthExceeded { depth, target }) => Outcome::Refused {
            depth,
            target: target.as_str().to_string(),
        },
        Err(other) => Outcome::Other(other.to_string()),
    }
}

/// `urn:chain:{k}` sources `urn:chain:{k-1}`; `urn:chain:0` answers a cacheable leaf.
/// So `urn:chain:{k}` issued at depth `n` reaches depth `n + k`: δ = k.
fn chains(mut space: EndpointSpace, deepest: u32, runs: &Arc<AtomicUsize>) -> EndpointSpace {
    for k in 0..=deepest {
        let runs = Arc::clone(runs);
        space = space.bind(
            Exact::new(format!("urn:chain:{k}")),
            AsyncFnEndpoint::new("chain", move |inv| {
                let runs = Arc::clone(&runs);
                Box::pin(async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    if k == 0 {
                        Ok(
                            Representation::new(ReprType::new("text/plain"), b"leaf".to_vec())
                                .cacheable(),
                        )
                    } else {
                        let inner = inv.source(&iri(&format!("urn:chain:{}", k - 1))).await?;
                        let mut bytes = format!("{k}<").into_bytes();
                        bytes.extend_from_slice(&inner.bytes);
                        Ok(Representation::new(ReprType::new("text/plain"), bytes).cacheable())
                    }
                })
            }),
        );
    }
    space
}

/// `urn:pad:{tag}:{d}` sources `urn:pad:{tag}:{d-1}`, and `urn:pad:{tag}:1` sources
/// `target`, relaying its answer: issued at depth 0, `urn:pad:{tag}:{d}` issues `target`
/// at depth `d`. Never cacheable itself, so every issue of it reaches `target` afresh.
fn pads(mut space: EndpointSpace, tag: &str, d: u32, target: &str) -> EndpointSpace {
    for i in 1..=d {
        let next = if i == 1 {
            target.to_string()
        } else {
            format!("urn:pad:{tag}:{}", i - 1)
        };
        space = space.bind(
            Exact::new(format!("urn:pad:{tag}:{i}")),
            AsyncFnEndpoint::new("pad", move |inv| {
                let next = next.clone();
                Box::pin(async move {
                    let inner = inv.source(&iri(&next)).await?;
                    Ok(Representation::new(inner.repr_type, inner.bytes))
                })
            }),
        );
    }
    space
}

fn source(kernel: &Kernel, target: &str) -> Outcome {
    outcome(block_on(kernel.issue(
        Request::new(Verb::Source, iri(target)),
        &Capability::root(),
    )))
}

/// The reproduction: `urn:chain:3` (δ = 3) cached at depth 0, then issued at depth 4,
/// where 4 + 3 > 6. A fresh evaluation is refused at depth 7; so must the warmed one be.
#[test]
fn a_deep_reuse_that_does_not_fit_is_not_served() {
    let runs = Arc::new(AtomicUsize::new(0));
    let space = || {
        pads(
            chains(EndpointSpace::new(), 3, &runs),
            "a",
            4,
            "urn:chain:3",
        )
    };
    let warmed = Kernel::new(Arc::new(space())).with_max_depth(MAX);
    assert_eq!(
        source(&warmed, "urn:chain:3"),
        Outcome::Value(b"3<2<1<leaf".to_vec())
    );
    let fresh = Kernel::new(Arc::new(space())).with_max_depth(MAX);
    let expected = source(&fresh, "urn:pad:a:4");
    assert_eq!(
        expected,
        Outcome::Refused {
            depth: 7,
            target: "urn:chain:0".into()
        }
    );
    assert_eq!(source(&warmed, "urn:pad:a:4"), expected);
}

/// The boundary: issued at depth 3, 3 + 3 = 6 = max_depth, so the entry IS reused and
/// nothing below it recomputes.
#[test]
fn an_entry_that_exactly_fits_is_reused() {
    let runs = Arc::new(AtomicUsize::new(0));
    let space = pads(
        chains(EndpointSpace::new(), 3, &runs),
        "b",
        3,
        "urn:chain:3",
    );
    let kernel = Kernel::new(Arc::new(space)).with_max_depth(MAX);
    assert_eq!(
        source(&kernel, "urn:chain:3"),
        Outcome::Value(b"3<2<1<leaf".to_vec())
    );
    assert_eq!(runs.load(Ordering::SeqCst), 4);
    assert_eq!(
        source(&kernel, "urn:pad:b:3"),
        Outcome::Value(b"3<2<1<leaf".to_vec())
    );
    assert_eq!(
        runs.load(Ordering::SeqCst),
        4,
        "the chain was served from the cache"
    );
}

/// δ through a hit: `urn:chain:3` computed while `urn:chain:2` was already cached
/// (δ = 2) records δ = 3, not the 1 it measured itself. Issued at depth 4, 4 + 3 > 6:
/// not served, and the fresh refusal comes back. Recording only its own level would have
/// let it through (4 + 1 ≤ 6).
#[test]
fn depth_consumed_propagates_through_a_hit_inside_a_computation() {
    let runs = Arc::new(AtomicUsize::new(0));
    let space = || {
        pads(
            chains(EndpointSpace::new(), 3, &runs),
            "c",
            4,
            "urn:chain:3",
        )
    };
    let warmed = Kernel::new(Arc::new(space())).with_max_depth(MAX);
    source(&warmed, "urn:chain:2");
    let before = runs.load(Ordering::SeqCst);
    assert_eq!(
        source(&warmed, "urn:chain:3"),
        Outcome::Value(b"3<2<1<leaf".to_vec())
    );
    assert_eq!(
        runs.load(Ordering::SeqCst),
        before + 1,
        "chain:3 ran, its chain:2 was a hit"
    );
    let fresh = Kernel::new(Arc::new(space())).with_max_depth(MAX);
    let expected = source(&fresh, "urn:pad:c:4");
    assert!(matches!(expected, Outcome::Refused { .. }), "{expected:?}");
    assert_eq!(source(&warmed, "urn:pad:c:4"), expected);
}

/// A refusal is still never cached: `urn:chain:3` refused under the pad, then issued at
/// the top, computes and answers its value rather than a cached refusal.
#[test]
fn a_depth_refusal_is_still_never_cached() {
    let runs = Arc::new(AtomicUsize::new(0));
    let space = pads(
        chains(EndpointSpace::new(), 3, &runs),
        "d",
        4,
        "urn:chain:3",
    );
    let kernel = Kernel::new(Arc::new(space)).with_max_depth(MAX);
    assert!(matches!(
        source(&kernel, "urn:pad:d:4"),
        Outcome::Refused { depth: 7, .. }
    ));
    assert_eq!(
        source(&kernel, "urn:chain:3"),
        Outcome::Value(b"3<2<1<leaf".to_vec())
    );
}

/// A small seeded loop (the crate uses neither proptest nor quickcheck): random warm-ups
/// and random start depths over chains of up to 5; the warmed kernel's answer always
/// equals a fresh kernel's.
#[test]
fn cached_and_fresh_answers_always_agree() {
    let mut seed: u64 = 0x3792_5eed;
    let mut next = |bound: u32| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        u32::try_from(seed % u64::from(bound)).unwrap()
    };
    let runs = Arc::new(AtomicUsize::new(0));
    let mut space = chains(EndpointSpace::new(), 5, &runs);
    for d in 1..=MAX + 1 {
        for k in 0..=5 {
            space = pads(space, &format!("{d}x{k}"), d, &format!("urn:chain:{k}"));
        }
    }
    let space = Arc::new(space);
    let name = |d: u32, k: u32| {
        if d == 0 {
            format!("urn:chain:{k}")
        } else {
            format!("urn:pad:{d}x{k}:{d}")
        }
    };
    for _ in 0..40 {
        let warmed = Kernel::new(space.clone()).with_max_depth(MAX);
        for _ in 0..next(4) + 1 {
            let (d, k) = (next(MAX + 2), next(6));
            source(&warmed, &name(d, k));
        }
        for _ in 0..6 {
            let (d, k) = (next(MAX + 2), next(6));
            let fresh = Kernel::new(space.clone()).with_max_depth(MAX);
            assert_eq!(
                source(&warmed, &name(d, k)),
                source(&fresh, &name(d, k)),
                "chain {k} issued at depth {d}"
            );
        }
    }
}
