//! **Sealed names** (ledger #563): a module must not be able to override core or
//! security names. A sealed name is answered by its owner — core, the host, or one
//! level — or not at all: every other level skips it; the host's injected
//! corridors may still stand in for it. What the topology shows is refused when the
//! kernel is built; what it cannot show is refused on resolution. Never a silent
//! skip. `docs/design/resolution-context.md`, "Sealed names".

use std::sync::{Arc, Mutex};

use futures::executor::block_on;
use ikigai_core::{
    AliasTable, AsyncFnEndpoint, Capability, Confine, EndpointSpace, Error, Exact, Fallback,
    FnEndpoint, Iri, Kernel, Level, Mount, ReprType, Representation, Request, Resolution, Scope,
    SealError, SealOwner, Space, SpaceEntry, TraceEvent, Tracer, UriTemplate, Verb, SEALED_NOTE,
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

fn constant(name: &'static str, body: &'static [u8]) -> FnEndpoint {
    FnEndpoint::new(name, move |_| Ok(text(body).cacheable()))
}

/// Sources `target` and answers with what came back, or the error as text.
fn reader(target: &'static str) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("reader", move |inv| {
        Box::pin(async move {
            match inv.source(&iri(target)).await {
                Ok(inner) => Ok(text(&inner.bytes)),
                Err(e) => Ok(text(format!("error: {e}").as_bytes())),
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

fn module(prefix: &str, level: Level) -> Arc<dyn Space> {
    Arc::new(Mount::new(prefix, Arc::new(level)))
}

fn level(name: &str, space: EndpointSpace) -> Level {
    Level::new(iri(name), Arc::new(space))
}

/// A space that resolves exactly as its inner one and says NOTHING about its
/// structure — the default `Space::topology`, an opaque node. What the build-time
/// check cannot see into.
struct Hidden(Arc<dyn Space>);

impl Space for Hidden {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        self.0.resolve(request, scope)
    }
    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        None
    }
}

fn hidden(space: EndpointSpace) -> Arc<dyn Space> {
    Arc::new(Hidden(Arc::new(space)))
}

/// The real trust set, bound at the root outside every level.
fn real_trust_set() -> Arc<dyn Space> {
    Arc::new(EndpointSpace::new().bind(Exact::new("urn:sign:trust-set"), constant("real", b"real")))
}

// ---- 1. Core and host seals ---------------------------------------------------

#[test]
fn a_module_binding_a_host_sealed_name_is_refused_at_build_naming_the_level_door_and_prefix() {
    let squatter = || {
        module(
            "urn:mod:",
            level(
                "urn:example:level:mod",
                EndpointSpace::new()
                    .bind(Exact::new("urn:mod:verify"), reader("urn:sign:trust-set"))
                    .bind(Exact::new("urn:sign:trust-set"), constant("fake", b"fake")),
            ),
        )
    };
    let refused = Kernel::check_sealing(squatter().as_ref(), ["urn:sign:"]).unwrap_err();
    assert_eq!(
        refused,
        SealError::Binds {
            level: Some(iri("urn:example:level:mod")),
            door: "urn:sign:trust-set".into(),
            prefix: "urn:sign:".into(),
            owner: SealOwner::Host,
        }
    );
    let message = refused.to_string();
    for part in [
        "urn:example:level:mod",
        "`urn:sign:trust-set`",
        "`urn:sign:`",
        "the host",
    ] {
        assert!(message.contains(part), "{message}");
    }
    // The builder is the same check, and panics with the same words.
    let panicked = std::panic::catch_unwind(|| Kernel::new(squatter()).with_sealed(["urn:sign:"]))
        .err()
        .and_then(|p| p.downcast_ref::<String>().cloned())
        .expect("with_sealed refuses");
    assert!(panicked.contains(&message), "{panicked}");

    // The same door OUTSIDE every level is the host's own binding: allowed.
    assert!(Kernel::check_sealing(real_trust_set().as_ref(), ["urn:sign:"]).is_ok());
}

#[test]
fn a_template_is_refused_only_where_a_request_for_the_sealed_name_can_reach_it() {
    let inside = |prefix: &str, pattern: &str| -> Result<(), SealError> {
        let root = module(
            prefix,
            level(
                "urn:example:level:mod",
                EndpointSpace::new()
                    .bind(UriTemplate::parse(pattern).unwrap(), constant("t", b"t")),
            ),
        );
        Kernel::check_sealing(root.as_ref(), ["urn:sign:"])
    };
    // Its head could expand into the sealed family, and nothing guards it: refused.
    assert!(matches!(
        inside("", "urn:{ns}:{id}"),
        Err(SealError::Binds { .. })
    ));
    // The same template behind a mount that admits no sealed name: a request for
    // one never gets there (level frames skip sealed names), so it is allowed.
    assert!(inside("urn:mod:", "urn:{ns}:{id}").is_ok());
    // A head that diverges from the sealed family is never a door of it.
    assert!(inside("", "urn:mod:{id}").is_ok());
}

#[test]
fn an_exact_door_with_a_literal_brace_is_read_as_the_name_it_is_not_as_a_template() {
    // `ik:matchKind` says how a pattern matches, and only a template's `{` opens an
    // expansion (ledger #644). Read as text, `urn:{braced}` looked like a template
    // whose head `urn:` could expand into `urn:sign:`, and was refused.
    let door = |grammar: Box<dyn Fn(EndpointSpace) -> EndpointSpace>| {
        let root = module(
            "",
            level("urn:example:level:mod", grammar(EndpointSpace::new())),
        );
        Kernel::check_sealing(root.as_ref(), ["urn:sign:"])
    };
    // The exact name `urn:{braced}` is one name, outside `urn:sign:`: allowed.
    assert!(door(Box::new(
        |space| space.bind(Exact::new("urn:{braced}"), constant("x", b"x"))
    ))
    .is_ok());
    // The same text as a TEMPLATE can expand into the sealed family: refused.
    assert!(matches!(
        door(Box::new(|space| space.bind(
            UriTemplate::parse("urn:{braced}").unwrap(),
            constant("x", b"x")
        ))),
        Err(SealError::Binds { .. })
    ));
    // And an exact name inside the family is refused however it is spelled.
    assert!(matches!(
        door(Box::new(
            |space| space.bind(Exact::new("urn:sign:{braced}"), constant("x", b"x"))
        )),
        Err(SealError::Binds { .. })
    ));
}

#[test]
fn a_sealed_name_requested_from_inside_a_level_skips_the_level_and_reaches_the_root() {
    // The module holds a copy the build check cannot see (an opaque space). Its own
    // sub-request for the sealed name skips its level and reaches the root's binding;
    // an UNSEALED name still resolves at the level first.
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        module(
            "urn:mod:",
            Level::new(
                iri("urn:example:level:mod"),
                Arc::new(Fallback::new(vec![
                    Arc::new(
                        EndpointSpace::new()
                            .bind(Exact::new("urn:mod:verify"), reader("urn:sign:trust-set"))
                            .bind(Exact::new("urn:mod:time"), reader("urn:time:zone")),
                    ),
                    hidden(
                        EndpointSpace::new()
                            .bind(Exact::new("urn:sign:trust-set"), constant("fake", b"fake"))
                            .bind(
                                Exact::new("urn:time:zone"),
                                constant("tz", b"module's zone"),
                            ),
                    ),
                ])),
            ),
        ),
        real_trust_set(),
        Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:time:zone"), constant("z", b"root's zone")),
        ),
    ]));
    let kernel = Kernel::new(root).with_sealed(["urn:sign:"]);
    assert_eq!(get(&kernel, "urn:mod:verify"), "real");
    assert_eq!(get(&kernel, "urn:mod:time"), "module's zone");

    // Unsealed, the same arrangement lets the module's copy win: the seal is the
    // only thing standing between the verifier and the fake.
    let open: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        module(
            "urn:mod:",
            Level::new(
                iri("urn:example:level:mod"),
                Arc::new(Fallback::new(vec![
                    Arc::new(
                        EndpointSpace::new()
                            .bind(Exact::new("urn:mod:verify"), reader("urn:sign:trust-set")),
                    ),
                    hidden(
                        EndpointSpace::new()
                            .bind(Exact::new("urn:sign:trust-set"), constant("fake", b"fake")),
                    ),
                ])),
            ),
        ),
        real_trust_set(),
    ]));
    assert_eq!(get(&Kernel::new(open), "urn:mod:verify"), "fake");
}

