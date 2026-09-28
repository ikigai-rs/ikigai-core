//! **Levels: the resolved scope** (ledger #563, phase 1). A `Level` is an opt-in,
//! always-named space: an endpoint found inside one runs in a second scope — the
//! host's injected corridors, then the level it was found in and each enclosing
//! level outward, then the root — and its sub-requests resolve there. This file pins
//! the phase-1 properties one test each, and, first, the property every later one
//! stands on: a kernel with no `Level` in it is byte-identical in answers, cache
//! keys and traces to the kernel before levels existed.

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    AsyncFnEndpoint, Capability, Confine, EndpointSpace, Exact, Fallback, FnEndpoint, Iri, Kernel,
    Mount, ReprType, Representation, Request, Scope, Space, TraceEvent, Tracer, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn text(bytes: &[u8]) -> Representation {
    Representation::new(ReprType::new("text/plain"), bytes.to_vec())
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

#[derive(Default)]
struct Collect(Mutex<Vec<TraceEvent>>);

impl Tracer for Collect {
    fn record(&self, event: TraceEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// Every event, minus the two fields that are not a function of the arrangement
/// (the worker thread's name and the clock readings, both absent or incidental).
fn render(events: &[TraceEvent]) -> String {
    events
        .iter()
        .map(|e| {
            format!(
                "{} hit={} span={} parent={:?} cap={:?} notes={:?}\n",
                e.target, e.cache_hit, e.span, e.parent, e.capability, e.notes
            )
        })
        .collect()
}

// ---- 0. No `Level`, no change ------------------------------------------------

/// An endpoint that writes the chain it runs in — as its sub-requests will see it —
/// into `log`, then sources `target` and answers with what came back.
fn probe(log: &Arc<Mutex<Vec<String>>>, name: &'static str, target: &'static str) -> AsyncFnEndpoint {
    let log = Arc::clone(log);
    AsyncFnEndpoint::new(name, move |inv| {
        let log = Arc::clone(&log);
        Box::pin(async move {
            log.lock().unwrap().push(format!(
                "{name}: {} {:016x}",
                inv.scope(),
                inv.scope().fingerprint()
            ));
            let inner = inv.source(&iri(target)).await?;
            Ok(text(&inner.bytes).cacheable())
        })
    })
}

/// The transcript of a fixed workload over a kernel with no `Level` in it: every
/// answer, the chain each endpoint saw and its fingerprint, every traced event, the
/// cache readout, and the fingerprints of the scopes a host builds. Pinned below
/// against the transcript main produced before levels existed (7774fc2, 0.1.80).
fn no_level_transcript() -> String {
    let log = Arc::new(Mutex::new(Vec::new()));
    let module: Arc<dyn Space> = Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:mod:outer"), probe(&log, "outer", "urn:mod:inner"))
            .bind(
                Exact::new("urn:mod:inner"),
                FnEndpoint::new("inner", |_| Ok(text(b"inner").cacheable())),
            )
            .named(iri("urn:example:space:mod")),
    );
    let corridor: Arc<dyn Space> = Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:c:doc"),
        FnEndpoint::new("doc", |_| Ok(text(b"doc").cacheable())),
    ));
    let plain: Arc<dyn Space> = Arc::new(
        EndpointSpace::new()
            .bind(
                Exact::new("urn:plain:leaf"),
                FnEndpoint::new("leaf", |_| Ok(text(b"leaf").cacheable())),
            )
            .bind_arc(
                Exact::new("urn:plain:confined"),
                Arc::new(Confine::new(
                    iri("urn:ctx:c"),
                    Arc::clone(&corridor),
                    Arc::new(probe(&log, "confined", "urn:c:doc")),
                )),
            ),
    );
    let kernel = Kernel::new(Arc::new(
        Fallback::new(vec![Arc::new(Mount::new("urn:mod:", module)), plain])
            .named(iri("urn:example:space:root")),
    ));
    let cap = Capability::root();
    let pinned = || {
        Scope::empty().with_named(
            iri("urn:ctx:time:2026-09-28"),
            Arc::new(EndpointSpace::new().bind(
                Exact::new("urn:mod:inner"),
                FnEndpoint::new("pinned", |_| Ok(text(b"pinned").cacheable())),
            )),
        )
    };

    let mut out = String::new();
    let traced = |request: Request, scope: Option<Scope>| {
        let collect = Arc::new(Collect::default());
        let answer = match scope {
            None => block_on(kernel.issue_traced(request, &cap, collect.clone())),
            Some(scope) => {
                kernel.set_tracer(collect.clone());
                let answer = block_on(kernel.issue_in(request, &cap, scope));
                kernel.clear_tracer();
                answer
            }
        };
        let answer = match answer {
            Ok(r) => String::from_utf8_lossy(&r.bytes).into_owned(),
            Err(e) => format!("error: {e}"),
        };
        format!("{answer}\n{}", render(&collect.0.lock().unwrap()))
    };
    out.push_str(&traced(source("urn:mod:outer"), None));
    out.push_str(&traced(source("urn:mod:outer"), None));
    out.push_str(&traced(source("urn:mod:outer"), Some(pinned())));
    out.push_str(&traced(source("urn:plain:confined"), None));
    out.push_str(&traced(source("urn:plain:leaf"), Some(pinned())));
    out.push_str(&traced(source("urn:nowhere"), Some(pinned())));
    out.push_str(&traced(source("urn:c:doc"), Some(pinned().sever())));
    for line in log.lock().unwrap().iter() {
        out.push_str(line);
        out.push('\n');
    }
    let readout = block_on(kernel.issue(source("urn:kernel:cache"), &cap)).unwrap();
    out.push_str(&String::from_utf8_lossy(&readout.bytes));
    for scope in [
        Scope::empty(),
        pinned(),
        pinned().sever(),
        pinned().confined(iri("urn:ctx:c"), corridor.clone()),
        Scope::empty().confined(iri("urn:ctx:c"), corridor.clone()),
        pinned().with_named(iri("urn:ctx:inner"), corridor.clone()),
    ] {
        out.push_str(&format!("{scope} {:016x}\n", scope.fingerprint()));
    }
    out
}

