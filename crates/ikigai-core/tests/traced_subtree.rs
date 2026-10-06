//! **A remote subtree lands in the resolution's own trace** (ledger #750, B1).
//! `Invocation::record_subtree` — how a mount forwarding to a remote kernel stitches
//! the remote's spans in — wrote to the kernel's GLOBAL tracer whatever trace the
//! resolution was in. Under `Kernel::issue_traced`, the per-call form a wire server
//! runs per connection, the subtree therefore missed the caller's collector and
//! landed in whatever global collector another tenant or the operator installed.

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Capability, EndpointSpace, Exact, Iri, Kernel, ReprType, Representation,
    Request, TraceEvent, Tracer, Verb,
};

const REMOTE: &str = "urn:remote:secret-tenant-a";

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

#[derive(Default)]
struct Recorder(Mutex<Vec<TraceEvent>>);

impl Tracer for Recorder {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Recorder {
    fn saw(&self, target: &str) -> Option<TraceEvent> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.target == target)
            .cloned()
    }
}

/// A kernel whose `urn:mount:x` stands in for a mount forwarding to a remote
/// kernel: when traced, it stitches a one-node remote subtree under itself.
fn forwarding_kernel() -> Kernel {
    let forwarder = AsyncFnEndpoint::new("forward", |inv| {
        Box::pin(async move {
            if inv.trace_span().is_some() {
                inv.record_subtree(vec![TraceEvent {
                    target: REMOTE.to_string(),
                    thread: "remote".to_string(),
                    started: None,
                    ended: None,
                    cache_hit: false,
                    span: 0,
                    parent: None,
                    capability: None,
                    notes: Vec::new(),
                }]);
            }
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"ok".to_vec(),
            ))
        })
    });
    Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:mount:x"), forwarder),
    ))
}

fn forward() -> Request {
    Request::new(Verb::Source, iri("urn:mount:x"))
}

#[test]
fn a_remote_subtree_stays_in_the_per_call_trace() {
    let kernel = forwarding_kernel();
    let global = Arc::new(Recorder::default());
    kernel.set_tracer(global.clone());
    let mine = Arc::new(Recorder::default());
    block_on(kernel.issue_traced(forward(), &Capability::root(), mine.clone())).unwrap();

    let mount = mine
        .saw("urn:mount:x")
        .expect("the mount node is in my trace");
    let remote = mine.saw(REMOTE).expect("the remote subtree is in my trace");
    assert_eq!(remote.parent, Some(mount.span), "stitched under the mount");
    assert!(
        global.saw(REMOTE).is_none(),
        "a per-call resolution's remote subtree leaked into the global tracer"
    );
}

/// A resolution traced into the global tracer still gets its remote subtree there.
#[test]
fn a_globally_traced_resolution_still_gets_its_remote_subtree() {
    let kernel = forwarding_kernel();
    let global = Arc::new(Recorder::default());
    kernel.set_tracer(global.clone());
    block_on(kernel.issue(forward(), &Capability::root())).unwrap();

    let mount = global.saw("urn:mount:x").expect("the mount node is traced");
    let remote = global.saw(REMOTE).expect("the remote subtree is traced");
    assert_eq!(remote.parent, Some(mount.span));
}
