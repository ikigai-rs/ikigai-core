//! **Cut listeners** (NetKernel News 7.5, 7.6): a host is told when a golden thread
//! is cut and what the cut invalidated, through a bounded queue it drains or awaits.
//! What these pin: a Sink is heard as exactly the threads it cut; the "recompute
//! before the first reader" pattern works over it with nothing but public API; a
//! listener learns only the names its own reads rest on; a full queue counts what it
//! dropped; a waiting host is woken; and a cut nobody listens for is not heard.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, AsyncFnEndpoint, Capability, CutEvent, EndpointSpace, Error, Exact, FnEndpoint, Iri,
    Kernel, ListenSpec, ReprType, Representation, Request, Verb, CAP_LISTEN, INVALIDATED_NAMED,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn sink(target: &str, content: &str) -> Request {
    Request::new(Verb::Sink, iri(target))
        .with_arg("content", ArgRef::Inline(content.as_bytes().to_vec()))
}

/// Cells `urn:example:cell:{A1,A2,B1}`: a cacheable read of what was last sunk.
fn cells() -> EndpointSpace {
    let mut space = EndpointSpace::new();
    for name in ["A1", "A2", "B1"] {
        let held = Arc::new(Mutex::new(b"0".to_vec()));
        let cell = FnEndpoint::new("cell", move |inv| match inv.request.verb {
            Verb::Sink => {
                *held.lock().unwrap() = inv.inline_arg("content")?.to_vec();
                Ok(Representation::new(
                    ReprType::new("text/plain"),
                    b"ok".to_vec(),
                ))
            }
            _ => Ok(
                Representation::new(ReprType::new("text/plain"), held.lock().unwrap().clone())
                    .cacheable(),
            ),
        });
        space = space.bind(Exact::new(format!("urn:example:cell:{name}")), cell);
    }
    space
}

/// `urn:example:double`: twice A1, counting its runs — the derived value a page shows.
fn with_double(space: EndpointSpace, runs: Arc<AtomicU32>) -> EndpointSpace {
    let double = AsyncFnEndpoint::new("double", move |inv| {
        let runs = Arc::clone(&runs);
        Box::pin(async move {
            runs.fetch_add(1, Ordering::SeqCst);
            let a1 = inv.source(&iri("urn:example:cell:A1")).await?;
            let n: i64 = String::from_utf8_lossy(&a1.bytes)
                .trim()
                .parse()
                .unwrap_or(0);
            Ok(Representation::new(
                ReprType::new("text/plain"),
                (n * 2).to_string().into_bytes(),
            )
            .cacheable())
        })
    });
    space.bind(Exact::new("urn:example:double"), double)
}

fn threads(events: &[CutEvent]) -> Vec<&str> {
    events.iter().map(|e| e.thread.as_str()).collect()
}

#[test]
fn a_sink_is_heard_as_exactly_the_threads_it_cut() {
    let runs = Arc::new(AtomicU32::new(0));
    let kernel = Kernel::new(Arc::new(with_double(cells(), Arc::clone(&runs))));
    let root = Capability::root();
    let listener = kernel
        .listen(ListenSpec::new().prefix("urn:example:cell:"), &root)
        .unwrap();

    block_on(kernel.issue(source("urn:example:double"), &root)).unwrap();
    block_on(kernel.issue(sink("urn:example:cell:A1", "21"), &root)).unwrap();
    let batch = listener.drain();
    assert_eq!(batch.dropped, 0);
    assert_eq!(threads(&batch.events), ["urn:example:cell:A1"]);
    assert_eq!(
        batch.events[0].invalidated,
        ["urn:example:cell:A1", "urn:example:double"],
        "the atom and the composite over it, both of which were being served"
    );
    assert_eq!(batch.events[0].invalidated_more, 0);

    // A write nothing had read still cuts, and is still heard; it invalidated nothing.
    block_on(kernel.issue(sink("urn:example:cell:B1", "1"), &root)).unwrap();
    // A cut outside the spec is not heard at all.
    kernel.cut("urn:example:elsewhere");
    let batch = listener.drain();
    assert_eq!(threads(&batch.events), ["urn:example:cell:B1"]);
    assert!(batch.events[0].invalidated.is_empty());
    assert!(listener.drain().is_empty(), "drained means empty");
}