#[test]
fn a_kernel_without_a_level_is_byte_identical_in_answers_cache_keys_and_traces() {
    let transcript = no_level_transcript();
    assert_eq!(transcript, NO_LEVEL_GOLDEN, "\n--- got ---\n{transcript}");
    // And stable run to run: nothing in it depends on a process-unique identity.
    assert_eq!(no_level_transcript(), transcript);
}

/// Captured on main at 7774fc2 (0.1.80) — before `Level` existed — by running the
/// workload above. Do not regenerate this to make a failing run pass: a difference
/// IS the regression this test exists to catch.
const NO_LEVEL_GOLDEN: &str = r#"inner
urn:mod:inner hit=false span=1 parent=Some(0) cap=None notes=[("answered-by", "urn:example:space:mod")]
urn:mod:outer hit=false span=0 parent=None cap=None notes=[("answered-by", "urn:example:space:mod")]
inner
urn:mod:outer hit=true span=0 parent=None cap=None notes=[("answered-by", "urn:example:space:mod")]
pinned
urn:mod:inner hit=false span=1 parent=Some(0) cap=None notes=[("answered-by", "urn:ctx:time:2026-09-28"), ("scope", "urn:ctx:time:2026-09-28 root")]
urn:mod:outer hit=false span=0 parent=None cap=None notes=[("answered-by", "urn:example:space:mod"), ("scope", "urn:ctx:time:2026-09-28 root")]
doc
urn:c:doc hit=false span=1 parent=Some(0) cap=None notes=[("answered-by", "urn:ctx:c"), ("scope", "urn:ctx:c severed")]
urn:plain:confined hit=false span=0 parent=None cap=None notes=[("answered-by", "urn:example:space:root")]
leaf
urn:plain:leaf hit=false span=2 parent=None cap=None notes=[("answered-by", "urn:example:space:root"), ("scope", "urn:ctx:time:2026-09-28 root")]
error: no endpoint resolved for urn:nowhere
urn:nowhere hit=false span=3 parent=None cap=None notes=[("scope", "urn:ctx:time:2026-09-28 root"), ("scope-unresolved", "urn:nowhere")]
error: no endpoint resolved for urn:c:doc
urn:c:doc hit=false span=4 parent=None cap=None notes=[("scope", "urn:ctx:time:2026-09-28 severed"), ("scope-unresolved", "urn:c:doc")]
outer: root 0000000000000000
outer: urn:ctx:time:2026-09-28 root c96f742eeb49fb34
confined: urn:ctx:c severed 1904a77fcaa47f66
cache
  entries  7 / 4096
  size     32 B / 64.0 MB
  urn:c:doc           text/plain                      3 B  1 thread    urn:ctx:c severed
  urn:mod:inner       text/plain                      5 B  1 thread    root
  urn:mod:inner       text/plain                      6 B  1 thread    urn:ctx:time:2026-09-28 root
  urn:mod:outer       text/plain                      5 B  2 threads   root
  urn:mod:outer       text/plain                      6 B  2 threads   urn:ctx:time:2026-09-28 root
  urn:plain:confined  text/plain                      3 B  2 threads   root
  urn:plain:leaf      text/plain                      4 B  1 thread    urn:ctx:time:2026-09-28 root
root 0000000000000000
urn:ctx:time:2026-09-28 root c96f742eeb49fb34
urn:ctx:time:2026-09-28 severed 4c49713390b16dad
urn:ctx:time:2026-09-28 urn:ctx:c severed 55b1dcab90716cb2
urn:ctx:c severed 1904a77fcaa47f66
urn:ctx:inner urn:ctx:time:2026-09-28 root b09da1fd8e882271
"#;
