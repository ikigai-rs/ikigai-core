//! **The run's byte budget** (ledger #2238, stage 2 of the streams design): one pool of the
//! bytes a resolution tree may buffer, minted from `urn:cap:stream:memory:<bytes>` under the
//! host's ceiling, drawn by every collect of a streamed body, refused with `OverBudget`
//! mid-stream too, and never a truncated answer.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::executor::block_on;
use ikigai_core::{
    cap_stream_memory, AsyncFnEndpoint, Body, ByteStream, Bytes, Capability, Endpoint,
    EndpointSpace, Error, Exact, FnEndpoint, Invocation, Iri, IssueContext, Kernel, Replay,
    ReprType, Representation, Request, Result, Verb,
};

fn source(target: &str) -> Request {
    Request::new(Verb::Source, Iri::parse(target).unwrap())
}

fn text() -> ReprType {
    ReprType::new("text/plain")
}

/// An endpoint answering a one-shot stream of `chunks`; its bytes face collects under the
/// run's pool, as the trait asks of a streaming endpoint.
struct Chunks(&'static [&'static [u8]]);

#[async_trait]
impl Endpoint for Chunks {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let budget = inv.memory_budget().expect("the kernel mints a byte pool");
        self.invoke_body(inv).await?.collect_within(budget).await
    }

    async fn invoke_body(&self, _inv: &Invocation<'_>) -> Result<Representation<Body>> {
        let chunks = self.0.iter().map(|c| Ok(Bytes::from_static(c)));
        Ok(Representation::from_body(
            text(),
            Body::once(futures::stream::iter(chunks)),
        ))
    }
}

/// Reads two streamed sub-bodies and buffers both, through `Invocation::collect`.
fn two_collects() -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("two", |inv| {
        Box::pin(async move {
            let mut out = Vec::new();
            for target in ["urn:a", "urn:b"] {
                let body = inv.source_body(&Iri::parse(target).unwrap()).await?.bytes;
                out.extend_from_slice(&inv.collect(body).await?);
            }
            Ok(Representation::new(text(), out))
        })
    })
}

fn budgeted(base: u64, ceiling: u64) -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:a"), Chunks(&[b"aa", b"a"]))
            .bind(Exact::new("urn:b"), Chunks(&[b"bbb"]))
            .bind(Exact::new("urn:mid"), Chunks(&[b"12", b"34", b"56"]))
            .bind(Exact::new("urn:two"), two_collects())
            .bind(
                Exact::new("urn:big-buffer"),
                FnEndpoint::new("big", |_| Ok(Representation::new(text(), vec![9u8; 100]))),
            ),
    ))
    .with_stream_memory(base, ceiling)
}

fn over_budget<T: std::fmt::Debug>(result: Result<T>) -> bool {
    matches!(result, Err(Error::OverBudget { .. }))
}

#[test]
fn a_stream_past_the_budget_is_refused_mid_stream_and_never_truncated() {
    let kernel = budgeted(5, 5);
    let err = block_on(kernel.issue(source("urn:mid"), &Capability::root())).unwrap_err();
    assert_eq!(
        err,
        Error::OverBudget {
            detail: "the body is larger than the run's byte budget: 1 byte(s) left, at least 6 needed (a `urn:cap:stream:memory:<bytes>` grant raises it, up to the host's ceiling)".into(),
        }
    );
    // Exactly enough is enough.
    let fits = budgeted(6, 6);
    assert_eq!(
        block_on(fits.issue(source("urn:mid"), &Capability::root()))
            .unwrap()
            .bytes,
        b"123456"
    );
}

#[test]
fn one_pool_serves_the_whole_resolution_tree() {
    // Each sub-body (3 bytes) fits alone; together they are 6, past a run of 5.
    let tight = budgeted(5, 5);
    assert!(over_budget(block_on(
        tight.issue(source("urn:two"), &Capability::root())
    )));
    let roomy = budgeted(6, 6);
    assert_eq!(
        block_on(roomy.issue(source("urn:two"), &Capability::root()))
            .unwrap()
            .bytes,
        b"aaabbb"
    );
}

