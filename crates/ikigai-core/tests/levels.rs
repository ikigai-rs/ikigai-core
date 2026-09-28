//! **Levels: the resolved scope** (ledger #563, phase 1). A `Level` is an opt-in,
//! always-named space: an endpoint found inside one runs in a second scope — the
//! host's injected corridors, then the level it was found in and each enclosing
//! level outward, then the root — and its sub-requests resolve there. This file pins
//! the phase-1 properties one test each, and, first, the property every later one
//! stands on: a kernel with no `Level` in it is byte-identical in answers, cache
//! keys and traces to the kernel before levels existed.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    builtins, ArgRef, AsyncFnEndpoint, Capability, Confine, Description, EndpointSpace, Error,
    Exact, Fallback, FnEndpoint, Invocation, Iri, Kernel, Level, MetaRenderer, Mount, ReprType,
    Representation, Request, Resolution, Scope, Space, TraceEvent, Tracer, Verb, LEVEL_NOTE,
    SCOPE_NOTE,
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
fn probe(
    log: &Arc<Mutex<Vec<String>>>,
    name: &'static str,
    target: &'static str,
) -> AsyncFnEndpoint {
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
            .bind(
                Exact::new("urn:mod:outer"),
                probe(&log, "outer", "urn:mod:inner"),
            )
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

// ---- 1. The resolved scope ----------------------------------------------------

/// A cacheable constant that counts how often it is computed.
fn counted(name: &'static str, body: &'static [u8], hits: &Arc<AtomicU32>) -> FnEndpoint {
    let hits = Arc::clone(hits);
    FnEndpoint::new(name, move |_| {
        hits.fetch_add(1, Ordering::SeqCst);
        Ok(text(body).cacheable())
    })
}

fn constant(name: &'static str, body: &'static [u8]) -> FnEndpoint {
    FnEndpoint::new(name, move |_| Ok(text(body).cacheable()))
}

/// An endpoint that records the chain it runs in into `seen`, sources `target`, and
/// answers with what came back — or with the error, as text.
fn reader(
    seen: &Arc<Mutex<Vec<String>>>,
    name: &'static str,
    target: &'static str,
) -> AsyncFnEndpoint {
    let seen = Arc::clone(seen);
    AsyncFnEndpoint::new(name, move |inv| {
        let seen = Arc::clone(&seen);
        Box::pin(async move {
            seen.lock()
                .unwrap()
                .push(format!("{name}: {}", inv.scope()));
            match inv.source(&iri(target)).await {
                Ok(inner) => Ok(text(&inner.bytes).cacheable()),
                Err(Error::Unresolved(t)) => Ok(text(format!("unresolved {t}").as_bytes())),
                Err(other) => Err(other),
            }
        })
    })
}

fn get(kernel: &Kernel, target: &str) -> String {
    match block_on(kernel.issue(source(target), &Capability::root())) {
        Ok(r) => String::from_utf8_lossy(&r.bytes).into_owned(),
        Err(e) => format!("error: {e}"),
    }
}

/// `Mount(prefix, Level(name, space))` — the module shape.
fn module(prefix: &str, name: &str, space: EndpointSpace) -> Arc<dyn Space> {
    Arc::new(Mount::new(
        prefix,
        Arc::new(Level::new(iri(name), Arc::new(space))),
    ))
}

#[test]
fn an_endpoint_in_a_level_reaches_its_siblings_by_short_name_and_they_stay_private() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kernel = Kernel::new(Arc::new(Fallback::new(vec![
        module(
            "urn:mod:",
            "urn:example:level:mod",
            EndpointSpace::new()
                .bind(
                    Exact::new("urn:mod:helper"),
                    reader(&seen, "helper", "urn:internal:helper"),
                )
                .bind(
                    Exact::new("urn:mod:shared"),
                    reader(&seen, "shared", "urn:shared:name"),
                )
                .bind(
                    Exact::new("urn:mod:outward"),
                    reader(&seen, "outward", "urn:root:only"),
                )
                .bind(Exact::new("urn:internal:helper"), constant("i", b"private"))
                .bind(Exact::new("urn:shared:name"), constant("l", b"level's")),
        ),
        Arc::new(
            EndpointSpace::new()
                .bind(Exact::new("urn:shared:name"), constant("r", b"root's"))
                .bind(Exact::new("urn:root:only"), constant("o", b"root only")),
        ),
    ])));

    // Module-relative: a name NOT under the module's prefix reaches its sibling.
    assert_eq!(get(&kernel, "urn:mod:helper"), "private");
    // Private: the same name from outside meets the guard, and nothing else binds it.
    assert!(get(&kernel, "urn:internal:helper").starts_with("error: no endpoint resolved"));
    // The module's own binding wins for its own sub-requests, the root's for everyone
    // else — the level is consulted before the root, the root is still behind it.
    assert_eq!(get(&kernel, "urn:mod:shared"), "level's");
    assert_eq!(get(&kernel, "urn:shared:name"), "root's");
    assert_eq!(get(&kernel, "urn:mod:outward"), "root only");
    assert_eq!(
        *seen.lock().unwrap(),
        [
            "helper: @urn:example:level:mod root",
            "shared: @urn:example:level:mod root",
            "outward: @urn:example:level:mod root",
        ]
    );
}

