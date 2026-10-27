//! **A scoped draw on the run's byte budget** (ledger #2381): `MemoryBudget::reserve` holds
//! bytes for as long as its `Reservation` lives and gives them back when it drops, so a
//! step holding one window of a stream at a time (a line tool, a tail, a parser buffer)
//! bounds each window by the budget without draining the pool over a long stream, and an
//! early return or a dropped stream cannot leak what it held.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::executor::block_on;
use futures::StreamExt;
use ikigai_core::{
    AsyncFnEndpoint, Body, Bytes, Capability, Endpoint, EndpointSpace, Error, Exact, FnEndpoint,
    Invocation, Iri, IssueContext, Kernel, MemoryBudget, ReprType, Representation, Request, Result,
    Verb,
};

fn source(target: &str) -> Request {
    Request::new(Verb::Source, Iri::parse(target).unwrap())
}

fn text() -> ReprType {
    ReprType::new("text/plain")
}

const LINES: usize = 10_000;
const LINE: &[u8] = b"fifteen bytes!\n";

/// A one-shot stream of `LINES` lines, one chunk each, counting the chunks polled from it:
/// 150 000 bytes, never held whole by anything below.
struct Lines(Arc<AtomicUsize>);

#[async_trait]
impl Endpoint for Lines {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let budget = inv.memory_budget().expect("the kernel mints a byte pool");
        self.invoke_body(inv).await?.collect_within(budget).await
    }

    async fn invoke_body(&self, _inv: &Invocation<'_>) -> Result<Representation<Body>> {
        let polled = Arc::clone(&self.0);
        let chunks = futures::stream::iter(0..LINES).map(move |_| {
            polled.fetch_add(1, Ordering::SeqCst);
            Ok(Bytes::from_static(LINE))
        });
        Ok(Representation::from_body(text(), Body::once(chunks)))
    }
}

/// How a line tool holds each line it reads: a reservation for its length, kept while
/// the line is held, or a permanent draw (what 0.2.1 allowed outside the crate).
#[derive(Clone, Copy)]
enum Hold {
    Reserve,
    Draw,
}

/// A line tool: reads `urn:lines` as a stream and holds one line at a time under the
/// run's pool, answering how many lines it saw. Stops early, with an error, at line
/// `stop_at` when it is set.
fn line_tool(hold: Hold, stop_at: Option<usize>) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("count-lines", move |inv| {
        Box::pin(async move {
            let budget: MemoryBudget = inv.memory_budget().expect("a pool").clone();
            let body = inv
                .source_body(&Iri::parse("urn:lines").unwrap())
                .await?
                .bytes;
            let mut stream = body.into_stream();
            let mut seen = 0usize;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                let len = chunk.len() as u64;
                let _held = match hold {
                    Hold::Reserve => Some(budget.reserve(len)?),
                    Hold::Draw => {
                        if !budget.try_draw(len) {
                            return Err(Error::OverBudget {
                                detail: "drained".into(),
                            });
                        }
                        None
                    }
                };
                seen += 1;
                if stop_at == Some(seen) {
                    return Err(Error::Endpoint("stopped early".into()));
                }
            }
            Ok(Representation::new(text(), seen.to_string().into_bytes()))
        })
    })
}

fn kernel(hold: Hold, stop_at: Option<usize>, polled: Arc<AtomicUsize>) -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:lines"), Lines(polled))
            .bind(Exact::new("urn:count"), line_tool(hold, stop_at)),
    ))
    .with_stream_memory(1 << 20, 1 << 20)
}

/// A run whose pool holds `bytes`, and a handle on that pool to read it afterwards.
fn run(bytes: u64) -> (IssueContext, MemoryBudget) {
    let run = IssueContext::new().with_memory_limit(bytes);
    let pool = run.memory_budget().unwrap().clone();
    (run, pool)
}

#[test]
fn a_long_stream_holding_one_window_at_a_time_never_drains_the_pool() {
    let polled = Arc::new(AtomicUsize::new(0));
    let kernel = kernel(Hold::Reserve, None, Arc::clone(&polled));
    // 64 bytes for 150 000 streamed: room for one line, never the whole.
    let (ctx, pool) = run(64);
    let answer =
        block_on(kernel.issue_with_context(source("urn:count"), &Capability::root(), ctx)).unwrap();
    assert_eq!(answer.bytes, LINES.to_string().as_bytes());
    assert_eq!(polled.load(Ordering::SeqCst), LINES);
    assert_eq!(pool.remaining(), 64, "every line's bytes came back");
}

