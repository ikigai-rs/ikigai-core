//! **Dynamic spaces, version one** (ledger #2006): a space that gains and loses doors
//! while the kernel runs, rebuilt from a declaration RESOURCE, and a session space
//! whose teardown is one cut. What these pin, one test per acceptance criterion of
//! the arc:
//!
//! 1. an edit that ADDS a door: after the rebuild it resolves and the catalog lists it;
//! 2. an edit that REMOVES one: it is Unresolved, and its cached answer is gone;
//! 3. a REFUSED edit (unknown endpoint, a host-sealed name, a sealing level): the old
//!    doors still answer, nothing they served was cut, the status names node and reason;
//! 4. a request running across the swap completes on the OLD doors;
//! 5. a session space's thread cut drops its answers and nothing outside it;
//! 6. no cached answer from before a successful swap survives it (the coarse cut);
//! 7. no declaration, or an invalid one, at start: an EMPTY space, and a status why;
//! 8. the topology renders `ik:DynamicSpace` with `ik:declaration` and the inner tree.
//!
//! Plus the rebuild loop itself ([`DynamicSpace::drive`]) hearing a write.
#![cfg(feature = "declare")]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::task::{Context, Poll};

use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Capability, DeclarationError, Description, DynamicRefusal, DynamicSpace, EndpointSpace,
    Error, Exact, Fallback, FnEndpoint, Iri, Kernel, Level, MetaRenderer, Mount, Registry,
    ReprType, Representation, Request, Scope, Space, SpaceCheck, Threaded, Topology, Verb,
    CAP_LISTEN,
};

const DECL: &str = "urn:test:decl";

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

/// A cacheable endpoint answering its own name, counting its runs.
fn door(name: &'static str, runs: Arc<AtomicU32>) -> Arc<FnEndpoint> {
    Arc::new(FnEndpoint::new(name, move |_| {
        runs.fetch_add(1, Ordering::SeqCst);
        Ok(Representation::new(ReprType::new("text/plain"), name.as_bytes().to_vec()).cacheable())
    }))
}

/// The declaration atom at `urn:test:decl`: a cacheable read of what was last sunk,
/// or NotFound while nothing was. The kernel cuts its thread after every Sink.
fn declaration_space(initial: Option<String>) -> EndpointSpace {
    let held = Arc::new(Mutex::new(initial));
    let atom = FnEndpoint::new("declaration", move |inv| match inv.request.verb {
        Verb::Sink => {
            let text = String::from_utf8(inv.inline_arg("content")?.to_vec()).unwrap();
            *held.lock().unwrap() = Some(text);
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"ok".to_vec(),
            ))
        }
        _ => match held.lock().unwrap().clone() {
            Some(text) => Ok(
                Representation::new(ReprType::new("text/turtle"), text.into_bytes()).cacheable(),
            ),
            None => Err(Error::NotFound("no declaration yet".into())),
        },
    });
    EndpointSpace::new().bind(Exact::new(DECL), atom)
}

/// Renders a description as its id: enough for the catalog to list a door.
struct IdRenderer;

impl MetaRenderer for IdRenderer {
    fn render(
        &self,
        description: &Description,
        _: &ReprType,
    ) -> ikigai_core::Result<Representation> {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            description.id.clone().into_bytes(),
        ))
    }
}

/// Everything a test needs: the kernel, the dynamic space inside its root, the
/// endpoints a declaration may bind, and how many times each ran.
struct World {
    kernel: Arc<Kernel>,
    dynamic: Arc<DynamicSpace>,
    a: Arc<FnEndpoint>,
    b: Arc<FnEndpoint>,
    runs_a: Arc<AtomicU32>,
}

fn world(initial: Option<String>, registry_extra: impl FnOnce(&mut Registry)) -> World {
    world_with(initial, registry_extra, |space| space)
}