#[test]
fn a_grant_raises_the_base_up_to_the_ceiling_and_root_gets_the_ceiling() {
    let kernel = budgeted(2, 6);
    let plain = Capability::scoped(["urn:cap:read"]);
    assert!(over_budget(block_on(kernel.issue(source("urn:a"), &plain))));
    let granted = Capability::scoped([cap_stream_memory(3)]);
    assert_eq!(
        block_on(kernel.issue(source("urn:a"), &granted))
            .unwrap()
            .bytes,
        b"aaa"
    );
    // A grant above the ceiling is clamped to it: 6 is not 7.
    let lavish = Capability::scoped([cap_stream_memory(1_000)]);
    assert_eq!(
        block_on(kernel.issue(source("urn:mid"), &lavish))
            .unwrap()
            .bytes,
        b"123456"
    );
    assert_eq!(
        block_on(kernel.issue(source("urn:two"), &Capability::root()))
            .unwrap()
            .bytes,
        b"aaabbb"
    );
}

#[test]
fn a_door_can_lower_a_runs_budget_and_never_raise_it() {
    let kernel = budgeted(64, 64);
    let root = Capability::root();
    let lowered = IssueContext::new().with_memory_limit(5);
    assert!(over_budget(block_on(kernel.issue_with_context(
        source("urn:two"),
        &root,
        lowered
    ))));
    // Asking for more than the capability's entitlement gets the entitlement.
    let small = kernel_with_ceiling(4);
    let raised = IssueContext::new().with_memory_limit(1_000);
    assert!(over_budget(block_on(small.issue_with_context(
        source("urn:mid"),
        &root,
        raised
    ))));
}

fn kernel_with_ceiling(ceiling: u64) -> Kernel {
    budgeted(ceiling, ceiling)
}

#[test]
fn a_buffer_an_endpoint_already_built_draws_nothing() {
    // 100 bytes against a 1-byte budget: already allocated, so handed over as before
    // bodies existed (0.1.112 answered it; refusing it after the fact frees nothing).
    let kernel = budgeted(1, 1);
    assert_eq!(
        block_on(kernel.issue(source("urn:big-buffer"), &Capability::root()))
            .unwrap()
            .bytes
            .len(),
        100
    );
}

/// A replay that counts its opens.
struct Stored {
    opened: Arc<AtomicUsize>,
}

impl Replay for Stored {
    fn open(&self) -> ByteStream {
        self.opened.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::iter([Ok(Bytes::from_static(b"stored!"))]))
    }

    fn len_hint(&self) -> Option<u64> {
        Some(7)
    }
}

#[test]
fn a_cached_replay_is_collected_under_the_callers_budget_on_a_hit() {
    let opened = Arc::new(AtomicUsize::new(0));
    let replay_opened = Arc::clone(&opened);
    struct Replayer(Arc<AtomicUsize>);
    #[async_trait]
    impl Endpoint for Replayer {
        async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
            let body = self.invoke_body(inv).await?;
            body.collect_within(inv.memory_budget().unwrap()).await
        }
        async fn invoke_body(&self, _inv: &Invocation<'_>) -> Result<Representation<Body>> {
            Ok(Representation::from_body(
                text(),
                Body::replay(Stored {
                    opened: Arc::clone(&self.0),
                }),
            )
            .cacheable())
        }
    }
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:stored"), Replayer(replay_opened)),
    ))
    .with_stream_memory(3, 64);
    // Root stores it (and reads it, 7 bytes, under its 64).
    let root = Capability::root();
    assert_eq!(
        block_on(kernel.issue(source("urn:stored"), &root))
            .unwrap()
            .bytes,
        b"stored!"
    );
    // The same caller under a door's lowered run hits the SAME entry (the cache is keyed
    // by capability, not by run). The hit is a replay, and collecting it draws.
    let lowered = IssueContext::new().with_memory_limit(3);
    assert!(over_budget(block_on(kernel.issue_with_context(
        source("urn:stored"),
        &root,
        lowered
    ))));
    // Refused from its length hint, before it was opened again.
    assert_eq!(opened.load(Ordering::SeqCst), 1);
    assert!(kernel.is_cached(&source("urn:stored"), &root));
}
