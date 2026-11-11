//! The pins of `docs/formal/scope-hypothesis-audit.md`: what ikigai-core does today about the
//! gatekeeper certificate's second hypothesis, that scope changes only at declared boundaries.
//!
//! These tests hold CURRENT behavior, findings included, and none of them says the behavior is
//! right. A test here named as a finding is meant to change when its finding is fixed: the fix
//! turns it red, and the audit's row is rewritten in the same pull request.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;
use futures::executor::block_on;
use ikigai_core::{
    Alias, AliasTable, ArgRef, ArgSpec, AsyncFnEndpoint, Bindings, Capability, Confine,
    Description, Endpoint, EndpointSpace, Error, Exact, Fallback, FixedClock, FnEndpoint,
    Invocation, Iri, Kernel, Level, Mount, ReprType, Representation, Request, Resolution, Resolved,
    Result, Scope, Space, SpaceKind, Verb,
};

fn iri(s: &str) -> Iri {
    Iri::parse(s).unwrap()
}

fn source(target: &str) -> Request {
    Request::new(Verb::Source, iri(target))
}

fn text(bytes: &'static [u8]) -> FnEndpoint {
    FnEndpoint::new("text", move |_| {
        Ok(Representation::new(ReprType::new("text/plain"), bytes.to_vec()).cacheable())
    })
}

fn door(name: &str, bytes: &'static [u8]) -> Arc<dyn Space> {
    Arc::new(EndpointSpace::new().bind(Exact::new(name), text(bytes)))
}

fn read(kernel: &Kernel, target: &str) -> Result<Vec<u8>> {
    block_on(kernel.issue(source(target), &Capability::root())).map(|answer| answer.bytes)
}

/// An endpoint that answers what `target` answers, confined to nothing of its own.
fn reader(target: &'static str) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("reader", move |inv| {
        Box::pin(async move { inv.source(&iri(target)).await })
    })
}

// ---- (b) `Invocation::confine`: chosen in the endpoint's body, unseen by the arrangement -----

/// The same severing, two ways. `Confine` is on the door as `ik:confinedTo`; a body calling
/// `Invocation::confine` is on no door, and the corridor it picked came from its argument.
#[test]
fn finding_invocation_confine_chooses_a_corridor_the_arrangement_never_shows() {
    let by_argument = AsyncFnEndpoint::new("by-argument", |inv| {
        Box::pin(async move {
            let (name, answer): (&str, &'static [u8]) = match inv.inline_str("pick")? {
                "a" => ("urn:ctx:a", b"from a"),
                _ => ("urn:ctx:b", b"from b"),
            };
            let corridor = door("urn:x", answer);
            inv.confine(iri(name), corridor).source(&iri("urn:x")).await
        })
    });
    let declared = Confine::new(
        iri("urn:ctx:declared"),
        door("urn:x", b"from declared"),
        Arc::new(reader("urn:x")),
    );
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:by-argument"), by_argument)
            .bind_arc(Exact::new("urn:declared"), Arc::new(declared)),
    ));

    let pick = |value: &str| {
        source("urn:by-argument").with_arg("pick", ArgRef::Inline(value.as_bytes().to_vec()))
    };
    let root = Capability::root();
    assert_eq!(
        block_on(kernel.issue(pick("a"), &root)).unwrap().bytes,
        b"from a"
    );
    assert_eq!(
        block_on(kernel.issue(pick("b"), &root)).unwrap().bytes,
        b"from b"
    );

    let SpaceKind::EndpointSpace { doors } = kernel.root_topology().kind else {
        panic!("the root is an endpoint space");
    };
    let confined_to = |pattern: &str| {
        doors
            .iter()
            .find(|door| door.pattern == pattern)
            .expect("the door is listed")
            .confined
            .as_ref()
            .and_then(|corridor| corridor.id.clone())
    };
    assert_eq!(confined_to("urn:declared"), Some(iri("urn:ctx:declared")));
    assert_eq!(confined_to("urn:by-argument"), None);
}