#[test]
fn a_sub_request_falls_outward_through_the_enclosing_levels_to_the_root() {
    // outer = Level(O, [Mount("urn:in:", Level(I, {in:door})), {o:sibling, o:probe}])
    let seen = Arc::new(Mutex::new(Vec::new()));
    let inner = module(
        "urn:in:",
        "urn:example:level:inner",
        EndpointSpace::new()
            .bind(
                Exact::new("urn:in:door"),
                reader(&seen, "in:door", "urn:o:probe"),
            )
            .bind(
                Exact::new("urn:in:root"),
                reader(&seen, "in:root", "urn:root:probe"),
            ),
    );
    let outer_space: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        inner,
        Arc::new(
            EndpointSpace::new()
                .bind(
                    Exact::new("urn:o:probe"),
                    reader(&seen, "o:probe", "urn:o:leaf"),
                )
                .bind(Exact::new("urn:o:leaf"), constant("leaf", b"outer leaf")),
        ),
    ]));
    let kernel = Kernel::new(Arc::new(Fallback::new(vec![
        Arc::new(Level::new(iri("urn:example:level:outer"), outer_space)) as Arc<dyn Space>,
        Arc::new(EndpointSpace::new().bind(
            Exact::new("urn:root:probe"),
            reader(&seen, "root:probe", "urn:o:leaf"),
        )),
    ])));

    // in:door (inner, inside outer) → o:probe, found in OUTER from the inner stack
    // → o:leaf, a sibling of o:probe at the outer level.
    assert_eq!(get(&kernel, "urn:in:door"), "outer leaf");
    // in:root → root:probe, found at the ROOT: it runs with no level, and o:leaf is
    // reachable from there only because the outer level is unguarded at the root.
    assert_eq!(get(&kernel, "urn:in:root"), "outer leaf");
    assert_eq!(
        *seen.lock().unwrap(),
        [
            "in:door: @urn:example:level:inner @urn:example:level:outer root",
            "o:probe: @urn:example:level:outer root",
            "in:root: @urn:example:level:inner @urn:example:level:outer root",
            "root:probe: root",
        ]
    );
}