#[test]
fn an_injected_corridor_still_stands_in_for_a_sealed_name_and_a_confined_one_does_not() {
    let pinned = Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:sign:trust-set"),
        constant("pinned", b"pinned"),
    ));
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        module(
            "urn:mod:",
            level(
                "urn:example:level:mod",
                EndpointSpace::new()
                    .bind(Exact::new("urn:mod:verify"), reader("urn:sign:trust-set")),
            ),
        ),
        real_trust_set(),
        Arc::new(
            EndpointSpace::new().bind_arc(
                Exact::new("urn:confined:verify"),
                Arc::new(Confine::new(
                    iri("urn:ctx:fake"),
                    Arc::new(
                        EndpointSpace::new()
                            .bind(Exact::new("urn:sign:trust-set"), constant("fake", b"fake")),
                    ),
                    Arc::new(reader("urn:sign:trust-set")),
                )),
            ),
        ),
    ]));
    let kernel = Kernel::new(root).with_sealed(["urn:sign:"]);
    // Host authority: an injected corridor answers the sealed name, inside the level too.
    let answer = block_on(kernel.issue_in(
        source("urn:mod:verify"),
        &Capability::root(),
        Scope::empty().with_named(iri("urn:ctx:pinned"), pinned),
    ))
    .unwrap();
    assert_eq!(answer.bytes, b"pinned");
    // An endpoint's own confinement is not host authority: the corridor is skipped
    // for the sealed name, the root is severed, and the name has nowhere to go.
    assert_eq!(
        get(&kernel, "urn:confined:verify"),
        "error: no endpoint resolved for urn:sign:trust-set"
    );
}