/// A corridor's name is what the cache keys on, and `Invocation::confine` takes any name. So an
/// endpoint can name its own corridor after another door's declared confinement and have its
/// answer served inside that confinement, under the same capability.
#[test]
fn finding_a_confinement_name_is_unchecked_so_one_endpoint_can_answer_for_anothers_corridor() {
    let forger = AsyncFnEndpoint::new("forger", |inv| {
        Box::pin(async move {
            inv.confine(iri("urn:ctx:shared"), door("urn:trust-set", b"forged"))
                .source(&iri("urn:trust-set"))
                .await
        })
    });
    let verifier = Confine::new(
        iri("urn:ctx:shared"),
        door("urn:trust-set", b"genuine"),
        Arc::new(reader("urn:trust-set")),
    );
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:forger"), forger)
            .bind_arc(Exact::new("urn:verifier"), Arc::new(verifier)),
    ));

    assert_eq!(read(&kernel, "urn:forger").unwrap(), b"forged");
    assert_eq!(read(&kernel, "urn:verifier").unwrap(), b"forged");
}

// ---- (b) `Scope::spaces`: a live corridor, reachable from any invocation --------------------

/// `inv.scope().spaces()` hands over the chain's corridors as live spaces. Resolving one and
/// invoking what it found dispatches outside the kernel, so the door's declared capability
/// floor, which a sub-request meets, is never consulted.
#[test]
fn finding_scope_spaces_lets_an_endpoint_dispatch_past_a_corridors_capability_floor() {
    let gated = text(b"gated answer").with_description(
        Description::new("gated")
            .verb(Verb::Source)
            .requires("urn:cap:gated"),
    );
    let corridor: Arc<dyn Space> =
        Arc::new(EndpointSpace::new().bind(Exact::new("urn:gated"), gated));

    let asks = AsyncFnEndpoint::new("asks", |inv| {
        Box::pin(async move { inv.source(&iri("urn:gated")).await })
    });
    let reaches_in = AsyncFnEndpoint::new("reaches-in", |inv| {
        Box::pin(async move {
            let request = source("urn:gated");
            let spaces = inv.scope().spaces();
            let Resolution::Hit(hit) = spaces[0].resolve(&request, inv.scope()) else {
                panic!("the corridor binds the name");
            };
            hit.endpoint.invoke(inv).await
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:asks"), asks)
            .bind(Exact::new("urn:reaches-in"), reaches_in),
    ));

    let scope = || Scope::empty().with_named(iri("urn:ctx:gated"), Arc::clone(&corridor));
    let weak = Capability::scoped(["urn:cap:other"]);
    let asked = block_on(kernel.issue_in(source("urn:asks"), &weak, scope()));
    assert!(matches!(asked, Err(Error::Denied(_))), "{asked:?}");
    let reached = block_on(kernel.issue_in(source("urn:reaches-in"), &weak, scope())).unwrap();
    assert_eq!(reached.bytes, b"gated answer");
}

// ---- (b) A kernel handle: `Issuer` and `Kernel::issue_in` take any chain ---------------------

/// Nothing keeps a `Kernel` out of an endpoint's closure, and whoever holds one chooses the chain.
/// This door reports `ik:confinedTo`, and its endpoint reads the root anyway: a checker reading
/// the arrangement concludes the opposite of what runs.
#[test]
fn finding_an_endpoint_holding_the_kernel_leaves_the_confinement_its_door_reports() {
    let handle: Arc<OnceLock<Weak<Kernel>>> = Arc::new(OnceLock::new());
    let escapes = {
        let handle = Arc::clone(&handle);
        AsyncFnEndpoint::new("escapes", move |inv| {
            let kernel = handle.get().and_then(Weak::upgrade).expect("attached");
            Box::pin(async move {
                assert!(inv.scope().is_severed(), "runs inside the confinement");
                let confined = inv.source(&iri("urn:secret")).await;
                assert!(
                    matches!(confined, Err(Error::Unresolved(_))),
                    "{confined:?}"
                );
                // The kernel takes the chain as an argument: here, the empty one.
                kernel
                    .issue_in(source("urn:secret"), inv.capability, Scope::empty())
                    .await
            })
        })
    };
    let door_ = Confine::new(
        iri("urn:ctx:nothing"),
        Arc::new(EndpointSpace::new()),
        Arc::new(escapes),
    );
    let kernel = Arc::new(Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:secret"), text(b"s3cr3t"))
            .bind_arc(Exact::new("urn:escapes"), Arc::new(door_)),
    )));
    handle.set(Arc::downgrade(&kernel)).unwrap();

    let SpaceKind::EndpointSpace { doors } = kernel.root_topology().kind else {
        panic!("the root is an endpoint space");
    };
    let reported = doors.iter().find(|door| door.pattern == "urn:escapes");
    assert!(reported.unwrap().confined.is_some());
    assert_eq!(read(&kernel, "urn:escapes").unwrap(), b"s3cr3t");
}

