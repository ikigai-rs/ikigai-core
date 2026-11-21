//! **A result built on an admission refusal is never cached, at any depth** (ledger
//! #3917, beside the admission-only rule of #3869).
//!
//! A sub-request that fails with anything but a miss makes the invocation that caught it
//! uncacheable (the `failed <iri>` clause of the uncached note), and `DepthExceeded`,
//! `OverBudget`, `Timeout` and `GaveUp` are never cached themselves, even with a stated
//! expiry. So a computation in which any request was refused for admission is never kept:
//! not the frame that caught the refusal, and not a frame above it, which inherits the
//! volatility through its dependency record. That is stricter than "reused only at its own
//! depth and run context", and it is what these pin:
//!
//! 1. A self-requesting endpoint that catches `DepthExceeded` one level too deep and
//!    answers a CACHEABLE count, read at depth 0 and from a composite at depth 1: each
//!    answer reflects its own depth, and the cache stays empty.
//! 2. A composite that catches an `OverBudget` or a `Timeout` from a sub-request and
//!    returns a cacheable fallback is not cached, and `urn:kernel:uncached` names the
//!    failed sub-request.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::executor::block_on;
use ikigai_core::{
    ArgRef, AsyncFnEndpoint, Body, Bytes, Capability, Clock, Endpoint, EndpointSpace, Error, Exact,
    Invocation, Iri, Kernel, ReprType, Representation, Request, Result, Time, TraceEvent, Tracer,
    Verb, UNCACHED_NOTE,
};

const MAX: u32 = 4;
const START: u64 = 1_000;

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(body: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec())
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn runs(counter: &Arc<AtomicU32>) -> u32 {
    counter.load(Ordering::SeqCst)
}

/// `urn:kernel:uncached` with `target=`: the rows the kernel kept for one name.
fn uncached_rows(kernel: &Kernel, target: &str) -> String {
    let request = source("urn:kernel:uncached").with_arg("target", ArgRef::Inline(target.into()));
    let body = block_on(kernel.issue(request, &Capability::root()))
        .unwrap()
        .bytes;
    String::from_utf8(body).unwrap()
}

#[derive(Default)]
struct Collect(Mutex<Vec<TraceEvent>>);
impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// The kernel's clock, fixed for the test.
struct At(AtomicU64);
impl Clock for At {
    fn now(&self) -> Time {
        Time::from_millis(self.0.load(Ordering::SeqCst))
    }
}

// 1 ─────────────────────────────────────────────────────────────────────────────

/// `urn:count` sources itself. The request one level past the bound is refused with
/// `DepthExceeded`; the frame that issued it catches the refusal and answers `0`, and
/// every frame above answers one more than the frame below, each CACHEABLE. So the
/// answer at depth `n` is `MAX - n`: it reflects its own depth.
fn counting(calls: &Arc<AtomicU32>) -> AsyncFnEndpoint {
    let calls = Arc::clone(calls);
    AsyncFnEndpoint::new("count", move |inv| {
        let calls = Arc::clone(&calls);
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            let below = match inv.source(&iri("urn:count")).await {
                Ok(inner) => {
                    String::from_utf8(inner.bytes)
                        .unwrap()
                        .parse::<u32>()
                        .unwrap()
                        + 1
                }
                Err(Error::DepthExceeded { .. }) => 0,
                Err(other) => return Err(other),
            };
            Ok(text(&below.to_string()).cacheable())
        })
    })
}

/// A cacheable composite that relays one sub-request: what it reads, it reads at depth 1.
fn relay(target: &'static str) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("relay", move |inv| {
        Box::pin(async move {
            let inner = inv.source(&iri(target)).await?;
            Ok(Representation::new(inner.repr_type, inner.bytes).cacheable())
        })
    })
}

#[test]
fn a_counting_endpoint_that_caught_a_depth_refusal_is_never_cached_at_any_depth() {
    let calls = Arc::new(AtomicU32::new(0));
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:count"), counting(&calls))
            .bind(Exact::new("urn:wrap"), relay("urn:count")),
    ))
    .with_max_depth(MAX);
    let root = Capability::root();
    let read = |target: &str| {
        String::from_utf8(block_on(kernel.issue(source(target), &root)).unwrap().bytes).unwrap()
    };

    // Depth 0 reaches depths 0..=MAX, and the request at MAX + 1 is refused.
    assert_eq!(read("urn:count"), MAX.to_string());
    assert_eq!(runs(&calls), MAX + 1);
    assert_eq!(kernel.cache_len(), 0, "nothing on the refused path is kept");

    // Under the same capability, from depth 1: its own count, not depth 0's.
    assert_eq!(read("urn:wrap"), (MAX - 1).to_string());
    assert_eq!(
        runs(&calls),
        MAX + 1 + MAX,
        "computed afresh, not served depth 0's"
    );
    assert_eq!(
        kernel.cache_len(),
        0,
        "the composite above inherits the volatility"
    );

    // And again: every frame recomputes, at either depth.
    assert_eq!(read("urn:count"), MAX.to_string());
    assert_eq!(read("urn:wrap"), (MAX - 1).to_string());
    assert_eq!(runs(&calls), 2 * (MAX + 1 + MAX));
    assert_eq!(kernel.cache_len(), 0);

    // Why, frame by frame: the one that caught the refusal names the failed request, and
    // every frame above it names the volatile one below.
    let collect = Arc::new(Collect::default());
    block_on(kernel.issue_traced(source("urn:count"), &root, collect.clone())).unwrap();
    let notes: Vec<String> = collect
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e.target == "urn:count")
        .filter_map(|e| {
            e.notes
                .iter()
                .find(|(k, _)| k == UNCACHED_NOTE)
                .map(|(_, v)| v.clone())
        })
        .collect();
    assert_eq!(
        notes.len() as u32,
        MAX + 1,
        "every frame says why: {notes:?}"
    );
    assert_eq!(
        notes.iter().filter(|n| *n == "failed urn:count").count(),
        1,
        "{notes:?}"
    );
    assert_eq!(
        notes
            .iter()
            .filter(|n| *n == "dependency urn:count")
            .count() as u32,
        MAX,
        "{notes:?}"
    );
}