#[test]
fn a_breach_the_topology_cannot_show_is_refused_on_resolution_and_traced() {
    // A level with an OPAQUE copy of a host-sealed name, reached from the root
    // walk ahead of the real one: the build check could not see it, so resolution
    // refuses it — loudly, never by quietly falling through to the real binding.
    let root: Arc<dyn Space> = Arc::new(Fallback::new(vec![
        Arc::new(Level::new(
            iri("urn:example:level:squat"),
            hidden(
                EndpointSpace::new()
                    .bind(Exact::new("urn:sign:trust-set"), constant("fake", b"fake")),
            ),
        )),
        real_trust_set(),
    ]));
    let kernel = Kernel::new(root).with_sealed(["urn:sign:"]);
    #[derive(Default)]
    struct Collect(Mutex<Vec<TraceEvent>>);
    impl Tracer for Collect {
        fn record(&self, event: TraceEvent) {
            self.0.lock().unwrap().push(event);
        }
    }
    let collect = Arc::new(Collect::default());
    let err = block_on(kernel.issue_traced(
        source("urn:sign:trust-set"),
        &Capability::root(),
        collect.clone(),
    ))
    .unwrap_err();
    let Error::Endpoint(message) = &err else {
        panic!("{err:?}")
    };
    assert!(message.contains("urn:example:level:squat"), "{message}");
    assert!(message.contains("sealed by the host"), "{message}");
    let events = collect.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0]
        .notes
        .iter()
        .any(|(k, v)| k == SEALED_NOTE && v == message));
}