#[test]
fn the_same_tool_drawing_without_giving_back_drains_the_pool() {
    // The defect the reservation removes: a permanent draw per line spends the pool for
    // good, so a stream of more than 64 bytes is refused though no more than 15 are held.
    let polled = Arc::new(AtomicUsize::new(0));
    let kernel = kernel(Hold::Draw, None, Arc::clone(&polled));
    let (ctx, pool) = run(64);
    let result = block_on(kernel.issue_with_context(source("urn:count"), &Capability::root(), ctx));
    assert!(
        matches!(result, Err(Error::OverBudget { .. })),
        "{result:?}"
    );
    assert_eq!(
        polled.load(Ordering::SeqCst),
        5,
        "64 / 15 lines, then refused"
    );
    assert!(pool.remaining() < LINE.len() as u64);
}

#[test]
fn an_early_return_gives_the_held_bytes_back() {
    let polled = Arc::new(AtomicUsize::new(0));
    let kernel = kernel(Hold::Reserve, Some(3), Arc::clone(&polled));
    let (ctx, pool) = run(64);
    let result = block_on(kernel.issue_with_context(source("urn:count"), &Capability::root(), ctx));
    assert!(matches!(result, Err(Error::Endpoint(_))), "{result:?}");
    assert_eq!(polled.load(Ordering::SeqCst), 3, "the stream stopped early");
    assert_eq!(
        pool.remaining(),
        64,
        "the line held at the return came back"
    );
}

#[test]
fn a_dropped_reservation_returns_its_bytes_and_one_past_the_remainder_is_over_budget() {
    let (_ctx, pool) = run(100);
    let held = pool.reserve(60).unwrap();
    assert_eq!(pool.remaining(), 40);
    // Past what remains: OverBudget, and nothing drawn.
    match pool.reserve(41) {
        Err(Error::OverBudget { detail }) => {
            assert!(detail.contains("urn:cap:stream:memory"), "{detail}")
        }
        other => panic!("expected OverBudget, got {other:?}"),
    }
    assert_eq!(pool.remaining(), 40);
    let exactly = pool.reserve(40).unwrap();
    assert_eq!(pool.remaining(), 0);
    drop(held);
    drop(exactly);
    assert_eq!(pool.remaining(), 100);
}

#[test]
fn a_reservation_in_a_dropped_stream_gives_its_bytes_back() {
    // A window held by a stream adapter, dropped by its reader before the end.
    let (_ctx, pool) = run(32);
    let budget = pool.clone();
    let windows = futures::stream::iter(0..1_000).map(move |_| budget.reserve(16));
    let mut windows = Box::pin(windows);
    let first = block_on(windows.next()).unwrap().unwrap();
    let second = block_on(windows.next()).unwrap().unwrap();
    assert!(matches!(
        block_on(windows.next()),
        Some(Err(Error::OverBudget { .. }))
    ));
    assert_eq!(pool.remaining(), 0);
    drop(windows);
    drop((first, second));
    assert_eq!(pool.remaining(), 32);
}

#[test]
fn a_child_pool_reservation_draws_from_and_returns_to_every_pool_above() {
    // A capability entitled to less than the run gets a child pool; a reservation there
    // is held in the run's pool too, and given back to both.
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::clone(&observed);
    let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:window"),
        FnEndpoint::new("window", move |inv| {
            let budget = inv.memory_budget().unwrap();
            let held = budget.reserve(10)?;
            seen.lock().unwrap().push(budget.remaining());
            drop(held);
            seen.lock().unwrap().push(budget.remaining());
            Ok(Representation::new(text(), b"ok".to_vec()))
        }),
    )))
    .with_stream_memory(30, 30);
    let (ctx, pool) = run(100);
    let child = Capability::scoped(["urn:cap:read"]);
    block_on(kernel.issue_with_context(source("urn:window"), &child, ctx)).unwrap();
    assert_eq!(*observed.lock().unwrap(), vec![20, 30]);
    assert_eq!(pool.remaining(), 100);
}

#[test]
fn a_kept_reservation_stays_drawn_as_a_draw_does() {
    let (_ctx, pool) = run(10);
    let mut held = pool.reserve(3).unwrap();
    held.try_grow(2).unwrap();
    assert!(matches!(held.try_grow(6), Err(Error::OverBudget { .. })));
    assert_eq!(held.bytes(), 5);
    held.shrink(100);
    assert_eq!((held.bytes(), pool.remaining()), (0, 10));
    held.try_grow(4).unwrap();
    held.keep();
    assert_eq!(pool.remaining(), 6);
}

#[test]
fn a_reservation_is_send_and_sync() {
    fn both<T: Send + Sync>() {}
    both::<ikigai_core::Reservation>();
}