fn world_with(
    initial: Option<String>,
    registry_extra: impl FnOnce(&mut Registry),
    configure: impl FnOnce(DynamicSpace) -> DynamicSpace,
) -> World {
    let runs_a = Arc::new(AtomicU32::new(0));
    let a = door("a", Arc::clone(&runs_a));
    let b = door("b", Arc::new(AtomicU32::new(0)));
    let mut registry = Registry::new();
    registry.register(a.clone()).unwrap();
    registry.register(b.clone()).unwrap();
    registry_extra(&mut registry);
    let dynamic = Arc::new(configure(DynamicSpace::new(
        "test",
        iri(DECL),
        registry.sealing(["urn:sealed:"]),
    )));
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        Arc::new(declaration_space(initial)),
        dynamic.clone(),
    ]));
    World {
        kernel: Arc::new(Kernel::with_meta_renderer(root, Arc::new(IdRenderer))),
        dynamic,
        a,
        b,
        runs_a,
    }
}

/// The Turtle of an endpoint space binding `doors` (pattern, endpoint) in order.
fn declare(doors: &[(&str, &Arc<FnEndpoint>)]) -> String {
    let mut space = EndpointSpace::new();
    for (pattern, endpoint) in doors {
        space = space.bind_arc(Exact::new(*pattern), Arc::clone(endpoint) as _);
    }
    space.topology().to_turtle()
}

impl World {
    fn get(&self, target: &str) -> Result<Vec<u8>, Error> {
        block_on(self.kernel.issue(source(target), &Capability::root())).map(|r| r.bytes)
    }

    fn cached(&self, target: &str) -> bool {
        self.kernel.is_cached(&source(target), &Capability::root())
    }

    /// Write the declaration (the kernel cuts its thread), then rebuild from it as the
    /// host's loop does on that cut.
    fn edit(&self, turtle: &str) -> Result<(), DynamicRefusal> {
        block_on(self.kernel.issue(sink(DECL, turtle), &Capability::root())).unwrap();
        block_on(self.dynamic.refresh(&self.kernel, &Capability::root()))
    }

    fn status_text(&self) -> String {
        String::from_utf8(self.get(self.dynamic.status_iri().as_str()).unwrap()).unwrap()
    }
}

/// Whether the catalog (each description rendered as its id, one per line) lists the
/// endpoint `name`.
fn lists(catalog: &str, name: &str) -> bool {
    catalog.lines().any(|line| line.trim() == name)
}

fn unresolved(result: Result<Vec<u8>, Error>) -> bool {
    matches!(result, Err(Error::Unresolved(_)))
}

// 1 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn an_edit_that_adds_a_door_resolves_and_is_in_the_catalog() {
    let w = world(None, |_| {});
    let only_a = declare(&[("urn:test:a", &w.a)]);
    w.edit(&only_a).unwrap();
    assert_eq!(w.get("urn:test:a").unwrap(), b"a");
    assert!(unresolved(w.get("urn:test:b")));
    let catalog = String::from_utf8(w.get("urn:kernel:catalog").unwrap()).unwrap();
    assert!(!lists(&catalog, "b"), "{catalog}");

    w.edit(&declare(&[("urn:test:a", &w.a), ("urn:test:b", &w.b)]))
        .unwrap();
    assert_eq!(w.get("urn:test:b").unwrap(), b"b");
    // The catalog hangs from the bindings thread, which the swap cut.
    let catalog = String::from_utf8(w.get("urn:kernel:catalog").unwrap()).unwrap();
    assert!(lists(&catalog, "b"), "{catalog}");
    assert_eq!(w.dynamic.status().installs, 2);
}

// 2 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn an_edit_that_removes_a_door_takes_its_cached_answer_with_it() {
    let w = world(None, |_| {});
    w.edit(&declare(&[("urn:test:a", &w.a), ("urn:test:b", &w.b)]))
        .unwrap();
    assert_eq!(w.get("urn:test:b").unwrap(), b"b");
    assert!(w.cached("urn:test:b"));

    w.edit(&declare(&[("urn:test:a", &w.a)])).unwrap();
    assert!(
        !w.cached("urn:test:b"),
        "the removed door's answer survived"
    );
    assert!(unresolved(w.get("urn:test:b")));
    let catalog = String::from_utf8(w.get("urn:kernel:catalog").unwrap()).unwrap();
    assert!(!lists(&catalog, "b"), "{catalog}");
}