/// `Invocation::with_issuer` starts a new invocation in the empty chain, whatever chain its
/// builder runs in: with a kernel in hand, a decorator can run the endpoint it wraps outside the
/// chain the decorator itself was given.
#[test]
fn finding_with_issuer_starts_an_invocation_in_the_empty_chain() {
    let request = source("urn:x");
    let bindings = Bindings::new();
    let root = Capability::root();
    let kernel = Kernel::new(door("urn:x", b"x"));
    let inv = Invocation::with_issuer(&request, &bindings, &root, &kernel);
    assert!(inv.scope().is_empty());
    assert_eq!(inv.depth(), 0);
    assert_eq!(block_on(inv.source(&iri("urn:x"))).unwrap().bytes, b"x");
}

// ---- (a) What an endpoint can build and cannot use ------------------------------------------

/// Every `Scope` builder is public and an endpoint can call them all. None of it reaches a
/// sub-request: a chain takes effect only by being issued in, and issuing in one takes a kernel.
#[test]
fn a_scope_an_endpoint_builds_is_inert() {
    let tries = AsyncFnEndpoint::new("tries", |inv| {
        Box::pin(async move {
            let own = inv.scope().fingerprint();
            let _built = inv
                .scope()
                .clone()
                .with_named(iri("urn:ctx:fake"), door("urn:x", b"from the fake"))
                .stack(&Scope::empty().with(door("urn:x", b"stacked")))
                .sever()
                .confined(iri("urn:ctx:behind"), door("urn:x", b"behind"));
            assert_eq!(inv.scope().fingerprint(), own);
            inv.source(&iri("urn:x")).await
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:x"), text(b"from the root"))
            .bind(Exact::new("urn:tries"), tries),
    ));
    assert_eq!(read(&kernel, "urn:tries").unwrap(), b"from the root");
}

/// `with_clock` takes the invocation by value, so an endpoint can reach it on a reborrow, whatever
/// `docs/design/resolution-scope.md` says about detached invocations. The clock stays on that
/// handle: it is never carried into a sub-request.
#[test]
fn a_clock_attached_on_a_reborrow_stays_on_the_handle() {
    let leaf = FnEndpoint::new("leaf", |inv| {
        assert_eq!(inv.now().map(|t| t.as_millis()), Some(1));
        Ok(Representation::new(
            ReprType::new("text/plain"),
            b"leaf".to_vec(),
        ))
    });
    let reborrows = AsyncFnEndpoint::new("reborrows", |inv| {
        Box::pin(async move {
            let bindings = Bindings::new();
            let handle = inv
                .with_bindings(&bindings)
                .with_clock(Arc::new(FixedClock::at(7)));
            assert_eq!(handle.now().map(|t| t.as_millis()), Some(7));
            handle.source(&iri("urn:leaf")).await
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:leaf"), leaf)
            .bind(Exact::new("urn:reborrows"), reborrows),
    ))
    .with_clock(Arc::new(FixedClock::at(1)));
    assert_eq!(read(&kernel, "urn:reborrows").unwrap(), b"leaf");
}

/// `SyncIssuer`, the one handle an invocation mints, inherits the chain of the invocation that
/// minted it: a closure holding one is exactly as confined as its endpoint.
#[cfg(not(target_family = "wasm"))]
#[test]
fn a_sync_issuer_stays_in_the_chain_of_the_invocation_that_minted_it() {
    let bridged = AsyncFnEndpoint::new("bridged", |inv| {
        Box::pin(async move {
            let confined = inv.confine(iri("urn:ctx:empty"), Arc::new(EndpointSpace::new()));
            let outcome = confined
                .scope_sync(|issuer| issuer.source(&iri("urn:secret")))
                .await?;
            assert!(matches!(outcome, Err(Error::Unresolved(_))), "{outcome:?}");
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"held".to_vec(),
            ))
        })
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:secret"), text(b"s3cr3t"))
            .bind(Exact::new("urn:bridged"), bridged),
    ));
    assert_eq!(read(&kernel, "urn:bridged").unwrap(), b"held");
}

// ---- Run-time naming: there is none ---------------------------------------------------------

/// A space's name is a claim the cache and the topology read, never an address: no request
/// reaches a space by naming it, in or out of a confinement.
#[test]
fn no_space_is_resolved_by_its_name() {
    let named = Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:x"), text(b"x"))
            .named(iri("urn:iki:space:example")),
    );
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![Arc::new(Level::new(
        iri("urn:example:level:l"),
        named,
    ))]));
    let kernel = Kernel::new(root);
    let corridor = || Scope::empty().with_named(iri("urn:ctx:c"), door("urn:y", b"y"));
    let cap = Capability::root();
    assert_eq!(read(&kernel, "urn:x").unwrap(), b"x");
    for name in [
        "urn:iki:space:example",
        "urn:iki:space:example:urn:x",
        "urn:example:level:l",
        "urn:example:level:l:urn:x",
    ] {
        assert!(
            matches!(read(&kernel, name), Err(Error::Unresolved(_))),
            "{name}"
        );
    }
    let by_corridor_name = block_on(kernel.issue_in(source("urn:ctx:c"), &cap, corridor()));
    assert!(matches!(by_corridor_name, Err(Error::Unresolved(_))));
}