#[test]
fn an_injected_corridor_still_stands_in_for_a_name_inside_a_level() {
    // The host's corridors are consulted BEFORE the level stack: host-chosen context
    // (a game's stored squares, a pinned time) stands in for any name, including a
    // module's own internal one.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kernel = Kernel::new(module(
        "urn:game:",
        "urn:example:level:game",
        EndpointSpace::new()
            .bind(
                Exact::new("urn:game:rules"),
                reader(&seen, "rules", "urn:stored:1:1"),
            )
            .bind(
                Exact::new("urn:stored:1:1"),
                constant("blank", b"empty square"),
            ),
    ));
    let game = || {
        Scope::empty().with_named(
            iri("urn:ctx:game:7"),
            Arc::new(EndpointSpace::new().bind(Exact::new("urn:stored:1:1"), constant("x", b"X"))),
        )
    };
    assert_eq!(get(&kernel, "urn:game:rules"), "empty square");
    let played =
        block_on(kernel.issue_in(source("urn:game:rules"), &Capability::root(), game())).unwrap();
    assert_eq!(played.bytes, b"X");
    assert_eq!(
        seen.lock().unwrap()[1],
        "rules: urn:ctx:game:7 @urn:example:level:game root"
    );
}

#[test]
fn two_levels_binding_the_same_short_name_keep_separate_cache_entries() {
    let (a_hits, b_hits) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kernel = Kernel::new(Arc::new(Fallback::new(vec![
        module(
            "urn:a:",
            "urn:example:level:a",
            EndpointSpace::new()
                .bind(Exact::new("urn:a:read"), reader(&seen, "a", "urn:short:x"))
                .bind(Exact::new("urn:short:x"), counted("ax", b"A's x", &a_hits)),
        ),
        module(
            "urn:b:",
            "urn:example:level:b",
            EndpointSpace::new()
                .bind(Exact::new("urn:b:read"), reader(&seen, "b", "urn:short:x"))
                .bind(Exact::new("urn:short:x"), counted("bx", b"B's x", &b_hits)),
        ),
    ])));
    for _ in 0..2 {
        assert_eq!(get(&kernel, "urn:a:read"), "A's x");
        assert_eq!(get(&kernel, "urn:b:read"), "B's x");
    }
    // Each computed once — the second round was served from the cache — and never
    // the other's answer: the same request id, two keys, because the sub-request's
    // scope names the level it was issued from.
    assert_eq!(a_hits.load(Ordering::SeqCst), 1);
    assert_eq!(b_hits.load(Ordering::SeqCst), 1);
    let readout = get(&kernel, "urn:kernel:cache");
    let rows: Vec<&str> = readout
        .lines()
        .filter(|l| l.trim_start().starts_with("urn:short:x"))
        .collect();
    assert_eq!(rows.len(), 2, "{readout}");
    assert!(rows[0].ends_with("@urn:example:level:a root"), "{readout}");
    assert!(rows[1].ends_with("@urn:example:level:b root"), "{readout}");
    // The outer reads are keyed in the empty chain: where they were found is a
    // function of the name and the chain they resolved in, which the key has.
    assert!(readout.contains("urn:a:read"));
}

#[test]
fn the_level_path_is_in_the_fingerprint_by_name_and_only_when_there_is_one() {
    let prints = Arc::new(Mutex::new(Vec::new()));
    let printer = {
        let prints = Arc::clone(&prints);
        move |name: &'static str| {
            let prints = Arc::clone(&prints);
            FnEndpoint::new(name, move |inv| {
                prints
                    .lock()
                    .unwrap()
                    .push((inv.scope().fingerprint(), inv.scope().to_string()));
                Ok(text(b"ok"))
            })
        }
    };
    let build = |a: &str| {
        Kernel::new(Arc::new(Fallback::new(vec![
            Arc::new(Level::new(
                iri(a),
                Arc::new(EndpointSpace::new().bind(Exact::new("urn:l:p"), printer("p"))),
            )) as Arc<dyn Space>,
            Arc::new(EndpointSpace::new().bind(Exact::new("urn:r:p"), printer("r"))),
        ])))
    };
    get(&build("urn:example:level:one"), "urn:l:p");
    get(&build("urn:example:level:one"), "urn:l:p"); // rebuilt, same name
    get(&build("urn:example:level:two"), "urn:l:p");
    get(&build("urn:example:level:one"), "urn:r:p"); // found at the root
    let prints = prints.lock().unwrap();
    assert_ne!(prints[0].0, 0);
    assert_eq!(
        prints[0], prints[1],
        "same name, same key: a level is its name"
    );
    assert_ne!(
        prints[0].0, prints[2].0,
        "a different level is a different key"
    );
    assert_eq!(
        prints[3],
        (0, "root".to_string()),
        "no level, the empty chain"
    );
}