// 3 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_refused_edit_keeps_the_old_doors_and_cuts_nothing() {
    let w = world(None, |_| {});
    let good = declare(&[("urn:test:a", &w.a)]);
    w.edit(&good).unwrap();
    assert_eq!(w.get("urn:test:a").unwrap(), b"a");
    assert!(w.cached("urn:test:a"));
    let in_force = w.dynamic.status().in_force;

    // An endpoint name the host never registered.
    let unknown = good.replace("\"a\"", "\"nope\"");
    let refused = w.edit(&unknown).unwrap_err();
    assert!(
        matches!(refused, DynamicRefusal::Declaration(DeclarationError::UnknownEndpoint { ref id, .. }) if id == "nope"),
        "{refused:?}"
    );
    assert!(
        w.cached("urn:test:a"),
        "a refused edit cut what the old doors served"
    );
    assert_eq!(w.get("urn:test:a").unwrap(), b"a");
    assert_eq!(
        w.runs_a.load(Ordering::SeqCst),
        1,
        "served from the cache throughout"
    );
    let status = w.dynamic.status();
    assert_eq!(status.installs, 1);
    assert_eq!(status.in_force, in_force);
    let text = w.status_text();
    assert!(
        text.contains("binds `nope`, which the host did not register"),
        "{text}"
    );
    assert!(
        text.contains("refused at: urn:ikigai:space:_:1:door:1"),
        "{text}"
    );

    // A door under a name the host sealed, inside a level: the seal check refuses it.
    let sealed = Level::new(
        iri("urn:test:level"),
        Arc::new(EndpointSpace::new().bind_arc(Exact::new("urn:sealed:x"), w.a.clone())),
    );
    let refused = w.edit(&sealed.topology().to_turtle()).unwrap_err();
    assert!(
        matches!(
            refused,
            DynamicRefusal::Declaration(DeclarationError::Sealing(_))
        ),
        "{refused:?}"
    );
    assert!(w.cached("urn:test:a"));

    // A level that seals names: the running kernel's seals cannot take it.
    let sealing = Mount::new(
        "urn:mod:",
        Arc::new(
            Level::new(
                iri("urn:test:sealing"),
                Arc::new(EndpointSpace::new().bind_arc(Exact::new("urn:mod:a"), w.a.clone())),
            )
            .sealing(["urn:mod:secret:"]),
        ),
    );
    let refused = w.edit(&sealing.topology().to_turtle()).unwrap_err();
    assert_eq!(
        refused,
        DynamicRefusal::SealingLevel {
            node: "urn:test:sealing".into()
        }
    );
    assert!(w.status_text().contains("refused at: urn:test:sealing"));
    assert!(w.cached("urn:test:a"));
    assert_eq!(w.dynamic.status().installs, 1);

    // The next good edit clears the refusal.
    w.edit(&declare(&[("urn:test:a", &w.a), ("urn:test:b", &w.b)]))
        .unwrap();
    assert!(w
        .status_text()
        .contains("refused: nothing since the last bind"));
}

#[test]
fn the_hosts_check_runs_before_bind_and_can_refuse() {
    let check: SpaceCheck = Arc::new(|candidate: Topology| {
        Box::pin(async move {
            // A rule over the candidate's Turtle, as a host's SPARQL ASK would be.
            if candidate.to_turtle().contains("urn:test:b") {
                Err("urn:test:b is not allowed here".to_string())
            } else {
                Ok(())
            }
        })
    });
    let w = world_with(None, |_| {}, |space| space.with_check(check));
    w.edit(&declare(&[("urn:test:a", &w.a)])).unwrap();
    let refused = w
        .edit(&declare(&[("urn:test:a", &w.a), ("urn:test:b", &w.b)]))
        .unwrap_err();
    assert_eq!(
        refused,
        DynamicRefusal::Check {
            reason: "urn:test:b is not allowed here".into()
        }
    );
    assert!(unresolved(w.get("urn:test:b")));
    assert!(w.status_text().contains("the host's check refused it"));
}