// ---- (c) What a static reader cannot tell ---------------------------------------------------

/// A space outside core can report any `Level` it holds on a hit. The endpoint then runs in that
/// level, which is nowhere in the tree: all the arrangement shows is `ik:OpaqueSpace`, which is
/// the honest answer and makes a checker say "unknown".
struct ReportsALevel {
    level: Level,
}

impl Space for ReportsALevel {
    fn resolve(&self, request: &Request, _scope: &Scope) -> Resolution {
        if request.target.as_str() != "urn:opaque:door" {
            return Resolution::Miss;
        }
        let endpoint: Arc<dyn Endpoint> = Arc::new(reader("urn:private"));
        Resolution::Hit(Resolved::new(endpoint, Bindings::new()).within(&self.level))
    }
}

#[test]
fn finding_an_opaque_space_can_place_its_endpoint_in_a_level_the_tree_does_not_show() {
    let space = ReportsALevel {
        level: Level::new(
            iri("urn:example:level:hidden"),
            door("urn:private", b"behind the hidden level"),
        ),
    };
    let kernel = Kernel::new(Arc::new(space));
    assert_eq!(
        read(&kernel, "urn:opaque:door").unwrap(),
        b"behind the hidden level"
    );
    assert!(matches!(
        read(&kernel, "urn:private"),
        Err(Error::Unresolved(_))
    ));
    assert!(matches!(kernel.root_topology().kind, SpaceKind::Opaque));
}

/// A rule declared `ik:resolveAt "root"` re-issues in the view's resolved scope, so inside a
/// `Level` the level answers first and the root's door of that name is never reached.
#[test]
fn finding_a_view_declared_at_the_root_resolves_from_its_level_when_it_is_inside_one() {
    let views = Alias::new(
        Arc::new(AliasTable::new().exact_at_root("urn:m:view", "urn:target")),
        door("urn:target", b"the level's target"),
    );
    let level = Level::new(iri("urn:example:level:m"), Arc::new(views));
    let kernel = Kernel::new(Arc::new(Fallback::new(vec![
        Arc::new(Mount::new("urn:m:", Arc::new(level))),
        door("urn:target", b"the root's target"),
    ])));
    assert_eq!(read(&kernel, "urn:target").unwrap(), b"the root's target");
    assert_eq!(read(&kernel, "urn:m:view").unwrap(), b"the level's target");
}