// 2 ─────────────────────────────────────────────────────────────────────────────

/// A cacheable composite that sources `target` through `narrow` (a reborrow that runs the
/// run context out), and answers a cacheable fallback when the sub-request is refused
/// with an error `refused` accepts.
fn catching(
    target: &'static str,
    narrow: for<'a, 'b> fn(&'a Invocation<'b>) -> Invocation<'a>,
    refused: fn(&Error) -> bool,
    caught: &Arc<AtomicU32>,
) -> AsyncFnEndpoint {
    let caught = Arc::clone(caught);
    AsyncFnEndpoint::new("catching", move |inv| {
        let caught = Arc::clone(&caught);
        Box::pin(async move {
            match narrow(inv).source(&iri(target)).await {
                Ok(r) => Ok(Representation::new(r.repr_type, r.bytes).cacheable()),
                Err(e) if refused(&e) => {
                    caught.fetch_add(1, Ordering::SeqCst);
                    Ok(text("fallback").cacheable())
                }
                Err(e) => Err(e),
            }
        })
    })
}

/// Six bytes in a one-shot stream, collected under the run's byte pool.
struct SixBytes;

#[async_trait]
impl Endpoint for SixBytes {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let budget = inv.memory_budget().expect("the kernel mints a byte pool");
        self.invoke_body(inv).await?.collect_within(budget).await
    }

    async fn invoke_body(&self, _inv: &Invocation<'_>) -> Result<Representation<Body>> {
        let chunks = [b"123".as_slice(), b"456"].map(|c| Ok(Bytes::from_static(c)));
        Ok(Representation::from_body(
            ReprType::new("text/plain"),
            Body::once(futures::stream::iter(chunks)),
        ))
    }
}

#[test]
fn a_composite_that_caught_an_over_budget_sub_request_is_not_cached() {
    let caught = Arc::new(AtomicU32::new(0));
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:big"), SixBytes)
            .bind(
                Exact::new("urn:summary"),
                catching(
                    "urn:big",
                    |inv| inv.with_memory_limit(2),
                    |e| matches!(e, Error::OverBudget { .. }),
                    &caught,
                ),
            ),
    ))
    .with_stream_memory(64, 64);
    let root = Capability::root();
    for _ in 0..2 {
        let answer = block_on(kernel.issue(source("urn:summary"), &root)).unwrap();
        assert_eq!(answer.bytes, b"fallback");
    }
    assert_eq!(
        runs(&caught),
        2,
        "the fallback was computed again, not served"
    );
    assert_eq!(kernel.cache_len(), 0);
    let rows = uncached_rows(&kernel, "urn:summary");
    assert!(rows.contains("failed urn:big"), "{rows}");
}

#[test]
fn a_composite_that_caught_a_timed_out_sub_request_is_not_cached() {
    let caught = Arc::new(AtomicU32::new(0));
    // The sub-request refuses once its deadline has passed, and states an expiry that a
    // failure of any other kind would be cached for (#2657): a `Timeout` never is (#3869).
    let slow = AsyncFnEndpoint::new("slow", |inv| {
        Box::pin(async move {
            match inv.time_left() {
                Some(left) if left.is_zero() => {
                    inv.failure_expires_in(Duration::from_secs(60));
                    Err(Error::Timeout("the run's deadline passed".into()))
                }
                _ => Ok(text("done").cacheable()),
            }
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:slow"), slow)
            .bind(
                Exact::new("urn:report"),
                catching(
                    "urn:slow",
                    |inv| inv.with_not_after(Time::from_millis(START)),
                    |e| matches!(e, Error::Timeout(_)),
                    &caught,
                ),
            ),
    ))
    .with_clock(Arc::new(At(AtomicU64::new(START))));
    let root = Capability::root();
    for _ in 0..2 {
        let answer = block_on(kernel.issue(source("urn:report"), &root)).unwrap();
        assert_eq!(answer.bytes, b"fallback");
    }
    assert_eq!(
        runs(&caught),
        2,
        "the fallback was computed again, not served"
    );
    assert_eq!(kernel.cache_len(), 0);
    let rows = uncached_rows(&kernel, "urn:report");
    assert!(rows.contains("failed urn:slow"), "{rows}");
}