// 4 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_request_running_across_the_swap_completes_on_the_old_doors() {
    let (started_tx, started) = mpsc::channel::<()>();
    let (go, go_rx) = mpsc::channel::<()>();
    let started_tx = Mutex::new(started_tx);
    let go_rx = Mutex::new(go_rx);
    let slow = Arc::new(FnEndpoint::new("slow", move |_| {
        started_tx.lock().unwrap().send(()).unwrap();
        go_rx.lock().unwrap().recv().unwrap();
        Ok(Representation::new(ReprType::new("text/plain"), b"old".to_vec()).cacheable())
    }));
    let registered = slow.clone();
    let w = world(None, move |r| r.register(registered).unwrap());
    w.edit(&declare(&[("urn:test:slow", &slow), ("urn:test:a", &w.a)]))
        .unwrap();

    let kernel = Arc::clone(&w.kernel);
    let running = std::thread::spawn(move || {
        block_on(kernel.issue(source("urn:test:slow"), &Capability::root())).map(|r| r.bytes)
    });
    started.recv().unwrap(); // resolved, and inside the old endpoint
    w.edit(&declare(&[("urn:test:a", &w.a)])).unwrap();
    go.send(()).unwrap();

    assert_eq!(running.join().unwrap().unwrap(), b"old");
    // The answer computed across the swap was not kept, and the door is gone.
    assert!(!w.cached("urn:test:slow"));
    assert!(unresolved(w.get("urn:test:slow")));
}

// 5 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cutting_a_session_thread_drops_its_answers_and_nothing_outside_it() {
    let runs = Arc::new(AtomicU32::new(0));
    let greeting = door("greeting", Arc::clone(&runs));
    let doors = || -> Arc<dyn Space> {
        Arc::new(EndpointSpace::new().bind_arc(Exact::new("urn:test:greeting"), greeting.clone()))
    };
    let outside = door("outside", Arc::new(AtomicU32::new(0)));
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind_arc(Exact::new("urn:test:outside"), outside),
    ));
    let cap = Capability::root();
    let one: Arc<dyn Space> = Arc::new(Threaded::session("one", doors()).unwrap());
    let two: Arc<dyn Space> = Arc::new(Threaded::session("two", doors()).unwrap());
    let in_one = || Scope::empty().with(Arc::clone(&one));
    let in_two = || Scope::empty().with(Arc::clone(&two));
    let greet = || source("urn:test:greeting");

    block_on(kernel.issue_in(greet(), &cap, in_one())).unwrap();
    block_on(kernel.issue_in(greet(), &cap, in_two())).unwrap();
    block_on(kernel.issue(source("urn:test:outside"), &cap)).unwrap();
    assert!(kernel.is_cached_in(&greet(), &cap, &in_one()));
    assert!(kernel.is_cached_in(&greet(), &cap, &in_two()));
    assert_eq!(
        runs.load(Ordering::SeqCst),
        2,
        "two sessions, two partitions"
    );

    kernel.cut("urn:iki:space:session:one");
    assert!(!kernel.is_cached_in(&greet(), &cap, &in_one()));
    assert!(
        kernel.is_cached_in(&greet(), &cap, &in_two()),
        "another session was cut"
    );
    assert!(
        kernel.is_cached(&source("urn:test:outside"), &cap),
        "the root was cut"
    );
}

#[test]
fn a_session_name_is_escaped_and_is_its_thread() {
    let space = Threaded::session("a b", Arc::new(EndpointSpace::new())).unwrap();
    assert_eq!(space.id().unwrap().as_str(), "urn:iki:space:session:a%20b");
    assert_eq!(space.thread().as_str(), "urn:iki:space:session:a%20b");
}