#[test]
fn a_sealing_level_the_topology_cannot_see_is_refused_rather_than_unenforced() {
    // A level with seals, hidden under an opaque overlay: the kernel never saw its
    // seals, so it cannot enforce them — and says so instead of ignoring them.
    let hidden_level: Arc<dyn Space> = Arc::new(Hidden(module(
        "urn:mod:",
        level(
            "urn:example:level:mod",
            EndpointSpace::new().bind(Exact::new("urn:mod:secret"), constant("s", b"s")),
        )
        .sealing(["urn:mod:secret"]),
    )));
    let kernel = Kernel::new(hidden_level);
    let err = get(&kernel, "urn:mod:secret");
    assert!(err.contains("never registered"), "{err}");
}

// ---- 2. Module seals -------------------------------------------------------------

#[test]
fn a_module_seals_a_name_in_its_namespace_and_another_modules_copy_is_never_consulted() {
    // M seals `urn:m:key`; N holds a copy the build check cannot see. N's own
    // sub-request for M's sealed name skips N and reaches M's real binding — through
    // M's mount, from the root. M's own sub-requests resolve it at M.
    let m = module(
        "urn:m:",
        level(
            "urn:example:level:m",
            EndpointSpace::new()
                .bind(Exact::new("urn:m:key"), constant("m", b"M's key"))
                .bind(Exact::new("urn:m:read"), reader("urn:m:key")),
        )
        .sealing(["urn:m:key"]),
    );
    let n = module(
        "urn:n:",
        Level::new(
            iri("urn:example:level:n"),
            Arc::new(Fallback::new(vec![
                Arc::new(EndpointSpace::new().bind(Exact::new("urn:n:read"), reader("urn:m:key"))),
                hidden(
                    EndpointSpace::new().bind(Exact::new("urn:m:key"), constant("n", b"N's copy")),
                ),
            ])),
        ),
    );
    let kernel = Kernel::new(Arc::new(Fallback::new(vec![n, m])));
    assert_eq!(get(&kernel, "urn:n:read"), "M's key");
    assert_eq!(get(&kernel, "urn:m:read"), "M's key");
    assert_eq!(get(&kernel, "urn:m:key"), "M's key");
    let topology = get(&kernel, "urn:kernel:topology");
    assert!(
        topology.contains("<urn:example:level:m> a ik:Level ;\n    ik:seals \"urn:m:key\" ;"),
        "{topology}"
    );
    assert_eq!(
        kernel.sealed(),
        [
            ("urn:kernel:".to_string(), SealOwner::Core),
            (
                "urn:m:key".to_string(),
                SealOwner::Level(iri("urn:example:level:m"))
            ),
        ]
    );
}

#[test]
fn a_module_binding_another_modules_sealed_name_is_refused_at_build() {
    let m = module(
        "urn:m:",
        level(
            "urn:example:level:m",
            EndpointSpace::new().bind(Exact::new("urn:m:key"), constant("m", b"M")),
        )
        .sealing(["urn:m:key"]),
    );
    // N is mounted under `urn:m:` too, so a request for M's key can reach N's copy.
    let n = module(
        "urn:m:",
        level(
            "urn:example:level:n",
            EndpointSpace::new().bind(Exact::new("urn:m:key"), constant("n", b"N")),
        ),
    );
    let root = Fallback::new(vec![n, m]);
    assert_eq!(
        Kernel::check_sealing(&root, Vec::<String>::new()).unwrap_err(),
        SealError::Binds {
            level: Some(iri("urn:example:level:n")),
            door: "urn:m:key".into(),
            prefix: "urn:m:key".into(),
            owner: SealOwner::Level(iri("urn:example:level:m")),
        }
    );
}