#[test]
fn a_confinement_inside_a_level_leaves_the_level_stack_behind() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let corridor: Arc<dyn Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:doc:1"), constant("doc", b"doc")));
    let confined = |name: &'static str, target: &'static str| {
        Arc::new(Confine::new(
            iri("urn:ctx:doc:1"),
            Arc::clone(&corridor),
            Arc::new(reader(&seen, name, target)),
        ))
    };
    let kernel = Kernel::new(module(
        "urn:mod:",
        "urn:example:level:mod",
        EndpointSpace::new()
            .bind_arc(
                Exact::new("urn:mod:sibling"),
                confined("sib", "urn:mod:other"),
            )
            .bind_arc(Exact::new("urn:mod:doc"), confined("doc", "urn:doc:1"))
            .bind(Exact::new("urn:mod:other"), constant("other", b"sibling")),
    ));
    // The confinement cuts the arrangement off — the root AND the levels — so a
    // sibling is as unreachable as a root door, and the corridor still answers.
    assert_eq!(get(&kernel, "urn:mod:sibling"), "unresolved urn:mod:other");
    assert_eq!(get(&kernel, "urn:mod:doc"), "doc");
    assert_eq!(
        *seen.lock().unwrap(),
        ["sib: urn:ctx:doc:1 severed", "doc: urn:ctx:doc:1 severed"]
    );
}

#[test]
fn a_level_inside_a_confinement_is_consulted_before_the_confined_corridor_it_sits_in() {
    // An endpoint found inside a level that is inside the CONFINED corridor runs from
    // its own level outward: its level, then the corridor holding it, and no root.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let corridor: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        module(
            "urn:doc:",
            "urn:example:level:doc",
            EndpointSpace::new()
                .bind(
                    Exact::new("urn:doc:page"),
                    reader(&seen, "page", "urn:x:name"),
                )
                .bind(Exact::new("urn:x:name"), constant("in", b"level's")),
        ),
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:x:name"), constant("c", b"corridor's"))),
    ]));
    let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind_arc(
        Exact::new("urn:extract"),
        Arc::new(Confine::new(
            iri("urn:ctx:doc"),
            corridor,
            Arc::new(reader(&seen, "extract", "urn:doc:page")),
        )),
    )));
    assert_eq!(get(&kernel, "urn:extract"), "level's");
    assert_eq!(
        *seen.lock().unwrap(),
        [
            "extract: urn:ctx:doc severed",
            "page: @urn:example:level:doc urn:ctx:doc severed",
        ]
    );
}

#[test]
fn the_capability_floor_and_attenuation_still_apply_inside_a_level() {
    let gated = FnEndpoint::new("gated", |_| Ok(text(b"secret"))).with_description(
        Description::new("gated")
            .verb(Verb::Source)
            .requires("urn:cap:secret"),
    );
    let kernel = Kernel::new(module(
        "urn:mod:",
        "urn:example:level:mod",
        EndpointSpace::new()
            .bind(
                Exact::new("urn:mod:door"),
                reader(
                    &Arc::new(Mutex::new(Vec::new())),
                    "door",
                    "urn:internal:gated",
                ),
            )
            .bind(Exact::new("urn:internal:gated"), gated),
    ));
    let denied = block_on(kernel.issue(
        source("urn:mod:door"),
        &Capability::scoped(["urn:cap:other"]),
    ))
    .unwrap_err();
    assert!(matches!(denied, Error::Denied(_)), "{denied:?}");
    let allowed = block_on(kernel.issue(
        source("urn:mod:door"),
        &Capability::scoped(["urn:cap:secret"]),
    ))
    .unwrap();
    assert_eq!(allowed.bytes, b"secret");
}