// 6 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn no_cached_answer_survives_a_successful_swap() {
    let w = world(None, |_| {});
    w.edit(&declare(&[("urn:test:a", &w.a), ("urn:test:b", &w.b)]))
        .unwrap();
    w.get("urn:test:a").unwrap();
    w.get("urn:test:b").unwrap();
    assert!(w.cached("urn:test:a") && w.cached("urn:test:b"));

    // An edit that KEEPS urn:test:a (only reorders) still cuts it: version one is coarse.
    w.edit(&declare(&[("urn:test:b", &w.b), ("urn:test:a", &w.a)]))
        .unwrap();
    assert!(!w.cached("urn:test:a"));
    assert!(!w.cached("urn:test:b"));
    assert_eq!(w.get("urn:test:a").unwrap(), b"a");
    assert_eq!(
        w.runs_a.load(Ordering::SeqCst),
        2,
        "recomputed once after the swap"
    );
}

// 7 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn with_no_declaration_the_space_starts_empty_and_says_why() {
    let w = world(None, |_| {});
    let refused = block_on(w.dynamic.refresh(&w.kernel, &Capability::root())).unwrap_err();
    assert!(
        matches!(refused, DynamicRefusal::Unreadable { .. }),
        "{refused:?}"
    );
    assert!(unresolved(w.get("urn:test:a")));
    assert!(unresolved(w.get("urn:anything:else")));
    let status = w.dynamic.status();
    assert_eq!(status.in_force, None);
    assert_eq!(status.installs, 0);
    let text = w.status_text();
    assert!(
        text.contains("in force: nothing (the space is empty)"),
        "{text}"
    );
    assert!(
        text.contains("refused: the declaration could not be read"),
        "{text}"
    );
    assert!(text.contains("no declaration yet"), "{text}");
}

#[test]
fn with_an_invalid_declaration_the_space_starts_empty_and_says_why() {
    let w = world(Some("this is not turtle".into()), |_| {});
    let refused = block_on(w.dynamic.refresh(&w.kernel, &Capability::root())).unwrap_err();
    assert!(
        matches!(
            refused,
            DynamicRefusal::Declaration(DeclarationError::Malformed { .. })
        ),
        "{refused:?}"
    );
    assert!(unresolved(w.get("urn:test:a")));
    let text = w.status_text();
    assert!(text.contains("in force: nothing"), "{text}");
    assert!(text.contains("refused: not a declaration"), "{text}");
    assert!(text.contains("refused digest: b3:"), "{text}");
}

// 8 ─────────────────────────────────────────────────────────────────────────────

#[test]
fn the_topology_renders_the_dynamic_space_its_declaration_and_inner_tree() {
    let w = world(None, |_| {});
    w.edit(&declare(&[("urn:test:a", &w.a)])).unwrap();
    let turtle = w.kernel.root_topology().to_turtle();
    assert!(
        turtle.contains(
            "<urn:iki:space:dynamic:test> a ik:DynamicSpace ;\n    ik:declaration <urn:test:decl> ;\n    ik:space <urn:ikigai:space:_:"
        ),
        "{turtle}"
    );
    assert!(turtle.contains("ik:pattern \"urn:test:a\""), "{turtle}");

    // It reads back as the same tree, and a declaration cannot BUILD one.
    let read = Topology::from_turtle(&turtle).unwrap();
    assert_eq!(read, w.kernel.root_topology());
    let mut registry = Registry::new();
    registry.register(w.a.clone()).unwrap();
    // The root's second layer is the dynamic space (the first is the declaration atom).
    let Err(refused) = ikigai_core::build(&read.children[1], &registry) else {
        panic!("a declared dynamic space was built");
    };
    assert_eq!(
        refused,
        DeclarationError::Dynamic {
            node: "urn:iki:space:dynamic:test".into()
        }
    );

    // The swap changes the rendered inner tree (the topology is a face of the bindings).
    w.edit(&declare(&[("urn:test:b", &w.b)])).unwrap();
    let turtle = w.kernel.root_topology().to_turtle();
    assert!(turtle.contains("ik:pattern \"urn:test:b\""), "{turtle}");
    assert!(!turtle.contains("ik:pattern \"urn:test:a\""), "{turtle}");
}

