//! **The cache decides admission BEFORE it copies** (ledger #1734). The kernel used to
//! clone every cacheable answer into the cache's store and only then ask the policy
//! whether to keep it, so an answer over the byte budget was allocated twice and the
//! copy thrown away: the store dump at 1.5M quads peaked at 832 MB cacheable against
//! 509 MB bare (ledger #1732). These pin the number of payload-sized allocations one
//! read makes, with a counting allocator: one for an answer the cache refuses (the
//! endpoint's own), two for one it keeps (the endpoint's and the cache's copy).
//!
//! The allocator counts only on a thread that switched it on, and only allocations of
//! at least [`PAYLOAD`] bytes, so the other tests of this binary running in parallel
//! cannot move the count.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_core::{
    CacheBound, CachePolicy, Capability, EndpointSpace, EntryFacts, Exact, FnEndpoint, Iri, Kernel,
    Lru, ReprType, Representation, Request, Verb,
};

/// The payload size: four times the byte budget the tests give the cache.
const PAYLOAD: usize = 4 << 20;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static LARGE: Cell<usize> = const { Cell::new(0) };
}

struct CountLarge;

// SAFETY: every call is forwarded unchanged to `System`; the bookkeeping only reads
// and writes const-initialized thread-locals, which never allocate.
unsafe impl GlobalAlloc for CountLarge {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn note(size: usize) {
    if size >= PAYLOAD && COUNTING.try_with(Cell::get).unwrap_or(false) {
        let _ = LARGE.try_with(|n| n.set(n.get() + 1));
    }
}

#[global_allocator]
static ALLOCATOR: CountLarge = CountLarge;

/// Issue `request` on this thread and count the payload-sized allocations it made.
/// `block_on` polls on the calling thread and the endpoint is synchronous, so every
/// allocation of the read lands here.
fn large_allocations(kernel: &Kernel, request: Request) -> (usize, Representation) {
    LARGE.with(|n| n.set(0));
    COUNTING.with(|c| c.set(true));
    let answer = block_on(kernel.issue(request, &Capability::root()));
    COUNTING.with(|c| c.set(false));
    (LARGE.with(Cell::get), answer.unwrap())
}

fn request() -> Request {
    Request::new(Verb::Source, Iri::parse("urn:big").unwrap())
}

/// A kernel with one cacheable endpoint answering [`PAYLOAD`] bytes, under `policy`.
fn kernel(policy: Arc<dyn CachePolicy>) -> Kernel {
    let big = FnEndpoint::new("big", |_| {
        Ok(Representation::new(
            ReprType::new("application/octet-stream"),
            vec![7u8; PAYLOAD],
        )
        .cacheable())
    });
    Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:big"), big),
    ))
    .with_cache_policy(policy)
}

#[test]
fn an_answer_over_the_byte_budget_is_not_copied() {
    // A budget a quarter of the answer: the default `admit` refuses it.
    let kernel = kernel(Arc::new(Lru::with_bound(CacheBound::new(16, PAYLOAD / 4))));
    let (large, answer) = large_allocations(&kernel, request());
    assert_eq!(answer.bytes.len(), PAYLOAD);
    assert!(!kernel.is_cached(&request(), &Capability::root()));
    assert_eq!(
        large, 1,
        "only the endpoint's own payload: a refused answer is never copied"
    );
}

#[test]
fn an_answer_a_policy_refuses_is_not_copied() {
    struct AdmitNothing;
    impl CachePolicy for AdmitNothing {
        fn admit(&self, _: &EntryFacts<'_>) -> bool {
            false
        }
        fn victim(&self, _: &[EntryFacts<'_>]) -> Option<usize> {
            None
        }
    }
    let kernel = kernel(Arc::new(AdmitNothing));
    let (large, _) = large_allocations(&kernel, request());
    assert!(!kernel.is_cached(&request(), &Capability::root()));
    assert_eq!(large, 1, "a policy's refusal costs no copy either");
}

#[test]
fn an_admitted_answer_is_copied_once_and_served_from_the_cache() {
    let kernel = kernel(Arc::new(Lru::with_bound(CacheBound::new(16, PAYLOAD * 2))));
    let (large, _) = large_allocations(&kernel, request());
    assert!(kernel.is_cached(&request(), &Capability::root()));
    assert_eq!(
        large, 2,
        "the endpoint's payload and the one copy the cache keeps"
    );
    // A hit hands back a copy of the entry and computes nothing.
    let (large, answer) = large_allocations(&kernel, request());
    assert_eq!(answer.bytes.len(), PAYLOAD);
    assert_eq!(large, 1, "a hit clones the entry once");
}