#[test]
fn sealing_outside_its_namespace_is_refused_and_the_host_can_accept_one() {
    let squat = module(
        "urn:mod:",
        level("urn:example:level:mod", EndpointSpace::new()).sealing(["urn:sign:"]),
    );
    assert_eq!(
        Kernel::check_sealing(squat.as_ref(), Vec::<String>::new()).unwrap_err(),
        SealError::OutsideNamespace {
            level: iri("urn:example:level:mod"),
            prefix: "urn:sign:".into(),
            namespace: Some("urn:mod:".into()),
        }
    );
    // Unmounted, with no namespace accepted: nothing to seal under.
    let bare: Arc<dyn Space> =
        Arc::new(level("urn:example:level:mod", EndpointSpace::new()).sealing(["urn:mod:x"]));
    assert!(matches!(
        Kernel::check_sealing(bare.as_ref(), Vec::<String>::new()).unwrap_err(),
        SealError::OutsideNamespace {
            namespace: None,
            ..
        }
    ));
    // The host accepts a namespace for it: allowed.
    let accepted: Arc<dyn Space> = Arc::new(
        level("urn:example:level:mod", EndpointSpace::new())
            .in_namespace("urn:mod:")
            .sealing(["urn:mod:x"]),
    );
    assert!(Kernel::check_sealing(accepted.as_ref(), Vec::<String>::new()).is_ok());
}

#[test]
fn two_overlapping_claims_are_refused_naming_both() {
    // Two modules under one mount, sealing prefixes one inside the other.
    let root = Fallback::new(vec![
        module(
            "urn:shared:",
            level("urn:example:level:a", EndpointSpace::new()).sealing(["urn:shared:"]),
        ),
        module(
            "urn:shared:",
            level("urn:example:level:b", EndpointSpace::new()).sealing(["urn:shared:key"]),
        ),
    ]);
    assert_eq!(
        Kernel::check_sealing(&root, Vec::<String>::new()).unwrap_err(),
        SealError::Overlaps {
            prefix: "urn:shared:".into(),
            owner: SealOwner::Level(iri("urn:example:level:a")),
            other: "urn:shared:key".into(),
            other_owner: SealOwner::Level(iri("urn:example:level:b")),
        }
    );
}

#[test]
fn a_module_cannot_claim_a_core_or_host_seal() {
    // Core: `urn:kernel:` is core's, whatever mount a module sits under.
    let core = module(
        "urn:kernel:mod:",
        level("urn:example:level:mod", EndpointSpace::new()).sealing(["urn:kernel:mod:x"]),
    );
    assert_eq!(
        Kernel::check_sealing(core.as_ref(), Vec::<String>::new()).unwrap_err(),
        SealError::Overlaps {
            prefix: "urn:kernel:".into(),
            owner: SealOwner::Core,
            other: "urn:kernel:mod:x".into(),
            other_owner: SealOwner::Level(iri("urn:example:level:mod")),
        }
    );
    // Host: checked first, so the module is the one named as refused.
    let host = module(
        "urn:mod:",
        level("urn:example:level:mod", EndpointSpace::new()).sealing(["urn:mod:"]),
    );
    assert_eq!(
        Kernel::check_sealing(host.as_ref(), ["urn:mod:secret"]).unwrap_err(),
        SealError::Overlaps {
            prefix: "urn:mod:secret".into(),
            owner: SealOwner::Host,
            other: "urn:mod:".into(),
            other_owner: SealOwner::Level(iri("urn:example:level:mod")),
        }
    );
}

#[test]
fn an_alias_inside_a_level_is_a_door_for_the_names_it_admits() {
    // A table rewriting a sealed LOGICAL name inside a level answers that name as
    // surely as a binding would: the table is visible, so the build check reads it.
    let table = Arc::new(AliasTable::new().exact("urn:sign:trust-set", "urn:mod:fake"));
    let root = module(
        "",
        Level::new(
            iri("urn:example:level:mod"),
            Arc::new(ikigai_core::Alias::new(
                table,
                Arc::new(
                    EndpointSpace::new().bind(Exact::new("urn:mod:fake"), constant("f", b"f")),
                ),
            )),
        ),
    );
    assert!(matches!(
        Kernel::check_sealing(root.as_ref(), ["urn:sign:"]).unwrap_err(),
        SealError::Binds { ref door, .. } if door == "urn:sign:trust-set"
    ));
}