#[test]
fn the_found_path_is_innermost_first_and_survives_decoration() {
    let inner = Level::new(
        iri("urn:example:level:inner"),
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:m:x"), builtins::echo())),
    );
    let outer = Level::new(
        iri("urn:example:level:outer"),
        Arc::new(Mount::new("urn:m:", Arc::new(inner))),
    );
    let Resolution::Hit(hit) = outer
        .resolve(&source("urn:m:x"), &Scope::empty())
        .map_endpoint(|endpoint| endpoint)
    else {
        panic!("a hit")
    };
    let path: Vec<&str> = hit.levels().names().map(Iri::as_str).collect();
    assert_eq!(path, ["urn:example:level:inner", "urn:example:level:outer"]);
    assert_eq!(
        hit.answered_by.as_ref().map(Iri::as_str),
        Some("urn:example:level:inner"),
        "a level is named, so it answers when nothing inside it did"
    );
    let hit = hit.with_endpoint(Arc::new(builtins::echo()));
    assert_eq!(hit.levels().len(), 2);
    assert_eq!(outer.id().unwrap().as_str(), "urn:example:level:outer");
    assert_eq!(outer.entries().unwrap()[0].pattern, "urn:m:x");
}

#[test]
fn every_traced_event_names_the_level_it_was_found_in() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kernel = Kernel::new(module(
        "urn:mod:",
        "urn:example:level:mod",
        EndpointSpace::new()
            .bind(
                Exact::new("urn:mod:door"),
                reader(&seen, "door", "urn:internal:x"),
            )
            .bind(Exact::new("urn:internal:x"), constant("x", b"x")),
    ));
    let collect = Arc::new(Collect::default());
    block_on(kernel.issue_traced(source("urn:mod:door"), &Capability::root(), collect.clone()))
        .unwrap();
    let events = collect.0.lock().unwrap();
    let note = |e: &TraceEvent, key: &str| {
        e.notes
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    // The sub-request: found in the level, resolved IN the level's scope.
    assert_eq!(events[0].target, "urn:internal:x");
    assert_eq!(
        note(&events[0], LEVEL_NOTE).as_deref(),
        Some("urn:example:level:mod")
    );
    assert_eq!(
        note(&events[0], SCOPE_NOTE).as_deref(),
        Some("@urn:example:level:mod root")
    );
    // The outer request: found in the level, resolved in the empty chain.
    assert_eq!(events[1].target, "urn:mod:door");
    assert_eq!(
        note(&events[1], LEVEL_NOTE).as_deref(),
        Some("urn:example:level:mod")
    );
    assert_eq!(note(&events[1], SCOPE_NOTE), None);
}

// ---- 2. Space-scoped transreptors and the topology ---------------------------

/// Emits `text/turtle` for the canonical request and refuses anything else, so the
/// kernel goes to selection for any other face.
struct TurtleOnly;
impl MetaRenderer for TurtleOnly {
    fn render(
        &self,
        description: &Description,
        target: &ReprType,
    ) -> Result<Representation, Error> {
        match target.media_type.as_str() {
            "text/turtle" | "*/*" | "" => Ok(Representation::new(
                ReprType::new("text/turtle"),
                format!("<urn:x> ik:id \"{}\" .", description.id).into_bytes(),
            )
            .cacheable()),
            other => Err(Error::Endpoint(format!(
                "unsupported meta target `{other}`"
            ))),
        }
    }
}