/// By design (resolution-scope decision 6), a reference is read in the callee's resolved scope.
/// Across a level the same name can mean two things.
#[test]
fn designed_a_by_reference_argument_is_read_in_the_callees_scope() {
    struct Takes;
    #[async_trait]
    impl Endpoint for Takes {
        async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
            Ok(Representation::new(
                ReprType::new("text/plain"),
                inv.inline_arg("in")?.to_vec(),
            ))
        }
        fn describe(&self) -> Description {
            Description::new("takes")
                .verb(Verb::Source)
                .input(ArgSpec::new("in"))
        }
    }
    let module = EndpointSpace::new()
        .bind(Exact::new("urn:m:takes"), Takes)
        .bind(Exact::new("urn:name"), text(b"the module's meaning"));
    let kernel = Kernel::new(Arc::new(Fallback::new(vec![
        Arc::new(Mount::new(
            "urn:m:",
            Arc::new(Level::new(iri("urn:example:level:m"), Arc::new(module))),
        )),
        door("urn:name", b"the caller's meaning"),
    ])));
    assert_eq!(read(&kernel, "urn:name").unwrap(), b"the caller's meaning");
    let by_reference = source("urn:m:takes").with_arg("in", ArgRef::Reference(iri("urn:name")));
    let answer = block_on(kernel.issue(by_reference, &Capability::root())).unwrap();
    assert_eq!(answer.bytes, b"the module's meaning");
}

// ---- Durable context: what travels into a sub-request ---------------------------------------

/// A host's corridor and its clock reach every sub-request below it, at any depth, with no trace
/// in the root's arrangement: only the chain's own topology and the trace name them.
#[test]
fn finding_a_host_corridor_and_its_clock_travel_below_the_request_undeclared() {
    let clock_seen = Arc::new(AtomicU32::new(0));
    let leaf = {
        let clock_seen = Arc::clone(&clock_seen);
        AsyncFnEndpoint::new("leaf", move |inv| {
            let clock_seen = Arc::clone(&clock_seen);
            Box::pin(async move {
                let now = inv.now().expect("the chain carries a clock");
                clock_seen.store(now.as_millis() as u32, Ordering::SeqCst);
                inv.source(&iri("urn:shadowed")).await
            })
        })
    };
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:shadowed"), text(b"the root's"))
            .bind(Exact::new("urn:middle"), reader("urn:leaf"))
            .bind(Exact::new("urn:leaf"), leaf),
    ));
    let before = kernel.root_topology().to_turtle();
    let scope = Scope::empty().with_named_at(
        iri("urn:ctx:pinned"),
        door("urn:shadowed", b"the corridor's"),
        Arc::new(FixedClock::at(42)),
    );
    let answer =
        block_on(kernel.issue_in(source("urn:middle"), &Capability::root(), scope.clone()));
    assert_eq!(answer.unwrap().bytes, b"the corridor's");
    assert_eq!(clock_seen.load(Ordering::SeqCst), 42);
    assert_eq!(kernel.root_topology().to_turtle(), before);
    assert!(!before.contains("urn:ctx:pinned"));
    assert!(kernel
        .topology_in(&scope)
        .to_turtle()
        .contains("urn:ctx:pinned"));
}

/// The level stack does NOT travel: it belongs to where an endpoint was found, and each
/// resolution replaces it. A root endpoint called from inside a level runs outside every level.
#[test]
fn the_level_stack_does_not_travel_into_an_endpoint_found_outside_it() {
    let levels_seen = |seen: Arc<AtomicU32>| {
        AsyncFnEndpoint::new("reports", move |inv| {
            let seen = Arc::clone(&seen);
            Box::pin(async move {
                seen.store(inv.scope().levels().len() as u32 + 1, Ordering::SeqCst);
                Ok(Representation::new(
                    ReprType::new("text/plain"),
                    b"ok".to_vec(),
                ))
            })
        })
    };
    let at_root = Arc::new(AtomicU32::new(0));
    let in_level = Arc::new(AtomicU32::new(0));
    let module = EndpointSpace::new().bind(Exact::new("urn:m:calls"), {
        let in_level = Arc::clone(&in_level);
        AsyncFnEndpoint::new("calls", move |inv| {
            let in_level = Arc::clone(&in_level);
            Box::pin(async move {
                in_level.store(inv.scope().levels().len() as u32 + 1, Ordering::SeqCst);
                inv.source(&iri("urn:at-root")).await
            })
        })
    });
    let kernel = Kernel::new(Arc::new(Fallback::new(vec![
        Arc::new(Mount::new(
            "urn:m:",
            Arc::new(Level::new(iri("urn:example:level:m"), Arc::new(module))),
        )),
        Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:at-root"), levels_seen(Arc::clone(&at_root))),
        ),
    ])));
    read(&kernel, "urn:m:calls").unwrap();
    assert_eq!(in_level.load(Ordering::SeqCst), 2, "one level, plus one");
    assert_eq!(at_root.load(Ordering::SeqCst), 1, "no level, plus one");
}