#[test]
fn a_kernel_with_no_seals_beyond_cores_reports_just_that() {
    let kernel = Kernel::new(Arc::new(EndpointSpace::new()));
    assert_eq!(
        kernel.sealed(),
        [("urn:kernel:".to_string(), SealOwner::Core)]
    );
    let kernel = kernel
        .with_sealed(["urn:cap:", "urn:secret:"])
        .with_sealed(["urn:sign:"]);
    assert_eq!(
        kernel.sealed(),
        [
            ("urn:kernel:".to_string(), SealOwner::Core),
            ("urn:cap:".to_string(), SealOwner::Host),
            ("urn:secret:".to_string(), SealOwner::Host),
            ("urn:sign:".to_string(), SealOwner::Host),
        ]
    );
}

/// The owner level `urn:example:level:a` sealing `urn:a:`, behind an opaque
/// third-party space ([`Hidden`]) that answers whatever `squatter_inner` does.
fn squatted(squatter_inner: Arc<dyn Space>) -> Kernel {
    let owner = level(
        "urn:example:level:a",
        EndpointSpace::new().bind(Exact::new("urn:a:secret"), constant("real", b"REAL")),
    )
    .sealing(["urn:a:"]);
    Kernel::new(Arc::new(Fallback::new(vec![
        Arc::new(Hidden(squatter_inner)),
        Arc::new(Mount::new("urn:a:", Arc::new(owner))),
    ])))
}

/// Ledger #750, D1: the runtime seal check identifies the owning level by the
/// REGISTERED level object, not by its name. `Level::new` is public, so an opaque
/// space that wraps its fake in a `Level` it built itself under the owner's name is
/// not the owner, and is refused exactly as the bare squatter is.
#[test]
fn a_level_built_under_the_owners_name_is_not_the_owner() {
    let cap = Capability::root();

    // Control: the squatter answering directly is refused.
    let plain = squatted(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:a:secret"), constant("fake", b"FAKE")),
    ));
    let refused = block_on(plain.issue(source("urn:a:secret"), &cap));
    assert!(refused.is_err(), "control: {refused:?}");

    // The same fake inside a Level that claims the owner's name.
    let spoofed = squatted(Arc::new(level(
        "urn:example:level:a",
        EndpointSpace::new().bind(Exact::new("urn:a:secret"), constant("fake", b"FAKE")),
    )));
    let got = block_on(spoofed.issue(source("urn:a:secret"), &cap));
    assert!(
        matches!(&got, Err(Error::Endpoint(m)) if m.contains("urn:a:secret")),
        "a sealed name was answered by a level claiming the owner's name: {:?}",
        got.map(|r| String::from_utf8_lossy(&r.bytes).into_owned())
    );

    // And a spoof that ALSO declares the owner's seals is the unregistered level it
    // is, not the owner.
    let sealing_spoof = squatted(Arc::new(
        level(
            "urn:example:level:a",
            EndpointSpace::new().bind(Exact::new("urn:a:secret"), constant("fake", b"FAKE")),
        )
        .sealing(["urn:a:"]),
    ));
    let got = block_on(sealing_spoof.issue(source("urn:a:secret"), &cap));
    assert!(
        got.is_err(),
        "a sealing level the topology never showed was taken for the owner: {:?}",
        got.map(|r| String::from_utf8_lossy(&r.bytes).into_owned())
    );
}

/// The owner itself is still admitted — through its mount, from the root.
#[test]
fn the_registered_owner_still_answers_its_sealed_names() {
    let kernel = Kernel::new(Arc::new(Mount::new(
        "urn:a:",
        Arc::new(
            level(
                "urn:example:level:a",
                EndpointSpace::new().bind(Exact::new("urn:a:secret"), constant("real", b"REAL")),
            )
            .sealing(["urn:a:"]),
        ),
    )));
    let got = block_on(kernel.issue(source("urn:a:secret"), &Capability::root())).unwrap();
    assert_eq!(got.bytes, b"REAL");
}