fn transreptor(id: &'static str, tag: &'static str) -> FnEndpoint {
    FnEndpoint::new(id, move |inv: &Invocation<'_>| {
        let content = inv.inline_str("content").unwrap_or("");
        Ok(Representation::new(
            ReprType::new("application/rdf+xml"),
            format!("{tag}({content})").into_bytes(),
        )
        .cacheable())
    })
    .with_description(
        Description::new(id)
            .verb(Verb::Source)
            .input(ikigai_core::ArgSpec::new("content"))
            .input(ikigai_core::ArgSpec::new("as"))
            .transreptor(["text/turtle"], ["application/rdf+xml"]),
    )
}

#[test]
fn an_endpoint_plans_through_the_transreptor_its_own_level_binds() {
    // The root has NO transreptor; the module's level has one. Selection walks the
    // resolved scope, so the module's endpoint plans through it and a root endpoint
    // finds nothing — and a Meta face of a module endpoint is converted by it.
    let planner = |name: &'static str| {
        FnEndpoint::new(name, |inv| {
            let plan = inv.select_transreptor("text/turtle", "application/rdf+xml");
            Ok(text(
                plan.map(|p| p[0].endpoint.clone())
                    .unwrap_or_else(|| "none".into())
                    .as_bytes(),
            ))
        })
    };
    let kernel = Kernel::with_meta_renderer(
        Arc::new(Fallback::new(vec![
            module(
                "urn:mod:",
                "urn:example:level:mod",
                EndpointSpace::new()
                    .bind(Exact::new("urn:mod:plan"), planner("inside"))
                    .bind(
                        Exact::new("urn:rdf:transrept"),
                        transreptor("mod-rdf", "MODULE"),
                    ),
            ),
            Arc::new(EndpointSpace::new().bind(Exact::new("urn:root:plan"), planner("outside"))),
        ])),
        Arc::new(TurtleOnly),
    );
    assert_eq!(get(&kernel, "urn:mod:plan"), "urn:rdf:transrept");
    assert_eq!(get(&kernel, "urn:root:plan"), "none");
    assert_eq!(
        kernel.select_transreptor("text/turtle", "application/rdf+xml"),
        None,
        "not offered at the root: the transreptor is private to the module"
    );

    let meta = |target: &str| {
        let request = Request::new(Verb::Meta, iri(target))
            .with_arg("as", ArgRef::Inline(b"application/rdf+xml".to_vec()));
        block_on(kernel.issue(request, &Capability::root())).unwrap()
    };
    let inside = meta("urn:mod:plan");
    assert!(
        String::from_utf8_lossy(&inside.bytes).starts_with("MODULE("),
        "{inside:?}"
    );
    // Outside the level there is no plan, and Meta hands back canonical Turtle.
    assert_eq!(meta("urn:root:plan").repr_type.media_type, "text/turtle");
}

#[test]
fn the_topology_renders_a_level_and_the_chain_an_endpoint_inside_one_sees() {
    let kernel = Kernel::new(module(
        "urn:mod:",
        "urn:example:level:mod",
        EndpointSpace::new().bind(
            Exact::new("urn:mod:look"),
            AsyncFnEndpoint::new("look", |inv| {
                Box::pin(async move { inv.source(&iri("urn:kernel:topology")).await })
            }),
        ),
    ));
    let whole = get(&kernel, "urn:kernel:topology");
    assert!(
        whole.contains("<urn:example:level:mod> a ik:Level ;\n    ik:space <"),
        "{whole}"
    );
    // From inside: the chain's layers are the level, then the root.
    let inside = get(&kernel, "urn:mod:look");
    let entry = inside
        .lines()
        .find(|l| l.contains("a ik:Chain"))
        .expect("an entry node");
    assert!(entry.starts_with("<urn:ikigai:chain:"), "{inside}");
    assert!(
        !entry.contains("chain:root"),
        "the chain is not the empty one: {inside}"
    );
    assert!(
        inside.contains("rdf:first <urn:example:level:mod>"),
        "the level is a layer of the chain: {inside}"
    );
}