#[test]
fn the_status_has_a_turtle_face() {
    let w = world(None, |_| {});
    w.edit(&declare(&[("urn:test:a", &w.a)])).unwrap();
    let as_turtle = source(w.dynamic.status_iri().as_str())
        .with_arg("as", ArgRef::Inline(b"text/turtle".to_vec()));
    let turtle = block_on(w.kernel.issue(as_turtle, &Capability::root())).unwrap();
    let text = String::from_utf8(turtle.bytes).unwrap();
    let triples: Vec<_> = oxttl::TurtleParser::new()
        .for_reader(text.as_bytes())
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(triples.len(), 4, "{text}");
    let digest = w.dynamic.status().in_force.unwrap().to_string();
    assert!(text.contains(&format!("ik:inForce \"{digest}\"")), "{text}");
    assert!(
        text.contains("ik:installs \"1\"^^xsd:nonNegativeInteger"),
        "{text}"
    );
    // Live state: never cached.
    assert!(!w.cached(w.dynamic.status_iri().as_str()));
}

#[test]
fn a_declaration_cannot_shadow_the_status() {
    let w = world(None, |_| {});
    w.edit(&declare(&[("urn:iki:space:dynamic:test:status", &w.a)]))
        .unwrap();
    assert!(w
        .status_text()
        .starts_with("space: urn:iki:space:dynamic:test\n"));
}

// The loop ─────────────────────────────────────────────────────────────────────

/// Yield once, so a `select` alongside it polls its other side.
struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

#[test]
fn the_rebuild_loop_hears_a_write_and_rebuilds() {
    let w = world(Some(String::new()), |_| {});
    let only_a = declare(&[("urn:test:a", &w.a)]);
    block_on(w.kernel.issue(sink(DECL, &only_a), &Capability::root())).unwrap();
    let both = declare(&[("urn:test:a", &w.a), ("urn:test:b", &w.b)]);
    let cap = Capability::root();

    let test = async {
        let wait_for = |installs: u64| {
            let dynamic = Arc::clone(&w.dynamic);
            async move {
                for _ in 0..1000 {
                    if dynamic.status().installs >= installs {
                        return;
                    }
                    YieldNow(false).await;
                }
                panic!("the loop never bound install {installs}");
            }
        };
        // No `block_on` in here: this future is already on one.
        let get = |target: &'static str| w.kernel.issue(source(target), &cap);
        wait_for(1).await;
        assert_eq!(get("urn:test:a").await.unwrap().bytes, b"a");
        assert!(matches!(get("urn:test:b").await, Err(Error::Unresolved(_))));
        // Write the declaration, and only write it: the loop hears the cut.
        w.kernel.issue(sink(DECL, &both), &cap).await.unwrap();
        wait_for(2).await;
        assert_eq!(get("urn:test:b").await.unwrap().bytes, b"b");
    };
    let drive = w.dynamic.drive(&w.kernel, &cap);
    futures::pin_mut!(test, drive);
    match block_on(futures::future::select(test, drive)) {
        futures::future::Either::Left(((), _)) => {}
        futures::future::Either::Right((error, _)) => panic!("the loop stopped: {error}"),
    }
}

#[test]
fn the_rebuild_loop_needs_the_listen_grant() {
    let w = world(None, |_| {});
    let narrow = Capability::scoped(["urn:cap:fs:read"]);
    let error = block_on(w.dynamic.drive(&w.kernel, &narrow));
    assert!(
        matches!(error, Error::Denied(ref m) if m.contains(CAP_LISTEN)),
        "{error}"
    );
}