#[test]
fn the_host_recomputes_before_the_first_reader() {
    // The recalculating golden thread (News 7.6), built by a host from public API:
    // it keeps a set of reads it wants warm, drains the listener, and re-issues each
    // invalidated one under the registrant's capability. The reader who comes next
    // is served from the cache and never pays for the recomputation.
    let runs = Arc::new(AtomicU32::new(0));
    let kernel = Kernel::new(Arc::new(with_double(cells(), Arc::clone(&runs))));
    let host = Capability::scoped([CAP_LISTEN]);
    let listener = kernel
        .listen(ListenSpec::new().prefix("urn:example:"), &host)
        .unwrap();
    let warm = ["urn:example:double"];
    let recompute = |kernel: &Kernel| {
        for event in listener.drain().events {
            for target in event
                .invalidated
                .iter()
                .filter(|t| warm.contains(&t.as_str()))
            {
                block_on(kernel.issue(source(target), &host)).unwrap();
            }
        }
    };

    assert_eq!(
        block_on(kernel.issue(source("urn:example:double"), &host))
            .unwrap()
            .bytes,
        b"0"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    block_on(kernel.issue(sink("urn:example:cell:A1", "21"), &host)).unwrap();
    assert!(!kernel.is_cached(&source("urn:example:double"), &host));
    recompute(&kernel);
    assert_eq!(runs.load(Ordering::SeqCst), 2, "the host recomputed it");
    assert!(
        kernel.is_cached(&source("urn:example:double"), &host),
        "warm before anyone asks"
    );

    // The first reader after the edit: served from the cache, fresh.
    assert_eq!(
        block_on(kernel.issue(source("urn:example:double"), &host))
            .unwrap()
            .bytes,
        b"42"
    );
    assert_eq!(
        runs.load(Ordering::SeqCst),
        2,
        "the reader did not recompute"
    );
    assert!(
        listener.drain().is_empty(),
        "reads cut nothing, so recomputing does not feed the loop"
    );
}

#[test]
fn a_listener_learns_only_the_names_its_own_reads_rest_on() {
    let runs = Arc::new(AtomicU32::new(0));
    let kernel = Kernel::new(Arc::new(with_double(cells(), Arc::clone(&runs))));
    let root = Capability::root();
    let mine = Capability::scoped([CAP_LISTEN, "urn:cap:example:mine"]);
    let theirs = Capability::scoped(["urn:cap:example:theirs"]);

    // Registering is itself a capability.
    let refused = kernel.listen(ListenSpec::new().prefix("urn:example:"), &theirs);
    assert!(
        matches!(&refused, Err(Error::Denied(m)) if m.contains(CAP_LISTEN)),
        "{:?}",
        refused.err()
    );

    let listener = kernel
        .listen(ListenSpec::new().prefix("urn:example:"), &mine)
        .unwrap();
    let everything = kernel
        .listen(ListenSpec::new().prefix("urn:example:"), &root)
        .unwrap();

    // They read B1 and the composite over A1; I read A1 alone.
    block_on(kernel.issue(source("urn:example:cell:B1"), &theirs)).unwrap();
    block_on(kernel.issue(source("urn:example:double"), &theirs)).unwrap();
    block_on(kernel.issue(source("urn:example:cell:A1"), &mine)).unwrap();

    block_on(kernel.issue(sink("urn:example:cell:B1", "1"), &root)).unwrap();
    block_on(kernel.issue(sink("urn:example:cell:A1", "2"), &root)).unwrap();

    let heard = listener.drain().events;
    assert_eq!(
        threads(&heard),
        ["urn:example:cell:A1"],
        "B1 rests only on their read, so I never hear its name"
    );
    assert_eq!(
        heard[0].invalidated,
        ["urn:example:cell:A1"],
        "my entry, never their composite over the same thread"
    );

    let all = everything.drain().events;
    assert_eq!(
        threads(&all),
        ["urn:example:cell:B1", "urn:example:cell:A1"]
    );
    assert_eq!(
        all[1].invalidated,
        ["urn:example:cell:A1", "urn:example:double"]
    );
}

#[test]
fn a_full_queue_counts_what_it_dropped_and_keeps_the_prefix() {
    let kernel = Kernel::new(Arc::new(cells()));
    let listener = kernel
        .listen(
            ListenSpec::new().prefix("urn:example:t:").capacity(2),
            &Capability::root(),
        )
        .unwrap();
    for n in 0..5 {
        kernel.cut(format!("urn:example:t:{n}"));
    }
    let batch = listener.drain();
    assert_eq!(
        threads(&batch.events),
        ["urn:example:t:0", "urn:example:t:1"],
        "the oldest are kept: a prefix of the history, never a gap in it"
    );
    assert!(batch.events[0].sequence < batch.events[1].sequence);
    assert_eq!(batch.dropped, 3, "and the host is told it missed three");
    assert!(
        listener.drain().is_empty(),
        "the count resets once reported"
    );
}

#[test]
fn an_event_names_a_bounded_number_of_targets_and_counts_the_rest() {
    let extra = 6;
    let mut space = EndpointSpace::new();
    let mut names = Vec::new();
    for n in 0..(INVALIDATED_NAMED + extra) {
        let name = format!("urn:example:many:{n:03}");
        space = space.bind(
            Exact::new(name.clone()),
            FnEndpoint::new("many", |_| {
                Ok(
                    Representation::new(ReprType::new("text/plain"), b"x".to_vec())
                        .cacheable()
                        .depends_on("urn:example:shared"),
                )
            }),
        );
        names.push(name);
    }
    let kernel = Kernel::new(Arc::new(space));
    let root = Capability::root();
    for name in &names {
        block_on(kernel.issue(source(name), &root)).unwrap();
    }
    let listener = kernel
        .listen(ListenSpec::new().exact("urn:example:shared"), &root)
        .unwrap();
    kernel.cut("urn:example:shared");
    let event = listener.drain().events.remove(0);
    assert_eq!(event.invalidated.len(), INVALIDATED_NAMED);
    assert_eq!(event.invalidated_more, extra, "counted, not dropped");
    assert_eq!(
        event.invalidated[0], names[0],
        "sorted, so the named ones are stable"
    );
}

#[test]
fn a_waiting_host_is_woken_by_the_cut() {
    struct Flag(AtomicBool);
    impl Wake for Flag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let kernel = Kernel::new(Arc::new(cells()));
    let listener = kernel
        .listen(
            ListenSpec::new().exact("urn:example:t"),
            &Capability::root(),
        )
        .unwrap();
    let flag = Arc::new(Flag(AtomicBool::new(false)));
    let waker = Waker::from(Arc::clone(&flag));
    let mut cx = Context::from_waker(&waker);
    let mut wait = Box::pin(listener.wait());

    assert!(wait.as_mut().poll(&mut cx).is_pending());
    assert!(!flag.0.load(Ordering::SeqCst));
    kernel.cut("urn:example:t");
    assert!(
        flag.0.load(Ordering::SeqCst),
        "the cut woke the waiting host"
    );
    match wait.as_mut().poll(&mut cx) {
        Poll::Ready(batch) => assert_eq!(threads(&batch.events), ["urn:example:t"]),
        Poll::Pending => panic!("woken but not ready"),
    }
}

#[test]
fn a_cut_by_resource_is_heard_and_a_dropped_listener_is_forgotten() {
    let kernel = Kernel::new(Arc::new(cells()));
    let root = Capability::root();
    let kept = kernel
        .listen(ListenSpec::new().prefix("urn:example:"), &root)
        .unwrap();
    let gone = kernel
        .listen(ListenSpec::new().prefix("urn:example:"), &root)
        .unwrap();
    drop(gone);
    // `urn:kernel:cut`, the form a watcher or a remote peer uses, is the same cut.
    let cut = Request::new(Verb::Sink, iri("urn:kernel:cut"))
        .with_arg("thread", ArgRef::Inline(b"urn:example:watched".to_vec()));
    block_on(kernel.issue(cut, &root)).unwrap();
    assert_eq!(threads(&kept.drain().events), ["urn:example:watched"]);
}

#[test]
fn the_same_cut_reaches_every_listener_in_one_order() {
    let kernel = Kernel::new(Arc::new(cells()));
    let root = Capability::root();
    let a = kernel
        .listen(ListenSpec::new().prefix("urn:example:"), &root)
        .unwrap();
    let b = kernel
        .listen(ListenSpec::new().exact("urn:example:y"), &root)
        .unwrap();
    for thread in ["urn:example:x", "urn:example:y", "urn:example:z"] {
        kernel.cut(thread);
    }
    let a: BTreeMap<String, u64> = a
        .drain()
        .events
        .into_iter()
        .map(|e| (e.thread.as_str().to_string(), e.sequence))
        .collect();
    let b = b.drain().events;
    assert_eq!(a.len(), 3);
    assert_eq!(threads(&b), ["urn:example:y"]);
    assert_eq!(
        a["urn:example:y"], b[0].sequence,
        "one cut, one sequence, whoever hears it"
    );
}
